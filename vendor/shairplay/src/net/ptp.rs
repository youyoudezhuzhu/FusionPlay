//! Minimal PTP timing client for AirPlay 2 (Apple aPTP profile).
//!
//! Listens on UDP ports 319 (event) and 320 (general) for PTP Sync/Follow_Up/Announce
//! messages. Tracks master clock ID and computes local-to-master time offset with smoothing.
//! Ports 319/320 require root or CAP_NET_BIND_SERVICE.

// Some of the public timing helpers are scaffolding for future multi-room work.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::util::now_ns;

const PTP_SAMPLE_MAX_AGE_NS: u64 = 300_000_000_000;
const PENDING_SYNC_MAX_AGE_NS: u64 = 2_000_000_000;
/// Ignore implausible path-delay estimates (>100 ms one way).
const PATH_DELAY_MAX_NS: u64 = 100_000_000;
/// Minimum interval between Delay_Req probes.
const DELAY_REQ_MIN_INTERVAL_NS: u64 = 1_000_000_000;
/// Outstanding Delay_Req probes older than this are dropped unmeasured.
const PENDING_DELAY_REQ_MAX_AGE_NS: u64 = 2_000_000_000;

/// PTP message types (IEEE 1588).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PtpMessageType {
    /// Sync message.
    Sync = 0,
    /// Follow-up message (carries precise timestamp).
    FollowUp = 8,
    /// Delay request (slave → master path-delay probe).
    DelayReq = 1,
    /// Delay response (master → slave path-delay reply).
    DelayResp = 9,
    /// Announce message (master clock election).
    Announce = 11,
    /// Unrecognized message type.
    Other = 0xFF,
}

impl From<u8> for PtpMessageType {
    fn from(v: u8) -> Self {
        match v & 0x0F {
            0 => Self::Sync,
            1 => Self::DelayReq,
            8 => Self::FollowUp,
            9 => Self::DelayResp,
            11 => Self::Announce,
            _ => Self::Other,
        }
    }
}

/// Clock info shared between PTP receiver and audio pipeline.
#[derive(Debug, Clone, Default)]
pub struct PtpClockInfo {
    /// IEEE 1588 clock identity of the current master.
    pub master_clock_id: u64,
    /// Local time (ns) when offset was last calculated.
    pub local_time: u64, // ns, when offset was last calculated
    /// Add to local time to get master time (ns), using wrapping arithmetic so
    /// a negative signed offset is represented without losing precision.
    pub offset: u64, // add to local time to get master time
    /// Local time (ns) when this master was first seen.
    pub mastership_start_time: u64, // ns
}

/// Thread-safe PTP clock state.
#[derive(Clone)]
pub struct PtpClock {
    info: Arc<RwLock<PtpClockInfo>>,
}

impl Default for PtpClock {
    fn default() -> Self {
        Self::new()
    }
}

impl PtpClock {
    /// Create a new instance with default state.
    pub fn new() -> Self {
        Self {
            info: Arc::new(RwLock::new(PtpClockInfo::default())),
        }
    }

    /// Get a snapshot of the current clock state.
    pub fn get_info(&self) -> PtpClockInfo {
        self.info.read().unwrap().clone()
    }

    /// Update the clock state with a new offset measurement.
    pub fn update(&self, clock_id: u64, local_time: u64, offset: u64, mastership_start: u64) {
        let mut info = self.info.write().unwrap();
        info.master_clock_id = clock_id;
        info.local_time = local_time;
        info.offset = offset;
        info.mastership_start_time = mastership_start;
    }

    /// Get current master time in nanoseconds.
    pub fn master_time_now(&self) -> Option<u64> {
        let info = self.info.read().unwrap();
        if info.master_clock_id == 0 {
            return None;
        }
        let now = now_ns();
        Some(now.wrapping_add(info.offset))
    }

    /// Convert a timestamp in the PTP master's timebase to this machine's
    /// wall-clock time. `expected_clock_id == 0` accepts the current master.
    ///
    /// A stale or mismatched clock returns `None`, allowing the audio pipeline
    /// to retain its best-effort local fallback.
    pub(crate) fn local_time_for_master_time(&self, master_time_ns: u64, expected_clock_id: u64) -> Option<u64> {
        self.local_time_for_master_time_at(master_time_ns, expected_clock_id, now_ns())
    }

    pub(crate) fn local_time_for_master_time_at(
        &self,
        master_time_ns: u64,
        expected_clock_id: u64,
        local_now_ns: u64,
    ) -> Option<u64> {
        let info = self.info.read().unwrap();
        if info.master_clock_id == 0
            || (expected_clock_id != 0 && expected_clock_id != info.master_clock_id)
            || local_now_ns.saturating_sub(info.local_time) > PTP_SAMPLE_MAX_AGE_NS
        {
            return None;
        }

        Some(master_time_ns.wrapping_sub(info.offset))
    }
}

