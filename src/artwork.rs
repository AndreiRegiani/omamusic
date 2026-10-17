use gpui_kit::{Image, ImageFormat};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
struct Covers {
    raw: HashMap<String, Vec<u8>>,
    rendered: HashMap<(String, u32), Arc<Image>>,
}

/// Raw covers and rendered thumbnails, invalidated together on library scans.
#[derive(Clone, Default)]
pub struct ArtworkCache {
    inner: Arc<Mutex<Covers>>,
}

impl ArtworkCache {
    pub fn replace_all(&self, map: HashMap<String, Vec<u8>>) {
        let mut covers = self.inner.lock();
        covers.raw = map;
        covers.rendered.clear();
    }

    pub fn thumbnail(&self, album_id: &str, size: u32) -> Option<Arc<Image>> {
        let mut covers = self.inner.lock();
        let key = (album_id.to_string(), size);
        if let Some(image) = covers.rendered.get(&key) {
            return Some(image.clone());
        }
        let mut png = Vec::new();
        image::load_from_memory(covers.raw.get(album_id)?)
            .ok()?
            .thumbnail(size, size)
            .to_rgba8()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, png));
        covers.rendered.insert(key, image.clone());
        Some(image)
    }
}
