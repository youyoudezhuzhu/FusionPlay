//! Bounded diagnostic I/O for the Windows and fnOS receiver processes.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
    time::Duration,
};

pub const MIB: u64 = 1024 * 1024;
pub const IDLE_RETENTION: Duration = Duration::from_secs(7 * 86400);
const MAX_RECORD: usize = 16 * 1024;

pub fn read_tail(path: &Path, bytes: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    tail(&mut file, bytes)
}

fn tail(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let end = file.metadata()?.len();
    let start = end.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end - start).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn trim(file: &mut File, max: u64, keep: u64) -> io::Result<()> {
    if file.metadata()?.len() > max {
        let rest = tail(file, keep)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&rest)?;
    }
    Ok(())
}

pub struct BoundedLog {
    file: Mutex<File>,
    max: u64,
    keep: u64,
}

impl BoundedLog {
    pub fn open(path: &Path, max: u64, keep: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let sink = Self {
            file: Mutex::new(file),
            max,
            keep,
        };
        sink.maintain()?;
        Ok(sink)
    }

    pub fn maintain(&self) -> io::Result<()> {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if file
            .metadata()?
            .modified()?
            .elapsed()
            .is_ok_and(|age| age >= IDLE_RETENTION)
        {
            file.set_len(0)?;
        }
        trim(&mut file, self.max, self.keep)
    }

    pub fn append(&self, bytes: &[u8]) -> io::Result<()> {
        let limit = MAX_RECORD.min((self.max - self.keep) as usize);
        let suffix = b" [truncated]\n";
        let truncated = bytes.len() > limit;
        let line = if truncated {
            &bytes[..limit - suffix.len()]
        } else {
            bytes
        };
        let size = line.len() + if truncated { suffix.len() } else { 0 };
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        trim(&mut file, self.max - size as u64, self.keep)?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(line)?;
        if truncated {
            file.write_all(suffix)?;
        }
        Ok(())
    }
}

/// Keep warning/error context when verbose protocol traffic fills the main log.
#[derive(Clone)]
pub struct DiagnosticFiles {
    main: std::sync::Arc<BoundedLog>,
    errors: std::sync::Arc<BoundedLog>,
    mirror_stderr: bool,
}

impl DiagnosticFiles {
    pub fn open(path: &Path, max: u64, keep: u64, mirror_stderr: bool) -> io::Result<Self> {
        Ok(Self {
            main: BoundedLog::open(path, max, keep)?.into(),
            errors: BoundedLog::open(&path.with_file_name("errors.log"), MIB / 4, MIB / 16)?.into(),
            mirror_stderr,
        })
    }

    pub fn maintain(&self) -> io::Result<()> {
        self.main.maintain()?;
        self.errors.maintain()
    }

    pub fn append(&self, bytes: &[u8], important: bool) -> io::Result<()> {
        let history = if important {
            self.errors.append(bytes)
        } else {
            Ok(())
        };
        self.main.append(bytes).and(history)
    }
}

pub struct EventWriter<'a> {
    files: &'a DiagnosticFiles,
    important: bool,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DiagnosticFiles {
    type Writer = EventWriter<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            files: self,
            important: false,
        }
    }
    fn make_writer_for(&'a self, metadata: &tracing::Metadata<'_>) -> Self::Writer {
        EventWriter {
            files: self,
            important: *metadata.level() <= tracing::Level::WARN,
        }
    }
}

impl Write for EventWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.files.append(bytes, self.important)?;
        if self.files.mirror_stderr {
            let _ = io::stderr().write_all(bytes);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_error_survives_info_flood_and_preserves_inode() {
        use tracing_subscriber::fmt::MakeWriter;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fusionplay.log");
        let files = DiagnosticFiles::open(&path, 65536, 8192, false).unwrap();
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(&path.metadata().unwrap());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(files.clone())
            .finish();
        std::thread::spawn(move || {
            tracing::subscriber::with_default(subscriber, || {
                tracing::error!("receiver-error-42");
                tracing::info!("noise");
            })
        })
        .join()
        .unwrap();
        for _ in 0..40 {
            files.make_writer().write_all(&[b'n'; 16000]).unwrap();
        }
        let kept = std::fs::read_to_string(dir.path().join("errors.log")).unwrap();
        assert!(kept.contains("receiver-error-42") && !kept.contains("noise"));
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("receiver-error-42"));
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&path.metadata().unwrap()),
            inode
        );
    }

    #[test]
    fn bounded_concurrent_writes_and_idle_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("trace");
        let log = BoundedLog::open(&path, 65536, 8192).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..40 {
                        log.append(&[b'x'; 10000]).unwrap();
                    }
                });
            }
        });
        log.append(&[b'y'; 90000]).unwrap();
        log.append(b"last error\n").unwrap();
        assert!(path.metadata().unwrap().len() <= 65536);
        assert!(read_tail(&path, 40)
            .unwrap()
            .ends_with(b"[truncated]\nlast error\n"));
        drop(log);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - IDLE_RETENTION)
            .unwrap();
        let _reopened = BoundedLog::open(&path, 65536, 8192).unwrap();
        assert_eq!(path.metadata().unwrap().len(), 0);
    }
}
