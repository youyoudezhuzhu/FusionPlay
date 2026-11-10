//! File + stderr logging with size-bounded rotation.
//!
//! The UI receives user-facing logs through [`crate::ui::UiSink`]; this module
//! is for full diagnostic logs (`%LOCALAPPDATA%\FusionPlay\logs\fusionplay.log`)
//! that `exportLogs` bundles.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};

/// Rotate once the log file exceeds this size, keeping the newest tail only.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// When rotating, keep the newest portion (in bytes) of the file.
pub const KEEP_TAIL_BYTES: u64 = 1024 * 1024;

static LOG_SINK: OnceLock<Arc<LogFile>> = OnceLock::new();

struct LogFile {
    path: PathBuf,
    file: Mutex<File>,
    written: Mutex<u64>,
}

impl LogFile {
    fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("无法打开日志文件 {}", path.display()))?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            written: Mutex::new(written),
        })
    }

    fn append(&self, line: &str) {
        let bytes = line.as_bytes();
        let Ok(mut file) = self.file.lock() else {
            return;
        };
        let Ok(mut written) = self.written.lock() else {
            return;
        };
        if *written + bytes.len() as u64 > MAX_LOG_BYTES {
            drop(written);
            drop(file);
            if self.rotate().is_ok()
                && let Ok(mut written) = self.written.lock()
            {
                *written = 0;
            }
            if let Ok(mut file) = self.file.lock() {
                let _ = file.write_all(bytes);
            }
            return;
        }
        let _ = file.write_all(bytes);
        let _ = file.flush();
        *written += bytes.len() as u64;
    }

    fn rotate(&self) -> Result<()> {
        let full = fs::read(&self.path).unwrap_or_default();
        let start = full.len().saturating_sub(KEEP_TAIL_BYTES as usize);
        let tail = &full[start..];
        let mut file = File::create(&self.path)?;
        file.write_all(tail)?;
        if let Ok(mut handle) = self.file.lock()
            && let Ok(reopened) = OpenOptions::new().append(true).open(&self.path)
        {
            *handle = reopened;
        }
        Ok(())
    }
}

/// Thread-safe writer handed to `tracing`.
#[derive(Clone)]
pub struct LogWriter {
    sink: Option<Arc<LogFile>>,
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // tracing writes one event per `write` call in practice; forward as-is
        // after making sure the line ends with a newline.
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if let Some(sink) = &self.sink {
            sink.append(&text);
        } else if let Some(sink) = LOG_SINK.get() {
            sink.append(&text);
        }
        let _ = io::stderr().write_all(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        Ok(())
    }
}

/// Installs the global tracing subscriber. The returned guard keeps the file
/// writer alive (and is intentionally opaque).
pub fn init(level: &str, log_file: &Path) -> Result<Option<tracing::subscriber::DefaultGuard>> {
    let sink = Arc::new(LogFile::open(log_file)?);
    let _ = LOG_SINK.set(Arc::clone(&sink));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(level))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false)
        .with_writer(move || LogWriter {
            sink: Some(Arc::clone(&sink)),
        })
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    Ok(Some(guard))
}

/// Append a line to the shared diagnostic log (also used for non-tracing
/// diagnostics such as command errors).
pub fn append_line(line: &str) {
    if let Some(sink) = LOG_SINK.get() {
        sink.append(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_writes_lines_and_rotation_keeps_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/fusionplay.log");
        let sink = Arc::new(LogFile::open(&path).unwrap());
        sink.append("first line\n");
        sink.append("second line\n");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("first line"));
        assert!(text.contains("second line"));

        // Force a rotation by faking a huge existing size.
        {
            let mut written = sink.written.lock().unwrap();
            *written = MAX_LOG_BYTES;
        }
        sink.append("after rotation\n");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("after rotation"));
        assert!(text.len() as u64 <= KEEP_TAIL_BYTES + 64);
    }
}
