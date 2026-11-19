//! DLNA media playback for the Windows host.
//!
//! `airplay-receiver-core` owns the DLNA **control plane** (SSDP + AVTransport)
//! and emits `CoreEvent::DlnaMedia { url, .. }` when a control point starts a
//! track. This module turns that URI into PCM for the unified engine:
//!
//! ```text
//! http:// /file://  --stream-->  symphonia probe/decode  --+-->  downmix to
//!                                                          |      stereo
//!                                                          +-->  resample
//!                                                                48 kHz f32
//!                                                                 |
//!                                                                 v
//!                                              AudioEngine::write_dlna_pcm(Dlna)
//! ```
//!
//! # Supported matrix
//!
//! | Input                           | Status |
//! |---------------------------------|--------|
//! | `http://` + `Content-Length`    | supported, streamed |
//! | `http://` + `Transfer-Encoding: chunked` | supported, streamed |
//! | `http://` connection-close body | supported, streamed |
//! | `http://` / `https://` 301/302/303/307/308 | followed (max [`MAX_REDIRECTS`], scheme switches max [`MAX_SCHEME_SWITCHES`]) |
//! | `https://`                      | supported (rustls + webpki roots, certificate checked) |
//! | HTTP `Range: bytes=`            | supported (`206` -> byte seek, `200` -> sequential fallback) |
//! | `file://` local file            | supported, seekable |
//! | HLS (`m3u8`/`m3u`, `*mpegurl`)  | **not supported** (explicit error) |
//! | video tracks / `upnp:class` video | **not supported** (explicit error) |
//! | `video/flv` audio (NetEase Cloud Music) | supported (FLV AAC **and** MP3 demuxed) |
//! | 401/403 from a CDN | retried once with the legacy `FusionPlay-<platform>/<ver>` UA |
//! | mp3 / flac / wav / aac / ogg / alac / aiff / mp4 audio | supported containers |
//!
//! Downloading is fully streaming: the HTTP body is decoded (content-length,
//! chunked, or until-close) as the decoder pulls bytes, so a multi-GB file never
//! lands in RAM. The fixed buffering is bounded by [`STREAM_BUFFER_BYTES`].
//!
//! # Error handling contract
//!
//! * Empty/illegal URIs, HLS and video are rejected **synchronously** by
//!   [`DlnaPlayer::play_track`] before any network or audio activity; the
//!   caller maps the `Err` into `lastError`. `http://` and `https://` media are
//!   both accepted (TLS certificates are always checked).
//! * Background failures (timeouts, truncated bodies, non-200 responses,
//!   unsupported codecs) are reported through [`DlnaEvent::Error`] and always
//!   finish with `Playback { playing: false }` + `Stopped`.
//! * Corrupt packets are skipped; after [`MAX_CONSECUTIVE_DECODE_ERRORS`]
//!   consecutive failures decoding aborts with an error.
//! * When another source (AirPlay/MiPlay) becomes active, or the core releases
//!   the DLNA lease, decoding stops promptly (checked before every socket read
//!   and every packet) and queued PCM is flushed.
//!
//! # Threading
//!
//! [`DlnaPlayer::play_track`] validates and returns immediately; all
//! network/decode work happens on a dedicated `fusionplay-dlna` thread.
//! [`DlnaPlayer::stop`] raises the stop token and returns immediately; the
//! worker aborts within one socket-poll interval (< 250 ms).
//!
//! # Seeking
//!
//! [`DlnaPlayer::seek`] posts a request to the decode worker. Formats whose
//! container reports seekability get an in-place Symphonia time seek
//! (`seek_mode=format`); otherwise the worker reopens the HTTP stream with
//! `Range: bytes=<approx>-` (`seek_mode=range`, byte offset estimated from the
//! duration and content length) or, without Range support, discards decoded
//! audio (`seek_mode=discard`). All three paths are logged with `tracing`.
//! P2-1: the sender-driven Stop/SetURI/Play contract is unchanged and
//! auto-advance stays off (the host has no next URI).

use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
#[path = "../../../src/shared_audio/stereo.rs"]
mod stereo;
use stereo::Converter;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoderOptions, well_known::{CODEC_ID_AAC, CODEC_ID_MP3},
};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration as MediaDuration, Time, Timestamp};

use crate::audio::{AudioEngine, TARGET_CHANNELS, TARGET_RATE};
use crate::session::Source;

/// Default connect/read inactivity timeout for DLNA HTTP transfers.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound for the TCP connect phase (kept independent from the read
/// timeout so "host unreachable" fails much faster than a stalled body read).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Socket read/write poll interval. Keeping it short means `stop()` aborts a
/// blocked read quickly; the overall inactivity deadline is separate.
const READ_POLL: Duration = Duration::from_millis(200);

/// Maximum number of HTTP redirects followed for one media URI.
const MAX_REDIRECTS: usize = 4;

/// How many redirects may also switch between `http://` and `https://`.
const MAX_SCHEME_SWITCHES: usize = 2;

/// Browser-like User-Agent for DLNA media pulls.
///
/// Several Chinese music CDNs (notably QQ Music) answer non-browser clients
/// with 403, while NetEase CDNs accept the generic client string. Chrome's UA
/// is accepted by all of them, so it is the default; deployments that need a
/// custom string can set `FUSIONPLAY_DLNA_USER_AGENT`.
///
/// A CDN may still reject the browser UA with 401/403. In that case the pull is
/// retried once with the legacy `FusionPlay-Windows/<version>` UA (the string
/// the 1.0 clients used), which several CDNs whitelist. That fallback UA can be
/// overridden with `FUSIONPLAY_DLNA_FALLBACK_USER_AGENT`.
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                                 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Legacy User-Agent used for the single 401/403 retry, see
/// [`DEFAULT_USER_AGENT`].
const LEGACY_USER_AGENT: &str = concat!("FusionPlay-Windows/", env!("CARGO_PKG_VERSION"));

/// HTTP header parsing limits (defensive against broken/malicious servers).
const MAX_HEADER_LINE: usize = 8 * 1024;
const MAX_HEADER_COUNT: usize = 128;

/// Fixed `MediaSourceStream` ring size. This is the only non-trivial download
/// buffer: the body is pulled through it on demand, never accumulated.
const STREAM_BUFFER_BYTES: usize = 256 * 1024;

/// Hard memory budget for the streaming path.
const MAX_STREAM_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const _: () = assert!(STREAM_BUFFER_BYTES + 1024 * 1024 <= MAX_STREAM_MEMORY_BYTES);

/// Consecutive packet decode failures tolerated before aborting the track.
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 32;

/// How often progress is published during playback (UI-friendly).
const STATE_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Metadata attached to a DLNA media start. Mirrors the fields of
/// `fusionplay_core::CoreEvent::DlnaMedia` so the event can be forwarded
/// without losing title/duration information.
#[derive(Debug, Clone, Default)]
pub struct DlnaTrack {
    /// Core playback epoch; echoes back in every [`DlnaEvent`].
    pub epoch: u64,
    pub url: String,
    pub media_kind: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub content_type: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub duration_ms: Option<u64>,
    pub start_position_ms: u64,
}

/// Playback notification emitted by [`DlnaPlayer`]. `Debug`/`Clone` so the
/// protocol host can clone it while dispatching to several consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlnaEvent {
    /// Position update (~4 Hz while playing, once at start).
    Progress {
        epoch: u64,
        position_ms: u64,
        duration_ms: u64,
    },
    /// Playback started (`playing = true`) or ended (`false`).
    Playback { epoch: u64, playing: bool },
    /// Background failure (network/decode). Synchronous validation errors are
    /// returned by [`DlnaPlayer::play_track`] instead.
    Error { message: String },
    /// The worker finished (completed, stopped, taken over or failed). Always
    /// the last event of an epoch, except when the playback was superseded by a
    /// newer track, in which case no terminal event is emitted. A seek does not
    /// emit `Stopped`; playback continues in the same epoch.
    Stopped { epoch: u64 },
}

/// Non-blocking event callback (`Arc<dyn Fn(DlnaEvent) + Send + Sync>`).
pub type DlnaEventSink = Arc<dyn Fn(DlnaEvent) + Send + Sync>;

struct CurrentPlayback {
    token: Arc<AtomicBool>,
    /// Pending seek request applied by the decode worker.
    seek: Arc<Mutex<Option<u64>>>,
    /// Set when a newer playback supersedes this one (new track): the worker
    /// must not emit terminal events for the abandoned epoch.
    abandoned: Arc<AtomicBool>,
    /// Cleared by the worker when it exits.
    playing: Arc<AtomicBool>,
}

/// Host-side DLNA player: HTTP pull + symphonia decode into the unified engine.
pub struct DlnaPlayer {
    engine: Arc<AudioEngine>,
    emit: DlnaEventSink,
    current: Mutex<Option<CurrentPlayback>>,
    /// Linear volume applied when PCM is handed to the engine (masked to 0 on
    /// mute). Stored as `f32` bits so it can be read from the decode thread.
    volume_gain_bits: Arc<AtomicU32>,
    timeout_ms: AtomicU64,
    /// P2-1: opt-in auto-advance hook. The 1.3.x DLNA control plane is
    /// sender-driven (Stop/SetAVTransportURI/Play) and the host has no next
    /// URI, so this defaults to off and only changes the end-of-stream log.
    auto_advance: AtomicBool,
}

impl DlnaPlayer {
    pub fn new(engine: Arc<AudioEngine>, emit: DlnaEventSink) -> Arc<Self> {
        Arc::new(Self {
            engine,
            emit,
            current: Mutex::new(None),
            volume_gain_bits: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            timeout_ms: AtomicU64::new(DEFAULT_TIMEOUT.as_millis() as u64),
            auto_advance: AtomicBool::new(false),
        })
    }