/// Parse a PTP Follow_Up message and extract the preciseOriginTimestamp.
/// Returns (clock_id, precise_origin_timestamp_ns, correction_field_ns).
pub fn parse_follow_up(buf: &[u8]) -> Option<(u64, u64, i64)> {
    if buf.len() < 54 {
        return None;
    }
    let msg_type = PtpMessageType::from(buf[0]);
    if msg_type != PtpMessageType::FollowUp {
        return None;
    }

    // Clock identity: bytes 20..28
    let clock_id = u64::from_be_bytes(buf[20..28].try_into().ok()?);

    // Correction field: bytes 8..16 (signed, in units of 2^-16 ns)
    let correction_raw = i64::from_be_bytes(buf[8..16].try_into().ok()?);
    let correction_ns = correction_raw / 65536;

    // preciseOriginTimestamp: bytes 34..44 (6-byte seconds + 4-byte nanoseconds)
    let seconds_hi = u16::from_be_bytes([buf[34], buf[35]]) as u64;
    let seconds_lo = u32::from_be_bytes(buf[36..40].try_into().ok()?) as u64;
    let nanoseconds = u32::from_be_bytes(buf[40..44].try_into().ok()?) as u64;
    let seconds = (seconds_hi << 32) | seconds_lo;
    let timestamp_ns = seconds * 1_000_000_000 + nanoseconds;

    Some((clock_id, timestamp_ns.wrapping_add(correction_ns as u64), correction_ns))
}

/// Parsed PTP Announce fields used for grandmaster selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceDetails {
    /// Clock identity of the source we receive from (sourcePortIdentity).
    pub source_clock_id: u64,
    /// Elected grandmaster identity.
    pub grandmaster_id: u64,
    /// Grandmaster priority 1.
    pub priority1: u8,
    /// Grandmaster priority 2.
    pub priority2: u8,
    /// Number of boundary clocks between us and the grandmaster.
    pub steps_removed: u16,
}

impl AnnounceDetails {
    /// IEEE-1588 best-master ordering (lower is better).
    fn better_than(&self, other: &Self) -> bool {
        (self.priority1, self.priority2, self.steps_removed, self.grandmaster_id)
            < (other.priority1, other.priority2, other.steps_removed, other.grandmaster_id)
    }
}

/// Parse a PTP Announce message and extract the source clock identity.
pub fn parse_announce(buf: &[u8]) -> Option<u64> {
    parse_announce_details(buf).map(|details| details.source_clock_id)
}

/// Parse the Announce fields needed for master-clock selection.
pub(crate) fn parse_announce_details(buf: &[u8]) -> Option<AnnounceDetails> {
    // Announce is 64 bytes: header(34) + originTimestamp(10) + utcOffset(2) +
    // reserved(1) + priority1(1) + quality(4) + priority2(1) + gmIdentity(8) +
    // stepsRemoved(2) + timeSource(1).
    if buf.len() < 64 || PtpMessageType::from(buf[0]) != PtpMessageType::Announce {
        return None;
    }
    Some(AnnounceDetails {
        source_clock_id: u64::from_be_bytes(buf[20..28].try_into().ok()?),
        priority1: buf[47],
        priority2: buf[52],
        grandmaster_id: u64::from_be_bytes(buf[53..61].try_into().ok()?),
        steps_removed: u16::from_be_bytes(buf[61..63].try_into().ok()?),
    })
}

/// Parse a PTP Delay_Resp: returns `(sequence_id, receiveTimestamp_ns)`.
pub(crate) fn parse_delay_resp(buf: &[u8]) -> Option<(u16, u64)> {
    if buf.len() < 44 || PtpMessageType::from(buf[0]) != PtpMessageType::DelayResp {
        return None;
    }
    let sequence_id = u16::from_be_bytes(buf[30..32].try_into().ok()?);
    let seconds_hi = u16::from_be_bytes(buf[34..36].try_into().ok()?) as u64;
    let seconds_lo = u32::from_be_bytes(buf[36..40].try_into().ok()?) as u64;
    let nanoseconds = u32::from_be_bytes(buf[40..44].try_into().ok()?) as u64;
    if nanoseconds >= 1_000_000_000 {
        return None;
    }
    let seconds = (seconds_hi << 32) | seconds_lo;
    Some((sequence_id, seconds * 1_000_000_000 + nanoseconds))
}

