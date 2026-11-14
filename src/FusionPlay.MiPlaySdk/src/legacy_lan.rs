//! Opt-in UDP 5355 `_miplay_lan._tcp` legacy DNS-SD responder.
//!
//! Some HyperOS builds' `SystemUI` still browse the legacy MiPlay LAN service
//! over UDP 5355 (legacy unicast DNS-SD) instead of mDNS 5353. FusionPlay
//! intentionally does **not** advertise that route by default: HyperOS renders
//! it as a second generic television and the route is duplicated next to the
//! query-scoped Lyra/Mi Connect pair (see `docs/MIPLAY_DUPLICATE_ROUTES.md`).
//!
//! The channel is therefore strictly opt-in behind
//! `FUSIONPLAY_MIPLAY_LEGACY_LAN=1`. When the switch is absent the receiver
//! never binds port 5355 and the default wire behaviour is unchanged.
//!
//! Wire contract implemented here:
//! - bind UDP 5355 with `SO_REUSEADDR`, join 224.0.0.251, multicast loop off;
//! - answer `_miplay_lan._tcp` PTR queries with a legacy unicast response
//!   (`PTR + SRV(8899) + TXT + A`, no question section, TTL 10, query id
//!   echoed, no cache-flush bit as required by RFC 6762 §6.7);
//! - announce to 224.0.0.251:5355 three times (~100 ms apart) with TTL 60;
//! - a single TXT string carrying the Xiaomi legacy JSON identity.

use crate::discovery::{
    legacy_lan_id_hash, legacy_mi_connect_instance, parse_dns_questions, parse_hardware_address,
    push_label, push_pointer, push_u16, push_u32,
};
use crate::media::EventEmitter;
use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(crate) const LEGACY_LAN_PORT: u16 = 5355;
const LEGACY_LAN_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const LEGACY_LAN_SERVICE_TYPE: &str = "_miplay_lan._tcp.local.";
const LEGACY_LAN_SERVICE_QNAME: &[u8] = b"\x0b_miplay_lan\x04_tcp\x05local\x00";
const LEGACY_LAN_SERVICE_NAME: &str = "_miplay_lan._tcp.local";
const LEGACY_LAN_LOCAL_LABEL_OFFSET: u16 = 12 + 1 + 11 + 1 + 4;
const LEGACY_LAN_SERVICE_PORT: u16 = 8899;
const LEGACY_LAN_UNICAST_TTL: u32 = 10;
const LEGACY_LAN_ANNOUNCE_TTL: u32 = 60;
const LEGACY_LAN_ANNOUNCEMENT_COUNT: usize = 3;
const LEGACY_LAN_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(100);
const LEGACY_LAN_RECEIVE_TIMEOUT: Duration = Duration::from_millis(250);
/// mDNS/LLMNR traffic on the link-local group is only valid with TTL 255.
const LEGACY_LAN_IP_TTL: u32 = 255;
/// Legacy `_miplay_lan` is the television-shaped identity. The captured
/// official record carries `"type":3`; keep it pinned to the TV category even
/// when the picker category (`dev`) of the mDNS route is changed.
const LEGACY_LAN_TXT_TYPE: u8 = 3;
const LEGACY_LAN_ENV: &str = "FUSIONPLAY_MIPLAY_LEGACY_LAN";

/// Everything the legacy channel needs from the receiver identity. The idm
/// instance id is the same seed that derives the mDNS `_mi-connect` idHash,
/// and the hardware address is the same value the mDNS TXT advertises as
/// Base64 `mac`, so both channels describe one identity.
#[derive(Clone, Debug)]
pub(crate) struct LegacyLanIdentity {
    pub receiver_name: String,
    pub idm_instance_id: String,
    /// MAC source: `ReceiverConfig::hardware_address` when present, otherwise
    /// the media device id (which may itself encode a hardware address).
    pub hardware_address: String,
    pub local_ip: Ipv4Addr,
}

#[derive(Clone, Debug)]
struct LegacyLanAdvert {
    instance: String,
    hostname: String,
    id_hash: String,
    /// One DNS character-string: length byte followed by the JSON payload.
    txt: Vec<u8>,
    local_ip: Ipv4Addr,
}

impl LegacyLanAdvert {
    fn from_identity(identity: &LegacyLanIdentity) -> Self {
        let mac = legacy_lan_mac(Some(&identity.hardware_address), &identity.idm_instance_id);
        let txt_json = legacy_lan_txt_json(identity, mac);
        Self {
            instance: legacy_mi_connect_instance(
                &identity.receiver_name,
                &identity.idm_instance_id,
            ),
            hostname: legacy_lan_hostname(mac),
            id_hash: legacy_lan_id_hash(&identity.idm_instance_id),
            txt: encode_character_string(&txt_json),
            local_ip: identity.local_ip,
        }
    }
}

