//! mDNS service registration for AirPlay network discovery.
//!
//! Platform-conditional: uses `astro-dnssd` (native Bonjour) on macOS,
//! `mdns-sd` (pure Rust) on Linux and other platforms.

use crate::error::NetworkError;
use crate::util;

// --- AP1 mDNS TXT record constants ---

/// TXT record version.
pub(crate) const RAOP_TXTVERS: &str = "1";
/// Audio channels.
pub(crate) const RAOP_CH: &str = "2";
/// Codecs: 0=PCM, 1=ALAC.
pub(crate) const RAOP_CN: &str = "0,1";
/// Encryption types: 0=none, 1=RSA.
pub(crate) const RAOP_ET: &str = "0,1";
/// Server version flag.
pub(crate) const RAOP_SV: &str = "false";
/// Digest auth supported.
pub(crate) const RAOP_DA: &str = "true";
/// Sample rate.
pub(crate) const RAOP_SR: &str = "44100";
/// Sample size (bits).
pub(crate) const RAOP_SS: &str = "16";
/// Protocol version.
pub(crate) const RAOP_VN: &str = "3";
/// Transport protocols.
pub(crate) const RAOP_TP: &str = "TCP,UDP";
/// Metadata types: 0=text, 1=artwork, 2=progress.
pub(crate) const RAOP_MD: &str = "0,1,2";
/// System managed.
pub(crate) const RAOP_SM: &str = "false";
/// Encryption key type.
pub(crate) const RAOP_EK: &str = "1";
/// AP2 encryption types advertised in `_raop` TXT (0=none, 3/5=FairPlay).
#[cfg(feature = "ap2")]
pub(crate) const RAOP_AP2_ET: &str = "0,3,5";
/// AP2 `_raop` protocol version (65537 = 0x10001).
#[cfg(feature = "ap2")]
pub(crate) const RAOP_AP2_VN: &str = "65537";

/// Global feature bitmask for AP1 discovery.
pub(crate) const GLOBAL_FEATURES: u32 = 0x7;
/// Device model identifier.
pub(crate) const GLOBAL_MODEL: &str = crate::raop::config::GLOBAL_MODEL;

/// Audio-only model identifier (2nd-gen AirPort Express, AirPlay 2 audio-only).
/// Smartphones show this receiver as a speaker, not as an Apple TV / video
/// target; the model is what iOS uses for the device icon and video choices.
pub(crate) const AUDIO_ONLY_MODEL: &str = crate::raop::config::AUDIO_ONLY_MODEL;
/// Software version string.
pub(crate) const GLOBAL_VERSION: &str = "130.14";
/// Darwin major version corresponding to the advertised macOS 15 profile.
///
/// Real Macs publish this in `_device-info._tcp`; Apple clients use that
/// service, rather than the AirPlay model alone, to classify a receiver as a
/// computer.
pub(crate) const DEVICE_INFO_OSX_VERSION: &str = "24";

// --- AP2 mDNS TXT record constants ---

/// Source version string.
#[cfg(feature = "ap2")]
pub(crate) const AP2_SRCVERS: &str = crate::raop::config::AP2_SRCVERS;
/// OS version string.
#[cfg(feature = "ap2")]
pub(crate) const AP2_OSVERS: &str = "15.6";
/// Firmware version string.
#[cfg(feature = "ap2")]
pub(crate) const AP2_FW_VERSION: &str = "77.40.00";
/// Protocol version string.
#[cfg(feature = "ap2")]
pub(crate) const AP2_PROTOVERS: &str = crate::raop::config::AP2_PROTOVERS;

/// mDNS service information for AirPlay network discovery.
#[derive(Debug, Clone)]
pub struct AirPlayServiceInfo {
    /// RAOP service name ("AABBCCDDEEFF@Display Name").
    pub raop_name: String,
    /// AirPlay display name.
    pub airplay_name: String,
    /// RTSP listening port.
    pub port: u16,
    /// TXT records for _raop._tcp.
    pub raop_txt: Vec<(String, String)>,
    /// TXT records for _airplay._tcp.
    pub airplay_txt: Vec<(String, String)>,
    /// TXT records for _device-info._tcp.
    pub device_info_txt: Vec<(String, String)>,
}

