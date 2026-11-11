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
- Ctrl+E: show or hide the Edit panel
- In the Edit panel: click to play from there, drag to select, Shift+click or Shift+drag to change the selection, scroll to zoom, right-drag (or scroll sideways) to move along
- Esc: clear the selection
- Delete: cut out the selection; Ctrl+T: crop to it; Ctrl+L: replace it with silence
- Ctrl+Z: undo; Ctrl+Shift+Z or Ctrl+Y: redo
- L: loop the selection on or off (otherwise playback stops at its end)
- Ctrl+S: Save Copy — the edits next to the original, in its format, as `name-edited` (then `name-edited-2`…)
- Ctrl+Shift+S: Save As (the original file is never changed)
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

Bundled libraries, built from source into the program: LAME (MP3 encoding, LGPL), libvorbis with the aoTuV and Lancer patches and libogg (Ogg Vorbis encoding, BSD-3-Clause). The Rust crates' licences are in `Cargo.lock`'s dependency list; all are GPL-3-compatible.