    /// Override the HTTP connect/read inactivity timeout (tests, slow NAS).
    pub fn set_timeout(&self, timeout: Duration) {
        self.timeout_ms
            .store(timeout.as_millis().max(1) as u64, Ordering::Release);
    }

    /// Whether a track is currently being decoded/played by the worker.
    pub fn is_playing(&self) -> bool {
        self.current
            .lock()
            .map(|guard| {
                guard
                    .as_ref()
                    .is_some_and(|current| current.playing.load(Ordering::Acquire))
            })
            .unwrap_or(false)
    }

    /// Start playback. Validates the URI/format synchronously (unsupported
    /// media returns `Err` before any state change) and returns immediately;
    /// network + decoding happen on a background thread.
    pub fn play_track(&self, track: DlnaTrack) -> Result<()> {
        validate_track(&track)?;
        let epoch = track.epoch;
        self.supersede_current();
        self.launch(track, epoch)
    }

    /// Stop the currently playing track. Safe to call from any thread; the
    /// worker aborts within one socket-poll interval.
    pub fn stop(&self) {
        let token = self
            .current
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|current| Arc::clone(&current.token)));
        if let Some(token) = token {
            token.store(true, Ordering::Release);
        }
    }

    /// Request a seek while playing. Returns immediately; the decode worker
    /// applies it (Symphonia time seek when the format is seekable, else a
    /// Range-based reopen, else discard-from-start fallback).
    pub fn seek(&self, position_ms: u64) -> Result<()> {
        let guard = self
            .current
            .lock()
            .map_err(|_| anyhow!("DLNA 播放器状态异常，无法跳转"))?;
        let Some(current) = guard.as_ref() else {
            bail!("当前没有正在播放的 DLNA 媒体，无法跳转");
        };
        if let Ok(mut slot) = current.seek.lock() {
            *slot = Some(position_ms);
        }
        Ok(())
    }

    /// P2-1: opt-in auto-advance hook (default **off**). The host has no next
    /// URI in the 1.3.x control plane, so enabling this only changes the
    /// end-of-stream log; sender-driven Stop/SetURI/Play ordering is unchanged.
    pub fn set_auto_advance(&self, enabled: bool) {
        self.auto_advance.store(enabled, Ordering::Release);
    }

    /// Set the DLNA sender volume (0..=100 %) and mute flag. Applied to PCM as
    /// it is written to the engine; already queued audio keeps its gain.
    pub fn set_volume(&self, percent: u8, muted: bool) {
        let gain = if muted {
            0.0
        } else {
            percent.min(100) as f32 / 100.0
        };
        self.volume_gain_bits
            .store(gain.to_bits(), Ordering::Release);
    }

    fn supersede_current(&self) {
        let previous = self
            .current
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(previous) = previous {
            previous.abandoned.store(true, Ordering::Release);
            previous.token.store(true, Ordering::Release);
        }
    }

    fn launch(&self, track: DlnaTrack, epoch: u64) -> Result<()> {
        let token = Arc::new(AtomicBool::new(false));
        let seek = Arc::new(Mutex::new(None));
        let abandoned = Arc::new(AtomicBool::new(false));
        let playing = Arc::new(AtomicBool::new(false));
        let track = Arc::new(track);
        {
            let mut guard = self
                .current
                .lock()
                .map_err(|_| anyhow!("DLNA 播放器状态异常"))?;
            *guard = Some(CurrentPlayback {
                token: Arc::clone(&token),
                seek: Arc::clone(&seek),
                abandoned: Arc::clone(&abandoned),
                playing: Arc::clone(&playing),
            });
        }

        let engine = Arc::clone(&self.engine);
        let emit = Arc::clone(&self.emit);
        let gain_bits = Arc::clone(&self.volume_gain_bits);
        let timeout = Duration::from_millis(self.timeout_ms.load(Ordering::Acquire).max(1));
        let worker_playing = Arc::clone(&playing);
        let worker_token = Arc::clone(&token);
        let worker_abandoned = Arc::clone(&abandoned);
        let worker_seek = Arc::clone(&seek);
        let spawn = std::thread::Builder::new()
            .name("fusionplay-dlna".to_owned())
            .spawn(move || {
                let result = run_track(
                    &engine,
                    &emit,
                    &worker_token,
                    &worker_abandoned,
                    &worker_playing,
                    epoch,
                    &track,
                    &gain_bits,
                    &worker_seek,
                    timeout,
                );
                worker_playing.store(false, Ordering::Release);
                let _ = result;
            });
        if let Err(error) = spawn {
            playing.store(false, Ordering::Release);
            abandoned.store(true, Ordering::Release);
            token.store(true, Ordering::Release);
            if let Ok(mut guard) = self.current.lock()
                && guard
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(&current.token, &token))
            {
                *guard = None;
            }
            return Err(anyhow!("无法启动 DLNA 播放线程：{error}"));
        }
        Ok(())
    }
}

