# FusionPlay Android AirPlay 改造与当前状态

> 用途：给外部审查者（例如让 GPT 直接读仓库）快速了解：相对原版 FusionPlay，
> Android 端针对 AirPlay 到底改了什么、为什么改、目前真机是什么现象、哪些问题
> 仍未解决。
>
> 基线：本轮 AirPlay 深入改造前的 **Android 1.3.5（commit `b42984c`）**。
> 如果你所说的“原版”是更早的 1.2.3 开源版本，可以用文末的 git 命令查看更早差异。
> 当前代码：`main`，最新 release `android-v1.4.3`（2026-11-16）。
> **注意：1.4.3 的 direct-feed 仍有节奏缺陷；paced direct + 480/240ms 缓冲的修复已提交到 main
> （main 分支），但尚未构建 release、尚未经过 iPhone 真机验证，不能宣称 AirPlay 2
> 已彻底解决。**

## 1. 一句话结论

原版 AirPlay 实现可以建立会话、收元数据，但在 Android/iOS 真机组合下存在：
**身份被当成 TV、AAC 解码/缓冲格式假设错误、定时播放锚点不可用、音量被当成
远程 DACP 命令**等问题。经过 1.4.0–1.4.3 的修改，身份、AAC、FLUSH、接收端音量
已经修到“能收到并解码 PCM、能开始填充输出缓冲”的程度；但 1.4.3 的 direct-feed
兜底目前是**无节奏地把所有已缓冲 PCM 塞进 240ms 本地缓冲**，会瞬间溢出/欠载，
所以**声音仍不稳定/不可用**。同时 iPhone 端音量仍显示 100%，FusionPlay 调音量
无法反向影响 iPhone UI，这是 AirPlay 2 音量语义问题，尚未解决。

## 2. 原版 AirPlay 行为（1.3.5 基线）

以下描述的是本轮改造前的大致形态，具体以 `b42984c` 代码为准：

- **发现/自述**：mDNS 已经是音频接收器，但 `GET /info`、AP2 `updateInfo`、
  `/server-info` 仍可能返回 `AppleTV6,2` 和带 video/HLS 位的 features，导致
  iOS 把它识别成电视/投屏目标。
- **AirPlay 2 buffered audio**：已有 103 流、AAC/ALAC、PTP/RTCP 基础代码，
  但缺少针对真实 iPhone AAC 数据、FLUSH 序列号、无 PTP 时钟场景的完整处理。
- **播放调度**：使用 `SETRATEANCHORTIME` 的 anchor 做定时 playout；没有 PTP
  共享时钟时走本地 fallback。
- **音量**：按“远程控制命令”处理，向 sender 的 DACP/MediaRemote 发 `SetVolume`；
  发送端不公布该命令时会返回“当前设备不支持此远程控制命令”。
- **元数据/歌词**：DMAP 只解析 minm/asar/asal/asgn/astm/astn/asdk；没有歌词字段。
  Android UI 对 AirPlay 直接按“不支持歌词”处理。
- **UI**：AirPlay 走居中布局；封面/标题/作者/歌词的顺序和动画没有针对 AirPlay
  频繁 now-playing 更新做特殊处理。
- **音频输出**：Android 使用 Oboe + 本地缓冲；没有针对 ColorOS/OPPO 的
  underrun/无声诊断。

## 3. 相对基线改了哪些 AirPlay 相关东西

### 3.1 身份/发现：不再伪装成 TV

文件：

- `vendor/shairplay/src/net/features.rs`
- `vendor/shairplay/src/net/mdns.rs`
- `vendor/shairplay/src/raop/handlers_ap2.rs`
- `vendor/shairplay/src/raop/handlers_hls.rs`
- `vendor/shairplay/src/raop/config.rs`
- `vendor/shairplay/src/raop/connection.rs`
- `vendor/shairplay/src/raop/server.rs`
- `src/AirPlayReceiver.Core/src/android_jni.rs`
- `src/AirPlayReceiver.Core/src/host.rs`

改动：

1. 新增 `audio_only_features()`，直接构建纯音频 AP2 feature set
   （`0x0001C340405D4A00`），不再从 video 集清位；明确清掉
   bit0/1/2/4/5/6/7/10/12/13/25/28/33/42/44/49 等视频/投屏位。
2. `GET /info`、加密 event channel 的 `updateInfo`、`/server-info` 统一使用
   audio-only features 和 `AirPort10,115` 模型；不再返回 `AppleTV6,2`。