/// Build a 44-byte IEEE-1588 Delay_Req for `sequence_id`.
pub(crate) fn build_delay_req(sequence_id: u16) -> [u8; 44] {
    let mut buf = [0u8; 44];
    buf[0] = PtpMessageType::DelayReq as u8; // messageType
    buf[1] = 0x02; // PTP version 2
    buf[2..4].copy_from_slice(&44u16.to_be_bytes()); // messageLength
    buf[30..32].copy_from_slice(&sequence_id.to_be_bytes());
    buf[32] = 0x01; // controlField: delay_req
    buf[33] = 0x7f; // logMessageInterval
    buf
}

fn parse_source_and_sequence(buf: &[u8], expected_type: PtpMessageType) -> Option<(u64, u16)> {
    if buf.len() < 32 || PtpMessageType::from(buf[0]) != expected_type {
        return None;
    }

    let clock_id = u64::from_be_bytes(buf[20..28].try_into().ok()?);
    let sequence_id = u16::from_be_bytes(buf[30..32].try_into().ok()?);
    Some((clock_id, sequence_id))
}

#[derive(Debug, Clone, Copy)]
struct PendingSync {
    local_time_ns: u64,
    correction_ns: i64,
}

struct ClockDiscipline {
    pending_syncs: HashMap<(u64, u16), PendingSync>,
    smoother: OffsetSmoother,
    active_clock_id: u64,
    mastership_start_time: u64,
    /// Best Announce seen so far (master-clock selection).
    announced: Option<AnnounceDetails>,
    /// Measured one-way path delay in ns, added to the master origin timestamp.
    path_delay_ns: i64,
    /// Outstanding Delay_Req probes: sequence id → local send time (ns).
    pending_delay_reqs: HashMap<u16, u64>,
    next_delay_req_sequence: u16,
    last_delay_req_ns: Option<u64>,
    /// Address the last Sync came from (Delay_Req target).
    master_addr: Option<std::net::SocketAddr>,
}

impl ClockDiscipline {
    fn new() -> Self {
        Self {
            pending_syncs: HashMap::new(),
            smoother: OffsetSmoother::new(),
            active_clock_id: 0,
            mastership_start_time: 0,
            announced: None,
            path_delay_ns: 0,
            pending_delay_reqs: HashMap::new(),
            next_delay_req_sequence: 0,
            last_delay_req_ns: None,
            master_addr: None,
        }
    }

    /// Apply a grandmaster Announce. A strictly better master resets the
    /// discipline (pending Syncs, offset smoothing) so stale samples from the
    /// previous master cannot leak into the new clock.
    fn observe_announce(&mut self, details: AnnounceDetails) -> bool {
        if let Some(current) = self.announced
            && !details.better_than(&current)
        {
            return false;
        }
        let changed = self.announced != Some(details);
        if changed {
            self.pending_syncs.clear();
            self.smoother.reset();
            self.active_clock_id = 0;
        }
        self.announced = Some(details);
        changed
    }

    /// Whether Follow_Up messages from `clock_id` belong to the selected master.
    fn accepts_clock(&self, clock_id: u64) -> bool {
        self.announced
            .is_none_or(|details| details.source_clock_id == clock_id || details.grandmaster_id == clock_id)
    }

    /// Register a Delay_Req probe (rate-limited). Returns the sequence id to send.
    fn register_delay_req(&mut self, local_now_ns: u64) -> Option<u16> {
        if let Some(last) = self.last_delay_req_ns
            && local_now_ns.saturating_sub(last) < DELAY_REQ_MIN_INTERVAL_NS
        {
            return None;
        }
        self.last_delay_req_ns = Some(local_now_ns);
        self.pending_delay_reqs.retain(|_, sent| {
            local_now_ns.saturating_sub(*sent) <= PENDING_DELAY_REQ_MAX_AGE_NS
        });
        let sequence = self.next_delay_req_sequence;
        self.next_delay_req_sequence = self.next_delay_req_sequence.wrapping_add(1);
        self.pending_delay_reqs.insert(sequence, local_now_ns);
        Some(sequence)
    }

    /// Process a Delay_Resp and update the smoothed one-way path delay.
    fn observe_delay_resp(&mut self, sequence_id: u16, local_recv_ns: u64) -> Option<u64> {
        let sent_ns = self.pending_delay_reqs.remove(&sequence_id)?;
        let round_trip = local_recv_ns.saturating_sub(sent_ns);
        if round_trip == 0 || round_trip > PATH_DELAY_MAX_NS.saturating_mul(2) {
            return None;
        }
        let one_way = round_trip / 2;
        self.path_delay_ns = if self.path_delay_ns == 0 {
            one_way as i64
        } else {
            (self.path_delay_ns + one_way as i64) / 2
        };
        Some(one_way)
    }

