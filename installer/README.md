# FusionPlay Windows 安装包（Inno Setup 6）

`installer/fusionplay.iss` 把 Flutter Windows 发布目录打包成管理员权限的安装程序：

- 安装到 `{autopf}\FusionPlay`（Program Files）；
- 安装 `FusionPlay.exe`、`FusionPlayCore.exe`、`data\`、`flutter_windows.dll` 及插件 DLL（整个 Release 目录递归）；
- 开始菜单快捷方式（桌面快捷方式为可选任务，默认不勾选）；
- 安装时用 `netsh advfirewall` 给 `FusionPlayCore.exe` 添加 `private,domain` 入站放行规则，
  卸载时删除该规则（AirPlay / DLNA / 小米妙播都从侧车收发网络流量）；
- 卸载时询问是否删除 `%LOCALAPPDATA%\FusionPlay` 用户数据，默认保留；
- 输出文件名 `FusionPlay-Setup-<显示版本>-x64.exe`。

## 前置条件

先构建好 Release 目录（包含侧车）：

```powershell
pwsh -File flutter/tool/build_windows.ps1
# 或只构建不打包安装器
pwsh -File flutter/tool/build_windows.ps1 -SkipInstaller
```

脚本会生成：

```text
flutter/build/windows/x64/runner/Release/
  FusionPlay.exe
  FusionPlayCore.exe        # 由 build_windows.ps1 从 cargo 输出拷入
  data/                     # Flutter assets / icudtl.dat / app.so
  flutter_windows.dll
  ...
```

以及 `dist/FusionPlay-Windows-<ver>-x64.zip` 和 `.sha256`。

## 手动打包安装器

安装 [Inno Setup 6](https://jrsoftware.org/isinfo.php) 后（CI 用 `choco install innosetup -y`），
在**仓库根目录**执行：

```powershell
$version = (Get-Content VERSION -Raw).Trim()
$installerVersion = if (Test-Path INSTALLER_VERSION) { (Get-Content INSTALLER_VERSION -Raw).Trim() } else { "$version.0" }
& 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe' `
  "/DMyAppVersion=$installerVersion" `
  "/DMyAppDisplayVersion=$version" `
  "/O$((Get-Location).Path)\dist" `
  'installer\fusionplay.iss'
```

- `/DMyAppVersion`：4 段数字版本（写进 AppVersion / VERSIONINFO，默认 `1.3.0.0`）。
- `/DMyAppDisplayVersion`：显示版本，用于安装包文件名与 AppVerName（默认 `1.3.0`）。
- `/O<目录>`：输出目录；不传时按脚本里的 `OutputDir=output` 输出到 `installer/output/`（已 gitignore）。

`[Files]` 的 `Source` 是相对 `.iss` 文件位置的
`..\flutter\build\windows\x64\runner\Release\*`，所以 ISCC 从哪个目录调用都可以。

## CI

`.github/workflows/windows.yml` 在 `windows-latest` 上构建侧车和 Flutter 应用、跑侧车冒烟测试、
生成 zip 与 Inno Setup 安装包并上传 artifact；`windows-v*` tag 会把 zip / setup / sha256
挂到同名 GitHub Release（tag `windows-v1.3.0` → Release `FusionPlay Windows 1.3.0`）。

## 签名

1.3.0 的安装包不做代码签名（无证书）。Windows SmartScreen 会提示"未知发布者"，属于预期行为；
后续可加 `SignTool` 配置。
