# Liquid Player → FusionPlay 播放页移植报告

## 1. 上游审查

上游：<https://github.com/OnlyHouska/liquid-player>

- 技术栈：Electron + React 19 + TypeScript + Vite + `motion`；主进程通过 MPRIS2/Spotify 读取播放状态。
- 播放页结构（`src/App.tsx`）：
  - `TitleBar` → `stage`（`stage__head` = Artwork + TrackMeta；歌词模式为紧凑封面 + 左对齐歌词；`stage__transport` = Timeline + Controls）→ `SecondaryBar`（volume/output/queue/lyrics/menu）→ QueuePanel。
  - `AmbientBackground` 从封面取色，背景为大幅模糊/漂移的封面色。
  - 普通模式封面居中、元数据居中、封面下方是时间轴和控制；歌词模式封面缩到 84px、信息在 top-left，歌词占据 860px 宽列，进度和控制下移。
  - CSS token（`src/ui/styles/tokens.css`）：`--r-art:24px`、`--r-pill:24px`、`--glass-blur:40px`、`--art-size:min(clamp(240px,44vh,500px),40vw)`、`--ease-out:cubic-bezier(.16,1,.3,1)`；玻璃是 `backdrop-filter: blur(40px) saturate(180%)` + 1px 边缘高光，不是自研 shader 折射。
- **许可证审查（关键限制）**：
  - 仓库**没有 LICENSE 文件**；
  - `package.json` 为 `"private": true`，也未声明 `license`；
  - 因此没有公开授权可以复制其源代码、CSS 或素材。本次**没有复制上游代码/图片**；只依据公开截图和 CSS 视觉参数，用 Flutter 重新实现等效布局。若需要逐行复用源码，必须先取得作者书面许可。
- 未在本机实际启动 Electron 原版（无桌面显示环境）；视觉基准使用仓库 `docs/now-playing.png`、`docs/lyrics.png`、`docs/no-clutter.png`。

## 2. 移植后的播放器

改动主要在 `flutter/lib/ui/player_view.dart`，复用 FusionPlay 现有状态、控制、焦点和菜单逻辑。

### 普通模式

- 背景：沿用 host 的 `ArtworkBackground`（封面派生模糊背景）+ 深色可读性渐变。
- 封面居中，尺寸随可用高度/宽度缩放；元数据居中：来源/协议（大写小字）→ 标题 → 歌手 → 专辑。
- 细时间轴（当前时间 / 负剩余时间）位于控制上方；上一曲、白色圆形播放/暂停、下一曲居中。
- 底部 `SecondaryBar`：音量/静音、歌词开关、更多菜单；无首页之外的功能假按钮。
- 无歌词时同一布局显示渠道提示，不出现空白歌词框。

### 歌词模式

- 顶部：84/92px 紧凑封面 + 左侧来源/标题/歌手。
- 下方：现有 `LyricsView`（同步、逐字、静态、AirPlay 标题歌词全部保留），当前行高亮，前后行降低透明度。
- 时间轴与上一曲/播放/下一曲下移；底部 secondary bar 保留歌词开关和菜单。

### 保留的业务能力

- AirPlay/MiPlay/DLNA 状态与协议/音质信息。
- 封面、标题、歌手、专辑、进度、时长、音量、静音。
- `controls.playPause/previous/next/seek` 能力判断；不支持时不生成按钮。
- 歌词全部模式、AirPlay 标题歌词、歌词显隐、切歌重置。
- 更多菜单、设置/检查更新/导出日志/关于回调。
- 方向键/遥控器焦点链、reduce motion、错误 toast。

### 删除/清理

- `player_layout.dart` 及其测试已删除。
- 播放页不再使用旧 `LiquidGlass`/`_GlassPaint` 外观、旧方向双栏布局、旧底部 dock 代码；旧 landscape/portrait/centered/dock 方法已从 `player_view.dart` 删除。
- 播放器新布局不依赖自研 shader；secondary bar 使用 CSS 等价的 `BackdropFilter(40px)` + 1px 低透明度边线，避免过度白色高光。
- `liquid_glass.dart` 仍保留给设置/菜单/弹窗（本次不越界改动其他页面）。

## 3. 截图

- `docs/liquid-player-port/lyrics-960x540.png`
- `docs/liquid-player-port/no-lyrics-960x540.png`
- `docs/liquid-player-port/lyrics-800x360.png`
- `docs/liquid-player-port/no-lyrics-800x360.png`
- `docs/liquid-player-port/tablet-1280x800.png`
- `docs/liquid-player-port/loading-960x540.png`

截图由 Flutter widget test 的 `RepaintBoundary.toImage` 生成，使用演示数据；不是 Android 真机截图。

## 4. 验收结果

已执行：

- `flutter analyze`：无 error（仅 1 条 null-aware 风格 info）。
- `flutter test test/lyrics_player_test.dart`：**31 passed**。
- 布局覆盖 960×540、800×360、620×320、1280×800、420×760。
- AirPlay/MiPlay/DLNA 无歌词提示、控制能力显隐、音量乐观提交、焦点链、歌词加载/切歌/显隐测试均保留。

**未执行（按“未明确要求不编译”的当前约束）**：

- Android APK 构建、真机/Android TV 遥控器实机验证、Profile 帧率/玻璃渲染性能。
- 上游 Electron 原版实际运行截图（本机无显示环境）。
- 逐像素视觉差异比对（上游没有 LICENSE，不能把其截图/素材复制进仓库作为 golden）。

## 5. 无法完全还原/限制

- 上游有 shuffle、repeat、queue、输出设备、Spotify 源等 FusionPlay 不具备的能力；本次**没有添加对应按钮**。
- 上游 secondary bar 的 System Output/Queue/Player 信息未移植（无对应真实能力）。
- 字体：上游用 Inter；FusionPlay 仍用 MiSans。
- 封面取色：上游由 JS palette 生成多组 HSL；Flutter 沿用现有封面背景，未复制其取色算法。
- 玻璃：上游是 CSS `backdrop-filter` 毛玻璃，不是真实折射；为忠实原版，播放器没有继续叠加自研折射高光。
- 横屏高度很矮（如 800×360）时歌词可视行数少于原版，这是可用高度差异，不是布局缩放。
- 许可证：无 LICENSE，不能直接复制上游代码；本报告的实现是独立 Flutter 重写。

## 6. 结论

已把 Liquid Player 的**单列居中播放页 + 歌词模式 + 底部 secondary bar**视觉结构移植到 FusionPlay，并保留 FusionPlay 的真实功能、控制能力、歌词和焦点逻辑。由于没有上游 LICENSE，本次是“按视觉行为重写”，不是源码级复用；Android 构建/真机验收未在当前约束下执行。
