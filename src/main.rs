mod app;
mod artwork;
mod config;
mod fuzzy;
mod library;
mod metadata;
mod models;
mod player;

use app::Omamusic;
use gpui_kit::{actions, px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use std::sync::Arc;

use crate::config::AppConfig;
use crate::library::Library;
use crate::player::Player;

actions!(
    omamusic,
    [TogglePlay, NextTrack, PrevTrack, Rescan, FocusSearch]
);

fn main() {
    if std::env::args().any(|a| a == "--dump-library") {
        dump_library();
        return;
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_omarchy::init(cx);

            cx.bind_keys([
                gpui_kit::KeyBinding::new("space", TogglePlay, None),
                gpui_kit::KeyBinding::new("right", NextTrack, None),
                gpui_kit::KeyBinding::new("left", PrevTrack, None),
                gpui_kit::KeyBinding::new("ctrl-r", Rescan, None),
                gpui_kit::KeyBinding::new("ctrl-p", FocusSearch, None),
            ]);

            let library = Arc::new(Library::new().expect("library"));
            let player = Arc::new(Player::new(AppConfig::load().volume).expect("player"));

            cx.on_action({
                let player = player.clone();
                move |_: &TogglePlay, cx| {
                    player.toggle();
                    cx.refresh_windows();
                }
            });
            cx.on_action({
                let player = player.clone();
                move |_: &NextTrack, cx| {
                    player.next();
                    cx.refresh_windows();
                }
            });
            cx.on_action({
                let player = player.clone();
                move |_: &PrevTrack, cx| {
                    player.previous();
                    cx.refresh_windows();
                }
            });
            cx.on_action({
                let library = library.clone();
                move |_: &Rescan, cx| {
                    library.request_rescan();
                    cx.refresh_windows();
                }
            });

            cx.open_window(
                WindowOptions {
                    titlebar: None,
                    window_min_size: Some(size(px(560.), px(420.))),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1020.), px(680.)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| Omamusic::new(library.clone(), player.clone(), window, cx))
                },
            )
            .expect("open omamusic");
            cx.activate(true);
        });
}

fn dump_library() {
    let lib = Library::new().expect("library");
    for _ in 0..150 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let s = lib.snapshot();
        if !s.folders.is_empty() && !s.scanning && s.version > 0 {
            break;
        }
    }
    let s = lib.snapshot();
    println!("albums={} tracks={}", s.albums.len(), s.all_tracks.len());
    for a in &s.albums {
        println!(
            "ALBUM {:?} artist={:?} year={} n={}",
            a.title,
            a.artist,
            a.year,
            a.tracks.len()
        );
        for t in a.tracks.iter() {
            println!(
                "  TRK {:?} artist={:?} album={:?} no={} dur={:.1} file={}",
                t.title,
                t.artist,
                t.album,
                t.track_number,
                t.duration,
                t.path.display()
            );
        }
    }
    for q in [
        "neon",
        "test artist",
        "song",
        "midnight drive",
        "zzz-no-match",
    ] {
        lib.set_query(q.to_string());
        let s = lib.snapshot();
        println!(
            "query={q:?} albums={} tracks={} artists={}",
            s.filtered_albums.len(),
            s.filtered_tracks.len(),
            s.artists.len()
        );
    }
}