3. 不广告 `displays`，不广告 HLS/video 能力；mDNS 的 `features`/`ft`/`am`/`model`
   与 RTSP 自述保持一致。
4. 测试覆盖 feature 位、mdns、info profile、Android audio-only 设置。

真机结果：1.4.1/1.4.2/1.4.3 日志中 AirPlay 以 audio 流建立，
`media_kind=audio`，不再出现被当电视的情况（至少日志侧如此）。

### 3.2 AirPlay 2 音频接收/解码/缓冲

文件：

- `vendor/shairplay/src/raop/buffered_audio.rs`（最大改动）
- `vendor/shairplay/src/codec/aac.rs`
- `vendor/shairplay/src/codec/alac.rs`
- `vendor/shairplay/src/codec/resample.rs`
- `vendor/shairplay/src/raop/handlers_ap2.rs`
- `vendor/shairplay/src/raop/realtime_audio.rs`
- `vendor/shairplay/src/raop/types.rs`
- `src/AirPlayReceiver.Core/src/audio.rs`

改动：

1. **buffered ALAC/AAC**：按 103 SETUP 的 SSRC 选择解码器；ALAC 标准
   `ALACSpecificConfig`；未知 SSRC、`SSRC_NONE`（ACK/flush）跳过，不偷格式。
2. **AAC 帧格式**：确认 iPhone AirPlay 2 明文是 **raw AAC-LC data block**。
   symphonia 直接解 raw；若 sender 带 ADTS，先检测并剥离；raw 失败才合成
   ADTS 回退。之前“无条件补 ADTS”会把声音解成数字静音。
3. **FLUSH 语义**：`FLUSHBUFFERED` 的 `flushFromSeq/flushUntilSeq` 是 RTP
   **序列号**，旧代码拿它和 RTP 时间戳低 32 位比较。现在维护 seq→timestamp
   历史映射，只删真正命中的帧；无法映射时忽略。
4. **播放调度**：
   - 定时 playout：`SETRATEANCHORTIME` + PTP/RTCP anchor；无 PTP 时本地 fallback。
   - 1.4.3 增加 **direct-feed 兜底（默认开启）**：绕过 anchor，直接把解码后的
     帧按 RTP 时间顺序送入 `AudioSession`。目的只是恢复声音，牺牲了播放节奏。
5. **本地输出缓冲**：
   - Android `AudioBuffer` 240ms 容量 / 120ms 预填充（1.4.2 从 480/240 回退）。
   - Oboe `LowLatency` + Shared + f32 48k。
   - 增加 underrun/预填充/已收 PCM/push/clear/锁失败诊断。
6. **PTP/RTCP/长时播放**（1.3.6 及之前已存在/加强）：RTCP time announce、
   64-bit RTP 时间戳扩展、ALAC fuzz、PTP best-effort；无共享 PTP 时不保证
   多房间 sample-accurate 同步。

### 3.3 音量/反向控制

文件：

- `vendor/shairplay/src/raop/handlers_ap1.rs`
- `src/AirPlayReceiver.Core/src/audio.rs`
- `flutter/android/app/src/main/kotlin/com/fusionplay/android/FusionPlayRuntime.kt`
- `flutter/android/app/src/main/kotlin/com/airplayreceiver/desktop/backend/FusionPlayDiagnosticLogger.kt`

改动：

1. AirPlay 音量不再要求 sender 支持 `RemoteCommand::SetVolume`：
   core 先把 0–100% 映射到本地 gain（0% = -144dB），返回成功；sender 若支持
   DACP/MediaRemote SetVolume 才 best-effort 镜像，失败不影响本地。
2. `GET_PARAMETER volume` 返回接收端当前 dB（原固定 `0.000000`，会让发送端
   一直认为接收端是 0dB/100%）。
3. sender 的 `SET_PARAMETER volume` 与本地 UI 音量共享 `volume_db_bits` /
   `gain_bits`；`CoreEvent::Volume` 同时带 db/percent。
4. Android AirPlay 路由不再同时改系统媒体音量，避免“系统音量 × core gain”
   双重衰减；DLNA/妙播保持原行为。
5. 顶部错误 toast 7 秒自动消失；音量滑杆松手只发一次命令，避免反复触发错误。

未解决：

- iPhone 控制中心仍显示 100%。AirPlay 2 没有标准的“接收端主动把音量推送到
  发送端 UI”机制；`GET_PARAMETER` 只在 sender 查询时返回当前值，不会主动刷新。
