use crate::library::Library;
use crate::models::{PlayerState, Track};
use crate::mpris;
use crate::player::Player;
use async_channel::Sender;
use mpris_server::zbus;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.omamusic";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";

pub enum Command {
    ShowWindow,
    Quit,
}

#[derive(Default)]
struct NowPlaying {
    track: Option<Track>,
    state: PlayerState,
    cover: Option<String>,
}

pub fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub fn raise_running() -> bool {
    async_io::block_on(async {
        let Ok(connection) = zbus::Connection::session().await else {
            return false;
        };
        connection
            .call_method(
                Some(BUS_NAME),
                MPRIS_PATH,
                Some("org.mpris.MediaPlayer2"),
                "Raise",
                &(),
            )
            .await
            .is_ok()
    })
}

pub fn start(player: Arc<Player>, library: Arc<Library>, commands: Sender<Command>) {
    std::thread::spawn(move || async_io::block_on(sync(player, library, commands)));
}

async fn sync(player: Arc<Player>, library: Arc<Library>, commands: Sender<Command>) {
    let server = match mpris::serve(player.clone(), commands).await {
        Ok(server) => Some(server),
        Err(err) => {
            eprintln!("mpris: {err}");
            None
        }
    };
    let mut now = NowPlaying::default();
    let mut position = 0.0;
    loop {
        async_io::Timer::after(Duration::from_millis(300)).await;
        let track = player.current();
        let state = player.state();
        let track_changed = track.as_ref().map(|t| &t.path) != now.track.as_ref().map(|t| &t.path);
        let album_changed =
            track.as_ref().map(|t| &t.album_id) != now.track.as_ref().map(|t| &t.album_id);
        let changed = track_changed || state != now.state;

        let previous = position;
        position = player.position();
        let expected = previous
            + if state == PlayerState::Playing {
                0.3
            } else {
                0.0
            };
        if let Some(server) = &server {
            if !track_changed && (position - expected).abs() > 1.0 {
                server.seeked(position).await;
            }
        }

        if !changed {
            continue;
        }
        if album_changed {
            now.cover = track.as_ref().and_then(|t| cover(&library, &t.album_id));
        }
        now.track = track;
        now.state = state;
        if let Some(server) = &server {
            server.imp().set_art(now.cover.clone());
            server.changed(track_changed).await;
        }
    }
}

fn cover(library: &Library, album_id: &str) -> Option<String> {
    let large = library.covers().thumbnail(album_id, 512)?;
    let dir = dirs::cache_dir()?.join("omamusic").join("covers");
    std::fs::create_dir_all(&dir).ok()?;
    let file: PathBuf = dir.join(format!("{:016x}.png", hash(album_id)));
    if !file.exists() {
        std::fs::write(&file, large.bytes()).ok()?;
    }
    Some(format!("file://{}", file.display()))
}
