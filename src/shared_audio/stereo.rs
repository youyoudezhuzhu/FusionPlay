//! Shared Windows and fnOS DLNA stereo conversion.
use anyhow::{anyhow, Result};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    calculate_cutoff, Async, FixedAsync, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::{Channels, Position};

use super::TARGET_RATE;
const TARGET_CHANNELS: usize = 2;
const RESAMPLE_CHUNK_FRAMES: usize = 1024;

/// Converts decoded source audio into the engine's 48 kHz stereo f32 format.
///
/// Multi-channel input is *mixed* (ITU-style coefficients for positioned
/// layouts, a conservative L/R/centre heuristic for layouts without positions)
/// rather than dropping channels.
pub(super) struct Converter {
    pub(super) source_rate: u32,
    pub(super) planes: usize,
    downmix: Downmix,
    resampler: Option<StreamResampler>,
    stereo: Vec<f32>,
}

impl Converter {
    pub(super) fn new(source_rate: u32, channels: &Channels, planes: usize) -> Result<Self> {
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

    pub(super) fn convert(&mut self, interleaved: &[f32], out: &mut Vec<f32>) -> Result<()> {
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

    pub(super) fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