- 反过来在 FusionPlay 调音量，iPhone UI 不会同步；用户预期“反向控制手机的
  音量”在当前协议路径下没有实现。
- 1.4.3 最新日志中 `volume` 事件后紧跟 `command_result`，说明这些音量事件
  主要是 FusionPlay 本地命令；没有看到 sender 主动发 `SET_PARAMETER volume`
  的证据（日志字段有限）。

### 3.4 元数据/歌词/播放器 UI

文件：

- `flutter/lib/models/lyrics.dart`
- `flutter/lib/ui/player_view.dart`
- `flutter/lib/models/app_state.dart`
- `flutter/lib/main.dart`
- `flutter/lib/ui/update_dialogs.dart`（非 AirPlay 协议，但相关错误提示）
- `src/AirPlayReceiver.Core/src/audio.rs`（unknown DMAP 诊断）
- `vendor/shairplay/src/proto/dmap.rs`

改动：

1. 删除“AirPlay 2 不提供歌词”的硬编码；歌词可用性按**实际 metadata**判断，
   没有则显示“音频流未提供歌词”。
2. AirPlay 布局按实际歌词决定：有歌词时封面左/歌词右；无歌词时居中。
   顺序统一为封面 → 曲名 → 作者 → 歌词（标题在作者上方）。
3. 封面动画以**封面 source 身份**为 key，不再因 now-playing/歌词频繁更新而
   每行翻转。
4. 未知 DMAP 标签只记录“4 字符标签+长度”（不记录内容），用于确认 iPhone
   是否发送厂商扩展歌词字段（例如 `asly`）；目前未发现标准歌词字段。
5. AirPlay 的现在播放元数据经常更新，UI 侧对重复元数据/封面做去重。

### 3.5 诊断与发布相关（非协议但影响排障）

- 持久化日志现在包含：输出启动配置、`AirPlay 音频数据已开始输出`、
  `Android 音频输出已开始播放`、underrun/预填充、10 秒 PCM/push/clear 心跳、
  buffered 收包/解码/静音计数、delivery 模式/送出帧/flush 计数。
- Release APK 改为 arm64 单架构（31.8MB → 12.4MB），字体子集化；这与 AirPlay
  无关，但会影响用户测试包。
- AirPlay 相关单元/集成测试：vendor features/mdns/buffered/AAC/ALAC/RTCP/PTP；
  core audio/volume/session；Flutter lyrics/layout/volume/error toast。

## 4. 版本时间线（AirPlay 相关）

| 版本 | commit/tag | 主要 AirPlay 变化 | 真机结果 |
|---|---|---|---|
| 1.3.5 基线 | `b42984c` | 原版 AirPlay 2 基础 | 元数据可用；声音/身份/音量有问题 |
| 1.3.6 | `bd4b8ba` / `1147a6b` | buffered ALAC、OS27/RTCP、PTP、长时播放加固 | 解码能力增强，仍有真机问题 |
| 1.4.0 | `e3cd386` | 玻璃/UI 等，非 AirPlay 主体 | — |
| 1.4.1 | `988d567` | audio-only 身份、歌词/layout、Oboe 480/None | 身份修正；**完全无声** |
| 1.4.2 | `c105434` | AAC ADTS/raw、接收端音量、Oboe 回退、诊断、12.4MB | PCM 解码成功但本地缓冲只有首帧/0ms |
| 1.4.3 | `2ac80cb` | direct-feed 绕过 anchor、FLUSH seq 映射、诊断 | PCM 流动、输出开始，但**无节奏 direct-feed 导致溢出/欠载** |

当前 `main` 已包含 **paced direct-feed** 修复：把 direct-feed
改成按本地 RTP 锚点 1x 投递，并把 Android 缓冲调到 480/240ms。该修复只经过
单元测试，尚未构建 release、尚未真机验证。

## 5. 当前真机现象（1.4.3，2026-11-16 多 App 对比）

设备：OPPO PKJ110，Android 17，arm64，FusionPlay 1.4.3/10403。

### 5.1 网易云音乐（iPhone 发送）

用户描述：开始只播放约 **2 秒**，这 2 秒**破音**，之后没有声音；App 仍显示在播放/可操作。

日志（`persistent/fusionplay.jsonl`）对应段：

