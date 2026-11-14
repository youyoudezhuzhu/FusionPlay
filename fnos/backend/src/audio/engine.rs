//! Hardware-agnostic multi-device output engine.
//!
//! Design rules (see `fnos/AUDIT.md` for the deficiencies this replaces):
//! - `write()` never performs device I/O. It applies the master gain once and
//!   bulk-copies the result into each device's bounded ring (a short mutex, no
//!   blocking syscall). A full ring drops the oldest whole frames and counts
//!   them in `EngineStatus::dropped_frames`.
//! - Every enabled device owns a ring, a thread and a resampler. The thread
//!   blocks in the device write (the device clock); when the ring is empty it
//!   sleeps on a condvar (long timeout when idle) instead of polling every few
//!   milliseconds, so an idle server does not spin for 24 h.
//! - Clock drift is measured from the ring fill in frames, expressed in ppm,
//!   clamped to [`MAX_DRIFT_PPM`] and applied through the resampler ratio.
//! - A device that fails to open or returns a write error is marked offline;
//!   a watchdog re-scans (only while offline devices exist) and restarts it.
//!   Opening/prefilling happens on the device thread, never on `write()`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use super::device::{DeviceProvider, DeviceSink, SampleFormat};
use super::resample::{
    DriftAction, DriftEstimator, DriftTierConfig, SincResampler, DRIFT_CALIBRATION_UPDATES,
    DRIFT_CALIBRATION_WARMUP, DRIFT_FILL_SMOOTHING, MAX_DRIFT_PPM,
};
use super::{DeviceInfo, DeviceStatus, EngineStatus, OutputBackend};
use crate::config::{AudioSettings, DeviceSetting};
use crate::session::Source;
use crate::volume::device_db_to_gain;

/// Interleaved channels of the master mix. All protocol hosts deliver stereo.
pub const MASTER_CHANNELS: usize = 2;

/// Jitter buffer target, in ms. The ring is pre-filled with silence to this
/// level (plus the calibration delay) when a device opens.
pub const RING_TARGET_MS: usize = 120;

/// Ring capacity beyond the calibration delay, in ms. Overflow drops the oldest
/// frames; `buffer_ms` can therefore never exceed `RING_EXTRA_MS + delay_ms`.
pub const RING_EXTRA_MS: usize = 300;

/// How long without new audio before a device is considered idle (not starving).
const IDLE_AFTER: Duration = Duration::from_millis(750);

/// Minimum gap between two underrun reports for one device.
const UNDERRUN_MIN_GAP: Duration = Duration::from_millis(250);

/// Maximum number of frames popped per device write.
const MAX_BATCH_FRAMES: usize = 1_024;

/// Default hot-plug re-scan period.
pub const DEFAULT_RESCAN_INTERVAL: Duration = Duration::from_secs(2);

const FADE_IN_MS: f32 = 20.0;

/// Every device owns its own curve; the legacy global curve is validated only
/// when a not-yet-migrated config still carries one.
fn validate_curves(settings: &AudioSettings) -> Result<()> {
    for device in &settings.devices {
        device.curve.validate().map_err(anyhow::Error::msg).map_err(|error| {
            error.context(format!("设备 {} 的音量曲线不合法", device.id))
        })?;
    }
    Ok(())
}

fn frames_for_ms(ms: f32, rate: u32) -> usize {
    (((ms.max(0.0) as f64) / 1000.0) * rate.max(1) as f64).round() as usize
}

fn f32_bits(value: f32) -> u32 {
    value.to_bits()
}

fn bits_f32(bits: u32) -> f32 {
    f32::from_bits(bits)
}

fn encode_source(source: Source) -> u8 {
    match source {
        Source::AirPlay => 1,
        Source::Dlna => 2,
        Source::MiPlay => 3,
    }
}

fn decode_source(value: u8) -> Option<Source> {
    match value {
        1 => Some(Source::AirPlay),
        2 => Some(Source::Dlna),
        3 => Some(Source::MiPlay),
        _ => None,
    }
}

/// Worker/device state surfaced through [`DeviceStatus::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerState {
    Offline = 0,
    Starting = 1,
    Running = 2,
    Idle = 3,
    Error = 4,
}

impl WorkerState {
    fn as_str(self) -> &'static str {
        match self {
            WorkerState::Offline => "offline",
            WorkerState::Starting => "starting",
            WorkerState::Running => "running",
            WorkerState::Idle => "idle",
            WorkerState::Error => "error",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => WorkerState::Starting,
            2 => WorkerState::Running,
            3 => WorkerState::Idle,
            4 => WorkerState::Error,
            _ => WorkerState::Offline,
        }
    }
}

/// A queued `test_tone` request. The device thread synthesises the tone at its
/// own negotiated rate and writes it directly, so the audio ring is never
/// polluted with test audio (the previous implementation pushed 1 s of tone
/// into the ring).
#[derive(Debug, Clone, Copy)]
pub struct ToneRequest {
    pub frequency_hz: f32,
    pub duration_ms: u32,
}

impl Default for ToneRequest {
    fn default() -> Self {
        Self { frequency_hz: 1_000.0, duration_ms: 1_000 }
    }
}

struct RingInner {
    samples: VecDeque<f32>,
    last_push: Option<Instant>,
}

/// Bounded interleaved f32 ring with a condvar used to wake the device thread.
struct DeviceRing {
    inner: Mutex<RingInner>,
    cv: Condvar,
    channels: usize,
    capacity_frames: AtomicUsize,
}

enum WaitOutcome {
    /// Data was available; `fill` is the ring level measured before the pop
    /// (used as the clock-drift input).
    Data { fill: usize },
    /// Timed out with an empty ring.
    Empty,
    Stop,
}

