//! Stable desktop installers published by the FusionPlay release workflows.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const REPO: &str = "https://github.com/youyoudezhuzhu/FusionPlay";
const API: &str = "https://api.github.com/repos/youyoudezhuzhu/FusionPlay/releases";
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Platform {
    Windows,
    NasX86,
    NasArm,
}

impl Platform {
    fn prefix(self) -> &'static str {
        match self {
            Self::Windows => "windows-v",
            _ => "fnos-v",
        }
    }
    fn names(self, version: &str) -> (String, String) {
        match self {
            Self::Windows => {
                let name = format!("FusionPlay-Setup-{version}-x64.exe");
                (name.clone(), name)
            }
            Self::NasX86 => (
                "fusionplay-fnos.fpk".into(),
                format!("fusionplay-fnos-{version}-x86_64.fpk"),
            ),
            Self::NasArm => (
                "fusionplay-fnos-arm64.fpk".into(),
                format!("fusionplay-fnos-{version}-arm64.fpk"),
            ),
        }
    }
}

fn version(s: &str) -> Option<[u32; 3]> {
    let mut parts = s.split('.');
    let mut result = [0; 3];
    for part in &mut result {
        let text = parts.next()?;
        *part = text.parse().ok()?;
        if part.to_string() != text {
            return None;
        }
    }
    parts.next().is_none().then_some(result)
}

pub struct Package {
    pub tag: String,
    pub filename: String,
    url: String,
    digest: String,
    bytes: u64,
}

impl Package {
    fn from_release(platform: Platform, release: &Value, current: &str) -> Result<Self> {
        ensure!(
            release["draft"] == false && release["prerelease"] == false,
            "需要正式发布的版本"
        );
        let tag = release["tag_name"].as_str().context("缺少版本标签")?;
        let label = tag
            .strip_prefix(platform.prefix())
            .context("安装包平台不匹配")?;
        ensure!(
            version(label).context("版本号无效")?
                > version(current.trim()).context("当前版本号无效")?,
            "已是最新版本"
        );
        let (asset_name, filename) = platform.names(label);
        let asset = release["assets"]
            .as_array()
            .context("缺少发布附件")?
            .iter()
            .find(|v| v["name"] == asset_name)
            .context("该版本尚未发布对应架构的安装包")?;
        let bytes = asset["size"].as_u64().context("缺少安装包大小")?;
        ensure!((1..=MAX_PACKAGE).contains(&bytes), "安装包大小无效");
        let digest = asset["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .context("发布附件缺少 SHA-256")?;
        ensure!(
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
            "安装包 SHA-256 无效"
        );
        Ok(Self {
            tag: tag.to_owned(),
            filename,
            bytes,
            digest: digest.to_ascii_lowercase(),
            url: format!("{REPO}/releases/download/{tag}/{asset_name}"),
        })
    }
}

fn curl() -> Command {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let system = std::env::var_os("SystemRoot")
            .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows"));
        let mut c = Command::new(std::path::PathBuf::from(system).join(r"System32\curl.exe"));
        c.creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.args([
        "-q",
        "--fail",
        "--silent",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "12",
        "--user-agent",
        "FusionPlay-updater",
    ]);
    cmd
}

fn metadata(url: &str) -> Result<Value> {
    let output = curl()
        .args(["--max-time", "40", "--max-filesize", "8388608", url])
        .output()
        .context("无法运行 curl 检查 GitHub 发布")?;
    ensure!(
        output.status.success(),
        "无法读取 GitHub 发布信息 ({})",
        output.status
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn check(platform: Platform, current: &str) -> Result<Value> {
    let installed = version(current.trim()).context("当前版本号无效")?;
    let mut latest = installed;
    let mut selected = None;
    for page in 1.. {
        let data = metadata(&format!("{API}?per_page=100&page={page}"))?;
        let rows = data.as_array().context("发布列表无效")?;
        for release in rows {
            if release["draft"] != false || release["prerelease"] != false {
                continue;
            }
            let Some(label) = release["tag_name"]
                .as_str()
                .and_then(|s| s.strip_prefix(platform.prefix()))
            else {
                continue;
            };
            if let Some(v) = version(label).filter(|v| *v > latest) {
                latest = v;
                selected = Some((label.to_owned(), release.clone()));
            }
        }
        if rows.len() < 100 {
            break;
        }
    }
    let Some((label, release)) = selected else {
        return Ok(json!({"status":"up_to_date"}));
    };
    let package = Package::from_release(platform, &release, current)?;
    Ok(
        json!({"status":"available","releaseKey":package.tag,"versionName":label,
        "filename":package.filename,"apkSize":package.bytes,"publishedAt":release["published_at"],"notes":release["body"]}),
    )
}

pub fn resolve(platform: Platform, current: &str, tag: &str) -> Result<Package> {
    let target = tag
        .strip_prefix(platform.prefix())
        .and_then(version)
        .context("无效的更新版本")?;
    ensure!(
        target > version(current.trim()).context("当前版本号无效")?,
        "该版本无需下载"
    );
    let release = metadata(&format!("{API}/tags/{tag}"))?;
    ensure!(release["tag_name"] == tag, "版本已改变，请重新检查更新");
    Package::from_release(platform, &release, current)
}

pub fn download(
    pkg: &Package,
    dir: &Path,
    progress: impl FnMut(u64, u64) -> Result<()>,
    authorize_publish: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure!(dir.is_absolute() && dir.is_dir(), "请选择可访问的目录");
    let dest = dir.join(&pkg.filename);
    ensure!(
        !dest.try_exists()?,
        "目录内已有同名安装包，请选择其他目录或移走旧文件"
    );
    let mut temp = tempfile::Builder::new()
        .prefix(".fusionplay-")
        .suffix(".part")
        .tempfile_in(dir)
        .context("无法在所选目录创建文件，请检查写权限和可用空间")?;
    let mut child = curl()
        .args([
            "--max-time",
            "900",
            "--speed-time",
            "30",
            "--speed-limit",
            "128",
            &pkg.url,
        ])
        .stdout(Stdio::piped())
        .spawn()
        .context("无法启动下载")?;
    let copied = copy_verified(
        pkg,
        BufReader::new(child.stdout.take().unwrap()),
        &mut temp,
        progress,
    );
    if copied.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    copied?;
    ensure!(status.success(), "下载中断 ({status})");
    temp.as_file().sync_all()?;
    authorize_publish()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))?;
    }
    temp.persist_noclobber(dest)
        .context("保存安装包失败，请检查权限、可用空间和同名文件")?;
    Ok(())
}

