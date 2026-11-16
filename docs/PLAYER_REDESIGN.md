# FusionPlay 播放页 Apple Music 横屏重构

## 1. 源码审查与数据流

数据流没有改变：

```
AirPlay/MiPlay/DLNA/本地
  → Rust core / Kotlin host
  → AppState.playback (PlaybackSnapshot)
  → PlayerView
      ├─ _displayMetadata() → 封面左侧歌曲信息
      ├─ PlaybackControls → 前一首/播放/下一首/进度能力
      └─ LyricsView(lyrics, lyricsLoading, positionMs, trackKey)
```

涉及并保留的文件/能力：

| 现有能力 | 数据/控件 | 本次处理 |
| --- | --- | --- |
| 协议标识 | `playback.protocol` → `playbackProtocolLabel` | 顶栏 chip 保留；音质文本另起轻量 chip |
| 歌词显隐 | `_lyricsVisible`、`_lyricsToggleVisible` | 保留按钮、状态和切歌重置语义 |
| 歌词数据 | `PlaybackSnapshot.lyrics` / `lyricsLoading` / `LyricsDocument` / `LyricLine` / `LyricsWord` | 完全沿用；只换布局容器 |
| 同步歌词/逐字歌词/静态歌词 | `LyricsView` 现有时间轴、逐字高亮、手动滚动 | 不重写，只把 `LyricsView` 放入右侧玻璃面板 |
| AirPlay 标题歌词 | `_observeAirPlayTitle`、`_effectiveLyricsFor`、`_displayMetadata`、`_airPlayTitleCaptures`、`_airPlayTitleLyrics` | 原逻辑保留；布局改为右栏歌词，封面身份判断不变 |
| 播放/暂停 | `onPlayback(PlaybackCommand.playPause)`、`controls.playPause` | 保留 `_playVisible` 与能力判断 |
| 上一曲/下一曲 | `controls.previous` / `controls.next` | 不支持的发送端继续隐藏对应按钮 |
| 进度拖动 | `onSeek`、`controls.seek`、乐观拖动/回执 | 复用原进度条逻辑和 pending seek 处理 |
| 音量 | `onVolume`、乐观更新、静音恢复、拖动结束提交 | 复用 `_volumeControls`/`_commitVolume` |
| 更多菜单 | `onOpenMenu`、设置/检查更新/导出日志/关于 | 顶栏三点按钮保留；玻璃样式走同一 `LiquidGlass` |
| 遥控器/键盘焦点 | `RemoteFocusFrame`、`_progressFocus`、`_playPauseFocus` 等 | 保留全部焦点节点与方向键路径；横屏内联后仍可导航 |
| 封面 | `ArtworkCard`、`_displayedArtworkIdentity`、翻转方向 | 复用，AirPlay 元数据/歌词更新不改变身份 |
| 背景 | host 的 `ArtworkBackground` | 保留封面动态背景；无歌词时不再额外显示空白歌词列 |

## 2. 新横屏结构

### 有歌词 / 歌词加载中

```
Column
├─ 顶栏：协议 chip + 音质 chip + 歌词开关 + 更多菜单
└─ Row
   ├─ 左栏（约 43%）：封面、歌曲、作者、歌词类型 chip、
   │   进度 + 时间、上一曲/播放/下一曲、音量/静音
   └─ 右栏（约 57%）：LiquidGlass 歌词面板
        └─ 现有 LyricsView（同步/逐字/静态/加载骨架）
```

短屏（<360 高）左栏整体使用 `FittedBox.scaleDown`，在 800×360、620×320 下不裁切任何控件；不会删除播放/音量/进度功能。

### 无歌词 / 用户主动隐藏歌词

不渲染空白歌词列，也不再使用旧版整宽底部 dock。横屏改为**大封面 + 右侧歌曲/作者/进度/播放控制/音量/静音**的独立布局（`_appleNoLyricsBody`）；发送端未提供歌词的提示保留为次要一行。空态与无歌词共用这一布局，因此启动后也能看出播放页已重构。

### 加载中

`lyricsLoading == true` 时仍使用双栏，右栏由 `LyricsView` 渲染骨架；歌词到达后只是右栏局部内容替换，不改变 `showLyrics` key，不重置封面/播放状态。

## 3. Liquid Glass

`flutter/lib/ui/liquid_glass.dart` 保留 `GlassLevel` token 和稳定 fallback：

- 单层 `BackdropFilter` 模糊背景，避免叠加多层离屏。
- `CustomPaint` 绘制 tint/gradient、顶部镜面高光、边缘 lens band、底部 caustic、rim 与吸收阴影。
- `interactive` 只更新 `ValueNotifier`，空闲时不跑 ticker；`RepaintBoundary` 隔离。
- **未启用** `ImageFilter.shader` 真实屏幕空间折射：历史 Android Impeller 白帧/反馈问题（flutter#187820、#170820）；当前为高质量降级。没有把普通透明卡片冒充真实折射。

应用范围：歌词面板、播放/上一曲/下一曲、音量、歌词开关、菜单按钮、无歌词 dock。歌词正文没有逐行玻璃；封面没有厚玻璃覆盖。

## 4. 动画

- 有歌词/无歌词切换：现有 300ms `AnimatedSwitcher` fade+scale；`layoutBuilder` 继续避免旧空态与歌词同时出现。
- 封面：沿用 `ArtworkCard` 的 identity + 方向语义；AirPlay title 变化只更新歌词/信息，不切封面。
- 播放按钮/控件：`AnimatedPressScale`、`_ControlEntrance` 保留；降低动画模式走 `MediaQuery.disableAnimations`。
- 进度 250ms ticker 只影响左栏；右栏 `RepaintBoundary` 隔离。

## 5. 验收与测试

已覆盖/更新：

- 有歌词双栏、AirPlay title 歌词、无歌词居中、歌词加载中、隐藏/恢复歌词。
- 短横屏 620×320、800×360、960×540、1280×800。
- 焦点链、播放能力按钮显隐、音量乐观更新/提交、进度拖动。
- 左侧信息顺序改为横向：封面 → 标题/作者；歌词在右栏。

调试截图（测试生成，非正式播放数据）：

- `docs/player-redesign/lyrics-960x540.png`
- `docs/player-redesign/lyrics-800x360.png`
- `docs/player-redesign/no-lyrics-960x540.png`
- `docs/player-redesign/no-lyrics-800x360.png`
- `docs/player-redesign/tablet-1280x800.png`
- `docs/player-redesign/loading-960x540.png`

生成方式：`flutter test test/player_preview_test.dart`，输出到 `/tmp/fusionplay-preview`。

## 6. 未验证/限制

- 未在真实 Android TV/手机 GPU 上逐帧测 60 FPS；需在 800×360/960×540 设备用 DevTools 验证。
- `ImageFilter.shader` 折射仍关闭；若后续确认 Impeller 修复，可单独启用并 A/B。
- 大型歌词面板的 BackdropFilter 是主要 GPU 成本，必要时可在低端设备降级为无模糊深色玻璃。
- 未验证所有第三方发送端的歌词字段差异；AirPlay title 识别沿用 1.5.x 启发式，未在本次 UI 重构中改业务规则。
