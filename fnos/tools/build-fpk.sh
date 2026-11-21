#!/bin/bash
# 构建 fnOS FPK：Rust release 二进制 + 静态前端 + fnpack。
#
# 用法：
#   ./fnos/tools/build-fpk.sh
#   APP_BUILD_ROOT=/tmp/my-build ./fnos/tools/build-fpk.sh    # 自定义暂存目录
#
# 为什么必须在独立暂存目录里打包（总纲 §4.1）：
#   fnpack 会把它处理的目录权限封成 0000（fnOS ACL 坑），连属主都 rm -rf 不掉，
#   而且 cp -r 会把该 ACL 复制出去。所以绝不在仓库目录里直接 fnpack build。
#
# 打包前的硬性守卫（失败即退出，绝不产出「看起来成功」的坏包）：
#   1. manifest.version == fnos/backend/Cargo.toml version == 二进制 --version
#      （/healthz 报的就是二进制版本，三者不一致 = 用户看到的版本是假的）
#   2. manifest.platform 与构建机架构一致（含架构特定二进制时不能写 all）
#   3. manifest.service_port == cmd/main 的默认端口
#   4. desktop_applaunchname 在 app/ui/config 中存在同名入口
#   5. fnpack 输出必须包含 "Packing successfully"（fnpack 打包失败仍可能返回 0，总纲 §4.1）
set -euo pipefail
umask 022

ROOT="$(cd "$(dirname "$0")/.." && pwd)"        # fnos/
REPO="$(cd "$ROOT/.." && pwd)"
PKG="$ROOT/package"
STAGE_ROOT="${APP_BUILD_ROOT:-/tmp/fusionplay-fnos-build}"
source "$ROOT/tools/fpk-platform.sh"
FNPACK="$ROOT/tools/fnpack-linux-amd64"
PACKER=("$FNPACK")
DIST="$ROOT/dist"

die() { echo "❌ $*" >&2; exit 1; }
manifest_get() {
    # INI：取第一个 key 的值，去掉引号/空白
    grep -m1 -E "^[[:space:]]*$1[[:space:]]*=" "$PKG/manifest" \
        | sed 's/^[^=]*=//' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' \
              -e 's/^"//' -e 's/"$//'
}

export PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:$PATH"
if [ -z "${FPK_BACKEND_BIN:-}" ]; then command -v cargo >/dev/null || die "缺少 cargo"; fi
[ -x "$FNPACK" ] || die "缺少 fnpack 可执行文件：$FNPACK"
if [ "$FPK_PLATFORM" = arm ]; then
    command -v qemu-x86_64 >/dev/null || die "缺少 qemu-x86_64，请安装 qemu-user 运行仓库内的 fnpack。"
    PACKER=(qemu-x86_64 "$FNPACK")
fi
[ -f "$REPO/LICENSE" ] || die "仓库根缺少 LICENSE（AGPL 分发必需）"
grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" "$REPO/LICENSE" \
    || die "LICENSE 不是 AGPL 文本（本项目为 AGPL-3.0-only）"
[ -f "$ROOT/frontend/index.html" ] || die "缺少 fnos/frontend/index.html（Web UI 未提交？）"
[ -f "$ROOT/backend/Cargo.toml" ] || die "缺少 fnos/backend/Cargo.toml"

APPNAME="$(manifest_get appname)"
VERSION="$(manifest_get version)"
SERVICE_PORT="$(manifest_get service_port)"
DESKTOP_LAUNCH="$(manifest_get desktop_applaunchname)"
DESKTOP_UIDIR="$(manifest_get desktop_uidir)"

[ -n "$APPNAME" ] || die "manifest 缺少 appname"
[ "$APPNAME" = "fusionplay-fnos" ] || die "appname 必须是 fusionplay-fnos（当前 $APPNAME）"
[ -n "$VERSION" ] || die "manifest 缺少 version"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9]+)?$ ]] \
    || die "manifest.version=$VERSION 非法：只允许 x.y.z 或 x.y.z-r（不能带 -beta/-fix 等后缀，fnpack 会拒绝）"

# ---------------- 守卫 1：版本三方一致 ----------------
BACKEND_VERSION="$(grep -m1 -E '^version[[:space:]]*=' "$ROOT/backend/Cargo.toml" \
    | sed 's/^[^=]*=//' | tr -d ' "')"
