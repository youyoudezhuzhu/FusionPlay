"""Run from anywhere after flutter test -r expanded > logs/test.txt."""
import csv
from pathlib import Path
from shutil import copyfile

from PIL import Image

audit = Path(__file__).resolve().parents[1]
screens = audit / "screenshots"
metrics = []
for line in (audit / "logs/test.txt").read_text().splitlines():
    if line.startswith("METRIC|P"):
        metrics.append(line.split("|")[1:])
with (audit / "measurements.csv").open("w", newline="") as output:
    writer = csv.writer(output)
    writer.writerow("scene size element left top right bottom width height".split())
    writer.writerows(metrics)


def rect(scene, element):
    row = next(row for row in metrics if row[:3:2] == [scene, element])
    return tuple(float(value) for value in row[3:7])


def crop(source, dest, box):
    with Image.open(screens / source) as image:
        image.crop(tuple(round(2 * v) for v in box)).save(screens / dest)


prev = rect("P03", "prev")
nxt = rect("P03", "next")
volume = rect("P03", "volume")
crop("player/P03.png", "glass/G01.png", (prev[0]-4, prev[1]-18, nxt[2]+4, nxt[3]+18))
crop("player/P03.png", "glass/G02.png", (volume[0]-4, volume[1]-4, volume[2]+4, volume[3]+4))
crop("player/P03.png", "glass/G03.png", (885, 0, 980, 45))
crop("glass/G06.png", "glass/G08.png", (490, 100, 920, 365))
crop("player/P01.png", "details/P01-control-stack.png", (185, 100, 375, 328))
for name in ["G06", "G07"]:
    copyfile(screens / f"glass/{name}.png", screens / f"settings/{name}.png")
for name in ["glass-optics", "glass-optics-flat"]:
    copyfile(Path("/tmp/fusionplay-preview") / f"{name}.png", screens / f"glass/{name}.png")
