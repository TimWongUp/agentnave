"""Draw the menu bar icon's drip frames into icons/drip/NN.png (44 px, template black).

The dot falls onto the N's right stem like a drop of water, squashes, springs back above
its place and settles. Run with `python3 icons/drip.py` on macOS; AppKit draws the SVGs.
"""

import pathlib
import subprocess
import tempfile

HERE = pathlib.Path(__file__).parent

# (dot centre y, dot radius x, dot radius y, stem top y); the rest pose is tray.svg.
FRAMES = [
    (10.0, 4.0, 4.0, 20.0),
    (10.6, 3.9, 4.2, 20.0),
    (11.6, 3.8, 4.5, 20.0),
    (13.0, 3.6, 4.8, 20.0),
    (14.8, 3.5, 5.0, 20.0),
    (16.4, 4.4, 3.6, 20.5),
    (17.0, 5.0, 3.0, 21.2),
    (16.4, 4.6, 3.4, 20.6),
    (14.6, 3.7, 4.6, 20.0),
    (12.2, 3.6, 4.8, 20.0),
    (10.0, 3.7, 4.5, 20.0),
    (8.4, 3.9, 4.2, 20.0),
    (7.6, 4.0, 4.0, 20.0),
    (7.9, 4.0, 4.0, 20.0),
    (8.9, 3.9, 4.1, 20.0),
    (10.2, 4.2, 3.8, 20.0),
    (10.1, 4.1, 3.9, 20.0),
    (10.0, 4.0, 4.0, 20.0),
]

SVG = """<svg xmlns="http://www.w3.org/2000/svg" width="44" height="44" viewBox="0 0 44 44">
  <path d="M10 35V10L32 35V{top}" fill="none" stroke="#000" stroke-width="6.5" stroke-linecap="round" stroke-linejoin="round"/>
  <ellipse cx="32" cy="{cy}" rx="{rx}" ry="{ry}" fill="#000"/>
</svg>
"""

RENDER = """import AppKit
let a = CommandLine.arguments
for i in stride(from: 1, to: a.count, by: 2) {
    let img = NSImage(contentsOf: URL(fileURLWithPath: a[i]))!
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 44, pixelsHigh: 44, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    img.draw(in: NSRect(x: 0, y: 0, width: 44, height: 44))
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[i + 1]))
}
"""


def main() -> None:
    out = HERE / "drip"
    out.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        args: list[str] = []
        for i, (cy, rx, ry, top) in enumerate(FRAMES):
            svg = pathlib.Path(tmp, f"{i:02}.svg")
            svg.write_text(SVG.format(cy=cy, rx=rx, ry=ry, top=top))
            args += [str(svg), str(out / f"{i:02}.png")]
        script = pathlib.Path(tmp, "render.swift")
        script.write_text(RENDER)
        subprocess.run(["swift", str(script), *args], check=True)


if __name__ == "__main__":
    main()
