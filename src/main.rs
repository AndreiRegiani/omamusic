mod app;
mod artwork;
mod config;
mod desktop;
mod fuzzy;
mod library;
mod metadata;
mod models;
mod mpris;
mod player;
mod tray;

use app::Omamusic;
use desktop::Command;
use gpui_kit::{actions, px, size, App, AppContext, Bounds, QuitMode, WindowBounds, WindowOptions};
use std::sync::Arc;

use crate::config::AppConfig;
use crate::library::Library;
use crate::player::Player;

actions!(
    omamusic,
    [
        TogglePlay,
        NextTrack,
        PrevTrack,
        Rescan,
        FocusSearch,
        GoBack,
        Quit
    ]
);

fn main() {
    if std::env::args().any(|a| a == "--dump-library") {
        dump_library();
        return;
    }
    if desktop::raise_running() {
        return;
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_omarchy::init(cx);
            cx.set_quit_mode(QuitMode::Explicit);

            cx.bind_keys([
                gpui_kit::KeyBinding::new("space", TogglePlay, None),
                gpui_kit::KeyBinding::new("right", NextTrack, None),
                gpui_kit::KeyBinding::new("left", PrevTrack, None),
                gpui_kit::KeyBinding::new("ctrl-r", Rescan, None),
                gpui_kit::KeyBinding::new("ctrl-p", FocusSearch, None),
                gpui_kit::KeyBinding::new("alt-left", GoBack, None),
                gpui_kit::KeyBinding::new("ctrl-q", Quit, None),
            ]);

            let library = Arc::new(Library::new().expect("library"));
            let config = AppConfig::load();
            let player = Arc::new(Player::new(config.volume).expect("player"));
            desktop::set_cover_icon(!config.plain_tray_icon);
            let (commands, received) = async_channel::unbounded();
            let has_tray = desktop::start(player.clone(), library.clone(), commands);

            cx.on_action(|_: &Quit, cx| cx.quit());

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

            cx.on_window_closed(move |cx, _| {
                if !cx.windows().is_empty() {
                    return;
                }
                let config = AppConfig::load();
                if !has_tray || config.quit_on_close {
                    cx.quit();
                } else if !config.tray_hint_shown {
                    AppConfig::update(|c| c.tray_hint_shown = true);
                    desktop::notify(
                        "omamusic is still running",
                        "Find it in the tray. To stop playback and exit, choose Quit from the tray menu.",
                    );
                }
            })
            .detach();

            cx.spawn({
                let library = library.clone();
                let player = player.clone();
                async move |cx| {
                    while let Ok(command) = received.recv().await {
                        cx.update(|cx| match command {
                            Command::Quit => cx.quit(),
                            Command::ShowWindow => show_window(&library, &player, cx),
                            Command::ToggleWindow => match cx.windows().first() {
                                Some(window) => {
                                    let _ = window.update(cx, |_, window, _| window.remove_window());
                                }
                                None => show_window(&library, &player, cx),
                            },
                        });
                    }
                }
            })
            .detach();

            if !(has_tray && config.start_hidden) {
                show_window(&library, &player, cx);
            }
            cx.activate(true);
        });
}

fn show_window(library: &Arc<Library>, player: &Arc<Player>, cx: &mut App) {
    if let Some(window) = cx.windows().first() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let (library, player) = (library.clone(), player.clone());
    let opened = cx.open_window(
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
        |window, cx| cx.new(|cx| Omamusic::new(library, player, window, cx)),
    );
    if let Err(err) = opened {
        eprintln!("open window: {err:#}");
    }
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
