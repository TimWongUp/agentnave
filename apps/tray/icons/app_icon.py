"""Draw the App icon (Dock, Finder, installer) from the N mark into icons/*.png, icon.icns and icon.ico.

The mark is tray.svg's one-stroke N with its dot, light on a dark macOS-style rounded square.
Run with `python3 icons/app_icon.py` on macOS; AppKit draws the SVG, iconutil packs the icns.
"""

import pathlib
import subprocess
import sys
import tempfile

from PIL import Image

HERE = pathlib.Path(__file__).parent
OUT = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else HERE

# 1024 canvas on Apple's icon grid: an 824 body inset by 100, corners of 185; the 44-unit mark
# is scaled by 13 and centred in the body.
SVG = """<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <rect x="100" y="100" width="824" height="824" rx="185" fill="#1c1c1e"/>
  <g transform="translate(233 225) scale(13)">
    <path d="M10 35V10L32 35V20" fill="none" stroke="#f5f5f7" stroke-width="6.5" stroke-linecap="round" stroke-linejoin="round"/>
    <circle cx="32" cy="10" r="4" fill="#f5f5f7"/>
  </g>
</svg>
"""

RENDER = """import AppKit
let a = CommandLine.arguments
let img = NSImage(contentsOf: URL(fileURLWithPath: a[1]))!
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1024, pixelsHigh: 1024, bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
img.draw(in: NSRect(x: 0, y: 0, width: 1024, height: 1024))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        svg, script, source = (pathlib.Path(tmp, n) for n in ("icon.svg", "render.swift", "source.png"))
        svg.write_text(SVG)
        script.write_text(RENDER)
        subprocess.run(["swift", str(script), str(svg), str(source)], check=True)
        big = Image.open(source)
        big.save(OUT / "source.png")
        for name, size in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)):
            big.resize((size, size), Image.LANCZOS).save(OUT / name)
        big.save(OUT / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 256)])
        iconset = pathlib.Path(tmp, "icon.iconset")
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            big.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
            big.resize((size * 2, size * 2), Image.LANCZOS).save(iconset / f"icon_{size}x{size}@2x.png")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(OUT / "icon.icns")], check=True)


if __name__ == "__main__":
    main()
