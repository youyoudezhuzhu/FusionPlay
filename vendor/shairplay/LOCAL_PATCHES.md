# 本地 shairplay 0.7.0 修改说明

本目录来自 `shairplay` 0.7.0 的 crates.io 发布包，并作为源码依赖参与构建。
项目没有把它伪装成未经修改的上游版本。

## 修改内容

- 为 `RemoteControl` 增加传输类型标识。
- 复用 AirPlay 2 加密 event channel，发送实验性的 MediaRemote 命令。
- 对 event channel 的入站明文进行 RTSP 分帧，支持 TCP/加密块分片与粘包；解析
  `updateMRSupportedCommands` 并使用相同加密通道回复带原始 `CSeq` 的 RTSP 200。
- 捕获 AirPlay 1/2 请求中的 `DACP-ID` 与 `Active-Remote`，优先通过
  DACP 发送媒体命令，并校验 HTTP 2xx 响应；DACP 失败后可回退到 MediaRemote。
- 解析 MediaRemote 的播放 `0`、暂停 `1` 与切换 `2`；发送端只公布
  Play/Pause 时，会结合播放速率选择正确命令。
- 连续播放/暂停时，以接收端最新控制意图对齐 MediaRemote 与 RTSP 两条独立
  通道；短时忽略乱序到达的旧速率回报，避免旧暂停覆盖新播放。
- 支持播放、暂停、播放/暂停切换、上一曲、下一曲，以及按绝对位置跳转；
  跳转通过 DACP `dacp.playingtime` 或 MediaRemote 命令 `24` 发送。
- 正确处理发送端回发的 RTSP `PAUSE`：返回 200、暂停 AirPlay 2
  buffered playout 并保留 RTP/RTSP 会话；后续 `RECORD` 恢复播放状态。
- event channel 断开后立即撤销该会话的控制能力。
- 区分 AirPlay 2 流级与连接级 TEARDOWN：停止单条音频流时保留主 RTSP
  连接和事件通道，只有连接级 TEARDOWN 才断开。
- 未识别的 buffered RTP SSRC 不再伪报为 AAC、48 kHz 或双声道。
- 增加 MediaRemote plist、RTSP 封装、命令能力解析和去重测试。
- 删除发布包中仅供示例使用的 dev-dependencies，避免本地库测试解析无关 GUI 依赖。
  为运行 `tests/integration.rs`（需要 `serial_test`），补回最小 dev-dependency；
  `examples/player` 仍依赖 cpal/serde/tracing-subscriber，因此用 `dev-player`
  feature 将其排除在默认 `cargo test` 之外。

## 2026-11 AirPlay 2 协议栈补丁

- **buffered ALAC（type 103）**：SETUP 解析 `audioFormat`/`spf`；每条 103 连接按
  RTP SSRC 选择 ALAC 或 AAC 解码器（ALAC 使用标准 `ALACSpecificConfig`：
  frameLength=`spf` 或 4096、pb40/mb10/kb14/maxRun255，采样率/位深/声道来自
  SSRC）。格式切换会重建解码器/重采样器；初始化失败发 `on_error`，不再静默。
- **未知 SSRC / OS 27**：`AudioSsrc::None`（含 ACK/flush 的 `SSRC_NONE`）包在解密
  前跳过，不更新当前格式、不喂旧解码器、不定锚；首个有效音频包才定锚。
- **SETMAGICCOOKIE**：新增 `POST /SETMAGICCOOKIE` 与裸方法 `SETMAGICCOOKIE *`
  路由（200、bplist 仅 debug 解析）；AP2 `OPTIONS` 的 `Public` 增加
  `SETMAGICCOOKIE`/`SETPEERSX`。
- **RTCP control channel**：新增 `src/raop/rtcp.rs`；103 SETUP 的 `controlPort`
  UDP socket 保留并 spawn 监听任务（Drop 时 abort，无泄漏）。解析 type 215
  PTP / type 212 NTP time announce（compound/padding/畸形安全），最新 announce
  存入 `RaopShared::rtcp_announce`，在缺少 `SETRATEANCHORTIME` 权威 anchor 时
  经 `PtpClock` 换算成本地播放锚点（仅 PTP、2s 过期、不覆盖已有 anchor）；
  `SETRATEANCHORTIME` 的 timeline id 与 PTP master 不匹配时用 announce 的
  clock identity 兜底。
