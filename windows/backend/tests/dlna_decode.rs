//! End-to-end tests for the Windows DLNA pull + decode path.
//!
//! These tests do **not** touch a sound card: [`AudioEngine::start_capture`]
//! records exactly what the real engine would have played, while a loopback
//! `TcpListener` serves generated WAV files over HTTP with `Content-Length`,
//! `Transfer-Encoding: chunked`, connection-close framing and redirects.
//!
//! Covered behaviour:
//! * 44.1 kHz stereo WAV over `Content-Length` -> >0 frames pushed at 48 kHz
//!   stereo, playback ends with `Playback(false)` + `Stopped`, no error.
//! * chunked and connection-close bodies are streamed correctly.
//! * HTTP 302 redirects are followed.
//! * HTTP 404 produces an explicit `DlnaEvent::Error`.
//! * `https://`, HLS, video and empty URIs fail synchronously in `play_track`.
//! * `stop()` aborts a blocked read within one second.
//! * An AirPlay takeover (engine active source change) stops DLNA promptly.
//! * `file://` playback from disk works.
//! * `seek()` restarts the stream near the requested position.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fusionplay_windows::audio::{AudioEngine, EngineConfig};
use fusionplay_windows::dlna_player::{DlnaEvent, DlnaPlayer, DlnaTrack};
use fusionplay_windows::session::Source;

// ---------------------------------------------------------------------------
// Harness: capture engine + event channel + HTTP server
// ---------------------------------------------------------------------------

struct Harness {
    engine: Arc<AudioEngine>,
    player: Arc<DlnaPlayer>,
    events: Receiver<DlnaEvent>,
}

impl Harness {
    fn new() -> Self {
        let engine = AudioEngine::start_capture(Arc::new(|_| {}));
        engine.set_active_source(Some(Source::Dlna));
        let (sender, events) = mpsc::channel();
        let player = DlnaPlayer::new(
            Arc::clone(&engine),
            Arc::new(move |event: DlnaEvent| {
                let _ = sender.send(event);
            }),
        );
        Self {
            engine,
            player,
            events,
        }
    }

    fn play(&self, url: impl Into<String>) -> anyhow::Result<()> {
        self.player.play_track(DlnaTrack {
            epoch: 7,
            url: url.into(),
            media_kind: "audio".to_string(),
            ..DlnaTrack::default()
        })
    }

    fn play_track(&self, track: DlnaTrack) -> anyhow::Result<()> {
        self.player.play_track(track)
    }

    /// Wait for a terminal `Stopped` event and return every event seen so far.
    fn wait_stopped(&self, timeout: Duration) -> Vec<DlnaEvent> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for DlnaEvent::Stopped; events: {seen:?}");
            }
            match self.events.recv_timeout(remaining) {
                Ok(event) => {
                    let stopped = matches!(event, DlnaEvent::Stopped { .. });
                    seen.push(event);
                    if stopped {
                        return seen;
                    }
                }
                Err(error) => {
                    panic!("timed out waiting for DlnaEvent::Stopped ({error}); events: {seen:?}")
                }
            }
        }
    }

    /// Wait until `playing = true` has been observed (or fail).
    fn wait_playing(&self, timeout: Duration) -> Vec<DlnaEvent> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for playback start; events: {seen:?}");
            }
            match self.events.recv_timeout(remaining) {
                Ok(event) => {
                    let playing = matches!(
                        event,
                        DlnaEvent::Playback {
                            playing: true,
                            ..
                        }
                    );
                    seen.push(event);
                    if playing {
                        return seen;
                    }
                }
                Err(error) => panic!("timed out waiting for playback start ({error})"),
            }
        }
    }

    fn expect_no_error(&self, events: &[DlnaEvent]) {
        if let Some(DlnaEvent::Error { message }) = events
            .iter()
            .find(|event| matches!(event, DlnaEvent::Error { .. }))
        {
            panic!("unexpected DLNA error: {message}");
        }
    }

    fn captured_frames(&self) -> usize {
        self.engine.captured().len() / 2
    }
}

struct TestServer {
    address: SocketAddr,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
}

/// Spawn a loopback HTTP server that serves exactly `connections` requests with
/// `handler`. The server thread is detached; a test that stops playback early
/// may leave it blocked in `accept` until the process exits, which is harmless.
fn spawn_server<F>(connections: usize, handler: F) -> TestServer
where
    F: Fn(&mut TcpStream) -> io::Result<()> + Send + Sync + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("local addr");
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = handler(&mut stream);
        }
    });
    TestServer { address }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    let _ = reader.read_line(&mut request_line);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line == "\r\n" {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    request_line
}

fn write_full_response(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn write_chunked_response(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    )?;
    for chunk in body.chunks(16 * 1024) {
        write!(stream, "{:X}\r\n", chunk.len())?;
        stream.write_all(chunk)?;
        stream.write_all(b"\r\n")?;
    }
    stream.write_all(b"0\r\n\r\n")?;
    stream.flush()
}

fn write_until_close_response(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n"
    )?;
    stream.write_all(body)?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// WAV synthesis
