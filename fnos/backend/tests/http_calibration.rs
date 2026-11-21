//! Integration tests for the device-aware calibration HTTP API.
//!
//! Uses the real HTTP server and a `NullBackend` (no sound card), so this runs
//! on CI. Verifies that per-device curves are stored independently, survive a
//! service restart, and that the legacy global-curve request stays compatible.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use fusionplay_fnos::audio::{AudioEngine, NullBackend};
use fusionplay_fnos::config::{Config, DeviceSetting};
use fusionplay_fnos::http::{self, HttpApp};
use fusionplay_fnos::state::AppState;
use fusionplay_fnos::volume::{CurveKind, VolumeCurve};

fn free_port() -> SocketAddr {
    for _ in 0..50 {
        if let Ok(listener) = TcpListener::bind("127.0.0.1:0") {
            if let Ok(address) = listener.local_addr() {
                drop(listener);
                return address;
            }
        }
    }
    panic!("no free port");
}

struct Http {
    address: SocketAddr,
}

impl Http {
    fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let mut stream = TcpStream::connect(self.address).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let payload = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(head.as_bytes()).expect("write head");
        stream.write_all(payload.as_bytes()).expect("write body");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        let status: u16 = response
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .expect("status code");
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }
}

fn device(id: &str, curve: VolumeCurve) -> DeviceSetting {
    DeviceSetting {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        gain_db: 0.0,
        delay_ms: 0.0,
        sample_rate: 0,
        curve,
    }
}

fn start_app(dir: &Path, devices: Vec<DeviceSetting>) -> (Arc<HttpApp>, Http) {
    start_app_with_supervisor(dir, devices, None)
}

fn start_app_with_supervisor(dir: &Path, devices: Vec<DeviceSetting>,
    instances: Option<Arc<fusionplay_fnos::instances::SpeakerSupervisor>>) -> (Arc<HttpApp>, Http) {
    let config_path = dir.join("config.json");
    let mut config = Config::default();
    config.audio.devices = devices;
    config.save(&config_path).expect("save initial config");
    let config = Config::load(&config_path).expect("load initial config");
    let engine = AudioEngine::new(config.audio.clone(), NullBackend::new()).expect("engine");
    let app = Arc::new(HttpApp {
        instances,
        config_path,
        config: RwLock::new(config),
        state: AppState::new(),
        engine,
        www_dir: dir.join("www"),
        gateway_prefix: "/app/fusionplay-fnos".to_string(),
        started: std::time::Instant::now(),
        protocols: std::sync::Mutex::new(None),
    });
    let address = free_port();
    let _servers = http::start(Arc::clone(&app), Some(address), None).expect("http start");
    std::mem::forget(_servers); // detached threads live for the test process
    (app, Http { address })
}

fn curve(kind: CurveKind, points: Vec<(f32, f32)>) -> VolumeCurve {
    VolumeCurve { kind, points }
}

#[test]
fn receiver_settings_patch_preserves_audio_settings_and_rejects_bad_types() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, http) = start_app(dir.path(), vec![device("hw:a", VolumeCurve::default())]);
    let (status, body) = http.request("PATCH", "/api/config", Some(
        r#"{"name":"NAS 客厅","dlna":false,"device_type":"speaker"}"#,
    ));
    assert_eq!(status, 200, "{body}");
    let updated = Config::load(&dir.path().join("config.json")).unwrap();
    assert_eq!(updated.receiver.name, "NAS 客厅");
    assert!(!updated.protocols.dlna.enabled);
    assert!(updated.protocols.airplay.enabled);
    assert_eq!(updated.protocols.miplay.device_type, "speaker");
    assert_eq!(updated.audio.devices[0].id, "hw:a");
    assert_eq!(http.request("PATCH", "/api/config", Some(
        r#"{"device_type":"unknown"}"#,
    )).0, 400);
}

