//! Windows VERSIONINFO resource for `FusionPlayCore.exe`.
//!
//! The sidecar is shipped next to `FusionPlay.exe`, so Explorer/task manager and
//! the Inno Setup uninstaller show a proper product name and version for it. The
//! numeric PE version comes from the repository root `INSTALLER_VERSION`
//! (always 4 components, for example `1.3.0.0`), while the human readable
//! `FileVersion`/`ProductVersion` strings come from `VERSION` (for example
//! `1.3.0`).
//!
//! Resource compiler lookup order:
//! 1. `RC` environment variable (a full path to `rc.exe`),
//! 2. `rc.exe` on `PATH`,
//! 3. the newest `x64/rc.exe` under `%ProgramFiles(x86)%\Windows Kits\10\bin`,
//! 4. `llvm-rc` / `llvm-rc.exe` on `PATH` (used for Linux cross checking).
//!
//! When no compiler is available the build fails loudly unless
//! `FUSIONPLAY_SKIP_VERSION_RESOURCE=1` is set, which is only meant for local
//! `cargo check` runs without `llvm-rc` installed.
//!
//! Ownership: win-shell (T4).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Product name shown in the Windows file properties dialog.
const PRODUCT_NAME: &str = "FusionPlay";
/// Executable/InternalName of the sidecar binary.
const BINARY_NAME: &str = "FusionPlayCore";
/// Fallback when the repository root `INSTALLER_VERSION` file is missing.
const DEFAULT_INSTALLER_VERSION: &str = "1.3.0.0";
/// Fallback when the repository root `VERSION` file is missing.
const DEFAULT_DISPLAY_VERSION: &str = "1.3.0";

