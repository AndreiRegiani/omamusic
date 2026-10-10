use crate::library::Library;
use crate::models::{PlayerState, RepeatMode, Section};
use crate::player::Player;
use gpui_kit::base::input::InputState;
use gpui_kit::base::slider::{SliderState, SliderValue};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, rems, App, AppContext, ClickEvent, Context, Entity, ImageSource,
    InteractiveElement, IntoElement, ParentElement, Render, Resource, StatefulInteractiveElement,
    Styled, Window,
};
use gpui_omarchy::{
    avatar, button, empty_state, focus_scope, icon, icon_button, input, panel, separator, slider,
    with_tooltip, ActiveTheme, ButtonVariant, IconName,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

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
    last_user_seek: Instant,
    new_folder: Entity<InputState>,
}

impl Omamusic {
    pub fn new(
        library: Arc<Library>,
        player: Arc<Player>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_view(
            library,
            player,
            std::env::var("OMADRIVE_VIEW").ok(),
            window,
            cx,
        )
    }

    pub fn new_with_view(
        library: Arc<Library>,
        player: Arc<Player>,
        view: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
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
                .max(1000.)
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

        cx.observe(&volume, |this, state, cx| {
            let v = match state.read(cx).value() {
                SliderValue::Single(x) => x as f64,
                SliderValue::Range(_, b) => b as f64,
            };
            this.player.set_volume(v.clamp(0., 1.));
        })
        .detach();

        cx.observe(&progress_state, |this, state, cx| {
            let v = match state.read(cx).value() {
                SliderValue::Single(x) => x,
                SliderValue::Range(_, b) => b,
            };
            let dur = this.player.duration().max(1.0);
            let target = v as f64 / 1000. * dur;
            if (target - this.player.position()).abs() > 1.0 {
                this.player.seek(target);
                this.last_user_seek = Instant::now();
            }
        })
        .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let alive = this
                .update(cx, |this, cx| {
                    let v = this.library.snapshot().version;
                    if v != this.lib_version {
                        this.lib_version = v;
                    }
                    // Push player position into the progress slider (no Window needed).
                    if this.last_user_seek.elapsed() > std::time::Duration::from_secs(1) {
                        let pos = this.player.position() as f32;
                        let dur = this.player.duration().max(1.0) as f32;
                        let val = (pos / dur * 1000.).clamp(0., 1000.);
                        let cur = match this.progress_state.read(cx).value() {
                            SliderValue::Single(x) => x,
                            SliderValue::Range(_, b) => b,
                        };
                        if (cur - val).abs() > 2. {
                            this.progress_state.update(cx, |s, _| {
                                *s = SliderState::new()
                                    .min(0.)
                                    .max(1000.)
                                    .step(1.)
                                    .default_value(val);
                            });
                        }
                    }
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();

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

        let new_folder = cx.new(|cx| InputState::new(window, cx).placeholder("Enter folder path"));

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
            last_user_seek: Instant::now() - std::time::Duration::from_secs(10),
            new_folder,
        }
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
}

impl Omamusic {
    fn cover_image(
        &self,
        album_id: &str,
        size: u32,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Img> {
        let data_uri = self.library.covers().data_uri(album_id, size)?;
        let source = ImageSource::Resource(Resource::Uri(data_uri.into()));
        Some(img(source))
    }
}

fn sidebar_btn(
    id: impl Into<gpui_kit::ElementId>,
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

        focus_scope("omamusic").size_full().bg(theme.background).child(
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
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .max_w(rems(28.))
                                .child(input("search", &self.query, window, cx)),
                        )
                        .child(with_tooltip(
                            icon_button("rescan", IconName::RefreshCw, "Rescan library", ButtonVariant::Secondary, cx)
                                .on_click(cx.listener(|this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    this.library.request_rescan();
                                    cx.notify();
                                })),
                            "Rescan library (Ctrl+R)",
                        ))
                        .child(with_tooltip(
                            icon_button("settings", IconName::Settings, "Settings", ButtonVariant::Secondary, cx)
                                .on_click(cx.listener(|this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    this.show_settings = !this.show_settings;
                                    cx.notify();
                                })),
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
                                .child(sidebar_btn("nav-albums", "Albums", IconName::Disc3, self.section == Section::Albums, cx,
                                    cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                        this.section = Section::Albums;
                                        this.selected_album = None;
                                        cx.notify();
                                    })))
                                .child(sidebar_btn("nav-artists", "Artists", IconName::Mic, self.section == Section::Artists, cx,
                                    cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                        this.section = Section::Artists;
                                        this.selected_album = None;
                                        cx.notify();
                                    })))
                                .child(sidebar_btn("nav-songs", "Songs", IconName::Music, self.section == Section::Songs, cx,
                                    cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                        this.section = Section::Songs;
                                        this.selected_album = None;
                                        cx.notify();
                                    })))
                                .child(separator(cx))
                                .child(
                                    div()
                                        .text_size(rems(0.6875))
                                        .text_color(theme.secondary)
                                        .child(format!("{} albums · {} songs", snap.albums.len(), snap.all_tracks.len())),
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
                            div().flex_1().min_w_0().flex().flex_col().bg(theme.background).child(
                                if snap.folders.is_empty() {
                                    empty_state("No Music", "Add a folder with music in Settings.", cx).into_any_element()
                                } else {
                                    self.render_content(&snap, window, cx)
                                },
                            ),
                        ),
                )
                .child(separator(cx))
                .child(self.render_player_bar(current.as_ref(), playing, window, cx))
                .when(self.show_settings, |d| d.child(self.render_settings(window, cx))),
        )
    }
}