impl AirPlayServiceInfo {
    /// Create AP1 service info for mDNS registration.
    pub fn new(name: &str, port: u16, hwaddr: &[u8], password: bool) -> Self {
        Self::new_with_profile(name, port, hwaddr, password, false)
    }

    /// Create AP1 service info with an explicit audio-only profile.
    pub fn new_with_profile(
        name: &str,
        port: u16,
        hwaddr: &[u8],
        password: bool,
        audio_only: bool,
    ) -> Self {
        let hw_raop = util::hwaddr_raop(hwaddr);
        let hw_airplay = util::hwaddr_airplay(hwaddr);
        let raop_name = format!("{hw_raop}@{name}");

        let raop_txt = vec![
            ("txtvers".into(), RAOP_TXTVERS.into()),
            ("ch".into(), RAOP_CH.into()),
            ("cn".into(), RAOP_CN.into()),
            ("et".into(), RAOP_ET.into()),
            ("sv".into(), RAOP_SV.into()),
            ("da".into(), RAOP_DA.into()),
            ("sr".into(), RAOP_SR.into()),
            ("ss".into(), RAOP_SS.into()),
            ("pw".into(), (if password { "true" } else { "false" }).into()),
            ("vn".into(), RAOP_VN.into()),
            ("tp".into(), RAOP_TP.into()),
            ("md".into(), RAOP_MD.into()),
            ("vs".into(), GLOBAL_VERSION.into()),
            ("sm".into(), RAOP_SM.into()),
            ("ek".into(), RAOP_EK.into()),
        ];

        let model = if audio_only { AUDIO_ONLY_MODEL } else { GLOBAL_MODEL };
        // 0x7 advertises AirPlay video/photo; an audio-only receiver must only
        // advertise the audio feature bit (bit 9).
        let features = if audio_only { 1u32 << 9 } else { GLOBAL_FEATURES };
        let airplay_txt = vec![
            ("deviceid".into(), hw_airplay),
            ("features".into(), format!("0x{features:x}")),
            ("model".into(), model.into()),
        ];
        let mut device_info_txt = vec![("model".into(), model.to_string())];
        if !audio_only {
            device_info_txt.push(("osxvers".into(), DEVICE_INFO_OSX_VERSION.into()));
        }

        Self {
            raop_name,
            airplay_name: name.to_string(),
            port,
            raop_txt,
            airplay_txt,
            device_info_txt,
        }
    }

    /// Create AP2 service info with full AirPlay 2 feature flags.
    /// `pk_hex` is the hex-encoded Ed25519 public key, `pi` is the pairing identifier (UUID).
    #[cfg(feature = "ap2")]
    #[allow(clippy::too_many_arguments)]
    pub fn new_airplay2(
        name: &str,
        port: u16,
        hwaddr: &[u8],
        password: bool,
        pk_hex: &str,
        pi: &str,
        requires_pin_pairing: bool,
        already_paired: bool,
    ) -> Self {
        Self::new_airplay2_with_profile(
            name,
            port,
            hwaddr,
            password,
            pk_hex,
            pi,
            requires_pin_pairing,
            already_paired,
            false,
        )
    }

