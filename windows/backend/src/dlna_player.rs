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
//! | `http://` 301/302/303/307/308   | followed (max [`MAX_REDIRECTS`]) |
//! | `file://` local file            | supported, seekable |
//! | `https://`                      | **not supported** (no TLS stack bundled) |
//! | HLS (`m3u8`/`m3u`, `*mpegurl`)  | **not supported** (explicit error) |
//! | video tracks / `upnp:class` video | **not supported** (explicit error) |
//! | mp3 / flac / wav / aac / mp4 audio | supported containers |
//!
//! Downloading is fully streaming: the HTTP body is decoded (content-length,
//! chunked, or until-close) as the decoder pulls bytes, so a multi-GB file never
//! lands in RAM. The fixed buffering is bounded by [`STREAM_BUFFER_BYTES`].
//!
//! # Error handling contract
//!
//! * Empty/illegal URIs, `https://`, HLS and video are rejected
//!   **synchronously** by [`DlnaPlayer::play_track`] before any network or
//!   audio activity; the caller maps the `Err` into `lastError`.
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

use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    calculate_cutoff, Async, FixedAsync, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

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

/// Input chunk size (frames) handed to the rubato resampler at a time.
const RESAMPLE_CHUNK_FRAMES: usize = 1024;

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
    /// newer one (seek/next track), in which case no terminal event is emitted.
    Stopped { epoch: u64 },
}

/// Non-blocking event callback (`Arc<dyn Fn(DlnaEvent) + Send + Sync>`).
pub type DlnaEventSink = Arc<dyn Fn(DlnaEvent) + Send + Sync>;

struct CurrentPlayback {
    token: Arc<AtomicBool>,
    /// Set when a newer playback supersedes this one (seek/new track): the
    /// worker must not emit terminal events for the abandoned epoch.
    abandoned: Arc<AtomicBool>,
    /// Cleared by the worker when it exits.
    playing: Arc<AtomicBool>,
    track: Arc<DlnaTrack>,
    epoch: u64,
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
}

impl DlnaPlayer {
    pub fn new(engine: Arc<AudioEngine>, emit: DlnaEventSink) -> Arc<Self> {
        Arc::new(Self {
            engine,
            emit,
            current: Mutex::new(None),
            volume_gain_bits: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            timeout_ms: AtomicU64::new(DEFAULT_TIMEOUT.as_millis() as u64),
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

    /// Restart the current track from `position_ms`. The simplified
    /// implementation reopens the stream and discards decoded audio until the
    /// target position has been reached (no HTTP range requests).
    pub fn seek(&self, position_ms: u64) -> Result<()> {
        let (track, epoch) = {
            let guard = self
                .current
                .lock()
                .map_err(|_| anyhow!("DLNA 播放器状态异常，无法跳转"))?;
            let Some(current) = guard.as_ref() else {
                bail!("当前没有正在播放的 DLNA 媒体，无法跳转");
            };
            if !current.playing.load(Ordering::Acquire) {
                bail!("DLNA 媒体尚未开始播放，无法跳转");
            }
            current.abandoned.store(true, Ordering::Release);
            current.token.store(true, Ordering::Release);
            let mut track = (*current.track).clone();
            track.start_position_ms = position_ms;
            (track, current.epoch)
        };
        self.launch(track, epoch)
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
                abandoned: Arc::clone(&abandoned),
                playing: Arc::clone(&playing),
                track: Arc::clone(&track),
                epoch,
            });
        }

        let engine = Arc::clone(&self.engine);
        let emit = Arc::clone(&self.emit);
        let gain_bits = Arc::clone(&self.volume_gain_bits);
        let timeout = Duration::from_millis(self.timeout_ms.load(Ordering::Acquire).max(1));
        let worker_playing = Arc::clone(&playing);
        let worker_token = Arc::clone(&token);
        let worker_abandoned = Arc::clone(&abandoned);
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
    timeout: Duration,
) -> Result<PlaySummary> {
    let abort = Abort {
        stop: Arc::clone(stop),
        engine: Arc::clone(engine),
    };
    let duration_hint = track.duration_ms.unwrap_or(0);
    emit(DlnaEvent::Progress {
        epoch,
        position_ms: track.start_position_ms,
        duration_ms: duration_hint,
    });

    let result = (|| -> Result<PlaySummary> {
        let opened = open_source(&track.url, timeout, &abort)?;
        decode_and_play(
            engine, emit, &abort, playing, epoch, track, gain_bits, opened,
        )
    })();

    // Clear the playing flag before publishing the final events: observers that
    // react to `Stopped` must already see `is_playing() == false`.
    playing.store(false, Ordering::Release);
    let superseded = abandoned.load(Ordering::Acquire);
    if !superseded {
        if let Err(error) = &result {
            emit(DlnaEvent::Error {
                message: format!("DLNA 播放失败（{}）：{error:#}", track.url),
            });
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
) -> Result<PlaySummary> {
    let OpenedMedia {
        source,
        bytes,
        content_type,
    } = opened;

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
                "无法识别 DLNA 媒体格式（支持 mp3/flac/wav/aac/mp4 音频；不支持 HLS/视频）：{error}"
            )
        })?;

