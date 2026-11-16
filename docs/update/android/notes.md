## FusionPlay Android 1.4.3

### AirPlay 2 无声修复（重点）

- 根因不是解码、音量或 Oboe，而是 AirPlay 2 缓冲音频的**定时 playout 锚点**：没有 PTP 共享时钟时，`SETRATEANCHORTIME` 的本地回退锚点会不断重置到“现在”，发送端预缓冲的约 2 秒音频永远追不上播放目标，整场只送出首帧。这解释了 1.4.0–1.4.2 的“偶尔响一声/完全无声”。
- 1.4.3 改为**声音优先 direct-feed**：解密、解码、重采样后按 RTP 时间顺序立即送入本地输出，不再依赖不可用的播放锚点；本地 240ms/120ms 缓冲继续吸收网络抖动。定时 playout 代码保留在开关后，后续做多房间同步时再恢复。
- 修正 `FLUSHBUFFERED` 语义：用 RTP 序列号→时间戳映射，只删除真正落在 flush 区间的帧；无法映射时忽略，避免误清空。
- 新增诊断：收包/解码/静音计数、delivery 送出帧/采样/待播、FLUSH 删除帧、Android 输出 10 秒心跳（PCM/push/clear/underrun）。下次日志可直接区分“vendor 没送帧 / core 没 push / 缓冲未填充”。

### 音量

- FusionPlay 音量继续走接收端本地 gain，不依赖 iPhone 的远程 SetVolume；声音恢复后调音量会直接改变输出响度。
- iPhone 控制中心显示 100% 是发送端自身 UI 状态：AirPlay 2 没有标准的“接收端音量主动推送到发送端”机制，接收端只会在发送端查询时返回当前 dB；本轮不改变这一协议行为。

### 安装包

- 保持 1.4.2 的 arm64-v8a 单架构和字体子集，APK 约 12.4 MB；32 位设备需单独用 `-Abis armeabi-v7a` 构建。

### 安装

- 版本 1.4.3 / versionCode 10403，固定 release 签名，可直接覆盖 1.4.2。
- 1.3.4 及以上可在「设置 → 检查更新」直接升级。
