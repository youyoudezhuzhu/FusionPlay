# FusionPlay for fnOS（飞牛 NAS）

把 **AirPlay 2 / DLNA / 小米妙播（MiPlay）** 的音频接到飞牛 NAS 的音频输出设备：
统一多声卡输出、发送端主音量、每设备增益/延迟校准，并提供中文 Web 管理面板。

| 项目 | 值 |
|---|---|
| 应用标识（appname） | `fusionplay-fnos` |
| 当前版本 | `0.2.5`（x86_64） |
| 安装包 | `fusionplay-fnos.fpk`（GitHub Actions artifact / Release 附件） |
| 服务端口 | `18080`（仅绑定 `127.0.0.1`，供本机诊断；对外走 fnOS 统一网关） |
| 运行身份 | `run-as: package` + `join-groups: ["audio"]`（**非 root**） |
| 许可证 | AGPL-3.0-only（包内 `app/licenses/LICENSE`；源码：<https://github.com/youyoudezhuzhu/FusionPlay>） |

> FusionPlay 是**接收端**：手机/平板把音频「投放」给 NAS，NAS 再通过声卡 / HDMI / USB 解码器输出。
> 安装包与构建测试说明见 [fnos-build.md](fnos-build.md)；逐步安装与真机验收清单见 [INSTALL.md](INSTALL.md)。

## 1. 功能边界

### 支持

- **AirPlay 2 音频**：配对、播放/暂停、切歌、进度、发送端音量、封面/元数据（复用上游协议栈）。
- **DLNA MediaRenderer（控制面）**：SSDP 发现、AVTransport / RenderingControl、事件回传；媒体侧支持 mp3 / flac / wav / aac / mp4 / **ogg** 音频与**网易云常用的 FLV(AAC) 音频**（QQ 音乐/网易云 CDN 拉流使用浏览器 UA 规避 403）；
  媒体由本机 HTTP(S) 拉流解码（MP3 / FLAC / WAV / AAC / MP4 音频轨）后输出。
- **小米妙播（MiPlay）**：设备发现、鉴权、媒体接收；默认开启旧版固定密钥兼容（兼容老发送端）。
- **统一多声卡输出**：48 kHz 立体声主混音 → 每设备独立 ring + 重采样 + 漂移/延迟补偿；
  单声卡与多声卡走同一条路径。
- **音量与校准**：发送端主音量 → **每个音箱独立**音量曲线（线性/柔和/均衡/自定义）→ 每设备 dB →
  全局衰减 → 全局静音；每设备可独立设置采样率与延迟补偿。
- **配置迁移**：旧版的全局音量曲线在首次启动/导入时自动复制到各设备（幂等，不覆盖已有独立曲线）。
- **三协议仲裁**：同一时刻只有一个主音源，接管时挂起旧协议并 flush 队列。
- **Web 管理面板**（中文、响应式、浅色/深色/跟随系统）：四个页面 —— 概览、音频、接收、设置；
  音频页左侧设备列表 + 右侧选中设备的完整校准（响度/延迟/独立曲线），设置页含诊断与日志。

### 明确不支持（不会「假装成功」）

- AirPlay 2 **多房间采样级同步**（与 HomePod 等组成多房间不在本版范围）。
- DLNA **HLS / 视频 / 转码**（首版只做音频；视频投屏会明确报错）。
- 小米官方认证 / 厂商准入宣称（MiPlay 为兼容实现，失败会在日志与 UI 如实上报）。
- **ARM 安装包**（当前发行包 `platform = x86`；ARM 版需交叉编译后另出包）。
- 蓝牙 / PulseAudio / 网络音频输出（只走 ALSA `hw:` 设备）。
- 加密 / DRM 媒体、跨 NAS 组播、多实例同时运行。

## 2. 安装

> 详细步骤（含应用中心图形界面、CLI、安装后验收、真机验证清单）见 [INSTALL.md](INSTALL.md)。

