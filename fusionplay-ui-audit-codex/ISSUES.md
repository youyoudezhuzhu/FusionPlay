# 问题和复核状态

| 级别 | 复现及根因 | 涉及位置 | 状态 |
|---|---|---|---|
| P0 | 980×420 预热后切换 900×400 有歌词。Skia lightweight 的 BackdropFilterLayer 可污染控件外，单独裁切该层可恢复正常。 | third_party/.../lightweight_liquid_glass.dart，player_view.dart | 原路径隔离实验完成；生产控件统一替换。像素断言守卫 P01。 |
| P1 | DPR 2 时 shader 再除以采集倍率，导致局部光学法线和 UV 错位。几何位置原先使用上一帧缓存。 | shaders/liquid_glass_refraction.frag，liquid_glass.dart | 使用绘制时逻辑坐标和仿射轴；网格测试通过。 |
| P1 | 封面异步解码、渐变和 resize 发生时 revision 可能不变，采集中收到的请求也会遗漏。 | glass_backdrop.dart | 按边界重绘失效，合并请求，保留尾帧；增加颜色变化回归。 |
| P1 | 实际页面用不透明 Scaffold 遮挡捕获的背景，路由使用另一张静态渐变。 | main.dart | 共用背景并移除第二次捕获。 |
| P1 | 固定高度窗口导致差 1 px 切换 Row 与 Column，封面受比例上限压缩。 | player_view.dart | 根据可用行宽和传输控件预算分配；两个原边界有几何断言。 |
| P1 | 时间与轨道偏远。 | player_view.dart | 紧凑布局上移时间 3 px，热区维持 40 px。 |
| P1 | 更新和关于的 GlassDialog 强制关闭折射。 | liquid_glass.dart，update_dialogs.dart | 共享 popup 材质，截图含真实更新确认、进度和错误路由。 |
| P2 | 旧测试吞下布局异常；P19 的 null 封面被 fixture 默认值替换；两个预览测试依赖外部 /tmp 封面。 | audit_capture_test.dart 等 | 截图测试现在验证异常、使用仓库 fixture，并覆盖真实缺图。 |
| 验收阻塞 | 无连接 Android 设备。 | Android 手机、平板、TV | Impeller 的动态折射、硬件输入、性能、温升及长时稳定性待实测。 |
