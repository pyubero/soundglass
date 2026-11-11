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
- Ctrl+Shift+S: Save As (the original file is never changed), with the format, quality and sample rate, and whether to apply the speed and pitch
- Ctrl+O: open a file
- Ctrl+Q: quit

## Effects

The Effects menu works on the selection, or on the whole track when nothing is selected. Each item has presets and a Custom… value, and every effect can be undone:

- Fade in, Fade out: over the whole selection, or 1, 3 or 5 s from its start or end; the loudness changes evenly to the ear
- Gain: up or down by a number of dB
- Normalize: the peak near or at the maximum (−1 or 0 dBFS), or the loudness as streaming (−14 LUFS), as podcasts (−16) or as TV (−23)
- Trim silence: removes what is quieter than a level at the start and end
- Reverse: plays it backwards
- Channels: mono, stereo or swap left and right (always the whole track)

## Speed and pitch

The Playback menu changes how it plays, live, and the settings combine:

- Change speed and pitch: faster is also higher, like tape (0.5× to 2×)
- Pitch-preserved speed change: slower or faster at the same pitch (0.5× to 2×)
- Time-preserved pitch change: up or down by semitones at the same speed (−2 to +2, Custom… up to ±12 in steps of 0.1)

Reset puts all three back to normal. The last two use WSOLA, which can sound a little echoey or stuttery on dense mixes. Save As can apply them to the file.

## Install as the default audio player

```
./install.sh
```

This builds soundglass, copies it to `~/.local/bin`, adds `soundglass.desktop` to `~/.local/share/applications`, and makes it the default for MP3, FLAC, WAV, Ogg Vorbis, M4A/AAC, AIFF, CAF and Matroska audio. Opus is not supported yet.

To undo: delete `~/.local/bin/soundglass` and `~/.local/share/applications/soundglass.desktop`, then pick another default app in System Settings → Default Applications (or File Associations).

## Licence

GPL-3.0-or-later.

Bundled libraries, built from source into the program: LAME (MP3 encoding, LGPL), libvorbis with the aoTuV and Lancer patches and libogg (Ogg Vorbis encoding, BSD-3-Clause). The Rust crates' licences are in `Cargo.lock`'s dependency list; all are GPL-3-compatible.