/// TTL and class projection for the two packet shapes the legacy channel
/// emits. A legacy unicast answer echoes the query id and must not set the
/// cache-flush bit; the multicast announcement uses the standard cache-flush
/// bit for the unique SRV/TXT/A records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyLanResponseKind {
    Unicast { transaction_id: u16 },
    Announcement,
}

impl LegacyLanResponseKind {
    const fn transaction_id(self) -> u16 {
        match self {
            Self::Unicast { transaction_id } => transaction_id,
            Self::Announcement => 0,
        }
    }

    const fn ttl(self) -> u32 {
        match self {
            Self::Unicast { .. } => LEGACY_LAN_UNICAST_TTL,
            Self::Announcement => LEGACY_LAN_ANNOUNCE_TTL,
        }
    }

    const fn unique_record_class(self) -> u16 {
        match self {
            // RFC 6762 §6.7: legacy unicast answers must not set the
            // cache-flush bit, legacy resolvers do not understand it.
            Self::Unicast { .. } => 1,
            Self::Announcement => 0x8001,
        }
    }
}

/// Build one complete legacy DNS-SD packet (`PTR + SRV + TXT + A`).
///
/// The packet intentionally has `QDCOUNT = 0`: Xiaomi's legacy answer omits
/// the question section and clients match solely on the echoed transaction id.
fn build_legacy_lan_packet(
    advertisement: &LegacyLanAdvert,
    kind: LegacyLanResponseKind,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(256);
    push_u16(&mut packet, kind.transaction_id());
    push_u16(&mut packet, 0x8400); // QR + authoritative answer
    push_u16(&mut packet, 0); // QDCOUNT: legacy answer has no question section
    push_u16(&mut packet, 4); // ANCOUNT: PTR, SRV, TXT, A
    push_u16(&mut packet, 0); // NSCOUNT
    push_u16(&mut packet, 0); // ARCOUNT

    let service_offset = u16::try_from(packet.len()).expect("legacy DNS-SD packet offset");
    packet.extend_from_slice(LEGACY_LAN_SERVICE_QNAME);

    // PTR: owner `_miplay_lan._tcp.local` -> `<instance>._miplay_lan._tcp.local`
    push_u16(&mut packet, 12);
    push_u16(&mut packet, 1);
    push_u32(&mut packet, kind.ttl());
    push_u16(
        &mut packet,
        u16::try_from(1 + advertisement.instance.len() + 2).expect("legacy PTR length"),
    );
    let instance_label_offset = u16::try_from(packet.len()).expect("legacy DNS-SD packet offset");
    push_label(&mut packet, &advertisement.instance);
    push_pointer(&mut packet, service_offset);

    // SRV: instance -> priority 0, weight 0, port 8899, target `miplay-xxyyzz.local`
    push_pointer(&mut packet, instance_label_offset);
    push_u16(&mut packet, 33);
    push_u16(&mut packet, kind.unique_record_class());
    push_u32(&mut packet, kind.ttl());
    push_u16(
        &mut packet,
        u16::try_from(6 + 1 + advertisement.hostname.len() + 2).expect("legacy SRV length"),
    );
    push_u16(&mut packet, 0);
    push_u16(&mut packet, 0);
    push_u16(&mut packet, LEGACY_LAN_SERVICE_PORT);
    let hostname_label_offset = u16::try_from(packet.len()).expect("legacy DNS-SD packet offset");
    push_label(&mut packet, &advertisement.hostname);
    push_pointer(&mut packet, LEGACY_LAN_LOCAL_LABEL_OFFSET);

    // TXT: instance -> one character-string with the JSON identity.
    push_pointer(&mut packet, instance_label_offset);
    push_u16(&mut packet, 16);
    push_u16(&mut packet, kind.unique_record_class());
    push_u32(&mut packet, kind.ttl());
    push_u16(
        &mut packet,
        u16::try_from(advertisement.txt.len()).expect("legacy TXT length"),
    );
    packet.extend_from_slice(&advertisement.txt);

    // A: hostname -> receiver IPv4 address.
    push_pointer(&mut packet, hostname_label_offset);
    push_u16(&mut packet, 1);
    push_u16(&mut packet, kind.unique_record_class());
    push_u32(&mut packet, kind.ttl());
    push_u16(&mut packet, 4);
    packet.extend_from_slice(&advertisement.local_ip.octets());

    packet
}

