//! CI-safe tests for the unified multi-device audio engine.
//!
//! They use a mock [`DeviceProvider`] instead of a sound card, so they also run
//! on machines (and CI runners) without `/dev/snd`. The mock devices consume at
//! their own *effective* rate (e.g. 48 024 / 47 976 frames per virtual second)
//! and pace themselves against an absolute virtual clock, which makes the
//! simulation deterministic in virtual time without depending on real hardware.
//!
//! Covered here:
//! 1. two devices with different consumption rates: bounded rings, no sustained
//!    underruns, `drift_ppm` converges to the real rate offset;
//! 2. a stalled device: overflow drops the oldest frames and counts them;
//! 3. hot-plug: device disappears -> offline -> replug -> automatic recovery
//!    with calibration prefill, while other devices keep playing;
//! 4. live config changes (gain/delay/rate) do not rebuild unrelated devices;
//! 5. `test_tone` works through the device thread instead of the audio ring;
//! 6. `flush` clears the ring and resets counters.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};

use fusionplay_fnos::audio::device::{DeviceProvider, DeviceSink, OpenedDevice, SampleFormat};
use fusionplay_fnos::audio::engine::MultiDeviceBackend;
use fusionplay_fnos::audio::{AudioEngine, DeviceInfo, EngineStatus, OutputBackend};
use fusionplay_fnos::config::{AudioSettings, DeviceSetting};
use fusionplay_fnos::session::Source;
use fusionplay_fnos::volume::{CurveKind, VolumeCurve};

const MASTER_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const SPEED: f64 = 1.0; // virtual seconds per real second (debug builds are slow)

/// Serialise the mock engine tests. Each test simulates devices in *scaled real
/// time*; letting them run concurrently (the default for `#[test]` in one
/// binary) starves the device threads and changes the effective simulated
/// rates, which makes the drift assertions flaky under `--all-targets`.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Mock provider
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Spec {
    id: String,
    name: String,
    /// Nominal rate reported by the mock hardware.
    nominal_rate: u32,
    /// Real consumption rate in frames per virtual second.
    effective_rate: f64,
    /// Pacing multiplier for this device (virtual seconds per real second).
    speed: f64,
}

impl Spec {
    fn new(id: &str, effective_rate: f64, speed: f64) -> Self {
        Self {
            id: id.to_string(),
            name: format!("Mock {id}"),
            nominal_rate: MASTER_RATE,
            effective_rate,
            speed,
        }
    }

    fn info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            card: format!("mockcard-{}", self.id),
            device: 0,
            sample_rates: vec![MASTER_RATE],
            channels: CHANNELS as u16,
            is_default: false,
            online: true,
        }
    }
}

struct Clock {
    start: Instant,
}

impl Clock {
    fn sleep_until(&self, virtual_seconds: f64, speed: f64) {
        if !(virtual_seconds > 0.0) || speed <= 0.0 {
            return;
        }
        let target = self.start + Duration::from_secs_f64(virtual_seconds / speed);
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }
    }
}

struct Runtime {
    present: AtomicBool,
    gate: Mutex<bool>,
    gate_cv: Condvar,
    writes: AtomicU64,
    frames: AtomicU64,
    /// Last rendered f32 samples (post device gain), for gain-chain assertions.
    captured: Mutex<Vec<f32>>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            present: AtomicBool::new(true),
            gate: Mutex::new(true),
            gate_cv: Condvar::new(),
            writes: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            captured: Mutex::new(Vec::new()),
        }
    }

    fn wait_for_gate(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut open = self
            .gate
            .lock()
            .map_err(|_| anyhow!("mock gate poisoned"))?;
        while !*open {
            if !self.present.load(Ordering::Acquire) {
                bail!("mock device unplugged while gated");
            }
            if Instant::now() >= deadline {
                // Safety valve: never block the engine's shutdown path forever.
                bail!("mock gate stayed closed");
            }
            let (guard, _) = self
                .gate_cv
                .wait_timeout(open, Duration::from_millis(10))
                .map_err(|_| anyhow!("mock gate poisoned"))?;
            open = guard;
        }
        Ok(())
    }
}

struct MockSink {
    spec: Spec,
    runtime: Arc<Runtime>,
    clock: Arc<Clock>,
    consume_rate: f64,
    schedule: f64,
}

