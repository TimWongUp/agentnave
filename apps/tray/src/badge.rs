//! The menu bar icon: the N with its dot, then "=" and the running count in the chosen font.
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tauri::image::Image;
use tauri::tray::TrayIcon;

use crate::data::snapshot::Overview;
use serde_json::Value;

const DRIP_FRAME: Duration = Duration::from_millis(33);
/// The dot falls onto the N like a drop and springs back (icons/drip.py draws the frames; the
/// last one is the rest pose).
const DRIP: [&[u8]; 18] = [
    include_bytes!("../icons/drip/00.png"),
    include_bytes!("../icons/drip/01.png"),
    include_bytes!("../icons/drip/02.png"),
    include_bytes!("../icons/drip/03.png"),
    include_bytes!("../icons/drip/04.png"),
    include_bytes!("../icons/drip/05.png"),
    include_bytes!("../icons/drip/06.png"),
    include_bytes!("../icons/drip/07.png"),
    include_bytes!("../icons/drip/08.png"),
    include_bytes!("../icons/drip/09.png"),
    include_bytes!("../icons/drip/10.png"),
    include_bytes!("../icons/drip/11.png"),
    include_bytes!("../icons/drip/12.png"),
    include_bytes!("../icons/drip/13.png"),
    include_bytes!("../icons/drip/14.png"),
    include_bytes!("../icons/drip/15.png"),
    include_bytes!("../icons/drip/16.png"),
    include_bytes!("../icons/drip/17.png"),
];
/// Font choices and the macOS fonts they draw with; the first is the fallback.
const FONTS: [(&str, &str); 4] = [
    ("baskerville", "Baskerville-SemiBoldItalic"),
    ("futura", "Futura-Medium"),
    ("typewriter", "AmericanTypewriter-Semibold"),
    ("marker", "MarkerFelt-Wide"),
];
/// Where "=" starts in the 44 px N frame.
#[cfg(target_os = "macos")]
const TEXT_AT: u32 = 40;

static DRIPPING: AtomicBool = AtomicBool::new(false);
/// The count drawn in the icon (0 draws none) and the font it is drawn in.
static SHOWN: Mutex<(usize, &str)> = Mutex::new((0, FONTS[0].0));

/// How many subagents to show: those running, unless the count is switched off or the system
/// is not macOS, whose menu bar alone shows it.
pub fn count(overview: &Overview, current: impl FnOnce() -> Value) -> usize {
    if cfg!(target_os = "macos") && current()["tray_count"] == true {
        overview.count("running")
    } else {
        0
    }
}

fn font(general: &Value) -> &'static str {
    let chosen = general["tray_font"].clone();
    FONTS
        .iter()
        .find(|(name, _)| chosen == *name)
        .unwrap_or(&FONTS[0])
        .0
}

/// The resting icon with `count` beside the N; set as the tray's first icon.
pub fn initial(count: usize, general: &Value) -> Image<'static> {
    *SHOWN.lock().unwrap() = (count, font(general));
    rest()
}

/// Redraws the icon for a new count or font; a new count also lets the dot drip.
pub fn show(tray: &TrayIcon, count: usize, current: impl Fn() -> Value) {
    let next = (count, font(&current()));
    let previous = std::mem::replace(&mut *SHOWN.lock().unwrap(), next);
    if previous.0 != count {
        drip(tray, &current());
    } else if previous.1 != next.1 && !DRIPPING.load(Ordering::Acquire) {
        let _ = tray.set_icon_with_as_template(Some(rest()), true);
    }
}

/// Plays the drip once with the current count; a still icon takes the count when motion is reduced.
pub fn drip(tray: &TrayIcon, general: &Value) {
    if general["motion"] == "reduce" {
        if !DRIPPING.load(Ordering::Acquire) {
            let _ = tray.set_icon_with_as_template(Some(rest()), true);
        }
        return;
    }
    if DRIPPING.swap(true, Ordering::AcqRel) {
        return;
    }
    let tray = tray.clone();
    thread::spawn(move || {
        for png in DRIP {
            let _ = tray.set_icon_with_as_template(Some(frame(png)), true);
            thread::sleep(DRIP_FRAME);
        }
        DRIPPING.store(false, Ordering::Release);
        // A count that changed while the last frame was drawn still reaches the icon.
        let _ = tray.set_icon_with_as_template(Some(rest()), true);
    });
}

