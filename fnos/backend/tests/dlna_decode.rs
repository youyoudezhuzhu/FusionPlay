//! End-to-end tests for the fnOS DLNA pull + decode path.
//!
//! These tests do **not** touch a sound card: a recording `OutputBackend`
//! captures exactly what the real `AudioEngine` mixes, while a loopback
//! `TcpListener` serves generated WAV files over HTTP with both
//! `Content-Length` and `Transfer-Encoding: chunked` framing.
//!
//! Covered behaviour:
//! * 44.1 kHz stereo WAV over `Content-Length` -> >0 frames pushed at 48 kHz
//!   stereo, playback ends `idle`, no error, arbiter released.
//! * 32 kHz mono WAV over chunked encoding -> mono is up-mixed (both channels
//!   equal) and resampled to 48 kHz.
//! * HTTP 404, truncated body, stalled server (timeout), `https://`, empty and
//!   HLS URIs all fail explicitly and land in `last_error`.
//! * AirPlay takeover and an explicit `stop()` stop decoding promptly and stop
//!   feeding the engine.
//! * `file://` playback from disk (WAV, FLV-AAC, FLV-MP3, ALAC, AIFF).
//! * A CDN that answers 403 to the browser UA is retried once with the legacy
//!   `FusionPlay-fnOS/<version>` UA.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fusionplay_fnos::audio::{
    AudioEngine, DeviceInfo, EngineStatus, NullBackend, OutputBackend,
};
use fusionplay_fnos::config::AudioSettings;
use fusionplay_fnos::dlna_player::{DlnaPlayer, DlnaTrack};
use fusionplay_fnos::session::{Arbiter, Source};
use fusionplay_fnos::state::{AppState, SharedState};

// ---------------------------------------------------------------------------
// Recording backend: captures the engine mix without hardware.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecordingBackend {
    frames: AtomicU64,
    writes: AtomicU64,
    flushes: AtomicU64,
    bad_format: AtomicBool,
    peak_bits: AtomicU64,
    stereo_checked: AtomicU64,
    stereo_unequal: AtomicU64,
}

impl RecordingBackend {
    fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    fn flushes(&self) -> u64 {
        self.flushes.load(Ordering::Relaxed)
    }

    fn peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.load(Ordering::Relaxed) as u32)
    }

    fn assert_48k_stereo(&self) {
        assert!(
            !self.bad_format.load(Ordering::Relaxed),
            "engine received PCM that is not 48 kHz stereo"
        );
    }
}

impl OutputBackend for RecordingBackend {
    fn configure(&self, _settings: &AudioSettings) -> anyhow::Result<()> {
        Ok(())
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            mode: "recording".to_string(),
            source: Some(Source::Dlna.as_str().to_string()),
            master_frames: self.frames(),
            dropped_frames: 0,
            devices: Vec::new(),
        }
    }

    fn write(
        &self,
        source: Source,
        _epoch: u64,
        _sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        assert_eq!(source, Source::Dlna);
        if sample_rate != 48_000 || channels != 2 {
            self.bad_format.store(true, Ordering::Relaxed);
            return;
        }
        self.frames
            .fetch_add((samples.len() / 2) as u64, Ordering::Relaxed);
        self.writes.fetch_add(1, Ordering::Relaxed);
        let mut peak = self.peak();
        for frame in samples.chunks(2) {
            peak = peak.max(frame[0].abs()).max(frame[1].abs());
            self.stereo_checked.fetch_add(1, Ordering::Relaxed);
            if (frame[0] - frame[1]).abs() > 1e-5 {
                self.stereo_unequal.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.peak_bits.store(u64::from(peak.to_bits()), Ordering::Relaxed);
    }

    fn flush(&self, source: Source) {
        if source == Source::Dlna {
            self.flushes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn test_tone(&self, _device_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    backend: Arc<RecordingBackend>,
    arbiter: Arc<Arbiter>,
    state: SharedState,
    player: Arc<DlnaPlayer>,
}

fn harness() -> Harness {
    let backend = Arc::new(RecordingBackend::default());
    let engine = AudioEngine::new(AudioSettings::default(), backend.clone()).unwrap();
    let arbiter = Arc::new(Arbiter::new());
    let state = AppState::new();
    let player = DlnaPlayer::new(
        Arc::clone(&engine),
        Arc::clone(&arbiter),
        Arc::clone(&state),
    );
    Harness {
        backend,
        arbiter,
        state,
        player,
    }
}

// ---------------------------------------------------------------------------
// WAV synthesis (hand-written RIFF header, no extra dependencies)
// ---------------------------------------------------------------------------

fn wav_bytes(rate: u32, channels: u16, seconds: f64) -> Vec<u8> {
    assert!(channels >= 1);
    let frames = (rate as f64 * seconds).round() as u32;
    let bits_per_sample = 16u16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = rate * u32::from(block_align);
    let data_len = frames * u32::from(block_align);

    let mut wav = Vec::with_capacity(44 + data_len as usize);
    // RIFF header.
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk (PCM).
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data chunk.
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    for frame in 0..frames {
        let t = f64::from(frame) / f64::from(rate);
        let wave = (t * 440.0 * std::f64::consts::TAU).sin() * 0.4;
        for channel in 0..channels {
            let amplitude = if channel == 0 { wave } else { wave * 0.5 };
            let sample = (amplitude * f64::from(i16::MAX)) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
    }
    wav
}

// ---------------------------------------------------------------------------
// Loopback HTTP server helpers
// ---------------------------------------------------------------------------

fn read_request(mut stream: &TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    request
}

fn listen() -> TcpListener {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener
        .set_nonblocking(false)
        .expect("loopback listener should block");
    listener
}

/// Serve one request with `Content-Length` framing.
fn serve_content_length(body: Vec<u8>, content_type: &'static str) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });
    (port, handle)
}

/// Same as [`serve_content_length`], but sends the first `first_bytes` bytes
/// immediately, waits, and only then sends the rest. Used to prove playback
/// starts from a stream instead of buffering the whole body first.
fn serve_content_length_slow_tail(
    body: Vec<u8>,
    first_bytes: usize,
    delay: Duration,
) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        let split = first_bytes.min(body.len());
        stream.write_all(&body[..split]).unwrap();
        stream.flush().unwrap();
        thread::sleep(delay);
        let _ = stream.write_all(&body[split..]);
    });
    (port, handle)
}

/// Serve one request with `Transfer-Encoding: chunked` framing.
fn serve_chunked(body: Vec<u8>, chunk_size: usize) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let header = "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        stream.write_all(header.as_bytes()).unwrap();
        for chunk in body.chunks(chunk_size.max(1)) {
            let mut framed = format!("{:x}\r\n", chunk.len()).into_bytes();
            framed.extend_from_slice(chunk);
            framed.extend_from_slice(b"\r\n");
            if stream.write_all(&framed).is_err() {
                return;
            }
        }
        stream.write_all(b"0\r\n\r\n").unwrap();
    });
    (port, handle)
}