// ---------------------------------------------------------------------------

fn wav_bytes(sample_rate: u32, channels: u16, frames: usize, frequency: f32) -> Vec<u8> {
    let bits_per_sample = 16u16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_len = frames * block_align as usize;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    let mut phase = 0.0f32;
    let step = 2.0 * std::f32::consts::PI * frequency / sample_rate as f32;
    for _ in 0..frames {
        let value = (phase.sin() * 0.4 * i16::MAX as f32) as i16;
        phase += step;
        for _ in 0..channels {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[test]
fn serves_wav_over_http_content_length() {
    // 0.5 s of 44.1 kHz stereo audio -> ~24 000 frames at 48 kHz.
    let body = wav_bytes(44_100, 2, 22_050, 440.0);
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write_full_response(stream, "audio/wav", &body)
    });
    let harness = Harness::new();
    harness
        .play(server.url("/music/beep.wav"))
        .expect("play_track accepts a plain HTTP WAV");
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    assert!(events.iter().any(|event| matches!(
        event,
        DlnaEvent::Playback {
            playing: true,
            ..
        }
    )), "no playback start event: {events:?}");
    assert!(events.iter().any(|event| matches!(
        event,
        DlnaEvent::Playback {
            playing: false,
            ..
        }
    )), "no playback stop event: {events:?}");
    let frames = harness.captured_frames();
    assert!(
        (23_000..=24_300).contains(&frames),
        "unexpected resampled frame count: {frames}"
    );
    let captured = harness.engine.captured();
    assert!(rms(&captured) > 0.1, "captured audio is silent");
    // The decoder should have reported the track duration in progress events.
    assert!(
        events.iter().any(|event| matches!(
            event,
            DlnaEvent::Progress { position_ms, duration_ms, .. }
                if *duration_ms >= 400 && *duration_ms <= 700 && *position_ms > 0
        )),
        "progress did not report position/duration: {events:?}"
    );
}

#[test]
fn serves_wav_over_chunked_encoding() {
    // 32 kHz mono -> up-mixed to stereo and resampled to 48 kHz.
    let body = wav_bytes(32_000, 1, 16_000, 220.0);
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write_chunked_response(stream, "audio/wav", &body)
    });
    let harness = Harness::new();
    harness.play(server.url("/chunked.wav")).unwrap();
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    let captured = harness.engine.captured();
    assert!(!captured.is_empty());
    // Mono up-mix: both channels must carry the same signal.
    for frame in captured.chunks(2) {
        assert!((frame[0] - frame[1]).abs() < 1e-4);
    }
    let frames = captured.len() / 2;
    assert!(
        (20_000..=24_500).contains(&frames),
        "unexpected frame count for 0.5 s @ 32 kHz: {frames}"
    );
}

#[test]
fn serves_wav_until_connection_close() {
    let body = wav_bytes(48_000, 2, 9_600, 330.0);
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write_until_close_response(stream, "audio/wav", &body)
    });
    let harness = Harness::new();
    harness.play(server.url("/until-close.wav")).unwrap();
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    assert!(harness.captured_frames() >= 9_000);
}

#[test]
fn follows_http_redirects() {
    let body = wav_bytes(48_000, 2, 9_600, 550.0);
    let server = spawn_server(2, move |stream| {
        let request = read_request(stream);
        if request.starts_with("GET /real.wav") {
            write_full_response(stream, "audio/wav", &body)
        } else {
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: /real.wav\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )?;
            stream.flush()
        }
    });
    let harness = Harness::new();
    harness.play(server.url("/moved.wav")).unwrap();
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    assert!(harness.captured_frames() >= 9_000);
}

