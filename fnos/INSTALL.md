# FusionPlay fnOS 安装与验收指南

面向最终用户/测试同学。安装包：`fusionplay-fnos.fpk`（appname `fusionplay-fnos`，当前 `0.2.5`，x86_64）。
构建、CI 与本地沙箱测试见 [fnos-build.md](fnos-build.md)；功能与限制见 [README.md](README.md)。

## 0. 前置条件

| 项 | 要求 |
|---|---|
| NAS | 飞牛 fnOS，**x86_64** 机型（当前包 `platform = x86`，ARM 设备装不了） |
| 权限 | fnOS 管理员账号（安装、授权 audio 组需要） |
| 存储 | 至少一个可用存储空间（安装时需要选） |
| 端口 | `18080` 未被占用（本应用只绑 `127.0.0.1`） |
| 声卡 | 至少一块可用的 ALSA 播放设备（板载/HDMI/USB DAC） |

下载产物：GitHub Actions artifact `fusionplay-fnos-fpk`，或 Release 页面的
`fusionplay-fnos.fpk` 附件。下载后建议先校验：

```bash
sha256sum fusionplay-fnos.fpk
tar tzf fusionplay-fnos.fpk | head        # 应看到 manifest / cmd/ / config/ / ICON*.PNG / app.tgz
```

## 1. 方式 A：应用中心安装（推荐）

1. 打开 fnOS 桌面 → **应用中心** → **手动安装**。
2. 弹窗顶部会显示已选中的存储空间（**默认已选中，不要再点它**，展开下拉会遮挡上传区）。
3. 点「从电脑上传」，选择 `fusionplay-fnos.fpk`。
4. 解析完成后会弹出「未经验证应用的安全提示」，按钮是 **取消 / 同意**（不是「安装」）→ 点 **同意**。
5. 等待安装完成；应用图标会出现在桌面。

## 2. 方式 B：CLI 安装

本应用**没有向导**、**没有 `install_dep_apps`**，所以 CLI 可以直接安装：

```bash
# 1) 把 fpk 传到 NAS 上用户可见目录（示例 /vol1/1000/workspace）
trim-cli file upload /vol1/1000/workspace ./fusionplay-fnos.fpk --overwrite rename

# 2) 先 dry-run 确认解析
trim-cli app install-fpk --remote-path /vol1/1000/workspace/fusionplay-fnos.fpk \
    --volume-id 1 --dry-run --yes

# 3) 正式安装
trim-cli app install-fpk --remote-path /vol1/1000/workspace/fusionplay-fnos.fpk \
    --volume-id 1 --yes

# 4) 查状态（app list 可能是缓存，以 status 的 version 为准）
trim-cli app status fusionplay-fnos
```

> ⚠️ 上传重名文件时 trim-cli 会自动改名（`x.fpk` → `x_1.fpk`）。装机前用
> `tar xzOf fusionplay-fnos.fpk manifest | grep version` 确认版本，别装错文件。
> ⚠️ 同 appname + 同 version 重装会报 **errno 10236**：先卸载，或使用更高版本。

## 3. 安装后必做：授权音频权限（audio 组）

应用以**专用用户 + 非 root** 运行（`run-as: package`），`config/privilege` 里声明了
`join-groups: ["audio"]`。fnOS 的安全模型是「声明只是申请，授权由用户完成」：

1. 设置 → **应用管理** → **FusionPlay** → **权限设置**。
2. 勾选包含 **audio / 音频设备** 的权限项，保存。
3. 在应用中心里**重启一次** FusionPlay。

未授权的表现：应用能启动，但「声卡」页没有设备、播放无声；日志出现
`ALSA 后端启动失败 … no soundcards found` 或 `PermissionDenied`。
这是权限问题，**不是安装包损坏**。

验证（SSH，需要 root）：

```bash
# 应用用户是否已在 audio 组（groups 输出里应有 audio）
sudo -u fusionplay-fnos id

# /dev/snd 是否可读
sudo -u fusionplay-fnos test -r /dev/snd/controlC0 && echo "audio ok" || echo "audio 未授权/无设备"
```

## 4. 安装后基础验收（5 分钟）

### 4.1 应用状态与服务

```bash
trim-cli app status fusionplay-fnos          # status: running，version = 0.2.5
sudo ss -lntp | grep 18080                   # 127.0.0.1:18080 有监听
pgrep -af 'fusionplay-fnos/bin/fusionplay-fnos'   # 只有 1 个进程
```

### 4.2 健康检查（版本一眼可查）

```bash
curl -s http://127.0.0.1:18080/healthz
# 期望：{"status":"ok","version":"0.2.5","uptime":...}

# 走网关 socket（应用实际接法；路径经 /var/apps/.../target 软链）
curl -s --unix-socket /var/apps/fusionplay-fnos/target/fusionplay-fnos.sock \
     http://localhost/app/fusionplay-fnos/healthz

curl -s -o /dev/null -w '%{http_code}\n' \
     --unix-socket /var/apps/fusionplay-fnos/target/fusionplay-fnos.sock \
     http://localhost/app/fusionplay-fnos/      # 期望 200
```

