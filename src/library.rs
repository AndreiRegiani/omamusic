use crate::artwork::ArtworkCache;
use crate::config::AppConfig;
use crate::fuzzy::FuzzySearch;
use crate::metadata::MetadataReader;
use crate::models::{Album, Track};
use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct LibrarySnapshot {
    pub folders: Vec<PathBuf>,
    pub albums: Vec<Album>,
    pub filtered_albums: Vec<Album>,
    pub all_tracks: Vec<Track>,
    pub filtered_tracks: Vec<Track>,
    pub artists: Vec<(String, Vec<Album>)>,
    pub scanning: bool,
    pub query: String,
    pub version: u64,
}

pub struct Library {
    state: Arc<RwLock<LibrarySnapshot>>,
    covers: ArtworkCache,
    watcher: Mutex<Option<RecommendedWatcher>>,
    rescan_tx: Sender<()>,
}

impl Library {
    pub fn new() -> Result<Self> {
        let cfg = AppConfig::load();
        let state = Arc::new(RwLock::new(LibrarySnapshot {
            folders: cfg.folders.clone(),
            ..Default::default()
        }));
        let (tx, rx) = mpsc::channel::<()>();
        let rescan_tx = tx.clone();

        let watcher_opt = Self::make_watcher(&cfg.folders, &tx)?;

        let covers = ArtworkCache::new();
        let covers_c = covers.clone();
        let lib = Self {
            state: state.clone(),
            covers,
            watcher: Mutex::new(watcher_opt),
            rescan_tx,
        };

        // Background scan loop
        let state_c = state.clone();
        std::thread::spawn(move || {
            Self::do_scan(&state_c, &covers_c);
            while rx.recv().is_ok() {
                std::thread::sleep(Duration::from_millis(800));
                while rx.try_recv().is_ok() {}
                Self::do_scan(&state_c, &covers_c);
            }
        });

        Ok(lib)
    }

    pub fn snapshot(&self) -> LibrarySnapshot {
        self.state.read().clone()
    }

    pub fn covers(&self) -> &ArtworkCache {
        &self.covers
    }

    pub fn request_rescan(&self) {
        let _ = self.rescan_tx.send(());
    }

    pub fn set_query(&self, q: String) {
        {
            let mut s = self.state.write();
            if s.query == q {
                return;
            }
            s.query = q;
            Self::apply_search(&mut s);
            s.version += 1;
        }
    }

    pub fn add_folder(&self, path: PathBuf) {
        if !path.is_dir() {
            eprintln!("music folder does not exist: {}", path.display());
            return;
        }
        {
            let mut s = self.state.write();
            if s.folders.contains(&path) {
                return;
            }
            // prune subsumed
            s.folders.retain(|f| !f.starts_with(&path));
            if s.folders.iter().any(|f| path.starts_with(f)) {
                return;
            }
            s.folders.push(path);
            let mut cfg = AppConfig::load();
            cfg.folders = s.folders.clone();
            let _ = cfg.save();
        }
        // Folder changes need a fresh watcher: the original watcher only knows
        // about folders present when the application started.
        self.refresh_watches();
        self.request_rescan();
    }

    pub fn remove_folder(&self, path: &PathBuf) {
        {
            let mut s = self.state.write();
            s.folders.retain(|f| f != path);
            let mut cfg = AppConfig::load();
            cfg.folders = s.folders.clone();
            let _ = cfg.save();
        }
        self.refresh_watches();
        self.request_rescan();
    }

    fn refresh_watches(&self) {
        let folders = self.state.read().folders.clone();
        match Self::make_watcher(&folders, &self.rescan_tx) {
            Ok(watcher) => *self.watcher.lock() = watcher,
            Err(err) => eprintln!("watch music folders: {err:#}"),
        }
    }

