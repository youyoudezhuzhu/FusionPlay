// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Fabian Schmieder

//! Single Source of Truth (SSOT) for global AirPlay receiver capabilities and profiles.

/// Global device hardware model advertised to Apple clients.
///
/// An Apple TV hardware identifier makes senders present this receiver as an
/// Apple TV instead of a Mac, speaker, or generic computer. Keep it identical
/// in mDNS and `/info`, because
/// Apple clients cache the model against the persistent device identifier.
pub(crate) const GLOBAL_MODEL: &str = "AppleTV6,2";

// --- Audio output latency ---
//
// Advertised in the RTSP `RECORD` `Audio-Latency` header. The sender buffers
// at least this many frames before playing, so it trades end-to-end latency
// against tolerance for Wi-Fi jitter.

/// Default AirPlay 2 minimum output latency: 88,200 frames = 2 s at the
/// 44.1 kHz AirPlay clock. Genuine AirPlay receivers advertise this value;
/// advertising 0 drops the sender-side buffer, which causes stutter and breaks
/// multi-room sync with genuine receivers.
pub(crate) const DEFAULT_AUDIO_LATENCY_FRAMES: u32 = 88_200;

/// Classic AirPlay (AP1) fixed minimum output latency: 11,025 frames = 250 ms.
/// AP1 senders are tuned to this value; it is deliberately not configurable.
pub(crate) const AP1_AUDIO_LATENCY_FRAMES: u32 = 11_025;

/// Lowest configurable AP2 latency: 11,025 frames = 250 ms (the same floor
/// mature RAOP receivers use). Below this, senders cannot buffer enough to ride
/// out normal Wi-Fi jitter.
pub(crate) const MIN_AUDIO_LATENCY_FRAMES: u32 = AP1_AUDIO_LATENCY_FRAMES;

/// Highest configurable AP2 latency: 176,400 frames = 4 s.
pub(crate) const MAX_AUDIO_LATENCY_FRAMES: u32 = 176_400;

/// Clamp a host-supplied AirPlay 2 latency to the supported range.
///
/// Values outside `MIN_AUDIO_LATENCY_FRAMES..=MAX_AUDIO_LATENCY_FRAMES`
/// (including 0) fall back to [`DEFAULT_AUDIO_LATENCY_FRAMES`] so a bad config
/// can never make playback stutter or hang.
pub(crate) fn sanitize_audio_latency_frames(frames: u32) -> u32 {
    if (MIN_AUDIO_LATENCY_FRAMES..=MAX_AUDIO_LATENCY_FRAMES).contains(&frames) {
        frames
    } else {
        tracing::warn!(
            requested_frames = frames,
            default_frames = DEFAULT_AUDIO_LATENCY_FRAMES,
            "Requested AirPlay audio latency is out of range; using the protocol default"
        );
        DEFAULT_AUDIO_LATENCY_FRAMES
    }
}

/// RTSP/mDNS protocol version for AirPlay 2.
#[cfg(feature = "ap2")]
pub(crate) const AP2_PROTOVERS: &str = "1.1";

/// Software build/source version reported in GET /info and mDNS.
#[cfg(feature = "ap2")]
pub(crate) const AP2_SRCVERS: &str = "366.0";

/// AP2 status flag: audio output is available.
#[cfg(feature = "ap2")]
pub(crate) const AP2_STATUS_AUDIO_ATTACHED: u32 = 1 << 2;

/// AP2 status flag (bit 9): one-time HomeKit pairing (a PIN) is required. A PIN
/// accessory advertises this whether paired or not — matches real Apple TVs.
#[cfg(feature = "ap2")]
pub(crate) const AP2_STATUS_ONE_TIME_PAIRING_REQUIRED: u32 = 1 << 9;

/// AP2 status flag (bit 10): the accessory has completed HomeKit access-control
/// setup (i.e. it is paired). Advertised alongside bit 9 once paired.
#[cfg(feature = "ap2")]
pub(crate) const AP2_STATUS_DEVICE_SETUP_FOR_HK_ACCESS_CONTROL: u32 = 1 << 10;

