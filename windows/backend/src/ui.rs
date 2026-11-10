//! NDJSON writer used by `main.rs`.
//!
//! All sidecar → UI events pass through [`UiSink`]. [`StdioUiSink`] serializes
//! events to stdout from a dedicated writer thread and coalesces `app_state`
//! updates to at most one every [`STATE_MIN_INTERVAL`] (streaming progress
//! events arrive far faster than a UI can repaint).

use std::io::Write;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ui_protocol::{AppStateView, LogLevel, ReadyInfo, UiEvent};

/// Minimum spacing between two `app_state` lines (10 Hz).
pub const STATE_MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Sink contract used by the runtime. Implementations must be cheap to call
/// from protocol/event threads and must never block the audio path.
pub trait UiSink: Send + Sync {
    fn ready(&self, info: ReadyInfo);
    fn publish(&self, state: AppStateView);
    fn log(&self, level: LogLevel, source: &str, message: &str);
    fn response(&self, request_id: Option<&str>, command: &str, ok: bool, message: Option<&str>);
    fn fatal(&self, message: &str, exit_code: i32);
    fn exiting(&self, reason: &str);
}

/// Newline-delimited JSON writer over any `Write` (stdout in production).
pub struct JsonLineWriter<W: Write + Send + 'static> {
    sender: Sender<WriterMessage>,
    thread: Mutex<Option<JoinHandle<()>>>,
    _marker: std::marker::PhantomData<W>,
}

enum WriterMessage {
    Line(String),
    Flush(Sender<()>),
    Shutdown,
}

impl<W: Write + Send + 'static> JsonLineWriter<W> {
    pub fn new(mut output: W) -> Self {
        let (sender, receiver) = mpsc::channel::<WriterMessage>();
        let thread = thread::Builder::new()
            .name("fusionplay-ui-writer".to_owned())
            .spawn(move || writer_loop(&mut output, &receiver))
            .expect("无法启动 UI 输出线程");
        Self {
            sender,
            thread: Mutex::new(Some(thread)),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn send(&self, line: String) {
        let _ = self.sender.send(WriterMessage::Line(line));
    }

    /// Block until everything queued so far has been written.
    pub fn flush(&self) {
        let (done_sender, done_receiver) = mpsc::channel();
        if self.sender.send(WriterMessage::Flush(done_sender)).is_ok() {
            let _ = done_receiver.recv_timeout(Duration::from_secs(5));
        }
    }

    /// Flush outstanding lines, stop the writer thread and join it.
    pub fn shutdown(&self) {
        self.flush();
        let _ = self.sender.send(WriterMessage::Shutdown);
        if let Some(thread) = self.thread.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = thread.join();
        }
    }
}

impl<W: Write + Send + 'static> Drop for JsonLineWriter<W> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn writer_loop<W: Write>(output: &mut W, receiver: &Receiver<WriterMessage>) {
    loop {
        match receiver.recv() {
            Ok(WriterMessage::Line(line)) => {
                if writeln!(output, "{line}").is_err() {
                    return;
                }
                let _ = output.flush();
            }
            Ok(WriterMessage::Flush(done)) => {
                let _ = output.flush();
                let _ = done.send(());
            }
            Ok(WriterMessage::Shutdown) | Err(_) => {
                let _ = output.flush();
                return;
            }
        }
    }
}

/// [`UiSink`] that talks NDJSON to the Flutter UI on stdout.
pub struct StdioUiSink {
    writer: Arc<JsonLineWriter<std::io::Stdout>>,
    state: Mutex<StateCoalescer>,
}

struct StateCoalescer {
    latest: Option<AppStateView>,
    last_sent: Option<Instant>,
}

