# FusionPlay fnOS 版：架构与接口契约

> 基线：`fnos/fpk` 分支（从含 MiPlay 兼容修复的 `8af8545` 拉出）。
> 原则：复用 `src/AirPlayReceiver.Core` / `src/FusionPlay.MiPlaySdk`；只为"外部 PCM 输出"加最小钩子；
> 所有 fnOS 代码放 `fnos/`；不改 Android 行为、不改 Android CI。

## 音箱实例与播放页

父进程提供统一网关和实例目录。主接收器可向多只音箱输出同一条音流。每个独立音箱使用同一可执行文件启动子进程，拥有自己的协议身份、曲目、控制通道、解码器、仲裁器以及 ALSA 输出。妙播的 PCM 回调是进程级的，分进程保证它只写入当前音箱的音频引擎。下图描述一个接收器内部的数据流。

- `GET /api/instances` 返回可用播放器标识、显示名、设备和默认目标。网页将选定目标写入地址栏。
- `GET /api/instances/<id>/status`、`POST /api/instances/<id>/player` 和 `GET /api/instances/<id>/artwork/<filename>` 共用同一实例标识。父进程通过私有 Unix socket 转发独立音箱请求。
- `audio.instances` 保存稳定 ID、投送名称和声卡分配。配置校验拒绝同一设备重复分配；工作进程重启后从各自的数据目录恢复身份。额外音箱使用独立妙播控制与媒体发现端口，AirPlay 使用本地时钟回退，跨音箱播放进度可以独立变化。

## 1. 进程与数据流

```
                     ┌──────────────── fnos/backend: fusionplay-fnos（单二进制）────────────────┐
AirPlay 2 ──core─────┤  sink.rs ──┐                                                              │
DLNA(DMR) ──core─────┤  (PcmOutput)│   session.rs（三协议仲裁：同一时刻仅一个主音源）           │
MiPlay ───MiPlaySdk──┤  sink.rs ──┤            │                                               │
                     │  dlna_player.rs（HTTP 拉流 + symphonia 解码 → PCM）                     │
                     │                        ▼                                                │
                     │   audio/（统一引擎，48k stereo f32 主混音）                             │
                     │     volume：每设备曲线 → 每设备 dB → 全局衰减 → mute（发送端主音量唯一来源）│
                     │     fan-out：每设备独立 ring + 重采样 + 漂移补偿 + 延迟补偿              │
                     │        ├─ ALSA hw:CARD=x,DEV=0   ├─ hdmi  ├─ 板载 ...                   │
                     │  http.rs：/healthz /api/* + 静态 UI（$TRIM_APPDEST/www）                │
                     │  config.rs / state.rs / logging.rs / 诊断                               │
                     └─────────────────────────────────────────────────────────────────────────┘
```

- 协议栈以**进程内**方式复用（不复制代码）：AirPlay+DLNA 走 `fusionplay_core::run_receiver_with_arbiter`；
  MiPlay 走 `fusionplay_miplay_sdk::MiPlayReceiver::start`。
- 两者都新增可选 `PcmOutput` 注入；不设置时行为与 Android/桌面现状完全一致。

## 2. 对上游核心的最小改动（只加钩子）

### 2.1 `src/AirPlayReceiver.Core/src/audio.rs`

```rust
pub enum PcmSource { AirPlay, Dlna, MiPlay }          // 便于引擎按来源统计/仲裁
pub trait PcmOutput: Send + Sync {
    /// 主音源 PCM（f32 交错，sample_rate/channels 由 start_audio_external 固定为 48k/2）
    /// sender_gain: 发送端逻辑音量的线性幅度 0..=1（AirPlay 由 dB 换算，DLNA/MiPlay 由 % 换算）
    fn write(&self, source: PcmSource, epoch: u64, sender_gain: f32, sample_rate: u32,
             channels: u16, samples: &[f32]);
    fn flush(&self, source: PcmSource);
}

pub fn start_audio_external(
    events: Arc<EventSink>, arbiter: Arc<PlaybackArbiter>, artwork_dir: PathBuf,
    output: Arc<dyn PcmOutput>,
) -> Result<AudioRuntime>
```

- `AudioRuntime._stream` 在非 Android 改为 `OutputHandle { Cpal(cpal::Stream), External(...) }`；
  外部模式下起一个 pump 线程从既有 `AudioBuffer` 取数并按需 sleep，**复用**现有 handler
  （仲裁/音量/元数据/进度全部不变），只是把"cpal 回调"替换为 `PcmOutput::write`。
- `Settings` 增加 `pub audio_output: Option<Arc<dyn PcmOutput>>`；`run_receiver_with_arbiter`
  在 `Some` 时调用 `start_audio_external`。默认 `None` → 原有 cpal 路径。
- Android JNI 构造 `Settings` 处补 `audio_output: None`。

### 2.2 `src/FusionPlay.MiPlaySdk`

```rust
pub trait MiPlayPcmOutput: Send + Sync {
    fn write(&self, epoch: u64, sender_gain: f32, sample_rate: u32, channels: u16, samples: &[f32]);
    fn flush(&self);
    fn info(&self) -> Option<MiPlayVolumeInfo>;   // 可选：发送端音量百分比
}
impl ReceiverConfig { pub fn with_pcm_output(self, out: Arc<dyn MiPlayPcmOutput>) -> Self }
```