impl Drop for DlnaPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_track(
    engine: &Arc<AudioEngine>,
    emit: &DlnaEventSink,
    stop: &Arc<AtomicBool>,
    abandoned: &Arc<AtomicBool>,
    playing: &Arc<AtomicBool>,
    epoch: u64,
    track: &DlnaTrack,
    gain_bits: &Arc<AtomicU32>,
    seek_slot: &Arc<Mutex<Option<u64>>>,
    timeout: Duration,
) -> Result<PlaySummary> {
    let abort = Abort {
        stop: Arc::clone(stop),
        engine: Arc::clone(engine),
    };
    emit(DlnaEvent::Progress {
        epoch,
        position_ms: track.start_position_ms,
        duration_ms: track.duration_ms.unwrap_or(0),
    });

    // A seek the current format/source cannot apply in place returns
    // `SeekFallback`; this loop then reopens the stream at a byte range (server
    // supports Range) or discards decoded audio from 0.
    let mut request = PlaybackRequest {
        base_ms: track.start_position_ms,
        skip_ms: track.start_position_ms,
        mode: "initial",
    };
    let mut open_at: Option<SeekTarget> = None;
    let mut seek_reopens = 0usize;
    let result: Result<PlaySummary> = loop {
        if abort.aborted() {
            break Err(anyhow!("DLNA 播放已中止"));
        }
        let opened = match open_at {
            Some(target) => open_source_at(&track.url, timeout, &abort, Some(target)),
            None => open_source(&track.url, timeout, &abort),
        };
        let outcome = match opened {
            Ok(opened) => decode_and_play(
                engine, emit, &abort, playing, epoch, track, gain_bits, opened, request,
                seek_slot,
            ),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(DecodeOutcome::Finished {
                summary,
                position_base_ms,
            }) => {
                emit_progress(emit, epoch, track, &summary, position_base_ms);
                break Ok(summary);
            }
            Ok(DecodeOutcome::SeekFallback {
                target_ms,
                byte_seekable,
                content_length,
                duration_ms,
            }) => {
                seek_reopens += 1;
                if seek_reopens > MAX_SEEK_REOPENS {
                    break Err(anyhow!(
                        "DLNA seek 失败：重开流次数超过 {MAX_SEEK_REOPENS} 次"
                    ));
                }
                // Drop audio queued before the seek before reopening the source.
                engine.flush_source(Source::Dlna);
                let duration = track.duration_ms.or(duration_ms);
                let target = if byte_seekable {
                    estimate_seek_target(duration, content_length, target_ms)
                } else {
                    None
                };
                match target {
                    Some(target) => {
                        tracing::info!(
                            target: "dlna",
                            seek_mode = "range",
                            position_ms = target_ms,
                            byte = target.byte,
                            "DLNA seek 使用 Range 重开流"
                        );
                        request = PlaybackRequest {
                            // Progress is "audible media time": the seek target
                            // plus whatever plays after it.
                            base_ms: target_ms,
                            skip_ms: target_ms.saturating_sub(target.time_ms),
                            mode: "seek_range",
                        };
                        open_at = Some(target);
                    }
                    None => {
                        tracing::info!(
                            target: "dlna",
                            seek_mode = "discard",
                            position_ms = target_ms,
                            "DLNA seek 回退为从头解码丢弃（服务器不支持 Range 或时长未知）"
                        );
                        request = PlaybackRequest {
                            base_ms: target_ms,
                            skip_ms: target_ms,
                            mode: "seek_discard",
                        };
                        open_at = None;
                    }
                }
                continue;
            }
            Err(error) => break Err(error),
        }
    };

    // Clear the playing flag before publishing the final events: observers that
    // react to `Stopped` must already see `is_playing() == false`.
    playing.store(false, Ordering::Release);
    let superseded = abandoned.load(Ordering::Acquire);
    if !superseded {
        if let Err(error) = &result {
            emit(DlnaEvent::Error {
                message: format!("DLNA 播放失败（{}）：{error:#}", track.url),
            });
        } else {
            // P2-1: the 1.3.x DLNA control plane is sender-driven
            // (Stop/SetURI/Play); the host has no next URI, so auto-advance
            // stays off and this log documents the end-of-stream contract.
            tracing::info!(
                target: "dlna",
                auto_advance = false,
                "DLNA 播放完成（auto_advance=false；控制面由发送端 Next/SetURI 驱动，不自动切歌）"
            );
        }
        // Only flush while this source still owns the engine: a newer source's
        // freshly queued PCM must never be dropped.
        if engine.active_source() == Some(Source::Dlna) {
            engine.flush_source(Source::Dlna);
        }
        emit(DlnaEvent::Playback {
            epoch,
            playing: false,
        });
        emit(DlnaEvent::Stopped { epoch });
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn decode_and_play(
    engine: &Arc<AudioEngine>,
    emit: &DlnaEventSink,
    abort: &Abort,
    playing: &AtomicBool,
    epoch: u64,
    track: &DlnaTrack,
    gain_bits: &Arc<AtomicU32>,
    opened: OpenedMedia,
    request: PlaybackRequest,
    seek_slot: &Arc<Mutex<Option<u64>>>,
) -> Result<DecodeOutcome> {
    let OpenedMedia {
        source,
        bytes,
        content_type,
        start_ms,
        content_length,
        byte_seekable,
    } = opened;

    // Sniff the container before symphonia probes it: NetEase Cloud Music
    // serves audio-only DLNA casts as `video/flv`, which symphonia cannot
    // demux. Reading a few bytes up-front costs nothing and keeps the original
    // extension/MIME hints working for every other container.
    let (prefix, source) = read_prefix(source)?;
    let source: Box<dyn MediaSource> =
        Box::new(PrefixedMediaSource::new(prefix.clone(), source));
    if fusionplay_core::dlna_flv::is_flv(&prefix) {
        return decode_flv_and_play(
            engine,
            emit,
            abort,
            playing,
            epoch,
            track,
            gain_bits,
            source,
            bytes,
            request,
            seek_slot,
        );
    }

    let mut hint = Hint::new();
    if let Some(extension) = uri_extension(&track.url) {
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
        MediaSourceStreamOptions {
            buffer_len: STREAM_BUFFER_BYTES,
        },
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
                "无法识别 DLNA 媒体格式（支持 mp3/flac/wav/aac/ogg/alac/aiff/mp4 音频与 FLV AAC/MP3 音频；不支持 HLS/视频）：{error}"
            )
        })?;

    let selected = format
        .default_track(TrackType::Audio)
        .context("媒体中没有可解码的音频轨道（可能是纯视频或 HLS 流）")?;
    let track_id = selected.id;
    let track_time_base = selected.time_base;
    let selected_num_frames = selected.num_frames;
    let params = match selected.codec_params.clone() {
        Some(CodecParameters::Audio(params)) => params,
        Some(_) => bail!("所选轨道不是音频轨道（DLNA 视频暂不支持）"),
        None => bail!("音频轨道缺少解码参数（编解码器可能不受支持）"),
    };
    let probed_duration_ms = selected_num_frames
        .zip(params.sample_rate)
        .map(|(frames, rate)| frames.saturating_mul(1_000) / rate.max(1) as u64);

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .context("无法创建音频解码器（编解码器可能不受支持）")?;
    tracing::info!(
        target: "dlna",
        codec = decoder.codec_info().short_name,
        sample_rate = params.sample_rate.unwrap_or(0),
        channels = params.channels.as_ref().map(Channels::count).unwrap_or(0),
        "DLNA 解码器已创建"
    );

    let mut summary = PlaySummary {
        source_rate: params.sample_rate.unwrap_or(0),
        duration_ms: track.duration_ms.unwrap_or_else(|| {
            selected_num_frames
                .zip(params.sample_rate)
                .map(|(frames, rate)| frames.saturating_mul(1_000) / rate.max(1) as u64)
                .unwrap_or(0)
        }),
        ..PlaySummary::default()
    };

    let mut converter: Option<Converter> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut pcm: Vec<f32> = Vec::new();
    let mut consecutive_errors = 0u32;
    let mut last_state_update = Instant::now();
    let mut announced_playing = false;
    let mut position_base_ms = request.base_ms;
    let mut skip_frames =
        request.skip_ms.saturating_mul(TARGET_RATE as u64) / 1_000;
    if request.mode != "initial" {
        tracing::info!(
            target: "dlna",
            seek_mode = request.mode,
            base_ms = position_base_ms,
            skip_ms = request.skip_ms,
            opened_start_ms = start_ms,
            "DLNA 流已按 seek 重新打开"
        );
    }

    loop {
        if abort.aborted() {
            break;
        }
        if let Some(target_ms) = take_pending_seek(seek_slot) {
            match format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_millis_u64(target_ms),
                    track_id: Some(track_id),
                },
            ) {
                Ok(seeked) => {
                    let actual_ms = track_time_base
                        .and_then(|time_base| time_base.calc_time(seeked.actual_ts))
                        .map(|time| time.as_millis().max(0) as u64)
                        .unwrap_or(target_ms);
                    // The old stream position is gone: reset decoder and
                    // resampler state and drop audio queued before the seek.
                    decoder.reset();
                    converter = None;
                    engine.flush_source(Source::Dlna);
                    summary.decoded_frames = 0;
                    summary.output_frames = 0;
                    position_base_ms = actual_ms;
                    skip_frames = 0;
                    announced_playing = true;
                    tracing::info!(
                        target: "dlna",
                        seek_mode = "format",
                        position_ms = target_ms,
                        actual_ms,
                        "DLNA seek 使用格式时间 seek"
                    );
                    emit_progress(emit, epoch, track, &summary, position_base_ms);
                    continue;
                }
                Err(error) => {
                    tracing::info!(
                        target: "dlna",
                        seek_mode = "fallback",
                        position_ms = target_ms,
                        error = %error,
                        "DLNA 格式不支持时间 seek，回退重开流"
                    );
                    return Ok(DecodeOutcome::SeekFallback {
                        target_ms,
                        byte_seekable,
                        content_length,
                        duration_ms: track.duration_ms.or(probed_duration_ms),
                    });
                }
            }
        }

        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(error))
                if error.kind() == io::ErrorKind::UnexpectedEof =>
            {
                // Containers commonly signal a clean end-of-stream this way.
                break;
            }
            Err(SymphoniaError::IoError(error))
                if error.kind() == io::ErrorKind::Interrupted && abort.aborted() =>
            {
                break;
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
        }

        interleaved.resize(decoded.samples_interleaved(), 0.0);
        decoded.copy_to_slice_interleaved(interleaved.as_mut_slice());
        summary.decoded_frames += (interleaved.len() / planes) as u64;

        pcm.clear();
        let converter = converter.as_mut().expect("converter is initialised above");
        converter.convert(&interleaved, &mut pcm)?;
        if pcm.is_empty() {
            continue;
        }
        if abort.aborted() {
            break;
        }

        // Drop decoded audio before the requested start/seek position.
        if skip_frames > 0 {
            let frames = (pcm.len() / TARGET_CHANNELS as usize) as u64;
            if frames <= skip_frames {
                skip_frames -= frames;
                continue;
            }
            let offset = (skip_frames as usize) * TARGET_CHANNELS as usize;
            pcm.drain(..offset);
            skip_frames = 0;
        }

        if !announced_playing {
            announced_playing = true;
            playing.store(true, Ordering::Release);
            emit(DlnaEvent::Playback { epoch, playing: true });
        }
        summary.push_frames(engine, epoch, volume_gain(gain_bits), &pcm);
        if last_state_update.elapsed() >= STATE_UPDATE_INTERVAL {
            last_state_update = Instant::now();
            emit_progress(emit, epoch, track, &summary, position_base_ms);
        }
    }

    // Flush the resampler tail so the last ~ms of audio are not lost.
    if let Some(converter) = converter.as_mut() {
        pcm.clear();
        converter.finish(&mut pcm)?;
        if !pcm.is_empty() && !abort.aborted() && skip_frames == 0 {
            summary.push_frames(engine, epoch, volume_gain(gain_bits), &pcm);
        }
    }

    summary.bytes_read = bytes.load(Ordering::Relaxed);
    emit_progress(emit, epoch, track, &summary, position_base_ms);
    Ok(DecodeOutcome::Finished {
        summary,
        position_base_ms,
    })
}

    /// Host-local prefix replay wrapper for container sniffing.
///
/// The prefix is the *head* of the inner stream, so the stream length is the
/// inner length (a `SeekFrom::End` must not add the prefix again). Range-capable
/// HTTP sources therefore stay fully seekable through the wrapper, which is what
/// lets Symphonia read an MP4 `moov` atom that sits at the end of the file.
struct PrefixedMediaSource {
    prefix: Vec<u8>,
    offset: usize,
    position: u64,
    inner: Box<dyn MediaSource>,
}

impl PrefixedMediaSource {
    fn new(prefix: Vec<u8>, inner: Box<dyn MediaSource>) -> Self {
        Self {
            prefix,
            offset: 0,
            position: 0,
            inner,
        }
    }
}

impl Read for PrefixedMediaSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.offset < self.prefix.len() {
            let available = &self.prefix[self.offset..];
            let count = available.len().min(buffer.len());
            buffer[..count].copy_from_slice(&available[..count]);
            self.offset += count;
            self.position += count as u64;
            return Ok(count);
        }
        let count = self.inner.read(buffer)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for PrefixedMediaSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let prefix_len = self.prefix.len() as u64;
        let target = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(delta) => self
                .position
                .checked_add_signed(delta)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
            SeekFrom::End(delta) => {
                let length = self.inner.byte_len().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "stream length is unknown")
                })? as i64;
                length
                    .checked_add(delta)
                    .filter(|position| *position >= 0)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?
                    as u64
            }
        };
        self.inner.seek(SeekFrom::Start(target.max(prefix_len)))?;
        self.offset = target.min(prefix_len) as usize;
        self.position = target;
        Ok(target)
    }
}

