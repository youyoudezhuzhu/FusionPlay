//! Persisted settings (`settings.json`, schemaVersion 8).
//! Ownership: host-state (T2).
//!
//! The file lives in the sidecar data directory (`%LOCALAPPDATA%\FusionPlay` on
//! Windows) and is shared with the Android/Flutter `AppSettings` model: the JSON
//! keys are camelCase and `receiverName: null` means "use the system computer
//! name". Writes go through a temporary file followed by an atomic rename so a
//! crash can never leave a half-written `settings.json` behind.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::ui_protocol::SettingsView;

/// File name inside the data directory.
pub const SETTINGS_FILE_NAME: &str = "settings.json";
/// Schema version written by this build (matches the Dart/Android model).
pub const CURRENT_SCHEMA_VERSION: u32 = 8;
/// Oldest schema version accepted from disk. Older files are upgraded in place.
pub const OLDEST_SUPPORTED_SCHEMA_VERSION: u32 = 1;
/// Maximum receiver name length (AirPlay/DLNA/MiPlay discovery budget).
pub const MAX_RECEIVER_NAME_LENGTH: usize = 63;
/// Persisted MiPlay device identity values accepted by the UI.
pub const MIPLAY_DEVICE_IDENTITIES: [&str; 4] = ["vehicle", "television", "tablet", "speaker"];

/// Normalise user-controlled settings into their canonical persisted form.
///
/// * `schemaVersion` is always rewritten to [`CURRENT_SCHEMA_VERSION`].
/// * `receiverName` is trimmed, an empty string becomes `None` ("use the
///   computer name") and long names are truncated to
///   [`MAX_RECEIVER_NAME_LENGTH`] characters (not bytes).
/// * `miPlayDeviceIdentity` falls back to `television` when unknown, and the
///   legacy `display_speaker` value maps to `speaker`.
pub fn validate(settings: SettingsView) -> SettingsView {
    let receiver_name = settings
        .receiver_name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let receiver_name = match receiver_name {
        Some(name) if name.chars().count() > MAX_RECEIVER_NAME_LENGTH => {
            tracing::warn!("接收器名称超过 {MAX_RECEIVER_NAME_LENGTH} 个字符，已截断：{name}");
            Some(truncate_chars(&name, MAX_RECEIVER_NAME_LENGTH))
        }
        other => other,
    };
    let original_identity = settings.mi_play_device_identity.trim().to_owned();
    if !is_supported_miplay_identity(&original_identity) {
        tracing::warn!("不支持的 miPlayDeviceIdentity（{original_identity}），已回退为 television");
    }

    SettingsView {
        schema_version: CURRENT_SCHEMA_VERSION,
        receiver_name,
        startup_enabled: settings.startup_enabled,
        auto_wake_enabled: settings.auto_wake_enabled,
        advanced_effects_enabled: settings.advanced_effects_enabled,
        mi_play_enabled: settings.mi_play_enabled,
        mi_play_device_identity: normalise_miplay_identity(&settings.mi_play_device_identity),
        air_play_enabled: settings.air_play_enabled,
        dlna_enabled: settings.dlna_enabled,
    }
}

/// Whether an identity value is accepted from the UI (case-insensitive).
///
/// The legacy `display_speaker` spelling is still accepted and normalised to
/// `speaker`. This is the validity check used by [`SettingsStore::save`]; the
/// projection in [`validate`] additionally falls back to `television`.
pub fn is_supported_miplay_identity(value: &str) -> bool {
    let normalised = value.trim().to_ascii_lowercase();
    normalised == "display_speaker" || MIPLAY_DEVICE_IDENTITIES.contains(&normalised.as_str())
}

