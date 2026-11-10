//! Windows platform integration: data directory, computer name, startup
//! registration, single-instance guard, LAN interface selection, keep-awake and
//! diagnostics export. Ownership: host-state (T2).
//!
//! Every function must compile (and degrade gracefully) on Linux so the crate
//! can be tested on the CI host without a Windows machine. Windows-only code
//! lives in [`windows_impl`]; the non-Windows fallbacks live in [`fallback`].

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// One physical(ish) LAN interface usable for mDNS/DLNA discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanInterface {
    /// Friendly adapter name (`Ethernet`, `Wi-Fi`, `eth0`, `wlan0`, ...).
    pub name: String,
    pub ip: Ipv4Addr,
    /// Colon separated MAC address (`AA:BB:CC:DD:EE:FF`) when known.
    pub mac: Option<String>,
}

/// Holds the single-instance guard. Dropping it releases the guard so another
/// process (or a later test) can acquire it.
pub struct SingleInstance {
    #[cfg(windows)]
    handle: usize,
    #[cfg(not(windows))]
    _lock: std::fs::File,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            windows_impl::release_mutex(self.handle);
        }
        // The non-Windows file lock is released when the file is closed.
    }
}

/// `%LOCALAPPDATA%\FusionPlay` on Windows; XDG state dir elsewhere.
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("FusionPlay");
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("FusionPlay");
        }
        return std::env::temp_dir().join("FusionPlay");
    }
    #[cfg(not(windows))]
    {
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(state).join("fusionplay");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/state/fusionplay");
        }
        std::env::temp_dir().join("fusionplay")
    }
}

/// Device identity shared with the AirPlay/DLNA/MiPlay discovery services.
///
/// `COMPUTERNAME` is the fast path on Windows; `GetComputerNameW` is the
/// authoritative fallback. Linux uses `HOSTNAME`, `/proc/sys/kernel/hostname`
/// and finally the `hostname` command.
pub fn computer_name() -> Option<String> {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim().trim_end_matches('.').to_owned();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    #[cfg(windows)]
    {
        windows_impl::computer_name()
    }
    #[cfg(not(windows))]
    {
        fallback::computer_name()
    }
}

/// Whether the sidecar is registered to start with the current user's session.
pub fn startup_registered() -> bool {
    #[cfg(windows)]
    {
        windows_impl::startup_registered()
    }
    #[cfg(not(windows))]
    {
        fallback::startup_registered()
    }
}

/// Register/unregister `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
///
/// The stored value points at `FusionPlay.exe` next to the sidecar executable
/// (both files are installed into the same directory).
pub fn set_startup_enabled(enabled: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows_impl::set_startup_enabled(enabled)
    }
    #[cfg(not(windows))]
    {
        fallback::set_startup_enabled(enabled)
    }
}

/// Acquire the process-wide single-instance guard.
///
/// Windows uses the named mutex `Local\FusionPlay-Core`; other platforms use an
/// advisory file lock in the temp directory. Returns `None` when another
/// instance already owns the guard.
pub fn acquire_single_instance() -> Option<SingleInstance> {
    #[cfg(windows)]
    {
        windows_impl::acquire_mutex().map(|handle| SingleInstance { handle })
    }
    #[cfg(not(windows))]
    {
        fallback::acquire_file_lock().map(|lock| SingleInstance { _lock: lock })
    }
}

/// Keep the system awake while a stream is playing (Windows only; no-op
/// elsewhere).
pub fn set_keep_awake(awake: bool) {
    #[cfg(windows)]
    {
        windows_impl::set_keep_awake(awake);
    }
    #[cfg(not(windows))]
    {
        let _ = awake;
    }
}

/// Pick the best physical interface for discovery/broadcast.
///
/// Skips loopback, down and virtual adapters without a usable IPv4 address and
/// prefers the adapter that owns the default route (Ethernet over Wi-Fi).
pub fn pick_lan_interface() -> Option<LanInterface> {
    #[cfg(windows)]
    {
        windows_impl::pick_lan_interface()
    }
    #[cfg(not(windows))]
    {
        fallback::pick_lan_interface()
    }
}