#[test]
fn device_curves_are_independent_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, http) = start_app(
        dir.path(),
        vec![
            device("hw:a", curve(CurveKind::Linear, vec![])),
            device("hw:b", curve(CurveKind::Balanced, vec![])),
        ],
    );
    let custom = r#"{"kind":"custom","points":[[0.25,0.12],[0.75,0.66]]}"#;
    let payload = format!(
        r#"{{"global_attenuation_percent":80,"muted":false,"devices":[
            {{"id":"hw:a","gain_db":-6,"delay_ms":25,"curve":{custom}}},
            {{"id":"hw:b","gain_db":0,"delay_ms":0,"curve":{{"kind":"soft","points":[]}}}}
        ]}}"#
    );
    let (status, body) = http.request("PUT", "/api/calibration", Some(&payload));
    assert_eq!(status, 200, "{body}");

    // Distinct curves are returned by GET /api/calibration.
    let (status, body) = http.request("GET", "/api/calibration", None);
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let devices = value["devices"].as_array().unwrap();
    let a = devices.iter().find(|d| d["id"] == "hw:a").unwrap();
    let b = devices.iter().find(|d| d["id"] == "hw:b").unwrap();
    assert_eq!(a["curve"]["kind"], "custom");
    let point_y = a["curve"]["points"][1][1].as_f64().unwrap();
    assert!((point_y - 0.66).abs() < 1e-5, "point y = {point_y}");
    assert_eq!(b["curve"]["kind"], "soft");
    assert_eq!(a["gain_db"], -6.0);
    assert_eq!(a["delay_ms"], 25.0);

    // The config file on disk keeps both curves (service restart scenario).
    let loaded = Config::load(&dir.path().join("config.json")).unwrap();
    assert_eq!(loaded.config_version, 2);
    let a = loaded.audio.devices.iter().find(|d| d.id == "hw:a").unwrap();
    let b = loaded.audio.devices.iter().find(|d| d.id == "hw:b").unwrap();
    assert_eq!(a.curve.points, vec![(0.25, 0.12), (0.75, 0.66)]);
    assert_eq!(b.curve.kind, CurveKind::Soft);

    // Preview by device_id uses that device's stored curve.
    let (status, body) = http.request(
        "POST",
        "/api/calibration/preview",
        Some(r#"{"device_id":"hw:a"}"#),
    );
    assert_eq!(status, 200);
    let preview: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(preview["device_id"], "hw:a");
    assert_eq!(preview["curve"]["kind"], "custom");
    let samples = preview["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 101);
    // custom curve: 25 % logical -> ~12 % output.
    let at_25 = &samples[25];
    let output = at_25["output"].as_f64().unwrap();
    assert!((output - 0.12).abs() < 0.01, "preview output {output}");
}

#[test]
fn legacy_global_curve_request_applies_to_all_devices() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, http) = start_app(
        dir.path(),
        vec![
            device("hw:a", curve(CurveKind::Balanced, vec![])),
            device("hw:b", curve(CurveKind::Linear, vec![])),
        ],
    );
    let payload = r#"{"curve":{"kind":"soft","points":[]},"global_attenuation_percent":70}"#;
    let (status, body) = http.request("PUT", "/api/calibration", Some(payload));
    assert_eq!(status, 200, "{body}");
    let loaded = Config::load(&dir.path().join("config.json")).unwrap();
    assert!(loaded.audio.curve.is_default(), "legacy curve must not be stored globally");
    for device in &loaded.audio.devices {
        assert_eq!(device.curve.kind, CurveKind::Soft, "{}", device.id);
    }
    assert!((loaded.audio.global_attenuation_percent - 70.0).abs() < 1e-6);
}

#[test]
fn invalid_device_curve_is_rejected_and_config_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, http) = start_app(dir.path(), vec![device("hw:a", curve(CurveKind::Linear, vec![]))]);
    let payload = r#"{"devices":[{"id":"hw:a","curve":{"kind":"custom","points":[[0.6,0.5],[0.3,0.2]]}}]}"#;
    let (status, _body) = http.request("PUT", "/api/calibration", Some(payload));
    assert_eq!(status, 400);
    let loaded = Config::load(&dir.path().join("config.json")).unwrap();
    assert_eq!(loaded.audio.devices[0].curve.kind, CurveKind::Linear);
}


