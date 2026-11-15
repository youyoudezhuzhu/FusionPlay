//! AirPlay feature flags.
//!
//! 64-bit bitmask declared in mDNS `features` TXT record.
//! Reference: <https://emanuelecozzi.net/docs/airplay2/features/>

/// Individual AirPlay feature flags.
/// Source: <https://emanuelecozzi.net/docs/airplay2/features/>
///
/// Complete reference enum: only a feature-dependent subset of variants is
/// referenced by `receiver_features()` (the audio-only and `video` builds use
/// different sets), so the remaining variants are intentionally unused. Kept as
/// an exhaustive, documented bit map rather than pruned per build.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum AirPlayFeature {
    /// Bit 0 — AirPlay video v1
    SupportsAirPlayVideoV1 = 0,
    /// Bit 1 — AirPlay photo
    SupportsAirPlayPhoto = 1,
    /// Bit 2 — AirPlay video FairPlay
    SupportsAirPlayVideoFairPlay = 2,
    /// Bit 5 — AirPlay slideshow
    SupportsAirPlaySlideshow = 5,
    /// Bit 6 — AirPlay video volume control
    SupportsAirPlayVideoVolumeControl = 6,
    /// Bit 7 — AirPlay screen mirroring
    SupportsAirPlayScreen = 7,
    /// Bit 9 — AirPlay audio
    SupportsAirPlayAudio = 9,
    /// Bit 11 — Audio redundancy
    AudioRedundant = 11,
    /// Bit 14 — FairPlay authentication
    Authentication4 = 14,
    /// Bit 15 — Receive artwork image
    MetadataArtwork = 15,
    /// Bit 16 — Receive track progress
    MetadataProgress = 16,
    /// Bit 17 — Receive NowPlaying via DAAP
    MetadataNowPlayingDaap = 17,
    /// Bit 18 — Audio format support 0
    AudioFormats0 = 18,
    /// Bit 19 — Audio format support 1
    AudioFormats1 = 19,
    /// Bit 20 — Audio format support 2
    AudioFormats2 = 20,
    /// Bit 21 — Audio format support 3
    AudioFormats3 = 21,
    /// Bit 22 — Audio format support 4
    AudioFormats4 = 22,
    /// Bit 23 — RSA authentication (legacy, not used)
    Authentication1 = 23,
    /// Bit 26 — MFi authentication
    Authentication8Mfi = 26,
    /// Bit 27 — Legacy pairing
    SupportsLegacyPairing = 27,
    /// Bit 30 — Unified advertiser info
    HasUnifiedAdvertiserInfo = 30,
    /// Bit 32 — Volume control (when NOT CarPlay)
    SupportsVolume = 32,
    /// Bit 33 — Video play queue
    SupportsAirPlayVideoPlayQueue = 33,
    /// Bit 34 — AirPlay from cloud
    SupportsAirPlayFromCloud = 34,
    /// Bit 35 — TLS PSK
    SupportsTlsPsk = 35,
    /// Bit 38 — Unified media control
    SupportsUnifiedMediaControl = 38,
    /// Bit 40 — Buffered audio (AP2)
    SupportsBufferedAudio = 40,
    /// Bit 41 — PTP timing (AP2)
    SupportsPtp = 41,
    /// Bit 42 — Screen multi-codec
    SupportsScreenMultiCodec = 42,
    /// Bit 43 — System pairing
    SupportsSystemPairing = 43,
    /// Bit 44 — Valeria screen sender
    IsApValeriaScreenSender = 44,
    /// Bit 46 — HomeKit pairing and access control
    SupportsHkPairingAndAccessControl = 46,
    /// Bit 47 — Transient pairing
    SupportsTransientPairing = 47,
    /// Bit 48 — CoreUtils pairing and encryption
    SupportsCoreUtilsPairingAndEncryption = 48,
    /// Bit 49 — AirPlay video v2
    SupportsAirPlayVideoV2 = 49,
    /// Bit 50 — Receive NowPlaying via binary plist (overrides bit 17)
    MetadataNowPlayingBplist = 50,
    /// Bit 51 — Unified pair-setup and MFi
    SupportsUnifiedPairSetupAndMfi = 51,
    /// Bit 52 — Extended SETPEERS message
    SupportsSetPeersExtendedMessage = 52,
    /// Bit 54 — AP sync (multi-room)
    SupportsApSync = 54,
    /// Bit 55 — Wake on LAN
    SupportsWoL55 = 55,
    /// Bit 56 — Wake on LAN (alternate)
    SupportsWoL56 = 56,
    /// Bit 58 — Hangdog remote control (requires isAppleTV/isAppleAudioAccessory)
    SupportsHangdogRemoteControl = 58,
    /// Bit 59 — Audio stream connection setup
    SupportsAudioStreamConnectionSetup = 59,
    /// Bit 60 — Audio media data control (requires bit 59)
    SupportsAudioMediaDataControl = 60,
    /// Bit 61 — RFC 2198 redundancy
    SupportsRfc2198Redundancy = 61,
}