- 非 Android 输出路径：若配置了 `pcm_output`，不打开 cpal；把原本送 cpal 的 f32 PCM
  交给注入 sink（复用现有音量/下混/时钟逻辑）。默认 `None` → 原行为。

### 2.3 `src/AirPlayReceiver.Core/src/lib.rs`

```rust
pub use audio::{PcmOutput, PcmSource};
pub use takeover::{MediaSource, PlaybackArbiter};
pub use events::{CoreEvent, EventSink};
```

## 3. `fnos/backend`（新 crate `fusionplay-fnos`）

| 文件 | 职责 |
|---|---|
| `src/main.rs` | 解析 `--config/--state-dir/--listen-tcp/--listen-unix/--version`；初始化日志；起 AudioEngine、协议宿主、HTTP；SIGTERM 收尾 |
| `src/config.rs` | `Config { config_version, receiver, protocols{airplay,dlna,miplay{enabled,device_type}}, audio{global_attenuation_percent, muted, devices[{id,name,enabled,gain_db,delay_ms,sample_rate,curve}], legacy curve}, logging }`；原子落盘、导入导出、v1→v2 幂等迁移、默认值 |
| `src/state.rs` | 共享状态快照（`Arc<RwLock<AppState>>`）：协议状态、当前音源/媒体、发送端音量、设备状态、队列/丢帧、错误 |
| `src/session.rs` | 三协议仲裁：`Lease { source, epoch }`；`claim()` 接管并回调旧源 suspend；只放行当前 lease 的 PCM |
| `src/volume.rs` | `Curve::{Linear,Soft,Balanced,Custom}`、单调自定义点、逐设备曲线映射 `sender_logical -> amplitude`、`device_db_to_gain`、`delay_ms_to_frames`；测试 |
| `src/audio/mod.rs` | `DeviceInfo/DeviceSettings/EngineStatus`、`AudioEngine`（实现 `PcmOutput`）、`MockEngine`（CI 无设备） |
| `src/audio/alsa.rs` | ALSA 枚举（`/proc/asound` + `alsa` crate 校验、稳定 ID）、每设备 worker（ring + rubato 重采样 + 漂移比例 + 延迟补偿 + dB/mute）、热插拔重扫、单设备=多设备同一路径 |
| `src/audio/resample.rs` | 重采样/漂移比例封装（rubato），可单测 |
| `src/dlna_player.rs` | 取 `CoreEvent::DlnaMedia` 的 URL → HTTP(S) 拉流 → symphonia 解码（mp3/flac/wav/aac/isomp4 音频轨）→ PCM；MP3/FLAC/WAV/AAC 支持，HLS/视频首版报错 |
| `src/sink.rs` | 实现 `PcmOutput` + `MiPlayPcmOutput`：按 session lease 转发到 AudioEngine |
| `src/http.rs` | HTTP/1.1 服务（Unix socket + TCP），静态文件、JSON API、SSE 可选；`/healthz` 报版本 |
| `src/logging.rs` | 文件日志 5MiB 上限/1MiB 保尾、原地重写；级别可配 |
| `tests/` | 配置往返、音量曲线、增益链、仲裁接管、引擎 fan-out（mock/null）、FPK 结构 |

### 3.1 HTTP API 契约（前端按此实现）

| 方法/路径 | 返回 |
|---|---|
| `GET /healthz` | `{status:"ok",version,uptime}` |
| `GET /api/status` | `{version, uptime, protocols:{airplay:{enabled,state,name,port,clients},dlna:{...},miplay:{...}}, current_source, media:{title,artist,album,cover_url}, sender_volume:{airplay_db,dlna_percent,miplay_percent}, devices:[{id,name,online,sample_rate,channels,enabled,gain_db,delay_ms,buffer_ms,drift_ppm,underruns}], audio:{global_attenuation,curve,mute}, logs_level, errors:[...]}` |
| `GET/PUT /api/config` | 完整配置 JSON |
| `GET /api/devices` | ALSA 实测设备列表（id/name/card/device/rates/channels/default） |
| `POST /api/devices/refresh` | 重新枚举 |
| `POST /api/devices/test` | `{id}` 播放 1s 测试音 |
| `GET /api/calibration` | `{config_version, global_attenuation_percent, muted, devices[{id,name,enabled,gain_db,delay_ms,sample_rate,curve}]}` |
| `PUT /api/calibration` | 全局衰减/静音 + 每设备 `enabled/gain_db/delay_ms/sample_rate/curve`；legacy 顶层 `curve` 会应用到全部设备（兼容 v1，不再存为全局） |
| `POST /api/calibration/preview` | `{device_id?|curve?}` → `{device_id,curve,kind,points,samples[101]}`；可明确对应设备或其曲线数据 |
| `GET /api/logs?lines=N` | 最近 N 行 |
| `POST /api/logs/level` | `{level}` |
| `GET /api/export` / `POST /api/import` | 配置导入导出 |
| `GET /api/diagnostics` | 网络接口、ALSA 状态、进程/版本/构建信息 |

