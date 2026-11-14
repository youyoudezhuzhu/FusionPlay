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
//! * `file://` playback from disk.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
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
        for frame in samples.chunks_exact(2) {
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

    harness
        .player
        .play_uri(&format!("http://127.0.0.1:{port}/media/a.wav"))
        .expect("content-length WAV should play");
    server.join().unwrap();

    let frames = harness.backend.frames();
    expect_frames(frames, 1.5 * 48_000.0);
    harness.backend.assert_48k_stereo();
    assert!(harness.backend.peak() > 0.05, "decoded audio must not be silent");
    assert!(harness.backend.flushes() >= 1, "engine flush must run at end of track");

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
fn https_empty_hls_and_video_are_rejected_explicitly() {
    let harness = harness();

    let error = harness
        .player
        .play_uri("https://127.0.0.1:9/a.wav")
        .expect_err("https must be rejected");
    assert!(
        format!("{error:#}").contains("HTTPS"),
        "https error should be explicit: {error:#}"
    );
    assert!(
        harness
            .state
            .snapshot()
            .last_error
            .as_deref()
            .is_some_and(|last| last.contains("HTTPS"))
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
    let track = DlnaTrack {
        uri: &uri,
        media_kind: "audio",
        content_type: Some("video/flv"),
        upnp_class: Some("object.item.videoItem"),
        ..DlnaTrack::default()
    };
    harness
        .player
        .play_track(&track)
        .expect("FLV AAC audio playback should work");

    assert!(harness.backend.frames() > 20_000, "no FLV audio reached the engine");
    harness.backend.assert_48k_stereo();
    let state = harness.state.snapshot();
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
