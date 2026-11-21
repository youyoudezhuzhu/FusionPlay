//! Process-wide diagnostics for the UI thread and all receiver threads.
use crate::shared_logging::{DiagnosticFiles, MIB};
use anyhow::{Context, Result};
use std::{fs, path::Path, sync::OnceLock, time::Duration};
use tracing_subscriber::util::SubscriberInitExt;

static SINK: OnceLock<DiagnosticFiles> = OnceLock::new();

pub fn init(level: &str, path: &Path) -> Result<()> {
    let dir = path.parent().context("missing diagnostic directory")?;
    fs::create_dir_all(dir)?;
    let sink = DiagnosticFiles::open(path, 5 * MIB, MIB, true)?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(level))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(sink.clone())
        .finish()
        .try_init()?;
    let _ = SINK.set(sink.clone());
    let exports = dir
        .parent()
        .context("missing data directory")?
        .join("exports");
    let log_dir = dir.to_path_buf();
    let cleanup = move || {
        crate::windows_api::clean_exports(&exports);
        for name in ["player.log", "player.previous.log"] {
            let file = log_dir.join(name);
            if file.symlink_metadata().is_ok_and(|meta| {
                meta.is_file()
                    && meta
                        .modified()
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age >= crate::shared_logging::IDLE_RETENTION)
            }) {
                let _ = std::fs::remove_file(file);
            }
        }
    };
    cleanup();
    std::thread::Builder::new()
        .name("diagnostic-cleanup".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3600));
            let _ = sink.maintain();
            cleanup();
        })?;
    Ok(())
}

pub fn append_line(line: &str) {
    if let Some(sink) = SINK.get() {
        let _ = sink.append(
            format!("{} {line}\n", crate::ui::chrono_like_now()).as_bytes(),
            true,
        );
    }
}
