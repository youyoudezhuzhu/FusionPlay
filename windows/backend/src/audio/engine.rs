//! Single-device cpal output engine with a null fallback.
//!
//! Audio flow:
//!
//! ```text
//! AirPlay/MiPlay/DLNA  --gain + downmix-->  SincResampler(48 kHz)  --+
//!                                                                   |
//!                                             bounded ring (500 ms) |
//!                                                                   v
//!  cpal callback  <-- resample to device rate --  pop block (try_lock, never blocks)
//! ```
//!
//! Producers (protocol threads) never block the audio callback: they take a
//! short mutex on their own source converter and a short ring mutex. The cpal
//! callback only ever uses `try_lock`; on contention or an empty ring it emits
//! silence and increments the underrun counter (rate-limited to one per 250 ms).
//!
//! Clock drift between the sender clock and the output device is corrected the
//! same way as on fnOS: the ring fill level feeds a [`DriftEstimator`] whose
//! ppm offset modulates the callback resampler ratio.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use super::null::NullSink;
use super::resample::{
    DriftEstimator, SincResampler, DRIFT_CALIBRATION_UPDATES, DRIFT_CALIBRATION_WARMUP,
    DRIFT_FILL_SMOOTHING, MAX_DRIFT_PPM,
};
use super::{
    sanitize_gain, EngineConfig, EngineStatus, PcmSink, TARGET_CHANNELS, TARGET_RATE, WarnSink,
};
use crate::session::Source;

/// Ring target fill used as the drift-control reference (frames @ 48 kHz).
const RING_TARGET_MS: usize = 120;

/// Ring capacity in ms. Overflow drops the oldest whole frames so latency
/// stays bounded after a scheduler stall instead of drifting behind forever.
const RING_CAPACITY_MS: usize = 500;

/// How long without new audio before the output is considered idle (not
/// starving), so a paused receiver does not accumulate underruns.
const IDLE_AFTER: Duration = Duration::from_millis(750);

/// Minimum gap between two underrun counters for the callback.
const UNDERRUN_MIN_GAP: Duration = Duration::from_millis(250);

/// Upper bound of master frames pulled from the ring per callback iteration.
const MAX_CALLBACK_INPUT_FRAMES: usize = 1_024;

/// Extra input frames pulled per callback so resampler latency does not force
/// an artificial underrun.
const CALLBACK_LOOKAHEAD_FRAMES: usize = 64;

/// Requested output period: a fixed 10 ms device buffer (WASAPI shared mode
/// otherwise picks 20-30 ms by itself, which is the dominant output latency).
const OUTPUT_PERIOD_MS: f64 = 10.0;

/// Conservative output period estimate when the device rejects the fixed
/// buffer request and the stream runs on its default buffer.
const FALLBACK_OUTPUT_PERIOD_MS: f64 = 20.0;

/// Driver/OS scheduling overhead added on top of ring fill + device period
/// when reporting this output stage's latency to the MiPlay scheduler.
const OUTPUT_LATENCY_CONSTANT_MS: f64 = 5.0;

/// Lower bound for the requested fixed buffer (hardware-friendly minimum).
const MIN_FIXED_BUFFER_FRAMES: u32 = 64;

/// Upper bound for the requested fixed buffer (5 frames at 48 kHz would be a
/// 500 ms period, no backend accepts that as a "small" buffer).
const MAX_FIXED_BUFFER_FRAMES: u32 = 4_800;

/// Device frames for a nominal 10 ms fixed buffer at `sample_rate`, clamped to
/// a range every output backend can plausibly serve.
fn fixed_buffer_frames(sample_rate: u32) -> u32 {
    (sample_rate / 100).clamp(MIN_FIXED_BUFFER_FRAMES, MAX_FIXED_BUFFER_FRAMES)
}

const SOURCE_COUNT: usize = 3;

fn source_index(source: Source) -> usize {
    match source {
        Source::AirPlay => 0,
        Source::Dlna => 1,
        Source::MiPlay => 2,
    }
}

fn encode_source(source: Source) -> u8 {
    source_index(source) as u8 + 1
}

fn decode_source(value: u8) -> Option<Source> {
    match value {
        1 => Some(Source::AirPlay),
        2 => Some(Source::Dlna),
        3 => Some(Source::MiPlay),
        _ => None,
    }
}

fn new_ingest_array() -> [Mutex<SourceIngest>; SOURCE_COUNT] {
    std::array::from_fn(|_| Mutex::new(SourceIngest::new()))
}

// ---------------------------------------------------------------------------
// Bounded ring shared with the cpal callback
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RingInner {
    samples: VecDeque<f32>,
    last_push: Option<Instant>,
}

struct AudioRing {
    inner: Mutex<RingInner>,
    channels: usize,
    capacity_samples: usize,
}

impl AudioRing {
    fn new(channels: usize, capacity_frames: usize) -> Self {
        let channels = channels.max(1);
        let capacity_samples = capacity_frames.max(1) * channels;
        Self {
            inner: Mutex::new(RingInner {
                samples: VecDeque::with_capacity(capacity_samples.min(TARGET_RATE as usize)),
                last_push: None,
            }),
            channels,
            capacity_samples,
        }
    }

