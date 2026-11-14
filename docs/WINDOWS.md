# FusionPlay Windows 版

Windows 10/11（x64）上的 FusionPlay 接收端：把电脑变成支持 **AirPlay 2 / DLNA / 小米妙播（MiPlay）** 的局域网音乐投放接收器。

- 应用：`FusionPlay.exe`（Flutter 界面，与 Android 版同一套 UI）
- 侧车：`FusionPlayCore.exe`（Rust 协议后端，与 UI 同目录，随应用自动启动/退出）
- 数据目录：`%LOCALAPPDATA%\FusionPlay`（设置、日志、封面缓存、MiPlay 身份）

## 安装

### 方式一：安装包（推荐）

运行 `FusionPlay-Setup-<版本>-x64.exe`：

- 安装到 `C:\Program Files\FusionPlay`
- 创建开始菜单快捷方式，可选桌面快捷方式
- 自动为 `FusionPlayCore.exe` 添加**私有/域网络**的 Windows 防火墙入站放行规则（AirPlay 与妙播需要）
- 卸载时保留 `%LOCALAPPDATA%\FusionPlay` 中的设置与日志（如需彻底清理请手动删除）

安装包未做代码签名，Windows SmartScreen 可能提示"未知发布者"；选择"更多信息 → 仍要运行"即可。

### 方式二：便携版

解压 `FusionPlay-Windows-<版本>-x64.zip` 到任意目录，直接运行 `FusionPlay.exe`。

首次运行若没有安装包添加的防火墙规则，Windows 会弹出防火墙授权对话框；**只勾选专用网络**即可（公用网络下不建议开放接收端口）。若误点"取消"，可稍后在"Windows 安全中心 → 防火墙和网络保护 → 允许应用通过防火墙"里为 `FusionPlayCore.exe` 添加入站规则，或重新运行安装包。

## 使用

1. 启动 FusionPlay，界面显示"等待 AirPlay 2 / DLNA / 小米妙播投放"即表示接收器已就绪。
2. 手机/平板/电脑在投放菜单里选择接收器名称（默认是电脑的计算机名，可在设置里自定义）。
3. 支持：
   - iPhone / iPad / Mac：AirPlay 音频
   - 支持 DLNA 的音乐 App、NAS、电视：DLNA 投放（mp3 / flac / wav / aac / m4a / **ogg** 音频，以及**网易云音乐常用的 FLV(AAC) 音频**；QQ 音乐/网易云 CDN 拉流使用浏览器 UA 以规避 403；HTTPS 媒体源、HLS、视频仍不支持）
   - 小米手机/平板：妙播（MiPlay）
4. 设置里可切换接收器名称、开机自启、自动唤醒（播放时阻止系统休眠）、三个协议开关、妙播设备身份（车机/电视/平板/音响）。

### 端口与网络

| 用途 | 端口/协议 |
|---|---|
| AirPlay 2 / DLNA 发现 | UDP 5353（mDNS）、UDP 1900（SSDP） |
| AirPlay 2 音频 | TCP/UDP 动态端口（默认从 7000 起） |
| 小米妙播 | UDP 5353、TCP 8899 及动态 RTP 端口 |

只建议在**可信的私有局域网**使用；Windows 版默认不做跨网段、公网端口映射或鉴权网关。

### 延迟与音画同步

- **妙播（MiPlay）**：1.3.1 起接收端按手机的发送端时钟（timer-server 同步 + TimeOffset）排程播放，并按主机输出延迟做补偿；正常约 0.8–1 秒（与手机系统播放器同步），不再是"到达即播"造成的数秒延迟。
- **AirPlay 2**：默认广告 2 秒缓冲（`Audio-Latency: 88200`，AirPlay 2 协议默认）。想要更低延迟可把 `%LOCALAPPDATA%\FusionPlay\settings.json` 里的 `airPlayLatencyMs` 改成 `500`～`1000`，或用环境变量 `FUSIONPLAY_AIRPLAY_LATENCY_MS=800` 启动侧车；越低越省延迟，但 Wi-Fi 抖动时更容易断音。修改后重启 FusionPlay 生效。
- 输出设备自身延迟另计：蓝牙耳机/音箱通常 +100–300ms，HDMI 电视/显示器 +50–150ms；内置/有线扬声器基本可忽略。
- 诊断：`docs/WINDOWS_ARCHITECTURE.md` 的日志字段里会打印 `audio_synchronization_started`（含 `synchronization_mode=sender_clock`、`queue_delay_ms`、`output_latency_ms`），可用于确认妙播是否进入发送端时钟同步；设置 `FUSIONPLAY_MIPLAY_CLOCK_OBSERVER=1` 会额外写出时钟观测 JSONL 到数据目录。

