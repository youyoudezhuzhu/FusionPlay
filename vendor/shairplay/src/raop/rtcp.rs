//! Minimal RTCP control-channel parser for AirPlay 2 (AP2).
//!
//! AP2 senders keep a UDP "control port" open for the lifetime of a connection
//! (advertised in the type 103 SETUP response) and use it for RTCP. The packets
//! of interest here are the *time announces*:
//!
//! * type 215 (`TIME_ANNOUNCE_PTP`, 28 bytes): `senderRtpTimestamp` (play-at
//!   frame) plus the sender's PTP master clock time and clock identity.
//! * type 212 (`TIME_ANNOUNCE_NTP`, 20 or 32 bytes): same shape but with an NTP
//!   timestamp instead of a PTP one.
//!
//! They tell the receiver which RTP frame should be audible at a given sender
//! clock instant, which lets the buffered-audio playout derive a playout
//! deadline even before `SETRATEANCHORTIME` arrives. Sender/Receiver reports and
//! retransmit requests are drained without further processing.
//!
//! Layout follows the captures documented by the openairplay `airplay2-receiver`
//! (see `ap2/connections/control.py`) and shairport-sync's
//! `rtp_ap2_control_receiver`.

/// RTCP Sender Report (drained, not interpreted).
#[allow(dead_code)]
pub(crate) const RTCP_TYPE_SENDER_REPORT: u8 = 200;
/// RTCP Receiver Report (drained, not interpreted).
#[allow(dead_code)]
pub(crate) const RTCP_TYPE_RECEIVER_REPORT: u8 = 201;
/// AP2 time announce using an NTP timestamp.
pub(crate) const RTCP_TYPE_TIME_ANNOUNCE_NTP: u8 = 212;
/// AP2 time announce using a PTP timestamp.
pub(crate) const RTCP_TYPE_TIME_ANNOUNCE_PTP: u8 = 215;

/// How far behind the sender's reported `play-at` frame the receiver should be
/// for the RTCP-announce fallback. shairport-sync uses 11025 frames here (its
/// realtime-stream fudge factor); for buffered audio the anchor is normally
/// provided authoritatively by `SETRATEANCHORTIME`, so this is only a fallback.
pub(crate) const RTCP_ANCHOR_FUDGE_FRAMES: u32 = 11_025;

/// A parsed time announce: the sender's RTP timeline mapped onto its clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeAnnounce {
    /// RTP timestamp the sender says should be playing at `master_time_ns`
    /// (after applying [`RTCP_ANCHOR_FUDGE_FRAMES`] this is the anchor frame).
    pub(crate) play_at_rtp: u32,
    /// Sender clock time for `play_at_rtp`: PTP master nanoseconds (type 215)
    /// or Unix/NTP time in nanoseconds (type 212).
    pub(crate) master_time_ns: u64,
    /// PTP clock identity of the sender's elected grandmaster (0 for NTP).
    pub(crate) clock_id: u64,
    /// Local receive time in the same timebase as [`crate::util::now_ns`].
    pub(crate) received_at_ns: u64,
    /// Whether this announce uses PTP (`true`) or NTP (`false`) time.
    pub(crate) is_ptp: bool,
}

/// One decomposed RTCP packet of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RtcpEvent {
    /// A sender time announce (PTP or NTP flavour).
    TimeAnnounce(TimeAnnounce),
}

fn be_u32(buf: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(offset..offset + 4)?.try_into().ok()?))
}

fn be_u64(buf: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_be_bytes(buf.get(offset..offset + 8)?.try_into().ok()?))
}

/// Parse one RTCP packet (not a compound packet) at the start of `buf`.
///
/// Returns `None` for anything that is not a well-formed time announce,
/// including malformed/truncated lengths and bad padding. This function never
/// panics and never allocates.
pub(crate) fn parse_rtcp_packet(buf: &[u8], received_at_ns: u64) -> Option<RtcpEvent> {
    if buf.len() < 4 {
        return None;
    }
    // Version must be 2 (the RTCP version used by AirPlay).
    if buf[0] >> 6 != 2 {
        return None;
    }
    let padding = buf[0] & 0x20 != 0;
    let payload_type = buf[1];
    let total_len = (usize::from(u16::from_be_bytes([buf[2], buf[3]])) + 1) * 4;
    if total_len < 4 || total_len > buf.len() {
        return None;
    }

    let mut packet = &buf[..total_len];
    if padding {
        let padding_len = *packet.last()? as usize;
        // Padding must fit inside the packet (and leave room for the header).
        if padding_len == 0 || padding_len > packet.len() - 4 {
            return None;
        }
        packet = &packet[..packet.len() - padding_len];
    }

    match (payload_type, packet.len()) {
        (RTCP_TYPE_TIME_ANNOUNCE_PTP, 28) => {
            let sender_rtp = be_u32(packet, 4)?;
            let play_at_rtp = be_u32(packet, 16)?;
            let master_time_ns = be_u64(packet, 8)?;
            let clock_id = be_u64(packet, 20)?;
            Some(RtcpEvent::TimeAnnounce(TimeAnnounce {
                // shairport-sync anchors on the frame that already includes the
                // stream latency, backing off its realtime fudge factor.
                play_at_rtp: play_at_rtp.max(sender_rtp).saturating_sub(RTCP_ANCHOR_FUDGE_FRAMES),
                master_time_ns,
                clock_id,
                received_at_ns,
                is_ptp: true,
            }))
        }
        (RTCP_TYPE_TIME_ANNOUNCE_NTP, 20 | 32) => {
            let sender_rtp = be_u32(packet, 4)?;
            let play_at_rtp = be_u32(packet, 16)?;
            // 64-bit NTP timestamp: upper 32 bits seconds, lower 32 bits fraction.
            let seconds = u64::from(be_u32(packet, 8)?);
            let fraction = u64::from(be_u32(packet, 12)?);
            let master_time_ns = seconds
                .saturating_mul(1_000_000_000)
                .saturating_add((fraction.saturating_mul(1_000_000_000)) >> 32);
            Some(RtcpEvent::TimeAnnounce(TimeAnnounce {
                play_at_rtp: play_at_rtp.max(sender_rtp).saturating_sub(RTCP_ANCHOR_FUDGE_FRAMES),
                master_time_ns,
                clock_id: 0,
                received_at_ns,
                is_ptp: false,
            }))
        }
        _ => None,
    }
}