impl DeviceRing {
    fn new(channels: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(RingInner { samples: VecDeque::new(), last_push: None }),
            cv: Condvar::new(),
            channels: channels.max(1),
            capacity_frames: AtomicUsize::new(0),
        })
    }

    fn set_capacity_frames(&self, frames: usize) {
        self.capacity_frames.store(frames.max(1), Ordering::Relaxed);
    }

    fn capacity_frames(&self) -> usize {
        self.capacity_frames.load(Ordering::Relaxed).max(1)
    }

    fn fill_frames(&self) -> usize {
        self.inner
            .lock()
            .map(|guard| guard.samples.len() / self.channels)
            .unwrap_or(0)
    }

    fn last_push_ago(&self) -> Option<Duration> {
        self.inner
            .lock()
            .ok()
            .and_then(|guard| guard.last_push.map(|instant| instant.elapsed()))
    }

    fn notify_wake(&self) {
        self.cv.notify_all();
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.samples.clear();
        }
        self.cv.notify_all();
    }

    fn prefill_silence(&self, frames: usize) {
        if frames == 0 {
            return;
        }
        let capacity = self.capacity_frames() * self.channels;
        if let Ok(mut guard) = self.inner.lock() {
            let samples = (frames * self.channels).min(capacity);
            guard.samples.extend(std::iter::repeat(0.0f32).take(samples));
            if guard.samples.len() > capacity {
                let excess = guard.samples.len() - capacity;
                let excess = excess - excess % self.channels;
                guard.samples.drain(..excess);
            }
        }
        self.cv.notify_all();
    }

    fn discard_frames(&self, frames: usize) {
        if frames == 0 {
            return;
        }
        if let Ok(mut guard) = self.inner.lock() {
            let samples = (frames * self.channels).min(guard.samples.len());
            guard.samples.drain(..samples);
        }
    }

    /// Push scaled samples; drops the oldest whole frames on overflow and
    /// returns the number of dropped frames.
    fn push_scaled(&self, samples: &[f32], gain: f32) -> u64 {
        if samples.is_empty() {
            return 0;
        }
        // A zero master gain still pushes silence: muting must not starve the
        // device (that would turn mute into a stream of xruns).
        let capacity_samples = self.capacity_frames() * self.channels;
        let mut dropped_frames = 0u64;
        let was_empty;
        {
            let mut guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(_) => return 0,
            };
            was_empty = guard.samples.is_empty();
            if (gain - 1.0).abs() < f32::EPSILON {
                guard.samples.extend(samples.iter().copied());
            } else {
                guard.samples.extend(samples.iter().map(|sample| sample * gain));
            }
            if guard.samples.len() > capacity_samples {
                let excess = guard.samples.len() - capacity_samples;
                let excess = excess - excess % self.channels;
                guard.samples.drain(..excess);
                dropped_frames = (excess / self.channels) as u64;
            }
            guard.last_push = Some(Instant::now());
        }
        if was_empty {
            self.cv.notify_one();
        }
        dropped_frames
    }

    /// Push samples with a linear block-level gain ramp from `from` to `to`,
    /// then to `to`. This smooths sender-volume / global-attenuation changes
    /// without a per-sample curve evaluation (curves are evaluated once per
    /// block in `write_inner`).
    fn push_ramped(&self, samples: &[f32], from: f32, to: f32) -> u64 {
        if samples.is_empty() {
            return 0;
        }
        if (from - to).abs() < 1e-6 || samples.len() < self.channels * 2 {
            return self.push_scaled(samples, to);
        }
        let capacity_samples = self.capacity_frames() * self.channels;
        let mut dropped_frames = 0u64;
        let was_empty;
        {
            let mut guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(_) => return 0,
            };
            was_empty = guard.samples.is_empty();
            let frames = samples.len() / self.channels;
            let step = (to - from) / frames.max(1) as f32;
            for frame in 0..frames {
                let gain = from + step * frame as f32;
                for channel in 0..self.channels {
                    guard.samples.push_back(samples[frame * self.channels + channel] * gain);
                }
            }
            if guard.samples.len() > capacity_samples {
                let excess = guard.samples.len() - capacity_samples;
                let excess = excess - excess % self.channels;
                guard.samples.drain(..excess);
                dropped_frames = (excess / self.channels) as u64;
            }
            guard.last_push = Some(Instant::now());
        }
        if was_empty {
            self.cv.notify_one();
        }
        dropped_frames
    }

    /// Wait for data (condvar, not a poll loop) and pop up to `max_frames`.
    fn wait_batch(
        &self,
        stop: &AtomicBool,
        timeout: Duration,
        max_frames: usize,
        out: &mut Vec<f32>,
    ) -> WaitOutcome {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return WaitOutcome::Stop,
        };
        loop {
            if stop.load(Ordering::Acquire) {
                return WaitOutcome::Stop;
            }
            if !guard.samples.is_empty() {
                break;
            }
            let (next, result) = match self.cv.wait_timeout(guard, timeout) {
                Ok(pair) => pair,
                Err(_) => return WaitOutcome::Stop,
            };
            guard = next;
            if guard.samples.is_empty() {
                if stop.load(Ordering::Acquire) {
                    return WaitOutcome::Stop;
                }
                // Either the timeout elapsed or a command woke us (flush,
                // tone, stop). Return to the worker loop so it can react.
                let _ = result;
                return WaitOutcome::Empty;
            }
        }
        let fill = guard.samples.len() / self.channels;
        let take = fill.min(max_frames.max(1));
        out.clear();
        out.extend(guard.samples.drain(..take * self.channels));
        WaitOutcome::Data { fill }
    }
}

