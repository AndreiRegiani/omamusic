use async_channel::{Receiver, Sender};
use gpui_kit::{Image, ImageFormat};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type Key = (String, u32);

#[derive(Default)]
struct Covers {
    raw: HashMap<String, Arc<[u8]>>,
    rendered: HashMap<Key, Arc<Image>>,
    pending: HashSet<Key>,
    generation: u64,
}

/// Raw covers and rendered thumbnails, invalidated together on library scans.
#[derive(Clone)]
pub struct ArtworkCache {
    inner: Arc<Mutex<Covers>>,
    requests: Sender<Key>,
    ready: Receiver<()>,
}

impl Default for ArtworkCache {
    fn default() -> Self {
        let inner = Arc::new(Mutex::new(Covers::default()));
        let (requests, jobs) = async_channel::unbounded::<Key>();
        let (done, ready) = async_channel::bounded(1);
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
        for _ in 0..workers {
            let (inner, jobs, done) = (inner.clone(), jobs.clone(), done.clone());
            std::thread::spawn(move || {
                while let Ok(key) = jobs.recv_blocking() {
                    render_into(&inner, key);
                    let _ = done.try_send(());
                }
            });
        }
        Self {
            inner,
            requests,
            ready,
        }
    }
}

fn render(raw: &[u8], size: u32) -> Option<Arc<Image>> {
    let mut png = Vec::new();
    image::load_from_memory(raw)
        .ok()?
        .thumbnail(size, size)
        .to_rgba8()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)))
}

fn render_into(inner: &Mutex<Covers>, key: Key) {
    let (raw, generation) = {
        let covers = inner.lock();
        if covers.rendered.contains_key(&key) {
            return;
        }
        (covers.raw.get(&key.0).cloned(), covers.generation)
    };
    let image = raw.and_then(|raw| render(&raw, key.1));
    let mut covers = inner.lock();
    if let Some(image) = image.filter(|_| covers.generation == generation) {
        covers.pending.remove(&key);
        covers.rendered.insert(key, image);
    }
}

impl ArtworkCache {
    pub fn replace_all(&self, map: HashMap<String, Vec<u8>>) {
        let mut covers = self.inner.lock();
        covers.raw = map.into_iter().map(|(k, v)| (k, v.into())).collect();
        covers.rendered.clear();
        covers.pending.clear();
        covers.generation += 1;
    }

    pub fn cached(&self, album_id: &str, size: u32) -> Option<Arc<Image>> {
        let mut covers = self.inner.lock();
        let key = (album_id.to_string(), size);
        if let Some(image) = covers.rendered.get(&key) {
            return Some(image.clone());
        }
        if covers.raw.contains_key(album_id) && covers.pending.insert(key.clone()) {
            let _ = self.requests.try_send(key);
        }
        None
    }

    pub fn ready(&self) -> Receiver<()> {
        self.ready.clone()
    }
}