- **长时播放**：RTP 时间戳扩展为 64-bit wrap-aware 时间线（处理乱序/多圈 wrap/
  seek 回退 rebase），buffer/anchor/target/flush 全部使用扩展时间；解码 PCM
  缓冲上限约 20s。
- **PTP（best effort，无 NQPTP）**：Announce 主时钟选择（优先级/步数，切换时
  重置 pending Sync 与平滑器）、忽略非选中 master 的 Follow_Up、Delay_Req/Resp
  往返路径延迟测量并补偿；无 announce/delay 时保持原行为。无共享 PTP 时钟时
  不保证与其他接收器 sample-accurate 同步。
- **安全加固**：ALAC 解码器校验 frame length/位深/rice 参数/声明样本数/声道布局/
  输出边界，并用 wrapping/i64 运算杜绝畸形输入 panic；random frame+config 2000
  次 fuzz 单测；RTCP 随机 datagram fuzz 单测。
- **SETPEERS/SETPEERSX**：peer 列表记录到 `RaopShared::peers`（仅诊断，不影响
  单机播放逻辑）。
- **测试元数据**：修正 `tests/integration.rs` 中过时的 AP1 `Audio-Latency: 0`
  断言（现为 11025）与 HLS 构建 feature bits 期望；新增 SETMAGICCOOKIE
  integration 测试。
- **真机验证限制**：无 iOS/macOS 27 设备，SETMAGICCOOKIE、Apple Music 无损
  buffered ALAC、RTCP timing 仅通过单元/集成测试与公开抓包结构验证。

## 逆向依据与限制

MediaRemote 不是公开的 AirPlay 2 API。本实现参考了 2026 年 2 月公开的协议抓包：

<https://gist.github.com/MinshuG/20225d1923999c980d8545f7ac46fe6f>

抓包直接确认了：

- event channel 中的 `POST /command RTSP/1.0` 二进制 plist 形状；
- `sendMediaRemoteCommand`；
- 播放 `0`、暂停 `1`、播放/暂停切换 `2`；
- 下一曲 `4`、上一曲 `5`、推进循环模式 `7`。
- 绝对位置跳转 `24`，位置放在
  `kMRMediaRemoteOptionPlaybackPosition`（秒）。

以下仍是实验性推断，必须针对目标 iOS/iPadOS/macOS 版本做真机验证：

- `DestinationDeviceUIDs` 当前使用接收器的 AirPlay pairing ID（`pi`）；
- 下一曲、上一曲、循环命令的兼容 `value` 字符串；
- 自定义 `SenderID` 的接受范围；
- 分组/多房间路由中的目标 UUID 语义。

命令成功结果只表示消息已经写入加密 event channel，不表示 Apple 发送端已经执行。

## 许可证

上游 Cargo 元数据声明 `LGPL-3.0-or-later`，但
`src/raop/config.rs` 标注 `GPL-3.0-only`。请同时阅读本目录的 `LICENSE`
和项目根目录的 `THIRD_PARTY_NOTICES.md`。在上游澄清前，本项目只按内部研究原型交付。

## 2026-11 Android 1.4.0 实机问题修复

- **AirPlay 被识别成 TV/投屏目标（已修）**：根因有两个，均在"接收器自述"路径上：
  1. `GET /info`（及加密 event channel 的 `updateInfo`、`/server-info`）此前用
     `receiver_features_for_pairing` 生成 `features` 且固定 `model=AppleTV6,2`，
     `hls` 构建下会带上 bit0（AirPlay video v1）/ bit4（HLS），iOS/macOS 因此把
     音频接收器当电视/投屏目标。现在这些响应根据 `audio_only` 使用纯音频
     feature set 和 `AirPort10,115` 模型。
  2. `receiver_features_for_pairing_with(audio_only=true)` 不再从 `video` 特性集
     清位，而是直接构建纯音频 AP2 feature set（见 `audio_only_features`），
     保证任何 feature 组合下都不含 bit0/1/2/4/5/6/7/10/12/13/25/28/33/42/44/49。
  - mDNS `_airplay`/`_raop` 的 `features`/`ft` 与 `am`/`model` 本来就是纯音频值
    （0x0001C340405D4A00 / AirPort10,115），现在 `/info` 与真实广播一致。
  - 回归测试：vendor `net::features`、`net::mdns`、`handlers_ap2::info_profile`；
    core `host::tests::audio_only_settings_produce_an_audio_only_service_info`
    与 Android `android_jni` 设置断言。
