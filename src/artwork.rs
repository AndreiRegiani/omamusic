use base64::{engine::general_purpose, Engine as _};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Covers {
    raw: HashMap<String, Vec<u8>>,
    rendered: HashMap<(String, u32), String>,
}

/// Raw covers and rendered thumbnails, invalidated together on library scans.
#[derive(Clone, Default)]
pub struct ArtworkCache {
    inner: Arc<Mutex<Covers>>,
}

impl ArtworkCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_all(&self, map: HashMap<String, Vec<u8>>) {
        let mut covers = self.inner.lock().unwrap();
        covers.raw = map;
        covers.rendered.clear();
    }

    pub fn data_uri(&self, album_id: &str, size: u32) -> Option<String> {
        let mut covers = self.inner.lock().unwrap();
        let key = (album_id.to_string(), size);
        if let Some(uri) = covers.rendered.get(&key) {
            return Some(uri.clone());
        }
        let (pixels, width, height) = decode_image(covers.raw.get(album_id)?, size)?;
        let image = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, pixels)?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let uri = format!(
            "data:image/png;base64,{}",
            general_purpose::STANDARD.encode(png)
        );
        covers.rendered.insert(key, uri.clone());
        Some(uri)
    }
}

/// Decode image bytes to RGBA pixels suitable for GPUI image rendering.
pub fn decode_image(data: &[u8], max_dim: u32) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(data).ok()?;
    let thumbnail = img.thumbnail(max_dim, max_dim);
    let rgba = thumbnail.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some((rgba.into_raw(), w, h))
}