/// Return the transaction id when `packet` is a `_miplay_lan._tcp` PTR
/// question that this responder should answer.
fn legacy_lan_query_id(packet: &[u8]) -> Option<u16> {
    if packet.len() < 12 {
        return None;
    }
    // QR bit set means the packet is already a response; never answer those
    // or two responders would ping-pong forever.
    if packet[2] & 0x80 != 0 {
        return None;
    }
    let transaction_id = u16::from_be_bytes([packet[0], packet[1]]);
    let matches = parse_dns_questions(packet).iter().any(|(qname, qtype, _)| {
        if *qtype != 12 {
            return false;
        }
        let qname = qname.to_ascii_lowercase();
        qname == LEGACY_LAN_SERVICE_NAME
            || qname.ends_with(&format!(".{LEGACY_LAN_SERVICE_NAME}"))
    });
    matches.then_some(transaction_id)
}

fn build_legacy_lan_unicast_response(
    advertisement: &LegacyLanAdvert,
    query: &[u8],
) -> Option<Vec<u8>> {
    let transaction_id = legacy_lan_query_id(query)?;
    Some(build_legacy_lan_packet(
        advertisement,
        LegacyLanResponseKind::Unicast { transaction_id },
    ))
}

/// MAC advertised by the legacy TXT. The hardware address is preferred; when
/// the host has none, a stable locally-administered unicast address is derived
/// from the same seed as the IDM identity so the ID/hostname stay stable
/// across restarts instead of changing on every launch.
fn legacy_lan_mac(hardware_address: Option<&str>, stable_seed: &str) -> [u8; 6] {
    if let Some(address) = hardware_address.and_then(parse_hardware_address) {
        return address;
    }
    let digest = Sha256::digest(stable_seed.as_bytes());
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&digest[..6]);
    mac[0] = (mac[0] & 0xfc) | 0x02;
    mac
}

fn legacy_lan_hostname(mac: [u8; 6]) -> String {
    format!(
        "miplay-{:02x}{:02x}{:02x}",
        mac[3], mac[4], mac[5]
    )
}

fn format_mac(mac: [u8; 6]) -> String {
    let mut formatted = String::with_capacity(17);
    for (index, byte) in mac.iter().enumerate() {
        if index > 0 {
            formatted.push(':');
        }
        formatted.push_str(&format!("{byte:02X}"));
    }
    formatted
}

fn legacy_lan_txt_json(identity: &LegacyLanIdentity, mac: [u8; 6]) -> String {
    // `ID` is the low 31 bits of the big-endian MAC suffix, matching the
    // identifier Xiaomi's sender derives for the legacy route.
    let id = u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]) & 0x7fff_ffff;
    format!(
        "{{\"supportLyra\":true,\"mac\":\"{}\",\"ID\":{},\"type\":{},\"idhash\":\"{}\",\"extraAbility\":0}}",
        format_mac(mac),
        id,
        LEGACY_LAN_TXT_TYPE,
        legacy_lan_id_hash(&identity.idm_instance_id),
    )
}

/// Encode one DNS TXT character-string (length byte + payload). The legacy TXT
/// is a single JSON object; a longer payload must not be silently truncated,
/// so this only clamps at the wire maximum after debug builds assert the size.
fn encode_character_string(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    // The legacy JSON identity is bounded well below 255 bytes; keep the same
    // defensive clamp the mDNS TXT builder uses instead of panicking if a
    // future field grows unexpectedly.
    let length = u8::try_from(bytes.len()).unwrap_or(u8::MAX);
    debug_assert!(
        bytes.len() <= usize::from(u8::MAX),
        "legacy MiPlay TXT payload exceeds one DNS character-string"
    );
    let mut encoded = Vec::with_capacity(1 + usize::from(length));
    encoded.push(length);
    encoded.extend_from_slice(&bytes[..usize::from(length)]);
    encoded
}

