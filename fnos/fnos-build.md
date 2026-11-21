# FusionPlay fnOS：构建、测试与 CI

本地编译播放器需要 Flutter 3.32.8。构建脚本会编译 `flutter/lib/backend/fnos/player.dart` 并将播放器、字体及 CanvasKit 一起写入 `www/player/`。管理页从播放器菜单进入。


面向开发者。用户安装/验收见 [INSTALL.md](INSTALL.md)；功能边界见 [README.md](README.md)。

## 1. 目录布局

```
fnos/
├── package/                  # FPK 打包源（形态 B：编译二进制）
│   ├── manifest              # INI，官方字段（platform 不是 arch、desc 不是 description…）
│   ├── ICON.PNG / ICON_256.PNG
│   ├── app/ui/config         # 桌面入口 + 网关（gatewayPrefix/gatewaySocket/url）
│   ├── cmd/                  # 9 个生命周期脚本（全部 chmod +x）
│   └── config/{privilege,resource}
├── backend/                  # Rust 服务端（crate: fusionplay-fnos）
├── frontend/                 # 纯静态 Web UI（无 npm 构建）
├── tools/
│   ├── build-fpk.sh          # 组装 + fnpack 打包（含守卫）
│   ├── check-fpk.sh          # 交付物结构/规范检查
│   ├── simulate-install.sh   # 沙箱模拟安装/启动/停止/卸载
│   ├── simulate-upgrade.sh   # 升级路径回归（正常 + 孤儿进程）
│   ├── fpk-sim-lib.sh        # 上面两个脚本的共享函数
│   └── fnpack-linux-amd64    # fnpack 二进制（x86_64 打包器）
└── dist/                    # x86_64 和 ARM64 的独立 FPK
```

## 2. 本机构建依赖

| 依赖 | 用途 | 安装 |
|---|---|---|
| Rust stable | 编译 `fnos/backend`（cpal → ALSA） | <https://rustup.rs> |
| `libasound2-dev` + `pkg-config` | alsa-sys / cpal | `sudo apt-get install -y libasound2-dev pkg-config` |
| `tar` / `curl` / `python3` | 打包、检查、模拟测试 | 系统自带或 apt |
| `fnpack` | 生成 `.fpk` | x86_64 使用仓库自带工具，ARM64 使用飞牛官方 Linux ARM 工具 |

在 ARM64 Linux 构建机安装官方 fnpack。以下命令在仓库根目录运行。

```sh
mkdir -p "$HOME/.local/bin"
curl -fL https://static2.fnnas.com/fnpack/fnpack-1.3.3-linux-arm -o "$HOME/.local/bin/fnpack"
echo "213c05a3df07ff881110d9f227926cd2d19601fc4f621815e673bd9ec0ff9373  $HOME/.local/bin/fnpack" | sha256sum -c -
export FPK_FNPACK="$HOME/.local/bin/fnpack"
chmod 0755 "$FPK_FNPACK"
```