impl MediaSource for PrefixedMediaSource {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
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

/// Decode an FLV audio stream (NetEase Cloud Music DLNA casts) into the
    /// engine.
    ///
    /// Both FLV sound formats are supported: AAC (SoundFormat 10) raw frames
    /// and MP3 (SoundFormat 2), where the core demuxer re-assembles MPEG audio
    /// frames that are split across FLV tags. FLV has no random access in this
    /// implementation, so a seek returns [`DecodeOutcome::SeekFallback`] (the
    /// caller discards decoded audio from the beginning).
    #[allow(clippy::too_many_arguments)]
    fn decode_flv_and_play(
        engine: &Arc<AudioEngine>,
        emit: &DlnaEventSink,
        abort: &Abort,
        playing: &AtomicBool,
        epoch: u64,
        track: &DlnaTrack,
        gain_bits: &Arc<AtomicU32>,
        source: Box<dyn MediaSource>,
        bytes: Arc<AtomicU64>,
        request: PlaybackRequest,
        seek_slot: &Arc<Mutex<Option<u64>>>,
    ) -> Result<DecodeOutcome> {
        let mut stream = fusionplay_core::dlna_flv::FlvAudioStream::new(source);
        let info = stream
            .info()
            .context("无法解析 FLV 音频流（网易云等音乐 App 的投屏容器）")?;
        let layout = info
            .channel_layout()
            .context("FLV 音频声道布局暂不支持（仅支持单声道/立体声）")?;
        let codec_id = match info.codec {
            fusionplay_core::dlna_flv::FlvAudioCodec::Aac => CODEC_ID_AAC,
            fusionplay_core::dlna_flv::FlvAudioCodec::Mp3 => CODEC_ID_MP3,
        };
        let mut parameters = AudioCodecParameters::new();
        parameters
            .for_codec(codec_id)
            .with_sample_rate(info.sample_rate)
            .with_channels(layout);
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&parameters, &AudioDecoderOptions::default())
            .with_context(|| format!("无法创建 FLV {} 解码器", info.codec.as_str()))?;
        tracing::info!(
            target: "dlna",
            container = "flv",
            codec = info.codec.as_str(),
            sample_rate = info.sample_rate,
            channels = info.channels,
            "DLNA 解码器已创建"
        );

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
        let position_base_ms = request.base_ms;
        let mut skip_frames =
            request.skip_ms.saturating_mul(TARGET_RATE as u64) / 1_000;
        if request.mode != "initial" {
            tracing::info!(
                target: "dlna",
                seek_mode = request.mode,
                base_ms = position_base_ms,
                skip_ms = request.skip_ms,
                "DLNA FLV 流已按 seek 重新打开"
            );
        }

        loop {
            if abort.aborted() {
                break;
            }
            if let Some(target_ms) = take_pending_seek(seek_slot) {
                tracing::info!(
                    target: "dlna",
                    seek_mode = "fallback",
                    position_ms = target_ms,
                    "DLNA FLV 不支持随机访问，回退重开流"
                );
                return Ok(DecodeOutcome::SeekFallback {
                    target_ms,
                    byte_seekable: false,
                    content_length: None,
                    duration_ms: track.duration_ms,
                });
            }
            let frame = match stream.next_frame() {
                Ok(Some(frame)) => frame,
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
                MediaDuration::new(u64::from(frame.samples)),
                frame.data,
            );
            timestamp = timestamp.saturating_add(u64::from(frame.samples));
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
            }

            interleaved.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(interleaved.as_mut_slice());
            // `planes` is clamped to >= 1 above.
            summary.decoded_frames += (interleaved.len() / planes) as u64;

            pcm.clear();
            let converter = converter.as_mut().expect("converter is initialised above");
            converter.convert(&interleaved, &mut pcm)?;
            if pcm.is_empty() {
                continue;
            }
            if abort.aborted() {
                break;
            }

            // Drop decoded audio before the requested start/seek position.
            if skip_frames > 0 {
                let frames = (pcm.len() / TARGET_CHANNELS as usize) as u64;
                if frames <= skip_frames {
                    skip_frames -= frames;
                    continue;
                }
                let offset = (skip_frames as usize) * TARGET_CHANNELS as usize;
                pcm.drain(..offset);
                skip_frames = 0;
            }

            summary.push_frames(engine, epoch, volume_gain(gain_bits), &pcm);

            if !announced_playing {
                announced_playing = true;
                playing.store(true, Ordering::Release);
                emit(DlnaEvent::Playback { epoch, playing: true });
            }
            if last_state_update.elapsed() >= STATE_UPDATE_INTERVAL {
                last_state_update = Instant::now();
                emit_progress(emit, epoch, track, &summary, position_base_ms);
            }
        }

        if let Some(converter) = converter.as_mut() {
            pcm.clear();
            converter.finish(&mut pcm)?;
            if !pcm.is_empty() && !abort.aborted() {
                summary.push_frames(engine, epoch, volume_gain(gain_bits), &pcm);
            }
        }

        summary.bytes_read = bytes.load(Ordering::Relaxed);
        emit_progress(emit, epoch, track, &summary, position_base_ms);
        Ok(DecodeOutcome::Finished { summary, position_base_ms })
    }

fn emit_progress(
    emit: &DlnaEventSink,
    epoch: u64,
    track: &DlnaTrack,
    summary: &PlaySummary,
    position_base_ms: u64,
) {
    let _ = track;
    emit(DlnaEvent::Progress {
        epoch,
        position_ms: summary.position_ms(position_base_ms),
        duration_ms: summary.duration_ms,
    });
}

fn volume_gain(bits: &AtomicU32) -> f32 {
    f32::from_bits(bits.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Playback bookkeeping
// ---------------------------------------------------------------------------

/// How the current stream was opened relative to the requested play position.
#[derive(Debug, Clone, Copy)]
struct PlaybackRequest {
    /// Media time (ms) corresponding to the first byte of the opened stream.
    base_ms: u64,
    /// Decoded output (ms) to discard before it becomes audible.
    skip_ms: u64,
    /// `"initial"` / `"seek_range"` / `"seek_discard"` (diagnostics only).
    mode: &'static str,
}

/// Result of one decode pass: natural end, or a seek that needs a reopened
/// stream because this format/source cannot seek in place.
enum DecodeOutcome {
    /// Decode reached the end of the stream. `position_base_ms` is the media
    /// time of the audible start **after** any in-place format seek, so callers
    /// report the correct final position.
    Finished {
        summary: PlaySummary,
        position_base_ms: u64,
    },
    SeekFallback {
        target_ms: u64,
        byte_seekable: bool,
        content_length: Option<u64>,
        duration_ms: Option<u64>,
    },
}

/// Maximum number of stream reopens caused by seeks in one playback.
const MAX_SEEK_REOPENS: usize = 2;

/// Take (and clear) a seek request posted by [`DlnaPlayer::seek`].
fn take_pending_seek(slot: &Mutex<Option<u64>>) -> Option<u64> {
    slot.lock().ok().and_then(|mut request| request.take())
}

/// Estimate a byte offset for a time-based seek, conservatively rounding down
/// so the decoder only ever has to discard a small remainder.
fn estimate_seek_target(
    duration_ms: Option<u64>,
    content_length: Option<u64>,
    target_ms: u64,
) -> Option<SeekTarget> {
    let duration = duration_ms.filter(|duration| *duration > 0)?;
    let length = content_length.filter(|length| *length > 0)?;
    let byte = ((length as u128) * (target_ms as u128) / (duration as u128)) as u64;
    let byte = byte.min(length.saturating_sub(1));
    if byte == 0 {
        return None;
    }
    let estimated_ms = ((byte as u128) * (duration as u128) / (length as u128)) as u64;
    Some(SeekTarget {
        byte,
        time_ms: estimated_ms.min(target_ms),
    })
}

#[derive(Default)]
struct PlaySummary {
    source_rate: u32,
    source_channels: usize,
    duration_ms: u64,
    decoded_frames: u64,
    output_frames: u64,
    skipped_packets: u64,
    bytes_read: u64,
}

impl PlaySummary {
    fn push_frames(&mut self, engine: &AudioEngine, epoch: u64, gain: f32, pcm: &[f32]) {
        if pcm.len() < TARGET_CHANNELS as usize {
            return;
        }
        self.output_frames += (pcm.len() / TARGET_CHANNELS as usize) as u64;
        engine.write_dlna_pcm(
            epoch,
            gain,
            TARGET_RATE,
            TARGET_CHANNELS,
            pcm,
        );
    }

    fn position_ms(&self, start_position_ms: u64) -> u64 {
        start_position_ms.saturating_add(
            self.output_frames.saturating_mul(1_000) / TARGET_RATE as u64,
        )
    }
}

// ---------------------------------------------------------------------------
// URI validation / display helpers
// ---------------------------------------------------------------------------

/// Validate scheme and reject unsupported media up-front with an explicit
/// reason (before any network/audio activity).
fn validate_track(track: &DlnaTrack) -> Result<()> {
    let uri = track.url.as_str();
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        bail!("DLNA 媒体地址为空");
    }
    if trimmed != uri {
        bail!("DLNA 媒体地址包含首尾空白：{uri}");
    }
    if uri
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("DLNA 媒体地址包含非法字符（空白/控制字符）：{uri}");
    }

    let lower = uri.to_ascii_lowercase();
    let Some(scheme_end) = lower.find("://") else {
        bail!("不支持的 DLNA 媒体地址（仅支持 http://、https:// 与 file://）：{uri}");
    };
    match &lower[..scheme_end] {
        "http" | "https" => {
            // TLS hostname/SNI validity is checked here, certificates at connect.
            parse_http_url(uri)?;
        }
        "file" => {
            file_path_from_uri(uri)?;
        }
        scheme => bail!(
            "不支持的 DLNA 媒体地址协议 {scheme}://（仅支持 http://、https:// 与 file://）：{uri}"
        ),
    }

    // HLS playlists are not media streams we can decode.
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let path_lower = path.to_ascii_lowercase();
    if path_lower.ends_with(".m3u8") || path_lower.ends_with(".m3u") {
        bail!("DLNA HLS 播放列表（m3u8/m3u）暂不支持，请使用音频文件直链：{uri}");
    }
    if let Some(content_type) = track.content_type.as_deref() {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.contains("mpegurl") {
            bail!("DLNA HLS 播放列表暂不支持，请使用音频文件直链：{uri}");
        }
        if content_type.starts_with("video/") && !is_flv_content_type(&content_type) {
            bail!(
                "DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/ogg/alac/aiff/mp4 音轨，以及网易云常用的 FLV 音频（AAC/MP3））：{uri}"
            );
        }
    }
    let flv_audio = path_lower.ends_with(".flv")
        || track
            .content_type
            .as_deref()
            .is_some_and(is_flv_content_type);
    if track.media_kind.eq_ignore_ascii_case("video") && !flv_audio {
        bail!(
            "DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/ogg/alac/aiff/mp4 音轨，以及网易云常用的 FLV 音频（AAC/MP3））：{uri}"
        );
    }
    Ok(())
}

