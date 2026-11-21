"""Exercise binding and per-output calibration in Chromium (pip install playwright)."""
import json
import os
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


def main():
    devices = [
        {"id": name, "name": name, "enabled": name in ("Living room", "HDMI"),
         "curve": {"kind": "balanced", "points": []}}
        for name in ("Living room", "HDMI", "Study", "USB")
    ]
    instances = [{"id": "room", "name": "Study", "device_id": "Study"}]
    repo = Path(__file__).resolve().parents[2]
    handler = partial(SimpleHTTPRequestHandler, directory=repo / "fnos/frontend")
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server, sync_playwright() as playwright:
        Thread(target=server.serve_forever, daemon=True).start()
        browser = playwright.chromium.launch(executable_path=os.getenv("CHROMIUM"))
        page = browser.new_page(viewport={"width": 390, "height": 760})

        output_edits = []
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))

        def api(route):
            request = route.request
            path = urlparse(request.url).path
            if path == "/api/instances/room/curve" and request.method == "PUT":
                edit = json.loads(request.post_data)
                assert edit["device_id"] == "Study"
                devices[2]["curve"] = edit["curve"]
                route.fulfill(json={"ok": True})
            elif path == "/api/instances" and request.method == "POST":
                binding = json.loads(request.post_data)
                assert binding["device_id"] == "USB"
                if binding["name"] == "Study":
                    route.fulfill(status=409, json={"error": "投送名称重复"})
                else:
                    instances.append({"id": "desk", **binding})
                    route.fulfill(json={"id": "desk"})
            elif path == "/api/instances":
                route.fulfill(json={"default": "default", "instances": [
                    {"id": "default", "name": "Living room"}, *instances]})
            elif path == "/api/outputs" and request.method == "PUT":
                edit = request.post_data_json
                output_edits.append(edit)
                devices[:] = edit["audio"]["devices"]
                route.fulfill(json={"audio": edit["audio"]})
            elif path == "/api/config":
                route.fulfill(json={"audio": {"devices": devices, "instances": instances, "global_attenuation_percent": 100}})
            elif path == "/api/devices":
                route.fulfill(json={"devices": []})
            else:
                route.fulfill(json={})

        page.route("**/api/**", api)
        page.route("**/player/fnos-mark.png",
                   lambda route: route.fulfill(path=repo / "flutter/web/fnos-mark.png"))
        page.route("**/player/assets/assets/fonts/MiSansUI.ttf",
                   lambda route: route.fulfill(path=repo / "flutter/assets/fonts/MiSansUI.ttf"))
        base = f"http://127.0.0.1:{server.server_port}/speakers.html?instance="
        root = f"http://127.0.0.1:{server.server_port}/"
        page.route("**/player/index.html*", lambda route: route.fulfill(body="<html></html>", content_type="text/html"))
        for old, new in [("#/overview", "player/index.html"), ("#/settings", "player/index.html"),
                         ("#/receive", "player/index.html"), ("#/unknown", "player/index.html"),
                         ("#/audio", "speakers.html"), ("#/curve", "speakers.html")]:
            page.goto(root + "index.html?instance=room" + old)
            fragment = {"#/settings":"#/settings", "#/receive":"#/settings", "#/audio":"#outputs",
                        "#/curve":"#curve"}.get(old, "")
            expect(page).to_have_url(root + new + "?instance=room" + fragment)
            expect(page.locator(".main-nav, #page-overview, #page-audio")).to_have_count(0)
        for old in ["js/main.js", "js/page-audio.js", "js/page-receive.js", "js/page-settings.js", "style.css"]:
            assert page.request.get(root + old).status == 404
        page.goto(base + "room#rooms")
        page.get_by_role("link", name="管理输出").click()
        page.locator("#output-select").select_option("Study")
        expect(page.locator("#output-enable")).to_be_disabled()
        expect(page.locator("#output-owner")).to_contain_text("Study")
        page.locator("#output-select").select_option("HDMI")
        page.locator("#output-delay").fill("123")
        page.locator("#output-delay").press("Tab")
        with page.expect_navigation(wait_until="load"):
            page.locator("#outputs-save").click()
        expect(page.locator("#outputs-save")).to_be_disabled()
        assert output_edits and next(d for d in devices if d["id"] == "HDMI")["delay_ms"] == 123
        assert not errors, errors
        page.goto(base + "room#rooms")
        expect(page.locator(".room")).to_have_count(2)
        page.locator(".room").filter(has_text="Study").get_by_text("音量曲线").click()
        expect(page.locator("#room-label")).to_have_text("Study")
        expect(page.locator("#curve-device")).to_have_value("Study")
        page.get_by_role("button", name="柔和", exact=True).click()
        page.locator("#save").click()
        expect(page.locator("#save-state")).to_contain_text("已保存")
        edited = [d["id"] for d in devices if d["curve"]["kind"] == "soft"]
        assert edited == ["Study"], edited
        page.reload()
        expect(page.get_by_role("button", name="柔和", exact=True)).to_have_attribute("aria-pressed", "true")

        page.goto(base + "default#curve")
        expect(page.locator("#editor")).to_be_hidden()
        expect(page.locator("#curve-device option")).to_have_count(3)
        page.locator("#curve-device").select_option("HDMI")
        expect(page.locator("#editor")).to_be_visible()
        page.goto(base + "room&device=HDMI#curve")
        expect(page.locator("#editor")).to_be_hidden()
        page.goto(base + "default#curve")
        picker = page.locator("#curve-device")
        picker.select_option("HDMI")
        page.get_by_role("button", name="柔和", exact=True).click()
        picker.select_option("Living room")
        dialog = page.get_by_role("dialog")
        expect(dialog.get_by_text("放弃更改")).to_be_visible()
        page.keyboard.press("Escape")
        expect(picker).to_have_value("HDMI")
        picker.select_option("Living room")
        dialog.get_by_text("放弃更改").click()
        expect(picker).to_have_value("Living room")
        expect(page.locator("#save")).to_be_disabled()
        page.goto(base + "deleted#curve")
        expect(page.locator("#notice")).to_contain_text("此实例已移除")
        expect(page.locator("#editor")).to_be_hidden()

        page.goto(base + "room#edit")
        expect(page.locator("#room-name")).to_have_value("Study")
        expect(page.locator("#bind-device")).to_have_value("Study")
        page.goto(base + "room#new")
        expect(page.locator("#bind")).to_be_disabled()
        occupied = page.locator("#bind-device option[value='Study']")
        expect(occupied).to_be_disabled()
        expect(occupied).to_contain_text("已绑定「Study」")
        page.locator("#room-name").fill("Study")
        page.locator("#bind-device").select_option("USB")
        page.locator("#bind").click()
        expect(page.locator("#notice")).to_contain_text("投送名称重复")
        expect(page.locator("#editor")).to_be_hidden()
        page.locator("#room-name").fill("Desk")
        page.locator("#bind").click()
        expect(page.locator("#room-label")).to_have_text("Desk")
        expect(page.locator("#curve-device")).to_have_value("USB")
        expect(page.locator("#binding-result")).to_contain_text("已绑定USB")
        expect(page.locator("#editor")).to_be_visible()
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        for n in range(1, 17):
            device_id = f"speaker-{n}"
            devices.append({"id": device_id, "name": device_id, "enabled": False})
            instances.append({"id": device_id, "name": f"Room {n}", "device_id": device_id})
        page.goto(base + "room#rooms")
        cards = page.locator(".room")
        expect(cards).to_have_count(19)
        for width, height in [(1280, 720), (390, 760)]:
            page.set_viewport_size({"width": width, "height": height})
            page.evaluate("window.scrollTo(0, 0)")
            if width == 1280:
                expect(cards.nth(7)).to_be_in_viewport(ratio=1)
            expect(cards.last).not_to_be_in_viewport(ratio=1)
            cards.last.scroll_into_view_if_needed()
            expect(cards.last).to_be_in_viewport(ratio=1)
            assert page.evaluate("scrollY > 0 && document.documentElement.scrollWidth <= innerWidth")
        cards.last.get_by_text("编辑实例").click()
        expect(page.locator("#room-name")).to_have_value("Room 16")
        remove = page.locator("#remove-instance")
        remove.click()
        expect(dialog.get_by_text("取消")).to_be_focused()
        dialog.get_by_text("取消").click()
        expect(remove).to_be_focused()
        expect(page.locator("#room-name")).to_have_value("Room 16")
        browser.close()


if __name__ == "__main__":
    main()
