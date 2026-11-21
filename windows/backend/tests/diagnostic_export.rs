use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn failed_fire_and_forget_commands_reach_the_exported_report() {
    let dir = tempfile::tempdir().unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_FusionPlayCore"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["--null-audio", "--no-airplay", "--no-dlna", "--no-miplay"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut input = process.stdin.take().unwrap();
        writeln!(input, r#"{{"command":"volume"}}"#).unwrap();
        for _ in 0..5 {
            writeln!(input, r#"{{"command":"exportLogs"}}"#).unwrap();
        }
        writeln!(input, r#"{{"command":"shutdown"}}"#).unwrap();
    }
    assert!(process.wait().unwrap().success());
    let reports: Vec<_> = dir.path().join("exports").read_dir().unwrap().collect();
    assert_eq!(reports.len(), 3);
    for entry in reports {
        let report = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(report.contains("ERROR command=volume volume 缺少 percent"));
    }
}