fn create_legacy_lan_socket(local_ip: Ipv4Addr) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("create legacy MiPlay DNS-SD socket")?;
    // Windows LLMNR owns 5355; SO_REUSEADDR lets both listeners coexist and
    // is what Xiaomi's sender expects from a DNS-SD participant.
    socket
        .set_reuse_address(true)
        .context("enable legacy MiPlay DNS-SD socket reuse")?;
    // mDNS/LLMNR questions are addressed to 224.0.0.251, so binding the
    // selected unicast address would filter them out before recv_from.
    let bind_ip = Ipv4Addr::UNSPECIFIED;
    socket
        .bind(&SocketAddrV4::new(bind_ip, LEGACY_LAN_PORT).into())
        .with_context(|| {
            format!("bind legacy MiPlay DNS-SD socket to {bind_ip}:{LEGACY_LAN_PORT}")
        })?;
    socket
        .set_multicast_if_v4(&local_ip)
        .context("select legacy MiPlay DNS-SD adapter")?;
    socket
        .set_multicast_ttl_v4(LEGACY_LAN_IP_TTL)
        .context("set legacy MiPlay DNS-SD multicast TTL")?;
    socket
        .set_ttl_v4(LEGACY_LAN_IP_TTL)
        .context("set legacy MiPlay DNS-SD unicast TTL")?;
    // Announcements must not feed our own responder socket back into the
    // receive loop.
    socket
        .set_multicast_loop_v4(false)
        .context("disable legacy MiPlay DNS-SD multicast loopback")?;

    let socket: UdpSocket = socket.into();
    socket
        .join_multicast_v4(&LEGACY_LAN_GROUP, &local_ip)
        .context("join legacy MiPlay DNS-SD multicast group")?;
    socket
        .set_read_timeout(Some(LEGACY_LAN_RECEIVE_TIMEOUT))
        .context("set legacy MiPlay DNS-SD receive timeout")?;
    Ok(socket)
}

fn send_legacy_lan_announcements(
    socket: &UdpSocket,
    advertisement: &LegacyLanAdvert,
    shutdown: &AtomicBool,
    events: &EventEmitter,
) {
    let destination = SocketAddr::V4(SocketAddrV4::new(LEGACY_LAN_GROUP, LEGACY_LAN_PORT));
    let packet = build_legacy_lan_packet(advertisement, LegacyLanResponseKind::Announcement);
    for round in 1..=LEGACY_LAN_ANNOUNCEMENT_COUNT {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let sent = socket.send_to(&packet, destination);
        events(json!({
            "event": "legacy_lan_announcement",
            "protocol": "xiaomi_miplay",
            "service": LEGACY_LAN_SERVICE_TYPE,
            "destination": destination.to_string(),
            "bytes": packet.len(),
            "round": round,
            "ttl": LEGACY_LAN_ANNOUNCE_TTL,
            "success": sent.is_ok(),
            "error": sent.err().map(|error| error.to_string()),
        }));
        if round < LEGACY_LAN_ANNOUNCEMENT_COUNT {
            thread::sleep(LEGACY_LAN_ANNOUNCEMENT_INTERVAL);
        }
    }
}

fn legacy_lan_worker(
    socket: UdpSocket,
    advertisement: LegacyLanAdvert,
    shutdown: Arc<AtomicBool>,
    events: EventEmitter,
) {
    send_legacy_lan_announcements(&socket, &advertisement, &shutdown, &events);
    let local_address = socket
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "unknown".to_owned());
    let mut buffer = [0u8; 4096];
    while !shutdown.load(Ordering::Acquire) {
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let query = &buffer[..length];
                let Some(response) = build_legacy_lan_unicast_response(&advertisement, query)
                else {
                    // Port 5355 also carries LLMNR and other DNS-SD services;
                    // ignored packets are expected and must not spam events.
                    continue;
                };
                let sent = socket.send_to(&response, source);
                events(json!({
                    "event": "legacy_lan_query",
                    "protocol": "xiaomi_miplay",
                    "service": LEGACY_LAN_SERVICE_TYPE,
                    "source": source.to_string(),
                    "destination": local_address,
                    "bytes": response.len(),
                    "query_wire_hex": hex::encode(query),
                    "reply_wire_hex": hex::encode(&response),
                    "unicast": true,
                    "ttl": LEGACY_LAN_UNICAST_TTL,
                    "success": sent.is_ok(),
                    "error": sent.err().map(|error| error.to_string()),
                }));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                events(json!({
                    "event": "legacy_lan_receive_failed",
                    "protocol": "xiaomi_miplay",
                    "service": LEGACY_LAN_SERVICE_TYPE,
                    "address": local_address,
                    "port": LEGACY_LAN_PORT,
                    "message": error.to_string(),
                }));
                break;
            }
        }
    }
    events(json!({
        "event": "legacy_lan_stopped",
        "protocol": "xiaomi_miplay",
        "service": LEGACY_LAN_SERVICE_TYPE,
    }));
}

