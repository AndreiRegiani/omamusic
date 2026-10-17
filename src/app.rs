use crate::config::AppConfig;
use crate::library::{Library, LibrarySnapshot};
use crate::models::{Album, PlayerState, RepeatMode, Section, Track};
use crate::player::Player;
use gpui_kit::base::input::InputState;
use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, rems, AnyElement, App, AppContext, ClickEvent, Context, ElementId, Entity,
    ImageSource, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Render,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_omarchy::{
    avatar, avatar_image, button, empty_state, focus_scope, icon, icon_button, input, panel,
    separator, slider, with_tooltip, ActiveTheme, ButtonVariant, IconName,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const PROGRESS_STEPS: f32 = 1000.;

pub struct Omamusic {
    pub library: Arc<Library>,
    pub player: Arc<Player>,
    section: Section,
    query: Entity<InputState>,
    volume: Entity<SliderState>,
    progress_state: Entity<SliderState>,
    selected_album: Option<String>,
    show_settings: bool,
    lib_version: u64,
    was_playing: bool,
    seeking: bool,
    new_folder: Entity<InputState>,
}

fn initials(text: &str) -> String {
    text.chars().take(2).collect()
}

fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "0:00".into();
    }
    let t = secs as u64;
    let m = t / 60;
    let s = t % 60;
    if m >= 60 {
        format!("{}:{:02}:{:02}", m / 60, m % 60, s)
    } else {
        format!("{m}:{s:02}")
    }
}

