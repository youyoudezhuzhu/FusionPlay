#!/bin/bash
# simulate-upgrade.sh — 升级路径回归测试，守住「升级后功能没变」这个最隐蔽的故障（总纲 §5.5）。
#
# 覆盖两个场景：
#   场景 1 正常升级：旧包安装/启动 → 替换 target 代码 → upgrade_init（断言端口释放、PID 删除、
#                    旧进程消失）→ upgrade_callback（断言 /healthz 报【新】版本、配置保留、进程换成新二进制）
#   场景 2 孤儿进程升级：PID 文件丢失但旧进程仍占着端口（升级残留/手工删了 PID），
#                    upgrade_init 仍必须能按精确路径/端口清掉它，upgrade_callback 拉起新版本。
#
# 新版本怎么来（不改仓库、不改 src/backend）：
#   把 fnos/backend 复制到临时目录，只把 Cargo.toml 的 crate version +1 后重新编译，
#   再用 fnpack 把旧包 repack 成新版本测试包（app/BUILD-INFO、manifest 同步）。
#   这样 /healthz 报的版本确实变化，测的是真实的「换二进制 + 跑升级脚本」路径。
#
# 用法：
#   ./fnos/tools/simulate-upgrade.sh [old.fpk] [new-version]
#   默认 old.fpk = fnos/dist/fusionplay-fnos.fpk
#
# 注意：编译新版本会短暂复用 fnos/backend/target（cargo 自身有锁）；脚本结束前会
#       重新构建真实版本，避免影响后续 build-fpk。
set -uo pipefail

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SELF_DIR/.." && pwd)"
REPO="$(cd "$ROOT/.." && pwd)"
FNPACK="$ROOT/tools/fnpack-linux-amd64"
# shellcheck source=fpk-sim-lib.sh
source "$SELF_DIR/fpk-sim-lib.sh"

OLD_FPK="${1:-$ROOT/dist/fusionplay-fnos.fpk}"
NEW_VERSION="${2:-}"
sim_require_tools
export PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || { echo "❌ 升级测试需要 cargo（会重建一个 +1 版本的测试二进制）" >&2; exit 1; }
[ -x "$FNPACK" ] || { echo "❌ 缺少 fnpack：$FNPACK" >&2; exit 1; }
[ -f "$OLD_FPK" ] || { echo "❌ 找不到旧包 $OLD_FPK（先跑 ./fnos/tools/build-fpk.sh）" >&2; exit 1; }
OLD_FPK="$(cd "$(dirname "$OLD_FPK")" && pwd)/$(basename "$OLD_FPK")"

SANDBOX="$(sim_sandbox)"
OLD_WORK="$SANDBOX/old"
NEW_PKG="$SANDBOX/newpkg"
mkdir -p "$OLD_WORK" "$NEW_PKG"
CMD_MAIN=""

cleanup() {
    set +u
    if [ -n "${CMD_MAIN:-}" ] && [ -f "$CMD_MAIN" ]; then
        /bin/bash "$CMD_MAIN" stop >/dev/null 2>&1 || true
    fi
    sim_cleanup_sandbox_processes "$SANDBOX"
    if [ "${SIM_FAIL:-0}" -gt 0 ]; then
        echo "ℹ️  测试失败，沙箱保留便于排查：$SANDBOX"
    else
        rm -rf "$SANDBOX"
    fi
}
trap cleanup EXIT INT TERM

die() { echo "❌ $*" >&2; exit 1; }
bump_version() {
    # 0.1.0 -> 0.1.1；0.1.0-3 -> 0.1.0-4；非法输入返回空
    local v="$1" major minor patch suffix
    if [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)-([0-9]+)$ ]]; then
        major="${BASH_REMATCH[1]}"; minor="${BASH_REMATCH[2]}"; patch="${BASH_REMATCH[3]}"
        suffix=$((BASH_REMATCH[4] + 1))
        printf '%s.%s.%s-%s\n' "$major" "$minor" "$patch" "$suffix"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        major="${BASH_REMATCH[1]}"; minor="${BASH_REMATCH[2]}"; patch="${BASH_REMATCH[3]}"
        patch=$((patch + 1))
        printf '%s.%s.%s\n' "$major" "$minor" "$patch"
    else
        printf '\n'
    fi
}

