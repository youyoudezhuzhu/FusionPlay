# FusionPlay PR #2 第二轮 UI 回归与液态玻璃验收

## 1. 分支与环境

- 仓库：https://github.com/youyoudezhuzhu/FusionPlay
- 分支：`ui/player-control-spacing-cover-20261118`
- HEAD：`e0d82b0`（`Use horizontal artwork and controls layout on short ultrawide lyrics screens`）
- 上一轮：`f3916a5`，报告 `fusionplay-ui-review/REVIEW.md`
- Flutter 3.32.8 / Dart 3.8.1
- 环境：Linux x86_64，`flutter_test`（VM，Skia）
- **未使用 Android 真机，未启用 Impeller**；玻璃动态折射无法确认
- 截图 DPR=2，PNG 物理尺寸 = 逻辑尺寸 ×2

## 2. 自动化测试

| 命令 | 退出码 | 结果 |
|---|---|---|
| `flutter pub get` | 0 | 成功 |
| `flutter analyze` | 1 | 21 条 info（全部来自 vendored `liquid_glass_widgets`），**无 error / 无 warning**；退出码 1 是 analyzer 对 info 的处理 |
| `flutter test` | 0 | **209 passed**，0 failed |
| `flutter test test/player_preview_test.dart test/lyrics_player_test.dart` | 0 | **36 passed**，0 failed |
| `flutter test test/ui_review_round2_test.dart` | 0 | **2 passed**，P01–P20 截图采集完成 |

- RenderFlex overflow：P01–P20 全部 `none`
- Shader 日志：`[LightweightGlass] ✓ Shader precached (native shared mode)`，说明 Skia 下 `liquid_glass_widgets` 使用 lightweight 降级路径
- 完整日志：`logs/flutter-analyze.log`、`logs/flutter-test.log`、`logs/required-tests.log`、`logs/capture.log`

## 3. 截图

### P01–P20

| 编号 | 分辨率 | 状态 | 截图 |
|---|---|---|---|
| P01 | 900×400 | 有歌词 | ![P01](screenshots/player/P01.png) |
| P02 | 900×400 | 无歌词 | ![P02](screenshots/player/P02.png) |
| P03 | 980×420 | 有歌词 | ![P03](screenshots/player/P03.png) |
| P04 | 980×420 | 无歌词 | ![P04](screenshots/player/P04.png) |
| P05 | 840×360 | 有歌词 | ![P05](screenshots/player/P05.png) |
| P06 | 840×360 | 无歌词 | ![P06](screenshots/player/P06.png) |
| P07 | 620×320 | 有歌词 | ![P07](screenshots/player/P07.png) |
| P08 | 620×320 | 无歌词 | ![P08](screenshots/player/P08.png) |
| P09 | 1280×800 | 有歌词 | ![P09](screenshots/player/P09.png) |
| P10 | 1280×800 | 无歌词 | ![P10](screenshots/player/P10.png) |
| P11 | 1920×1080 | 有歌词 | ![P11](screenshots/player/P11.png) |
| P12 | 1920×1080 | 无歌词 | ![P12](screenshots/player/P12.png) |
| P13 | 899×400 | 有歌词 | ![P13](screenshots/player/P13.png) |
| P14 | 900×349 | 有歌词 | ![P14](screenshots/player/P14.png) |
| P15 | 900×350 | 有歌词 | ![P15](screenshots/player/P15.png) |
| P16 | 900×460 | 有歌词 | ![P16](screenshots/player/P16.png) |
| P17 | 900×461 | 有歌词 | ![P17](screenshots/player/P17.png) |
| P18 | 980×420 | 超长歌名/歌手 | ![P18](screenshots/player/P18.png) |
| P19 | 980×420 | 无封面/无歌词 | ![P19](screenshots/player/P19.png) |
| P20 | 980×420 | 暂停/接近终点 | ![P20](screenshots/player/P20.png) |

### 玻璃与弹窗

| 编号 | 场景 | 截图 |
|---|---|---|
| G01 | 上一曲/下一曲 | ![G01](screenshots/glass/G01.png) |
| G02 | 音量胶囊 | ![G02](screenshots/glass/G02.png) |
| G03 | 顶部按钮 | ![G03](screenshots/glass/G03.png) |
| G04 | 菜单完全展开 | ![G04](screenshots/glass/G04.png) |
| G05 | 菜单展开中间帧 | ![G05](screenshots/glass/G05.png) |
| G06 | 设置面板 | ![G06](screenshots/glass/G06.png) |
| G07 | 设置滚动后 | ![G07](screenshots/glass/G07.png) |
| G08 | 设置内部卡片 | ![G08](screenshots/glass/G08.png) |
| G09 | 错误气泡 | ![G09](screenshots/glass/G09.png) |
| G10 | 关于弹窗 | ![G10](screenshots/glass/G10.png) |

真实存在但未截图的弹窗：更新检查/更新进度/确认类对话框（需要模拟更新状态）。项目内存在的弹窗类型：关于、更新、错误 toast、日志确认。

## 4. 布局测量

完整数据：`measurements.csv`。

