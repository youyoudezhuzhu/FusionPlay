//! cpal output engine (48 kHz stereo f32 master, null fallback).
//! Ownership: audio-dlna (T1).
//!
//! The three protocol hosts push decoded PCM here through three different
//! contracts:
//! - AirPlay: [`fusionplay_core::PcmOutput`] (`write`/`flush`)
//! - MiPlay: [`fusionplay_miplay_sdk::MiPlayPcmOutput`] (`write`/`flush`)
//! - DLNA: [`AudioEngine::write_dlna_pcm`] from the streaming decoder
//!
//! Every source is converted to the fixed 48 kHz / stereo / f32 master mix,
//! scaled by the sender gain and appended to one bounded lock-free-ish ring.
//! The cpal callback drains that ring without blocking; if the default device
//! cannot run at 48 kHz the callback resamples to the negotiated device rate
//! (windowed sinc, with slow clock-drift correction).
//!
//! `--null-audio` or any device-open failure switches the engine to the null
//! backend: PCM is dropped, `mode` becomes `"null"` and startup still succeeds.

pub mod engine;
pub mod null;
pub mod resample;

pub use engine::AudioEngine;

use serde::Serialize;

/// The unified master mix format. All protocol hosts deliver into this format.
pub const TARGET_RATE: u32 = 48_000;
pub const TARGET_CHANNELS: u16 = 2;

/// Non-blocking warning callback shared by the engine and the UI log bridge
/// (`Arc<dyn Fn(&str) + Send + Sync>`).
pub type WarnSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Startup configuration resolved from the sidecar CLI.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// `--null-audio`: never touch a sound card, drop PCM (CI / no audio).
    pub null_audio: bool,
    /// `--output-device`: cpal device name or id; `None` = system default.
    pub output_device: Option<String>,
}

/// Live engine status surfaced through the NDJSON `app_state`/diagnostics.
///
/// `sample_rate`/`channels` describe the negotiated device stream (null mode
/// reports the fixed 48 kHz / 2 ch master format).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EngineStatus {
    /// `"cpal"` for the real output device, `"null"` when muted.
    pub mode: String,
    pub device_name: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub underruns: u64,
}

/// Internal sink contract: receives interleaved 48 kHz stereo f32.
///
/// Implementations must be cheap and must not block the caller (protocol
/// threads call this with decoder-sized blocks).
pub(crate) trait PcmSink: Send + Sync {
    fn consume(&self, samples: &[f32]);
}

/// Clamp a sender gain into a sane linear range. Non-finite gains (bad remote
/// value) fall back to unity instead of poisoning the whole stream.
pub(crate) fn sanitize_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 4.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_gain_handles_bad_values() {
        assert_eq!(sanitize_gain(0.5), 0.5);
        assert_eq!(sanitize_gain(-1.0), 0.0);
        assert_eq!(sanitize_gain(100.0), 4.0);
        assert_eq!(sanitize_gain(f32::NAN), 1.0);
        assert_eq!(sanitize_gain(f32::INFINITY), 1.0);
    }
}
