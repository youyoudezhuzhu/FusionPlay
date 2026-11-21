"""Test the fnOS curve editor with Chromium (pip install playwright)."""
import json
import os
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


def main():
    devices = [{"id": name, "name": name, "enabled": name != "Study",
                "gain_db": 0, "delay_ms": 0, "sample_rate": 0,
                "curve": {"kind": "balanced", "points": []}}
               for name in ["Living room", "HDMI", "Study"]]
    config = {"config_version": 2, "receiver": {"name": "Living room"},
              "audio": {"devices": devices, "instances": [
                  {"id": "room", "name": "Study", "device_id": "Study"}]},
              "protocols": {}, "logging": {}}
    frontend = Path(__file__).resolve().parents[1] / "frontend"
    handler = partial(SimpleHTTPRequestHandler, directory=frontend)
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server, sync_playwright() as playwright:
        Thread(target=server.serve_forever, daemon=True).start()
        browser = playwright.chromium.launch(executable_path=os.getenv("CHROMIUM"))
        page = browser.new_page(viewport={"width": 390, "height": 760})

        def api(route):
            nonlocal config
            path = urlparse(route.request.url).path
            if path == "/api/config" and route.request.method == "PUT":
                config = json.loads(route.request.post_data)
                route.fulfill(json={"config": config})
            elif path == "/api/config":
                route.fulfill(json=config)
            elif path == "/api/devices":
                route.fulfill(json={"devices": []})
            else:
                route.fulfill(json={})

        page.route("**/api/**", api)
        page.route("**/healthz", lambda route: route.fulfill(json={"status": "ok"}))
        base = f"http://127.0.0.1:{server.server_port}/index.html?instance="
        page.goto(base + "room#/curve")
        expect(page.locator("#audio-panel-name")).to_have_text("Study")
        expect(page.locator("#curve-device")).to_be_hidden()
        page.get_by_role("button", name="柔和", exact=True).click()
        page.locator("#savebar-save").click()
        expect(page.locator("#savebar")).to_be_hidden()
        edited = [d["id"] for d in config["audio"]["devices"] if d.get("curve", {}).get("kind") == "soft"]
        assert edited == ["Study"], edited

        page.goto(base + "default#/curve")
        expect(page.locator("#audio-device-panel")).to_be_hidden()
        expect(page.locator("#curve-device option")).to_have_count(3)
        page.locator("#curve-device").select_option("HDMI")
        expect(page.locator("#audio-panel-name")).to_have_text("HDMI")
        page.goto(base + "deleted#/curve")
        expect(page.locator("#audio-empty-sub")).to_have_text("此播放器没有已分配的输出音箱。")
        expect(page.locator("#audio-device-panel")).to_be_hidden()
        browser.close()
        server.shutdown()


if __name__ == "__main__":
    main()
