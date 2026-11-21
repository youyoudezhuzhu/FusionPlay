//! A short-lived helper, independent of the running receiver and its single-instance lock.
use anyhow::{ensure, Context, Result};
use fusionplay_windows::{
    shared_update::{self, Platform},
    VERSION,
};
use serde_json::{json, Value};
use std::{ffi::OsString, io::Write, path::Path};

fn event(value: Value) -> Result<()> {
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(())
}

pub fn run(args: &[OsString]) -> Result<()> {
    match args {
        [cmd] if cmd == "--check-update" => {
            event(shared_update::check(Platform::Windows, VERSION)?)
        }
        [cmd, tag, directory] if cmd == "--save-update" => {
            let dir = Path::new(directory);
            ensure!(dir.is_absolute(), "请选择完整目录");
            let pkg = shared_update::resolve(
                Platform::Windows,
                VERSION,
                tag.to_str().context("版本无效")?,
            )?;
            shared_update::download(
                &pkg,
                dir,
                |bytes, total| event(json!({"bytes":bytes,"total":total})),
                || Ok(()),
            )?;
            event(json!({"path":dir.join(pkg.filename)}))
        }
        _ => anyhow::bail!("无效的更新参数"),
    }
}
