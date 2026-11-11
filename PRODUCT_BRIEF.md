# Music app (working name) — Product Brief

*Status: draft, 2026-11-11.*

## What it is
A small, fast audio player and quick editor for Linux and Windows, written in Rust. It is lightview for sound: open a file and hear it at once, then do the few small jobs people usually open Audacity for, and save the result to a copy.

## Goals
1. **Fast.** The window appears in under 200 ms, sound starts before the whole file is decoded, and the waveform fills in as it loads.
2. **Safe.** The original file is never changed. Edits are saved only to a copy, with full undo and redo. Delete moves to the Trash.
3. **Small.** A handful of tools that each do one job. Not a DAW.
4. **Private.** No telemetry, no account. It goes online only when you ask (track identification, AI downloads).

## Not doing
- Multitrack mixing, effect chains, VST/LV2 plugins, MIDI editing
- A music library, playlist database or streaming services
- Recording studio features (simple recording is an open question)
- Identifying a clip recorded from the microphone, Shazam-style: there is no open database of commercial music to match against

## Carried over from lightview
egui/eframe UI, menus and assignable shortcuts, settings file, translations, screen reader support and high contrast theme, next file in folder decoded ahead, undo and Trash, Info window, .desktop default-app setup, .deb/.msi packaging, licence notices, `--bench` timings, and the opt-in ONNX Runtime and model manager.

## Proposed stack
symphonia (decoding), cpal (sound output), rubato (resampling), lofty (tags), hound (WAV), a FLAC encoder, LAME through mp3lame-encoder (MP3), libvorbis or libopus (Ogg/Opus).

## Milestones

### 1. Spike
- **Play from the command line.** `app song.mp3` opens a window and plays. We measure the time to first sound.

### 1.5. Usable player
- **Phone-shaped window.** Opens at 380 × 680 points; Settings can remember the last size instead.
- **Minimal view.** Menu bar on top for future tools; album cover (or a placeholder); title, artist and album, each scrolling when too long to fit; a thin seek bar with times; previous, play/pause and next buttons.
- **Previous/next in folder.** The next file is decoded ahead and follows the current one without a gap.
- **Default app.** `install.sh` installs it and makes it the default for every audio format it plays.

### 2. Player
- **Play, pause, seek.** Space to play/pause, click or arrow keys to jump.
- **Waveform overview.** The whole track drawn across the window; click anywhere to play from there.
- **Previous/next in folder.** Page Up/Down moves through the folder's audio files, and the next one is decoded ahead so it starts at once.
- **Gapless playback.** Albums that flow from track to track play without a gap.
- **Volume and speed.** A volume slider and playback speed from 0.5× to 2×.
- **Track Info.** A window with title, artist, album, cover art, length, format, codec, bitrate, sample rate, channels, bit depth, file size, loudness (LUFS) and peak level, and all other tags read-only.
- **Title bar.** File name, length and format.
- **Open and Open Recent, default app.** The system file dialog, the last ten files, and a .desktop file so it can be the default for audio files.

### 3. Looping and Edits
- **Select a region.** Drag on the waveform; zoom in for precise edges.
- **Play or loop the selection.** Hear exactly what you selected, over and over.
- **Cut, crop, silence.** Remove the selection, keep only the selection, or replace it with silence.
- **Undo and redo.** For every edit.
- **Save As.** WAV, FLAC, MP3 and Ogg Vorbis, with the quality chosen in the dialog. Tags are kept. Never overwrites the original.

### 4. Quick fixes for the entire track (by default) or to the entire selection
- **Fade in / fade out.** A smooth start or end.
- **Gain.** Make the track louder or quieter by a set number of dB.
- **Normalize.** Bring the peak to a level, or the loudness to a target LUFS so several files sound equally loud.
- **Trim silence.** Remove the quiet bits at the start and end.
- **Mono, stereo, channel swap.** Mix down to mono, duplicate mono to stereo, or swap left and right.
- **Reverse.** Play the selection backwards.
- **Change sample rate.** For example 48 kHz to 44.1 kHz, chosen in Save As.

