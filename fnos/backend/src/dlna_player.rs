//! DLNA media playback for the fnOS host.
//!
//! `airplay-receiver-core` owns the DLNA **control plane** (SSDP + AVTransport)
//! and emits `CoreEvent::DlnaMedia { url, .. }` when a control point starts a
//! track. This module turns that URI into PCM for the unified engine:
//!
//! ```text
//! http(s):// /file://  --stream-->  symphonia probe/decode  --+-->  downmix to
//!                                                             |      stereo
//!                                                             +-->  resample
//!                                                                   48 kHz f32
//!                                                                    |
//!                                                                    v
//!                                                     AudioEngine::push_pcm(Dlna)
//! ```
//!
//! # Supported matrix (first fnOS release)
//!
//! | Input                           | Status |
//! |---------------------------------|--------|
//! | `http://` + `Content-Length`    | supported, streamed |
//! | `http://` + `Transfer-Encoding: chunked` | supported, streamed |
//! | `http://` connection-close body | supported, streamed |
//! | `http://` 301/302/303/307/308   | followed (max [`MAX_REDIRECTS`]) |
//! | `file://` local file            | supported, seekable |
//! | `https://`                      | **not supported** (no TLS stack bundled) |
//! | HLS (`m3u8`/`m3u`, `*mpegurl`)  | **not supported** (explicit error) |
//! | video tracks / `upnp:class` video | **not supported** (explicit error) |
//! | mp3 / flac / wav / aac / mp4 audio | supported containers |
//!
//! Downloading is fully streaming: the HTTP body is decoded (content-length,
//! chunked, or until-close) as the decoder pulls bytes, so a multi-GB file never
//! lands in RAM. The fixed buffering is bounded by [`STREAM_BUFFER_BYTES`] (the
//! `MediaSourceStream` ring) plus a small constant amount of codec/resampler
//! state — far below the 8 MiB budget enforced by [`MAX_STREAM_MEMORY_BYTES`].
//!
//! # Error handling contract
//!
//! * Empty/illegal URIs, `https://`, HLS and video are rejected **before** any
//!   network or arbiter activity, and the reason is written to
//!   [`SharedState::record_error`] so `/api/status` exposes it.
//! * Timeouts, truncated bodies and non-200 responses produce explicit errors
//!   (`last_error`), never a silent "playing" state.
//! * Corrupt packets are skipped; after [`MAX_CONSECUTIVE_DECODE_ERRORS`]
//!   consecutive failures decoding aborts with an error.
//! * When another source (AirPlay/MiPlay) takes the arbiter lease, decoding
//!   stops promptly (checked in the decode loop and before every socket read;
//!   the read timeout is the backstop for a fully idle server), queued PCM is
//!   flushed when appropriate and the lease is released.
//!
//! # Tests
//!
//! Integration tests live in `tests/dlna_decode.rs`: they synthesize WAV files,
//! serve them over a loopback `TcpListener` (content-length and chunked), and
//! drive a real [`AudioEngine`] with a recording/null backend.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoderOptions, well_known::CODEC_ID_AAC,
};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration as MediaDuration, Timestamp};

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    calculate_cutoff, Async, FixedAsync, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

use crate::audio::AudioEngine;
use crate::session::{Arbiter, Lease, Source};
use crate::state::SharedState;

/// The unified engine's master mix format.
const TARGET_RATE: u32 = 48_000;
const TARGET_CHANNELS: usize = 2;

/// Default connect/read timeout for DLNA HTTP transfers. LAN media servers are
/// expected to answer quickly; a stalled server must fail instead of hanging.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound for the TCP connect phase (kept independent from read timeout so
/// "host unreachable" fails much faster than a stalled body read).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of HTTP redirects followed for one media URI.
const MAX_REDIRECTS: usize = 4;

/// Browser-like User-Agent for DLNA media pulls. Several Chinese music CDNs
/// (notably QQ Music) answer non-browser clients with 403; Chrome's UA is
/// accepted by all of them. Override with `FUSIONPLAY_DLNA_USER_AGENT`.
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) \
                                  AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// HTTP header parsing limits (defensive against broken/malicious servers).
const MAX_HEADER_LINE: usize = 8 * 1024;
const MAX_HEADER_COUNT: usize = 128;

/// Fixed `MediaSourceStream` ring size. This is the only non-trivial download
/// buffer: the body is pulled through it on demand, never accumulated.
const STREAM_BUFFER_BYTES: usize = 256 * 1024;

/// Hard memory budget for the streaming path. Codec state and the resampler
/// working set are small constants; this assertion documents/enforces that the
/// fixed stream buffer stays far below it.
const MAX_STREAM_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const _: () = assert!(STREAM_BUFFER_BYTES + 1024 * 1024 <= MAX_STREAM_MEMORY_BYTES);

/// Consecutive packet decode failures tolerated before aborting the track.
/// Isolated bad packets in otherwise valid streams are skipped.
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 32;

/// How often the shared state is refreshed during playback (UI-friendly).
const STATE_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// Input chunk size (frames) handed to the rubato resampler at a time.
const RESAMPLE_CHUNK_FRAMES: usize = 1024;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Metadata attached to a DLNA media start. Mirrors the fields of
/// `airplay_receiver_core::CoreEvent::DlnaMedia` so the event can be forwarded
/// without losing title/duration information.
#[derive(Debug, Clone, Default)]
pub struct DlnaTrack<'a> {
    pub uri: &'a str,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub artwork_url: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub bitrate_bps: Option<u64>,
    pub sample_rate: Option<u32>,
    pub bits_per_sample: Option<u16>,
    pub channels: Option<u16>,
    pub upnp_class: Option<&'a str>,
    pub media_kind: &'a str,
    pub duration_ms: Option<u64>,
    pub start_position_ms: u64,
}

/// Snapshot of the most recent (or in-flight) DLNA playback. Exposed for
/// diagnostics even though `/api/status` currently surfaces the human-readable
/// `protocols.dlna.detail` text.
#[derive(Debug, Clone, Default)]
pub struct DlnaStats {
    pub uri: String,
    pub title: Option<String>,
    pub source_rate: u32,
    pub source_channels: usize,
    pub decoded_frames: u64,
    pub output_frames: u64,
    pub bytes_read: u64,
    pub skipped_packets: u64,
    pub finished: bool,
    pub error: Option<String>,
    pub elapsed: Duration,
}

/// Host-side DLNA player: HTTP pull + symphonia decode into the unified engine.
pub struct DlnaPlayer {
    engine: Arc<AudioEngine>,
    arbiter: Arc<Arbiter>,
    state: SharedState,
    /// Stop token of the currently playing track (if any). A new track replaces
    /// it; `stop()` and the arbiter suspend hook raise it.
    current: Mutex<Option<Arc<AtomicBool>>>,
    timeout_ms: AtomicU64,
    stats: Mutex<Option<DlnaStats>>,
}

impl DlnaPlayer {
    pub fn new(
        engine: Arc<AudioEngine>,
        arbiter: Arc<Arbiter>,
        state: SharedState,
    ) -> Arc<Self> {
        let player = Arc::new(Self {
            engine,
            arbiter: Arc::clone(&arbiter),
            state,
            current: Mutex::new(None),
            timeout_ms: AtomicU64::new(DEFAULT_TIMEOUT.as_millis() as u64),
            stats: Mutex::new(None),
        });
        // When AirPlay/MiPlay takes over, the arbiter calls this hook outside
        // its lock. It raises our stop token so even a source blocked in a
        // socket read returns quickly (the read timeout remains the backstop).
        let weak = Arc::downgrade(&player);
        arbiter.set_suspend_hook(
            Source::Dlna,
            Arc::new(move |_lease| {
                if let Some(player) = weak.upgrade() {
                    player.stop();
                }
            }),
        );
        player
    }