/// File extension used as a probe hint (e.g. raw ADTS `.aac` files).
fn uri_extension(uri: &str) -> Option<String> {
    let path = uri.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    let extension = name.rsplit_once('.')?.1;
    if extension.is_empty()
        || extension.len() > 5
        || !extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
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

/// Where the bytes come from, plus the live download counter and the metadata
/// needed to implement HTTP range seeks.
struct OpenedMedia {
    source: Box<dyn MediaSource>,
    bytes: Arc<AtomicU64>,
    content_type: Option<String>,
    /// Media time (ms) the first byte of `source` corresponds to. Non-zero only
    /// when the stream was reopened at a byte offset for a seek fallback.
    start_ms: u64,
    /// Total resource length in bytes, when known.
    content_length: Option<u64>,
    /// Whether the source can be reopened at an arbitrary byte offset
    /// (`file://` or an HTTP server with `Range` support).
    byte_seekable: bool,
}

/// Byte offset + matching media time used to reopen a stream for a seek.
#[derive(Debug, Clone, Copy)]
struct SeekTarget {
    byte: u64,
    time_ms: u64,
}

fn open_source(uri: &str, timeout: Duration, abort: &Abort) -> Result<OpenedMedia> {
    open_source_at(uri, timeout, abort, None)
}

/// Open the media stream, optionally restarting the HTTP/file stream at
/// `at.byte` so a seek does not have to discard audio from the very beginning.
fn open_source_at(
    uri: &str,
    timeout: Duration,
    abort: &Abort,
    at: Option<SeekTarget>,
) -> Result<OpenedMedia> {
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        open_http(uri, timeout, abort, at)
    } else if lower.starts_with("file://") {
        open_file(uri, at)
    } else {
        bail!("不支持的 DLNA 媒体地址（仅支持 http://、https:// 与 file://）：{uri}");
    }
}

/// Extracts the part after `file://`, accepting `file://localhost/...`.
fn file_uri_rest(uri: &str) -> Result<&str> {
    let scheme_end = uri
        .find("://")
        .ok_or_else(|| anyhow!("非法 file:// 地址：{uri}"))?;
    let mut rest = &uri[scheme_end + 3..];
    // Accept both file:///path and file://localhost/path.
    if let Some(stripped) = rest.strip_prefix("localhost")
        && (stripped.is_empty() || stripped.starts_with('/'))
    {
        rest = stripped;
    }
    if rest.is_empty() {
        bail!("file:// 地址缺少路径：{uri}");
    }
    Ok(rest)
}

fn file_path_from_uri(uri: &str) -> Result<String> {
    #[cfg(windows)]
    {
        file_path_from_uri_windows(uri)
    }
    #[cfg(not(windows))]
    {
        let path = percent_decode(file_uri_rest(uri)?)?;
        if !path.starts_with('/') {
            bail!("file:// 必须是绝对路径：{uri}");
        }
        Ok(path)
    }
}

/// Windows spelling normalisation, kept platform-independent so it is unit
/// tested on every host (the windows-latest CI runs the same cases).
///
/// Accepted forms (all percent-decoded before this step):
///   `file://C:\\Media\\song.mp3`
///   `file://C:/Media/song.mp3`
///   `file:///C:/Media/song.mp3`
///   `file://server/share/song.mp3`   (UNC)
#[cfg_attr(not(windows), allow(dead_code))]
fn file_path_from_uri_windows(uri: &str) -> Result<String> {
    let mut path = percent_decode(file_uri_rest(uri)?)?.replace('\\', "/");
    // `file:////server/share` (double slash authority) is already UNC-shaped.
    if let Some(unc) = path.strip_prefix("//") {
        if unc.is_empty() {
            bail!("file:// 地址缺少服务器名：{uri}");
        }
        return Ok(format!("\\\\{}", unc.replace('/', "\\")));
    }
    let rooted_uri_path = path.starts_with('/');
    if rooted_uri_path {
        let stripped = path.trim_start_matches('/');
        if is_windows_drive_path(stripped) {
            path = stripped.to_owned();
            return Ok(path.replace('/', "\\"));
        }
        // `file:///song.mp3` is a POSIX path without a drive: not addressable.
        bail!("file:// 必须是绝对路径：{uri}");
    }
    if is_windows_drive_path(&path) {
        return Ok(path.replace('/', "\\"));
    }
    // `file://server/share/song.mp3`: the authority is a UNC server name.
    if path.contains('/') {
        return Ok(format!("\\\\{}", path.replace('/', "\\")));
    }
    bail!("file:// 必须是绝对路径：{uri}")
}

/// `C:/...` or `C:\\...` — a Windows drive-letter rooted path.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_windows_drive_path(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(chars.next(), Some(letter) if letter.is_ascii_alphabetic())
        && matches!(chars.next(), Some(':'))
        && matches!(chars.next(), Some('/') | Some('\\'))
}

fn open_file(uri: &str, at: Option<SeekTarget>) -> Result<OpenedMedia> {
    let path = file_path_from_uri(uri)?;
    let metadata =
        std::fs::metadata(&path).with_context(|| format!("无法访问本地媒体文件 {path}"))?;
    if metadata.is_dir() {
        bail!("file:// 地址指向目录而非媒体文件：{path}");
    }
    let mut file =
        std::fs::File::open(&path).with_context(|| format!("无法打开本地媒体文件 {path}"))?;
    if let Some(target) = at {
        file.seek(SeekFrom::Start(target.byte))
            .with_context(|| format!("无法跳转到本地媒体文件位置 {}", target.byte))?;
    }
    let bytes = Arc::new(AtomicU64::new(0));
    let source = CountingFile {
        inner: file,
        bytes: Arc::clone(&bytes),
    };
    Ok(OpenedMedia {
        source: Box::new(source),
        bytes,
        content_type: None,
        start_ms: at.map(|target| target.time_ms).unwrap_or(0),
        content_length: Some(metadata.len()),
        byte_seekable: true,
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
// HTTP/HTTPS transport (plain TCP + rustls)
// ---------------------------------------------------------------------------

/// One media transport stream: plain TCP or TLS (rustls, certificate checked).
enum MediaStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for MediaStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for MediaStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

/// Process-wide rustls client configuration using the webpki/Mozilla roots.
fn tls_client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            // With `default-features = false` there may be no process provider.
            let _ = rustls::crypto::ring::default_provider().install_default();
            let roots =
                RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

/// Connect the TCP transport and, for `https://`, perform the TLS handshake.
///
/// The socket read/write timeouts are deliberately short here (they are set to
/// [`READ_POLL`] by the caller before the handshake) so aborting a stalled
/// connect is fast; [`CONNECT_TIMEOUT`] bounds the whole handshake.
fn connect_media_stream(
    url: &HttpUrl,
    timeout: Duration,
    abort: &Abort,
) -> Result<MediaStream> {
    let addrs = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .with_context(|| format!("无法解析 DLNA 媒体服务器地址 {}（{}）", url.authority, url.host))?;
    let connect_timeout = timeout.min(CONNECT_TIMEOUT);
    let mut tcp = None;
    let mut last_error = None;
    for addr in addrs {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
        match TcpStream::connect_timeout(&addr, connect_timeout) {
            Ok(connected) => {
                tcp = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some(tcp) = tcp else {
        let detail = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "没有可用的地址".to_string());
        bail!("无法连接 DLNA 媒体服务器 {}：{detail}", url.authority);
    };
    let _ = tcp.set_nodelay(true);
    let poll = READ_POLL.min(timeout);
    tcp.set_read_timeout(Some(poll))
        .context("无法设置 DLNA 媒体读取超时")?;
    tcp.set_write_timeout(Some(poll))
        .context("无法设置 DLNA 媒体写入超时")?;
    if !url.tls {
        return Ok(MediaStream::Plain(tcp));
    }

    // rustls validates the chain against the webpki roots and that the
    // certificate matches `url.host` (SNI + SAN). Invalid names are rejected
    // before any bytes are sent.
    let server_name = ServerName::try_from(url.host.clone()).map_err(|_| {
        anyhow!(
            "DLNA HTTPS 主机名非法（无法生成 TLS SNI，仅支持 ASCII 域名或 IP）：{}",
            url.host
        )
    })?;
    let connection = ClientConnection::new(tls_client_config(), server_name)
        .map_err(|error| anyhow!("无法初始化 DLNA TLS 连接（{}）：{error}", url.authority))?;
    let mut stream = StreamOwned::new(connection, tcp);
    let handshake_deadline = Instant::now() + connect_timeout.max(Duration::from_secs(1));
    while stream.conn.is_handshaking() {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
        match stream.conn.complete_io(&mut stream.sock) {
            Ok((0, 0)) => {
                bail!("DLNA TLS 握手期间服务器关闭了连接（{}）", url.authority)
            }
            Ok(_) => {}
            Err(error) if retryable_io(&error) => {
                if Instant::now() >= handshake_deadline {
                    bail!("DLNA TLS 握手超时（{}）：{error}", url.authority);
                }
            }
            Err(error) => bail!(
                "DLNA TLS 握手失败（{}）：{error}（证书校验失败或服务器不支持 HTTPS）",
                url.authority
            ),
        }
    }
    stream
        .sock
        .set_read_timeout(Some(poll))
        .context("无法设置 DLNA TLS 读取超时")?;
    stream
        .sock
        .set_write_timeout(Some(poll))
        .context("无法设置 DLNA TLS 写入超时")?;
    Ok(MediaStream::Tls(Box::new(stream)))
}

/// Parsed HTTP(S) media URL.
#[derive(Debug, Clone)]
struct HttpUrl {
    /// `true` for `https://` (TLS), `false` for `http://`.
    tls: bool,
    host: String,
    /// Effective port (explicit, or the scheme default).
    port: u16,
    authority: String,
    request_target: String,
}

impl HttpUrl {
    const fn default_port(tls: bool) -> u16 {
        if tls { 443 } else { 80 }
    }

    fn scheme(&self) -> &'static str {
        if self.tls { "https" } else { "http" }
    }

    /// `Host` header: include the port only when it is not the scheme default.
    fn host_header(&self) -> String {
        if self.port == Self::default_port(self.tls) {
            self.host.clone()
        } else {
            self.authority.clone()
        }
    }
}

impl std::fmt::Display for HttpUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}://{}{}",
            self.scheme(),
            self.authority,
            self.request_target
        )
    }
}