impl StdioUiSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            writer: Arc::new(JsonLineWriter::new(std::io::stdout())),
            state: Mutex::new(StateCoalescer {
                latest: None,
                last_sent: None,
            }),
        })
    }

    pub fn writer(&self) -> Arc<JsonLineWriter<std::io::Stdout>> {
        Arc::clone(&self.writer)
    }

    fn send_event(&self, event: &UiEvent) {
        match serde_json::to_string(event) {
            Ok(line) => self.writer.send(line),
            Err(error) => {
                // Last-resort diagnostics: the event stream must not panic.
                let _ = writeln!(std::io::stderr(), "无法序列化 UI 事件：{error}");
            }
        }
    }

    fn flush_state(&self) {
        if let Ok(mut coalescer) = self.state.lock()
            && let Some(state) = coalescer.latest.take()
        {
            coalescer.last_sent = Some(Instant::now());
            drop(coalescer);
            self.send_event(&UiEvent::AppState {
                state: Box::new(state),
            });
        }
    }
}

impl UiSink for StdioUiSink {
    fn ready(&self, info: ReadyInfo) {
        self.send_event(&UiEvent::Ready {
            app: info.app,
            role: info.role,
            version: info.version,
            data_dir: info.data_dir,
            log_file: info.log_file,
            null_audio: info.null_audio,
        });
        self.writer.flush();
    }

    fn publish(&self, state: AppStateView) {
        let ready_to_send = match self.state.lock() {
            Ok(mut coalescer) => {
                coalescer.latest = Some(state);
                let now = Instant::now();
                match coalescer.last_sent {
                    Some(last) if now.duration_since(last) < STATE_MIN_INTERVAL => false,
                    _ => {
                        coalescer.last_sent = Some(now);
                        true
                    }
                }
            }
            Err(_) => false,
        };
        if ready_to_send {
            self.flush_state();
        }
    }

    fn log(&self, level: LogLevel, source: &str, message: &str) {
        self.send_event(&UiEvent::Log {
            timestamp: chrono_like_now(),
            level,
            source: source.to_owned(),
            message: message.to_owned(),
        });
    }

    fn response(&self, request_id: Option<&str>, command: &str, ok: bool, message: Option<&str>) {
        self.send_event(&UiEvent::Response {
            request_id: request_id.map(str::to_owned),
            command: command.to_owned(),
            ok,
            message: message.map(str::to_owned),
        });
    }

    fn fatal(&self, message: &str, exit_code: i32) {
        self.send_event(&UiEvent::Fatal {
            message: message.to_owned(),
            exit_code,
        });
        self.writer.flush();
    }

    fn exiting(&self, reason: &str) {
        self.flush_state();
        self.send_event(&UiEvent::Exiting {
            reason: reason.to_owned(),
        });
        self.writer.flush();
    }
}

/// ISO-8601 local timestamp without pulling in a date-time crate.
///
/// Windows UI only displays the value in diagnostics; seconds precision is
/// enough and the offset is reported by the OS.
pub fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs();
    // Days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescer_keeps_only_the_latest_pending_state() {
        let sink = StdioUiSink::new();
        // Force the "inside the coalescing window" branch deterministically:
        // without this a slow test machine could flush every state immediately.
        let state_with_port = |port: u16| AppStateView {
            receiver_port: Some(port),
            ..AppStateView::default()
        };
        for port in [7000_u16, 7001, 7002] {
            {
                let mut coalescer = sink.state.lock().unwrap();
                coalescer.last_sent = Some(Instant::now());
            }
            sink.publish(state_with_port(port));
        }
        let coalescer = sink.state.lock().unwrap();
        assert_eq!(
            coalescer.latest.as_ref().and_then(|state| state.receiver_port),
            Some(7002)
        );
    }

    #[test]
    fn timestamps_are_iso_8601_utc() {
        let stamp = chrono_like_now();
        assert_eq!(stamp.len(), 20, "unexpected timestamp format: {stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
    }

    #[test]
    fn json_line_writer_flushes_and_joins() {
        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Buffer(Arc::new(Mutex::new(Vec::new())));
        let writer = JsonLineWriter::new(buffer.clone());
        writer.send("{\"a\":1}".to_owned());
        writer.send("{\"b\":2}".to_owned());
        writer.flush();
        let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "{\"a\":1}\n{\"b\":2}\n");
        writer.shutdown();
    }
}
