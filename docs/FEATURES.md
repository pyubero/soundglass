# soundglass features

Everything soundglass can do, by area. [`MENUS.md`](MENUS.md) lists every menu item and its
keyboard shortcut.

soundglass is in development: this list grows with each milestone in
[`PRODUCT_BRIEF.md`](PRODUCT_BRIEF.md).

## Playing

- **Sound at once.** Playback starts before the whole file is decoded. `--timings` prints the time
  to the window and to the first sound; `--bench` exits at the first sound.
- **An empty window to start.** Without a file it opens with Open… (the menus that need a file are
  greyed out).
- Space plays and pauses; ← and → go back or forward 5 s.
- **Media keys.** The keyboard's play/pause, next and previous keys work even while another window
  is focused: on Linux soundglass is an MPRIS player on D-Bus (Plasma's and GNOME's media controls
  show it as "soundglass"), on Windows a System Media Transport Controls session. No title or cover
  is sent, and the desktop can't seek.
- **Seek bar** as a line or a waveform (File → Settings…).
- Drag a file or folder onto the window to play it (a folder plays its first audio file). This works
  on X11 and Windows; **it isn't supported on Wayland yet.**
- **Full screen** with F11, without the menu bar (Esc leaves it once there's no selection).
- **Cover and visualiser.** The cover shows at most 256 px; the small button in its space's
  bottom-right corner makes it fill the space instead (and back), remembered across runs. The button
  next to it (also shown without a cover) swaps the cover for a visualiser: colour waves that follow
  the music under the playhead (bass for brightness and speed, treble for ripple size, a flash and a
  colour jump on a sudden bass hit). It freezes while paused and is remembered across runs.
- **Live reload.** When another program changes the open file (re-tagging it, say), soundglass asks
  whether to reload it; Reload reads it again and carries on from the same place, dropping unsaved
  edits (it says so first). Its own tag and cover writes don't ask. The file is checked once a
  second.

## Browsing a folder

- Page Up / Page Down play the previous or next file in the folder, in natural order (`2` before
  `10`). The mouse's back and forward buttons do the same.
- **Play order.** The button right of Next cycles In order, Repeat track and Shuffle; with Shuffle,
  Page Up / Page Down follow the shuffled order.
- **Folder panel** (Ctrl+B, on the right) lists `..`, the subfolders and the audio files by name
  (🗀 folders, ♪ tracks): click a folder to look inside it (playback carries on), click a file to
  play it. It refreshes within a second when another program adds, renames or deletes a file there.
- **Track Info** (Ctrl+I, on the left) shows the tags, the audio details and the cover.
- Each panel widens the window instead of squeezing the player.
- Open Recent (File menu) lists the files opened before.

## Editing

Edits never change the original file: they are kept in soundglass until you save a copy. Every edit
can be undone and redone.

- **Edit panel** (Ctrl+E): click to play from there, drag to select, Shift+click or Shift+drag to
  change the selection, scroll to zoom, right-drag (or scroll sideways) to move along. Esc clears
  the selection.
- **Cut out** the selection (Delete), **crop** to it (Ctrl+T), or **replace it with silence**
  (Ctrl+L).
- **Undo** (Ctrl+Z) and **redo** (Ctrl+Shift+Z or Ctrl+Y). Past the edits, Undo brings back a file
  deleted this session.
- **Save Copy** (Ctrl+S) writes the edits next to the original, in its format, as `name-edited`
  (then `name-edited-2`…).
- **Save As** (Ctrl+Shift+S) saves a copy with a choice of format, quality and sample rate, and
  whether to apply the speed and pitch. The original file is never changed. Save As and Save Copy
  also take tag edits not yet saved.
- Closing the window (Ctrl+W or Ctrl+Q) asks first about unsaved edits.

## Effects

The Effects menu works on the selection, or on the whole track when nothing is selected. Each item
has presets and a Custom… value, and every effect can be undone:

- **Fade in, Fade out:** over the whole selection, or 1, 3 or 5 s from its start or end; the
  loudness changes evenly to the ear.
- **Gain:** up or down by a number of dB (±1, ±3, ±6).
- **Normalize:** the peak near or at the maximum (−1 or 0 dBFS), or the loudness as streaming
  (−14 LUFS), as podcasts (−16) or as TV (−23).
- **Trim silence:** removes what is quieter than a level at the start and end.
- **Reverse:** plays it backwards.
- **Channels:** mono, stereo or swap left and right (always the whole track).

## Speed and pitch

The Track menu's speed and pitch items change how it plays, live, and the settings combine:

- **Change speed and pitch:** faster is also higher, like tape (0.5× to 2×).
- **Pitch-preserved speed change:** slower or faster at the same pitch (0.5× to 2×).
- **Time-preserved pitch change:** up or down by semitones at the same speed (−2 to +2, Custom… up
  to ±12 in steps of 0.1).

Reset puts all three back to normal. The last two use WSOLA, which can sound a little echoey or
stuttery on dense mixes ([`TIME_STRETCH.md`](TIME_STRETCH.md)). Save As can apply them to the file.

## Tags, covers and names

- **Tag editor.** In Track Info, Edit (in Tags) changes the title, artist, album, year, track,
  genre, BPM, key and cover; the estimate buttons (Practice mode) fill BPM and Key, and ½× / 2×
  halve or double the BPM. Save… asks first, then writes the tags into the file; the audio is
  checked unchanged before the file is replaced.
- **Chinese, Japanese and Korean tags** are drawn with the system's CJK font (on Linux the one
  fontconfig picks, such as Noto Sans CJK from `fonts-noto-cjk`; on Windows Microsoft YaHei), in the
  Chinese style; without one they show as boxes.