    /// Create AP2 service info with an explicit audio-only profile.
    #[cfg(feature = "ap2")]
    #[allow(clippy::too_many_arguments)]
    pub fn new_airplay2_with_profile(
        name: &str,
        port: u16,
        hwaddr: &[u8],
        password: bool,
        pk_hex: &str,
        pi: &str,
        requires_pin_pairing: bool,
        already_paired: bool,
        audio_only: bool,
    ) -> Self {
        let hw_raop = util::hwaddr_raop(hwaddr);
        let hw_airplay = util::hwaddr_airplay(hwaddr);
        let raop_name = format!("{hw_raop}@{name}");

        let features = super::features::receiver_features_for_pairing_with(
            requires_pin_pairing,
            audio_only,
        );
        let model = if audio_only { AUDIO_ONLY_MODEL } else { GLOBAL_MODEL };
        let features_lo = features & 0xFFFFFFFF;
        let features_hi = (features >> 32) & 0xFFFFFFFF;
        let ft = format!("0x{features_lo:X},0x{features_hi:X}");
        let status_flags = crate::raop::config::ap2_status_flags(requires_pin_pairing, already_paired);

        let raop_txt = vec![
            // AP1 compatibility fields (allows classic AirPlay fallback)
            ("cn".into(), RAOP_CN.into()),
            ("da".into(), RAOP_DA.into()),
            ("et".into(), RAOP_AP2_ET.into()),
            ("pw".into(), (if password { "true" } else { "false" }).into()),
            // AP2 fields
            ("ft".into(), ft.clone()),
            ("fv".into(), AP2_FW_VERSION.into()),
            ("sf".into(), format!("0x{status_flags:X}")),
            ("md".into(), RAOP_MD.into()),
            ("am".into(), model.into()),
            ("pk".into(), pk_hex.into()),
            ("tp".into(), RAOP_TP.into()), // TCP for AP1 fallback, UDP for AP2
            ("vn".into(), RAOP_AP2_VN.into()),
            ("vs".into(), AP2_SRCVERS.into()),
            ("ov".into(), AP2_OSVERS.into()),
        ];

        let airplay_txt = vec![
            ("acl".into(), "0".into()),
            ("btaddr".into(), "00:00:00:00:00:00".into()),
            ("deviceid".into(), hw_airplay),
            ("features".into(), ft),
            ("flags".into(), format!("0x{status_flags:X}")),
            ("gid".into(), pi.into()),
            ("igl".into(), "0".into()),
            ("gcgl".into(), "0".into()),
            ("model".into(), model.into()),
            ("protovers".into(), AP2_PROTOVERS.into()),
            ("pi".into(), pi.into()),
            ("pk".into(), pk_hex.into()),
            ("srcvers".into(), AP2_SRCVERS.into()),
            ("osvers".into(), AP2_OSVERS.into()),
            ("vv".into(), "2".into()),
            ("fv".into(), AP2_FW_VERSION.into()),
        ];
        let mut device_info_txt = vec![("model".into(), model.to_string())];
        if !audio_only {
            device_info_txt.push(("osxvers".into(), DEVICE_INFO_OSX_VERSION.into()));
        }

        Self {
            raop_name,
            airplay_name: name.to_string(),
            port,
            raop_txt,
            airplay_txt,
            device_info_txt,
        }
    }
}

/// Convert TXT record pairs to a `HashMap` for mDNS registration.
fn txt_map(txt: &[(String, String)]) -> std::collections::HashMap<String, String> {
    txt.iter().cloned().collect()
}

// --- macOS: native Bonjour via astro-dnssd ---

/// mDNS service registration for AirPlay.
///
/// On macOS, uses native Bonjour via `astro-dnssd`.
/// On Linux and other platforms, uses pure Rust `mdns-sd`.
#[cfg(target_os = "macos")]
pub(crate) struct MdnsService {
    _raop_reg: Option<astro_dnssd::RegisteredDnsService>,
    _airplay_reg: Option<astro_dnssd::RegisteredDnsService>,
    _device_info_reg: Option<astro_dnssd::RegisteredDnsService>,
}

#[cfg(target_os = "macos")]
impl MdnsService {
    /// Create a new mDNS service manager.
    pub(crate) fn new() -> Result<Self, NetworkError> {
        Ok(Self {
            _raop_reg: None,
            _airplay_reg: None,
            _device_info_reg: None,
        })
    }

    /// Register the _raop._tcp mDNS service.
    pub(crate) fn register_raop(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let reg = astro_dnssd::DNSServiceBuilder::new("_raop._tcp", info.port)
            .with_name(&info.raop_name)
            .with_txt_record(txt_map(&info.raop_txt))
            .register()
            .map_err(|e| NetworkError::Mdns(format!("{e:?}")))?;
        tracing::info!(name = %info.raop_name, port = info.port, "mDNS: _raop._tcp registered");
        self._raop_reg = Some(reg);
        Ok(())
    }

    /// Register the _airplay._tcp mDNS service.
    #[cfg(feature = "ap2")]
    pub(crate) fn register_airplay(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let reg = astro_dnssd::DNSServiceBuilder::new("_airplay._tcp", info.port)
            .with_name(&info.airplay_name)
            .with_txt_record(txt_map(&info.airplay_txt))
            .register()
            .map_err(|e| NetworkError::Mdns(format!("{e:?}")))?;
        tracing::info!(name = %info.airplay_name, port = info.port, "mDNS: _airplay._tcp registered");
        self._airplay_reg = Some(reg);
        Ok(())
    }

