# Android 播放页（YAQMC 风格 + 液态玻璃）

播放页参考 YAQMC 的音乐播放界面重做：左侧大封面与曲目信息，右侧常驻大号歌词，
底部悬浮播放控制栏；设置从右上角三点菜单以液态玻璃 morph 成右侧半屏面板。

## 布局

- **顶部**：左上保留音频来源胶囊（AirPlay 2 / 小米妙播 / DLNA），不显示音质/码率；右上三点玻璃按钮（遥控器可聚焦）。
- **左列**（约 36% 宽，210–380px）：大圆角封面卡（24–26 圆角、阴影、播放中轻微呼吸 1.008），
  下方标题/艺人/歌词状态 chip。
- **右列**：有歌词时显示歌词区，当前行高亮、非当前行降透明度、上下渐隐、自动居中滚动、
  手动滚动后暂停并显示「回到当前」；逐字歌词沿用卡拉 OK 填色。
- **无歌词时**（AirPlay 永不提供 / DLNA 或妙播本次没有 / 用户手动隐藏）：不再保留右侧空白，
  改为单栏居中布局——大封面居中、标题/艺人居中，按渠道显示一行弱化说明
  （AirPlay 2 不提供歌词 / 此发送端未提供歌词 / 此歌曲没有歌词 / 暂无歌词）。
- 有歌词时右上角提供「歌词」切换按钮（默认显示；换曲/换来源后恢复显示；同曲内可手动隐藏），
  隐藏后同样进入封面居中布局并保留播放控制。
- **底部悬浮 dock**：进度条 + 时间；上一曲/播放/下一曲；右侧音量图标 + 滑杆（0–100，静音切换）。
- **TV / 电视盒**：Kotlin 通过 PackageManager（LEANBACK/TELEVISION）或 uiMode 检测电视，
  状态 map 下发 `isTelevision`；电视上使用 28/14px overscan 安全边距、4px 焦点框 + 焦点放大
  1.02，设置面板宽度上限提高到 `min(720, 屏宽/2)`；设置打开时默认焦点在 MiPlay 行（避免
  遥控器确定键直接弹软键盘），名称输入框仍是手机/平板的默认焦点。
- **手机横屏适配**：高度 <430px 时启用紧凑 chrome——dock 内边距/按钮/音量滑杆缩小、顶部留白收紧；
  无歌词居中布局按剩余高度动态决定封面尺寸（空间不足时退化为小图标），并保证标题/渠道提示
  完整显示在 dock 上方，不再被裁切或重叠。
- **紧凑/矮屏**：<260px 内容高度进入 dense 模式（隐藏艺人/chip、单行标题），620x320/800x360 不溢出。

## 交互与能力 gating

- 每帧数据来自 `stateMap().controls`：
  - `playPause`：本地可播放即显示（AirPlay/DLNA 可在接收端暂停/恢复）；
  - `previous` / `next`：仅当来源支持反向控制（妙播 reverse control 的
    `previous_track`/`next_track`，或 AirPlay/DLNA 发送端声明的 remote commands）；
  - `seek`：时长已知且可 seek。
- 不支持的控制不渲染，避免 D-pad 死焦点。
- 三点菜单：设置、检查更新（仅 Android）、导出日志、关于（版本/协议/设备名）。
  菜单从三点按钮的圆形 **morph** 成圆角方形玻璃面板（Rect/圆角插值，约 260ms），
  不再是简单淡入；**菜单保持按钮的右上角为锚点**（右/上边缘不移动，只向左/下展开），
  圆角半径等于按钮圆的半径，因此右上角的弧度与圆一致。
  打开菜单/设置时原三点圆形按钮淡出 + 缩到 0.86 并禁用交互/焦点，
  关闭动画结束后才淡入回来，视觉上圆被菜单“吸收”。点击外部/Escape/返回关闭，焦点回到三点按钮。