/// Slow chunked server: used to observe a running playback and stop/take over
/// while the body is still being transferred.
fn serve_chunked_slow(
    body: Vec<u8>,
    chunk_size: usize,
    delay: Duration,
) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let header = "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        if stream.write_all(header.as_bytes()).is_err() {
            return;
        }
        for chunk in body.chunks(chunk_size.max(1)) {
            let mut framed = format!("{:x}\r\n", chunk.len()).into_bytes();
            framed.extend_from_slice(chunk);
            framed.extend_from_slice(b"\r\n");
            // The client may intentionally disconnect (stop/takeover); a write
            // error simply ends the slow producer.
            if stream.write_all(&framed).is_err() {
                return;
            }
            if stream.flush().is_err() {
                return;
            }
            thread::sleep(delay);
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    (port, handle)
}

fn serve_status(status_line: &'static str) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (port, handle)
}

/// `Content-Length` claims more bytes than are actually sent, then the server
/// closes the connection: this must surface as an explicit "download
/// interrupted" error.
fn serve_truncated(body: Vec<u8>, keep: usize) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        let mut stream = stream;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&body[..keep.min(body.len())]).unwrap();
        // Dropping the stream closes the TCP connection mid-body.
    });
    (port, handle)
}

/// The first response closes early. The second must request the exact byte
/// offset, even when the initial reply did not advertise Accept-Ranges.
fn serve_resumable(body: Vec<u8>, keep: usize, same_object: bool) -> (u16, JoinHandle<()>) {
    let listener = listen();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let accept = || {
            let start = Instant::now();
            loop {
                match listener.accept() {
                    Ok((stream, _)) => return stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(start.elapsed() < Duration::from_secs(8), "no HTTP request");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
        };
        let length = body.len();
        {
            let mut stream = accept();
            read_request(&stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {length}\r\nETag: \"audio1\"\r\nConnection: close\r\n\r\n"
            ).unwrap();
            stream.write_all(&body[..keep]).unwrap();
        }
        let mut stream = accept();
        let request = read_request(&stream);
        assert!(String::from_utf8_lossy(&request).contains(&format!("Range: bytes={keep}-")));
        let tag = if same_object { "audio1" } else { "audio2" };
        write!(
            stream,
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {keep}-{}/{length}\r\nContent-Length: {}\r\nETag: \"{tag}\"\r\nConnection: close\r\n\r\n",
            length - 1, length - keep
        ).unwrap();
        let _ = stream.write_all(&body[keep..]);
    });
    (port, server)
}