- **AirPlay 2 播放卡顿稳定化**：审查未发现丢包/时间戳解码 bug（AAC 44100→48000
  路径、buffered 103 播放均正常）。低风险稳定化：
  - Android 本地输出缓冲 240→480 ms、预填充 120→240 ms（仍远小于协议 2s）；
  - Oboe `PerformanceMode::LowLatency` → `None`（ColorOS/OPPO 上 Shared+LowLatency
    的小硬件缓冲更易 underrun）；
  - `AudioBuffer` 增加限频可观测性：累计 underrun / 重新预填充次数、当前缓冲 ms，
    启动时输出一条缓冲配置日志，发生欠载时最多每 2s 一条 warning（供下次日志确认）。
  - 延迟影响：稳态本地延迟 +240 ms（约 2.24–2.48 s 总延迟），换来更稳的输出。
- **AirPlay 歌词协议诊断**：核对 AirPlay DMAP 标签（vendor 解析器、shairport-sync、
  openairplay `dxxp.py`）均**没有歌词字段**；`TrackMetadata` 只有
  minm/asar/asal/asgn/astm/astn/asdk。因此不伪造 lyrics 下发。新增协议层诊断：
  未知 DMAP tag 以「4 字符标签+长度」形式（不含任何内容）经 core
  `CoreEvent::Log` 每标签限频记录一次，供用户下次导出日志确认是否有厂商扩展
  （如 `asly`）。日志中 now_playing 的 title_length 变化通常是发送端 App 自身的
  now-playing 标题更新，不能据此认定存在歌词字段。

## 2026-11 Android 1.4.1 无声/音量修正

1.4.1 实机日志（AirPlay 2、AAC 44100→48000、`playback_state playing:true`）显示：
- `stream_started` 正常、进度/元数据/远程控制正常，但本地输出始终
  `Android 音频输出欠载 ... 当前缓冲 0 ms / 容量 480 ms`；
- FusionPlay 调音量返回 `当前投放设备不支持此远程控制命令`，iPhone 端音量固定 100%。

修复：

- **AAC 帧格式**：AirPlay 2 buffered AAC 明文是 **raw AAC-LC data block**（无 ADTS
  头）。symphonia 的 AAC 解码器直接吃 raw data block；给它喂拼接了 ADTS 头的
  数据会被当成原始比特流解成数字静音。`AacDecoder` 现在：
  - 检测 ADTS（sync + 帧长校验）时先**剥离** 7/9 字节头再解码；
  - 原始 payload 直接解码（生产路径）；
  - 只在 raw 解码失败时用合成的 ADTS 头做兼容回退（`wrap_adts`）。
  - 新增真实音频测试：`tests/data/aac/tone_44100_stereo.aac`（ffmpeg 编码的
    440 Hz AAC-LC 44.1k 立体声）——raw 帧解码、ADTS 剥离一致性、连续多帧、
    以及 `receive_loop` 端到端（解密→解码→重采样→入 playout buffer，断言非静音）。
- **Android 输出配置回退到 1.4.0 可出声配置**：Oboe `PerformanceMode::LowLatency`，
  本地缓冲 240 ms / 预填充 120 ms。1.4.1 的 `None` + 480/240 在 ColorOS 上没有
  产生任何可听输出，因此以恢复声音优先；`AudioBuffer` 的 underrun/预填充计数与
  启动日志保留，并新增「音频数据已开始输出」「输出已开始播放」以及
  「会话建立 6 秒内未收到解码 PCM」告警，用于下一份日志定位。
