#!/usr/bin/env python3
"""Linux-side NDJSON smoke test for the Windows sidecar (FusionPlayCore).

The Windows CI runs the equivalent PowerShell smoke test on windows-latest.
This script lets us verify the same contract locally (no audio device needed)
before pushing to CI:

    python3 tools/windows_sidecar_smoke.py --exe windows/backend/target/debug/FusionPlayCore

It starts the sidecar with --null-audio --no-miplay, waits for `ready` and an
app_state with receiverReady=true, exercises a few UI commands (including a
settings round-trip), then sends `shutdown` and asserts a clean exit.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


class EventStream:
    def __init__(self, process: subprocess.Popen[str]) -> None:
        self.process = process
        self.events: "queue.Queue[dict]" = queue.Queue()
        self.raw: list[str] = []
        self.stderr: list[str] = []
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            line = line.strip()
            if not line:
                continue
            self.raw.append(line)
            try:
                self.events.put(json.loads(line))
            except json.JSONDecodeError:
                self.events.put({"type": "unparsed", "raw": line})

    def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self.stderr.append(line.rstrip())

    def wait_for(self, predicate, timeout: float, description: str) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            try:
                event = self.events.get(timeout=max(0.05, min(remaining, 0.5)))
            except queue.Empty:
                continue
            if predicate(event):
                return event
        raise AssertionError(
            f"等待 {description} 超时（{timeout}s）。已收到 {len(self.raw)} 行 stdout；"
            f"前 10 行：{self.raw[:10]}；stderr 尾部：{self.stderr[-10:]}"
        )

    def drain(self) -> list[dict]:
        drained = []
        while True:
            try:
                drained.append(self.events.get_nowait())
            except queue.Empty:
                return drained


def send(process: subprocess.Popen[str], payload: dict) -> None:
    assert process.stdin is not None
    process.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
    process.stdin.flush()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, help="FusionPlayCore binary path")
    parser.add_argument("--timeout", type=float, default=45.0)
    parser.add_argument("--keep", action="store_true", help="keep the temp data dir")
    args = parser.parse_args()

    exe = Path(args.exe).resolve()
    if not exe.is_file():
        print(f"FAIL: 侧车不存在：{exe}", file=sys.stderr)
        return 2

    data_dir = Path(tempfile.mkdtemp(prefix="fusionplay-smoke-"))
    process = subprocess.Popen(
        [
            str(exe),
            "--null-audio",
            "--no-miplay",
            "--data-dir",
            str(data_dir),
            "--log-level",
            "info",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )
    stream = EventStream(process)
    failures: list[str] = []

    try:
        ready = stream.wait_for(lambda event: event.get("type") == "ready", args.timeout, "ready")
        assert ready.get("version"), "ready 事件缺少 version"
        print(f"OK ready: version={ready['version']} nullAudio={ready.get('nullAudio')}")

        state = stream.wait_for(
            lambda event: event.get("type") == "app_state"
            and event.get("state", {}).get("receiverReady") is True,
            args.timeout,
            "app_state.receiverReady",
        )
        receiver_state = state["state"]
        print(
            "OK receiverReady:"
            f" name={receiver_state.get('receiverName')!r}"
            f" port={receiver_state.get('receiverPort')}"
            f" device={receiver_state.get('receiverDeviceId')}"
        )

        # Settings round-trip through a command; the sidecar must echo the new
        # name in the next app_state and persist settings.json.
        send(process, {"command": "setReceiverName", "value": "FP 冒烟测试", "requestId": "smoke-1"})
        renamed = stream.wait_for(
            lambda event: event.get("type") == "app_state"
            and event["state"].get("receiverName") == "FP 冒烟测试",
            args.timeout,
            "setReceiverName app_state",
        )
        send(process, {"command": "setReceiverName", "value": "", "requestId": "smoke-2"})
        stream.wait_for(
            lambda event: event.get("type") == "app_state"
            and event["state"].get("settings", {}).get("receiverName") is None,
            15,
            "settings.receiverName cleared",
        )
        settings_file = data_dir / "settings.json"
        if not settings_file.is_file():
            failures.append("settings.json 未生成")
        else:
            persisted = json.loads(settings_file.read_text(encoding="utf-8"))
            if persisted.get("schemaVersion") != 8:
                failures.append(f"settings.json schemaVersion={persisted.get('schemaVersion')}")

        # Export diagnostics writes a bundle under exports/.
        send(process, {"command": "exportLogs", "requestId": "smoke-3"})
        deadline = time.monotonic() + 20
        export_ok = False
        while time.monotonic() < deadline and not export_ok:
            for event in stream.drain():
                if event.get("type") == "app_state":
                    hint = event["state"].get("lastError") or ""
                    if "导出" in hint:
                        export_ok = True
                    break
            time.sleep(0.1)
        exports = list((data_dir / "exports").glob("*")) if (data_dir / "exports").is_dir() else []
        if not exports:
            failures.append("exportLogs 未在 exports/ 生成诊断包")
        else:
            print(f"OK exportLogs: {exports[-1].name}")

        # Unknown commands must be rejected without crashing the sidecar.
        send(process, {"command": "definitelyNotACommand", "requestId": "smoke-4"})
        response = stream.wait_for(
            lambda event: event.get("type") == "response"
            and event.get("requestId") == "smoke-4",
            10,
            "unknown command response",
        )
        if response.get("ok") is not False:
            failures.append("未知命令应返回 ok=false")

        # Clean shutdown.
        send(process, {"command": "shutdown"})
        try:
            exit_code = process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            failures.append("shutdown 后进程未退出")
            exit_code = -1
        if exit_code != 0:
            failures.append(f"shutdown 退出码 {exit_code}（期望 0）")
        else:
            print("OK shutdown exit=0")

        log_file = data_dir / "logs" / "fusionplay.log"
        if not log_file.is_file():
            failures.append("logs/fusionplay.log 未生成")
        else:
            print(f"OK log file: {log_file.stat().st_size} bytes")

        artifacts = [
            path
            for path in receiver_state.get("receiverPort") and [] or []
        ]
        del artifacts
    finally:
        if process.poll() is None:
            process.kill()
        if args.keep:
            print(f"数据目录保留：{data_dir}")
        else:
            shutil.rmtree(data_dir, ignore_errors=True)

    if failures:
        print("FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print("SMOKE PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
