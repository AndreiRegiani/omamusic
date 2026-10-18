use crate::config::AppConfig;
use crate::library::{Library, LibrarySnapshot};
use crate::models::{Album, PlayerState, Section, Track};
use crate::player::Player;
use crate::{FocusSearch, GoBack};
use gpui_kit::base::input::InputState;
use gpui_kit::base::{Popover, Selectable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, rems, uniform_list, AnyElement, App, AppContext, ClickEvent, Context, Div, ElementId,
    Entity, ImageSource, InteractiveElement, IntoElement, MouseButton, NavigationDirection,
    ParentElement, PathPromptOptions, Render, Stateful, StatefulInteractiveElement, Styled, Window,
};
use gpui_omarchy::{
    avatar, avatar_image, button, empty_state, focus_scope, icon, icon_button, input, menu, panel,
    separator, switch, with_tooltip, ActiveTheme, ButtonVariant, IconName, MenuItem,
};
use rand::Rng;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct Omamusic {
    pub library: Arc<Library>,
    pub player: Arc<Player>,
    section: Section,
    query: Entity<InputState>,
    pages: Vec<Page>,
    selection: BTreeSet<usize>,
    anchor: Option<usize>,
    show_settings: bool,
    lib_version: u64,
    prefs: AppConfig,
    new_folder: Entity<InputState>,
}

#[derive(Clone, PartialEq)]
enum Page {
    Album(String),
    Artist(String),
}

#[derive(Clone, Copy)]
enum RowAction {
    Play,
    PlayNext,
    AddToQueue,
    GoToAlbum,
    GoToArtist,
    Remove,
}

const TRACK_ACTIONS: &[RowAction] = &[
    RowAction::Play,
    RowAction::PlayNext,
    RowAction::AddToQueue,
    RowAction::GoToAlbum,
    RowAction::GoToArtist,
];
const QUEUE_ACTIONS: &[RowAction] = &[
    RowAction::Play,
    RowAction::GoToAlbum,
    RowAction::GoToArtist,
    RowAction::Remove,
];
const ALBUM_ACTIONS: &[RowAction] = &[
    RowAction::Play,
    RowAction::PlayNext,
    RowAction::AddToQueue,
    RowAction::GoToArtist,
];
const ARTIST_ACTIONS: &[RowAction] = &[RowAction::Play, RowAction::PlayNext, RowAction::AddToQueue];

impl RowAction {
    fn group(self) -> u8 {
        match self {
            RowAction::Play | RowAction::PlayNext | RowAction::AddToQueue => 0,
            RowAction::GoToAlbum | RowAction::GoToArtist => 1,
            RowAction::Remove => 2,
        }
    }

    fn item(self) -> MenuItem {
        let (label, icon_name) = match self {
            RowAction::Play => ("Play", IconName::Play),
            RowAction::PlayNext => ("Play Next", IconName::ListStart),
            RowAction::AddToQueue => ("Add to Queue", IconName::ListEnd),
            RowAction::GoToAlbum => ("Go to Album", IconName::Disc3),
            RowAction::GoToArtist => ("Go to Artist", IconName::Mic),
            RowAction::Remove => ("Remove from Queue", IconName::ListX),
        };
        MenuItem::new(label).icon(icon_name)
    }
}

struct MenuTrigger(AnyElement);

impl IntoElement for MenuTrigger {
    type Element = AnyElement;

    fn into_element(self) -> AnyElement {
        self.0
    }
}

impl Selectable for MenuTrigger {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        false
    }
}

pub fn cover(
    library: &Library,
    album_id: Option<&str>,
    size: f32,
    name: &str,
    cx: &App,
) -> impl IntoElement {
    let cover = avatar(initials(name), cx).size(px(size));
    match album_id.and_then(|id| library.covers().cached(id, size as u32 * 2)) {
        Some(image) => cover.image(avatar_image(ImageSource::Image(image))),
        None => cover,
    }
}

fn initials(text: &str) -> String {
    text.chars().take(2).collect()
}