#[test]
fn http_error_is_reported_through_the_event_sink() {
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
        stream.flush()
    });
    let harness = Harness::new();
    harness.play(server.url("/missing.wav")).unwrap();
    let events = harness.wait_stopped(Duration::from_secs(10));
    let error = events
        .iter()
        .find_map(|event| match event {
            DlnaEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .expect("404 must produce a DlnaEvent::Error");
    assert!(error.contains("404"), "unexpected error text: {error}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DlnaEvent::Playback { playing: false, .. })),
        "error path must still finish with Playback(false)"
    );
}

#[test]
fn rejects_unsupported_media_synchronously() {
    let harness = Harness::new();
    let cases = [
        (
            "https://example.com/a.mp3".to_string(),
            "HTTPS",
            "audio",
        ),
        ("http://127.0.0.1:1/live.m3u8".to_string(), "HLS", "audio"),
        ("http://127.0.0.1:1/movie.mp4".to_string(), "视频", "video"),
        ("   ".to_string(), "为空", "audio"),
    ];
    for (url, expected, media_kind) in cases {
        let error = harness
            .play_track(DlnaTrack {
                url: url.clone(),
                media_kind: media_kind.to_string(),
                content_type: (media_kind == "video").then(|| "video/mp4".to_string()),
                ..DlnaTrack::default()
            })
            .expect_err(&format!("{url} should be rejected"));
        assert!(
            error.to_string().contains(expected),
            "{url}: expected {expected}, got {error}"
        );
        assert!(!harness.player.is_playing());
    }
    // Synchronous rejections must not have started a worker or emitted events.
    thread::sleep(Duration::from_millis(100));
    assert!(harness.events.try_recv().is_err());
}

#[test]
fn file_url_plays_from_disk() {
    let bytes = wav_bytes(44_100, 2, 22_050, 660.0);
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("local.wav");
    std::fs::write(&path, &bytes).expect("write wav");
    let url = format!("file://{}", path.display());
    let harness = Harness::new();
    harness.play(url).expect("file:// is supported");
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    let frames = harness.captured_frames();
    assert!(
        (23_000..=24_300).contains(&frames),
        "unexpected frame count: {frames}"
    );
}

#[test]
fn stop_aborts_a_blocked_read_within_a_second() {
    let body = wav_bytes(48_000, 2, 480_000, 440.0); // 10 s
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        // Enough for the probe plus a burst, then stall.
        stream.write_all(&body[..96 * 1024])?;
        stream.flush()?;
        thread::sleep(Duration::from_secs(3));
        Ok(())
    });
    let harness = Harness::new();
    harness.play(server.url("/stalled.wav")).unwrap();
    harness.wait_playing(Duration::from_secs(10));

    let started = Instant::now();
    harness.player.stop();
    let mut exited = false;
    while started.elapsed() < Duration::from_millis(1_000) {
        if !harness.player.is_playing() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        exited,
        "DLNA worker did not exit within 1 s after stop()"
    );
    let events = harness.wait_stopped(Duration::from_secs(2));
    assert!(events.iter().any(|event| matches!(event, DlnaEvent::Stopped { .. })));
}

#[test]
fn airplay_takeover_stops_dlna_promptly() {
    let body = wav_bytes(44_100, 2, 44_100, 440.0); // 1 s
    let server = spawn_server(1, move |stream| {
        read_request(stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )?;
        // Throttle to ~2x real time so playback is still in progress when the
        // takeover happens.
        for chunk in body.chunks(8 * 1024) {
            write!(stream, "{:X}\r\n", chunk.len())?;
            stream.write_all(chunk)?;
            stream.write_all(b"\r\n")?;
            stream.flush()?;
            thread::sleep(Duration::from_millis(10));
        }
        stream.write_all(b"0\r\n\r\n")?;
        stream.flush()
    });
    let harness = Harness::new();
    harness.play(server.url("/long.wav")).unwrap();
    harness.wait_playing(Duration::from_secs(10));

    // Simulate AirPlay claiming the output (protocols.rs does this through the
    // arbiter -> AudioEngine::set_active_source).
    harness.engine.set_active_source(Some(Source::AirPlay));
    let started = Instant::now();
    let events = harness.wait_stopped(Duration::from_secs(3));
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "takeover took too long: {:?}",
        started.elapsed()
    );
    assert!(events.iter().any(|event| matches!(event, DlnaEvent::Stopped { .. })));
    assert!(!harness.player.is_playing());
}

#[test]
fn seek_restarts_near_the_requested_position() {
    // 0.8 s @ 44.1 kHz = 35 280 frames (~141 KiB); throttled to ~200 KiB/s.
    let body = wav_bytes(44_100, 2, 35_280, 440.0);
    let server = spawn_server(2, move |stream| {
        read_request(stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )?;
        for chunk in body.chunks(4 * 1024) {
            if write!(stream, "{:X}\r\n", chunk.len()).is_err() {
                return Ok(());
            }
            if stream.write_all(chunk).is_err() {
                return Ok(());
            }
            if stream.write_all(b"\r\n").is_err() {
                return Ok(());
            }
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(20));
        }
        stream.write_all(b"0\r\n\r\n")?;
        stream.flush()
    });
    let harness = Harness::new();
    harness.play(server.url("/seekable.wav")).unwrap();
    harness.wait_playing(Duration::from_secs(10));
    // Drop audio queued before the seek so the assertion only counts frames
    // produced by the restarted stream.
    harness.engine.clear_captured();
    harness.player.seek(400).expect("seek on a playing track");
    let events = harness.wait_stopped(Duration::from_secs(10));
    harness.expect_no_error(&events);
    let frames = harness.captured_frames();
    // Remaining 0.4 s at 48 kHz ≈ 19 200 frames (tolerant because the old
    // worker may have queued a last block before it observed the abort).
    assert!(
        (10_000..=26_000).contains(&frames),
        "unexpected frame count after seek to 400 ms: {frames}"
    );
}

#[test]
fn null_audio_engine_status_is_sane_without_hardware() {
    let engine = AudioEngine::start(
        EngineConfig {
            null_audio: true,
            output_device: None,
        },
        Arc::new(|_| {}),
    )
    .expect("null engine starts");
    let status = engine.status();
    assert_eq!(status.mode, "null");
    assert_eq!(status.sample_rate, 48_000);
    assert_eq!(status.channels, 2);
    assert_eq!(status.underruns, 0);
}