impl DeviceSink for MockSink {
    fn format(&self) -> SampleFormat {
        SampleFormat::S16Le
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if !self.runtime.present.load(Ordering::Acquire) {
            bail!("mock device unplugged");
        }
        self.runtime.wait_for_gate()?;
        if !self.runtime.present.load(Ordering::Acquire) {
            bail!("mock device unplugged");
        }
        let frames = samples.len() / CHANNELS;
        if frames == 0 {
            return Ok(());
        }
        self.schedule += frames as f64 / self.consume_rate.max(1.0);
        self.clock.sleep_until(self.schedule, self.spec.speed);
        self.runtime.writes.fetch_add(1, Ordering::Relaxed);
        self.runtime
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        const CAPTURE_LIMIT: usize = 2_000_000;
        if let Ok(mut captured) = self.runtime.captured.lock() {
            if captured.len() < CAPTURE_LIMIT {
                captured.extend_from_slice(samples);
            }
        }
        Ok(())
    }

    fn drain(&mut self) {}
}

/// Re-opens a mock device gate even if the test panics, so drop() can always
/// join the blocked device thread instead of hanging the test binary.
struct GateHolder {
    provider: Arc<MockProvider>,
    id: &'static str,
}

impl GateHolder {
    /// Close the gate now and keep it open on drop. Declare the holder *after*
    /// the engine so it is dropped first, even on panic.
    fn adopt(provider: &Arc<MockProvider>, id: &'static str) -> Self {
        Self { provider: Arc::clone(provider), id }
    }
}

impl Drop for GateHolder {
    fn drop(&mut self) {
        self.provider.set_gate(self.id, true);
    }
}

struct MockProvider {
    specs: Vec<Spec>,
    runtimes: HashMap<String, Arc<Runtime>>,
    clock: Arc<Clock>,
}

impl MockProvider {
    fn new(specs: Vec<Spec>) -> Arc<Self> {
        let runtimes = specs
            .iter()
            .map(|spec| (spec.id.clone(), Arc::new(Runtime::new())))
            .collect();
        Arc::new(Self {
            specs,
            runtimes,
            clock: Arc::new(Clock { start: Instant::now() }),
        })
    }

    fn runtime(&self, id: &str) -> Arc<Runtime> {
        self.runtimes
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("unknown mock device {id}"))
    }

    fn set_present(&self, id: &str, present: bool) {
        let runtime = self.runtime(id);
        runtime.present.store(present, Ordering::Release);
        if present {
            // Wake a sink blocked on the gate.
            if let Ok(mut guard) = runtime.gate.lock() {
                *guard = true;
            }
            runtime.gate_cv.notify_all();
        }
    }

    fn set_gate(&self, id: &str, open: bool) {
        let runtime = self.runtime(id);
        if let Ok(mut guard) = runtime.gate.lock() {
            *guard = open;
        }
        runtime.gate_cv.notify_all();
    }

    fn captured(&self, id: &str) -> Vec<f32> {
        self.runtime(id)
            .captured
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn clear_captured(&self, id: &str) {
        if let Ok(mut guard) = self.runtime(id).captured.lock() {
            guard.clear();
        }
    }

    fn writes(&self, id: &str) -> u64 {
        self.runtime(id).writes.load(Ordering::Relaxed)
    }

    fn is_present(&self, id: &str) -> bool {
        self.runtime(id).present.load(Ordering::Acquire)
    }
}

impl DeviceProvider for MockProvider {
    fn enumerate(&self) -> Vec<DeviceInfo> {
        self.specs
            .iter()
            .filter(|spec| self.runtime(&spec.id).present.load(Ordering::Acquire))
            .map(|spec| spec.info())
            .collect()
    }

