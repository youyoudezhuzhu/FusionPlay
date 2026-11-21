from pathlib import Path
import argparse
import hashlib
import os
import shutil
import tarfile
import tempfile
import threading
from io import BytesIO
from PIL import Image, ImageChops, ImageStat
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from functools import partial
from playwright.sync_api import sync_playwright, expect

repo = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(
    description="Render fnOS production pages against a local eight-room fixture."
)
parser.add_argument(
    "--sdk-archive",
    type=Path,
    required=True,
    help="Pinned @trimjs/web-app 0.6.0 tarball used by build-fpk.sh",
)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
if (
    hashlib.sha256(args.sdk_archive.read_bytes()).hexdigest()
    != "a60f04172186017ac5b4e4d2b2af64c11fa699b48c9e3a69577e7c06f88ab4ec"
):
    parser.error("unexpected SDK checksum")
out = args.output
out.mkdir(parents=True, exist_ok=True)
names = ["客厅", "书房", "餐厅", "卧室", "工作室", "影音室", "阳台", "儿童房"]
ids = ["default", *[f"room-{n}" for n in range(1, 8)]]
rooms = [
    {"id": i, "name": n, "device_id": f"usb{k}"}
    for k, (i, n) in enumerate(zip(ids, names))
]
devices = [
    dict(
        id="usb" + str(k),
        name=n + " USB 音箱",
        enabled=k == 0,
        gain_db=0,
        delay_ms=0,
        sample_rate=0,
        curve={"kind": "soft", "points": []},
    )
    for k, n in enumerate([*names, "备用"])
]
cfg = {
    "receiver": {"name": "客厅"},
    "protocols": {p: {"enabled": True} for p in ["miplay", "airplay", "dlna"]},
    "audio": {
        "devices": devices,
        "instances": rooms[1:],
        "global_attenuation_percent": 95,
        "muted": False,
    },
}
lyrics = None


def status(room):
    source = {"default": "dlna", "room-1": "airplay", "room-2": "miplay"}.get(room)
    return dict(
        current_source=source,
        receiver_name=names[ids.index(room)],
        media={
            "title": "Starlight",
            "artist": "Muse · Drones",
            "duration_ms": 210000,
            "position_ms": 84000,
            "cover_url": "artwork/current.jpg",
            "lyrics_text": lyrics,
        },
        protocols={
            p: {"state": "playing" if p == source else "ready"}
            for p in cfg["protocols"]
        },
        sender_volume={"dlna_percent": 68, "airplay_percent": 68, "miplay_percent": 68},
        player_commands=[
            "seek",
            "previous_track",
            "next_track",
            "set_volume",
            "play_pause",
        ],
        logging={"level": "info"},
        version="1.7.10",
        errors=[],
    )


photo = repo / "fusionplay-ui-audit-codex/cover-study/alpenglow.jpg"


def api(r):
    path = r.request.url.split("/api/")[-1].split("?")[0]
    if r.request.method != "GET":
        raise Exception(r.request.method + " " + path)
    if "artwork/" in path:
        return r.fulfill(path=photo)
    if path.endswith("/status"):
        data = status(path.split("/")[1])
    else:
        data = {
            "config": cfg,
            "instances": {"default": "default", "instances": rooms},
            "devices": {"devices": devices},
            "status": status("default"),
            "logs": {"lines": ["[INFO] 音箱已就绪", "[INFO] 接收服务已启动"]},
            "diagnostics": {
                "backend": "ALSA",
                "interfaces": ["eth0"],
                "alsa_cards": "USB Audio",
            },
            "update/check": {"status": "up_to_date"},
        }[path]
    r.fulfill(json=data)