```
11:30:46 AirPlay 音频数据已开始输出：codec=aac 44100→48000，首块 2224 采样
11:30:47 Android 音频输出已开始播放：当前缓冲 240/240 ms
11:30:49 已收 PCM 3,243,358 采样，push 1455 次，underrun 2，缓冲 0 ms
11:30:55 AirPlay FLUSH：seq 0..21180，匹配 1455 个已收包，删除 0 帧
11:30:55 AirPlay 2 音乐已暂停
11:30:56 新的音频数据开始输出（stream 重建）
11:30:59 已收 PCM 6,738,612 采样，push 3023 次，underrun 4，缓冲 0
11:31:04 delivery：direct，已送 1589 帧 / 3,542,060 采样，待播 0，rate=1
11:31:07 收包 1811、解码 1810 帧 / 4,034,694 采样，失败 0、静音 0
11:31:09 已收 PCM 11,618,146 采样，push 5212 次，underrun 6，缓冲 0
```

解读：解码/解密全部成功，但 direct-feed 在 2 秒内把约 30+ 秒的音频（11.6M f32 采样）塞进 240ms 缓冲，必然溢出、丢帧，所以开头“破音”；
随后出现 `播放已暂停` / FLUSH / stream 重建，PCM 在 11:31:09 后基本不再增长。
“2 秒后无声”至少有两层：**direct-feed 无节奏导致初始 burst 破碎**，以及 **sender 暂停/断流后我们没有持续音频**。
不能只归因于网易云“格式不对”，因为同一套 AAC 解码在哔哩哔哩段是成功的。

### 5.2 哔哩哔哩（iPhone 发送）

用户描述：基本能正常播放，偶尔一下破音。

日志（11:33:30 之后）对应段：

```
11:33:41 underrun 18、预填充 39，缓冲 39/240 ms
11:33:49 underrun 28、预填充 53，缓冲 182/240 ms
11:34:09 underrun 49、预填充 79，缓冲 0/240 ms
11:34:09 已收 PCM 15,191,414 采样，push 6815 次，clear 29
```

解读：哔哩哔哩发送端大致按 1x 节奏发包，direct-feed“来一包送一包”在这种情况下近似实时，所以整体能听；
但 120ms 预填充/240ms 容量对 ColorOS 的调度抖动太小，underrun 频繁，表现为偶尔破音/卡一下。
另外日志中出现少量 `解码静音`（明文 6 字节）的告警，属于短帧/priming/静音帧，不是主因。

### 5.3 为什么两个 App 表现差这么多

- 现象看起来矛盾，其实是同一底层缺陷在不同发送端行为下的两种表现：
  1. **direct-feed 没有按 RTP 时间戳限速**，只按“收到就送”。发送端近似 1x 时（哔哩哔哩）问题被掩盖；
     发送端先发一段预缓冲/突发、再暂停或变节奏时（网易云），瞬间灌满 240ms 缓冲，开头破音，随后欠载/断流。
  2. **本地缓冲偏小**（240ms 容量 / 120ms 预填充）放大了 ColorOS 的调度抖动，所以哔哩哔哩仍有偶发破音。
  3. **sender 的 PAUSE / FLUSH / stream 重建** 在网易云段明显更多；需要区分“发送端真的暂停”与“我们误处理 FLUSH/RTSP”，目前日志只能确认发生了这些事件，不能证明是谁先发起。
- 因此不能得出结论“哔哩哔哩能用，所以 AirPlay 已经没问题”；它只是发送节奏恰好接近 direct-feed 的假设。

## 6. 已知根因/未决问题
## 6. 已知根因/未决问题

1. **direct-feed 无节奏（已实现修复，待真机验证）**
   - 1.4.3 `take_all` 一次性清空 playout buffer 送 core，导致 AudioBuffer
     溢出/欠载。
   - 修复：direct 模式改为 **paced direct**：保存本地 RTP 锚点（最早帧 timestamp +
     本地 now - 40ms），按 `target = anchor_rtp + elapsed * rtp_clock_rate` 用
     `take_ready(target)` 1x 投递；忽略发送端不可用的绝对 anchor；PAUSE 清空并
     重置锚点，RECORD 重新锚定；FLUSH 删除帧后重锚；积压超过 2s 时裁掉最旧帧。
   - 本地 Android 缓冲同时从 240/120ms 调到 **480/240ms**（Oboe 保持 LowLatency）。
   - 单元测试已覆盖 1x、2s/5s burst、backlog 上界、sender anchor 忽略、seek 回退、
     FLUSH 序列号；**尚无修复后的真机 underrun 数据**，不能把“预期”写成“已解决”。