### 4.3 mDNS 发现（AirPlay/DLNA 能否被搜到）

```bash
sudo avahi-browse -rt _airplay._tcp | head    # 应能看到 FusionPlay
sudo avahi-browse -rt _raop._tcp   | head
```

不可见时依次检查：应用是否 running；audio 权限是否已授权；**环境里有没有 `CI`**
（上游 shairplay 看到 `CI` 会跳过 mDNS 注册；本包 `cmd/main` 已 `unset CI`）；
系统 avahi/mDNS 服务是否运行。真机已验证 `_airplay._tcp` + `_raop._tcp` 可被发现。

### 4.4 桌面入口 / Web UI

- 桌面点击 FusionPlay 图标，应打开中文管理面板（**概览 / 音频 / 接收 / 设置** 四页）。
- 浏览器直接访问网关路径 `http://<NAS>:5666/app/fusionplay-fnos/`（需登录飞牛），应返回 200，
  页面资源（`js/main.js`、`js/page-audio.js`、`style.css`）不 404。
- 主题按钮应在「跟随系统 → 浅色 → 深色」间切换，所有卡片/输入/曲线同步换色。
- 远程访问请认准桌面图标或 `/app/fusionplay-fnos/`；**不要**把 `http://<NAS>:18080` 当对外入口
  （端口只绑 loopback）。

### 4.5 日志

```bash
sudo tail -n 100 /vol1/@appdata/fusionplay-fnos/fusionplay.log
sudo tail -n 100 /vol1/@appdata/fusionplay-fnos/app.log
```

期望：有 `FusionPlay fnOS 0.2.5 启动`、`HTTP 已启动`、`协议宿主已启动`；
没有反复重启/崩溃堆栈。日志超过 5 MiB 会原地保留尾部 1 MiB（inode 不变）。

### 4.6 数据与配置目录

```bash
sudo ls -l /vol1/@apphome/fusionplay-fnos/config.json   # 首次启动后自动生成
sudo ls -l /vol1/@appdata/fusionplay-fnos/              # app.log、fusionplay.log、app.pid
```

## 5. 真机功能验收清单

> 这些项目**必须在真实 fnOS + 真实网络 + 真实发送端上验证**，CI/本地沙箱覆盖不到。
> 建议逐项记录「通过/失败 + 现象 + 日志时间点」。

### 5.1 AirPlay 2（需要 iPhone/iPad/Mac）

- [ ] `sudo avahi-browse -rt _airplay._tcp` 能看到 `FusionPlay`（mDNS 广播正常）。
- [ ] 控制中心音频输出列表里能看到 FusionPlay（默认名 `FusionPlay`，可在 UI 改）。
- [ ] 首次连接可完成配对（临时配对，无 PIN）；再次连接无需重复配对。
- [ ] 播放音乐：NAS 对应声卡出声；UI 概览显示当前音源 = AirPlay、曲名/歌手/封面。
- [ ] 播放/暂停/切歌：NAS 端暂停与恢复、切歌跟随；UI 状态同步。
- [ ] 发送端音量：拖手机音量条，NAS 输出响度随之变化（UI 显示 airplay_db）。
- [ ] 断开 AirPlay 后，UI 返回空闲；再次投放可正常接管。
- [ ] 异常：手机中途锁屏/切后台，音频不断流、不重复。

### 5.2 DLNA（需要 DLNA 控制端 App，如 BubbleUPnP）

- [ ] 控制端能发现 `FusionPlay`（MediaRenderer）。
- [ ] 播放 MP3 / FLAC / WAV / AAC：出声；进度/暂停/恢复/停止正常。
- [ ] 尝试 HLS / 视频：UI/日志应**明确报不支持**，不伪造成功。

### 5.3 小米妙播 MiPlay（需要 HyperOS 设备）

- [ ] 妙播/投音界面能发现 FusionPlay。
- [ ] 播放/暂停/切歌/音量随发送端变化。
- [ ] 老发送端（旧版固定密钥）可用（兼容默认开启）。
- [ ] 厂商准入失败时：日志写明原因，UI 不显示虚假成功。

### 5.4 多声卡与逐设备校准

- [ ] 至少两块输出设备时，UI「音频」页都能看到并启用；同时出声（如 HDMI + USB DAC）。
- [ ] 点击不同设备卡片，右侧显示该设备自己的设置（名称/响度/延迟/曲线互不串扰）。
- [ ] 给不同设备选择不同预设/自定义曲线：听感/输出幅度应不同；重启后各自的曲线保留。
- [ ] 拖动每设备增益（dB）与延迟（ms）能听到变化；重启后设置保留（config.json）。
- [ ] 全局衰减与全局静音对所有设备同时生效，但不改变手机端显示的主音量。
- [ ] 播放中拔掉一块声卡：其它设备不中断；重新插上后在 UI 刷新设备可恢复。
- [ ] 单声卡场景同样可用（与多声卡同一路径）。