    /// Producer path: bounded copy, drops the oldest frames on overflow.
    fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.last_push = Some(Instant::now());
        if samples.len() >= self.capacity_samples {
            inner.samples.clear();
            let keep_from = samples.len() - self.capacity_samples;
            inner.samples.extend(samples[keep_from..].iter().copied());
            return;
        }
        let overflow = inner
            .samples
            .len()
            .saturating_add(samples.len())
            .saturating_sub(self.capacity_samples);
        if overflow > 0 {
            let buffered = inner.samples.len();
            inner.samples.drain(..overflow.min(buffered));
        }
        inner.samples.extend(samples.iter().copied());
    }

    /// Consumer path (audio callback). `None` means a producer holds the lock
    /// right now: callers must treat that as a momentary underrun, never block.
    fn try_pop_into(&self, destination: &mut [f32]) -> Option<usize> {
        let mut inner = self.inner.try_lock().ok()?;
        let count = destination.len().min(inner.samples.len());
        for slot in destination[..count].iter_mut() {
            *slot = inner.samples.pop_front().unwrap_or(0.0);
        }
        Some(count)
    }

    fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.samples.clear();
        }
    }

    /// Fill level in frames (0 when the callback currently cannot lock).
    fn fill_frames(&self) -> usize {
        self.inner
            .try_lock()
            .map(|inner| inner.samples.len() / self.channels)
            .unwrap_or(0)
    }

    /// Fill level of queued master audio in milliseconds.
    ///
    /// Read-only diagnostic/scheduling path, never the audio callback: a
    /// blocking lock is fine here and avoids reporting a spurious 0 while a
    /// producer is appending a block.
    fn fill_millis(&self) -> f64 {
        let frames = self
            .inner
            .lock()
            .map(|inner| inner.samples.len() / self.channels)
            .unwrap_or(0);
        frames as f64 * 1_000.0 / TARGET_RATE as f64
    }

    fn last_push_age(&self) -> Option<Duration> {
        self.inner
            .try_lock()
            .ok()
            .and_then(|inner| inner.last_push.map(|instant| instant.elapsed()))
    }

    fn is_idle(&self) -> bool {
        self.fill_frames() == 0
            && self
                .last_push_age()
                .is_none_or(|age| age > IDLE_AFTER)
    }
}

// ---------------------------------------------------------------------------
// Source format conversion (any rate/channels -> 48 kHz stereo, gain applied)
// ---------------------------------------------------------------------------

struct SourceIngest {
    input_rate: u32,
    stereo: Vec<f32>,
    master: Vec<f32>,
    resampler: Option<SincResampler>,
}

impl SourceIngest {
    fn new() -> Self {
        Self {
            input_rate: 0,
            stereo: Vec::new(),
            master: Vec::new(),
            resampler: None,
        }
    }

    fn reset(&mut self) {
        self.input_rate = 0;
        self.stereo.clear();
        self.master.clear();
        self.resampler = None;
    }

    /// Convert one decoded block into the 48 kHz stereo master mix. The sender
    /// gain is applied exactly once here, before the ring.
    fn render(
        &mut self,
        sample_rate: u32,
        channels: u16,
        gain: f32,
        samples: &[f32],
    ) -> &[f32] {
        let Self {
            input_rate,
            stereo,
            master,
            resampler,
        } = self;
        master.clear();
        if samples.is_empty() || channels == 0 || sample_rate == 0 {
            return master;
        }
        let gain = sanitize_gain(gain);
        let input_channels = channels as usize;

        // Fast path: native master format with unity gain, no copy at all.
        if sample_rate == TARGET_RATE && input_channels == TARGET_CHANNELS as usize && gain == 1.0
        {
            *resampler = None;
            *input_rate = sample_rate;
            master.extend_from_slice(samples);
            return master;
        }

        // A rate change invalidates the resampler history.
        if *input_rate != sample_rate {
            *input_rate = sample_rate;
            *resampler = None;
        }

        stereo.clear();
        match input_channels {
            1 => {
                stereo.reserve(samples.len() * 2);
                for &sample in samples {
                    let value = sample * gain;
                    stereo.push(value);
                    stereo.push(value);
                }
            }
            2 => {
                stereo.reserve(samples.len());
                stereo.extend(samples.iter().map(|sample| sample * gain));
            }
            count => {
                let frames = samples.len() / count;
                stereo.reserve(frames * 2);
                for frame in samples.chunks_exact(count) {
                    let mut left = frame[0] * gain;
                    let mut right = frame[1] * gain;
                    // Fold unknown extra channels into both sides, scaled so a
                    // full-scale multi-channel frame cannot clip.
                    for extra in &frame[2..] {
                        let value = *extra * gain * 0.5;
                        left += value;
                        right += value;
                    }
                    stereo.push(left.clamp(-1.0, 1.0));
                    stereo.push(right.clamp(-1.0, 1.0));
                }
            }
        }

        if sample_rate == TARGET_RATE {
            *resampler = None;
            master.extend_from_slice(stereo);
            return master;
        }

        if resampler.is_none() {
            *resampler = Some(SincResampler::new(TARGET_CHANNELS as usize));
        }
        let ratio = TARGET_RATE as f64 / sample_rate as f64;
        if let Some(engine) = resampler.as_mut() {
            engine.process(stereo, ratio, master);
        } else {
            master.extend_from_slice(stereo);
        }
        master
    }
}

// ---------------------------------------------------------------------------
// Sinks
// ---------------------------------------------------------------------------

struct DeviceSink {
    ring: Arc<AudioRing>,
}

impl PcmSink for DeviceSink {
    fn consume(&self, samples: &[f32]) {
        self.ring.push(samples);
    }
}

// ---------------------------------------------------------------------------
// cpal callback state
// ---------------------------------------------------------------------------

