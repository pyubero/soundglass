# Music app (working name) — Product Brief

*Status: draft, 2026-11-11; split into v1 (player) and v2 (Practice mode) 2026-11-12; v0.1.0 released 2026-11-15; regrouped into v1–v4 2026-11-15.*

## What it is
A small, fast music player for Linux and Windows, written in Rust, with a quick editor: lightview for sound. Open a file and hear it at once, move through its folder, do the few small jobs people usually open Audacity for, and save the result to a copy.

It comes in four parts:
- **v1, the player:** playing, browsing a folder, tags, quick edits and fixes, a few AI tools for cleaning up audio, MIDI files played with a basic piano.
- **v2, Practice mode:** tools for musicians and musicology that dig into a track: loops, the spectrogram with notes, tempo, key, chords, a metronome, stems. They are switched on with View → Practice mode, which is off by default; with it off they are neither shown nor worked out.
- **v3, our own models and engines:** better chord recognition, beat tracking, notes to MIDI and time-stretching, built or trained by us.
- **v4, a music library (very tentative):** a library, m3u playlists, recommendations, and tidying up a whole collection.

## Goals
1. **Fast.** The window appears in under 200 ms, sound starts before the whole file is decoded, and the waveform fills in as it loads.
2. **Safe.** The original file is never changed. Edits are saved only to a copy, with full undo and redo. The app never deletes a file.
3. **Small.** A handful of tools that each do one job. Not a DAW.
4. **Private.** No telemetry, no account. It goes online only when you ask (track identification, AI downloads).

## Not doing
- Multitrack mixing, effect chains, VST/LV2 plugins, MIDI editing
- Streaming services; a library database and playlists only as the very tentative v4 (browsing the folders on disk is in)
- Recording studio features (simple recording is an open question)
- Identifying a clip recorded from the microphone, Shazam-style: there is no open database of commercial music to match against

## Carried over from lightview
egui/eframe UI, menus and assignable shortcuts, settings and app state in eframe's data folder (`app.ron`; `~/.local/share/soundglass/`, `%APPDATA%\soundglass\data\` on Windows), themes as editable XML files there, translations, screen reader support and high contrast theme, next file in folder decoded ahead, undo, Info window, .desktop default-app setup, .deb/.msi packaging, licence notices, `--bench` timings, and the opt-in ONNX Runtime and model manager.

## Proposed stack
symphonia (decoding), cpal (sound output), rubato (resampling), lofty (tags), hound (WAV), a FLAC encoder, LAME through mp3lame-encoder (MP3), libvorbis or libopus (Ogg/Opus).

## v1: Player

### Packaging
- **.deb and .msi.** Like lightview's `package-deb.sh` and WiX file: they install only the binary, the .desktop file and the icon; every user file (settings, recent files, themes, AI models) lives in the data folder, so the packages and `install.sh` behave the same. Done: `package-deb.sh`, `packaging/windows/soundglass.wxs` and `.github/workflows/build.yml` (awaiting the first run on GitHub).

### 1. Spike
- **Play from the command line.** `app song.mp3` opens a window and plays. We measure the time to first sound.

### 1.5. Usable player
- **Phone-shaped window.** Opens at 380 × 680 points (Settings can change that size); Settings can remember the last size instead.
- **Minimal view.** Menu bar on top for future tools; album cover (or a placeholder); title, artist and album, each scrolling when too long to fit; a thin seek bar with times; previous, play/pause and next buttons.
- **Previous/next in folder.** The next file is decoded ahead and follows the current one without a gap.
- **Default app.** `install.sh` installs it and makes it the default for every audio format it plays.

### 2. Player
- **Play, pause, seek.** Space to play/pause, click or arrow keys to jump.
- **Waveform overview.** The whole track drawn across the window; click anywhere to play from there.
- **Previous/next in folder.** Page Up/Down moves through the folder's audio files, and the next one is decoded ahead so it starts at once.
- **Gapless playback.** Albums that flow from track to track play without a gap.
- **Volume and speed.** A volume slider and playback speed from 0.5× to 2×.
- **Play order.** A button right of Next cycles In order, Repeat track and Shuffle (each track once, in a random order, then stop); the order and the volume are kept between runs.
- **Track Info.** A window with title, artist, album, cover art, length, format, codec, bitrate, sample rate, channels, bit depth, file size, loudness (LUFS) and peak level, and all other tags read-only.
- **Title bar.** File name, length and format.
- **Open and Open Recent, default app.** The system file dialog, the last ten files, and a .desktop file so it can be the default for audio files.