两种架构使用同一份 Flutter Web 输出。设置 `FPK_PLAYER_WEB_DIR` 可复用从同一提交构建的 Web 目录，后台在对应架构的 Linux 上原生编译和运行。CI 会先构建一次播放器，再把相同文件交给两个 FPK 作业。[飞牛 fnpack 文档](https://developer.fnnas.com/docs/cli/fnpack)

## 3. 构建 FPK

```bash
# 1) release 二进制（build-fpk 内部也会跑，增量构建很快）
cargo build --release --manifest-path fnos/backend/Cargo.toml

# 2) 组装 + 打包 + 自检
./fnos/tools/build-fpk.sh

# 产物
ls -lh fnos/dist/*.fpk
```

`build-fpk.sh` 在打包**之前**做这些硬性守卫（失败即退出，不会产出「看起来成功」的坏包）：

1. `manifest.version` == `fnos/backend/Cargo.toml` 的 `version` == 二进制 `--version`
   （`/healthz` 报的就是二进制版本，三者不一致 = 用户看到的版本是假的）。
2. `manifest.platform` 与构建机架构一致（x86_64 → `x86`，aarch64 → `arm`）；
   含架构特定二进制时不能写 `all`。
3. `manifest.service_port` == `cmd/main` 里 `TRIM_SERVICE_PORT` 默认值。
4. `desktop_applaunchname` 在 `app/ui/config` 中存在同名入口。
5. `LICENSE` 是 AGPL 文本、`frontend/index.html` 存在、`cmd/*` 齐全。
6. fnpack 输出必须包含 **`Packing successfully`**（fnpack 打包失败时退出码仍可能是 0，总纲 §4.1）。
7. 打包后做双层 tar 自检，并调用 `check-fpk.sh`。

打包在独立暂存目录（默认 `/tmp/fusionplay-fnos-build`，可用 `APP_BUILD_ROOT` 覆盖），
避免 fnpack 的 0000 ACL 污染仓库。

## 4. 版本号规则与 bump 流程

- `manifest.version` 只允许 `x.y.z` 或 `x.y.z-r`（`-beta/-fix` 之类会被 fnpack 拒绝）。
- **每次重新打包都要 bump**；同 appname + 同 version 重装 = `errno 10236`。
- 版本必须三处一致（`/healthz` 报二进制版本）：

```bash
# 1) 改 crate 版本
$EDITOR fnos/backend/Cargo.toml          # version = "0.1.1"
# 2) 让 Cargo.lock 跟上（cargo 会自动更新本包条目）
cargo build --release --manifest-path fnos/backend/Cargo.toml
# 3) 同步 manifest（appname/service_port/desktop_* 保持不变）
$EDITOR fnos/package/manifest            # version = 0.1.1，更新 changelog
# 4) 打包（守卫会再核对一次）
./fnos/tools/build-fpk.sh
```

`BUILD-INFO` 由 `build-fpk.sh` 自动写入（version / git commit / 构建时间），无需手改。

## 5. 交付物检查：`check-fpk.sh`

```bash
./fnos/tools/check-fpk.sh fnos/dist/fusionplay-fnos.fpk
```

覆盖：

- 顶层：`manifest`、`cmd/` 9 个脚本（**全部可执行**）、`config/{privilege,resource}`、
  `ICON.PNG`、`ICON_256.PNG`、`app.tgz`。
- `app.tgz`：可执行 `bin/fusionplay-fnos`、`www/{index.html,app.js,style.css}`、`ui/config`、
  `ui/images/icon_{64,256}.png`、`licenses/{LICENSE,SOURCE.txt}`、`BUILD-INFO`。
- manifest：官方字段名（拒绝 `arch`/`description`/`developer`/`port`/`icon` 等不存在的字段）、
  必填字段、`source=thirdparty`、`platform∈{x86,arm,all}`、`service_port=18080`、
  `desktop_uidir=ui`、`desktop_applaunchname=fusionplay-fnos.main`、version 格式。
- 版本三方一致：manifest == `app/BUILD-INFO` == 二进制 `--version`。
- `app/ui/config`：入口 key 与 `desktop_applaunchname` 一致、`type=iframe`、
  `gatewayPrefix=/app/fusionplay-fnos`、`gatewaySocket=fusionplay-fnos.sock`、`url` 在前缀下、
  图标占位符只允许 `images/icon_{0}.png`；禁止裸 `{...}` 占位符。
- `config/privilege`：合法 JSON、`run-as=package`（**非 root**）、`username/groupname=appname`、
  `join-groups` 含 `audio`。
- `licenses/LICENSE` 是 AGPL-3.0 文本；图标尺寸 64×64 / 256×256。
- `cmd/main`：默认端口 == `service_port`、用 `/healthz` 判启动成功、`status` 返回 3、
  socket 名与 `gatewaySocket` 一致、不依赖 `$TRIM_APPDEST/cmd/main`（真机 cmd 在 `/var/apps/<app>/cmd`）。

## 6. 沙箱模拟安装：`simulate-install.sh`

```bash
./fnos/tools/simulate-install.sh fnos/dist/fusionplay-fnos.fpk
# 指定端口/联调真实声卡：
SIM_PORT=18999 FUSIONPLAY_AUDIO_BACKEND=alsa ./fnos/tools/simulate-install.sh fnos/dist/fusionplay-fnos.fpk
```

在 `mktemp` 沙箱里解包 fpk，设置临时 `TRIM_APPDEST/TRIM_PKGVAR/TRIM_PKGHOME/TRIM_PKGETC/TRIM_PKGTMP`
（并断言全部在沙箱内），然后完整跑一遍生命周期并断言：

- `install_init` / `install_callback` / `main start` 返回 0；`status` = 0；`/healthz` 200 且版本 = 包版本。
- 子进程环境：`HOME` = `$TRIM_PKGHOME`（不是 `/root`）、`DATA_LIBRARY_PATH` 非空且兜底为 `TRIM_PKGVAR`。
- `config.json` / `fusionplay.log` / `app.log` 落盘；`/api/status`、`/api/config`、静态 UI 200。
- 网关：`/app/fusionplay-fnos/healthz` 200；裸前缀返回 3xx 且 `Location` 带斜杠；
  Unix socket 存在、权限 0666、`/healthz` 200。
- 幂等：重复 `install_callback` 不重启；`config_callback` 重启且 PID 变化。
- 停止后：`status` = 3、PID 文件/socket 删除、端口释放、无残留进程、`/healthz` 不再应答。
- 卸载后：`uninstall_*` 返回 0、无残留 PID/socket；数据目录按策略保留。

安全：健康检查用**随机空闲端口**且断言 `!= manifest.service_port`（避免打到真机线上实例）；
清理只针对沙箱内二进制精确路径，绝不 `pkill` 系统/其它应用进程。

## 7. 升级回归：`simulate-upgrade.sh`

```bash
./fnos/tools/simulate-upgrade.sh fnos/dist/fusionplay-fnos.fpk
# 在 ARM64 构建机上
./fnos/tools/simulate-upgrade.sh fnos/dist/fusionplay-fnos-arm64.fpk
```

x86_64 使用已发布的 0.2.5 做真实双包升级。ARM64 的首个版本 0.2.6 用同一候选包演练替换文件和恢复服务。发布首个 ARM64 版本后，后续候选使用已发布的 ARM64 0.2.6 做双包升级并校验 Release 资产摘要。离线测试可传入同一架构的两个不同版本。

```bash
./fnos/tools/simulate-upgrade.sh /path/to/old.fpk /path/to/new.fpk
```

覆盖运行中升级、丢失或复用 PID、已停用升级及端口被其他程序占用。按前置钩子、文件替换、后置钩子、应用中心端口预检及启动的顺序执行，验证新进程、双入口健康状态及持久化数据。本项目保留上一发布包的 glibc 兼容范围。可通过 `FPK_BACKEND_BIN` 提供在兼容 Linux 工具链上独立构建的 release 后台，仍会执行版本和包内容检查。


## 8. GitHub Actions（`.github/workflows/build-fnos.yml`）

触发：

- `workflow_dispatch`：手动出包。
- push tag `fnos-*`：出包并附到 GitHub Release。

两条原生构建任务分别使用 ubuntu-22.04 和 ubuntu-22.04-arm，按架构独立缓存和测试。

1. `apt-get install libasound2-dev pkg-config build-essential cmake curl tar iproute2 procps python3`
   （cpal/alsa-sys 需要 ALSA 头文件）。
2. Rust stable（`dtolnay/rust-toolchain@stable`）。
3. `actions/cache` 缓存 cargo registry 与各 `target/`，cache key 覆盖所有
   `Cargo.lock`/`Cargo.toml`。
4. 后端单测：`cargo test --manifest-path fnos/backend/Cargo.toml --all-targets`（null 音频，无声卡）。
5. 协议核心单测：`src/AirPlayReceiver.Core --lib`、`src/FusionPlay.MiPlaySdk --lib`
   （都用 `--lib`；step 级超时 20 分钟，避免个别网络用例挂住整个 job）。
6. `cargo build --release --manifest-path fnos/backend/Cargo.toml`。
7. `./fnos/tools/build-fpk.sh` → `./fnos/tools/check-fpk.sh`。
8. `simulate-install.sh` + `simulate-upgrade.sh`（沙箱、随机端口、null 音频）。
9. `actions/upload-artifact@v4` 分别上传 `fusionplay-fnos.fpk` 和 `fusionplay-fnos-arm64.fpk`（30 天）。
10. tag 时 `softprops/action-gh-release@v2` 附到 Release。

**关于 `CI` 环境变量（真机必踩坑）**：上游 `shairplay` 用 `CI` 作为测试开关，存在时**跳过
mDNS 注册**（AirPlay 在发送端完全搜不到，但 HTTP 面板正常）。`cmd/main` 启动服务前会
`unset CI`，所以 GitHub Actions 的 `CI=true` 不会影响包内服务；如果自己写启动/调试脚本，
也不要给服务进程注入 `CI=1`。

**CI 注意事项**（已处理）：

- Release 上传需要 `permissions: contents: write`，否则 tag 构建最后一步 403。
- ARM64 使用固定版本及 SHA-256 校验的官方下载文件。
- 下载 artifact 必须 `curl -L`（会 302 到对象存储，不跟随会拿到空文件）。
- `cargo`/`target` 缓存命中后构建时间显著下降；缓存 key 必须包含 `Cargo.lock`。
- 发包前务必 bump 版本（三处一致），否则 `build-fpk.sh` 会直接失败。
- 不要改动 `.github/workflows/android.yml`（Android 构建独立）。

查看/触发：

```bash
# 手动触发（注意：dispatch 成功返回 204 且 body 为空，别当 JSON 解析）
gh workflow run build-fnos.yml --ref fnos/fpk

# 看运行
gh run list --workflow=build-fnos.yml
```

## 9. 发布流程

1. 改代码 + 单测通过 + bump 版本（三处一致）。
2. 本地跑完整验证（见下节）。
3. 提交并打 tag：`git tag fnos-v0.1.1 && git push origin fnos-v0.1.1`
   （tag 格式必须是 `fnos-*`，否则不会触发）。
4. CI 分别产出 x86_64 和 ARM64 的 FPK，两条构建任务全部通过后才会附到同一个 Release。
5. 真机按 [INSTALL.md](INSTALL.md) 验收（尤其 audio 授权与升级路径）。

## 10. 本地一键验证

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# 1) 编译 + 单测
cargo test --manifest-path fnos/backend/Cargo.toml --all-targets
cargo test --manifest-path src/AirPlayReceiver.Core/Cargo.toml --lib
cargo test --manifest-path src/FusionPlay.MiPlaySdk/Cargo.toml --lib

# 2) 构建 + 检查 + 模拟安装 + 升级回归
cargo build --release --manifest-path fnos/backend/Cargo.toml
./fnos/tools/build-fpk.sh
./fnos/tools/check-fpk.sh fnos/dist/fusionplay-fnos.fpk
./fnos/tools/simulate-install.sh fnos/dist/fusionplay-fnos.fpk
./fnos/tools/simulate-upgrade.sh fnos/dist/fusionplay-fnos.fpk
```

全部通过后再打 tag。

## 11. 日志与轮转约定（打包相关）

- 后端 `logging.rs`：`$TRIM_PKGVAR/fusionplay.log` 5 MiB 上限、超限原地保留尾部 1 MiB（inode 不变）。
- `cmd/main`：`$TRIM_PKGVAR/app.log` 同样 tail-trim 原地重写；**禁止** `mv/rename`（会把日志写进已改名的旧 inode）。
- 两个调用点都要保留：`cmd/main` 启动/停止时、后端自身写入时；长期挂着的 daemon 不允许无限增长。

## 12. 交付纪律

- 不把服务常驻 root：`run-as: package` + `join-groups: ["audio"]`，`check-fpk.sh` 会校验。
- 不删除或改写 Android CI（`.github/workflows/android.yml`）。
- FPK 内含 AGPL LICENSE + 源码地址 + BUILD-INFO（commit），满足 AGPL 源码义务。
- 对外发布前做隐私清理（真实姓名/内网 IP/口令示例）；manifest 元数据中性化。
- **不要**顺手把包装到别人的 NAS 上；真机安装先询问。