/// Parse every RTCP time announce in a (possibly compound) datagram.
///
/// Compound RTCP packets concatenate several RTCP packets; only the first
/// packet of a compound may carry the padding flag. Iteration stops at the
/// first malformed length so a corrupted datagram cannot loop or over-read.
pub(crate) fn parse_rtcp_compound(buf: &[u8], received_at_ns: u64) -> Vec<TimeAnnounce> {
    let mut announces = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= buf.len() {
        let total_len = (usize::from(u16::from_be_bytes([buf[offset + 2], buf[offset + 3]])) + 1) * 4;
        if total_len < 4 || offset + total_len > buf.len() {
            break;
        }
        let packet = &buf[offset..offset + total_len];
        if let Some(RtcpEvent::TimeAnnounce(announce)) = parse_rtcp_packet(packet, received_at_ns) {
            announces.push(announce);
        }
        offset += total_len;
    }
    announces
}

/// Owns the per-connection RTCP control listener and stops it on drop.
///
/// The control UDP socket is advertised in the SETUP response and kept open for
/// the lifetime of the RTSP connection. Dropping the `RtcpListener` (connection
/// teardown, AP2 stream replacement, or a failed SETUP) aborts the task and
/// releases the socket, so a connection cannot leak a socket or task.
pub(crate) struct RtcpListener {
    handle: tokio::task::JoinHandle<()>,
}

impl RtcpListener {
    /// Spawn a listener that records the latest time announce into `announce`.
    pub(crate) fn spawn(
        socket: tokio::net::UdpSocket,
        announce: std::sync::Arc<std::sync::Mutex<Option<TimeAnnounce>>>,
    ) -> Self {
        let handle = tokio::spawn(async move {
            rtcp_receive_loop(socket, announce).await;
        });
        Self { handle }
    }
}