- **接收端音量**：AirPlay 音量属于接收端能力，不再要求 sender 的 MediaRemote
  暴露 `SetVolume`：
  - core `set_volume` 直接设置本地输出 gain（0–100% → 0–1，0% 记为 -144 dB），
    返回成功；sender 若确实支持 `SetVolume`（AP1 DACP）则 best-effort 镜像失败
    不影响本地；
  - `GET_PARAMETER volume` 返回接收端当前 dB（原来是固定 0 dB，导致发送端永远
    显示 100%）；
  - sender 的 `SET_PARAMETER volume` 与本机 UI 音量共享同一状态；`CoreEvent::Volume`
    同时上报 dB 与百分比，Android 诊断日志现在会记录 `db`/`volume_percent`；
  - Android 端 AirPlay 路由不再同时改系统媒体音量（core gain 为唯一音量），
    避免双重衰减；DLNA/妙播路径保持原行为。
- **缓冲诊断**：buffered/realtime 音频管线新增限频诊断（收包、解密失败、解码失败、
  解码静音、诊断行每 10 秒），经新增 `AudioHandler::on_diagnostic` 写入 core
  `CoreEvent::Log`（持久化日志 `persistent/fusionplay.jsonl`）。

## 2026-11 Android 1.4.2 无声根因与声音优先兜底

1.4.2 实机日志（`/tmp/fplogs3`，OPPO OPD2506，AAC 44100→48000）证据：
- 收包/解密/解码全部正常（`收包 1895、解密失败 0、解码失败 0、解码静音 0`）；
- core 收到首块 PCM（2230 采样）后**再无后续**：本地 `AudioBuffer` 一直
  `underrun 1/2、重新预填充 3→11、缓冲 0 ms`，且从未出现
  `Android 音频输出已开始播放`；
- 结论：问题不在解码/音量，而在 **buffered_audio 的定时 playout**：解码后的 PCM
  只被 `delivery_loop` 送出首帧，之后全部滞留。

根因（安全推断）：`SETRATEANCHORTIME` 的 anchor 时间是**网络/PTP 时间**，而本机
没有 PTP 共享时钟，`set_rate_anchor` 走 `LocalFallback` 时把 `anchor_local` 设为
本地 now，忽略发送端约 2 s 的 lead；发送端周期性重发 SETRATEANCHORTIME 时每次都
把 anchor 重置到“现在”，而缓冲头的 RTP 时间戳始终在 anchor 之后约 2 s，
`playout_target_rtp` 追不上，于是永远没有 due 帧（只有 RECORD/Resume 锚定到最早
已缓冲帧的那一刻会送出首帧）。这也解释了 1.4.0/1.4.1 的“偶尔响一两声”。

修复：
- **直接喂音（direct-feed）兜底，默认开启**：`buffered_audio` 在
  `receive → decrypt → decode → resample` 成功后，`delivery_loop` 立即按 RTP 时间
  顺序把所有已缓冲帧送入 `AudioSession`（`take_all`），不再依赖 anchor/rate/
  target 的定时门控；本机 `AudioBuffer` 的 120 ms 预填充继续吸收网络抖动。
  RTSP PAUSE 时丢弃缓冲帧、RECORD 恢复，避免暂停时积压。
- FLUSH 语义修正：`FLUSHBUFFERED` 的 `flushFromSeq/flushUntilSeq` 是 **RTP 序列号**，
  旧代码拿它和 RTP 时间戳低 32 位比较（可能清空整段或完全无效）。现在按收到的
  包维护 `seq(16-bit) → 扩展时间戳` 历史（上限 4096），仅删除真正落在该序列号区间
  内的帧；无法映射时**忽略**而不是误删，并记录诊断。
- 诊断增强（限频写入 core 日志）：
  - buffered：收包数、解码帧/采样数、解密/解码失败、解码静音；
  - delivery：模式（direct/timed）、已送出帧/采样、待播帧、rate、flush 次数/删除帧数；
  - FLUSH：请求 seq 区间、映射包数、实际删除帧数；
  - Android 输出每 10 s 心跳：已收 PCM 采样、push 次数、clear 次数、锁失败次数、
    underrun/预填充、当前缓冲；并保留 underrun/“输出已开始播放”日志。
