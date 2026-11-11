# soundglass

A small, fast audio player for Linux, written in Rust. Open a file and hear it at once. See [PRODUCT_BRIEF.md](PRODUCT_BRIEF.md) for where it is heading.

## Build and run

Needs Rust and the ALSA headers (`sudo apt install libasound2-dev` on Debian/Ubuntu).

```
cargo run --release -- song.mp3
```

Without a file it asks for one. `--timings` prints the time to the window and to the first sound; `--bench` exits at the first sound.

## Keys

- Space: play/pause
- Left / Right: back or forward 5 s
- Page Up / Page Down: previous or next file in the folder
- Ctrl+O: open a file
- Ctrl+Q: quit

## Install as the default audio player

```
./install.sh
```

This builds soundglass, copies it to `~/.local/bin`, adds `soundglass.desktop` to `~/.local/share/applications`, and makes it the default for MP3, FLAC, WAV, Ogg Vorbis, M4A/AAC, AIFF, CAF and Matroska audio. Opus is not supported yet.

To undo: delete `~/.local/bin/soundglass` and `~/.local/share/applications/soundglass.desktop`, then pick another default app in System Settings → Default Applications (or File Associations).

## Licence

GPL-3.0-or-later.
