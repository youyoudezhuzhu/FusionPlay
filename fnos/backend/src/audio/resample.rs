//! Resampling helpers and the per-device clock-drift loop.
//!
//! Every output device runs on its own hardware clock. The engine keeps a
//! bounded ring buffer per device and measures its fill level (in frames); the
//! [`DriftEstimator`] turns that measurement into a slow rate correction in
//! **ppm** that the resampler applies, so a fast/slow DAC does not drain/fill
//! the ring forever.
//!
//! On top of that continuous loop a 4-tier supervisor
//! ([`DriftTierConfig`] / [`DriftAction`]) handles abnormal excursions: a dead
//! zone where only the fine loop acts, a gentle and an aggressive rate clamp,
//! and a frame-aligned rebuild when the ring is too far from target. The
//! band edges are sized from the fnOS ring geometry (120 ms target inside a
//! 300 ms ring), not copied from the multi-second-buffer reference model.
//!
//! Two resamplers live here:
//! - [`SincResampler`]: windowed-sinc streaming resampler used on the device
//!   output path. Arbitrary ratio (device rate / master rate, plus drift) with
//!   an anti-aliasing low-pass derived from the ratio. No fixed chunk size, so
//!   it fits the "pop a batch, write a batch" worker loop.
//! - [`LinearResampler`]: small linear interpolator retained for the DLNA
//!   decode path (`dlna_player.rs` resamples decoded streams to 48 kHz before
//!   handing them to the engine). It is not used for device output any more.

use std::f32::consts::PI;

/// Maximum correction applied to a device (ppm). Keep this small: audible
/// pitch changes are unacceptable; 1000 ppm is ~0.1 % and already generous.
pub const MAX_DRIFT_PPM: f32 = 1000.0;

/// Proportional gain of the fill-level -> ppm loop, in ppm per frame of error.
/// The loop time constant is `~976 / gain` worker batches (a batch is ~21 ms
/// at 48 kHz), so K = 6 gives ~3.5 s. Larger = faster but noisier; the fill
/// measurement is quantised by the source chunk size, so the gain cannot be
/// pushed much higher without the output saturating on measurement noise.
pub const DRIFT_PPM_PER_FRAME: f32 = 8.0;

/// Low-pass coefficient applied to the raw fill measurement before the
/// proportional term. The fill level only changes by ~1 frame per 1000 ppm per
/// worker batch, so the raw signal is dominated by source-chunk quantisation;
/// ~0.5 s of averaging is required to recover the drift.
pub const DRIFT_FILL_SMOOTHING: f32 = 0.04;

/// Low-pass coefficient applied to the ppm output. This is a control loop, not
/// an audio signal: smoothing removes the residual quantisation jitter before
/// it reaches the resampler ratio.
pub const DRIFT_PPM_SMOOTHING: f32 = 0.12;

/// Number of worker batches skipped after a device opens before the reference
/// fill is measured. This lets the startup/prefill transient die out.
pub const DRIFT_CALIBRATION_WARMUP: usize = 48;

/// Number of worker batches averaged to learn the *reference* ring fill. The
/// reference is the measured mean fill, so the constant sampling bias of a
/// quantised producer (a source chunk is 256-1024 frames) cancels out instead
/// of being mistaken for clock drift. At 48 kHz a worker batch is ~21 ms, so
/// warm-up + window is ~2.4 s.
pub const DRIFT_CALIBRATION_UPDATES: usize = 64;

/// Ring fill -> clock drift estimator.
///
/// Sign convention matches [`crate::audio::DeviceStatus::drift_ppm`]:
/// **positive ppm means the device runs fast** (it consumes more frames per
/// second than nominal), so the engine must produce slightly more output
/// frames per input frame. A negative value means the device is slow.
pub struct DriftEstimator {
    target_frames: f32,
    ppm: f32,
    max_ppm: f32,
    fill_smoothing: f32,
    ppm_smoothing: f32,
    fill_avg: f32,
    primed: bool,
    /// Calibration state: skip a warm-up and then average the fill to learn the
    /// reference level; no drift correction happens until it is learned.
    calibration_warmup: usize,
    calibration_updates: usize,
    warmup_done: usize,
    calibration_done: usize,
    calibration_sum: f32,
    calibrating: bool,
}