### 3. Edits
- **Select a region.** Drag on the waveform; zoom in for precise edges.
- **Play the selection.** Hear exactly what you selected (looping it is in Practice mode).
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
- **Study time-stretching first.** Compare writing our own (WSOLA, phase vocoder) with existing crates: licence, quality on music, and whether it runs in real time for the slow-down loop in Practice mode. Pick one before building. The findings are in [docs/TIME_STRETCH.md](docs/TIME_STRETCH.md).
- **Change speed or pitch.** Separately: slower without lower pitch, or a semitone up without faster. Done with our own WSOLA, live while playing, as three Track items: Change speed and pitch (tape style), Pitch-preserved speed change, and Time-preserved pitch change. Save As can apply them to the file.

### 6. Files and tags
- **Edit tags.** Title, artist, album, year, track number, genre and cover art, saved to a copy or, with confirmation, in place (tags only, never the audio). Done: Edit in Track Info's Tags section; the copy's audio is checked identical before it replaces the file, and Save As / Save Copy take unsaved tag edits.
- **Rename from tags.** For example `01 - Artist - Title.mp3`, shown before anything is renamed. Done: File → Rename from Tags… on the current file, with an editable pattern that is remembered; it never replaces an existing file, and adds `_` to names Windows reserves (CON, NUL, COM1…).
- DONT IMPLEMENT **Split into tracks.** Split a long recording (vinyl, a concert) at silences or markers, with a preview of the cut points.
- DONT IMPLEMENT **Join files.** Put several files one after another into one file.
- DONT IMPLEMENT **Convert a folder.** Save every file in a folder in another format and quality.

### 7. Folder browser
- **Folder panel.** A side panel listing the folder's subfolders and audio files by name (natural order, hidden ones left out), with `..` to go up, to click through. No database. Done: View → Folder (Ctrl+B) on the right, with Track Info moved to the left and the player in the middle; clicking a folder only browses, clicking a file plays it (and its folder becomes the one Page Up/Down moves through).

### 8. Tidy up
- **Identify track.** Make a Chromaprint fingerprint of the file (pure Rust, rusty-chromaprint), look it up online in AcoustID, then fill in the missing tags from MusicBrainz. Show the match and its score, and on request write the tags and rename the file. Only the fingerprint and the length are sent, and only when you run it. Free for non-commercial use, with one application key; at most 3 lookups per second (AcoustID) and 1 per second (MusicBrainz). Works on whole files, not clips. Done (awaiting a check online): the AcoustID key goes in Settings → AcoustID Settings; Track → Identify Track… lists AcoustID's matches; Use Match adds the MusicBrainz genre and fills Track Info's tag editor, so Save… and Rename from Tags… do the writing. Track → Identify Album… does a whole folder: it suggests the release most files are on, then Apply to All Tracks, Fill In All Tracks or Apply and Rename writes every file.