/// Canonical MiPlay identity value (`vehicle|television|tablet|speaker`).
pub fn normalise_miplay_identity(value: &str) -> String {
    let normalised = value.trim().to_ascii_lowercase();
    match normalised.as_str() {
        // Older clients persisted `display_speaker` for what the UI now calls
        // `speaker`; keep accepting it so upgrades do not lose the choice.
        "display_speaker" => "speaker".to_owned(),
        identity if MIPLAY_DEVICE_IDENTITIES.contains(&identity) => identity.to_owned(),
        _ => "television".to_owned(),
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    value.chars().take(max_chars).collect()
}

/// Reads and writes `data_dir/settings.json`.
pub struct SettingsStore {
    data_dir: PathBuf,
    settings_path: PathBuf,
}

impl SettingsStore {
    pub fn new(data_dir: PathBuf) -> Self {
        let settings_path = data_dir.join(SETTINGS_FILE_NAME);
        Self {
            data_dir,
            settings_path,
        }
    }

    /// Absolute path of the settings file (diagnostics / tests).
    pub fn path(&self) -> &Path {
        &self.settings_path
    }

    /// Load persisted settings.
    ///
    /// A missing file yields defaults. A corrupt file yields defaults as well,
    /// but the original bytes are first copied to `settings.corrupt-<ts>.json`
    /// so the user can recover them from the data directory.
    pub fn load(&self) -> SettingsView {
        let text = match fs::read_to_string(&self.settings_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SettingsView::default();
            }
            Err(_) => return SettingsView::default(),
        };
        if text.trim().is_empty() {
            self.backup_corrupted(&text);
            return SettingsView::default();
        }
        match serde_json::from_str::<SettingsView>(&text) {
            Ok(settings) => validate(settings),
            Err(_) => {
                self.backup_corrupted(&text);
                SettingsView::default()
            }
        }
    }

    /// Persist validated settings atomically (tmp file + rename).
    ///
    /// An unrecognised `miPlayDeviceIdentity` is rejected instead of being
    /// silently rewritten, so the command dispatcher can report the failure to
    /// the UI (`response.ok == false`). Cosmetic normalisation (name trimming /
    /// truncation, schema upgrade) is still applied transparently.
    pub fn save(&self, settings: &SettingsView) -> Result<()> {
        if !is_supported_miplay_identity(&settings.mi_play_device_identity) {
            tracing::warn!(
                "拒绝保存设置：不支持的 miPlayDeviceIdentity（{}）",
                settings.mi_play_device_identity
            );
            anyhow::bail!(
                "不支持的 miPlay 设备类型：{}（允许 vehicle/television/tablet/speaker）",
                settings.mi_play_device_identity
            );
        }
        let validated = validate(settings.clone());
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("无法创建设置目录 {}", self.data_dir.display()))?;

        let mut encoded = serde_json::to_vec_pretty(&validated).context("无法序列化设置")?;
        encoded.push(b'\n');

        let temporary = self.temporary_path();
        // `create_new` guarantees we never clobber another writer's temp file.
        let write_result = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .with_context(|| format!("无法写入临时设置文件 {}", temporary.display()))?;
            file.write_all(&encoded)
                .context("无法写入临时设置文件内容")?;
            file.flush().context("无法刷新临时设置文件")?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }

        if let Err(error) = fs::rename(&temporary, &self.settings_path) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| {
                format!(
                    "无法以原子方式替换设置文件 {}（临时文件 {}）",
                    self.settings_path.display(),
                    temporary.display()
                )
            });
        }
        Ok(())
    }

    fn temporary_path(&self) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or_default();
        self.data_dir.join(format!(
            ".{}.{}.{}.tmp",
            SETTINGS_FILE_NAME,
            std::process::id(),
            nanos
        ))
    }

    fn backup_corrupted(&self, contents: &str) {
        let directory = self
            .settings_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.data_dir.clone());
        if fs::create_dir_all(&directory).is_err() {
            return;
        }
        let timestamp = sanitise_for_file_name(&crate::ui::chrono_like_now());
        let backup = directory.join(format!("settings.corrupt-{timestamp}.json"));
        let _ = fs::write(backup, contents);
    }
}

