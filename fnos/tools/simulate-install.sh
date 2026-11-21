#!/bin/bash
# simulate-install.sh — 在没有 fnOS 应用中心的情况下，用一个临时目录完整模拟
# 安装 → 启动 → 健康检查 → 配置重载 → 停止 → 卸载 的生命周期。
#
# 用法：
#   ./fnos/tools/simulate-install.sh [app.fpk]
#   SIM_PORT=18999 ./fnos/tools/simulate-install.sh      # 指定端口（默认随机空闲端口）
#   FUSIONPLAY_AUDIO_BACKEND=alsa ./fnos/tools/simulate-install.sh   # 真机声卡联调时用
#
# 它验证的是「包里的脚本 + 二进制」组合是否能在 fnOS 的目录/环境变量模型下跑通：
#   install_init → install_callback → main start → status=0 → /healthz（版本）
#   → HOME/DATA_LIBRARY_PATH → config_init/callback → stop → status=3 → uninstall_*
#
# 安全：#1 只用 mktemp 沙箱（断言所有 TRIM_* 在沙箱内）；
#       #2 健康检查用随机端口且 != manifest.service_port（不会打到真机线上实例）；
#       #3 清理只按沙箱内二进制精确路径（不 pkill 其它进程）。
set -uo pipefail

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SELF_DIR/.." && pwd)"
# shellcheck source=fpk-sim-lib.sh
source "$SELF_DIR/fpk-sim-lib.sh"

FPK="${1:-$ROOT/dist/fusionplay-fnos.fpk}"
sim_require_tools
[ -f "$FPK" ] || { echo "❌ 找不到 $FPK（先跑 ./fnos/tools/build-fpk.sh）" >&2; exit 1; }
FPK="$(cd "$(dirname "$FPK")" && pwd)/$(basename "$FPK")"

SANDBOX="$(sim_sandbox)"
WORK="$SANDBOX/work"
TARGET="$SANDBOX/target"
VAR="$SANDBOX/var"
PKGHOME="$SANDBOX/home"
ETC="$SANDBOX/etc"
TMP_SIM="$SANDBOX/tmp"
mkdir -p "$WORK" "$TARGET" "$VAR" "$PKGHOME" "$ETC" "$TMP_SIM"
CMD_MAIN=""

cleanup() {
    set +u
    if [ -n "${CMD_MAIN:-}" ] && [ -f "$CMD_MAIN" ]; then
        /bin/bash "$CMD_MAIN" stop >/dev/null 2>&1 || true   # 必须带当前 TRIM_* 环境变量
    fi
    sim_cleanup_sandbox_processes "$SANDBOX"
    if [ "${SIM_FAIL:-0}" -gt 0 ]; then
        echo "ℹ️  测试失败，沙箱保留便于排查：$SANDBOX"
    else
        rm -rf "$SANDBOX"
    fi
}
trap cleanup EXIT INT TERM

echo "▶ 解包 FPK：$FPK"
tar -xzf "$FPK" -C "$WORK" || { echo "❌ FPK 解包失败"; exit 1; }
[ -f "$WORK/manifest" ] || { echo "❌ FPK 缺 manifest"; exit 1; }
[ -f "$WORK/app.tgz" ] || { echo "❌ FPK 缺 app.tgz"; exit 1; }
tar -xzf "$WORK/app.tgz" -C "$TARGET" || { echo "❌ app.tgz 解包失败"; exit 1; }
CMD_MAIN="$WORK/cmd/main"

APPNAME="$(sim_manifest_get "$WORK/manifest" appname)"
VERSION="$(sim_manifest_get "$WORK/manifest" version)"
DECLARED_PORT="$(sim_manifest_get "$WORK/manifest" service_port)"
PORT="${SIM_PORT:-$(sim_pick_port "$DECLARED_PORT")}"
BIN="$TARGET/bin/fusionplay-fnos"

echo "▶ 应用：$APPNAME $VERSION；测试端口：$PORT（manifest 声明 $DECLARED_PORT）"
sim_ne "$DECLARED_PORT" "$PORT" "测试端口不等于 manifest.service_port（避免打到线上实例）"