    /// Override connect/read timeout (tests and slow NAS tuning).
    pub fn set_timeout(&self, timeout: Duration) {
        self.timeout_ms
            .store(timeout.as_millis().max(1) as u64, Ordering::Release);
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.load(Ordering::Acquire).max(1))
    }

    /// Stop the currently playing DLNA track, if any. Safe to call from any
    /// thread (including arbiter suspend hooks).
    pub fn stop(&self) {
        if let Some(token) = self.current.lock().unwrap().as_ref() {
            token.store(true, Ordering::Release);
        }
    }

    /// Last/current playback diagnostics.
    pub fn stats(&self) -> Option<DlnaStats> {
        self.stats.lock().unwrap().clone()
    }

    /// Start playback for a bare DLNA media URI (no metadata). Blocking; call
    /// from a worker thread.
    pub fn play_uri(&self, uri: &str) -> Result<()> {
        self.play_track(&DlnaTrack { uri, ..DlnaTrack::default() })
    }

    /// Start playback for a DLNA media URI with AVTransport metadata. Blocking;
    /// call from a worker thread.
    pub fn play_track(&self, track: &DlnaTrack<'_>) -> Result<()> {
        // A new track supersedes the previous one played by this player.
        self.stop();
        let token = Arc::new(AtomicBool::new(false));
        *self.current.lock().unwrap() = Some(Arc::clone(&token));

        let result = self.run_track(track, &token);

        let mut current = self.current.lock().unwrap();
        if current
            .as_ref()
            .is_some_and(|running| Arc::ptr_eq(running, &token))
        {
            *current = None;
        }
        drop(current);
        result
    }

    fn run_track(&self, track: &DlnaTrack<'_>, token: &Arc<AtomicBool>) -> Result<()> {
        // Reject bad/unsupported input before touching the arbiter or network.
        if let Err(error) = validate_track(track) {
            self.record_failure(&format!("DLNA 媒体被拒绝：{error:#}"));
            return Err(error);
        }

        let timeout = self.timeout();
        let started = Instant::now();
        let title = display_title(track);

        let claim = self.arbiter.claim(Source::Dlna, track.uri);
        let lease = claim.lease;
        let epoch = lease.epoch;
        let abort = AbortHandle {
            stop: Arc::clone(token),
            arbiter: Arc::clone(&self.arbiter),
            lease,
        };

        self.state.set_current_source(Some(Source::Dlna));
        {
            let mut state = self.state.state.write().unwrap();
            state.dlna.state = "buffering".into();
            state.dlna.detail = format!("正在连接 DLNA 媒体：{title}");
            state.media.title = Some(title.clone());
            state.media.artist = track.artist.map(str::to_string);
            state.media.album = track.album.map(str::to_string);
            state.media.cover_url = track.artwork_url.map(str::to_string);
            state.media.duration_ms = track.duration_ms;
            state.media.position_ms = Some(track.start_position_ms);
        }

        // Fetch + decode. An abort (stop/takeover) shows up here as an error,
        // but it is classified as a clean stop below via `abort.end_reason()`.
        let playback = (|| -> Result<PlaySummary> {
            let opened = open_source(track.uri, timeout, &abort)?;
            self.decode_and_play(opened, track, &abort, epoch, started)
        })();

        // Sample the end reason *before* releasing, otherwise the release
        // itself looks like a takeover. If another source stole the lease,
        // `release` is a no-op and `current_source` belongs to that source —
        // leave it alone.
        let reason = abort.end_reason();
        let still_owner = self.arbiter.accepts(lease);
        let new_source = self.arbiter.current().map(|current| current.source);
        // Flushing drops the device rings. Always flush when we still own the
        // engine, and also when AirPlay/MiPlay took over so stale DLNA audio
        // does not leak into the new source. Never flush a new DLNA track's
        // freshly queued PCM.
        if still_owner || new_source != Some(Source::Dlna) {
            self.engine.flush(Source::Dlna);
        }
        if still_owner && self.arbiter.release(lease) {
            // Only clear the shared source if *this* lease was still current;
            // a takeover racing with the release must not clobber its source.
            self.state.set_current_source(None);
        }
        let elapsed = started.elapsed();

        let result = match playback {
            Ok(summary) => {
                let detail = match reason {
                    EndReason::Completed => {
                        format!("播放完成：{title}{}", summary.suffix())
                    }
                    EndReason::Stopped => format!("已停止：{title}"),
                    EndReason::TakenOver => {
                        format!("被其他音源接管，停止 DLNA 播放：{title}")
                    }
                };
                {
                    let mut state = self.state.state.write().unwrap();
                    state.dlna.state = "idle".into();
                    state.dlna.detail = detail;
                    if reason == EndReason::Completed {
                        state.media.position_ms =
                            Some(summary.position_ms(track.start_position_ms));
                    }
                }
                self.store_stats(track, Some(&summary), elapsed, None);
                Ok(())
            }
            // Aborted while opening the stream: stop/takeover, not an error.
            Err(_) if reason != EndReason::Completed => {
                let detail = match reason {
                    EndReason::TakenOver => {
                        format!("被其他音源接管，停止 DLNA 播放：{title}")
                    }
                    _ => format!("已停止：{title}"),
                };
                {
                    let mut state = self.state.state.write().unwrap();
                    state.dlna.state = "idle".into();
                    state.dlna.detail = detail;
                }
                self.store_stats(track, None, elapsed, None);
                Ok(())
            }
            Err(error) => {
                self.record_failure(&format!("DLNA 播放失败（{}）：{error:#}", track.uri));
                self.store_stats(track, None, elapsed, Some(format!("{error:#}")));
                Err(error)
            }
        };
        result
    }

    fn record_failure(&self, message: &str) {
        self.state.record_error(message.to_string());
        let mut state = self.state.state.write().unwrap();
        state.dlna.state = "error".into();
        state.dlna.detail = message.to_string();
    }

    fn store_stats(
        &self,
        track: &DlnaTrack<'_>,
        summary: Option<&PlaySummary>,
        elapsed: Duration,
        error: Option<String>,
    ) {
        let stats = DlnaStats {
            uri: track.uri.to_string(),
            title: Some(display_title(track)),
            source_rate: summary.map(|summary| summary.source_rate).unwrap_or(0),
            source_channels: summary.map(|summary| summary.source_channels).unwrap_or(0),
            decoded_frames: summary.map(|summary| summary.decoded_frames).unwrap_or(0),
            output_frames: summary.map(|summary| summary.output_frames).unwrap_or(0),
            bytes_read: summary.map(|summary| summary.bytes_read).unwrap_or(0),
            skipped_packets: summary
                .map(|summary| summary.skipped_packets)
                .unwrap_or(0),
            finished: true,
            error,
            elapsed,
        };
        *self.stats.lock().unwrap() = Some(stats);
    }

    /// Decode until EOF/stop/takeover, pushing 48 kHz stereo f32 to the engine.
    fn decode_and_play(
        &self,
        opened: OpenedMedia,
        track: &DlnaTrack<'_>,
        abort: &AbortHandle,
        epoch: u64,
        started: Instant,
    ) -> Result<PlaySummary> {
        let OpenedMedia { source, bytes, content_type } = opened;

        // Sniff the container before symphonia probes it: NetEase Cloud Music
        // serves audio-only DLNA casts as `video/flv` (AAC in an FLV
        // container), which symphonia cannot demux.
        let (prefix, source) = read_prefix(source)?;
        let source: Box<dyn MediaSource> = Box::new(
            fusionplay_core::dlna_flv::PrefixedSource::new(prefix.clone(), source),
        );
        if fusionplay_core::dlna_flv::is_flv(&prefix) {
            return self.decode_flv_and_play(source, track, abort, epoch, started, bytes);
        }

        let mut hint = Hint::new();
        if let Some(extension) = uri_extension(track.uri) {
            hint.with_extension(&extension);
        }
        if let Some(mime) = content_type.as_deref() {
            let mime = mime.split(';').next().unwrap_or("").trim();
            if !mime.is_empty() {
                hint.mime_type(mime);
            }
        }

        let mss = MediaSourceStream::new(
            source,
            MediaSourceStreamOptions { buffer_len: STREAM_BUFFER_BYTES },
        );
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|error| {
                anyhow!(
                    "无法识别 DLNA 媒体格式（支持 mp3/flac/wav/aac/ogg/mp4 音频与 FLV AAC 音频；不支持 HLS/视频）：{error}"
                )
            })?;

        let selected = format
            .default_track(TrackType::Audio)
            .context("媒体中没有可解码的音频轨道（可能是纯视频或 HLS 流）")?;
        let track_id = selected.id;
        let params = match selected.codec_params.clone() {
            Some(CodecParameters::Audio(params)) => params,
            Some(_) => bail!("所选轨道不是音频轨道（DLNA 视频暂不支持）"),
            None => bail!("音频轨道缺少解码参数（编解码器可能不受支持）"),
        };

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .context("无法创建音频解码器（编解码器可能不受支持）")?;
        let codec_name = decoder.codec_info().short_name.to_string();

        let mut summary = PlaySummary {
            source_rate: params.sample_rate.unwrap_or(0),
            source_channels: params
                .channels
                .as_ref()
                .map(Channels::count)
                .unwrap_or(0),
            ..PlaySummary::default()
        };
        let mut converter: Option<Converter> = None;
        let mut interleaved: Vec<f32> = Vec::new();
        let mut pcm: Vec<f32> = Vec::new();
        let mut consecutive_errors = 0u32;
        let mut last_state_update = Instant::now();
        let mut announced_playing = false;

        loop {
            if abort.aborted() {
                break;
            }
            let packet = match format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    // Containers commonly signal a clean end-of-stream this way.
                    break
                }
                Err(SymphoniaError::IoError(error))
                    if error.kind() == io::ErrorKind::Interrupted && abort.aborted() =>
                {
                    break
                }
                Err(error) => {
                    return Err(anyhow!(
                        "读取 DLNA 媒体流失败（网络中断或容器损坏）：{error}"
                    ))
                }
            };
            if packet.track_id != track_id {
                continue;
            }

            let decoded = match decoder.decode(&packet) {
                Ok(decoded) => {
                    consecutive_errors = 0;
                    decoded
                }
                Err(error) => {
                    if abort.aborted() {
                        break;
                    }
                    summary.skipped_packets += 1;
                    if matches!(error, SymphoniaError::ResetRequired) {
                        // Decoder wants a fresh state (e.g. stream reconfig);
                        // not a corrupt packet.
                        decoder.reset();
                        consecutive_errors = 0;
                        continue;
                    }
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                        return Err(anyhow!(
                            "连续 {MAX_CONSECUTIVE_DECODE_ERRORS} 个音频包解码失败后中止（最后一个错误：{error}）"
                        ));
                    }
                    // Skip this bad packet and keep going.
                    continue;
                }
            };

            let spec = decoded.spec();
            let rate = spec.rate().max(1);
            let channels = spec.channels().clone();
            let planes = decoded.num_planes().max(1);
            let rebuild = converter
                .as_ref()
                .map(|converter| converter.source_rate != rate || converter.planes != planes)
                .unwrap_or(true);
            if rebuild {
                converter = Some(Converter::new(rate, &channels, planes)?);
                summary.source_rate = rate;
                summary.source_channels = planes;
                {
                    // Surface the real source format for the UI/diagnostics. The
                    // engine always receives 48 kHz stereo, so show both sides.
                    let mut state = self.state.state.write().unwrap();
                    state.media.detail_codec = Some(format!(
                        "{codec_name} · {} · {} 声道 → 48 kHz stereo",
                        format_rate(rate),
                        planes
                    ));
                    if announced_playing {
                        state.dlna.detail = format!(
                            "正在播放：{} · 采样率切换为 {}",
                            display_title(track),
                            format_rate(rate)
                        );
                    }
                }
            }

            interleaved.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(interleaved.as_mut_slice());
            if planes > 0 {
                summary.decoded_frames += (interleaved.len() / planes) as u64;
            }

            pcm.clear();
            let converter = converter.as_mut().expect("converter is initialised above");
            converter.convert(&interleaved, &mut pcm)?;
            if pcm.is_empty() {
                continue;
            }
            if abort.aborted() {
                break;
            }
            summary.push_frames(
                &self.engine,
                epoch,
                &pcm,
            );

            if !announced_playing {
                announced_playing = true;
                let mut state = self.state.state.write().unwrap();
                state.dlna.state = "playing".into();
            }
            if last_state_update.elapsed() >= STATE_UPDATE_INTERVAL {
                last_state_update = Instant::now();
                self.publish_progress(track, &summary, &bytes, started);
            }
        }

        // Flush the resampler tail so the last ~ms of audio are not lost.
        if let Some(converter) = converter.as_mut() {
            pcm.clear();
            converter.finish(&mut pcm)?;
            if !pcm.is_empty() && !abort.aborted() {
                summary.push_frames(&self.engine, epoch, &pcm);
            }
        }

        summary.bytes_read = bytes.load(Ordering::Relaxed);
        self.publish_progress(track, &summary, &bytes, started);
        Ok(summary)
    }

    /// Decode an FLV AAC stream (NetEase Cloud Music DLNA casts) into the engine.
    fn decode_flv_and_play(
        &self,
        source: Box<dyn MediaSource>,
        track: &DlnaTrack<'_>,
        abort: &AbortHandle,
        epoch: u64,
        started: Instant,
        bytes: Arc<AtomicU64>,
    ) -> Result<PlaySummary> {
        let mut stream = fusionplay_core::dlna_flv::FlvAacAdtsStream::new(source);
        let info = stream
            .info()
            .context("无法解析 FLV 音频流（网易云等音乐 App 的投屏容器）")?;
        let layout = info
            .channel_layout()
            .context("FLV 音频声道布局暂不支持（仅支持单声道/立体声）")?;
        let mut parameters = AudioCodecParameters::new();
        parameters
            .for_codec(CODEC_ID_AAC)
            .with_sample_rate(info.sample_rate)
            .with_channels(layout);
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&parameters, &AudioDecoderOptions::default())
            .context("无法创建 FLV AAC 解码器")?;

        let mut summary = PlaySummary {
            source_rate: info.sample_rate,
            source_channels: usize::from(info.channels),
            ..PlaySummary::default()
        };
        let mut converter: Option<Converter> = None;
        let mut interleaved: Vec<f32> = Vec::new();
        let mut pcm: Vec<f32> = Vec::new();
        let mut timestamp: u64 = 0;
        let mut consecutive_errors = 0u32;
        let mut last_state_update = Instant::now();
        let mut announced_playing = false;

        loop {
            if abort.aborted() {
                break;
            }
            let payload = match read_adts_payload(&mut stream) {
                Ok(Some(payload)) => payload,
                Ok(None) => break,
                Err(_) if abort.aborted() => break,
                Err(error) => {
                    return Err(anyhow!(
                        "读取 FLV 音频流失败（网络中断或容器损坏）：{error}"
                    ))
                }
            };
            let packet = Packet::new(
                0,
                Timestamp::new(timestamp as i64),
                MediaDuration::new(1024),
                payload,
            );
            timestamp = timestamp.saturating_add(1024);
            let decoded = match decoder.decode(&packet) {
                Ok(decoded) => {
                    consecutive_errors = 0;
                    decoded
                }
                Err(error) => {
                    if abort.aborted() {
                        break;
                    }
                    summary.skipped_packets += 1;
                    if matches!(error, SymphoniaError::ResetRequired) {
                        decoder.reset();
                        consecutive_errors = 0;
                        continue;
                    }
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                        return Err(anyhow!(
                            "连续 {MAX_CONSECUTIVE_DECODE_ERRORS} 个 FLV 音频包解码失败后中止（最后一个错误：{error}）"
                        ));
                    }
                    continue;
                }
            };

            let spec = decoded.spec();
            let rate = spec.rate().max(1);
            let channels = spec.channels().clone();
            let planes = decoded.num_planes().max(1);
            let rebuild = converter
                .as_ref()
                .map(|converter| converter.source_rate != rate || converter.planes != planes)
                .unwrap_or(true);
            if rebuild {
                converter = Some(Converter::new(rate, &channels, planes)?);
                summary.source_rate = rate;
                summary.source_channels = planes;
                {
                    let mut state = self.state.state.write().unwrap();
                    state.media.detail_codec = Some(format!(
                        "aac(flv) · {} · {} 声道 → 48 kHz stereo",
                        format_rate(rate),
                        planes
                    ));
                }
            }

            interleaved.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(interleaved.as_mut_slice());
            if planes > 0 {
                summary.decoded_frames += (interleaved.len() / planes) as u64;
            }

            pcm.clear();
            let converter = converter.as_mut().expect("converter is initialised above");
            converter.convert(&interleaved, &mut pcm)?;
            if pcm.is_empty() {
                continue;
            }
            if abort.aborted() {
                break;
            }
            summary.push_frames(&self.engine, epoch, &pcm);

            if !announced_playing {
                announced_playing = true;
                let mut state = self.state.state.write().unwrap();
                state.dlna.state = "playing".into();
            }
            if last_state_update.elapsed() >= STATE_UPDATE_INTERVAL {
                last_state_update = Instant::now();
                self.publish_progress(track, &summary, &bytes, started);
            }
        }

        if let Some(converter) = converter.as_mut() {
            pcm.clear();
            converter.finish(&mut pcm)?;
            if !pcm.is_empty() && !abort.aborted() {
                summary.push_frames(&self.engine, epoch, &pcm);
            }
        }

        summary.bytes_read = bytes.load(Ordering::Relaxed);
        self.publish_progress(track, &summary, &bytes, started);
        Ok(summary)
    }

    fn publish_progress(
        &self,
        track: &DlnaTrack<'_>,
        summary: &PlaySummary,
        bytes: &Arc<AtomicU64>,
        started: Instant,
    ) {
        let bytes_read = bytes.load(Ordering::Relaxed);
        let detail = format!(
            "正在播放：{} · {} → 48 kHz · 已解码 {} 帧 · {} 字节",
            display_title(track),
            format_rate(summary.source_rate),
            summary.decoded_frames,
            bytes_read
        );
        {
            let mut state = self.state.state.write().unwrap();
            state.dlna.state = "playing".into();
            state.dlna.detail = detail;
            state.media.position_ms = Some(summary.position_ms(track.start_position_ms));
        }
        let stats = DlnaStats {
            uri: track.uri.to_string(),
            title: Some(display_title(track)),
            source_rate: summary.source_rate,
            source_channels: summary.source_channels,
            decoded_frames: summary.decoded_frames,
            output_frames: summary.output_frames,
            bytes_read,
            skipped_packets: summary.skipped_packets,
            finished: false,
            error: None,
            elapsed: started.elapsed(),
        };
        *self.stats.lock().unwrap() = Some(stats);
    }
}