/// Accepts the request and never answers (until the test drops the listener).
fn serve_stall(hold: Duration) -> (u16, JoinHandle<()>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        read_request(&stream);
        thread::sleep(hold);
        drop(stream);
    });
    (port, handle)
}

/// Serve two requests: the first is rejected with 403 (the CDN dislikes the
/// browser UA), the second returns the media. Records the `User-Agent` header of
/// every request so the test can assert the one-time fallback.
fn serve_ua_fallback(body: Vec<u8>) -> (u16, JoinHandle<()>, Arc<Mutex<Vec<String>>>) {
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let agents = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = Arc::clone(&agents);
    let handle = thread::spawn(move || {
        for attempt in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            let request = read_request(&stream);
            let text = String::from_utf8_lossy(&request);
            let agent = text
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
                .map(|line| line.trim().to_string())
                .unwrap_or_default();
            recorded.lock().unwrap().push(agent);
            let mut stream = stream;
            if attempt == 0 {
                let response =
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            } else {
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        }
    });
    (port, handle, agents)
}

/// Loopback HTTP server that optionally honours `Range: bytes=N-` and records
/// every requested range, so tests can assert the seek transport path.
struct TestHttpServer {
    port: u16,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Option<String>>>>,
    handle: Option<JoinHandle<()>>,
}

impl TestHttpServer {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn ranges(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .cloned()
            .collect()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Serve `body` over loopback HTTP until dropped. `range_capable` controls
/// whether `Range: bytes=N-` yields a `206 Partial Content` response.
fn serve_http_bytes(
    body: Vec<u8>,
    content_type: &'static str,
    range_capable: bool,
    throttle: Option<Duration>,
) -> TestHttpServer {
    let listener = listen();
    listener
        .set_nonblocking(true)
        .expect("loopback listener non-blocking");
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let server_stop = Arc::clone(&stop);
    let server_requests = Arc::clone(&requests);
    let handle = thread::spawn(move || {
        let body = Arc::new(body);
        while !server_stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let request = read_request(&stream);
                    let text = String::from_utf8_lossy(&request);
                    let range = text
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
                        .map(|line| line.trim().to_string());
                    let requested_range = range.as_deref().and_then(parse_range_header);
                    let start = requested_range.unwrap_or(0);
                    server_requests.lock().unwrap().push(range);
                    let mut stream = stream;
                    let (status, extra_headers, payload) =
                        if range_capable
                            && requested_range.is_some()
                            && (start as usize) < body.len()
                        {
                            let start = start as usize;
                            (
                                "206 Partial Content",
                                format!(
                                    "Content-Range: bytes {start}-{}/{}\r\n",
                                    body.len() - 1,
                                    body.len()
                                ),
                                body[start..].to_vec(),
                            )
                        } else {
                            ("200 OK", String::new(), body.as_ref().clone())
                        };
                    let accept_ranges = if range_capable {
                        "Accept-Ranges: bytes\r\n"
                    } else {
                        ""
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{accept_ranges}{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    if stream.write_all(response.as_bytes()).is_err() {
                        continue;
                    }
                    match throttle {
                        Some(delay) => {
                            for chunk in payload.chunks(16 * 1024) {
                                if stream.write_all(chunk).is_err() {
                                    break;
                                }
                                thread::sleep(delay);
                            }
                        }
                        None => {
                            let _ = stream.write_all(&payload);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return,
            }
        }
    });
    TestHttpServer {
        port,
        stop,
        requests,
        handle: Some(handle),
    }
}

fn parse_range_header(value: &str) -> Option<u64> {
    let rest = value.trim();
    let rest = rest
        .strip_prefix("Range:")
        .or_else(|| rest.strip_prefix("range:"))?
        .trim();
    let rest = rest.strip_prefix("bytes=")?;
    rest.split('-').next()?.trim().parse().ok()
}

/// Minimal TLS server presenting a **self-signed** certificate from the test
/// fixtures; used to prove the client rejects untrusted certificates.
fn serve_self_signed_tls(connections: usize) -> (u16, JoinHandle<()>) {
    let certificate = rustls::pki_types::CertificateDer::from(
        include_bytes!("data/tls_test_cert.der").to_vec(),
    );
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("data/tls_test_key.der").to_vec(),
    );
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key),
        )
        .expect("self-signed TLS server config");
    let config = Arc::new(config);
    let listener = listen();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for _ in 0..connections {
            let Ok((tcp, _)) = listener.accept() else {
                return;
            };
            let config = Arc::clone(&config);
            let _ = thread::spawn(move || {
                let Ok(connection) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut stream = rustls::StreamOwned::new(connection, tcp);
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
            });
        }
    });
    (port, handle)
}

fn play_in_background(player: Arc<DlnaPlayer>, uri: String) -> Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = player.play_uri(&uri).map_err(|error| format!("{error:#}"));
        let _ = sender.send(result);
    });
    receiver
}

