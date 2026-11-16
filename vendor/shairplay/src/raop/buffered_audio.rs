//! AirPlay 2 buffered audio processor (stream type 103).
//!
//! Receives encrypted AAC/ALAC packets over TCP, decrypts with
//! ChaCha20-Poly1305, decodes (symphonia AAC or the crate's ALAC decoder),
//! resamples/mixes down, and delivers F32LE PCM through a timed playout buffer.
//!
//! Three concurrent tasks:
//! - **Receiver** (tokio): accepts TCP, decrypts, decodes, buffers by RTP timestamp
//! - **Command handler** (tokio): processes SetRate/Flush/Stop from RTSP thread
//! - **Delivery** (std::thread): timed playout using anchor-based scheduling
//!
//! RTP timestamps are extended to a 64-bit, wrap-aware timeline: a 44.1 kHz u32
//! timestamp wraps every ~27 hours, and the old `u32` + signed-difference
//! playout math started misbehaving once a stream passed 2^31 frames (~13.5 h).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::codec::aac::{AacDecoder, AudioSsrc};
use crate::codec::alac::{AlacDecoder, AlacFormat, alac_specific_config};
use crate::error::{CodecError, NetworkError, ShairplayError};
use crate::net::ptp::PtpClock;
use crate::raop::audio_pipeline::{NONCE_TRAIL_LEN, RTP_HEADER_LEN, decrypt_rtp_chacha};
use crate::raop::rtcp::TimeAnnounce;
use crate::raop::{AudioCodec, AudioFormat, AudioHandler, SourceAudioCodec, SourceAudioFormat};
use crate::util::now_ns;

/// Default ALAC frame length when the SETUP does not advertise `spf`. Apple
/// streaming ALAC normally uses `spf`, but 4096 is the standard ALAC frame
/// length and keeps the decoder usable if it is omitted.
const DEFAULT_ALAC_FRAME_LENGTH: u32 = 4_096;

/// Ignore RTCP time announces older than this (the sender stopped announcing,
/// so its RTP↔clock mapping is no longer trustworthy).
const TIME_ANNOUNCE_MAX_AGE_NS: u64 = 2_000_000_000;

/// Bound the decoded PCM held per stream (~20 s) so a sender that stops
/// consuming playout (paused clock, network stall) cannot grow memory without
/// limit.
const MAX_BUFFERED_SECONDS: usize = 20;

/// How far the sender's RTP timeline may move backwards before it is treated
/// as a restart (seek/new track) instead of packet reordering.
const TIMELINE_REBASE_SECONDS: u32 = 30;

/// Packets remembered for mapping FLUSHBUFFERED sequence numbers back to RTP
/// timestamps. A flush normally covers the last few seconds (≈100 packets).
const MAX_SEQ_HISTORY: usize = 4_096;

#[derive(Debug, Clone)]
/// Output configuration passed from the server builder.
pub(crate) struct OutputConfig {
    /// Target sample rate, or None for source native rate.
    pub(crate) sample_rate: Option<u32>,
    /// Maximum output channels, or None to pass through.
    pub(crate) max_channels: Option<u8>,
}

/// Input configuration passed from the type-103 SETUP and the RTCP control
/// channel.
#[derive(Debug, Clone)]
pub(crate) struct BufferedSourceConfig {
    /// ALAC parameters advertised by the SETUP `audioFormat` field, if that
    /// field describes an ALAC format.
    pub(crate) alac_format: Option<AlacFormat>,
    /// Samples per ALAC frame (`spf`), when the sender provided it.
    pub(crate) samples_per_frame: Option<u32>,
    /// Latest RTCP time announce, written by the per-connection control-channel
    /// listener and read by the playout.
    pub(crate) time_announce: Arc<Mutex<Option<TimeAnnounce>>>,
    /// Sound-first fallback: deliver decoded frames as soon as they arrive
    /// instead of gating them on anchor/RTP-timestamp timing.
    ///
    /// 1.4.0–1.4.2 devices decoded/decrypted every packet but the timed playout
    /// delivered only the first frame, so the local output stayed silent. The
    /// local `AudioBuffer` cushion still provides jitter tolerance; this can be
    /// turned off once the timed path is validated on device.
    pub(crate) direct_feed: bool,
}

impl Default for BufferedSourceConfig {
    fn default() -> Self {
        Self {
            alac_format: None,
            samples_per_frame: None,
            time_announce: Arc::new(Mutex::new(None)),
            direct_feed: true,
        }
    }
}

#[derive(Debug)]
/// Commands sent from the RTSP handler thread to the playout engine.
pub enum PlayoutCommand {
    /// Set playback rate and anchor point. rate=0 means pause.
    SetRate {
        /// RTP timestamp at the anchor point.
        anchor_rtp: u32,
        /// Network time at the anchor point (ns).
        anchor_time_ns: u64,
        /// PTP timeline identity for `anchor_time_ns`, or zero if omitted.
        anchor_clock_id: u64,
        /// Playback rate (1 = playing, 0 = paused).
        rate: u32,
    },
    /// Pause playout while preserving the current anchor and buffered stream.
    Pause,
    /// Resume the existing stream, re-anchoring to the earliest buffered frame.
    Resume,
    /// Flush buffered frames in the given RTP timestamp range.
    Flush {
        /// First timestamp to flush.
        from_seq: u32,
        /// Last timestamp to flush.
        until_seq: u32,
    },
    /// Stop playback and tear down.
    Stop,
}

/// Wrap-aware extension of 32-bit RTP timestamps onto a monotonic 64-bit
/// timeline. Reordering is handled by keeping the latest extended value as the
/// reference; a jump further back than `max_backward_frames` is interpreted as
/// the sender restarting its timeline (seek/new track).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TimestampExtender {
    last_raw: u32,
    last_ext: u64,
    initialized: bool,
}

impl TimestampExtender {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Extend `raw` onto the 64-bit timeline. Returns the extended value and
    /// whether the timeline was rebased (a large backward jump was detected).
    pub(crate) fn extend_tracking(&mut self, raw: u32, max_backward_frames: u32) -> (u64, bool) {
        if !self.initialized {
            self.initialized = true;
            self.last_raw = raw;
            self.last_ext = u64::from(raw);
            return (self.last_ext, false);
        }

        let delta = i64::from(raw.wrapping_sub(self.last_raw) as i32);
        if delta < -i64::from(max_backward_frames) {
            // The sender's timeline restarted (seek, new track) or the delta is
            // genuinely ambiguous; rebase rather than mapping into the future.
            self.last_raw = raw;
            self.last_ext = u64::from(raw);
            return (self.last_ext, true);
        }

        let candidate = if delta >= 0 {
            self.last_ext + delta as u64
        } else {
            self.last_ext.saturating_sub((-delta) as u64)
        };
        if candidate > self.last_ext {
            self.last_ext = candidate;
            self.last_raw = raw;
        }
        (candidate, false)
    }

    /// Extend `raw` onto the timeline, discarding any reorder bookkeeping.
    pub(crate) fn extend(&mut self, raw: u32, max_backward_frames: u32) -> u64 {
        self.extend_tracking(raw, max_backward_frames).0
    }

}

/// Which source established the current playout anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorSource {
    /// No anchor yet.
    None,
    /// Authoritative `SETRATEANCHORTIME` anchor (RTP timestamp + network time).
    SetRate,
    /// Anchored to the earliest buffered frame after RECORD/resume.
    Resume,
    /// Derived from an RTCP time announce (fallback when SETRATE is missing).
    RtcpAnnounce,
}

struct PlayoutState {
    /// rtp_timestamp (extended 64-bit) → F32 PCM samples
    buffer: BTreeMap<u64, Vec<f32>>,
    timestamps: TimestampExtender,
    anchor_rtp: u64,
    anchor_local_ns: u64,
    anchor_valid: bool,
    anchor_source: AnchorSource,
    rate: u32,
    rtp_clock_rate: u32,
    sample_rate: u32,
    channels: u8,
    source_format: Option<SourceAudioFormat>,
    /// Total number of `f32` samples currently held in `buffer`.
    buffered_f32: usize,
    max_buffered_f32: usize,
    stopped: bool,
    format_changed: bool,
    /// See [`BufferedSourceConfig::direct_feed`].
    direct_feed: bool,
    /// sequence number → extended RTP timestamp of packets seen recently.
    seq_history: VecDeque<(u16, u64)>,
    /// Diagnostics: FLUSH commands applied and frames they removed.
    flushes: u64,
    flushed_frames: u64,
}

/// Decoder chosen for the stream's SSRC. Apple can switch AAC↔ALAC mid-stream.
enum BufferedDecoder {
    Aac(AacDecoder),
    Alac(AlacDecoder),
}