### 已知限制（1.3.1）

- 仅音频接收：不支持 AirPlay 镜像/视频、DLNA 视频、HLS、DRM 内容；不会向发送端广告视频能力。
- 不支持 Windows 系统媒体控制中心（SMTC）/媒体键，后续版本补充。
- AirPlay 低延迟目前通过 settings.json 字段/环境变量配置，暂未做进设置界面。
- 不支持 DLNA HTTPS 媒体源；本地网络建议使用 HTTP 媒体服务器。
- 数据目录固定为当前用户的 `%LOCALAPPDATA%`，不支持多用户共享一套接收器。
- 同一时间只支持一个活动的音频输出设备（系统默认设备；多设备 fan-out 是 fnOS 版特性）。

## 从源码构建（Windows 10/11）

前置：Flutter 3.32.8、Visual Studio 2022（含"使用 C++ 的桌面开发"）、Rust stable（MSVC 工具链）、PowerShell 7。

```powershell
git clone https://github.com/youyoudezhuzhu/FusionPlay.git
cd FusionPlay
.\flutter\tool\build_windows.ps1
```

产物：

- `dist\FusionPlay-Windows-<版本>-x64.zip`（便携版）
- `dist\FusionPlay-Setup-<版本>-x64.exe`（安装了 Inno Setup 6 时生成）
- `dist\*.sha256`（校验值）

只构建侧车（调试用）：

```powershell
cd windows\backend
cargo test
cargo build --release
```

只构建界面：

```powershell
cd flutter
flutter pub get
flutter build windows --release --dart-define=FUSIONPLAY_VERSION=1.3.0
```

把 `windows\backend\target\release\FusionPlayCore.exe` 拷到 `flutter\build\windows\x64\runner\Release\` 后即可运行 `FusionPlay.exe`。

### 本机（Linux）验证协议侧车

侧车是跨平台 Rust 代码，可以在 Linux 上以 null 音频后端跑通完整 NDJSON 契约：

```bash
cd windows/backend
cargo build
python3 ../../tools/windows_sidecar_smoke.py --exe target/debug/FusionPlayCore
```

## 故障排查

- **手机投放列表里看不到电脑**：确认电脑与手机在同一子网；Windows 防火墙已允许 `FusionPlayCore.exe` 入站；不要禁用"网络发现"所在功能；云桌面/虚拟网卡较多时妙播可能选错网卡（日志里有 `interface_index` 与 `address_resolution_host`）。
- **能发现但无声音**：Windows 声音输出默认设备是否正确；侧车日志 `%LOCALAPPDATA%\FusionPlay\logs\fusionplay.log` 中查找 `audio` 行；无声卡/远程桌面会话会自动回退 null 模式（能发现、无声音）。
- **改名后设备中心出现两个同名/旧名实例**：妙播改名会自动向旧名称发送 goodbye，若仍出现，重启手机端投放面板或重启 FusionPlay。
- **UI 无响应/白屏**：确认 `FusionPlayCore.exe` 与 `FusionPlay.exe` 在同一目录且未被杀毒软件拦截；设置里"导出日志"会生成 `%LOCALAPPDATA%\FusionPlay\exports\FusionPlay-diagnostics-*.txt`。

## 架构与协议细节

见 [WINDOWS_ARCHITECTURE.md](WINDOWS_ARCHITECTURE.md)：进程模型、NDJSON 事件/命令契约、音频链路、状态字段语义、验收标准。