### 9. Local AI (optional download)
The runtime and model manager, as in lightview: Local AI → Download AI Runtime… and AI Models… (CPU, and NVIDIA's CUDA as a second download). Done. The tools, each a milestone: Remove noise (DeepFilterNet3), Transcribe speech (Whisper small, Parakeet TDT 0.6B v3 or Whisper large-v3-turbo, chosen in AI Models…), and for Practice mode Split into stems (HTDemucs), Melody to MIDI (Basic Pitch) and Find beats (Beat This!, which brings back the Metronome).
- **Remove noise.** Clean background noise and hum from voice recordings. Done: Local AI → Remove Noise writes name-denoised.flac with DeepFilterNet3 and opens it.
- **Transcribe speech.** Turn a talk or voice memo into text you can copy. Done: Local AI → Transcribe Speech with Parakeet TDT 0.6B v3, Whisper small or Whisper large-v3-turbo (chosen in AI Models…), in a window with Copy and Save as Text….

### 10. MIDI files
- **Play MIDI.** Open and play .mid and .midi files like audio files (Open, the Folder panel, Page Up/Down, default app) with a built-in basic piano sound. No MIDI editing.
- **Save MIDI as audio.** Save As renders a MIDI file to WAV, FLAC, MP3 or Ogg Vorbis.

## v2: Practice mode
Everything here is behind View → Practice mode, which adds a Practice menu (Loop selection, Spectrogram) and turns on the tempo, key and chord analysis.

### Practice tools
- **A–B loop with slow-down.** Loop a hard passage at a slower speed without changing the pitch. Done with existing tools: drag on the Edit waveform to select, Practice → Loop selection (L), Track → Pitch-preserved speed change.
- **Spectrogram.** See frequencies over time, to spot hum, clicks or where notes are. This shows the spectrogram first in a separate panel where we can zoom in/out and right click and drag for panning. Done: Practice → Spectrogram, its own window with a log frequency axis, click to seek, with note lines and their settings on its toolbar.
- **BPM and key.** Estimate the tempo and musical key of the track. Done with the bpm-key crate: shown in Track Info, offered in its tag editor (with ½× / 2×), and Key ≈ … on the Spectrogram toolbar sets Scale shown.
- DONT IMPLEMENT **Markers with notes.** Drop named markers (chorus, solo) and jump between them; they can be saved next to the file. Only if there is an automatic way of doing this.
- **Metronome.** A click at a set tempo, alone or over the track, with the song's tempo by default and on the right spots, beware of swing. Done (over the track): Local AI → Find Beats (Beat This!) finds the beats, the volume popup's Metronome (or Practice → Metronome) clicks on them with its own volume; a click alone at a set tempo is not built. Before that: a first try (Ellis dynamic-programming beat tracker on bpm-key's onset envelope) placed the clicks too badly on the test album; it waits for our own beat tracker in v3; the attempt and its results are in [docs/BEAT_TRACKING.md](docs/BEAT_TRACKING.md).
- **Chord recognition** Check if it is even possible. A first try (chroma, major/minor/7th templates, Viterbi) finds the main chords but not the changes: see [docs/CHORDS.md](docs/CHORDS.md). Built anyway, to be improved in v3: Spectrogram → Chords shows them in a bar along the top.

### Local AI for practice (optional download)
In the Local AI menu with the others, whatever the mode.
- **Split into stems.** Vocals, drums, bass and the rest as separate files, for karaoke or practising along. Done: Local AI → Split into Stems (HTDemucs v4) writes the four stems into name-stems/ and opens the vocals.
- **Melody to MIDI.** Turn a sung or played melody into a MIDI file. Done: Local AI → Melody to MIDI (Basic Pitch) writes name-melody.mid next to the track.

## v3: Our own models and engines (proposed)
Proposed, not committed. Our own versions of the Practice tools whose first tries fall short, written by us or trained by us.
- **Better chord recognition.** Find the changes, not only the main chords, starting from the ideas in [docs/CHORDS.md](docs/CHORDS.md).
- **Our own beat tracker.** Beats placed well enough for the Metronome without the Beat This! download, swing included, and a click alone at a set tempo; the first try and the ideas are in [docs/BEAT_TRACKING.md](docs/BEAT_TRACKING.md).
- **Notes and melody to MIDI.** Our own model that turns a recording, chords included, into notes and a MIDI file, better than Basic Pitch.

### Our own engine: the Griffin–Lim modulator in Rust
- **Build on the audio-tools Griffin–Lim modulator** to make a better, improved, faster version in pure Rust. Ideally it has two settings: Fast, for real time (frame by frame with a small look-ahead, like RTISI-LA), and Quality, slower and offline (more iterations, started from phase-vocoder phases). It replaces WSOLA where it sounds better. The ideas to start from are in [docs/TIME_STRETCH.md](docs/TIME_STRETCH.md), section 3.

## v4: Music library (very tentative)
Not committed: only if the player and Practice mode are done and it still fits "small".
- **Manage a library.** Collect the music folders into one library to browse by artist, album and genre.
- **Playlists.** Open, make and save m3u playlists.
- **Music recommendations.** Suggest what to play next from your own files, worked out locally.
- **Clean up a music folder.** Identify Track (v1 §8) in one pass over a folder and all its subfolders: identify every file, show a list of proposed tags and new names to accept or skip, then apply. Files that aren't found are left alone. Ctrl+Z undoes the renames. About 1,000 files take 6 minutes at AcoustID's limit. Later: waits, like Identify Track, for an AcoustID key.
- **Group duplicate songs.** Find copies of the same recording in a folder by how they sound, even in different formats or bitrates, and even when one copy is trimmed or starts later (peak-based matching like Olaf or Panako, written by us; it compares only your own files, so it needs no database), show the groups, and move each into its own folder. Nothing is deleted, and Ctrl+Z puts everything back.
- **Use a local database (later, for power users).** Point Identify Track at your own AcoustID and MusicBrainz server built from their public data exports, so nothing goes online. Too big to ship with the app.

## Open questions
- Is simple microphone recording in scope (voice memos, tuner)?
- MIDI's piano sound: our own small synthesizer, or a SoundFont player crate with a bundled piano (a new dependency, to approve).
- Track identification: is rusty-chromaprint's output identical to the original Chromaprint, and what match score to accept automatically.