struct DeviceControl {
    stop: AtomicBool,
    gain: AtomicU32,
    delay_ms: AtomicU32,
    tone: Mutex<Option<ToneRequest>>,
    /// Bumped by `flush()` so the worker resets its drift/resampler state.
    reset_epoch: AtomicU64,
}

struct DeviceStats {
    online: AtomicBool,
    state: AtomicU8,
    written_frames: AtomicU64,
    underruns: AtomicU64,
    drift_ppm: AtomicU32,
    actual_rate: AtomicU32,
    channels: AtomicU32,
    format: Mutex<Option<SampleFormat>>,
    last_error: Mutex<Option<String>>,
}

impl DeviceStats {
    fn new() -> Self {
        Self {
            online: AtomicBool::new(false),
            state: AtomicU8::new(WorkerState::Offline as u8),
            written_frames: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            drift_ppm: AtomicU32::new(0.0f32.to_bits()),
            actual_rate: AtomicU32::new(0),
            channels: AtomicU32::new(MASTER_CHANNELS as u32),
            format: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    fn set_state(&self, state: WorkerState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    fn state(&self) -> WorkerState {
        WorkerState::from_u8(self.state.load(Ordering::Relaxed))
    }

    fn set_error(&self, message: String) {
        eprintln!("[audio] {message}");
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(message);
        }
    }

    fn clear_error(&self) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = None;
        }
    }