    /// Register the Mac identity hint used by Apple clients to classify this
    /// AirPlay receiver as a computer.
    pub(crate) fn register_device_info(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let reg = astro_dnssd::DNSServiceBuilder::new("_device-info._tcp", 0)
            .with_name(&info.airplay_name)
            .with_txt_record(txt_map(&info.device_info_txt))
            .register()
            .map_err(|e| NetworkError::Mdns(format!("{e:?}")))?;
        tracing::info!(
            name = %info.airplay_name,
            "_device-info._tcp registered"
        );
        self._device_info_reg = Some(reg);
        Ok(())
    }

    /// Unregister the _raop._tcp mDNS service.
    pub(crate) fn unregister_raop(&mut self) {
        self._raop_reg = None;
    }
    /// Unregister the _airplay._tcp mDNS service.
    pub(crate) fn unregister_airplay(&mut self) {
        self._airplay_reg = None;
    }
    /// Unregister the computer identity service.
    pub(crate) fn unregister_device_info(&mut self) {
        self._device_info_reg = None;
    }
}

#[cfg(target_os = "macos")]
impl Drop for MdnsService {
    fn drop(&mut self) {
        self.unregister_raop();
        self.unregister_airplay();
        self.unregister_device_info();
    }
}

// --- Linux/other: pure Rust mdns-sd ---

#[cfg(not(target_os = "macos"))]
/// mDNS service registration for AirPlay (Linux: pure Rust mdns-sd).
pub(crate) struct MdnsService {
    daemon: mdns_sd::ServiceDaemon,
    raop_fullname: Option<String>,
    airplay_fullname: Option<String>,
    device_info_fullname: Option<String>,
}

#[cfg(not(target_os = "macos"))]
impl MdnsService {
    /// Create a new mDNS service manager.
    pub(crate) fn new() -> Result<Self, NetworkError> {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        Ok(Self {
            daemon,
            raop_fullname: None,
            airplay_fullname: None,
            device_info_fullname: None,
        })
    }

    /// Register the _raop._tcp mDNS service.
    pub(crate) fn register_raop(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let svc = mdns_sd::ServiceInfo::new(
            "_raop._tcp.local.",
            &info.raop_name,
            &format!("{}.local.", gethostname::gethostname().to_string_lossy()),
            "",
            info.port,
            txt_map(&info.raop_txt),
        )
        .map(|svc| svc.enable_addr_auto())
        .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        self.raop_fullname = Some(svc.get_fullname().to_string());
        self.daemon
            .register(svc)
            .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        tracing::info!(name = %info.raop_name, port = info.port, "mDNS: _raop._tcp registered");
        Ok(())
    }

    /// Register the _airplay._tcp mDNS service.
    #[cfg(feature = "ap2")]
    pub(crate) fn register_airplay(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let svc = mdns_sd::ServiceInfo::new(
            "_airplay._tcp.local.",
            &info.airplay_name,
            &format!("{}.local.", gethostname::gethostname().to_string_lossy()),
            "",
            info.port,
            txt_map(&info.airplay_txt),
        )
        .map(|svc| svc.enable_addr_auto())
        .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        self.airplay_fullname = Some(svc.get_fullname().to_string());
        self.daemon
            .register(svc)
            .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        tracing::info!(name = %info.airplay_name, port = info.port, "mDNS: _airplay._tcp registered");
        Ok(())
    }

    /// Register the Mac identity hint used by Apple clients to classify this
    /// AirPlay receiver as a computer.
    pub(crate) fn register_device_info(&mut self, info: &AirPlayServiceInfo) -> Result<(), NetworkError> {
        let svc = mdns_sd::ServiceInfo::new(
            "_device-info._tcp.local.",
            &info.airplay_name,
            &format!("{}.local.", gethostname::gethostname().to_string_lossy()),
            "",
            0,
            txt_map(&info.device_info_txt),
        )
        .map(|svc| svc.enable_addr_auto())
        .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        self.device_info_fullname = Some(svc.get_fullname().to_string());
        self.daemon
            .register(svc)
            .map_err(|e| NetworkError::Mdns(format!("{e}")))?;
        tracing::info!(
            name = %info.airplay_name,
            "_device-info._tcp registered"
        );
        Ok(())
    }

