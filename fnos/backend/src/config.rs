//! Persistent JSON configuration for the fnOS host.
//!
//! Layout mirrors the Web UI: receiver identity, per-protocol switches,
//! audio output/calibration, logging. The file lives in `$TRIM_PKGHOME`
//! (`config.json`) and is written atomically.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::volume::VolumeCurve;

/// Current on-disk config schema. v1 had a single global `audio.curve`; v2
/// moves the curve into every device (`audio.devices[].curve`). Keeping the
/// version explicit makes the one-way migration idempotent and safe for
/// importing v1 exports.
pub const CONFIG_VERSION: u32 = 2;

/// Missing `config_version` means a v1 config file (the field did not exist
/// then), even though `Config::default()` writes the current version.
fn legacy_config_version() -> u32 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `0`/missing = v1 config that still needs migration.
    #[serde(default = "legacy_config_version")]
    pub config_version: u32,
    pub receiver: ReceiverSettings,
    pub protocols: ProtocolSettings,
    pub audio: AudioSettings,
    pub logging: LoggingSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            receiver: ReceiverSettings::default(),
            protocols: ProtocolSettings::default(),
            audio: AudioSettings::default(),
            logging: LoggingSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceiverSettings {
    /// Name advertised over AirPlay / DLNA / MiPlay.
    pub name: String,
    /// Optional AirPlay pairing PIN. `None` = transient pairing (no PIN).
    pub pin: Option<String>,
    /// Network interface name, empty = auto.
    pub interface: String,
}