impl Drop for DlnaPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Playback bookkeeping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    Completed,
    Stopped,
    TakenOver,
}

#[derive(Default)]
struct PlaySummary {
    source_rate: u32,
    source_channels: usize,
    decoded_frames: u64,
    output_frames: u64,
    skipped_packets: u64,
    bytes_read: u64,
}

impl PlaySummary {
    fn push_frames(&mut self, engine: &AudioEngine, epoch: u64, pcm: &[f32]) {
        if pcm.len() < TARGET_CHANNELS {
            return;
        }
        self.output_frames += (pcm.len() / TARGET_CHANNELS) as u64;
        engine.push_pcm(
            Source::Dlna,
            epoch,
            1.0,
            TARGET_RATE,
            TARGET_CHANNELS as u16,
            pcm,
        );
    }

    fn position_ms(&self, start_position_ms: u64) -> u64 {
        start_position_ms
            .saturating_add(self.output_frames.saturating_mul(1000) / TARGET_RATE as u64)
    }

    fn suffix(&self) -> String {
        format!(
            " · {} → 48 kHz · 已解码 {} 帧（推送 {} 帧） · {} 字节",
            format_rate(self.source_rate),
            self.decoded_frames,
            self.output_frames,
            self.bytes_read
        )
    }
}

/// Reads the first bytes of a media stream so the caller can sniff the
/// container, then hands back a source that replays them.
fn read_prefix(mut source: Box<dyn MediaSource>) -> Result<(Vec<u8>, Box<dyn MediaSource>)> {
    const SNIFF_BYTES: usize = 4;
    let mut prefix = vec![0u8; SNIFF_BYTES];
    let mut filled = 0;
    while filled < SNIFF_BYTES {
        match source.read(&mut prefix[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(anyhow!(
                    "读取 DLNA 媒体流用于格式识别失败（网络中断）：{error}"
                ));
            }
        }
    }
    prefix.truncate(filled);
    Ok((prefix, source))
}