    /// Unregister the _raop._tcp mDNS service.
    pub(crate) fn unregister_raop(&mut self) {
        if let Some(name) = self.raop_fullname.take() {
            let _ = self.daemon.unregister(&name);
        }
    }

    /// Unregister the _airplay._tcp mDNS service.
    pub(crate) fn unregister_airplay(&mut self) {
        if let Some(name) = self.airplay_fullname.take() {
            let _ = self.daemon.unregister(&name);
        }
    }
    /// Unregister the computer identity service.
    pub(crate) fn unregister_device_info(&mut self) {
        if let Some(name) = self.device_info_fullname.take() {
            let _ = self.daemon.unregister(&name);
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for MdnsService {
    fn drop(&mut self) {
        self.unregister_raop();
        self.unregister_airplay();
        self.unregister_device_info();
        let _ = self.daemon.shutdown();
    }
}

#[cfg(all(test, feature = "ap2"))]
mod tests {
    use super::*;

    #[test]
    fn ap2_raop_txt_has_required_fields() {
        let info = AirPlayServiceInfo::new_airplay2(
            "Test Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "12345678-1234-1234-1234-123456789abc",
            false,
            false,
        );

        let find = |key: &str| info.raop_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        // AP2 _raop._tcp must have these fields (matching shairport-sync)
        assert_eq!(find("vn"), Some("65537")); // AP2 version, not "3"
        assert_eq!(find("tp"), Some("TCP,UDP")); // TCP for AP1 fallback, UDP for AP2
        assert!(find("ft").unwrap().contains(",")); // features has hi,lo
        assert!(find("pk").is_some()); // Ed25519 public key
        assert!(find("sf").is_some()); // status flags
        assert_eq!(find("cn"), Some("0,1"));
        assert_eq!(find("da"), Some("true"));
        assert_eq!(find("pw"), Some("false"));
    }

    #[test]
    fn ap2_airplay_txt_has_required_fields() {
        let info = AirPlayServiceInfo::new_airplay2(
            "Test Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcd1234",
            "my-uuid-here",
            false,
            false,
        );

        let find = |key: &str| info.airplay_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        // AP2 _airplay._tcp required fields
        assert_eq!(find("acl"), Some("0"));
        assert!(find("deviceid").is_some());
        assert!(find("features").is_some());
        assert!(find("flags").is_some());
        assert!(find("gid").is_some());
        assert_eq!(find("model"), Some("AppleTV6,2"));
        assert_eq!(find("protovers"), Some("1.1"));
        assert!(find("pi").is_some());
        assert!(find("pk").is_some());
        assert_eq!(find("vv"), Some("2"));
    }

    #[test]
    fn audio_only_profile_advertises_speaker_model_without_video_bits() {
        let info = AirPlayServiceInfo::new_airplay2_with_profile(
            "NAS Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcd1234",
            "my-uuid-here",
            false,
            false,
            true,
        );
        let find = |records: &[(String, String)], key: &str| {
            records.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str().to_string())
        };
        // Audio-only model: iOS renders a speaker (AirPort Express), not a TV.
        assert_eq!(find(&info.airplay_txt, "model").as_deref(), Some("AirPort10,115"));
        assert_eq!(find(&info.raop_txt, "am").as_deref(), Some("AirPort10,115"));
        assert_eq!(find(&info.device_info_txt, "model").as_deref(), Some("AirPort10,115"));
        assert!(find(&info.device_info_txt, "osxvers").is_none(), "audio-only must not advertise a macOS hint");
        let features_txt = find(&info.airplay_txt, "features").expect("features");
        let mut parts = features_txt.split(',');
        let lo = u64::from_str_radix(parts.next().unwrap().trim_start_matches("0x"), 16).unwrap();
        let hi = u64::from_str_radix(parts.next().unwrap().trim_start_matches("0x"), 16).unwrap();
        let features = lo | (hi << 32);
        assert_ne!(features & (1 << 9), 0, "audio feature must stay");
        assert_ne!(features & (1u64 << 40), 0, "AP2 buffered audio must stay");
        assert_ne!(features & (1u64 << 41), 0, "PTP must stay");
        // No video/photo/screen/HLS capability may be advertised at all,
        // otherwise iOS/macOS offer the receiver as a TV target.
        for bit in [0u8, 1, 2, 4, 5, 6, 7, 10, 12, 13, 25, 28, 33, 42, 44, 49] {
            assert_eq!(
                features & (1u64 << bit),
                0,
                "audio-only mDNS features leaked video bit {bit}"
            );
        }
        assert_eq!(
            find(&info.raop_txt, "ft").as_deref(),
            Some(features_txt.as_str()),
            "raop ft and airplay features stay consistent"
        );

        // Classic AP1 profile also switches model and drops the video bits.
        let ap1 = AirPlayServiceInfo::new_with_profile(
            "NAS Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            true,
        );
        assert_eq!(find(&ap1.airplay_txt, "model").as_deref(), Some("AirPort10,115"));
        assert_eq!(find(&ap1.airplay_txt, "features").as_deref(), Some("0x200"));
    }

    #[test]
    fn apple_tv_identity_is_advertised_consistently() {
        let info = AirPlayServiceInfo::new_airplay2(
            "Windows",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcd1234",
            "my-uuid-here",
            false,
            false,
        );

        fn find<'a>(txt: &'a [(String, String)], key: &str) -> Option<&'a str> {
            txt.iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value.as_str())
        }