impl Default for ReceiverSettings {
    fn default() -> Self {
        Self { name: "FusionPlay".to_string(), pin: None, interface: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtocolSettings {
    pub airplay: ProtocolSwitch,
    pub dlna: ProtocolSwitch,
    pub miplay: MiPlaySwitch,
}

impl Default for ProtocolSettings {
    fn default() -> Self {
        Self {
            airplay: ProtocolSwitch { enabled: true },
            dlna: ProtocolSwitch { enabled: true },
            miplay: MiPlaySwitch::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtocolSwitch {
    pub enabled: bool,
}

impl Default for ProtocolSwitch {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// MiPlay exposes the receiver to HyperOS senders as a specific device category.
/// The Android app lets the user switch this identity (车机/电视/平板/音响);
/// fnOS mirrors the same four values so the sender UI shows the matching icon.
pub const MIPLAY_DEVICE_TYPES: &[&str] = &["vehicle", "television", "tablet", "speaker", "display_speaker"];

pub fn default_miplay_device_type() -> String {
    "television".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MiPlaySwitch {
    pub enabled: bool,
    /// `vehicle | television | tablet | speaker` (`display_speaker` is accepted
    /// as a legacy alias of `speaker`, matching the Android app).
    #[serde(default = "default_miplay_device_type")]
    pub device_type: String,
}

impl Default for MiPlaySwitch {
    fn default() -> Self {
        Self { enabled: true, device_type: default_miplay_device_type() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Named sets of ALSA device IDs sharing the active receiver stream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_groups: Vec<OutputGroup>,
    /// Master mix rate. The engine runs at this rate and resamples per device.
    pub sample_rate: u32,
    /// Global calibration attenuation, 0–100 % (100 = no extra attenuation).
    pub global_attenuation_percent: f32,
    /// Legacy v1 global curve. It is migrated into each device on load and is
    /// **not** used by the audio engine any more. Kept (and skipped when
    /// neutral) so old exports can still be imported.
    #[serde(default, skip_serializing_if = "VolumeCurve::is_default")]
    pub curve: VolumeCurve,
    pub muted: bool,
    pub devices: Vec<DeviceSetting>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            output_groups: Vec::new(),
            sample_rate: 48_000,
            global_attenuation_percent: 100.0,
            curve: VolumeCurve::default(),
            muted: false,
            devices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputGroup {
    pub name: String,
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSetting {
    /// Stable ALSA id, e.g. `hw:CARD=ultra,DEV=0`.
    pub id: String,
    /// Human label captured at selection time (informational only).
    pub name: String,
    pub enabled: bool,
    /// Per-device loudness balance, -30..=0 dB (0 = unity).
    pub gain_db: f32,
    /// Manual calibration delay, ms (>= 0).
    pub delay_ms: f32,
    /// Preferred device rate, 0 = device default.
    pub sample_rate: u32,
    /// Per-device volume response curve (sender logical volume -> amplitude).
    #[serde(default)]
    pub curve: VolumeCurve,
}

impl Default for DeviceSetting {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: false,
            gain_db: 0.0,
            delay_ms: 0.0,
            sample_rate: 0,
            curve: VolumeCurve::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    /// `error|warn|info|debug|trace`
    pub level: String,
    pub max_bytes: u64,
    pub keep_bytes: u64,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self { level: "info".to_string(), max_bytes: 5 * 1024 * 1024, keep_bytes: 1024 * 1024 }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Config = serde_json::from_str(&text)
                    .with_context(|| format!("无法解析配置 {}", path.display()))?;
                let migrated = cfg.migrate();
                cfg.validate()?;
                if migrated {
                    // Persist the migrated schema once, so a downgrade cannot
                    // silently resurrect the old global curve.
                    cfg.save(path)?;
                }
                Ok(cfg)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(error) => Err(error).with_context(|| format!("无法读取配置 {}", path.display())),
        }
    }

    /// One-way, idempotent v1 -> v2 migration: the old global curve becomes the
    /// default curve of every device that does not already have its own curve.
    /// Returns whether anything changed (callers persist the result).
    pub fn migrate(&mut self) -> bool {
        if self.config_version >= CONFIG_VERSION {
            return false;
        }
        let legacy = self.audio.curve.clone();
        let legacy_is_effective = !legacy.is_default();
        if legacy_is_effective {
            for device in &mut self.audio.devices {
                if device.curve.is_default() {
                    device.curve = legacy.clone();
                }
            }
        }
        self.audio.curve = VolumeCurve::default();
        self.config_version = CONFIG_VERSION;
        true
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        // Always write the current schema version, even when the caller did not
        // bump it explicitly (e.g. after import).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut snapshot = self.clone();
        snapshot.config_version = CONFIG_VERSION;
        let text = serde_json::to_string_pretty(&snapshot)?;
        std::fs::write(&tmp, text).with_context(|| format!("无法写临时配置 {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("无法落盘配置 {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.receiver.name.trim().is_empty() {
            anyhow::bail!("接收器名称不能为空");
        }
        if self.audio.sample_rate != 44_100 && self.audio.sample_rate != 48_000 {
            anyhow::bail!("音源采样率仅支持 44100/48000");
        }
        if !MIPLAY_DEVICE_TYPES.contains(&self.protocols.miplay.device_type.as_str()) {
            anyhow::bail!(
                "小米妙播设备类型不合法：{}（可选：车机 vehicle / 电视 television / 平板 tablet / 音响 speaker）",
                self.protocols.miplay.device_type
            );
        }
        if !(0.0..=100.0).contains(&self.audio.global_attenuation_percent) {
            anyhow::bail!("全局衰减必须在 0–100%");
        }
        let mut names = std::collections::HashSet::new();
        for group in &self.audio.output_groups {
            anyhow::ensure!(!group.name.trim().is_empty() && names.insert(group.name.trim()),
                "输出方案名称不能为空或重复");
            anyhow::ensure!(!group.devices.is_empty() && group.devices.iter().all(|id| !id.is_empty()),
                "输出方案至少需要一个音箱");
        }
        if !self.audio.curve.is_default() {
            self.audio.curve.validate().map_err(anyhow::Error::msg)?;
        }
        for device in &self.audio.devices {
            device
                .curve
                .validate()
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("设备 {} 的音量曲线不合法", device.id))?;
        }
        for device in &self.audio.devices {
            if !(-30.0..=0.0).contains(&device.gain_db) {
                anyhow::bail!("设备 {} 的响度平衡必须在 -30–0 dB", device.id);
            }
            if device.delay_ms < 0.0 || device.delay_ms > 2_000.0 {
                anyhow::bail!("设备 {} 的延迟补偿必须在 0–2000 ms", device.id);
            }
        }
        Ok(())
    }

    /// Replace everything except identity fields that must survive re-imports.
    pub fn apply_import(&mut self, imported: Config) -> Result<()> {
        imported.validate()?;
        self.protocols = imported.protocols;
        self.audio = imported.audio;
        self.logging = imported.logging;
        // Keep the local receiver identity name? No: config import is explicit.
        self.receiver = imported.receiver;
        self.config_version = imported.config_version;
        self.migrate();
        Ok(())
    }
}

/// Default config path for the app: `$TRIM_PKGHOME/config.json`, else cwd.
pub fn default_config_path() -> PathBuf {
    if let Some(home) = std::env::var_os("TRIM_PKGHOME") {
        return PathBuf::from(home).join("config.json");
    }
    if let Some(data) = std::env::var_os("TRIM_PKGVAR") {
        return PathBuf::from(data).join("config.json");
    }
    PathBuf::from("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::CurveKind;

    fn legacy_json() -> String {
        r#"{
          "receiver": {"name": "Legacy"},
          "audio": {
            "global_attenuation_percent": 80,
            "curve": {"kind": "custom", "points": [[0.25, 0.15], [0.75, 0.7]]},
            "devices": [
              {"id": "hw:a", "name": "A", "enabled": true, "gain_db": -3, "delay_ms": 12, "sample_rate": 0},
              {"id": "hw:b", "name": "B", "enabled": false, "gain_db": 0, "delay_ms": 0, "sample_rate": 48000}
            ]
          }
        }"#
        .to_string()
    }

    #[test]
    fn roundtrip_keeps_per_device_curve_and_skips_legacy_curve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.receiver.name = "测试接收器".into();
        cfg.audio.global_attenuation_percent = 80.0;
        let mut device = DeviceSetting {
            id: "hw:CARD=ultra,DEV=0".into(),
            name: "极境-ultra".into(),
            enabled: true,
            gain_db: -3.0,
            delay_ms: 12.0,
            sample_rate: 48_000,
            ..Default::default()
        };
        device.curve = VolumeCurve { kind: CurveKind::Custom, points: vec![(0.3, 0.2), (0.7, 0.6)] };
        cfg.audio.devices.push(device);
        cfg.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"config_version\": 2"));
        assert!(!text.contains("\"curve\": {\n      \"kind\": \"balanced\""), "legacy global curve must not be written");
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.receiver.name, "测试接收器");
        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert!((loaded.audio.global_attenuation_percent - 80.0).abs() < 1e-6);
        assert_eq!(loaded.audio.devices.len(), 1);
        assert_eq!(loaded.audio.devices[0].curve.points, vec![(0.3, 0.2), (0.7, 0.6)]);
        assert!(loaded.audio.curve.is_default(), "legacy curve is cleared after migration");
        // Default file is created on first load.
        let missing = dir.path().join("new.json");
        let created = Config::load(&missing).unwrap();
        assert_eq!(created.receiver.name, "FusionPlay");
        assert_eq!(created.config_version, CONFIG_VERSION);
        assert!(missing.exists());
    }

    #[test]
    fn migrates_v1_global_curve_to_each_device_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, legacy_json()).unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert!(cfg.audio.curve.is_default());
        assert_eq!(cfg.audio.devices.len(), 2);
        for device in &cfg.audio.devices {
            assert_eq!(device.curve.kind, CurveKind::Custom, "{} got legacy curve", device.id);
            assert_eq!(device.curve.points, vec![(0.25, 0.15), (0.75, 0.7)]);
        }
        // gain/delay/enabled/sample_rate survive the migration untouched.
        assert_eq!(cfg.audio.devices[0].gain_db, -3.0);
        assert_eq!(cfg.audio.devices[0].delay_ms, 12.0);
        assert!(cfg.audio.devices[0].enabled);
        assert_eq!(cfg.audio.devices[1].sample_rate, 48_000);
        // The migrated file is written back and no longer carries the global curve.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"config_version\": 2"));
        assert!(!text.contains("\"global_attenuation_percent\": 80.0,\n    \"curve\""));
    }

    #[test]
    fn migration_is_idempotent_and_never_overwrites_explicit_device_curves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let legacy = legacy_json().replace(
            r#"{"id": "hw:a", "name": "A", "enabled": true, "gain_db": -3, "delay_ms": 12, "sample_rate": 0}"#,
            r#"{"id": "hw:a", "name": "A", "enabled": true, "gain_db": -3, "delay_ms": 12, "sample_rate": 0, "curve": {"kind": "custom", "points": [[0.5, 0.9]]}}"#,
        );
        std::fs::write(&path, legacy).unwrap();
        let first = Config::load(&path).unwrap();
        let device_a = first.audio.devices.iter().find(|d| d.id == "hw:a").unwrap();
        assert_eq!(device_a.curve.points, vec![(0.5, 0.9)], "explicit curve must win");
        let device_b = first.audio.devices.iter().find(|d| d.id == "hw:b").unwrap();
        assert_eq!(device_b.curve.points, vec![(0.25, 0.15), (0.75, 0.7)]);
        // Second load is a no-op.
        let second = Config::load(&path).unwrap();
        assert_eq!(second.config_version, CONFIG_VERSION);
        assert_eq!(second.audio.devices[0].curve, first.audio.devices[0].curve);
        assert_eq!(second.audio.devices[1].curve, first.audio.devices[1].curve);
        let mut third = second.clone();
        assert!(!third.migrate(), "migration must report no change once on v2");
    }

    #[test]
    fn importing_v1_export_migrates_and_keeps_calibration() {
        let imported: Config = serde_json::from_str(&legacy_json()).unwrap();
        let mut local = Config::default();
        local.apply_import(imported).unwrap();
        assert_eq!(local.config_version, CONFIG_VERSION);
        assert!(local.audio.curve.is_default());
        assert_eq!(local.audio.devices.len(), 2);
        assert_eq!(local.audio.devices[0].gain_db, -3.0);
        assert_eq!(local.audio.devices[0].curve.kind, CurveKind::Custom);
    }

    #[test]
    fn rejects_invalid_device_curve() {
        let mut cfg = Config::default();
        cfg.audio.devices.push(DeviceSetting {
            id: "x".into(),
            curve: VolumeCurve { kind: CurveKind::Custom, points: vec![(0.5, 0.6), (0.4, 0.2)] },
            ..Default::default()
        });
        assert!(cfg.validate().is_err(), "non-monotonic per-device curve must be rejected");
        cfg.audio.devices[0].curve.points.clear();
        cfg.audio.devices[0].gain_db = 3.0;
        assert!(cfg.validate().is_err(), "positive device gain must be rejected");
        cfg.audio.devices[0].gain_db = 0.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn miplay_device_type_defaults_and_roundtrips() {
        // Old configs without the field (or without protocols.miplay at all)
        // keep the Android default identity: television.
        let old: Config = serde_json::from_str(r#"{"protocols":{"miplay":{"enabled":true}}}"#).unwrap();
        assert_eq!(old.protocols.miplay.device_type, "television");
        assert!(old.protocols.miplay.enabled);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.protocols.miplay.device_type = "vehicle".into();
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.protocols.miplay.device_type, "vehicle");

        cfg.protocols.miplay.device_type = "toaster".into();
        assert!(cfg.validate().is_err(), "unknown identity must be rejected");
        cfg.protocols.miplay.device_type = "display_speaker".into();
        assert!(cfg.validate().is_ok(), "Android legacy alias is accepted");
    }

    #[test]
    fn default_config_is_v2_with_neutral_device_curves() {
        let cfg = Config::default();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert!(cfg.audio.curve.is_default());
        assert!(cfg.audio.devices.is_empty());
    }
}