fn wait_for(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    condition()
}

fn expect_frames(actual: u64, expected: f64) {
    let ratio = actual as f64 / expected;
    assert!(
        (0.96..=1.04).contains(&ratio),
        "expected ~{expected:.0} frames at 48 kHz, got {actual} (ratio {ratio:.4})"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn content_length_wav_plays_as_48k_stereo_and_releases_arbiter() {
    let harness = harness();
    let wav = wav_bytes(44_100, 2, 1.5);
    let (port, server) = serve_content_length(wav, "audio/wav");

    let receiver = play_in_background(
        Arc::clone(&harness.player),
        format!("http://127.0.0.1:{port}/media/a.wav"),
    );
    assert!(wait_for(|| harness.backend.frames() > 0, Duration::from_secs(5)));
    let first_frames = harness.backend.frames();
    assert!(
        first_frames <= 12_000,
        "fast HTTP sent {first_frames} frames before the sink could play them"
    );
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("content-length WAV should finish")
        .expect("content-length WAV should play");
    server.join().unwrap();

    let frames = harness.backend.frames();
    expect_frames(frames, 1.5 * 48_000.0);
    harness.backend.assert_48k_stereo();
    assert!(harness.backend.peak() > 0.05, "decoded audio must not be silent");
    assert_eq!(harness.backend.flushes(), 0, "natural EOF must let the device play the tail");

    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
    assert!(
        state.dlna.detail.contains("播放完成"),
        "detail should report completion: {}",
        state.dlna.detail
    );
    assert!(
        state.dlna.detail.contains("48 kHz") && state.dlna.detail.contains("字节"),
        "detail should report rate/frames/bytes: {}",
        state.dlna.detail
    );
    assert_eq!(state.media.title.as_deref(), Some("a.wav"));
    assert!(
        state
            .media
            .detail_codec
            .as_deref()
            .is_some_and(|codec| codec.contains("48 kHz")),
        "codec detail should mention the engine format: {:?}",
        state.media.detail_codec
    );
    let position = state.media.position_ms.unwrap_or(0);
    assert!(
        (1_400..=1_600).contains(&position),
        "position should track playback (~1500 ms), got {position}"
    );
    assert_eq!(harness.arbiter.current(), None, "lease must be released");

    let stats = harness.player.stats().expect("stats are recorded");
    assert!(stats.finished);
    assert!(stats.decoded_frames > 0 && stats.output_frames > 0);
    assert!(
        stats.elapsed >= Duration::from_millis(1_450),
        "fast HTTP WAV must take its audio duration to play, got {:?}",
        stats.elapsed
    );
    assert!(stats.bytes_read > 0);
    assert!(stats.error.is_none());
}

#[test]
fn playback_starts_before_the_entire_body_is_downloaded() {
    let harness = harness();
    // ~2 s of audio: ~350 KB body. Only the first ~120 KB arrive immediately.
    let wav = wav_bytes(44_100, 2, 2.0);
    let (port, server) =
        serve_content_length_slow_tail(wav, 120_000, Duration::from_millis(1_000));

    let started = Instant::now();
    let receiver = play_in_background(
        Arc::clone(&harness.player),
        format!("http://127.0.0.1:{port}/stream.wav"),
    );
    let produced = wait_for(
        || harness.backend.frames() > 4_800,
        Duration::from_secs(5),
    );
    let first_audio = started.elapsed();
    assert!(produced, "streaming playback never produced audio");
    assert!(
        first_audio < Duration::from_millis(700),
        "audio only started after the full body was buffered ({first_audio:?}); the HTTP body must be streamed"
    );

    let result = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("streaming playback did not finish");
    assert!(result.is_ok(), "streaming playback failed: {result:?}");
    server.join().unwrap();

    expect_frames(harness.backend.frames(), 2.0 * 48_000.0);
    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none());
}

/// The task-specified baseline wiring: `NullBackend` + real `AudioEngine`,
/// `Arbiter` and `SharedState` (no recording backend involved).
#[test]
fn null_backend_plays_http_wav_and_reports_idle() {
    let backend = NullBackend::new();
    let engine = AudioEngine::new(AudioSettings::default(), backend.clone()).unwrap();
    let arbiter = Arc::new(Arbiter::new());
    let state = AppState::new();
    let player = DlnaPlayer::new(engine, Arc::clone(&arbiter), Arc::clone(&state));

    let (port, server) = serve_content_length(wav_bytes(44_100, 2, 1.5), "audio/wav");
    player
        .play_uri(&format!("http://127.0.0.1:{port}/a.wav"))
        .expect("NullBackend playback should succeed");
    server.join().unwrap();

    assert!(backend.master_frames() > 0, "frames must reach the real AudioEngine");
    let snapshot = state.snapshot();
    assert_eq!(snapshot.dlna.state, "idle", "detail: {}", snapshot.dlna.detail);
    assert!(snapshot.last_error.is_none());
    assert_eq!(arbiter.current(), None);
}

#[test]
fn chunked_mono_32k_wav_is_upmixed_and_resampled() {
    let harness = harness();
    let wav = wav_bytes(32_000, 1, 1.2);
    let (port, server) = serve_chunked(wav, 1_500);

    harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/mono.wav"))
        .expect("chunked mono WAV should play");
    server.join().unwrap();

    expect_frames(harness.backend.frames(), 1.2 * 48_000.0);
    harness.backend.assert_48k_stereo();
    assert!(harness.backend.peak() > 0.05);
    // Mono must be duplicated, not routed to a single speaker.
    assert_eq!(
        harness.backend.stereo_unequal.load(Ordering::Relaxed),
        0,
        "mono up-mix should make left and right identical"
    );

    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none());
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn missing_media_reports_http_404_in_state() {
    let harness = harness();
    let (port, server) = serve_status("404 Not Found");

    let error = harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/missing.wav"))
        .expect_err("404 must fail");
    server.join().unwrap();

    let message = format!("{error:#}");
    assert!(message.contains("404"), "error should mention 404: {message}");

    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "error");
    assert!(
        state
            .last_error
            .as_deref()
            .is_some_and(|last| last.contains("404")),
        "last_error must carry the 404: {:?}",
        state.last_error
    );
    assert_eq!(harness.backend.frames(), 0);
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn https_network_errors_empty_hls_and_video_are_rejected_explicitly() {
    let harness = harness();

    // P1-1: https:// is now supported, so a dead port produces a network/TLS
    // error instead of "HTTPS not supported".
    let error = harness
        .player
        .play_uri("https://127.0.0.1:9/a.wav")
        .expect_err("https to a dead port must fail");
    let text = format!("{error:#}");
    assert!(
        !text.contains("HTTPS 媒体暂不支持"),
        "https must no longer be rejected up-front: {text}"
    );
    assert!(
        text.contains("连接") || text.contains("TLS"),
        "expected a connect/TLS error: {text}"
    );

    // Invalid TLS hostnames are rejected before any connection attempt.
    let error = harness
        .player
        .play_uri("https://nas_name/a.wav")
        .expect_err("invalid host must be rejected");
    assert!(
        format!("{error:#}").contains("主机名非法"),
        "invalid host error: {error:#}"
    );

    let error = harness.player.play_uri("   ").expect_err("empty URI must fail");
    assert!(format!("{error:#}").contains("为空"));

    let error = harness
        .player
        .play_uri("http://127.0.0.1:9/live.m3u8")
        .expect_err("HLS must be rejected");
    assert!(format!("{error:#}").contains("HLS"));

    let error = harness
        .player
        .play_uri("ftp://127.0.0.1/a.wav")
        .expect_err("unsupported scheme must fail");
    assert!(format!("{error:#}").contains("不支持"));

    // Rejections must not leave a stale lease behind.
    assert_eq!(harness.arbiter.current(), None);
    assert_eq!(harness.backend.frames(), 0);
    assert_eq!(harness.state.snapshot().dlna.state, "error");
}

#[test]
fn premature_eof_resumes_exactly_once_without_joining_a_changed_object() {
    for same_object in [true, false] {
        let harness = harness();
        let wav = wav_bytes(48_000, 2, 1.0);
        let (port, server) = serve_resumable(wav, 72_032, same_object);
        let result = harness.player.play_uri(&format!(
            "http://127.0.0.1:{port}/network.wav?token=temporary-signature"
        ));
        server.join().unwrap();
        if same_object {
            result.expect("matching 206 must resume the same stream");
            expect_frames(harness.backend.frames(), 48_000.0);
            assert!(harness.state.snapshot().errors.is_empty());
        } else {
            result.expect_err("a changed object must never be spliced into PCM");
            let errors = harness.state.snapshot().errors;
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("Content-Length"));
            assert!(!errors[0].contains("temporary-signature"));
        }
    }
}

