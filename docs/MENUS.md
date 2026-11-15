# soundglass menus

The menu bar as built in `src/main.rs` (`ui`, the `MenuBar`), with the default keys from
`src/keys.rs`. File → Settings… → Change Keyboard Shortcuts… changes or removes them, and gives a
key to any item listed here except Open Recent and Loop selection. The bar is hidden in full screen.
A greyed-out item says why when you hover it. `▸` marks a submenu.

Each item: **name** (default key) — when it's enabled — what it does.

## File

- **Open…** (Ctrl+O) — always — the system file dialog.
- **Open Recent ▸** — once a file has been opened — the files opened before (hover shows the full
  path); files that no longer exist are dropped.
- **Close** (Ctrl+W) — always — closes the window (asks first about unsaved edits).
- **Show in File Manager** (Show in Explorer on Windows) — a file is open — opens the file manager
  with the file selected, or just its folder.
- **Copy Path** — a file is open — copies the file's full path as text.
- **Copy File** — a file is open — puts the file itself on the clipboard; it stays there while
  soundglass runs.
- **Save Copy** (Ctrl+S) — the track is loaded, with unsaved edits — writes `name-edited` next to
  the original, in its format, with no dialog (`-edited-2` and so on). Never replaces a file.
- **Save As…** (Ctrl+Shift+S) — the track is loaded — saves a copy as WAV, FLAC, MP3 or Ogg Vorbis,
  with the quality, sample rate and whether to apply the speed and pitch.
- **Rename…** (F2) — the track is loaded and no tags are being saved — asks for the new name, with
  the extension left unselected. Never replaces a file; playback and edits carry on.
- **Delete…** — a file is open — asks first, then moves the file to the Trash. Ctrl+Z restores it.
- **Rename from Tags…** — as Rename… — renames from a pattern such as
  `{track} - {artist} - {title}`, with a preview.
- **Settings…** (Ctrl+,) — always — see [`FEATURES.md`](FEATURES.md#settings).
- **Show Log** — always — opens the file manager at `soundglass.log`, the log kept on your computer
  to attach to a bug report. It is never sent anywhere ([`FEATURES.md`](FEATURES.md#privacy)).
- **Quit** (Ctrl+Q) — always — closes the window (asks first about unsaved edits).

## Track

The speed and pitch submenus have presets and Custom…; the one in use is marked.

- **Change speed and pitch ▸** — 0.5×, 0.75×, 1×, 1.25×, 1.5×, 2×, Custom….
- **Pitch-preserved speed change ▸** — the same values.
- **Time-preserved pitch change ▸** — −2 to +2 semitones, Custom… (up to ±12).
- **Reset** — the speed or pitch isn't normal — puts all three back to normal.
- **Identify Track…** — an AcoustID key is set in Settings and the track is loaded — looks the
  track up online (only when you click Look up) and fills the tag editor.
- **Identify Album…** — as Identify Track… — looks up every file in the folder, then Apply to All
  Tracks, Fill In All Tracks or Apply and Rename.
- **Fill Metadata From ▸** Wikipedia…, Discogs…, MusicBrainz… — the track is loaded — searches that
  source for the title and artist (from the tags, or else the file name) and fills the tag editor;
  after a MusicBrainz match, it offers to contribute the fingerprint to AcoustID.
- **Add/Replace Album Cover…** — a file is open, no tags are being saved and Track Info's tag editor
  is closed — picks an image and saves it as the cover of this track or of all the folder's tracks.
- **Remove Album Cover…** — as Add/Replace — removes the cover from this track or all of them.

## Edit

- **Undo** (Ctrl+Z) — there's something to undo — the current track's edits first, then the files
  deleted this session, newest first.
- **Redo** (Ctrl+Shift+Z or Ctrl+Y) — there's something to redo.
- **Cut out** (Delete) — a part is selected in the Edit panel — removes it.
- **Crop** (Ctrl+T) — a part is selected — keeps only the selection.
- **Silence** (Ctrl+L) — a part is selected — replaces it with silence.

## Effects

Enabled when a file is open. The first line says whether the effects work **on the selection** or
**on the whole track**. Every item, preset and Custom… can get a key.

- **Fade in ▸**, **Fade out ▸** — Whole selection, 1 s, 3 s, 5 s, Custom….
- **Gain ▸** — −6, −3, −1, +1, +3, +6 dB, Custom….
- **Normalize ▸** — Peak near max (−1 dBFS), Peak at max (0 dBFS), As streaming (−14 LUFS), As
  podcasts (−16 LUFS), As TV (−23 LUFS), Custom peak…, Custom loudness….
- **Trim silence ▸** — Silence (−60 dB), Faint noise (−50 dB), Hiss (−40 dB), Custom….
- **Reverse**.
- **Channels ▸** — always the whole track — Mono, Stereo, Swap left and right.

Custom… opens a dialog with the value; it's enabled once the track is loaded.

## Local AI

Each tool is enabled once the AI runtime and a model for it are installed, the track is loaded and
no other AI tool is running. With a selection in the Edit panel, the tools say "(selection)" and
work on it.

- **Remove Noise**
- **Transcribe Speech**
- **Split into Stems**
- **Melody to MIDI**
- **Find Beats**
- **AI Models…** — always — the models, GPU support, the output format and each model's settings.
- **Download AI Runtime (… MB)…** — shown until the runtime is installed — shows its licence, then
  downloads ONNX Runtime.

What each tool does: [`FEATURES.md`](FEATURES.md#ai-tools).

## View

- **Track Info** (Ctrl+I) — always — shows or hides the panel on the left.
- **Folder** (Ctrl+B) — always — shows or hides the Folder panel on the right.
- **Edit** (Ctrl+E) — always — shows or hides the Edit panel at the bottom.
- **Full Screen** (F11) — always — hides the menu bar; Esc leaves it once there's no selection.
- **Practice mode** — always — adds the Practice menu and the tempo, key and chord estimates.

## Practice (only in Practice mode)

- **Loop selection** (L, fixed) — always — loops the selection; otherwise playback stops at its end.
- **Metronome** — beats were found with Local AI → Find Beats — clicks on them while the track plays.
- **Spectrogram** — always — opens or closes the Spectrogram window.

## Keys without a menu item

These are fixed and can't be given to a menu item:

- **Space** — play or pause.
- **← / →** — back or forward 5 s.
- **Page Up / Page Down** — previous or next file in the folder (in the shuffled order with
  Shuffle).
- **Media keys** (play/pause, next, previous) — as Space and Page Down / Page Up, also while
  another window is focused.
- **L** — Loop selection (Practice mode).
- **Esc** — clear the selection; leaves full screen once there's no selection.
- **Enter** — accepts a dialog.
- **Ctrl+C, Ctrl+X, Ctrl+V** — copy, cut and paste in text fields.

## Mouse

- **Back / forward buttons** — previous or next file.
- **Drag a file or folder onto the window** — plays it (X11 and Windows, not Wayland yet).
- **Edit panel** — click to play from there, drag to select, Shift+click or Shift+drag to change the
  selection, scroll to zoom, right-drag (or scroll sideways) to move along.
- **Spectrogram** — click to play from there, drag to select, scroll to zoom, Ctrl+scroll to zoom
  the frequencies, right-drag to move around; Space, L, Esc, ←/→ and Page Up/Down work there too.
- **Folder panel** — click a folder to look inside it, a file to play it.
