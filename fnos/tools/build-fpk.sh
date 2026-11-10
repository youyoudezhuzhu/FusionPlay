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

ROOT="$(cd "$(dirname "$0")/.." && pwd)"        # fnos/
REPO="$(cd "$ROOT/.." && pwd)"
PKG="$ROOT/package"
STAGE_ROOT="${APP_BUILD_ROOT:-/tmp/fusionplay-fnos-build}"
FNPACK="$ROOT/tools/fnpack-linux-amd64"
DIST="$ROOT/dist"

die() { echo "❌ $*" >&2; exit 1; }
manifest_get() {
    # INI：取第一个 key 的值，去掉引号/空白
    grep -m1 -E "^[[:space:]]*$1[[:space:]]*=" "$PKG/manifest" \
        | sed 's/^[^=]*=//' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' \
              -e 's/^"//' -e 's/"$//'
}

export PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || die "缺少 cargo（宿主机需要 Rust stable）"
[ -x "$FNPACK" ] || die "缺少 fnpack 可执行文件：$FNPACK（该二进制应提交进仓库）"
[ -f "$REPO/LICENSE" ] || die "仓库根缺少 LICENSE（AGPL 分发必需）"
grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" "$REPO/LICENSE" \
    || die "LICENSE 不是 AGPL 文本（本项目为 AGPL-3.0-only）"
[ -f "$ROOT/frontend/index.html" ] || die "缺少 fnos/frontend/index.html（Web UI 未提交？）"
[ -f "$ROOT/backend/Cargo.toml" ] || die "缺少 fnos/backend/Cargo.toml"

APPNAME="$(manifest_get appname)"
VERSION="$(manifest_get version)"
PLATFORM="$(manifest_get platform)"
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

# ---------------- 守卫 2：platform 与架构一致 ----------------
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64|amd64)  EXPECT_PLATFORM="x86" ;;
    aarch64|arm64) EXPECT_PLATFORM="arm" ;;
    *)             EXPECT_PLATFORM="all" ;;
esac
[ "$PLATFORM" = "$EXPECT_PLATFORM" ] \
    || die "manifest.platform=$PLATFORM 与构建机架构 $HOST_ARCH 不一致（应写 $EXPECT_PLATFORM）。
   含架构特定二进制时 platform 不能用 all（ARM 机器装 x86 包会直接崩）。"

# ---------------- 守卫 3：service_port 与 cmd/main 默认一致 ----------------
MAIN_PORT="$(grep -oE 'TRIM_SERVICE_PORT:-[0-9]+' "$PKG/cmd/main" | head -1 | grep -oE '[0-9]+$')"
[ -n "$MAIN_PORT" ] || die "cmd/main 里找不到 TRIM_SERVICE_PORT 默认值"
[ "$SERVICE_PORT" = "$MAIN_PORT" ] \
    || die "manifest.service_port=$SERVICE_PORT 与 cmd/main 默认端口=$MAIN_PORT 不一致"

# ---------------- 守卫 4：桌面入口三处对齐 ----------------
[ "$DESKTOP_UIDIR" = "ui" ] || die "desktop_uidir 必须是 ui（当前 $DESKTOP_UIDIR）"
grep -q "\"$DESKTOP_LAUNCH\"" "$PKG/app/ui/config" \
    || die "app/ui/config 缺少入口 \"$DESKTOP_LAUNCH\"（desktop_applaunchname=$DESKTOP_LAUNCH）"
echo "▶ 守卫通过：version=$VERSION（manifest=backend），platform=$PLATFORM，service_port=$SERVICE_PORT，launch=$DESKTOP_LAUNCH"

echo "▶ cargo build --release（$HOST_ARCH）"
cargo build --release --manifest-path "$ROOT/backend/Cargo.toml"
BIN="$ROOT/backend/target/release/fusionplay-fnos"
[ -x "$BIN" ] || die "未生成 release 二进制：$BIN"
BIN_VERSION="$("$BIN" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
[ "$BIN_VERSION" = "$VERSION" ] \
    || die "二进制 --version=$BIN_VERSION 与 manifest.version=$VERSION 不一致（/healthz 会报错版本）"