impl Drop for RtcpListener {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn rtcp_receive_loop(
    socket: tokio::net::UdpSocket,
    announce: std::sync::Arc<std::sync::Mutex<Option<TimeAnnounce>>>,
) {
    let mut buf = [0u8; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, from)) => {
                let received_at_ns = crate::util::now_ns();
                for time_announce in parse_rtcp_compound(&buf[..len], received_at_ns) {
                    tracing::debug!(
                        %from,
                        len,
                        is_ptp = time_announce.is_ptp,
                        clock_id = format!("{:016x}", time_announce.clock_id),
                        play_at_rtp = time_announce.play_at_rtp,
                        "RTCP time announce"
                    );
                    if let Ok(mut slot) = announce.lock() {
                        *slot = Some(time_announce);
                    }
                }
            }
            Err(error) => {
                tracing::debug!(%error, "RTCP control listener stopped");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PTP time announce from an openairplay capture:
    /// `90 d7 00 06 21 36 78 19 00 01 a4 03 e3 be 94 b5 21 37 a5 90
    ///  01 02 03 04 05 06 07 08`
    fn ptp_announce() -> Vec<u8> {
        vec![
            0x90, 0xd7, 0x00, 0x06, 0x21, 0x36, 0x78, 0x19, 0x00, 0x01, 0xa4, 0x03, 0xe3, 0xbe, 0x94,
            0xb5, 0x21, 0x37, 0xa5, 0x90, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        ]
    }

    #[test]
    fn parses_ptp_time_announce() {
        let announce = match parse_rtcp_packet(&ptp_announce(), 42).unwrap() {
            RtcpEvent::TimeAnnounce(a) => a,
        };
        // play-at frame minus the 11025-frame realtime fudge factor.
        assert_eq!(announce.play_at_rtp, 0x2137_a590 - 11_025);
        assert_eq!(announce.master_time_ns, 0x0001_a403_e3be_94b5);
        assert_eq!(announce.clock_id, 0x0102_0304_0506_0708);
        assert_eq!(announce.received_at_ns, 42);
        assert!(announce.is_ptp);
    }

    #[test]
    fn parses_ntp_time_announce() {
        // 20-byte NTP announce: header + senderRtp + ntp seconds/fraction + playAtRtp.
        let mut packet = vec![0x80, 0xd4, 0x00, 0x04];
        packet.extend_from_slice(&0x1122_3344u32.to_be_bytes()); // senderRtpTimestamp
        packet.extend_from_slice(&1_700_000_000u32.to_be_bytes()); // NTP seconds
        packet.extend_from_slice(&0x8000_0000u32.to_be_bytes()); // NTP fraction = 0.5 s
        packet.extend_from_slice(&0x5566_7788u32.to_be_bytes()); // playAtRtpTimestamp

        let announce = match parse_rtcp_packet(&packet, 7).unwrap() {
            RtcpEvent::TimeAnnounce(a) => a,
        };
        assert_eq!(announce.play_at_rtp, 0x5566_7788 - 11_025);
        assert_eq!(announce.master_time_ns, 1_700_000_000_500_000_000);
        assert_eq!(announce.clock_id, 0);
        assert!(!announce.is_ptp);
    }

    #[test]
    fn rejects_truncated_and_malformed_packets() {
        let announce = ptp_announce();
        // Truncated length/datagram.
        assert!(parse_rtcp_packet(&announce[..12], 0).is_none());
        assert!(
            parse_rtcp_packet(&announce[..announce.len() - 1], 0).is_none(),
            "length field exceeding the datagram must be rejected"
        );
        // Wrong RTCP version.
        let mut wrong_version = announce.clone();
        wrong_version[0] = 0x40;
        assert!(parse_rtcp_packet(&wrong_version, 0).is_none());
        // Length field that is not a multiple of 4/too small.
        let mut bad_length = announce.clone();
        bad_length[2] = 0;
        bad_length[3] = 0;
        assert!(parse_rtcp_packet(&bad_length, 0).is_none());
        // Invalid padding length.
        let mut bad_padding = announce.clone();
        bad_padding[0] |= 0x20;
        *bad_padding.last_mut().unwrap() = 0xff;
        assert!(parse_rtcp_packet(&bad_padding, 0).is_none());
        // Empty/short datagrams.
        assert!(parse_rtcp_packet(&[], 0).is_none());
        assert!(parse_rtcp_packet(&[0x80], 0).is_none());
    }

    #[test]
    fn ignores_sender_reports_and_unknown_types() {
        // A 28-byte Sender Report must be drained without being misparsed.
        let mut sr = vec![0x80, RTCP_TYPE_SENDER_REPORT, 0x00, 0x06];
        sr.resize(28, 0);
        assert!(parse_rtcp_packet(&sr, 0).is_none());

        let mut unknown = ptp_announce();
        unknown[1] = 0xfe;
        assert!(parse_rtcp_packet(&unknown, 0).is_none());
    }

    #[test]
    fn compound_packet_returns_every_time_announce() {
        let mut compound = Vec::new();
        // Sender Report, then two PTP announces.
        let mut sr = vec![0x80, RTCP_TYPE_SENDER_REPORT, 0x00, 0x06];
        sr.resize(28, 0);
        compound.extend_from_slice(&sr);
        compound.extend_from_slice(&ptp_announce());
        let mut second = ptp_announce();
        second[4..8].copy_from_slice(&0x2136_7819u32.to_be_bytes());
        compound.extend_from_slice(&second);

        let announces = parse_rtcp_compound(&compound, 99);
        assert_eq!(announces.len(), 2);
        assert_eq!(announces[0].received_at_ns, 99);
        assert_eq!(announces[0].master_time_ns, 0x0001_a403_e3be_94b5);

        // Garbage after a valid packet stops parsing instead of looping.
        compound.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(parse_rtcp_compound(&compound, 0).len(), 2);
    }

    #[tokio::test]
    async fn listener_records_announce_from_the_control_channel() {
        use std::sync::{Arc, Mutex};

        let listener_socket = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let listener_addr = listener_socket.local_addr().unwrap();
        let announce = Arc::new(Mutex::new(None));
        let listener = RtcpListener::spawn(listener_socket, Arc::clone(&announce));

        let sender = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        sender.send_to(&ptp_announce(), listener_addr).await.unwrap();

        let recorded = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(slot) = announce.lock()
                    && slot.is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(recorded.is_ok(), "RTCP listener did not record the time announce");

        let recorded = announce.lock().unwrap().expect("announce");
        assert_eq!(recorded.play_at_rtp, 0x2137_a590 - 11_025);
        assert!(recorded.is_ptp);

        // Dropping the listener aborts the task (and closes the socket).
        drop(listener);
    }

    #[test]
    fn random_datagrams_never_panic() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..5_000 {
            let len = (seed % 129) as usize;
            let mut datagram = vec![0u8; len];
            for byte in &mut datagram {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = seed as u8;
            }
            let _ = parse_rtcp_packet(&datagram, 0);
            let _ = parse_rtcp_compound(&datagram, 0);
        }
    }
}