#[test]
fn truncated_download_is_an_explicit_error() {
    let harness = harness();
    let wav = wav_bytes(44_100, 2, 1.5);
    let keep = wav.len() / 2;
    let (port, server) = serve_truncated(wav, keep);

    let error = harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/broken.wav"))
        .expect_err("truncated body must fail");
    server.join().unwrap();

    let message = format!("{error:#}");
    assert!(
        message.contains("中断") || message.contains("Content-Length"),
        "truncation error should be explicit: {message}"
    );
    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "error");
    assert!(state.last_error.is_some());
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn stalled_server_times_out_with_explicit_error() {
    let harness = harness();
    harness.player.set_timeout(Duration::from_millis(200));
    let (port, server) = serve_stall(Duration::from_millis(600));

    let started = Instant::now();
    let error = harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/stall.wav"))
        .expect_err("stalled server must fail");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timeout must not hang: {:?}",
        started.elapsed()
    );
    server.join().unwrap();

    let message = format!("{error:#}");
    assert!(
        message.contains("超时"),
        "timeout error should be explicit: {message}"
    );
    assert!(harness.state.snapshot().last_error.is_some());
}

#[test]
fn airplay_takeover_stops_dlna_playback() {
    let harness = harness();
    // ~5 s of audio trickled over the network so takeover happens mid-track.
    let wav = wav_bytes(44_100, 2, 5.0);
    let (port, server) = serve_chunked_slow(wav, 4 * 1024, Duration::from_millis(5));

    let receiver = play_in_background(
        Arc::clone(&harness.player),
        format!("http://127.0.0.1:{port}/long.wav"),
    );
    let playing = wait_for(
        || harness.backend.frames() > 4_800,
        Duration::from_secs(10),
    );
    assert!(playing, "playback never produced audio");

    let takeover = harness.arbiter.claim(Source::AirPlay, "integration-test");
    assert_eq!(takeover.suspended.map(|lease| lease.source), Some(Source::Dlna));

    let result = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("DLNA playback did not stop after takeover");
    assert!(result.is_ok(), "takeover should be a clean stop: {result:?}");
    let _ = server.join();

    assert_eq!(harness.arbiter.current_source(), Some(Source::AirPlay));
    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none(), "takeover is not an error: {:?}", state.last_error);
    assert!(
        state.dlna.detail.contains("停止") || state.dlna.detail.contains("接管"),
        "detail should explain the stop: {}",
        state.dlna.detail
    );

    let frames_after_stop = harness.backend.frames();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        harness.backend.frames(),
        frames_after_stop,
        "no PCM may reach the engine after takeover"
    );
}

