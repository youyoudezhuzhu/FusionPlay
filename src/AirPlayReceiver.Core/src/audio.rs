use crate::events::{CoreEvent, EventSink};
use crate::takeover::{MediaLease, MediaSource, PlaybackArbiter};
use anyhow::{Context, Result, bail};
#[cfg(not(target_os = "android"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(not(target_os = "android"))]
use cpal::{FromSample, SizedSample};
#[cfg(target_os = "android")]
use oboe::{
    AudioOutputCallback, AudioOutputStreamSafe, AudioStream, AudioStreamBuilder, ContentType,
    DataCallbackResult, Error as OboeError, PerformanceMode, SampleRateConversionQuality,
    SharingMode, Stereo, Usage,
};
use shairplay::{
    AudioFormat, AudioHandler, AudioSession, RemoteCommand, RemoteControl, SourceAudioCodec,
    TrackMetadata,
};
#[cfg(not(target_os = "android"))]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
#[cfg(target_os = "android")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
#[cfg(target_os = "android")]
use std::thread;
#[cfg(target_os = "android")]
#[cfg(target_os = "android")]
use std::time::Duration;

/// Which protocol produced a PCM block. Used by host-side unified output
/// engines (e.g. the fnOS multi-device ALSA engine) for status and arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PcmSource {
    AirPlay,
    Dlna,
    MiPlay,
}

impl PcmSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AirPlay => "airplay",
            Self::Dlna => "dlna",
            Self::MiPlay => "miplay",
        }
    }
}

/// Optional external PCM destination.
///
/// When a host installs one via [`Settings::audio_output`] (fnOS), the core no
/// longer opens a local cpal stream: it forwards decoded AirPlay PCM to this
/// sink together with the sender's logical volume (linear 0..=1). Android and
/// desktop hosts that do not set it keep the existing cpal/Oboe behaviour.
pub trait PcmOutput: Send + Sync {
    fn write(
        &self,
        source: PcmSource,
        epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    );
    fn flush(&self, source: PcmSource);
}

/// Maximum decoded PCM waiting for the Windows callback.
///
/// Network playout is already timestamped and buffered by the AirPlay/DLNA
/// transports. Keeping another multi-second queue here can only make sound
/// (and therefore sender-side lyrics) drift behind after a scheduler stall.
/// The timed AirPlay playout layer has already absorbed network jitter before
/// samples reach this queue. Keep only a short WASAPI scheduling cushion here:
/// a larger queue can remain permanently full after one scheduler stall and
/// make the audible output (not the sender's lyric clock) trail by a quarter
/// second. Overflow always discards the oldest samples so sound catches up.
#[cfg(not(target_os = "android"))]
const OUTPUT_BACKLOG_MILLISECONDS: usize = 80;
/// Android can briefly delay even a high-priority Oboe callback while the
/// system changes routes, restores an AudioTrack, or schedules heavy UI work.
/// 1.4.0 was the last release that produced audible AirPlay output on ColorOS
/// with a 240 ms cushion; 1.4.1's 480 ms / 240 ms cushion (with a larger
/// prefill) never primed on the same device, so keep the known-good values.
#[cfg(target_os = "android")]
const OUTPUT_BACKLOG_MILLISECONDS: usize = 240;
/// Amount of decoded PCM collected before the platform output starts draining.
///
/// AirPlay realtime audio arrives in short network packets. Starting from the
/// first packet exposes normal Wi-Fi scheduling jitter as repeated gaps. This
/// small cushion also lets Android's Oboe callback recover cleanly after an
/// underrun without adding a large steady-state delay.
#[cfg(not(target_os = "android"))]
const OUTPUT_PREFILL_MILLISECONDS: usize = 40;
/// A larger one-time/recovery fill prevents a short scheduling delay at the
/// start of a connection from becoming a repeating fill/drain cycle.
#[cfg(target_os = "android")]
const OUTPUT_PREFILL_MILLISECONDS: usize = 120;

struct AudioBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
    prefill: usize,
    primed: bool,
    /// True while the output is waiting to build the initial/recovery prefill.
    waiting_for_prefill: bool,
    /// Number of times the queue ran dry while output was primed.
    underruns: u64,
    /// Number of times output had to wait for the prefill cushion again.
    prefill_waits: u64,
    /// Number of successful producer pushes.
    pushes: u64,
    /// Number of times the queue was cleared (new stream / flush / restart).
    clears: u64,
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    channels: usize,
}

/// Snapshot of the local output cushion, used for rate-limited diagnostics.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AudioBufferStats {
    underruns: u64,
    prefill_waits: u64,
    buffered_frames: usize,
    capacity_frames: usize,
    prefill_frames: usize,
    /// True once the output has started draining real PCM.
    primed: bool,
    /// Producer pushes / queue clears since startup.
    pushes: u64,
    clears: u64,
}

impl AudioBuffer {
    fn new(sample_rate: u32, channels: u16) -> Self {
        Self::new_with_limits(
            sample_rate,
            channels,
            OUTPUT_BACKLOG_MILLISECONDS,
            OUTPUT_PREFILL_MILLISECONDS,
        )
    }

    /// Build a buffer with explicit millisecond limits (also used by tests).
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    fn new_with_limits(
        sample_rate: u32,
        channels: u16,
        backlog_milliseconds: usize,
        prefill_milliseconds: usize,
    ) -> Self {
        let channels = channels.max(1) as usize;
        let capacity = sample_rate as usize * channels * backlog_milliseconds / 1_000;
        let prefill = sample_rate as usize * channels * prefill_milliseconds / 1_000;
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            prefill: prefill.clamp(1, capacity.max(1)),
            primed: false,
            waiting_for_prefill: false,
            underruns: 0,
            prefill_waits: 0,
            pushes: 0,
            clears: 0,
            channels,
        }
    }

    fn push(&mut self, input: &[f32]) {
        self.pushes = self.pushes.saturating_add(1);
        if input.len() >= self.capacity {
            self.samples.clear();
            self.samples.extend(
                input[input.len().saturating_sub(self.capacity)..]
                    .iter()
                    .copied(),
            );
            return;
        }
        let overflow = self
            .samples
            .len()
            .saturating_add(input.len())
            .saturating_sub(self.capacity);
        if overflow > 0 {
            self.samples.drain(..overflow.min(self.samples.len()));
        }
        self.samples.extend(input.iter().copied());
    }

    fn pop(&mut self) -> f32 {
        if !self.primed {
            if self.samples.len() < self.prefill {
                if !self.waiting_for_prefill {
                    self.waiting_for_prefill = true;
                    self.prefill_waits = self.prefill_waits.saturating_add(1);
                }
                return 0.0;
            }
            self.primed = true;
            self.waiting_for_prefill = false;
        }
        match self.samples.pop_front() {
            Some(sample) => sample,
            None => {
                // Ran dry while audible: the next output resumes only after the
                // full prefill cushion has been rebuilt.
                self.primed = false;
                self.underruns = self.underruns.saturating_add(1);
                0.0
            }
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.primed = false;
        self.waiting_for_prefill = false;
        self.clears = self.clears.saturating_add(1);
    }

    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    fn stats(&self) -> AudioBufferStats {
        AudioBufferStats {
            underruns: self.underruns,
            prefill_waits: self.prefill_waits,
            buffered_frames: self.samples.len() / self.channels,
            capacity_frames: self.capacity / self.channels,
            prefill_frames: self.prefill / self.channels,
            primed: self.primed,
            pushes: self.pushes,
            clears: self.clears,
        }
    }
}

pub struct AudioRuntime {
    pub handler: Arc<ReceiverAudioHandler>,
    #[cfg(not(target_os = "android"))]
    pub _stream: OutputHandle,
    #[cfg(target_os = "android")]
    pub _stream: AndroidAudioOutput,
    pub device_name: String,
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: &'static str,
    pub bits_per_sample: u8,
}

/// Owns the local output resource for a running receiver.
///
/// `Cpal` is the original desktop/Windows path. `External` is a host-installed
/// [`PcmOutput`] (fnOS): the cpal callback is replaced by a pump thread that
/// drains the same `AudioBuffer` and calls the sink.
#[cfg(not(target_os = "android"))]
pub enum OutputHandle {
    Cpal(cpal::Stream),
    External {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    },
}