impl BufferedDecoder {
    fn for_ssrc(ssrc: AudioSsrc, config: &BufferedSourceConfig) -> Result<Self, String> {
        if ssrc.is_alac() {
            let sample_rate = ssrc.sample_rate().ok_or("ALAC SSRC without sample rate")?;
            let bit_depth = ssrc.bit_depth().ok_or("ALAC SSRC without bit depth")?;
            let channels = ssrc.channels().ok_or("ALAC SSRC without channel count")?;
            // The SETUP `audioFormat` should agree with the SSRC; if it does, its
            // bit depth is preferred (it is what the sender negotiated).
            let bit_depth = config
                .alac_format
                .filter(|format| {
                    format.sample_rate == sample_rate && format.channels == channels
                })
                .map(|format| format.bit_depth)
                .unwrap_or(bit_depth);
            let frame_length = config
                .samples_per_frame
                .unwrap_or(DEFAULT_ALAC_FRAME_LENGTH)
                .clamp(1, crate::codec::alac::MAX_ALAC_SAMPLES_PER_FRAME);
            let mut decoder = AlacDecoder::new(i32::from(bit_depth), i32::from(channels));
            decoder.set_info(&alac_specific_config(
                sample_rate,
                bit_depth,
                channels,
                frame_length,
            ));
            if !decoder.is_configured() {
                return Err(format!(
                    "ALAC decoder configuration rejected (sample_rate={sample_rate}, bit_depth={bit_depth}, channels={channels}, frame_length={frame_length})"
                ));
            }
            Ok(Self::Alac(decoder))
        } else if ssrc.is_aac() {
            let sample_rate = ssrc.sample_rate().ok_or("AAC SSRC without sample rate")?;
            let channels = ssrc.channels().ok_or("AAC SSRC without channel count")?;
            AacDecoder::new(sample_rate, channels)
                .map(Self::Aac)
                .map_err(|error| format!("AAC decoder init failed: {error}"))
        } else {
            Err(format!("unrecognized audio SSRC {ssrc:?}"))
        }
    }

    fn source_codec(&self) -> SourceAudioCodec {
        match self {
            Self::Aac(_) => SourceAudioCodec::Aac,
            Self::Alac(_) => SourceAudioCodec::Alac,
        }
    }

    fn decode(&mut self, payload: &[u8]) -> Option<Vec<f32>> {
        match self {
            Self::Aac(decoder) => decoder.decode(payload).map(|raw| {
                raw.chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect()
            }),
            Self::Alac(decoder) => decoder.decode_frame_f32(payload),
        }
    }
}

/// Rate limiter so a sender flooding unrecognized SSRCs cannot flood the log.
struct RateLimitedWarn {
    last_ns: u64,
    interval_ns: u64,
}

impl RateLimitedWarn {
    fn new(interval: Duration) -> Self {
        Self {
            last_ns: 0,
            interval_ns: interval.as_nanos() as u64,
        }
    }

    fn allow(&mut self, now_ns: u64) -> bool {
        if now_ns.saturating_sub(self.last_ns) >= self.interval_ns {
            self.last_ns = now_ns;
            true
        } else {
            false
        }
    }
}

fn pause_playout(state: &mut PlayoutState) {
    state.rate = 0;
}

fn resume_playout(state: &mut PlayoutState) {
    state.rate = 1;
    if let Some(&first_ts) = state.buffer.keys().next() {
        let lead_frames = u64::from(state.rtp_clock_rate / 10);
        state.anchor_rtp = first_ts.saturating_sub(lead_frames);
        state.anchor_local_ns = now_ns();
        state.anchor_valid = true;
        state.anchor_source = AnchorSource::Resume;
    } else {
        // The next received frame will establish the anchor.
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
    }
}

/// Whether a 16-bit RTP sequence number falls inside `from..=until`, handling
/// wraparound.
fn seq_in_range(seq: u16, from: u16, until: u16) -> bool {
    if from <= until {
        seq >= from && seq <= until
    } else {
        seq >= from || seq <= until
    }
}