    fn observe_sync(&mut self, clock_id: u64, sequence_id: u16, local_time_ns: u64, correction_ns: i64) {
        self.pending_syncs
            .retain(|_, pending| local_time_ns.saturating_sub(pending.local_time_ns) <= PENDING_SYNC_MAX_AGE_NS);
        self.pending_syncs.insert(
            (clock_id, sequence_id),
            PendingSync {
                local_time_ns,
                correction_ns,
            },
        );
    }

    fn observe_follow_up(&mut self, clock: &PtpClock, clock_id: u64, sequence_id: u16, master_origin_ns: u64) -> bool {
        let Some(sync) = self.pending_syncs.remove(&(clock_id, sequence_id)) else {
            return false;
        };

        if self.active_clock_id != clock_id {
            self.active_clock_id = clock_id;
            self.mastership_start_time = sync.local_time_ns;
            self.smoother.reset();
        }

        // The path-delay measurement removes the asymmetric/queuing error that
        // would otherwise look like a constant offset between the clocks.
        let corrected_master_origin = master_origin_ns
            .wrapping_add(sync.correction_ns as u64)
            .wrapping_add(self.path_delay_ns as u64);
        let raw_offset = corrected_master_origin.wrapping_sub(sync.local_time_ns);
        let offset = self.smoother.update(raw_offset, sync.local_time_ns);
        clock.update(clock_id, sync.local_time_ns, offset, self.mastership_start_time);
        true
    }
}

/// AirPlay 2 PTP sink: bind the PTP event (319) and general (320) ports and
/// drain the sender's clock stream. Matching Sync/Follow_Up packets discipline
/// the clock used by buffered-audio playout. With nothing bound, every PTP
/// packet the sender emits bounces an
/// ICMPv6 port-unreachable, which the sender reads as "no PTP peer here" and
/// stalls the buffered-audio start by several seconds; accepting the packets
/// removes that stall (measured ~2.6s → ~0.5s before the type-103 SETUP).
///
/// Received messages are decoded at `debug`; the first is noted once at `info`.
/// Binds gracefully — ports <1024 may need root / `CAP_NET_BIND_SERVICE`; on bind
/// failure it logs one line and returns, leaving normal operation untouched.
/// IPv6-unspecified bind (AirPlay timing traffic is IPv6). Set `SHAIRPLAY_NO_PTP`
/// to skip the bind (for A/B measuring the effect).
pub(crate) async fn spawn_ptp_sink(clock: PtpClock) {
    if std::env::var("SHAIRPLAY_NO_PTP").is_ok() {
        tracing::info!("PTP sink disabled via SHAIRPLAY_NO_PTP — expect slow AP2 connect");
        return;
    }
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let discipline = Arc::new(Mutex::new(ClockDiscipline::new()));
    let mut bound = 0u8;
    for port in [319u16, 320u16] {
        match tokio::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, port)).await {
            Ok(sock) => {
                bound += 1;
                let seen = seen.clone();
                let clock = clock.clone();
                let discipline = discipline.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        match sock.recv_from(&mut buf).await {
                            Ok((n, from)) => {
                                let received_at_ns = now_ns();
                                if let Some(sequence_id) = handle_ptp_packet(
                                    port,
                                    from,
                                    &buf[..n],
                                    received_at_ns,
                                    &seen,
                                    &clock,
                                    &discipline,
                                ) {
                                    // Best-effort path-delay probe: send a Delay_Req
                                    // back to the PTP master's source address. No
                                    // response is required for playback to work.
                                    let target = discipline
                                        .lock()
                                        .ok()
                                        .and_then(|discipline| discipline.master_addr);
                                    if let Some(target) = target {
                                        let request = build_delay_req(sequence_id);
                                        if let Err(error) = sock.send_to(&request, target).await {
                                            tracing::debug!(%error, %target, "PTP Delay_Req send failed");
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(port, "PTP sink recv error: {e}");
                                break;
                            }
                        }
                    }
                });
            }
            Err(e) => tracing::warn!(
                port,
                error = %e,
                "PTP sink: cannot bind (ports <1024 may need sudo / setcap CAP_NET_BIND_SERVICE)"
            ),
        }
    }
    if bound > 0 {
        tracing::info!(
            ports = bound,
            "PTP sink active on 319/320 — accepting sender clock (keeps AP2 connect fast)"
        );
    }
}