#[cfg(not(target_os = "android"))]
impl Drop for OutputHandle {
    fn drop(&mut self) {
        if let OutputHandle::External { stop, handle } = self {
            stop.store(true, Ordering::Release);
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(target_os = "android")]
type AndroidAudioStream = oboe::AudioStreamAsync<oboe::Output, AndroidAudioCallback>;

#[cfg(target_os = "android")]
enum AndroidAudioCommand {
    Restart(OboeError),
    Shutdown,
}

#[cfg(target_os = "android")]
pub struct AndroidAudioOutput {
    commands: mpsc::Sender<AndroidAudioCommand>,
}

#[cfg(target_os = "android")]
impl Drop for AndroidAudioOutput {
    fn drop(&mut self) {
        let _ = self.commands.send(AndroidAudioCommand::Shutdown);
    }
}

#[cfg(target_os = "android")]
pub struct AndroidAudioCallback {
    buffer: Arc<Mutex<AudioBuffer>>,
    gain_bits: Arc<AtomicU32>,
    commands: mpsc::Sender<AndroidAudioCommand>,
}

#[cfg(target_os = "android")]
impl AudioOutputCallback for AndroidAudioCallback {
    type FrameType = (f32, Stereo);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn AudioOutputStreamSafe,
        frames: &mut [(f32, f32)],
    ) -> DataCallbackResult {
        let gain = f32::from_bits(self.gain_bits.load(Ordering::Relaxed));
        // The producer only holds this lock while copying into an 80 ms
        // bounded queue. Waiting for that short critical section is safer than
        // replacing a whole hardware callback with silence; repeated try-lock
        // misses were able to leave Android reporting playback with no sound.
        if let Ok(mut buffer) = self.buffer.lock() {
            for (left, right) in frames {
                *left = buffer.pop() * gain;
                *right = buffer.pop() * gain;
            }
        } else {
            frames.fill((0.0, 0.0));
        }
        DataCallbackResult::Continue
    }

    fn on_error_after_close(&mut self, _stream: &mut dyn AudioOutputStreamSafe, error: OboeError) {
        let _ = self.commands.send(AndroidAudioCommand::Restart(error));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutputDevice {
    pub name: String,
    pub id: String,
    pub is_default: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: &'static str,
    pub bits_per_sample: u8,
}

/// Enumerates every usable Windows render endpoint for the settings page.
/// One broken endpoint must not hide the remaining speakers, HDMI sinks,
/// Bluetooth devices, or virtual render devices.
#[cfg(not(target_os = "android"))]
pub fn list_output_devices() -> Result<Vec<AudioOutputDevice>> {
    let host = cpal::default_host();
    let default_id = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let devices = host
        .output_devices()
        .context("无法枚举 Windows 音频输出设备")?;
    let mut seen = HashSet::new();
    let mut outputs = Vec::new();
    for device in devices {
        let Ok(id) = device.id().map(|id| id.to_string()) else {
            continue;
        };
        if !seen.insert(id.to_ascii_lowercase()) {
            continue;
        }
        let Ok(description) = device.description() else {
            continue;
        };
        let Ok(configuration) = device.default_output_config() else {
            continue;
        };
        let (sample_format, bits_per_sample) = output_format_details(configuration.sample_format());
        outputs.push(AudioOutputDevice {
            name: description.name().to_owned(),
            is_default: default_id
                .as_ref()
                .is_some_and(|default| default.eq_ignore_ascii_case(&id)),
            id,
            sample_rate: configuration.sample_rate(),
            channels: configuration.channels(),
            sample_format,
            bits_per_sample,
        });
    }
    outputs.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(outputs)
}

#[cfg(target_os = "android")]
pub fn list_output_devices() -> Result<Vec<AudioOutputDevice>> {
    Ok(vec![AudioOutputDevice {
        name: "Android 系统音频输出".to_owned(),
        id: "android-default".to_owned(),
        is_default: true,
        sample_rate: 48_000,
        channels: 2,
        sample_format: "float",
        bits_per_sample: 32,
    }])
}

#[cfg(not(target_os = "android"))]
fn output_format_details(sample_format: cpal::SampleFormat) -> (&'static str, u8) {
    match sample_format {
        cpal::SampleFormat::F32 => ("float", 32),
        cpal::SampleFormat::I16 => ("signed", 16),
        cpal::SampleFormat::U16 => ("unsigned", 16),
        _ => ("unknown", 0),
    }
}

#[cfg(not(target_os = "android"))]
pub fn start_audio(
    events: Arc<EventSink>,
    arbiter: Arc<PlaybackArbiter>,
    artwork_dir: PathBuf,
    requested_device: Option<&str>,
) -> Result<AudioRuntime> {
    let host = cpal::default_host();
    let device = match requested_device {
        Some(requested) => host
            .output_devices()
            .context("无法枚举 Windows 音频输出设备")?
            .find(|candidate| {
                candidate
                    .id()
                    .map(|id| id.to_string().eq_ignore_ascii_case(requested))
                    .unwrap_or(false)
                    || candidate
                        .description()
                        .map(|description| description.name().eq_ignore_ascii_case(requested))
                        .unwrap_or(false)
            })
            .with_context(|| format!("找不到指定的 Windows 音频输出设备：{requested}"))?,
        None => host
            .default_output_device()
            .context("Windows 没有可用的默认音频输出设备")?,
    };
    let supported = device
        .default_output_config()
        .context("无法读取默认音频输出格式")?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let (sample_format_name, bits_per_sample) = match sample_format {
        cpal::SampleFormat::F32 => ("float", 32),
        cpal::SampleFormat::I16 => ("signed", 16),
        cpal::SampleFormat::U16 => ("unsigned", 16),
        unsupported => bail!("暂不支持 Windows 默认音频格式：{unsupported}"),
    };
    let device_name = device
        .description()
        .map(|description| description.name().to_owned())
        .unwrap_or_else(|_| "Windows 默认扬声器".to_owned());
    let device_id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| device_name.clone());

    let config = cpal::StreamConfig {
        channels,
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let buffer = Arc::new(Mutex::new(AudioBuffer::new(sample_rate, channels)));
    let gain_bits = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
    let volume_db_bits = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let pcm_received = Arc::new(AtomicU64::new(0));
    let buffer_lock_failures = Arc::new(AtomicU64::new(0));

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_output_stream::<f32>(
            &device,
            &config,
            Arc::clone(&buffer),
            Arc::clone(&gain_bits),
            Arc::clone(&events),
        ),
        cpal::SampleFormat::I16 => build_output_stream::<i16>(
            &device,
            &config,
            Arc::clone(&buffer),
            Arc::clone(&gain_bits),
            Arc::clone(&events),
        ),
        cpal::SampleFormat::U16 => build_output_stream::<u16>(
            &device,
            &config,
            Arc::clone(&buffer),
            Arc::clone(&gain_bits),
            Arc::clone(&events),
        ),
        unsupported => bail!("暂不支持 Windows 默认音频格式：{unsupported}"),
    }
    .context("无法打开 Windows 默认音频输出")?;
    stream.play().context("无法启动 Windows 音频输出")?;

    let handler = Arc::new(ReceiverAudioHandler {
        buffer,
        gain_bits,
        volume_db_bits,
        pcm_received,
        buffer_lock_failures,
        events,
        arbiter,
        artwork_dir,
        output_channels: channels as usize,
        progress_timebase: AtomicU32::new(0),
        remote_controls: Mutex::new(RemoteControls::default()),
        client_connections: Mutex::new(ClientConnections::default()),
        current_session: Arc::new(Mutex::new(None)),
    });

    Ok(AudioRuntime {
        handler,
        _stream: OutputHandle::Cpal(stream),
        device_name,
        device_id,
        sample_rate,
        channels,
        sample_format: sample_format_name,
        bits_per_sample,
    })
}

/// Start the receiver without a local output device: decoded PCM is forwarded
/// to a host-installed [`PcmOutput`] (fnOS unified engine). The existing
/// handler (arbitration, volume, metadata, remote control) is unchanged; only
/// the cpal callback is replaced by a pump thread.
#[cfg(not(target_os = "android"))]
pub fn start_audio_external(
    events: Arc<EventSink>,
    arbiter: Arc<PlaybackArbiter>,
    artwork_dir: PathBuf,
    output: Arc<dyn PcmOutput>,
) -> Result<AudioRuntime> {
    const EXTERNAL_RATE: u32 = 48_000;
    const EXTERNAL_CHANNELS: u16 = 2;
    let buffer = Arc::new(Mutex::new(AudioBuffer::new(EXTERNAL_RATE, EXTERNAL_CHANNELS)));
    let gain_bits = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
    let volume_db_bits = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let pcm_received = Arc::new(AtomicU64::new(0));
    let buffer_lock_failures = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let pump = {
        let buffer = Arc::clone(&buffer);
        let gain_bits = Arc::clone(&gain_bits);
        let stop = Arc::clone(&stop);
        let output = Arc::clone(&output);
        std::thread::Builder::new()
            .name("fusionplay-external-pump".to_owned())
            .spawn(move || {
                let channels = EXTERNAL_CHANNELS as usize;
                let max_frames = (EXTERNAL_RATE as usize) / 50; // 20 ms per write
                let mut scratch: Vec<f32> = Vec::with_capacity(max_frames * channels);
                while !stop.load(Ordering::Acquire) {
                    scratch.clear();
                    let sender_gain = f32::from_bits(gain_bits.load(Ordering::Relaxed));
                    if let Ok(mut queued) = buffer.lock() {
                        let take = queued.samples.len().min(max_frames * channels);
                        scratch.extend(queued.samples.drain(..take));
                    }
                    if scratch.is_empty() {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                    output.write(
                        PcmSource::AirPlay,
                        0,
                        sender_gain,
                        EXTERNAL_RATE,
                        EXTERNAL_CHANNELS,
                        &scratch,
                    );
                }
                output.flush(PcmSource::AirPlay);
            })
            .context("无法启动外部音频输出线程")?
    };

    let handler = Arc::new(ReceiverAudioHandler {
        buffer,
        gain_bits,
        volume_db_bits,
        pcm_received,
        buffer_lock_failures,
        events,
        arbiter,
        artwork_dir,
        output_channels: EXTERNAL_CHANNELS as usize,
        progress_timebase: AtomicU32::new(0),
        remote_controls: Mutex::new(RemoteControls::default()),
        client_connections: Mutex::new(ClientConnections::default()),
        current_session: Arc::new(Mutex::new(None)),
    });

    Ok(AudioRuntime {
        handler,
        _stream: OutputHandle::External { stop, handle: Some(pump) },
        device_name: "统一音频引擎（fnOS）".to_owned(),
        device_id: "fusionplay-unified".to_owned(),
        sample_rate: EXTERNAL_RATE,
        channels: EXTERNAL_CHANNELS,
        sample_format: "float",
        bits_per_sample: 32,
    })
}

#[cfg(target_os = "android")]
pub fn start_audio(
    events: Arc<EventSink>,
    arbiter: Arc<PlaybackArbiter>,
    artwork_dir: PathBuf,
    _requested_device: Option<&str>,
) -> Result<AudioRuntime> {
    let sample_rate = 48_000;
    let channels = 2;
    let buffer = Arc::new(Mutex::new(AudioBuffer::new(sample_rate, channels)));
    let gain_bits = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
    let volume_db_bits = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
    let pcm_received = Arc::new(AtomicU64::new(0));
    let buffer_lock_failures = Arc::new(AtomicU64::new(0));
    let (commands, command_receiver) = mpsc::channel();
    let initial_stream = open_android_audio_stream(
        Arc::clone(&buffer),
        Arc::clone(&gain_bits),
        commands.clone(),
        sample_rate,
    )?;
    let output_events = Arc::clone(&events);
    let output_buffer = Arc::clone(&buffer);
    let output_gain = Arc::clone(&gain_bits);
    let output_commands = commands.clone();
    let pcm_received_monitor = Arc::clone(&pcm_received);
    let lock_failures_monitor = Arc::clone(&buffer_lock_failures);
    thread::Builder::new()
        .name("airplay-audio-route".to_owned())
        .spawn(move || {
            manage_android_audio_output(
                initial_stream,
                command_receiver,
                output_commands,
                output_buffer,
                output_gain,
                output_events,
                sample_rate,
                Arc::clone(&pcm_received_monitor),
                Arc::clone(&lock_failures_monitor),
            );
        })
        .context("无法启动 Android 音频路由监控")?;
    let stream = AndroidAudioOutput { commands };

    let handler = Arc::new(ReceiverAudioHandler {
        buffer,
        gain_bits,
        volume_db_bits,
        pcm_received,
        buffer_lock_failures,
        events,
        arbiter,
        artwork_dir,
        output_channels: channels as usize,
        progress_timebase: AtomicU32::new(0),
        remote_controls: Mutex::new(RemoteControls::default()),
        client_connections: Mutex::new(ClientConnections::default()),
        current_session: Arc::new(Mutex::new(None)),
    });

    Ok(AudioRuntime {
        handler,
        _stream: stream,
        device_name: "Android 系统音频输出".to_owned(),
        device_id: "android-default".to_owned(),
        sample_rate,
        channels,
        sample_format: "float",
        bits_per_sample: 32,
    })
}

#[cfg(target_os = "android")]
fn open_android_audio_stream(
    buffer: Arc<Mutex<AudioBuffer>>,
    gain_bits: Arc<AtomicU32>,
    commands: mpsc::Sender<AndroidAudioCommand>,
    sample_rate: u32,
) -> Result<AndroidAudioStream> {
    let callback = AndroidAudioCallback {
        buffer,
        gain_bits,
        commands,
    };
    // 1.4.1 switched to `PerformanceMode::None` together with a 480/240 ms
    // cushion and ColorOS then produced no sound at all. Restore the 1.4.0
    // known-good settings: the protocol already buffers 2 s, so the small
    // LowLatency hardware buffer plus the 240/120 ms app cushion is what last
    // produced audible output on these devices.
    let mut stream = AudioStreamBuilder::default()
        .set_usage(Usage::Media)
        .set_content_type(ContentType::Music)
        .set_performance_mode(PerformanceMode::LowLatency)
        .set_sharing_mode(SharingMode::Shared)
        .set_sample_rate(sample_rate as i32)
        .set_sample_rate_conversion_quality(SampleRateConversionQuality::Medium)
        .set_format_conversion_allowed(true)
        .set_format::<f32>()
        .set_channel_count::<Stereo>()
        .set_callback(callback)
        .open_stream()
        .context("无法打开 Android 音频输出")?;
    stream.start().context("无法启动 Android 音频输出")?;
    Ok(stream)
}

#[cfg(target_os = "android")]
fn manage_android_audio_output(
    initial_stream: AndroidAudioStream,
    receiver: mpsc::Receiver<AndroidAudioCommand>,
    commands: mpsc::Sender<AndroidAudioCommand>,
    buffer: Arc<Mutex<AudioBuffer>>,
    gain_bits: Arc<AtomicU32>,
    events: Arc<EventSink>,
    sample_rate: u32,
    pcm_received: Arc<AtomicU64>,
    buffer_lock_failures: Arc<AtomicU64>,
) {
    let mut stream = Some(initial_stream);
    // One startup line so the next exported log shows the active cushion and
    // Oboe mode even when playback is otherwise healthy.
    if let Ok(buffer) = buffer.lock() {
        let stats = buffer.stats();
        let message = format!(
            "Android 音频输出已就绪：Oboe 低延迟模式，本地缓冲 {} ms（容量 {} ms，{} 帧），采样率 {} Hz",
            stats.prefill_frames * 1_000 / sample_rate.max(1) as usize,
            stats.capacity_frames * 1_000 / sample_rate.max(1) as usize,
            stats.prefill_frames,
            sample_rate
        );
        events.emit(CoreEvent::Log {
            level: "info",
            message: &message,
        });
    }
    let mut last_stats = AudioBufferStats::default();
    let mut announced_start = false;
    let mut last_heartbeat = std::time::Instant::now();
    loop {
        let command = match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Rate-limited observability: report only when an underrun or
                // a prefill restart actually happened since the last report.
                let stats = buffer.lock().map(|buffer| buffer.stats()).unwrap_or_default();
                if stats.primed && !announced_start {
                    announced_start = true;
                    let message = format!(
                        "Android 音频输出已开始播放：当前缓冲 {:.0} ms / 容量 {:.0} ms",
                        stats.buffered_frames as f64 * 1_000.0 / sample_rate.max(1) as f64,
                        stats.capacity_frames as f64 * 1_000.0 / sample_rate.max(1) as f64
                    );
                    events.emit(CoreEvent::Log {
                        level: "info",
                        message: &message,
                    });
                }
                // 10 s heartbeat: shows whether decoded PCM is reaching the
                // local queue (push count / total samples) and whether the
                // producer or the consumer is the bottleneck.
                if last_heartbeat.elapsed() >= std::time::Duration::from_secs(10) {
                    last_heartbeat = std::time::Instant::now();
                    let message = format!(
                        "Android 输出诊断：已收 PCM {} 采样，push {} 次，clear {} 次，锁失败 {} 次，underrun {}，预填充 {}，当前缓冲 {:.0} ms / 容量 {:.0} ms",
                        pcm_received.load(Ordering::Relaxed),
                        stats.pushes,
                        stats.clears,
                        buffer_lock_failures.load(Ordering::Relaxed),
                        stats.underruns,
                        stats.prefill_waits,
                        stats.buffered_frames as f64 * 1_000.0 / sample_rate.max(1) as f64,
                        stats.capacity_frames as f64 * 1_000.0 / sample_rate.max(1) as f64
                    );
                    events.emit(CoreEvent::Log {
                        level: "info",
                        message: &message,
                    });
                }
                if stats.underruns > last_stats.underruns
                    || stats.prefill_waits > last_stats.prefill_waits
                {
                    let message = format!(
                        "Android 音频输出欠载：累计 underrun {} 次、重新预填充 {} 次，当前缓冲 {:.0} ms / 容量 {:.0} ms",
                        stats.underruns,
                        stats.prefill_waits,
                        stats.buffered_frames as f64 * 1_000.0 / sample_rate.max(1) as f64,
                        stats.capacity_frames as f64 * 1_000.0 / sample_rate.max(1) as f64
                    );
                    events.emit(CoreEvent::Log {
                        level: "warning",
                        message: &message,
                    });
                    last_stats = stats;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            AndroidAudioCommand::Shutdown => break,
            AndroidAudioCommand::Restart(error) => {
                stream.take();
                if let Ok(mut queued) = buffer.lock() {
                    queued.clear();
                }
                let message = format!("Android 音频路由已变化，正在恢复输出：{error}");
                events.emit(CoreEvent::Log {
                    level: "warning",
                    message: &message,
                });
                for delay_millis in ANDROID_AUDIO_RESTART_DELAYS_MILLIS {
                    thread::sleep(Duration::from_millis(delay_millis));
                    match open_android_audio_stream(
                        Arc::clone(&buffer),
                        Arc::clone(&gain_bits),
                        commands.clone(),
                        sample_rate,
                    ) {
                        Ok(replacement) => {
                            stream = Some(replacement);
                            events.emit(CoreEvent::Log {
                                level: "info",
                                message: "Android 音频输出已切换到当前系统路由",
                            });
                            break;
                        }
                        Err(_) => continue,
                    }
                }
                if stream.is_none() {
                    events.emit(CoreEvent::Error {
                        message: "Android 音频路由切换后无法恢复输出",
                    });
                    break;
                }
                last_stats = buffer.lock().map(|buffer| buffer.stats()).unwrap_or_default();
            }
        }
    }
}

#[cfg(target_os = "android")]
const ANDROID_AUDIO_RESTART_DELAYS_MILLIS: [u64; 6] = [50, 100, 200, 400, 800, 1_600];

#[cfg(not(target_os = "android"))]
fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    output_buffer: Arc<Mutex<AudioBuffer>>,
    output_gain: Arc<AtomicU32>,
    output_events: Arc<EventSink>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let gain = f32::from_bits(output_gain.load(Ordering::Relaxed));
            // A producer now holds this lock only for the final bounded queue
            // append. Waiting for that very short section is preferable to
            // replacing an entire WASAPI period with silence on contention.
            if let Ok(mut buffer) = output_buffer.lock() {
                for sample in data {
                    *sample = T::from_sample(buffer.pop() * gain);
                }
            } else {
                for sample in data {
                    *sample = T::from_sample(0.0);
                }
            }
        },
        move |error| {
            let message = format!("Windows 音频输出错误：{error}");
            output_events.emit(CoreEvent::Error { message: &message });
        },
        None,
    )
}