struct CallbackState {
    ring: Arc<AudioRing>,
    underruns: Arc<AtomicU64>,
    out_channels: usize,
    /// Output rate / 48 kHz (device frames per master frame).
    ratio: f64,
    drift: DriftEstimator,
    resampler: SincResampler,
    /// Ring pop scratch (interleaved 48 kHz stereo).
    input: Vec<f32>,
    /// Resampled output queue (interleaved 48 kHz stereo).
    master: Vec<f32>,
    /// Samples of `master` already written to the device.
    consumed: usize,
    last_underrun: Option<Instant>,
}

impl CallbackState {
    fn new(
        ring: Arc<AudioRing>,
        underruns: Arc<AtomicU64>,
        out_rate: u32,
        out_channels: u16,
    ) -> Self {
        let target_frames = TARGET_RATE as usize * RING_TARGET_MS / 1_000;
        Self {
            ring,
            underruns,
            out_channels: (out_channels as usize).max(1),
            ratio: out_rate.max(1) as f64 / TARGET_RATE as f64,
            drift: DriftEstimator::with_calibration(
                target_frames,
                DRIFT_FILL_SMOOTHING,
                MAX_DRIFT_PPM,
                DRIFT_CALIBRATION_WARMUP,
                DRIFT_CALIBRATION_UPDATES,
            ),
            resampler: SincResampler::new(TARGET_CHANNELS as usize),
            input: Vec::new(),
            master: Vec::new(),
            consumed: 0,
            last_underrun: None,
        }
    }

    fn master_frames(&self) -> usize {
        self.master
            .len()
            .saturating_sub(self.consumed)
            / TARGET_CHANNELS as usize
    }

    fn effective_ratio(&self) -> f64 {
        self.ratio * self.drift.ratio()
    }

    fn compact_master(&mut self) {
        if self.consumed == 0 {
            return;
        }
        if self.consumed >= self.master.len() {
            self.master.clear();
            self.consumed = 0;
        } else if self.consumed >= 4_096 {
            self.master.drain(..self.consumed);
            self.consumed = 0;
        }
    }

    fn note_underrun(&mut self) {
        let recent = self
            .ring
            .last_push_age()
            .is_some_and(|age| age <= IDLE_AFTER);
        if !recent {
            return;
        }
        if self
            .last_underrun
            .is_none_or(|last| last.elapsed() >= UNDERRUN_MIN_GAP)
        {
            self.underruns.fetch_add(1, Ordering::Relaxed);
            self.last_underrun = Some(Instant::now());
        }
    }

    fn render<T>(&mut self, data: &mut [T])
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = self.out_channels;
        let frames = data.len() / channels;
        if frames == 0 {
            return;
        }
        self.compact_master();
        if self.master_frames() == 0 && self.ring.is_idle() {
            fill_silence(data);
            return;
        }

        let mut rounds = 0;
        while self.master_frames() < frames && rounds < 16 {
            rounds += 1;
            let deficit = frames - self.master_frames();
            let wanted_input = ((deficit as f64) / self.effective_ratio()).ceil() as usize
                + CALLBACK_LOOKAHEAD_FRAMES;
            let wanted_input = wanted_input.clamp(64, MAX_CALLBACK_INPUT_FRAMES);
            self.input
                .resize(wanted_input * TARGET_CHANNELS as usize, 0.0);
            let popped = self
                .ring
                .try_pop_into(&mut self.input)
                .unwrap_or(0);
            // Keep whole stereo frames only.
            let popped = popped - popped % TARGET_CHANNELS as usize;
            if popped == 0 {
                self.note_underrun();
                break;
            }
            let ppm = self.drift.update(self.ring.fill_frames() as f32);
            let ratio = (self.ratio * (1.0 + ppm as f64 / 1_000_000.0)).clamp(0.25, 4.0);
            let mut produced = std::mem::take(&mut self.master);
            self.resampler
                .process(&self.input[..popped], ratio, &mut produced);
            self.master = produced;
        }

        let available = self.master_frames().min(frames);
        for frame in 0..available {
            let base = self.consumed + frame * TARGET_CHANNELS as usize;
            write_frame(
                data,
                frame,
                channels,
                self.master[base],
                self.master[base + 1],
            );
        }
        if available < frames {
            self.note_underrun();
            fill_silence_from(data, available, channels);
        }
        self.consumed += available * TARGET_CHANNELS as usize;
    }
}

/// Map one 48 kHz stereo frame into the device's sample type and channel count.
#[inline]
fn write_frame<T>(data: &mut [T], frame: usize, channels: usize, left: f32, right: f32)
where
    T: SizedSample + FromSample<f32>,
{
    let base = frame * channels;
    if channels == 1 {
        data[base] = T::from_sample((left + right) * 0.5);
        return;
    }
    data[base] = T::from_sample(left);
    data[base + 1] = T::from_sample(right);
    for slot in &mut data[base + 2..base + channels] {
        *slot = T::EQUILIBRIUM;
    }
}

fn fill_silence<T: Sample>(data: &mut [T]) {
    for slot in data.iter_mut() {
        *slot = T::EQUILIBRIUM;
    }
}

fn fill_silence_from<T: Sample>(data: &mut [T], first_frame: usize, channels: usize) {
    let start = (first_frame * channels).min(data.len());
    for slot in &mut data[start..] {
        *slot = T::EQUILIBRIUM;
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct StatusInfo {
    mode: String,
    device_name: Option<String>,
    sample_rate: u32,
    channels: u16,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            mode: String::new(),
            device_name: None,
            sample_rate: TARGET_RATE,
            channels: TARGET_CHANNELS,
        }
    }
}