/// Owns the opt-in responder thread. Dropping it stops the thread and joins so
/// a receiver restart never leaves a stale 5355 listener behind.
pub(crate) struct LegacyLanAdvertisement {
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LegacyLanAdvertisement {
    fn start(identity: LegacyLanIdentity, events: EventEmitter) -> Result<Self> {
        let advertisement = LegacyLanAdvert::from_identity(&identity);
        let socket = create_legacy_lan_socket(identity.local_ip)?;
        events(json!({
            "event": "legacy_lan_ready",
            "protocol": "xiaomi_miplay",
            "service": LEGACY_LAN_SERVICE_TYPE,
            "address": identity.local_ip.to_string(),
            "port": LEGACY_LAN_PORT,
            "instance": advertisement.instance.clone(),
            "hostname": advertisement.hostname.clone(),
            "id_hash": advertisement.id_hash.clone(),
            "unicast_ttl": LEGACY_LAN_UNICAST_TTL,
            "announcement_ttl": LEGACY_LAN_ANNOUNCE_TTL,
            "announcement_rounds": LEGACY_LAN_ANNOUNCEMENT_COUNT,
            "environment_switch": LEGACY_LAN_ENV,
            "external_service_required": false,
        }));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::Builder::new()
            .name("miplay-legacy-lan-5355".to_owned())
            .spawn(move || legacy_lan_worker(socket, advertisement, worker_shutdown, events))
            .context("spawn legacy MiPlay DNS-SD responder")?;
        Ok(Self {
            shutdown,
            worker: Some(worker),
        })
    }
}

impl Drop for LegacyLanAdvertisement {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn legacy_lan_enabled_from(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE"))
}

/// Start the opt-in responder when `enabled` is true. A disabled responder
/// returns `None` before any socket is created, which is the default path and
/// keeps the previous behaviour (no port 5355 binding) bit-for-bit.
pub(crate) fn start_if_enabled(
    enabled: bool,
    identity: LegacyLanIdentity,
    events: EventEmitter,
) -> Option<LegacyLanAdvertisement> {
    if !enabled {
        return None;
    }
    match LegacyLanAdvertisement::start(identity, Arc::clone(&events)) {
        Ok(advertisement) => Some(advertisement),
        Err(error) => {
            // The legacy channel is a diagnostic compatibility option: a port
            // conflict (for example another MiPlay receiver) must never abort
            // startup of the primary mDNS routes.
            events(json!({
                "event": "legacy_lan_start_failed",
                "protocol": "xiaomi_miplay",
                "service": LEGACY_LAN_SERVICE_TYPE,
                "port": LEGACY_LAN_PORT,
                "message": format!("{error:#}"),
            }));
            None
        }
    }
}

pub(crate) fn start_from_env(
    identity: LegacyLanIdentity,
    events: EventEmitter,
) -> Option<LegacyLanAdvertisement> {
    let enabled = legacy_lan_enabled_from(std::env::var(LEGACY_LAN_ENV).ok().as_deref());
    start_if_enabled(enabled, identity, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MiPlayDeviceType;
    use crate::discovery::{build_mi_connect_txt, parse_dns_name};
    use base64::Engine;
    use serde_json::Value;

    const SEED: &str = "c86c85e8-cd15-4ebd-b898-c15e75deb923";
    const HARDWARE_ADDRESS: &str = "A0-36-BC-25-05-43";

    fn test_identity(receiver_name: &str) -> LegacyLanIdentity {
        LegacyLanIdentity {
            receiver_name: receiver_name.to_owned(),
            idm_instance_id: SEED.to_owned(),
            hardware_address: HARDWARE_ADDRESS.to_owned(),
            local_ip: Ipv4Addr::new(192, 168, 31, 128),
        }
    }

    fn test_advertisement() -> LegacyLanAdvert {
        LegacyLanAdvert::from_identity(&test_identity("ASUS"))
    }

    fn noop_events() -> EventEmitter {
        Arc::new(|_| {})
    }

    fn build_query(transaction_id: u16, qname_wire: &[u8], qtype: u16) -> Vec<u8> {
        let mut query = Vec::with_capacity(12 + qname_wire.len() + 4);
        push_u16(&mut query, transaction_id);
        push_u16(&mut query, 0); // standard query
        push_u16(&mut query, 1); // QDCOUNT
        push_u16(&mut query, 0);
        push_u16(&mut query, 0);
        push_u16(&mut query, 0);
        query.extend_from_slice(qname_wire);
        push_u16(&mut query, qtype);
        push_u16(&mut query, 1); // IN
        query
    }

    fn instance_query(advertisement: &LegacyLanAdvert, transaction_id: u16) -> Vec<u8> {
        let mut qname = Vec::new();
        push_label(&mut qname, &advertisement.instance);
        qname.extend_from_slice(LEGACY_LAN_SERVICE_QNAME);
        build_query(transaction_id, &qname, 12)
    }

    struct ParsedRecord {
        owner: String,
        record_type: u16,
        class: u16,
        ttl: u32,
        rdata: Vec<u8>,
        /// Absolute offset of the RDATA inside the full packet, needed to
        /// follow compressed name pointers in the SRV target.
        rdata_offset: usize,
        next_offset: usize,
    }

    fn read_record(packet: &[u8], offset: usize) -> ParsedRecord {
        let mut cursor = offset;
        let owner = parse_dns_name(packet, &mut cursor).expect("record owner name");
        let record_type = u16::from_be_bytes([packet[cursor], packet[cursor + 1]]);
        let class = u16::from_be_bytes([packet[cursor + 2], packet[cursor + 3]]);
        let ttl = u32::from_be_bytes([
            packet[cursor + 4],
            packet[cursor + 5],
            packet[cursor + 6],
            packet[cursor + 7],
        ]);
        let data_length =
            usize::from(u16::from_be_bytes([packet[cursor + 8], packet[cursor + 9]]));
        let rdata_start = cursor + 10;
        let next_offset = rdata_start + data_length;
        ParsedRecord {
            owner,
            record_type,
            class,
            ttl,
            rdata: packet[rdata_start..next_offset].to_vec(),
            rdata_offset: rdata_start,
            next_offset,
        }
    }

    fn records(packet: &[u8]) -> Vec<ParsedRecord> {
        let answer_count = usize::from(u16::from_be_bytes([packet[6], packet[7]]));
        let mut offset = 12usize;
        let mut answers = Vec::with_capacity(answer_count);
        for _ in 0..answer_count {
            let record = read_record(packet, offset);
            offset = record.next_offset;
            answers.push(record);
        }
        assert_eq!(offset, packet.len(), "trailing bytes in legacy packet");
        answers
    }

    fn txt_entries(txt: &[u8]) -> Vec<String> {
        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < txt.len() {
            let length = usize::from(txt[offset]);
            offset += 1;
            entries.push(String::from_utf8_lossy(&txt[offset..offset + length]).into_owned());
            offset += length;
        }
        entries
    }

    fn legacy_txt_json(advertisement: &LegacyLanAdvert) -> Value {
        assert_eq!(usize::from(advertisement.txt[0]) + 1, advertisement.txt.len());
        serde_json::from_slice(&advertisement.txt[1..]).expect("legacy TXT JSON")
    }

    #[test]
    fn legacy_lan_is_opt_in_and_disabled_by_default() {
        assert!(!legacy_lan_enabled_from(None));
        for disabled in ["", "0", "false", "yes", "on", "  "] {
            assert!(
                !legacy_lan_enabled_from(Some(disabled)),
                "unexpected enable for {disabled:?}"
            );
        }
        for enabled in ["1", "true", "TRUE", " 1 "] {
            assert!(
                legacy_lan_enabled_from(Some(enabled)),
                "expected enable for {enabled:?}"
            );
        }

        // The default path returns before creating any socket, so port 5355
        // is never bound unless the switch is explicitly set.
        assert!(start_if_enabled(false, test_identity("ASUS"), noop_events()).is_none());
    }

    #[test]
    fn legacy_lan_identity_is_derived_from_the_hardware_address() {
        let advertisement = test_advertisement();
        let mac = legacy_lan_mac(Some(HARDWARE_ADDRESS), SEED);
        assert_eq!(mac, [0xA0, 0x36, 0xBC, 0x25, 0x05, 0x43]);
        assert_eq!(advertisement.hostname, "miplay-250543");
        assert_eq!(advertisement.instance, "ASUS(sly3iRBCZ7)");

        let txt = legacy_txt_json(&advertisement);
        assert_eq!(txt["supportLyra"], Value::Bool(true));
        assert_eq!(txt["mac"], Value::String("A0:36:BC:25:05:43".to_owned()));
        let expected_id = u32::from_be_bytes([0xBC, 0x25, 0x05, 0x43]) & 0x7fff_ffff;
        assert_eq!(txt["ID"], Value::Number(expected_id.into()));
        assert_eq!(txt["type"], Value::Number(3.into()));
        assert_eq!(txt["idhash"], Value::String("c2x5".to_owned()));
        assert_eq!(txt["extraAbility"], Value::Number(0.into()));

        // Without a usable hardware address the MAC stays deterministic and a
        // valid locally-administered unicast address rather than a random one.
        let fallback_mac = legacy_lan_mac(Some("not-a-mac"), SEED);
        assert_eq!(
            fallback_mac,
            legacy_lan_mac(Some("not-a-mac"), SEED),
            "fallback MAC must be stable across restarts"
        );
        assert_eq!(fallback_mac[0] & 0x01, 0, "fallback MAC must be unicast");
        assert_eq!(
            fallback_mac[0] & 0x02,
            0x02,
            "fallback MAC must be locally administered"
        );
        let fallback = LegacyLanAdvert::from_identity(&LegacyLanIdentity {
            hardware_address: "not-a-mac".to_owned(),
            ..test_identity("ASUS")
        });
        assert_eq!(fallback.hostname.len(), "miplay-".len() + 6);
        assert_eq!(
            fallback.hostname,
            format!(
                "miplay-{:02x}{:02x}{:02x}",
                fallback_mac[3], fallback_mac[4], fallback_mac[5]
            )
        );
    }

    #[test]
    fn legacy_unicast_packet_has_the_captured_record_layout() {
        let advertisement = test_advertisement();
        let response = build_legacy_lan_packet(
            &advertisement,
            LegacyLanResponseKind::Unicast {
                transaction_id: 0x1234,
            },
        );

        // Header: echoed query id, QR+AA, no question section, four answers.
        assert_eq!(
            &response[..12],
            &[0x12, 0x34, 0x84, 0x00, 0, 0, 0, 4, 0, 0, 0, 0]
        );

        let answers = records(&response);
        assert_eq!(answers.len(), 4);

        // PTR
        let ptr = &answers[0];
        assert_eq!(ptr.owner, "_miplay_lan._tcp.local");
        assert_eq!(ptr.record_type, 12);
        assert_eq!(ptr.class, 1, "legacy unicast responses have no cache-flush");
        assert_eq!(ptr.ttl, LEGACY_LAN_UNICAST_TTL);
        let mut ptr_cursor = ptr.rdata_offset;
        assert_eq!(
            parse_dns_name(&response, &mut ptr_cursor).unwrap(),
            "ASUS(sly3iRBCZ7)._miplay_lan._tcp.local"
        );

        // SRV
        let srv = &answers[1];
        assert_eq!(srv.owner, "ASUS(sly3iRBCZ7)._miplay_lan._tcp.local");
        assert_eq!(srv.record_type, 33);
        assert_eq!(srv.class, 1);
        assert_eq!(srv.ttl, LEGACY_LAN_UNICAST_TTL);
        assert_eq!(&srv.rdata[..6], &[0, 0, 0, 0, 0x22, 0xc3]); // priority, weight, port 8899
        let mut srv_cursor = srv.rdata_offset + 6;
        assert_eq!(
            parse_dns_name(&response, &mut srv_cursor).unwrap(),
            "miplay-250543.local"
        );

        // TXT
        let txt = &answers[2];
        assert_eq!(txt.owner, "ASUS(sly3iRBCZ7)._miplay_lan._tcp.local");
        assert_eq!(txt.record_type, 16);
        assert_eq!(txt.class, 1);
        assert_eq!(txt.ttl, LEGACY_LAN_UNICAST_TTL);
        assert_eq!(usize::from(txt.rdata[0]) + 1, txt.rdata.len());
        let parsed: Value = serde_json::from_slice(&txt.rdata[1..]).unwrap();
        assert_eq!(parsed["idhash"], Value::String("c2x5".to_owned()));

        // A
        let address = &answers[3];
        assert_eq!(address.owner, "miplay-250543.local");
        assert_eq!(address.record_type, 1);
        assert_eq!(address.class, 1);
        assert_eq!(address.ttl, LEGACY_LAN_UNICAST_TTL);
        assert_eq!(address.rdata, [192, 168, 31, 128]);
    }

    #[test]
    fn legacy_announcement_uses_multicast_ttl_and_cache_flush() {
        let advertisement = test_advertisement();
        let announcement = build_legacy_lan_packet(
            &advertisement,
            LegacyLanResponseKind::Announcement,
        );
        let unicast = build_legacy_lan_packet(
            &advertisement,
            LegacyLanResponseKind::Unicast {
                transaction_id: 0x1234,
            },
        );

        // Announcements are unsolicited: transaction id 0, still no question.
        assert_eq!(&announcement[..2], &[0, 0]);
        assert_eq!(
            &announcement[..12],
            &[0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0]
        );
        let unicast_answers = records(&unicast);
        let announcement_answers = records(&announcement);
        assert_eq!(announcement_answers.len(), 4);

        // PTR is a shared record: class stays IN in both shapes, only the TTL
        // differs.
        assert_eq!(announcement_answers[0].record_type, 12);
        assert_eq!(announcement_answers[0].class, 1);
        assert_eq!(unicast_answers[0].class, 1);
        assert_eq!(
            announcement_answers[0].ttl,
            LEGACY_LAN_ANNOUNCE_TTL,
            "announcement TTL"
        );
        assert_eq!(unicast_answers[0].ttl, LEGACY_LAN_UNICAST_TTL);

        // Unique records (SRV/TXT/A) are cache-flushed on multicast but must
        // stay plain IN in the legacy unicast answer.
        for (announcement_record, unicast_record) in announcement_answers
            .iter()
            .zip(unicast_answers.iter())
            .skip(1)
        {
            assert_eq!(
                announcement_record.class, 0x8001,
                "unique record type {} needs cache-flush",
                announcement_record.record_type
            );
            assert_eq!(unicast_record.class, 1);
            assert_eq!(announcement_record.ttl, LEGACY_LAN_ANNOUNCE_TTL);
            assert_eq!(unicast_record.ttl, LEGACY_LAN_UNICAST_TTL);
        }
    }

    #[test]
    fn legacy_lan_answers_only_legacy_service_queries() {
        let advertisement = test_advertisement();

        // Service browse PTR query -> answered with the echoed id.
        let browse = build_query(0xBEEF, LEGACY_LAN_SERVICE_QNAME, 12);
        let response =
            build_legacy_lan_unicast_response(&advertisement, &browse).expect("browse answered");
        assert_eq!(&response[..2], &[0xBE, 0xEF]);
        assert_eq!(&response[4..6], &[0, 0], "no question section");

        // Instance-qualified browse is answered as well.
        assert!(build_legacy_lan_unicast_response(
            &advertisement,
            &instance_query(&advertisement, 7),
        )
        .is_some());

        // Non-PTR questions are not part of the legacy contract.
        assert!(build_legacy_lan_unicast_response(
            &advertisement,
            &build_query(1, LEGACY_LAN_SERVICE_QNAME, 16),
        )
        .is_none());
        assert!(build_legacy_lan_unicast_response(
            &advertisement,
            &build_query(1, LEGACY_LAN_SERVICE_QNAME, 33),
        )
        .is_none());

        // Other services are ignored.
        let mi_connect_qname = b"\x0b_mi-connect\x04_udp\x05local\x00";
        assert!(
            build_legacy_lan_unicast_response(
                &advertisement,
                &build_query(1, mi_connect_qname, 12),
            )
            .is_none()
        );

        // A response packet (QR bit) is never answered, otherwise two
        // responders would loop on each other's packets.
        let mut reply = browse.clone();
        reply[2] |= 0x80;
        assert!(build_legacy_lan_unicast_response(&advertisement, &reply).is_none());

        // Truncated / empty packets are ignored without panicking.
        assert!(build_legacy_lan_unicast_response(&advertisement, &[]).is_none());
        assert!(build_legacy_lan_unicast_response(&advertisement, &[0u8; 8]).is_none());
        assert!(
            build_legacy_lan_unicast_response(&advertisement, &browse[..browse.len() - 1])
                .is_none()
        );
    }

    #[test]
    fn legacy_identity_matches_the_mdns_id_hash() {
        let advertisement = test_advertisement();

        // idHash advertised by the mDNS `_mi-connect` TXT (same identity seed).
        let mdns_txt = build_mi_connect_txt(
            "ASUS",
            SEED,
            HARDWARE_ADDRESS,
            MiPlayDeviceType::Television,
        );
        let entries = txt_entries(&mdns_txt);
        let mdns_id_hash = entries
            .iter()
            .find_map(|entry| entry.strip_prefix("idHash="))
            .expect("mDNS TXT idHash");
        assert_eq!(mdns_id_hash, "c2x5");

        let legacy_txt = legacy_txt_json(&advertisement);
        let legacy_id_hash = legacy_txt["idhash"].as_str().expect("legacy idhash");
        assert_eq!(legacy_id_hash.len(), 4);

        // Both spellings decode to the identical three identity bytes.
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(mdns_id_hash)
                .unwrap(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(legacy_id_hash)
                .unwrap()
        );
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(legacy_id_hash)
                .unwrap(),
            b"sly"
        );

        // Both channels publish the exact same DNS-SD instance label.
        assert_eq!(
            advertisement.instance,
            legacy_mi_connect_instance("ASUS", SEED)
        );
    }

    #[test]
    fn legacy_instance_name_is_sanitized_like_the_mdns_route() {
        let advertisement =
            LegacyLanAdvert::from_identity(&test_identity("客厅.电视\n"));
        assert!(!advertisement.instance.contains('.'));
        assert!(!advertisement.instance.chars().any(char::is_control));
        assert!(advertisement.instance.ends_with("(sly3iRBCZ7)"));
        assert!(advertisement.instance.len() <= 63);
        assert_eq!(
            advertisement.instance,
            legacy_mi_connect_instance("客厅.电视", SEED)
        );
    }

    #[test]
    fn base64url_alphabet_rewrites_the_legacy_id_hash() {
        assert_eq!(
            crate::discovery::to_base64url_alphabet("ab+/cd=="),
            "ab-_cd=="
        );
        assert_eq!(crate::discovery::to_base64url_alphabet("c2x5"), "c2x5");
    }
}