/// Build AP2 statusFlags for the selected pairing mode.
///
/// Matches the flag *structure* real Apple TVs advertise (`flags=0x18644` =
/// bits 2,6,9,10,15,16): a PIN accessory always keeps bit 9
/// (`OneTimePairingRequired`) and, once paired, *adds* bit 10
/// (`DeviceSetupForHKAccessControl`) — so `sf=0x204` unpaired, `sf=0x604` paired.
///
/// Do NOT instead clear bit 9 to `0x004`: that is a transient (PIN-less)
/// accessory's flag value, so the sender treats us as transient and attempts a
/// transient pair-setup we must reject (stalling the connection). And bit 10
/// *without* bit 9 (`0x404`) makes the sender attempt transient on every
/// connection. Bit 9 says "I'm a PIN device"; bit 10 says "already set up".
#[cfg(feature = "ap2")]
pub(crate) fn ap2_status_flags(requires_pin_pairing: bool, already_paired: bool) -> u32 {
    let mut flags = AP2_STATUS_AUDIO_ATTACHED;
    if requires_pin_pairing {
        flags |= AP2_STATUS_ONE_TIME_PAIRING_REQUIRED;
        if already_paired {
            flags |= AP2_STATUS_DEVICE_SETUP_FOR_HK_ACCESS_CONTROL;
        }
    }
    flags
}

#[cfg(all(test, feature = "ap2"))]
mod status_flag_tests {
    use super::*;

    #[test]
    fn status_flags_match_apple_tv_structure() {
        assert_eq!(ap2_status_flags(false, false), 0x004, "transient (PIN-less)");
        assert_eq!(ap2_status_flags(false, true), 0x004, "transient (paired irrelevant)");
        assert_eq!(ap2_status_flags(true, false), 0x204, "PIN, unpaired");
        assert_eq!(
            ap2_status_flags(true, true),
            0x604,
            "PIN, paired (bits 9+10, like Apple TV)"
        );
    }
}

#[cfg(test)]
mod audio_latency_tests {
    use super::*;

    #[test]
    fn default_is_two_seconds_and_classic_floor_is_250ms() {
        assert_eq!(DEFAULT_AUDIO_LATENCY_FRAMES, 88_200);
        assert_eq!(AP1_AUDIO_LATENCY_FRAMES, 11_025);
    }

    #[test]
    fn valid_latencies_are_preserved() {
        for frames in [MIN_AUDIO_LATENCY_FRAMES, 22_050, 44_100, MAX_AUDIO_LATENCY_FRAMES] {
            assert_eq!(sanitize_audio_latency_frames(frames), frames);
        }
    }

    #[test]
    fn out_of_range_latencies_fall_back_to_the_default() {
        for frames in [
            0,
            1,
            MIN_AUDIO_LATENCY_FRAMES - 1,
            MAX_AUDIO_LATENCY_FRAMES + 1,
            u32::MAX,
        ] {
            assert_eq!(
                sanitize_audio_latency_frames(frames),
                DEFAULT_AUDIO_LATENCY_FRAMES,
                "{frames} frames must fall back to the protocol default"
            );
        }
    }
}

// --- Screen Mirroring (Video) Display Specifications ---
// Only consumed by the `video` screen-mirroring path in handlers_ap2.

/// Width in pixels advertised for the virtual display target.
#[cfg(feature = "video")]
pub(crate) const MIRRORING_WIDTH: i64 = 1920;

/// Height in pixels advertised for the virtual display target.
#[cfg(feature = "video")]
pub(crate) const MIRRORING_HEIGHT: i64 = 1080;

/// Frame rate advertised for the virtual display target.
#[cfg(feature = "video")]
pub(crate) const MIRRORING_FPS: i64 = 60;

/// Static UUID tag for the virtual display target.
#[cfg(feature = "video")]
pub(crate) const MIRRORING_UUID: &str = "shairplay_display";

/// Display features bitmask advertised for screen mirroring.
#[cfg(feature = "video")]
pub(crate) const MIRRORING_FEATURES: i64 = 2;