fn anchor_first_resumed_packet(state: &mut PlayoutState, timestamp: u64) {
    if state.rate == 0 || state.anchor_valid {
        return;
    }
    let lead_frames = u64::from(state.rtp_clock_rate / 10);
    state.anchor_rtp = timestamp.saturating_sub(lead_frames);
    state.anchor_local_ns = now_ns();
    state.anchor_valid = true;
    state.anchor_source = AnchorSource::Resume;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorMapping {
    Ptp,
    LocalFallback,
}

fn set_rate_anchor(
    state: &mut PlayoutState,
    ptp_clock: &PtpClock,
    anchor_rtp: u32,
    anchor_time_ns: u64,
    anchor_clock_id: u64,
    // PTP clock identity learned from RTCP time announces; used to resolve the
    // anchor when `anchor_clock_id` is a network-timeline id that does not
    // match the PTP sink's master id.
    fallback_clock_id: u64,
    rate: u32,
    local_now_ns: u64,
) -> (AnchorMapping, usize) {
    state.rate = rate;
    if rate == 0 {
        return (AnchorMapping::LocalFallback, 0);
    }

    let max_backward = state
        .rtp_clock_rate
        .saturating_mul(TIMELINE_REBASE_SECONDS);
    let timeline_rebased = if anchor_rtp != 0 {
        let (anchor_ext, rebased) = state.timestamps.extend_tracking(anchor_rtp, max_backward);
        state.anchor_rtp = anchor_ext;
        state.anchor_valid = true;
        state.anchor_source = AnchorSource::SetRate;
        rebased
    } else if let Some(&first_ts) = state.buffer.keys().next() {
        let lead_frames = u64::from(state.rtp_clock_rate / 10);
        state.anchor_rtp = first_ts.saturating_sub(lead_frames);
        state.anchor_valid = true;
        state.anchor_source = AnchorSource::Resume;
        false
    } else {
        // SETRATEANCHORTIME can arrive before the first buffered packet. Let
        // packet insertion establish an anchor instead of comparing a random
        // RTP timestamp against zero.
        state.anchor_rtp = 0;
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
        false
    };

    if timeline_rebased {
        // A seek/new track restarted the sender's timestamp timeline; frames
        // buffered on the old timeline are stale by definition.
        debug!("RTP timeline rebased by SETRATEANCHORTIME; dropping buffered frames");
        state.buffer.clear();
        state.buffered_f32 = 0;
    }

    let mapped_local_time = (anchor_rtp != 0 && anchor_time_ns != 0)
        .then(|| {
            ptp_clock
                .local_time_for_master_time_at(anchor_time_ns, anchor_clock_id, local_now_ns)
                .or_else(|| {
                    (fallback_clock_id != 0 && fallback_clock_id != anchor_clock_id).then(|| {
                        ptp_clock.local_time_for_master_time_at(anchor_time_ns, fallback_clock_id, local_now_ns)
                    })?
                })
        })
        .flatten();
    let mapping = if let Some(anchor_local_ns) = mapped_local_time {
        state.anchor_local_ns = anchor_local_ns;
        AnchorMapping::Ptp
    } else {
        state.anchor_local_ns = local_now_ns;
        AnchorMapping::LocalFallback
    };

    let stale: Vec<u64> = state
        .buffer
        .keys()
        .filter(|&&ts| ts < state.anchor_rtp)
        .copied()
        .collect();
    let discarded = stale.len();
    for timestamp in stale {
        if let Some(data) = state.buffer.remove(&timestamp) {
            state.buffered_f32 = state.buffered_f32.saturating_sub(data.len());
        }
    }

    (mapping, discarded)
}

fn playout_target_rtp(state: &PlayoutState, local_now_ns: u64) -> Option<u64> {
    if !state.anchor_valid || local_now_ns < state.anchor_local_ns {
        return None;
    }

    let elapsed_ns = local_now_ns - state.anchor_local_ns;
    let elapsed_frames = ((elapsed_ns as u128) * (state.rtp_clock_rate as u128) / 1_000_000_000) as u64;
    state.anchor_rtp.checked_add(elapsed_frames)
}

/// Try to (re)anchor playout from the latest RTCP time announce.
///
/// This is only a fallback: an authoritative `SETRATEANCHORTIME` anchor is
/// never overridden, and NTP announces (no PTP clock mapping) are ignored.
fn refresh_rtcp_anchor(
    state: &mut PlayoutState,
    ptp_clock: &PtpClock,
    announce: &Mutex<Option<TimeAnnounce>>,
    local_now_ns: u64,
) -> bool {
    if state.rate == 0 || state.anchor_source == AnchorSource::SetRate {
        return false;
    }
    let announce = {
        let Ok(guard) = announce.lock() else {
            return false;
        };
        let Some(announce) = *guard else {
            return false;
        };
        announce
    };
    if !announce.is_ptp
        || local_now_ns.saturating_sub(announce.received_at_ns) > TIME_ANNOUNCE_MAX_AGE_NS
    {
        return false;
    }
    let Some(anchor_local_ns) = ptp_clock
        .local_time_for_master_time_at(announce.master_time_ns, announce.clock_id, local_now_ns)
        .or_else(|| ptp_clock.local_time_for_master_time_at(announce.master_time_ns, 0, local_now_ns))
    else {
        return false;
    };

    let max_backward = state
        .rtp_clock_rate
        .saturating_mul(TIMELINE_REBASE_SECONDS);
    state.anchor_rtp = state.timestamps.extend(announce.play_at_rtp, max_backward);
    state.anchor_local_ns = anchor_local_ns;
    state.anchor_valid = true;
    state.anchor_source = AnchorSource::RtcpAnnounce;
    debug!(
        anchor_rtp = state.anchor_rtp,
        clock_id = format!("{:016x}", announce.clock_id),
        "Playout anchored from RTCP time announce"
    );
    true
}

/// Insert decoded PCM while keeping the buffer bounded.
fn push_buffered(state: &mut PlayoutState, timestamp: u64, samples: Vec<f32>) {
    state.buffered_f32 = state.buffered_f32.saturating_add(samples.len());
    state.buffer.insert(timestamp, samples);
    while state.buffered_f32 > state.max_buffered_f32 && state.buffer.len() > 1 {
        let Some(oldest) = state.buffer.keys().next().copied() else {
            break;
        };
        if let Some(data) = state.buffer.remove(&oldest) {
            state.buffered_f32 = state.buffered_f32.saturating_sub(data.len());
        }
    }
}

fn take_ready(state: &mut PlayoutState, target_rtp: u64) -> Vec<(u64, Vec<f32>)> {
    let ready: Vec<u64> = state
        .buffer
        .keys()
        .filter(|&&ts| ts <= target_rtp)
        .copied()
        .collect();
    let mut frames = Vec::with_capacity(ready.len());
    for timestamp in ready {
        if let Some(data) = state.buffer.remove(&timestamp) {
            state.buffered_f32 = state.buffered_f32.saturating_sub(data.len());
            frames.push((timestamp, data));
        }
    }
    frames
}

/// Remove every buffered frame, preserving RTP timestamp order. Used by the
/// sound-first direct-feed mode (no timing gate).
fn take_all(state: &mut PlayoutState) -> Vec<(u64, Vec<f32>)> {
    let keys: Vec<u64> = state.buffer.keys().copied().collect();
    let mut frames = Vec::with_capacity(keys.len());
    for timestamp in keys {
        if let Some(data) = state.buffer.remove(&timestamp) {
            state.buffered_f32 = state.buffered_f32.saturating_sub(data.len());
            frames.push((timestamp, data));
        }
    }
    frames
}

/// TCP listener for buffered audio. Binds a port and spawns the processing pipeline.
pub(crate) struct BufferedAudioProcessor {
    /// TCP listener waiting for the iPhone to connect.
    pub(crate) listener: TcpListener,
    /// Server-wide mapping from the sender's PTP clock to local time.
    pub(crate) ptp_clock: PtpClock,
    /// Input format derived from the type-103 SETUP.
    pub(crate) source_config: BufferedSourceConfig,
}

impl BufferedAudioProcessor {
    /// Start the processing pipeline. Returns a command sender for playback control.
    pub(crate) fn start(
        self,
        shk: [u8; 32],
        output_config: OutputConfig,
        handler: Arc<dyn AudioHandler>,
    ) -> tokio::sync::mpsc::UnboundedSender<PlayoutCommand> {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let default_sr = output_config.sample_rate.unwrap_or(44100);

        // In direct-feed mode start playing immediately (sound-first); RTSP
        // PAUSE/RECORD still flip `rate` afterwards.
        let initial_rate = if self.source_config.direct_feed { 1 } else { 0 };
        let state = Arc::new((
            Mutex::new(PlayoutState {
                buffer: BTreeMap::new(),
                timestamps: TimestampExtender::new(),
                anchor_rtp: 0,
                anchor_local_ns: 0,
                anchor_valid: false,
                anchor_source: AnchorSource::None,
                rate: initial_rate,
                rtp_clock_rate: 44_100,
                sample_rate: default_sr,
                channels: 2,
                source_format: None,
                buffered_f32: 0,
                max_buffered_f32: default_sr as usize * 2 * MAX_BUFFERED_SECONDS,
                stopped: false,
                format_changed: false,
                direct_feed: self.source_config.direct_feed,
                seq_history: VecDeque::new(),
                flushes: 0,
                flushed_frames: 0,
            }),
            Condvar::new(),
        ));

        // Delivery thread
        let state2 = state.clone();
        let handler2 = handler.clone();
        let output_config2 = output_config.clone();
        let ptp_clock2 = self.ptp_clock.clone();
        let announce2 = Arc::clone(&self.source_config.time_announce);
        std::thread::spawn(move || {
            delivery_loop(state2, handler2, output_config2, ptp_clock2, announce2);
        });

        // Command handler
        let state3 = state.clone();
        let handler3 = handler.clone();
        let ptp_clock = self.ptp_clock.clone();
        let announce_clock = Arc::clone(&self.source_config.time_announce);
        let mut cmd_rx = cmd_rx;
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                let (lock, cvar) = &*state3;
                let mut s = lock.lock().unwrap();
                match cmd {
                    PlayoutCommand::SetRate {
                        anchor_rtp,
                        anchor_time_ns,
                        anchor_clock_id,
                        rate,
                    } => {
                        let was_paused = s.rate == 0;
                        if rate == 0 {
                            s.rate = 0;
                            info!("Playout paused");
                        } else {
                            let command_time_ns = now_ns();
                            let fallback_clock_id = announce_clock
                                .lock()
                                .ok()
                                .and_then(|slot| *slot)
                                .filter(|announce| {
                                    announce.is_ptp
                                        && command_time_ns.saturating_sub(announce.received_at_ns)
                                            <= TIME_ANNOUNCE_MAX_AGE_NS
                                })
                                .map(|announce| announce.clock_id)
                                .unwrap_or(0);
                            let (mapping, discarded) = set_rate_anchor(
                                &mut s,
                                &ptp_clock,
                                anchor_rtp,
                                anchor_time_ns,
                                anchor_clock_id,
                                fallback_clock_id,
                                rate,
                                command_time_ns,
                            );
                            if discarded != 0 {
                                debug!(discarded, "Discarded stale frames");
                            }
                            if was_paused {
                                info!(
                                    anchor_rtp,
                                    anchor_local_ns = s.anchor_local_ns,
                                    ptp_synchronized = mapping == AnchorMapping::Ptp,
                                    "Playout started"
                                );
                            }
                        }
                        cvar.notify_all();
                    }
                    PlayoutCommand::Pause => {
                        // A plain RTSP PAUSE does not carry a new RTP anchor.
                        // Preserve the old one and, critically, keep the stream
                        // and decoder alive for the matching RECORD.
                        pause_playout(&mut s);
                        info!("Playout paused");
                        cvar.notify_all();
                    }
                    PlayoutCommand::Resume => {
                        resume_playout(&mut s);
                        info!("Playout resumed");
                        cvar.notify_all();
                    }
                    PlayoutCommand::Flush { from_seq, until_seq } => {
                        // FLUSHBUFFERED carries RTP **sequence numbers**, while the
                        // playout buffer is keyed by RTP timestamps. Map the range
                        // through the packet history; comparing raw values used to
                        // delete whole buffers or nothing at all.
                        let from_seq = from_seq as u16;
                        let until_seq = until_seq as u16;
                        let mapped: Vec<u64> = s
                            .seq_history
                            .iter()
                            .filter(|(seq, _)| seq_in_range(*seq, from_seq, until_seq))
                            .map(|(_, ts)| *ts)
                            .collect();
                        let removed = if let (Some(min), Some(max)) =
                            (mapped.iter().min(), mapped.iter().max())
                        {
                            let keys: Vec<u64> =
                                s.buffer.range(*min..=*max).map(|(ts, _)| *ts).collect();
                            let mut removed = 0usize;
                            for k in keys {
                                if let Some(data) = s.buffer.remove(&k) {
                                    s.buffered_f32 =
                                        s.buffered_f32.saturating_sub(data.len());
                                    removed += 1;
                                }
                            }
                            removed
                        } else {
                            // The range could not be mapped to buffered timestamps:
                            // never interpret sequence numbers as timestamps.
                            0
                        };
                        s.flushes = s.flushes.saturating_add(1);
                        s.flushed_frames = s.flushed_frames.saturating_add(removed as u64);
                        debug!(from_seq, until_seq, removed, mapped = mapped.len(), "Flushed");
                        handler3.on_diagnostic(
                            "info",
                            &format!(
                                "AirPlay FLUSH：seq {from_seq}..{until_seq}，匹配到 {} 个已收包，删除 {removed} 帧缓冲",
                                mapped.len()
                            ),
                        );
                        cvar.notify_all();
                    }
                    PlayoutCommand::Stop => {
                        s.stopped = true;
                        s.buffer.clear();
                        s.buffered_f32 = 0;
                        cvar.notify_all();
                        break;
                    }
                }
            }
        });

        // Receiver task. A sender may close and reopen only the buffered-audio
        // TCP socket when pausing. Keep accepting sequential data connections
        // until the RTSP stream is explicitly stopped.
        let state4 = state.clone();
        let source_config = self.source_config;
        let ptp_clock4 = self.ptp_clock.clone();

        tokio::spawn(async move {
            loop {
                if state4.0.lock().is_ok_and(|state| state.stopped) {
                    break;
                }

                let (stream, addr) = match self.listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Buffered audio accept failed: {e}");
                        handler.on_error(&ShairplayError::Network(NetworkError::Io(e)));
                        return;
                    }
                };
                if state4.0.lock().is_ok_and(|state| state.stopped) {
                    break;
                }

                info!(%addr, "Buffered audio client connected");
                receive_loop(
                    stream,
                    &shk,
                    output_config.clone(),
                    &source_config,
                    &ptp_clock4,
                    state4.clone(),
                    &handler,
                )
                .await;

                if state4.0.lock().is_ok_and(|state| state.stopped) {
                    break;
                }
                info!("Buffered audio client disconnected; waiting for stream resume");
            }
        });

        cmd_tx
    }
}