/// Fill `buf` until it is full, returning the number of bytes read.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Pulls one ADTS frame from the FLV demuxer's AAC output and returns its raw
/// AAC payload (the ADTS header is stripped).
fn read_adts_payload<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 7];
    let read = read_full(reader, &mut header)?;
    if read == 0 {
        return Ok(None);
    }
    if read < header.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "FLV ADTS 头被截断",
        ));
    }
    if header[0] != 0xFF || header[1] & 0xF6 != 0xF0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "FLV AAC 帧同步字非法",
        ));
    }
    let header_len = if header[1] & 0x01 == 0 { 9 } else { 7 };
    if header_len == 9 {
        let mut crc = [0u8; 2];
        let read = read_full(reader, &mut crc)?;
        if read < crc.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "FLV ADTS CRC 被截断",
            ));
        }
    }
    let frame_len = ((usize::from(header[3] & 0x03)) << 11)
        | (usize::from(header[4]) << 3)
        | (usize::from(header[5]) >> 5);
    if frame_len <= header_len || frame_len > 0x1FFF {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "FLV AAC 帧长度非法",
        ));
    }
    let mut payload = vec![0u8; frame_len - header_len];
    let read = read_full(reader, &mut payload)?;
    if read < payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "FLV AAC 帧被截断",
        ));
    }
    Ok(Some(payload))
}

fn format_rate(rate: u32) -> String {
    if rate == 0 {
        "未知采样率".to_string()
    } else if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    }
}

// ---------------------------------------------------------------------------
// URI validation / display helpers
// ---------------------------------------------------------------------------

/// Validate scheme and reject unsupported media up-front with an explicit
/// reason (before claiming the arbiter or opening a socket).
fn validate_track(track: &DlnaTrack<'_>) -> Result<()> {
    let uri = track.uri;
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        bail!("DLNA 媒体地址为空");
    }
    if trimmed != uri {
        bail!("DLNA 媒体地址包含首尾空白：{uri}");
    }
    if uri.chars().any(|character| character.is_control() || character.is_whitespace()) {
        bail!("DLNA 媒体地址包含非法字符（空白/控制字符）：{uri}");
    }

    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("https://") {
        bail!("DLNA HTTPS 媒体暂不支持（首版未打包 TLS 栈）：{uri}");
    }
    let Some(scheme_end) = lower.find("://") else {
        bail!("不支持的 DLNA 媒体地址（仅支持 http:// 与 file://）：{uri}");
    };
    match &lower[..scheme_end] {
        "http" => {
            parse_http_url(uri)?;
        }
        "file" => {
            file_path_from_uri(uri)?;
        }
        scheme => bail!("不支持的 DLNA 媒体地址协议 {scheme}://（仅支持 http:// 与 file://）：{uri}"),
    }

    // HLS playlists are not media streams we can decode.
    let path = uri.split(|character| character == '?' || character == '#').next().unwrap_or(uri);
    let path_lower = path.to_ascii_lowercase();
    if path_lower.ends_with(".m3u8") || path_lower.ends_with(".m3u") {
        bail!("DLNA HLS 播放列表（m3u8/m3u）暂不支持，请使用音频文件直链：{uri}");
    }
    if let Some(content_type) = track.content_type {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.contains("mpegurl") {
            bail!("DLNA HLS 播放列表暂不支持，请使用音频文件直链：{uri}");
        }
        if content_type.starts_with("video/") && !is_flv_content_type(&content_type) {
            bail!(
                "DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/ogg/mp4 音轨，以及网易云常用的 FLV 音频）：{uri}"
            );
        }
    }
    let flv_audio = path_lower.ends_with(".flv")
        || track
            .content_type
            .is_some_and(is_flv_content_type);
    if track.media_kind.eq_ignore_ascii_case("video") && !flv_audio {
        bail!(
            "DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/ogg/mp4 音轨，以及网易云常用的 FLV 音频）：{uri}"
        );
    }
    if track
        .upnp_class
        .is_some_and(|class| class.to_ascii_lowercase().contains("videoitem"))
        && !flv_audio
    {
        bail!(
            "DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/ogg/mp4 音轨，以及网易云常用的 FLV 音频）：{uri}"
        );
    }
    Ok(())
}

/// Best-effort title: AVTransport title, else the last path segment.
fn display_title(track: &DlnaTrack<'_>) -> String {
    if let Some(title) = track.title.map(str::trim).filter(|title| !title.is_empty()) {
        return title.to_string();
    }
    uri_file_name(track.uri).unwrap_or_else(|| track.uri.to_string())
}

fn uri_file_name(uri: &str) -> Option<String> {
    let without_fragment = uri.split('#').next().unwrap_or(uri);
    let without_query = without_fragment.split('?').next().unwrap_or(without_fragment);
    // Drop `scheme://authority`, then take the last path segment.
    let path = match without_query.find("://") {
        Some(scheme_end) => {
            let rest = &without_query[scheme_end + 3..];
            match rest.find('/') {
                Some(index) => &rest[index..],
                None => return None,
            }
        }
        None => without_query,
    };
    let trimmed = path.trim_end_matches('/');
    let name = trimmed.rsplit('/').next()?;
    if name.is_empty() {
        return None;
    }
    Some(percent_decode(name).unwrap_or_else(|_| name.to_string()))
}

/// File extension used as a probe hint (e.g. raw ADTS `.aac` files).
fn uri_extension(uri: &str) -> Option<String> {
    let path = uri.split(|character| character == '?' || character == '#').next()?;
    let name = path.rsplit('/').next()?;
    let extension = name.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 5
        || !extension.chars().all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

fn percent_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("非法百分号转义：{input}");
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| anyhow!("非法百分号转义：{input}"))?;
            decoded.push(hex);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| anyhow!("file:// 路径不是合法 UTF-8：{input}"))
}

// ---------------------------------------------------------------------------
// Media sources
// ---------------------------------------------------------------------------

/// Where the bytes come from, plus the live download counter.
struct OpenedMedia {
    source: Box<dyn MediaSource>,
    bytes: Arc<AtomicU64>,
    content_type: Option<String>,
}

fn open_source(uri: &str, timeout: Duration, abort: &AbortHandle) -> Result<OpenedMedia> {
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http://") {
        open_http(uri, timeout, abort)
    } else if lower.starts_with("file://") {
        open_file(uri)
    } else if lower.starts_with("https://") {
        bail!("DLNA HTTPS 媒体暂不支持（首版未打包 TLS 栈）：{uri}");
    } else {
        bail!("不支持的 DLNA 媒体地址（仅支持 http:// 与 file://）：{uri}");
    }
}

