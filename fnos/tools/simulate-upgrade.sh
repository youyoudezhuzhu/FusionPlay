#!/bin/bash
# Exercise a distributed FPK and a candidate FPK, with persistent directories
# outside the replaced payload. Supply both paths to run entirely offline.
set -euo pipefail
SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SELF_DIR/fpk-sim-lib.sh"
source "$SELF_DIR/fpk-platform.sh"
sim_require_tools
SANDBOX="$(sim_sandbox)"
PORT_HOLDER=""
OTHER_PID=""
cleanup() {
    if [ -n "$PORT_HOLDER" ]; then kill "$PORT_HOLDER" 2>/dev/null || true; fi
    if [ -n "$OTHER_PID" ]; then kill "$OTHER_PID" 2>/dev/null || true; fi
    sim_cleanup_sandbox_processes "$SANDBOX"
    rm -rf -- "$SANDBOX"
}
trap cleanup EXIT
die() { echo "FAIL: $*" >&2; exit 1; }

same_version_rehearsal=false
case $# in
    0|1)
        NEW="${1:-$SELF_DIR/../dist/$FPK_FILENAME}"
        OLD="$SANDBOX/$FPK_FILENAME"
        tar -xzOf "$NEW" manifest > "$SANDBOX/candidate-manifest"
        candidate_version=$(sim_manifest_get "$SANDBOX/candidate-manifest" version)
        repo=youyoudezhuzhu/FusionPlay
        gh api --paginate "repos/$repo/releases?per_page=100" \
            --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name as $tag | .assets[] | [$tag, .name, (.digest // "-")] | @tsv' \
            > "$SANDBOX/release-assets"
        baseline=$(python3 - "$SANDBOX/release-assets" "$candidate_version" "$FPK_FILENAME" <<'PYBASE'
import re, sys
from pathlib import Path

def version(text):
    return tuple(map(int, text.split(".")))

catalog, candidate, filename = sys.argv[1:]
compatible = []
for line in Path(catalog).read_text().splitlines():
    tag, name, digest = line.split("\t")
    match = re.fullmatch(r"fnos-v(\d+\.\d+\.\d+)", tag)
    if match and name == filename and version(match[1]) <= version(candidate):
        # Prefer an actual upgrade, even if the candidate is already published.
        compatible.append((version(match[1]) < version(candidate), version(match[1]), tag, digest))
if compatible:
    _, _, tag, digest = max(compatible)
    print(tag, digest)
PYBASE
        )
        if [ -n "$baseline" ]; then
            read -r baseline_tag digest <<< "$baseline"
            [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die "missing SHA-256 for $baseline_tag/$FPK_FILENAME"
            gh release download "$baseline_tag" --repo "$repo" --pattern "$FPK_FILENAME" --dir "$SANDBOX"
            echo "${digest#sha256:}  $OLD" | sha256sum -c -
            if [ "$baseline_tag" = "fnos-v$candidate_version" ]; then same_version_rehearsal=true; fi
        else
            # First published package for this architecture.
            same_version_rehearsal=true
            OLD="$NEW"
        fi
        ;;
    2) OLD="$1"; NEW="$2" ;;
    *) die "Usage: $0 [candidate.fpk] or $0 old.fpk new.fpk" ;;
esac
mkdir "$SANDBOX/old" "$SANDBOX/new"
tar -xzf "$OLD" -C "$SANDBOX/old"
tar -xzf "$NEW" -C "$SANDBOX/new"
for path in "$SANDBOX"/{old,new}/manifest; do
    [ "$(sim_manifest_get "$path" appname)" = fusionplay-fnos ] || die "unexpected appname"
    [ "$(sim_manifest_get "$path" platform)" = "$FPK_PLATFORM" ] || die "FPK does not match host architecture"
done
OLD_VER=$(sim_manifest_get "$SANDBOX/old/manifest" version)
NEW_VER=$(sim_manifest_get "$SANDBOX/new/manifest" version)
if "$same_version_rehearsal"; then
    echo "No earlier published $FPK_PLATFORM FPK: exercising same-version replacement ($NEW_VER)"
else
    [ "$OLD_VER" != "$NEW_VER" ] && [ "$(printf '%s\n' "$OLD_VER" "$NEW_VER" | sort -V | tail -1)" = "$NEW_VER" ] ||
        die "覆盖升级需要递增的版本：$OLD_VER → $NEW_VER"
    echo "Testing packaged upgrade $OLD_VER → $NEW_VER"
fi

# Runtime state and package home stay on the original storage volume.
export TRIM_APPNAME=fusionplay-fnos FUSIONPLAY_AUDIO_BACKEND=null
unset DATA_LIBRARY_PATH
for scenario in running orphan stopped; do
    root="$SANDBOX/$scenario"
    mkdir -p "$root"/{volume/target,home,var,etc,tmp}
    ln -s "$root/volume/target" "$root/target"
    export TRIM_APPDEST="$root/target" TRIM_PKGHOME="$root/home" TRIM_PKGVAR="$root/var"
    export TRIM_PKGETC="$root/etc" TRIM_PKGTMP="$root/tmp" TRIM_TEMP_LOGFILE="$root/tmp/error"
    TRIM_SERVICE_PORT=$(sim_pick_port 18080); export TRIM_SERVICE_PORT
    export TRIM_APPVER="$OLD_VER" TRIM_APP_STATUS=INSTALL
    old_cmd="$SANDBOX/old/cmd"
    new_cmd="$SANDBOX/new/cmd"
    tar -xzf "$SANDBOX/old/app.tgz" -C "$TRIM_APPDEST"
    cat >"$TRIM_PKGHOME/config.json" <<'JSON'
{"config_version":2,"receiver":{"name":"Upgrade Room"},"protocols":{"airplay":{"enabled":false},"dlna":{"enabled":false},"miplay":{"enabled":false}},"audio":{"devices":[{"id":"mock:study","name":"书房","enabled":false,"gain_db":-3.0,"delay_ms":12,"curve":{"kind":"custom","points":[[0,0],[0.5,0.3],[1,1]]}}]}}
JSON
    if [ "$scenario" = orphan ]; then
        # 0.2.5 preview builds also persisted speaker bindings in this config.
        python3 - "$TRIM_PKGHOME/config.json" <<'PYROOM'
import json, sys
from pathlib import Path
path = Path(sys.argv[1])
cfg = json.loads(path.read_text())
cfg["audio"]["instances"] = [{"id":"study","name":"Study","device_id":"mock:study"}]
path.write_text(json.dumps(cfg))
PYROOM
    fi
    before=$(sim_file_hash "$TRIM_PKGHOME/config.json")
    mkdir -p "$TRIM_PKGVAR/receivers"
    echo "persisted sender identity" >"$TRIM_PKGVAR/receivers/upgrade-marker"
    bash "$old_cmd/install_init"
    bash "$old_cmd/install_callback"
    bash "$old_cmd/main" start
    [ "$(sim_health_version "$TRIM_SERVICE_PORT")" = "$OLD_VER" ] || die "old service failed to start"
    old_pid=$(sim_pid_of "$TRIM_PKGVAR/app.pid")
    export TRIM_APP_STATUS=UPGRADE TRIM_OLD_APPVER="$OLD_VER"
    case "$scenario" in
        running)
            # The OS may stop the app before calling the upgrade hook.
            bash "$old_cmd/main" stop
            bash "$old_cmd/upgrade_init"
            ;;
        orphan)
            # Simulate a lost PID followed by OS PID reuse. The *incoming*
            # pre-upgrade hook must identify the orphan by its executable.
            sleep 120 & OTHER_PID=$!
            printf '%s\n' "$OTHER_PID" >"$TRIM_PKGVAR/app.pid"
            bash "$new_cmd/upgrade_init"
            kill -0 "$OTHER_PID" || die "recycled PID was signalled"
            kill "$OTHER_PID"; wait "$OTHER_PID" || true; OTHER_PID=""
            ;;
        stopped)
            bash "$old_cmd/main" stop
            bash "$new_cmd/upgrade_init"
            ;;
    esac
    [ ! -e "/proc/$old_pid/exe" ] || die "old process survived the pre-upgrade hook"
    sim_wait_port_free "$TRIM_SERVICE_PORT" || die "old port not released before replacement"
    rm -rf "$root/volume/target"
    mkdir "$root/volume/target"
    tar -xzf "$SANDBOX/new/app.tgz" -C "$TRIM_APPDEST"
    export TRIM_APPVER="$NEW_VER"
    if [ "$scenario" = stopped ]; then
        if TRIM_APPVER="0.0.0" bash "$new_cmd/upgrade_callback"; then die "mixed version accepted"; fi
        grep -q "$NEW_VER" "$TRIM_TEMP_LOGFILE" || die "missing package version diagnostic"
    fi
    bash "$new_cmd/upgrade_callback"
    bash "$new_cmd/upgrade_callback"
    status=0; bash "$new_cmd/main" status || status=$?
    [ "$status" = 3 ] || die "upgrade hook has started the service"
    [ ! -S "$TRIM_APPDEST/fusionplay-fnos.sock" ] || die "gateway socket claimed by hook"
    sim_wait_port_free "$TRIM_SERVICE_PORT" || die "fnOS checkport would reject enable"
    if [ "$scenario" = stopped ]; then
        python3 -m http.server "$TRIM_SERVICE_PORT" --bind 127.0.0.1 >"$root/other.log" 2>&1 & PORT_HOLDER=$!
        for _ in {1..50}; do [ "$(sim_port_listeners "$TRIM_SERVICE_PORT")" != 0 ] && break; sleep 0.1; done
        if bash "$new_cmd/main" start; then die "foreign listener accepted as our service"; fi
        grep -q "$TRIM_SERVICE_PORT" "$TRIM_TEMP_LOGFILE" || die "missing actionable port error"
        kill -0 "$PORT_HOLDER" || die "foreign listener was killed"
        kill "$PORT_HOLDER"; wait "$PORT_HOLDER" || true; PORT_HOLDER=""
    fi
    # Emulate the framework restoring a previously running app, or the user
    # enabling a previously disabled app later, after the port preflight.
    export TRIM_APP_STATUS=START
    bash "$new_cmd/main" start
    bash "$new_cmd/main" status
    new_pid=$(sim_pid_of "$TRIM_PKGVAR/app.pid")
    bash "$new_cmd/main" start
    [ "$(sim_pid_of "$TRIM_PKGVAR/app.pid")" = "$new_pid" ] || die "duplicate start"
    [ "$(sim_proc_exe_hash "$new_pid")" = "$(sim_file_hash "$TRIM_APPDEST/bin/fusionplay-fnos")" ] || die "wrong executable"
    [ "$(sim_health_version "$TRIM_SERVICE_PORT")" = "$NEW_VER" ] || die "wrong TCP version"
    curl -fsS --unix-socket "$TRIM_APPDEST/fusionplay-fnos.sock" http://localhost/healthz |
        python3 -c 'import json,sys; assert json.load(sys.stdin)["version"] == sys.argv[1]' "$NEW_VER"
    [ "$(sim_file_hash "$TRIM_PKGHOME/config.json")" = "$before" ] || die "user config overwritten"
    grep -qx 'persisted sender identity' "$TRIM_PKGVAR/receivers/upgrade-marker" || die "state lost"
    curl -fsS "http://127.0.0.1:$TRIM_SERVICE_PORT/api/config" | python3 -c '
import json, math, sys
v = json.load(sys.stdin)
d = v["audio"]["devices"][0]
assert v["receiver"]["name"] == "Upgrade Room" and d["id"] == "mock:study"
assert d["gain_db"] == -3 and d["delay_ms"] == 12 and d["curve"]["kind"] == "custom", d
x, y = d["curve"]["points"][1]
assert x == 0.5 and math.isclose(y, 0.3, abs_tol=1e-6), d'
    if [ "$scenario" = orphan ]; then
        curl -fsS "http://127.0.0.1:$TRIM_SERVICE_PORT/api/instances" | python3 -c '
import json,sys
assert {"id":"study","name":"Study","device_id":"mock:study"} in json.load(sys.stdin)["instances"]'
        curl -fsS "http://127.0.0.1:$TRIM_SERVICE_PORT/api/instances/study/status" >/dev/null
    fi
    bash "$new_cmd/main" stop
    sim_wait_port_free "$TRIM_SERVICE_PORT" || die "new service did not stop"
    echo "PASS: $scenario ($OLD_VER → $NEW_VER), saved configuration and gateway verified"
done
