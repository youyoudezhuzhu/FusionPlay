# FusionPlay 播放界面最终调整报告

## 0. 参考图说明

本轮最终一次消息里附了两张示意图（有歌词/无歌词），已按图调整：左侧协议只保留协议名；删除音质字段与封面下“来源/格式”小字；时间紧贴进度条；封面改为独立的 `FloatingAlbumArt`，去掉黑色厚底座；无歌词改为居中的“封面 + 信息”组合。

示意图位图尺寸约 1836×857，非实际 Android 截图；无法做逐像素 golden 对照，只能按布局关系对齐。

## 1. 第三方液态玻璃

### 审计

| 项目 | 结果 |
| --- | --- |
| `sdegenaar/liquid_glass_widgets` 最新源码 | 1.17.5（MIT），`flutter: ">=3.41.0"` |
| 备选 `whynotmake-it/flutter_liquid_glass` | 当前要求 Flutter >= 3.47，不能用于本项目 |
| 本项目 SDK | Flutter 3.32.8 / Dart 3.8.1 |
| 结论 | 最新版不可直接依赖；使用 **0.18.4（MIT）** vendored 源码 |

### 实际接入

- 路径：`flutter/third_party/liquid_glass_widgets/`
- 版本锁定：0.18.4；保留 MIT `LICENSE` 与版权声明；根目录 `THIRD_PARTY_NOTICES.md`。
- 兼容性补丁：移除 4 处 Flutter 3.32 不支持的 `filterQuality:` 参数；`vector_math` 固定 2.3.0。
- 未修改 shader 算法/视觉参数。
- `main()` 调用 `LiquidGlassWidgets.initialize()`；播放页用 `LiquidGlassWidgets.wrap` 提供共享层上下文。
- Android 渲染路径：库使用 fragment shader / Impeller；**Widget 测试运行在 Skia，会走库的 lightweight fallback，不是真机折射**。本轮没有 Android SDK/真机，无法验证 Android 上的最终光学效果，故不标记“Android 玻璃已通过”。

### 使用范围

仅：

- 上一曲 / 播放暂停 / 下一曲：`GlassIconButton`，同一材质参数，仅尺寸不同；
- 音量胶囊：`GlassContainer` + 普通滑杆（避免玻璃套玻璃）；
- 顶部歌词显隐、更多菜单：`GlassIconButton`。

**没有**给封面、歌词区或整个播放器加玻璃外框；旧的 `_GlassPaint`/自研高光 shader 已从播放页移除，也没有叠加到第三方玻璃上。

## 2. 布局

- 无歌词：左侧悬浮大封面；右侧标题/歌手/专辑、进度/时间、三个玻璃按钮、玻璃音量胶囊。无大玻璃卡片。
- 有歌词：左侧封面/信息/进度/按钮/音量，右侧背景直铺歌词；无歌词玻璃框。
- 悬浮封面：1:1、圆角、多层柔和环境阴影 + 极细边缘环境光；无厚黑边、无持续大幅旋转；保留轻微呼吸。
- 不使用 `FittedBox.scaleDown`；无底部 Dock；无旧纵向居中堆叠。
- 左栏宽度按可用宽度动态计算（约 40%，有最小/最大约束），封面按左栏高度预算计算。
- 手机 21:9、16:9、平板、TV 均用同一组件树，通过实际约束/高度预算调整尺寸。

## 3. 逐句流式歌词

- 完整 LRC/逐字时间轴：继续走 `LyricsView`，保留逐字卡拉 OK、按真实时间滚动、当前行高亮。
- 只有当前一句：`_StreamingLyricsView` 维护最近 **7 句**历史；最新一句为当前高亮，旧句上移并降低透明度；只有一句时在右侧略低于中央显示。
- 不伪造未来歌词、不伪造逐字时间戳。
- 历史队列绑定 `trackKey`（当前歌曲/播放会话）；切歌清空，重复行按 `startMs+文本` 去重，避免旧歌词混入。
- AirPlay 标题歌词继续使用标题捕获历史，只展示最近 7 句。

## 4. 截图

`docs/player-final/`

- 21:9 有歌词：`lyrics-840x360.png`、`lyrics-900x400.png`、`lyrics-980x420.png`、`lyrics-1008x432.png`、`lyrics-1080x462.png`
- 21:9 无歌词：`no-lyrics-840x360.png`、`no-lyrics-1080x462.png`
- 逐句：`stream-single-line-840x360.png`、`stream-history-840x360.png`
- 16:9：`lyrics-1280x720.png`
- 平板：`tablet-1280x800.png`
- TV：`tv-1920x1080.png`
- 长标题：`lyrics-long-title-840x360.png`
- 特写：`glass-controls-closeup.png`、`volume-capsule-closeup.png`、`floating-cover-closeup.png`

## 5. 实际验证

- `flutter analyze`：**无 error、无 warning**；20 条 info，全部来自 vendored `liquid_glass_widgets` 的 deprecated API 提示。
- `flutter test`：**207 passed**。
- 21:9 尺寸 840×360 / 900×400 / 980×420 / 1008×432 / 1080×462 无 overflow；16:9、1280×800、1920×1080 也已渲染。
- 新增流式歌词测试：单句 → 第二句 → 第三句，历史保留且不伪造未来。
- 顶部只显示协议（AirPlay 2 / DLNA / MiPlay），不显示 ALAC、AAC 等音质格式。
- 封面使用独立 `FloatingAlbumArt`：柔光阴影、细边缘光，无黑色厚底座；不裁切阴影。
- 620×320 矮横屏、420×760 纵向及 21:9 尺寸均无 RenderFlex overflow。
- `flutter build apk --debug`：**失败：No Android SDK found**；环境无 Android SDK，无法构建 APK。
- 未使用 `FittedBox.scaleDown`；无 620×320/420×760/21:9 的 RenderFlex overflow。
- 未做真机 Android/Android TV、刘海/挖孔 Insets、Profile 帧率、第三方库 Demo 实际运行；这些项目不标记通过。

## 6. 限制

1. 没有收到参考图，无法证明像素级一致。
2. Android 真机玻璃/性能未验证；Widget 测试中的 Skia fallback 不能代表 Impeller 效果。
3. 经确认，歌词就是歌词，不再追加中英双语展示；截图使用普通单语歌词。
4. 录制逐句变化视频需要设备/录屏链路，本轮只提供单句/多句截图与测试证据。
5. 随机播放/循环未添加，因为当前协议没有真实对应能力。
