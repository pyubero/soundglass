# Changelog

What changed in each release of soundglass. [`docs/FEATURES.md`](docs/FEATURES.md) describes every
feature in detail. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- MIDI files (`.mid`, `.midi`) play with a built-in piano, and Save As renders them to audio.

### Fixed

- The Spectrogram window shows the soundglass icon (it showed a generic one on Wayland).

## [0.1.0] - 2026-11-15

The first release: a small, fast music player and quick editor for Linux and Windows.

### Playing and browsing

- Sound before the whole file is decoded; Page Up / Page Down move through the folder in natural
  order, with Repeat track and Shuffle.
- Folder panel (Ctrl+B) to browse subfolders, and Track Info (Ctrl+I) with tags, audio details and
  the cover.
- A cover view that can fill its space, and a visualiser that follows the music.
- Reloads the open file when another program changes it (it asks first).
- Plays MP3, FLAC, WAV, Ogg Vorbis, M4A/AAC, AIFF, CAF and Matroska audio.

### Editing, never on the original

- Edit panel (Ctrl+E) with a zoomable waveform: cut out, crop, replace with silence, undo and redo.
- Effects on the selection or the whole track: fades, gain, normalize (peak or loudness: streaming,
  podcast, TV), trim silence, reverse, mono, stereo and channel swap.
- Save Copy (Ctrl+S) and Save As (WAV, FLAC, MP3, Ogg Vorbis, any sample rate): every save goes to a
  new file, which is checked before it's kept.
- Speed and pitch: tape-style speed, speed without a pitch change, and pitch without a speed change,
  live while playing, and optionally applied when saving.

### Tags, covers and names

- Tag editor in Track Info (title, artist, album, year, track, genre, BPM, key, cover). Tags written
  in place only after a check that the audio is unchanged.
- Add, replace or remove the album cover on one track or the whole folder.
- Rename from Tags with an editable pattern; Rename, Delete (to the trash, undoable), Copy Path and
  Copy File.
- Identify Track and Identify Album by audio fingerprint (AcoustID, with a free key) and MusicBrainz.
- Fill Metadata From Wikipedia, Discogs or MusicBrainz, searched by title and artist. The matches on
  the track's album, and the original releases, are listed first.

### Practice mode (View → Practice mode)

- Loop the selection, estimated tempo and key, and a Spectrogram window with note lines, scales,
  instrument ranges and a chord bar.
- A metronome that clicks on the beats found by the Find Beats AI tool.

### Local AI tools (optional downloads)

- Remove Noise (DeepFilterNet3), Transcribe Speech (Parakeet TDT 0.6B v3, Whisper small or Whisper
  large-v3-turbo), Split into Stems (HTDemucs), Melody to MIDI (Basic Pitch) and Find Beats (Beat
  This!).
- They run on the computer, on the CPU or an NVIDIA GPU. The runtime and models are downloaded only
  when you ask, each one after you accept its licence, and checked by SHA-256.

### Look and feel

- Light, dark and high-contrast themes (or following the system), plus your own XML themes;
  changeable keyboard shortcuts; menus in English, ready for translation.

### Privacy

- No telemetry and no account. soundglass goes online only when you ask: to identify a track or
  fill its metadata, or to download the AI tools.

### Packages

- A `.deb` for Ubuntu 22.04, Debian 12 or newer (64-bit), and a `.msi` installer for Windows 10
  and 11 (64-bit).

[Unreleased]: https://github.com/pyubero/soundglass/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/pyubero/soundglass/releases/tag/v0.1.0
