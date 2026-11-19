//! Minimal HTTP/1.1 server for the fnOS management UI and JSON API.
//!
//! Serves both a Unix domain socket (fnOS unified gateway, `/app/<app>/...`)
//! and an optional TCP listener for local debugging. Requests are handled on
//! detached threads; audio never runs here.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use crate::audio::AudioEngine;
use crate::config::Config;
use crate::logging;
use crate::state::SharedState;
use crate::volume::{CurveKind, VolumeCurve};

pub struct HttpApp {
    pub config_path: PathBuf,
    pub config: RwLock<Config>,
    pub state: SharedState,
    pub engine: Arc<AudioEngine>,
    pub www_dir: PathBuf,
    pub gateway_prefix: String,
    pub started: std::time::Instant,
    /// Protocol hosts; config changes restart them so toggles/name/interface
    /// take effect without a manual service restart.
    pub protocols: Mutex<Option<Arc<crate::protocols::ProtocolHosts>>>,
}

impl HttpApp {
    pub fn save_config(&self, mut config: Config) -> Result<()> {
        // Any write path (config PUT, calibration PUT, import) must migrate
        // v1 exports before validation so a legacy global curve is copied to
        // the devices instead of being silently dropped.
        config.migrate();
        config.validate()?;
        config.save(&self.config_path)?;
        *self.config.write().unwrap() = config;
        Ok(())
    }

    pub fn snapshot(&self) -> Config {
        self.config.read().unwrap().clone()
    }
}

/// Max accepted request body: 1 MiB (config import only).
const MAX_BODY: usize = 1024 * 1024;

pub fn start(
    app: Arc<HttpApp>,
    tcp_addr: Option<std::net::SocketAddr>,
    unix_path: Option<PathBuf>,
) -> Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::new();
    if let Some(path) = unix_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path)
            .with_context(|| format!("无法绑定 Unix socket {}", path.display()))?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
        let app = Arc::clone(&app);
        handles.push(
            std::thread::Builder::new()
                .name("http-unix".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let app = Arc::clone(&app);
                        std::thread::spawn(move || {
                            if let Err(error) = serve_unix(stream, app) {
                                tracing::debug!("unix 请求处理失败：{error:#}");
                            }
                        });
                    }
                })
                .context("无法启动 Unix socket HTTP 线程")?,
        );
    }
    if let Some(addr) = tcp_addr {
        let listener = TcpListener::bind(addr).with_context(|| format!("无法绑定 TCP {addr}"))?;
        let app = Arc::clone(&app);
        handles.push(
            std::thread::Builder::new()
                .name("http-tcp".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let app = Arc::clone(&app);
                        std::thread::spawn(move || {
                            if let Err(error) = serve_tcp(stream, app) {
                                tracing::debug!("tcp 请求处理失败：{error:#}");
                            }
                        });
                    }
                })
                .context("无法启动 TCP HTTP 线程")?,
        );
    }
    Ok(handles)
}

fn serve_unix(mut stream: std::os::unix::net::UnixStream, app: Arc<HttpApp>) -> Result<()> {
    let request = read_request(&mut stream)?;
    handle(request, &mut |status, content_type, body, headers| {
        write_response(&mut stream, status, content_type, body, headers)
    }, app)
}

fn serve_tcp(mut stream: TcpStream, app: Arc<HttpApp>) -> Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
    let request = read_request(&mut stream)?;
    handle(request, &mut |status, content_type, body, headers| {
        write_response(&mut stream, status, content_type, body, headers)
    }, app)
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request<S: Read>(stream: &mut S) -> Result<Request> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let read = stream.read(&mut chunk).context("读取请求失败")?;
        if read == 0 {
            anyhow::bail!("连接提前关闭");
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_subslice(&buffer, b"\r\n\r\n") {
            header_end = position + 4;
            break;
        }
        if buffer.len() > 64 * 1024 {
            anyhow::bail!("请求头过大");
        }
    }
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("GET").to_string();
    let path = first.next().unwrap_or("/").to_string();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_BODY {
        anyhow::bail!("请求体过大");
    }
    let mut body = buffer[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).context("读取请求体失败")?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Request { method, path, body })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn write_response<S: Write>(
    stream: &mut S,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for (name, value) in extra_headers {
        head.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    head.extend_from_slice(b"\r\n");
    head.extend_from_slice(body);
    stream.write_all(&head).context("写响应失败")?;
    stream.flush().ok();
    Ok(())
}

fn respond_json(
    respond: &mut dyn FnMut(u16, &str, &[u8], &[(&str, &str)]) -> Result<()>,
    value: Value,
) -> Result<()> {
    let body = serde_json::to_vec(&value)?;
    respond(200, "application/json; charset=utf-8", &body, &[])
}

fn respond_error(
    respond: &mut dyn FnMut(u16, &str, &[u8], &[(&str, &str)]) -> Result<()>,
    status: u16,
    message: &str,
) -> Result<()> {
    let body = serde_json::to_vec(&json!({"ok": false, "error": message}))?;
    respond(status, "application/json; charset=utf-8", &body, &[])
}

fn request_json(request: &Request) -> Result<Value> {
    if request.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&request.body).context("请求体不是合法 JSON")
}