echo "▶ 读取旧包：$OLD_FPK"
tar -xzf "$OLD_FPK" -C "$OLD_WORK" || die "旧包解包失败"
APPNAME="$(sim_manifest_get "$OLD_WORK/manifest" appname)"
OLD_VERSION="$(sim_manifest_get "$OLD_WORK/manifest" version)"
[ -n "$APPNAME" ] || die "旧包 manifest 缺 appname"
[ -n "$OLD_VERSION" ] || die "旧包 manifest 缺 version"
[ -z "$NEW_VERSION" ] && NEW_VERSION="$(bump_version "$OLD_VERSION")"
[ -n "$NEW_VERSION" ] || die "无法从 $OLD_VERSION 推导新版本号"
[ "$NEW_VERSION" != "$OLD_VERSION" ] || die "新版本号不能等于旧版本号"
echo "▶ 升级测试：$APPNAME  $OLD_VERSION  →  $NEW_VERSION"

# ── 1. 重建一个 +1 版本的测试二进制（不改仓库，只改临时副本的 Cargo.toml）────────
echo
echo "=== 准备：构建 $NEW_VERSION 测试二进制 ==="
# The decoder also imports shared_audio modules relative to fnos/backend.
BK="$SANDBOX/fnos/backend"
mkdir -p "$SANDBOX/src"
ln -s "$REPO/src/shared_audio" "$SANDBOX/src/shared_audio"
mkdir -p "$BK"
( cd "$ROOT/backend" && tar --exclude='./target' -cf - . ) | tar -xf - -C "$BK" || die "复制 backend 失败"
[ -f "$BK/Cargo.toml" ] || die "临时 backend 缺 Cargo.toml"
sed -i -E "s/^version[[:space:]]*=[[:space:]]*\"[^\"]*\"/version = \"$NEW_VERSION\"/" "$BK/Cargo.toml"
# path 依赖改成绝对路径（临时目录不在仓库内）
sed -i -E "s#path = \"\.\./\.\./#path = \"$REPO/#g" "$BK/Cargo.toml"
grep -q "path = \"$REPO/src/AirPlayReceiver.Core\"" "$BK/Cargo.toml" || die "backend path 依赖改写失败"
[ -f "$BK/Cargo.lock" ] || cp "$ROOT/backend/Cargo.lock" "$BK/Cargo.lock" 2>/dev/null || true

TARGET_DIR="$ROOT/backend/target"
echo "▶ cargo build（变体 $NEW_VERSION，复用 $TARGET_DIR）"
if ! cargo build --release --manifest-path "$BK/Cargo.toml" --target-dir "$TARGET_DIR" \
        > "$SANDBOX/build-new.log" 2>&1; then
    tail -30 "$SANDBOX/build-new.log" >&2
    die "构建 $NEW_VERSION 变体二进制失败"
fi
NEW_BIN="$SANDBOX/bin-$NEW_VERSION"
cp -f "$TARGET_DIR/release/fusionplay-fnos" "$NEW_BIN" || die "复制变体二进制失败"
chmod +x "$NEW_BIN"
NEW_BIN_VERSION="$("$NEW_BIN" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
[ "$NEW_BIN_VERSION" = "$NEW_VERSION" ] || die "变体二进制 --version=$NEW_BIN_VERSION 与目标 $NEW_VERSION 不一致"
sim_ok "变体二进制构建完成：--version = $NEW_VERSION"

echo "▶ 恢复真实构建（避免污染 fnos/backend/target）"
if ! cargo build --release --manifest-path "$ROOT/backend/Cargo.toml" --target-dir "$TARGET_DIR" \
        > "$SANDBOX/build-real.log" 2>&1; then
    tail -30 "$SANDBOX/build-real.log" >&2
    die "恢复真实 release 构建失败（fnos/backend/target 可能残留变体，请手动重新 build）"
fi
REAL_BIN_VERSION="$("$TARGET_DIR/release/fusionplay-fnos" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
[ "$REAL_BIN_VERSION" = "$OLD_VERSION" ] \
    || die "真实二进制版本=$REAL_BIN_VERSION 与旧包版本=$OLD_VERSION 不一致（dist 里的包过期了？先跑 build-fpk）"
sim_ok "真实构建已恢复：--version = $REAL_BIN_VERSION"