pub fn format_time(secs: f64) -> String {
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
        cx.observe(&query, |this, state, cx| {
            let text = state.read(cx).text().to_string();
            this.library.set_query(text);
            this.pages.clear();
            this.clear_selection();
            cx.notify();
        })
        .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let alive = this.update(cx, |this, cx| this.tick(false, cx)).is_ok();
            if !alive {
                break;
            }
        })
        .detach();

        let changes = player.subscribe();
        cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                let alive = this.update(cx, |this, cx| this.tick(true, cx)).is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        let covers = library.covers().subscribe();
        cx.spawn(async move |this, cx| {
            while covers.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        let view = std::env::var("OMADRIVE_VIEW").ok();
        let (section, pages, show_settings) = match view.as_deref() {
            Some("artists") => (Section::Artists, Vec::new(), false),
            Some("songs") => (Section::Songs, Vec::new(), false),
            Some("queue") => (Section::Queue, Vec::new(), false),
            Some("settings") => (Section::Albums, Vec::new(), true),
            Some("album") => {
                let first = library.snapshot().albums.first().map(|a| a.id.clone());
                (
                    Section::Albums,
                    first.map(Page::Album).into_iter().collect(),
                    false,
                )
            }
            _ => (Section::Albums, Vec::new(), false),
        };

        Self {
            library,
            player,
            section,
            query,
            pages,
            selection: BTreeSet::new(),
            anchor: None,
            show_settings,
            lib_version: 0,
            prefs: AppConfig::load(),
            new_folder,
        }
    }

    fn tick(&mut self, force: bool, cx: &mut Context<Self>) {
        let version = self.library.version();
        if !force && version == self.lib_version {
            return;
        }
        if version != self.lib_version {
            self.clear_selection();
        }
        self.lib_version = version;
        cx.notify();
    }

    pub fn show_album(&mut self, album_id: String, cx: &mut Context<Self>) {
        self.navigate(Page::Album(album_id), cx);
    }

    fn cover(&self, album_id: Option<&str>, size: f32, name: &str, cx: &App) -> impl IntoElement {
        cover(&self.library, album_id, size, name, cx)
    }

    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.pages.last() != Some(&page) {
            self.pages.push(page);
        }
        self.clear_selection();
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.pages.pop();
        self.clear_selection();
        cx.notify();
    }

    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.pages.clear();
        self.clear_selection();
        cx.notify();
    }

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.anchor = None;
    }

    fn select_row(&mut self, row: usize, event: &ClickEvent) -> bool {
        let m = event.modifiers();
        let toggle = m.control || m.platform;
        match self.anchor {
            Some(anchor) if m.shift => {
                if !toggle {
                    self.selection.clear();
                }
                self.selection.extend(anchor.min(row)..=anchor.max(row));
                return false;
            }
            _ if toggle => {
                if !self.selection.remove(&row) {
                    self.selection.insert(row);
                }
            }
            _ => {
                self.selection = BTreeSet::from([row]);
            }
        }
        self.anchor = Some(row);
        event.click_count() >= 2 && !toggle
    }

    fn track_action(
        &mut self,
        action: RowAction,
        list: &[Track],
        row: usize,
        queue: bool,
        cx: &mut Context<Self>,
    ) {
        let rows: Vec<usize> = if self.selection.contains(&row) {
            self.selection.iter().copied().collect()
        } else {
            vec![row]
        };
        let tracks: Vec<Track> = rows.iter().filter_map(|&i| list.get(i).cloned()).collect();
        let Some(first) = tracks.first().cloned() else {
            return;
        };
        match action {
            RowAction::Play if queue => self.player.jump(row),
            RowAction::Play if tracks.len() > 1 => self.player.play(tracks, 0),
            RowAction::Play => self.player.play(list.to_vec(), row),
            RowAction::PlayNext => self.player.play_next(tracks),
            RowAction::AddToQueue => self.player.enqueue(tracks),
            RowAction::GoToAlbum => self.navigate(Page::Album(first.album_id), cx),
            RowAction::GoToArtist => self.navigate(Page::Artist(first.artist), cx),
            RowAction::Remove => {
                self.player.remove(&rows);
                self.clear_selection();
            }
        }
        cx.notify();
    }

    fn tracks_action(&mut self, action: RowAction, tracks: &[Track], cx: &mut Context<Self>) {
        match action {
            RowAction::Play => self.player.play(tracks.to_vec(), 0),
            RowAction::PlayNext => self.player.play_next(tracks.to_vec()),
            RowAction::AddToQueue => self.player.enqueue(tracks.to_vec()),
            RowAction::GoToAlbum | RowAction::GoToArtist | RowAction::Remove => {}
        }
        cx.notify();
    }

    fn context_menu(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        actions: &'static [RowAction],
        on_select: impl Fn(&mut Self, RowAction, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Popover {
        let this = cx.entity().downgrade();
        let items = actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let item = action.item();
                if i > 0 && actions[i - 1].group() != action.group() {
                    item.separator_before()
                } else {
                    item
                }
            })
            .collect();
        menu(
            id,
            MenuTrigger(trigger.into_any_element()),
            items,
            move |index, _, cx| {
                let _ = this.update(cx, |this, cx| on_select(this, actions[index], cx));
            },
        )
        .mouse_button(MouseButton::Right)
    }

    fn track_row(
        &self,
        id: &'static str,
        row: usize,
        list: Arc<[Track]>,
        queue: bool,
        content: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Popover {
        let theme = cx.omarchy().clone();
        let click_list = list.clone();
        let content = content
            .when(self.selection.contains(&row), |d| {
                d.bg(theme.selected_fill())
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if this.select_row(row, event) {
                    this.track_action(RowAction::Play, &click_list, row, queue, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    if !this.selection.contains(&row) {
                        this.selection = BTreeSet::from([row]);
                        this.anchor = Some(row);
                        cx.notify();
                    }
                }),
            );
        let actions = if queue { QUEUE_ACTIONS } else { TRACK_ACTIONS };
        Self::context_menu(
            (id, row),
            content,
            actions,
            move |this, action, cx| this.track_action(action, &list, row, queue, cx),
            cx,
        )
        .w_full()
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |q, cx| {
            q.set_value("", window, cx);
            q.focus(window, cx);
        });
    }

    fn pref_switch(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        change: fn(&mut AppConfig, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        switch(id, label, checked, cx).on_change(move |on, _, _, cx| {
            let _ = this.update(cx, |this, cx| {
                change(&mut this.prefs, on);
                AppConfig::update(|c| change(c, on));
                crate::desktop::set_cover_icon(!this.prefs.plain_tray_icon);
                cx.notify();
            });
        })
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
        let theme = cx.omarchy().clone();
        let nav = [
            ("nav-albums", "Albums", IconName::Disc3, Section::Albums),
            ("nav-artists", "Artists", IconName::Mic, Section::Artists),
            ("nav-songs", "Songs", IconName::Music, Section::Songs),
            ("nav-queue", "Up Next", IconName::ListMusic, Section::Queue),
        ];

        focus_scope("omamusic")
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.back(cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| this.back(cx)),
            )
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
                                                this.set_section(section, cx)
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
                                    .child(
                                        if snap.folders.is_empty() && self.section != Section::Queue
                                        {
                                            empty_state("No Music", "Add a folder with music.", cx)
                                                .child(
                                                    button(
                                                        "empty-add-folder",
                                                        "Add Folder…",
                                                        ButtonVariant::Primary,
                                                        cx,
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.browse_folder(cx)
                                                    })),
                                                )
                                                .into_any_element()
                                        } else if snap.albums.is_empty()
                                            && snap.scanning
                                            && self.section != Section::Queue
                                        {
                                            empty_state(
                                                "Scanning…",
                                                "Reading the music folders.",
                                                cx,
                                            )
                                            .into_any_element()
                                        } else {
                                            self.render_content(&snap, window, cx)
                                        },
                                    ),
                            ),
                    )
                    .when(self.show_settings, |d| {
                        d.child(self.render_settings(&snap, window, cx))
                    }),
            )
    }
}

