## FusionPlay Android 1.4.5

### 1.4.4 复盘

1.4.4 的 paced direct 已经在跑，但网易云仍“约 2 秒破音后无声”。日志确认：
- 网易云约 35 秒 AAC 在约 5 秒内突发到达；
- 旧实现 `trim_direct_backlog` 删除旧帧后，**播放锚点仍停在已删除的旧帧位置**，target 长期追不上缓冲头，于是没有 due 帧；
- 所以 1.4.4 的节奏修复没有真正生效。

### 1.4.5 修复

- **burst 快进 + 重锚**：当缓冲最新帧领先 target 超过 5 秒时，丢弃旧 burst、只保留最新约 2 秒，并把本地锚点前移到新缓冲头；之后恢复 1x 投递。单元测试覆盖 35 秒突发。
- **`audioBufferSize` 实验**：SETUP(103) 从 1MB 改为 **64KB（约 2 秒 AAC）**，让发送端不要一次灌几十秒；SETUP 诊断会打印实际值。若某发送端因此不工作，可回退到 1MB。
- **FLUSHBUFFERED 正确解析**：优先使用 `flushFromTS/flushUntilTS`（RTP 时间戳），没有时再用 23-bit 序列号映射；RTP 序列号按前 4 字节 &0x7FFFFF 解析。
- **RTSP 方向诊断**：每 5 秒记录 SETUP/RECORD/FLUSHBUFFERED/SETRATEANCHORTIME/feedback/TEARDOWN/SET_PARAMETER 计数与最后 CSeq/响应，用于判断网易云是 sender 主动暂停还是我们处理有误。
- Android 心跳增加 push/s、pop/s、已收 PCM/s（期望约 96000 f32/s），便于区分发送端停发、投递突发、输出消费过快。
- 未继续盲目加大缓冲，保持 480/240ms + LowLatency。

### 验证建议

- 用网易云音乐和哔哩哔哩各连续播放 10 分钟。
- 重点看：
  - 是否连续出声、开头是否还破音；
  - `AP2 SETUP(103)：audioBufferSize=65536`；
  - `RTSP 接收诊断` 是否显示 sender 主动 PAUSE/TEARDOWN；
  - `播放诊断` 的最近 f32/s 是否接近 96000，backlog 是否只在开始时丢弃一次；
  - `Android 输出诊断` 的 push/s 与 pop/s 是否都约 96000。
- 如果网易云仍 2 秒断流，请导出日志；下一步将根据 RTSP 诊断确认 sender flow-control，再考虑 RTCP RR/feedback 或回退 audioBufferSize。
- 本次仍未经过 iPhone 真机验证，不能宣称已彻底解决。

### 安装

- 版本 1.4.5 / versionCode 10405，固定 release 签名，可直接覆盖 1.4.4。
- APK 约 12.4MB，arm64-v8a；1.3.4+ 可在「设置 → 检查更新」升级。
