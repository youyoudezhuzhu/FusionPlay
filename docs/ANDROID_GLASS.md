# FusionPlay Android Liquid Glass 视觉系统

本文件记录 Android Flutter 端的液态玻璃材质、统一组件、性能取舍与已知限制。
对应的 Web/fnOS 管理界面尚未使用这套实现。

## 1. 旧实现与问题

- 各页面分别写死 `LiquidGlass` 的 tint/blur/圆角，控件与弹窗材质不一致。
- 玻璃只有模糊 + 半透明色，高光固定、边缘缺少厚度，观感偏“半透明塑料”。
- 弹窗仍是白色 `AlertDialog`，与播放页/设置页的深色玻璃割裂。
- 曾尝试 `ImageFilter.shader` 做透镜折射；Flutter 3.32 在多个玻璃层重叠、动画触发重绘时
  会把上一帧（含自身输出）反馈进下一帧，整块玻璃闪白色底
  （[flutter/flutter#187820](https://github.com/flutter/flutter/issues/187820)、
  [#170820](https://github.com/flutter/flutter/issues/170820)）。
- 设置页顶栏用不透明深色渐变模拟遮罩，看起来像一条黑带；内容还在 body 顶部被裁切。
- 设置页打开时自动聚焦名称输入框，导致软键盘自动弹出。

## 2. 新玻璃系统

实现位置：`flutter/lib/ui/liquid_glass.dart`。

### 材质分级

| Level | 用途 | 视觉 |
|---|---|---|
| `GlassLevel.card` | 基础内容卡片/小组件 | 浅 tint、柔和高光、轻阴影 |
| `GlassLevel.control` | 按钮、开关、播放控制、三点按钮 | 较强 rim/sheen、可按压/指针响应 |
| `GlassLevel.panel` | 菜单、设置面板、弹窗 | 深色 tint、更强边缘/高光/阴影 |

`GlassSpec` 统一管理 tint、blur、圆角、边框透明度、rim、edge、sheen、shadow；页面不再各自
写死玻璃颜色。旧参数（`borderRadius`、`blurSigma`、`tint`、`tintGradient`）保持兼容。

### 渲染与光学

- 使用单个 `BackdropFilter` 高斯模糊做实时背景采样；上面叠加 tint/渐变、内侧 rim 高光
  （左上亮、右下暗）、细边框与克制阴影。
- 动态高光：`Listener`/`MouseRegion` 读取指针/按压位置，通过 `ValueNotifier` 驱动
  `CustomPaint` 的径向高光；不启动 ticker、不每帧 `setState`，空闲/动画期间无额外 GPU 工作。
- 聚焦状态经 `FusionFocusHighlight` 提升 rim/sheen；菜单/面板形变是几何 rect/radius 变化，
  高光随形状裁剪。
- `GlassSurface` 用于“外层已经是玻璃”的内部卡片：只画 token 表面、不叠加第二层模糊。
- `GlassDialog`/`showGlassDialog` 统一深色玻璃弹窗，入场仅 opacity + scale，reduce motion 直接显示。

### 明确未实现：真实屏幕空间折射

当前 Flutter/Impeller 组合下，shader 背板滤波（`ImageFilter.shader`）在多层玻璃叠加时会出现
白色反馈，已为稳定性关闭。因此：

- 已实现：实时背景模糊采样、动态指针高光、形状相关 rim/edge、统一材质分级。
- 未实现：根据曲率/法线偏移 UV 的真实几何折射，以及动态色散。
- 不把 `BackdropFilter` 模糊描述为“折射”。待上游修复或改用单层 shader 架构后再评估。

## 3. 组件迁移清单

- 播放页：三点按钮、歌词按钮、底部 dock、上一曲/播放/下一曲控件。
- 菜单/设置：morph 容器、设置面板、设置内部半透明卡片/输入/开关。
- 弹窗：关于、检查更新全部状态（检查中、最新、发现新版本、下载/安装、失败/权限）。
- `RemoteFocusFrame` 聚焦高光联动。
- 无其他 `BackdropFilter`；普通列表/文本不单独加折射层。

## 4. 设置页顶栏与键盘

### 顶栏

`FusionSettingsSheet` 结构：

```
ClipRect(Stack[
  ShaderMask(顶部渐隐, CustomScrollView[
    PinnedHeaderSliver(仅占位 SizedBox(headerHeight)),
    SliverList(设置内容),
  ]),
  Positioned(顶层玻璃 header：标题 + 关闭按钮),
])
```

- 内容真正从固定 header 下方滚过，不再在 body 顶部裁切后盖深色渐变。
- header 只用半透明白色高光渐变（无黑色实色带、无 1px 分隔线），标题/关闭按钮在遮罩之上。
- 内容进入 header 区域时由顶部 ShaderMask 渐隐，滚动停止后保持稳定。

### 键盘

- 进入设置不再自动聚焦名称输入框（TV/手机均不弹键盘；默认焦点在 MiPlay）。
- 名称默认是只读展示 + 编辑按钮；点击后才进入编辑态、聚焦并弹键盘。
- 取消恢复原名且不回调；应用原子调用一次 `onReceiverName`，成功后回展示态并收起键盘。
- 恢复默认只清空编辑文本，仍需应用才持久化。

## 5. 动画与性能

- 玻璃完全静态：无 shader ticker；指针高光只在事件时更新。
- morph overlay 不再用 controller `addListener(setState)` 重建整棵树，改为局部
  `AnimatedBuilder`；设置子树按同一 widget 实例传入；玻璃/菜单/面板加 `RepaintBoundary`。
- 动画只使用 transform/opacity；保留 reduce motion 行为。
- 已知动画测试只验证不抛异常与布局，真实设备帧率仍需按验收清单实测。

## 6. 验收状态

| 项目 | 状态 |
|---|---|
| 统一材质 token / 分级 | 已实现，Flutter tests 覆盖 |
| 指针/按压动态高光 | 已实现，单测覆盖构建与无 ticker |
| 实时背景模糊 | 已实现（BackdropFilter） |
| 真实屏幕空间折射 | 因 Flutter/Impeller 问题未实现，保持关闭 |
| 播放页/菜单/设置玻璃统一 | 已实现，widget tests |
| 关于弹窗玻璃化 | 已实现，tests |
| 检查更新全部状态玻璃化 | 已实现，tests |
| 设置顶栏无黑带 + 内容渐隐 | 已实现，结构/style tests |
| 进入设置不弹键盘 | 已实现，焦点 tests |
| 编辑名称取消/应用 | 已实现，tests |
| 真机帧率/GPU 数据 | 尚未验证 |

## 7. 已知限制

- 真实折射/色散未实现（原因见上）。
- TV 字号未随观看距离放大；overscan 为固定 28/14。
- 高光为指针/按压驱动，不随环境光自动变化。
- 检查更新没有持久 “有新版本” accent 状态，仍走弹窗流程。
- 弹窗内容高度受屏幕限制，极小屏需要内部滚动。
