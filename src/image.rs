//! Terminal image rendering (M7).
//!
//! Wraps `ratatui-image`'s picker/protocol machinery so the rest of the
//! codebase can decode image files and render them via whatever graphics
//! protocol the current terminal supports (Kitty, iTerm2, Sixel, or the
//! halfblocks fallback). The picker is initialized lazily once per process.

use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

use anyhow::{Context, Result};
use image::DynamicImage;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

static PICKER: OnceLock<Mutex<Picker>> = OnceLock::new();

fn picker() -> &'static Mutex<Picker> {
    PICKER.get_or_init(|| {
        // Fallback path: if nobody called `init_early` before the terminal
        // was put in raw mode + alt screen, the query below will usually
        // fail (crossterm's event stream eats the response) and we end up
        // in halfblocks. Callers should prefer `init_early()` from main.
        let p = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        Mutex::new(p)
    })
}

/// Run the terminal-capability query eagerly, while stdio is still normal
/// (before `enable_raw_mode()` / alt-screen setup). This is the only way the
/// Kitty / iTerm2 / Sixel detection sequence can round-trip without being
/// eaten by the ratatui/crossterm event reader later. Idempotent.
pub fn init_early() {
    let _ = picker();
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

pub fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase),
        Some(ext) if IMAGE_EXTS.contains(&ext.as_str())
    )
}

/// Read + decode an image file. CPU-bound — spawn in a blocking task.
pub fn decode_image(path: &Path) -> Result<DynamicImage> {
    // Cap on decoded pixel count: 8 MP (≈ 32 MB RGBA) before we bail. Larger
    // images get rejected with an error rather than silently nuking memory.
    const MAX_PIXELS: u64 = 8 * 1024 * 1024;

    let reader = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("sniffing format of {}", path.display()))?;

    if let Some((w, h)) = reader
        .into_dimensions()
        .ok()
        .map(|d| (d.0 as u64, d.1 as u64))
        && w.saturating_mul(h) > MAX_PIXELS
    {
        anyhow::bail!(
            "image is {}×{} px (>{} MP cap); refusing to decode",
            w,
            h,
            MAX_PIXELS / 1_000_000
        );
    }

    image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("sniffing format of {}", path.display()))?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))
}

/// Wrap a decoded image in a ratatui-image stateful protocol for rendering.
pub fn new_protocol(img: DynamicImage) -> StatefulProtocol {
    picker()
        .lock()
        .expect("image picker mutex poisoned")
        .new_resize_protocol(img)
}

/// True when the terminal supports a real graphics protocol (Kitty / iTerm2
/// / Sixel), not just the halfblocks fallback. Callers use this to decide
/// whether to mention tmux passthrough in the onboarding toast.
pub fn has_graphics_protocol() -> bool {
    picker()
        .lock()
        .ok()
        .map(|p| !matches!(p.protocol_type(), ProtocolType::Halfblocks))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn recognises_common_image_extensions() {
        assert!(is_image_path(Path::new("a.png")));
        assert!(is_image_path(Path::new("a.PNG")));
        assert!(is_image_path(Path::new("photo.jpg")));
        assert!(is_image_path(Path::new("photo.JPEG")));
        assert!(is_image_path(Path::new("loop.gif")));
        assert!(is_image_path(Path::new("x.webp")));
    }

    #[test]
    fn rejects_non_images() {
        assert!(!is_image_path(Path::new("foo.rs")));
        assert!(!is_image_path(Path::new("README.md")));
        assert!(!is_image_path(Path::new("no_extension")));
    }
}