# ── 2. 用 fnpack 把旧包 repack 成新版本测试包 ────────────────────────────────
echo
echo "=== 准备：repack $NEW_VERSION 测试包 ==="
tar -xzf "$OLD_FPK" -C "$NEW_PKG" || die "新包解包失败"
mkdir -p "$NEW_PKG/app"
tar -xzf "$NEW_PKG/app.tgz" -C "$NEW_PKG/app" || die "app.tgz 解包失败"
# 原 app.tgz 必须移出 newpkg（否则会被 fnpack 当普通文件打进包）。
# 注意：真正用于替换 target 的是 repack 后【新包】里的 app.tgz，稍后从 NEW_FPK 提取。
mv -f "$NEW_PKG/app.tgz" "$SANDBOX/orig-app-$OLD_VERSION.tgz"
cp -f "$NEW_BIN" "$NEW_PKG/app/bin/fusionplay-fnos"
chmod +x "$NEW_PKG/app/bin/fusionplay-fnos"
sed -i -E "s/^version([[:space:]]*=).*\$/version\1 $NEW_VERSION/" "$NEW_PKG/manifest"
grep -qE "^version[[:space:]]*=[[:space:]]*$NEW_VERSION[[:space:]]*\$" "$NEW_PKG/manifest" \
    || die "改写新包 manifest.version 失败"
printf 'version=%s\ncommit=%s\nbuilt=%s\n' "$NEW_VERSION" \
    "$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)" \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" > "$NEW_PKG/app/BUILD-INFO"