/// Write a human-readable diagnostics bundle for `exportLogs`.
///
/// The bundle is a plain-text file under `data_dir/exports/` (no zip
/// dependency): system summary, current settings/state and the log tail. The
/// generated path is returned so the UI can display it in `lastError`.
pub fn export_diagnostics(
    data_dir: &Path,
    log_file: &Path,
    state: &crate::ui_protocol::AppStateView,
) -> anyhow::Result<PathBuf> {
    use std::fmt::Write as _;

    let exports_dir = data_dir.join("exports");
    std::fs::create_dir_all(&exports_dir)
        .map_err(|error| anyhow::anyhow!("无法创建导出目录 {}：{error}", exports_dir.display()))?;
    let timestamp = sanitise_for_file_name(&crate::ui::chrono_like_now());
    let path = exports_dir.join(format!("FusionPlay-diagnostics-{timestamp}.txt"));

    let mut report = String::new();
    let _ = writeln!(report, "FusionPlay diagnostics");
    let _ = writeln!(report, "=====================");
    let _ = writeln!(report, "generated: {}", crate::ui::chrono_like_now());
    let _ = writeln!(report, "version: {}", crate::VERSION);
    let _ = writeln!(
        report,
        "os: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        report,
        "computerName: {}",
        computer_name().unwrap_or_else(|| "unknown".to_owned())
    );
    let _ = writeln!(report, "dataDir: {}", data_dir.display());
    let _ = writeln!(report, "logFile: {}", log_file.display());
    let _ = writeln!(report, "startupRegistered: {}", startup_registered());
    match pick_lan_interface() {
        Some(interface) => {
            let _ = writeln!(
                report,
                "lanInterface: {} ip={} mac={}",
                interface.name,
                interface.ip,
                interface.mac.as_deref().unwrap_or("unknown")
            );
        }
        None => {
            let _ = writeln!(report, "lanInterface: none");
        }
    }

    let _ = writeln!(report, "\n--- settings ---");
    match serde_json::to_string_pretty(&state.settings) {
        Ok(json) => {
            let _ = writeln!(report, "{json}");
        }
        Err(error) => {
            let _ = writeln!(report, "无法序列化设置：{error}");
        }
    }
    let _ = writeln!(report, "\n--- app state ---");
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            let _ = writeln!(report, "{json}");
        }
        Err(error) => {
            let _ = writeln!(report, "无法序列化应用状态：{error}");
        }
    }

    let _ = writeln!(report, "\n--- log tail ({}) ---", log_file.display());
    match read_log_tail(log_file, 2000) {
        Some(tail) => {
            report.push_str(&tail);
            if !tail.ends_with('\n') {
                report.push('\n');
            }
        }
        None => {
            let _ = writeln!(report, "(日志文件不可用)");
        }
    }

    std::fs::write(&path, report)
        .map_err(|error| anyhow::anyhow!("无法写入诊断文件 {}：{error}", path.display()))?;
    tracing::info!("诊断包已导出：{}", path.display());
    Ok(path)
}

fn read_log_tail(log_file: &Path, max_lines: usize) -> Option<String> {
    let bytes = std::fs::read(log_file).ok()?;
    // Bounded read: only the last 4 MiB can contribute the log tail.
    const MAX_BYTES: usize = 4 * 1024 * 1024;
    let start = bytes.len().saturating_sub(MAX_BYTES);
    let text = String::from_utf8_lossy(&bytes[start..]);
    let mut lines: Vec<&str> = text.lines().rev().take(max_lines).collect();
    lines.reverse();
    Some(lines.join("\n"))
}