/// Decode one inbound PTP datagram at `debug` (message type from the low nibble
/// of byte 0, domain, source, length, plus Follow_Up/Announce detail). The first
/// packet seen is noted once at `info` to confirm the sender engaged PTP.
/// Decode one inbound PTP datagram. Returns a Delay_Req sequence id when the
/// caller should send a path-delay probe to the master's address.
fn handle_ptp_packet(
    port: u16,
    from: std::net::SocketAddr,
    buf: &[u8],
    received_at_ns: u64,
    seen: &std::sync::atomic::AtomicBool,
    clock: &PtpClock,
    discipline: &Mutex<ClockDiscipline>,
) -> Option<u16> {
    if !seen.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::info!(%from, "PTP: receiving sender clock — AP2 timing path healthy");
    }
    let raw_type = buf.first().map(|b| b & 0x0F).unwrap_or(0xFF);
    let domain = buf.get(4).copied().unwrap_or(0);
    let name = match raw_type {
        0 => "Sync",
        1 => "Delay_Req",
        8 => "Follow_Up",
        9 => "Delay_Resp",
        11 => "Announce",
        12 => "Signaling",
        13 => "Management",
        _ => "?",
    };
    tracing::debug!(port, %from, len = buf.len(), msg = name, raw_type, domain, "PTP in");

    if let Some((clock_id, sequence_id)) = parse_source_and_sequence(buf, PtpMessageType::Sync) {
        let correction_raw = i64::from_be_bytes(buf[8..16].try_into().unwrap_or([0; 8]));
        let correction_ns = correction_raw / 65_536;
        if let Ok(mut discipline) = discipline.lock() {
            discipline.master_addr = Some(from);
            discipline.observe_sync(clock_id, sequence_id, received_at_ns, correction_ns);
            return discipline.register_delay_req(received_at_ns);
        }
    } else if let Some((clock_id, origin_ns, corr_ns)) = parse_follow_up(buf) {
        let sequence_id = parse_source_and_sequence(buf, PtpMessageType::FollowUp).map(|(_, sequence_id)| sequence_id);
        let synchronized = sequence_id.is_some_and(|sequence_id| {
            discipline.lock().is_ok_and(|mut discipline| {
                if !discipline.accepts_clock(clock_id) {
                    tracing::debug!(
                        clock_id = format!("{clock_id:016x}"),
                        "Ignoring Follow_Up from a non-selected PTP master"
                    );
                    return false;
                }
                discipline.observe_follow_up(clock, clock_id, sequence_id, origin_ns)
            })
        });
        tracing::debug!(
            clock_id = format!("{clock_id:016x}"),
            origin_ns,
            corr_ns,
            synchronized,
            "  ↳ Follow_Up"
        );
    } else if let Some(details) = parse_announce_details(buf) {
        let changed = discipline.lock().is_ok_and(|mut discipline| discipline.observe_announce(details));
        tracing::debug!(
            source_clock_id = format!("{:016x}", details.source_clock_id),
            grandmaster_id = format!("{:016x}", details.grandmaster_id),
            priority1 = details.priority1,
            priority2 = details.priority2,
            steps_removed = details.steps_removed,
            changed,
            "  ↳ Announce (grandmaster)"
        );
    } else if let Some((sequence_id, receive_ts_ns)) = parse_delay_resp(buf) {
        let path_delay = discipline
            .lock()
            .ok()
            .and_then(|mut discipline| discipline.observe_delay_resp(sequence_id, received_at_ns));
        tracing::debug!(
            sequence_id,
            receive_ts_ns,
            ?path_delay,
            "  ↳ Delay_Resp"
        );
    }
    None
}

/// Offset smoother matching NQPTP behavior.
pub struct OffsetSmoother {
    previous_offset: u64,
    previous_time: u64,
    mastership_start: u64,
    initialized: bool,
}

impl Default for OffsetSmoother {
    fn default() -> Self {
        Self::new()
    }
}

impl OffsetSmoother {
    /// Create a new instance with default state.
    pub fn new() -> Self {
        Self {
            previous_offset: 0,
            previous_time: 0,
            mastership_start: 0,
            initialized: false,
        }
    }