pub struct ReceiverAudioHandler {
    buffer: Arc<Mutex<AudioBuffer>>,
    gain_bits: Arc<AtomicU32>,
    /// Receiver volume in dB (0.0 = full scale, -144.0 = mute), reported to the
    /// sender by `GET_PARAMETER volume` and updated by both the sender's
    /// `SET_PARAMETER volume` and the local UI volume.
    volume_db_bits: Arc<AtomicU32>,
    /// Cumulative decoded PCM samples handed to the local output queue.
    pcm_received: Arc<AtomicU64>,
    /// Times the output queue mutex could not be taken in `audio_process`.
    buffer_lock_failures: Arc<AtomicU64>,
    events: Arc<EventSink>,
    arbiter: Arc<PlaybackArbiter>,
    artwork_dir: PathBuf,
    output_channels: usize,
    progress_timebase: AtomicU32,
    remote_controls: Mutex<RemoteControls>,
    client_connections: Mutex<ClientConnections>,
    /// Last logical AirPlay session, retained across a sender's temporary
    /// audio-stream teardown so pause/resume does not discard now-playing data.
    current_session: Arc<Mutex<Option<Arc<AudioSessionState>>>>,
}

struct AudioSessionState {
    lease: Mutex<MediaLease>,
    suspended: AtomicBool,
    format: AudioFormat,
    metadata: Mutex<Option<TrackMetadata>>,
    cover_art_path: Mutex<Option<String>>,
    progress: Mutex<Option<(u64, u64)>>,
}

impl AudioSessionState {
    fn new(lease: MediaLease, format: AudioFormat, cached: Option<&AudioSessionState>) -> Self {
        let suspended = cached.is_some_and(|state| state.suspended.load(Ordering::Acquire));
        Self {
            lease: Mutex::new(lease),
            suspended: AtomicBool::new(suspended),
            format,
            metadata: Mutex::new(
                cached
                    .and_then(|state| state.metadata.lock().ok())
                    .and_then(|metadata| metadata.clone()),
            ),
            cover_art_path: Mutex::new(
                cached
                    .and_then(|state| state.cover_art_path.lock().ok())
                    .and_then(|path| path.clone()),
            ),
            progress: Mutex::new(
                cached
                    .and_then(|state| state.progress.lock().ok())
                    .and_then(|progress| *progress),
            ),
        }
    }

    fn has_cached_media(&self) -> bool {
        self.metadata
            .lock()
            .is_ok_and(|metadata| metadata.is_some())
            || self
                .cover_art_path
                .lock()
                .is_ok_and(|cover_art| cover_art.is_some())
            || self
                .progress
                .lock()
                .is_ok_and(|progress| progress.is_some())
    }

    fn lease(&self) -> Option<MediaLease> {
        self.lease.lock().ok().map(|lease| *lease)
    }

    fn replace_lease(&self, lease: MediaLease) {
        if let Ok(mut current) = self.lease.lock() {
            *current = lease;
        }
    }
}

#[derive(Default)]
struct RemoteControls {
    dacp: Option<Arc<dyn RemoteControl>>,
    media_remote: Option<Arc<dyn RemoteControl>>,
}

impl RemoteControls {
    fn update(&mut self, remote: Arc<dyn RemoteControl>, available: bool) {
        let slot = if remote.transport_name() == "dacp" {
            &mut self.dacp
        } else {
            &mut self.media_remote
        };

        if available {
            *slot = Some(remote);
        } else if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &remote))
        {
            slot.take();
        }
    }

    fn preferred(&self) -> Option<Arc<dyn RemoteControl>> {
        self.dacp.as_ref().or(self.media_remote.as_ref()).cloned()
    }

    fn candidates(&self) -> Vec<Arc<dyn RemoteControl>> {
        self.dacp
            .iter()
            .chain(self.media_remote.iter())
            .cloned()
            .collect()
    }

    fn ui_state(&self) -> Option<(Vec<RemoteCommand>, &'static str)> {
        let preferred = self.preferred()?;
        let mut available = Vec::new();
        for remote in self.candidates() {
            for command in remote.available_commands() {
                if !available.contains(&command) {
                    available.push(command);
                }
            }
        }
        Some((available, preferred.transport_name()))
    }
}

/// Tracks the physical RTSP/HTTP sockets that belong to the logical AirPlay
/// client session.
///
/// AirPlay 2 routinely opens separate media, event and feedback connections.
/// Closing one auxiliary socket after a MediaRemote pause command must not be
/// presented as the sender disconnecting while other sockets are still alive.
#[derive(Default)]
struct ClientConnections {
    counts: HashMap<String, usize>,
    total: usize,
}

impl ClientConnections {
    /// Returns true when this is the first socket of a logical client session.
    fn connected(&mut self, address: &str) -> bool {
        *self.counts.entry(address.to_owned()).or_default() += 1;
        self.total += 1;
        self.total == 1
    }

    /// Returns true only when the final socket of the logical session closed.
    fn disconnected(&mut self, address: &str) -> bool {
        let Some(count) = self.counts.get_mut(address) else {
            return false;
        };
        *count -= 1;
        self.total = self.total.saturating_sub(1);
        if *count == 0 {
            self.counts.remove(address);
        }
        self.total == 0
    }
}