fn file_path_from_uri(uri: &str) -> Result<String> {
    let scheme_end = uri
        .find("://")
        .ok_or_else(|| anyhow!("非法 file:// 地址：{uri}"))?;
    let mut rest = &uri[scheme_end + 3..];
    // Accept both file:///path and file://localhost/path.
    if let Some(stripped) = rest.strip_prefix("localhost") {
        if stripped.is_empty() || stripped.starts_with('/') {
            rest = stripped;
        }
    }
    if rest.is_empty() {
        bail!("file:// 地址缺少路径：{uri}");
    }
    let path = percent_decode(rest)?;
    if !path.starts_with('/') {
        bail!("file:// 必须是绝对路径：{uri}");
    }
    Ok(path)
}

fn open_file(uri: &str) -> Result<OpenedMedia> {
    let path = file_path_from_uri(uri)?;
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("无法访问本地媒体文件 {path}"))?;
    if metadata.is_dir() {
        bail!("file:// 地址指向目录而非媒体文件：{path}");
    }
    let file = std::fs::File::open(&path)
        .with_context(|| format!("无法打开本地媒体文件 {path}"))?;
    let bytes = Arc::new(AtomicU64::new(0));
    let source = CountingFile {
        inner: file,
        bytes: Arc::clone(&bytes),
    };
    Ok(OpenedMedia {
        source: Box::new(source),
        bytes,
        content_type: None,
    })
}

struct CountingFile {
    inner: std::fs::File,
    bytes: Arc<AtomicU64>,
}

impl Read for CountingFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

impl Seek for CountingFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

impl MediaSource for CountingFile {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.metadata().ok().map(|metadata| metadata.len())
    }
}

// ---------------------------------------------------------------------------
// HTTP/1.1 GET with streaming body
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct HttpUrl {
    host: String,
    port: u16,
    authority: String,
    request_target: String,
}

impl HttpUrl {
    fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            self.authority.clone()
        }
    }
}

/// Parse an `http://host[:port][/path][?query]` URI. Rejects the malformed
/// shapes we cannot safely request instead of guessing.
fn parse_http_url(uri: &str) -> Result<HttpUrl> {
    let scheme_end = uri
        .find("://")
        .ok_or_else(|| anyhow!("非法 HTTP 地址：{uri}"))?;
    let rest = &uri[scheme_end + 3..];
    let authority_end = rest
        .find(|character| character == '/' || character == '?' || character == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        bail!("DLNA 媒体地址缺少主机名：{uri}");
    }
    if authority.contains('@') {
        bail!("DLNA 媒体地址不支持 userinfo：{uri}");
    }

    let mut target = if authority_end < rest.len() {
        rest[authority_end..].to_string()
    } else {
        String::from("/")
    };
    if target.starts_with('?') {
        target.insert(0, '/');
    }
    if let Some(fragment) = target.find('#') {
        target.truncate(fragment);
    }
    if target.is_empty() {
        target.push('/');
    }

    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: [::1] or [::1]:8080
        let close = stripped
            .find(']')
            .ok_or_else(|| anyhow!("非法 IPv6 主机：{uri}"))?;
        let host = &stripped[..close];
        let after = &stripped[close + 1..];
        let port = if after.is_empty() {
            80
        } else if let Some(port) = after.strip_prefix(':') {
            port.parse::<u16>()
                .with_context(|| format!("非法端口号：{uri}"))?
        } else {
            bail!("非法 IPv6 主机（缺少 ] 之后的分隔）：{uri}");
        };
        if host.is_empty() {
            bail!("DLNA 媒体地址缺少主机名：{uri}");
        }
        (host.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => {
                if host.is_empty() {
                    bail!("DLNA 媒体地址缺少主机名：{uri}");
                }
                let port = port
                    .parse::<u16>()
                    .with_context(|| format!("非法端口号：{uri}"))?;
                (host.to_string(), port)
            }
            None => (authority.to_string(), 80),
        }
    };

    Ok(HttpUrl {
        host,
        port,
        authority: authority.to_string(),
        request_target: target,
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn resolve_redirect(base: &HttpUrl, location: &str) -> Result<HttpUrl> {
    let location = location.trim();
    if location.is_empty() {
        bail!("DLNA 媒体服务器返回空重定向地址");
    }
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("https://") {
        bail!("DLNA HTTPS 媒体暂不支持（首版未打包 TLS 栈）：{location}");
    }
    if lower.starts_with("http://") {
        return parse_http_url(location);
    }
    if let Some(stripped) = lower.strip_prefix("//") {
        // Scheme-relative: reuse http.
        let prefix_len = location.len() - stripped.len();
        return parse_http_url(&format!("http:{}", &location[prefix_len - 1..]));
    }
    // Relative redirect: resolve against the current request target.
    let current_path = base.request_target.split('?').next().unwrap_or("/");
    let directory = match current_path.rfind('/') {
        Some(index) => &current_path[..=index],
        None => "/",
    };
    let merged = if location.starts_with('/') {
        location.to_string()
    } else {
        format!("{directory}{location}")
    };
    let merged = merged.split('#').next().unwrap_or(&merged).to_string();
    Ok(HttpUrl {
        host: base.host.clone(),
        port: base.port,
        authority: base.authority.clone(),
        request_target: merged,
    })
}

struct HttpHead {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    reader: BufReader<TcpStream>,
}

impl HttpHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name == name)
            .map(|(_, value)| value.as_str())
    }
}

fn open_http(uri: &str, timeout: Duration, abort: &AbortHandle) -> Result<OpenedMedia> {
    let bytes = Arc::new(AtomicU64::new(0));
    let mut url = parse_http_url(uri)?;
    let mut redirects = 0;

    loop {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
        let response = http_request(&url, timeout)?;
        if is_redirect(response.status) {
            if redirects >= MAX_REDIRECTS {
                bail!("DLNA 媒体服务器重定向次数过多（>{MAX_REDIRECTS}）：{uri}");
            }
            let location = response
                .header("location")
                .ok_or_else(|| anyhow!("HTTP {} 响应缺少 Location 头：{uri}", response.status))?
                .to_string();
            url = resolve_redirect(&url, &location)?;
            redirects += 1;
            continue;
        }
        if response.status != 200 {
            bail!(
                "DLNA 媒体服务器返回 HTTP {} {}：{uri}",
                response.status,
                response.reason
            );
        }
        if let Some(encoding) = response.header("content-encoding") {
            let encoding = encoding.trim().to_ascii_lowercase();
            if !encoding.is_empty() && encoding != "identity" {
                bail!("DLNA 媒体使用了不支持的压缩传输（Content-Encoding: {encoding}）：{uri}");
            }
        }

        let chunked = response
            .header("transfer-encoding")
            .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
        let content_length = response
            .header("content-length")
            .and_then(|value| value.trim().parse::<u64>().ok());
        let connection_close = response
            .header("connection")
            .is_some_and(|value| value.to_ascii_lowercase().contains("close"));

        let body_kind = if chunked {
            BodyKind::Chunked
        } else if let Some(content_length) = content_length {
            BodyKind::Length(content_length)
        } else if connection_close {
            // HTTP/1.0-style body delimited by connection close.
            BodyKind::UntilClose
        } else {
            bail!(
                "DLNA 媒体服务器既未提供 Content-Length 也不是 chunked，无法流式读取（保持连接会无限等待）：{uri}"
            );
        };

        let content_type = response.header("content-type").map(str::to_string);
        let body = HttpBody {
            reader: response.reader,
            kind: body_kind,
            phase: ChunkedPhase::Size,
            done: false,
            bytes: Arc::clone(&bytes),
            timeout,
            abort: abort.clone(),
        };
        let source = HttpSource {
            body,
            content_length: if chunked { None } else { content_length },
        };
        return Ok(OpenedMedia {
            source: Box::new(source),
            bytes,
            content_type,
        });
    }
}

fn dlna_user_agent() -> String {
    std::env::var("FUSIONPLAY_DLNA_USER_AGENT")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_AGENT.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_flv_content_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "video/flv" | "video/x-flv" | "application/x-flv"
    )
}