unset DATA_LIBRARY_PATH   # fnOS 不注入该变量；清掉宿主环境的同名变量，才能测到真实兜底

export TRIM_APPDEST="$TARGET"
export TRIM_PKGVAR="$VAR"
export TRIM_PKGHOME="$PKGHOME"
export TRIM_PKGETC="$ETC"
export TRIM_PKGTMP="$TMP_SIM"
export TRIM_SERVICE_PORT="$PORT"
export TRIM_TEMP_LOGFILE="$VAR/install-error.log"
export TRIM_APPNAME="$APPNAME"
export TRIM_APPVER="$VERSION"
export FUSIONPLAY_AUDIO_BACKEND="${FUSIONPLAY_AUDIO_BACKEND:-null}"   # 默认 null：CI/构建机没有声卡也应可测
# 最后一道保险：所有路径必须落在本次沙箱内
for pair in "TRIM_APPDEST=$TRIM_APPDEST" "TRIM_PKGVAR=$TRIM_PKGVAR" \
            "TRIM_PKGHOME=$TRIM_PKGHOME" "TRIM_PKGETC=$TRIM_PKGETC" \
            "TRIM_PKGTMP=$TRIM_PKGTMP" "TRIM_TEMP_LOGFILE=$TRIM_TEMP_LOGFILE"; do
    sim_require_under "${pair%%=*}" "${pair#*=}" "$SANDBOX"
done

run_main() { timeout 120 /bin/bash "$CMD_MAIN" "$@" >> "$SANDBOX/main.log" 2>&1; }
status_code() { /bin/bash "$CMD_MAIN" status >/dev/null 2>&1; echo $?; }

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "=== 1. 包内结构 / 版本一致性 ==="
sim_eq "fusionplay-fnos" "$APPNAME" "appname = fusionplay-fnos"
sim_nonempty "$VERSION" "manifest.version 非空（$VERSION）"
sim_true "cmd/main 存在且可执行" test -x "$CMD_MAIN"
BIN_VERSION="$("$BIN" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
sim_eq "$VERSION" "${BIN_VERSION:-<无法执行>}" "包内二进制 --version 与 manifest.version 一致"
UI_SOCK="$(python3 - "$TARGET/ui/config" <<'PY'
import json,sys
try:
    cfg=json.load(open(sys.argv[1], encoding="utf-8"))
    print(list(cfg.get(".url",{}).values())[0].get("gatewaySocket",""))
except Exception:
    print("")
PY
)"
sim_eq "$APPNAME.sock" "$UI_SOCK" "ui/config gatewaySocket 与 cmd/main 的 socket 名一致"

echo
echo "=== 2. 安装前状态 ==="
sim_eq "3" "$(status_code)" "未安装/未启动时 status = 3（fnOS 约定）"
run_main stop; sim_eq "0" "$?" "未启动时 stop 幂等返回 0"

echo
echo "=== 3. 安装生命周期（install_init → install_callback → main start）==="
timeout 120 /bin/bash "$WORK/cmd/install_init" > "$SANDBOX/install_init.log" 2>&1
sim_eq "0" "$?" "install_init 返回 0"
timeout 120 /bin/bash "$WORK/cmd/install_callback" > "$SANDBOX/install_callback.log" 2>&1
sim_eq "0" "$?" "install_callback 返回 0"
sim_eq "3" "$(status_code)" "启用前服务保持停止"
sim_eq "0" "$(sim_port_listeners "$PORT")" "安装回调为系统的端口预检保留空闲端口"
run_main start; sim_eq "0" "$?" "main start 返回 0（启动成功判据 = /healthz 通过）"