impl ReceiverAudioHandler {
    fn current_session(&self) -> Option<Arc<AudioSessionState>> {
        self.current_session
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    fn emit_stream_started(&self, state: &AudioSessionState, lease: MediaLease) {
        let format = state.format;
        let source_codec = format.source.map(|source| match source.codec {
            SourceAudioCodec::Alac => "alac",
            SourceAudioCodec::Aac => "aac",
        });
        self.events.emit(CoreEvent::StreamStarted {
            source: "airplay",
            epoch: lease.epoch(),
            source_codec,
            source_sample_rate: format.source.and_then(|source| source.sample_rate),
            source_channels: format.source.and_then(|source| source.channels),
            source_bits: format.source.and_then(|source| source.bits),
            decoded_sample_rate: format.sample_rate,
            decoded_channels: format.channels,
            decoded_bits: format.bits,
        });
    }

    fn emit_cached_session_state(&self, state: &AudioSessionState, lease: MediaLease) {
        if let Some(metadata) = state
            .metadata
            .lock()
            .ok()
            .and_then(|metadata| metadata.clone())
        {
            self.events.emit(CoreEvent::NowPlaying {
                source: "airplay",
                epoch: lease.epoch(),
                title: metadata.title.as_deref(),
                artist: metadata.artist.as_deref(),
                album: metadata.album.as_deref(),
                genre: metadata.genre.as_deref(),
                duration_ms: metadata.duration_ms,
            });
        }
        if let Some(path) = state
            .cover_art_path
            .lock()
            .ok()
            .and_then(|path| path.clone())
        {
            self.events.emit(CoreEvent::CoverArt {
                source: "airplay",
                epoch: lease.epoch(),
                path: &path,
            });
        }
        if let Some((position_ms, duration_ms)) =
            state.progress.lock().ok().and_then(|progress| *progress)
        {
            self.events.emit(CoreEvent::Progress {
                source: "airplay",
                epoch: lease.epoch(),
                position_ms,
                duration_ms,
            });
        }
    }

    fn request_remote_pause_best_effort(&self) {
        let candidates = self
            .remote_controls
            .lock()
            .map(|controls| controls.candidates())
            .unwrap_or_default();
        // A suspender runs while the arbiter transition lock is held. Remote
        // transports may block or synchronously trigger playback callbacks, so
        // never perform their I/O inside that critical section.
        let _ = std::thread::Builder::new()
            .name("airplay-pause".to_owned())
            .spawn(move || {
                for remote in candidates {
                    if !remote_supports_command(&remote.available_commands(), &RemoteCommand::Pause)
                    {
                        continue;
                    }
                    if remote.send_command(RemoteCommand::Pause).is_ok() {
                        break;
                    }
                }
            });
    }

    /// Apply a UI volume percentage (0..=100) to the receiver's local output
    /// gain and remember the matching attenuation for sender volume queries.
    ///
    /// AirPlay volume belongs to the receiver: Apple TV/HomePod-style senders
    /// never expose a transport `SetVolume`, so treating this as a remote
    /// command made the local volume control unusable.
    pub fn apply_local_volume_percent(&self, percent: u8, lease: Option<MediaLease>) {
        let percent = percent.min(100);
        let gain = f32::from(percent as u16) / 100.0;
        let db = if percent == 0 {
            -144.0
        } else {
            20.0 * gain.log10()
        };
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
        self.volume_db_bits.store(db.to_bits(), Ordering::Relaxed);
        let Some(lease) = lease else {
            return;
        };
        self.arbiter.run_if_current(lease, || {
            self.events.emit(CoreEvent::Volume {
                source: "airplay",
                epoch: lease.epoch(),
                db,
                percent,
            });
        });
    }

    pub fn send_remote_command(&self, command: RemoteCommand) -> Result<()> {
        let active_lease = self
            .arbiter
            .current_lease(MediaSource::AirPlayAudio)
            .or_else(|| self.arbiter.current_lease(MediaSource::AirPlayVideo));
        if active_lease.is_none() && self.arbiter.current_source().is_some() {
            bail!("AirPlay is not the active playback source");
        }
        let event_lease =
            active_lease.or_else(|| self.current_session().and_then(|session| session.lease()));
        // Resolve a UI toggle against the core's latest projected state before
        // crossing into DACP/MediaRemote. Explicit Play/Pause commands are
        // idempotent when a user presses rapidly, and they keep the sender and
        // receiver on the same desired state even if an older RTSP callback is
        // still in flight on AirPlay's separate control connection.
        let transport_command = if command == RemoteCommand::PlayPause {
            self.current_session()
                .map(|session| {
                    if session.suspended.load(Ordering::Acquire) {
                        RemoteCommand::Play
                    } else {
                        RemoteCommand::Pause
                    }
                })
                .unwrap_or(RemoteCommand::PlayPause)
        } else {
            command
        };
        // AirPlay volume is receiver-side: apply it locally first and report
        // success. AP2 MediaRemote does not advertise SetVolume, and the old
        // failure path surfaced "当前投放设备不支持此远程控制命令" while the
        // local volume never changed.
        if let &RemoteCommand::SetVolume(percent) = &transport_command {
            self.apply_local_volume_percent(percent, event_lease);
            // AP1 DACP senders do advertise SetVolume; mirror the change
            // best-effort so their slider stays in sync. Failures are ignored
            // because the local gain already took effect.
            if let Ok(remotes) = self.remote_controls.lock() {
                for remote in remotes.candidates() {
                    if remote_supports_command(
                        &remote.available_commands(),
                        &RemoteCommand::SetVolume(percent),
                    ) {
                        let _ = remote.send_command(RemoteCommand::SetVolume(percent));
                    }
                }
            }
            return Ok(());
        }
        let candidates = self
            .remote_controls
            .lock()
            .map_err(|_| anyhow::anyhow!("远程控制状态不可用"))?
            .candidates();
        if candidates.is_empty() {
            self.events.emit(CoreEvent::RemoteControlUnavailable {
                source: Some("airplay"),
                epoch: event_lease.map(MediaLease::epoch),
                reason: "当前投放设备尚未提供远程控制通道",
            });
            bail!("当前投放设备尚未提供远程控制通道");
        }

        let mut last_error = None;
        let mut state_changed = false;
        let mut supported = false;
        for remote in candidates {
            if !remote_supports_command(&remote.available_commands(), &transport_command) {
                continue;
            }
            supported = true;
            match remote.send_command(transport_command.clone()) {
                Ok(()) => {
                    if state_changed {
                        self.emit_remote_control_state("远程控制通道不可用");
                    }
                    self.project_successful_remote_transport_command(
                        &transport_command,
                        event_lease,
                    );
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(format!(
                        "{} 远程控制通道已断开：{error}",
                        remote.transport_name()
                    ));
                    if let Ok(mut stored) = self.remote_controls.lock() {
                        stored.update(remote, false);
                    }
                    state_changed = true;
                }
            }
        }

        if let Some(message) = last_error {
            self.emit_remote_control_state(&message);
            bail!(message);
        }
        if !supported {
            bail!("当前投放设备不支持此远程控制命令");
        }
        bail!("远程控制命令发送失败");
    }

    fn project_successful_remote_transport_command(
        &self,
        command: &RemoteCommand,
        event_lease: Option<MediaLease>,
    ) {
        let Some(session) = self.current_session() else {
            return;
        };
        let playing = match command {
            RemoteCommand::Pause => {
                session.suspended.store(true, Ordering::Release);
                false
            }
            RemoteCommand::Play => {
                session.suspended.store(false, Ordering::Release);
                true
            }
            RemoteCommand::PlayPause => session.suspended.fetch_xor(true, Ordering::AcqRel),
            _ => return,
        };
        if !playing && let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        let Some(lease) = event_lease.or_else(|| session.lease()) else {
            return;
        };
        self.events.emit(CoreEvent::PlaybackState {
            source: "airplay",
            epoch: lease.epoch(),
            playing,
        });
    }

    fn emit_remote_control_state(&self, unavailable_reason: &str) {
        let lease = self
            .arbiter
            .current_lease(MediaSource::AirPlayAudio)
            .or_else(|| self.arbiter.current_lease(MediaSource::AirPlayVideo));
        let Some(lease) = lease else {
            return;
        };
        self.arbiter.run_if_current(lease, || {
            self.emit_remote_control_state_current(unavailable_reason, lease);
        });
    }

    fn emit_remote_control_state_current(&self, unavailable_reason: &str, lease: MediaLease) {
        let state = self
            .remote_controls
            .lock()
            .ok()
            .and_then(|stored| stored.ui_state());
        let Some((available, transport)) = state else {
            self.events.emit(CoreEvent::RemoteControlUnavailable {
                source: Some("airplay"),
                epoch: Some(lease.epoch()),
                reason: unavailable_reason,
            });
            return;
        };

        let commands = ui_remote_capabilities(&available);
        if commands.is_empty() {
            self.events.emit(CoreEvent::RemoteControlUnavailable {
                source: Some("airplay"),
                epoch: Some(lease.epoch()),
                reason: unavailable_reason,
            });
            return;
        }
        self.events.emit(CoreEvent::RemoteControlAvailable {
            source: "airplay",
            epoch: lease.epoch(),
            commands,
            transport,
            experimental: transport == "airplay2_mediaremote_experimental",
        });
    }

    pub fn suspend_for_takeover(&self, lease: MediaLease) {
        let Some(session) = self.current_session() else {
            return;
        };
        if session.lease() != Some(lease) {
            return;
        }
        session.suspended.store(true, Ordering::Release);
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        // Keep the RAOP/RTSP session alive. A supported sender is asked to
        // pause, while the local suspended flag is the immediate, reliable
        // single-output fence.
        self.request_remote_pause_best_effort();
        self.events.emit(CoreEvent::PlaybackState {
            source: "airplay",
            epoch: lease.epoch(),
            playing: false,
        });
    }
}

fn remote_supports_command(available: &[RemoteCommand], requested: &RemoteCommand) -> bool {
    if available.contains(requested) {
        return true;
    }
    if matches!(
        requested,
        RemoteCommand::Play | RemoteCommand::Pause | RemoteCommand::PlayPause
    ) {
        return available.iter().any(|command| {
            matches!(
                command,
                RemoteCommand::PlayPause | RemoteCommand::Play | RemoteCommand::Pause
            )
        });
    }
    match requested {
        RemoteCommand::SeekToPosition(_) => available
            .iter()
            .any(|command| matches!(command, RemoteCommand::SeekToPosition(_))),
        RemoteCommand::SetVolume(_) => available
            .iter()
            .any(|command| matches!(command, RemoteCommand::SetVolume(_))),
        _ => false,
    }
}

fn ui_remote_capabilities(available: &[RemoteCommand]) -> Vec<&'static str> {
    let mut commands = Vec::new();

    if available.iter().any(|command| {
        matches!(
            command,
            RemoteCommand::PlayPause | RemoteCommand::Play | RemoteCommand::Pause
        )
    }) {
        commands.push("play_pause");
    }
    if available
        .iter()
        .any(|command| matches!(command, RemoteCommand::PreviousTrack))
    {
        commands.push("previous_track");
    }
    if available
        .iter()
        .any(|command| matches!(command, RemoteCommand::NextTrack))
    {
        commands.push("next_track");
    }
    if available
        .iter()
        .any(|command| matches!(command, RemoteCommand::SeekToPosition(_)))
    {
        commands.push("seek");
    }

