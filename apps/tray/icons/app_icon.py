"""Draw the App icon (Dock, Finder, installer) from the N mark.

The mark is tray.svg's one-stroke N with its dot. On macOS 26 the icon follows the system
appearance: a black N on a light square in light mode, a white N on a dark square in dark mode.
That pair lives in AppIcon.icon (Icon Composer) and is compiled into Assets.car; icon.icns,
icon.ico and the PNGs carry the light version for older macOS, Windows and the bundler.

Run with `python3 icons/app_icon.py` on macOS with Xcode 26 or later: AppKit draws the SVGs,
actool compiles the catalog and iconutil packs the icns. CI only copies Assets.car, so its
older Xcode cannot drop the dark version.
"""

import json
import pathlib
import subprocess
import tempfile

from PIL import Image

HERE = pathlib.Path(__file__).parent
LIGHT, DARK = "#f5f5f7", "#1c1c1e"

# 1024 canvas; the 44-unit mark spans about 52% of the square. The flat icon's square is the
# 824 body on Apple's icon grid (scale 14.5); Icon Composer's square is the whole canvas (scale 18).
# The letter grows but keeps the earlier weight: stroke and dot stay at 10.3% and 6.3% of the
# square, so in mark units they are thinner than tray.svg's 6.5 and 4.
MARK = """<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  {back}
  <g transform="{place}">
    <path d="M10 35V10L32 35V20" fill="none" stroke="{ink}" stroke-width="5.83" stroke-linecap="round" stroke-linejoin="round"/>
    <circle cx="32" cy="10" r="3.59" fill="{ink}"/>
  </g>
</svg>
"""
# The flat icon draws its own square (an 824 body inset by 100, corners of 185) with a faint rim
# so the light square stays visible on a light desktop.
SQUARE = f'<rect x="100" y="100" width="824" height="824" rx="185" fill="{LIGHT}" stroke="#d2d2d7" stroke-width="2"/>'

# Icon Composer draws the square itself; only its fill and the glyph image change with appearance.
ICON_JSON = {
    "fill-specializations": [
        {"value": {"solid": "srgb:0.96078,0.96078,0.96863,1.00000"}},
        {"appearance": "dark", "value": {"solid": "srgb:0.10980,0.10980,0.11765,1.00000"}},
    ],
    "groups": [
        {
            "layers": [
                {
                    "image-name": "N.png",
                    "image-name-specializations": [
                        {"value": "N.png"},
                        {"appearance": "dark", "value": "N-dark.png"},
                    ],
                    "name": "N",
                    "glass": False,
                }
            ]
        }
    ],
    "supported-platforms": {"squares": "shared"},
}

RENDER = """import AppKit
let a = CommandLine.arguments
for i in stride(from: 1, to: a.count, by: 2) {
    let img = NSImage(contentsOf: URL(fileURLWithPath: a[i]))!
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1024, pixelsHigh: 1024, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    img.draw(in: NSRect(x: 0, y: 0, width: 1024, height: 1024))
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[i + 1]))
}
"""


def main() -> None:
    catalog = HERE / "AppIcon.icon"
    (catalog / "Assets").mkdir(parents=True, exist_ok=True)
    (catalog / "icon.json").write_text(json.dumps(ICON_JSON, indent=2) + "\n")
    with tempfile.TemporaryDirectory() as tmp:
        t = pathlib.Path(tmp)
        flat, layer = "translate(200 192) scale(14.5)", "translate(125 114) scale(18)"
        (t / "flat.svg").write_text(MARK.format(back=SQUARE, place=flat, ink=DARK))
        (t / "glyph.svg").write_text(MARK.format(back="", place=layer, ink=DARK))
        (t / "glyph-dark.svg").write_text(MARK.format(back="", place=layer, ink=LIGHT))
        (t / "render.swift").write_text(RENDER)
        drawings = [
            (t / "flat.svg", t / "flat.png"),
            (t / "glyph.svg", catalog / "Assets" / "N.png"),
            (t / "glyph-dark.svg", catalog / "Assets" / "N-dark.png"),
        ]
        subprocess.run(
            ["swift", str(t / "render.swift"), *(str(p) for pair in drawings for p in pair)],
            check=True,
        )

        big = Image.open(t / "flat.png")
        big.save(HERE / "source.png")
        for name, size in (
            ("32x32.png", 32),
            ("128x128.png", 128),
            ("128x128@2x.png", 256),
            ("icon.png", 512),
        ):
            big.resize((size, size), Image.LANCZOS).save(HERE / name)
        big.save(HERE / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 256)])
        iconset = t / "icon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            big.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
            big.resize((size * 2, size * 2), Image.LANCZOS).save(
                iconset / f"icon_{size}x{size}@2x.png"
            )
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(HERE / "icon.icns")], check=True
        )

        out = t / "car"
        out.mkdir()
        subprocess.run(
            [
                "xcrun",
                "actool",
                str(catalog),
                "--compile",
                str(out),
                "--output-format",
                "human-readable-text",
                "--output-partial-info-plist",
                str(out / "partial.plist"),
                "--app-icon",
                "AppIcon",
                "--include-all-app-icons",
                "--enable-on-demand-resources",
                "NO",
                "--development-region",
                "en",
                "--target-device",
                "mac",
                "--minimum-deployment-target",
                "11.0",
                "--platform",
                "macosx",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        (HERE / "Assets.car").write_bytes((out / "Assets.car").read_bytes())


if __name__ == "__main__":
    main()