### 5.5 稳定性 / 资源

- [ ] 连续播放 ≥ 2 小时无崩溃、无内存持续上涨。
- [ ] `fusionplay.log` 超 5 MiB 后仍在增长且 inode 不变（原地轮转）。
- [ ] 浏览器挂 Web UI 一段时间（SSE/轮询）无卡死。

### 5.6 重启 / 开机自启

- [ ] 重启 NAS 后应用自动启动（应用中心 status=running，`/healthz` 通）。
- [ ] `config.json` 中的设备/校准设置仍在。

## 6. 升级验收

1. 记录升级前版本：`curl -s http://127.0.0.1:18080/healthz`。
2. 应用中心上传**更高版本**的 fpk 并升级（同版本会 errno 10236）。
3. 升级完成后：
   ```bash
   trim-cli app status fusionplay-fnos          # version = 新版本
   curl -s http://127.0.0.1:18080/healthz       # version = 新版本
   sudo ss -lntp | grep 18080                   # 监听进程启动时间 = 刚刚
   sudo pgrep -af 'fusionplay-fnos/bin/fusionplay-fnos'   # 只有 1 个进程
   ```
4. 配置检查：`sudo cat /vol1/@apphome/fusionplay-fnos/config.json` 中设备/校准/接收器名未丢；
   从 0.1.x 升级时，`config_version` 应变为 `2`，旧全局 `audio.curve` 已复制到每个设备的 `curve`。
5. 功能抽测：AirPlay/DLNA 各投放一次、设备页正常。
6. 若升级后打不开：查看应用中心提示（`TRIM_TEMP_LOGFILE`）与
   `/vol1/@appdata/fusionplay-fnos/app.log`，必要时手动「启动」一次。

## 7. 卸载与残留检查

- 应用中心 → FusionPlay → 卸载（默认保留 `@apphome` 配置与 `@appdata` 日志）。
- 卸载后检查：

```bash
trim-cli app status fusionplay-fnos          # 应返回 not found
sudo ss -lntp | grep 18080                   # 无输出
pgrep -af 'fusionplay-fnos/bin/fusionplay-fnos'   # 无输出
ls -d /var/apps/fusionplay-fnos /vol1/@appcenter/fusionplay-fnos 2>/dev/null  # 应不存在
```

- 需要彻底清理（不可恢复，仅 root）：

```bash
sudo rm -rf /vol1/@apphome/fusionplay-fnos /vol1/@appdata/fusionplay-fnos \
            /vol1/@appcenter/fusionplay-fnos /vol1/@appconf/fusionplay-fnos
```

## 8. 常见安装/启动失败与处理

| 现象 | 原因 | 处置 |
|---|---|---|
| 应用中心解析失败/拒绝安装 | 包不完整或版本非法（只接受 `x.y.z[-r]`） | 重新下载；`tar tzf` 校验；检查 manifest.version |
| 安装完成但打不开 | audio 未授权导致 ALSA 初始化失败（后端会降级，但 UI 可能先报错） | 授权 audio 组后重启应用 |
| 手机搜不到 FusionPlay | 运行环境带 `CI=1`（shairplay 跳过 mDNS）或 avahi/mDNS 未运行 | 去掉 `CI` 环境变量并重启应用；`sudo avahi-browse -rt _airplay._tcp` 验证 |
| `errno 10236` | 同 appname + version 已安装 | 先卸载或换更高版本 |
| 启动后端口占用 | 18080 被其它进程占用 | `sudo ss -lntp \| grep 18080` 找到并停止占用者 |
| CLI 被拒（wizard/dep） | 本包无向导无依赖，正常不会被拒；若出现请核对是否装错文件 | 用 `tar xzOf … manifest` 核对 appname/version |
| 卸载后端口仍占用 | 旧进程残留 | `pkill -f /var/apps/fusionplay-fnos/target/bin/fusionplay-fnos`（先确认是本应用） |

## 9. 回滚

- 想回到旧版本：卸载当前版本（数据默认保留）→ 安装旧版本 fpk。注意旧版本号需低于当前，
  否则同样报 errno 10236（已安装时）。
- `config.json` 在 `@apphome` 会保留；若旧版本不兼容新配置字段，可在 Web UI 里恢复默认/重新导入。

## 10. 许可证与源码

本应用以 **AGPL-3.0-only** 分发，包内包含完整许可证 `app/licenses/LICENSE` 与源码地址
`app/licenses/SOURCE.txt`、`app/BUILD-INFO`（版本/commit/构建时间）。
对应源码：<https://github.com/youyoudezhuzhu/FusionPlay>（tag/commit 见 BUILD-INFO）。