impl DriftEstimator {
    /// `smoothing` is the fill low-pass coefficient; values around 0.1 are a
    /// good compromise between convergence speed and measurement noise. Values
    /// outside `0..=1` are clamped.
    pub fn new(target_frames: usize, smoothing: f32, max_ppm: f32) -> Self {
        Self {
            target_frames: target_frames.max(1) as f32,
            ppm: 0.0,
            max_ppm: max_ppm.clamp(1.0, MAX_DRIFT_PPM),
            fill_smoothing: smoothing.clamp(0.002, 1.0),
            ppm_smoothing: DRIFT_PPM_SMOOTHING,
            fill_avg: target_frames.max(1) as f32,
            primed: false,
            calibration_warmup: 0,
            calibration_updates: 0,
            warmup_done: 0,
            calibration_done: 0,
            calibration_sum: 0.0,
            calibrating: false,
        }
    }

    /// Same as [`DriftEstimator::new`], but skip `warmup` measurements, then
    /// average `window` measurements to learn the reference fill, and only
    /// then start correcting. See [`DRIFT_CALIBRATION_WARMUP`] and
    /// [`DRIFT_CALIBRATION_UPDATES`].
    pub fn with_calibration(
        target_frames: usize,
        smoothing: f32,
        max_ppm: f32,
        warmup: usize,
        window: usize,
    ) -> Self {
        let mut estimator = Self::new(target_frames, smoothing, max_ppm);
        estimator.calibration_warmup = warmup;
        estimator.calibration_updates = window;
        estimator.calibrating = window > 0;
        estimator
    }

    pub fn ppm(&self) -> f32 {
        self.ppm
    }

    pub fn target_frames(&self) -> f32 {
        self.target_frames
    }

    pub fn max_ppm(&self) -> f32 {
        self.max_ppm
    }

    /// Forget the measured state (used after a flush / device reopen). The
    /// reference fill is learned again, because a reconnect re-prefills the
    /// ring and the producer chunk phase may have changed.
    pub fn reset(&mut self) {
        self.ppm = 0.0;
        self.fill_avg = self.target_frames;
        self.primed = false;
        self.warmup_done = 0;
        self.calibration_done = 0;
        self.calibration_sum = 0.0;
        self.calibrating = self.calibration_updates > 0;
    }

    /// Feed the measured ring fill level (frames) and return the new correction.
    pub fn update(&mut self, fill_frames: f32) -> f32 {
        if !fill_frames.is_finite() {
            return self.ppm;
        }
        if !self.primed {
            // First measurement after (re)start: initialise the filter instead
            // of treating the prefill transient as a clock error.
            self.fill_avg = fill_frames;
            self.primed = true;
        } else {
            self.fill_avg += (fill_frames - self.fill_avg) * self.fill_smoothing;
        }
        if self.calibrating {
            if self.warmup_done < self.calibration_warmup {
                self.warmup_done += 1;
                return self.ppm;
            }
            self.calibration_done += 1;
            self.calibration_sum += self.fill_avg;
            if self.calibration_done >= self.calibration_updates {
                self.target_frames = (self.calibration_sum / self.calibration_done as f32).max(1.0);
                self.ppm = 0.0;
                self.calibrating = false;
            }
            return self.ppm;
        }
        // Positive when the ring is below target -> device is consuming faster
        // than the master produces -> produce more frames (positive ppm).
        let error = self.target_frames - self.fill_avg;
        let target_ppm = (error * DRIFT_PPM_PER_FRAME).clamp(-self.max_ppm, self.max_ppm);
        self.ppm += (target_ppm - self.ppm) * self.ppm_smoothing;
        self.ppm = self.ppm.clamp(-self.max_ppm, self.max_ppm);
        self.ppm
    }

    /// Output/input frame ratio for the device resampler at unity nominal rate.
    pub fn ratio(&self) -> f64 {
        1.0 + (self.ppm as f64) / 1_000_000.0
    }

    /// True while the loop is still learning its reference fill (warm-up or
    /// calibration window). The target is nominal until calibration finishes,
    /// so tier decisions are meaningless and must not be taken.
    pub fn calibrating(&self) -> bool {
        self.calibrating
    }

    /// Smoothed signed ring-fill error in frames (`target - fill`).
    ///
    /// Positive means the ring is *below* target, i.e. the device consumes
    /// faster than nominal, which matches the positive-ppm sign convention.
    pub fn error_frames(&self) -> f32 {
        self.target_frames - self.fill_avg
    }

    /// [`DriftEstimator::error_frames`] expressed in milliseconds at `rate`.
    pub fn error_ms(&self, rate: u32) -> f32 {
        self.error_frames() * 1_000.0 / rate.max(1) as f32
    }
}

// ---------------------------------------------------------------------------
// 4-tier drift supervisor (esp-sonic-sync model, sized for the fnOS ring)
// ---------------------------------------------------------------------------