/// Build a 64-bit features bitmask from a set of feature flags.
pub(crate) fn features_from(flags: &[AirPlayFeature]) -> u64 {
    let mut val: u64 = 0;
    for &f in flags {
        val |= 1u64 << (f as u8);
    }
    val
}

/// Features for an audio-only AirPlay 2 receiver.
///
/// Features for an audio-only AirPlay 2 receiver.
///
/// Known-good bitmask: `0x0001C340405D4A00` (matches shairport-sync).
/// Tested working with iOS 18 on 2026-04-04.
///
/// # Warning
///
/// Do NOT add bits without testing on a real iPhone. Extra bits (e.g.
/// `SupportsAirPlayVideoV2`, `SupportsVolume`, `Authentication1`) cause
/// the iPhone to expect capabilities we don't implement, resulting in
/// the device being hidden from AirPlay discovery or immediate TEARDOWN
/// after SETUP.
///
/// Bits that were tested and MUST NOT be set for audio-only:
/// - bit 15 (MetadataArtwork) — causes discovery failure
/// - bit 17 (MetadataNowPlayingDaap) — causes discovery failure
/// - bit 23 (Authentication1/RSA) — causes discovery failure
/// - bit 27 (SupportsLegacyPairing) — causes discovery failure
/// - bit 32 (SupportsVolume) — causes discovery failure
/// - bit 49 (SupportsAirPlayVideoV2) — causes immediate TEARDOWN
/// - bit 50 (MetadataNowPlayingBplist) — causes discovery failure
/// - bit 52 (SupportsSetPeersExtendedMessage) — causes discovery failure
/// - bit 59/60/61 (AudioStreamConn/MediaDataCtrl/Rfc2198) — causes discovery failure
pub fn receiver_features() -> u64 {
    receiver_features_for_pairing(false)
}

/// Features for an audio-only receiver.
///
/// An audio-only receiver must **never** be advertised as a video target: Apple
/// senders decide between audio and video/screen-mirroring from these bits (and
/// the advertised model). Do not derive this set by clearing bits from the
/// `video`/`hls` profiles — build the pure audio capability set directly so a
/// future profile change cannot reintroduce video bits.
pub(crate) fn receiver_features_for_pairing_with(
    requires_pin_pairing: bool,
    audio_only: bool,
) -> u64 {
    if audio_only {
        return audio_only_features(requires_pin_pairing);
    }
    receiver_features_for_pairing(requires_pin_pairing)
}

/// Pure audio-only AirPlay 2 feature set (model `AirPort10,115`).
///
/// Contains audio, FairPlay, progress, an audio format range, unified media
/// control, buffered audio, PTP, HomeKit pairing and transient pairing. It
/// deliberately contains **no** video/photo/screen/HLS bits, independent of
/// which optional crate features (`video`, `hls`) are compiled in.
pub(crate) fn audio_only_features(requires_pin_pairing: bool) -> u64 {
    use AirPlayFeature::*;
    let bits: &[AirPlayFeature] = &[
        SupportsAirPlayAudio,                  // bit 9
        AudioRedundant,                        // bit 11
        Authentication4,                       // bit 14 — FairPlay
        MetadataProgress,                      // bit 16
        AudioFormats0,                         // bit 18
        AudioFormats1,                         // bit 19
        AudioFormats2,                         // bit 20
        AudioFormats4,                         // bit 22
        HasUnifiedAdvertiserInfo,              // bit 30
        SupportsUnifiedMediaControl,           // bit 38
        SupportsBufferedAudio,                 // bit 40
        SupportsPtp,                           // bit 41
        SupportsHkPairingAndAccessControl,     // bit 46
        SupportsTransientPairing,              // bit 47
        SupportsCoreUtilsPairingAndEncryption, // bit 48
    ];
    let mut features = features_from(bits);
    if requires_pin_pairing {
        features &= !(1u64 << (SupportsTransientPairing as u8));
    }
    features
}