fn copy_verified(
    pkg: &Package,
    mut src: impl Read,
    mut out: impl Write,
    mut progress: impl FnMut(u64, u64) -> Result<()>,
) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut buf = [0; 64 * 1024];
    let mut bytes = 0;
    let mut last = Instant::now();
    progress(0, pkg.bytes)?;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        ensure!(bytes <= pkg.bytes, "安装包超过发布大小");
        out.write_all(&buf[..n])
            .context("写入失败，请检查磁盘空间和写权限")?;
        hasher.update(&buf[..n]);
        if bytes == pkg.bytes || last.elapsed() >= Duration::from_millis(200) {
            progress(bytes, pkg.bytes)?;
            last = Instant::now();
        }
    }
    ensure!(
        bytes == pkg.bytes && format!("{:x}", hasher.finalize()) == pkg.digest,
        "安装包大小或 SHA-256 不匹配，请重试"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str, name: &str) -> Value {
        json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":[
            {"name":name,"size":3,"digest":format!("sha256:{:x}",Sha256::digest(b"fpk"))}]})
    }
    #[test]
    fn pins_platform_version_and_architecture() {
        let arm = release("fnos-v0.2.10", "fusionplay-fnos-arm64.fpk");
        let pkg = Package::from_release(Platform::NasArm, &arm, "0.2.9").unwrap();
        assert_eq!(pkg.filename, "fusionplay-fnos-0.2.10-arm64.fpk");
        assert!(Package::from_release(Platform::NasX86, &arm, "0.2.9").is_err());
        assert!(Package::from_release(Platform::NasArm, &arm, "0.2.10").is_err());
        let win = release("windows-v1.10.0", "FusionPlay-Setup-1.10.0-x64.exe");
        assert!(Package::from_release(Platform::Windows, &win, "1.9.9").is_ok());
        let zip = release("windows-v1.10.0", "FusionPlay-Windows-1.10.0-x64.zip");
        assert!(Package::from_release(Platform::Windows, &zip, "1.9.9").is_err());
        let mut beta = win.clone();
        beta["prerelease"] = true.into();
        assert!(Package::from_release(Platform::Windows, &beta, "1.9.9").is_err());
        assert!(version("1.10.0-beta").is_none());
    }

    #[test]
    fn rejects_wrong_digest_partial_and_oversized_packages() {
        let pkg = Package::from_release(
            Platform::NasX86,
            &release("fnos-v1.0.0", "fusionplay-fnos.fpk"),
            "0.9.0",
        )
        .unwrap();
        let mut counts = vec![];
        copy_verified(&pkg, &b"fpk"[..], Vec::new(), |n, t| {
            counts.push((n, t));
            Ok(())
        })
        .unwrap();
        assert_eq!(counts, [(0, 3), (3, 3)]);
        for bytes in [&b"bad"[..], b"fp", b"fpks"] {
            assert!(copy_verified(&pkg, bytes, Vec::new(), |_, _| Ok(())).is_err());
        }
    }

    /// Opt-in release smoke test. Downloads published files into a disposable directory.
    #[test]
    #[ignore]
    fn saves_published_packages_without_overwriting() {
        for (platform, tag) in [
            (Platform::Windows, "windows-v1.3.2"),
            (Platform::NasX86, "fnos-v0.2.5"),
        ] {
            let pkg = resolve(platform, "0.0.0", tag).unwrap();
            let dir = tempfile::tempdir().unwrap();
            assert!(download(
                &pkg,
                dir.path(),
                |_, _| Ok(()),
                || anyhow::bail!("授权已撤销")
            )
            .is_err());
            assert_eq!(dir.path().read_dir().unwrap().count(), 0);
            download(&pkg, dir.path(), |_, _| Ok(()), || Ok(())).unwrap();
            assert_eq!(
                dir.path().join(&pkg.filename).metadata().unwrap().len(),
                pkg.bytes
            );
            assert!(download(&pkg, dir.path(), |_, _| Ok(()), || Ok(())).is_err());
        }
    }
}