impl Omamusic {
    pub fn new(
        library: Arc<Library>,
        player: Arc<Player>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
        let new_folder = cx.new(|cx| InputState::new(window, cx).placeholder("Enter folder path"));
        let vol = player.volume() as f32;
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(vol)
        });
        let progress_state = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(PROGRESS_STEPS)
                .step(1.)
                .default_value(0.)
        });

        cx.observe(&query, |this, state, cx| {
            let text = state.read(cx).text().to_string();
            this.library.set_query(text);
            this.selected_album = None;
            cx.notify();
        })
        .detach();

        cx.subscribe(&volume, |this, _, event: &SliderEvent, _| match event {
            SliderEvent::Change(value) => this.player.set_volume(value.end() as f64),
            SliderEvent::Release(_) => {
                let volume = this.player.volume();
                AppConfig::update(|c| c.volume = volume);
            }
        })
        .detach();

        cx.subscribe(
            &progress_state,
            |this, _, event: &SliderEvent, _| match event {
                SliderEvent::Change(_) => this.seeking = true,
                SliderEvent::Release(value) => {
                    this.seeking = false;
                    let dur = this.player.duration().max(1.0);
                    this.player
                        .seek(value.end() as f64 / PROGRESS_STEPS as f64 * dur);
                }
            },
        )
        .detach();

        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let alive = this
                .update_in(cx, |this, window, cx| this.tick(window, cx))
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();

        let view = std::env::var("OMADRIVE_VIEW").ok();
        let (section, selected_album, show_settings) = match view.as_deref() {
            Some("artists") => (Section::Artists, None, false),
            Some("songs") => (Section::Songs, None, false),
            Some("settings") => (Section::Albums, None, true),
            Some("album") => {
                let first = library.snapshot().albums.first().map(|a| a.id.clone());
                (Section::Albums, first, false)
            }
            _ => (Section::Albums, None, false),
        };

        Self {
            library,
            player,
            section,
            query,
            volume,
            progress_state,
            selected_album,
            show_settings,
            lib_version: 0,
            was_playing: false,
            seeking: false,
            new_folder,
        }
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let version = self.library.version();
        let playing = self.player.state() == PlayerState::Playing;
        if version == self.lib_version && !playing && !self.was_playing {
            return;
        }
        self.lib_version = version;
        self.was_playing = playing;
        if !self.seeking {
            let dur = self.player.duration().max(1.0) as f32;
            let value =
                (self.player.position() as f32 / dur * PROGRESS_STEPS).clamp(0., PROGRESS_STEPS);
            self.progress_state
                .update(cx, |s, cx| s.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn cover(&self, album_id: Option<&str>, size: f32, name: &str, cx: &App) -> impl IntoElement {
        let cover = avatar(initials(name), cx).size(px(size));
        match album_id.and_then(|id| self.library.covers().thumbnail(id, size as u32 * 2)) {
            Some(image) => cover.image(avatar_image(ImageSource::Image(image))),
            None => cover,
        }
    }

    fn control_button(
        id: &'static str,
        icon_name: IconName,
        label: &'static str,
        variant: ButtonVariant,
        tooltip: &'static str,
        action: fn(&Player),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        with_tooltip(
            icon_button(id, icon_name, label, variant, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    action(&this.player);
                    cx.notify();
                },
            )),
            tooltip,
        )
    }

    fn browse_folder(&mut self, cx: &mut Context<Self>) {
        let path = self.new_folder.read(cx).text().to_string();
        if !path.is_empty() {
            self.library.add_folder(PathBuf::from(path));
            cx.notify();
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Music Folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        this.library.add_folder(path);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

fn sidebar_btn(
    id: impl Into<ElementId>,
    label: &str,
    icon_name: IconName,
    selected: bool,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let t = cx.omarchy().clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(rems(0.5))
        .px(rems(0.625))
        .py(rems(0.4375))
        .rounded_none()
        .cursor_pointer()
        .text_size(rems(0.75))
        .font_family(t.font.clone())
        .text_color(if selected { t.foreground } else { t.secondary })
        .bg(if selected {
            t.selected_fill()
        } else {
            t.background
        })
        .hover(|s| s.bg(t.hover_fill()))
        .child(icon(icon_name).size(rems(0.875)))
        .child(label.to_string())
        .on_click(on_click)
}

impl Render for Omamusic {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.library.snapshot();
        let current = self.player.current();
        let playing = self.player.state() == PlayerState::Playing;
        let theme = cx.omarchy().clone();
        let nav = [
            ("nav-albums", "Albums", IconName::Disc3, Section::Albums),
            ("nav-artists", "Artists", IconName::Mic, Section::Artists),
            ("nav-songs", "Songs", IconName::Music, Section::Songs),
        ];

        focus_scope("omamusic")
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_w(px(480.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.5))
                            .px(rems(0.75))
                            .py(rems(0.5))
                            .border_b_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .child(div().flex_1().min_w_0().max_w(rems(28.)).child(input(
                                "search",
                                &self.query,
                                window,
                                cx,
                            )))
                            .child(with_tooltip(
                                icon_button(
                                    "rescan",
                                    IconName::RefreshCw,
                                    "Rescan library",
                                    ButtonVariant::Secondary,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.library.request_rescan();
                                        cx.notify();
                                    },
                                )),
                                "Rescan library (Ctrl+R)",
                            ))
                            .child(with_tooltip(
                                icon_button(
                                    "settings",
                                    IconName::Settings,
                                    "Settings",
                                    ButtonVariant::Secondary,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_settings = !this.show_settings;
                                        cx.notify();
                                    },
                                )),
                                "Settings",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .p(rems(0.5))
                                    .w(rems(11.))
                                    .min_w(rems(8.))
                                    .flex_shrink_0()
                                    .border_r_1()
                                    .border_color(theme.border)
                                    .bg(theme.background)
                                    .children(nav.map(|(id, label, icon_name, section)| {
                                        sidebar_btn(
                                            id,
                                            label,
                                            icon_name,
                                            self.section == section,
                                            cx,
                                            cx.listener(move |this, _, _, cx| {
                                                this.section = section;
                                                this.selected_album = None;
                                                cx.notify();
                                            }),
                                        )
                                    }))
                                    .child(separator(cx))
                                    .child(
                                        div()
                                            .text_size(rems(0.6875))
                                            .text_color(theme.secondary)
                                            .child(format!(
                                                "{} albums · {} songs",
                                                snap.albums.len(),
                                                snap.all_tracks.len()
                                            )),
                                    )
                                    .when(snap.scanning, |d| {
                                        d.child(
                                            div()
                                                .text_size(rems(0.6875))
                                                .text_color(theme.secondary)
                                                .child("Scanning…"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .bg(theme.background)
                                    .child(if snap.folders.is_empty() {
                                        empty_state(
                                            "No Music",
                                            "Add a folder with music in Settings.",
                                            cx,
                                        )
                                        .into_any_element()
                                    } else {
                                        self.render_content(&snap, cx)
                                    }),
                            ),
                    )
                    .child(separator(cx))
                    .child(self.render_player_bar(current.as_ref(), playing, window, cx))
                    .when(self.show_settings, |d| {
                        d.child(self.render_settings(&snap, window, cx))
                    }),
            )
    }
}