#[test]
fn stop_releases_lease_and_stops_feeding_engine() {
    let harness = harness();
    let wav = wav_bytes(44_100, 2, 5.0);
    let (port, server) = serve_chunked_slow(wav, 4 * 1024, Duration::from_millis(5));

    let receiver = play_in_background(
        Arc::clone(&harness.player),
        format!("http://127.0.0.1:{port}/long.wav"),
    );
    assert!(
        wait_for(|| harness.backend.frames() > 4_800, Duration::from_secs(10)),
        "playback never produced audio"
    );
    harness.player.set_paused(true);
    thread::sleep(Duration::from_millis(75));
    let paused_frames = harness.backend.frames();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(harness.backend.frames(), paused_frames, "paused decode still feeding PCM");
    harness.player.set_paused(false);
    assert!(wait_for(|| harness.backend.frames() > paused_frames, Duration::from_secs(5)));
    harness.player.stop();

    let result = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("DLNA playback did not stop");
    assert!(result.is_ok(), "stop should be a clean stop: {result:?}");
    let _ = server.join();

    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none());
    assert!(state.dlna.detail.contains("停止"));
    assert_eq!(harness.arbiter.current(), None, "stop must release the lease");

    let frames_after_stop = harness.backend.frames();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        harness.backend.frames(),
        frames_after_stop,
        "no PCM may reach the engine after stop"
    );
}