### 5. Speed and pitch
- **Study time-stretching first.** Compare writing our own (WSOLA, phase vocoder) with existing crates: licence, quality on music, and whether it runs in real time for the slow-down loop in milestone 7. Pick one before building. The findings are in [docs/TIME_STRETCH.md](docs/TIME_STRETCH.md).
- **Change speed or pitch.** Separately: slower without lower pitch, or a semitone up without faster. Done with our own WSOLA, live while playing, as three Playback items: Change speed and pitch (tape style), Pitch-preserved speed change, and Time-preserved pitch change. Save As can apply them to the file.

### 6. Files and tags
- **Edit tags.** Title, artist, album, year, track number, genre and cover art, saved to a copy or, with confirmation, in place (tags only, never the audio). Done: Edit in Track Info's Tags section; the copy's audio is checked identical before it replaces the file, and Save As / Save Copy take unsaved tag edits.
- **Rename from tags.** For example `01 - Artist - Title.mp3`, shown before anything is renamed. Done: File → Rename from Tags… on the current file, with an editable pattern that is remembered; it never replaces an existing file.
- DONT IMPLEMENT **Split into tracks.** Split a long recording (vinyl, a concert) at silences or markers, with a preview of the cut points.
- DONT IMPLEMENT **Join files.** Put several files one after another into one file.
- DONT IMPLEMENT **Convert a folder.** Save every file in a folder in another format and quality.

### 7. Practice tools
- **A–B loop with slow-down.** Loop a hard passage at a slower speed without changing the pitch. Done with existing tools: drag on the Edit waveform to select, L to loop, Playback → Pitch-preserved speed change.
- **Spectrogram.** See frequencies over time, to spot hum, clicks or where notes are. This shows the spectrogram first in a separate panel where we can zoom in/out and right click and drag for panning. Done: View → Spectrogram, its own window with a log frequency axis, click to seek, with note lines and their settings on its toolbar.
- **BPM and key.** Estimate the tempo and musical key of the track.
- **Markers with notes.** Drop named markers (chorus, solo) and jump between them; they can be saved next to the file. Only if there is an automatic way of doing this.
- **Metronome.** A click at a set tempo, alone or over the track, with the song's tempo by default and on the right spots, beware of swing.
- **Chord recognition** Check if it is even possible.


### 8. Tidy up
- **Identify track.** Make a Chromaprint fingerprint of the file (pure Rust, rusty-chromaprint), look it up online in AcoustID, then fill in the missing tags from MusicBrainz. Show the match and its score, and on request write the tags and rename the file. Only the fingerprint and the length are sent, and only when you run it. Free for non-commercial use, with one application key; at most 3 lookups per second (AcoustID) and 1 per second (MusicBrainz). Works on whole files, not clips.
- **Clean up a music folder.** The same in one pass over a folder and all its subfolders: identify every file, show a list of proposed tags and new names to accept or skip, then apply. Files that aren't found are left alone. Ctrl+Z undoes the renames. About 1,000 files take 6 minutes at AcoustID's limit.
- **Group duplicate songs.** Find copies of the same recording in a folder by how they sound, even in different formats or bitrates, and even when one copy is trimmed or starts later (peak-based matching like Olaf or Panako, written by us; it compares only your own files, so it needs no database), show the groups, and move each into its own folder. Nothing is deleted, and Ctrl+Z puts everything back.
- **Use a local database (later, for power users).** Point Identify Track at your own AcoustID and MusicBrainz server built from their public data exports, so nothing goes online. Too big to ship with the app.

### 9. Local AI (optional download)
- **Split into stems.** Vocals, drums, bass and the rest as separate files, for karaoke or practising along.
- **Remove noise.** Clean background noise and hum from voice recordings.
- **Transcribe speech.** Turn a talk or voice memo into text you can copy.
- **Melody to MIDI.** Turn a sung or played melody into a MIDI file.

### 10. Our own engine: the Griffin–Lim modulator in Rust
- **Build on the audio-tools Griffin–Lim modulator** to make a better, improved, faster version in pure Rust. Ideally it has two settings: Fast, for real time (frame by frame with a small look-ahead, like RTISI-LA), and Quality, slower and offline (more iterations, started from phase-vocoder phases). It replaces WSOLA where it sounds better. The ideas to start from are in [docs/TIME_STRETCH.md](docs/TIME_STRETCH.md), section 3.

## Open questions
- Name.
- Is simple microphone recording in scope (voice memos, tuner)?
- Track identification: is rusty-chromaprint's output identical to the original Chromaprint, and what match score to accept automatically.