fn http_request(url: &HttpUrl, timeout: Duration) -> Result<HttpHead> {
    let addrs = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .with_context(|| format!("无法解析 DLNA 媒体服务器地址 {}（{}）", url.authority, url.host))?;
    let connect_timeout = timeout.min(CONNECT_TIMEOUT);
    let mut stream = None;
    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, connect_timeout) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some(stream) = stream else {
        let detail = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "没有可用的地址".to_string());
        bail!("无法连接 DLNA 媒体服务器 {}：{detail}", url.authority);
    };
    stream
        .set_read_timeout(Some(timeout))
        .context("无法设置 DLNA 媒体读取超时")?;
    stream
        .set_write_timeout(Some(timeout))
        .context("无法设置 DLNA 媒体写入超时")?;
    let _ = stream.set_nodelay(true);

    let mut stream = stream;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.request_target,
        url.host_header(),
        dlna_user_agent()
    );
    stream
        .write_all(request.as_bytes())
        .with_context(|| format!("无法向 DLNA 媒体服务器 {} 发送 GET 请求", url.authority))?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let status_line = read_http_line(&mut reader, timeout)
        .with_context(|| format!("读取 DLNA 媒体服务器 {} 的 HTTP 状态行失败", url.authority))?;
    let (status, reason) = parse_status_line(&status_line)
        .with_context(|| format!("DLNA 媒体服务器返回非法 HTTP 状态行：{status_line}"))?;
    let mut headers = Vec::new();
    for _ in 0..MAX_HEADER_COUNT {
        let line = read_http_line(&mut reader, timeout)
            .with_context(|| format!("读取 DLNA 媒体服务器 {} 的 HTTP 响应头失败", url.authority))?;
        if line.is_empty() {
            return Ok(HttpHead { status, reason, headers, reader });
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    bail!("DLNA 媒体服务器返回的 HTTP 响应头超过 {MAX_HEADER_COUNT} 行");
}

fn parse_status_line(line: &str) -> Result<(u16, String)> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.to_ascii_uppercase().starts_with("HTTP/") {
        bail!("不是 HTTP 响应");
    }
    let status = parts
        .next()
        .ok_or_else(|| anyhow!("缺少状态码"))?
        .parse::<u16>()
        .map_err(|_| anyhow!("状态码不是数字"))?;
    let reason = parts.next().unwrap_or("").trim().to_string();
    Ok((status, reason))
}

/// Read one CRLF-terminated header line with a hard size cap (defends against
/// servers that never send a newline).
fn read_http_line(reader: &mut BufReader<TcpStream>, timeout: Duration) -> io::Result<String> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) => return Err(map_stream_error(error, timeout, "读取 DLNA HTTP 响应")),
        };
        if available.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "DLNA 媒体服务器提前关闭连接（HTTP 响应不完整）",
            ));
        }
        let capacity = MAX_HEADER_LINE.saturating_sub(line.len()) + 1;
        let take = available.len().min(capacity);
        if let Some(position) = available[..take].iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&available[..position + 1]);
            reader.consume(position + 1);
            break;
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.len() > MAX_HEADER_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DLNA 媒体服务器 HTTP 响应行过长",
            ));
        }
    }
    while line.last().is_some_and(|byte| *byte == b'\r' || *byte == b'\n') {
        line.pop();
    }
    String::from_utf8(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "DLNA HTTP 响应头不是 UTF-8"))
}

enum BodyKind {
    Length(u64),
    Chunked,
    UntilClose,
}

#[derive(Clone, Copy)]
enum ChunkedPhase {
    Size,
    Data(u64),
    DataCrlf,
    Trailers,
}

/// Streaming HTTP body: decodes `Content-Length`, chunked and connection-close
/// framing on demand. Holds O(1) memory regardless of media size.
struct HttpBody {
    reader: BufReader<TcpStream>,
    kind: BodyKind,
    phase: ChunkedPhase,
    done: bool,
    bytes: Arc<AtomicU64>,
    timeout: Duration,
    abort: AbortHandle,
}

impl HttpBody {
    fn read_some(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // Checked before every socket read so stop/takeover is noticed between
        // reads (the read timeout is the backstop for an idle server).
        if self.abort.aborted() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "DLNA 播放已中止（停止或音源被接管）",
            ));
        }
        match self.reader.read(buffer) {
            Ok(read) => Ok(read),
            Err(error) => Err(map_stream_error(error, self.timeout, "读取 DLNA 媒体流")),
        }
    }

    fn read_exact_body(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = self.read_some(&mut buffer[filled..])?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "DLNA chunked 媒体流在块边界处提前结束（服务器提前关闭连接）",
                ));
            }
            filled += read;
        }
        Ok(())
    }
}

impl Read for HttpBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.done {
                return Ok(0);
            }
            match self.kind {
                BodyKind::Length(remaining) => {
                    if remaining == 0 {
                        self.done = true;
                        return Ok(0);
                    }
                    let want = buffer.len().min(remaining as usize);
                    let read = self.read_some(&mut buffer[..want])?;
                    if read == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "DLNA 下载中断：Content-Length 仍差 {remaining} 字节未收到（服务器提前关闭连接）"
                            ),
                        ));
                    }
                    self.kind = BodyKind::Length(remaining - read as u64);
                    self.bytes.fetch_add(read as u64, Ordering::Relaxed);
                    return Ok(read);
                }
                BodyKind::UntilClose => {
                    let read = self.read_some(buffer)?;
                    if read == 0 {
                        self.done = true;
                    } else {
                        self.bytes.fetch_add(read as u64, Ordering::Relaxed);
                    }
                    return Ok(read);
                }
                BodyKind::Chunked => match self.phase {
                    ChunkedPhase::Size => {
                        let line = match read_http_line(&mut self.reader, self.timeout) {
                            Ok(line) => line,
                            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "DLNA chunked 媒体流中断：未收到结束块（0 长度块）就断开连接",
                                ))
                            }
                            Err(error) => return Err(error),
                        };
                        let size_field = line.split(';').next().unwrap_or("").trim();
                        if size_field.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "DLNA chunked 媒体流缺少块长度",
                            ));
                        }
                        let size = u64::from_str_radix(size_field, 16).map_err(|_| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("DLNA chunked 媒体流块长度非法：{line}"),
                            )
                        })?;
                        self.phase = if size == 0 {
                            ChunkedPhase::Trailers
                        } else {
                            ChunkedPhase::Data(size)
                        };
                    }
                    ChunkedPhase::Data(remaining) => {
                        if remaining == 0 {
                            self.phase = ChunkedPhase::DataCrlf;
                            continue;
                        }
                        let want = buffer.len().min(remaining as usize);
                        let read = self.read_some(&mut buffer[..want])?;
                        if read == 0 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("DLNA chunked 媒体流中断：块内仍差 {remaining} 字节（服务器提前关闭连接）"),
                            ));
                        }
                        self.bytes.fetch_add(read as u64, Ordering::Relaxed);
                        self.phase = ChunkedPhase::Data(remaining - read as u64);
                        return Ok(read);
                    }
                    ChunkedPhase::DataCrlf => {
                        let mut crlf = [0u8; 2];
                        self.read_exact_body(&mut crlf)?;
                        if &crlf != b"\r\n" {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "DLNA chunked 媒体流块结束标记非法",
                            ));
                        }
                        self.phase = ChunkedPhase::Size;
                    }
                    ChunkedPhase::Trailers => {
                        let line = match read_http_line(&mut self.reader, self.timeout) {
                            Ok(line) => line,
                            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "DLNA chunked 媒体流中断：缺少结束空行就断开连接",
                                ))
                            }
                            Err(error) => return Err(error),
                        };
                        if line.is_empty() {
                            self.done = true;
                            return Ok(0);
                        }
                    }
                },
            }
        }
    }
}

struct HttpSource {
    body: HttpBody,
    content_length: Option<u64>,
}

impl Read for HttpSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.body.read(buffer)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, _position: SeekFrom) -> io::Result<u64> {
        Err(io::Error::other("DLNA HTTP 媒体流不支持 seek"))
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

/// Map low-level socket errors to actionable Chinese diagnostics.
fn map_stream_error(error: io::Error, timeout: Duration, what: &str) -> io::Error {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{what}超时（{timeout:?} 内无数据，媒体服务器过慢或网络中断）"),
        ),
        io::ErrorKind::UnexpectedEof => io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("{what}：DLNA 媒体服务器提前断开连接"),
        ),
        io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe => io::Error::new(
            error.kind(),
            format!("{what}：DLNA 媒体连接被重置/断开"),
        ),
        _ => io::Error::new(error.kind(), format!("{what}失败：{error}")),
    }
}

// ---------------------------------------------------------------------------
// Abort handling (user stop + arbiter takeover)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AbortHandle {
    stop: Arc<AtomicBool>,
    arbiter: Arc<Arbiter>,
    lease: Lease,
}

impl AbortHandle {
    fn aborted(&self) -> bool {
        self.stop.load(Ordering::Acquire) || !self.arbiter.accepts(self.lease)
    }

    fn end_reason(&self) -> EndReason {
        if self.stop.load(Ordering::Acquire) {
            EndReason::Stopped
        } else if !self.arbiter.accepts(self.lease) {
            EndReason::TakenOver
        } else {
            EndReason::Completed
        }
    }
}

// ---------------------------------------------------------------------------
// Downmix + 48 kHz resample pipeline
// ---------------------------------------------------------------------------

