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