impl Omamusic {
    fn render_content(
        &mut self,
        snap: &Arc<LibrarySnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.pages.last().cloned() {
            Some(Page::Album(id)) => {
                if let Some(album) = snap.albums.iter().find(|a| a.id == id) {
                    return self.render_album_detail(album, cx);
                }
            }
            Some(Page::Artist(name)) => return self.render_artist_albums(&name, snap, window, cx),
            None => {}
        }
        match self.section {
            Section::Albums => {
                self.render_album_grid("albums", snap.filtered_albums.clone(), window, cx)
            }
            Section::Artists => self.render_artist_list(snap, cx),
            Section::Songs => self.render_song_list(snap, cx),
            Section::Queue => self.render_queue(cx),
        }
    }

    fn render_artist_albums(
        &mut self,
        name: &str,
        snap: &LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.omarchy().clone();
        let albums: Arc<[Album]> = snap
            .albums
            .iter()
            .filter(|a| a.artist == name || a.tracks.iter().any(|t| t.artist == name))
            .cloned()
            .collect();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.75))
                    .px(rems(0.75))
                    .pt(rems(0.75))
                    .child(
                        button("artist-back", "Back", ButtonVariant::Secondary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .child(
                        div()
                            .text_size(rems(1.125))
                            .text_color(theme.foreground)
                            .child(name.to_string()),
                    ),
            )
            .child(self.render_album_grid("artist-albums", albums, window, cx))
            .into_any_element()
    }

    fn render_album_grid(
        &mut self,
        id: &'static str,
        albums: Arc<[Album]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rem = window.rem_size();
        let gap = rem * 0.75;
        let width = window.viewport_size().width - rem * 11. - px(1.) - gap * 2.;
        let columns = (((width + gap) / (rem * 10. + gap)).floor() as usize).max(1);
        let rows = albums.len().div_ceil(columns);
        uniform_list(
            id,
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(albums.len());
                        div().flex().gap(gap).px(gap).pt(gap).children(
                            (start..end).map(|idx| this.album_card(&albums[idx], idx, cx)),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .pb(gap)
        .into_any_element()
    }

    fn album_card(&self, a: &Album, idx: usize, cx: &mut Context<Self>) -> Popover {
        let theme = cx.omarchy().clone();
        let id = a.id.clone();
        let tracks = a.tracks.clone();
        let n = a.tracks.len();
        let count = format!("{n} song{}", if n == 1 { "" } else { "s" });
        let artist = a.artist.clone();
        let card = div()
            .id(("album", idx))
            .w(rems(10.))
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
            .on_click(cx.listener(move |this, _, _, cx| this.navigate(Page::Album(id.clone()), cx)))
            .child(
                div()
                    .text_size(rems(0.75))
                    .text_color(theme.foreground)
                    .truncate()
                    .child(a.title.clone()),
            )
            .child(
                div()
                    .text_size(rems(0.6875))
                    .text_color(theme.secondary)
                    .truncate()
                    .child(a.artist.clone()),
            )
            .child(
                div()
                    .text_size(rems(0.6875))
                    .text_color(theme.secondary)
                    .child(count),
            );
        Self::context_menu(
            ("album-menu", idx),
            card,
            ALBUM_ACTIONS,
            move |this, action, cx| match action {
                RowAction::GoToArtist => this.navigate(Page::Artist(artist.clone()), cx),
                _ => this.tracks_action(action, &tracks, cx),
            },
            cx,
        )
    }

    fn render_artist_list(
        &mut self,
        snap: &Arc<LibrarySnapshot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snap = snap.clone();
        uniform_list(
            "artists",
            snap.artists.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|idx| {
                        let (name, albums) = &snap.artists[idx];
                        this.artist_row(idx, name, albums, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn artist_row(
        &self,
        idx: usize,
        name: &str,
        albums: &[Album],
        cx: &mut Context<Self>,
    ) -> Popover {
        let theme = cx.omarchy().clone();
        let song_count: usize = albums.iter().map(|a| a.tracks.len()).sum();
        let an = albums.len();
        let label = format!(
            "{name} · {an} album{} · {song_count} songs",
            if an == 1 { "" } else { "s" }
        );
        let aname = name.to_string();
        let tracks: Vec<Track> = albums
            .iter()
            .flat_map(|a| a.tracks.iter().cloned())
            .collect();
        let row = div()
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
            .child(self.cover(albums.first().map(|a| a.id.as_str()), 40., name, cx))
            .on_click(
                cx.listener(move |this, _, _, cx| this.navigate(Page::Artist(aname.clone()), cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(0.75))
                    .text_color(theme.foreground)
                    .truncate()
                    .child(label),
            );
        Self::context_menu(
            ("artist-menu", idx),
            row,
            ARTIST_ACTIONS,
            move |this, action, cx| this.tracks_action(action, &tracks, cx),
            cx,
        )
        .w_full()
    }

    fn render_song_list(&mut self, snap: &LibrarySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let tracks = snap.filtered_tracks.clone();
        uniform_list(
            "songs",
            tracks.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|i| this.song_row(i, &tracks, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn song_row(&self, i: usize, tracks: &Arc<[Track]>, cx: &mut Context<Self>) -> Popover {
        let theme = cx.omarchy().clone();
        let t = &tracks[i];
        let bg = if i.is_multiple_of(2) {
            theme.background
        } else {
            theme.hover_fill()
        };
        let cell = |width: f32, text: String| {
            div()
                .w(rems(width))
                .text_size(rems(0.6875))
                .text_color(theme.secondary)
                .truncate()
                .child(text)
        };
        let row = div()
            .id(("song", i))
            .flex()
            .items_center()
            .gap(rems(0.75))
            .px(rems(0.75))
            .py(rems(0.375))
            .bg(bg)
            .hover(|s| s.bg(theme.hover_fill()))
            .cursor_pointer()
            .child(cell(2., format!("{}", i + 1)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(0.75))
                    .text_color(theme.foreground)
                    .truncate()
                    .child(t.title.clone()),
            )
            .child(cell(10., t.artist.clone()))
            .child(cell(10., t.album.clone()))
            .child(cell(3., format_time(t.duration)));
        self.track_row("song", i, tracks.clone(), false, row, cx)
    }

    fn render_album_detail(&mut self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        let total = album.tracks.iter().map(|t| t.duration).sum::<f64>();
        let n = album.tracks.len();
        let subtitle = [
            album.artist.clone(),
            if album.year > 0 {
                album.year.to_string()
            } else {
                String::new()
            },
            format!("{n} song{}", if n == 1 { "" } else { "s" }),
            format_time(total),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let current = self.player.current();
        let playing = self.player.state() == PlayerState::Playing;
        let play = |shuffle: bool, tracks: Arc<[Track]>| {
            move |this: &mut Omamusic, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                let start = if shuffle {
                    rand::thread_rng().gen_range(0..tracks.len())
                } else {
                    0
                };
                this.player.set_shuffle(shuffle);
                this.player.play(tracks.to_vec(), start);
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
                                                .on_click(
                                                    cx.listener(|this, _, _, cx| this.back(cx)),
                                                ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .children(album.tracks.iter().enumerate().map(|(i, t)| {
                                let is_current = current.as_ref().is_some_and(|c| c.path == t.path);
                                let number = if is_current {
                                    icon(if playing {
                                        IconName::Volume2
                                    } else {
                                        IconName::Volume
                                    })
                                    .size(rems(0.875))
                                    .text_color(theme.accent)
                                    .into_any_element()
                                } else if t.track_number > 0 {
                                    t.track_number.to_string().into_any_element()
                                } else {
                                    "".into_any_element()
                                };
                                let row = div()
                                    .id(("album-track", i))
                                    .flex()
                                    .items_center()
                                    .gap(rems(0.75))
                                    .px(rems(0.5))
                                    .py(rems(0.375))
                                    .hover(|s| s.bg(theme.hover_fill()))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .w(rems(2.))
                                            .flex()
                                            .justify_end()
                                            .text_color(theme.secondary)
                                            .child(number),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .gap(rems(0.5))
                                            .text_color(theme.foreground)
                                            .child(t.title.clone())
                                            .when(t.artist != album.artist, |d| {
                                                d.child(
                                                    div()
                                                        .text_color(theme.secondary)
                                                        .child(t.artist.clone()),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.secondary)
                                            .child(format_time(t.duration)),
                                    );
                                self.track_row(
                                    "album-track-menu",
                                    i,
                                    album.tracks.clone(),
                                    false,
                                    row,
                                    cx,
                                )
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_queue(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.omarchy().clone();
        let (queue, index) = self.player.queue();
        if queue.is_empty() {
            return empty_state(
                "Queue Empty",
                "Play something or add songs to the queue.",
                cx,
            )
            .into_any_element();
        }
        let playing = self.player.state() == PlayerState::Playing;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.75))
                    .px(rems(0.75))
                    .py(rems(0.5))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(rems(0.75))
                            .text_color(theme.secondary)
                            .child(format!(
                                "{} songs · {}",
                                queue.len(),
                                format_time(queue.iter().map(|t| t.duration).sum())
                            )),
                    )
                    .child(
                        button("clear-queue", "Clear", ButtonVariant::Secondary, cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.player.stop();
                                this.clear_selection();
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                uniform_list(
                    "queue",
                    queue.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range
                            .map(|i| this.queue_row(i, &queue, index, playing, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn queue_row(
        &self,
        i: usize,
        queue: &Arc<[Track]>,
        index: Option<usize>,
        playing: bool,
        cx: &mut Context<Self>,
    ) -> Popover {
        let theme = cx.omarchy().clone();
        let t = &queue[i];
        let is_current = index == Some(i);
        let past = index.is_some_and(|current| i < current);
        let number = if is_current {
            icon(if playing {
                IconName::Volume2
            } else {
                IconName::Volume
            })
            .size(rems(0.875))
            .text_color(theme.accent)
            .into_any_element()
        } else {
            (i + 1).to_string().into_any_element()
        };
        let row = div()
            .id(("queue-row", i))
            .flex()
            .items_center()
            .gap(rems(0.75))
            .px(rems(0.75))
            .py(rems(0.375))
            .text_size(rems(0.75))
            .text_color(if past {
                theme.secondary
            } else {
                theme.foreground
            })
            .hover(|s| s.bg(theme.hover_fill()))
            .cursor_pointer()
            .child(
                div()
                    .w(rems(2.))
                    .flex()
                    .justify_end()
                    .text_color(theme.secondary)
                    .child(number),
            )
            .child(self.cover(Some(&t.album_id), 28., &t.album, cx))
            .child(div().flex_1().min_w_0().truncate().child(t.title.clone()))
            .child(
                div()
                    .w(rems(10.))
                    .text_color(theme.secondary)
                    .truncate()
                    .child(t.artist.clone()),
            )
            .child(
                div()
                    .w(rems(3.))
                    .text_color(theme.secondary)
                    .child(format_time(t.duration)),
            );
        self.track_row("queue-menu", i, queue.clone(), true, row, cx)
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
                    .gap(rems(0.25))
                    .child(self.pref_switch(
                        "pref-close-to-tray",
                        "Close window to tray",
                        !self.prefs.quit_on_close,
                        |c, on| c.quit_on_close = !on,
                        cx,
                    ))
                    .child(self.pref_switch(
                        "pref-tray-cover",
                        "Use album cover as tray icon",
                        !self.prefs.plain_tray_icon,
                        |c, on| c.plain_tray_icon = !on,
                        cx,
                    ))
                    .child(self.pref_switch(
                        "pref-start-hidden",
                        "Start hidden in tray",
                        self.prefs.start_hidden,
                        |c, on| c.start_hidden = on,
                        cx,
                    )),
            )
            .child(separator(cx))
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