/// Opened hardware output, kept alive by [`AudioEngine`].
struct OpenedDevice {
    stream: cpal::Stream,
    ring: Arc<AudioRing>,
    underruns: Arc<AtomicU64>,
    sample_rate: u32,
    channels: u16,
    device_name: String,
    /// Estimated device output period in milliseconds (10 ms when the fixed
    /// buffer request was accepted, the fallback estimate otherwise).
    output_period_millis: f64,
}

/// The unified audio engine. One instance owns the default output device (or
/// the null sink) and accepts PCM from all three protocols.
pub struct AudioEngine {
    sink: Arc<dyn PcmSink>,
    ring: Option<Arc<AudioRing>>,
    null_sink: Option<Arc<NullSink>>,
    ingest: [Mutex<SourceIngest>; SOURCE_COUNT],
    /// Encoded active source (1..=3, 0 = none). PCM from other sources is
    /// dropped immediately.
    active: AtomicU8,
    stopped: AtomicBool,
    underruns: Arc<AtomicU64>,
    info: RwLock<StatusInfo>,
    stream: Mutex<Option<cpal::Stream>>,
    /// Device output period estimate in ms; 0 for the null backend.
    output_period_millis: f64,
}

impl AudioEngine {
    /// Start the engine. Never fails because of audio hardware: `--null-audio`
    /// selects the null sink and any cpal failure falls back to it as well
    /// (with one Chinese warning through `warn`).
    pub fn start(config: EngineConfig, warn: WarnSink) -> Result<Arc<Self>> {
        Self::start_with(config, warn, open_device)
    }

    /// Testable variant of [`AudioEngine::start`] with an injectable device
    /// opener (unit tests never touch a sound card).
    fn start_with<F>(config: EngineConfig, warn: WarnSink, open: F) -> Result<Arc<Self>>
    where
        F: FnOnce(&EngineConfig, &WarnSink) -> Result<OpenedDevice>,
    {
        if config.null_audio {
            return Ok(Self::with_null(false));
        }
        // cpal host initialisation itself can panic on a machine without an
        // audio subsystem; that must still degrade to the null sink.
        let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            open(&config, &warn)
        }));
        match opened {
            Ok(Ok(device)) => Ok(Self::with_device(device)),
            Ok(Err(error)) => {
                warn(&format!(
                    "音频输出不可用，已切换为静默模式（{error}）；接收器仍可被发现与控制。"
                ));
                Ok(Self::with_null(false))
            }
            Err(_) => {
                warn("音频子系统初始化失败，已切换为静默模式；接收器仍可被发现与控制。");
                Ok(Self::with_null(false))
            }
        }
    }

    /// Null engine that records the master mix. Test/diagnostic helper only.
    #[doc(hidden)]
    pub fn start_capture(warn: WarnSink) -> Arc<Self> {
        let _ = warn;
        Self::with_null(true)
    }

    fn with_null(capture: bool) -> Arc<Self> {
        let null_sink = Arc::new(NullSink::new(capture));
        let sink: Arc<dyn PcmSink> = Arc::clone(&null_sink) as Arc<dyn PcmSink>;
        Arc::new(Self {
            sink,
            ring: None,
            null_sink: Some(null_sink),
            ingest: new_ingest_array(),
            active: AtomicU8::new(0),
            stopped: AtomicBool::new(false),
            underruns: Arc::new(AtomicU64::new(0)),
            info: RwLock::new(StatusInfo {
                mode: "null".to_string(),
                device_name: None,
                sample_rate: TARGET_RATE,
                channels: TARGET_CHANNELS,
            }),
            stream: Mutex::new(None),
            output_period_millis: 0.0,
        })
    }

    fn with_device(opened: OpenedDevice) -> Arc<Self> {
        let ring = Arc::clone(&opened.ring);
        let sink: Arc<dyn PcmSink> = Arc::new(DeviceSink {
            ring: Arc::clone(&ring),
        });
        let status = StatusInfo {
            mode: "cpal".to_string(),
            device_name: Some(opened.device_name),
            sample_rate: opened.sample_rate,
            channels: opened.channels,
        };
        Arc::new(Self {
            sink,
            ring: Some(ring),
            null_sink: None,
            ingest: new_ingest_array(),
            active: AtomicU8::new(0),
            stopped: AtomicBool::new(false),
            underruns: opened.underruns,
            info: RwLock::new(status),
            stream: Mutex::new(Some(opened.stream)),
            output_period_millis: opened.output_period_millis,
        })
    }

    /// Current engine status. Cheap; safe to call at the UI refresh rate.
    pub fn status(&self) -> EngineStatus {
        let info = self.info.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        EngineStatus {
            mode: info.mode.clone(),
            device_name: info.device_name.clone(),
            sample_rate: info.sample_rate,
            channels: info.channels,
            underruns: self.underruns.load(Ordering::Relaxed),
        }
    }

    /// Estimated latency of this output stage in milliseconds: the queued
    /// master ring fill + one device output period + a driver/scheduling
    /// margin. The MiPlay scheduler uses it to align PCM with the sender's
    /// presentation clock; the null backend has no output stage and reports
    /// `0.0`.
    pub fn output_latency_millis(&self) -> f64 {
        let Some(ring) = &self.ring else {
            return 0.0;
        };
        ring.fill_millis() + self.output_period_millis + OUTPUT_LATENCY_CONSTANT_MS
    }

    /// Drop every queued sample for `source` (used on takeover/seek). The ring
    /// is shared, so this clears it unconditionally: only one source may be
    /// active at a time, everything else is being discarded anyway.
    pub fn flush_source(&self, source: Source) {
        let index = source_index(source);
        if let Ok(mut ingest) = self.ingest[index].lock() {
            ingest.reset();
        }
        if let Some(ring) = &self.ring {
            ring.clear();
        }
    }

    /// DLNA PCM entry point (the decoder always converts to 48 kHz stereo
    /// before pushing, `sender_gain` is the DLNA volume as a linear factor).
    pub fn write_dlna_pcm(
        &self,
        epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        self.write_internal(Source::Dlna, epoch, sender_gain, sample_rate, channels, samples);
    }

    /// Only PCM from this source reaches the output; `None` mutes everything.
    /// `protocols.rs` mirrors `Arbiter::current_source()` into this method.
    ///
    /// Switching sources drops everything still queued: the ring is shared, so
    /// a few hundred milliseconds of the previous source must not leak into the
    /// new one (and releasing the last source must mute immediately).
    pub fn set_active_source(&self, source: Option<Source>) {
        let encoded = source.map(encode_source).unwrap_or(0);
        let previous = self.active.swap(encoded, Ordering::AcqRel);
        if previous == encoded {
            return;
        }
        if let Some(ring) = &self.ring {
            ring.clear();
        }
        if let Ok(mut ingests) = self.ingest.iter().map(|ingest| ingest.lock()).collect::<Result<Vec<_>, _>>() {
            for ingest in ingests.iter_mut() {
                ingest.reset();
            }
        }
    }

    pub fn active_source(&self) -> Option<Source> {
        decode_source(self.active.load(Ordering::Acquire))
    }

    pub fn is_active_source(&self, source: Source) -> bool {
        self.active_source() == Some(source)
    }

    /// Stop output and drop the device stream. Idempotent.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut guard) = self.stream.lock()
            && let Some(stream) = guard.take()
        {
            let _ = stream.pause();
            drop(stream);
        }
        if let Some(ring) = &self.ring {
            ring.clear();
        }
        for ingest in &self.ingest {
            if let Ok(mut ingest) = ingest.lock() {
                ingest.reset();
            }
        }
    }

    /// Master frames consumed by the null sink (tests/diagnostics).
    #[doc(hidden)]
    pub fn null_frames(&self) -> u64 {
        self.null_sink
            .as_ref()
            .map(|sink| sink.frames())
            .unwrap_or(0)
    }

    /// Copy of the recorded master mix (empty unless built by
    /// [`AudioEngine::start_capture`]).
    #[doc(hidden)]
    pub fn captured(&self) -> Vec<f32> {
        self.null_sink
            .as_ref()
            .map(|sink| sink.captured())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn clear_captured(&self) {
        if let Some(sink) = &self.null_sink {
            sink.clear_captured();
        }
    }

    fn write_internal(
        &self,
        source: Source,
        _epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        if self.stopped.load(Ordering::Acquire)
            || samples.is_empty()
            || channels == 0
            || sample_rate == 0
        {
            return;
        }
        // Only the active source is audible; non-active PCM is dropped without
        // blocking the protocol thread.
        if self.active.load(Ordering::Acquire) != encode_source(source) {
            return;
        }
        let index = source_index(source);
        let Ok(mut ingest) = self.ingest[index].lock() else {
            return;
        };
        let rendered = ingest.render(sample_rate, channels, sender_gain, samples);
        self.sink.consume(rendered);
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.stream.lock()
            && let Some(stream) = guard.take()
        {
            let _ = stream.pause();
            drop(stream);
        }
    }
}