impl Omamusic {
    fn render_content(
        &mut self,
        snap: &crate::library::LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        if let Some(album_id) = self.selected_album.clone() {
            if snap.albums.iter().any(|a| a.id == album_id) {
                return self.render_album_detail(
                    snap.albums.iter().find(|a| a.id == album_id).unwrap(),
                    window,
                    cx,
                );
            }
        }
        match self.section {
            Section::Albums => self.render_album_grid(snap, window, cx),
            Section::Artists => self.render_artist_list(snap, window, cx),
            Section::Songs => self.render_song_list(snap, cx),
        }
    }

    fn render_album_grid(
        &mut self,
        snap: &crate::library::LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.omarchy().clone();
        div()
            .id("albums-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(rems(0.75))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(rems(0.75))
                    .children(snap.filtered_albums.iter().take(400).enumerate().map(|(idx, a)| {
                        let id = a.id.clone();
                        let title = a.title.clone();
                        let artist = a.artist.clone();
                        let n = a.tracks.len();
                        let count = format!("{n} song{}", if n == 1 { "" } else { "s" });
                        let cover = self.cover_image(&id, 120, window, cx);
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
                            .on_click(cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                this.selected_album = Some(id.clone());
                                cx.notify();
                            }))
                            .child(if let Some(cover) = cover { cover.into_any_element() } else { avatar(title.chars().take(2).collect::<String>(), cx).into_any_element() })
                            .child(div().text_size(rems(0.75)).text_color(theme.foreground).child(title))
                            .child(div().text_size(rems(0.6875)).text_color(theme.secondary).child(artist))
                            .child(div().text_size(rems(0.6875)).text_color(theme.secondary).child(count))
                    })),
            )
            .into_any_element()
    }

    fn render_artist_list(
        &mut self,
        snap: &crate::library::LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.omarchy().clone();
        div().id("artists-scroll").flex_1().min_h_0().overflow_y_scroll().child(
            div().flex().flex_col().children(snap.artists.iter().enumerate().map(|(idx, (name, albums))| {
                let song_count: usize = albums.iter().map(|a| a.tracks.len()).sum();
                let an = albums.len();
                let label = format!("{name} · {an} album{} · {song_count} songs", if an == 1 { "" } else { "s" });
                let aname = name.clone();
                let first_album = albums.first().map(|a| a.id.clone());
                let cover = first_album.and_then(|id| self.cover_image(&id, 40, window, cx));
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
                    .on_click(cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        this.section = Section::Albums;
                        this.library.set_query(aname.clone());
                        cx.notify();
                    }))
                    .child(if let Some(cover) = cover { cover.into_any_element() } else { avatar(name.chars().take(2).collect::<String>(), cx).into_any_element() })
                    .child(div().text_size(rems(0.75)).text_color(theme.foreground).child(label))
            })),
        ).into_any_element()
    }

    fn render_song_list(
        &mut self,
        snap: &crate::library::LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
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
                        let title = t.display_title().to_string();
                        let artist = t.display_artist().to_string();
                        let album = t.display_album().to_string();
                        let dur = Self::format_time(t.duration);
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
                            .on_click(cx.listener(
                                move |this: &mut Omamusic,
                                      _: &ClickEvent,
                                      _: &mut Window,
                                      cx: &mut Context<Self>| {
                                    this.player.play(all.clone(), i);
                                    cx.notify();
                                },
                            ))
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
                                    .child(title),
                            )
                            .child(
                                div()
                                    .w(rems(10.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(artist),
                            )
                            .child(
                                div()
                                    .w(rems(10.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(album),
                            )
                            .child(
                                div()
                                    .w(rems(3.))
                                    .text_size(rems(0.6875))
                                    .text_color(theme.secondary)
                                    .child(dur),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_album_detail(
        &mut self,
        album: &crate::models::Album,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.omarchy().clone();
        let total = album.tracks.iter().map(|t| t.duration).sum::<f64>();
        let subtitle = format!(
            "{} · {} songs · {}",
            album.artist,
            album.tracks.len(),
            Self::format_time(total)
        );
        let tracks = album.tracks.clone();
        let tracks2 = album.tracks.clone();
        let tracks3 = album.tracks.clone();
        let _tracks4 = album.tracks.clone();
        let _tracks5 = album.tracks.clone();
        let _tracks6 = album.tracks.clone();
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
                            .child(avatar(album.title.chars().take(2).collect::<String>(), cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(rems(0.25))
                                    .child(div().text_size(rems(1.125)).text_color(theme.foreground).child(album.title.clone()))
                                    .child(div().text_size(rems(0.75)).text_color(theme.secondary).child(subtitle))
.child(
                                                div()
                                                    .flex()
                                                    .gap(rems(0.5))
.child(
                                                    button("play-album", "Play", ButtonVariant::Primary, cx).on_click(
                                                        cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                                            this.player.set_shuffle(false);
                                                            this.player.play(tracks.clone(), 0);
                                                            cx.notify();
                                                        }),
                                                    ),
                                                )
                                                .child(
                                                    button("shuffle-album", "Shuffle", ButtonVariant::Secondary, cx).on_click(
                                                        cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                                            this.player.set_shuffle(true);
                                                            this.player.play(tracks3.clone(), 0);
                                                            cx.notify();
                                                        }),
                                                    ),
                                                )
                                                    .child(
                                                        button("back", "Back", ButtonVariant::Secondary, cx).on_click(
                                                            cx.listener(|this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                                                this.selected_album = None;
                                                                cx.notify();
                                                            }),
                                                        ),
                                                    ),
                                            ),
                            ),
                    )
                    .child(
                        div().flex().flex_col().children(album.tracks.iter().enumerate().map(|(i, t)| {
                            let all = tracks2.clone();
                            let row_title = t.display_title().to_string();
                            div()
                                .id(("album-track", i))
                                .flex()
                                .items_center()
                                .gap(rems(0.75))
                                .px(rems(0.5))
                                .py(rems(0.375))
                                .hover(|s| s.bg(theme.hover_fill()))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    this.player.play(all.clone(), i);
                                    cx.notify();
                                }))
                                .child(div().w(rems(2.)).text_color(theme.secondary).child(format!("{}", t.track_number)))
                                .child(div().flex_1().text_color(theme.foreground).child(row_title))
                                .child(div().text_color(theme.secondary).child(Self::format_time(t.duration)))
                        })),
                    ),
            )
            .into_any_element()
    }

    fn render_player_bar(
        &mut self,
        current: Option<&crate::models::Track>,
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
            .map(|t| t.display_title().to_string())
            .unwrap_or_else(|| "Not Playing".into());
        let sub = current
            .map(|t| format!("{} — {}", t.display_artist(), t.display_album()))
            .unwrap_or_default();
        let shuffle = self.player.shuffle();
        let repeat = self.player.repeat();

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
            .child(avatar(
                current
                    .map(|t| t.display_title().chars().take(2).collect::<String>())
                    .unwrap_or_else(|| "♪".into()),
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
                    .child(with_tooltip(
                        icon_button(
                            "shuffle",
                            IconName::Shuffle,
                            "Shuffle",
                            if shuffle {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            },
                            cx,
                        )
                        .on_click(cx.listener(
                            |this: &mut Omamusic,
                             _: &ClickEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                let v = !this.player.shuffle();
                                this.player.set_shuffle(v);
                                cx.notify();
                            },
                        )),
                        "Shuffle",
                    ))
                    .child(with_tooltip(
                        icon_button(
                            "prev",
                            IconName::SkipBack,
                            "Previous",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this: &mut Omamusic,
                             _: &ClickEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                this.player.previous();
                                cx.notify();
                            },
                        )),
                        "Previous",
                    ))
                    .child(with_tooltip(
                        icon_button(
                            "play",
                            if playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            },
                            if playing { "Pause" } else { "Play" },
                            ButtonVariant::Primary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this: &mut Omamusic,
                             _: &ClickEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                this.player.toggle();
                                cx.notify();
                            },
                        )),
                        "Play/Pause (Space)",
                    ))
                    .child(with_tooltip(
                        icon_button(
                            "next",
                            IconName::SkipForward,
                            "Next",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this: &mut Omamusic,
                             _: &ClickEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                this.player.next();
                                cx.notify();
                            },
                        )),
                        "Next",
                    ))
                    .child(with_tooltip(
                        icon_button(
                            "repeat",
                            if repeat == RepeatMode::One {
                                IconName::Repeat1
                            } else {
                                IconName::Repeat
                            },
                            "Repeat",
                            if repeat == RepeatMode::Off {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Primary
                            },
                            cx,
                        )
                        .on_click(cx.listener(
                            |this: &mut Omamusic,
                             _: &ClickEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                this.player.set_repeat(this.player.repeat().next());
                                cx.notify();
                            },
                        )),
                        "Repeat",
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
                            .child(Self::format_time(pos)),
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
                            .child(Self::format_time(dur)),
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

    fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.library.snapshot();
        let new_folder = cx.new(|cx| InputState::new(window, cx).placeholder("Enter folder path"));

        panel("Settings", cx)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.5))
                    .children(snap.folders.iter().map(|f| {
                        let p = f.clone();
                        let label = f.to_string_lossy().to_string();
                        div()
                        .flex()
                        .items_center()
                        .gap(rems(0.5))
                        .child(div().flex_1().text_size(rems(0.6875)).child(label))
                        .child(
                            button("rm-folder", "Remove", ButtonVariant::Secondary, cx)
                                .on_click(cx.listener(
                                move |this: &mut Omamusic,
                                      _: &ClickEvent,
                                      _: &mut Window,
                                      cx: &mut Context<Self>| {
                                    this.library.remove_folder(&p);
                                    cx.notify();
                                },
                            )),
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
                            .child(input("new-folder", &new_folder, window, cx)),
                    )
                    .child(
                        button("browse-folder", "Browse…", ButtonVariant::Secondary, cx).on_click(
                            cx.listener(
                                move |this: &mut Omamusic,
                                      _: &ClickEvent,
                                      _: &mut Window,
                                      cx: &mut Context<Self>| {
                                    let path = this.new_folder.read(cx).text().to_string();
                                    if !path.is_empty() {
                                        this.library.add_folder(PathBuf::from(path));
                                    } else {
                                        // Try to open a folder picker using zenity or kdialog
                                        if let Ok(output) = std::process::Command::new("zenity")
                                            .args([
                                                "--file-selection",
                                                "--directory",
                                                "--title=Select Music Folder",
                                            ])
                                            .output()
                                        {
                                            let path = String::from_utf8_lossy(&output.stdout)
                                                .trim()
                                                .to_string();
                                            if !path.is_empty() {
                                                this.library.add_folder(PathBuf::from(path));
                                            }
                                        } else if let Ok(output) =
                                            std::process::Command::new("kdialog")
                                                .args([
                                                    "--getexistingdirectory",
                                                    "--title=Select Music Folder",
                                                ])
                                                .output()
                                        {
                                            let path = String::from_utf8_lossy(&output.stdout)
                                                .trim()
                                                .to_string();
                                            if !path.is_empty() {
                                                this.library.add_folder(PathBuf::from(path));
                                            }
                                        }
                                    }
                                    cx.notify();
                                },
                            ),
                        ),
                    ),
            )
            .child(
                button("add-folder", "Add ~/Music", ButtonVariant::Primary, cx).on_click(
                    cx.listener(
                        |this: &mut Omamusic,
                         _: &ClickEvent,
                         _: &mut Window,
                         cx: &mut Context<Self>| {
                            if let Some(music) = dirs::audio_dir() {
                                this.library.add_folder(music);
                            }
                            cx.notify();
                        },
                    ),
                ),
            )
            .child(
                button("close-settings", "Close", ButtonVariant::Secondary, cx).on_click(
                    cx.listener(
                        |this: &mut Omamusic,
                         _: &ClickEvent,
                         _: &mut Window,
                         cx: &mut Context<Self>| {
                            this.show_settings = false;
                            cx.notify();
                        },
                    ),
                ),
            )
    }
}