| 编号 | 尺寸 | 关键测量 |
|---|---|---|
| P01 | 900x400 | 封面 150.7×150.7（高占比 37.7%），播放中心 Y=243.0，偏差=0.0px，进度-时间间距=-10.0px |
| P02 | 900x400 | 封面 273.7×273.7（高占比 68.4%），播放中心 Y=263.0，偏差=4.0px，进度-时间间距=-6.0px |
| P03 | 980x420 | 封面 159.1×159.1（高占比 37.9%），播放中心 Y=253.0，偏差=0.0px，进度-时间间距=-10.0px |
| P04 | 980x420 | 封面 288.9×288.9（高占比 68.8%），播放中心 Y=273.0，偏差=4.0px，进度-时间间距=-6.0px |
| P05 | 840x360 | 封面 90.0×90.0（高占比 25.0%），播放中心 Y=275.0，偏差=5.0px，进度-时间间距=-10.0px |
| P06 | 840x360 | 封面 230.5×230.5（高占比 64.0%），播放中心 Y=231.5，偏差=5.0px，进度-时间间距=-10.0px |
| P07 | 620x320 | 封面 64.0×64.0（高占比 20.0%），播放中心 Y=242.0，偏差=5.0px，进度-时间间距=-10.0px |
| P08 | 620x320 | 封面 198.3×198.3（高占比 62.0%），播放中心 Y=211.5，偏差=5.0px，进度-时间间距=-10.0px |
| P13 | 899x400 | 封面 150.5×150.5（高占比 37.6%），播放中心 Y=243.0，偏差=0.0px，进度-时间间距=-10.0px |
| P14 | 900x349 | 封面 79.0×79.0（高占比 22.6%），播放中心 Y=264.0，偏差=5.0px，进度-时间间距=-10.0px |
| P15 | 900x350 | 封面 80.1×80.1（高占比 22.9%），播放中心 Y=265.0，偏差=5.0px，进度-时间间距=-10.0px |
| P16 | 900x460 | 封面 158.3×158.3（高占比 34.4%），播放中心 Y=273.0，偏差=0.0px，进度-时间间距=-10.0px |
| P17 | 900x461 | 封面 158.4×158.4（高占比 34.4%），播放中心 Y=273.5，偏差=0.0px，进度-时间间距=-10.0px |
| P18 | 980x420 | 封面 159.5×159.5（高占比 38.0%），播放中心 Y=253.0，偏差=0.0px，进度-时间间距=-10.0px |
| P19 | 980x420 | 封面 288.9×288.9（高占比 68.8%），播放中心 Y=273.0，偏差=4.0px，进度-时间间距=-6.0px |
| P20 | 980x420 | 封面 159.1×159.1（高占比 37.9%），播放中心 Y=253.0，偏差=0.0px，进度-时间间距=-10.0px |

验收对照：
- 900×400 封面目标约 160px：实际 **150.7px**（未达标，差 9.3px）。
- 980×420 封面目标约 170–180px：实际 **159.1px**（未达标，差 11–21px）。
- 播放按钮居中偏差：P01/P03 = **0.0px**，达标（目标 ≤3px）。
- 进度条到时间文字：Widget 矩形间距 -10px（进度控件含 40px 热区 + time translate）；截图中可见间距约 10px，略高于建议 4–8px。
- 无纵横溢出，所有 P01–P20 `RenderFlex overflow = none`。
- 封面占屏高：P01 37.7%，P03 37.9%；无歌词 P02 68.4%，差异明显。

## 5. P0 模糊脏层专项

**结果：仍然存在，未修复。**

- 仅 **900×400 有歌词**复现（P01）；900×400 无歌词（P02）正常；980×420 有歌词（P03）正常；840×360 有歌词（P05）正常。
- 现象：左侧封面/标题/进度条/按钮被整块红橙色模糊层覆盖，右侧歌词正常；顶部 AirPlay 标签和歌词按钮呈白色糊块。
- 与宽度/高度组合相关：900×400 命中新横向布局条件（compact、panelHeight 350–460、panelWidth≥330）。
- 与 `GlassQuality.premium`、`useOwnLayer: true`、`GlassBackdropScope` 的纹理采样可能相关，但本轮只做对照观察，未修改代码验证因果。
- 980×420、840×360 不出现，说明不是全局必然。
- 完整图：`screenshots/player/P01.png`；正常对照：`screenshots/player/P02.png`、`P03.png`。

## 6. 液态玻璃验收

- 播放器：上一曲/下一曲/音量胶囊/顶部按钮在 G01–G03 中表现为半透明棕色玻璃，边缘细高光，**无明显背景线条折射偏移**。
- 设置：G06/G07 透出背景色，内部卡片为深色半透明，无强折射。
- 弹窗：G09/G10 与播放器材质基本统一，文字可读。
- 动态证明：`D02-grid-980x420.png` 高对比网格背景下未观察到清晰线条位移；无法证明实时折射。
- Impeller 对比：**未验证**（无 Android 设备）。
- 录屏：**未生成**（环境无录屏能力）。

## 7. 功能回归

在 Widget 测试层面验证：
- 播放/暂停、上一曲/下一曲、进度拖动、音量、歌词显隐、菜单开合、菜单→设置、焦点链、错误气泡、关于弹窗均有既有测试覆盖且本轮 `flutter test` 全绿。
- 真机触摸/遥控器/Android 返回键/更新流程：**未验证**（无设备/网络模拟）。

## 8. 性能

- 无真机 Profile 环境，帧时间、GPU、内存、Shader 编译卡顿、菜单/歌词连续 30 次：**未验证**。

## 9. 结论

- 已通过：自动化测试（209/36 全绿）、无 RenderFlex 溢出、播放按钮居中偏差 0px、封面较上一轮增大、菜单/设置/气泡材质统一。
- 未通过：900×400 有歌词 P0 模糊脏层仍存在；封面未达到目标尺寸；进度条到时间文字约 10px 略超建议。
- 未验证：Android Impeller 动态折射、真机功能、性能、录屏。
- **建议：不合并 PR #2**，先修复 P0 脏层并补齐真机玻璃验证。

详见 `issues.md` 与 `comparison.md`。