    commands
}

fn progress_milliseconds(start: u32, current: u32, end: u32, timebase: u32) -> Option<(u64, u64)> {
    if timebase == 0 {
        return None;
    }
    let position = current.wrapping_sub(start) as u64;
    let duration = end.wrapping_sub(start) as u64;
    // Apple senders can emit a zeroed progress sentinel while transitioning to
    // pause. It is not a seek-to-zero and must not erase the last known
    // position. Likewise, reject malformed positions beyond the track end.
    if duration == 0 || position > duration {
        return None;
    }
    Some((
        position.saturating_mul(1000) / u64::from(timebase),
        duration.saturating_mul(1000) / u64::from(timebase),
    ))
}


/// Report previously unseen DMAP tags (four-character tag + payload length)
/// through the event log, once per tag per process.
///
/// AirPlay's DMAP metadata has no documented lyrics tag; if iOS/macOS 27 or an
/// app starts sending one (e.g. `asly`), this rate-limited line makes it
/// visible in the next exported log without recording any metadata content.
fn report_unknown_dmap_tags(events: &EventSink, unknown_tags: &[(String, u32)]) {
    use std::collections::HashSet;
    use std::sync::OnceLock;

    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    const MAX_REPORTED_TAGS: usize = 256;
    if unknown_tags.is_empty() {
        return;
    }
    let Ok(mut reported) = REPORTED.get_or_init(|| Mutex::new(HashSet::new())).lock() else {
        return;
    };
    if reported.len() >= MAX_REPORTED_TAGS {
        return;
    }
    let mut fresh = Vec::new();
    for (tag, len) in unknown_tags {
        if reported.insert(tag.clone()) {
            fresh.push(format!("{tag}({len})"));
            if fresh.len() >= 32 || reported.len() >= MAX_REPORTED_TAGS {
                break;
            }
        }
    }
    drop(reported);
    if fresh.is_empty() {
        return;
    }
    let message = format!(
        "AirPlay 元数据包含未知 DMAP 标签（诊断，仅标签名+长度）：{}",
        fresh.join(", ")
    );
    events.emit(CoreEvent::Log {
        level: "info",
        message: &message,
    });
}

fn merge_track_metadata(
    current: Option<&TrackMetadata>,
    incoming: &TrackMetadata,
) -> (TrackMetadata, bool) {
    fn text(value: &Option<String>) -> Option<String> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    let sanitized = TrackMetadata {
        title: text(&incoming.title),
        artist: text(&incoming.artist),
        album: text(&incoming.album),
        genre: text(&incoming.genre),
        duration_ms: incoming.duration_ms.filter(|duration| *duration > 0),
        track_number: incoming.track_number.filter(|number| *number > 0),
        disc_number: incoming.disc_number.filter(|number| *number > 0),
        // Diagnostics are reported from `on_metadata`, never merged/persisted.
        unknown_tags: Vec::new(),
    };
    let identity_changed = current.is_some_and(|current| {
        let title_changed = sanitized
            .title
            .as_ref()
            .zip(text(&current.title).as_ref())
            .is_some_and(|(incoming, current)| incoming != current);
        let artist_changed = sanitized
            .artist
            .as_ref()
            .zip(text(&current.artist).as_ref())
            .is_some_and(|(incoming, current)| incoming != current)
            && sanitized.title.as_ref() == text(&current.title).as_ref();
        let track_number_changed = sanitized
            .track_number
            .zip(current.track_number)
            .is_some_and(|(incoming, current)| incoming != current);
        title_changed || artist_changed || track_number_changed
    });
    if identity_changed || current.is_none() {
        return (sanitized, identity_changed);
    }

    let mut merged = current.cloned().unwrap_or_default();
    if sanitized.title.is_some() {
        merged.title = sanitized.title;
    }
    if sanitized.artist.is_some() {
        merged.artist = sanitized.artist;
    }
    if sanitized.album.is_some() {
        merged.album = sanitized.album;
    }
    if sanitized.genre.is_some() {
        merged.genre = sanitized.genre;
    }
    if sanitized.duration_ms.is_some() {
        merged.duration_ms = sanitized.duration_ms;
    }
    if sanitized.track_number.is_some() {
        merged.track_number = sanitized.track_number;
    }
    if sanitized.disc_number.is_some() {
        merged.disc_number = sanitized.disc_number;
    }
    (merged, false)
}

impl AudioHandler for ReceiverAudioHandler {
    fn audio_init(&self, format: AudioFormat) -> Box<dyn AudioSession> {
        let cached_session = self.current_session();
        let resumes_logical_session = cached_session
            .as_ref()
            .is_some_and(|session| session.has_cached_media());
        let (lease, _transition) = self.arbiter.begin_takeover(
            MediaSource::AirPlayAudio,
            "audio",
            if resumes_logical_session {
                "airplay_audio_resume"
            } else {
                "airplay_audio_stream"
            },
            false,
        );
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        self.progress_timebase.store(
            format
                .source
                .and_then(|source| source.sample_rate)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
        let session_state = Arc::new(AudioSessionState::new(
            lease,
            format,
            cached_session.as_deref(),
        ));
        if let Ok(mut current) = self.current_session.lock() {
            *current = Some(Arc::clone(&session_state));
        }
        self.emit_stream_started(&session_state, lease);
        if resumes_logical_session {
            self.emit_cached_session_state(&session_state, lease);
        }
        let initially_playing = !session_state.suspended.load(Ordering::Acquire);
        self.events.emit(CoreEvent::Status {
            state: if initially_playing {
                "streaming"
            } else {
                "paused"
            },
            message: if initially_playing {
                "正在播放 AirPlay 2 音乐"
            } else {
                "AirPlay 2 音乐已暂停"
            },
        });
        self.events.emit(CoreEvent::PlaybackState {
            source: "airplay",
            epoch: lease.epoch(),
            playing: initially_playing,
        });
        self.emit_remote_control_state_current(
            "The sender has not provided an AirPlay media-control channel",
            lease,
        );
        let pcm_samples = Arc::new(AtomicU64::new(0));
        let stream_label = describe_stream_format(&format);
        watch_audio_session_start(
            Arc::clone(&self.events),
            Arc::clone(&pcm_samples),
            stream_label.clone(),
        );
        Box::new(ReceiverAudioSession {
            buffer: Arc::clone(&self.buffer),
            events: Arc::clone(&self.events),
            arbiter: Arc::clone(&self.arbiter),
            state: session_state,
            source_channels: format.channels as usize,
            output_channels: self.output_channels,
            pcm_samples,
            stream_label,
            pcm_received: Arc::clone(&self.pcm_received),
            buffer_lock_failures: Arc::clone(&self.buffer_lock_failures),
        })
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume_db_bits.load(Ordering::Relaxed))
    }

    fn on_diagnostic(&self, level: &str, message: &str) {
        self.events.emit(CoreEvent::Log { level, message });
    }

    fn on_volume(&self, volume: f32) {
        let Some(lease) = self.arbiter.current_lease(MediaSource::AirPlayAudio) else {
            return;
        };
        let gain = if volume <= -144.0 {
            0.0
        } else {
            10.0_f32.powf(volume / 20.0).clamp(0.0, 1.0)
        };
        self.arbiter.run_if_current(lease, || {
            self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
            self.volume_db_bits
                .store(volume.clamp(-144.0, 0.0).to_bits(), Ordering::Relaxed);
            self.events.emit(CoreEvent::Volume {
                source: "airplay",
                epoch: lease.epoch(),
                db: volume,
                percent: (gain * 100.0).round() as u8,
            });
        });
    }

    fn on_metadata(&self, metadata: &TrackMetadata) {
        report_unknown_dmap_tags(&self.events, &metadata.unknown_tags);
        let Some(session) = self.current_session() else {
            return;
        };
        let (metadata, track_changed) = session
            .metadata
            .lock()
            .map(|mut cached| {
                let merged = merge_track_metadata(cached.as_ref(), metadata);
                *cached = Some(merged.0.clone());
                merged
            })
            .unwrap_or_else(|_| merge_track_metadata(None, metadata));
        if track_changed {
            if let Ok(mut cover_art_path) = session.cover_art_path.lock() {
                cover_art_path.take();
            }
            if let Ok(mut progress) = session.progress.lock() {
                progress.take();
            }
        }
        let Some(lease) = session.lease() else {
            return;
        };
        self.arbiter.run_if_current(lease, || {
            self.events.emit(CoreEvent::NowPlaying {
                source: "airplay",
                epoch: lease.epoch(),
                title: metadata.title.as_deref(),
                artist: metadata.artist.as_deref(),
                album: metadata.album.as_deref(),
                genre: metadata.genre.as_deref(),
                duration_ms: metadata.duration_ms,
            });
        });
    }