fn sanitise_for_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(windows)]
mod windows_impl {
    use std::net::Ipv4Addr;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_BUFFER_OVERFLOW, ERROR_FILE_NOT_FOUND,
        ERROR_SUCCESS, GetLastError, HANDLE,
    };
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, GetBestInterface, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
        RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::System::WindowsProgramming::GetComputerNameW;

    use super::LanInterface;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const STARTUP_VALUE_NAME: &str = "FusionPlay";
    const MUTEX_NAME: &str = r"Local\FusionPlay-Core";

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn computer_name() -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut size = buffer.len() as u32;
        // SAFETY: `buffer` is a valid, writable buffer of `size` UTF-16 code
        // units; the API writes at most `size` units and a terminating NUL.
        let ok = unsafe { GetComputerNameW(buffer.as_mut_ptr(), &mut size) };
        if ok == 0 || size == 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&buffer[..size as usize])
            .trim()
            .to_owned();
        (!name.is_empty()).then_some(name)
    }

    pub(super) fn acquire_mutex() -> Option<usize> {
        let name = wide(MUTEX_NAME);
        // SAFETY: `name` is a NUL-terminated UTF-16 string; a null
        // SECURITY_ATTRIBUTES requests default security.
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: `handle` was just returned by CreateMutexW; GetLastError is
        // only read immediately after, as documented.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return None;
        }
        // The raw handle is stored as `usize` so `SingleInstance` stays
        // `Send + Sync` and can live inside the runtime's shared state.
        Some(handle as usize)
    }

    pub(super) fn release_mutex(handle: usize) {
        if handle != 0 {
            // SAFETY: `handle` was returned by CreateMutexW and is released
            // exactly once from `SingleInstance::drop`.
            unsafe { CloseHandle(handle as HANDLE) };
        }
    }

    pub(super) fn set_keep_awake(awake: bool) {
        let flags = if awake {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // SAFETY: SetThreadExecutionState is a pure state flag setter; failure
        // (for example on unsupported systems) is non-fatal.
        unsafe { SetThreadExecutionState(flags) };
    }

    pub(super) fn startup_registered() -> bool {
        let subkey = wide(RUN_KEY);
        let value_name = wide(STARTUP_VALUE_NAME);
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `subkey` is NUL-terminated; `key` receives the opened handle.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return false;
        }
        let mut value_type: REG_VALUE_TYPE = 0;
        let mut size = 0u32;
        // SAFETY: Querying with a null buffer first asks for the required size;
        // `key` is a valid open key handle.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value_name.as_ptr(),
                ptr::null(),
                &mut value_type,
                ptr::null_mut(),
                &mut size,
            )
        };
        // SAFETY: `key` was opened above and is closed exactly once.
        unsafe { RegCloseKey(key) };
        // A REG_SZ needs at least one UTF-16 unit plus the NUL terminator.
        status == ERROR_SUCCESS && value_type == REG_SZ && size > 2
    }

    pub(super) fn set_startup_enabled(enabled: bool) -> anyhow::Result<()> {
        if enabled {
            register_startup()
        } else {
            unregister_startup()
        }
    }

    fn register_startup() -> anyhow::Result<()> {
        let command = startup_command()?;
        let subkey = wide(RUN_KEY);
        let value_name = wide(STARTUP_VALUE_NAME);
        let mut key: HKEY = ptr::null_mut();
        let mut disposition = 0u32;
        // SAFETY: both wide strings are NUL-terminated and `key`/`disposition`
        // are valid out pointers; default security is requested.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                ptr::null(),
                &mut key,
                &mut disposition,
            )
        };
        if status != ERROR_SUCCESS {
            anyhow::bail!("无法打开开机自启注册表项（错误码 {status}）");
        }
        let data = wide(&command);
        let bytes = (data.len() * std::mem::size_of::<u16>()) as u32;
        // SAFETY: `key` is open, `value_name` is NUL-terminated and `data`
        // holds `bytes` bytes including the terminating NUL.
        let status = unsafe {
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast::<u8>(),
                bytes,
            )
        };
        // SAFETY: `key` was opened above and is closed exactly once.
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            anyhow::bail!("无法写入开机自启注册表值（错误码 {status}）");
        }
        tracing::info!("已启用开机自启：{command}");
        Ok(())
    }

    fn unregister_startup() -> anyhow::Result<()> {
        let subkey = wide(RUN_KEY);
        let value_name = wide(STARTUP_VALUE_NAME);
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `subkey` is NUL-terminated; `key` receives the opened handle.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            anyhow::bail!("无法打开开机自启注册表项（错误码 {status}）");
        }
        // SAFETY: `key` is open and `value_name` is NUL-terminated.
        let status = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
        // SAFETY: `key` was opened above and is closed exactly once.
        unsafe { RegCloseKey(key) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            tracing::info!("已关闭开机自启");
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "无法删除开机自启注册表值（错误码 {status}）"
            ))
        }
    }

    fn startup_command() -> anyhow::Result<String> {
        let executable = std::env::current_exe()
            .map_err(|error| anyhow::anyhow!("无法定位侧车可执行文件：{error}"))?;
        let directory = executable
            .parent()
            .ok_or_else(|| anyhow::anyhow!("侧车可执行文件没有父目录"))?;
        Ok(directory
            .join("FusionPlay.exe")
            .to_string_lossy()
            .into_owned())
    }

    struct Candidate {
        if_index: u32,
        if_type: u32,
        interface: LanInterface,
    }

    pub(super) fn pick_lan_interface() -> Option<LanInterface> {
        let candidates = enumerate_adapters();
        let default_route_index = best_interface_index();
        let mut best: Option<(i32, LanInterface)> = None;
        for candidate in candidates {
            let mut score = 0i32;
            if default_route_index == Some(candidate.if_index) {
                score += 100;
            }
            match candidate.if_type {
                IF_TYPE_ETHERNET_CSMACD => score += 20,
                IF_TYPE_IEEE80211 => score += 10,
                _ => {}
            }
            if candidate.interface.mac.is_some() {
                score += 5;
            }
            match &mut best {
                None => best = Some((score, candidate.interface)),
                Some((best_score, _)) if score > *best_score => {
                    best = Some((score, candidate.interface));
                }
                Some(_) => {}
            }
        }
        best.map(|(_, interface)| interface)
    }

    fn best_interface_index() -> Option<u32> {
        // 8.8.8.8 in network byte order: asks the routing table which adapter
        // owns the default route without sending any packet.
        let destination = u32::from_be_bytes([8, 8, 8, 8]);
        let mut index = 0u32;
        // SAFETY: `index` is a valid out pointer; GetBestInterface does not
        // transmit traffic for this call.
        let status = unsafe { GetBestInterface(destination, &mut index) };
        (status == 0).then_some(index)
    }

    fn enumerate_adapters() -> Vec<Candidate> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 16 * 1024;
        for _ in 0..4 {
            // `u64` keeps the buffer aligned for the adapter struct.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8) + 2];
            // SAFETY: `buffer` is at least `size` bytes and correctly aligned;
            // `size` receives the actual required size on overflow.
            let status = unsafe {
                GetAdaptersAddresses(
                    u32::from(AF_UNSPEC),
                    flags,
                    ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            if status == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if status != 0 {
                return Vec::new();
            }
            return parse_adapters(buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>());
        }
        Vec::new()
    }

    fn parse_adapters(first: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        let mut current = first;
        while !current.is_null() {
            // SAFETY: nodes come from the OS-populated linked list and remain
            // valid while `buffer` (owned by the caller) is alive.
            let adapter = unsafe { &*current };
            current = adapter.Next;

            if adapter.OperStatus != IfOperStatusUp {
                continue;
            }
            if adapter.IfType != IF_TYPE_ETHERNET_CSMACD && adapter.IfType != IF_TYPE_IEEE80211 {
                continue;
            }
            let mac_length = usize::try_from(adapter.PhysicalAddressLength)
                .unwrap_or(8)
                .min(adapter.PhysicalAddress.len());
            let mac = format_mac(&adapter.PhysicalAddress[..mac_length]);

            let mut address = adapter.FirstUnicastAddress;
            let mut ipv4 = None;
            while !address.is_null() {
                // SAFETY: unicast nodes belong to the same OS-populated list.
                let unicast = unsafe { &*address };
                address = unicast.Next;
                let socket_address = &unicast.Address;
                if socket_address.lpSockaddr.is_null() {
                    continue;
                }
                // SAFETY: `lpSockaddr` points to a `sockaddr`; the family is
                // checked before casting to `SOCKADDR_IN`.
                let sockaddr = unsafe { &*socket_address.lpSockaddr };
                if sockaddr.sa_family != AF_INET {
                    continue;
                }
                // SAFETY: family is AF_INET, so the pointer is a SOCKADDR_IN.
                let sockaddr_in = unsafe { &*(socket_address.lpSockaddr as *const SOCKADDR_IN) };
                // SAFETY: `S_addr` is the active member of the IN_ADDR union.
                let raw = unsafe { sockaddr_in.sin_addr.S_un.S_addr };
                let address = Ipv4Addr::from(raw.to_ne_bytes());
                if address.is_loopback() || address.is_unspecified() || address.is_link_local() {
                    continue;
                }
                ipv4 = Some(address);
                break;
            }
            let Some(ip) = ipv4 else { continue };

            // SAFETY: adapter fields come from the OS-populated linked list;
            // `Anonymous1.Anonymous.IfIndex` is valid after the length prefix.
            let if_index = unsafe { adapter.Anonymous1.Anonymous.IfIndex };
            let name = wide_ptr_to_string(adapter.FriendlyName)
                .or_else(|| wide_ptr_to_string(adapter.Description))
                .unwrap_or_else(|| format!("adapter{if_index}"));

            candidates.push(Candidate {
                if_index,
                if_type: adapter.IfType,
                interface: LanInterface { name, ip, mac },
            });
        }
        candidates
    }

    fn wide_ptr_to_string(pointer: *const u16) -> Option<String> {
        if pointer.is_null() {
            return None;
        }
        let mut length = 0usize;
        // SAFETY: the pointer targets a NUL-terminated UTF-16 string owned by
        // the adapter list, which outlives this call.
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` was computed by scanning the same buffer.
        let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
        let value = String::from_utf16_lossy(slice).trim().to_owned();
        (!value.is_empty()).then_some(value)
    }

    fn format_mac(bytes: &[u8]) -> Option<String> {
        if bytes.len() < 6 {
            return None;
        }
        let formatted = bytes[..6]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        (formatted != "00:00:00:00:00:00").then_some(formatted)
    }
}

#[cfg(not(windows))]
mod fallback {
    use std::fs::{File, OpenOptions};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::LanInterface;

    static STARTUP_ENABLED: AtomicBool = AtomicBool::new(false);

    pub(super) fn computer_name() -> Option<String> {
        if let Ok(name) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
            let trimmed = name.trim().to_owned();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        let output = std::process::Command::new("hostname").output().ok()?;
        let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!name.is_empty()).then_some(name)
    }

    pub(super) fn startup_registered() -> bool {
        STARTUP_ENABLED.load(Ordering::Relaxed)
    }

    pub(super) fn set_startup_enabled(enabled: bool) -> anyhow::Result<()> {
        // Non-Windows hosts have no registry; keep the flag in memory so the
        // rest of the runtime (and tests) can observe it.
        STARTUP_ENABLED.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn acquire_file_lock() -> Option<File> {
        let path = std::env::temp_dir().join("fusionplay-core-single-instance.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .ok()?;
        file.try_lock().ok()?;
        Some(file)
    }

    pub(super) fn pick_lan_interface() -> Option<LanInterface> {
        let interfaces = if_addrs::get_if_addrs().ok()?;
        let mut best: Option<(i32, LanInterface)> = None;
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            let ip = match &interface.addr {
                if_addrs::IfAddr::V4(address) => address.ip,
                if_addrs::IfAddr::V6(_) => continue,
            };
            if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
                continue;
            }
            let mac = read_mac(&interface.name);
            let mut score = 0i32;
            let lowered = interface.name.to_ascii_lowercase();
            if lowered.starts_with("eth") || lowered.starts_with("en") {
                score += 20;
            } else if lowered.starts_with("wl") {
                score += 10;
            }
            if interface.is_oper_up() {
                score += 5;
            }
            if mac.is_some() {
                score += 5;
            }
            match &mut best {
                None => {
                    best = Some((
                        score,
                        LanInterface {
                            name: interface.name,
                            ip,
                            mac,
                        },
                    ))
                }
                Some((best_score, _)) if score > *best_score => {
                    best = Some((
                        score,
                        LanInterface {
                            name: interface.name,
                            ip,
                            mac,
                        },
                    ));
                }
                Some(_) => {}
            }
        }
        best.map(|(_, interface)| interface)
    }

    fn read_mac(interface_name: &str) -> Option<String> {
        let path = format!("/sys/class/net/{interface_name}/address");
        let mac = std::fs::read_to_string(path).ok()?.trim().to_owned();
        let normalised = mac.to_ascii_uppercase();
        (normalised != "00:00:00:00:00:00" && normalised.split(':').count() == 6)
            .then_some(normalised)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_data_dir_is_absolute_ish_and_stable() {
        let first = default_data_dir();
        assert!(!first.as_os_str().is_empty());
        assert_eq!(first, default_data_dir());
        #[cfg(windows)]
        assert!(first.to_string_lossy().contains("FusionPlay"));
        #[cfg(not(windows))]
        assert!(first.to_string_lossy().contains("fusionplay"));
    }

    #[test]
    fn computer_name_is_available() {
        let name = computer_name().expect("computer name");
        assert!(!name.trim().is_empty());
        assert!(!name.ends_with('.'));
    }

    #[test]
    fn single_instance_is_exclusive_and_reusable_after_drop() {
        let first = acquire_single_instance().expect("first instance");
        assert!(
            acquire_single_instance().is_none(),
            "second acquisition must fail"
        );
        drop(first);
        let again = acquire_single_instance().expect("released guard can be reacquired");
        drop(again);
    }

    #[cfg(not(windows))]
    #[test]
    fn startup_flag_roundtrip_on_non_windows() {
        assert!(!startup_registered());
        set_startup_enabled(true).expect("enable");
        assert!(startup_registered());
        set_startup_enabled(false).expect("disable");
        assert!(!startup_registered());
    }

    #[cfg(windows)]
    #[test]
    fn startup_registry_query_does_not_panic() {
        let _ = startup_registered();
    }

    #[test]
    fn keep_awake_is_callable() {
        set_keep_awake(true);
        set_keep_awake(false);
    }

    #[test]
    fn lan_interface_selection_is_consistent() {
        let interface = pick_lan_interface();
        if let Some(interface) = interface {
            assert!(!interface.ip.is_loopback());
            assert!(!interface.ip.is_unspecified());
            if let Some(mac) = interface.mac {
                assert_eq!(mac.split(':').count(), 6, "unexpected MAC: {mac}");
            }
            assert!(!interface.name.is_empty());
        }
    }

    #[test]
    fn export_diagnostics_writes_settings_state_and_log_tail() {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_dir = directory.path().join("FusionPlay");
        let log_file = data_dir.join("logs/fusionplay.log");
        std::fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        std::fs::write(&log_file, "first log line\nsecond log line\n").unwrap();

        let state = crate::ui_protocol::AppStateView {
            initialized: true,
            receiver_name: "客厅电脑".to_owned(),
            ..crate::ui_protocol::AppStateView::default()
        };

        let path = export_diagnostics(&data_dir, &log_file, &state).expect("export");
        assert!(path.starts_with(data_dir.join("exports")));
        assert!(path.is_file());
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert!(file_name.starts_with("FusionPlay-diagnostics-"));
        assert!(file_name.ends_with(".txt"));
        assert!(!file_name.contains(':'));

        let report = std::fs::read_to_string(&path).unwrap();
        assert!(report.contains("FusionPlay diagnostics"));
        assert!(report.contains(crate::VERSION));
        assert!(report.contains("客厅电脑"));
        assert!(report.contains("second log line"));
        assert!(report.contains("startupRegistered:"));
        assert!(report.contains("lanInterface:"));
        assert!(report.contains("\"schemaVersion\": 8"));
    }

    #[test]
    fn export_diagnostics_tolerates_missing_log_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_dir = directory.path().join("FusionPlay");
        let path = export_diagnostics(
            &data_dir,
            &data_dir.join("logs/fusionplay.log"),
            &crate::ui_protocol::AppStateView::default(),
        )
        .expect("export");
        let report = std::fs::read_to_string(path).unwrap();
        assert!(report.contains("(日志文件不可用)"));
    }

    #[test]
    fn export_diagnostics_keeps_only_the_log_tail() {
        let directory = tempfile::tempdir().expect("temp dir");
        let data_dir = directory.path().join("FusionPlay");
        let log_file = data_dir.join("logs/fusionplay.log");
        std::fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        let mut log = String::new();
        for index in 0..3000 {
            log.push_str(&format!("line-{index}\n"));
        }
        std::fs::write(&log_file, log).unwrap();
        let path = export_diagnostics(
            &data_dir,
            &log_file,
            &crate::ui_protocol::AppStateView::default(),
        )
        .expect("export");
        let report = std::fs::read_to_string(path).unwrap();
        assert!(report.contains("line-2999"));
        assert!(!report.contains("line-0\n"));
    }
}