### 3.2 音量模型（v2：逐设备曲线）

发送端（AirPlay dB / DLNA % / MiPlay %）→ 统一逻辑音量 `sender_gain (0..=1)`，这是唯一的
"主音量"来源，NAS 不反向改写。音频引擎对每个启用设备独立计算：

```
device_gain = device.curve.apply(sender_gain)      // 该设备自己的响应曲线
            * device_db_to_gain(device.gain_db)     // 该设备响度平衡（-30..0 dB）
            * global_attenuation_percent / 100      // 全局衰减
            * (muted ? 0 : 1)                       // 全局静音
```

- 曲线按块计算一次，ring 内做块级线性增益斜坡，避免爆音；设备线程另有增益斜坡与淡入。
- 发送端静音（`sender_gain == 0`）经任何合法曲线（首尾固定 0/0）都映射为 0，优先于本地校准。
- 旧配置的 `audio.curve`（v1 全局曲线）在加载时复制给所有尚无独立曲线的设备，
  然后清空并写回 `config_version=2`；迁移幂等，且不会覆盖已存在独立曲线的设备。

### 3.3 MiPlay 设备身份

`protocols.miplay.device_type`（默认 `television`）决定妙播发送端看到的设备类别与图标：
`vehicle` 车机 / `television` 电视 / `tablet` 平板 / `speaker` 音响（`display_speaker` 作为 Android
旧别名映射为 `speaker`）。该字段进入协议宿主重启指纹，修改后保存会重新广播；发送端侧图标
（车机/电视/平板/音响）随之变化。Android 与 fnOS 使用同一组取值。

### 3.4 协议运行状态词表（三协议 + 两个页面共用）

后端只输出以下规范状态，`/api/status.protocols.*.state` 与 Web UI 的概览/接收页共用同一词表，
不允许再出现 `ready/listening/running` 三套同义状态：

| state | UI 文案 | 含义 |
|---|---|---|
| `disabled` | 已关闭 | 协议在设置中关闭 |
| `starting` | 启动中 | 协议宿主正在启动 |
| `waiting` | 等待投放 | 接收服务就绪，等待发送端投放（空闲态） |
| `loading` | 加载中 | DLNA 媒体正在拉流/探测 |
| `streaming` | 播放中 | 正在播放该协议音频 |
| `paused` | 已暂停 | 发送端暂停 |
| `error` | 异常 | 协议启动/播放失败（详情见诊断） |

每个协议同时提供一句规范 `detail`（如「等待 AirPlay 2 投放」「等待 DLNA 投放」「等待小米妙播投放」），
概览协议卡与接收页展开详情读同一字段。

## 4. FPK 包（形态 B：编译二进制）

```
fnos/package/                       # 打包源（fnpack/）
├── manifest                        # appname=fusionplay-fnos, version=x.y.z, source=thirdparty, service_port=18080
├── ICON.PNG / ICON_256.PNG
├── app/ui/config                   # iframe -> /app/fusionplay-fnos/
├── cmd/{install_init,install_callback,upgrade_init,upgrade_callback,
│        uninstall_init,uninstall_callback,config_init,config_callback,main}
├── config/privilege                # run-as package + join-groups ["audio"]（/dev/snd 访问，保持非 root）
└── config/resource
```

- `cmd/main` 必须：`status` 严格 0/3；start 前清理陈旧进程；轮询 `/healthz` 判成功；
  `HOME` 无条件覆盖为 `$TRIM_PKGHOME`；`DATA_LIBRARY_PATH` 兜底 `TRIM_PKGVAR`；日志原地轮转。
- 二进制与 Web UI 由构建脚本放入 `app/bin/fusionplay-fnos`、`app/www/`；`server/VERSION` 写版本。
- 数据：`$TRIM_PKGVAR`（日志/pid/socket）、`$TRIM_PKGHOME`（config.json、MiPlay identity、DLNA 缓存）。

## 5. CI（`.github/workflows/build-fnos.yml`）

- `workflow_dispatch`（可指定 ref）+ `push`（tags `fnos-*`）。
- ubuntu-latest：`libasound2-dev` + Rust stable + `cargo test`（core/MiPlaySdk/backend 单测）+ `cargo build --release`。
- 前端为纯静态资源（无需 npm），直接复制。
- 用仓库内 `fnos/tools/fnpack-linux-amd64` 打包（校验 `Packing successfully`），
  `tar tzf` 两层校验后上传 Artifact；tag 时附到 Release。
- 不改动、不删除 `.github/workflows/android.yml`。

## 6. 验证边界（必须如实上报）

- CI 只验证：编译、单测、FPK 结构、HTTP API/mock 引擎。
- **待真机验证**：iPhone/iPad AirPlay 2 投放与暂停/切歌/音量；DLNA 控制端；
  HyperOS MiPlay 投放；真实多声卡输出与漂移/延迟校准；N150 性能。
- 不宣称：AirPlay 2 多房间采样级同步、小米官方认证、DLNA HLS/视频支持。
