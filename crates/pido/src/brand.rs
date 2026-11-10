//! The product mark, embedded in the binary.
//!
//! gpui's image pipeline decodes raster data, so the pieces the UI draws are
//! PNGs carried in the executable (the SVGs in `assets/` are the source of
//! truth for documents and the web, where a vector is wanted). Each
//! `ImageSource` needs a stable `id` so gpui can cache the decoded texture.
//!
//! The wordmark comes in two palettes: the navy-to-cyan original for light
//! grounds, and a light-first version whose navy end would otherwise vanish
//! against a dark theme's background.

use std::sync::Arc;

use gpui::{Image, ImageFormat, ImageSource};

/// `pido`, set as the wordmark, for light backgrounds.
const WORDMARK_LIGHT: &[u8] = include_bytes!("../assets/pido-word-light.png");
/// The same wordmark, recoloured for dark backgrounds.
const WORDMARK_DARK: &[u8] = include_bytes!("../assets/pido-word-dark.png");

/// The wordmark for a dark or light ground, `id` distinct from other images
/// on screen.
pub fn wordmark(dark: bool, id: u64) -> ImageSource {
    let bytes = if dark { WORDMARK_DARK } else { WORDMARK_LIGHT };
    ImageSource::Image(Arc::new(Image {
        format: ImageFormat::Png,
        bytes: bytes.to_vec(),
        id,
    }))
}
