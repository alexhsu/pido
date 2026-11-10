//! The product mark, embedded in the binary.
//!
//! gpui's image pipeline decodes raster data, so the pieces the UI draws are
//! PNGs carried in the executable (the SVG in `assets/` is the source of truth
//! for documents and the web, where a vector is wanted). Each `ImageSource`
//! needs a stable `id` so gpui can cache the decoded texture.

use std::sync::Arc;

use gpui::{Image, ImageFormat, ImageSource};

/// `pido`, set as the wordmark. For the welcome screen.
const WORDMARK: &[u8] = include_bytes!("../assets/pido-word-1024.png");
/// The π-in-ring mark alone. For small, square slots such as the sidebar.
const MARK: &[u8] = include_bytes!("../assets/pido-mark-256.png");

/// The wordmark, `id` distinct from other images on screen.
pub fn wordmark(id: u64) -> ImageSource {
    ImageSource::Image(Arc::new(Image {
        format: ImageFormat::Png,
        bytes: WORDMARK.to_vec(),
        id,
    }))
}

/// The square mark, `id` distinct from other images on screen.
pub fn mark(id: u64) -> ImageSource {
    ImageSource::Image(Arc::new(Image {
        format: ImageFormat::Png,
        bytes: MARK.to_vec(),
        id,
    }))
}