/// NetEase Cloud Music casts audio-only streams as `video/flv` with a
/// `videoItem` UPnP class; the host must demux FLV AAC instead of rejecting it.
#[test]
fn flv_aac_audio_plays_from_disk() {
    let harness = harness();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_aac.flv");
    let uri = format!("file://{}", path.display());
    let event = serde_json::json!({
        "url": uri, "media_kind": "audio", "content_type": "video/flv",
        "upnp_class": "object.item.videoItem", "title": "Overture",
        "artwork_url": "https://example.test/cover.jpg"
    });
    let track = serde::Deserialize::deserialize(&event).unwrap();
    harness
        .player
        .play_track(&track)
        .expect("FLV AAC audio playback should work");

    assert!(harness.backend.frames() > 20_000, "no FLV audio reached the engine");
    let stats = harness.player.stats().unwrap();
    let pcm_duration = Duration::from_secs_f64(stats.output_frames as f64 / 48_000.0);
    assert!(
        stats.elapsed + Duration::from_millis(50) >= pcm_duration,
        "FLV decoded in {:?} but contains {pcm_duration:?} of audio",
        stats.elapsed
    );
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
    assert_eq!(state.media.title.as_deref(), Some("Overture"));
    assert_eq!(state.media.cover_url.as_deref(), Some("https://example.test/cover.jpg"));
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn ogg_vorbis_audio_plays_from_disk() {
    let harness = harness();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_vorbis.ogg");
    let uri = format!("file://{}", path.display());
    let track = DlnaTrack {
        uri: &uri,
        media_kind: "audio",
        content_type: Some("audio/ogg"),
        ..DlnaTrack::default()
    };
    harness
        .player
        .play_track(&track)
        .expect("OGG/Vorbis playback should work");

    assert!(harness.backend.frames() > 20_000, "no OGG audio reached the engine");
    let state = harness.state.snapshot();
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
}

#[test]
fn file_uri_plays_from_disk() {
    let harness = harness();
    let directory = tempfile::tempdir().unwrap();
    let path = PathBuf::from(directory.path()).join("local.wav");
    std::fs::write(&path, wav_bytes(48_000, 2, 1.0)).unwrap();

    let uri = format!("file://{}", path.display());
    harness
        .player
        .play_uri(&uri)
        .expect("local file playback should work");

    expect_frames(harness.backend.frames(), 48_000.0);
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert!(state.last_error.is_none());
    assert_eq!(state.media.title.as_deref(), Some("local.wav"));
    assert_eq!(harness.arbiter.current(), None);
}

/// NetEase-style FLV containers also carry MP3 audio (SoundFormat 2); the core
/// demuxer re-assembles the MPEG frames before the host decodes them.
#[test]
fn flv_mp3_audio_plays_from_disk() {
    let harness = harness();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_mp3.flv");
    let uri = format!("file://{}", path.display());
    let track = DlnaTrack {
        uri: &uri,
        media_kind: "audio",
        content_type: Some("video/flv"),
        ..DlnaTrack::default()
    };
    harness
        .player
        .play_track(&track)
        .expect("FLV MP3 audio playback should work");

    // LAME pads the MP3 stream to whole frames (40 x 1152 samples for a 1 s
    // fixture), so the decoded length is slightly over one second.
    expect_frames(harness.backend.frames(), 50_155.0);
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
    assert_eq!(state.dlna.state, "idle", "detail: {}", state.dlna.detail);
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn alac_m4a_audio_plays_from_disk() {
    let harness = harness();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_alac.m4a");
    let uri = format!("file://{}", path.display());
    let track = DlnaTrack {
        uri: &uri,
        media_kind: "audio",
        content_type: Some("audio/mp4"),
        ..DlnaTrack::default()
    };
    harness
        .player
        .play_track(&track)
        .expect("ALAC/M4A playback should work");

    expect_frames(harness.backend.frames(), 48_000.0);
    let state = harness.state.snapshot();
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
    assert_eq!(harness.arbiter.current(), None);
}

#[test]
fn aiff_audio_plays_from_disk() {
    let harness = harness();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_aiff.aiff");
    let uri = format!("file://{}", path.display());
    let track = DlnaTrack {
        uri: &uri,
        media_kind: "audio",
        content_type: Some("audio/aiff"),
        ..DlnaTrack::default()
    };
    harness
        .player
        .play_track(&track)
        .expect("AIFF playback should work");

    expect_frames(harness.backend.frames(), 24_000.0);
    let state = harness.state.snapshot();
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);
    assert_eq!(harness.arbiter.current(), None);
}

/// A CDN may reject the browser User-Agent with 403; the player must reconnect
/// once with the legacy `FusionPlay-fnOS/<version>` UA and succeed.
#[test]
fn http_403_retries_once_with_the_legacy_user_agent() {
    let harness = harness();
    let wav = wav_bytes(48_000, 2, 1.0);
    let (port, server, agents) = serve_ua_fallback(wav);
    harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/guarded.wav"))
        .expect("403 fallback should play");
    server.join().unwrap();

    expect_frames(harness.backend.frames(), 48_000.0);
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
    assert!(state.last_error.is_none(), "unexpected error: {:?}", state.last_error);

    let agents = agents.lock().unwrap();
    assert_eq!(agents.len(), 2, "expected exactly two HTTP attempts: {agents:?}");
    assert!(
        agents[0].to_ascii_lowercase().contains("chrome"),
        "first attempt must use the browser UA: {:?}",
        agents[0]
    );
    assert!(
        agents[1].contains("FusionPlay-fnOS/"),
        "retry must use the legacy FusionPlay UA: {:?}",
        agents[1]
    );
}

/// P1-2: an M4A whose `moov` atom sits at the end can only be demuxed if the
/// HTTP source supports byte-range seeking. This exercises the full path
/// (initial `Range: bytes=0-` probe -> 206 -> Symphonia seek -> playback).
#[test]
fn http_range_enables_m4a_streaming_from_network() {
    let harness = harness();
    let body = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sine_alac.m4a"),
    )
    .expect("fixture");
    let server = serve_http_bytes(body, "audio/mp4", true, None);
    harness
        .player
        .play_uri(&server.url("/music/track.m4a"))
        .expect("Range-capable M4A should stream over HTTP");

    expect_frames(harness.backend.frames(), 48_000.0);
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
    assert!(
        state.last_error.is_none(),
        "unexpected error: {:?}",
        state.last_error
    );
    let ranges = server.ranges();
    assert!(
        ranges
            .iter()
            .filter_map(|range| parse_range_header(range))
            .any(|start| start > 0),
        "expected a byte-range seek into the file (moov at end), got {ranges:?}"
    );
}