impl Omamusic {
    fn render_content(&mut self, snap: &LibrarySnapshot, cx: &mut Context<Self>) -> AnyElement {
        if let Some(album) = self
            .selected_album
            .as_ref()
            .and_then(|id| snap.albums.iter().find(|a| &a.id == id))
        {
            return self.render_album_detail(album, cx);
        }
        match self.section {
            Section::Albums => self.render_album_grid(snap, cx),
            Section::Artists => self.render_artist_list(snap, cx),
            Section::Songs => self.render_song_list(snap, cx),
        }
    }

    fn render_album_grid(&mut self, snap: &LibrarySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        div()
            .id("albums-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(rems(0.75))
            .child(
                div().flex().flex_wrap().gap(rems(0.75)).children(
                    snap.filtered_albums
                        .iter()
                        .take(400)
                        .enumerate()
                        .map(|(idx, a)| {
                            let id = a.id.clone();
                            let n = a.tracks.len();
                            let count = format!("{n} song{}", if n == 1 { "" } else { "s" });
                            div()
                                .id(("album", idx))
                                .w(rems(10.))
                                .min_w(rems(8.))
                                .flex()
                                .flex_col()
                                .gap(rems(0.375))
                                .p(rems(0.5))
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.background)
                                .hover(|s| s.bg(theme.hover_fill()))
                                .cursor_pointer()
                                .child(self.cover(Some(&a.id), 128., &a.title, cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected_album = Some(id.clone());
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .text_size(rems(0.75))
                                        .text_color(theme.foreground)
                                        .child(a.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(rems(0.6875))
                                        .text_color(theme.secondary)
                                        .child(a.artist.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(rems(0.6875))
                                        .text_color(theme.secondary)
                                        .child(count),
                                )
                        }),
                ),
            )
            .into_any_element()
    }

    fn render_artist_list(&mut self, snap: &LibrarySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        div()
            .id("artists-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(
                        snap.artists
                            .iter()
                            .enumerate()
                            .map(|(idx, (name, albums))| {
                                let song_count: usize = albums.iter().map(|a| a.tracks.len()).sum();
                                let an = albums.len();
                                let label = format!(
                                    "{name} · {an} album{} · {song_count} songs",
                                    if an == 1 { "" } else { "s" }
                                );
                                let aname = name.clone();
                                div()
                                    .id(("artist", idx))
                                    .flex()
                                    .items_center()
                                    .gap(rems(0.5))
                                    .px(rems(0.75))
                                    .py(rems(0.5))
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .hover(|s| s.bg(theme.hover_fill()))
                                    .cursor_pointer()
                                    .child(self.cover(
                                        albums.first().map(|a| a.id.as_str()),
                                        40.,
                                        name,
                                        cx,
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.section = Section::Albums;
                                        this.query.update(cx, |q, cx| {
                                            q.set_value(aname.as_str(), window, cx)
                                        });
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .text_size(rems(0.75))
                                            .text_color(theme.foreground)
                                            .child(label),
                                    )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_song_list(&mut self, snap: &LibrarySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        let tracks = snap.filtered_tracks.clone();
        div()
            .id("songs-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(tracks.iter().take(800).enumerate().map(|(i, t)| {
                        let all = tracks.clone();
                        let bg = if i % 2 == 0 {
                            theme.background
                        } else {
                            theme.hover_fill()
                        };
                        div()
                            .id(("song", i))
                            .flex()
                            .items_center()
                            .gap(rems(0.75))
                            .px(rems(0.75))
                            .py(rems(0.375))
                            .bg(bg)
                            .hover(|s| s.bg(theme.hover_fill()))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.player.play(all.to_vec(), i);
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(rems(2.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(format!("{}", i + 1)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(rems(0.75))
                                    .text_color(theme.foreground)
                                    .child(t.title.clone()),
                            )
                            .child(
                                div()
                                    .w(rems(10.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(t.artist.clone()),
                            )
                            .child(
                                div()
                                    .w(rems(10.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(t.album.clone()),
                            )
                            .child(
                                div()
                                    .w(rems(3.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(format_time(t.duration)),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_album_detail(&mut self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        let total = album.tracks.iter().map(|t| t.duration).sum::<f64>();
        let subtitle = format!(
            "{} · {} songs · {}",
            album.artist,
            album.tracks.len(),
            format_time(total)
        );
        let play = |shuffle: bool, tracks: Arc<[Track]>| {
            move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.player.set_shuffle(shuffle);
                this.player.play(tracks.to_vec(), 0);
                cx.notify();
            }
        };
        div()
            .id("album-detail-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(rems(1.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.75))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(rems(1.))
                            .child(self.cover(Some(&album.id), 176., &album.title, cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(rems(0.25))
                                    .child(
                                        div()
                                            .text_size(rems(1.125))
                                            .text_color(theme.foreground)
                                            .child(album.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(rems(0.75))
                                            .text_color(theme.secondary)
                                            .child(subtitle),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap(rems(0.5))
                                            .child(
                                                button(
                                                    "play-album",
                                                    "Play",
                                                    ButtonVariant::Primary,
                                                    cx,
                                                )
                                                .on_click(
                                                    cx.listener(play(false, album.tracks.clone())),
                                                ),
                                            )
                                            .child(
                                                button(
                                                    "shuffle-album",
                                                    "Shuffle",
                                                    ButtonVariant::Secondary,
                                                    cx,
                                                )
                                                .on_click(
                                                    cx.listener(play(true, album.tracks.clone())),
                                                ),
                                            )
                                            .child(
                                                button(
                                                    "back",
                                                    "Back",
                                                    ButtonVariant::Secondary,
                                                    cx,
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.selected_album = None;
                                                    cx.notify();
                                                })),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .children(album.tracks.iter().enumerate().map(|(i, t)| {
                                let all = album.tracks.clone();
                                div()
                                    .id(("album-track", i))
                                    .flex()
                                    .items_center()
                                    .gap(rems(0.75))
                                    .px(rems(0.5))
                                    .py(rems(0.375))
                                    .hover(|s| s.bg(theme.hover_fill()))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.player.play(all.to_vec(), i);
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .w(rems(2.))
                                            .text_color(theme.secondary)
                                            .child(format!("{}", t.track_number)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(theme.foreground)
                                            .child(t.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.secondary)
                                            .child(format_time(t.duration)),
                                    )
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_player_bar(
        &mut self,
        current: Option<&Track>,
        playing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.omarchy().clone();
        let has_track = current.is_some();
        let pos = if has_track {
            self.player.position()
        } else {
            0.0
        };
        let dur = if has_track {
            self.player.duration().max(1.0)
        } else {
            0.0
        };
        let title = current
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Not Playing".into());
        let sub = current
            .map(|t| format!("{} — {}", t.artist, t.album))
            .unwrap_or_default();
        let shuffle = self.player.shuffle();
        let repeat = self.player.repeat();
        let toggled = |on: bool| {
            if on {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            }
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rems(0.75))
            .px(rems(0.75))
            .py(rems(0.5))
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(self.cover(
                current.map(|t| t.album_id.as_str()),
                40.,
                current.map_or("♪", |t| t.title.as_str()),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(rems(10.))
                    .min_w(rems(6.))
                    .child(
                        div()
                            .text_size(rems(0.75))
                            .text_color(theme.foreground)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(rems(0.6875))
                            .text_color(theme.secondary)
                            .child(sub),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.375))
                    .child(Self::control_button(
                        "shuffle",
                        IconName::Shuffle,
                        "Shuffle",
                        toggled(shuffle),
                        "Shuffle",
                        |p| p.set_shuffle(!p.shuffle()),
                        cx,
                    ))
                    .child(Self::control_button(
                        "prev",
                        IconName::SkipBack,
                        "Previous",
                        ButtonVariant::Secondary,
                        "Previous",
                        Player::previous,
                        cx,
                    ))
                    .child(Self::control_button(
                        "play",
                        if playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        if playing { "Pause" } else { "Play" },
                        ButtonVariant::Primary,
                        "Play/Pause (Space)",
                        Player::toggle,
                        cx,
                    ))
                    .child(Self::control_button(
                        "next",
                        IconName::SkipForward,
                        "Next",
                        ButtonVariant::Secondary,
                        "Next",
                        Player::next,
                        cx,
                    ))
                    .child(Self::control_button(
                        "repeat",
                        if repeat == RepeatMode::One {
                            IconName::Repeat1
                        } else {
                            IconName::Repeat
                        },
                        "Repeat",
                        toggled(repeat != RepeatMode::Off),
                        "Repeat",
                        |p| p.set_repeat(p.repeat().next()),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(rems(6.))
                    .items_center()
                    .gap(rems(0.5))
                    .child(
                        div()
                            .w(rems(3.))
                            .text_size(rems(0.6875))
                            .text_color(theme.secondary)
                            .child(format_time(pos)),
                    )
                    .child(div().flex_1().min_w_0().child(slider(
                        &self.progress_state,
                        !has_track,
                        window,
                        cx,
                    )))
                    .child(
                        div()
                            .w(rems(3.))
                            .text_size(rems(0.6875))
                            .text_color(theme.secondary)
                            .child(format_time(dur)),
                    ),
            )
            .child(icon(IconName::Volume2).size(rems(0.875)))
            .child(
                div()
                    .w(rems(6.))
                    .min_w(rems(4.))
                    .child(slider(&self.volume, false, window, cx)),
            )
    }

    fn render_settings(
        &mut self,
        snap: &LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        panel("Settings", cx)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.5))
                    .children(snap.folders.iter().map(|f| {
                        let p = f.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap(rems(0.5))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(rems(0.6875))
                                    .child(f.to_string_lossy().to_string()),
                            )
                            .child(
                                button("rm-folder", "Remove", ButtonVariant::Secondary, cx)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.library.remove_folder(&p);
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap(rems(0.5))
                    .child(
                        div()
                            .flex_1()
                            .child(input("new-folder", &self.new_folder, window, cx)),
                    )
                    .child(
                        button("browse-folder", "Browse…", ButtonVariant::Secondary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.browse_folder(cx))),
                    ),
            )
            .child(
                button("add-folder", "Add ~/Music", ButtonVariant::Primary, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        if let Some(music) = dirs::audio_dir() {
                            this.library.add_folder(music);
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("close-settings", "Close", ButtonVariant::Secondary, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_settings = false;
                        cx.notify();
                    }),
                ),
            )
    }
}