2. **iPhone 音量固定 100% / 反向音量控制**
   - AirPlay 2 接收端音量是 receiver-side；FusionPlay 的 gain 只影响本地输出。
   - `GET_PARAMETER volume` 已返回当前 dB，但 sender 不主动查询/刷新 UI；没有标准
     的 receiver→sender 主动音量推送。
   - iPhone 是否显示接收端音量可能和 `SupportsVolume`（feature bit 32）有关；当前
     audio-only feature set **故意不含 bit32**（历史测试：加 bit32 会导致发现失败/
     立即 TEARDOWN）。不要在没有真机验证的情况下打开。
   - 需要确认用户所说的“反向控制手机音量”究竟是 AirPlay 标准行为预期，还是希望
     FusionPlay 通过私有 DACP/MediaRemote 改 iPhone 本地音量；后者不是公开协议。

3. **AirPlay 2 定时 playout 在无 PTP 时不可用**
   - `SETRATEANCHORTIME` 的 anchor 是网络/PTP 时间；无共享时钟时本地 fallback
     不断被重锚，缓冲头时间戳永远在 anchor 之后。
   - 1.4.3 用 direct-feed 绕过；长期方案是相对 RTP 锚定（不依赖发送端网络时间），
     或在有 PTP 的环境使用定时 playout。

4. **歌词**
   - 标准 AirPlay 2 DMAP 没有歌词字段；如果 iPhone 端显示歌词，不代表通过
     AirPlay 发送。当前只做“有 metadata 就显示、没有就提示”和未知标签诊断，
     不伪造歌词。
   - 若用户需要“手机歌词同步”，需要发送端 App 提供额外通道/厂商扩展，或改用
     其他投屏协议；这不是当前 AirPlay 接收端能单独解决的。

5. **身份/电视目标**
   - 已在 `/info`/`updateInfo`/`/server-info`/mDNS 统一 audio-only；如仍被识别为
     TV，需要抓包确认是否有其他自述路径（例如 HLS/video 请求）并检查 iOS 缓存。

6. **历史噪音/旧结论**
   - 1.4.1 曾把 AAC raw 误判为需要补 ADTS，导致数字静音；1.4.2 已纠正为
     raw 直解、ADTS 剥离。
   - 1.4.0/1.4.1 的 Oboe 480/240 + `PerformanceMode::None` 组合在 ColorOS 上
     无声，1.4.2 已回退到 LowLatency + 240/120。
   - 这些不是当前 1.4.3 的主要问题。

## 6.1 PCM 计数口径（重要）

代码里的 `pcm_received` / `delivered_samples` 统计的是 **f32 交织采样总数**
（stereo 时每帧 2 个 f32），不是每声道帧数，也不是字节数。换算：

- 每声道帧数 = 交织采样数 / 声道数（Android 输出固定 2）；
- 音频秒数 = 每声道帧数 / 48000。

所以 1.4.3 网易云段：

```
3,243,358 交织采样 / 2 = 1,621,679 帧/声道
1,621,679 / 48,000 ≈ 33.8 秒音频
```

在约 2 秒墙钟内送入 33.8 秒音频 ≈ **17x 实时**。如果误把 3,243,358 当“每声道
48000 帧”，会算成 67.6 秒，所以文档和日志已改为显式同时记录“交织采样 / 帧/声道 /
约多少秒音频”。B 站段的计数也按同一口径理解。

## 7. 关键文件索引

- 身份/发现：`vendor/shairplay/src/net/features.rs`、`net/mdns.rs`、
  `raop/handlers_ap2.rs`、`handlers_hls.rs`、`raop/config.rs`、`raop/server.rs`
- 解码/缓冲/调度：`vendor/shairplay/src/raop/buffered_audio.rs`、
  `codec/aac.rs`、`codec/alac.rs`、`codec/resample.rs`、`raop/realtime_audio.rs`
- 定时/PTP：`vendor/shairplay/src/raop/rtcp.rs`、`net/ptp.rs`
- 音量/远程控制：`vendor/shairplay/src/raop/handlers_ap1.rs`、
  `src/AirPlayReceiver.Core/src/audio.rs`、`raop/types.rs`
- 元数据：`vendor/shairplay/src/proto/dmap.rs`、`src/AirPlayReceiver.Core/src/audio.rs`
- Android UI：`flutter/lib/ui/player_view.dart`、`flutter/lib/models/lyrics.dart`、
  `flutter/lib/models/app_state.dart`