[ -n "$BACKEND_VERSION" ] || die "无法读取 fnos/backend/Cargo.toml 的 version"
[ "$VERSION" = "$BACKEND_VERSION" ] \
    || die "版本不一致：manifest.version=$VERSION，但 fnos/backend/Cargo.toml version=$BACKEND_VERSION。
   /healthz 报的是二进制版本；请同步：改 backend Cargo.toml + Cargo.lock，或改 manifest。"

# ---------------- 守卫 3：service_port 与 cmd/main 默认一致 ----------------
MAIN_PORT="$(grep -oE 'TRIM_SERVICE_PORT:-[0-9]+' "$PKG/cmd/main" | head -1 | grep -oE '[0-9]+$')"
[ -n "$MAIN_PORT" ] || die "cmd/main 里找不到 TRIM_SERVICE_PORT 默认值"
[ "$SERVICE_PORT" = "$MAIN_PORT" ] \
    || die "manifest.service_port=$SERVICE_PORT 与 cmd/main 默认端口=$MAIN_PORT 不一致"

# ---------------- 守卫 4：桌面入口三处对齐 ----------------
[ "$DESKTOP_UIDIR" = "ui" ] || die "desktop_uidir 必须是 ui（当前 $DESKTOP_UIDIR）"
grep -q "\"$DESKTOP_LAUNCH\"" "$PKG/app/ui/config" \
    || die "app/ui/config 缺少入口 \"$DESKTOP_LAUNCH\"（desktop_applaunchname=$DESKTOP_LAUNCH）"
echo "▶ 守卫通过：version=$VERSION（manifest=backend），platform=$FPK_PLATFORM，service_port=$SERVICE_PORT，launch=$DESKTOP_LAUNCH"

if [ -n "${FPK_BACKEND_BIN:-}" ]; then
    BIN="$FPK_BACKEND_BIN"
else
    echo "▶ cargo build --release ($(uname -m))"
    cargo build --locked --release --manifest-path "$ROOT/backend/Cargo.toml"
    BIN="${CARGO_TARGET_DIR:-$ROOT/backend/target}/release/fusionplay-fnos"
fi
[ -x "$BIN" ] || die "未生成 release 二进制：$BIN"
BIN_VERSION="$("$BIN" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
[ "$BIN_VERSION" = "$VERSION" ] \
    || die "二进制 --version=$BIN_VERSION 与 manifest.version=$VERSION 不一致（/healthz 会报错版本）"

# ---------------- 组装独立暂存目录 ----------------
STAGE="$STAGE_ROOT/$FPK_PLATFORM/$APPNAME"
echo "▶ 组装暂存目录 $STAGE"
mkdir -p "$STAGE_ROOT"
chmod -R u+rwX "$STAGE" 2>/dev/null || true
rm -rf "$STAGE"
mkdir -p "$STAGE"
[ -z "$(ls -A "$STAGE" 2>/dev/null)" ] || die "暂存目录未清空，残留旧文件会被打进包：$STAGE"

cp -r "$PKG/." "$STAGE/"
# ★ cp 之后立刻修权限，再 mkdir（从带 TrimACL 的目录复制出来可能是 d---------）
chmod -R u+rwX "$STAGE" 2>/dev/null || true

sed -i -E "s/^platform([[:space:]]*)=.*/platform\1= $FPK_PLATFORM/" "$STAGE/manifest"
mkdir -p "$STAGE/app/bin" "$STAGE/app/www" "$STAGE/app/licenses"
cp -f "$BIN" "$STAGE/app/bin/fusionplay-fnos"
cp -r "$ROOT/frontend/." "$STAGE/app/www/"
PLAYER_WEB="${FPK_PLAYER_WEB_DIR:-$REPO/flutter/build/web}"
if [ -z "${FPK_PLAYER_WEB_DIR:-}" ]; then
    (
        cd "$REPO/flutter"
        flutter pub get --enforce-lockfile
        flutter build web --release --no-pub --no-web-resources-cdn --pwa-strategy=none -t lib/backend/fnos/player.dart
    )
fi
[ -s "$PLAYER_WEB/main.dart.js" ] && [ -s "$PLAYER_WEB/index.html" ] ||
    die "缺少 Flutter Web 播放器：$PLAYER_WEB"
mkdir -p "$STAGE/app/www/player"
cp -r "$PLAYER_WEB/." "$STAGE/app/www/player/"
cp -f "$REPO/LICENSE" "$STAGE/app/licenses/LICENSE"
if [ -f "$REPO/THIRD_PARTY_NOTICES.md" ]; then
    cp -f "$REPO/THIRD_PARTY_NOTICES.md" "$STAGE/app/licenses/THIRD_PARTY_NOTICES.md"