impl fusionplay_core::PcmOutput for AudioEngine {
    fn write(
        &self,
        source: fusionplay_core::PcmSource,
        epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        self.write_internal(
            Source::from(source),
            epoch,
            sender_gain,
            sample_rate,
            channels,
            samples,
        );
    }

    fn flush(&self, source: fusionplay_core::PcmSource) {
        self.flush_source(Source::from(source));
    }
}

impl fusionplay_miplay_sdk::MiPlayPcmOutput for AudioEngine {
    fn write(&self, sender_gain: f32, sample_rate: u32, channels: u16, samples: &[f32]) {
        self.write_internal(Source::MiPlay, 0, sender_gain, sample_rate, channels, samples);
    }

    fn flush(&self) {
        self.flush_source(Source::MiPlay);
    }

    /// Queue + device latency of this host output stage, so the SDK can
    /// schedule PCM against the sender's presentation clock instead of
    /// applying its conservative default. The null backend reports 0 ms.
    fn output_latency_millis(&self) -> Option<f64> {
        Some(AudioEngine::output_latency_millis(self))
    }
}

// ---------------------------------------------------------------------------
// Device selection / stream construction
// ---------------------------------------------------------------------------

fn open_device(config: &EngineConfig, warn: &WarnSink) -> Result<OpenedDevice> {
    let host = cpal::default_host();
    let device = resolve_output_device(&host, config.output_device.as_deref())?;
    let device_name = device
        .description()
        .map(|description| description.name().to_string())
        .unwrap_or_else(|_| "未知音频输出设备".to_string());
    let supported = select_config(&device)?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();

    let ring = Arc::new(AudioRing::new(
        TARGET_CHANNELS as usize,
        TARGET_RATE as usize * RING_CAPACITY_MS / 1_000,
    ));
    let underruns = Arc::new(AtomicU64::new(0));
    let stream_config = supported.config();
    let (stream, output_period_millis) = build_stream(
        &device,
        &stream_config,
        Arc::clone(&ring),
        Arc::clone(&underruns),
        supported.sample_format(),
        warn,
    )?;
    stream.play().context("无法启动音频输出流")?;
    Ok(OpenedDevice {
        stream,
        ring,
        underruns,
        sample_rate,
        channels,
        device_name,
        output_period_millis,
    })
}

