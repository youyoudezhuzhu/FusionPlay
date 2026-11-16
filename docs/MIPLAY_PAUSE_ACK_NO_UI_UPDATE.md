# MiPlay 接收端：收到 PAUSE 并回 ACK，但播放状态/进度条不更新（legacy 发送端）

- 状态：待排查（问题报告，未修改代码）
- 报告时间：2026-11-16
- 涉及组件：`FusionPlay.MiPlaySdk/src/protocol.rs`（8899 控制服务）
- 对端：PushMi（Android MiPlay 发送端，2.6.20a10），走 legacy `SAFETY_AUTH` 路径（0x02/0x03 + 0x6c + OPEN + RTSP）

## 1. 现象

PushMi 开始推送后（FusionPlay 正常显示封面/歌名/进度）：

1. 在 FusionPlay 播放页点「暂停」：手机侧音乐确实暂停了（PushMi 日志 `音箱请求：暂停`），FusionPlay 也把暂停请求成功发给了手机。
2. 但 FusionPlay 界面仍显示「播放中」，进度条继续往前走（不是停住）。
3. 再点一次暂停键不能恢复（因为 UI 一直认为在播放）；从手机侧恢复/暂停，FusionPlay 仍然显示播放。
4. PushMi 侧确认发送了 `PAUSE(0x04)` 和 `SET_MEDIA_STATE(0x5E, 0)`，并收到了接收端的 `PAUSE_ACK`：

```
11-16 23:16:39.577  音箱请求：暂停
11-16 23:16:42.834  音箱已确认暂停
11-16 23:16:44.839  音箱已确认暂停   # 发送端暂停期间每 2s 重发 PAUSE + 暂停快照
11-16 23:16:46.843  音箱已确认暂停
11-16 23:16:48.850  音箱已确认暂停
```

5. FusionPlay 日志里只有 `remote_control action=pause`（UI → 手机方向），没有对应的 `playback_state` 暂停事件：

```
13:57:44.983 remote_control action=pause outcome=success
...
playback_state applied active_state_updated   # 大量，只有这一种 reason
```

即：**ACK 回了，但接收端没有把「已暂停」应用到播放状态/UI。**

## 2. 代码位置与初步分析

`protocol.rs` 中 PAUSE/RESUME 的处理顺序是：

1. 先回 `PAUSE_ACK` / `RESUME_ACK`；
2. 再查 `hub.active_source_session_id(session_id, remote.ip())`；
3. 只有查到才调用 `observe_playback(...)` 和 `emit_playback(...)` 更新状态/UI。

推断：第 2 步对 PushMi 这种 legacy 发送端返回了 `None`，于是：

- ACK 正常（第 1 步已执行），所以发送端看到「已确认暂停」；
- 但 UI 状态没有更新，`playback_state` 不产生暂停事件；
- 进度条继续按「播放中」外推。

可能的根因方向（按优先级）：

1. **legacy 控制会话没有被登记为 active source**：`register_control_route` / `active_source_session_id` 的映射只在新 HyperConnect/OPEN 流程里建立，legacy `SAFETY_AUTH` + OPEN 的会话没有登记，或登记的 `session_id` 与收到 PAUSE 的控制连接不一致。
2. `SET_MEDIA_STATE(0x5E)` 的 paused 快照按现有设计被 `observe_playback_snapshot` 忽略（只接受 weak resume），所以唯一能改 UI 的只剩 PAUSE 命令，而它被上面的映射挡掉了。
3. 媒体/控制连接分属不同 socket（companion socket）时，PAUSE 到达的控制连接不是登记 OPEN 的那一条。

## 3. 建议排查步骤

1. 在 PAUSE/RESUME 分支里打印 `session_id`、`remote.ip()`、`active_source_session_id(...)` 的返回值和当前 `control_routes` 的 key 列表，用 PushMi 复现一次即可看到映射缺失。
2. 检查 `register_control_route` 与 `mark_*` 的调用点：legacy 路径（`SAFETY_AUTH` 握手、`0x00 OPEN wfd://`）是否也会登记 route；`active_source_session_id` 是否按 remote IP 做了 fallback。
3. 修复方向（任选其一或组合）：
   - PAUSE/RESUME 在拿不到 `active_source_session_id` 时，按 `remote.ip()` 找最近的 active session（或最近一次 `observe_media_info` 的会话）更新 `playback_gate` 并 `emit_playback`；
   - 或把 legacy 控制连接登记成 source session（推荐，能同时修复 `SET_MEDIA_STATE` 等其它状态同步）；
   - 保留「先 ACK 再更新」的顺序不变，避免发送端重试。
4. 验证：PushMi 2.6.20a10 开始推送 → FusionPlay 点暂停 → 期望 FusionPlay 显示暂停、进度条停住；再点恢复 → 恢复播放；手机侧暂停/恢复 → FusionPlay 同步。

## 4. 复现环境

- 发送端：PushMi Android 2.6.20a10（legacy SAFETY_AUTH MiPlay 发送端，8899 控制 + RTSP 音频）
- 接收端：FusionPlay Android（1.4.8/1.4.9 均复现）
- 网络：同一 Wi-Fi 局域网直连
- 备注：同一发送端对真实小米音箱（客厅小爱音箱 Pro）发 PAUSE 时，音箱回 ACK 且接受暂停；问题只在 FusionPlay 接收端出现。