    let selected = format
        .default_track(TrackType::Audio)
        .context("媒体中没有可解码的音频轨道（可能是纯视频或 HLS 流）")?;
    let track_id = selected.id;
    let selected_num_frames = selected.num_frames;
    let params = match selected.codec_params.clone() {
        Some(CodecParameters::Audio(params)) => params,
        Some(_) => bail!("所选轨道不是音频轨道（DLNA 视频暂不支持）"),
        None => bail!("音频轨道缺少解码参数（编解码器可能不受支持）"),
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .context("无法创建音频解码器（编解码器可能不受支持）")?;

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
    // Frames still to discard so the audible output starts at the requested
    // seek position (used by the simplified seek implementation).
    let mut skip_frames =
        track.start_position_ms.saturating_mul(TARGET_RATE as u64) / 1_000;

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

        // Drop decoded audio before the requested seek position.
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
            emit(DlnaEvent::Playback {
                epoch,
                playing: true,
            });
        }
        summary.push_frames(engine, epoch, volume_gain(gain_bits), &pcm);
        if last_state_update.elapsed() >= STATE_UPDATE_INTERVAL {
            last_state_update = Instant::now();
            emit_progress(emit, epoch, track, &summary);
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
    emit_progress(emit, epoch, track, &summary);
    Ok(summary)
}

fn emit_progress(emit: &DlnaEventSink, epoch: u64, track: &DlnaTrack, summary: &PlaySummary) {
    emit(DlnaEvent::Progress {
        epoch,
        position_ms: summary.position_ms(track.start_position_ms),
        duration_ms: summary.duration_ms,
    });
}

fn volume_gain(bits: &AtomicU32) -> f32 {
    f32::from_bits(bits.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Playback bookkeeping
// ---------------------------------------------------------------------------

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
        scheme => {
            bail!("不支持的 DLNA 媒体地址协议 {scheme}://（仅支持 http:// 与 file://）：{uri}")
        }
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
        if content_type.starts_with("video/") {
            bail!("DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/mp4 音轨）：{uri}");
        }
    }
    if track.media_kind.eq_ignore_ascii_case("video") {
        bail!("DLNA 视频暂不支持（仅支持音频：mp3/flac/wav/aac/mp4 音轨）：{uri}");
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

/// Where the bytes come from, plus the live download counter.
struct OpenedMedia {
    source: Box<dyn MediaSource>,
    bytes: Arc<AtomicU64>,
    content_type: Option<String>,
}

fn open_source(uri: &str, timeout: Duration, abort: &Abort) -> Result<OpenedMedia> {
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

fn open_file(uri: &str) -> Result<OpenedMedia> {
    let path = file_path_from_uri(uri)?;
    let metadata =
        std::fs::metadata(&path).with_context(|| format!("无法访问本地媒体文件 {path}"))?;
    if metadata.is_dir() {
        bail!("file:// 地址指向目录而非媒体文件：{path}");
    }
    let file =
        std::fs::File::open(&path).with_context(|| format!("无法打开本地媒体文件 {path}"))?;
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
        .find(['/', '?', '#'])
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

fn open_http(uri: &str, timeout: Duration, abort: &Abort) -> Result<OpenedMedia> {
    let bytes = Arc::new(AtomicU64::new(0));
    let mut url = parse_http_url(uri)?;
    let mut redirects = 0;

    loop {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
        let response = http_request(&url, timeout, abort)?;
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

fn http_request(url: &HttpUrl, timeout: Duration, abort: &Abort) -> Result<HttpHead> {
    let addrs = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .with_context(|| format!("无法解析 DLNA 媒体服务器地址 {}（{}）", url.authority, url.host))?;
    let connect_timeout = timeout.min(CONNECT_TIMEOUT);
    let mut stream = None;
    let mut last_error = None;
    for addr in addrs {
        if abort.aborted() {
            bail!("DLNA 播放已中止");
        }
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
    // Poll in short slices so `stop()` can abort a blocked read; the overall
    // inactivity deadline is enforced by the retry loops below.
    let poll = READ_POLL.min(timeout);
    stream
        .set_read_timeout(Some(poll))
        .context("无法设置 DLNA 媒体读取超时")?;
    stream
        .set_write_timeout(Some(poll))
        .context("无法设置 DLNA 媒体写入超时")?;
    let _ = stream.set_nodelay(true);

    let mut stream = stream;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: FusionPlay-Windows/{}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.request_target,
        url.host_header(),
        crate::VERSION
    );
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
    reader: &mut BufReader<TcpStream>,
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
    reader: BufReader<TcpStream>,
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
                        let line = match read_http_line(&mut self.reader, self.timeout, &self.abort)
                        {
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
                                format!(
                                    "DLNA chunked 媒体流中断：块内仍差 {remaining} 字节（服务器提前关闭连接）"
                                ),
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
                        let line = match read_http_line(&mut self.reader, self.timeout, &self.abort)
                        {
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
                .extend_from_slice(&interleaved[..frames * TARGET_CHANNELS as usize]);
        } else {
            self.stereo.reserve(frames * TARGET_CHANNELS as usize);
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

        let (mut sum_left, mut sum_right) = taps.iter().fold(
            (0.0f32, 0.0f32),
            |(left, right), (_, tap_left, tap_right)| (left + tap_left, right + tap_right),
        );
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

        let identity = planes == TARGET_CHANNELS as usize
            && taps.len() == TARGET_CHANNELS as usize
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

    fn mix(&self, frame: &[f32]) -> [f32; TARGET_CHANNELS as usize] {
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
    if left && !right {
        (surround, 0.0)
    } else if right && !left {
        (0.0, surround)
    } else {
        // Centre, stereo-of-both-sides, and unknown positions all contribute
        // equally to L/R (the coefficients are already normalised).
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
            TARGET_CHANNELS as usize,
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
            pending: Vec::with_capacity(RESAMPLE_CHUNK_FRAMES * TARGET_CHANNELS as usize),
            delay_frames,
        })
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        while self.pending.len() >= self.chunk * TARGET_CHANNELS as usize {
            self.process_chunk(out)?;
        }
        Ok(())
    }

    fn process_chunk(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let frames = (self.pending.len() / TARGET_CHANNELS as usize).min(self.chunk);
        let adapter = InterleavedSlice::new(
            &self.pending[..frames * TARGET_CHANNELS as usize],
            TARGET_CHANNELS as usize,
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
        self.pending.drain(..frames * TARGET_CHANNELS as usize);
        Ok(())
    }

    /// Flush the buffered tail: zero-pad the final partial chunk, resample it,
    /// and keep only the frames that correspond to real input.
    fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let frames = self.pending.len() / TARGET_CHANNELS as usize;
        if frames == 0 {
            return Ok(());
        }
        let mut padded = vec![0.0f32; self.chunk * TARGET_CHANNELS as usize];
        padded[..frames * TARGET_CHANNELS as usize].copy_from_slice(&self.pending);
        let adapter = InterleavedSlice::new(&padded, TARGET_CHANNELS as usize, self.chunk)
            .map_err(|error| anyhow!("重采样尾部缓冲无效：{error}"))?;
        let translated = self
            .inner
            .process(&adapter, 0, None)
            .map_err(|error| anyhow!("重采样尾部刷新失败：{error}"))?;
        let mut data = translated.take_data();
        self.trim_delay(&mut data);
        let keep_frames = ((frames as f64) * self.ratio).ceil() as usize;
        let keep_samples = (keep_frames * TARGET_CHANNELS as usize).min(data.len());
        out.extend_from_slice(&data[..keep_samples]);
        self.pending.clear();
        Ok(())
    }

    fn trim_delay(&mut self, data: &mut Vec<f32>) {
        if self.delay_frames == 0 {
            return;
        }
        let drop_frames = self.delay_frames.min(data.len() / TARGET_CHANNELS as usize);
        data.drain(..drop_frames * TARGET_CHANNELS as usize);
        self.delay_frames -= drop_frames;
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

    #[test]
    fn stream_resampler_keeps_frame_ratio() {
        let mut resampler = StreamResampler::new(44_100).unwrap();
        let input = vec![0.25f32; 44_100 * 2];
        let mut out = Vec::new();
        resampler.push(&input, &mut out).unwrap();
        resampler.finish(&mut out).unwrap();
        let frames = out.len() / 2;
        assert!(
            (frames as i64 - 48_000).abs() < 200,
            "got {frames} frames"
        );
        // Ignore the sinc filter's start-up and end-of-stream transient; the
        // steady state must reproduce the DC input exactly.
        let steady = &out[2 * 200..2 * (frames - 400)];
        assert!(
            steady.iter().all(|sample| (*sample - 0.25).abs() < 1e-3),
            "steady-state DC changed: min={:?} max={:?}",
            steady.iter().copied().fold(f32::INFINITY, f32::min),
            steady.iter().copied().fold(f32::NEG_INFINITY, f32::max)
        );
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