/// P1-2: seeking while playing a Range-capable HTTP stream keeps the stream
/// open (format seek) or reopens it with `Range: bytes=N-`; both must produce
/// a `Range` start > 0 and keep playback going to the end.
#[test]
fn seek_while_playing_uses_range_and_keeps_playback() {
    let harness = harness();
    let wav = wav_bytes(48_000, 2, 4.0);
    let server = serve_http_bytes(wav, "audio/wav", true, Some(Duration::from_millis(10)));
    let receiver = play_in_background(Arc::clone(&harness.player), server.url("/music/seek.wav"));
    assert!(
        wait_for(
            || harness.state.snapshot().dlna.state == "playing",
            Duration::from_secs(5)
        ),
        "playback did not start"
    );
    thread::sleep(Duration::from_millis(120));
    harness
        .player
        .seek(2_000)
        .expect("seek while playing must be accepted");
    receiver
        .recv_timeout(Duration::from_secs(15))
        .expect("playback must finish")
        .expect("playback after seek");

    let state = harness.state.snapshot();
    assert!(
        state.last_error.is_none(),
        "unexpected error: {:?}",
        state.last_error
    );
    let position = state.media.position_ms.unwrap_or(0);
    assert!(
        position >= 3_000,
        "final position should be near the end after seeking to 2 s: {position}"
    );
    let ranges = server.ranges();
    assert!(
        ranges
            .iter()
            .filter_map(|range| parse_range_header(range))
            .any(|start| start > 0),
        "seek must use a byte range, got {ranges:?}"
    );
}

/// P1-2 fallback: a server that ignores `Range` (always 200) cannot seek, so
/// the player reopens the stream and discards decoded audio instead.
#[test]
fn seek_without_range_falls_back_to_discard() {
    let harness = harness();
    let wav = wav_bytes(48_000, 2, 3.0);
    let server = serve_http_bytes(wav, "audio/wav", false, Some(Duration::from_millis(10)));
    let receiver = play_in_background(Arc::clone(&harness.player), server.url("/music/no-range.wav"));
    assert!(
        wait_for(
            || harness.state.snapshot().dlna.state == "playing",
            Duration::from_secs(5)
        ),
        "playback did not start"
    );
    thread::sleep(Duration::from_millis(100));
    harness
        .player
        .seek(1_500)
        .expect("seek must be accepted even without Range");
    receiver
        .recv_timeout(Duration::from_secs(15))
        .expect("playback must finish")
        .expect("playback after discard fallback");

    let state = harness.state.snapshot();
    assert!(
        state.last_error.is_none(),
        "unexpected error: {:?}",
        state.last_error
    );
    assert!(
        state.media.position_ms.unwrap_or(0) >= 2_500,
        "final position should be near the end: {:?}",
        state.media.position_ms
    );
    // Either the format supported an in-place seek (no reopen) or the stream
    // was reopened and decoded audio discarded; both must keep the position
    // honest and produce no error.
    assert!(
        server.request_count() >= 1,
        "request log should not be empty"
    );
}

/// P2-2: TLS certificates are always validated; a self-signed certificate must
/// be rejected with an explicit Chinese error (no hidden insecure fallback).
#[test]
fn https_self_signed_certificate_is_rejected() {
    let harness = harness();
    let (port, server) = serve_self_signed_tls(1);
    let error = harness
        .player
        .play_uri(&format!("https://localhost:{port}/a.wav"))
        .expect_err("self-signed certificate must be rejected");
    let _ = server.join();
    let text = format!("{error:#}");
    assert!(
        text.contains("TLS") || text.contains("证书"),
        "expected an explicit TLS/certificate error: {text}"
    );
    assert_eq!(harness.backend.frames(), 0);
}

/// P1-1: speaking TLS to a plaintext HTTP endpoint fails the handshake with an
/// explicit error instead of hanging or silently playing.
#[test]
fn https_against_plain_http_server_fails_handshake() {
    let harness = harness();
    let (port, server) = serve_status("400 Bad Request");
    let error = harness
        .player
        .play_uri(&format!("https://127.0.0.1:{port}/a.wav"))
        .expect_err("TLS handshake against plain HTTP must fail");
    let _ = server.join();
    assert!(
        format!("{error:#}").contains("TLS"),
        "expected a TLS handshake error: {error:#}"
    );
    assert_eq!(harness.backend.frames(), 0);
}