#[test]
fn import_v1_export_migrates_curves_to_devices() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, http) = start_app(dir.path(), vec![device("hw:a", curve(CurveKind::Linear, vec![]))]);
    let v1 = r#"{"config":{
        "receiver":{"name":"LegacyImport"},
        "audio":{
          "global_attenuation_percent":80,
          "curve":{"kind":"custom","points":[[0.25,0.15],[0.75,0.7]]},
          "devices":[{"id":"hw:a","name":"A","enabled":true,"gain_db":-3,"delay_ms":12,"sample_rate":0},
                     {"id":"hw:b","name":"B","enabled":false,"gain_db":0,"delay_ms":0,"sample_rate":0}]
        }
    }}"#;
    let (status, body) = http.request("POST", "/api/import", Some(v1));
    assert_eq!(status, 200, "{body}");
    let loaded = Config::load(&dir.path().join("config.json")).unwrap();
    assert_eq!(loaded.config_version, 2);
    assert_eq!(loaded.receiver.name, "LegacyImport");
    assert!(loaded.audio.curve.is_default(), "legacy global curve must be cleared");
    for device in &loaded.audio.devices {
        assert_eq!(
            device.curve.points,
            vec![(0.25, 0.15), (0.75, 0.7)],
            "{} did not receive the legacy curve",
            device.id
        );
    }
    let a = loaded.audio.devices.iter().find(|d| d.id == "hw:a").unwrap();
    assert_eq!(a.gain_db, -3.0);
    assert_eq!(a.delay_ms, 12.0);
}

#[test]
fn output_groups_persist_without_changing_calibration_or_receiver_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (app, http) = start_app(dir.path(), vec![
        device("hw:living", VolumeCurve::default()),
        device("hw:kitchen", VolumeCurve::default()),
    ]);
    let mut config = serde_json::to_value(app.snapshot()).unwrap();
    let groups = serde_json::json!([
        {"name":"客厅","devices":["hw:living"]},
        {"name":"全屋","devices":["hw:living","hw:kitchen"]}
    ]);
    config["audio"]["output_groups"] = groups.clone();
    config["audio"]["devices"][1]["enabled"] = false.into();
    config["audio"]["devices"][1]["delay_ms"] = 120.into();
    assert_eq!(http.request("PUT", "/api/config", Some(&config.to_string())).0, 200);
    let saved = serde_json::to_value(Config::load(&app.config_path).unwrap()).unwrap();
    assert_eq!(saved["audio"]["output_groups"], groups);
    assert_eq!(saved["audio"]["devices"][1]["delay_ms"], 120.0);
    assert!(!app.engine.settings().devices[1].enabled);
    assert_eq!(saved["receiver"], config["receiver"]);
}

#[test]
fn catalog_scopes_commands_and_never_falls_back_to_default_for_missing_speakers() {
    let dir = tempfile::tempdir().unwrap();
    let mut usb = device("hw:usb", VolumeCurve::default());
    usb.enabled = false;
    let (app, http) = start_app(dir.path(), vec![device("hw:main", VolumeCurve::default()), usb]);
    let mut cfg = serde_json::to_value(app.snapshot()).unwrap();
    cfg["audio"]["instances"] = serde_json::json!([{"id":"kitchen","name":"Kitchen","device_id":"hw:usb"}]);
    assert_eq!(http.request("PUT", "/api/config", Some(&cfg.to_string())).0, 200);
    let (code, catalog) = http.request("GET", "/app/fusionplay-fnos/api/instances", None);
    assert_eq!(code, 200);
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert_eq!(catalog["default"], "default");
    assert_eq!(catalog["instances"][0]["device_id"], "hw:main");
    assert_eq!(catalog["instances"][1]["name"], "Kitchen");
    assert_eq!(http.request("GET", "/api/instances/default/status", None).0, 200);
    // This fixture deliberately has no worker supervisor. The default route
    // is healthy, but neither status nor commands may leak across instances.
    assert_eq!(http.request("GET", "/api/instances/kitchen/status", None).0, 503);
    let command = Some(r#"{"source":"airplay","command":"play_pause"}"#);
    assert_eq!(http.request("POST", "/api/instances/kitchen/player", command).0, 503);
    assert_eq!(http.request("POST", "/api/instances/removed/player", command).0, 404);
    assert_eq!(http.request("GET", "/api/instances/kitchen/artwork/../config.json", None).0, 404);
    assert_eq!(http.request("GET", "/api/instances/default/config", None).0, 404);

    cfg["audio"]["instances"][0]["device_id"] = "hw:main".into();
    assert_eq!(http.request("PUT", "/api/config", Some(&cfg.to_string())).0, 400);
    assert_eq!(Config::load(&app.config_path).unwrap().audio.instances[0].device_id, "hw:usb");

    // Giving the last default output to an independent speaker removes the
    // dormant default room from the catalog and its scoped control API.
    cfg["audio"]["devices"][0]["enabled"] = false.into();
    assert_eq!(http.request("PUT", "/api/config", Some(&cfg.to_string())).0, 200);
    let catalog: serde_json::Value =
        serde_json::from_str(&http.request("GET", "/api/instances", None).1).unwrap();
    assert_eq!(catalog["default"], "kitchen");
    assert_eq!(catalog["instances"].as_array().unwrap().len(), 1);
    assert_eq!(http.request("GET", "/api/instances/default/status", None).0, 404);
}

#[test]
fn failed_worker_creation_restores_persisted_and_live_device_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A regular file blocks only the new room's private state directory.
    std::fs::create_dir(root.join("instances")).unwrap();
    std::fs::write(root.join("instances/office"), "unavailable").unwrap();
    let supervisor = Arc::new(fusionplay_fnos::instances::SpeakerSupervisor::new(root, root.join("www")));
    let (app, http) = start_app_with_supervisor(root, vec![device("usb", VolumeCurve::default())], Some(supervisor));
    let before = serde_json::to_value(app.snapshot()).unwrap();
    let mut next = before.clone();
    next["audio"]["devices"][0]["enabled"] = false.into();
    next["audio"]["instances"] = serde_json::json!([{"id":"office","name":"Office","device_id":"usb"}]);
    let (status, error) = http.request("PUT", "/api/config", Some(&next.to_string()));
    assert_eq!(status, 400, "{error}");
    assert_eq!(serde_json::to_value(app.snapshot()).unwrap(), before);
    assert_eq!(serde_json::to_value(Config::load(&app.config_path).unwrap()).unwrap(), before);
    assert!(app.engine.settings().devices[0].enabled);
    assert_eq!(http.request("GET", "/api/instances/office/status", None).0, 404);
}

