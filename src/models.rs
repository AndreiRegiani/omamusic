use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: u32,
    pub year: u32,
    pub duration: f64,
    pub album_id: String,
}

#[derive(Debug, Clone)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: u32,
    pub tracks: Arc<[Track]>,
    pub track_titles: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Albums,
    Artists,
    Songs,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn next(&self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerState {
    #[default]
    Stopped,
    Playing,
    Paused,
}