- Android 原生：`flutter/android/app/src/main/kotlin/com/fusionplay/android/FusionPlayRuntime.kt`、
  `.../backend/FusionPlayDiagnosticLogger.kt`
- 历史补丁说明：`vendor/shairplay/LOCAL_PATCHES.md`、`docs/AIRPLAY_AUDIO_ONLY.md`

## 7.1 自动化测试与修复前后对比

### 自动化测试（本次提交时）

- vendor `CI=1 cargo test --features hls --lib`：**232 passed**（含 paced direct 新增测试）；
- core `cargo test --lib --target x86_64-unknown-linux-gnu`：**124 passed**；
- 之前同一提交前跑过 vendor ap2 225 / hls 232 / video 226、core 124；
  `cargo build --features hls` 无 warning。
- 未做 30 分钟真机/虚拟长跑；paced direct 的 1x 模型有单元测试，长稳测试仍待补。

### 修复前后对比（前=1.4.3 实测；后=代码/单测预期，未真机）

| 项目 | 1.4.3（修复前） | paced direct + 480/240ms（修复后） |
|---|---|---|
| direct 投递 | `take_all`，收到即全送 | `take_ready`，按本地 RTP 锚点 1x |
| 网易云初始 burst | 2 秒内 3.24M 交织采样（约 33.8s 音频），约 17x | 积压最多保留 2s，按 1x 播放；单元测试 1s 约 47 个包 |
| Android 缓冲 | 120ms 预填充 / 240ms 容量 | 240ms 预填充 / 480ms 容量 |
| 网易云 | 约 2 秒破音后无声；underrun 2，缓冲 0 | **预期**连续出声；未真机验证 |
| 哔哩哔哩 | 约 1 分钟 49 次 underrun，偶发破音 | **预期** underrun 接近 0、缓冲更稳；未真机验证 |
| 延迟 | 约 2.0–2.2s | 约 2.2–2.5s（+240ms 本地缓冲） |

**结论边界：以上“修复后”是代码逻辑与单元测试的预期，不是 iPhone 实测结果。**

## 8. 给审查者的建议问题

1. 在无 PTP 的 AirPlay 2 接收器上，`SETRATEANCHORTIME` 的正确用法是什么？
   是否应该忽略 anchor 时间，仅用 RTP 时间戳相对首帧本地锚定？
2. iPhone 的 AirPlay 音量滑块到底控制 sender 本地音量还是 receiver 音量？
   是否需要 `SupportsVolume`（bit32）才能让 iPhone 显示/控制接收端音量？
   如果要反向同步，是否存在公开/可靠的 event/plist 通道？
3. 当前 `direct-feed` 改为 paced（RTP 相对锚点 + 1x 投递）是否足够，还是应该
   在 `SETRATEANCHORTIME` 后使用发送端 2s lead 做真正定时播放？
4. AirPlay 2 的 `FLUSHBUFFERED` 与 RTP sequence/timestamp 的正确映射关系；
   当前 seq_history 映射是否覆盖启动/seek/暂停恢复的所有情况？
5. 是否存在 Apple 私有的歌词/now-playing 扩展字段，值得在 DMAP unknown tag
   诊断中继续追查？

## 9. 复现/查看差异

```bash
# 基线到当前 AirPlay 相关文件差异
git diff b42984c..HEAD -- \
  vendor/shairplay/src \
  src/AirPlayReceiver.Core/src/audio.rs

# 只看 AirPlay 提交
git log --oneline b42984c..HEAD -- \
  vendor/shairplay src/AirPlayReceiver.Core/src/audio.rs
```

更早的原版（1.2.3 开源初始提交）：

```bash
git diff 58129cc..HEAD -- vendor/shairplay/src src/AirPlayReceiver.Core/src/audio.rs
```

## 10. 当前发布

- 最新 release：`android-v1.4.3`
- APK：`FusionPlay-Android-1.4.3.apk`，约 12.4MB，arm64-v8a
- 更新清单：`updates/android/stable.json` / GitHub Pages
- 1.4.3 包含旧的 direct-feed，节奏问题未修完。
- 当前 main 已提交 paced direct + 480/240ms 缓冲，但**尚未构建 APK、未经过 iPhone
  真机验证**；下一版应先构建测试包，用网易云音乐和哔哩哔哩各连续测试 10 分钟，
  重点看 underrun、缓冲是否稳定、网易云是否仍 2 秒断流，再谈音量/歌词。