fn handle(
    request: Request,
    respond: &mut dyn FnMut(u16, &str, &[u8], &[(&str, &str)]) -> Result<()>,
    app: Arc<HttpApp>,
) -> Result<()> {
    let mut path = request.path.split('?').next().unwrap_or("/").to_string();
    // fnOS unified gateway forwards the full path (`/app/<name>/...`): strip it.
    if !app.gateway_prefix.is_empty() {
        if path == app.gateway_prefix {
            // fnOS gateway does not strip the prefix; the UI must load with a
            // trailing slash so its relative asset URLs resolve.
            let location = format!("{}/", app.gateway_prefix);
            return respond(307, "text/plain", b"", &[("Location", location.as_str())]);
        }
        if let Some(rest) = path.strip_prefix(&format!("{}/", app.gateway_prefix)) {
            path = format!("/{rest}");
        }
    }
    if path.is_empty() {
        path = "/".into();
    }
    let method = request.method.clone();
    let request = std::sync::Arc::new(request);
    let method = method.as_str();
    match (method, path.as_str()) {
        ("GET", "/healthz") => respond_json(&mut *respond, json!({
            "status": "ok",
            "version": crate::VERSION,
            "uptime": app.started.elapsed().as_secs(),
        })),
        ("GET", "/api/status") => {
            let config = app.snapshot();
            let runtime = app.state.snapshot();
            let engine = app.engine.status();
            respond_json(&mut *respond, json!({
                "version": crate::VERSION,
                "uptime": app.started.elapsed().as_secs(),
                "started_at": app.state.started.elapsed().as_secs(),
                "protocols": {
                    "airplay": runtime.airplay,
                    "dlna": runtime.dlna,
                    "miplay": runtime.miplay,
                },
                "current_source": runtime.current_source,
                "media": runtime.media,
                "sender_volume": runtime.sender_volume,
                "devices": engine.devices,
                "audio": {
                    "mode": engine.mode,
                    "master_frames": engine.master_frames,
                    "dropped_frames": engine.dropped_frames,
                    "global_attenuation_percent": config.audio.global_attenuation_percent,
                    "muted": config.audio.muted,
                },
                "logging": {"level": logging::level_name()},
                "errors": runtime.errors,
                "last_error": runtime.last_error,
            }))
        }
        ("GET", "/api/config") => respond_json(&mut *respond, json!(app.snapshot())),
        ("PUT", "/api/config") => match request_json(&request).and_then(|value| {
            let mut incoming: Config = serde_json::from_value(value).context("配置字段不合法")?;
            incoming.migrate();
            incoming.validate()?;
            app.save_config(incoming.clone())?;
            app.engine.configure(incoming.audio.clone())?;
            if let Some(hosts) = app.protocols.lock().unwrap().clone() {
                hosts.restart(&incoming)?;
            }
            Ok(incoming)
        }) {
            Ok(config) => respond_json(&mut *respond, json!({"ok": true, "config": config})),
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("GET", "/api/devices") => respond_json(&mut *respond, json!({"devices": app.engine.devices()})),
        ("POST", "/api/devices/refresh") => {
            let config = app.snapshot();
            let result = app.engine.configure(config.audio.clone());
            match result {
                Ok(()) => respond_json(&mut *respond, json!({"ok": true, "devices": app.engine.devices()})),
                Err(error) => respond_error(&mut *respond, 500, &format!("{error:#}")),
            }
        }
        ("POST", "/api/devices/test") => match request_json(&request) {
            Ok(value) => {
                let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
                match app.engine.test_tone(id) {
                    Ok(()) => respond_json(&mut *respond, json!({"ok": true})),
                    Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
                }
            }
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("GET", "/api/calibration") => {
            let config = app.snapshot();
            respond_json(&mut *respond, json!({
                "ok": true,
                "config_version": config.config_version,
                "global_attenuation_percent": config.audio.global_attenuation_percent,
                "muted": config.audio.muted,
                "devices": config.audio.devices,
            }))
        }
        ("PUT", "/api/calibration") => match request_json(&request) {
            Ok(value) => {
                let mut config = app.snapshot();
                if let Some(percent) = value.get("global_attenuation_percent").and_then(|v| v.as_f64()) {
                    config.audio.global_attenuation_percent = percent as f32;
                }
                if let Some(muted) = value.get("muted").and_then(|v| v.as_bool()) {
                    config.audio.muted = muted;
                }
                // Legacy v1 clients send a single global `curve`. v2 has no
                // global curve: apply it to every device (the documented
                // compatibility behaviour) and do not store it globally.
                if let Some(curve) = value.get("curve") {
                    match serde_json::from_value::<VolumeCurve>(curve.clone()) {
                        Ok(parsed) => {
                            if let Err(error) = parsed.validate() {
                                return respond_error(&mut *respond, 400, &error);
                            }
                            for device in &mut config.audio.devices {
                                device.curve = parsed.clone();
                            }
                        }
                        Err(error) => {
                            return respond_error(&mut *respond, 400, &format!("curve 不合法：{error}"));
                        }
                    }
                }
                if let Some(devices) = value.get("devices").and_then(|v| v.as_array()) {
                    let mut unknown = Vec::new();
                    for entry in devices {
                        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else { continue };
                        let Some(device) = config.audio.devices.iter_mut().find(|d| d.id == id) else {
                            unknown.push(id.to_string());
                            continue;
                        };
                        if let Some(gain) = entry.get("gain_db").and_then(|v| v.as_f64()) {
                            device.gain_db = gain as f32;
                        }
                        if let Some(delay) = entry.get("delay_ms").and_then(|v| v.as_f64()) {
                            device.delay_ms = delay as f32;
                        }
                        if let Some(enabled) = entry.get("enabled").and_then(|v| v.as_bool()) {
                            device.enabled = enabled;
                        }
                        if let Some(rate) = entry.get("sample_rate").and_then(|v| v.as_u64()) {
                            device.sample_rate = rate as u32;
                        }
                        if let Some(curve) = entry.get("curve") {
                            match serde_json::from_value::<VolumeCurve>(curve.clone()) {
                                Ok(parsed) => {
                                    if let Err(error) = parsed.validate() {
                                        return respond_error(&mut *respond, 400, &format!("设备 {id} 的曲线不合法：{error}"));
                                    }
                                    device.curve = parsed;
                                }
                                Err(error) => {
                                    return respond_error(&mut *respond, 400, &format!("设备 {id} 的曲线不合法：{error}"));
                                }
                            }
                        }
                    }
                    if !unknown.is_empty() {
                        return respond_error(&mut *respond, 400, &format!("未知设备：{}", unknown.join(", ")));
                    }
                }
                match app.save_config(config.clone()) {
                    Ok(()) => match app.engine.configure(config.audio.clone()) {
                        Ok(()) => respond_json(&mut *respond, json!({
                            "ok": true,
                            "config_version": config.config_version,
                            "global_attenuation_percent": config.audio.global_attenuation_percent,
                            "muted": config.audio.muted,
                            "devices": config.audio.devices,
                        })),
                        Err(error) => respond_error(&mut *respond, 500, &format!("{error:#}")),
                    },
                    Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
                }
            }
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("POST", "/api/calibration/preview") => match request_json(&request) {
            Ok(value) => {
                let device_id = value.get("device_id").and_then(|v| v.as_str()).map(|v| v.to_string());
                let config = app.snapshot();
                let curve = if let Some(curve) = value.get("curve") {
                    match serde_json::from_value::<VolumeCurve>(curve.clone()) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            return respond_error(&mut *respond, 400, &format!("curve 不合法：{error}"));
                        }
                    }
                } else if let Some(id) = device_id.as_deref() {
                    match config.audio.devices.iter().find(|device| device.id == id) {
                        Some(device) => device.curve.clone(),
                        None => return respond_error(&mut *respond, 404, &format!("设备不存在：{id}")),
                    }
                } else {
                    return respond_error(&mut *respond, 400, "需要 curve 或 device_id");
                };
                if let Err(error) = curve.validate() {
                    return respond_error(&mut *respond, 400, &error);
                }
                let samples: Vec<Value> = (0..=100)
                    .map(|i| {
                        let logical = i as f32 / 100.0;
                        json!({"logical": logical, "output": curve.apply(logical)})
                    })
                    .collect();
                respond_json(&mut *respond, json!({
                    "ok": true,
                    "device_id": device_id,
                    "curve": curve,
                    "kind": curve.kind,
                    "points": curve.points,
                    "samples": samples,
                }))
            }
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("GET", "/api/logs") => {
            let requested = request
                .path
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("")
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == "lines")
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(200);
            let lines = requested.clamp(1, 500);
            respond_json(&mut *respond, json!({"lines": logging::recent_lines(lines), "requested": requested}))
        }
        ("POST", "/api/logs/level") => match request_json(&request) {
            Ok(value) => {
                let level = value.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                let mut config = app.snapshot();
                config.logging.level = level.to_string();
                match logging::set_level(level) {
                    Ok(()) => {
                        let _ = app.save_config(config);
                        respond_json(&mut *respond, json!({"ok": true, "level": logging::level_name()}))
                    }
                    Err(error) => respond_error(&mut *respond, 500, &format!("{error:#}")),
                }
            }
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("GET", "/api/export") => respond_json(&mut *respond, json!({"ok": true, "config": app.snapshot()})),
        ("POST", "/api/import") => match request_json(&request) {
            Ok(value) => {
                let incoming = match value.get("config") {
                    Some(config) => serde_json::from_value::<Config>(config.clone()),
                    None => serde_json::from_value::<Config>(value),
                };
                match incoming.context("配置字段不合法") {
                    Ok(mut config) => {
                        config.migrate();
                        match app.save_config(config.clone()) {
                        Ok(()) => match app.engine.configure(config.audio.clone()) {
                            Ok(()) => {
                                if let Some(hosts) = app.protocols.lock().unwrap().clone() {
                                    let _ = hosts.restart(&config);
                                }
                                respond_json(&mut *respond, json!({"ok": true, "config": config}))
                            }
                            Err(error) => respond_error(&mut *respond, 500, &format!("{error:#}")),
                        },
                            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
                        }
                    }
                    Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
                }
            }
            Err(error) => respond_error(&mut *respond, 400, &format!("{error:#}")),
        },
        ("GET", "/api/diagnostics") => {
            let alsa = crate::audio::alsa::diagnostics();
            let interfaces = std::fs::read_dir("/sys/class/net")
                .map(|dir| {
                    dir.flatten()
                        .map(|entry| entry.file_name().to_string_lossy().to_string())
                        .filter(|name| name != "lo")
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            respond_json(&mut *respond, json!({
                "version": crate::VERSION,
                "uptime": app.started.elapsed().as_secs(),
                "interfaces": interfaces,
                "alsa_cards": alsa.cards,
                "alsa_pcm": alsa.pcm,
                "backend": app.engine.status().mode,
                "log_path": logging::path().map(|p| p.display().to_string()),
            }))
        }
        ("GET", artwork) if artwork.starts_with("/artwork/") => {
            let name = &artwork["/artwork/".len()..];
            if !artwork_filename(name) {
                return respond(404, "text/plain", b"not found", &[]);
            }
            serve_static(name, respond, &crate::protocols::state_dir().join("artwork"))
        }
        ("GET", _) => serve_static(&path, respond, &app.www_dir),
        _ => respond_error(&mut *respond, 405, "不支持的请求方法"),
    }
}

/// Only the content-addressed image names emitted by the AirPlay core.
pub(crate) fn artwork_filename(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    matches!(ext, "png" | "jpg") && stem.strip_prefix("cover-art-")
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn serve_static(
    path: &str,
    respond: &mut dyn FnMut(u16, &str, &[u8], &[(&str, &str)]) -> Result<()>,
    www_dir: &Path,
) -> Result<()> {
    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() { "index.html" } else { relative };
    if relative.contains("..") {
        return respond(400, "text/plain", b"bad path", &[]);
    }
    let file = www_dir.join(relative);
    match std::fs::read(&file) {
        Ok(body) => {
            let content_type = match file.extension().and_then(|e| e.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "application/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("jpg") => "image/jpeg",
                Some("png") => "image/png",
                Some("ico") => "image/x-icon",
                Some("json") => "application/json; charset=utf-8",
                _ => "application/octet-stream",
            };
            respond(200, content_type, &body, &[])
        }
        Err(_) => respond(404, "application/json; charset=utf-8", b"{\"error\":\"not found\"}", &[]),
    }
}

/// Parse a `logical` value into a curve preview (used by unit tests).
pub fn preview_points(curve: &VolumeCurve) -> Vec<(f32, f32)> {
    (0..=100)
        .map(|i| {
            let logical = i as f32 / 100.0;
            (logical, curve.apply(logical))
        })
        .collect()
}

/// Convenience for tests: curve kind label.
pub fn curve_label(kind: CurveKind) -> &'static str {
    match kind {
        CurveKind::Linear => "linear",
        CurveKind::Soft => "soft",
        CurveKind::Balanced => "balanced",
        CurveKind::Custom => "custom",
    }
}