    /// Process a new offset sample. Returns the smoothed offset.
    pub fn update(&mut self, raw_offset: u64, reception_time: u64) -> u64 {
        if !self.initialized {
            self.previous_offset = raw_offset;
            self.previous_time = reception_time;
            self.mastership_start = reception_time;
            self.initialized = true;
            return raw_offset;
        }

        let jitter = raw_offset as i64 - self.previous_offset as i64;
        let mastership_time = reception_time.saturating_sub(self.mastership_start) as i64;

        let smoothed = if jitter < 0 {
            // Negative jitter: clamp and apply slowly
            let clamped = jitter.max(-2_500_000);
            if mastership_time > 1_000_000_000 {
                (self.previous_offset as i64 + clamped / 256) as u64
            } else {
                self.previous_offset
            }
        } else if mastership_time < 1_000_000_000 {
            // Early: accept positive changes quickly
            (self.previous_offset as i64 + jitter) as u64
        } else {
            // Later: smooth positive changes
            (self.previous_offset as i64 + jitter / 16) as u64
        };

        self.previous_offset = smoothed;
        self.previous_time = reception_time;
        smoothed
    }

    /// Reset the smoother to uninitialized state.
    pub fn reset(&mut self) {
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_follow_up_valid() {
        // Construct a minimal Follow_Up message (54 bytes)
        let mut buf = vec![0u8; 54];
        buf[0] = 0x08; // Follow_Up type
        // Clock identity at bytes 20..28
        buf[20..28].copy_from_slice(&0xAABBCCDD11223344u64.to_be_bytes());
        // Correction field at bytes 8..16 (0 for simplicity)
        // preciseOriginTimestamp at bytes 34..44
        // seconds_hi = 0, seconds_lo = 1000, nanoseconds = 500000000
        buf[36..40].copy_from_slice(&1000u32.to_be_bytes());
        buf[40..44].copy_from_slice(&500_000_000u32.to_be_bytes());

        let (clock_id, ts, corr) = parse_follow_up(&buf).unwrap();
        assert_eq!(clock_id, 0xAABBCCDD11223344);
        assert_eq!(ts, 1000 * 1_000_000_000 + 500_000_000);
        assert_eq!(corr, 0);
    }

    #[test]
    fn parse_announce_valid() {
        let mut buf = vec![0u8; 64];
        buf[0] = 0x0B; // Announce type
        buf[20..28].copy_from_slice(&0x1234567890ABCDEFu64.to_be_bytes());
        let clock_id = parse_announce(&buf).unwrap();
        assert_eq!(clock_id, 0x1234567890ABCDEF);
    }

    #[test]
    fn smoother_first_sample_passthrough() {
        let mut s = OffsetSmoother::new();
        let result = s.update(1_000_000, 100_000_000);
        assert_eq!(result, 1_000_000);
    }

    #[test]
    fn smoother_positive_jitter_early() {
        let mut s = OffsetSmoother::new();
        s.update(1_000_000, 0);
        // Early phase (< 1s mastership): positive jitter accepted fully
        let result = s.update(1_100_000, 500_000_000);
        assert_eq!(result, 1_100_000);
    }

    #[test]
    fn smoother_negative_jitter_clamped() {
        let mut s = OffsetSmoother::new();
        s.update(1_000_000, 0);
        // Early phase: negative jitter ignored
        let result = s.update(900_000, 500_000_000);
        assert_eq!(result, 1_000_000); // unchanged
    }

    #[test]
    fn ptp_clock_master_time() {
        let clock = PtpClock::new();
        assert!(clock.master_time_now().is_none());
        clock.update(1, now_ns(), 42, now_ns());
        let mt = clock.master_time_now().unwrap();
        assert!(mt > 42); // should be now + 42
    }

    #[test]
    fn ptp_clock_maps_master_timestamp_to_local_clock() {
        let clock = PtpClock::new();
        let local_now = 10_000_000_000;
        clock.update(0xAABB, local_now, 90_000_000_000, local_now);

        assert_eq!(
            clock.local_time_for_master_time_at(100_500_000_000, 0xAABB, local_now),
            Some(10_500_000_000)
        );
        assert_eq!(
            clock.local_time_for_master_time_at(100_500_000_000, 0xCCDD, local_now),
            None
        );
    }

    #[test]
    fn matched_sync_and_follow_up_update_clock_mapping() {
        let clock = PtpClock::new();
        let mut discipline = ClockDiscipline::new();
        let local_sync_time = 10_000_000_000;

        discipline.observe_sync(0xAABB, 7, local_sync_time, 0);
        assert!(discipline.observe_follow_up(&clock, 0xAABB, 7, 100_000_000_000));

        assert_eq!(
            clock.local_time_for_master_time_at(100_500_000_000, 0xAABB, local_sync_time),
            Some(10_500_000_000)
        );
    }

    #[test]
    fn unmatched_follow_up_does_not_publish_clock() {
        let clock = PtpClock::new();
        let mut discipline = ClockDiscipline::new();

        assert!(!discipline.observe_follow_up(&clock, 0xAABB, 7, 100_000_000_000));
        assert_eq!(clock.get_info().master_clock_id, 0);
    }
}

#[cfg(test)]
mod announce_and_delay_tests {
    use super::*;

