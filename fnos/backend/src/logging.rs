//! File logging with bounded size and an in-place tail trim.
//!
//! Renders `tracing` events (from our code and the reused protocol crates) to
//! `$TRIM_PKGVAR/fusionplay.log`. Rotation rewrites the same inode (tail-trim),
//! so a long-running daemon never needs a restart and keeps its fd.

use crate::config::SpeakerInstance;
use crate::shared_logging::{self, DiagnosticFiles, MIB};
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Registry;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

static LEVEL: AtomicU8 = AtomicU8::new(2); // 0=error 1=warn 2=info 3=debug 4=trace

/// Runtime-adjustable global level filter (the Web UI can change it without a
/// restart; `tracing` has no built-in runtime level switch without `reload`).
struct DynamicLevel;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for DynamicLevel {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>, _context: LayerContext<'_, S>) -> bool {
        let max = LEVEL.load(Ordering::Relaxed);
        let needed = match *metadata.level() {
            tracing::Level::ERROR => 0,
            tracing::Level::WARN => 1,
            tracing::Level::INFO => 2,
            tracing::Level::DEBUG => 3,
            tracing::Level::TRACE => 4,
        };
        needed <= max
    }
}

pub fn path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}

fn level_from_str(level: &str) -> (u8, &'static str) {
    match level.trim().to_ascii_lowercase().as_str() {
        "error" => (0, "error"),
        "warn" => (1, "warn"),
        "debug" => (3, "debug"),
        "trace" => (4, "trace"),
        _ => (2, "info"),
    }
}

pub fn init(dir: &Path, level: &str, max_bytes: u64, keep_bytes: u64) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("无法创建日志目录 {}", dir.display()))?;
    let log_file = dir.join("fusionplay.log");
    let worker = std::env::var_os("FUSIONPLAY_PARENT_PID").is_some();
    let max = max_bytes.clamp(MIB, if worker { MIB } else { 5 * MIB });
    let keep = keep_bytes.clamp(64 * 1024, max / 2);
    let sink = DiagnosticFiles::open(&log_file, max, keep, false)
        .with_context(|| format!("无法打开日志 {}", log_file.display()))?;
    let (numeric, _) = level_from_str(level);
    LEVEL.store(numeric, Ordering::Relaxed);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(sink.clone());
    Registry::default()
        .with(DynamicLevel)
        .with(file_layer)
        .try_init()?;
    LOG_PATH.set(log_file).ok();
    let lifecycle = dir.join("app.log");
    std::thread::Builder::new()
        .name("log-housekeeping".into())
        .spawn(move || loop {
            let _ = sink.maintain();
            if !worker {
                // fnOS and startup stderr keep the original inode open for append.
                if let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&lifecycle) {
                    let _ = shared_logging::trim(&mut file, 5 * MIB, MIB);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
        })?;
    Ok(())
}

pub fn set_level(level: &str) -> Result<()> {
    let (numeric, _name) = level_from_str(level);
    LEVEL.store(numeric, Ordering::Relaxed);
    Ok(())
}

pub fn level_name() -> &'static str {
    match LEVEL.load(Ordering::Relaxed) {
        0 => "error",
        1 => "warn",
        3 => "debug",
        4 => "trace",
        _ => "info",
    }
}

pub fn recent_lines(lines: usize) -> Vec<String> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(bytes) = shared_logging::read_tail(&path, 256 * 1024) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut rows: Vec<_> = text.lines().rev().take(lines).map(str::to_owned).collect();
    rows.reverse();
    rows
}

/// config.validate restricts instance IDs to single path components.
pub fn export_report(dir: &Path, speakers: &[SpeakerInstance]) -> String {
    let budget = (2 * MIB / (2 * speakers.len() as u64 + 3)).min(256 * 1024);
    let mut report = format!(
        "FusionPlay fnOS {} ({})\n",
        crate::VERSION,
        std::env::consts::ARCH
    );
    let mut section = |label: &str, file: PathBuf| {
        report.push_str(&format!("\n--- {label} ---\n"));
        match shared_logging::read_tail(&file, budget) {
            Ok(data) => report.push_str(&shared_logging::redact(&String::from_utf8_lossy(&data))),
            Err(e) => report.push_str(&format!("无法读取日志 ({})", e.kind())),
        }
        report.push('\n');
    };
    section("服务启停 / stderr", dir.join("app.log"));
    section("主接收器 WARN / ERROR", dir.join("errors.log"));
    section("主接收器", dir.join("fusionplay.log"));
    for speaker in speakers {
        section(
            &format!("音箱 {} ({}) WARN / ERROR", speaker.name, speaker.id),
            dir.join("instances").join(&speaker.id).join("errors.log"),
        );
        section(
            &format!("音箱 {} ({})", speaker.name, speaker.id),
            dir.join("instances")
                .join(&speaker.id)
                .join("fusionplay.log"),
        );
    }
    report
}

pub fn clean_retired_logs(dir: &Path, speakers: &[SpeakerInstance]) {
    let Ok(entries) = dir.join("instances").read_dir() else {
        return;
    };
    let mut retired = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ty| ty.is_dir())
            || speakers.iter().any(|s| entry.file_name() == s.id.as_str())
        {
            continue;
        }
        for name in ["fusionplay.log", "errors.log"] {
            let log = entry.path().join(name);
            if let Ok(meta) = log.symlink_metadata() {
                if meta.is_file() {
                    if let Ok(time) = meta.modified() {
                        retired.push((time, log, meta.len()));
                    }
                }
            }
        }
    }
    retired.sort_unstable_by(|a, b| b.cmp(a));
    let mut used = 0;
    for (time, path, size) in retired {
        if time
            .elapsed()
            .is_ok_and(|age| age >= shared_logging::IDLE_RETENTION)
            || used + size > 4 * MIB
        {
            let _ = std::fs::remove_file(path);
        } else {
            used += size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};
    #[test]
    fn export_includes_room_and_startup_errors_even_after_a_long_utf8_log() {
        let root = tempfile::tempdir().unwrap();
        let room = root.path().join("instances/kitchen");
        fs::create_dir_all(&room).unwrap();
        fs::write(root.path().join("app.log"), "startup failure").unwrap();
        fs::write(
            root.path().join("fusionplay.log"),
            "曲".repeat(140000) + "新事件",
        )
        .unwrap();
        fs::write(room.join("errors.log"), "ALSA open failed").unwrap();
        let speaker = SpeakerInstance {
            id: "kitchen".into(),
            name: "厨房".into(),
            device_id: "usb".into(),
        };
        let report = export_report(root.path(), &[speaker]);
        for expected in [
            "startup failure",
            "新事件",
            "厨房 (kitchen)",
            "ALSA open failed",
        ] {
            assert!(report.contains(expected), "{expected}");
        }
        assert!(report.len() < 300 * 1024);
    }

    #[test]
    fn retired_logs_expire_without_removing_speaker_identity_or_active_logs() {
        let root = tempfile::tempdir().unwrap();
        let active = SpeakerInstance {
            id: "live".into(),
            name: "live".into(),
            device_id: "usb".into(),
        };
        for room in ["live", "old"] {
            let path = root.path().join("instances").join(room);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("config.json"), "{}").unwrap();
            let log = std::fs::File::create(path.join("errors.log")).unwrap();
            log.set_modified(SystemTime::now() - shared_logging::IDLE_RETENTION)
                .unwrap();
        }
        clean_retired_logs(root.path(), &[active]);
        assert!(root.path().join("instances/live/errors.log").exists());
        assert!(root.path().join("instances/old/config.json").exists());
        assert!(!root.path().join("instances/old/errors.log").exists());
    }
}