- 设置：从菜单矩形继续 **morph** 成右侧玻璃面板（约 320ms），宽 `min(560, 屏宽/2)`
  （电视 `min(720, 屏宽/2)`），上下右 16px 边距，严格不超过右半屏。
  外层是深色毛玻璃（唯一真实 BackdropFilter），内部为深色半透明玻璃表面：
  分组卡片 + 细白边、白色文字层级、单色玻璃开关（白底深色滑块）、玻璃胶囊按钮、选中设备行白色高亮 + 白色勾选；
  不再使用白色实心卡片。设置面板使用 Flutter 官方 `PinnedHeaderSliver` +
  `CustomScrollView`：标题栏钉住，内容真正从标题栏下方滚过（不再在 body 顶部裁切后
  盖遮罩）；标题栏背景从不透明深色渐变到透明，底部没有 1px 硬分割线。
  分层关闭：设置 → 菜单 → 三点按钮；打开时焦点在顶部「自定义名称」，关闭后逐层归还。
- reduce motion 时所有 morph 直接到位；玻璃容器全程静态（无 shader ticker）。

## 电视启动入口

AndroidManifest 增加了 `LEANBACK_LAUNCHER` activity-alias 与 320×180 TV banner
（`drawable-nodpi/tv_banner.png`），并声明 `touchscreen/faketouch/leanback` 均非必需，
同一 APK 可被 Android TV 桌面收录；遥控器方向键/确定键/返回键走常规焦点链。

## 液态玻璃实现

Flutter 官方目前没有提供 Liquid Glass 原生 API（上游 issue
[flutter#170310](https://github.com/flutter/flutter/issues/170310) 仍 open）；社区包
（liquid_glass_widgets / liquid_glass_renderer 等）要求 Flutter 3.41+，本项目固定在
Flutter 3.32.8。因此这里自研实现，参考了以下开源项目的做法：

- [YAQMC](https://github.com/YAQMC/YAQMC)：播放页信息层级、歌词排版、ActionMenu 锚定逻辑；
- [FlClash](https://github.com/chen08209/FlClash)：玻璃面板的模糊/动效组织；
- [flutter_liquid_glass](https://github.com/colbymaloy/flutter_liquid_glass) 与
  [liquid_glass_widgets](https://github.com/sdegenaar/liquid_glass_widgets)：镜片折射、
  色散、Beer-Lambert 边缘吸收、Fresnel/specular 高光等 shader 思路（MIT）。

实现位置：`flutter/lib/ui/liquid_glass.dart` + `flutter/shaders/liquid_glass.frag`。

- 默认使用 `BackdropFilter` 毛玻璃 + tint/渐变/描边/高光叠层（Windows、测试与 Android 均走这条稳定路径）。
- 实验性的 `ImageFilter.shader` 折射路径默认**关闭**：Flutter 3.32 在多个 `BackdropFilter` 玻璃层
  重叠时会把上一帧（含自身输出）反馈进下一帧，任何动画触发重绘都会让玻璃闪白色底
 （flutter/flutter#187820、#170820）。需要实验时用
  `--dart-define=FUSIONPLAY_GLASS_SHADER=true` 显式开启。
- 组件会对 `MediaQuery.disableAnimations` 关闭时间动画；
- 可调参数：`borderRadius`、`blurSigma`、`tint`、`refraction`、`specular`、`thickness`、
  `absorption`、`animate`。

使用位置：三点按钮、三点菜单、设置浮层、底部 dock；主内容避免大面积玻璃，保持 YAQMC 的
深色 + 封面取色风格。

## 已知限制

- 液态玻璃真实折射只能在 Android 真机（Impeller）目测，单测/桌面走 fallback。
- 设置浮层背后是半透明压暗而非整屏 BackdropFilter 模糊（性能考虑）。
- 菜单锚点固定右上，不做视口翻转/夹取。
- 大歌词字号上限 32px，超宽屏右侧留白略多。