- **方式 A（推荐）应用中心**：下载 `fusionplay-fnos.fpk` → 应用中心 → 手动安装 →
  选择文件 → 安全提示点「同意」。存储空间默认已选中，**不要再点它**（展开下拉会遮挡上传区）。
- **方式 B（CLI）**：本应用没有向导、没有 `install_dep_apps`，可用 `trim-cli` 直接安装：
  ```bash
  trim-cli app install-fpk --remote-path /vol1/1000/workspace/fusionplay-fnos.fpk \
      --volume-id 1 --yes
  ```
- ⚠️ 同 `appname` + 同 `version` 重装会报 **errno 10236**（应用已存在）：先卸载，或使用更高版本号。
- **安装后必须在 Web UI 授权音频权限**（否则看不到声卡、播放无声）：
  设置 → 应用管理 → FusionPlay → 权限设置 → 勾选 **audio / 音频设备** → 保存。
  详见下节「ALSA 权限说明」。

## 3. 访问入口

| 入口 | 地址 | 说明 |
|---|---|---|
| 桌面图标 | `/app/fusionplay-fnos/` | 走 fnOS 统一网关（Unix socket），HTTPS / FnConnect 远程可用 |
| 本机调试 | `http://127.0.0.1:18080/` | **只绑 loopback**，仅供 SSH 隧道/本机诊断，不是对外入口 |
| 健康检查 | `/healthz` | 返回 `{"status":"ok","version":"x.y.z","uptime":N}`（升级后用它核对版本） |
| 状态 API | `/api/status` | 协议状态、当前音源、设备、音量、错误列表 |

网关接法：后端监听 `$TRIM_APPDEST/fusionplay-fnos.sock`（权限 0666），
`app/ui/config` 的 `gatewayPrefix=/app/fusionplay-fnos`、`gatewaySocket=fusionplay-fnos.sock`；
裸前缀 `/app/fusionplay-fnos` 会 307 跳到带斜杠路径。

### 3.1 mDNS / AirPlay 发现注意（重要）

- AirPlay / DLNA 的「被发现」依赖 mDNS 广播（`_airplay._tcp` / `_raop._tcp` / `_device-info._tcp`）。
- 上游 `shairplay` 用环境变量 **`CI`** 作为测试开关：**存在 `CI` 时跳过 mDNS 注册**，
  表现为手机完全搜不到接收器，但 Web 管理页一切正常——极难排查。
  本包的 `cmd/main` 在启动服务前会显式 `unset CI`，请不要在 fnOS 应用环境、启动脚本或
  编排工具里再设置 `CI=1`。
- 排查“搜不到设备”：
  ```bash
  sudo avahi-browse -rt _airplay._tcp     # 应能看到 FusionPlay
  sudo avahi-browse -rt _raop._tcp
  ```
  若命令存在但看不到，先确认应用在运行、audio 权限已授权，再检查 avahi/mDNS 服务是否运行。
- 系统需要 mDNS 服务（飞牛默认自带 avahi；被关闭时 AirPlay/DLNA 发现会失效，HTTP 面板不受影响）。

## 4. ALSA 权限说明（为什么不是 root）

- `config/privilege` 声明 `"defaults": {"run-as": "package"}`：专用应用用户运行，**不是 root**，
  不监听特权端口、不修改系统配置、只能访问自己的目录。
- `"join-groups": ["audio"]`：申请加入 `audio` 组，以获得 `/dev/snd/*` 的读写权限。
- fnOS 的安全模型是「**声明只是申请，授权由用户完成**」：安装后仍需在
  设置 → 应用管理 → FusionPlay → 权限设置里勾选音频/audio 组并保存。
- 未授权时的表现：应用仍能启动（后端自动降级为 null 输出），但设备列表为空、播放无声；
  诊断页与日志会出现 `ALSA 后端启动失败…` / `no soundcards found` 之类提示。**这是权限问题，不是包坏了。**
