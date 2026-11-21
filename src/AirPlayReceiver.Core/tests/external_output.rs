//! Verifies the host-installed PCM output hook (used by the fnOS unified
//! engine): decoded AirPlay PCM reaches `PcmOutput` with the sender's logical
//! volume, without opening a cpal device. Runs on CI with no sound card.

use fusionplay_core::{
    EventSink, PcmOutput, PcmSource, PlaybackArbiter, start_audio_external,
};
use shairplay::{AudioCodec, AudioFormat, AudioHandler, AudioSession};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct CountingSink {
    frames: AtomicU64,
    writes: AtomicU64,
    sender_gain_bits: AtomicU32,
    rate: AtomicU32,
    channels: AtomicU32,
    flushed_airplay: AtomicBool,
}

impl PcmOutput for CountingSink {
    fn write(
        &self,
        source: PcmSource,
        _epoch: u64,
        sender_gain: f32,
        sample_rate: u32,
        channels: u16,
        samples: &[f32],
    ) {
        assert_eq!(source, PcmSource::AirPlay);
        self.sender_gain_bits.store(sender_gain.to_bits(), Ordering::Relaxed);
        self.rate.store(sample_rate, Ordering::Relaxed);
        self.channels.store(channels as u32, Ordering::Relaxed);
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.frames
            .fetch_add((samples.len() / channels.max(1) as usize) as u64, Ordering::Release);
    }

    fn flush(&self, source: PcmSource) {
        if source == PcmSource::AirPlay {
            self.flushed_airplay.store(true, Ordering::Relaxed);
        }
    }
}

#[test]
fn external_output_forwards_pcm_and_sender_gain() {
    let sink = Arc::new(CountingSink::default());
    let events = Arc::new(EventSink::new());
    let arbiter = Arc::new(PlaybackArbiter::new(Arc::clone(&events)));
    let state_dir = tempfile::tempdir().unwrap();
    let runtime = start_audio_external(
        Arc::clone(&events),
        Arc::clone(&arbiter),
        state_dir.path().to_path_buf(),
        Arc::clone(&sink) as Arc<dyn PcmOutput>,
    )
    .expect("external audio runtime");

    let format = AudioFormat {
        codec: AudioCodec::Pcm,
        bits: 32,
        channels: 2,
        sample_rate: 48_000,
        source: None,
    };
    let mut session: Box<dyn AudioSession> = AudioHandler::audio_init(&*runtime.handler, format);
    // Sender volume: -6.02 dB -> linear 0.5.
    AudioHandler::on_volume(&*runtime.handler, -6.02);
    let mut render_block = || {
        let expected = sink.frames.load(Ordering::Acquire) + 960;
        session.audio_process(&[0.25f32; 960 * 2]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while sink.frames.load(Ordering::Acquire) < expected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(sink.frames.load(Ordering::Acquire), expected, "frames forwarded");
        f32::from_bits(sink.sender_gain_bits.load(Ordering::Relaxed))
    };
    let gain = render_block();
    assert!((gain - 0.5).abs() < 0.01, "sender gain forwarded, got {gain}");
    runtime.handler.apply_local_volume_percent(0, None);
    assert_eq!(render_block(), 0.0, "the host slider must mute PCM");
    runtime.handler.apply_local_volume_percent(100, None);
    assert_eq!(render_block(), 1.0, "full volume restores unity gain");
    assert!(sink.writes.load(Ordering::Relaxed) >= 1);
    assert_eq!(sink.rate.load(Ordering::Relaxed), 48_000);
    assert_eq!(sink.channels.load(Ordering::Relaxed), 2);
    assert_eq!(runtime.sample_rate, 48_000);
    assert_eq!(runtime.channels, 2);

    drop(session);
    drop(runtime);
    assert!(
        sink.flushed_airplay.load(Ordering::Relaxed),
        "dropping the receiver flushes the external sink"
    );
}