        assert_eq!(info.airplay_name, "Windows");
        assert_eq!(find(&info.raop_txt, "am"), Some(GLOBAL_MODEL));
        assert_eq!(find(&info.airplay_txt, "model"), Some(GLOBAL_MODEL));
        assert_eq!(find(&info.device_info_txt, "model"), Some(GLOBAL_MODEL));
        assert_eq!(find(&info.device_info_txt, "osxvers"), Some(DEVICE_INFO_OSX_VERSION));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn device_info_service_accepts_the_standard_zero_port() {
        let service = mdns_sd::ServiceInfo::new(
            "_device-info._tcp.local.",
            "Windows",
            "windows.local.",
            "",
            0,
            std::collections::HashMap::from([
                ("model".to_string(), GLOBAL_MODEL.to_string()),
                ("osxvers".to_string(), DEVICE_INFO_OSX_VERSION.to_string()),
            ]),
        )
        .expect("valid _device-info service");

        assert_eq!(service.get_port(), 0);
    }

    #[test]
    fn ap2_raop_name_format() {
        let info = AirPlayServiceInfo::new_airplay2(
            "My Speaker",
            5000,
            &[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            false,
            "pk",
            "pi",
            false,
            false,
        );
        assert_eq!(info.raop_name, "123456789ABC@My Speaker");
        assert_eq!(info.airplay_name, "My Speaker");
    }

    #[test]
    #[cfg(not(feature = "video"))]
    fn ap2_pin_pairing_txt_changes_features_and_flags() {
        let info = AirPlayServiceInfo::new_airplay2(
            "Test Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcd1234",
            "my-uuid-here",
            true,
            false,
        );
        let raop = |key: &str| info.raop_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let airplay = |key: &str| info.airplay_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        let expected_features = if cfg!(feature = "hls") {
            "0x405D4A11,0x14340"
        } else {
            "0x405D4A00,0x14340"
        };
        assert_eq!(raop("ft"), Some(expected_features));
        assert_eq!(airplay("features"), Some(expected_features));
        assert_eq!(raop("sf"), Some("0x204"));
        assert_eq!(airplay("flags"), Some("0x204"));
    }

    #[test]
    #[cfg(feature = "video")]
    fn ap2_pin_pairing_txt_changes_flags_with_video_features() {
        let info = AirPlayServiceInfo::new_airplay2(
            "Test Speaker",
            7000,
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            false,
            "abcd1234",
            "my-uuid-here",
            true,
            false,
        );
        let raop = |key: &str| info.raop_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let airplay = |key: &str| info.airplay_txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        assert_eq!(raop("ft"), Some("0x527FFEE6,0x0"));
        assert_eq!(airplay("features"), Some("0x527FFEE6,0x0"));
        assert_eq!(raop("sf"), Some("0x204"));
        assert_eq!(airplay("flags"), Some("0x204"));
    }
}