    fn make_watcher(folders: &[PathBuf], tx: &Sender<()>) -> Result<Option<RecommendedWatcher>> {
        if folders.is_empty() {
            return Ok(None);
        }
        let tx = tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                    ) {
                        let _ = tx.send(());
                    }
                }
            })?;
        for folder in folders {
            if folder.is_dir() {
                watcher.watch(folder, RecursiveMode::Recursive)?;
            }
        }
        Ok(Some(watcher))
    }

    fn apply_search(s: &mut LibrarySnapshot) {
        if s.query.trim().is_empty() {
            s.filtered_albums = s.albums.clone();
            s.filtered_tracks = s.all_tracks.clone();
        } else {
            let search = FuzzySearch::new(&s.query);
            let mut scored: Vec<(i32, usize)> = Vec::new();
            for (i, a) in s.albums.iter().enumerate() {
                let sc = search.score(&a.title).max(search.score(&a.artist)).max(
                    search.score(
                        &a.tracks
                            .iter()
                            .map(|t| t.title.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                );
                if sc > 0 {
                    scored.push((sc, i));
                }
            }
            scored.sort_by_key(|a| std::cmp::Reverse(a.0));
            scored.truncate(300);
            s.filtered_albums = scored
                .into_iter()
                .map(|(_, i)| s.albums[i].clone())
                .collect();

            let mut tscored: Vec<(i32, usize)> = Vec::new();
            for (i, t) in s.all_tracks.iter().enumerate() {
                let sc = search
                    .score(&t.title)
                    .max(search.score(&t.artist))
                    .max(search.score(&t.album));
                if sc > 0 {
                    tscored.push((sc, i));
                }
            }
            tscored.sort_by_key(|a| std::cmp::Reverse(a.0));
            tscored.truncate(500);
            s.filtered_tracks = tscored
                .into_iter()
                .map(|(_, i)| s.all_tracks[i].clone())
                .collect();
        }

        let mut map: HashMap<String, Vec<Album>> = HashMap::new();
        for a in &s.filtered_albums {
            map.entry(a.artist.clone()).or_default().push(a.clone());
        }
        let mut artists: Vec<_> = map
            .into_iter()
            .map(|(name, mut albums)| {
                albums.sort_by(|a, b| a.year.cmp(&b.year).then_with(|| a.title.cmp(&b.title)));
                (name, albums)
            })
            .collect();
        artists.sort_by_key(|a| a.0.to_lowercase());
        s.artists = artists;
    }

    fn do_scan(state: &Arc<RwLock<LibrarySnapshot>>, covers: &ArtworkCache) {
        {
            state.write().scanning = true;
        }
        let folders = state.read().folders.clone();
        if folders.is_empty() {
            let mut s = state.write();
            s.albums.clear();
            s.all_tracks.clear();
            s.filtered_albums.clear();
            s.filtered_tracks.clear();
            s.artists.clear();
            s.scanning = false;
            s.version += 1;
            return;
        }

        let reader = MetadataReader::new();
        let exts = [
            "mp3", "flac", "ogg", "m4a", "mp4", "aac", "wav", "aiff", "caf",
        ];
        let mut paths: Vec<PathBuf> = Vec::new();
        for folder in &folders {
            for entry in walkdir::WalkDir::new(folder)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().is_file()
                        && e.path()
                            .extension()
                            .and_then(|x| x.to_str())
                            .map(|x| exts.contains(&x.to_lowercase().as_str()))
                            .unwrap_or(false)
                })
            {
                paths.push(entry.path().to_path_buf());
            }
        }

        // Parallel-ish: blocking in thread pool via spawn_blocking chunks
        let mut album_map: HashMap<String, Vec<Track>> = HashMap::new();
        let mut cover_map: HashMap<String, Vec<u8>> = HashMap::new();

        for p in paths {
            if let Ok(md) = reader.read(&p) {
                let dir = p
                    .parent()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_default();
                let album_name = if md.album.is_empty() {
                    p.parent()
                        .and_then(|d| d.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown Album")
                        .to_string()
                } else {
                    md.album.clone()
                };
                let key = format!("{dir}\n{album_name}");
                let track = Track {
                    path: p.clone(),
                    title: if md.title.is_empty() {
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                            .to_string()
                    } else {
                        md.title.clone()
                    },
                    artist: if md.artist.is_empty() {
                        "Unknown Artist".to_string()
                    } else {
                        md.artist.clone()
                    },
                    album: album_name,
                    track_number: md.track_number,
                    year: md.year,
                    duration: md.duration.max(0.0),
                    album_id: key.clone(),
                };
                if let Some(art) = md.artwork {
                    cover_map.entry(key.clone()).or_insert(art);
                }
                album_map.entry(key).or_default().push(track.clone());
            }
        }

        let mut albums: Vec<Album> = Vec::with_capacity(album_map.len());
        for (id, mut tracks) in album_map {
            tracks.sort_by(|a, b| {
                if a.track_number > 0 && b.track_number > 0 && a.track_number != b.track_number {
                    a.track_number.cmp(&b.track_number)
                } else {
                    a.path.cmp(&b.path)
                }
            });
            let artist = if tracks.iter().all(|t| t.artist == tracks[0].artist) {
                tracks[0].artist.clone()
            } else {
                "Various Artists".to_string()
            };
            let year = tracks.iter().map(|t| t.year).max().unwrap_or(0);
            let title = tracks[0].album.clone();
            albums.push(Album {
                id,
                title,
                artist,
                year,
                tracks,
            });
        }
        albums.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then_with(|| a.year.cmp(&b.year))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });

        covers.replace_all(cover_map);

        // Song order follows album order, like the reference player.
        let all_tracks = albums.iter().flat_map(|a| a.tracks.clone()).collect();
        let mut s = state.write();
        s.albums = albums;
        s.all_tracks = all_tracks;
        s.scanning = false;
        Self::apply_search(&mut s);
        s.version += 1;
    }
}