# ---------------- 组装独立暂存目录 ----------------
STAGE="$STAGE_ROOT/$APPNAME"
echo "▶ 组装暂存目录 $STAGE"
mkdir -p "$STAGE_ROOT"
chmod -R u+rwX "$STAGE_ROOT" 2>/dev/null || true
rm -rf "$STAGE"
mkdir -p "$STAGE"
[ -z "$(ls -A "$STAGE" 2>/dev/null)" ] || die "暂存目录未清空，残留旧文件会被打进包：$STAGE"

cp -r "$PKG/." "$STAGE/"
# ★ cp 之后立刻修权限，再 mkdir（从带 TrimACL 的目录复制出来可能是 d---------）
chmod -R u+rwX "$STAGE" 2>/dev/null || true

mkdir -p "$STAGE/app/bin" "$STAGE/app/www" "$STAGE/app/licenses"
cp -f "$BIN" "$STAGE/app/bin/fusionplay-fnos"
cp -r "$ROOT/frontend/." "$STAGE/app/www/"
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
    chmod +x "$STAGE/cmd/$s"
done
chmod +x "$STAGE/app/bin/fusionplay-fnos"
[ -f "$STAGE/app/www/index.html" ] || die "app/www/index.html 缺失（前端未复制成功）"

# ---------------- fnpack 打包 ----------------
PACK_LOG="$STAGE_ROOT/fnpack-build.log"
echo "▶ fnpack build（$APPNAME $VERSION）"
set +e
( cd "$STAGE" && "$FNPACK" build ) 2>&1 | tee "$PACK_LOG"
set -e
# ★ fnpack 打包失败时也可能返回 0，必须解析输出（总纲 §4.1）
if ! grep -q "Packing successfully" "$PACK_LOG"; then
    echo "❌ fnpack 打包失败，原始输出：" >&2
    cat "$PACK_LOG" >&2
    exit 1
fi
FPK="$STAGE/$APPNAME.fpk"
[ -f "$FPK" ] || die "fnpack 报告成功但未找到产物 $FPK"

mkdir -p "$DIST"
OUT="$DIST/$APPNAME.fpk"
cp -f "$FPK" "$OUT"
chmod 644 "$OUT"
echo "✅ 产出: $OUT"

# ---------------- 双层 tar 自检 + 结构检查 ----------------
# 注意：不要用 `tar -tzf x | grep -q`——grep -q 提前退出会让 tar 收到 SIGPIPE，
# 在 `set -o pipefail` 下管线被判失败（假阴性）。先把清单落到文件再 grep。
echo "▶ 双层 tar 自检"
LIST="$STAGE_ROOT/fpk-list.txt"
APPLIST="$STAGE_ROOT/fpk-app-list.txt"
tar -tzf "$OUT" > "$LIST"
tar -xzOf "$OUT" app.tgz | tar -tzf - > "$APPLIST"
grep -qx "manifest" "$LIST" || die "FPK 缺少 manifest"
grep -qx "app.tgz" "$LIST" || die "FPK 缺少 app.tgz"
grep -qx "bin/fusionplay-fnos" "$APPLIST" || die "app.tgz 缺少 bin/fusionplay-fnos"
grep -qx "ui/config" "$APPLIST" || die "app.tgz 缺少 ui/config"
grep -qx "licenses/LICENSE" "$APPLIST" || die "app.tgz 缺少 licenses/LICENSE"

if [ -x "$ROOT/tools/check-fpk.sh" ]; then
    echo "▶ 结构/规范检查（check-fpk.sh）"
    "$ROOT/tools/check-fpk.sh" "$OUT" || die "check-fpk.sh 未通过"
fi

ls -lh "$OUT"
echo "✅ 构建完成：$OUT（version=$VERSION，platform=$PLATFORM）"
