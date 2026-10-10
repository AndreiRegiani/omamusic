# Omamusic

A local music player for Omarchy Linux, inspired by the native macOS [Overdrive](https://github.com/skhaz/overdrive) interface. The application is written in Rust with GPUI and uses the active Omarchy theme.

## Install

On Omarchy / Arch Linux:

```sh
cargo install --path .
```

The installed player needs no separate media player, codec package, or GStreamer plugin. It uses the desktop's normal audio service through PulseAudio (including PipeWire's PulseAudio compatibility service), with ALSA available as a fallback. The optional folder picker uses `zenity` or `kdialog`; you can also type a directory path in Settings.

Building from source on Linux requires the ALSA development headers for CPAL (`alsa-lib` on Arch/Omarchy or `libasound2-dev` on Ubuntu), in addition to GPUI's normal graphics build requirements. These are build requirements, not playback plugins or companion programs.

Run `omamusic`, open Settings, and add a music folder. Configuration is stored at `~/.config/omamusic/config.json`.

## Features

- Local, read-only library scan with album, artist and song views; no database, accounts or network services.
- Embedded metadata and cover art for MP3, FLAC, MP4/M4A and Vorbis comments, with folder cover art fallback.
- In-process decoding through Rodio/Symphonia and output through CPAL. Supports MP3, FLAC, Vorbis, AAC/ALAC, WAV and AIFF without installed codecs.
- Play/pause, seek, previous/next, shuffle, repeat and saved volume.
- Fuzzy search and album detail views inspired by Overdrive.
- Automatic rescan on filesystem changes, including folders added in Settings.
- Omarchy theme following through `gpui-omarchy`.

Space toggles playback, Left/Right changes tracks, and Ctrl+R rescans the library. A folder picker uses `zenity` or `kdialog` if available.

## Architecture

`src/library.rs` scans configured directories on a background thread and keeps an in-memory snapshot. `src/metadata.rs` reads local tags without requiring a codec plugin. `src/player.rs` decodes and buffers audio on a worker thread and feeds CPAL's output callback, keeping disk reads and decoding off the audio thread. `src/app.rs` renders the GPUI interface and consumes the library and playback snapshots. `src/config.rs` persists folders and volume.

Opus is not decoded by the bundled Symphonia version, so `.opus` files are not included in the library. Opus streams inside `.ogg` containers are also unsupported.

MPRIS integration and system media key registration are not implemented yet. Track transitions occur at end of file but are not gapless.

## Development

```sh
cargo fmt --check
cargo check --offline
cargo clippy --offline --all-targets -- -D warnings
cargo test --offline
```

The regular playback tests use a simulated audio output; the ID3/MP3 regression test also requires `ffmpeg` to generate its fixture. The app itself does not use `ffmpeg`. To verify the actual desktop output with a silent WAV, run `cargo test system_audio_output_plays_file -- --ignored` from a desktop session.

## License

MIT