fn sanitise_for_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = SettingsStore::new(directory.path().to_path_buf());
        (directory, store)
    }

    #[test]
    fn missing_file_loads_defaults_without_creating_anything() {
        let (directory, store) = store();
        let loaded = store.load();
        assert_eq!(loaded, SettingsView::default());
        assert!(!directory.path().join(SETTINGS_FILE_NAME).exists());
    }

    #[test]
    fn save_and_load_roundtrip_is_camel_case_and_schema_v8() {
        let (directory, store) = store();
        let settings = SettingsView {
            receiver_name: Some("  客厅电脑  ".to_owned()),
            startup_enabled: false,
            auto_wake_enabled: false,
            advanced_effects_enabled: true,
            mi_play_enabled: false,
            mi_play_device_identity: "speaker".to_owned(),
            air_play_enabled: false,
            dlna_enabled: false,
            ..SettingsView::default()
        };
        store.save(&settings).expect("save");

        let text = fs::read_to_string(store.path()).expect("read settings");
        let json: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(json["schemaVersion"], 8);
        assert_eq!(json["receiverName"], "客厅电脑");
        assert_eq!(json["startupEnabled"], false);
        assert_eq!(json["autoWakeEnabled"], false);
        assert_eq!(json["advancedEffectsEnabled"], true);
        assert_eq!(json["miPlayEnabled"], false);
        assert_eq!(json["miPlayDeviceIdentity"], "speaker");
        assert_eq!(json["airPlayEnabled"], false);
        assert_eq!(json["dlnaEnabled"], false);
        // No temporary files are left behind.
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left behind");

        let loaded = store.load();
        assert_eq!(loaded, validate(settings));
    }

    #[test]
    fn save_creates_missing_data_directory() {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_dir = directory.path().join("nested/FusionPlay");
        let store = SettingsStore::new(data_dir.clone());
        store.save(&SettingsView::default()).expect("save");
        assert!(data_dir.join(SETTINGS_FILE_NAME).is_file());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_and_is_backed_up() {
        let (directory, store) = store();
        let corrupt = "{ this is not json";
        fs::write(store.path(), corrupt).expect("write corrupt");

        let loaded = store.load();
        assert_eq!(loaded, SettingsView::default());

        let backups: Vec<_> = fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().starts_with("settings.corrupt-"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(backups.len(), 1, "expected exactly one backup");
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), corrupt);
    }

    #[test]
    fn non_object_json_and_empty_file_are_treated_as_corrupt() {
        let (directory, store) = store();
        fs::write(store.path(), "[]").unwrap();
        assert_eq!(store.load(), SettingsView::default());
        fs::write(store.path(), "   \n").unwrap();
        assert_eq!(store.load(), SettingsView::default());
        let backups = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.corrupt-")
            })
            .count();
        assert!(backups >= 1);
    }

    #[test]
    fn partial_json_uses_field_defaults() {
        let (_directory, store) = store();
        fs::write(store.path(), r#"{"startupEnabled":false}"#).unwrap();
        let loaded = store.load();
        assert!(!loaded.startup_enabled);
        assert!(loaded.auto_wake_enabled);
        assert!(loaded.mi_play_enabled);
        assert_eq!(loaded.mi_play_device_identity, "television");
        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn validate_normalises_receiver_name_and_identity() {
        let validated = validate(SettingsView {
            receiver_name: Some("   ".to_owned()),
            mi_play_device_identity: " DISPLAY_SPEAKER ".to_owned(),
            schema_version: 3,
            ..SettingsView::default()
        });
        assert_eq!(validated.receiver_name, None);
        assert_eq!(validated.mi_play_device_identity, "speaker");
        assert_eq!(validated.schema_version, 8);

        let long_name = "音".repeat(80);
        let validated = validate(SettingsView {
            receiver_name: Some(long_name),
            mi_play_device_identity: "unknown-device".to_owned(),
            ..SettingsView::default()
        });
        assert_eq!(
            validated.receiver_name,
            Some("音".repeat(MAX_RECEIVER_NAME_LENGTH))
        );
        assert_eq!(validated.mi_play_device_identity, "television");
    }

    #[test]
    fn validate_accepts_every_documented_identity() {
        for identity in MIPLAY_DEVICE_IDENTITIES {
            assert!(is_supported_miplay_identity(identity));
            let validated = validate(SettingsView {
                mi_play_device_identity: identity.to_owned(),
                ..SettingsView::default()
            });
            assert_eq!(validated.mi_play_device_identity, identity);
        }
        // Legacy spelling is accepted and canonicalised.
        assert!(is_supported_miplay_identity("display_speaker"));
        let validated = validate(SettingsView {
            mi_play_device_identity: "display_speaker".to_owned(),
            ..SettingsView::default()
        });
        assert_eq!(validated.mi_play_device_identity, "speaker");
    }

    #[test]
    fn save_rejects_unknown_miplay_identity_without_touching_the_file() {
        let (directory, store) = store();
        store.save(&SettingsView::default()).expect("initial save");
        let before = fs::read_to_string(store.path()).unwrap();

        let error = store
            .save(&SettingsView {
                mi_play_device_identity: "toaster".to_owned(),
                ..SettingsView::default()
            })
            .expect_err("unknown identity must be rejected");
        assert!(error.to_string().contains("toaster"));
        assert_eq!(fs::read_to_string(store.path()).unwrap(), before);

        // No stray temporary file either.
        let leftovers = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn save_accepts_legacy_display_speaker_and_persists_speaker() {
        let (_directory, store) = store();
        store
            .save(&SettingsView {
                mi_play_device_identity: "display_speaker".to_owned(),
                ..SettingsView::default()
            })
            .expect("legacy identity save");
        assert_eq!(store.load().mi_play_device_identity, "speaker");
    }

    #[test]
    fn load_normalises_values_read_from_disk() {
        let (_directory, store) = store();
        fs::write(
            store.path(),
            r#"{"receiverName":"  hi  ","miPlayDeviceIdentity":"CAR","schemaVersion":2}"#,
        )
        .unwrap();
        let loaded = store.load();
        assert_eq!(loaded.receiver_name.as_deref(), Some("hi"));
        assert_eq!(loaded.mi_play_device_identity, "television");
        assert_eq!(loaded.schema_version, 8);
    }

    #[test]
    fn old_schema_versions_are_upgraded_to_current_schema() {
        let (_directory, store) = store();
        fs::write(store.path(), r#"{"schemaVersion":1,"dlnaEnabled":false}"#).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.schema_version, 8);
        assert!(!loaded.dlna_enabled);
    }

    #[test]
    fn concurrent_saves_leave_a_valid_file() {
        use std::sync::Arc;
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(SettingsStore::new(directory.path().to_path_buf()));
        let mut handles = Vec::new();
        for index in 0..8 {
            let store = Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                let settings = SettingsView {
                    receiver_name: Some(format!("receiver-{index}")),
                    ..SettingsView::default()
                };
                store.save(&settings).expect("save");
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let text = fs::read_to_string(store.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["schemaVersion"], 8);
        assert!(json["receiverName"].as_str().is_some());
    }
}