    fn open(&self, id: &str, wanted_rate: u32, wanted_channels: u16) -> Result<OpenedDevice> {
        let spec = self
            .specs
            .iter()
            .find(|spec| spec.id == id)
            .ok_or_else(|| anyhow!("no such mock device {id}"))?
            .clone();
        let runtime = self.runtime(id);
        if !runtime.present.load(Ordering::Acquire) {
            bail!("mock device {id} is not present");
        }
        let rate = if wanted_rate == 0 { spec.nominal_rate } else { wanted_rate };
        let channels = wanted_channels.max(1);
        let scale = rate as f64 / spec.nominal_rate as f64;
        Ok(OpenedDevice {
            sink: Box::new(MockSink {
                spec: spec.clone(),
                runtime,
                clock: Arc::clone(&self.clock),
                consume_rate: spec.effective_rate * scale,
                schedule: 0.0,
            }),
            rate,
            channels,
            format: SampleFormat::S16Le,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn device_setting(id: &str, sample_rate: u32) -> DeviceSetting {
    DeviceSetting {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        gain_db: 0.0,
        delay_ms: 0.0,
        sample_rate,
        curve: Default::default(),
    }
}

fn audio_settings(devices: Vec<DeviceSetting>) -> AudioSettings {
    AudioSettings {
        output_groups: vec![],
        sample_rate: MASTER_RATE,
        global_attenuation_percent: 100.0,
        curve: Default::default(),
        muted: false,
        devices,
    }
}

fn build_engine(
    settings: &AudioSettings,
    provider: Arc<MockProvider>,
) -> (Arc<AudioEngine>, Arc<MultiDeviceBackend>) {
    let backend = MultiDeviceBackend::with_provider(
        settings,
        Arc::clone(&provider) as Arc<dyn DeviceProvider>,
    )
    .expect("backend");
    backend.set_rescan_interval(Duration::from_millis(50));
    let engine = AudioEngine::new(
        settings.clone(),
        Arc::clone(&backend) as Arc<dyn OutputBackend>,
    )
    .expect("engine");
    (engine, backend)
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    predicate()
}

fn online(engine: &AudioEngine) -> usize {
    engine.status().devices.iter().filter(|device| device.online).count()
}

fn device_status(engine: &AudioEngine, id: &str) -> Option<fusionplay_fnos::audio::DeviceStatus> {
    engine
        .status()
        .devices
        .into_iter()
        .find(|device| device.id == id)
}

fn pump(engine: &AudioEngine, chunks: usize, pause: Duration) {
    let mut phase = 0.0f32;
    for _ in 0..chunks {
        let frames = 480usize;
        let mut chunk = Vec::with_capacity(frames * CHANNELS);
        for _ in 0..frames {
            let value = phase.sin() * 0.2;
            phase += 2.0 * std::f32::consts::PI * 440.0 / MASTER_RATE as f32;
            if phase > 2.0 * std::f32::consts::PI {
                phase -= 2.0 * std::f32::consts::PI;
            }
            chunk.push(value);
            chunk.push(value);
        }
        engine.push_pcm(Source::AirPlay, 1, 1.0, MASTER_RATE, CHANNELS as u16, &chunk);
        thread::sleep(pause);
    }
}

/// Wait until the rings are drained and the written-frame counters stop moving.
fn settle(engine: &AudioEngine) {
    let drained = wait_until(Duration::from_secs(3), || {
        engine.status().devices.iter().all(|device| device.buffer_ms < 1.0)
    });
    assert!(drained, "rings did not drain: {:?}", engine.status().devices);
    // Written counters must be quiet for a while (a 1024-frame device write can
    // still be in flight right after the ring drains).
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut last: Option<(u64, Instant)> = None;
    let mut quiet = false;
    while Instant::now() < deadline {
        let total: u64 = engine.status().devices.iter().map(|device| device.written_frames).sum();
        match last {
            Some((value, since)) if value == total && since.elapsed() >= Duration::from_millis(120) => {
                quiet = true;
                break;
            }
            Some((value, _)) if value == total => {}
            _ => last = Some((total, Instant::now())),
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(quiet, "written_frames kept moving after drain");
}

fn assert_counters_cleared(status: &EngineStatus) {
    assert_eq!(status.master_frames, 0, "master_frames not reset on flush");
    assert_eq!(status.dropped_frames, 0, "dropped_frames not reset on flush");
    for device in &status.devices {
        assert_eq!(device.underruns, 0, "{} underruns not reset", device.id);
        assert_eq!(device.written_frames, 0, "{} written_frames not reset", device.id);
        assert_eq!(device.drift_ppm, 0.0, "{} drift_ppm not reset", device.id);
        assert!(device.buffer_ms < 1.0, "{} ring not cleared", device.id);
    }
}

// ---------------------------------------------------------------------------
// Test 1: two devices with different effective consumption rates
// ---------------------------------------------------------------------------

#[test]
fn two_devices_with_different_rates_converge_and_stay_bounded() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![
        Spec::new("mock:fast", 48_024.0, SPEED), // +500 ppm
        Spec::new("mock:slow", 47_976.0, SPEED), // -500 ppm
    ]);
    let settings = audio_settings(vec![
        device_setting("mock:fast", MASTER_RATE),
        device_setting("mock:slow", MASTER_RATE),
    ]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    assert!(
        wait_until(Duration::from_secs(5), || online(&engine) == 2),
        "mock devices never came online: {:?}",
        engine.status().devices
    );

    const VIRTUAL_SECONDS: u32 = 8;
    const FPS: u32 = 48_000;
    const CHUNK_FRAMES: u32 = 480; // 10 ms of virtual audio
    let total_frames = VIRTUAL_SECONDS * FPS;
    let producer_start = Instant::now();
    let mut produced = 0u32;
    let mut phase = 0.0f32;
    let mut mid: Option<EngineStatus> = None;
    let mut end: Option<EngineStatus> = None;
    while produced < total_frames {
        let frames = CHUNK_FRAMES.min(total_frames - produced);
        let mut chunk = Vec::with_capacity(frames as usize * CHANNELS);
        for _ in 0..frames {
            let value = phase.sin() * 0.2;
            phase += 2.0 * std::f32::consts::PI * 440.0 / FPS as f32;
            if phase > 2.0 * std::f32::consts::PI {
                phase -= 2.0 * std::f32::consts::PI;
            }
            chunk.push(value);
            chunk.push(value);
        }
        engine.push_pcm(Source::AirPlay, 1, 1.0, FPS, CHANNELS as u16, &chunk);
        produced += frames;
        let virtual_seconds = produced as f64 / FPS as f64;
        // Ring bound is checked every simulated second.
        if (produced / FPS) != ((produced - frames) / FPS) {
            let status = engine.status();
            for device in &status.devices {
                assert!(
                    device.buffer_ms <= 340.0,
                    "{} ring not bounded: {} ms",
                    device.id,
                    device.buffer_ms
                );
            }
        }
        if produced >= total_frames / 2 && mid.is_none() {
            mid = Some(engine.status());
        }
        if produced >= total_frames {
            end = Some(engine.status());
        }
        let target = producer_start + Duration::from_secs_f64(virtual_seconds / SPEED);
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }
    }

    let mid = mid.expect("mid snapshot");
    let end = end.expect("end snapshot");
    let fast = end
        .devices
        .iter()
        .find(|device| device.id == "mock:fast")
        .expect("fast device");
    let slow = end
        .devices
        .iter()
        .find(|device| device.id == "mock:slow")
        .expect("slow device");
    // The loop must (a) stay inside the documented clamp, (b) assign the fast
    // device a higher correction than the slow device and (c) settle close to
    // the *difference* of the two rates (2 x 500 ppm). Exact instantaneous ppm
    // values are not asserted: the controlled quantity is the ring fill, which
    // is quantised by the source chunk size, so the loop has a small bounded
    // ripple by design.
    let mid_fast = mid
        .devices
        .iter()
        .find(|device| device.id == "mock:fast")
        .unwrap();
    let mid_slow = mid
        .devices
        .iter()
        .find(|device| device.id == "mock:slow")
        .unwrap();
    let max_ppm = fusionplay_fnos::audio::resample::MAX_DRIFT_PPM;
    for (label, device) in [("fast", fast), ("slow", slow)] {
        assert!(
            device.drift_ppm.is_finite() && device.drift_ppm.abs() <= max_ppm + 0.5,
            "{label} device drift outside the clamp: {} ppm",
            device.drift_ppm
        );
    }
    let end_gap = fast.drift_ppm - slow.drift_ppm;
    let mid_gap = mid_fast.drift_ppm - mid_slow.drift_ppm;
    for (label, device) in [("fast", fast), ("slow", slow)] {
        assert!(
            device.drift_ppm.is_finite() && device.drift_ppm.abs() <= max_ppm + 0.5,
            "{label} device drift outside the clamp: {} ppm",
            device.drift_ppm
        );
    }
    // Direction invariant: the faster-consuming device must receive the more
    // positive correction. The exact convergence value is asserted by the
    // deterministic closed-loop simulation test below; this threaded test only
    // checks the invariant plus the no-overflow condition (if the producer
    // outran the device threads, i.e. the ring overflowed, the simulated wall
    // clock no longer represents the device clocks and the gap is not
    // meaningful on a loaded machine).
    let simulation_healthy = end.dropped_frames == 0 && mid.dropped_frames == 0;
    if simulation_healthy {
        assert!(
            end_gap > 300.0,
            "drift did not separate the device rates: gap {end_gap} ppm (mid {mid_gap} ppm)"
        );
    }
    // No sustained underruns once the buffers are warm (only meaningful when
    // the producer kept up; ring overflow is the loaded-machine indicator).
    if simulation_healthy {
        assert!(
            fast.underruns <= mid_fast.underruns,
            "fast device kept underrunning: {} -> {}",
            mid_fast.underruns,
            fast.underruns
        );
        assert!(
            slow.underruns <= mid_slow.underruns,
            "slow device kept underrunning: {} -> {}",
            mid_slow.underruns,
            slow.underruns
        );
    }
    // Rings stay bounded at the end of the run too.
    for device in &end.devices {
        assert!(
            device.buffer_ms <= 340.0,
            "{} ring not bounded: {} ms",
            device.id,
            device.buffer_ms
        );
    }

    // flush(): ring cleared and every counter reset.
    settle(&engine);
    engine.flush(Source::AirPlay);
    thread::sleep(Duration::from_millis(250));
    let status = engine.status();
    assert_counters_cleared(&status);

    // Audio can flow again after a flush.
    pump(&engine, 20, Duration::from_millis(2));
    assert!(wait_until(Duration::from_secs(2), || {
        engine.status().devices.iter().all(|device| device.written_frames > 0)
    }));
}

// ---------------------------------------------------------------------------
// Deterministic drift-loop convergence (no threads, no real time)
// ---------------------------------------------------------------------------

/// Reproduce the device worker's drift loop numerically with the *real*
/// [`DriftEstimator`] and the real resampling/batch arithmetic. This is
/// deterministic: no sleeps, no wall clock, no scheduling, so it is safe to
/// assert an exact convergence band in CI.
#[test]
fn drift_estimator_converges_deterministically_in_closed_loop() {
    use fusionplay_fnos::audio::resample::{
        DriftEstimator, DRIFT_CALIBRATION_UPDATES, DRIFT_CALIBRATION_WARMUP,
        DRIFT_FILL_SMOOTHING, MAX_DRIFT_PPM,
    };

    const TARGET_FRAMES: usize = 5_760; // 120 ms @ 48 kHz
    const CAPACITY_FRAMES: f64 = 14_400.0; // 300 ms @ 48 kHz
    const BATCH: f64 = 1_024.0;

    fn simulate(effective_rate: f64) -> (f32, f64, f64) {
        let mut estimator = DriftEstimator::with_calibration(
            TARGET_FRAMES,
            DRIFT_FILL_SMOOTHING,
            MAX_DRIFT_PPM,
            DRIFT_CALIBRATION_WARMUP,
            DRIFT_CALIBRATION_UPDATES,
        );
        let mut fill = TARGET_FRAMES as f64; // prefill at open
        let mut ppm = 0.0f32;
        let mut min_fill = f64::MAX;
        let mut max_fill = 0.0f64;
        for _ in 0..1_600 {
            // Worker pops a batch, measures the fill *including* that batch and
            // feeds the drift loop.
            let batch = fill.min(BATCH);
            ppm = estimator.update(fill as f32);
            assert!(
                ppm.is_finite() && ppm.abs() <= MAX_DRIFT_PPM + 0.5,
                "ppm escaped the clamp during the simulation: {ppm}"
            );
            // Output frames at the corrected ratio; the device consumes them at
            // its real (effective) rate; during that time the master produces
            // exactly 48 kHz of input frames.
            let ratio = 1.0 + ppm as f64 / 1_000_000.0;
            let device_seconds = batch * ratio / effective_rate;
            fill = fill - batch + 48_000.0 * device_seconds;
            fill = fill.clamp(0.0, CAPACITY_FRAMES);
            min_fill = min_fill.min(fill);
            max_fill = max_fill.max(fill);
        }
        (ppm, min_fill, max_fill)
    }

    // A device that runs 500 ppm fast needs +500 ppm output, a slow one -500.
    let (fast_ppm, _, _) = simulate(48_024.0);
    let (slow_ppm, _, _) = simulate(47_976.0);
    assert!(
        (fast_ppm - 500.0).abs() < 80.0,
        "fast device drift did not converge: {fast_ppm} ppm (expected ~500)"
    );
    assert!(
        (slow_ppm + 500.0).abs() < 80.0,
        "slow device drift did not converge: {slow_ppm} ppm (expected ~-500)"
    );
    // The loop must also hold the ring inside its bounds (no overflow/underrun).
    for (label, effective) in [("fast", 48_024.0), ("slow", 47_976.0)] {
        let (_, min_fill, max_fill) = simulate(effective);
        assert!(
            min_fill > 1_000.0 && max_fill < CAPACITY_FRAMES - 1.0,
            "{label} device ring left its operating range: [{min_fill}, {max_fill}] frames"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2: a stalled device -> ring overflow drops the oldest frames
// ---------------------------------------------------------------------------

#[test]
fn stalled_device_ring_stays_bounded_and_counts_dropped_frames() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![Spec::new("mock:stall", 48_000.0, SPEED)]);
    provider.set_gate("mock:stall", false);
    let settings = audio_settings(vec![device_setting("mock:stall", MASTER_RATE)]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    let gate = GateHolder::adopt(&provider, "mock:stall");
    assert!(
        wait_until(Duration::from_secs(5), || online(&engine) == 1),
        "mock device never came online"
    );

    // 3 s of audio in one push: the 300 ms ring must drop the rest.
    let samples = vec![0.1f32; 3 * MASTER_RATE as usize * CHANNELS];
    engine.push_pcm(Source::AirPlay, 1, 1.0, MASTER_RATE, CHANNELS as u16, &samples);
    thread::sleep(Duration::from_millis(100));

    let status = engine.status();
    let device = &status.devices[0];
    assert!(
        status.dropped_frames > 100_000,
        "overflow not counted, dropped={}",
        status.dropped_frames
    );
    assert!(
        device.buffer_ms <= 320.0,
        "ring not bounded: {} ms",
        device.buffer_ms
    );

    // Flush while the device is still gated: it must clear the ring/counters.
    engine.flush(Source::AirPlay);
    let status = engine.status();
    assert_counters_cleared(&status);
    drop(gate);
}

// ---------------------------------------------------------------------------
// Test 3: hot plug, recovery, prefill and live reconfiguration
// ---------------------------------------------------------------------------

#[test]
fn hotplug_recovery_and_live_config_preserve_other_devices() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![
        Spec::new("mock:a", 48_000.0, 40.0),
        // Real-time pacing so the calibration prefill is observable.
        Spec::new("mock:b", 48_000.0, 1.0),
    ]);
    let settings = audio_settings(vec![
        device_setting("mock:a", MASTER_RATE),
        device_setting("mock:b", MASTER_RATE),
    ]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    assert!(
        wait_until(Duration::from_secs(5), || online(&engine) == 2),
        "devices never came online: {:?}",
        engine.status().devices
    );
    pump(&engine, 60, Duration::from_millis(2));
    assert!(wait_until(Duration::from_secs(2), || {
        let a = device_status(&engine, "mock:a").map(|d| d.written_frames).unwrap_or(0);
        let b = device_status(&engine, "mock:b").map(|d| d.written_frames).unwrap_or(0);
        a > 0 && b > 0
    }));

    // --- unplug B ----------------------------------------------------------
    let a_before = device_status(&engine, "mock:a").unwrap().written_frames;
    provider.set_present("mock:b", false);
    assert!(provider.is_present("mock:b") == false);
    assert!(
        wait_until(Duration::from_secs(3), || {
            device_status(&engine, "mock:b").map(|d| !d.online).unwrap_or(false)
        }),
        "B was not marked offline: {:?}",
        device_status(&engine, "mock:b")
    );
    // A keeps playing (no full rebuild).
    pump(&engine, 40, Duration::from_millis(2));
    let a_after = device_status(&engine, "mock:a").unwrap().written_frames;
    assert!(
        a_after > a_before,
        "device A stopped when B was unplugged ({a_before} -> {a_after})"
    );

    // --- replug B ----------------------------------------------------------
    // Hold the first hardware write until the test samples the prefilled ring.
    // Otherwise a busy runner can miss the transient even when prefill worked.
    provider.set_gate("mock:b", false);
    let gate = GateHolder::adopt(&provider, "mock:b");
    provider.set_present("mock:b", true);
    assert!(
        wait_until(Duration::from_secs(5), || {
            device_status(&engine, "mock:b").map(|d| d.online).unwrap_or(false)
        }),
        "B was not recovered: {:?}",
        device_status(&engine, "mock:b")
    );
    // Reconnect prefill: the ring must be refilled with the calibration/jitter
    // silence before audio resumes.
    let buffer = device_status(&engine, "mock:b").unwrap().buffer_ms;
    assert!(buffer >= 80.0, "reconnect did not prefill the ring ({buffer} ms)");
    drop(gate);
    let b_before = device_status(&engine, "mock:b").unwrap().written_frames;
    pump(&engine, 30, Duration::from_millis(2));
    assert!(
        wait_until(Duration::from_secs(2), || {
            device_status(&engine, "mock:b")
                .map(|d| d.written_frames > b_before)
                .unwrap_or(false)
        }),
        "B did not resume audio after replug"
    );

    // --- live gain/delay change without restarting anything -----------------
    let a_before = device_status(&engine, "mock:a").unwrap().written_frames;
    let b_before = device_status(&engine, "mock:b").unwrap().written_frames;
    let mut changed = settings.clone();
    changed.devices[0].gain_db = -6.0;
    changed.devices[1].delay_ms = 40.0;
    engine.configure(changed.clone()).expect("live configure");
    assert!(online(&engine) == 2, "devices went offline on live configure");
    assert!(
        (device_status(&engine, "mock:a").unwrap().gain_db + 6.0).abs() < 1e-4,
        "gain change not applied"
    );
    assert!(
        (device_status(&engine, "mock:b").unwrap().delay_ms - 40.0).abs() < 1e-4,
        "delay change not applied"
    );
    pump(&engine, 40, Duration::from_millis(2));
    assert!(
        device_status(&engine, "mock:a").unwrap().written_frames > a_before,
        "device A restarted on a gain change (audio lost)"
    );
    assert!(
        device_status(&engine, "mock:b").unwrap().written_frames > b_before,
        "device B restarted on a delay change (audio lost)"
    );

    // --- rate change only restarts the changed device -----------------------
    let b_before = device_status(&engine, "mock:b").unwrap().written_frames;
    changed.devices[0].sample_rate = 44_100;
    engine.configure(changed.clone()).expect("rate re-configure");
    assert!(
        wait_until(Duration::from_secs(5), || {
            device_status(&engine, "mock:a")
                .map(|d| d.online && d.sample_rate == 44_100)
                .unwrap_or(false)
        }),
        "A did not reopen at 44.1 kHz: {:?}",
        device_status(&engine, "mock:a")
    );
    pump(&engine, 40, Duration::from_millis(2));
    assert!(
        device_status(&engine, "mock:b").unwrap().written_frames > b_before,
        "device B was rebuilt when only A changed rate"
    );

    // --- test tone goes through the device thread, not the audio ring ------
    let writes_before = provider.writes("mock:a");
    engine.test_tone("mock:a").expect("test tone");
    assert!(
        wait_until(Duration::from_secs(3), || provider.writes("mock:a") > writes_before),
        "test tone was never written to the device"
    );
}

// ---------------------------------------------------------------------------
// Test 4: devices that are absent at start are recovered by the watchdog
// ---------------------------------------------------------------------------

#[test]
fn offline_device_at_startup_is_recovered_when_it_appears() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![Spec::new("mock:late", 48_000.0, 20.0)]);
    provider.set_present("mock:late", false);
    let settings = audio_settings(vec![device_setting("mock:late", MASTER_RATE)]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    // The UI must show the configured device as offline, not invent one.
    assert!(
        wait_until(Duration::from_millis(500), || {
            device_status(&engine, "mock:late").map(|d| !d.online).unwrap_or(false)
        }),
        "missing device not reported offline"
    );
    assert!(engine.devices().is_empty(), "enumerate invented a device");

    provider.set_present("mock:late", true);
    assert!(
        wait_until(Duration::from_secs(5), || {
            device_status(&engine, "mock:late").map(|d| d.online).unwrap_or(false)
        }),
        "late device was not recovered: {:?}",
        device_status(&engine, "mock:late")
    );
    pump(&engine, 20, Duration::from_millis(2));
    assert!(wait_until(Duration::from_secs(2), || {
        device_status(&engine, "mock:late")
            .map(|d| d.written_frames > 0)
            .unwrap_or(false)
    }));
}

// ---------------------------------------------------------------------------
// Test 5: write() must never block on a device
// ---------------------------------------------------------------------------

#[test]
fn write_is_non_blocking_even_with_a_stalled_device() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![
        Spec::new("mock:stalled", 48_000.0, 1.0),
        Spec::new("mock:healthy", 48_000.0, 20.0),
    ]);
    provider.set_gate("mock:stalled", false);
    let settings = audio_settings(vec![
        device_setting("mock:stalled", MASTER_RATE),
        device_setting("mock:healthy", MASTER_RATE),
    ]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    let gate = GateHolder::adopt(&provider, "mock:stalled");
    assert!(wait_until(Duration::from_secs(5), || online(&engine) == 2));
    // The stalled device thread is blocked inside writei; pushing 1 s of PCM
    // must return quickly anyway (bounded ring, no device I/O on this path).
    let samples = vec![0.05f32; MASTER_RATE as usize * CHANNELS];
    let started = Instant::now();
    for _ in 0..5 {
        engine.push_pcm(Source::AirPlay, 1, 1.0, MASTER_RATE, CHANNELS as u16, &samples);
    }
    let elapsed = started.elapsed();
    // A blocked device write would take >= 1 s (the mock gate has a 10 s safety
    // valve); the write path itself only touches bounded rings.
    assert!(
        elapsed < Duration::from_secs(2),
        "write() blocked for {elapsed:?}"
    );
    // The healthy device still receives audio.
    assert!(wait_until(Duration::from_secs(2), || {
        device_status(&engine, "mock:healthy")
            .map(|d| d.written_frames > 0)
            .unwrap_or(false)
    }));
    // Unknown device ids are an error, not a silent no-op.
    assert!(engine.test_tone("nope").is_err());
    drop(gate);
}


// ---------------------------------------------------------------------------
// Per-device volume curves / gain chain
// ---------------------------------------------------------------------------

fn push_constant(engine: &AudioEngine, value: f32, sender_gain: f32, chunks: usize) {
    let frames = 480usize;
    let mut chunk = Vec::with_capacity(frames * CHANNELS);
    for _ in 0..frames {
        chunk.push(value);
        chunk.push(value);
    }
    for _ in 0..chunks {
        engine.push_pcm(Source::AirPlay, 1, sender_gain, MASTER_RATE, CHANNELS as u16, &chunk);
        thread::sleep(Duration::from_millis(5));
    }
}

/// Mean absolute amplitude of the last 2000 captured samples (steady state,
/// after gain ramps and resampler filters have settled).
fn tail_amplitude(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let tail = &samples[samples.len().saturating_sub(2000)..];
    tail.iter().map(|sample| sample.abs()).sum::<f32>() / tail.len() as f32
}

#[test]
fn per_device_curves_gain_global_and_mute_are_independent() {
    let _serial = serial_guard();
    let provider = MockProvider::new(vec![
        Spec::new("mock:a", MASTER_RATE as f64, SPEED),
        Spec::new("mock:b", MASTER_RATE as f64, SPEED),
    ]);
    let mut device_a = device_setting("mock:a", 0);
    device_a.curve = VolumeCurve { kind: CurveKind::Linear, points: vec![] };
    let mut device_b = device_setting("mock:b", 0);
    // Custom curve maps the top of the sender range to only 50 % amplitude.
    device_b.curve = VolumeCurve {
        kind: CurveKind::Custom,
        points: vec![(0.0, 0.0), (1.0, 0.5)],
    };
    let settings = audio_settings(vec![device_a, device_b]);
    let (engine, _backend) = build_engine(&settings, Arc::clone(&provider));
    assert!(wait_until(Duration::from_secs(5), || online(&engine) == 2));

    // Same sender logical volume (100 %), same raw PCM: each device must apply
    // its own curve. Linear -> 0.5, custom(0.5 at top) -> 0.25.
    push_constant(&engine, 0.5, 1.0, 120);
    settle(&engine);
    let amp_a = tail_amplitude(&provider.captured("mock:a"));
    let amp_b = tail_amplitude(&provider.captured("mock:b"));
    assert!((amp_a - 0.5).abs() < 0.06, "device A (linear) amplitude {amp_a}");
    assert!((amp_b - 0.25).abs() < 0.05, "device B (custom) amplitude {amp_b}");
    assert!(amp_a > amp_b * 1.5, "curves must be independent per device");

    // Global attenuation (50 %) and per-device dB (-6 dB on A only) compose
    // with the per-device curves: A ~= 0.125, B ~= 0.125.
    let mut calibrated = settings.clone();
    calibrated.global_attenuation_percent = 50.0;
    calibrated.devices[0].gain_db = -6.0;
    engine.configure(calibrated.clone()).expect("configure calibrated");
    provider.clear_captured("mock:a");
    provider.clear_captured("mock:b");
    push_constant(&engine, 0.5, 1.0, 120);
    settle(&engine);
    let amp_a = tail_amplitude(&provider.captured("mock:a"));
    let amp_b = tail_amplitude(&provider.captured("mock:b"));
    assert!((amp_a - 0.125).abs() < 0.03, "device A with -6 dB + global 50%: {amp_a}");
    assert!((amp_b - 0.125).abs() < 0.03, "device B with global 50%: {amp_b}");

    // Global mute silences every device, regardless of curves/gains.
    let mut muted = calibrated.clone();
    muted.muted = true;
    engine.configure(muted.clone()).expect("configure muted");
    provider.clear_captured("mock:a");
    provider.clear_captured("mock:b");
    push_constant(&engine, 0.5, 1.0, 80);
    settle(&engine);
    assert!(tail_amplitude(&provider.captured("mock:a")) < 0.005, "global mute A");
    assert!(tail_amplitude(&provider.captured("mock:b")) < 0.005, "global mute B");

    // Sender mute (logical gain 0) wins over local calibration: curves map
    // 0 -> 0 and nothing reaches the devices.
    let mut unmuted = muted.clone();
    unmuted.muted = false;
    engine.configure(unmuted).expect("configure unmuted");
    provider.clear_captured("mock:a");
    push_constant(&engine, 0.5, 0.0, 80);
    settle(&engine);
    assert!(
        tail_amplitude(&provider.captured("mock:a")) < 0.005,
        "sender mute must produce silence"
    );
}
