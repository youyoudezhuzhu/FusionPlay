# 妙播（MiPlay）设备标识、发现通道与诊断

本文汇总 2026-09 之后对小米妙播接收端的实测结论与 FusionPlay 的对应实现，来源包括
上游 Android 版逆向、[MYHealer/miplay-mini-amp](https://github.com/MYHealer/miplay-mini-amp)
（ESP32-S3 实机验证）与 [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay)（MIT）。

## 1. 设备类型（dev / sec / Lyra category）

`_mi-connect._udp` TXT 的 `dev` 决定手机妙播列表里的图标/分类。2026-09-28 实机逐个烧录验证：

| 值 | 手机显示 | 备注 |
|---|---|---|
| 1 | 手机 | |
| 2 | 电视 / Android TV | **默认值；实测可正常投屏** |
| 3 | 电脑 | |
| 4 | 音箱 | **部分 HyperOS 版本可能不进妙播投屏列表**（网络层可发现，但列表不显示） |
| 5 | 车 | |
| 6 / 7 / 8 | 蓝牙喇叭 | 三者显示相同（未映射值统一回落） |
| 18 | （未验证） | FusionPlay Android 版沿用的"平板"取值；ESP32 实测未覆盖，表现待确认 |

FusionPlay 默认 `television(2)`，设置里可切换。**如果选了"音响"后手机妙播列表里找不到设备，先切回"电视"再试**——这是手机端行为，不是接收端故障。

`sec` 字段：参考实现的文档记录 `sec` 与 `dev` 取同值；FusionPlay 历史上固定发 `sec=2`（TV 身份），默认 TV 时一致。其他类型下二者不一致是否会被手机拒收尚未实测，因此：

- 默认保持 `sec=2` 不变；
- 可用环境变量 `FUSIONPLAY_MIPLAY_SEC`（1..=5）在启动侧车前覆盖，便于实测（越界会回退 2 并记日志）。

Lyra 物理同步通道另有一套设备类别（TV=3、屏音箱=5、车=8 等），见 `src/FusionPlay.MiPlaySdk/src/lib.rs` 的 `lyra_protocol_value()`。

## 2. 显示名约束（重点）

妙播把接收端实例名拼成 `"<显示名>(<身份后缀>)"` 写进 mDNS/5355 的实例名（DNS label）：

- **不能包含 `.`**：点号是 DNS label 分隔符，手机解析 PTR/SRV 压缩指针时会把一个实例名当成两个 label，整包被判畸形而静默丢弃（表现："设备明明应答了，手机列表里没有"）。
- 实例名编码后必须 ≤ 63 字节（单 label 上限），后缀和括号也占长度；中文按 UTF-8 3 字节/字计算。

FusionPlay 的处理（`discovery.rs`）：

- 构建实例名时把 `.` 替换为 `-`，去掉控制字符/首尾空白；
- 按 UTF-8 字符边界截断，保证 `名字(后缀)` 总长 ≤ 63 字节；
- 5355 legacy 通道（见下）复用同一实例名，避免两条通道身份不一致。

> 注意：接收器名里的其他符号（emoji、空格）可以保留；真正会导致"设备消失"的是点号与超长。

## 3. 发现通道：mDNS 5353（默认）与 UDP 5355（opt-in）

手机侧存在两条发现路径：

1. **UDP 5355 `_miplay_lan._tcp`（legacy DNS-SD）**：`SystemUI` 在部分 HyperOS 版本上向 5355 发 PTR 查询；响应是 legacy unicast DNS（无 question section，PTR+SRV+TXT+A，SRV 端口 8899），TXT 为单条 JSON：
   ```json
   {"supportLyra":true,"mac":"AA:BB:CC:DD:EE:FF","ID":<MAC 后 4 字节大端 & 0x7FFFFFFF>,
    "type":3,"idhash":"<4 字符>","extraAbility":0}
   ```
   其中 `idhash` 在 5355 通道使用 **base64url** 字母表（`+`→`-`、`/`→`_`），必须与 mDNS TXT 的 `idHash` 同源。
2. **mDNS 5353 `_mi-connect._udp` + `_lyra-mdns._udp`**：FusionPlay 默认走这条（查询触发的 Lyra/Mi Connect 路由），身份字段见 `discovery.rs`。

历史原因：FusionPlay 1.x **刻意没有**常驻广告 `_miplay_lan`，因为 HyperOS 会把它当成"第二台通用电视"，造成重复/幽灵路由（见 `docs/MIPLAY_DUPLICATE_ROUTES.md`）。

因此 5355 通道做成 **opt-in**：

```powershell
# Windows：启动前设置，或写入系统环境变量后重启 FusionPlay
set FUSIONPLAY_MIPLAY_LEGACY_LAN=1
```

开启后如果出现重复设备/重复路由，请关掉并反馈；默认关闭时行为与旧版完全一致。

## 4. 大帧与内存

- 控制帧（`SetMediaInfo`）常携带封面 base64（WebP `UklGR…`）与逐字歌词（yrc），单帧可达 **256KB**；ESP32 侧 64KB 缓冲区实测会因超限丢同步。FusionPlay 控制帧上限 16MB，有 256KB 级别回归测试。
- 媒体侧 PES/ADTS 缓冲 64KB；妙播队列上限 1.5s、目标队列约 800ms（减去主机输出延迟）。

## 5. HE-AAC 与 codec 诊断

发送端可能使用 HE-AAC（AAC+，SBR/PS）。FusionPlay 的 Rust 解码器按 AAC-LC 初始化；已加入 `audio_codec_diagnostic` 事件：

- 每次会话首次 ADTS 帧时上报 `adts_profile`、`sample_rate`、`channels`；
- profile≠1 或 22.05/24kHz 时 `verdict` 为 `possible_he_aac`；
- 事件可在 `%LOCALAPPDATA%\FusionPlay\logs\fusionplay.log` / 诊断导出中看到，用于确认是否需要引入 SBR 解码器（涉及许可证评估）。

## 6. 可调诊断参数（环境变量，启动侧车前设置）

| 变量 | 默认 | 说明 |
|---|---|---|
| `FUSIONPLAY_MIPLAY_LEGACY_LAN` | 关 | 开启 UDP 5355 legacy 发现通道 |
| `FUSIONPLAY_MIPLAY_SEC` | 2 | 覆盖 TXT `sec`（1..=5） |
| `FUSIONPLAY_MIPLAY_LATE_DROP_MS` | 25 | 运行中迟到帧硬丢弃阈值（1..=500ms） |
| `FUSIONPLAY_MIPLAY_INITIAL_LATE_DROP_MS` | 2 | 起播阶段迟到帧硬丢弃阈值 |
| `FUSIONPLAY_MIPLAY_CLOCK_OBSERVER` | 关 | 输出时钟同步 JSONL 到数据目录 |
| `FUSIONPLAY_DLNA_USER_AGENT` | 浏览器 UA | 覆盖 DLNA 拉流 UA（个别 CDN 需要） |

## 7. 参考与许可

- [MYHealer/miplay-mini-amp](https://github.com/MYHealer/miplay-mini-amp)（AGPL-3.0）：ESP32-S3 最小耳放，其 README §9 给出了 idHash 派生、AppData 布局、dev 类型表、显示名约束的实机结论；协议层源自同作者 RLCD 项目，**不是**从 FusionPlay 拷贝。
- [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay)（MIT）：ESP32 上的 DLNA+MiPlay 实现，5355 legacy 应答与 256KB 大帧经验来自此项目。
- 本文数据仅用于协议兼容研究；FusionPlay 与小米公司无隶属关系。