with tempfile.TemporaryDirectory() as work:
    stage = Path(work)
    (stage / "player").symlink_to(repo / "flutter/build/web", target_is_directory=True)
    shutil.copytree(repo / "fnos/frontend", stage, dirs_exist_ok=True)
    with tarfile.open(args.sdk_archive) as sdk:
        (stage / "js/trim-web-app.js").write_bytes(
            sdk.extractfile("package/dist/index.js").read()
        )
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), partial(SimpleHTTPRequestHandler, directory=work)
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}/"
    with sync_playwright() as web:
        b = web.chromium.launch(
            executable_path=os.getenv("CHROMIUM", "/usr/bin/chromium"),
            args=["--enable-unsafe-swiftshader", "--force-renderer-accessibility"],
        )
        ctx = b.new_context(viewport={"width": 1100, "height": 680})
        ctx.route("**/api/**", api)
        page = ctx.new_page()
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))

        def snap(name):
            page.evaluate("document.fonts.ready")
            page.mouse.move(0, 0)
            page.wait_for_timeout(450)
            assert page.evaluate("document.documentElement.scrollWidth<=innerWidth"), (
                name
            )
            page.screenshot(path=out / ("fnos-" + name + ".png"))
            print("captured", name, flush=True)

        def photo_loaded():
            page.wait_for_function(
                'document.querySelector("#scene-art").naturalWidth>0 && !document.querySelector("#scene-art").hidden'
            )

        def html(name, url, sel, mode=True):
            page.goto(base + url)
            card = page.locator(sel).first
            expect(card).to_be_visible()
            photo_loaded()
            page.evaluate("window.scrollTo(0,0)")
            if mode:
                page.wait_for_function(
                    's=>document.querySelector(s)?.style.getPropertyValue("--surface-lens").includes("url")',
                    arg=sel,
                )
            else:
                assert card.evaluate("e=>getComputedStyle(e).backdropFilter") == "none"
            snap(name)

        page.goto(base + "player/index.html?instance=default")
        page.locator("flutter-view").wait_for(state="attached")
        page.wait_for_timeout(1100)

        def sem():
            if page.locator("flt-semantics-placeholder").count():
                page.locator("flt-semantics-placeholder").evaluate("e=>e.click()")

        sem()
        page.wait_for_timeout(1800)
        snap("player")
        lyrics = "[00:10.00]The night is calling\n[01:10.00]And the stars are dancing\n[01:24.00]With you by my side\n[01:50.00]Everything feels right"
        page.wait_for_timeout(1800)
        snap("lyrics")
        lyrics = None
        page.wait_for_timeout(1500)
        page.get_by_role("button", name="飞牛音箱").click()
        snap("speaker-menu")
        def open_settings():
            page.goto("about:blank")
            page.goto(base + "player/index.html?instance=default#/settings")
            page.locator("flutter-view").wait_for(state="attached")
            page.wait_for_timeout(1500)
            sem()

        for name, label in [
            ("logs", "日志与故障信息"),
            ("network", "网络接口"),
            ("pairing", "AirPlay 配对 PIN"),
        ]:
            open_settings()
            if name == "logs":
                snap("settings")
            page.mouse.move(900, 590)
            page.mouse.wheel(0, 740)
            page.wait_for_timeout(650)
            page.get_by_role("button", name=label, exact=True).click(timeout=8000)
            action = "关闭" if name == "logs" else "取消"
            expect(page.get_by_role("button", name=action, exact=True)).to_be_visible()
            snap(name)
            page.get_by_role("button", name=action, exact=True).click()
        for section, room, sel in [
            ("rooms", "default", ".room"),
            ("new", "default", "#bind-form"),
            ("edit", "room-1", "#bind-form"),
            ("curve", "room-1", "#editor"),
            ("outputs", "default", "#outputs-form"),
        ]:
            html(section, "speakers.html?instance=" + room + "#" + section, sel)
            if section == "curve":
                probe = page.locator("#probe")
                thumb = page.locator(".probe-lens")
                stripes = page.add_style_tag(content="""
                  .probe-slider {background:repeating-linear-gradient(to right,#182944 0 6px,#c2e4ff 6px 11px)}
                """)
                for target in ["Home", "End"]:
                    probe.press(target)
                    page.wait_for_timeout(350)
                    bent = Image.open(BytesIO(thumb.screenshot())).convert("RGB")
                    thumb.evaluate("e=>e.style.backdropFilter='none'")
                    flat = Image.open(BytesIO(thumb.screenshot())).convert("RGB")
                    thumb.evaluate("e=>e.style.removeProperty('backdrop-filter')")
                    diff = ImageStat.Stat(ImageChops.difference(bent, flat)).mean
                    assert max(diff) > 2, (target, diff)
                stripes.evaluate("e=>e.remove()")
            if section == "edit":
                page.locator("#remove-instance").click()
                expect(page.locator("dialog")).to_be_visible()
                assert page.evaluate("document.activeElement.value === 'cancel'")
                snap("confirm")
                page.keyboard.press("Escape")
                expect(page.locator("dialog")).not_to_be_visible()
                expect(page.locator("#remove-instance")).to_be_focused()
        html("update", "update.html?instance=room-1", ".card")
        expect(page.locator("#release")).to_have_text("已是最新版本")
        # Measure displacement on the live card, retaining blur, tint and border.

        page.add_style_tag(
            content=""".scene img,.scene::after{display:none}.scene{background:repeating-linear-gradient(65deg,#030714 0 70px,#fbfbff 70px 145px)}"""
        )
        lens = page.locator(".card")

        def raster():
            page.wait_for_timeout(200)
            return Image.open(BytesIO(lens.screenshot())).convert("RGB")

        a = raster()
        bounds = lens.bounding_box()
        page.mouse.move(bounds["x"] + bounds["width"]/2, bounds["y"] + bounds["height"]/2)
        page.mouse.down()
        pressed = raster()
        page.mouse.up()
        restored = raster()
        center = (24, 24, a.width - 24, a.height - 24)
        assert ImageChops.difference(a, pressed).crop(center).getbbox(), "no contact displacement"
        assert ImageChops.difference(a, restored).getbbox() is None, "contact did not settle"
        lens.evaluate("e=>e.style.setProperty('--surface-lens','brightness(1)')")
        delta = ImageChops.difference(a, raster())
        w, h = delta.size
        inner = ImageStat.Stat(delta.crop((24, 24, w - 24, h - 24))).mean
        rim = ImageStat.Stat(delta.crop((2, 20, 10, h - 20))).mean
        print("optical difference", rim, inner, flush=True)
        assert max(rim) > max(inner) + 0.2
        page.evaluate("fpSetEffects(false)")
        html("curve-off", "speakers.html?instance=room-1#curve", "#editor", False)
        for section, room, selector in [
            ("rooms", "default", ".room"),
            ("new", "default", "#bind-form"),
            ("edit", "room-1", "#bind-form"),
            ("outputs", "default", "#outputs-form"),
            ("update", "room-1", ".card"),
        ]:
            url = (
                "update.html?instance=room-1"
                if section == "update"
                else f"speakers.html?instance={room}#{section}"
            )
            page.goto(base + url)
            card = page.locator(selector).first
            expect(card).to_be_visible()
            assert card.evaluate("e=>getComputedStyle(e).backdropFilter") == "none"
            assert (
                card.evaluate("e=>getComputedStyle(e).backgroundColor")
                == "rgb(37, 37, 48)"
            )
        page.evaluate("fpSetEffects(true)")
        page.set_viewport_size({"width": 390, "height": 760})
        html("rooms-mobile", "speakers.html?instance=room-1#rooms", ".room")
        page.locator(".room").last.scroll_into_view_if_needed()
        expect(page.locator(".room").last).to_be_in_viewport()
        html("curve-mobile", "speakers.html?instance=room-1#curve", "#editor")
        assert not errors, errors
        b.close()