    fn announce_packet(source: u64, grandmaster: u64, priority1: u8, priority2: u8, steps: u16) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        buf[0] = 0x0B; // Announce
        buf[20..28].copy_from_slice(&source.to_be_bytes());
        buf[47] = priority1;
        buf[52] = priority2;
        buf[53..61].copy_from_slice(&grandmaster.to_be_bytes());
        buf[61..63].copy_from_slice(&steps.to_be_bytes());
        buf
    }

    #[test]
    fn parses_announce_details() {
        let details = parse_announce_details(&announce_packet(0xAABB, 0xCCDD, 10, 20, 3)).unwrap();
        assert_eq!(details.source_clock_id, 0xAABB);
        assert_eq!(details.grandmaster_id, 0xCCDD);
        assert_eq!(details.priority1, 10);
        assert_eq!(details.priority2, 20);
        assert_eq!(details.steps_removed, 3);

        assert!(parse_announce_details(&[]).is_none());
        assert!(parse_announce_details(&[0u8; 63]).is_none());
        let mut wrong_type = announce_packet(1, 1, 1, 1, 1);
        wrong_type[0] = 0x00;
        assert!(parse_announce_details(&wrong_type).is_none());
    }

    #[test]
    fn better_announce_resets_discipline_and_filters_foreign_masters() {
        let mut discipline = ClockDiscipline::new();
        let first = parse_announce_details(&announce_packet(0xA, 0xA, 128, 128, 1)).unwrap();
        assert!(discipline.observe_announce(first));
        discipline.observe_sync(0xA, 1, 100, 0);
        assert_eq!(discipline.pending_syncs.len(), 1);

        // A worse grandmaster is ignored and does not disturb pending Syncs.
        let worse = parse_announce_details(&announce_packet(0xB, 0xB, 200, 128, 1)).unwrap();
        assert!(!discipline.observe_announce(worse));
        assert_eq!(discipline.pending_syncs.len(), 1);
        assert!(discipline.accepts_clock(0xA));
        assert!(!discipline.accepts_clock(0xB));

        // A better grandmaster becomes the selected clock and resets state.
        let better = parse_announce_details(&announce_packet(0xC, 0xC, 10, 10, 0)).unwrap();
        assert!(discipline.observe_announce(better));
        assert!(discipline.pending_syncs.is_empty());
        assert!(discipline.accepts_clock(0xC));
        assert!(!discipline.accepts_clock(0xA));
    }

    #[test]
    fn delay_req_response_measures_path_delay() {
        let mut discipline = ClockDiscipline::new();
        let sent_at = 1_000_000_000u64;
        let sequence = discipline.register_delay_req(sent_at).unwrap();
        // Rate limited.
        assert!(discipline.register_delay_req(sent_at + 1).is_none());

        let one_way = discipline
            .observe_delay_resp(sequence, sent_at + 10_000_000)
            .expect("path delay");
        assert_eq!(one_way, 5_000_000);
        assert_eq!(discipline.path_delay_ns, 5_000_000);
    }

    #[test]
    fn unknown_or_implausible_delay_responses_are_ignored() {
        let mut discipline = ClockDiscipline::new();
        assert!(discipline.observe_delay_resp(42, 100).is_none());

        let sent_at = 0u64;
        let sequence = discipline.register_delay_req(sent_at).unwrap();
        // A round trip above the sanity limit must be dropped.
        assert!(discipline.observe_delay_resp(sequence, 10_000_000_000).is_none());
        assert_eq!(discipline.path_delay_ns, 0);
    }

    #[test]
    fn parse_delay_resp_validates_fields_and_timestamp() {
        let mut resp = vec![0u8; 44];
        resp[0] = 0x09; // Delay_Resp
        resp[30..32].copy_from_slice(&7u16.to_be_bytes());
        resp[36..40].copy_from_slice(&1000u32.to_be_bytes());
        resp[40..44].copy_from_slice(&500_000_000u32.to_be_bytes());
        assert_eq!(
            parse_delay_resp(&resp),
            Some((7, 1_000_500_000_000))
        );

        // Nanoseconds field must be below 1e9.
        resp[40..44].copy_from_slice(&1_000_000_000u32.to_be_bytes());
        assert!(parse_delay_resp(&resp).is_none());

        assert!(parse_delay_resp(&resp[..32]).is_none());
        resp[0] = 0x08;
        assert!(parse_delay_resp(&resp).is_none());
    }