    fn on_coverart(&self, coverart: &[u8]) {
        let Some(session) = self.current_session() else {
            return;
        };
        let Some(lease) = session.lease() else {
            return;
        };
        let filename = if coverart.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else {
            "jpg"
        };
        if std::fs::create_dir_all(&self.artwork_dir).is_err() {
            return;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        coverart.hash(&mut hasher);
        let path = self
            .artwork_dir
            .join(format!("cover-art-{:016x}.{filename}", hasher.finish()));
        if std::fs::write(&path, coverart).is_ok() {
            let path_string = path.to_string_lossy();
            if let Ok(mut cached) = session.cover_art_path.lock() {
                *cached = Some(path_string.to_string());
            }
            self.arbiter.run_if_current(lease, || {
                self.events.emit(CoreEvent::CoverArt {
                    source: "airplay",
                    epoch: lease.epoch(),
                    path: &path_string,
                });
            });
        }
    }

    fn on_progress(&self, start: u32, current: u32, end: u32) {
        let Some(session) = self.current_session() else {
            return;
        };
        let Some(lease) = session.lease() else {
            return;
        };
        let Some((position_ms, duration_ms)) = progress_milliseconds(
            start,
            current,
            end,
            self.progress_timebase.load(Ordering::Relaxed),
        ) else {
            return;
        };
        if let Ok(mut cached) = session.progress.lock() {
            *cached = Some((position_ms, duration_ms));
        }
        self.arbiter.run_if_current(lease, || {
            self.events.emit(CoreEvent::Progress {
                source: "airplay",
                epoch: lease.epoch(),
                position_ms,
                duration_ms,
            });
        });
    }

    fn on_playback_state(&self, playing: bool) {
        let Some(session) = self.current_session() else {
            return;
        };
        let Some(mut lease) = session.lease() else {
            return;
        };
        if playing && !self.arbiter.is_current(lease) {
            // Only an explicit sender playback transition may reclaim a
            // suspended session. PCM packets themselves never auto-take over,
            // which prevents packets already in flight from fighting the new
            // source.
            let (new_lease, transition) = self.arbiter.begin_takeover(
                MediaSource::AirPlayAudio,
                "audio",
                "airplay_audio_resume",
                false,
            );
            lease = new_lease;
            session.replace_lease(new_lease);
            if let Ok(mut buffer) = self.buffer.lock() {
                buffer.clear();
            }
            self.emit_stream_started(&session, new_lease);
            self.emit_cached_session_state(&session, new_lease);
            self.emit_remote_control_state_current(
                "The sender has not provided an AirPlay media-control channel",
                new_lease,
            );
            drop(transition);
        }
        session.suspended.store(!playing, Ordering::Release);
        if !playing && let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        self.arbiter.run_if_current(lease, || {
            self.events.emit(CoreEvent::PlaybackState {
                source: "airplay",
                epoch: lease.epoch(),
                playing,
            });
        });
    }

    fn on_remote_control(&self, remote: Arc<dyn RemoteControl>) {
        let available = remote.available_commands();
        let commands = ui_remote_capabilities(&available);

        if let Ok(mut stored) = self.remote_controls.lock() {
            stored.update(remote, !commands.is_empty());
        }
        self.emit_remote_control_state("投放设备未授权可用的媒体控制命令，或控制通道已断开");
    }

    fn on_client_connected(&self, address: &str) {
        let first_connection = self
            .client_connections
            .lock()
            .map(|mut connections| connections.connected(address))
            .unwrap_or(true);
        if !first_connection {
            return;
        }
        // A brand-new logical client must not inherit media from a device that
        // disconnected earlier. Auxiliary connections from the same active
        // sender do not take this branch.
        if let Ok(mut current) = self.current_session.lock() {
            current.take();
        }
        self.events.emit(CoreEvent::ClientConnected { address });
        self.events.emit(CoreEvent::Status {
            state: "connected",
            message: "AirPlay 设备已连接",
        });
    }

    fn on_client_disconnected(&self, address: &str) {
        let final_connection = self
            .client_connections
            .lock()
            .map(|mut connections| connections.disconnected(address))
            .unwrap_or(true);
        if !final_connection {
            return;
        }
        if let Ok(mut current) = self.current_session.lock() {
            current.take();
        }
        self.events.emit(CoreEvent::ClientDisconnected { address });
        self.events.emit(CoreEvent::Status {
            state: "ready",
            message: "等待 AirPlay 2 投放",
        });
    }

    fn on_error(&self, error: &shairplay::ShairplayError) {
        let message = error.to_string();
        self.events.emit(CoreEvent::Error { message: &message });
    }
}

struct ReceiverAudioSession {
    buffer: Arc<Mutex<AudioBuffer>>,
    events: Arc<EventSink>,
    arbiter: Arc<PlaybackArbiter>,
    state: Arc<AudioSessionState>,
    source_channels: usize,
    output_channels: usize,
    /// Total decoded PCM samples pushed to the output queue for this stream.
    /// Used to prove (in exported logs) that audio actually reaches the local
    /// output instead of silently failing in decrypt/decode/SETUP.
    pcm_samples: Arc<AtomicU64>,
    /// Human-readable stream format for diagnostics.
    stream_label: String,
    /// Cumulative decoded PCM samples handed to the output queue (all streams).
    pcm_received: Arc<AtomicU64>,
    /// Times the output queue mutex could not be taken.
    buffer_lock_failures: Arc<AtomicU64>,
}

impl ReceiverAudioSession {
    fn note_audio_delivered(&self, samples: usize) {
        if samples == 0 {
            return;
        }
        let previous = self
            .pcm_samples
            .fetch_add(samples as u64, Ordering::AcqRel);
        if previous == 0 {
            let message = format!(
                "AirPlay 音频数据已开始输出：{}（首块 {} 个采样）",
                self.stream_label, samples
            );
            self.events.emit(CoreEvent::Log {
                level: "info",
                message: &message,
            });
        }
    }
}

/// Format a short description of the negotiated AirPlay stream for logs.
fn describe_stream_format(format: &AudioFormat) -> String {
    match format.source {
        Some(source) => {
            let codec = match source.codec {
                SourceAudioCodec::Aac => "aac",
                SourceAudioCodec::Alac => "alac",
            };
            format!(
                "codec={codec} 源 {} Hz / {} 声道 → 输出 {} Hz / {} 声道",
                source.sample_rate.unwrap_or(0),
                source.channels.unwrap_or(0),
                format.sample_rate,
                format.channels
            )
        }
        None => format!(
            "PCM 源 {} Hz / {} 声道",
            format.sample_rate, format.channels
        ),
    }
}

/// Warn (once) when an AirPlay audio session starts but no decoded PCM arrives.
///
/// This is the decisive diagnostic for "sender reports playing but the receiver
/// is silent": it distinguishes a decode/decrypt/SETUP failure from a local
/// output failure.
fn watch_audio_session_start(
    events: Arc<EventSink>,
    pcm_samples: Arc<AtomicU64>,
    stream_label: String,
) {
    let _ = std::thread::Builder::new()
        .name("airplay-audio-watchdog".to_owned())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(6));
            if pcm_samples.load(Ordering::Acquire) == 0 {
                let message = format!(
                    "AirPlay 已建立音频会话（{stream_label}）但 6 秒内未收到任何解码 PCM；请检查流 SETUP、会话密钥解密或解码格式"
                );
                events.emit(CoreEvent::Log {
                    level: "warning",
                    message: &message,
                });
            }
        });
}

impl AudioSession for ReceiverAudioSession {
    fn audio_process(&mut self, samples: &[f32]) {
        if self.state.suspended.load(Ordering::Acquire) {
            return;
        }
        let Some(lease) = self.state.lease() else {
            return;
        };
        if !self.arbiter.is_current(lease) {
            return;
        }
        if self.source_channels == self.output_channels {
            if let Ok(mut buffer) = self.buffer.lock()
                && !self.state.suspended.load(Ordering::Acquire)
                && self.arbiter.is_current(lease)
            {
                buffer.push(samples);
                self.pcm_received
                    .fetch_add(samples.len() as u64, Ordering::Relaxed);
            } else {
                self.buffer_lock_failures.fetch_add(1, Ordering::Relaxed);
            }
            self.note_audio_delivered(samples.len());
            return;
        }

        // Channel conversion can allocate and touch thousands of samples.
        // Do it before taking the shared output queue lock; otherwise the
        // real-time WASAPI callback loses the lock and emits a whole buffer of
        // silence, which is heard as intermittent stutter.
        let mut converted =
            Vec::with_capacity(samples.len() / self.source_channels.max(1) * self.output_channels);
        for frame in samples.chunks_exact(self.source_channels.max(1)) {
            if self.source_channels == 1 {
                converted.extend(std::iter::repeat_n(frame[0], self.output_channels));
                continue;
            }

            for channel in 0..self.output_channels {
                converted.push(frame.get(channel).copied().unwrap_or(0.0));
            }
        }
        if let Ok(mut buffer) = self.buffer.lock()
            && !self.state.suspended.load(Ordering::Acquire)
            && self.arbiter.is_current(lease)
        {
            buffer.push(&converted);
            self.pcm_received
                .fetch_add(converted.len() as u64, Ordering::Relaxed);
        } else {
            self.buffer_lock_failures.fetch_add(1, Ordering::Relaxed);
        }
        self.note_audio_delivered(converted.len());
    }

    fn audio_flush(&mut self) {
        let Some(lease) = self.state.lease() else {
            return;
        };
        if !self.state.suspended.load(Ordering::Acquire)
            && self.arbiter.is_current(lease)
            && let Ok(mut buffer) = self.buffer.lock()
            && !self.state.suspended.load(Ordering::Acquire)
            && self.arbiter.is_current(lease)
        {
            buffer.clear();
        }
    }
}