- 排查顺序：① Web UI 权限是否已勾选；② `ls -l /dev/snd` 是否存在；③
  `sudo -u fusionplay-fnos test -r /dev/snd/controlC0 && echo ok`；④ 重启应用后再看设备页。

## 5. 配置与数据目录

fnOS 会用 `TRIM_*` 环境变量把不同用途的目录传给应用（升级时的保留策略不同）：

| 变量 | 典型路径 | 本应用放什么 | 升级 | 卸载 |
|---|---|---|---|---|
| `TRIM_APPDEST` | `/vol1/@appcenter/fusionplay-fnos`（`/var/apps/fusionplay-fnos/target` 软链） | 二进制 `bin/`、前端 `www/`、`ui/`、`licenses/`、`BUILD-INFO` | 整体替换 | 删除 |
| `TRIM_PKGVAR` | `/vol1/@appdata/fusionplay-fnos` | `app.log`、`fusionplay.log`、`app.pid`、运行期状态 | **保留** | 默认保留 |
| `TRIM_PKGHOME` | `/vol1/@apphome/fusionplay-fnos` | **`config.json`**、MiPlay 身份、DLNA 缓存 | **保留** | 默认保留 |
| `TRIM_PKGETC` | `/vol1/@appconf/fusionplay-fnos` | fnOS 管理的配置 | 保留 | 视策略 |
| `TRIM_PKGTMP` | `/vol1/@apptemp/fusionplay-fnos` | 临时文件 | — | 删除 |

- **`config.json`** 放在 `$TRIM_PKGHOME/config.json`：首次启动自动生成默认值，之后在 Web UI 保存时**原子写入**；
  升级不会覆盖。字段包含 receiver、协议开关、音频设备（含每设备 `curve`/`gain_db`/`delay_ms`/`sample_rate`）、
  全局衰减/静音、日志级别与 `config_version`（v1→v2 自动迁移）。
- 配置迁移：`GET /api/export` 导出、`POST /api/import` 导入（导入会校验非法值）。
- 想手动查看（需要 root 或应用用户权限）：
  ```bash
  sudo cat /vol1/@apphome/fusionplay-fnos/config.json
  ```

## 6. 日志

| 文件 | 内容 | 轮转 |
|---|---|---|
| `$TRIM_PKGVAR/fusionplay.log` | 应用主日志（tracing）：启动/协议/音频/错误 | **5 MiB 上限，超限原地保留尾部 1 MiB**（tail-trim，inode 不变，无需重启） |
| `$TRIM_PKGVAR/app.log` | `cmd/main` 启停日志 + 二进制 stdout/stderr | 同上（原地轮转） |

- Web UI「日志」页读取 `/api/logs?lines=N`，级别可热调（`POST /api/logs/level`），改完无需重启。
- SSH 查看：
  ```bash
  sudo tail -n 200 /vol1/@appdata/fusionplay-fnos/fusionplay.log
  sudo tail -f /vol1/@appdata/fusionplay-fnos/app.log
  ```
- 应用日志目录权限较严（`@appdata` 常为 750），读不到时优先用 UI 的日志页。

## 7. 升级

1. 每次重新打包 **必须 bump `manifest.version`**（且与 `fnos/backend/Cargo.toml`、
   二进制 `--version` 保持一致，构建脚本会强制校验）。同 `appname`+`version` 重装 = errno 10236。
2. fnOS 升级流程：替换 `TRIM_APPDEST` 代码 → `upgrade_init`（**停服务**、删 PID/socket、按端口兜底清理）
   → `upgrade_callback`（**重新拉起**并等 `/healthz` 通过）。本包两个脚本已按此实现。
3. 用户数据（`config.json`、日志）在 `$TRIM_PKGHOME` / `$TRIM_PKGVAR`，升级保留。
4. 升级后核对：
   ```bash
   curl -s http://127.0.0.1:18080/healthz        # version 应为新版本
   sudo ss -lntp | grep 18080                    # 监听进程应为新启动时间
   ```
