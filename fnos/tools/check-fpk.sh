#!/bin/bash
# FPK 交付物结构 + 规范检查（CI 与本地都用）。失败即非 0 退出。
#
# 覆盖（对应总纲 §2 manifest 字段表 / §2.6 privilege / §3 桌面入口 / §4 打包 / 附录 D）：
#   - 必填文件与目录：manifest / cmd 9 脚本（全部可执行）/ config/{privilege,resource} /
#     ICON.PNG / ICON_256.PNG / app.tgz(bin+www+ui+licenses)
#   - manifest 官方字段名（拒绝 arch/description/developer/port 等不存在的字段）、必填、取值
#   - manifest.version == app/BUILD-INFO version == 二进制 --version
#   - service_port == cmd/main 默认端口；desktop_uidir / desktop_applaunchname 与 ui/config 对齐
#   - privilege：合法 JSON、run-as=package（非 root）、join-groups 含 audio
#   - AGPL LICENSE 文本存在
#   - 图标尺寸 64/256
set -uo pipefail
umask 022

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
FPK="${1:-$SELF_DIR/../dist/fusionplay-fnos.fpk}"
EXPECTED_APPNAME="fusionplay-fnos"
EXPECTED_PORT="18080"
EXPECTED_UIDIR="ui"
EXPECTED_LAUNCH="fusionplay-fnos.main"
EXPECTED_PLATFORM="$(uname -m | sed -e 's/x86_64/x86/' -e 's/aarch64/arm/')"