/// Converts decoded source audio into the engine's 48 kHz stereo f32 format.
///
/// Multi-channel input is *mixed* (ITU-style coefficients for positioned
/// layouts, a conservative L/R/centre heuristic for layouts without positions)
/// rather than dropping channels.
struct Converter {
    source_rate: u32,
    planes: usize,
    downmix: Downmix,
    resampler: Option<StreamResampler>,
    stereo: Vec<f32>,
}

impl Converter {
    fn new(source_rate: u32, channels: &Channels, planes: usize) -> Result<Self> {
        Ok(Self {
            source_rate,
            planes: planes.max(1),
            downmix: Downmix::new(channels, planes),
            resampler: if source_rate == TARGET_RATE {
                None
            } else {
                Some(StreamResampler::new(source_rate)?)
            },
            stereo: Vec::new(),
        })
    }

    fn convert(&mut self, interleaved: &[f32], out: &mut Vec<f32>) -> Result<()> {
        let frames = interleaved.len() / self.planes;
        self.stereo.clear();
        if self.downmix.is_direct() {
            self.stereo
                .extend_from_slice(&interleaved[..frames * TARGET_CHANNELS]);
        } else {
            self.stereo.reserve(frames * TARGET_CHANNELS);
            for frame in interleaved.chunks_exact(self.planes) {
                let [left, right] = self.downmix.mix(frame);
                self.stereo.push(left);
                self.stereo.push(right);
            }
        }
        out.clear();
        match self.resampler.as_mut() {
            Some(resampler) => resampler.push(&self.stereo, out)?,
            None => out.extend_from_slice(&self.stereo),
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        out.clear();
        match self.resampler.as_mut() {
            Some(resampler) => resampler.finish(out),
            None => Ok(()),
        }
    }
}

/// Channel layout aware stereo downmixer. Coefficients follow the common ITU-R
/// BS.775 style matrix (centre/surround at -3 dB, LFE at -6 dB), then each side
/// is normalised so a full-scale multi-channel frame cannot clip.
struct Downmix {
    direct: bool,
    taps: Vec<(usize, f32, f32)>,
}

impl Downmix {
    fn new(channels: &Channels, planes: usize) -> Self {
        const S: f32 = 0.707_106_77;

        let mut taps: Vec<(usize, f32, f32)> = Vec::new();
        match channels {
            Channels::Positioned(positions) => {
                let mut channel_index = 0usize;
                for bit in 0..64u32 {
                    let flag = positions.bits() & (1u64 << bit);
                    if flag == 0 {
                        continue;
                    }
                    let position = Position::from_bits_truncate(flag);
                    let (left, right) = position_gains(position, S);
                    taps.push((channel_index, left, right));
                    channel_index += 1;
                }
            }
            _ => {
                taps = fallback_gains(planes);
            }
        }

        if taps.len() != planes && planes > 0 {
            // Layout/plane mismatch (unusual codec configuration): fall back to
            // an index-based mix so no audio channel is silently dropped.
            taps = fallback_gains(planes);
        }

        let (mut sum_left, mut sum_right) =
            taps.iter().fold((0.0f32, 0.0f32), |(left, right), (_, tap_left, tap_right)| {
                (left + tap_left, right + tap_right)
            });
        // A one-sided layout (mono, or a single speaker position) must not mute
        // the opposite channel: duplicate the covered side instead.
        if sum_left <= f32::EPSILON && sum_right > f32::EPSILON {
            for tap in taps.iter_mut() {
                tap.1 = tap.2;
            }
            sum_left = sum_right;
        } else if sum_right <= f32::EPSILON && sum_left > f32::EPSILON {
            for tap in taps.iter_mut() {
                tap.2 = tap.1;
            }
            sum_right = sum_left;
        }
        if sum_left > f32::EPSILON {
            for tap in taps.iter_mut() {
                tap.1 /= sum_left;
            }
        }
        if sum_right > f32::EPSILON {
            for tap in taps.iter_mut() {
                tap.2 /= sum_right;
            }
        }

        let identity = planes == TARGET_CHANNELS
            && taps.len() == TARGET_CHANNELS
            && (taps[0].0, taps[0].1, taps[0].2) == (0, 1.0, 0.0)
            && (taps[1].0, taps[1].1, taps[1].2) == (1, 0.0, 1.0);
        Downmix {
            direct: identity,
            taps,
        }
    }

    fn is_direct(&self) -> bool {
        self.direct
    }

    fn mix(&self, frame: &[f32]) -> [f32; TARGET_CHANNELS] {
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for &(index, tap_left, tap_right) in &self.taps {
            let sample = frame.get(index).copied().unwrap_or(0.0);
            left += sample * tap_left;
            right += sample * tap_right;
        }
        [left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0)]
    }
}

fn position_gains(position: Position, surround: f32) -> (f32, f32) {
    const LEFT: Position = Position::FRONT_LEFT
        .union(Position::SIDE_LEFT)
        .union(Position::REAR_LEFT)
        .union(Position::FRONT_LEFT_CENTER)
        .union(Position::TOP_FRONT_LEFT)
        .union(Position::TOP_REAR_LEFT)
        .union(Position::TOP_SIDE_LEFT)
        .union(Position::BOTTOM_FRONT_LEFT);
    const RIGHT: Position = Position::FRONT_RIGHT
        .union(Position::SIDE_RIGHT)
        .union(Position::REAR_RIGHT)
        .union(Position::FRONT_RIGHT_CENTER)
        .union(Position::TOP_FRONT_RIGHT)
        .union(Position::TOP_REAR_RIGHT)
        .union(Position::TOP_SIDE_RIGHT)
        .union(Position::BOTTOM_FRONT_RIGHT);
    const CENTER: Position = Position::FRONT_CENTER
        .union(Position::REAR_CENTER)
        .union(Position::TOP_CENTER)
        .union(Position::TOP_FRONT_CENTER)
        .union(Position::TOP_REAR_CENTER)
        .union(Position::BOTTOM_FRONT_CENTER);
    const LFE: Position = Position::LFE1.union(Position::LFE2);

    if position == Position::FRONT_LEFT || position == Position::FRONT_LEFT_WIDE {
        return (1.0, 0.0);
    }
    if position == Position::FRONT_RIGHT || position == Position::FRONT_RIGHT_WIDE {
        return (0.0, 1.0);
    }
    if position.intersects(LFE) {
        // LFE content is mixed in quietly to keep bass energy on stereo rigs.
        return (0.5, 0.5);
    }
    let left = position.intersects(LEFT);
    let right = position.intersects(RIGHT);
    let center = position.intersects(CENTER);
    if left && !right {
        (surround, 0.0)
    } else if right && !left {
        (0.0, surround)
    } else if center || (left && right) {
        (surround, surround)
    } else {
        (surround, surround)
    }
}

/// Index-based mix for layouts symphonia does not describe with positions.
fn fallback_gains(planes: usize) -> Vec<(usize, f32, f32)> {
    const S: f32 = 0.707_106_77;
    match planes {
        0 => Vec::new(),
        1 => vec![(0, 1.0, 1.0)],
        2 => vec![(0, 1.0, 0.0), (1, 0.0, 1.0)],
        _ => {
            let mut taps = vec![(0, 1.0, 0.0), (1, 0.0, 1.0)];
            let surround = S / (planes - 2) as f32;
            for index in 2..planes {
                taps.push((index, surround, surround));
            }
            taps
        }
    }
}

/// Streaming resampler (rubato sinc) from an arbitrary source rate to 48 kHz.
struct StreamResampler {
    inner: Async<f32>,
    /// Number of input frames the resampler wants per call (FixedAsync::Input).
    chunk: usize,
    ratio: f64,
    pending: Vec<f32>,
    /// Resampler group delay, trimmed from the head so positions/timestamps
    /// stay honest and playback does not start with a silent gap.
    delay_frames: usize,
}

impl StreamResampler {
    fn new(source_rate: u32) -> Result<Self> {
        let ratio = TARGET_RATE as f64 / source_rate.max(1) as f64;
        const SINC_LEN: usize = 128;
        const OVERSAMPLING: usize = 128;
        let f_cutoff: f32 = calculate_cutoff(SINC_LEN, WindowFunction::BlackmanHarris2);
        let params = SincInterpolationParameters {
            sinc_len: SINC_LEN,
            f_cutoff,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: OVERSAMPLING,
            window: WindowFunction::BlackmanHarris2,
        };
        let inner = Async::<f32>::new_sinc(
            ratio,
            1.1,
            &params,
            RESAMPLE_CHUNK_FRAMES,
            TARGET_CHANNELS,
            FixedAsync::Input,
        )
        .map_err(|error| {
            anyhow!("无法初始化重采样器（{source_rate} Hz → {TARGET_RATE} Hz）：{error}")
        })?;
        let chunk = inner.input_frames_next();
        let delay_frames = inner.output_delay();
        Ok(Self {
            inner,
            chunk,
            ratio,
            pending: Vec::with_capacity(RESAMPLE_CHUNK_FRAMES * TARGET_CHANNELS),
            delay_frames,
        })
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        while self.pending.len() >= self.chunk * TARGET_CHANNELS {
            self.process_chunk(&mut *out)?;
        }
        Ok(())
    }