/// TCP receive loop: reads length-prefixed packets, decrypts, decodes, buffers.
///
/// Generic over the reader so the framing/decode/SSRC logic can be unit-tested
/// with an in-memory duplex stream instead of a real socket.
async fn receive_loop<R: tokio::io::AsyncRead + Unpin>(
    mut stream: R,
    shk: &[u8; 32],
    output_config: OutputConfig,
    source_config: &BufferedSourceConfig,
    ptp_clock: &PtpClock,
    state: Arc<(Mutex<PlayoutState>, Condvar)>,
    handler: &Arc<dyn AudioHandler>,
) {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit};

    let cipher = ChaCha20Poly1305::new(shk.into());
    let mut len_buf = [0u8; 2];
    let mut decoder: Option<BufferedDecoder> = None;
    let mut current_ssrc = AudioSsrc::None;
    let mut stream_resampler: Option<crate::codec::resample::StreamResampler> = None;
    let mut source_channels: u8 = 2;
    let mut output_channels: u8 = 2;
    let mut unknown_ssrc_warn = RateLimitedWarn::new(Duration::from_secs(5));
    // Diagnostic counters: exported through `on_diagnostic` so the next device
    // log shows whether PCM is missing because of decrypt, decode or framing.
    let mut packets: u64 = 0;
    let mut decrypt_failures: u64 = 0;
    let mut decode_failures: u64 = 0;
    let mut silent_decodes: u64 = 0;
    let mut pushed_frames: u64 = 0;
    let mut pushed_samples: u64 = 0;
    let mut first_decrypt_warn = RateLimitedWarn::new(Duration::from_secs(60));
    let mut first_decode_warn = RateLimitedWarn::new(Duration::from_secs(60));
    let mut first_silent_warn = RateLimitedWarn::new(Duration::from_secs(60));
    let mut last_diagnostic = now_ns();

    loop {
        if state.0.lock().is_ok_and(|state| state.stopped) {
            break;
        }
        if stream.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let total_len = u16::from_be_bytes(len_buf) as usize;
        if total_len < 2 {
            break;
        }

        let mut packet = vec![0u8; total_len - 2];
        if stream.read_exact(&mut packet).await.is_err() {
            break;
        }
        if packet.len() <= RTP_HEADER_LEN + NONCE_TRAIL_LEN {
            continue;
        }

        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        let timestamp = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let ssrc_val = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let ssrc = AudioSsrc::from_u32(ssrc_val);

        // AirPlay 2 / OS 27 sends SSRC_NONE control packets (ACK, flush) and
        // possibly packets of an unrecognized audio format on the same channel.
        // They must not be decoded, must not steal the current format, and must
        // not be fed to the previous decoder (which produced garbage audio).
        if ssrc == AudioSsrc::None {
            if ssrc_val == 0 {
                debug!(
                    timestamp,
                    "Skipping SSRC_NONE buffered-audio control packet"
                );
            } else if unknown_ssrc_warn.allow(now_ns()) {
                warn!(
                    ssrc = format!("{ssrc_val:#010x}"),
                    "Skipping buffered-audio packet with unrecognized SSRC"
                );
            }
            continue;
        }

        // Detect format change on the first *valid* audio packet. This is what
        // anchors playback when the sender's first blocks carry an unknown SSRC.
        if ssrc != current_ssrc {
            let (Some(src_sr), Some(src_ch)) = (ssrc.sample_rate(), ssrc.channels()) else {
                continue;
            };
            current_ssrc = ssrc;
            info!(ssrc = ?ssrc, src_sr, src_ch, "Audio format detected");

            match BufferedDecoder::for_ssrc(ssrc, source_config) {
                Ok(created) => {
                    info!(codec = ?created.source_codec(), "Buffered audio decoder initialized");
                    decoder = Some(created);
                }
                Err(error) => {
                    warn!(%error, "Failed to initialize buffered audio decoder");
                    handler.on_error(&ShairplayError::Codec(CodecError::UnsupportedFormat(error)));
                    decoder = None;
                }
            }

            let target_sr = output_config.sample_rate.unwrap_or(src_sr);
            let target_ch = output_config.max_channels.map(|max| src_ch.min(max)).unwrap_or(src_ch);

            stream_resampler = crate::codec::resample::StreamResampler::new(src_sr, target_sr, target_ch as usize);
            if stream_resampler.is_some() {
                debug!(from = src_sr, to = target_sr, "Resampler initialized");
            }

            source_channels = src_ch;
            output_channels = target_ch;

            // Signal format change to delivery thread
            let (lock, cvar) = &*state;
            let mut s = lock.lock().unwrap();
            let source_format = SourceAudioFormat {
                codec: if ssrc.is_alac() {
                    SourceAudioCodec::Alac
                } else {
                    SourceAudioCodec::Aac
                },
                bits: ssrc.bit_depth(),
                channels: Some(src_ch),
                sample_rate: Some(src_sr),
            };
            let format_changed =
                s.sample_rate != target_sr || s.channels != target_ch || s.source_format != Some(source_format);
            // RTP timestamps advance in the source clock domain even when PCM
            // is resampled to a different output rate.
            s.rtp_clock_rate = src_sr;
            s.sample_rate = target_sr;
            s.channels = target_ch;
            s.max_buffered_f32 = target_sr as usize * target_ch as usize * MAX_BUFFERED_SECONDS;
            s.source_format = Some(source_format);
            s.format_changed |= format_changed;
            cvar.notify_all();
        }

        let Some(decoder) = decoder.as_mut() else {
            // No decoder for this format; keep draining so ACK/flush control
            // packets are not misparsed as audio.
            continue;
        };

        packets = packets.saturating_add(1);
        // Decrypt the ChaCha20-Poly1305 RTP frame.
        let Some(plaintext) = decrypt_rtp_chacha(&cipher, &packet) else {
            debug!("Audio decrypt failed");
            decrypt_failures = decrypt_failures.saturating_add(1);
            if first_decrypt_warn.allow(now_ns()) {
                handler.on_diagnostic(
                    "warning",
                    &format!(
                        "AirPlay 缓冲音频解密失败（累计 {decrypt_failures} 包，最近包长 {} 字节）；请检查 SETUP 会话密钥/分包",
                        packet.len()
                    ),
                );
            }
            continue;
        };

        let Some(samples) = decoder.decode(&plaintext) else {
            debug!("Audio decode failed");
            decode_failures = decode_failures.saturating_add(1);
            if first_decode_warn.allow(now_ns()) {
                handler.on_diagnostic(
                    "warning",
                    &format!(
                        "AirPlay 缓冲音频解码失败（累计 {decode_failures} 包，SSRC={:?}，明文 {} 字节）",
                        current_ssrc,
                        plaintext.len()
                    ),
                );
            }
            continue;
        };

        if samples.iter().all(|sample| *sample == 0.0) && plaintext.iter().any(|byte| *byte != 0) {
            silent_decodes = silent_decodes.saturating_add(1);
            if first_silent_warn.allow(now_ns()) {
                handler.on_diagnostic(
                    "warning",
                    &format!(
                        "AirPlay 缓冲音频解码结果为静音（SSRC={:?}，明文 {} 字节）；疑似帧格式/解码器不匹配",
                        current_ssrc,
                        plaintext.len()
                    ),
                );
            }
        }

        if now_ns().saturating_sub(last_diagnostic) >= 10_000_000_000 {
            last_diagnostic = now_ns();
            handler.on_diagnostic(
                "info",
                &format!(
                    "AirPlay 缓冲音频诊断：收包 {packets}、解码 {pushed_frames} 帧 / {pushed_samples} 采样、解密失败 {decrypt_failures}、解码失败 {decode_failures}、解码静音 {silent_decodes}、首帧 {} 个采样",
                    samples.len()
                ),
            );
        }

        // Mix down + resample to the output format.
        let samples = crate::codec::resample::mixdown_and_resample(
            samples,
            source_channels,
            output_channels,
            &mut stream_resampler,
        );

        let (lock, cvar) = &*state;
        let mut s = lock.lock().unwrap();
        let max_backward = s.rtp_clock_rate.saturating_mul(TIMELINE_REBASE_SECONDS);
        let (timestamp_ext, timeline_rebased) = s.timestamps.extend_tracking(timestamp, max_backward);
        if timeline_rebased {
            debug!(
                timestamp,
                "Buffered audio RTP timeline restarted; dropping stale frames"
            );
            s.buffer.clear();
            s.buffered_f32 = 0;
            s.anchor_valid = false;
            s.anchor_source = AnchorSource::None;
        }
        pushed_frames = pushed_frames.saturating_add(1);
        pushed_samples = pushed_samples.saturating_add(samples.len() as u64);
        s.seq_history.push_back((seq, timestamp_ext));
        while s.seq_history.len() > MAX_SEQ_HISTORY {
            s.seq_history.pop_front();
        }
        push_buffered(&mut s, timestamp_ext, samples);
        anchor_first_resumed_packet(&mut s, timestamp_ext);
        refresh_rtcp_anchor(&mut s, ptp_clock, &source_config.time_announce, now_ns());
        cvar.notify_all();
    }
    debug!("Buffered audio receive loop ended");
}