[ -f "$FPK" ] || { echo "❌ 找不到 $FPK"; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail=0
ok()  { printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1"; fail=1; }
check_file() { [ -e "$1" ] && ok "$2" || bad "$2"; }
check_exec() { [ -x "$1" ] && ok "$2" || bad "$2"; }

echo "检查 $FPK"
echo "▶ 解包"
tar -xzf "$FPK" -C "$TMP" || { echo "❌ 不是合法 tar.gz 包"; exit 1; }
[ -f "$TMP/manifest" ] || { echo "❌ 缺 manifest"; exit 1; }
[ -f "$TMP/app.tgz" ] || { echo "❌ 缺 app.tgz"; exit 1; }
mkdir -p "$TMP/app"
tar -xzf "$TMP/app.tgz" -C "$TMP/app" || { echo "❌ app.tgz 解包失败"; exit 1; }

echo "▶ 顶层结构"
check_file "$TMP/manifest" "含 manifest"
check_file "$TMP/app.tgz" "含 app.tgz（app 目录）"
check_file "$TMP/config/privilege" "含 config/privilege"
check_file "$TMP/config/resource" "含 config/resource"
check_file "$TMP/ICON.PNG" "含 ICON.PNG"
check_file "$TMP/ICON_256.PNG" "含 ICON_256.PNG"
for script in install_init install_callback upgrade_init upgrade_callback \
              uninstall_init uninstall_callback config_init config_callback main; do
    check_file "$TMP/cmd/$script" "含 cmd/$script"
    check_exec "$TMP/cmd/$script" "cmd/$script 可执行"
done

echo "▶ app.tgz 内容"
check_exec "$TMP/app/bin/fusionplay-fnos" "含可执行 bin/fusionplay-fnos"
check_file "$TMP/app/www/player/main.dart.js" "共享 Flutter 播放器"
check_file "$TMP/app/www/player/fnos-mark.png" "播放器右下角的飞牛标识"
check_file "$TMP/app/www/player/canvaskit/canvaskit.wasm" "离线 CanvasKit"
for asset in speakers.html speakers.css js/speakers.js js/curve.js js/glass.js; do
    check_file "$TMP/app/www/$asset" "含音箱设置资源 $asset"
done
check_file "$TMP/app/www/index.html" "含 www/index.html"
check_file "$TMP/app/www/js/main.js" "含 www/js/main.js"
check_file "$TMP/app/www/js/core.js" "含 www/js/core.js"
check_file "$TMP/app/www/js/page-audio.js" "含 www/js/page-audio.js"
check_file "$TMP/app/www/style.css" "含 www/style.css"
check_file "$TMP/app/ui/config" "含 ui/config"
check_file "$TMP/app/ui/images/icon_64.png" "含 ui/images/icon_64.png"
check_file "$TMP/app/ui/images/icon_256.png" "含 ui/images/icon_256.png"
check_file "$TMP/app/licenses/LICENSE" "含 licenses/LICENSE"
check_file "$TMP/app/licenses/SOURCE.txt" "含 licenses/SOURCE.txt（AGPL 源码地址）"
check_file "$TMP/app/BUILD-INFO" "含 BUILD-INFO（版本/commit）"

echo "▶ manifest / privilege / resource / ui / LICENSE / BUILD-INFO 规范"
python3 - "$TMP" "$EXPECTED_APPNAME" "$EXPECTED_PORT" "$EXPECTED_UIDIR" "$EXPECTED_LAUNCH" "$EXPECTED_PLATFORM" <<'PY'
import json, re, struct, sys
from pathlib import Path

tmp = Path(sys.argv[1])
exp_app, exp_port, exp_uidir, exp_launch, exp_platform = sys.argv[2:7]
fails = []

def ok(msg): print(f"  ok   {msg}")
def bad(msg):
    print(f"  FAIL {msg}"); fails.append(msg)

def load_ini(path):
    m = {}
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw.strip()
        if not line or line.startswith(("#", ";")) or "=" not in line:
            continue
        k, v = line.split("=", 1)
        v = v.strip()
        if len(v) >= 2 and v[0] == v[-1] == '"':
            v = v[1:-1]
        m[k.strip()] = v
    return m

def png_size(path):
    data = path.read_bytes()[:24]
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    w, h = struct.unpack(">II", data[16:24])
    return (w, h)

man = load_ini(tmp / "manifest")
required = ["appname", "version", "display_name", "desc", "source", "platform",
            "maintainer", "maintainer_url", "service_port", "desktop_uidir",
            "desktop_applaunchname"]
for field in required:
    if man.get(field, "").strip():
        ok(f"manifest 含必填字段 {field}")
    else:
        bad(f"manifest 缺必填字段 {field}")

forbidden = ["arch", "description", "developer", "port", "icon", "icon256", "main", "category"]
for field in forbidden:
    if field in man:
        bad(f"manifest 使用了官方不存在的字段 {field}（规范字段见总纲 §2.2）")
    else:
        ok(f"manifest 无非法字段 {field}")

if man.get("appname") == exp_app:
    ok(f"appname = {exp_app}")
else:
    bad(f"appname 应为 {exp_app}，实际 {man.get('appname')!r}")

version = man.get("version", "")
if re.fullmatch(r"\d+\.\d+\.\d+(-[0-9]+)?", version):
    ok(f"version 格式合法：{version}")
else:
    bad(f"version 非法（只允许 x.y.z / x.y.z-r）：{version!r}")

if man.get("source") == "thirdparty":
    ok("source = thirdparty")
else:
    bad(f"source 必须是 thirdparty，实际 {man.get('source')!r}")

platform = man.get("platform", "")
if platform in ("x86", "arm", "all"):
    ok(f"platform 取值合法：{platform}")
else:
    bad(f"platform 取值非法（x86/arm/all）：{platform!r}")
bin_path = tmp / "app" / "bin" / "fusionplay-fnos"
is_elf = bin_path.is_file() and bin_path.read_bytes()[:4] == b"\x7fELF"
if is_elf and platform == "all":
    bad("包含架构特定的 ELF 二进制时 platform 不能写 all（装到别的架构会直接崩）")
elif is_elf and platform != exp_platform:
    bad(f"platform={platform} 与检测到的构建架构 {exp_platform} 不一致")
else:
    ok("platform 与二进制架构匹配")

if man.get("service_port") == exp_port:
    ok(f"service_port = {exp_port}")
else:
    bad(f"service_port 应为 {exp_port}，实际 {man.get('service_port')!r}")

if man.get("desktop_uidir") == exp_uidir:
    ok(f"desktop_uidir = {exp_uidir}")
else:
    bad(f"desktop_uidir 应为 {exp_uidir}")
if man.get("desktop_applaunchname") == exp_launch:
    ok(f"desktop_applaunchname = {exp_launch}")
else:
    bad(f"desktop_applaunchname 应为 {exp_launch}")

# ---- ui/config ----
try:
    ui = json.loads((tmp / "app/ui/config").read_text(encoding="utf-8"))
    entry = ui.get(".url", {}).get(exp_launch)
    if not entry:
        bad(f"ui/config 缺少入口 {exp_launch}")
    else:
        ok(f"ui/config 含入口 {exp_launch}")
        if entry.get("type") == "iframe":
            ok("ui/config 入口 type = iframe")
        else:
            bad(f"ui/config 入口 type 应为 iframe，实际 {entry.get('type')!r}")
        if entry.get("gatewayPrefix") == f"/app/{exp_app}":
            ok(f"gatewayPrefix = /app/{exp_app}")
        else:
            bad(f"gatewayPrefix 应为 /app/{exp_app}，实际 {entry.get('gatewayPrefix')!r}")
        if entry.get("gatewaySocket") == f"{exp_app}.sock":
            ok(f"gatewaySocket = {exp_app}.sock")
        else:
            bad(f"gatewaySocket 应为 {exp_app}.sock，实际 {entry.get('gatewaySocket')!r}")
        url = str(entry.get("url", ""))
        if url == f"/app/{exp_app}" or url.startswith(f"/app/{exp_app}/"):
            ok(f"ui/config url 在网关前缀下：{url}")
        else:
            bad(f"ui/config url 应以 /app/{exp_app} 开头，实际 {url!r}")
        if entry.get("icon") == "images/icon_{0}.png":
            ok("ui/config icon 使用 images/icon_{0}.png 占位符")
        else:
            bad(f"ui/config icon 应写 images/icon_{{0}}.png，实际 {entry.get('icon')!r}")
        for k, v in entry.items():
            if k == "icon":
                continue
            if "${" in str(v) or ("{" in str(v) and "}" in str(v)):
                bad(f"ui/config 字段 {k} 含 {v!r}：只认 ${{...}} 占位符，裸花括号会原样进 iframe 地址")
        for img in ("images/icon_64.png", "images/icon_256.png"):
            if (tmp / "app" / "ui" / img).is_file():
                ok(f"ui/config 引用图标存在：{img}")
            else:
                bad(f"ui/config 引用图标缺失：{img}")
except Exception as exc:  # noqa: BLE001
    bad(f"ui/config 不是合法 JSON：{exc}")

# ---- privilege ----
try:
    priv = json.loads((tmp / "config/privilege").read_text(encoding="utf-8"))
    ok("config/privilege 是合法 JSON")
    if priv.get("defaults", {}).get("run-as") == "package":
        ok("privilege run-as = package（非 root 常驻）")
    else:
        bad(f"privilege defaults.run-as 必须为 package（当前 {priv.get('defaults', {}).get('run-as')!r}）")
    if priv.get("username") == exp_app and priv.get("groupname") == exp_app:
        ok("privilege username/groupname 与 appname 一致")
    else:
        bad("privilege username/groupname 必须等于 appname")
    groups = [str(g).lower() for g in priv.get("join-groups", [])]
    if "audio" in groups:
        ok("privilege join-groups 含 audio（/dev/snd 访问，保持非 root）")
    else:
        bad("privilege 缺 join-groups audio（否则没有声卡权限）")
except Exception as exc:  # noqa: BLE001
    bad(f"config/privilege 不是合法 JSON：{exc}")

# ---- resource ----
try:
    res = json.loads((tmp / "config/resource").read_text(encoding="utf-8"))
    if isinstance(res, dict):
        ok("config/resource 是合法 JSON 对象")
    else:
        bad("config/resource 必须是 JSON 对象")
except Exception as exc:  # noqa: BLE001
    bad(f"config/resource 不是合法 JSON：{exc}")

# ---- BUILD-INFO ----
try:
    info = {}
    for line in (tmp / "app/BUILD-INFO").read_text(encoding="utf-8").splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            info[k.strip()] = v.strip()
    if info.get("version") == version:
        ok(f"BUILD-INFO version 与 manifest 一致：{version}")
    else:
        bad(f"BUILD-INFO version={info.get('version')!r} 与 manifest={version!r} 不一致")
    if info.get("commit"):
        ok(f"BUILD-INFO 记录 commit：{info['commit']}")
    else:
        bad("BUILD-INFO 缺 commit")
    if re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", info.get("built", "")):
        ok(f"BUILD-INFO 记录构建时间：{info['built']}")
    else:
        bad(f"BUILD-INFO built 时间格式非法：{info.get('built')!r}")
except Exception as exc:  # noqa: BLE001
    bad(f"BUILD-INFO 不可读：{exc}")

# ---- LICENSE（AGPL） ----
try:
    lic = (tmp / "app/licenses/LICENSE").read_text(encoding="utf-8", errors="replace")
    if "GNU AFFERO GENERAL PUBLIC LICENSE" in lic and "Version 3" in lic:
        ok("licenses/LICENSE 是 AGPL-3.0 文本")
    else:
        bad("licenses/LICENSE 不是 AGPL-3.0 文本")
except Exception as exc:  # noqa: BLE001
    bad(f"licenses/LICENSE 不可读：{exc}")

# ---- 图标尺寸 ----
for name, want in (("ICON.PNG", (64, 64)), ("ICON_256.PNG", (256, 256))):
    try:
        size = png_size(tmp / name)
        if size == want:
            ok(f"{name} 尺寸 {size[0]}x{size[1]}")
        else:
            bad(f"{name} 尺寸应为 {want[0]}x{want[1]}，实际 {size}")
    except Exception as exc:  # noqa: BLE001
        bad(f"{name} 不是合法 PNG：{exc}")

# The package account can differ from the account that extracted the archive.
for entry in (tmp / "app").rglob("*"):
    mode = entry.stat().st_mode
    required = 0o5 if entry.is_dir() or entry == tmp / "app/bin/fusionplay-fnos" else 0o4
    if mode & required != required:
        bad(f"包用户无法访问 {entry.relative_to(tmp)}，权限为 {mode & 0o777:o}")
for entry in (tmp / "cmd").iterdir():
    if entry.stat().st_mode & 0o5 != 0o5:
        bad(f"包用户无法执行 {entry.relative_to(tmp)}")

# ---- cmd/main 静态约定 ----
try:
    main_txt = (tmp / "cmd/main").read_text(encoding="utf-8", errors="replace")
    m = re.search(r"TRIM_SERVICE_PORT:-(\d+)", main_txt)
    if m and m.group(1) == exp_port:
        ok(f"cmd/main 默认端口与 service_port 一致：{exp_port}")
    else:
        bad(f"cmd/main 默认端口应等于 service_port={exp_port}，实际 {m.group(1) if m else '未找到'}")
    if re.search(r"/healthz", main_txt):
        ok("cmd/main 使用 /healthz 作为启动成功判据")
    else:
        bad("cmd/main 未使用 /healthz 健康检查")
    if re.search(r"exit 3", main_txt):
        ok("cmd/main status 返回 3 表示未运行")
    else:
        bad("cmd/main status 未返回 3（fnOS 要求 0/3）")
    if f"{exp_app}.sock" in main_txt:
        ok(f"cmd/main socket 文件名与 gatewaySocket 一致：{exp_app}.sock")
    else:
        bad(f"cmd/main 未使用 {exp_app}.sock（与 ui/config gatewaySocket 不一致）")
    if "/cmd/main" in main_txt:
        bad("cmd/main 仍引用 .../cmd/main 路径（真机 cmd 在 /var/apps/<app>/cmd，TRIM_APPDEST 下没有 cmd/）")
    else:
        ok("cmd 脚本不依赖 $TRIM_APPDEST/cmd/main（真机 cmd 在 /var/apps/<app>/cmd）")
except Exception as exc:  # noqa: BLE001
    bad(f"cmd/main 不可读：{exc}")

sys.exit(1 if fails else 0)
PY
[ $? -ne 0 ] && fail=1

echo "▶ 版本三方一致（manifest / BUILD-INFO / 二进制 --version）"
# v0.2.5 supported systems with glibc 2.34. Catch a build runner silently
# raising the ELF requirement before publishing an in-place upgrade.
if ! symbols=$(LC_ALL=C readelf --version-info "$TMP/app/bin/fusionplay-fnos"); then
    bad "无法读取后台 ELF 运行库需求"
else
    glibc=$(printf '%s\n' "$symbols" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -Vu | tail -1)
    if [ -n "$glibc" ] && [ "$(printf '%s\n' GLIBC_2.34 "$glibc" | sort -V | tail -1)" = GLIBC_2.34 ]; then
        ok "后台运行库需求 $glibc 在旧版兼容范围内"
    else
        bad "后台运行库需求 ${glibc:-未知} 超出 0.2.5 发布包基线 GLIBC_2.34"
    fi
fi
MANIFEST_VERSION="$(grep -m1 -E '^[[:space:]]*version[[:space:]]*=' "$TMP/manifest" | sed 's/^[^=]*=//' | tr -d ' "')"
BIN_VERSION="$("$TMP/app/bin/fusionplay-fnos" --version 2>/dev/null | head -1 | tr -d '[:space:]')"
if [ -n "$BIN_VERSION" ] && [ "$BIN_VERSION" = "$MANIFEST_VERSION" ]; then
    ok "二进制 --version = manifest.version = $MANIFEST_VERSION"
else
    bad "二进制 --version=${BIN_VERSION:-<无法执行>} 与 manifest.version=$MANIFEST_VERSION 不一致"
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "❌ FPK 检查失败"
    exit 1
fi
echo "✅ FPK 检查通过"