    fn reset_counters(&self) {
        self.written_frames.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
        self.drift_ppm.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

/// One configured output device: ring, control block, counters, thread.
struct DeviceWorker {
    id: String,
    setting: RwLock<DeviceSetting>,
    wanted_rate: AtomicU32,
    enabled: AtomicBool,
    retired: AtomicBool,
    provider: Arc<dyn DeviceProvider>,
    source_rate: Arc<AtomicU32>,
    ring: Arc<DeviceRing>,
    /// Gain of the last block pushed into the ring (curve * global attenuation).
    /// `u32::MAX` means "uninitialised" so the first block does not ramp from 0.
    last_curve_gain: AtomicU32,
    control: Arc<DeviceControl>,
    stats: Arc<DeviceStats>,
    info: RwLock<DeviceInfo>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DeviceWorker {
    fn new(
        setting: DeviceSetting,
        wanted_rate: u32,
        info: DeviceInfo,
        provider: Arc<dyn DeviceProvider>,
        source_rate: Arc<AtomicU32>,
    ) -> Arc<Self> {
        let rate = wanted_rate.max(1);
        let ring = DeviceRing::new(MASTER_CHANNELS);
        ring.set_capacity_frames(frames_for_ms(RING_EXTRA_MS as f32 + setting.delay_ms, rate));
        let control = Arc::new(DeviceControl {
            stop: AtomicBool::new(false),
            gain: AtomicU32::new(f32_bits(device_db_to_gain(setting.gain_db))),
            delay_ms: AtomicU32::new(f32_bits(setting.delay_ms)),
            tone: Mutex::new(None),
            reset_epoch: AtomicU64::new(0),
        });
        Arc::new(Self {
            id: setting.id.clone(),
            setting: RwLock::new(setting),
            wanted_rate: AtomicU32::new(rate),
            enabled: AtomicBool::new(true),
            retired: AtomicBool::new(false),
            provider,
            source_rate,
            ring,
            last_curve_gain: AtomicU32::new(u32::MAX),
            control,
            stats: Arc::new(DeviceStats::new()),
            info: RwLock::new(info),
            thread: Mutex::new(None),
        })
    }

    fn id(&self) -> &str {
        &self.id
    }

    /// Snapshot of the current per-device settings (including its curve).
    fn setting(&self) -> DeviceSetting {
        self.setting
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn online(&self) -> bool {
        self.stats.online.load(Ordering::Relaxed)
    }

    fn accepts_audio(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) && self.online()
    }

    fn thread_running(&self) -> bool {
        self.thread
            .lock()
            .map(|guard| guard.as_ref().map(|handle| !handle.is_finished()).unwrap_or(false))
            .unwrap_or(false)
    }

    fn update_setting(&self, setting: DeviceSetting, wanted_rate: u32) {
        let rate = if self.online() {
            self.stats.actual_rate.load(Ordering::Relaxed).max(1)
        } else {
            wanted_rate.max(1)
        };
        let gain = device_db_to_gain(setting.gain_db);
        let delay = setting.delay_ms;
        if let Ok(mut guard) = self.setting.write() {
            *guard = setting;
        }
        self.wanted_rate.store(wanted_rate.max(1), Ordering::Relaxed);
        self.enabled.store(true, Ordering::Relaxed);
        self.control.gain.store(f32_bits(gain), Ordering::Relaxed);
        self.control.delay_ms.store(f32_bits(delay), Ordering::Relaxed);
        self.ring
            .set_capacity_frames(frames_for_ms(RING_EXTRA_MS as f32 + delay, rate));
        self.ring.notify_wake();
    }

    fn update_info(&self, info: DeviceInfo) {
        if let Ok(mut guard) = self.info.write() {
            *guard = info;
        }
    }


    /// Open the device, prefill silence and service the ring until stopped or a
    /// fatal device error occurs. Runs on the per-device thread.
    fn spawn(self: &Arc<Self>) {
        if self.retired.load(Ordering::Acquire) {
            return;
        }
        let mut slot = match self.thread.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        if let Some(handle) = slot.as_ref() {
            if !handle.is_finished() {
                return;
            }
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
        self.control.stop.store(false, Ordering::Release);
        self.stats.online.store(false, Ordering::Relaxed);
        self.stats.set_state(WorkerState::Starting);
        let worker = Arc::clone(self);
        let name = format!(
            "audio-{}",
            self.id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>()
        );
        match thread::Builder::new().name(name).spawn(move || run_worker(&worker)) {
            Ok(handle) => *slot = Some(handle),
            Err(error) => {
                self.stats.set_error(format!("设备 {} 无法创建输出线程：{error}", self.id));
                self.stats.set_state(WorkerState::Error);
            }
        }
    }

    /// Signal the worker to exit and wait for it.
    fn stop_and_join(&self) {
        self.control.stop.store(true, Ordering::Release);
        self.ring.notify_wake();
        if let Ok(mut slot) = self.thread.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
        self.stats.online.store(false, Ordering::Relaxed);
        if self.stats.state() != WorkerState::Error {
            self.stats.set_state(WorkerState::Offline);
        }
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.enabled.store(false, Ordering::Relaxed);
    }

    fn fail(&self, message: String) {
        self.stats.online.store(false, Ordering::Relaxed);
        self.stats.set_state(WorkerState::Error);
        self.stats.set_error(message);
        self.ring.clear();
    }

    fn status(&self) -> DeviceStatus {
        let setting = match self.setting.read() {
            Ok(guard) => guard.clone(),
            Err(_) => DeviceSetting::default(),
        };
        let online = self.online();
        let rate = if online {
            self.stats.actual_rate.load(Ordering::Relaxed).max(1)
        } else {
            self.wanted_rate.load(Ordering::Relaxed).max(1)
        };
        let fill = self.ring.fill_frames();
        let info_name = self
            .info
            .read()
            .map(|guard| guard.name.clone())
            .unwrap_or_default();
        let format = self
            .stats
            .format
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .map(|format| format.name().to_string())
            .unwrap_or_default();
        DeviceStatus {
            id: setting.id.clone(),
            name: if info_name.is_empty() { setting.name.clone() } else { info_name },
            online,
            enabled: setting.enabled,
            gain_db: setting.gain_db,
            delay_ms: setting.delay_ms,
            sample_rate: if online { rate } else { setting.sample_rate },
            channels: if online {
                self.stats.channels.load(Ordering::Relaxed) as u16
            } else {
                0
            },
            buffer_ms: (fill as f32) * 1000.0 / rate as f32,
            drift_ppm: bits_f32(self.stats.drift_ppm.load(Ordering::Relaxed)),
            underruns: self.stats.underruns.load(Ordering::Relaxed),
            written_frames: self.stats.written_frames.load(Ordering::Relaxed),
            state: self.stats.state().as_str().to_string(),
            format,
        }
    }
}

fn run_worker(worker: &Arc<DeviceWorker>) {
    if worker.retired.load(Ordering::Acquire) {
        return;
    }
    let setting = match worker.setting.read() {
        Ok(guard) => guard.clone(),
        Err(_) => return,
    };
    let wanted_rate = if setting.sample_rate != 0 {
        setting.sample_rate
    } else {
        worker
            .source_rate
            .load(Ordering::Relaxed)
            .max(worker.wanted_rate.load(Ordering::Relaxed))
            .max(1)
    };
    let mut opened = match worker.provider.open(&worker.id, wanted_rate, MASTER_CHANNELS as u16) {
        Ok(opened) => opened,
        Err(error) => {
            worker.fail(format!("设备 {} 打开失败（将自动重试）：{error:#}", worker.id));
            return;
        }
    };
    let rate = opened.rate.max(1);
    worker.stats.actual_rate.store(rate, Ordering::Relaxed);
    worker.stats.channels.store(opened.channels.max(1) as u32, Ordering::Relaxed);
    if let Ok(mut guard) = worker.stats.format.lock() {
        *guard = Some(opened.format);
    }
    let delay_ms = bits_f32(worker.control.delay_ms.load(Ordering::Relaxed)).max(0.0);
    let delay_frames = frames_for_ms(delay_ms, rate);
    let target_frames = frames_for_ms(RING_TARGET_MS as f32, rate).max(64);
    worker
        .ring
        .set_capacity_frames(frames_for_ms(RING_EXTRA_MS as f32 + delay_ms, rate).max(target_frames + delay_frames + 64));
    // Prefill: jitter-buffer target + calibration delay, as silence. This is
    // repeated on every reconnect so a replugged device restarts with the same
    // latency budget instead of an instant underrun/pop.
    worker.ring.clear();
    worker.ring.prefill_silence(target_frames + delay_frames);
    worker.stats.clear_error();
    worker.stats.online.store(true, Ordering::Release);
    worker.stats.set_state(WorkerState::Running);

    let sink: &mut dyn DeviceSink = &mut *opened.sink;
    let mut resampler = SincResampler::new(MASTER_CHANNELS);
    let mut drift = DriftEstimator::with_calibration(
        target_frames,
        DRIFT_FILL_SMOOTHING,
        MAX_DRIFT_PPM,
        DRIFT_CALIBRATION_WARMUP,
        DRIFT_CALIBRATION_UPDATES,
    );
    // 4-tier drift supervisor (dead zone / gentle / aggressive / rebuild).
    // Read once per device open; `FUSIONPLAY_FNOS_DRIFT_TIERS=0` restores the
    // legacy continuous P-loop behaviour for A/B comparison.
    let drift_tiers = DriftTierConfig::from_env();
    let mut scratch: Vec<f32> = Vec::with_capacity(MAX_BATCH_FRAMES * MASTER_CHANNELS);
    let mut resampled: Vec<f32> = Vec::with_capacity(MAX_BATCH_FRAMES * MASTER_CHANNELS * 2);
    let mut gain = bits_f32(worker.control.gain.load(Ordering::Relaxed));
    let fade_total = frames_for_ms(FADE_IN_MS, rate).max(1);
    let mut fade_done = 0usize;
    let mut applied_delay = delay_frames;
    let mut last_underrun = Instant::now() - UNDERRUN_MIN_GAP;
    let mut reset_epoch = worker.control.reset_epoch.load(Ordering::Relaxed);
    let mut ever_played = false;

    loop {
        if worker.control.stop.load(Ordering::Acquire) || worker.retired.load(Ordering::Acquire) {
            break;
        }
        // A queued test tone bypasses the audio ring entirely: it is generated
        // at the negotiated device rate and written directly.
        let tone = worker
            .control
            .tone
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(tone) = tone {
            play_tone(sink, worker, &tone, rate, bits_f32(worker.control.gain.load(Ordering::Relaxed)));
            continue;
        }
        let epoch = worker.control.reset_epoch.load(Ordering::Relaxed);
        if epoch != reset_epoch {
            reset_epoch = epoch;
            drift.reset();
            resampler.reset();
            // A batch that was already handed to the device when flush() ran
            // may still complete; zero the counters once more so the reported
            // session starts clean after the flush point.
            worker.stats.reset_counters();
            applied_delay = frames_for_ms(
                bits_f32(worker.control.delay_ms.load(Ordering::Relaxed)).max(0.0),
                rate,
            );
            // A flush restarts accounting; the next audio starts with a short
            // fade-in so the transition is not a click.
            fade_done = 0;
            ever_played = false;
        }
        // Runtime calibration delay changes are applied by growing/shrinking
        // the ring, without restarting the device or touching other devices.
        let wanted_delay = frames_for_ms(
            bits_f32(worker.control.delay_ms.load(Ordering::Relaxed)).max(0.0),
            rate,
        );
        if wanted_delay != applied_delay {
            if wanted_delay > applied_delay {
                worker.ring.prefill_silence(wanted_delay - applied_delay);
            } else {
                worker.ring.discard_frames(applied_delay - wanted_delay);
            }
            applied_delay = wanted_delay;
            worker
                .ring
                .set_capacity_frames(frames_for_ms(RING_EXTRA_MS as f32, rate).max(target_frames + wanted_delay + 64));
        }
        let target_gain = bits_f32(worker.control.gain.load(Ordering::Relaxed));
        // Condvar wait, not a poll loop: while streaming we need a ~100 ms tick
        // for underrun accounting, but a truly idle device (no data for a
        // while) only needs an occasional wake-up to observe stop/commands.
        let idle = worker
            .ring
            .last_push_ago()
            .map(|elapsed| elapsed >= IDLE_AFTER)
            .unwrap_or(!ever_played);
        let timeout = if idle {
            Duration::from_secs(1)
        } else if ever_played {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };
        match worker.ring.wait_batch(&worker.control.stop, timeout, MAX_BATCH_FRAMES, &mut scratch) {
            WaitOutcome::Data { fill } => {
                if worker.control.reset_epoch.load(Ordering::Relaxed) != reset_epoch {
                    // This batch belongs to the flushed stream: drop it and
                    // let the top of the loop reset the DSP state.
                    continue;
                }
                if !ever_played || worker.stats.state() != WorkerState::Running {
                    worker.stats.set_state(WorkerState::Running);
                }
                ever_played = true;
                let applied_ppm = apply_drift_tier(
                    &mut drift,
                    &drift_tiers,
                    &worker.ring,
                    &mut resampler,
                    fill,
                    rate,
                );
                worker.stats.drift_ppm.store(applied_ppm.to_bits(), Ordering::Relaxed);
                let source_rate = worker.source_rate.load(Ordering::Relaxed).max(1) as f64;
                let nominal = rate as f64 / source_rate;
                let ratio = nominal * (1.0 + applied_ppm as f64 / 1_000_000.0);
                resampled.clear();
                resampler.process(&scratch, ratio, &mut resampled);
                if resampled.is_empty() {
                    continue;
                }
                apply_gain(
                    &mut resampled,
                    &mut gain,
                    target_gain,
                    &mut fade_done,
                    fade_total,
                    MASTER_CHANNELS,
                );
                match sink.write(&resampled) {
                    Ok(()) => {
                        let frames = resampled.len() / MASTER_CHANNELS;
                        worker
                            .stats
                            .written_frames
                            .fetch_add(frames as u64, Ordering::Relaxed);
                    }
                    Err(error) => {
                        worker.fail(format!("设备 {} 写入失败（将自动重试）：{error:#}", worker.id));
                        sink.drain();
                        return;
                    }
                }
            }
            WaitOutcome::Empty => {
                if worker.control.reset_epoch.load(Ordering::Relaxed) != reset_epoch {
                    continue;
                }
                if ever_played {
                    let idle = worker
                        .ring
                        .last_push_ago()
                        .map(|elapsed| elapsed >= IDLE_AFTER)
                        .unwrap_or(true);
                    if idle {
                        worker.stats.set_state(WorkerState::Idle);
                    } else if last_underrun.elapsed() >= UNDERRUN_MIN_GAP {
                        // The ring ran dry while audio was still being fed:
                        // report it (rate limited), but keep the stream quiet
                        // instead of fabricating samples.
                        worker.stats.underruns.fetch_add(1, Ordering::Relaxed);
                        last_underrun = Instant::now();
                    }
                }
            }
            WaitOutcome::Stop => break,
        }
    }
    sink.drain();
    worker.stats.online.store(false, Ordering::Relaxed);
    if worker.stats.state() != WorkerState::Offline {
        if worker.stats.state() != WorkerState::Error {
            worker.stats.set_state(WorkerState::Offline);
        }
    }
}

/// Feed one ring-fill measurement through the continuous P-loop and then the
/// 4-tier supervisor, returning the ppm the device resampler must apply.
///
/// - `Hold`: the fine proportional loop result is used unchanged (legacy
///   behaviour; the fnOS ring is only 120 ms, so a large absolute dead zone
///   that switched the P-loop off would let ordinary DAC drift accumulate).
/// - `Gentle` / `Aggressive`: the P-loop result is clamped to the tier's rate
///   limit (0.03 % / 0.1 %), so a large ring excursion cannot bend the pitch.
/// - `Recover`: the ring is realigned by whole frames (pad when below target,
///   drop the oldest frames when above) and both the estimator and resampler
///   state are reset; the ring restarts at the calibrated target.
fn apply_drift_tier(
    drift: &mut DriftEstimator,
    tiers: &DriftTierConfig,
    ring: &DeviceRing,
    resampler: &mut SincResampler,
    fill: usize,
    rate: u32,
) -> f32 {
    let ppm = drift.update(fill as f32);
    // While the reference fill is being learned the target is still nominal;
    // a tier decision (especially Recover) would act on a bogus error.
    if drift.calibrating() {
        return ppm;
    }
    match tiers.decide(drift.error_frames().round() as i64, rate) {
        DriftAction::Hold => ppm,
        DriftAction::Gentle { max_ppm } | DriftAction::Aggressive { max_ppm } => {
            ppm.clamp(-max_ppm, max_ppm)
        }
        DriftAction::Recover { realign_frames } => {
            let magnitude = usize::try_from(realign_frames.unsigned_abs()).unwrap_or(usize::MAX);
            if realign_frames > 0 {
                ring.prefill_silence(magnitude);
            } else {
                ring.discard_frames(magnitude);
            }
            drift.reset();
            resampler.reset();
            0.0
        }
    }
}

/// Linear gain ramp + startup fade-in, applied in place to interleaved f32.
fn apply_gain(
    samples: &mut [f32],
    current: &mut f32,
    target: f32,
    fade_done: &mut usize,
    fade_total: usize,
    channels: usize,
) {
    let frames = samples.len() / channels;
    if frames == 0 {
        return;
    }
    let start = *current;
    let step = (target - start) / frames as f32;
    let fade_total = fade_total.max(1);
    let fade_start = *fade_done;
    for frame in 0..frames {
        let gain = start + step * frame as f32;
        let fade = if fade_start < fade_total {
            ((fade_start + frame).min(fade_total) as f32) / fade_total as f32
        } else {
            1.0
        };
        let value = gain * fade;
        for channel in 0..channels {
            samples[frame * channels + channel] *= value;
        }
    }
    *current = target;
    *fade_done = (fade_start + frames).min(fade_total);
}

fn play_tone(
    sink: &mut dyn DeviceSink,
    worker: &Arc<DeviceWorker>,
    tone: &ToneRequest,
    rate: u32,
    gain: f32,
) {
    let duration_frames = frames_for_ms(tone.duration_ms as f32, rate).max(1);
    let fade_frames = frames_for_ms(5.0, rate).max(1);
    let frequency = tone.frequency_hz.clamp(20.0, (rate as f32 / 2.0) - 100.0).max(20.0);
    let amplitude = 0.2f32 * gain.clamp(0.0, 1.0);
    let mut chunk: Vec<f32> = Vec::with_capacity(MAX_BATCH_FRAMES * MASTER_CHANNELS);
    let mut frame = 0usize;
    while frame < duration_frames {
        if worker.control.stop.load(Ordering::Acquire) {
            return;
        }
        let take = (duration_frames - frame).min(MAX_BATCH_FRAMES);
        chunk.clear();
        let mut phase = 2.0f32 * std::f32::consts::PI * frequency * frame as f32 / rate as f32;
        let phase_step = 2.0f32 * std::f32::consts::PI * frequency / rate as f32;
        for _ in 0..take {
            let envelope = if frame < fade_frames {
                frame as f32 / fade_frames as f32
            } else if frame + fade_frames >= duration_frames {
                (duration_frames - frame) as f32 / fade_frames as f32
            } else {
                1.0
            };
            let value = phase.sin() * amplitude * envelope.clamp(0.0, 1.0);
            phase += phase_step;
            chunk.push(value);
            chunk.push(value);
            frame += 1;
        }
        if let Err(error) = sink.write(&chunk) {
            worker.fail(format!("设备 {} 测试音写入失败：{error:#}", worker.id));
            sink.drain();
            return;
        }
        worker
            .stats
            .written_frames
            .fetch_add(take as u64, Ordering::Relaxed);
    }
}

struct RescanState {
    shutdown: Mutex<bool>,
    cv: Condvar,
    interval: Mutex<Duration>,
}

pub struct MultiDeviceBackend {
    settings: RwLock<Arc<AudioSettings>>,
    workers: RwLock<Arc<Vec<Arc<DeviceWorker>>>>,
    provider: Arc<dyn DeviceProvider>,
    master_frames: AtomicU64,
    dropped_frames: AtomicU64,
    source: AtomicU8,
    source_rate: Arc<AtomicU32>,
    lease: RwLock<Option<(Source, u64)>>,
    rescan: Arc<RescanState>,
    watchdog: Mutex<Option<JoinHandle<()>>>,
}

impl MultiDeviceBackend {
    /// Create an ALSA-backed engine.
    pub fn new(settings: &AudioSettings) -> Result<Arc<Self>> {
        Self::with_provider(settings, Arc::new(super::alsa::AlsaProvider::new()))
    }

    /// Create an engine with an injected provider (used by the CI mock tests and
    /// by diagnostics).
    pub fn with_provider(
        settings: &AudioSettings,
        provider: Arc<dyn DeviceProvider>,
    ) -> Result<Arc<Self>> {
        validate_curves(settings)?;
        let backend = Arc::new(Self {
            settings: RwLock::new(Arc::new(settings.clone())),
            workers: RwLock::new(Arc::new(Vec::new())),
            provider,
            master_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            source: AtomicU8::new(0),
            source_rate: Arc::new(AtomicU32::new(settings.sample_rate.max(1))),
            lease: RwLock::new(None),
            rescan: Arc::new(RescanState {
                shutdown: Mutex::new(false),
                cv: Condvar::new(),
                interval: Mutex::new(DEFAULT_RESCAN_INTERVAL),
            }),
            watchdog: Mutex::new(None),
        });
        backend.configure(settings)?;
        backend.start_watchdog();
        Ok(backend)
    }

    /// Hot-plug re-scan period. Primarily useful for tests; the default is 2 s
    /// and the watchdog only probes when at least one device is offline.
    pub fn set_rescan_interval(&self, interval: Duration) {
        let interval = interval.max(Duration::from_millis(10));
        if let Ok(mut guard) = self.rescan.interval.lock() {
            *guard = interval;
        }
        self.rescan.cv.notify_all();
    }

    /// Restrict which source/epoch may reach the outputs (used by the protocol
    /// host once it starts driving the engine directly).
    pub fn set_lease(&self, lease: Option<(Source, u64)>) {
        if let Ok(mut guard) = self.lease.write() {
            *guard = lease;
        }
    }

    fn start_watchdog(self: &Arc<Self>) {
        let weak: Weak<MultiDeviceBackend> = Arc::downgrade(self);
        let rescan = Arc::clone(&self.rescan);
        let handle = thread::Builder::new()
            .name("audio-hotplug".to_string())
            .spawn(move || watchdog_loop(weak, rescan))
            .ok();
        if let Ok(mut slot) = self.watchdog.lock() {
            *slot = handle;
        }
    }

    fn provider_devices(&self) -> Vec<DeviceInfo> {
        self.provider.enumerate()
    }

    fn configure(&self, settings: &AudioSettings) -> Result<()> {
        validate_curves(settings)?;
        // Publish the new master gain chain first: `write()` only takes a short
        // read lock on this Arc, so volume/mute/curve changes are immediate.
        if let Ok(mut guard) = self.settings.write() {
            *guard = Arc::new(settings.clone());
        }
        self.source_rate
            .store(settings.sample_rate.max(1), Ordering::Relaxed);
        // Probe without holding any lock: `write()` keeps running against the
        // current worker list while the scan happens.
        let known = self.provider.enumerate();
        let current = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        let mut existing: HashMap<String, Arc<DeviceWorker>> = current
            .iter()
            .map(|worker| (worker.id.clone(), Arc::clone(worker)))
            .collect();
        let mut next: Vec<Arc<DeviceWorker>> = Vec::new();
        let mut stop_list: Vec<(Arc<DeviceWorker>, bool)> = Vec::new();
        let mut start_list: Vec<Arc<DeviceWorker>> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for desired in settings.devices.iter().filter(|device| device.enabled) {
            if !seen.insert(desired.id.clone()) {
                continue;
            }
            let wanted_rate = if desired.sample_rate != 0 {
                desired.sample_rate
            } else {
                settings.sample_rate.max(1)
            };
            if let Some(worker) = existing.remove(&desired.id) {
                let rate_changed = worker.wanted_rate.load(Ordering::Relaxed) != wanted_rate;
                let was_online = worker.online();
                worker.update_setting(desired.clone(), wanted_rate);
                next.push(Arc::clone(&worker));
                if rate_changed {
                    stop_list.push((Arc::clone(&worker), false));
                    start_list.push(Arc::clone(&worker));
                } else if !was_online && !worker.thread_running() {
                    start_list.push(Arc::clone(&worker));
                }
            } else {
                let info = known
                    .iter()
                    .find(|info| info.id == desired.id)
                    .cloned()
                    .unwrap_or_else(|| DeviceInfo {
                        id: desired.id.clone(),
                        name: desired.name.clone(),
                        card: String::new(),
                        device: 0,
                        sample_rates: Vec::new(),
                        channels: 0,
                        is_default: false,
                        online: false,
                    });
                let worker = DeviceWorker::new(
                    desired.clone(),
                    wanted_rate,
                    info,
                    Arc::clone(&self.provider),
                    Arc::clone(&self.source_rate),
                );
                next.push(Arc::clone(&worker));
                start_list.push(worker);
            }
        }
        for (_, worker) in existing.drain() {
            worker.retire();
            stop_list.push((worker, true));
        }
        // Swap the list before stopping removed workers so `write()` never
        // targets a device that is being torn down.
        if let Ok(mut guard) = self.workers.write() {
            *guard = Arc::new(next);
        }
        for (worker, _retired) in &stop_list {
            worker.stop_and_join();
        }
        for worker in start_list {
            worker.spawn();
        }
        Ok(())
    }

    fn rescan_once(&self) {
        let workers = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        let offline: Vec<Arc<DeviceWorker>> = workers
            .iter()
            .filter(|worker| {
                worker.enabled.load(Ordering::Relaxed)
                    && !worker.online()
                    && !worker.thread_running()
                    && !worker.retired.load(Ordering::Relaxed)
            })
            .cloned()
            .collect();
        if offline.is_empty() {
            return;
        }
        // Slow provider scan with no lock held: audio keeps flowing meanwhile.
        let present = self.provider.enumerate();
        if present.is_empty() {
            return;
        }
        let present_ids: HashSet<&str> = present.iter().map(|info| info.id.as_str()).collect();
        // Devices that vanished while idle: the worker thread has no pending
        // write to fail, so mark it offline here and stop it.
        for worker in workers.iter() {
            if worker.online() && !present_ids.contains(worker.id()) {
                worker.fail(format!("设备 {} 已移除（热插拔）", worker.id));
                worker.stop_and_join();
            }
        }
        for worker in offline {
            if let Some(info) = present.iter().find(|info| info.id == worker.id()) {
                worker.update_info(info.clone());
                worker.spawn();
            }
        }
    }

    fn status_inner(&self) -> EngineStatus {
        let workers = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        let devices: Vec<DeviceStatus> = workers.iter().map(|worker| worker.status()).collect();
        EngineStatus {
            mode: "alsa".to_string(),
            source: decode_source(self.source.load(Ordering::Relaxed))
                .map(|source| source.as_str().to_string()),
            master_frames: self.master_frames.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            devices,
        }
    }

    fn write_inner(
        &self,
        source: Source,
        epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        if samples.is_empty() {
            return;
        }
        let frames = if channels == 0 { 0 } else { samples.len() / channels as usize };
        if channels as usize != MASTER_CHANNELS || samples.len() < MASTER_CHANNELS {
            self.dropped_frames
                .fetch_add(frames.max(1) as u64, Ordering::Relaxed);
            return;
        }
        if let Ok(lease) = self.lease.read() {
            if let Some((lease_source, lease_epoch)) = *lease {
                if lease_source != source || lease_epoch != epoch {
                    self.dropped_frames
                        .fetch_add(frames as u64, Ordering::Relaxed);
                    return;
                }
            }
        }
        // Per-device gain chain: sender logical volume -> that device's curve
        // -> global attenuation -> mute. Curves are evaluated once per block;
        // the block-level ramp inside the ring smooths changes.
        let settings = match self.settings.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        let global_gain = if settings.muted {
            0.0
        } else {
            settings.global_attenuation_percent.clamp(0.0, 100.0) / 100.0
        };
        self.source.store(encode_source(source), Ordering::Relaxed);
        if sample_rate > 0 {
            self.source_rate.store(sample_rate, Ordering::Relaxed);
        }
        self.master_frames.fetch_add(frames as u64, Ordering::Relaxed);
        let workers = match self.workers.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        let mut dropped = 0u64;
        for worker in workers.iter() {
            if !worker.accepts_audio() {
                continue;
            }
            let curve_gain = worker.setting().curve.apply(sender_gain);
            let target = curve_gain * global_gain;
            let last_bits = worker.last_curve_gain.load(Ordering::Relaxed);
            let from = if last_bits == u32::MAX {
                target
            } else {
                bits_f32(last_bits)
            };
            worker
                .last_curve_gain
                .store(f32_bits(target), Ordering::Relaxed);
            dropped += worker.ring.push_ramped(samples, from, target);
        }
        if dropped > 0 {
            self.dropped_frames.fetch_add(dropped, Ordering::Relaxed);
        }
    }

    fn flush_inner(&self) {
        let workers = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        for worker in workers.iter() {
            worker.ring.clear();
            worker.control.reset_epoch.fetch_add(1, Ordering::Relaxed);
            worker.stats.reset_counters();
            worker.ring.notify_wake();
        }
        // A flush starts a new accounting epoch: counters restart at zero so
        // the UI reports statistics for the current stream, not the lifetime.
        self.master_frames.store(0, Ordering::Relaxed);
        self.dropped_frames.store(0, Ordering::Relaxed);
    }

    fn test_tone_inner(&self, device_id: &str) -> Result<()> {
        let workers = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        let worker = workers
            .iter()
            .find(|worker| worker.id() == device_id)
            .ok_or_else(|| anyhow!("设备未启用或不存在：{device_id}"))?;
        if !worker.online() {
            return Err(anyhow!("设备 {device_id} 当前离线，无法播放测试音"));
        }
        if let Ok(mut guard) = worker.control.tone.lock() {
            *guard = Some(ToneRequest::default());
        }
        worker.ring.notify_wake();
        Ok(())
    }
}

impl OutputBackend for MultiDeviceBackend {
    fn configure(&self, settings: &AudioSettings) -> Result<()> {
        MultiDeviceBackend::configure(self, settings)
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.provider_devices()
    }

    fn status(&self) -> EngineStatus {
        self.status_inner()
    }

    fn write(
        &self,
        source: Source,
        epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        self.write_inner(source, epoch, sender_gain, sample_rate, channels, samples)
    }

    fn flush(&self, _source: Source) {
        self.flush_inner()
    }

    fn test_tone(&self, device_id: &str) -> Result<()> {
        self.test_tone_inner(device_id)
    }
}

impl Drop for MultiDeviceBackend {
    fn drop(&mut self) {
        if let Ok(mut shutdown) = self.rescan.shutdown.lock() {
            *shutdown = true;
        }
        self.rescan.cv.notify_all();
        if let Ok(mut slot) = self.watchdog.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
        let workers = self
            .workers
            .read()
            .map(|guard| Arc::clone(&guard))
            .unwrap_or_else(|_| Arc::new(Vec::new()));
        for worker in workers.iter() {
            worker.retire();
            worker.control.stop.store(true, Ordering::Release);
            worker.ring.notify_wake();
        }
        for worker in workers.iter() {
            if let Ok(mut slot) = worker.thread.lock() {
                if let Some(handle) = slot.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

fn watchdog_loop(weak: Weak<MultiDeviceBackend>, rescan: Arc<RescanState>) {
    loop {
        let interval = rescan
            .interval
            .lock()
            .map(|guard| *guard)
            .unwrap_or(DEFAULT_RESCAN_INTERVAL);
        {
            let mut shutdown = match rescan.shutdown.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if *shutdown {
                return;
            }
            match rescan.cv.wait_timeout(shutdown, interval) {
                Ok((next, _)) => {
                    shutdown = next;
                    if *shutdown {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        let Some(backend) = weak.upgrade() else {
            return;
        };
        backend.rescan_once();
    }
}
