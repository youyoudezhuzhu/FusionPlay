"""Compare rasterized text with the same pixels after hiding only the glyphs."""
from io import BytesIO
import json

from PIL import Image


def luminance(rgb):
    def linear(v):
        n = v / 255
        return n / 12.92 if n <= .04045 else ((n + .055) / 1.055) ** 2.4
    return sum(c * linear(v) for c, v in zip((.2126, .7152, .0722), rgb))


def measure(page, selectors):
    rects = page.evaluate("""selectors => selectors.map(selector => {
      const el = document.querySelector(selector);
      el.dataset.contrastProbe = '';
      const r = el.getBoundingClientRect();
      return [Math.ceil(r.left),Math.ceil(r.top),Math.floor(r.right),Math.floor(r.bottom)];
    })""", selectors)
    def raster():
        return Image.open(BytesIO(page.screenshot())).convert("RGB")
    normal = raster()
    css = page.add_style_tag(content="""[data-contrast-probe], [data-contrast-probe]::placeholder {
      -webkit-text-fill-color: transparent !important; text-shadow:none !important;
    }""")
    clear = raster()
    css.evaluate("e=>e.remove()")
    page.evaluate("document.querySelectorAll('[data-contrast-probe]').forEach(e=>delete e.dataset.contrastProbe)")
    results = {}
    for name, rect in zip(selectors, rects):
        x, y, right, bottom = rect
        box = (max(0,x), max(0,y), min(normal.width,right), min(normal.height,bottom))
        values = []
        for ink, bg in zip(normal.crop(box).getdata(), clear.crop(box).getdata()):
            if ink == bg:
                continue
            a, b = luminance(ink), luminance(bg)
            values.append((max(a,b) + .05) / (min(a,b) + .05))
        assert len(values) >= 8, (name, "no visible glyphs")
        values.sort()
        results[name] = round(values[int((len(values)-1)*.97)], 2)
    return results


def check_contrast(page, base, out):
    results = {}
    for effects in [True, False]:
        page.goto(base + "speakers.html?instance=default#rooms")
        page.evaluate("value => fpSetEffects(value)", effects)
        for section, route, selectors in [
            ("rooms","speakers.html?instance=default#rooms",["#back","#add-room",".room h2",".room .media"]),
            ("bind","speakers.html?instance=room-1#new",["#bind-title","label[for=room-name]","#room-name"]),
            ("outputs","speakers.html?instance=default#outputs",["#outputs-rooms","label[for=output-delay]"]),
            ("curve","speakers.html?instance=room-1#curve",["#room-label",".axis","#readout","#modes [aria-pressed=true]"]),
            ("update","update.html?instance=room-1",["#back","#release","#message"]),
            ("confirm","speakers.html?instance=room-1#edit",["#confirm-title","#confirm-accept","button[value=cancel]"]),
        ]:
            page.goto(base + route)
            page.wait_for_function("document.querySelector('#scene-art').naturalWidth>0")
            if section == "confirm":
                page.locator("#remove-instance").click()
            if section == "curve":
                page.locator(selectors[-1]).hover()
            page.evaluate("document.fonts.ready")
            for color in ["white","black"]:
                css = page.add_style_tag(content="#scene-art{filter:brightness(0)" +
                                         (" invert(1)" if color=="white" else "") + " !important}")
                label = f"{section}-{color}-{effects}"
                results[label] = measure(page, selectors)
                if section == "curve" and color == "white" and effects:
                    page.screenshot(path=out / "curve-white-hover.png")
                css.evaluate("e=>e.remove()")
    page.goto(base + "speakers.html?instance=room-1#curve")
    graph = page.locator("#graph")
    graph.wait_for(state="visible")
    for scheme in ["light", "dark"]:
        page.emulate_media(forced_colors="active", color_scheme=scheme)
        page.wait_for_timeout(200)
        drawn = Image.open(BytesIO(graph.screenshot())).convert("RGB")
        blank = page.add_style_tag(content="#graph{opacity:0!important}")
        bg = Image.open(BytesIO(graph.screenshot())).convert("RGB")
        blank.evaluate("e=>e.remove()")
        w,h = drawn.size
        left = 30 if w < 320 else 36
        points = {}
        for t in [.37, .63]:
            amplitude = page.evaluate("t=>FP.curve.outputAt('soft', [], t)", t)
            x = round(left + t * (w-left-12))
            y = round(10 + (1-amplitude)*(h-31))
            ratios = []
            for row in range(y-2, y+3):
                a,b = luminance(drawn.getpixel((x,row))), luminance(bg.getpixel((x,row)))
                ratios.append((max(a,b)+.05)/(min(a,b)+.05))
            points[str(t)] = round(max(ratios),2)
        results["forced-curve-"+scheme] = points
    (out / "fnos-contrast.json").write_text(json.dumps(results, indent=2))
    failures = [(label,key,r) for label,values in results.items() for key,r in values.items() if r < (3 if label.startswith("forced-curve") else 4.5)]
    assert not failures, failures