- 定时 playout 代码保留（`BufferedSourceConfig::direct_feed = false`），待真机验证后
  可恢复；若恢复，需要为无 PTP 场景改用相对 RTP 锚定（把 anchor_local 前移到
  缓冲头），否则仍会饥饿。

## 2026-11 Android 1.4.3 后续：paced direct + 更大本地缓冲

1.4.3 实机日志（`/tmp/fplogs5`，OPPO PKJ110）暴露 direct-feed 的节奏问题：
`take_all` 把发送端瞬时到达的帧全部塞进 240 ms 的本地队列 —— 网易云音乐
（首个 ~1 MB 预缓冲约 30 s 音频在 2 s 内到达）瞬间溢出：11:30:49 两秒内
`已收 PCM 3.24M 采样 / push 1455`，随后缓冲 0、underrun，只剩破音；哔哩哔哩
发送端大致 1x，能持续播放但 ColorOS 回调抖动下 underrun 快速增长（1 分钟 49 次）。

修复：

- **paced direct（本地节奏直投）**：不再 `take_all`。direct 模式用纯本地锚点
  （`direct_anchor_rtp = 最早缓冲帧时间戳`，`direct_anchor_local_ns = now - 40ms`），
  之后 `target = anchor_rtp + elapsed*rtp_clock_rate`，用 `take_ready(target)` 按 1x
  投递。发送端 `SETRATEANCHORTIME` 的绝对 anchor 一律忽略（没有共享时钟），
  `rate` 只用于暂停/恢复；暂停清空缓冲并重置锚点，RECORD/恢复重新锚定最早帧；
  RTP timeline rebase、新 stream、FLUSH 删除帧、seek 回退都会重置锚点。
- **延迟上界**：direct 模式若发送端一次送来超过 2 s 的积压，按 RTP 时间戳删掉最旧的
  帧、只保留最近 2 s（`trim_direct_backlog`），避免延迟无限增长；40 ms 提前量
  让本地队列有一点调度余量。
- **Android 本地缓冲调大**：`OUTPUT_BACKLOG_MILLISECONDS = 480`、
  `OUTPUT_PREFILL_MILLISECONDS = 240`，Oboe 仍为 `LowLatency`（不用 `None`）。
  稳态本地延迟 +240 ms（总计约 2.2–2.5 s，仍由 AirPlay 协议 2 s 主导），用于吸收
  ColorOS 回调抖动、消除 B 站段的 underrun/破音。
- **诊断**：播放诊断行现在包含模式（direct-paced/timed）、已送出帧/采样、
  最近一个窗口的采样率（应 ≈ 输出采样率×声道 ≈ 96000 f32/s）、待播帧、锚点与本地
  偏移、flush/删除帧、backlog 丢弃帧数；Android 输出心跳含已收 PCM/push/clear/
  锁失败/underrun。
- 测试：`paced_direct_feed_releases_frames_at_one_x`（2 s 预缓冲首轮只放 due 帧，
  1 s 只放约 47 帧）、`direct_anchor_ignores_the_senders_absolute_anchor`、
  `direct_anchor_resets_when_earliest_frame_moves_backwards`、
  `direct_backlog_is_bounded_to_two_seconds`。
- 关于网易云“2 秒后无声”：日志中 `playback_state false/true` 来自发送端
  `SETRATEANCHORTIME` 的 rate 更新（真实 sender PAUSE/RECORD），并有真实
  `FLUSHBUFFERED` 与 stream restart，不是我们把 FLUSH 误当 PAUSE。其 2 s 后停止
  与本机 240 ms 缓冲被 30 s burst 溢出同时发生；paced direct + 2 s backlog 上界
  后可继续按 1x 播放。若 sender 在填满其 1 MB 预缓冲后仍需反馈才继续，下一步应
  发送 RTCP RR/或调小 SETUP 回复中的 `audioBufferSize`（当前 0x100000），再用日志
  确认 sender 是否继续按 1x 发包。
