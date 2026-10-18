use crate::desktop::{self, Command};
use crate::models::{PlayerState, RepeatMode};
use crate::player::Player;
use async_channel::Sender;
use mpris_server::zbus::{fdo, Result};
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};
use parking_lot::Mutex;
use std::sync::Arc;

pub struct Mpris {
    player: Arc<Player>,
    commands: Sender<Command>,
    art: Mutex<Option<String>>,
}

impl Mpris {
    pub fn set_art(&self, url: Option<String>) {
        *self.art.lock() = url;
    }

    fn send(&self, command: Command) -> fdo::Result<()> {
        self.commands
            .try_send(command)
            .map_err(|err| fdo::Error::Failed(err.to_string()))
    }

    fn status(&self) -> PlaybackStatus {
        match self.player.state() {
            PlayerState::Playing => PlaybackStatus::Playing,
            PlayerState::Paused => PlaybackStatus::Paused,
            PlayerState::Stopped => PlaybackStatus::Stopped,
        }
    }

    fn loop_status(&self) -> LoopStatus {
        match self.player.repeat() {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::All => LoopStatus::Playlist,
            RepeatMode::One => LoopStatus::Track,
        }
    }

    fn metadata(&self) -> Metadata {
        let Some(track) = self.player.current() else {
            return Metadata::builder().trackid(TrackId::NO_TRACK).build();
        };
        let id = format!(
            "/org/omamusic/track/t{:016x}",
            desktop::hash(&track.path.to_string_lossy())
        );
        let mut metadata = Metadata::builder()
            .title(track.title)
            .artist([track.artist])
            .album(track.album)
            .length(seconds(self.player.duration()))
            .url(format!("file://{}", track.path.display()));
        if let Ok(id) = TrackId::try_from(id) {
            metadata = metadata.trackid(id);
        }
        if track.track_number > 0 {
            metadata = metadata.track_number(track.track_number as i32);
        }
        if let Some(art) = self.art.lock().clone() {
            metadata = metadata.art_url(art);
        }
        metadata.build()
    }
}

fn seconds(secs: f64) -> Time {
    Time::from_micros((secs.max(0.0) * 1_000_000.0) as i64)
}

pub struct MprisServer(Server<Mpris>);

impl MprisServer {
    pub fn imp(&self) -> &Mpris {
        self.0.imp()
    }

    pub async fn changed(&self, track_changed: bool) {
        let imp = self.0.imp();
        let has_track = imp.player.current().is_some();
        let mut properties = vec![
            Property::PlaybackStatus(imp.status()),
            Property::CanGoNext(has_track),
            Property::CanGoPrevious(has_track),
            Property::CanSeek(has_track),
            Property::LoopStatus(imp.loop_status()),
            Property::Shuffle(imp.player.shuffle()),
        ];
        if track_changed || has_track {
            properties.push(Property::Metadata(imp.metadata()));
        }
        if let Err(err) = self.0.properties_changed(properties).await {
            eprintln!("mpris: {err}");
        }
    }

    pub async fn seeked(&self, position: f64) {
        let signal = Signal::Seeked {
            position: seconds(position),
        };
        if let Err(err) = self.0.emit(signal).await {
            eprintln!("mpris: {err}");
        }
    }
}

pub async fn serve(player: Arc<Player>, commands: Sender<Command>) -> Result<MprisServer> {
    let imp = Mpris {
        player,
        commands,
        art: Mutex::new(None),
    };
    Ok(MprisServer(Server::new("omamusic", imp).await?))
}

impl RootInterface for Mpris {
    async fn raise(&self) -> fdo::Result<()> {
        self.send(Command::ShowWindow)
    }

    async fn quit(&self) -> fdo::Result<()> {
        self.send(Command::Quit)
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("omamusic".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("omamusic".into())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for Mpris {
    async fn next(&self) -> fdo::Result<()> {
        self.player.next();
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.player.previous();
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.player.pause();
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.player.toggle();
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.player.stop();
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        self.player.resume();
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let target = self.player.position() + offset.as_micros() as f64 / 1_000_000.0;
        if target >= self.player.duration() {
            self.player.next();
        } else {
            self.player.seek(target);
        }
        Ok(())
    }

    async fn set_position(&self, _track_id: TrackId, position: Time) -> fdo::Result<()> {
        let target = position.as_micros() as f64 / 1_000_000.0;
        if (0.0..=self.player.duration()).contains(&target) {
            self.player.seek(target);
        }
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("OpenUri is not supported".into()))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.status())
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(self.loop_status())
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> Result<()> {
        self.player.set_repeat(match loop_status {
            LoopStatus::None => RepeatMode::Off,
            LoopStatus::Playlist => RepeatMode::All,
            LoopStatus::Track => RepeatMode::One,
        });
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.player.shuffle())
    }

    async fn set_shuffle(&self, shuffle: bool) -> Result<()> {
        self.player.set_shuffle(shuffle);
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.metadata())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.player.volume())
    }

    async fn set_volume(&self, volume: Volume) -> Result<()> {
        self.player.set_volume(volume);
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(seconds(self.player.position()))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.player.current().is_some())
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.player.current().is_some())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.player.current().is_some())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}