- **A cover for the whole folder.** Opening a track without a cover in a folder with exactly one
  image (JPEG, PNG, TIFF, WebP, BMP or GIF) asks whether to use that image as the cover of all the
  folder's tracks (replacing their covers); Add Cover (the dialog shows the image) writes it into
  each file the same checked way. Each folder is asked once per run.
- **Track → Add/Replace Album Cover…** picks an image (the same formats; all but JPEG and PNG are
  saved as JPEG, at most 1000 px), shows it, and saves it as the cover of this track or of all the
  folder's tracks (Track Info's tag editor must be closed); **Remove Album Cover…** removes it from
  this track or all of them the same way.
- **Track → Identify Track…** (online, only when you click Look up) sends the file's Chromaprint
  fingerprint and length to AcoustID and lists the matches with their score; Use Match fetches the
  genre from MusicBrainz (sending only the recording's ID) and fills Track Info's tag editor, only
  the empty fields unless you untick that. Nothing is written until Save… there. It is greyed out
  until you paste a free AcoustID application key (from acoustid.org/new-application) into Settings
  → AcoustID Settings.
- **Track → Identify Album…** looks up every file in the playing track's folder (one AcoustID request
  a second, with a progress bar and Cancel), suggests the releases most of them are on, and previews
  each file's new track number and title. Apply to All Tracks writes the album's tags into every
  matched file, Fill In All Tracks only fills empty tags, and Apply and Rename also renames each file
  with the Rename from Tags pattern. The genre comes from MusicBrainz, once for the album. Files not
  on the chosen album are left alone; there is no undo.
