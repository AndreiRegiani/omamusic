use crate::desktop::{Command, NowPlaying};
use crate::models::PlayerState;
use crate::player::Player;
use async_channel::Sender;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Orientation, ToolTip};
use std::sync::Arc;

pub struct Tray {
    pub now: NowPlaying,
    player: Arc<Player>,
    commands: Sender<Command>,
}

impl Tray {
    pub fn new(player: Arc<Player>, commands: Sender<Command>) -> Self {
        Self {
            now: NowPlaying::default(),
            player,
            commands,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send_blocking(command);
    }

    fn cover_icon(&self) -> Option<&Icon> {
        let cover = self.now.cover.as_ref().filter(|_| self.now.cover_icon)?;
        Some(if self.now.state == PlayerState::Paused {
            &cover.dimmed
        } else {
            &cover.icon
        })
    }
}

fn item(
    label: &str,
    icon_name: &str,
    activate: impl Fn(&mut Tray) + Send + 'static,
) -> MenuItem<Tray> {
    StandardItem {
        label: label.into(),
        icon_name: icon_name.into(),
        activate: Box::new(activate),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "omamusic".into()
    }

    fn title(&self) -> String {
        "omamusic".into()
    }

    fn icon_name(&self) -> String {
        if self.cover_icon().is_some() {
            String::new()
        } else {
            "audio-x-generic".into()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.cover_icon().cloned().into_iter().collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let Some(track) = &self.now.track else {
            return ToolTip {
                title: "omamusic".into(),
                ..Default::default()
            };
        };
        let paused = if self.now.state == PlayerState::Paused {
            " · Paused"
        } else {
            ""
        };
        ToolTip {
            title: track.title.clone(),
            description: format!("{} — {}{paused}", track.artist, track.album),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::ToggleWindow);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.player.toggle();
    }

    fn scroll(&mut self, delta: i32, orientation: Orientation) {
        if orientation != Orientation::Vertical {
            return;
        }
        if delta > 0 {
            self.player.previous();
        } else {
            self.player.next();
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let has_track = self.now.track.is_some();
        let mut menu = Vec::new();
        if let Some(track) = &self.now.track {
            menu.push(
                StandardItem {
                    label: format!("{} — {}", track.title, track.artist).replace('_', "__"),
                    enabled: false,
                    icon_data: self
                        .now
                        .cover
                        .as_ref()
                        .map(|c| c.menu_png.clone())
                        .unwrap_or_default(),
                    ..Default::default()
                }
                .into(),
            );
            menu.push(MenuItem::Separator);
        }
        let (label, icon_name) = if self.now.state == PlayerState::Playing {
            ("Pause", "media-playback-pause")
        } else {
            ("Play", "media-playback-start")
        };
        menu.extend([
            item(label, icon_name, |tray| tray.player.toggle()),
            item("Previous", "media-skip-backward", |tray| {
                tray.player.previous()
            }),
            item("Next", "media-skip-forward", |tray| tray.player.next()),
        ]);
        if !has_track {
            for entry in &mut menu {
                if let MenuItem::Standard(standard) = entry {
                    standard.enabled = false;
                }
            }
        }
        menu.extend([
            MenuItem::Separator,
            item("Show omamusic", "window-new", |tray| {
                tray.send(Command::ShowWindow)
            }),
            MenuItem::Separator,
            item("Quit omamusic", "application-exit", |tray| {
                tray.send(Command::Quit)
            }),
        ]);
        menu
    }
}