fn main() {
    println!("cargo:rerun-if-changed=../../VERSION");
    println!("cargo:rerun-if-changed=../../INSTALLER_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=FUSIONPLAY_SKIP_VERSION_RESOURCE");

    // The version resource is Windows-only; every other platform (including a
    // plain Linux host build) simply has no PE resources to embed.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let display_version = read_version_file("../../VERSION", DEFAULT_DISPLAY_VERSION);
    let installer_version =
        read_version_file("../../INSTALLER_VERSION", DEFAULT_INSTALLER_VERSION);
    let numeric_version = numeric_version(&installer_version);

    let output_directory =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR"));
    let resource_script = output_directory.join("fusionplay-core-version.rc");
    let compiled_resource = output_directory.join("fusionplay-core-version.res");

    fs::write(
        &resource_script,
        version_resource(&display_version, numeric_version),
    )
    .expect("unable to write the FusionPlayCore VERSIONINFO resource");

    let Some(resource_compiler) = find_resource_compiler() else {
        if skip_version_resource() {
            println!(
                "cargo:warning=FusionPlayCore VERSIONINFO resource skipped because no resource \
                 compiler (rc.exe/llvm-rc) was found and FUSIONPLAY_SKIP_VERSION_RESOURCE=1 is set"
            );
            return;
        }
        panic!(
            "no Windows resource compiler found for FusionPlayCore.exe VERSIONINFO: set RC to \
             rc.exe, put rc.exe on PATH, install the Windows SDK, or install llvm-rc. Set \
             FUSIONPLAY_SKIP_VERSION_RESOURCE=1 to build without an embedded version resource \
             (not for release builds)."
        );
    };

    // rc.exe accepts `/nologo`; llvm-rc accepts it as well (MSVC compatible).
    let output = Command::new(&resource_compiler)
        .arg("/nologo")
        .arg(format!("/fo{}", compiled_resource.display()))
        .arg(&resource_script)
        .output()
        .expect("unable to start the Windows resource compiler");
    if !output.status.success() {
        panic!(
            "Windows resource compilation failed with {}:\n{}{}",
            resource_compiler.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    println!("cargo:rustc-link-arg-bins={}", compiled_resource.display());
}

/// Reads a repository version file, falling back to `default` when it is
/// missing, unreadable or empty.
fn read_version_file(relative_path: &str, default: &str) -> String {
    let value = fs::read_to_string(relative_path)
        .map(|content| content.trim().to_owned())
        .unwrap_or_default();
    if value.is_empty() {
        default.to_owned()
    } else {
        value
    }
}

/// `FUSIONPLAY_SKIP_VERSION_RESOURCE=1` opts out of the version resource when no
/// compiler exists (local `cargo check` on a machine without llvm-rc).
fn skip_version_resource() -> bool {
    matches!(
        env::var("FUSIONPLAY_SKIP_VERSION_RESOURCE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

/// Converts `MAJOR.MINOR.PATCH[.BUILD][-anything]` into the 4-part numeric
/// version Windows resource scripts require. Missing or non-numeric components
/// become zero, so malformed input never produces an uncompilable `.rc`.
fn numeric_version(display_version: &str) -> [u16; 4] {
    let mut result = [0_u16; 4];
    for (index, component) in display_version.split('.').take(4).enumerate() {
        let digits: String = component
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        result[index] = digits.parse().unwrap_or(0);
    }
    result
}

/// Renders the VERSIONINFO resource script consumed by rc.exe / llvm-rc.
fn version_resource(display_version: &str, version: [u16; 4]) -> String {
    let escaped_version = display_version.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},{revision}
PRODUCTVERSION {major},{minor},{patch},{revision}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x00040004L
FILETYPE 0x00000001L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "FusionPlay Community\0"
            VALUE "FileDescription", "{product}\0"
            VALUE "FileVersion", "{display_version}\0"
            VALUE "InternalName", "{binary}\0"
            VALUE "OriginalFilename", "{binary}.exe\0"
            VALUE "ProductName", "{product}\0"
            VALUE "ProductVersion", "{display_version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#,
        major = version[0],
        minor = version[1],
        patch = version[2],
        revision = version[3],
        product = PRODUCT_NAME,
        binary = BINARY_NAME,
        display_version = escaped_version,
    )
}

/// Locates a Windows resource compiler, preferring the official Windows SDK
/// `rc.exe` and falling back to `llvm-rc` for Linux cross checks.
fn find_resource_compiler() -> Option<PathBuf> {
    if let Some(configured) = env::var_os("RC") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(rc) = find_on_path(&["rc.exe", "rc"]) {
        return Some(rc);
    }

    if let Some(rc) = find_windows_sdk_rc() {
        return Some(rc);
    }

    find_on_path(&["llvm-rc", "llvm-rc.exe"])
}

/// Looks up an executable on `PATH`.
fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path_value = env::var_os("PATH")?;
    for directory in env::split_paths(&path_value) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Finds the newest `x64/rc.exe` under `%ProgramFiles(x86)%\Windows Kits\10\bin`.
fn find_windows_sdk_rc() -> Option<PathBuf> {
    let program_files = env::var_os("ProgramFiles(x86)")?;
    let sdk_bin = Path::new(&program_files).join("Windows Kits/10/bin");
    let mut candidates = fs::read_dir(sdk_bin)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64/rc.exe"))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    // SDK versions sort lexicographically ("10.0.22621.0" < "10.0.26100.0"),
    // so the last entry is the newest installed SDK.
    candidates.sort();
    candidates.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_version_is_padded_to_four_components() {
        assert_eq!(numeric_version("1.3.0.0"), [1, 3, 0, 0]);
        assert_eq!(numeric_version("1.3"), [1, 3, 0, 0]);
        assert_eq!(numeric_version("2.0.1.7"), [2, 0, 1, 7]);
        assert_eq!(numeric_version(""), [0, 0, 0, 0]);
    }

    #[test]
    fn resource_uses_fusionplay_names() {
        let resource = version_resource("1.3.0", numeric_version("1.3.0.0"));
        assert!(resource.contains("FILEVERSION 1,3,0,0"));
        assert!(resource.contains("PRODUCTVERSION 1,3,0,0"));
        assert!(resource.contains("VALUE \"FileDescription\", \"FusionPlay\\0\""));
        assert!(resource.contains("VALUE \"ProductName\", \"FusionPlay\\0\""));
        assert!(resource.contains("VALUE \"InternalName\", \"FusionPlayCore\\0\""));
        assert!(resource.contains("VALUE \"OriginalFilename\", \"FusionPlayCore.exe\\0\""));
        assert!(resource.contains("VALUE \"FileVersion\", \"1.3.0\\0\""));
        assert!(resource.contains("VALUE \"ProductVersion\", \"1.3.0\\0\""));
    }

    #[test]
    fn display_version_is_escaped() {
        let resource = version_resource("1.3.0 \"beta\"", [1, 3, 0, 0]);
        assert!(resource.contains("VALUE \"FileVersion\", \"1.3.0 \\\"beta\\\"\\0\""));
    }
}