// The reference model (esp-sonic-sync, reverse-engineered MiPlay) uses a
// 4-level drift state machine with 100 / 300 / 600 ms boundaries. Those
// numbers assume a multi-second jitter buffer. The fnOS engine keeps a
// `RING_TARGET_MS = 120` ms jitter target inside a `RING_EXTRA_MS = 300` ms
// ring (`audio::engine`), so the bands are scaled to that geometry instead of
// copied: an error can physically never exceed `300 - 120 = 180` ms above the
// target (overflow drops the oldest frames) or `120` ms below it (underrun).
//
// - `DRIFT_HOLD_MS`: 5 ms is roughly one 240-frame source chunk at 48 kHz and
//   sits well inside the fine proportional loop's noise band. Below it no
//   coarse tier correction is applied; the continuous P-loop keeps running
//   (deleting the P-loop for "small" errors would leave a ±500 ppm DAC
//   uncorrected for minutes on a 120 ms ring).
// - `DRIFT_GENTLE_MS`: 40 ms is one third of the jitter target. A moderate
//   excursion is corrected at most at `DRIFT_GENTLE_PPM` (0.03 % speed) so a
//   local glitch cannot audibly bend the pitch.
// - `DRIFT_AGGRESSIVE_MS`: 100 ms is roughly the 120 ms jitter target itself.
//   It is smaller than both the 120 ms below-target slack and the 180 ms
//   above-target headroom, so the rebuild tier is reachable in *both*
//   directions: a fast device that has drained the ring to < 20 ms, and a
//   stalled one that has filled it past 220 ms. Inside this band the full
//   `MAX_DRIFT_PPM` (0.1 %, the existing hard clamp) pulls the ring back.
// - Above `DRIFT_AGGRESSIVE_MS`: recover by dropping / re-prefilling whole
//   frames back to the calibrated target, then re-calibrate the estimator.

/// |error| below this is handled by the fine P-loop only (no tier action).
pub const DRIFT_HOLD_MS: f32 = 5.0;
/// Upper bound of the gentle band.
pub const DRIFT_GENTLE_MS: f32 = 40.0;
/// Upper bound of the aggressive band; above it the ring is rebuilt.
pub const DRIFT_AGGRESSIVE_MS: f32 = 100.0;
/// Rate clamp used inside the gentle band (0.03 %, inaudible pitch change).
pub const DRIFT_GENTLE_PPM: f32 = 300.0;
/// Rate clamp used inside the aggressive band: the existing hard clamp.
pub const DRIFT_AGGRESSIVE_PPM: f32 = MAX_DRIFT_PPM;

/// Which drift countermeasure the supervisor selects for the current error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DriftAction {
    /// Dead zone: no coarse correction, the fine proportional loop keeps the
    /// ring on target exactly as before this tier layer existed.
    Hold,
    /// Moderate excursion: clamp the correction to `max_ppm` (gentle rate).
    Gentle { max_ppm: f32 },
    /// Large excursion: clamp the correction to `max_ppm` (aggressive rate).
    Aggressive { max_ppm: f32 },
    /// Huge excursion: drop or re-prefill whole frames back to target.
    /// Positive = ring below target (pad with silence), negative = ring above
    /// target (discard the oldest frames); the value is always whole frames.
    Recover { realign_frames: i64 },
}

/// Thresholds and rate clamps of the 4-tier drift supervisor.
///
/// Defaults are enabled; every field can be overridden from the environment
/// (`FUSIONPLAY_FNOS_DRIFT_TIERS=0` disables the tier layer and keeps the
/// legacy continuous P-loop, `FUSIONPLAY_FNOS_DRIFT_{HOLD,GENTLE,AGGRESSIVE}_MS`
/// and `FUSIONPLAY_FNOS_DRIFT_{GENTLE,AGGRESSIVE}_PPM` tune the bands).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftTierConfig {
    pub enabled: bool,
    pub hold_ms: f32,
    pub gentle_ms: f32,
    pub aggressive_ms: f32,
    pub gentle_ppm: f32,
    pub aggressive_ppm: f32,
}

impl Default for DriftTierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hold_ms: DRIFT_HOLD_MS,
            gentle_ms: DRIFT_GENTLE_MS,
            aggressive_ms: DRIFT_AGGRESSIVE_MS,
            gentle_ppm: DRIFT_GENTLE_PPM,
            aggressive_ppm: DRIFT_AGGRESSIVE_PPM,
        }
    }
}

