# 问题清单（第二轮）

## P0

### P0-1 900×400 有歌词模糊脏层仍存在
- 严重程度：P0
- 截图：`screenshots/player/P01.png`；正常对照 `P02.png`、`P03.png`
- 复现：900×400，有歌词，播放中；同状态无歌词正常，980×420/840×360 有歌词正常
- 可能位置：`flutter/lib/ui/player_view.dart` 新增横向布局分支（`player-controls-column` Row）+ 玻璃 `useOwnLayer/premium` + `GlassBackdropScope` 采样
- 建议：禁用/替换该尺寸下的横向分支做对照；检查 backdrop 纹理坐标/尺寸；为不支持 Impeller 的设备显式降级

### P0-2 Android Impeller 动态折射未验证
- 严重程度：P0（验收阻塞）
- 截图：无（环境限制）
- 说明：Skia 日志显示 lightweight shader 降级，premium 未实际执行
- 建议：真机 Profile 下重跑 G01/G02/网格对比并录屏

## P1

### P1-1 高度 349↔350、460↔461 布局突然切换
- 截图：`P14.png`（纵向）vs `P15.png`（横向）；`P16.png`（横向）vs `P17.png`（纵向）
- 位置：`player_view.dart` 横向条件 `panelHeight >= 350 && panelHeight <= 460 && panelWidth >= 330`
- 影响：1px 高度变化导致封面/控件位置大幅跳变
- 建议：改为平滑过渡或统一布局，减少硬阈值

### P1-2 封面未达目标尺寸
- P01 实际 150.7px（目标约 160）；P03 实际 159.1px（目标 170–180）
- 位置：横向分支 `artworkSize = min(innerWidth*.46, min(panelHeight*.43, 190))`
- 建议：提高上限并重新分配右侧控制区宽度/间距，避免溢出

### P1-3 进度条到时间文字约 10px
- 目标 4–8px；Widget 矩形 -10px（含热区），视觉约 10px
- 位置：`_liquidTimeline` 的 `Transform.translate(offset: Offset(0, compact ? -10 : -8))`
- 建议：按实际可见轨道位置校准，保留触摸热区

### P1-4 顶部 AirPlay 标签与歌词按钮发虚/白色糊块
- 截图：P03/P05/P14 顶部
- 位置：`_liquid21TopBar` 玻璃按钮 + 背景采样层
- 建议：排查玻璃层是否覆盖到非玻璃控件

### P1-5 玻璃仍偏平/偏棕，缺少目标图轻薄透明感
- 截图：G01/G02
- 位置：`player_view.dart` `GlassContainer` 参数；PR2 premium 在 Skia 无效
- 建议：真机 Impeller 验证后再调参数

## P2

- P2-1 未录屏（环境无录屏能力）
- P2-2 更新类弹窗未截图
- P2-3 真机性能/遥控器/返回键未验证
- P2-4 P19 无封面时仍渲染方形占位图（按设计可能是预期；需确认）