/// Features for an audio-only AirPlay 2 receiver with explicit pairing mode.
///
/// The default receiver profile mirrors shairport-sync and supports transient
/// PIN-less pairing. When a one-time PIN is required, clear the transient
/// pairing bit so clients choose normal M1-M6 HomeKit pair-setup.
pub(crate) fn receiver_features_for_pairing(requires_pin_pairing: bool) -> u64 {
    #[cfg(not(feature = "video"))]
    use AirPlayFeature::*;

    // Known-good bitmask: 0x0001C340405D4A00
    // Verified with iOS 18 + shairport-sync reference.
    // Each bit is annotated with its position for easy cross-referencing
    // with https://emanuelecozzi.net/docs/airplay2/features/
    // When video is enabled, use UxPlay's proven feature set (0x527FFEE6).
    // Bit 27 (legacy pairing) must be OFF — otherwise the iPhone does
    // pair-setup/verify and the ECDH hash corrupts the FairPlay key.
    // No AP2 bits (40, 41, 46, 48) — pure legacy protocol for video.
    #[cfg(feature = "video")]
    {
        let _ = requires_pin_pairing;
        use AirPlayFeature::*;
        let mut val = features_from(&[
            SupportsAirPlayPhoto,              // bit 1
            SupportsAirPlayVideoFairPlay,      // bit 2
            SupportsAirPlaySlideshow,          // bit 5
            SupportsAirPlayVideoVolumeControl, // bit 6
            SupportsAirPlayScreen,             // bit 7
            SupportsAirPlayAudio,              // bit 9
            AudioRedundant,                    // bit 11
            Authentication4,                   // bit 14
            MetadataArtwork,                   // bit 15
            MetadataProgress,                  // bit 16
            MetadataNowPlayingDaap,            // bit 17
            AudioFormats0,                     // bit 18
            AudioFormats1,                     // bit 19
            AudioFormats2,                     // bit 20
            AudioFormats3,                     // bit 21
            AudioFormats4,                     // bit 22
            HasUnifiedAdvertiserInfo,          // bit 30
        ]);
        // Bits 10/12/13/25/28 have no public AirPlay-spec names — UxPlay sets them
        // empirically for legacy screen-mirroring compat. Kept as raw shifts
        // deliberately; the exact value is pinned by `video_receiver_uses_uxplay_features`.
        val |= (1 << 10) | (1 << 12) | (1 << 13) | (1 << 25) | (1 << 28);
        val
    }

    #[cfg(not(feature = "video"))]
    {
        let bits: Vec<AirPlayFeature> = vec![
            SupportsAirPlayAudio,                  // bit 9
            AudioRedundant,                        // bit 11
            Authentication4,                       // bit 14 — FairPlay
            MetadataProgress,                      // bit 16
            AudioFormats0,                         // bit 18
            AudioFormats1,                         // bit 19
            HasUnifiedAdvertiserInfo,              // bit 30
            SupportsUnifiedMediaControl,           // bit 38
            SupportsBufferedAudio,                 // bit 40
            SupportsPtp,                           // bit 41
            SupportsHkPairingAndAccessControl,     // bit 46
            SupportsCoreUtilsPairingAndEncryption, // bit 48
            AudioFormats2,                         // bit 20
            AudioFormats4,                         // bit 22
            SupportsTransientPairing,              // bit 47
        ];

        let mut features = features_from(&bits);
        if requires_pin_pairing {
            features &= !(1u64 << (SupportsTransientPairing as u8));
        }
        // HTTP video relay is independent from the raw screen-mirroring
        // transport. Keep the known-good AP2 audio profile and add only the
        // two capabilities implemented by the `/play` HLS control path:
        // AirPlay video v1 (bit 0) and HTTP Live Streaming (bit 4).
        #[cfg(feature = "hls")]
        {
            features |= (1u64 << 0) | (1u64 << 4);
        }
        features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_bits_correct() {
        assert_eq!(1u64 << (AirPlayFeature::SupportsAirPlayAudio as u8), 1 << 9);
        assert_eq!(1u64 << (AirPlayFeature::SupportsPtp as u8), 1 << 41);
        assert_eq!(1u64 << (AirPlayFeature::SupportsHangdogRemoteControl as u8), 1 << 58);
    }

    #[test]
    fn features_from_builds_bitmask() {
        let f = features_from(&[AirPlayFeature::SupportsAirPlayAudio, AirPlayFeature::SupportsPtp]);
        assert!(f & (1 << 9) != 0);
        assert!(f & (1 << 41) != 0);
        assert!(f & (1 << 0) == 0);
    }

    /// VIDEO_BITS are the feature bits Apple senders use to decide that a
    /// receiver can act as a video / photo / screen-mirroring target.
    const VIDEO_BITS: &[u8] = &[0, 1, 2, 4, 5, 6, 7, 10, 12, 13, 25, 28, 33, 42, 44, 49];

    #[test]
    fn audio_only_features_are_pure_audio_regardless_of_build_features() {
        for pin in [false, true] {
            let features = receiver_features_for_pairing_with(pin, true);
            for &bit in VIDEO_BITS {
                assert_eq!(
                    features & (1u64 << bit),
                    0,
                    "audio-only receiver must never advertise video bit {bit} (pin={pin})"
                );
            }
            // Audio, FairPlay, metadata, AP2 timing/buffered audio and pairing
            // capabilities must stay.
            for bit in [9u8, 11, 14, 16, 18, 19, 20, 22, 30, 38, 40, 41, 46, 48] {
                assert_ne!(
                    features & (1u64 << bit),
                    0,
                    "audio-only receiver must keep needed bit {bit}"
                );
            }
            if pin {
                assert_eq!(features & (1u64 << 47), 0, "PIN pairing clears transient bit");
            } else {
                assert_ne!(features & (1u64 << 47), 0, "transient pairing bit kept without PIN");
            }
            // Exact values are pinned: 0x0001C340405D4A00 (no PIN) and
            // 0x00014340405D4A00 (PIN, transient bit cleared).
            let expected = if pin {
                0x0001_4340_405D_4A00u64
            } else {
                0x0001_C340_405D_4A00u64
            };
            assert_eq!(
                features, expected,
                "audio-only feature set drifted (pin={pin})"
            );
        }
    }

    #[test]
    fn non_audio_only_profiles_keep_their_existing_behaviour() {
        // Sanity: the audio-only fix must not change the full-capability
        // profiles (video build still uses UxPlay's set, hls adds relay bits).
        #[cfg(feature = "video")]
        assert_eq!(receiver_features_for_pairing_with(false, false), 0x527F_FEE6);
        #[cfg(all(not(feature = "video"), feature = "hls"))]
        assert_eq!(
            receiver_features_for_pairing_with(false, false),
            0x0001_C340_405D_4A11
        );
        #[cfg(all(not(feature = "video"), not(feature = "hls")))]
        assert_eq!(
            receiver_features_for_pairing_with(false, false),
            0x0001_C340_405D_4A00
        );
    }

    #[test]
    #[cfg(all(not(feature = "video"), not(feature = "hls")))]
    fn audio_receiver_has_required_bits() {
        let f = receiver_features();
        // Pin the exact known-good value (matches shairport-sync, iOS 18 verified).
        assert_eq!(f, 0x0001_C340_405D_4A00, "audio receiver features drifted");
        // Core AP2 requirements (from shairport-sync)
        assert!(f & (1 << 9) != 0, "SupportsAirPlayAudio");
        assert!(f & (1 << 11) != 0, "AudioRedundant");
        assert!(f & (1 << 40) != 0, "SupportsBufferedAudio");
        assert!(f & (1 << 41) != 0, "SupportsPtp");
        assert!(f & (1 << 14) != 0, "Authentication4 (FairPlay)");
        assert!(f & (1 << 38) != 0, "SupportsUnifiedMediaControl");
        assert!(f & (1 << 46) != 0, "SupportsHKPairing");
        assert!(f & (1 << 48) != 0, "SupportsCoreUtilsPairing");
    }

    #[test]
    #[cfg(all(not(feature = "video"), not(feature = "hls")))]
    fn pin_pairing_clears_transient_bit() {
        let f = receiver_features_for_pairing(true);
        assert_eq!(f, 0x0001_4340_405D_4A00, "PIN pairing features drifted");
        assert!(f & (1 << 46) != 0, "SupportsHKPairing");
        assert!(f & (1 << 48) != 0, "SupportsCoreUtilsPairing");
        assert_eq!(f & (1 << 47), 0, "transient pairing bit must be off");
    }

    #[test]
    #[cfg(feature = "hls")]
    fn hls_keeps_ap2_audio_and_adds_only_video_relay_bits() {
        let f = receiver_features();
        assert_eq!(f, 0x0001_C340_405D_4A11);
        assert_ne!(f & (1 << 0), 0, "AirPlay video v1");
        assert_ne!(f & (1 << 4), 0, "HTTP Live Streaming");
        assert_ne!(f & (1 << 40), 0, "AP2 buffered audio remains enabled");
        assert_eq!(f & (1 << 7), 0, "screen mirroring is not advertised");
    }

    #[test]
    #[cfg(feature = "video")]
    fn video_receiver_uses_uxplay_features() {
        assert_eq!(receiver_features(), 0x527FFEE6);
    }
}