impl DriftTierConfig {
    /// Read the configuration from the environment, falling back to the
    /// defaults for every unset or unparsable variable. Values are sanitised
    /// (finite, monotonic bands, ppm inside the existing clamp).
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(raw) = std::env::var("FUSIONPLAY_FNOS_DRIFT_TIERS") {
            let value = raw.trim().to_ascii_lowercase();
            config.enabled =
                !matches!(value.as_str(), "0" | "off" | "false" | "no" | "disabled");
        }
        config.hold_ms = env_f32("FUSIONPLAY_FNOS_DRIFT_HOLD_MS", config.hold_ms);
        config.gentle_ms = env_f32("FUSIONPLAY_FNOS_DRIFT_GENTLE_MS", config.gentle_ms);
        config.aggressive_ms = env_f32("FUSIONPLAY_FNOS_DRIFT_AGGRESSIVE_MS", config.aggressive_ms);
        config.gentle_ppm = env_f32("FUSIONPLAY_FNOS_DRIFT_GENTLE_PPM", config.gentle_ppm);
        config.aggressive_ppm =
            env_f32("FUSIONPLAY_FNOS_DRIFT_AGGRESSIVE_PPM", config.aggressive_ppm);
        config.sanitized()
    }

    /// Force the configuration into a usable shape: finite non-negative band
    /// edges with `hold < gentle < aggressive`, and rate clamps inside
    /// `1..=MAX_DRIFT_PPM` with `gentle_ppm <= aggressive_ppm`. Invalid bands
    /// fall back to the defaults rather than silently inverting the tiers.
    pub fn sanitized(self) -> Self {
        let mut config = self;
        config.hold_ms = finite_non_negative(config.hold_ms, DRIFT_HOLD_MS);
        config.gentle_ms = finite_non_negative(config.gentle_ms, DRIFT_GENTLE_MS);
        config.aggressive_ms = finite_non_negative(config.aggressive_ms, DRIFT_AGGRESSIVE_MS);
        if !(config.hold_ms < config.gentle_ms && config.gentle_ms < config.aggressive_ms) {
            config.hold_ms = DRIFT_HOLD_MS;
            config.gentle_ms = DRIFT_GENTLE_MS;
            config.aggressive_ms = DRIFT_AGGRESSIVE_MS;
        }
        config.gentle_ppm = finite_non_negative(config.gentle_ppm, DRIFT_GENTLE_PPM)
            .clamp(1.0, MAX_DRIFT_PPM);
        config.aggressive_ppm = finite_non_negative(config.aggressive_ppm, DRIFT_AGGRESSIVE_PPM)
            .clamp(config.gentle_ppm, MAX_DRIFT_PPM);
        config
    }

    /// Select the countermeasure for a signed frame error (`target - fill`).
    ///
    /// The error is converted to milliseconds at the device rate, so the bands
    /// are rate-independent; `realign_frames` is the original whole-frame
    /// error, keeping the recovery path frame-aligned.
    pub fn decide(&self, error_frames: i64, rate: u32) -> DriftAction {
        if !self.enabled || error_frames == 0 {
            return DriftAction::Hold;
        }
        let error_ms = error_frames as f32 * 1_000.0 / rate.max(1) as f32;
        let magnitude = error_ms.abs();
        if magnitude < self.hold_ms {
            DriftAction::Hold
        } else if magnitude <= self.gentle_ms {
            DriftAction::Gentle {
                max_ppm: self.gentle_ppm,
            }
        } else if magnitude <= self.aggressive_ms {
            DriftAction::Aggressive {
                max_ppm: self.aggressive_ppm,
            }
        } else {
            DriftAction::Recover {
                realign_frames: error_frames,
            }
        }
    }
}