chmod +x "$NEW_PKG"/cmd/* 2>/dev/null || true
PACK_LOG="$SANDBOX/fnpack-new.log"
set +e
( cd "$NEW_PKG" && "$FNPACK" build ) 2>&1 | tee "$PACK_LOG"
set -e
grep -q "Packing successfully" "$PACK_LOG" || { cat "$PACK_LOG" >&2; die "repack $NEW_VERSION 失败"; }
NEW_FPK="$NEW_PKG/$APPNAME.fpk"
[ -f "$NEW_FPK" ] || die "repack 未生成 $NEW_FPK"
sim_ok "新版本测试包已生成：$NEW_FPK"
"$ROOT/tools/check-fpk.sh" "$NEW_FPK" > "$SANDBOX/check-new.log" 2>&1
RC=$?
sim_eq "0" "$RC" "新版本测试包通过 check-fpk.sh（版本/结构一致）"
if [ "$RC" -ne 0 ]; then tail -20 "$SANDBOX/check-new.log"; fi
NEWCMD="$NEW_PKG/cmd"
# 用 repack 后的新包里的 app.tgz 作为「升级后 target 代码」（含新版本二进制）
NEW_APPTGZ="$SANDBOX/app-$NEW_VERSION.tgz"
tar -xzOf "$NEW_FPK" app.tgz > "$NEW_APPTGZ" || die "提取新版本 app.tgz 失败"
sim_ok "已从新包提取升级用 app.tgz（$NEW_VERSION）"

# 场景环境初始化：设置 TRIM_* 并解包指定版本代码
scenario_env() {
    local dir="$1" port="$2" fpk="$3"
    S_ROOT="$dir"; S_WORK="$dir/work"; S_TARGET="$dir/target"; S_VAR="$dir/var"
    S_HOME="$dir/home"; S_ETC="$dir/etc"
    mkdir -p "$S_WORK" "$S_TARGET" "$S_VAR" "$S_HOME" "$S_ETC"
    tar -xzf "$fpk" -C "$S_WORK" || die "场景解包失败：$fpk"
    tar -xzf "$S_WORK/app.tgz" -C "$S_TARGET" || die "场景 app.tgz 解包失败"
    CMD_MAIN="$S_WORK/cmd/main"
    unset DATA_LIBRARY_PATH   # fnOS 不注入该变量；清掉宿主同名变量，测真实兜底
    export TRIM_APPDEST="$S_TARGET" TRIM_PKGVAR="$S_VAR" TRIM_PKGHOME="$S_HOME" \
           TRIM_PKGETC="$S_ETC" TRIM_SERVICE_PORT="$port" TRIM_APPNAME="$APPNAME" \
           TRIM_TEMP_LOGFILE="$S_VAR/upgrade-error.log" \
           FUSIONPLAY_AUDIO_BACKEND="${FUSIONPLAY_AUDIO_BACKEND:-null}"
    for pair in "TRIM_APPDEST=$TRIM_APPDEST" "TRIM_PKGVAR=$TRIM_PKGVAR" \
                "TRIM_PKGHOME=$TRIM_PKGHOME" "TRIM_PKGETC=$TRIM_PKGETC" \
                "TRIM_TEMP_LOGFILE=$TRIM_TEMP_LOGFILE"; do
        sim_require_under "${pair%%=*}" "${pair#*=}" "$SANDBOX"
    done
}
scenario_port() { sim_pick_port "$(sim_manifest_get "$OLD_WORK/manifest" service_port)"; }
mrun() { timeout 120 "$@" >> "$SANDBOX/main.log" 2>&1; echo $?; }
status_code() { /bin/bash "$CMD_MAIN" status >/dev/null 2>&1; echo $?; }
replace_target_with_new() {
    rm -rf "$S_TARGET"
    mkdir -p "$S_TARGET"
    tar -xzf "$NEW_APPTGZ" -C "$S_TARGET" || die "替换新版本代码失败"
    CMD_MAIN="$NEWCMD/main"
}

# ── 3. 场景 1：正常升级 ─────────────────────────────────────────────────────
echo
echo "=== 场景 1：正常升级 $OLD_VERSION → $NEW_VERSION ==="
PORT1="$(scenario_port)"
scenario_env "$SANDBOX/s1" "$PORT1" "$OLD_FPK"
# 预置用户配置：升级后必须原样保留
cat > "$S_HOME/config.json" <<'JSON'
{"receiver":{"name":"upgrade-marker"}}
JSON
sim_eq "0" "$(mrun /bin/bash "$CMD_MAIN" stop)" "旧包安装前 stop 幂等返回 0"
sim_eq "0" "$(mrun /bin/bash "$S_WORK/cmd/install_init")" "旧包 install_init 返回 0"
sim_eq "0" "$(mrun /bin/bash "$S_WORK/cmd/install_callback")" "旧包 install_callback 返回 0"
sim_eq "0" "$(mrun /bin/bash "$CMD_MAIN" start)" "旧包 main start 返回 0"
sim_wait_health "$PORT1" || true
OLD_PID="$(sim_pid_of "$S_VAR/app.pid")"
sim_alive "$OLD_PID" "升级前旧版本服务在运行（pid=${OLD_PID:-空}）"
sim_eq "$OLD_VERSION" "$(sim_health_version "$PORT1")" "升级前 /healthz 报旧版本 $OLD_VERSION"
sim_matches 'upgrade-marker' "$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT1/api/config" 2>/dev/null)" "升级前用户配置已生效（receiver.name=upgrade-marker）"
sim_eq "1" "$(sim_pids_count "$S_TARGET/bin/fusionplay-fnos")" "升级前只有 1 个进程"

# fnOS 升级：替换 target 代码（cmd 脚本用新包的）
replace_target_with_new
NEW_BIN_HASH="$(sim_file_hash "$NEW_BIN")"
sim_eq "$NEW_BIN_HASH" "$(sim_file_hash "$S_TARGET/bin/fusionplay-fnos")" "替换后的 target 二进制 = 新版本二进制"
sim_eq "$NEW_VERSION" "$("$S_TARGET/bin/fusionplay-fnos" --version 2>/dev/null | head -1 | tr -d '[:space:]')" "target 内二进制 --version = $NEW_VERSION"

echo "▶ upgrade_init（升级前必须停服务）"
sim_eq "0" "$(mrun /bin/bash "$NEWCMD/upgrade_init")" "upgrade_init 返回 0"
sim_eq "0" "$(sim_port_listeners "$PORT1")" "upgrade_init 后端口已释放（0 个监听）"
sim_eq "0" "$(sim_pids_count "$S_TARGET/bin/fusionplay-fnos")" "upgrade_init 后无残留旧进程"
sim_true "upgrade_init 后 PID 文件已删除" test ! -f "$S_VAR/app.pid"
sim_true "upgrade_init 后 socket 已删除" test ! -S "$S_TARGET/fusionplay-fnos.sock"
sim_wait_health_down "$PORT1" || true
sim_ne "200" "$(sim_http_code "http://127.0.0.1:$PORT1/healthz")" "upgrade_init 后 /healthz 不再应答"
sim_true "旧进程确实已退出（pid=${OLD_PID:-?}）" bash -c "! kill -0 ${OLD_PID:-0} 2>/dev/null"

echo "▶ upgrade_callback（升级后必须重新拉起）"
sim_eq "0" "$(mrun /bin/bash "$NEWCMD/upgrade_callback")" "upgrade_callback 返回 0"
sim_wait_health "$PORT1" || true
NEW_PID="$(sim_pid_of "$S_VAR/app.pid")"
sim_alive "$NEW_PID" "upgrade_callback 后新版本服务在运行（pid=${NEW_PID:-空}）"
sim_eq "0" "$(status_code)" "新版本 status = 0"
sim_eq "$NEW_VERSION" "$(sim_health_version "$PORT1")" "upgrade_callback 后 /healthz 报【新】版本 $NEW_VERSION"
sim_ne "$OLD_PID" "$NEW_PID" "服务进程已更换（PID 变化）"
sim_eq "$NEW_BIN_HASH" "$(sim_proc_exe_hash "$NEW_PID")" "正在运行的确实是新版本二进制（/proc/pid/exe 哈希一致）"
sim_eq "$S_HOME" "$(sim_proc_env "$NEW_PID" HOME)" "升级后 HOME 仍指向 TRIM_PKGHOME"
sim_matches 'upgrade-marker' "$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT1/api/config" 2>/dev/null)" "升级后用户配置保留（receiver.name=upgrade-marker）"
sim_eq "1" "$(sim_pids_count "$S_TARGET/bin/fusionplay-fnos")" "升级后只有 1 个进程（无旧进程残留）"
sim_true "升级后 socket 重新建立" test -S "$S_TARGET/fusionplay-fnos.sock"
sim_eq "0" "$(mrun /bin/bash "$CMD_MAIN" stop)" "场景 1 收尾 stop 返回 0"
sim_eq "3" "$(status_code)" "场景 1 收尾 status = 3"

# ── 4. 场景 2：孤儿进程（PID 文件丢失）升级 ─────────────────────────────────
echo
echo "=== 场景 2：PID 文件丢失的孤儿进程升级 ==="
PORT2="$(scenario_port)"
scenario_env "$SANDBOX/s2" "$PORT2" "$OLD_FPK"
sim_eq "0" "$(mrun /bin/bash "$S_WORK/cmd/install_init")" "场景 2 install_init 返回 0"
sim_eq "0" "$(mrun /bin/bash "$S_WORK/cmd/install_callback")" "场景 2 install_callback 返回 0"
sim_eq "0" "$(mrun /bin/bash "$CMD_MAIN" start)" "场景 2 main start 返回 0"
sim_wait_health "$PORT2" || true
ORPHAN_PID="$(sim_pid_of "$S_VAR/app.pid")"
sim_alive "$ORPHAN_PID" "场景 2 旧进程在运行（pid=${ORPHAN_PID:-空}）"
sim_eq "$OLD_VERSION" "$(sim_health_version "$PORT2")" "场景 2 升级前 /healthz 报旧版本"
rm -f "$S_VAR/app.pid"
sim_true "已删除 PID 文件（模拟升级残留的孤儿进程）" test ! -f "$S_VAR/app.pid"
sim_alive "$ORPHAN_PID" "旧进程仍活着并占着端口"
replace_target_with_new
sim_eq "0" "$(mrun /bin/bash "$NEWCMD/upgrade_init")" "场景 2 upgrade_init 返回 0（不依赖 PID 文件）"
sim_true "upgrade_init 清掉了无 PID 文件的孤儿进程（pid=${ORPHAN_PID:-?}）" bash -c "! kill -0 ${ORPHAN_PID:-0} 2>/dev/null"
sim_eq "0" "$(sim_port_listeners "$PORT2")" "场景 2 upgrade_init 后端口已释放"
sim_eq "0" "$(mrun /bin/bash "$NEWCMD/upgrade_callback")" "场景 2 upgrade_callback 返回 0"
sim_wait_health "$PORT2" || true
sim_eq "$NEW_VERSION" "$(sim_health_version "$PORT2")" "场景 2 升级后 /healthz 报新版本 $NEW_VERSION"
sim_eq "0" "$(status_code)" "场景 2 升级后 status = 0"
sim_eq "1" "$(sim_pids_count "$S_TARGET/bin/fusionplay-fnos")" "场景 2 升级后只有 1 个进程"
sim_eq "0" "$(mrun /bin/bash "$CMD_MAIN" stop)" "场景 2 收尾 stop 返回 0"
sim_eq "3" "$(status_code)" "场景 2 收尾 status = 3"

echo
sim_summary || {
    echo "提示：构建/服务日志在 $SANDBOX 下（build-new.log / main.log / app.log）"
    exit 1
}
echo "✅ 升级路径（正常 + 孤儿进程）全部通过"