fn resolve_output_device(host: &cpal::Host, requested: Option<&str>) -> Result<cpal::Device> {
    let Some(requested) = requested.map(str::trim).filter(|name| !name.is_empty()) else {
        return host
            .default_output_device()
            .ok_or_else(|| anyhow!("系统没有默认音频输出设备"));
    };
    if let Ok(id) = requested.parse::<cpal::DeviceId>()
        && let Some(device) = host.device_by_id(&id)
    {
        return Ok(device);
    }
    let mut partial = None;
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let name = device
                .description()
                .map(|description| description.name().to_string())
                .unwrap_or_default();
            let id = device.id().map(|id| id.to_string()).unwrap_or_default();
            if name.eq_ignore_ascii_case(requested) || id == requested {
                return Ok(device);
            }
            if partial.is_none() && name.to_lowercase().contains(&requested.to_lowercase()) {
                partial = Some(device);
            }
        }
    }
    if let Some(device) = partial {
        return Ok(device);
    }
    bail!("找不到指定的音频输出设备：{requested}")
}

/// Prefer a 48 kHz stereo stream so the device callback runs without
/// resampling; fall back to the device default (resampled in the callback).
fn select_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let mut fallback = None;
    if let Ok(ranges) = device.supported_output_configs() {
        for range in ranges {
            if !playable_format(range.sample_format()) {
                continue;
            }
            let Some(candidate) = range.try_with_sample_rate(TARGET_RATE) else {
                continue;
            };
            if candidate.channels() == TARGET_CHANNELS {
                if candidate.sample_format() == SampleFormat::F32 {
                    return Ok(candidate);
                }
                if fallback.is_none() {
                    fallback = Some(candidate);
                }
            }
        }
    }
    if let Some(config) = fallback {
        return Ok(config);
    }
    let default = device
        .default_output_config()
        .context("无法获取音频输出设备的默认格式")?;
    if !playable_format(default.sample_format()) {
        bail!(
            "音频输出设备使用了不支持的采样格式：{}",
            default.sample_format()
        );
    }
    Ok(default)
}

fn playable_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I24
            | SampleFormat::I32
            | SampleFormat::I64
            | SampleFormat::U8
            | SampleFormat::U16
            | SampleFormat::U24
            | SampleFormat::U32
            | SampleFormat::U64
    )
}

/// Build the output stream, preferring a fixed 10 ms device buffer.
///
/// WASAPI shared mode otherwise picks its own 20-30 ms buffer, which dominates
/// the output latency. If the backend rejects the fixed size the stream is
/// rebuilt on the device default buffer and one Chinese warning is reported
/// through `warn`. Returns the stream together with the output period estimate
/// used for [`AudioEngine::output_latency_millis`].
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    ring: Arc<AudioRing>,
    underruns: Arc<AtomicU64>,
    format: SampleFormat,
    warn: &WarnSink,
) -> Result<(cpal::Stream, f64)> {
    let fixed_frames = fixed_buffer_frames(config.sample_rate);
    let mut fixed_config = config.clone();
    fixed_config.buffer_size = cpal::BufferSize::Fixed(fixed_frames);
    match build_stream_with(
        device,
        &fixed_config,
        Arc::clone(&ring),
        Arc::clone(&underruns),
        format,
        Arc::clone(warn),
    ) {
        Ok(stream) => return Ok((stream, OUTPUT_PERIOD_MS)),
        Err(error) => {
            warn(&format!(
                "音频输出不支持固定 {OUTPUT_PERIOD_MS:.0}ms 小缓冲（{fixed_frames} 帧），\
                 已回退到设备默认缓冲：{error}"
            ));
        }
    }
    let stream = build_stream_with(device, config, ring, underruns, format, Arc::clone(warn))
        .context("无法创建音频输出流")?;
    Ok((stream, FALLBACK_OUTPUT_PERIOD_MS))
}

/// One stream-build attempt (the state and error callback are recreated per
/// attempt so a rejected fixed buffer can be retried with the default buffer).
fn build_stream_with(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    ring: Arc<AudioRing>,
    underruns: Arc<AtomicU64>,
    format: SampleFormat,
    warn: WarnSink,
) -> Result<cpal::Stream> {
    let state = CallbackState::new(ring, underruns, config.sample_rate, config.channels);
    let error_callback = move |error: cpal::StreamError| {
        warn(&format!("音频输出流错误：{error}"));
    };
    match format {
        SampleFormat::F32 => build_typed::<f32>(device, config, state, error_callback),
        SampleFormat::F64 => build_typed::<f64>(device, config, state, error_callback),
        SampleFormat::I8 => build_typed::<i8>(device, config, state, error_callback),
        SampleFormat::I16 => build_typed::<i16>(device, config, state, error_callback),
        SampleFormat::I24 => build_typed::<cpal::I24>(device, config, state, error_callback),
        SampleFormat::I32 => build_typed::<i32>(device, config, state, error_callback),
        SampleFormat::I64 => build_typed::<i64>(device, config, state, error_callback),
        SampleFormat::U8 => build_typed::<u8>(device, config, state, error_callback),
        SampleFormat::U16 => build_typed::<u16>(device, config, state, error_callback),
        SampleFormat::U24 => build_typed::<cpal::U24>(device, config, state, error_callback),
        SampleFormat::U32 => build_typed::<u32>(device, config, state, error_callback),
        SampleFormat::U64 => build_typed::<u64>(device, config, state, error_callback),
        other => Err(anyhow!("不支持的音频采样格式：{other}")),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: CallbackState,
    error_callback: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| state.render(data),
            error_callback,
            None,
        )
        .context("无法创建音频输出流")
}