#[test]
fn speaker_curve_writes_follow_persisted_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (_, http) = start_app(dir.path(), vec![device("usb", VolumeCurve::default())]);
    let create = serde_json::json!({"device_id":"usb","name":"Study"}).to_string();
    let (code, response) = http.request("POST", "/api/instances", Some(&create));
    assert_eq!(code, 200, "{response}");
    let body: serde_json::Value = serde_json::from_str(&response).unwrap();
    let route = format!("/api/instances/{}/curve", body["id"].as_str().unwrap());
    let edit = serde_json::json!({"device_id":"usb", "original":VolumeCurve::default(),
        "curve":{"kind":"soft","points":[]}}).to_string();
    assert_eq!(http.request("PUT", "/api/instances/default/curve", Some(&edit)).0, 409);
    assert_eq!(http.request("PUT", &route, Some(&edit)).0, 200);
    assert_eq!(http.request("PUT", &route, Some(&edit)).0, 409);
    assert_eq!(http.request("POST", "/api/instances", Some(&create)).0, 409);
}

#[test]
fn editing_a_room_preserves_identity_calibration_and_other_owners() {
    let dir = tempfile::tempdir().unwrap();
    let soft = VolumeCurve {kind: CurveKind::Soft, points: vec![]};
    let (app, http) = start_app(dir.path(), vec![device("a", VolumeCurve::default()), device("b", soft.clone())]);
    let add = |name, output| {
        let body = serde_json::json!({"name":name, "device_id":output}).to_string();
        let (code, result) = http.request("POST", "/api/instances", Some(&body));
        assert_eq!(code, 200, "{result}");
        serde_json::from_str::<serde_json::Value>(&result).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let id = add("Study", "a");
    let path = format!("/api/instances/{id}/binding");
    let edit = serde_json::json!({"name":"Desk", "device_id":"b", "original":{"name":"Study","device_id":"a"}}).to_string();
    assert_eq!(http.request("PUT", &path, Some(&edit)).0, 200);
    assert_eq!(http.request("PUT", &path, Some(&edit)).0, 409);
    let other = add("Kitchen", "a");
    let conflict = serde_json::json!({"name":"Desk", "device_id":"a", "original":{"name":"Desk","device_id":"b"}}).to_string();
    let (code, message) = http.request("PUT", &path, Some(&conflict));
    assert_eq!(code, 409);
    assert!(message.contains("Kitchen"));
    let cfg = Config::load(&app.config_path).unwrap();
    assert!(cfg.audio.instances.iter().any(|s| s.id == id && s.name == "Desk" && s.device_id == "b"));
    assert!(cfg.audio.instances.iter().any(|s| s.id == other && s.device_id == "a"));
    assert_eq!(cfg.audio.devices.iter().find(|d| d.id == "b").unwrap().curve, soft);
}
