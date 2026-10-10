use crate::config::AppConfig;
use crate::models::{PlayerState, RepeatMode, Track};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::num::NonZero;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

type AudioSource = UniformSourceIterator<Decoder<BufReader<File>>>;

#[derive(Default)]
struct Playback {
    queue: Vec<Track>,
    order: Vec<Track>,
    index: usize,
    state: PlayerState,
    volume: f64,
    shuffle: bool,
    repeat: RepeatMode,
    duration: f64,
    current: Option<Track>,
}

#[derive(Default)]
struct AudioBuffer {
    samples: VecDeque<f32>,
    played_samples: u64,
    base_position: f64,
}

struct Shared {
    playback: Mutex<Playback>,
    audio: Mutex<AudioBuffer>,
    channels: usize,
    sample_rate: u32,
}

enum Command {
    Load(Track),
    Seek(f64),
    Clear,
    Shutdown,
}

pub struct Player {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
    _stream: Option<Mutex<cpal::Stream>>,
    #[cfg(test)]
    test_driver: Option<JoinHandle<()>>,
    #[cfg(test)]
    test_running: Arc<AtomicBool>,
}

fn following(index: usize, len: usize, repeat: RepeatMode, manual: bool) -> Option<usize> {
    if len == 0 {
        None
    } else if repeat == RepeatMode::One && !manual {
        Some(index)
    } else if index + 1 < len {
        Some(index + 1)
    } else if repeat == RepeatMode::Off {
        None
    } else {
        Some(0)
    }
}

fn select(playback: &mut Playback, index: usize) -> Option<Track> {
    playback.index = index;
    let track = playback.queue.get(index)?.clone();
    playback.duration = track.duration;
    playback.current = Some(track.clone());
    playback.state = PlayerState::Playing;
    Some(track)
}

fn stop(playback: &mut Playback) {
    playback.state = PlayerState::Stopped;
    playback.current = None;
    playback.duration = 0.0;
}

fn fill_output<T: cpal::SizedSample + cpal::FromSample<f32>>(data: &mut [T], shared: &Shared) {
    let (playing, volume) = {
        let state = shared.playback.lock();
        (state.state == PlayerState::Playing, state.volume as f32)
    };
    let mut audio = shared.audio.lock();
    for sample in data {
        let value = if playing {
            if let Some(value) = audio.samples.pop_front() {
                audio.played_samples += 1;
                value * volume
            } else {
                0.0
            }
        } else {
            0.0
        };
        *sample = T::from_sample(value.clamp(-1.0, 1.0));
    }
}

fn build_output<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<cpal::Stream> {
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| fill_output(data, &shared),
        |err| eprintln!("audio output: {err}"),
        None,
    )?)
}

fn open_source(track: &Track, channels: usize, rate: u32) -> Result<(AudioSource, f64)> {
    let file = File::open(&track.path).with_context(|| format!("open {}", track.path.display()))?;
    let decoder =
        Decoder::try_from(file).with_context(|| format!("decode {}", track.path.display()))?;
    let duration = decoder
        .total_duration()
        .map_or(track.duration, |d| d.as_secs_f64());
    let channels = NonZero::new(channels as u16).context("zero output channels")?;
    let rate = NonZero::new(rate).context("zero output sample rate")?;
    Ok((
        UniformSourceIterator::new(decoder, channels, rate),
        duration,
    ))
}

fn load_source(shared: &Shared, track: Track) -> Option<AudioSource> {
    {
        let mut audio = shared.audio.lock();
        audio.samples.clear();
        audio.played_samples = 0;
        audio.base_position = 0.0;
    }
    match open_source(&track, shared.channels, shared.sample_rate) {
        Ok((source, duration)) => {
            shared.playback.lock().duration = duration;
            Some(source)
        }
        Err(err) => {
            eprintln!("playback: {err:#}");
            stop(&mut shared.playback.lock());
            None
        }
    }
}