5. 若升级后服务没起来：`upgrade_callback` 会把原因写进 `$TRIM_TEMP_LOGFILE`（应用中心会提示），
   同时 `$TRIM_PKGVAR/app.log` 有详细日志；可手动在应用中心点「启动」。

## 8. 卸载

- 应用中心 → FusionPlay → 卸载。**默认保留** `@apphome`（配置）与 `@appdata`（日志）数据，
  重装后可恢复；不保留数据时请在卸载向导/应用中心选择删除。
- 彻底清理（不可恢复，仅 root）：
  ```bash
  # 确认应用已卸载、端口已释放后
  sudo rm -rf /vol1/@apphome/fusionplay-fnos /vol1/@appdata/fusionplay-fnos \
              /vol1/@appcenter/fusionplay-fnos /vol1/@appconf/fusionplay-fnos
  ```
- 卸载后自查：`ss -lntp | grep 18080` 无输出、`pgrep -af fusionplay-fnos` 无输出。

## 9. 已知限制

- **仅 x86_64**：当前包 `platform = x86`，ARM 飞牛设备不能安装。
- **多房间同步**不做采样级对齐；多台设备之间使用统一主时钟 + 漂移补偿，但不等同于 AirPlay 多房间。
- **DLNA 仅音频**且不支持 HLS/视频/转码；不支持的格式会在 UI/日志明确报错。网易云 `video/flv` 形式的音频投屏已按音频处理。
- **MiPlay 为兼容实现**，不宣称官方认证；部分厂商发送端可能因准入限制无法连接（日志会写明）。
- **只输出到 ALSA 硬件设备**（`hw:CARD=…`）；没有 PulseAudio/蓝牙/网络音频。
- 拔插声卡后需要重新扫描（Web UI「刷新设备」）或重启应用。
- 多实例/端口冲突：服务端口固定 18080，同机只能跑一个实例。
- 真实 `/dev/snd` 权限依赖用户在应用中心授权 audio 组；CI 与本地模拟无法覆盖真实声卡链路。
- **发现依赖 mDNS/avahi**：系统 mDNS 服务被关闭、或运行环境被注入 `CI=1` 时，发送端搜不到接收器（本包已 `unset CI`，但系统服务需自行保证）。
- 真机验证项（iPhone/iPad、DLNA 控制端、HyperOS、多声卡、N150 性能）见 [INSTALL.md](INSTALL.md) 的验收清单。

## 10. 故障排查

| 症状 | 可能原因 | 处置 |
|---|---|---|
| 装完打不开 / 白屏 | audio 未授权导致启动失败；或网关入口不对 | 看 `app.log`；权限设置里授权 audio；从桌面图标进 `/app/fusionplay-fnos/` |
| 设备列表为空、播放无声 | audio 组未授权 / `/dev/snd` 不可访问 | 应用中心授权音频权限 → 重启应用 |
| `/healthz` 版本和预期不符 | 旧进程没停（升级遗留） | 重启应用；确认 `upgrade_init` 已停服务（本包已修） |
| 重装报 errno 10236 | 同 appname + version | 卸载旧版，或 bump 版本号 |
| 端口 18080 被占用 | 同机已有实例或其它应用 | 停掉占用进程；`sudo ss -lntp \| grep 18080` 确认 |
| 日志文件长不大/没轮转 | 检查 `fusionplay.log` 是否 5 MiB 上限、inode 是否变化 | 本包为原地 tail-trim，不换 inode |
| 远程访问白屏但局域网正常 | 用了 `http://NAS:18080` 而不是网关 | 用桌面图标 / `/app/fusionplay-fnos/`（端口只绑 loopback） |
| 手机/电脑搜不到 FusionPlay | 运行环境带 `CI=1`（shairplay 跳过 mDNS）或系统 mDNS/avahi 未运行 | 去掉 CI 并重启应用；`sudo avahi-browse -rt _airplay._tcp` 验证 |

更多构建、测试与 CI 细节见 [fnos-build.md](fnos-build.md)。
