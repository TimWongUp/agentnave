# Brand assets

This directory contains the project-local assets used by the AgentNave README.

- `readme-art.html` — editable source of the README art. It stacks the tray App's painted characters and
  office props from `apps/tray/ui/art/` the way the App does; `?view=banner|cast&lang=zh|en` picks the image.
- `readme-banner-zh.webp`, `readme-banner-en.webp` — rendered 1280×640 README banners (at 2×).
- `readme-cast-zh.webp`, `readme-cast-en.webp` — rendered cast sheets of the office characters (at 2×).
- `agentnave-banner.svg`, `agentnave-banner.png` — the previous banner, no longer referenced.
- `tim-wong-mark.png` — project-local copy of the canonical personal brand mark.

To re-render on macOS, screenshot each view with headless Chrome at 2× (banner 1280×640, cast 1280×480), trim
the transparent margin and save as WebP:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --hide-scrollbars \
  --default-background-color=00000000 --force-device-scale-factor=2 --window-size=1280,640 \
  --virtual-time-budget=5000 --screenshot=banner-zh.png \
  "file://$PWD/docs/assets/readme-art.html?view=banner&lang=zh"
```

The banner uses the Yuanti SC and Arial Rounded MT Bold fonts that ship with macOS.

The canonical personal-brand collection is maintained outside this repository. Preserve the mark's
colors, silhouette, proportions, and transparent background; do not redraw it with a generation
model.