else
    echo "  ⚠️  未找到 THIRD_PARTY_NOTICES.md，跳过第三方声明" >&2
fi
cat > "$STAGE/app/licenses/SOURCE.txt" <<TXT
FusionPlay fnOS FPK — 本包内二进制由仓库源码构建：
https://github.com/youyoudezhuzhu/FusionPlay （AGPL-3.0-only）
对应源码 tag/commit 见本包 app/BUILD-INFO，完整对应源码可从上述仓库获取。
TXT
printf 'version=%s\ncommit=%s\nbuilt=%s\n' \
    "$VERSION" \
    "$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)" \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" > "$STAGE/app/BUILD-INFO"

# ---------------- 清理开发残留 / 赋执行位 ----------------
find "$STAGE" -name '__pycache__' -type d -print0 2>/dev/null | xargs -0 -r rm -rf || true
find "$STAGE" -name '*.pyc' -delete 2>/dev/null || true
find "$STAGE" -name '.DS_Store' -delete 2>/dev/null || true
rm -f "$STAGE"/*.fpk "$STAGE/app/config.json" 2>/dev/null || true
chmod -R u+rwX "$STAGE" 2>/dev/null || true
for s in install_init install_callback upgrade_init upgrade_callback \
         uninstall_init uninstall_callback config_init config_callback main; do
    [ -f "$STAGE/cmd/$s" ] || die "暂存目录缺少 cmd/$s"
done
# Packages are unpacked by the framework and run by a separate package user.
# Do not distribute the checkout owner's private 0700/0600 modes.
find "$STAGE" -type d -exec chmod 0755 {} +
find "$STAGE" -type f -exec chmod 0644 {} +
chmod 0755 "$STAGE"/cmd/* "$STAGE/app/bin/fusionplay-fnos"
[ -f "$STAGE/app/www/index.html" ] || die "app/www/index.html 缺失（前端未复制成功）"

# ---------------- fnpack 打包 ----------------
PACK_LOG="$STAGE_ROOT/$FPK_PLATFORM/fnpack-build.log"
echo "▶ fnpack build（$APPNAME $VERSION）"
set +e
( cd "$STAGE" && "${PACKER[@]}" build ) 2>&1 | tee "$PACK_LOG"
set -e
# ★ fnpack 打包失败时也可能返回 0，必须解析输出（总纲 §4.1）
if ! grep -q "Packing successfully" "$PACK_LOG"; then
    echo "❌ fnpack 打包失败，原始输出：" >&2
    cat "$PACK_LOG" >&2
    exit 1
fi
FPK="$STAGE/$APPNAME.fpk"
[ -f "$FPK" ] || die "fnpack 报告成功但未找到产物 $FPK"

# ---------------- 双层 tar 自检 + 结构检查 ----------------
# 注意：不要用 `tar -tzf x | grep -q`——grep -q 提前退出会让 tar 收到 SIGPIPE，
# 在 `set -o pipefail` 下管线被判失败（假阴性）。先把清单落到文件再 grep。
echo "▶ 双层 tar 自检"
LIST="$STAGE_ROOT/$FPK_PLATFORM/fpk-list.txt"
APPLIST="$STAGE_ROOT/$FPK_PLATFORM/fpk-app-list.txt"
tar -tzf "$FPK" > "$LIST"
tar -xzOf "$FPK" app.tgz | tar -tzf - > "$APPLIST"
grep -qx "manifest" "$LIST" || die "FPK 缺少 manifest"
grep -qx "app.tgz" "$LIST" || die "FPK 缺少 app.tgz"
grep -qx "bin/fusionplay-fnos" "$APPLIST" || die "app.tgz 缺少 bin/fusionplay-fnos"
grep -qx "ui/config" "$APPLIST" || die "app.tgz 缺少 ui/config"
grep -qx "licenses/LICENSE" "$APPLIST" || die "app.tgz 缺少 licenses/LICENSE"

echo "▶ 结构/规范检查（check-fpk.sh）"
"$ROOT/tools/check-fpk.sh" "$FPK" || die "check-fpk.sh 未通过"

mkdir -p "$DIST"
OUT="$DIST/$FPK_FILENAME"
PENDING=$(mktemp "$DIST/.candidate.XXXXXX")
trap 'rm -f -- "$PENDING"' EXIT
install -m 0644 "$FPK" "$PENDING"
mv -f -- "$PENDING" "$OUT"
ls -lh "$OUT"
echo "✅ 构建完成：$OUT（version=$VERSION，platform=$FPK_PLATFORM）"