echo
echo "=== 4. 运行状态 / 健康检查 ==="
PID="$(sim_pid_of "$VAR/app.pid")"
sim_alive "$PID" "PID 文件记录的服务进程存活（pid=${PID:-空}）"
sim_eq "0" "$(status_code)" "status = 0 表示运行中"
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/healthz")" "/healthz 返回 200"
sim_eq "ok" "$(sim_health_json "$PORT" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null)" "/healthz status = ok"
sim_eq "$VERSION" "$(sim_health_version "$PORT")" "/healthz 报出的版本 = 包版本（升级/回滚一眼可查）"
sim_eq "1" "$(sim_pids_count "$BIN")" "只有 1 个本应用进程"

echo
echo "=== 5. 环境变量注入（HOME / DATA_LIBRARY_PATH）==="
sim_eq "$PKGHOME" "$(sim_proc_env "$PID" HOME)" "服务进程 HOME 指向 TRIM_PKGHOME（不是 /root）"
DATA_PATH="$(sim_proc_env "$PID" DATA_LIBRARY_PATH)"
sim_nonempty "$DATA_PATH" "服务进程 DATA_LIBRARY_PATH 非空（=$DATA_PATH）"
sim_eq "$VAR" "$DATA_PATH" "DATA_LIBRARY_PATH 兜底为 TRIM_PKGVAR"
sim_eq "$PORT" "$(sim_proc_env "$PID" TRIM_SERVICE_PORT)" "服务进程 TRIM_SERVICE_PORT 与测试端口一致"

echo
echo "=== 6. 配置文件 / 日志落盘 ==="
sim_true "首次启动生成 config.json（$PKGHOME/config.json）" test -s "$PKGHOME/config.json"
sim_true "config.json 是合法 JSON" python3 -m json.tool "$PKGHOME/config.json"
sim_true "文件日志 fusionplay.log 已生成" test -s "$VAR/fusionplay.log"
sim_true "运行日志 app.log 已生成" test -s "$VAR/app.log"

echo
echo "=== 7. HTTP API / 静态 UI ==="
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/api/status")" "/api/status 返回 200"
sim_eq "$VERSION" "$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/status" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version",""))' 2>/dev/null)" "/api/status 报版本与包一致"
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/api/diagnostics")" "/api/diagnostics 返回 200"
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/")" "首页 / 返回 200"
sim_true "首页 HTML 内容正常" bash -c "curl -fsS --max-time 2 'http://127.0.0.1:$PORT/' | grep -qi '<html'"
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/js/main.js")" "静态资源 /js/main.js 返回 200"
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/style.css")" "静态资源 /style.css 返回 200"

echo
echo "=== 8. fnOS 统一网关前缀 ==="
sim_eq "200" "$(sim_http_code "http://127.0.0.1:$PORT/app/fusionplay-fnos/healthz")" "/app/<app>/healthz 返回 200（后端正确剥离网关前缀）"
BARE_CODE="$(sim_http_code "http://127.0.0.1:$PORT/app/fusionplay-fnos")"
sim_matches '^3[0-9][0-9]$' "$BARE_CODE" "裸前缀 /app/fusionplay-fnos 返回 3xx（实际 $BARE_CODE）"
LOCATION="$(curl -s -o /dev/null -D - --max-time 2 "http://127.0.0.1:$PORT/app/fusionplay-fnos" 2>/dev/null | tr -d '\r' | sed -n 's/^[Ll]ocation: //p' | head -1)"
sim_eq "/app/fusionplay-fnos/" "$LOCATION" "裸前缀跳转 Location 指向带斜杠路径"

echo
echo "=== 9. Unix 域套接字（网关实际接法）==="
SOCK="$TARGET/fusionplay-fnos.sock"
sim_true "socket 文件存在（$SOCK）" test -S "$SOCK"
sim_eq "666" "$(stat -c %a "$SOCK" 2>/dev/null)" "socket 权限 0666（网关进程可连）"
sim_eq "200" "$(sim_http_code --unix-socket "$SOCK" http://localhost/healthz)" "unix socket /healthz 返回 200"
sim_eq "$VERSION" "$(curl -fsS --max-time 2 --unix-socket "$SOCK" http://localhost/healthz 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version",""))' 2>/dev/null)" "unix socket 报出版本"