/// Validate the host part of a media URL. Rejects malformed names so TLS SNI
/// can never be silently missing/invalid; IP literals are allowed.
fn validate_media_host(host: &str, uri: &str) -> Result<()> {
    if host.is_empty() {
        bail!("DLNA 媒体地址缺少主机名：{uri}");
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    if host.len() > 253 {
        bail!("DLNA 媒体地址主机名过长：{uri}");
    }
    let valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    });
    if !valid {
        bail!("DLNA 媒体地址主机名非法（仅支持 ASCII 域名或 IP）：{uri}");
    }
    Ok(())
}

/// Parse an `http(s)://host[:port][/path][?query]` URI.
///
/// Rejects the malformed shapes we cannot safely request instead of guessing.
fn parse_http_url(uri: &str) -> Result<HttpUrl> {
    let scheme_end = uri
        .find("://")
        .ok_or_else(|| anyhow!("非法 HTTP(S) 媒体地址：{uri}"))?;
    let scheme = uri[..scheme_end].to_ascii_lowercase();
    let tls = match scheme.as_str() {
        "http" => false,
        "https" => true,
        other => bail!(
            "不支持的 DLNA 媒体地址协议 {other}://（仅支持 http://、https:// 与 file://）：{uri}"
        ),
    };
    let rest = &uri[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
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
            HttpUrl::default_port(tls)
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
            None => (authority.to_string(), HttpUrl::default_port(tls)),
        }
    };

    validate_media_host(&host, uri)?;
    Ok(HttpUrl {
        tls,
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
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return parse_http_url(location);
    }
    if let Some(scheme_end) = lower.find("://") {
        bail!(
            "DLNA 重定向到不支持的协议 {}://（仅支持 http/https）：{location}",
            &lower[..scheme_end]
        );
    }
    if lower.starts_with("//") {
        // Scheme-relative redirect: keep the current scheme.
        return parse_http_url(&format!("{}://{}", base.scheme(), &location[2..]));
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
        tls: base.tls,
        host: base.host.clone(),
        port: base.port,
        authority: base.authority.clone(),
        request_target: merged,
    })
}

/// One parsed HTTP response (headers consumed, body still unread).
struct HttpHead {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    reader: BufReader<MediaStream>,
}

impl HttpHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Everything needed to reopen the same HTTP resource for a range seek.
#[derive(Clone)]
struct HttpContext {
    url: HttpUrl,
    timeout: Duration,
    abort: Abort,
    user_agent: String,
}

/// A response translated into a streaming body plus byte-range metadata.
struct HttpEstablished {
    context: HttpContext,
    reader: BufReader<MediaStream>,
    body_kind: BodyKind,
    /// Absolute media byte position of the first body byte.
    position: u64,
    /// Total resource length in bytes (Content-Length or Content-Range total).
    content_length: Option<u64>,
    range_supported: bool,
    content_type: Option<String>,
}

fn parse_content_length(value: Option<&str>) -> Result<Option<u64>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| anyhow!("DLNA 媒体服务器返回非法 Content-Length：{value}"))
}

/// Parse `Content-Range: bytes START-END/TOTAL` (TOTAL may be `*`) and check
/// the bounds against the requested range start. Never panics on malformed
/// values.
fn parse_content_range(
    value: Option<&str>,
    requested_start: u64,
) -> Result<(u64, u64, Option<u64>)> {
    let Some(value) = value.map(str::trim) else {
        bail!("DLNA Range 响应缺少 Content-Range 头");
    };
    let Some(rest) = value.strip_prefix("bytes ") else {
        bail!("DLNA Content-Range 单位非法（仅支持 bytes）：{value}");
    };
    let (range, total) = rest
        .split_once('/')
        .ok_or_else(|| anyhow!("DLNA Content-Range 格式非法：{value}"))?;
    let (start_text, end_text) = range
        .split_once('-')
        .ok_or_else(|| anyhow!("DLNA Content-Range 范围格式非法：{value}"))?;
    let start = start_text
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow!("DLNA Content-Range 起始位置非法：{value}"))?;
    let end = end_text
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow!("DLNA Content-Range 结束位置非法：{value}"))?;
    if end < start {
        bail!("DLNA Content-Range 结束位置小于起始位置：{value}");
    }
    if start != requested_start {
        bail!(
            "DLNA Range 响应起始位置不匹配（请求 {requested_start}，响应 {start}）：{value}"
        );
    }
    let total = if total.trim() == "*" {
        None
    } else {
        Some(
            total
                .trim()
                .parse::<u64>()
                .map_err(|_| anyhow!("DLNA Content-Range 总长度非法：{value}"))?,
        )
    };
    if let Some(total) = total
        && end >= total
    {
        bail!("DLNA Content-Range 超出资源长度（{value}）");
    }
    Ok((start, end, total))
}