/// Timed playout delivery thread. Wakes on condvar, delivers due frames to AudioSession.
fn delivery_loop(
    state: Arc<(Mutex<PlayoutState>, Condvar)>,
    handler: Arc<dyn AudioHandler>,
    _output_config: OutputConfig,
    ptp_clock: PtpClock,
    time_announce: Arc<Mutex<Option<TimeAnnounce>>>,
) {
    let (lock, cvar) = &*state;
    let mut session: Option<Box<dyn crate::raop::AudioSession>> = None;
    let mut delivered_samples: u64 = 0;
    let mut delivered_frames: u64 = 0;
    let mut last_diagnostic = now_ns();

    loop {
        let mut s = lock.lock().unwrap();
        if s.stopped {
            break;
        }

        // Lazy init or reinit session on format change. In direct-feed mode the
        // first decoded packet may arrive before any anchor exists.
        if (session.is_none() || s.format_changed) && s.source_format.is_some() {
            s.format_changed = false;
            let format = AudioFormat {
                codec: AudioCodec::Pcm,
                bits: 32,
                channels: s.channels,
                sample_rate: s.sample_rate,
                source: s.source_format,
            };
            info!(?format, "Audio session initialized");
            // Emit the previous session's stop before the replacement session
            // announces its start. Reconnected streams with the same format do
            // not take this path and therefore retain their session.
            drop(session.take());
            session = Some(handler.audio_init(format));
        }

        let ready: Vec<(u64, Vec<f32>)> = if s.direct_feed {
            if s.rate == 0 {
                // Paused: drop buffered audio instead of playing it later.
                let dropped = take_all(&mut s).len();
                if dropped != 0 {
                    debug!(dropped, "Direct feed dropped frames while paused");
                }
                let (guard, _) = cvar
                    .wait_timeout(s, Duration::from_millis(20))
                    .expect("playout state mutex");
                s = guard;
                continue;
            }
            if s.buffer.is_empty() || session.is_none() {
                let (guard, _) = cvar
                    .wait_timeout(s, Duration::from_millis(10))
                    .expect("playout state mutex");
                s = guard;
                continue;
            }
            take_all(&mut s)
        } else {
            if s.rate == 0 || !s.anchor_valid || s.buffer.is_empty() {
                let (guard, _) = cvar
                    .wait_timeout(s, Duration::from_millis(20))
                    .expect("playout state mutex");
                s = guard;
                if !s.stopped && s.rate != 0 && !s.anchor_valid {
                    refresh_rtcp_anchor(&mut s, &ptp_clock, &time_announce, now_ns());
                }
                continue;
            }
            let target_rtp = playout_target_rtp(&s, now_ns());
            target_rtp
                .map(|target_rtp| take_ready(&mut s, target_rtp))
                .unwrap_or_default()
        };
        let buffered_frames = s.buffered_f32 / usize::from(s.channels.max(1));
        let rate = s.rate;
        let flushes = s.flushes;
        let flushed_frames = s.flushed_frames;
        let direct_feed = s.direct_feed;
        drop(s);

        if let Some(ref mut sess) = session {
            for (_, frame) in &ready {
                delivered_samples = delivered_samples.saturating_add(frame.len() as u64);
                delivered_frames = delivered_frames.saturating_add(1);
                sess.audio_process(frame);
            }
        }

        // Rate-limited playout diagnostics: the next device log shows whether
        // frames actually leave this loop (and how many flushes interfered).
        if now_ns().saturating_sub(last_diagnostic) >= 5_000_000_000 {
            last_diagnostic = now_ns();
            handler.on_diagnostic(
                "info",
                &format!(
                    "AirPlay 播放诊断：模式={}，已送出 {} 帧 / {} 采样，待播 {} 帧，rate={}，flush {} 次（共删 {} 帧）",
                    if direct_feed { "direct" } else { "timed" },
                    delivered_frames,
                    delivered_samples,
                    buffered_frames,
                    rate,
                    flushes,
                    flushed_frames
                ),
            );
        }

        if ready.is_empty() && !direct_feed {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    info!("Delivery loop ended");
}

#[cfg(test)]
mod tests {
    use super::{
        AnchorMapping, AnchorSource, BufferedDecoder, BufferedSourceConfig, DEFAULT_ALAC_FRAME_LENGTH, OutputConfig,
        PlayoutState, TimestampExtender, anchor_first_resumed_packet, pause_playout, playout_target_rtp,
        receive_loop, refresh_rtcp_anchor, resume_playout, set_rate_anchor,
    };
    use crate::codec::aac::AudioSsrc;
    use crate::net::ptp::PtpClock;
    use crate::raop::rtcp::TimeAnnounce;
    use crate::raop::{AudioFormat, AudioHandler, AudioSession, SourceAudioCodec};
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Condvar, Mutex};
    use tokio::io::AsyncWriteExt;

    fn playout_state() -> PlayoutState {
        PlayoutState {
            buffer: BTreeMap::new(),
            timestamps: TimestampExtender::new(),
            anchor_rtp: 123_456,
            anchor_local_ns: 42,
            anchor_valid: true,
            anchor_source: AnchorSource::SetRate,
            rate: 1,
            rtp_clock_rate: 48_000,
            sample_rate: 48_000,
            channels: 2,
            source_format: None,
            buffered_f32: 0,
            max_buffered_f32: 48_000 * 2 * 20,
            stopped: false,
            format_changed: false,
            direct_feed: false,
            seq_history: VecDeque::new(),
            flushes: 0,
            flushed_frames: 0,
        }
    }

    #[test]
    fn default_source_config_uses_sound_first_direct_feed() {
        assert!(
            BufferedSourceConfig::default().direct_feed,
            "direct feed must be the default while timed playout starves on device"
        );
    }

    #[test]
    fn sequence_numbers_map_into_flush_ranges_with_wraparound() {
        use super::seq_in_range;

        assert!(seq_in_range(5, 3, 7));
        assert!(!seq_in_range(8, 3, 7));
        // Range wrapping past 65535.
        assert!(seq_in_range(65_535, 65_534, 1));
        assert!(seq_in_range(0, 65_534, 1));
        assert!(!seq_in_range(2, 65_534, 1));
    }

    #[test]
    fn take_all_returns_frames_in_timestamp_order_and_empties_the_buffer() {
        use super::take_all;

        let mut state = playout_state();
        state.buffer.insert(300, vec![3.0]);
        state.buffer.insert(100, vec![1.0]);
        state.buffer.insert(200, vec![2.0]);
        state.buffered_f32 = 3;

        let frames = take_all(&mut state);

        assert_eq!(
            frames.iter().map(|(ts, _)| *ts).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
        assert!(state.buffer.is_empty());
        assert_eq!(state.buffered_f32, 0);
    }

    #[test]
    fn pause_preserves_anchor_and_buffer_for_resume() {
        let mut state = playout_state();
        state.buffer.insert(130_000, vec![0.25, -0.25]);

        pause_playout(&mut state);

        assert_eq!(state.rate, 0);
        assert_eq!(state.anchor_rtp, 123_456);
        assert!(state.anchor_valid);
        assert_eq!(state.buffer.len(), 1);
    }

    #[test]
    fn record_resume_waits_for_and_anchors_the_next_packet() {
        let mut state = playout_state();
        pause_playout(&mut state);
        resume_playout(&mut state);

        assert_eq!(state.rate, 1);
        assert!(!state.anchor_valid);

        anchor_first_resumed_packet(&mut state, 240_000);

        assert!(state.anchor_valid);
        assert_eq!(state.anchor_rtp, 235_200);
        assert_eq!(state.anchor_source, AnchorSource::Resume);
    }

    #[test]
    fn ptp_anchor_maps_master_time_to_local_time() {
        let mut state = playout_state();
        let clock = PtpClock::new();
        let local_now = 10_000_000_000;
        let local_to_master_offset = 90_000_000_000;
        clock.update(0x1122, local_now, local_to_master_offset, local_now);

        let (mapping, discarded) = set_rate_anchor(&mut state, &clock, 240_000, 100_500_000_000, 0x1122, 0, 1, local_now);

        assert_eq!(mapping, AnchorMapping::Ptp);
        assert_eq!(discarded, 0);
        assert_eq!(state.anchor_local_ns, 10_500_000_000);
        assert_eq!(state.anchor_source, AnchorSource::SetRate);
    }

    #[test]
    fn future_ptp_anchor_is_not_due_until_local_anchor_time() {
        let mut state = playout_state();
        state.anchor_rtp = 240_000;
        state.anchor_local_ns = 10_500_000_000;
        state.anchor_valid = true;

        assert_eq!(playout_target_rtp(&state, 10_499_999_999), None);
        assert_eq!(playout_target_rtp(&state, 10_500_000_000), Some(240_000));
        assert_eq!(playout_target_rtp(&state, 11_500_000_000), Some(288_000));
    }

    #[test]
    fn missing_ptp_clock_keeps_local_fallback() {
        let mut state = playout_state();
        let local_now = 10_000_000_000;

        let (mapping, _) = set_rate_anchor(
            &mut state,
            &PtpClock::new(),
            240_000,
            100_500_000_000,
            0x1122,
            0,
            1,
            local_now,
        );

        assert_eq!(mapping, AnchorMapping::LocalFallback);
        assert_eq!(state.anchor_local_ns, local_now);
        assert_eq!(playout_target_rtp(&state, local_now), Some(240_000));
    }

    #[test]
    fn playout_uses_source_rtp_clock_rate_after_resampling() {
        let mut state = playout_state();
        state.sample_rate = 96_000;
        state.rtp_clock_rate = 48_000;
        state.anchor_rtp = 240_000;
        state.anchor_local_ns = 10_000_000_000;

        assert_eq!(playout_target_rtp(&state, 11_000_000_000), Some(288_000));
    }

    // --- 64-bit wrap-aware timestamps ---

    #[test]
    fn timestamp_extender_handles_wraparound() {
        let mut extender = TimestampExtender::new();
        assert_eq!(extender.extend(0xFFFF_FF00, 1_000_000), 0xFFFF_FF00);
        // 0x100 after the wrap is 0x200 frames later, not a huge backwards jump.
        assert_eq!(extender.extend(0x0000_0100, 1_000_000), 0x1_0000_0100);
        assert_eq!(extender.extend(0x0000_0200, 1_000_000), 0x1_0000_0200);
        // A reordered packet maps back without moving the reference.
        assert_eq!(extender.extend(0xFFFF_FFF0, 1_000_000), 0xFFFF_FFF0);
        assert_eq!(extender.extend(0x0000_0300, 1_000_000), 0x1_0000_0300);
    }

    #[test]
    fn timestamp_extender_handles_multiple_wraps_and_rebases() {
        let mut extender = TimestampExtender::new();
        // Advance in ~1/4-cycle steps so a few calls cross the 2^32 boundary
        // twice; each step is a plausible forward delta (reordering-safe).
        let steps = [
            0u32, 0x4000_0000, 0x8000_0000, 0xC000_0000, 0x0000_0000, 0x4000_0000, 0x8000_0000,
            0xC000_0000, 0x0000_0000,
        ];
        assert_eq!(extender.extend(steps[0], 1_000_000), 0);
        let mut expected = 0u64;
        for raw in &steps[1..] {
            expected += 0x4000_0000;
            assert_eq!(extender.extend(*raw, 1_000_000), expected);
        }
        assert!(expected > u64::from(u32::MAX) * 2, "crossed two full wraps");

        // A seek far backwards restarts the timeline instead of mapping ahead.
        let (extended, rebased) = extender.extend_tracking(0x8000_0000, 100_000);
        assert!(rebased);
        assert_eq!(extended, 0x8000_0000);
    }

    #[test]
    fn timestamps_beyond_2_pow_31_keep_playing() {
        // ~13.5 h at 44.1 kHz (the old signed-i32 math broke here).
        let mut state = playout_state();
        state.rtp_clock_rate = 44_100;
        state.anchor_rtp = 0x8000_0000;
        state.anchor_local_ns = 10_000_000_000;
        state.anchor_valid = true;
        state
            .buffer
            .insert(0x8000_1000, vec![0.0; 8]);
        state
            .buffer
            .insert(0x8000_0F00, vec![0.0; 8]);
        state.buffered_f32 = 16;

        // 100 ms later: the playout target is past both frames.
        let target = playout_target_rtp(&state, 10_100_000_000).expect("target");
        assert_eq!(target, 0x8000_0000 + 4_410);
        let ready = super::take_ready(&mut state, target);
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].0, 0x8000_0F00);
        assert_eq!(ready[1].0, 0x8000_1000);
    }

    #[test]
    fn set_rate_anchor_across_wrap_discards_old_cycle_frames() {
        let mut state = playout_state();
        state.rtp_clock_rate = 44_100;
        // Packets arrived just before the wrap, so the 64-bit timeline knows
        // which cycle the next anchor belongs to.
        let first = state.timestamps.extend(0xFFFF_FF00, 1_000_000);
        state.buffer.insert(first, vec![0.0; 4]);
        state.buffered_f32 = 4;

        // A SETRATEANCHORTIME anchor just after the wrap (raw 1000 is 0x4e8
        // frames after 0xffff_ff00 on the 64-bit timeline).
        let (_, discarded) = set_rate_anchor(&mut state, &PtpClock::new(), 1_000, 0, 0, 0, 1, 42);
        assert_eq!(discarded, 1);
        assert!(state.buffer.is_empty());
        assert_eq!(state.anchor_rtp, 0x1_0000_03e8);
    }

    // --- RTCP time-announce fallback ---

    #[test]
    fn set_rate_anchor_falls_back_to_the_rtcp_clock_id() {
        let clock = PtpClock::new();
        let local_now = 10_000_000_000;
        clock.update(0x1122, local_now, 90_000_000_000, local_now);

        // SETRATEANCHORTIME can carry a network-timeline id that is not the PTP
        // sink's master id. The RTCP time announce advertises the actual PTP
        // clock id, so the anchor can still be mapped onto master time.
        let mut state = playout_state();
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
        state.rate = 0;
        let (mapping, _) =
            set_rate_anchor(&mut state, &clock, 240_000, 100_500_000_000, 0x9999, 0x1122, 1, local_now);
        assert_eq!(mapping, AnchorMapping::Ptp);
        assert_eq!(state.anchor_local_ns, 10_500_000_000);

        // Without the RTCP-derived clock id the mapping stays on the local fallback.
        let mut state = playout_state();
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
        state.rate = 0;
        let (mapping, _) =
            set_rate_anchor(&mut state, &clock, 240_000, 100_500_000_000, 0x9999, 0, 1, local_now);
        assert_eq!(mapping, AnchorMapping::LocalFallback);
    }

    #[test]
    fn rtcp_announce_anchors_playout_via_ptp_clock() {
        let mut state = playout_state();
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
        state.rate = 1;

        let clock = PtpClock::new();
        let local_now = 10_000_000_000;
        clock.update(0x0102_0304_0506_0708, local_now, 90_000_000_000, local_now);
        let announce = Mutex::new(Some(TimeAnnounce {
            play_at_rtp: 500_000,
            master_time_ns: 100_500_000_000,
            clock_id: 0x0102_0304_0506_0708,
            received_at_ns: local_now,
            is_ptp: true,
        }));

        assert!(refresh_rtcp_anchor(&mut state, &clock, &announce, local_now));
        assert!(state.anchor_valid);
        assert_eq!(state.anchor_source, AnchorSource::RtcpAnnounce);
        assert_eq!(state.anchor_rtp, 500_000);
        assert_eq!(state.anchor_local_ns, 10_500_000_000);
    }

    #[test]
    fn rtcp_announce_does_not_override_set_rate_anchor_or_work_without_ptp() {
        // A SETRATE anchor is authoritative.
        let mut state = playout_state();
        state.anchor_source = AnchorSource::SetRate;
        state.anchor_valid = true;
        let clock = PtpClock::new();
        let local_now = 10_000_000_000;
        clock.update(0x1122, local_now, 90_000_000_000, local_now);
        let announce = Mutex::new(Some(TimeAnnounce {
            play_at_rtp: 500_000,
            master_time_ns: 100_500_000_000,
            clock_id: 0x1122,
            received_at_ns: local_now,
            is_ptp: true,
        }));
        assert!(!refresh_rtcp_anchor(&mut state, &clock, &announce, local_now));
        assert_eq!(state.anchor_rtp, 123_456);

        // Without a synchronised PTP clock the local fallback is retained.
        let mut state = playout_state();
        state.anchor_valid = false;
        state.anchor_source = AnchorSource::None;
        assert!(!refresh_rtcp_anchor(&mut state, &PtpClock::new(), &announce, local_now));
        assert!(!state.anchor_valid);

        // Stale announces are ignored.
        let old = Mutex::new(Some(TimeAnnounce {
            play_at_rtp: 500_000,
            master_time_ns: 100_500_000_000,
            clock_id: 0x1122,
            received_at_ns: 0,
            is_ptp: true,
        }));
        let mut state = playout_state();
        assert!(!refresh_rtcp_anchor(&mut state, &clock, &old, local_now));
    }

    // --- Decoder selection ---

    #[test]
    fn selects_alac_decoder_for_alac_ssrcs() {
        for (ssrc, sample_rate, bit_depth) in [
            (AudioSsrc::Alac44100S16Stereo, 44_100u32, 16u8),
            (AudioSsrc::Alac48000S24Stereo, 48_000u32, 24u8),
        ] {
            let config = BufferedSourceConfig {
                samples_per_frame: Some(352),
                ..Default::default()
            };
            let decoder = BufferedDecoder::for_ssrc(ssrc, &config).expect("ALAC decoder");
            assert_eq!(decoder.source_codec(), SourceAudioCodec::Alac);
            match decoder {
                BufferedDecoder::Alac(decoder) => {
                    assert_eq!(decoder.sample_rate(), sample_rate);
                    assert_eq!(decoder.sample_size_config(), bit_depth);
                    assert_eq!(decoder.max_samples_per_frame(), 352);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn alac_frame_length_defaults_and_uses_spf() {
        let ssrc = AudioSsrc::Alac44100S16Stereo;
        let without_spf = BufferedDecoder::for_ssrc(ssrc, &BufferedSourceConfig::default()).unwrap();
        match without_spf {
            BufferedDecoder::Alac(decoder) => {
                assert_eq!(decoder.max_samples_per_frame(), DEFAULT_ALAC_FRAME_LENGTH)
            }
            _ => unreachable!(),
        }

        let with_spf = BufferedSourceConfig {
            samples_per_frame: Some(1024),
            ..Default::default()
        };
        match BufferedDecoder::for_ssrc(ssrc, &with_spf).unwrap() {
            BufferedDecoder::Alac(decoder) => assert_eq!(decoder.max_samples_per_frame(), 1024),
            _ => unreachable!(),
        }
    }

    #[test]
    fn selects_aac_decoder_for_aac_ssrcs_and_rejects_unknown() {
        let aac = BufferedSourceConfig::default();
        assert_eq!(
            BufferedDecoder::for_ssrc(AudioSsrc::Aac44100F24Stereo, &aac)
                .unwrap()
                .source_codec(),
            SourceAudioCodec::Aac
        );
        assert!(BufferedDecoder::for_ssrc(AudioSsrc::None, &aac).is_err());
    }

    // --- Buffered ALAC decode through the real receive loop ---

    struct NoopSession;

    impl AudioSession for NoopSession {
        fn audio_process(&mut self, _samples: &[f32]) {}
    }

    struct NoopHandler;

    impl AudioHandler for NoopHandler {
        fn audio_init(&self, _format: AudioFormat) -> Box<dyn AudioSession> {
            Box::new(NoopSession)
        }
    }

    /// Minimal MSB-first bit writer for building uncompressed ALAC frames.
    struct BitWriter {
        bytes: Vec<u8>,
        nbits: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                nbits: 0,
            }
        }
        fn put(&mut self, val: u32, bits: u32) {
            for i in (0..bits).rev() {
                if self.nbits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if (val >> i) & 1 == 1 {
                    let last = self.bytes.len() - 1;
                    self.bytes[last] |= 1 << (7 - (self.nbits % 8));
                }
                self.nbits += 1;
            }
        }
    }

    /// Uncompressed stereo ALAC frame, `frames` interleaved (left, right) samples.
    fn alac_stereo_frame_16(frames: &[(u16, u16)]) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(1, 3); // channels: stereo
        w.put(0, 4);
        w.put(0, 12);
        w.put(0, 1); // has_size = 0 → max frame samples
        w.put(0, 2); // uncompressed_bytes
        w.put(1, 1); // is_not_compressed
        for &(left, right) in frames {
            w.put(left as u32, 16);
            w.put(right as u32, 16);
        }
        w.bytes
    }

    /// Uncompressed stereo 24-bit ALAC frame, `frames` interleaved samples.
    fn alac_stereo_frame_24(frames: &[(i32, i32)]) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(1, 3);
        w.put(0, 4);
        w.put(0, 12);
        w.put(0, 1);
        w.put(0, 2);
        w.put(1, 1);
        for &(left, right) in frames {
            w.put((left as u32) & 0x00ff_ffff, 24);
            w.put((right as u32) & 0x00ff_ffff, 24);
        }
        w.bytes
    }

    /// Encrypt one audio payload into the length-prefixed buffered-audio wire
    /// format: `u16 length` + RTP header + ChaCha20-Poly1305 ciphertext + nonce.
    fn audio_packet(shk: &[u8; 32], timestamp: u32, ssrc: u32, plaintext: &[u8]) -> Vec<u8> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead, aead::Payload};

        let cipher = ChaCha20Poly1305::new(shk.into());
        // RTP: byte0/1 flags, bytes 2..4 sequence, bytes 4..8 timestamp,
        // bytes 8..12 SSRC (the AEAD authenticates timestamp+SSRC as AAD).
        let header = [
            0x80,
            0x60,
            0,
            1,
            (timestamp >> 24) as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (ssrc >> 24) as u8,
            (ssrc >> 16) as u8,
            (ssrc >> 8) as u8,
            ssrc as u8,
        ];
        let nonce_tail = [0x42u8; 8];
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&nonce_tail);
        let ciphertext = cipher
            .encrypt(
                chacha20poly1305::Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &header[4..12],
                },
            )
            .unwrap();

        let mut packet = header.to_vec();
        packet.extend_from_slice(&ciphertext);
        packet.extend_from_slice(&nonce_tail);

        let mut framed = ((packet.len() + 2) as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&packet);
        framed
    }

    async fn run_receive_loop(
        packets: Vec<Vec<u8>>,
        config: BufferedSourceConfig,
        output_config: OutputConfig,
        state: Arc<(Mutex<PlayoutState>, Condvar)>,
    ) {
        let shk = [7u8; 32];
        let (mut writer, reader) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            for packet in packets {
                if writer.write_all(&packet).await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        let handler: Arc<dyn AudioHandler> = Arc::new(NoopHandler);
        receive_loop(
            reader,
            &shk,
            output_config,
            &config,
            &PtpClock::new(),
            state,
            &handler,
        )
        .await;
    }

    fn test_state(rate: u32) -> Arc<(Mutex<PlayoutState>, Condvar)> {
        let mut s = playout_state();
        s.rate = rate;
        s.anchor_valid = rate != 0;
        s.anchor_source = if rate != 0 {
            AnchorSource::SetRate
        } else {
            AnchorSource::None
        };
        s.buffer.clear();
        s.buffered_f32 = 0;
        Arc::new((Mutex::new(s), Condvar::new()))
    }

    #[tokio::test]
    async fn decodes_buffered_alac_44k_16bit_stereo() {
        let shk = [7u8; 32];
        let frames = [(0x0102u16, 0x1111u16), (0x7FFFu16, 0x8000u16), (0u16, 0xFFFFu16), (0x1234, 0x5678)];
        let payload = alac_stereo_frame_16(&frames);
        let packet = audio_packet(&shk, 1_000, AudioSsrc::Alac44100S16Stereo as u32, &payload);
        let config = BufferedSourceConfig {
            samples_per_frame: Some(4),
            ..Default::default()
        };
        let state = test_state(0);
        run_receive_loop(
            vec![packet],
            config,
            OutputConfig {
                sample_rate: None,
                max_channels: None,
            },
            Arc::clone(&state),
        )
        .await;

        let s = state.0.lock().unwrap();
        assert_eq!(s.buffer.len(), 1, "one decoded ALAC packet buffered");
        let samples = s.buffer.values().next().unwrap();
        assert_eq!(samples.len(), frames.len() * 2);
        let expected_left = frames[0].0 as i16 as f32 / 32768.0;
        let expected_right = frames[0].1 as i16 as f32 / 32768.0;
        assert!((samples[0] - expected_left).abs() < f32::EPSILON);
        assert!((samples[1] - expected_right).abs() < f32::EPSILON);
        let format = s.source_format.expect("source format");
        assert_eq!(format.codec, SourceAudioCodec::Alac);
        assert_eq!(format.bits, Some(16));
        assert_eq!(format.sample_rate, Some(44_100));
        assert_eq!(format.channels, Some(2));
    }

    #[tokio::test]
    async fn decodes_buffered_alac_48k_24bit_stereo() {
        let shk = [7u8; 32];
        let frames = [(0i32, 0x7f_ffffi32), (-0x80_0000, -1)];
        let payload = alac_stereo_frame_24(&frames);
        let packet = audio_packet(&shk, 5_000, AudioSsrc::Alac48000S24Stereo as u32, &payload);
        let config = BufferedSourceConfig {
            alac_format: Some(crate::codec::alac::AlacFormat {
                sample_rate: 48_000,
                bit_depth: 24,
                channels: 2,
            }),
            samples_per_frame: Some(2),
            ..Default::default()
        };
        let state = test_state(0);
        run_receive_loop(
            vec![packet],
            config,
            OutputConfig {
                sample_rate: None,
                max_channels: None,
            },
            Arc::clone(&state),
        )
        .await;

        let s = state.0.lock().unwrap();
        let samples = s.buffer.values().next().expect("decoded frame");
        assert_eq!(samples.len(), 4);
        assert!((samples[0] - 0.0).abs() < f32::EPSILON);
        assert!((samples[1] - (0x7f_ffff as f32 / 8_388_608.0)).abs() < f32::EPSILON);
        assert!((samples[2] - (-0x80_0000 as f32 / 8_388_608.0)).abs() < f32::EPSILON);
        let format = s.source_format.expect("source format");
        assert_eq!(format.codec, SourceAudioCodec::Alac);
        assert_eq!(format.bits, Some(24));
        assert_eq!(format.sample_rate, Some(48_000));
    }

    /// Parse ADTS frames from the bundled AAC-LC 44.1 kHz stereo test asset and
    /// return the raw AAC payloads (header stripped) that AirPlay carries.
    fn aac_test_payloads() -> Vec<Vec<u8>> {
        let data: &[u8] = include_bytes!("../../tests/data/aac/tone_44100_stereo.aac");
        let mut frames = Vec::new();
        let mut off = 0;
        while off + 7 <= data.len() {
            if data[off] != 0xFF || (data[off + 1] & 0xF6) != 0xF0 {
                off += 1;
                continue;
            }
            let len = (((data[off + 3] & 0x03) as usize) << 11)
                | ((data[off + 4] as usize) << 3)
                | ((data[off + 5] as usize) >> 5);
            if len < 7 || off + len > data.len() {
                break;
            }
            frames.push(data[off + 7..off + len].to_vec());
            off += len;
        }
        // Skip the leading ffmpeg metadata frame; use real audio frames.
        frames.into_iter().skip(1).collect()
    }

    #[tokio::test]
    async fn decodes_real_aac_lc_frames_from_the_buffered_audio_path() {
        let shk = [7u8; 32];
        let payloads = aac_test_payloads();
        assert!(payloads.len() > 4, "test asset must carry audio frames");
        let mut packets = Vec::new();
        for (index, payload) in payloads.iter().enumerate().take(12) {
            let timestamp = 1_000 + (index as u32) * 1024;
            packets.push(audio_packet(
                &shk,
                timestamp,
                AudioSsrc::Aac44100F24Stereo as u32,
                payload,
            ));
        }
        let state = test_state(0);
        run_receive_loop(
            packets,
            BufferedSourceConfig::default(),
            OutputConfig {
                sample_rate: Some(48_000),
                max_channels: Some(2),
            },
            Arc::clone(&state),
        )
        .await;

        let s = state.0.lock().unwrap();
        assert_eq!(s.source_format.expect("source format").codec, SourceAudioCodec::Aac);
        assert!(!s.buffer.is_empty(), "AAC packets must decode into the playout buffer");
        let total: usize = s.buffer.values().map(Vec::len).sum();
        assert!(total > 0, "decoded AAC must produce PCM samples");
        let peak = s
            .buffer
            .values()
            .flat_map(|frame| frame.iter())
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.001, "decoded 440 Hz tone must not be silent, peak={peak}");
    }

    #[tokio::test]
    async fn unknown_ssrc_and_ssrc_none_packets_are_skipped_without_feeding_the_decoder() {
        let shk = [7u8; 32];
        let frames = [(0x1111u16, 0x2222u16), (0x3333, 0x4444), (0x5555, 0x6666), (0x7777, 0x8888)];
        let valid_payload = alac_stereo_frame_16(&frames);
        // A first packet with an unrecognized SSRC whose payload *would* decode
        // if it were fed to an ALAC decoder, then an SSRC_NONE control packet,
        // then a valid ALAC packet. Only the valid packet may be buffered.
        let unknown_ssrc_packet = audio_packet(&shk, 900, 0xDEAD_BEEF, &valid_payload);
        let ssrc_none_packet = audio_packet(&shk, 950, 0, &[0u8; 16]);
        let valid_packet = audio_packet(&shk, 1_000, AudioSsrc::Alac44100S16Stereo as u32, &valid_payload);
        let config = BufferedSourceConfig {
            samples_per_frame: Some(4),
            ..Default::default()
        };
        let state = test_state(0);
        run_receive_loop(
            vec![unknown_ssrc_packet, ssrc_none_packet, valid_packet],
            config,
            OutputConfig {
                sample_rate: None,
                max_channels: None,
            },
            Arc::clone(&state),
        )
        .await;

        let s = state.0.lock().unwrap();
        assert_eq!(s.buffer.len(), 1, "only the valid ALAC packet is decoded");
        assert_eq!(s.buffer.keys().next().copied(), Some(1_000));
        assert_eq!(s.source_format.expect("format").codec, SourceAudioCodec::Alac);
    }

    #[tokio::test]
    async fn unknown_ssrc_first_packet_does_not_anchor_playback() {
        let shk = [7u8; 32];
        let unknown = audio_packet(&shk, 500, 0x1234_5678, &[9u8; 32]);
        let config = BufferedSourceConfig::default();
        let state = test_state(1);
        {
            let mut s = state.0.lock().unwrap();
            s.anchor_valid = false;
            s.anchor_source = AnchorSource::None;
        }
        run_receive_loop(
            vec![unknown],
            config,
            OutputConfig {
                sample_rate: None,
                max_channels: None,
            },
            Arc::clone(&state),
        )
        .await;

        let s = state.0.lock().unwrap();
        assert!(s.buffer.is_empty());
        assert!(!s.anchor_valid, "an unknown-SSRC first packet must not anchor playout");
    }
}
