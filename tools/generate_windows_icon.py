#!/usr/bin/env python3
"""Generate the Flutter Windows runner icon from the FusionPlay brand mark.

The Windows runner embeds `flutter/windows/runner/resources/app_icon.ico` as the
window/executable icon. Explorer, the taskbar and the Inno Setup shortcuts all
pick a different size out of the same .ico, so the file must contain multiple
raster sizes (16/24/32/48/64/128/256).

Usage (from anywhere; Pillow is required: `python3 -m pip install pillow`):

    python3 tools/generate_windows_icon.py
    python3 tools/generate_windows_icon.py --source brand-FusionPlay-Mark.png \
        --output flutter/windows/runner/resources/app_icon.ico

The script is idempotent and deterministic (LANCZOS downscaling, no timestamps),
so it can be re-run locally or in CI whenever the brand mark changes.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:  # pragma: no cover - friendly error for local/CI setups
    sys.exit(
        "Pillow is required to generate the Windows icon.\n"
        "Install it with: python3 -m pip install pillow"
    )

# Sizes Windows actually requests. 256 is stored as a PNG-compressed ICO frame,
# which Pillow does automatically.
ICON_SIZES = (16, 24, 32, 48, 64, 128, 256)

# The brand mark is a square RGBA PNG; the .ico frames are generated from a
# high-quality downscale. Anything smaller than `MIN_SOURCE_SIZE` would alias
# the 256px frame, so the source is upscaled first when necessary.
MIN_SOURCE_SIZE = 256


def repository_root() -> Path:
    """Return the repository root (the parent of this script's tools/ folder)."""
    return Path(__file__).resolve().parent.parent


def generate_icon(source: Path, destination: Path) -> Path:
    if not source.is_file():
        raise SystemExit(f"Brand mark not found: {source}")

    with Image.open(source) as image:
        image.load()
        # Flatten onto transparent RGBA so JPEG/CMYK sources also work.
        rgba = image.convert("RGBA")
        if rgba.width != rgba.height:
            # Keep the shortest side and center-crop to a square so the icon is
            # not distorted if the brand mark gains a wordmark later.
            side = min(rgba.width, rgba.height)
            left = (rgba.width - side) // 2
            top = (rgba.height - side) // 2
            rgba = rgba.crop((left, top, left + side, top + side))
        if rgba.width < MIN_SOURCE_SIZE:
            rgba = rgba.resize(
                (MIN_SOURCE_SIZE, MIN_SOURCE_SIZE), Image.Resampling.LANCZOS
            )

        frames = [
            rgba.resize((size, size), Image.Resampling.LANCZOS)
            for size in ICON_SIZES
        ]

        destination.parent.mkdir(parents=True, exist_ok=True)
        # Pillow picks the best per-frame encoding (PNG for 256x256); the
        # `append_images` order must be ascending for Windows to honour it.
        frames[-1].save(
            destination,
            format="ICO",
            sizes=[(size, size) for size in ICON_SIZES],
            append_images=frames[:-1],
        )

    return destination


def main() -> int:
    root = repository_root()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        type=Path,
        default=root / "brand-FusionPlay-Mark.png",
        help="Square-ish brand mark PNG (default: brand-FusionPlay-Mark.png).",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=root / "flutter" / "windows" / "runner" / "resources" / "app_icon.ico",
        help="Destination .ico (default: the Flutter Windows runner icon).",
    )
    args = parser.parse_args()

    destination = generate_icon(args.source.resolve(), args.output.resolve())
    sizes = ", ".join(f"{size}x{size}" for size in ICON_SIZES)
    print(f"Wrote {destination} ({sizes})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