impl Drop for ReceiverAudioSession {
    fn drop(&mut self) {
        // Do not remove the handler's strong session cache here. AirPlay 2 may
        // tear down only stream 96/103 while the logical sender connection and
        // MediaRemote channel stay alive; the replacement stream must inherit
        // metadata, artwork, progress and the sender's explicit transport state
        // from this session. The stopped event below projects the temporary
        // absence of a physical stream. Overwriting `suspended` here would make
        // a seamless stream replacement restart as paused even while the sender
        // is still playing.
        let Some(lease) = self.state.lease() else {
            return;
        };
        if !self.arbiter.release(lease) {
            return;
        }
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        self.events.emit(CoreEvent::PlaybackState {
            source: "airplay",
            epoch: lease.epoch(),
            playing: false,
        });
        self.events.emit(CoreEvent::StreamStopped {
            source: "airplay",
            epoch: lease.epoch(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        AudioBuffer, ClientConnections, ReceiverAudioHandler, RemoteControls, merge_track_metadata,
        progress_milliseconds, remote_supports_command, ui_remote_capabilities,
    };
    use crate::events::EventSink;
    use crate::takeover::{MediaSource, PlaybackArbiter};
    use shairplay::{
        AudioCodec, AudioFormat, AudioHandler, RemoteCommand, RemoteControl, ShairplayError,
        TrackMetadata,
    };

    struct FakeRemote {
        transport: &'static str,
        commands: Vec<RemoteCommand>,
    }

    struct RecordingRemote {
        commands: Vec<RemoteCommand>,
        sent: Arc<Mutex<Vec<RemoteCommand>>>,
    }

    impl RemoteControl for RecordingRemote {
        fn send_command(&self, command: RemoteCommand) -> Result<(), ShairplayError> {
            self.sent.lock().unwrap().push(command);
            Ok(())
        }

        fn available_commands(&self) -> Vec<RemoteCommand> {
            self.commands.clone()
        }

        fn transport_name(&self) -> &'static str {
            "recording"
        }
    }

    impl RemoteControl for FakeRemote {
        fn send_command(&self, _command: RemoteCommand) -> Result<(), ShairplayError> {
            Ok(())
        }

        fn available_commands(&self) -> Vec<RemoteCommand> {
            self.commands.clone()
        }

        fn transport_name(&self) -> &'static str {
            self.transport
        }
    }

    fn fake_remote(
        transport: &'static str,
        commands: Vec<RemoteCommand>,
    ) -> Arc<dyn RemoteControl> {
        Arc::new(FakeRemote {
            transport,
            commands,
        })
    }

    fn audio_format() -> AudioFormat {
        AudioFormat {
            codec: AudioCodec::Pcm,
            bits: 32,
            channels: 2,
            sample_rate: 48_000,
            source: None,
        }
    }

    fn test_handler() -> (
        ReceiverAudioHandler,
        Arc<PlaybackArbiter>,
        Arc<Mutex<AudioBuffer>>,
    ) {
        let events = Arc::new(EventSink::new());
        let arbiter = Arc::new(PlaybackArbiter::new(Arc::clone(&events)));
        let buffer = Arc::new(Mutex::new(AudioBuffer::new(48_000, 2)));
        (
            ReceiverAudioHandler {
                buffer: Arc::clone(&buffer),
                gain_bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
                volume_db_bits: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
                pcm_received: Arc::new(AtomicU64::new(0)),
                buffer_lock_failures: Arc::new(AtomicU64::new(0)),
                events,
                arbiter: Arc::clone(&arbiter),
                artwork_dir: PathBuf::new(),
                output_channels: 2,
                progress_timebase: AtomicU32::new(0),
                remote_controls: Mutex::new(RemoteControls::default()),
                client_connections: Mutex::new(ClientConnections::default()),
                current_session: Arc::new(Mutex::new(None)),
            },
            arbiter,
            buffer,
        )
    }

    #[test]
    fn android_style_cushion_prefills_and_reports_underruns() {
        // 480 ms backlog / 240 ms prefill is the Android stabilised cushion:
        // still well below the 2 s AirPlay 2 protocol latency.
        let mut buffer = AudioBuffer::new_with_limits(48_000, 2, 480, 240);
        let stats = buffer.stats();
        assert_eq!(stats.capacity_frames, 48_000 * 480 / 1_000);
        assert_eq!(stats.prefill_frames, 48_000 * 240 / 1_000);

        // Less than the prefill → silence, and the wait is counted once.
        let hundred_ms = vec![0.25_f32; 48_000 * 2 / 10];
        buffer.push(&hundred_ms);
        buffer.push(&hundred_ms);
        assert_eq!(buffer.pop(), 0.0);
        assert_eq!(buffer.pop(), 0.0);
        let stats = buffer.stats();
        assert_eq!(stats.prefill_waits, 1);
        assert_eq!(stats.underruns, 0);

        // Cross the prefill threshold → real audio starts.
        buffer.push(&hundred_ms);
        assert_eq!(buffer.pop(), 0.25);

        // Drain while primed → exactly one underrun event, never a panic.
        for _ in 0..(48_000 * 2 * 3 / 10) {
            let _ = buffer.pop();
        }
        let stats = buffer.stats();
        assert_eq!(stats.underruns, 1);
        assert_eq!(stats.buffered_frames, 0);
    }

    #[test]
    fn windows_output_backlog_is_bounded_and_keeps_the_newest_audio() {
        let mut buffer = AudioBuffer::new(1_000, 1);
        let samples: Vec<f32> = (0..300).map(|sample| sample as f32).collect();

        buffer.push(&samples);

        assert_eq!(buffer.samples.len(), 80);
        assert_eq!(buffer.pop(), 220.0);
    }

    #[test]
    fn unknown_dmap_tags_are_logged_once_without_content() {
        let (handler, _arbiter, _buffer) = test_handler();
        handler.on_metadata(&TrackMetadata {
            title: Some("Track".into()),
            unknown_tags: vec![("zzdiag".to_owned(), 64)],
            ..TrackMetadata::default()
        });
        handler.on_metadata(&TrackMetadata {
            title: Some("Track".into()),
            unknown_tags: vec![("zzdiag".to_owned(), 64)],
            ..TrackMetadata::default()
        });

        let messages: Vec<String> = handler
            .events
            .captured_events()
            .iter()
            .filter(|event| event["type"] == "log")
            .filter_map(|event| event["message"].as_str().map(ToOwned::to_owned))
            .filter(|message| message.contains("未知 DMAP 标签"))
            .collect();
        assert_eq!(messages.len(), 1, "unknown tag reported exactly once");
        assert!(messages[0].contains("zzdiag(64)"), "tag+length must be logged");
        assert!(
            !messages[0].contains("payload"),
            "no metadata content may be logged"
        );
    }

    #[test]
    fn auxiliary_airplay_disconnect_does_not_end_the_logical_client_session() {
        let mut connections = ClientConnections::default();

        assert!(connections.connected("192.0.2.10"));
        assert!(!connections.connected("192.0.2.10"));
        assert!(!connections.connected("2001:db8::10"));

        assert!(!connections.disconnected("192.0.2.10"));
        assert!(!connections.disconnected("2001:db8::10"));
        assert!(connections.disconnected("192.0.2.10"));
    }

    #[test]
    fn auxiliary_socket_close_does_not_emit_client_disconnected() {
        let (handler, _, _) = test_handler();

        handler.on_client_connected("192.0.2.10");
        handler.on_client_connected("192.0.2.10");
        handler.on_client_disconnected("192.0.2.10");

        let interim_events = handler.events.captured_events();
        assert_eq!(
            interim_events
                .iter()
                .filter(|event| event["type"] == "client_connected")
                .count(),
            1
        );
        assert!(
            interim_events
                .iter()
                .all(|event| event["type"] != "client_disconnected")
        );

        handler.on_client_disconnected("192.0.2.10");
        assert_eq!(
            handler
                .events
                .captured_events()
                .iter()
                .filter(|event| event["type"] == "client_disconnected")
                .count(),
            1
        );
    }

    #[test]
    fn windows_output_backlog_uses_interleaved_output_sample_units() {
        let buffer = AudioBuffer::new(48_000, 2);

        assert_eq!(buffer.capacity, 7_680);
        assert_eq!(buffer.prefill, 3_840);
    }

    #[test]
    fn output_waits_for_jitter_prefill_and_reprimes_after_underrun() {
        let mut buffer = AudioBuffer::new(1_000, 1);
        buffer.push(&[0.25; 39]);
        assert_eq!(buffer.pop(), 0.0);

        buffer.push(&[0.5]);
        assert_eq!(buffer.pop(), 0.25);
        while !buffer.samples.is_empty() {
            let _ = buffer.pop();
        }
        assert_eq!(buffer.pop(), 0.0);

        buffer.push(&[0.75; 39]);
        assert_eq!(buffer.pop(), 0.0);
        buffer.push(&[1.0]);
        assert_eq!(buffer.pop(), 0.75);
    }

    #[test]
    fn incremental_output_overflow_drops_only_the_oldest_samples() {
        let mut buffer = AudioBuffer::new(1_000, 1);
        buffer.push(&(0..50).map(|sample| sample as f32).collect::<Vec<_>>());
        buffer.push(&(50..100).map(|sample| sample as f32).collect::<Vec<_>>());

        assert_eq!(buffer.samples.len(), 80);
        assert_eq!(buffer.pop(), 20.0);
        assert_eq!(buffer.samples.back().copied(), Some(99.0));
    }

    #[test]
    fn play_pause_toggle_is_exposed_to_the_ui() {
        assert_eq!(
            ui_remote_capabilities(&[RemoteCommand::PlayPause]),
            vec!["play_pause"]
        );
    }

    #[test]
    fn play_pause_can_use_separate_play_or_pause_commands() {
        assert!(remote_supports_command(
            &[RemoteCommand::Play],
            &RemoteCommand::PlayPause
        ));
        assert!(remote_supports_command(
            &[RemoteCommand::Pause],
            &RemoteCommand::PlayPause
        ));
        assert!(remote_supports_command(
            &[RemoteCommand::PlayPause],
            &RemoteCommand::Pause
        ));
    }

    #[test]
    fn seek_capability_is_value_independent() {
        assert!(remote_supports_command(
            &[RemoteCommand::SeekToPosition(0)],
            &RemoteCommand::SeekToPosition(52_300)
        ));
        assert_eq!(
            ui_remote_capabilities(&[RemoteCommand::SeekToPosition(0)]),
            vec!["seek"]
        );
    }

    #[test]
    fn volume_capability_is_value_independent() {
        assert!(remote_supports_command(
            &[RemoteCommand::SetVolume(0)],
            &RemoteCommand::SetVolume(42)
        ));
    }

    #[test]
    fn dacp_is_preferred_and_media_remote_remains_as_fallback() {
        let media_remote = fake_remote(
            "airplay2_mediaremote_experimental",
            vec![RemoteCommand::PlayPause],
        );
        let dacp = fake_remote("dacp", vec![RemoteCommand::PlayPause]);
        let mut controls = RemoteControls::default();

        controls.update(media_remote.clone(), true);
        controls.update(dacp.clone(), true);

        assert_eq!(controls.preferred().unwrap().transport_name(), "dacp");
        assert_eq!(
            controls
                .candidates()
                .iter()
                .map(|remote| remote.transport_name())
                .collect::<Vec<_>>(),
            vec!["dacp", "airplay2_mediaremote_experimental"]
        );

        controls.update(dacp, false);
        assert_eq!(
            controls.preferred().unwrap().transport_name(),
            "airplay2_mediaremote_experimental"
        );

        controls.update(media_remote, false);
        assert!(controls.preferred().is_none());
    }

    #[test]
    fn ui_capabilities_are_the_union_of_all_live_airplay_transports() {
        let mut controls = RemoteControls::default();
        controls.update(fake_remote("dacp", vec![RemoteCommand::PlayPause]), true);
        controls.update(
            fake_remote(
                "airplay2_mediaremote_experimental",
                vec![
                    RemoteCommand::PreviousTrack,
                    RemoteCommand::NextTrack,
                    RemoteCommand::SeekToPosition(0),
                ],
            ),
            true,
        );

        let (commands, transport) = controls.ui_state().unwrap();

        assert_eq!(transport, "dacp");
        assert_eq!(
            ui_remote_capabilities(&commands),
            vec!["play_pause", "previous_track", "next_track", "seek"]
        );
    }

    #[test]
    fn airplay_remote_control_survives_a_temporary_stream_gap() {
        let (handler, _, _) = test_handler();
        handler
            .remote_controls
            .lock()
            .unwrap()
            .update(fake_remote("dacp", vec![RemoteCommand::Play]), true);

        assert!(handler.send_remote_command(RemoteCommand::Play).is_ok());
    }

    #[test]
    fn local_volume_from_ui_succeeds_without_a_remote_command() {
        let (handler, _, _) = test_handler();
        // A live AirPlay session provides the epoch for the UI volume event.
        let _session = handler.audio_init(audio_format());
        let event_index = handler.events.captured_events().len();
        // AirPlay 2 senders only expose next/prev/play-pause/seek on their
        // MediaRemote channel. Receiver volume must still work locally instead
        // of failing with "当前投放设备不支持此远程控制命令".
        handler
            .send_remote_command(RemoteCommand::SetVolume(30))
            .expect("UI volume must apply locally without a remote");

        let gain = f32::from_bits(handler.gain_bits.load(Ordering::Relaxed));
        assert!((gain - 0.30).abs() < 0.001, "local gain must be 30%: {gain}");
        let db = f32::from_bits(handler.volume_db_bits.load(Ordering::Relaxed));
        assert!(
            (db - 20.0 * 0.30_f32.log10()).abs() < 0.002,
            "reported dB must match the 30% gain: {db}"
        );

        let events = handler.events.captured_events();
        let volume_events = &events[event_index..];
        assert!(
            volume_events
                .iter()
                .any(|event| event["type"] == "volume" && event["percent"] == 30),
            "the UI must receive a volume event for its own change"
        );
        assert!(
            !volume_events.iter().any(|event| {
                event["type"] == "remote_control_unavailable"
                    || (event["type"] == "command_result"
                        && event["succeeded"].as_bool() == Some(false))
            }),
            "a local volume change must not enter the unsupported-remote error path"
        );
    }

    #[test]
    fn local_volume_mute_uses_full_attenuation() {
        let (handler, _, _) = test_handler();
        handler
            .send_remote_command(RemoteCommand::SetVolume(0))
            .expect("mute must succeed locally");
        let gain = f32::from_bits(handler.gain_bits.load(Ordering::Relaxed));
        assert_eq!(gain, 0.0);
        let db = f32::from_bits(handler.volume_db_bits.load(Ordering::Relaxed));
        assert_eq!(db, -144.0);
    }

    #[test]
    fn local_volume_still_mirrors_to_a_remote_that_supports_it() {
        let (handler, _, _) = test_handler();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let remote: Arc<dyn RemoteControl> = Arc::new(RecordingRemote {
            commands: vec![RemoteCommand::SetVolume(0)],
            sent: Arc::clone(&sent),
        });
        handler.remote_controls.lock().unwrap().update(remote, true);

        handler
            .send_remote_command(RemoteCommand::SetVolume(55))
            .expect("volume must succeed");

        assert!(
            sent.lock()
                .unwrap()
                .iter()
                .any(|command| matches!(command, RemoteCommand::SetVolume(55))),
            "AP1 DACP senders that support SetVolume must still be mirrored"
        );
        let gain = f32::from_bits(handler.gain_bits.load(Ordering::Relaxed));
        assert!((gain - 0.55).abs() < 0.001);
    }

    #[test]
    fn sender_volume_updates_the_volume_reported_to_the_sender() {
        let (handler, _, _) = test_handler();
        let _session = handler.audio_init(audio_format());

        handler.on_volume(-6.0);

        let db = f32::from_bits(handler.volume_db_bits.load(Ordering::Relaxed));
        assert!((db + 6.0).abs() < 0.001, "sender volume must be remembered: {db}");
        assert!((handler.volume() + 6.0).abs() < 0.001, "GET_PARAMETER volume must report it");
    }

    #[test]
    fn audio_pipeline_diagnostics_are_exported_as_log_events() {
        let (handler, _, _) = test_handler();
        handler.on_diagnostic("warning", "AAC decode failed");
        let messages: Vec<String> = handler
            .events
            .captured_events()
            .iter()
            .filter(|event| event["type"] == "log")
            .filter_map(|event| event["message"].as_str().map(ToOwned::to_owned))
            .collect();
        assert!(messages.iter().any(|message| message.contains("AAC decode failed")));
    }

    #[test]
    fn airplay_remote_control_cannot_override_another_active_source() {
        let (handler, arbiter, _) = test_handler();
        handler
            .remote_controls
            .lock()
            .unwrap()
            .update(fake_remote("dacp", vec![RemoteCommand::Play]), true);
        arbiter.takeover(
            MediaSource::Dlna,
            "audio",
            "test_dlna_takeover",
            false,
            |_| (),
        );

        assert!(handler.send_remote_command(RemoteCommand::Play).is_err());
    }

    #[test]
    fn successful_airplay_play_pause_is_projected_immediately() {
        let (handler, _, _) = test_handler();
        let _session = handler.audio_init(audio_format());
        handler.remote_controls.lock().unwrap().update(
            fake_remote(
                "airplay2_mediaremote_experimental",
                vec![RemoteCommand::PlayPause],
            ),
            true,
        );

        handler
            .send_remote_command(RemoteCommand::PlayPause)
            .unwrap();
        assert!(
            handler
                .current_session()
                .unwrap()
                .suspended
                .load(Ordering::Acquire)
        );

        handler
            .send_remote_command(RemoteCommand::PlayPause)
            .unwrap();
        assert!(
            !handler
                .current_session()
                .unwrap()
                .suspended
                .load(Ordering::Acquire)
        );
    }

    #[test]
    fn rapid_airplay_play_pause_uses_alternating_explicit_commands() {
        let (handler, _, _) = test_handler();
        let _session = handler.audio_init(audio_format());
        let sent = Arc::new(Mutex::new(Vec::new()));
        handler.remote_controls.lock().unwrap().update(
            Arc::new(RecordingRemote {
                commands: vec![RemoteCommand::Play, RemoteCommand::Pause],
                sent: Arc::clone(&sent),
            }),
            true,
        );

        for _ in 0..3 {
            handler
                .send_remote_command(RemoteCommand::PlayPause)
                .unwrap();
        }

        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                RemoteCommand::Pause,
                RemoteCommand::Play,
                RemoteCommand::Pause,
            ]
        );
        assert!(
            handler
                .current_session()
                .unwrap()
                .suspended
                .load(Ordering::Acquire)
        );
    }

    #[test]
    fn airplay_stream_restart_retains_now_playing_and_progress() {
        let (handler, _, _) = test_handler();
        let first_stream = handler.audio_init(audio_format());
        handler.on_metadata(&TrackMetadata {
            title: Some("Retained track".into()),
            artist: Some("Retained artist".into()),
            duration_ms: Some(180_000),
            ..TrackMetadata::default()
        });
        let first_state = handler.current_session().unwrap();
        *first_state.cover_art_path.lock().unwrap() = Some("retained-cover.jpg".into());
        *first_state.progress.lock().unwrap() = Some((52_000, 180_000));

        // AP2 commonly tears down stream 96/103 on pause while keeping the
        // logical sender and its MediaRemote channel connected.
        drop(first_stream);
        let retained_state = handler.current_session().unwrap();
        assert!(Arc::ptr_eq(&first_state, &retained_state));
        assert!(!retained_state.suspended.load(Ordering::Acquire));

        let restart_event_index = handler.events.captured_events().len();
        let second_stream = handler.audio_init(audio_format());
        let resumed_state = handler.current_session().unwrap();
        assert!(!Arc::ptr_eq(&first_state, &resumed_state));
        assert!(!resumed_state.suspended.load(Ordering::Acquire));
        let metadata = resumed_state.metadata.lock().unwrap().clone().unwrap();
        assert_eq!(metadata.title.as_deref(), Some("Retained track"));
        assert_eq!(metadata.artist.as_deref(), Some("Retained artist"));
        assert_eq!(metadata.duration_ms, Some(180_000));
        assert_eq!(
            resumed_state.cover_art_path.lock().unwrap().as_deref(),
            Some("retained-cover.jpg")
        );
        assert_eq!(
            *resumed_state.progress.lock().unwrap(),
            Some((52_000, 180_000))
        );
        let restart_events = handler.events.captured_events();
        assert!(
            restart_events[restart_event_index..]
                .iter()
                .any(|event| { event["type"] == "playback_state" && event["playing"] == true })
        );
        drop(second_stream);
    }

    #[test]
    fn paused_airplay_stream_restart_remains_paused() {
        let (handler, _, _) = test_handler();
        let first_stream = handler.audio_init(audio_format());
        handler.on_metadata(&TrackMetadata {
            title: Some("Paused track".into()),
            ..TrackMetadata::default()
        });
        handler.on_playback_state(false);
        let stopped_event_index = handler.events.captured_events().len();
        drop(first_stream);
        assert!(
            handler.events.captured_events()[stopped_event_index..]
                .iter()
                .any(|event| event["type"] == "stream_stopped")
        );

        let restart_event_index = handler.events.captured_events().len();
        let second_stream = handler.audio_init(audio_format());
        let resumed_state = handler.current_session().unwrap();
        assert!(resumed_state.suspended.load(Ordering::Acquire));

        let restart_events = handler.events.captured_events();
        let playback_events = restart_events[restart_event_index..]
            .iter()
            .filter(|event| event["type"] == "playback_state")
            .collect::<Vec<_>>();
        assert!(!playback_events.is_empty());
        assert!(
            playback_events
                .iter()
                .all(|event| event["playing"] == false)
        );
        drop(second_stream);
    }

    #[test]
    fn partial_metadata_updates_do_not_erase_known_track_fields() {
        let current = TrackMetadata {
            title: Some("Track".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            genre: Some("Genre".into()),
            duration_ms: Some(180_000),
            track_number: Some(3),
            disc_number: Some(1),
            unknown_tags: Vec::new(),
        };
        let incoming = TrackMetadata {
            title: None,
            artist: Some("Updated artist".into()),
            ..TrackMetadata::default()
        };

        let (merged, changed) = merge_track_metadata(Some(&current), &incoming);

        assert!(!changed);
        assert_eq!(merged.title.as_deref(), Some("Track"));
        assert_eq!(merged.artist.as_deref(), Some("Updated artist"));
        assert_eq!(merged.album.as_deref(), Some("Album"));
        assert_eq!(merged.duration_ms, Some(180_000));
    }

    #[test]
    fn new_track_metadata_does_not_keep_the_previous_artist_or_album() {
        let current = TrackMetadata {
            title: Some("Old track".into()),
            artist: Some("Old artist".into()),
            album: Some("Old album".into()),
            duration_ms: Some(180_000),
            ..TrackMetadata::default()
        };
        let incoming = TrackMetadata {
            title: Some("New track".into()),
            duration_ms: Some(200_000),
            ..TrackMetadata::default()
        };

        let (merged, changed) = merge_track_metadata(Some(&current), &incoming);

        assert!(changed);
        assert_eq!(merged.title.as_deref(), Some("New track"));
        assert_eq!(merged.artist, None);
        assert_eq!(merged.album, None);
        assert_eq!(merged.duration_ms, Some(200_000));
    }

    #[test]
    fn progress_uses_the_48khz_source_timebase() {
        assert_eq!(
            progress_milliseconds(96_000, 144_000, 240_000, 48_000),
            Some((1_000, 3_000))
        );
    }

    #[test]
    fn progress_is_not_emitted_when_the_source_timebase_is_unknown() {
        assert_eq!(progress_milliseconds(0, 48_000, 96_000, 0), None);
    }

    #[test]
    fn zeroed_pause_progress_does_not_reset_the_timeline() {
        assert_eq!(progress_milliseconds(0, 0, 0, 44_100), None);
    }

    #[test]
    fn malformed_progress_beyond_track_end_is_ignored() {
        assert_eq!(
            progress_milliseconds(96_000, 288_000, 240_000, 48_000),
            None
        );
    }

    #[test]
    fn preempted_audio_session_cannot_feed_the_output_buffer() {
        let (handler, arbiter, buffer) = test_handler();
        let mut session = handler.audio_init(audio_format());
        session.audio_process(&[0.25, -0.25]);
        let buffered_before_takeover = buffer.lock().unwrap().samples.len();

        arbiter.takeover(
            MediaSource::Dlna,
            "audio",
            "test_dlna_takeover",
            false,
            |_| (),
        );
        session.audio_process(&[0.5, -0.5]);

        assert_eq!(
            buffer.lock().unwrap().samples.len(),
            buffered_before_takeover
        );
        drop(session);
        assert_eq!(arbiter.current_source(), Some(MediaSource::Dlna));
    }

    #[test]
    fn old_audio_drop_does_not_release_a_newer_audio_session() {
        let (handler, arbiter, _) = test_handler();
        let old = handler.audio_init(audio_format());
        let current = handler.audio_init(audio_format());
        let current_lease = arbiter.current_lease(MediaSource::AirPlayAudio).unwrap();

        drop(old);
        assert!(arbiter.is_current(current_lease));
        drop(current);
        assert_eq!(arbiter.current_source(), None);
    }

    #[test]
    fn suspended_audio_only_reclaims_on_explicit_playback_resume() {
        let (handler, arbiter, buffer) = test_handler();
        let mut session = handler.audio_init(audio_format());
        let old_lease = arbiter.current_lease(MediaSource::AirPlayAudio).unwrap();
        session.audio_process(&[0.25, -0.25]);

        arbiter.takeover(
            MediaSource::Dlna,
            "audio",
            "test_dlna_takeover",
            false,
            |_| (),
        );
        handler.suspend_for_takeover(old_lease);
        let suspended_len = buffer.lock().unwrap().samples.len();
        session.audio_process(&[0.5, -0.5]);
        assert_eq!(buffer.lock().unwrap().samples.len(), suspended_len);
        assert_eq!(arbiter.current_source(), Some(MediaSource::Dlna));

        handler.on_playback_state(true);
        let resumed_lease = arbiter.current_lease(MediaSource::AirPlayAudio).unwrap();
        assert_ne!(resumed_lease, old_lease);
        session.audio_process(&[0.75, -0.75]);
        assert!(buffer.lock().unwrap().samples.len() > suspended_len);
    }
}