fn dlna_user_agent() -> String {
    user_agent_override("FUSIONPLAY_DLNA_USER_AGENT")
        .unwrap_or_else(|| DEFAULT_USER_AGENT.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// UA used for the one-time 401/403 retry.
fn dlna_fallback_user_agent() -> String {
    user_agent_override("FUSIONPLAY_DLNA_FALLBACK_USER_AGENT")
        .unwrap_or_else(|| LEGACY_USER_AGENT.to_string())
}

fn user_agent_override(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn is_flv_content_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "video/flv" | "video/x-flv" | "application/x-flv"
    )
}

/// Issue the media request, following redirects and the one-time 401/403 UA
/// fallback, then translate the response into a streaming body description.
fn establish_http(
    base: &HttpUrl,
    timeout: Duration,
    abort: &Abort,
    range_start: Option<u64>,
    initial_user_agent: Option<String>,
) -> Result<HttpEstablished> {
    let mut url = base.clone();
    let mut redirects = 0usize;
    let mut scheme_switches = 0usize;
    let mut user_agent = initial_user_agent.unwrap_or_else(dlna_user_agent);
    let mut fallback_attempted = false;

    loop {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
        let response = http_request(&url, timeout, abort, &user_agent, range_start)?;
        if matches!(response.status, 401 | 403) && !fallback_attempted {
            fallback_attempted = true;
            user_agent = dlna_fallback_user_agent();
            tracing::info!(
                target: "dlna",
                event = "used_fallback_ua",
                status = response.status,
                user_agent = %user_agent,
                "DLNA 服务器拒绝了浏览器 UA（HTTP {}），改用回退 UA 重试一次",
                response.status
            );
            continue;
        }
        if is_redirect(response.status) {
            if redirects >= MAX_REDIRECTS {
                bail!("DLNA 媒体服务器重定向次数过多（>{MAX_REDIRECTS}）：{url}");
            }
            let location = response
                .header("location")
                .ok_or_else(|| anyhow!("HTTP {} 响应缺少 Location 头：{url}", response.status))?
                .to_string();
            let next = resolve_redirect(&url, &location)?;
            if next.tls != url.tls {
                scheme_switches += 1;
                if scheme_switches > MAX_SCHEME_SWITCHES {
                    bail!(
                        "DLNA 媒体重定向跨 http/https 次数过多（>{MAX_SCHEME_SWITCHES}）：{url}"
                    );
                }
            }
            url = next;
            redirects += 1;
            continue;
        }

        let requested_start = range_start.unwrap_or(0);
        let (position, content_length, range_supported, range_length) =
            if response.status == 206 {
                let (start, end, total) =
                    parse_content_range(response.header("content-range"), requested_start)?;
                let length = end.saturating_sub(start).saturating_add(1);
                (start, total, true, Some(length))
            } else if response.status == 200 {
                let content_length = parse_content_length(response.header("content-length"))?;
                let accept_ranges = response
                    .header("accept-ranges")
                    .is_some_and(|value| value.to_ascii_lowercase().contains("bytes"));
                (0u64, content_length, accept_ranges && content_length.is_some(), None)
            } else {
                bail!(
                    "DLNA 媒体服务器返回 HTTP {} {}：{url}",
                    response.status,
                    response.reason
                );
            };
        if let Some(encoding) = response.header("content-encoding") {
            let encoding = encoding.trim().to_ascii_lowercase();
            if !encoding.is_empty() && encoding != "identity" {
                bail!("DLNA 媒体使用了不支持的压缩传输（Content-Encoding: {encoding}）：{url}");
            }
        }

        let chunked = response
            .header("transfer-encoding")
            .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
        let connection_close = response
            .header("connection")
            .is_some_and(|value| value.to_ascii_lowercase().contains("close"));
        let body_kind = if chunked {
            BodyKind::Chunked
        } else if let Some(length) = range_length {
            if let Some(header_length) =
                parse_content_length(response.header("content-length"))?
                && header_length != length
            {
                bail!(
                    "DLNA Range 响应长度不一致（Content-Length={header_length}，Content-Range={length}）：{url}"
                );
            }
            BodyKind::Length(length)
        } else if let Some(length) = content_length {
            BodyKind::Length(length)
        } else if connection_close {
            BodyKind::UntilClose
        } else {
            bail!(
                "DLNA 媒体服务器既未提供 Content-Length 也不是 chunked，无法流式读取（保持连接会无限等待）：{url}"
            );
        };

        let content_type = response.header("content-type").map(str::to_string);
        let HttpHead {
            reader,
            headers: _,
            reason: _,
            status: _,
        } = response;
        return Ok(HttpEstablished {
            context: HttpContext {
                url: url.clone(),
                timeout,
                abort: abort.clone(),
                user_agent,
            },
            reader,
            body_kind,
            position,
            content_length,
            range_supported,
            content_type,
        });
    }
}

/// Open an HTTP(S) media stream. The initial request always carries
/// `Range: bytes=0-`: a `206` response proves that byte-range seeking works,
/// a `200` response means the stream can only be played sequentially.
fn open_http(
    uri: &str,
    timeout: Duration,
    abort: &Abort,
    at: Option<SeekTarget>,
) -> Result<OpenedMedia> {
    let base = parse_http_url(uri)?;
    // Range is only requested when reopening at an offset. The initial request
    // stays a plain GET; servers advertise seekability with `Accept-Ranges`.
    let range_start = at.map(|target| target.byte);
    let established = establish_http(&base, timeout, abort, range_start, None)?;
    let content_type = established.content_type.clone();
    let content_length = established.content_length;
    let range_supported = established.range_supported;
    let position = established.position;
    let bytes = Arc::new(AtomicU64::new(0));
    let mut source = HttpSource::new(established, Arc::clone(&bytes));
    if let Some(target) = at
        && position != target.byte
    {
        source
            .skip_bytes(target.byte)
            .with_context(|| format!("无法跳转到 DLNA 媒体字节位置 {}", target.byte))?;
    }
    let content_length = content_length.or_else(|| source.byte_len());
    Ok(OpenedMedia {
        source: Box::new(source),
        bytes,
        content_type,
        start_ms: at.map(|target| target.time_ms).unwrap_or(0),
        content_length,
        byte_seekable: range_supported,
    })
}

fn http_request(
    url: &HttpUrl,
    timeout: Duration,
    abort: &Abort,
    user_agent: &str,
    range_start: Option<u64>,
) -> Result<HttpHead> {
    let mut stream = connect_media_stream(url, timeout, abort)?;
    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n",
        url.request_target,
        url.host_header(),
        user_agent
    );
    if let Some(start) = range_start {
        request.push_str(&format!("Range: bytes={start}-\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .with_context(|| format!("无法向 DLNA 媒体服务器 {} 发送 GET 请求", url.authority))?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let status_line = read_http_line(&mut reader, timeout, abort)
        .with_context(|| format!("读取 DLNA 媒体服务器 {} 的 HTTP 状态行失败", url.authority))?;
    let (status, reason) = parse_status_line(&status_line)
        .with_context(|| format!("DLNA 媒体服务器返回非法 HTTP 状态行：{status_line}"))?;
    let mut headers = Vec::new();
    for _ in 0..MAX_HEADER_COUNT {
        let line = read_http_line(&mut reader, timeout, abort)
            .with_context(|| format!("读取 DLNA 媒体服务器 {} 的 HTTP 响应头失败", url.authority))?;
        if line.is_empty() {
            return Ok(HttpHead {
                status,
                reason,
                headers,
                reader,
            });
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
/// servers that never send a newline) and abort/timeout handling.
fn read_http_line(
    reader: &mut BufReader<MediaStream>,
    timeout: Duration,
    abort: &Abort,
) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    loop {
        if abort.aborted() {
            return Err(aborted_error());
        }
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if retryable_io(&error) => {
                if Instant::now() >= deadline {
                    return Err(timeout_error(timeout, "读取 DLNA HTTP 响应"));
                }
                continue;
            }
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
    reader: BufReader<MediaStream>,
    kind: BodyKind,
    phase: ChunkedPhase,
    done: bool,
    bytes: Arc<AtomicU64>,
    timeout: Duration,
    abort: Abort,
}

impl HttpBody {
    fn read_some(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let deadline = Instant::now() + self.timeout;
        loop {
            // Checked before every socket read so stop/takeover is noticed
            // between reads; the short poll timeout is the abort backstop.
            if self.abort.aborted() {
                return Err(aborted_error());
            }
            match self.reader.read(buffer) {
                Ok(read) => return Ok(read),
                Err(error) if retryable_io(&error) => {
                    if Instant::now() >= deadline {
                        return Err(timeout_error(self.timeout, "读取 DLNA 媒体流"));
                    }
                }
                Err(error) => {
                    return Err(map_stream_error(error, self.timeout, "读取 DLNA 媒体流"))
                }
            }
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
                        let line = match read_http_line(
                            &mut self.reader,
                            self.timeout,
                            &self.abort,
                        ) {
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
                        let line = match read_http_line(
                            &mut self.reader,
                            self.timeout,
                            &self.abort,
                        ) {
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

/// HTTP(S) media source with optional byte-range seeking.
///
/// `is_seekable()` is only `true` when the server answered the initial
/// `Range: bytes=0-` with `206 Partial Content`; a `200` response keeps the
/// source sequential (Symphonia then falls back to linear playout).
struct HttpSource {
    body: HttpBody,
    content_length: Option<u64>,
    /// Absolute media byte position of the next byte `body` will return.
    position: u64,
    range_supported: bool,
    context: HttpContext,
    bytes: Arc<AtomicU64>,
}

impl HttpSource {
    fn new(established: HttpEstablished, bytes: Arc<AtomicU64>) -> Self {
        let HttpEstablished {
            context,
            reader,
            body_kind,
            position,
            content_length,
            range_supported,
            content_type: _,
        } = established;
        let body = HttpBody {
            reader,
            kind: body_kind,
            phase: ChunkedPhase::Size,
            done: false,
            bytes: Arc::clone(&bytes),
            timeout: context.timeout,
            abort: context.abort.clone(),
        };
        Self {
            body,
            content_length,
            position,
            range_supported,
            context,
            bytes,
        }
    }

    /// Read and discard bytes so the stream starts at `target`.
    fn skip_bytes(&mut self, target: u64) -> io::Result<()> {
        if target < self.position {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "DLNA 媒体流无法向前回退（服务器未按 Range 响应）",
            ));
        }
        let mut remaining = target - self.position;
        let mut buffer = [0u8; 16 * 1024];
        while remaining > 0 {
            let want = buffer.len().min(remaining as usize);
            let read = self.body.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("DLNA 媒体流在跳转位置之前结束（还差 {remaining} 字节）"),
                ));
            }
            remaining -= read as u64;
            self.position += read as u64;
        }
        Ok(())
    }

    /// Reopen the HTTP stream at `target` with a `Range: bytes=target-` request.
    fn refresh_from(&mut self, target: u64) -> io::Result<()> {
        let established = establish_http(
            &self.context.url,
            self.context.timeout,
            &self.context.abort,
            Some(target),
            Some(self.context.user_agent.clone()),
        )
        .map_err(|error| io::Error::other(format!("{error:#}")))?;
        let HttpEstablished {
            context,
            reader,
            body_kind,
            position,
            content_length,
            range_supported,
            content_type: _,
        } = established;
        let health_bytes = Arc::clone(&self.bytes);
        self.body = HttpBody {
            reader,
            kind: body_kind,
            phase: ChunkedPhase::Size,
            done: false,
            bytes: health_bytes,
            timeout: context.timeout,
            abort: context.abort.clone(),
        };
        self.context = context;
        self.content_length = content_length.or(self.content_length);
        self.range_supported = range_supported;
        if position == target {
            self.position = target;
            return Ok(());
        }
        if position == 0 {
            if target == 0 {
                // Full-body response for a range starting at 0 is equivalent.
                self.position = 0;
                return Ok(());
            }
            // The server ignored Range and sent the whole body: fall back to
            // discarding bytes and remember that later seeks are expensive.
            self.position = 0;
            self.range_supported = false;
            self.skip_bytes(target)?;
            return Ok(());
        }
        Err(io::Error::other(format!(
            "DLNA Range 响应起始位置异常（请求 {target}，响应 {position}）"
        )))
    }

    /// Compute a seek target from a [`SeekFrom`], validating it against the
    /// known resource length.
    fn resolve_seek(&self, from: SeekFrom, position: u64) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(delta) => position
                .checked_add_signed(delta)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DLNA seek 位置溢出"))?,
            SeekFrom::End(delta) => {
                let length = self.content_length.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "DLNA 媒体长度未知，无法从末尾 seek",
                    )
                })?;
                length
                    .checked_add_signed(delta)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "DLNA seek 位置溢出"))?
            }
        };
        if let Some(length) = self.content_length
            && target > length
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("DLNA seek 位置 {target} 超出媒体长度 {length}"),
            ));
        }
        Ok(target)
    }
}

impl Read for HttpSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.body.read(buffer)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if !self.range_supported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "DLNA HTTP 服务器不支持 Range，无法跳转",
            ));
        }
        let target = self.resolve_seek(from, self.position)?;
        if let Some(length) = self.content_length
            && target >= length
        {
            // Seeking to/past EOF needs no network round trip.
            self.body.done = true;
            self.position = target;
            return Ok(target);
        }
        if target == self.position {
            return Ok(target);
        }
        self.refresh_from(target)?;
        Ok(self.position)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        self.range_supported
    }

    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

fn retryable_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

fn aborted_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        "DLNA 播放已中止（停止或音源被接管）",
    )
}

fn timeout_error(timeout: Duration, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("{what}超时（{timeout:?} 内无数据，媒体服务器过慢或网络中断）"),
    )
}