fn rest() -> Image<'static> {
    frame(DRIP[DRIP.len() - 1])
}

/// One template image: the N frame, then "=count" on macOS; elsewhere the tray shows the N alone.
fn frame(png: &[u8]) -> Image<'static> {
    let n = Image::from_bytes(png).expect("bundled tray frames are valid PNGs");
    #[cfg(target_os = "macos")]
    {
        let (count, font) = *SHOWN.lock().unwrap();
        if count > 0 {
            let name = FONTS
                .iter()
                .find(|(key, _)| *key == font)
                .unwrap_or(&FONTS[0])
                .1;
            if let Some(text) = text::draw(name, &format!("={count}"), n.height()) {
                return beside(&n, &text);
            }
        }
    }
    n
}

/// The N with `text` laid from TEXT_AT; both are template black, so the more opaque pixel wins.
#[cfg(target_os = "macos")]
fn beside(n: &Image, text: &text::Text) -> Image<'static> {
    let height = n.height() as usize;
    let width = (TEXT_AT + text.width - text::PAD) as usize;
    let mut rgba = vec![0; width * height * 4];
    for (y, line) in n.rgba().chunks_exact(n.width() as usize * 4).enumerate() {
        rgba[y * width * 4..][..line.len()].copy_from_slice(line);
    }
    let left = (TEXT_AT - text::PAD) as usize;
    for (y, line) in text.rgba.chunks_exact(text.width as usize * 4).enumerate() {
        let start = (y * width + left) * 4;
        for (out, value) in rgba[start..start + line.len()].iter_mut().zip(line) {
            *out = (*out).max(*value);
        }
    }
    Image::new_owned(rgba, width as u32, height as u32)
}

/// Draws the count with the system's own fonts, so the repository ships no font or glyph images.
#[cfg(target_os = "macos")]
mod text {
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAttributedStringNSStringDrawing, NSBitmapImageRep, NSColor, NSDeviceRGBColorSpace,
        NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSGraphicsContext,
    };
    use objc2_core_foundation::CGPoint;
    use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

    /// Clear pixels on each side of the text's advance, for italic overhangs.
    pub const PAD: u32 = 4;
    /// The N's height in its 44 px frame (stroke ends included), and its baseline from the bottom.
    const CAP: f64 = 31.5;
    const BASELINE: f64 = 5.75;

    pub struct Text {
        pub width: u32,
        pub rgba: Vec<u8>,
    }

    /// `text` in the PostScript font `name`, its digits as tall as the N; None if the font is missing.
    pub fn draw(name: &str, text: &str, height: u32) -> Option<Text> {
        // The poll thread never drains a pool of its own; release AppKit's temporaries per draw.
        objc2::rc::autoreleasepool(|_| render(name, text, height))
    }

    fn render(name: &str, text: &str, height: u32) -> Option<Text> {
        let name = NSString::from_str(name);
        let probe = NSFont::fontWithName_size(&name, 40.0)?;
        let font = NSFont::fontWithName_size(&name, 40.0 * CAP / probe.capHeight())?;
        let black = NSColor::blackColor();
        let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let attributes: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&keys, &[font.as_ref() as &AnyObject, black.as_ref()]);
        let string = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attributes),
            )
        };
        let width = (string.size().width.ceil() as u32) + 2 * PAD;
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                width as isize,
                height as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )
        }?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        string.drawAtPoint(CGPoint::new(PAD as f64, BASELINE + font.descender()));
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let stride = rep.bytesPerRow() as usize;
        let data = rep.bitmapData();
        let row = width as usize * 4;
        let mut rgba = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            // SAFETY: the rep owns `height` rows of `stride` bytes, each holding `width` RGBA pixels.
            rgba.extend_from_slice(unsafe {
                std::slice::from_raw_parts(data.add(y * stride), row)
            });
        }
        Some(Text { width, rgba })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn every_font_draws_the_count_beside_the_n() {
        let n = Image::from_bytes(DRIP[DRIP.len() - 1]).unwrap();
        for (key, name) in FONTS {
            let text = text::draw(name, "=12", n.height()).unwrap_or_else(|| panic!("{key}"));
            let icon = beside(&n, &text);
            assert!(icon.width() > n.width(), "{key}");
            assert!(
                text.rgba
                    .iter()
                    .skip(3)
                    .step_by(4)
                    .any(|alpha| *alpha > 200),
                "{key}"
            );
        }
    }
}
