//! Null output sink: counts (and optionally records) the master mix without
//! touching a sound card.
//!
//! Used for `--null-audio`, for CI, and as the automatic fallback when no cpal
//! device can be opened. Production traffic is simply dropped, so the receiver
//! keeps working on a machine without audio hardware; tests use
//! [`super::AudioEngine::start_capture`] to inspect what would have been played.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::{PcmSink, TARGET_CHANNELS, TARGET_RATE};

/// How much master audio the test recorder keeps before dropping the oldest
/// samples (~8 s of 48 kHz stereo f32 = 3 MiB, bounded).
const CAPTURE_SECONDS: usize = 8;

pub(crate) struct NullSink {
    frames: AtomicU64,
    capture: bool,
    captured: Mutex<VecDeque<f32>>,
    capacity_samples: usize,
}

impl NullSink {
    pub(crate) fn new(capture: bool) -> Self {
        let capacity_samples =
            TARGET_RATE as usize * TARGET_CHANNELS as usize * CAPTURE_SECONDS;
        Self {
            frames: AtomicU64::new(0),
            capture,
            captured: Mutex::new(VecDeque::with_capacity(if capture {
                capacity_samples.min(64 * 1024)
            } else {
                0
            })),
            capacity_samples,
        }
    }

    /// Total master frames consumed since the engine started.
    pub(crate) fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Copy of the recorded master mix (tests only; empty unless capture was
    /// enabled at construction).
    pub(crate) fn captured(&self) -> Vec<f32> {
        self.captured
            .lock()
            .map(|guard| guard.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn clear_captured(&self) {
        if let Ok(mut guard) = self.captured.lock() {
            guard.clear();
        }
    }
}

impl PcmSink for NullSink {
    fn consume(&self, samples: &[f32]) {
        let frames = samples.len() / TARGET_CHANNELS as usize;
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
        if !self.capture {
            return;
        }
        // A poisoned lock must not take the audio path down; dropping the
        // capture silently is acceptable.
        let Ok(mut guard) = self.captured.lock() else {
            return;
        };
        let overflow = guard
            .len()
            .saturating_add(samples.len())
            .saturating_sub(self.capacity_samples);
        if overflow > 0 {
            let buffered = guard.len();
            guard.drain(..overflow.min(buffered));
        }
        guard.extend(samples.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_counts_frames_and_ignores_odd_tails() {
        let sink = NullSink::new(false);
        sink.consume(&[0.0; 96]);
        sink.consume(&[0.0; 95]);
        assert_eq!(sink.frames(), 48 + 47);
        assert!(sink.captured().is_empty());
    }

    #[test]
    fn null_sink_captures_and_bounds_memory() {
        let sink = NullSink::new(true);
        let block = vec![0.5f32; 1024];
        for _ in 0..2_000 {
            sink.consume(&block);
        }
        let captured = sink.captured();
        assert!(!captured.is_empty());
        assert!(captured.len() <= sink.capacity_samples);
        assert!(captured.iter().all(|sample| *sample == 0.5));
        sink.clear_captured();
        assert!(sink.captured().is_empty());
    }
}