/// Map low-level socket errors to actionable Chinese diagnostics.
fn map_stream_error(error: io::Error, timeout: Duration, what: &str) -> io::Error {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => timeout_error(timeout, what),
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
// Abort handling (user stop + source takeover)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Abort {
    stop: Arc<AtomicBool>,
    engine: Arc<AudioEngine>,
}

impl Abort {
    fn aborted(&self) -> bool {
        if self.stop.load(Ordering::Acquire) {
            return true;
        }
        // Any *other* active source means we were taken over. `None` is
        // tolerated: the core's takeover event may still be in flight while
        // the decode worker starts, and the engine simply drops early PCM.
        matches!(
            self.engine.active_source(),
            Some(source) if source != Source::Dlna
        )
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn track(url: &str) -> DlnaTrack {
        DlnaTrack {
            url: url.to_string(),
            ..DlnaTrack::default()
        }
    }

    fn validate(url: &str) -> Result<()> {
        validate_track(&track(url))
    }

    #[test]
    fn accepts_https_and_rejects_unsupported_schemes() {
        // P1-1: https media is supported (rustls + webpki roots).
        validate("https://example.com/a.mp3").expect("HTTPS media must validate");
        validate("https://nas.local:8443/music/a.flac?x=1").expect("HTTPS with port");
        let ftp = validate("ftp://example.com/a.mp3").unwrap_err().to_string();
        assert!(ftp.contains("不支持"), "got: {ftp}");
        let rtsp = validate("rtsp://example.com/a.mp3").unwrap_err().to_string();
        assert!(rtsp.contains("不支持"), "got: {rtsp}");
        let empty = validate("   ").unwrap_err().to_string();
        assert!(empty.contains("为空"), "got: {empty}");
        let relative = validate("media/a.mp3").unwrap_err().to_string();
        assert!(relative.contains("不支持"), "got: {relative}");
        // Invalid hosts are rejected before any TLS connection is attempted.
        assert!(validate("https://bad host/a.mp3").is_err());
        assert!(validate("https://nas_name/a.mp3").is_err());
        assert!(validate("https://例子.com/a.mp3").is_err());
    }

    #[test]
    fn parses_https_urls_with_correct_defaults_and_host_header() {
        let url = parse_http_url("https://music.example.com/track.flac?q=1").unwrap();
        assert!(url.tls);
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.port, 443);
        assert_eq!(url.request_target, "/track.flac?q=1");
        assert_eq!(url.host_header(), "music.example.com");

        let explicit = parse_http_url("https://music.example.com:8443/a.bin").unwrap();
        assert_eq!(explicit.port, 8443);
        assert_eq!(explicit.host_header(), "music.example.com:8443");

        let http_port = parse_http_url("http://music.example.com:80/a.mp3").unwrap();
        assert_eq!(http_port.host_header(), "music.example.com");
        let http_other = parse_http_url("http://music.example.com:8080/a.mp3").unwrap();
        assert_eq!(http_other.host_header(), "music.example.com:8080");

        let ipv6 = parse_http_url("https://[::1]:9000/a.mp3").unwrap();
        assert!(ipv6.tls);
        assert_eq!(ipv6.host, "::1");
        assert_eq!(ipv6.port, 9000);
        assert_eq!(ipv6.request_target, "/a.mp3");
    }

    #[test]
    fn parses_content_range_and_rejects_malformed_values() {
        let (start, end, total) =
            parse_content_range(Some("bytes 100-199/1000"), 100).expect("valid range");
        assert_eq!((start, end, total), (100, 199, Some(1000)));
        let (_, _, total) =
            parse_content_range(Some("bytes 0-9/*"), 0).expect("unknown total");
        assert_eq!(total, None);
        for malformed in [
            None,
            Some("items 0-9/10"),
            Some("bytes 0-9"),
            Some("bytes 9-0/10"),
            Some("bytes 5-9/10"),
            Some("bytes 0-9/5"),
            Some("bytes -1-9/10"),
            Some("bytes 0-18446744073709551616/10"),
        ] {
            assert!(
                parse_content_range(malformed, 0).is_err(),
                "must reject {malformed:?}"
            );
        }
    }

    #[test]
    fn estimates_range_targets_conservatively() {
        let target = estimate_seek_target(Some(200_000), Some(10 * 1024 * 1024), 50_000)
            .expect("range estimate");
        assert_eq!(target.byte, 2_621_440);
        assert!(target.time_ms <= 50_000);
        assert!(estimate_seek_target(None, Some(1000), 500).is_none());
        assert!(estimate_seek_target(Some(1000), None, 500).is_none());
        assert!(estimate_seek_target(Some(1000), Some(1000), 0).is_none());
    }

    #[test]
    fn tls_config_builds_with_webpki_roots() {
        let config = tls_client_config();
        assert!(Arc::strong_count(&config) >= 1);
        assert!(Arc::ptr_eq(&config, &tls_client_config()));
    }

    #[test]
    fn accepts_netease_flv_audio_and_ogg() {
        let mut flv = track("http://127.0.0.1:1/music/stream?id=42");
        flv.content_type = Some("video/flv".to_string());
        validate_track(&flv).expect("video/flv must be treated as audio");

        let mut video_class_flv = track("http://127.0.0.1:1/music/stream.flv");
        video_class_flv.media_kind = "video".to_string();
        validate_track(&video_class_flv)
            .expect("videoItem FLV audio must pass validation");

        validate("http://127.0.0.1:1/song.ogg").expect("OGG audio is supported");
    }

    #[test]
    fn rejects_hls_and_video_with_actionable_reasons() {
        let hls = validate("http://127.0.0.1:1/live.m3u8")
            .unwrap_err()
            .to_string();
        assert!(hls.contains("HLS"), "got: {hls}");
        let mut video_track = track("http://127.0.0.1:1/movie.mp4");
        video_track.content_type = Some("video/mp4".to_string());
        let video = validate_track(&video_track).unwrap_err().to_string();
        assert!(video.contains("视频"), "got: {video}");
        let mut kind_video = track("http://127.0.0.1:1/item");
        kind_video.media_kind = "video".to_string();
        let video = validate_track(&kind_video).unwrap_err().to_string();
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
        assert!(parse_http_url("https:///no-host/a.mp3").is_err());
        assert!(parse_http_url("https://nas:65536/a.mp3").is_err());
        assert!(parse_http_url("https://nas:0/a.mp3").is_ok());
        assert!(parse_http_url("rtsp://nas/a.mp3").is_err());
        assert!(parse_http_url("http://nas_name/a.mp3").is_err());
    }

    #[test]
    fn resolves_relative_and_cross_scheme_redirects() {
        let base = parse_http_url("http://nas:8200/music/list/a.mp3").unwrap();
        let absolute = resolve_redirect(&base, "/media/b.mp3").unwrap();
        assert_eq!(absolute.request_target, "/media/b.mp3");
        assert_eq!(absolute.port, 8200);
        assert!(!absolute.tls);
        let relative = resolve_redirect(&base, "b.mp3").unwrap();
        assert_eq!(relative.request_target, "/music/list/b.mp3");
        let full = resolve_redirect(&base, "http://other:9000/c.mp3").unwrap();
        assert_eq!(full.authority, "other:9000");

        // P1-1: http -> https and https -> http redirects are both allowed.
        let upgrade = resolve_redirect(&base, "https://cdn.example.com/c.mp3").unwrap();
        assert!(upgrade.tls);
        assert_eq!(upgrade.port, 443);
        let https_base = parse_http_url("https://nas/music/a.mp3").unwrap();
        let downgrade = resolve_redirect(&https_base, "http://nas2/a.mp3").unwrap();
        assert!(!downgrade.tls);
        assert_eq!(downgrade.port, 80);
        let scheme_relative = resolve_redirect(&https_base, "//other/a.mp3").unwrap();
        assert!(scheme_relative.tls);
        assert_eq!(scheme_relative.authority, "other");
        assert!(resolve_redirect(&base, "   ").is_err());
        assert!(resolve_redirect(&base, "ftp://nas/a.mp3").is_err());
    }

    #[test]
    fn percent_decodes_file_names() {
        assert_eq!(percent_decode("a%20b%2Fc.wav").unwrap(), "a b/c.wav");
        assert!(percent_decode("bad%2").is_err());
    }


}

#[cfg(test)]
mod windows_file_uri_tests {
    use super::file_path_from_uri_windows;

    #[test]
    fn accepts_windows_file_uri_spellings() {
        assert_eq!(
            file_path_from_uri_windows("file://C:\\Media\\song.mp3").unwrap(),
            "C:\\Media\\song.mp3"
        );
        assert_eq!(
            file_path_from_uri_windows("file://C:/Media/song.mp3").unwrap(),
            "C:\\Media\\song.mp3"
        );
        assert_eq!(
            file_path_from_uri_windows("file:///C:/Media/song.mp3").unwrap(),
            "C:\\Media\\song.mp3"
        );
        assert_eq!(
            file_path_from_uri_windows("file://localhost/C:/Media/song.mp3").unwrap(),
            "C:\\Media\\song.mp3"
        );
        assert_eq!(
            file_path_from_uri_windows("file://server/share/song.mp3").unwrap(),
            "\\\\server\\share\\song.mp3"
        );
    }

    #[test]
    fn decodes_percent_escapes_in_windows_paths() {
        assert_eq!(
            file_path_from_uri_windows("file:///C:/Media/%E6%AD%8C.mp3").unwrap(),
            "C:\\Media\\歌.mp3"
        );
    }

    #[test]
    fn rejects_relative_and_empty_windows_file_uris() {
        assert!(file_path_from_uri_windows("file://").is_err());
        assert!(file_path_from_uri_windows("file://song.mp3").is_err());
        assert!(file_path_from_uri_windows("file:///song.mp3").is_err());
    }
}