fn env_f32(name: &str, fallback: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn finite_non_negative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

const DEFAULT_TAPS: usize = 48;
const DEFAULT_PHASES: usize = 256;

/// Streaming windowed-sinc resampler with arbitrary, slowly varying ratio.
///
/// The kernel is precomputed for `phases` fractional positions and `taps`
/// filter taps; it is rebuilt only when the anti-alias cutoff (driven by the
/// ratio) changes by more than 1 %, i.e. essentially only on rate changes.
/// Drift corrections of a few hundred ppm reuse the cached kernel and only
/// change the input step.
pub struct SincResampler {
    channels: usize,
    taps: usize,
    phases: usize,
    /// `phases * taps` coefficients, row-major, each row normalised to sum 1.
    table: Vec<f32>,
    cutoff: f32,
    /// Current output position in input frames, relative to `pending[0]`.
    pos: f64,
    pending: Vec<f32>,
    // Bound pathological callers that stop consuming but keep feeding.
    max_pending_frames: usize,
}

impl SincResampler {
    pub fn new(channels: usize) -> Self {
        Self::with_quality(channels, DEFAULT_TAPS, DEFAULT_PHASES)
    }

    /// `taps` must be even and >= 4; `phases` should be >= 32.
    pub fn with_quality(channels: usize, taps: usize, phases: usize) -> Self {
        let channels = channels.max(1);
        let taps = if taps < 4 { 4 } else { taps & !1 };
        let phases = phases.max(8);
        let mut resampler = Self {
            channels,
            taps,
            phases,
            table: Vec::new(),
            cutoff: 0.0,
            pos: (taps / 2 - 1) as f64,
            pending: Vec::new(),
            max_pending_frames: (taps * 128).max(4096),
        };
        resampler.rebuild_kernel(1.0);
        resampler
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pos = (self.taps / 2 - 1) as f64;
    }

    /// Latency introduced by the look-ahead, in input frames (diagnostics/tests).
    pub fn latency_frames(&self) -> usize {
        self.taps - 1
    }

    fn rebuild_kernel(&mut self, cutoff: f32) {
        let cutoff = cutoff.clamp(0.05, 1.0);
        let taps = self.taps;
        let phases = self.phases;
        let half = (taps / 2) as f32;
        let mut table = vec![0.0f32; phases * taps];
        for phase in 0..phases {
            let frac = phase as f32 / phases as f32;
            let mut sum = 0.0f32;
            for k in 0..taps {
                // Distance (in input samples) between the interpolation point
                // and tap sample k. Even in sign for sinc + window.
                let t = frac - k as f32 + (half - 1.0);
                let x = cutoff * t;
                let sinc = if x.abs() < 1e-7 {
                    1.0
                } else {
                    (PI * x).sin() / (PI * x)
                };
                let window = if t.abs() >= half {
                    0.0
                } else {
                    // Blackman window over [-half, half].
                    let u = (t + half) / (2.0 * half);
                    0.42 - 0.5 * (2.0 * PI * u).cos() + 0.08 * (4.0 * PI * u).cos()
                };
                let value = cutoff * sinc * window;
                table[phase * taps + k] = value;
                sum += value;
            }
            if sum.abs() > 1e-9 {
                for k in 0..taps {
                    table[phase * taps + k] /= sum;
                }
            }
        }
        self.table = table;
        self.cutoff = cutoff;
    }

    /// Resample interleaved `input` and append interleaved output to `out`.
    ///
    /// `ratio` is output frames per input frame (device rate / master rate,
    /// multiplied by the drift correction).
    pub fn process(&mut self, input: &[f32], ratio: f64, out: &mut Vec<f32>) {
        let channels = self.channels;
        if !input.is_empty() {
            self.pending.extend_from_slice(input);
        }
        // Keep pathological growth bounded even if the caller stops draining.
        let max_samples = self.max_pending_frames * channels;
        if self.pending.len() > max_samples {
            let excess_frames = self.pending.len() / channels - self.max_pending_frames;
            let drop = excess_frames * channels;
            self.pending.drain(..drop);
            self.pos -= drop as f64 / channels as f64;
        }
        let ratio = if ratio.is_finite() { ratio.clamp(0.25, 4.0) } else { 1.0 };
        // Downsampling needs a low-pass at the new Nyquist; upsampling can keep
        // the full input band. 0.95 leaves a small transition band.
        let desired_cutoff = if ratio < 0.98 { (ratio * 0.92) as f32 } else { 1.0 };
        if (desired_cutoff - self.cutoff).abs() > 0.01 {
            self.rebuild_kernel(desired_cutoff);
        }

        let frames = self.pending.len() / channels;
        let taps = self.taps;
        let half = taps / 2;
        if frames < taps {
            return;
        }
        let step = 1.0 / ratio;
        loop {
            let base = self.pos.floor() as usize;
            // Need taps input samples centered on `base`: indices base+1-half ..
            // base+half. Fractional position is `base + frac`.
            if base + half >= frames {
                break;
            }
            let frac = (self.pos - base as f64) as f32;
            let phase = ((frac * self.phases as f32) as usize).min(self.phases - 1);
            let kernel = &self.table[phase * taps..(phase + 1) * taps];
            let start = base + 1 - half;
            for channel in 0..channels {
                let mut acc = 0.0f32;
                for k in 0..taps {
                    acc += self.pending[(start + k) * channels + channel] * kernel[k];
                }
                out.push(acc);
            }
            self.pos += step;
        }
        // Drop input frames that no future output can reference any more.
        let keep_from = (self.pos.floor() as usize).saturating_sub(half - 1);
        if keep_from > 0 {
            let drop_samples = keep_from.min(frames) * channels;
            self.pending.drain(..drop_samples);
            self.pos -= keep_from as f64;
        }
    }
}

/// Minimal streaming linear resampler used by the DLNA decode path. It keeps
/// the last frame for continuity; quality is intentionally modest because the
/// decoded stream is converted once to the master rate.
pub struct LinearResampler {
    channels: usize,
    pos: f64,
    pending: Vec<f32>,
}

impl LinearResampler {
    pub fn new(channels: usize) -> Self {
        Self { channels: channels.max(1), pos: 0.0, pending: Vec::new() }
    }

    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.pending.clear();
    }

    /// Push interleaved input; returns interleaved output at `ratio` speed
    /// (output frames per input frame).
    pub fn process(&mut self, input: &[f32], ratio: f64) -> Vec<f32> {
        let channels = self.channels;
        self.pending.extend_from_slice(input);
        let frames = self.pending.len() / channels;
        if frames == 0 {
            return Vec::new();
        }
        let ratio = if ratio.is_finite() { ratio.clamp(0.5, 2.0) } else { 1.0 };
        let step = 1.0 / ratio;
        let out_frames = ((frames as f64) / step).ceil() as usize;
        let mut out = Vec::with_capacity(out_frames * channels);
        for i in 0..out_frames {
            let sample_pos = self.pos + i as f64 * step;
            let idx = sample_pos.floor() as usize;
            let frac = (sample_pos - idx as f64) as f32;
            if idx >= frames {
                self.pos = sample_pos - frames as f64;
                let consumed = frames * channels;
                self.pending.drain(..consumed);
                return out;
            }
            let next = (idx + 1).min(frames - 1);
            for ch in 0..channels {
                let a = self.pending[idx * channels + ch];
                let b = self.pending[next * channels + ch];
                out.push(a + (b - a) * frac);
            }
        }
        self.pos = 0.0;
        self.pending.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, frames: usize, channels: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let value = (2.0 * PI * freq * i as f32 / RATE as f32).sin() * 0.5;
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
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn drift_estimator_sign_convention_and_clamp() {
        // Ring below target -> device fast -> positive ppm.
        let mut fast = DriftEstimator::new(4_800, 0.2, 500.0);
        for _ in 0..2_000 {
            fast.update(2_000.0);
        }
        assert!(fast.ppm() > 0.0 && fast.ppm() <= 500.0);

        // Ring above target -> device slow -> negative ppm.
        let mut slow = DriftEstimator::new(4_800, 0.2, 500.0);
        for _ in 0..2_000 {
            slow.update(9_000.0);
        }
        assert!(slow.ppm() < 0.0 && slow.ppm() >= -500.0);

        // Hard limit is respected even for absurd errors.
        let mut saturated = DriftEstimator::new(4_800, 0.5, 100_000.0);
        for _ in 0..2_000 {
            saturated.update(0.0);
        }
        assert!((saturated.ppm() - MAX_DRIFT_PPM).abs() < 0.5);
        for _ in 0..2_000 {
            saturated.update(1_000_000.0);
        }
        assert!((saturated.ppm() + MAX_DRIFT_PPM).abs() < 0.5);
    }

    #[test]
    fn drift_estimator_reaches_steady_state_offset() {
        // Device 500 ppm fast: steady state ppm ~= 500, fill offset ~= 500/gain.
        let target = 5_760.0; // 120 ms @ 48k
        let mut estimator = DriftEstimator::new(target as usize, DRIFT_FILL_SMOOTHING, MAX_DRIFT_PPM);
        let mut fill = target;
        // Simulate the closed loop in discrete update steps (each step ~ a
        // worker batch of 21 ms): fill moves according to ppm - device drift.
        for _ in 0..2_000 {
            let ppm = estimator.update(fill);
            fill += (ppm - 500.0) * 0.001 / 1_000.0 * 0.021 * RATE as f32;
        }
        assert!(
            (estimator.ppm() - 500.0).abs() < 60.0,
            "ppm did not converge: {}",
            estimator.ppm()
        );
    }

    #[test]
    fn sinc_unity_ratio_preserves_length_and_dc() {
        let mut resampler = SincResampler::new(2);
        let frames = 4_800;
        let mut out = Vec::new();
        // Work in chunks like the engine does.
        let chunk = 480;
        let input = vec![0.25f32; chunk * 2];
        for _ in 0..(frames / chunk) {
            resampler.process(&input, 1.0, &mut out);
        }
        let out_frames = out.len() / 2;
        // Startup loses `latency_frames`; the steady state is 1:1.
        assert!(
            (out_frames as i64 - frames as i64).abs() <= resampler.latency_frames() as i64 + 2,
            "got {out_frames} frames for {frames}"
        );
        for sample in out.iter().skip(2 * 128) {
            assert!((sample - 0.25).abs() < 1e-4);
        }
    }

    #[test]
    fn sinc_downsample_attenuates_aliasing() {
        // 23 kHz tone sampled at 48 kHz cannot survive a 44.1 kHz output; a
        // windowed-sinc resampler must reject it instead of folding it into
        // the audible band.
        let channels = 2;
        let mut resampler = SincResampler::new(channels);
        let ratio = 44_100.0 / 48_000.0;
        let mut out = Vec::new();
        let chunk_frames = 1_200;
        let input = {
            let mut v = Vec::with_capacity(chunk_frames * channels);
            for i in 0..chunk_frames {
                let value = (2.0 * PI * 23_000.0 * i as f32 / RATE as f32).sin() * 0.5;
                v.push(value);
                v.push(value);
            }
            v
        };
        for _ in 0..8 {
            resampler.process(&input, ratio, &mut out);
        }
        // Ignore the filter start-up transient.
        let steady: Vec<f32> = out.iter().skip(2 * 60).copied().collect();
        let level = rms(&steady);
        assert!(level < 0.02, "aliasing not attenuated, rms={level}");
    }

    #[test]
    fn sinc_keeps_in_band_tone() {
        let mut resampler = SincResampler::new(2);
        let input = sine(1_000.0, 1_200, 2);
        let mut out = Vec::new();
        for _ in 0..8 {
            resampler.process(&input, 44_100.0 / 48_000.0, &mut out);
        }
        let steady: Vec<f32> = out.iter().skip(2 * 100).copied().collect();
        let level = rms(&steady);
        let expected = 0.5 / 2.0f32.sqrt();
        assert!(
            (level - expected).abs() < 0.05,
            "in-band tone level {level} vs {expected}"
        );
    }

    #[test]
    fn linear_resampler_keeps_remainder_between_calls() {
        let mut resampler = LinearResampler::new(1);
        let mut total = 0;
        for chunk in [vec![0.0f32; 100], vec![1.0f32; 100]] {
            total += resampler.process(&chunk, 1.0).len();
        }
        assert!(total >= 199 && total <= 201, "got {total}");
    }

    #[test]
    fn linear_resampler_ratio_controls_frame_count() {
        let mut resampler = LinearResampler::new(2);
        let input = vec![0.0f32; 96_000];
        let out = resampler.process(&input, 44_100.0 / 48_000.0);
        let expected = 44_100.0;
        assert!(
            (out.len() as f64 / 2.0 - expected).abs() < 3.0,
            "got {} frames",
            out.len() / 2
        );
    }

    // -----------------------------------------------------------------------
    // 4-tier drift supervisor
    // -----------------------------------------------------------------------

    /// Convert a millisecond error into the whole-frame error the worker feeds
    /// the supervisor.
    fn error_frames(ms: f32, rate: u32) -> i64 {
        (ms * rate as f32 / 1_000.0).round() as i64
    }

    #[test]
    fn drift_error_accessors_follow_the_ppm_sign_convention() {
        // 100 ms target at 48 kHz. smoothing = 1.0 keeps the steps exact.
        let mut estimator = DriftEstimator::new(4_800, 1.0, 500.0);
        assert!(!estimator.calibrating());
        // Ring below target -> positive error (device consumes fast).
        estimator.update(2_400.0);
        assert!((estimator.error_frames() - 2_400.0).abs() < 1.0);
        assert!((estimator.error_ms(RATE) - 50.0).abs() < 0.1);
        // Ring above target -> negative error (device consumes slow).
        estimator.update(7_200.0);
        assert!(estimator.error_frames() < 0.0);
        assert!(estimator.error_ms(RATE) < 0.0);
        // Ms conversion is rate independent.
        assert!((estimator.error_ms(24_000) - estimator.error_ms(RATE) * 2.0).abs() < 0.5);
        // The estimator reports calibration until the reference is learned.
        let calibrating = DriftEstimator::with_calibration(
            4_800,
            DRIFT_FILL_SMOOTHING,
            MAX_DRIFT_PPM,
            DRIFT_CALIBRATION_WARMUP,
            DRIFT_CALIBRATION_UPDATES,
        );
        assert!(calibrating.calibrating());
    }

    #[test]
    fn drift_tier_bands_select_the_four_actions() {
        let config = DriftTierConfig::default();
        // Dead zone on both signs, including just below the edge.
        assert_eq!(config.decide(0, RATE), DriftAction::Hold);
        assert_eq!(config.decide(error_frames(4.9, RATE), RATE), DriftAction::Hold);
        assert_eq!(config.decide(-error_frames(4.9, RATE), RATE), DriftAction::Hold);
        // Gentle band: edge inclusive on both signs.
        assert_eq!(
            config.decide(error_frames(5.0, RATE), RATE),
            DriftAction::Gentle { max_ppm: DRIFT_GENTLE_PPM }
        );
        assert_eq!(
            config.decide(-error_frames(DRIFT_GENTLE_MS, RATE), RATE),
            DriftAction::Gentle { max_ppm: DRIFT_GENTLE_PPM }
        );
        // Aggressive band: just past the gentle edge up to the aggressive edge.
        assert_eq!(
            config.decide(error_frames(DRIFT_GENTLE_MS + 1.0, RATE), RATE),
            DriftAction::Aggressive { max_ppm: DRIFT_AGGRESSIVE_PPM }
        );
        assert_eq!(
            config.decide(-error_frames(DRIFT_AGGRESSIVE_MS, RATE), RATE),
            DriftAction::Aggressive { max_ppm: DRIFT_AGGRESSIVE_PPM }
        );
        // Rebuild: just past the aggressive edge, sign preserved.
        let beyond = error_frames(DRIFT_AGGRESSIVE_MS + 1.0, RATE);
        assert_eq!(config.decide(beyond, RATE), DriftAction::Recover { realign_frames: beyond });
        assert_eq!(
            config.decide(-beyond, RATE),
            DriftAction::Recover { realign_frames: -beyond }
        );
    }

    #[test]
    fn drift_tier_recover_is_frame_aligned_at_every_device_rate() {
        let config = DriftTierConfig::default();
        // 190 ms is beyond the aggressive edge at every common device rate.
        let error_ms = DRIFT_AGGRESSIVE_MS + 40.0;
        for rate in [44_100u32, 48_000, 96_000, 192_000] {
            let frames = error_frames(error_ms, rate);
            match config.decide(frames, rate) {
                DriftAction::Recover { realign_frames } => {
                    // The recovery path moves the *exact whole-frame* error, not
                    // a rounded millisecond value, so PCM frames cannot be torn.
                    assert_eq!(realign_frames, frames, "rate {rate}");
                }
                other => panic!("rate {rate}: expected Recover, got {other:?}"),
            }
            match config.decide(-frames, rate) {
                DriftAction::Recover { realign_frames } => {
                    assert_eq!(realign_frames, -frames, "rate {rate}");
                }
                other => panic!("rate {rate}: expected Recover, got {other:?}"),
            }
        }
    }

    #[test]
    fn drift_tiers_disabled_keep_the_legacy_fine_loop() {
        let config = DriftTierConfig { enabled: false, ..DriftTierConfig::default() };
        for error_ms in [0.0f32, 4.0, 40.0, 149.0, 151.0, 5_000.0] {
            let frames = error_frames(error_ms, RATE);
            assert_eq!(config.decide(frames, RATE), DriftAction::Hold, "{error_ms} ms");
            assert_eq!(config.decide(-frames, RATE), DriftAction::Hold, "-{error_ms} ms");
        }
    }

    #[test]
    fn drift_tier_config_sanitizes_bad_values_without_panicking() {
        let broken = DriftTierConfig {
            enabled: true,
            hold_ms: f32::NAN,
            gentle_ms: -5.0,
            aggressive_ms: f32::INFINITY,
            gentle_ppm: -100.0,
            aggressive_ppm: 1.0e9,
        }
        .sanitized();
        assert!(broken.hold_ms.is_finite() && broken.hold_ms >= 0.0);
        assert!(broken.hold_ms < broken.gentle_ms && broken.gentle_ms < broken.aggressive_ms);
        assert!((1.0..=MAX_DRIFT_PPM).contains(&broken.gentle_ppm));
        assert!(broken.aggressive_ppm >= broken.gentle_ppm);
        assert!(broken.aggressive_ppm <= MAX_DRIFT_PPM);

        // Inverted band edges fall back to the documented defaults.
        let inverted = DriftTierConfig {
            hold_ms: 100.0,
            gentle_ms: 50.0,
            aggressive_ms: 10.0,
            ..DriftTierConfig::default()
        }
        .sanitized();
        assert_eq!(inverted.hold_ms, DRIFT_HOLD_MS);
        assert_eq!(inverted.gentle_ms, DRIFT_GENTLE_MS);
        assert_eq!(inverted.aggressive_ms, DRIFT_AGGRESSIVE_MS);

        // Absurd/extreme errors stay panic-free (rate 0 is clamped to 1 Hz).
        assert!(matches!(
            inverted.decide(i64::MAX, 0),
            DriftAction::Recover { realign_frames } if realign_frames == i64::MAX
        ));
        assert!(matches!(
            inverted.decide(i64::MIN, 1),
            DriftAction::Recover { realign_frames } if realign_frames == i64::MIN
        ));
    }
}