    #[test]
    fn path_delay_compensates_the_follow_up_offset() {
        let clock = PtpClock::new();
        let mut discipline = ClockDiscipline::new();
        let sequence = discipline.register_delay_req(0).unwrap();
        discipline.observe_delay_resp(sequence, 10_000_000); // 5 ms one-way

        let local_sync_time = 10_000_000_000u64;
        discipline.observe_sync(0xAA, 7, local_sync_time, 0);
        assert!(discipline.observe_follow_up(&clock, 0xAA, 7, 100_000_000_000));

        // offset = master origin + one-way path delay − local receive time.
        assert_eq!(clock.get_info().offset, 90_005_000_000);
        assert_eq!(
            clock.local_time_for_master_time_at(100_500_000_000, 0xAA, local_sync_time),
            Some(10_495_000_000)
        );
    }

    #[test]
    fn delay_req_packet_has_a_valid_header() {
        let request = build_delay_req(0x1234);
        assert_eq!(request[0], 0x01, "Delay_Req message type");
        assert_eq!(request[1], 0x02, "PTP version 2");
        assert_eq!(u16::from_be_bytes([request[2], request[3]]), 44);
        assert_eq!(u16::from_be_bytes([request[30], request[31]]), 0x1234);
    }
}

/// PTP-anchored audio playout timing.
/// Converts RTP timestamps to local playout times using PTP clock offset.
pub struct PtpAnchor {
    /// PTP master clock identity.
    pub clock_id: u64,
    /// RTP timestamp at the anchor point.
    pub anchor_rtp: u32,
    /// Master network time (ns) at the anchor point.
    pub anchor_network_time_ns: u64,
    /// Audio sample rate (for RTP timestamp → time conversion).
    pub sample_rate: u32,
}

impl PtpAnchor {
    /// Set anchor from SETRATEANCHORTI parameters.
    pub fn new(clock_id: u64, rtp_time: u32, network_secs: u64, network_frac: u64, sample_rate: u32) -> Self {
        // Convert network time to nanoseconds (frac is 64-bit fixed point, MSB = 0.5)
        let frac_ns = ((network_frac >> 32) * 1_000_000_000) >> 32;
        let network_time_ns = network_secs * 1_000_000_000 + frac_ns;
        Self {
            clock_id,
            anchor_rtp: rtp_time,
            anchor_network_time_ns: network_time_ns,
            sample_rate,
        }
    }

    /// Given a PTP clock offset, compute the local time (ns) when an RTP frame should play.
    /// `ptp_offset` = add to local time to get master time (from PtpClock).
    pub fn local_playout_time(&self, rtp_timestamp: u32, ptp_offset: u64) -> u64 {
        let frame_diff = rtp_timestamp.wrapping_sub(self.anchor_rtp) as i64;
        let time_diff_ns = (frame_diff * 1_000_000_000) / self.sample_rate as i64;
        let master_playout = (self.anchor_network_time_ns as i64 + time_diff_ns) as u64;
        // local_time = master_time - offset
        master_playout.wrapping_sub(ptp_offset)
    }

    /// How many nanoseconds until this RTP frame should play?
    pub fn delay_until_playout(&self, rtp_timestamp: u32, ptp_offset: u64) -> i64 {
        let target = self.local_playout_time(rtp_timestamp, ptp_offset);
        let now = now_ns();
        target as i64 - now as i64
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    #[test]
    fn anchor_playout_at_anchor_point() {
        let anchor = PtpAnchor::new(1, 1000, 5, 0, 44100);
        // At the anchor RTP time, playout should be at anchor_network_time - offset
        let local = anchor.local_playout_time(1000, 100);
        assert_eq!(local, 5_000_000_000 - 100);
    }

    #[test]
    fn anchor_playout_one_second_later() {
        let anchor = PtpAnchor::new(1, 0, 10, 0, 44100);
        // 44100 frames later = 1 second
        let local = anchor.local_playout_time(44100, 0);
        assert_eq!(local, 11_000_000_000); // 10s + 1s
    }

    #[test]
    fn anchor_playout_with_offset() {
        let anchor = PtpAnchor::new(1, 0, 10, 0, 48000);
        // 48000 frames = 1 second, offset = 500ms
        let local = anchor.local_playout_time(48000, 500_000_000);
        // master time = 11s, local = 11s - 0.5s = 10.5s
        assert_eq!(local, 10_500_000_000);
    }

    #[test]
    fn anchor_network_frac_conversion() {
        // frac = 0x8000000000000000 means 0.5 seconds
        let anchor = PtpAnchor::new(1, 0, 1, 0x8000_0000_0000_0000, 44100);
        assert_eq!(anchor.anchor_network_time_ns, 1_500_000_000);
    }
}