    fn process_chunk(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let frames = (self.pending.len() / TARGET_CHANNELS).min(self.chunk);
        let adapter = InterleavedSlice::new(
            &self.pending[..frames * TARGET_CHANNELS],
            TARGET_CHANNELS,
            frames,
        )
        .map_err(|error| anyhow!("重采样输入缓冲无效：{error}"))?;
        let translated = self
            .inner
            .process(&adapter, 0, None)
            .map_err(|error| anyhow!("重采样失败（{frames} 帧）：{error}"))?;
        let mut data = translated.take_data();
        self.trim_delay(&mut data);
        out.extend_from_slice(&data);
        self.pending.drain(..frames * TARGET_CHANNELS);
        Ok(())
    }

    /// Flush the buffered tail: zero-pad the final partial chunk, resample it,
    /// and keep only the frames that correspond to real input.
    fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let frames = self.pending.len() / TARGET_CHANNELS;
        if frames == 0 {
            return Ok(());
        }
        let mut padded = vec![0.0f32; self.chunk * TARGET_CHANNELS];
        padded[..frames * TARGET_CHANNELS].copy_from_slice(&self.pending);
        let adapter = InterleavedSlice::new(&padded, TARGET_CHANNELS, self.chunk)
            .map_err(|error| anyhow!("重采样尾部缓冲无效：{error}"))?;
        let translated = self
            .inner
            .process(&adapter, 0, None)
            .map_err(|error| anyhow!("重采样尾部刷新失败：{error}"))?;
        let mut data = translated.take_data();
        self.trim_delay(&mut data);
        let keep_frames = ((frames as f64) * self.ratio).ceil() as usize;
        let keep_samples = (keep_frames * TARGET_CHANNELS).min(data.len());
        out.extend_from_slice(&data[..keep_samples]);
        self.pending.clear();
        Ok(())
    }

    fn trim_delay(&mut self, data: &mut Vec<f32>) {
        if self.delay_frames == 0 {
            return;
        }
        let drop_frames = self.delay_frames.min(data.len() / TARGET_CHANNELS);
        data.drain(..drop_frames * TARGET_CHANNELS);
        self.delay_frames -= drop_frames;
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str) -> DlnaTrack<'_> {
        DlnaTrack { uri, ..DlnaTrack::default() }
    }

    fn validate(uri: &str) -> Result<()> {
        validate_track(&track(uri))
    }

    #[test]
    fn rejects_https_and_unsupported_schemes_explicitly() {
        let https = validate("https://example.com/a.mp3").unwrap_err().to_string();
        assert!(https.contains("HTTPS"), "got: {https}");
        let ftp = validate("ftp://example.com/a.mp3").unwrap_err().to_string();
        assert!(ftp.contains("不支持"), "got: {ftp}");
        let rtsp = validate("rtsp://example.com/a.mp3").unwrap_err().to_string();
        assert!(rtsp.contains("不支持"), "got: {rtsp}");
        let empty = validate("   ").unwrap_err().to_string();
        assert!(empty.contains("为空"), "got: {empty}");
        let relative = validate("media/a.mp3").unwrap_err().to_string();
        assert!(relative.contains("不支持"), "got: {relative}");
    }

    #[test]
    fn accepts_netease_flv_audio_and_ogg() {
        let mut flv = track("http://127.0.0.1:1/music/stream?id=42");
        flv.content_type = Some("video/flv");
        validate_track(&flv).expect("video/flv must be treated as audio");

        let mut video_class_flv = track("http://127.0.0.1:1/music/stream.flv");
        video_class_flv.media_kind = "video";
        validate_track(&video_class_flv)
            .expect("videoItem FLV audio must pass validation");

        validate("http://127.0.0.1:1/song.ogg").expect("OGG audio is supported");
    }

    #[test]
    fn rejects_hls_and_video_with_actionable_reasons() {
        let hls = validate("http://127.0.0.1:1/live.m3u8").unwrap_err().to_string();
        assert!(hls.contains("HLS"), "got: {hls}");
        let mut video_track = track("http://127.0.0.1:1/movie.mp4");
        video_track.content_type = Some("video/mp4");
        let video = validate_track(&video_track).unwrap_err().to_string();
        assert!(video.contains("视频"), "got: {video}");
        let mut class_video = track("http://127.0.0.1:1/item");
        class_video.upnp_class = Some("object.item.videoItem");
        let video = validate_track(&class_video).unwrap_err().to_string();
        assert!(video.contains("视频"), "got: {video}");
    }

    #[test]
    fn parses_http_urls_and_rejects_malformed_ones() {
        let url = parse_http_url("http://192.168.1.2:8080/music/a%20b.mp3?token=1#frag").unwrap();
        assert_eq!(url.host, "192.168.1.2");
        assert_eq!(url.port, 8080);
        assert_eq!(url.request_target, "/music/a%20b.mp3?token=1");
        assert_eq!(url.host_header(), "192.168.1.2:8080");

        let default_port = parse_http_url("http://nas").unwrap();
        assert_eq!(default_port.port, 80);
        assert_eq!(default_port.request_target, "/");

        assert!(parse_http_url("http:///no-host/a.mp3").is_err());
        assert!(parse_http_url("http://nas:notaport/a.mp3").is_err());
        assert!(parse_http_url("http://user@nas/a.mp3").is_err());
    }

    #[test]
    fn resolves_relative_redirects() {
        let base = parse_http_url("http://nas:8200/music/list/a.mp3").unwrap();
        let absolute = resolve_redirect(&base, "/media/b.mp3").unwrap();
        assert_eq!(absolute.request_target, "/media/b.mp3");
        assert_eq!(absolute.port, 8200);
        let relative = resolve_redirect(&base, "b.mp3").unwrap();
        assert_eq!(relative.request_target, "/music/list/b.mp3");
        let full = resolve_redirect(&base, "http://other:9000/c.mp3").unwrap();
        assert_eq!(full.authority, "other:9000");
    }

    #[test]
    fn percent_decodes_file_names() {
        assert_eq!(percent_decode("a%20b%2Fc.wav").unwrap(), "a b/c.wav");
        assert!(percent_decode("bad%2").is_err());
        assert_eq!(uri_file_name("http://nas/a%20b.wav?x=1"), Some("a b.wav".to_string()));
        assert_eq!(uri_file_name("http://nas/"), None);
    }

    #[test]
    fn downmix_duplicates_mono_without_dropping_channels() {
        let mono = Channels::Positioned(Position::FRONT_LEFT);
        let downmix = Downmix::new(&mono, 1);
        assert!(!downmix.is_direct());
        let [left, right] = downmix.mix(&[0.5]);
        assert!((left - 0.5).abs() < 1e-6, "left={left}");
        assert!((right - 0.5).abs() < 1e-6, "right={right}");
    }

    #[test]
    fn downmix_passes_stereo_through_untouched() {
        let stereo = Channels::Positioned(Position::FRONT_LEFT | Position::FRONT_RIGHT);
        let downmix = Downmix::new(&stereo, 2);
        assert!(downmix.is_direct());
        assert_eq!(downmix.mix(&[0.25, -0.5]), [0.25, -0.5]);
    }

    #[test]
    fn downmix_mixes_centre_surround_and_lfe_into_stereo() {
        let layout = Position::FRONT_LEFT
            | Position::FRONT_RIGHT
            | Position::FRONT_CENTER
            | Position::LFE1
            | Position::REAR_LEFT
            | Position::REAR_RIGHT;
        let downmix = Downmix::new(&Channels::Positioned(layout), 6);
        // Front-left only must not leak into the right channel.
        let [left, right] = downmix.mix(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(left > 0.2, "left={left}");
        assert!(right.abs() < 1e-6, "right={right}");
        // A centre-only frame must reach both speakers equally.
        let [left, right] = downmix.mix(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(left > 0.1 && right > 0.1, "L={left} R={right}");
        assert!((left - right).abs() < 1e-6);
        // No full-scale frame may clip after mixing.
        let [left, right] = downmix.mix(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert!(left <= 1.0 && right <= 1.0);
        assert!(left > 0.3 && right > 0.3);
    }

    #[test]
    fn downmix_fallback_uses_all_channels_for_unknown_layouts() {
        let downmix = Downmix::new(&Channels::Discrete(6), 6);
        assert!(!downmix.is_direct());
        let [left, right] = downmix.mix(&[0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
        assert!(left > 0.2 && right > 0.2, "L={left} R={right}");
        assert!((left - right).abs() < 1e-6);
    }
}
