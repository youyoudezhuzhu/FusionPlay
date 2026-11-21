//! Extra speakers use separate receiver processes: MiPlay's PCM callback and
//! AirPlay's timing state are process-wide. Only the parent owns public HTTP.
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{Config, SpeakerInstance};

pub const PORT_OFFSET: &str = "FUSIONPLAY_RECEIVER_PORT_OFFSET";

fn project(parent: &Config, speaker: &SpeakerInstance) -> Config {
    let mut cfg = parent.clone();
    cfg.receiver.name = speaker.name.trim().to_owned();
    cfg.protocols.airplay.enabled = true;
    cfg.protocols.dlna.enabled = true;
    cfg.protocols.miplay.enabled = true;
    cfg.audio.instances.clear();
    cfg.audio.output_groups.clear();
    let mut output = parent.audio.devices.iter().find(|d| d.id == speaker.device_id)
        .cloned().expect("validated instance owns a configured device");
    output.enabled = true;
    cfg.audio.devices = vec![output];
    cfg
}

struct Worker {
    child: Child,
    socket: PathBuf,
    slot: u16,
    config: Vec<u8>,
}

impl Worker {
    fn update(&mut self, config: Vec<u8>) -> Result<()> {
        if self.config == config { return Ok(()) }
        let (status, _, body) = crate::http::request_worker(&self.socket, "PUT", "/api/config", &config)?;
        anyhow::ensure!(status == 200, "独立音箱保存配置失败：{}", String::from_utf8_lossy(&body));
        self.config = config;
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // Allow mDNS goodbyes and audio device shutdown, then reap even a
            // stuck process before another worker can take its device/ports.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM); }
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if !matches!(self.child.try_wait(), Ok(None)) { break }
                std::thread::sleep(Duration::from_millis(40));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

pub struct SpeakerSupervisor {
    root: PathBuf,
    www: PathBuf,
    workers: Mutex<BTreeMap<String, Worker>>,
}

impl SpeakerSupervisor {
    pub fn new(state_dir: &std::path::Path, www: PathBuf) -> Self {
        Self { root: state_dir.join("instances"), www, workers: Mutex::default() }
    }

    pub fn socket(&self, id: &str) -> Option<PathBuf> {
        self.workers.lock().unwrap().get(id).map(|w| w.socket.clone())
    }

    pub fn stop(&self) {
        self.workers.lock().unwrap().clear();
    }

    pub fn apply(&self, config: &Config) -> Result<()> {
        config.validate()?;
        let mut workers = self.workers.lock().unwrap();
        workers.retain(|id, w| config.audio.instances.iter().any(|s| &s.id == id)
            && matches!(w.child.try_wait(), Ok(None)));
        let mut failures = Vec::new();
        for speaker in &config.audio.instances {
            if let Err(err) = self.reconcile(&mut workers, config, speaker) {
                failures.push(format!("{}: {err:#}", speaker.name));
            }
        }
        anyhow::ensure!(failures.is_empty(), "{}", failures.join(" / "));
        Ok(())
    }

    fn reconcile(&self, workers: &mut BTreeMap<String, Worker>, parent: &Config, speaker: &SpeakerInstance) -> Result<()> {
        let cfg = project(parent, speaker);
        let bytes = serde_json::to_vec(&cfg)?;
        if let Some(worker) = workers.get_mut(&speaker.id) {
            return worker.update(bytes);
        }
        // Keep offsets small enough to avoid other well-known MiPlay service
        // ports, and never reuse a live worker's port pair.
        let slot = (1..=512).find(|i| workers.values().all(|w| w.slot != *i))
            .context("没有可用的音箱端口")?;
        let worker = self.spawn(speaker, slot, cfg, bytes)?;
        workers.insert(speaker.id.clone(), worker);
        Ok(())
    }

    fn spawn(&self, speaker: &SpeakerInstance, slot: u16, cfg: Config, bytes: Vec<u8>) -> Result<Worker> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
        let state = self.root.join(&speaker.id);
        let path = state.join("config.json");
        cfg.save(&path)?;
        // A request resolved before config changes must never land on a
        // different speaker if its port slot has since been reused.
        let socket = self.root.join(format!("{}.sock", speaker.id));
        let _ = std::fs::remove_file(&socket);
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("--config").arg(&path).arg("--state-dir").arg(&state)
            .arg("--www").arg(&self.www).arg("--listen-unix").arg(&socket)
            .env_remove("GATEWAY_SOCKET").env("GATEWAY_PREFIX", "")
            .env("SHAIRPLAY_NO_PTP", "1") // UDP 319/320 belong to the primary receiver.
            .env(PORT_OFFSET, slot.to_string())
            .env("FUSIONPLAY_PARENT_PID", std::process::id().to_string());
        // Avoid allocating one Tokio thread per NAS CPU in every speaker.
        if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
            command.env("TOKIO_WORKER_THREADS", "2");
        }
        let mut worker = Worker { child: command.spawn()?, socket, slot, config: bytes };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(12) {
            if UnixStream::connect(&worker.socket).is_ok() { return Ok(worker) }
            anyhow::ensure!(worker.child.try_wait()?.is_none(), "音箱 {} 启动失败", speaker.name);
            std::thread::sleep(Duration::from_millis(40));
        }
        anyhow::bail!("等待音箱 {} 启动超时", speaker.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceSetting;

    #[test]
    fn one_worker_one_calibrated_output_and_all_protocols() {
        let mut config = Config::default();
        config.protocols.airplay.enabled = false;
        config.protocols.dlna.enabled = false;
        config.protocols.miplay.enabled = false;
        let speaker = SpeakerInstance { id: "study".into(), name: "书房".into(), device_id: "usb".into() };
        config.audio.instances.push(speaker.clone());
        config.audio.devices.push(DeviceSetting { id: "usb".into(), gain_db: -3.0, ..Default::default() });
        let cfg = project(&config, &speaker);
        assert_eq!(cfg.receiver.name, "书房");
        assert!(cfg.protocols.airplay.enabled && cfg.protocols.dlna.enabled && cfg.protocols.miplay.enabled);
        assert!(cfg.audio.instances.is_empty() && cfg.audio.output_groups.is_empty());
        assert_eq!(cfg.audio.devices.len(), 1);
        assert!(cfg.audio.devices[0].enabled);
        assert_eq!(cfg.audio.devices[0].gain_db, -3.0);
        assert!(config.validate().is_ok());
        config.audio.devices[0].enabled = true;
        assert!(config.validate().is_err());
        config.audio.devices.clear();
        assert!(config.validate().is_err());
        config.audio.devices.push(DeviceSetting { id: "usb".into(), ..Default::default() });
        config.audio.instances[0].id = "../study".into();
        assert!(config.validate().is_err());
    }
}
