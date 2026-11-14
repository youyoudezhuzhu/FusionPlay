# 第三方参考项目与许可记录

FusionPlay 为 AGPL-3.0。以下项目仅用于**协议行为研究、实测数据对照与设计参考**；除特别注明外，没有拷贝其代码。

| 项目 | 许可 | 使用方式 |
|---|---|---|
| [MYHealer/miplay-mini-amp](https://github.com/MYHealer/miplay-mini-amp) | AGPL-3.0 | 参考其 README 的妙播实测结论（idHash 派生、AppData 布局、dev 类型表、显示名约束、内存占用）；其协议层源自同作者 RLCD 项目，与本仓库无代码复用 |
| [gitee mennian/esp_miplay](https://gitee.com/mennian/esp_miplay) | MIT | 参考 5355 legacy 应答报文结构、256KB 大帧经验、QQ/网易云 DLNA 兼容注释；未拷贝代码 |
| [MYHealer/esp-sonic-sync](https://github.com/MYHealer/esp-sonic-sync) | MIT | 参考 4 档漂移模型与 Sonic 旁路经验；FusionPlay 侧用自有 resampler 实现分级控制 |
| [MYHealer/dlna-esp32](https://github.com/MYHealer/dlna-esp32) | 未标注 | 仅阅读其 DLNA 行为说明 |
| [MYHealer/airkan-sdk](https://github.com/MYHealer/airkan-sdk) | Apache-2.0 | 仅评估"小米妙享/互联遥控"（HTTP 6095 认证 + TCP 6091 长连）协议；FusionPlay 尚未实现，若将来移植需保留其 Apache-2.0 声明 |
| [rbouteiller/airplay-esp32](https://github.com/rbouteiller/airplay-esp32) | GPL-3.0 + Espressif linking exception | 仅用于评估 ESP32 可行性；未拷贝 |
| [shairplay-rust](https://github.com/fabianlindfors/shairplay)（vendored 上游） | LGPL-3.0-or-later | 本仓库 `vendor/shairplay` 的上游；保持原有许可与版权头 |

注意：
- 若将来需要**直接拷贝**上述项目代码，必须逐文件确认许可兼容性（AGPL-3.0 仓库可吸收 MIT/Apache-2.0/AGPL 代码；GPL-3.0 与 Espressif 闭源解码器组合需要 linking exception）。
- 协议实现均为独立逆向/互操作研究；FusionPlay 与小米、Apple 无隶属关系，不宣称官方兼容认证。