echo
echo "=== 10. 幂等性（install_callback 重复执行）==="
OLD_PID="$PID"
timeout 120 /bin/bash "$WORK/cmd/install_callback" >> "$SANDBOX/install_callback-2.log" 2>&1
sim_eq "0" "$?" "install_callback 重复执行返回 0"
sim_eq "$OLD_PID" "$(sim_pid_of "$VAR/app.pid")" "重复 install_callback 不重启进程（PID 不变）"
sim_eq "1" "$(sim_pids_count "$BIN")" "重复执行后仍只有 1 个进程"
sim_eq "0" "$(status_code)" "重复执行后 status 仍为 0"

echo
echo "=== 11. 配置变更生命周期（config_init → config_callback 重启）==="
OLD_PID="$(sim_pid_of "$VAR/app.pid")"
timeout 120 /bin/bash "$WORK/cmd/config_init" > "$SANDBOX/config_init.log" 2>&1
sim_eq "0" "$?" "config_init 返回 0"
timeout 120 /bin/bash "$WORK/cmd/config_callback" > "$SANDBOX/config_callback.log" 2>&1
sim_eq "0" "$?" "config_callback 返回 0"
sim_wait_health "$PORT" || true
NEW_PID="$(sim_pid_of "$VAR/app.pid")"
sim_alive "$NEW_PID" "config_callback 后服务在运行（pid=${NEW_PID:-空}）"
sim_ne "$OLD_PID" "$NEW_PID" "config_callback 触发重启（PID 变化）"
sim_eq "$VERSION" "$(sim_health_version "$PORT")" "重启后 /healthz 版本不变"

echo
echo "=== 12. 停止 / 状态 ==="
run_main stop; sim_eq "0" "$?" "main stop 返回 0"
sim_eq "3" "$(status_code)" "停止后 status = 3"
sim_true "PID 文件已删除" test ! -f "$VAR/app.pid"
sim_true "socket 已删除" test ! -S "$SOCK"
sim_eq "0" "$(sim_port_listeners "$PORT")" "端口已释放（0 个监听）"
sim_eq "0" "$(sim_pids_count "$BIN")" "无残留服务进程"
run_main stop; sim_eq "0" "$?" "重复 stop 幂等返回 0"
sim_ne "200" "$(sim_http_code "http://127.0.0.1:$PORT/healthz")" "停止后 /healthz 不再返回 200"

echo
echo "=== 13. 卸载生命周期（uninstall_init → uninstall_callback）==="
timeout 120 /bin/bash "$WORK/cmd/uninstall_init" > "$SANDBOX/uninstall_init.log" 2>&1
sim_eq "0" "$?" "uninstall_init 返回 0"
sim_eq "3" "$(status_code)" "卸载前 stop 后 status = 3"
sim_eq "0" "$(sim_pids_count "$BIN")" "卸载流程后无残留服务进程"
sim_true "卸载流程后无残留 PID 文件" test ! -f "$VAR/app.pid"
sim_true "卸载流程后无残留 socket" test ! -S "$SOCK"
timeout 120 /bin/bash "$WORK/cmd/uninstall_callback" > "$SANDBOX/uninstall_callback.log" 2>&1
sim_eq "0" "$?" "uninstall_callback 返回 0"
sim_true "卸载后保留运行数据（日志）$VAR/fusionplay.log" test -s "$VAR/fusionplay.log"
sim_true "卸载后保留用户配置 $PKGHOME/config.json" test -s "$PKGHOME/config.json"

echo
echo "=== 14. main 非法参数 ==="
/bin/bash "$CMD_MAIN" bogus >/dev/null 2>&1
if [ "$?" -ne 0 ]; then sim_ok "main 未知子命令返回非 0"; else sim_bad "main 未知子命令应返回非 0"; fi

echo
sim_summary || {
    echo "提示：服务日志 $VAR/app.log、$VAR/fusionplay.log"
    exit 1
}
echo "✅ 模拟安装/启动/停止/卸载全部通过"