fn apply_command(
    command: Command,
    shared: &Shared,
    source: &mut Option<AudioSource>,
    end_of_file: &mut bool,
) -> bool {
    match command {
        Command::Load(track) => {
            *source = load_source(shared, track);
            *end_of_file = false;
        }
        Command::Seek(secs) => {
            if let Some(source) = source.as_mut() {
                if let Err(err) = source.try_seek(Duration::from_secs_f64(secs)) {
                    eprintln!("seek: {err}");
                } else {
                    let mut audio = shared.audio.lock();
                    audio.samples.clear();
                    audio.played_samples = 0;
                    audio.base_position = secs;
                    *end_of_file = false;
                }
            }
        }
        Command::Clear => {
            *source = None;
            *end_of_file = false;
            let mut audio = shared.audio.lock();
            audio.samples.clear();
            audio.played_samples = 0;
            audio.base_position = 0.0;
        }
        Command::Shutdown => return false,
    }
    true
}

fn run_decoder(shared: Arc<Shared>, commands: Receiver<Command>) {
    let mut source: Option<AudioSource> = None;
    let mut end_of_file = false;
    loop {
        while let Ok(command) = commands.try_recv() {
            if !apply_command(command, &shared, &mut source, &mut end_of_file) {
                return;
            }
        }

        if let Some(active) = source.as_mut() {
            let buffered = shared.audio.lock().samples.len();
            if !end_of_file && buffered < shared.sample_rate as usize * shared.channels {
                let mut block = Vec::with_capacity(4096);
                for _ in 0..4096 {
                    if let Some(sample) = active.next() {
                        block.push(sample);
                    } else {
                        end_of_file = true;
                        break;
                    }
                }
                shared.audio.lock().samples.extend(block);
                continue;
            }
            if end_of_file && buffered == 0 {
                let next = {
                    let mut playback = shared.playback.lock();
                    let next =
                        following(playback.index, playback.queue.len(), playback.repeat, false);
                    if let Some(index) = next {
                        select(&mut playback, index)
                    } else {
                        stop(&mut playback);
                        None
                    }
                };
                source = next.and_then(|track| load_source(&shared, track));
                end_of_file = false;
                continue;
            }
        }
        if source.is_none() {
            match commands.recv() {
                Ok(command) => {
                    if !apply_command(command, &shared, &mut source, &mut end_of_file) {
                        return;
                    }
                }
                Err(_) => return,
            }
            continue;
        }
        match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => {
                if !apply_command(command, &shared, &mut source, &mut end_of_file) {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

impl Player {
    pub fn new() -> Result<Self> {
        let preferred = cpal::default_host().id();
        let mut hosts = vec![preferred];
        hosts.extend(
            cpal::available_hosts()
                .into_iter()
                .filter(|id| *id != preferred),
        );
        let mut failures = Vec::new();
        for id in hosts {
            let Ok(host) = cpal::host_from_id(id) else {
                continue;
            };
            let Some(device) = host.default_output_device() else {
                failures.push(format!("{id}: no default output device"));
                continue;
            };
            match Self::from_device(&device) {
                Ok(player) => return Ok(player),
                Err(err) => failures.push(format!("{id}: {err:#}")),
            }
        }
        anyhow::bail!("no usable audio output: {}", failures.join("; "))
    }

    fn from_device(device: &cpal::Device) -> Result<Self> {
        let mut supported = device
            .default_output_config()
            .context("query audio output")?;
        if !matches!(
            supported.sample_format(),
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            supported = device
                .supported_output_configs()?
                .find(|config| {
                    config.sample_format() == cpal::SampleFormat::F32
                        && config.contains_rate(48_000)
                })
                .context("no supported PCM audio format")?
                .with_sample_rate(48_000);
        }
        let config = supported.config();
        if config.channels == 0 || config.sample_rate == 0 {
            anyhow::bail!("audio device reported an invalid stream configuration");
        }
        let mut player = Self::create(config.channels, config.sample_rate);
        let shared = player.shared.clone();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build_output::<f32>(device, config, shared)?,
            cpal::SampleFormat::I16 => build_output::<i16>(device, config, shared)?,
            cpal::SampleFormat::U16 => build_output::<u16>(device, config, shared)?,
            _ => anyhow::bail!(
                "unsupported audio sample format: {:?}",
                supported.sample_format()
            ),
        };
        stream.play().context("start audio output")?;
        player._stream = Some(Mutex::new(stream));
        Ok(player)
    }

    fn create(channels: u16, sample_rate: u32) -> Self {
        let config = AppConfig::load();
        let shared = Arc::new(Shared {
            playback: Mutex::new(Playback {
                volume: config.volume.clamp(0.0, 1.0),
                ..Default::default()
            }),
            audio: Mutex::new(AudioBuffer::default()),
            channels: channels as usize,
            sample_rate,
        });
        let (commands, receiver) = mpsc::channel();
        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || run_decoder(worker_shared, receiver));
        Self {
            shared,
            commands,
            worker: Some(worker),
            _stream: None,
            #[cfg(test)]
            test_driver: None,
            #[cfg(test)]
            test_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn play(&self, tracks: Vec<Track>, at: usize) {
        if tracks.is_empty() {
            return;
        }
        let track = {
            let mut p = self.shared.playback.lock();
            p.order = tracks.clone();
            p.queue = tracks;
            let start = at.min(p.queue.len() - 1);
            let index = if p.shuffle {
                let first = p.queue.remove(start);
                p.queue.shuffle(&mut rand::thread_rng());
                p.queue.insert(0, first);
                0
            } else {
                start
            };
            select(&mut p, index)
        };
        if let Some(track) = track {
            let _ = self.commands.send(Command::Load(track));
        }
    }

    pub fn toggle(&self) {
        if self.state() == PlayerState::Playing {
            self.pause();
        } else {
            self.resume();
        }
    }

    pub fn resume(&self) {
        let mut p = self.shared.playback.lock();
        if p.current.is_some() {
            p.state = PlayerState::Playing;
        }
    }

    pub fn pause(&self) {
        let mut p = self.shared.playback.lock();
        if p.current.is_some() {
            p.state = PlayerState::Paused;
        }
    }

    pub fn stop(&self) {
        let mut p = self.shared.playback.lock();
        stop(&mut p);
        p.queue.clear();
        p.order.clear();
        let _ = self.commands.send(Command::Clear);
    }

    pub fn next(&self) {
        let track = {
            let mut p = self.shared.playback.lock();
            let next = following(p.index, p.queue.len(), p.repeat, true);
            next.and_then(|index| select(&mut p, index))
        };
        if let Some(track) = track {
            let _ = self.commands.send(Command::Load(track));
        } else {
            self.stop();
        }
    }

    pub fn previous(&self) {
        let track = {
            let mut p = self.shared.playback.lock();
            let position = self.position();
            if position < 3.0 && p.index > 0 {
                let index = p.index - 1;
                select(&mut p, index)
            } else {
                None
            }
        };
        if let Some(track) = track {
            let _ = self.commands.send(Command::Load(track));
        } else {
            self.seek(0.0);
        }
    }

    pub fn seek(&self, secs: f64) {
        if self.current().is_some() && secs.is_finite() {
            let secs = secs.max(0.0);
            let _ = self.commands.send(Command::Seek(secs));
        }
    }

    pub fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        self.shared.playback.lock().volume = volume;
        let mut config = AppConfig::load();
        config.volume = volume;
        if let Err(err) = config.save() {
            eprintln!("save volume: {err:#}");
        }
    }

    pub fn volume(&self) -> f64 {
        self.shared.playback.lock().volume
    }
    pub fn set_shuffle(&self, enabled: bool) {
        let mut p = self.shared.playback.lock();
        p.shuffle = enabled;
        if let Some(current) = p.current.clone() {
            if enabled {
                p.queue.retain(|track| track.path != current.path);
                p.queue.shuffle(&mut rand::thread_rng());
                p.queue.insert(0, current);
                p.index = 0;
            } else {
                p.queue = p.order.clone();
                p.index = p
                    .order
                    .iter()
                    .position(|track| track.path == current.path)
                    .unwrap_or(0);
            }
        }
    }
    pub fn shuffle(&self) -> bool {
        self.shared.playback.lock().shuffle
    }
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.shared.playback.lock().repeat = mode;
    }
    pub fn repeat(&self) -> RepeatMode {
        self.shared.playback.lock().repeat
    }
    pub fn state(&self) -> PlayerState {
        self.shared.playback.lock().state
    }
    pub fn position(&self) -> f64 {
        let audio = self.shared.audio.lock();
        audio.base_position
            + audio.played_samples as f64
                / (self.shared.sample_rate as f64 * self.shared.channels as f64)
    }
    pub fn duration(&self) -> f64 {
        self.shared.playback.lock().duration
    }
    pub fn current(&self) -> Option<Track> {
        self.shared.playback.lock().current.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        #[cfg(test)]
        {
            self.test_running.store(false, Ordering::Relaxed);
            if let Some(driver) = self.test_driver.take() {
                let _ = driver.join();
            }
        }
    }
}

#[cfg(test)]
impl Player {
    fn new_test() -> Self {
        let mut player = Self::create(2, 48_000);
        player.test_running.store(true, Ordering::Relaxed);
        let shared = player.shared.clone();
        let running = player.test_running.clone();
        player.test_driver = Some(std::thread::spawn(move || {
            let mut block = vec![0.0f32; 960];
            while running.load(Ordering::Relaxed) {
                fill_output(&mut block, &shared);
                std::thread::sleep(Duration::from_millis(10));
            }
        }));
        player
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::MetadataReader;
    use std::time::Instant;

    fn track(path: std::path::PathBuf) -> Track {
        Track {
            path,
            title: "Test".into(),
            artist: String::new(),
            album: String::new(),
            track_number: 0,
            year: 0,
            duration: 0.0,
            album_id: String::new(),
        }
    }

    fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        condition()
    }

    fn write_silent_wav(path: &std::path::Path) {
        let samples = 44_100u32;
        let mut wav = Vec::with_capacity(44 + samples as usize * 2);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&samples.to_le_bytes());
        wav.extend_from_slice(&(samples * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples * 2).to_le_bytes());
        wav.resize(44 + samples as usize * 2, 0);
        std::fs::write(path, wav).unwrap();
    }

    #[test]
    fn wav_playback_controls_and_eof() {
        let path = std::env::temp_dir().join(format!("omamusic-test-{}.wav", std::process::id()));
        write_silent_wav(&path);

        let player = Player::new_test();
        player.play(vec![track(path.clone())], 0);
        assert!(
            wait_for(|| player.position() > 0.05),
            "WAV position did not advance"
        );
        assert!(player.duration() > 0.9);
        player.pause();
        assert_eq!(player.state(), PlayerState::Paused);
        player.resume();
        assert_eq!(player.state(), PlayerState::Playing);
        assert!(
            wait_for(|| player.state() == PlayerState::Stopped),
            "WAV did not finish"
        );
        drop(player);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mp3_with_id3_decodes_without_plugins_or_external_player() {
        let path =
            std::env::temp_dir().join(format!("omamusic-id3-test-{}.mp3", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-metadata",
                "title=ID3 test",
                "-metadata",
                "artist=Test artist",
                "-id3v2_version",
                "3",
                "-y",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg is required only to create the test fixture");
        assert!(status.success());
        let metadata = MetadataReader::new().read(&path).unwrap();
        assert_eq!(metadata.title, "ID3 test");
        let player = Player::new_test();
        player.play(vec![track(path.clone())], 0);
        assert!(
            wait_for(|| player.position() > 0.05),
            "MP3 position did not advance"
        );
        drop(player);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires a running desktop audio service"]
    fn system_audio_output_plays_file() {
        let path =
            std::env::temp_dir().join(format!("omamusic-device-test-{}.wav", std::process::id()));
        write_silent_wav(&path);
        let player = Player::new().unwrap();
        player.play(vec![track(path.clone())], 0);
        assert!(
            wait_for(|| player.position() > 0.05),
            "device did not consume audio"
        );
        drop(player);
        std::fs::remove_file(path).unwrap();
    }
}
