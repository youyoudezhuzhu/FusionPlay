//! Download an FPK into a folder granted to the app AND writable by this NAS user.
use crate::shared_update::{self, Platform};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd, net::UnixStream},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

fn platform() -> Result<Platform> {
    match std::env::consts::ARCH {
        "aarch64" => Ok(Platform::NasArm),
        "x86_64" => Ok(Platform::NasX86),
        _ => anyhow::bail!("没有为此 NAS 架构发布安装包"),
    }
}
pub fn check() -> Result<Value> {
    shared_update::check(platform()?, crate::VERSION)
}

// The public gateway socket also accepts local clients. Only the gateway web
// accounts/root may assert a different UID. Direct TCP has no gateway identity.
pub fn authentic_user(stream: &UnixStream, uid: u32) -> bool {
    let mut cred = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return false;
    }
    let peer = unsafe { cred.assume_init() }.uid;
    if peer == 0 || peer == uid {
        return true;
    }
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .any(|line| {
            let mut fields = line.split(':');
            matches!(fields.next(), Some("nginx" | "www-data"))
                && fields.nth(1).and_then(|id| id.parse::<u32>().ok()) == Some(peer)
        })
}

fn api(req: &str, data: Value) -> Result<Value> {
    let token = std::env::var("TRIM_API_TOKEN")
        .context("系统未提供文件授权服务，请从飞牛桌面启动应用并检查系统版本")?;
    ensure!(
        !token.is_empty() && !token.contains(['\r', '\n']),
        "系统授权凭据无效"
    );
    // curl handles chunked responses. Feed the secret via stdin, never argv/logs.
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "-q",
            "--silent",
            "--fail",
            "--max-time",
            "8",
            "--noproxy",
            "*",
            "--unix-socket",
            "/var/run/trim_open_gateway_apiscope.socket",
            "--header",
            "Content-Type: application/json",
            "--header",
            "@-",
            "--data-binary",
            &json!({"req":req,"appName":"fusionplay-fnos","data":data}).to_string(),
            "http://localhost/api/v1/trimapp",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    writeln!(child.stdin.take().unwrap(), "Authorization: Bearer {token}")?;
    let out = child.wait_with_output()?;
    ensure!(
        out.status.success(),
        "无法访问飞牛文件授权服务，请检查系统版本和应用权限"
    );
    let result: Value = serde_json::from_slice(&out.stdout)?;
    ensure!(result["code"] == 0, "飞牛文件授权失败 ({})", result["code"]);
    Ok(result["data"].clone())
}

fn check_grants(
    uid: u32,
    directory: &Path,
    call: impl Fn(&str, Value) -> Result<Value>,
) -> Result<()> {
    let grants = call("trim.file.getUserAccessibleFolders", json!({"uid":uid}))?;
    ensure!(
        grants["paths"]
            .as_array()
            .context("飞牛授权列表无效")?
            .iter()
            .filter_map(Value::as_str)
            .any(|p| Path::new(p) == directory),
        "所选目录尚未授权给 FusionPlay，请重新选择并确认授权"
    );
    let acl = call(
        "trim.file.checkUserACL",
        json!({"uid":uid,"path":directory}),
    )?;
    ensure!(
        acl.as_array()
            .context("飞牛权限结果无效")?
            .iter()
            .any(|row| row["path"].as_str().map(Path::new) == Some(directory)
                && row["writable"] == true
                && row["readable"] == true),
        "当前用户没有此目录的读写权限，请选择其他目录"
    );
    Ok(())
}

fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

// Pin each component so replacing a directory with a symlink cannot redirect a download.
fn open_directory(path: &Path) -> Result<File> {
    ensure!(
        path.is_absolute() && path.parent().is_some(),
        "请选择用户存储空间中的目录"
    );
    let mut file = File::open("/")?;
    for c in path.components().skip(1) {
        let Component::Normal(name) = c else {
            anyhow::bail!("所选目录路径无效");
        };
        file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(fd_path(&file).join(name))
            .context("无法打开所选目录，请检查应用目录权限")?;
    }
    Ok(file)
}

pub fn save(
    uid: u32,
    body: &[u8],
    progress: impl FnMut(u64, u64) -> Result<()>,
) -> Result<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Selection {
        tag: String,
        directory: PathBuf,
    }
    let selection: Selection = serde_json::from_slice(body)?;
    let dir = &selection.directory;
    check_grants(uid, dir, api)?;
    let handle = open_directory(dir)?;
    let pinned = fd_path(&handle);
    let pkg = shared_update::resolve(platform()?, crate::VERSION, &selection.tag)?;
    shared_update::download(&pkg, &pinned, progress, || {
        ensure!(
            std::fs::read_link(&pinned)? == *dir,
            "下载期间目录已移动，请重新选择"
        );
        check_grants(uid, dir, api)
    })?;
    Ok(dir.join(pkg.filename))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_both_the_users_grant_and_the_users_write_access() {
        let selected = Path::new("/vol1/home/music");
        for (folder, writable, expected) in [
            ("/vol1/home/music", true, true),
            ("/vol1/home/music", false, false),
            ("/vol1/home/other", true, false),
        ] {
            let result = check_grants(1002, selected, |method, args| {
                assert_eq!(args["uid"], 1002);
                match method {
                    "trim.file.getUserAccessibleFolders" => Ok(json!({"paths":[folder]})),
                    "trim.file.checkUserACL" => {
                        assert_eq!(args["path"], selected.to_str().unwrap());
                        Ok(json!([{"path":selected,"readable":true,"writable":writable}]))
                    }
                    _ => panic!("unexpected system API"),
                }
            });
            assert_eq!(result.is_ok(), expected);
        }
    }
    #[test]
    fn pins_selected_directory_and_rejects_symlinks() {
        let base = tempfile::tempdir().unwrap();
        let folder = base.path().join("音乐");
        std::fs::create_dir(&folder).unwrap();
        let handle = open_directory(&folder).unwrap();
        let mut test = File::create(fd_path(&handle).join("test")).unwrap();
        test.write_all(b"ok").unwrap();
        assert!(folder.join("test").exists());
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        assert!(open_directory(&link).is_err());
        assert!(open_directory(&folder.join("..")).is_err());
        assert!(open_directory(Path::new("somewhere")).is_err());
    }
}