- **Track → Fill Metadata From → Wikipedia… / Discogs… / MusicBrainz…** (online, only when you click
  Look up) searches by name instead of fingerprint, with no key needed. The Title and Artist boxes start
  with the track's tags (the tag editor's unsaved ones if it is open), or the file name without its
  leading track number when there is no title tag; you can change them. An Album box (the album tag,
  or the folder's name without a leading year) orders the list; only MusicBrainz receives it, as an
  optional part of the query that lifts recordings on that album without dropping the others. MusicBrainz and
  Discogs fetch up to 100 matches and list first the ones on that album, then the higher score, then
  originals before reissues, compilations and unofficial releases, then the earliest year; Wikipedia
  keeps its search order. The window then works like Identify Track. MusicBrainz lists recordings with their score and fetches the genre for the pick.
  Discogs lists releases (country and format), and Use Match reads the chosen release's track list
  for the track's title and number. English Wikipedia lists song articles and reads their infobox
  (artist, album, year, genre, track number).
- **Contribute to AcoustID?** After Use Match on a MusicBrainz match, soundglass asks whether to
  contribute the track's fingerprint, so that AcoustID recognises the song for everyone. Contribute
  looks the fingerprint up first and sends it, with its length, the MusicBrainz recording ID and
  your AcoustID user key, only when AcoustID has no match at all for it; a notice says which
  happened. Without the application key and user key in Settings, the prompt says so and offers
  Add AcoustID User Key, which opens Settings; the prompt comes back when Settings closes.
- **File → Rename…** (F2) renames the current file to a name you type (the extension starts
  unselected); it never replaces another file, and playback and edits carry on.
- **File → Rename from Tags…** renames the current file from a pattern such as
  `{track} - {artist} - {title}` (also `{album}`, `{year}`, `{genre}`), with a preview. A missing tag
  is left out, a name Windows reserves (`CON`, `NUL`, `COM1`…) gets a `_`, and an existing file is
  never replaced (changing only the letter case is fine).
- **File → Delete…** asks first, then moves the current file to the system Trash and shows the next
  file in the folder, stopped (no file if it was the last). Edit → Undo (Ctrl+Z), once the current
  track's edits are all undone, brings back the files deleted this session, newest first.
- **File → Show in File Manager** (Show in Explorer on Windows) opens the file manager with the
  current file selected, or just its folder if the file manager can't select files.
- **File → Copy Path** copies the file's full path as text; **Copy File** puts the file itself on
  the clipboard, for pasting into a chat or a file manager (it stays there while soundglass runs).

## Practice mode

View → Practice mode (off by default) adds a Practice menu with the tools for musicians, and the
tempo, key and chord estimates; with it off none of them is shown or worked out.

- **Tempo and key.** Track Info shows the estimated tempo and key (Audio, after loading; the first
  10 minutes are analysed). Tempo is counted as people tap along, so it can be an octave off.
- **Loop selection** (L): loops the selection on or off (otherwise playback stops at its end).
- **Spectrogram** opens in its own window (log frequency, 20 Hz up): click to play from there, drag
  to select (the same selection as the Edit panel, so Loop works on it), scroll to zoom,
  Ctrl+scroll to zoom the frequencies, right-drag to move around in both directions; Fit shows
  everything again. Space, L, Esc, ←/→ and Page Up/Down work there too. It is worked out once per
  file (and again after an edit or an Analysis change), so zooming and panning are instant.
- **Chords** (on the Spectrogram's toolbar) shows a bar along the top with the chords (major, minor
  and 7ths) against the track's time; they follow the edits and the zoom, and are named like the
  note lines. It's a first try: it finds the main chords better than when they change
  ([`CHORDS.md`](CHORDS.md)).
- **Key ≈ …** (once estimated) shows the detected key's scale (a minor key as its relative major).
- **Notes** shows lines at the notes' pitches (C thicker), named where they fit.
- **Spectrogram settings.** The tuning (A4), C D E or Do Ré Mi, sharps or flats, octave numbers,
  the scale shown (all notes or one of the 12 major scales, spelled as the key spells them), an
  instrument's range in concert pitch (trumpet E3–Bb5, tenor sax Ab2–Eb5), and the lines' look
  (colours for the tonic — C with all notes, the key's tonic with a scale — and the other lines,
  tonic width, label size). Its Analysis part sets the FFT size, window size, hop (256, 512 or 1024
  samples; raised for long files, sooner with big FFTs) — every FFT bin is kept and drawn, the
  window function, the level range, the lowest frequency and the colours, with Reset to defaults.
  All are remembered.
- **Metronome** (Practice → Metronome, or the volume button's popup) clicks on the beats found by
  Local AI → Find Beats ([AI tools](#ai-tools)).

## AI tools

Nothing AI is shipped. Local AI → Download AI Runtime… installs ONNX Runtime 1.31.1 (11 MB on Linux,
83 MB on Windows) into the data folder's `lib/`, and AI Models… downloads the models into `models/`.
Each download shows its licence first, is pinned by SHA-256 (a file that doesn't match is deleted)
and is only made when you click.

- **NVIDIA GPU.** With an NVIDIA driver, AI Models… also offers GPU support: ONNX Runtime's CUDA 12
  build and NVIDIA's libraries (about 1.5 GB, into `gpu/`); it takes effect after a restart. On a
  GTX 1050 it makes Split into Stems and Find Beats 2.6–3× faster; the other tools stay on the CPU,
  where they run faster.
- **Other graphics cards** (AMD, Intel; experimental). AI Models… offers ONNX Runtime's WebGPU plugin
  once the runtime is installed (6 MB on Linux, 13 MB on Windows, into `webgpu/`): it needs Vulkan
  on Linux (`libvulkan1`) or Direct3D 12 on Windows, only Find Beats uses it (about 2× faster on a
  GTX 1050; Split into Stems was 3× slower there than on the CPU, so it stays on the CPU), and a
  weak built-in chip can be slower than the CPU, so switch Use the GPU off if it doesn't help.
- With both installed, NVIDIA's is used; if the GPU can't be used, the CPU is, and AI Models… says
  why.

The tools work on the selection in the Edit panel, or on the whole track when nothing is selected
(the menu items then say "(selection)"); they read the track as edited and never change it, and
each writes a new file next to it and opens it. Audio is written as Ogg Vorbis q6 (about 6 MB for 4
minutes); AI Models → Output → FLAC (lossless) writes 24-bit FLAC instead (about 50 MB), for further
editing. With a selection, Remove Noise, Split into Stems and Melody to MIDI write just that
stretch, while subtitles and found beats keep the track's times (beats found in a selection replace
the earlier ones, so the Metronome clicks only there). A selection must be at least half a second.

Each installed model has a few settings in AI Models (Settings: …), kept per model, with Reset to
defaults: Remove Noise's strength (full, or at most 6–60 dB less noise), Split into Stems' overlap
and which stems to write, Melody to MIDI's onset and note thresholds, shortest note, note range,
held notes and the file's tempo, Find Beats' sensitivity, and for Transcribe Speech the level under
which quiet parts are skipped, plus Whisper's language (a code such as en or es, empty to detect
it), Translate to English, and Time marks (on by default), which gives the transcript as subtitles
(.srt) to save with Save as Subtitles…: Parakeet times them from its words (cues end at a sentence
end, a pause over 1 s, 7 s or 84 characters), Whisper small with its own segments; large-v3-turbo
gives no times of its own, so its cues are whole pieces of up to 29 s.

- **Remove Noise** (DeepFilterNet3, 16 MB): cleans background noise from speech and writes
  `name-denoised.ogg` at the track's sample rate. Made for voices; on music it can take away more
  than noise. On a 12-thread CPU it runs about 2.4× faster than real time for a stereo track.
- **Split into Stems** (HTDemucs v4, 166 MB): writes `drums`, `bass`, `other` and `vocals.ogg` into
  a new `name-stems` folder and opens the vocals; the Folder panel moves to that folder, and Page
  Down steps through the others. About real time on the CPU (a 4-minute song takes about 5
  minutes); much faster with GPU support. All four stems are held in memory, about 170 MB a minute
  of audio.
- **Melody to MIDI** (Spotify's Basic Pitch, 0.2 MB): turns what's played or sung, several notes at
  once, into `name-melody.mid` (120 bpm, Electric Piano), with Basic Pitch's default thresholds and
  no pitch bends. A minute of audio takes about a second.
- **Find Beats** (Beat This!, 191 MB): finds the beats and the first beat of each bar, shown as
  ticks along the bottom of the Spectrogram; it then turns on the Metronome (in the volume button's
  popup), which clicks on them while the track plays, every beat alike, with its own Clicks slider,
  through cuts, reversals and speed changes. Practice → Metronome is the same switch. When the file
  has no BPM tag, the tempo from the beats (the median gap between them) is written as one, with the
  same audio check as Write Tags. A song takes 10–30 s on the CPU. Speech and free-time passages get
  no beats ([`BEAT_TRACKING.md`](BEAT_TRACKING.md)).
- **Transcribe Speech** (three models; pick one in AI Models…): shows the track's words in a window,
  with Copy and Save as Text…. Long recordings are cut into pieces of at most 29 s at quiet moments;
  silent pieces are skipped. On a 12-thread CPU, for 164 s of a 1954 interview:
  - Parakeet TDT 0.6B v3 (671 MB): 25 European languages, punctuated, the most accurate and
    fastest, 14 s. CC BY 4.0: by NVIDIA.
  - Whisper small (375 MB): 99 languages, detected from the first piece with sound, 88 s.
  - Whisper large-v3-turbo (1,037 MB): 99 languages, about as slow as small on the CPU; it may leave
    out capitals and punctuation.

## Languages, shortcuts and accessibility

- **Twenty-eight languages:** English, Spanish, French, German, Italian, Portuguese, Polish, Dutch,
  Russian, Japanese, Chinese (Simplified and Traditional), Korean, Ukrainian, Turkish, Indonesian,
  Vietnamese, Czech, Swedish, Hungarian, Romanian, Greek, Danish, Finnish, Norwegian (Bokmål),
  Catalan (also used for Valencian), Basque and Galician, picked in File → Settings…. soundglass
  starts in your system's language. All but English are machine translations, partly reused from
  lightview: corrections are welcome ([`TRANSLATING.md`](TRANSLATING.md)).
- **Chinese, Japanese and Korean** are drawn with your system's CJK font (on Linux the one
  fontconfig picks, such as Noto Sans CJK from the `fonts-noto-cjk` package; on Windows the one
  built in). Without one, those texts show as boxes. The font is loaded once per start, so after
  switching between two of these languages, restart soundglass for the new one's character style.
- **Keyboard shortcuts.** File → Settings… → Change Keyboard Shortcuts…: every menu item, effect
  preset and speed preset can get a key, and the defaults can be changed or removed. Space, ←, →,
  Page Up/Down, L, Esc and Enter are fixed. Changes are kept in the settings.
- **Font size.** The interface's font size goes from 10 to 20 (the labels drawn on the Spectrogram
  and waveforms keep their own sizes).
- **High contrast theme**, editable like the others ([Themes](#themes)).

## Settings

File → Settings… (Ctrl+,) has:

- the language;
- the seek bar's look (line or waveform);
- the theme;
- the interface's font size (10 to 20);
- Remember window size, or else the size the window opens at (380 × 680 at first);
- Make soundglass the default player (Linux, for every type in `soundglass.desktop`);
- Change Keyboard Shortcuts…;
- AcoustID Settings (the application key for Identify Track and Identify Album, and your user key
  for contributing fingerprints);
- at the bottom, the version and Check for updates, which opens the Releases page (soundglass never
  checks on its own).

Settings are saved in `~/.local/share/soundglass/app.ron` (Windows: `%APPDATA%\soundglass\data\`).

A log for bug reports is kept next to it in `soundglass.log`, so a problem that can't be
reproduced still leaves a trace. It stays on your computer and is never sent anywhere
([Privacy](#privacy)). It's rotated to `soundglass.log.1` at 512 KB, so the two hold the most recent
0.5–1 MB. File → Show Log opens its folder. It records:

- the start: version, system, sound device (rate, channels) and graphics card;
- each file opened, described by its properties, never its name or folder: extension, container,
  codec, sample rate, channels, bit depth, length, size and number of tags; how long it took to
  decode and the memory in use after;
- each menu command and player action (play, pause, seek, edits…), and every notice shown;
- how long tempo, key and chords, saves, AI tools and online requests took (requests by host and
  path only, never their query);
- lag: any UI frame over 50 ms with the parts of the window that took the time; a frame that hasn't
  ended after 1 s, while it's stuck (at 1, 2, 4… s); frames missing for over half a second while
  playing in the focused window;
- sound dropouts, at most once a second: underruns, late or slow audio callbacks, and callbacks that
  had to play silence because the track wasn't ready;
- errors, panics and warnings from the libraries.

File and folder names never reach the log: where a message or error would show one, it reads
`<file>` or `<folder>`, and your home folder reads `~`.

## Themes

File → Settings… → Theme changes the colours and shapes of menus, panels, dialogs and buttons. The
seek bar, the waveforms and the play buttons use the theme's colours too. A theme can't move, add or
hide anything, and it doesn't change the fonts or the Spectrogram's colours (those are in its own
settings).

- **Theme files:** each theme is an `.xml` file in `~/.local/share/soundglass/themes` (Windows:
  `%APPDATA%\soundglass\data\themes`), which the Open themes folder button opens. The first time
  soundglass starts, it writes `dark.xml`, `light.xml` and `high-contrast.xml` there. Edit them, or
  copy one under a new name to make your own; a file's name is its name in the list. soundglass never
  overwrites or recreates these files, so edits and deletions stay. lightview's theme files work
  here too.
- **Choosing a theme again** reloads its file, so you can edit it and look at the result without
  restarting.
- **Follow system** (the default) uses `dark.xml` or `light.xml`, depending on the desktop's
  dark/light setting. On Linux it's read when soundglass starts, so after changing the desktop's
  setting, restart soundglass. When the desktop gives no preference, `dark.xml` is used.
- **The base theme:** built into soundglass and identical to the original Light theme. Any key a
  file leaves out takes the base theme's value. A file that's missing or can't be read (bad XML, a
  wrong value, bigger than 64 KiB) is replaced by the whole base theme, and Settings says what's
  wrong and on which line.

The three files set every key, so they work as templates. `mode` is `dark` or `light`: it decides
which half of Follow system the theme can stand in for, and the shades of things themes don't set,
such as shadows. Colours are `#rrggbb` or `#rrggbbaa`.

```xml
<theme mode="dark">  <!-- or "light" -->
  <colors panel="#1b1b1b" window="#1b1b1b" border="#3c3c3c" field="#0a0a0a" text="#8c8c8c"
          accent="#005c80" selected-text="#c0deff" hyperlink="#5aaaff" warn="#ff8f00" error="#ff0000"
          played="#5aaaff"/>
  <widget state="hovered" fill="#404040" text="#f5f5f5" border="#969696"/>
  <!-- … one widget element for each of the five states … -->
  <shape rounding="6" stroke="1" spacing="8,3" padding="4,1"/>
</theme>
```

- `colors`: `panel` and `window` are the background of the menu bar and panels, and of dialogs and
  menus; `border` is the border of dialogs and menus; `field` is the background of text fields;
  `text` is labels and other text; `accent` and `selected-text` are selected items and their text;
  `hyperlink`, `warn` and `error` are links, warnings and errors; `played` is the played part of the
  seek bar and the waveforms (left out, it's the file's own `accent`, not the base theme's, so
  lightview's themes still look as they did there).
- `widget`: `state` is `noninteractive` (separators, frames), `inactive`, `hovered`, `active`
  (pressed) or `open` (an open menu or list), one element per state; `fill`, `text` and `border` are
  that state's background, text and border.
- `shape`: `rounding` is the corner radius of buttons, menus and dialogs, 0–16; `stroke` is the
  border width, 0.5–4 (set a `border` colour for `inactive` too, or its border stays invisible);
  `spacing` and `padding` are the space between items and inside buttons, `x,y`, each 0–24.

Unknown elements and attributes are ignored.

## Privacy

soundglass sends nothing: no telemetry, no account. It goes online only when you ask:

- Identify Track… and Identify Album… (AcoustID and MusicBrainz), after you click Look up;
- Fill Metadata From (Wikipedia, Discogs or MusicBrainz), which sends only the title and artist
  you searched for (and the album, to MusicBrainz only) and the chosen match's ID, after you click
  Look up;
- Contribute to AcoustID, which looks the fingerprint up and, only if AcoustID doesn't know it,
  sends it with its length, the MusicBrainz recording ID and your user key, after you click Contribute;
- the AI runtime, GPU libraries and models, each after you accept its licence;
- Check for updates, which only opens the Releases page in your browser.

soundglass keeps a log on your computer, `soundglass.log` (File → Show Log), only to help you report
bugs. It is never sent anywhere: nothing in soundglass uploads or attaches it, and nobody sees it
unless you share it yourself. It records what you did and when, and describes the files you opened
by their properties (format, length, size…), never by their names or folders
([what it records](#settings)). It's plain text, so you can read it before you attach it to a bug
report, and delete it at any time; soundglass starts a new one.

## Supported formats

- **Plays:** MP3 (and MP2, MP1), FLAC, WAV, Ogg Vorbis, M4A/AAC (including `.m4b`), AIFF, CAF and
  Matroska audio (`.mka`). Opus is not supported yet (the decoder, symphonia, has none).
- **MIDI files** (`.mid`, `.midi`): played with a built-in piano, our own small synthesizer (every
  channel, drums included, plays piano); they behave like audio files and Save As renders them.
- **Saves (Save As):** WAV (16, 24 or 32-bit float), FLAC (16 or 24-bit), MP3 (constant 128–320
  kb/s), Ogg Vorbis (quality 0–10), at the track's sample rate or another one.
- **Covers:** JPEG, PNG, TIFF, WebP, BMP and GIF; all but JPEG and PNG are saved as JPEG.