// ---------------------------------------------------------------------------
// Tests (no sound card: null sink / injected device opener)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn capture_engine() -> Arc<AudioEngine> {
        AudioEngine::start_capture(Arc::new(|_| {}))
    }

    fn sine(frames: usize, channels: usize, frequency: f32, rate: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for frame in 0..frames {
            let value =
                (2.0 * std::f32::consts::PI * frequency * frame as f32 / rate).sin() * 0.5;
            for _ in 0..channels {
                out.push(value);
            }
        }
        out
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn inactive_source_pcm_is_dropped() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::AirPlay));
        engine.write_dlna_pcm(1, 1.0, TARGET_RATE, 2, &[0.5; 480]);
        engine.write_internal(Source::MiPlay, 0, 1.0, TARGET_RATE, 2, &[0.5; 480]);
        assert!(engine.captured().is_empty());
        assert_eq!(engine.null_frames(), 0);

        engine.write_internal(Source::AirPlay, 0, 1.0, TARGET_RATE, 2, &[0.5; 480]);
        assert_eq!(engine.captured().len(), 480);
        assert_eq!(engine.active_source(), Some(Source::AirPlay));
    }

    #[test]
    fn sender_gain_is_applied_once_on_write() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::AirPlay));
        engine.write_internal(Source::AirPlay, 0, 0.25, TARGET_RATE, 2, &[0.8; 96]);
        let captured = engine.captured();
        assert_eq!(captured.len(), 96);
        assert!(captured.iter().all(|sample| (*sample - 0.2).abs() < 1e-6));
    }

    #[test]
    fn mono_is_upmixed_to_stereo() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::MiPlay));
        let mono = sine(480, 1, 1_000.0, TARGET_RATE as f32);
        engine.write_internal(Source::MiPlay, 0, 1.0, TARGET_RATE, 1, &mono);
        let captured = engine.captured();
        assert_eq!(captured.len(), 960);
        for frame in captured.chunks(2) {
            assert!((frame[0] - frame[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn non_48k_pcm_is_resampled_to_the_master_rate() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::MiPlay));
        let frames = 4_410usize;
        let input = sine(frames, 2, 1_000.0, 44_100.0);
        engine.write_internal(Source::MiPlay, 0, 1.0, 44_100, 2, &input);
        let captured = engine.captured();
        let output_frames = captured.len() / 2;
        // 4410 input frames at 44.1 kHz are 4800 output frames at 48 kHz; the
        // windowed sinc only loses its short start-up latency.
        assert!(
            (4_600..=4_810).contains(&output_frames),
            "got {output_frames} output frames"
        );
        // The 1 kHz tone must survive the conversion.
        assert!(rms(&captured) > 0.2, "rms={}", rms(&captured));
    }

    #[test]
    fn flush_resets_queued_and_converter_state() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::Dlna));
        engine.write_dlna_pcm(1, 1.0, 44_100, 2, &[0.5; 4_410]);
        engine.clear_captured();
        engine.flush_source(Source::Dlna);
        // After a flush the resampler starts from scratch: the first block
        // yields less than the steady-state frame count (start-up latency).
        engine.write_dlna_pcm(2, 1.0, 44_100, 2, &[0.5; 441]);
        let frames = engine.captured().len() / 2;
        assert!(frames < 520, "flush did not reset the resampler: {frames}");
    }

    #[test]
    fn stop_drops_further_pcm() {
        let engine = capture_engine();
        engine.set_active_source(Some(Source::Dlna));
        engine.write_dlna_pcm(1, 1.0, TARGET_RATE, 2, &[0.25; 480]);
        engine.clear_captured();
        engine.stop();
        engine.write_dlna_pcm(2, 1.0, TARGET_RATE, 2, &[0.25; 480]);
        assert!(engine.captured().is_empty());
        assert_eq!(engine.status().underruns, 0);
    }

    #[test]
    fn null_mode_reports_the_master_format() {
        let engine = capture_engine();
        let status = engine.status();
        assert_eq!(status.mode, "null");
        assert_eq!(status.sample_rate, TARGET_RATE);
        assert_eq!(status.channels, TARGET_CHANNELS);
        assert_eq!(status.underruns, 0);
    }

    #[test]
    fn explicit_null_audio_does_not_warn() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&warnings);
        let warn: WarnSink = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let engine = AudioEngine::start(
            EngineConfig {
                null_audio: true,
                output_device: None,
            },
            warn,
        )
        .expect("null engine must start");
        assert_eq!(engine.status().mode, "null");
        assert_eq!(warnings.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn device_open_failure_falls_back_to_null_with_one_warning() {
        let messages = Arc::new(Mutex::new(Vec::<String>::new()));
        let collected = Arc::clone(&messages);
        let warn: WarnSink = Arc::new(move |message: &str| {
            if let Ok(mut guard) = collected.lock() {
                guard.push(message.to_string());
            }
        });
        let engine = AudioEngine::start_with(
            EngineConfig {
                null_audio: false,
                output_device: Some("does-not-exist".to_string()),
            },
            warn,
            |_config, _warn| Err(anyhow!("没有可用的音频设备")),
        )
        .expect("fallback engine must start");
        assert_eq!(engine.status().mode, "null");
        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("静默") && messages[0].contains("接收器"),
            "unexpected warning: {}",
            messages[0]
        );
    }

    #[test]
    fn ring_drops_oldest_frames_on_overflow() {
        // 2-frame capacity: pushing 3 frames must drop the oldest frame.
        let ring = AudioRing::new(2, 2);
        ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(ring.fill_frames(), 2);
        let mut destination = [0.0f32; 6];
        let popped = ring.try_pop_into(&mut destination).unwrap();
        assert_eq!(popped, 4);
        assert_eq!(destination, [3.0, 4.0, 5.0, 6.0, 0.0, 0.0]);

        ring.push(&[9.0; 100]);
        assert_eq!(ring.fill_frames(), 2);
        ring.clear();
        assert_eq!(ring.fill_frames(), 0);
    }

    #[test]
    fn write_frame_maps_stereo_into_device_layouts() {
        let mut mono = [0.0f32; 4];
        write_frame(&mut mono, 0, 1, 0.5, 0.1);
        assert!((mono[0] - 0.3).abs() < 1e-6);

        let mut stereo = [0.0f32; 4];
        write_frame(&mut stereo, 1, 2, 0.25, -0.25);
        assert_eq!(&stereo[2..], &[0.25, -0.25]);

        let mut surround = [1.0f32; 6];
        write_frame(&mut surround, 0, 6, 0.5, 0.75);
        assert!((surround[0] - 0.5).abs() < 1e-6);
        assert!((surround[1] - 0.75).abs() < 1e-6);
        assert_eq!(&surround[2..], &[0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn fixed_buffer_frames_requests_ten_millis_and_clamps() {
        // Nominal 10 ms at the common device rates.
        assert_eq!(fixed_buffer_frames(TARGET_RATE), 480);
        assert_eq!(fixed_buffer_frames(44_100), 441);
        assert_eq!(fixed_buffer_frames(96_000), 960);
        // Below 6.4 kHz the frame floor wins, so the period is longer than
        // 10 ms; above 480 kHz the ceiling caps the request.
        assert_eq!(fixed_buffer_frames(8_000), 80);
        assert_eq!(fixed_buffer_frames(4_000), MIN_FIXED_BUFFER_FRAMES);
        assert_eq!(fixed_buffer_frames(200_000), 2_000);
        assert_eq!(fixed_buffer_frames(960_000), MAX_FIXED_BUFFER_FRAMES);
    }

    #[test]
    fn null_engine_reports_zero_output_latency() {
        let engine = capture_engine();
        assert_eq!(engine.output_latency_millis(), 0.0);
        assert_eq!(
            fusionplay_miplay_sdk::MiPlayPcmOutput::output_latency_millis(engine.as_ref()),
            Some(0.0)
        );
    }

    #[test]
    fn ring_fill_millis_reports_queued_audio() {
        let ring = AudioRing::new(
            TARGET_CHANNELS as usize,
            TARGET_RATE as usize * RING_CAPACITY_MS / 1_000,
        );
        assert_eq!(ring.fill_millis(), 0.0);
        // 480 stereo frames = 10 ms of master audio.
        ring.push(&[0.5; 480 * TARGET_CHANNELS as usize]);
        assert!((ring.fill_millis() - 10.0).abs() < 1e-6);
        ring.clear();
        assert_eq!(ring.fill_millis(), 0.0);
    }

    /// Engine with a real [`AudioRing`] but no device stream, mirroring the
    /// state [`AudioEngine::with_device`] builds (unit tests cannot open a
    /// sound card). `output_period_millis` is injected explicitly.
    fn ring_engine(ring: Arc<AudioRing>, output_period_millis: f64) -> AudioEngine {
        AudioEngine {
            sink: Arc::new(DeviceSink {
                ring: Arc::clone(&ring),
            }),
            ring: Some(ring),
            null_sink: None,
            ingest: new_ingest_array(),
            active: AtomicU8::new(encode_source(Source::MiPlay)),
            stopped: AtomicBool::new(false),
            underruns: Arc::new(AtomicU64::new(0)),
            info: RwLock::new(StatusInfo {
                mode: "cpal".to_string(),
                device_name: None,
                sample_rate: TARGET_RATE,
                channels: TARGET_CHANNELS,
            }),
            stream: Mutex::new(None),
            output_period_millis,
        }
    }

    #[test]
    fn device_engine_output_latency_tracks_ring_fill() {
        let ring = Arc::new(AudioRing::new(
            TARGET_CHANNELS as usize,
            TARGET_RATE as usize * RING_CAPACITY_MS / 1_000,
        ));
        let engine = ring_engine(Arc::clone(&ring), OUTPUT_PERIOD_MS);

        // Empty ring: period + constant margin only.
        let idle = engine.output_latency_millis();
        assert!((idle - (OUTPUT_PERIOD_MS + OUTPUT_LATENCY_CONSTANT_MS)).abs() < 1e-6);

        // 20 ms of 48 kHz stereo PCM lands in the ring unchanged, so the
        // reported latency must grow by exactly that much.
        engine.write_internal(Source::MiPlay, 0, 1.0, TARGET_RATE, 2, &[0.25; 960 * 2]);
        assert!(ring.fill_millis() > 0.0);
        let expected = 20.0 + OUTPUT_PERIOD_MS + OUTPUT_LATENCY_CONSTANT_MS;
        let reported = engine.output_latency_millis();
        assert!(
            (reported - expected).abs() < 1e-6,
            "expected {expected} ms, got {reported} ms"
        );
        // The MiPlay sink contract exposes the same estimate.
        assert_eq!(
            fusionplay_miplay_sdk::MiPlayPcmOutput::output_latency_millis(&engine),
            Some(reported)
        );
    }
}
