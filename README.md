<p align="center">
  <img src="icon/soundglass.svg" width="128" alt="soundglass logo">
</p>

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
- View → Spectrogram opens it in its own window (log frequency, 20 Hz up): click to play from there, drag to select (the same selection as the Edit panel, so Loop works on it), scroll to zoom, Ctrl+scroll to zoom the frequencies, right-drag to move around in both directions; Fit shows everything again. Space, L, Esc, ←/→ and PageUp/Down work there too
- On the Spectrogram's toolbar, Chords shows a bar along the top with the chords (major, minor and 7ths) against the track's time; they follow the edits and the zoom, and are named like the note lines. It's a first try: it finds the main chords better than when they change (docs/CHORDS.md). Key ≈ … (once estimated) shows the detected key's scale (a minor key as its relative major). Notes shows lines at the notes' pitches (C thicker), named where they fit; Settings sets the tuning (A4), C D E or Do Ré Mi, sharps or flats, octave numbers, the scale shown (all notes or one of the 12 major scales, spelled as the key spells them), an instrument's range in concert pitch (trumpet E3–Bb5, tenor sax Ab2–Eb5), and the lines' look (colours for the tonic — C with all notes, the key's tonic with a scale — and the other lines, tonic width, label size). The spectrogram is worked out once per file (and again after an edit or an Analysis change), so zooming and panning are instant. Its Analysis part sets the FFT size, window size, hop (256, 512 or 1024 samples; raised for long files, sooner with big FFTs) — every FFT bin is kept and drawn, the window function, the level range, the lowest frequency and the colours, with Reset to defaults. All are remembered.
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

## Tags and names

- Track Info shows the estimated tempo and key (Audio, after loading; the first 10 minutes are analysed). Tempo is counted as people tap along, so it can be an octave off.
- In Track Info, Edit (in Tags) changes the title, artist, album, year, track, genre, BPM, key and cover; the estimate buttons fill BPM and Key, and ½× / 2× halve or double the BPM. Save… asks first, then writes the tags into the file; the audio is checked unchanged before the file is replaced. Save As and Save Copy also take tag edits not yet saved.
- File → Identify Track… (online, only when you click Look up) sends the file's Chromaprint fingerprint and length to AcoustID and lists the matches with their score; Use Match fetches the genre from MusicBrainz (sending only the recording's ID) and fills Track Info's tag editor, only the empty fields unless you untick that. Nothing is written until Save… there. It is greyed out until the build has an AcoustID key and a contact URL (`src/identify.rs`).
- File → Rename from Tags… renames the current file from a pattern such as `{track} - {artist} - {title}` (also `{album}`, `{year}`, `{genre}`), with a preview. A missing tag is left out, and an existing file is never replaced.

## Install as the default audio player

```
./install.sh
```

This builds soundglass, copies it to `~/.local/bin`, adds `soundglass.desktop` to `~/.local/share/applications`, and makes it the default for MP3, FLAC, WAV, Ogg Vorbis, M4A/AAC, AIFF, CAF and Matroska audio. Opus is not supported yet.

To undo: delete `~/.local/bin/soundglass`, `~/.local/share/applications/soundglass.desktop` and `~/.local/share/icons/hicolor/scalable/apps/soundglass.svg`, then pick another default app in System Settings → Default Applications (or File Associations).

## Licence

GPL-3.0-or-later.

Bundled libraries, built from source into the program: LAME (MP3 encoding, LGPL), libvorbis with the aoTuV and Lancer patches and libogg (Ogg Vorbis encoding, BSD-3-Clause). The Rust crates' licences are in `Cargo.lock`'s dependency list; all are GPL-3-compatible.
