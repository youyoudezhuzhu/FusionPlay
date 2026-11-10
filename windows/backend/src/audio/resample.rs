//! Resampling helpers and the output clock-drift loop (ported from fnOS).
//!
//! The Windows engine has a single default output device, but that device still
//! runs on its own hardware clock. The engine keeps a bounded ring buffer and
//! measures its fill level (in frames); the [`DriftEstimator`] turns that
//! measurement into a slow rate correction in **ppm** that the resampler
//! applies, so a fast/slow DAC does not drain/fill the ring forever.
//!
//! Two resamplers live here:
//! - [`SincResampler`]: windowed-sinc streaming resampler used on the device
//!   output path (inside the cpal callback). Arbitrary ratio (device rate /
//!   master rate, plus drift) with an anti-aliasing low-pass derived from the
//!   ratio. No fixed chunk size, so it fits the "pop a block, push a block"
//!   callback loop.
//! - [`LinearResampler`]: small linear interpolator retained for source
//!   ingestion (the engine converts non-48 kHz MiPlay/AirPlay PCM to the
//!   48 kHz master mix before it reaches the ring).

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
/// Sign convention: **positive ppm means the device runs fast** (it consumes
/// more frames per second than nominal), so the engine must produce slightly
/// more output frames per input frame. A negative value means the device is
/// slow.
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
                for (k, tap) in kernel.iter().enumerate() {
                    acc += self.pending[(start + k) * channels + channel] * tap;
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
        assert!((199..=201).contains(&total), "got {total}");
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
}
