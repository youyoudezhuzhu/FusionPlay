# FusionPlay 接手入口

先阅读 [跨平台维护与交接](docs/CROSS_PLATFORM_MAINTENANCE.md)。在 `integration/unified-platforms` 上维护应用代码，开始前核对工作区、远端分支及最新发布。

改共享协议或播放器时，沿 Android、Windows 和飞牛的宿主适配核查。飞牛有 x64 和 ARM64 两种 FPK。需要发布时使用一个 `vX.Y.Z` 标签构建四个安装包，随后验证 Android Pages 更新。报告中区分软件回归、安装包检查和实体设备结果。
