// No console window behind the app on Windows (debug builds keep it, for --timings output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai;
mod chords;
mod edit;
mod folder;
mod i18n;
mod identify;
mod keys;
mod loudness;
mod player;
mod save;
mod settings;
mod theme;
mod spectrum;
mod track;
mod tags;
mod visual;
mod tempo;
mod wsola;

use eframe::egui;
use player::{Cmd, Player};
use std::sync::atomic::Ordering::Relaxed;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use edit::Edit;
use track::Track;
use settings::Order;
use keys::Command;
use i18n::{tr, tr_key, trf};
use ai::Task;

/// Milliseconds between redraws while playing with a waveform shown.
const WAVE_REPAINT_MS: u64 = 33;

/// How far Left and Right jump.
const STEP_SECONDS: u64 = 5;
/// Previous restarts the track after this much has played, as Spotify does.
const RESTART_SECONDS: u64 = 3;

fn main() -> eframe::Result {
    let t0 = Instant::now();
    let (mut timings, mut bench, mut path) = (false, false, None);
    // `args()` panics on a file name that isn't valid Unicode.
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--timings") => timings = true,
            Some("--bench") => (timings, bench) = (true, true),
            _ => path = Some(std::path::PathBuf::from(arg)),
        }
    }
    // Started from the app menu, with no file: an empty window with Open….
    if bench && path.is_none() {
        eprintln!("usage: soundglass FILE [--timings | --bench]");
        std::process::exit(2);
    }
    let path = path.map(|path| std::fs::canonicalize(&path).unwrap_or_else(|e| {
        eprintln!("soundglass: {}: {e}", path.display());
        std::process::exit(1);
    }));

    // Decoding and opening the sound device overlap the window's OpenGL setup.
    let ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let track = path.as_ref().map_or_else(no_track, |path| Track::load(path.clone(), repaint(&ctx)));
    let files = path.as_ref().map(|path| folder::list_folder(path)).unwrap_or_default();
    let index = files.iter().position(|p| Some(p) == path.as_ref()).unwrap_or(0);
    let opening = std::thread::spawn(move || Player::open(t0, timings, bench));

    // winit reports the system theme on Windows only, so egui's fallback comes from the desktop portal,
    // read while the window opens. shortcut: read once per start; watch the portal's SettingChanged to follow live.
    #[cfg(target_os = "linux")]
    {
        let ctx = ctx.clone();
        std::thread::spawn(move || if let Some(theme) = theme::system_theme() {
            ctx.wait().options_mut(|o| o.fallback_theme = theme);
            ctx.wait().request_repaint();
        });
    }

    let name = track.path.file_name().map_or_else(|| "soundglass".into(), |n| n.to_string_lossy().into_owned());
    let mut settings = settings::load();
    i18n::set(*settings.language.get_or_insert_with(i18n::system_language));
    ai::set_use_gpu(settings.ai.use_gpu);
    let remembered = settings.window_size.filter(|_| settings.remember_window_size);
    let viewport = egui::ViewportBuilder::default().with_title(&name).with_inner_size(remembered.unwrap_or(settings.default_size)).with_min_inner_size(settings::MIN_SIZE)
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../icon/soundglass-256.png")).unwrap());
    // soundglass keeps the window size itself (see `App::save`): eframe's own saving can't follow Track Info.
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    let (show_info, show_edit) = (remembered.is_some() && settings.info_panel, remembered.is_some() && settings.edit_panel);
    let show_folder = remembered.is_some() && settings.folder_panel;
    let (window_size, info_width, edit_height, folder_width) = (remembered, settings.info_width, settings.edit_height, settings.folder_width);
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        let mut recent: Vec<PathBuf> = cc.storage.and_then(|s| s.get_string("recent")).unwrap_or_default().lines().map(PathBuf::from).collect();
        if !track.path.as_os_str().is_empty() {
            push_recent(&mut recent, &track.path);
        }
        cc.egui_ctx.set_fonts(fonts(None));
        let egui_ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || if let Some(cjk) = cjk_font() {
            egui_ctx.set_fonts(fonts(Some(Arc::new(cjk))));
        });
        theme::seed_folder();
        let theme_error = theme::apply(&cc.egui_ctx, &settings.theme);
        settings::apply_font_size(&cc.egui_ctx, settings.font_size);
        let player = opening.join().unwrap_or_else(|_| Err(tr("the sound device failed to open").into()));
        let volume = settings.volume;
        let (watched, changes) = folder::watch(repaint(&ctx), Duration::from_secs(1));
        // Asked here and not by the package: a package runs as root, defaults are per user (lightview's way).
        let default_check = (!settings.default_asked && folder::desktop_installed()).then(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            let notify = repaint(&ctx);
            std::thread::spawn(move || {
                let _ = tx.send(folder::is_default());
                notify();
            });
            rx
        });
        if let Ok(p) = &player {
            p.send(Cmd::Load(track.clone()));
            p.send(Cmd::Volume(volume_gain(volume)));
        }
        Ok(Box::new(App { track, next: None, files, index, shuffle: (random_seed(), index), advanced: 0, ctx, cover: None, visual: visual::Visual::new(), settings, volume, speed: wsola::Speed::NORMAL, titled: None, recent, custom_speed: None, custom_fx: None, show_settings: false, show_info, show_edit, edit_height, edit_view: EditView::default(), show_spectrum: false, show_spectrum_settings: false, spectrum_view: EditView::default(), spectrum: spectrum::Spectrogram::new(), history: None, looping: false, sent_region: None, metronome: false, click_volume: 0.7, sent_clicks: None, beats_seen: 0, strip_cache: WaveCache::default(), panel_cache: WaveCache::default(), pending: None, show_save: false, save_job: None, save_notice: None, save_choice: save::Choice::default(), tag_editor: None, tag_job: None, cover_checked: 0, cover_found: None, cover_offer: None, cover_asked: Default::default(), rename: None, rename_file: None, delete_prompt: false, deleted: Vec::new(), watched, changes, own_stamp: None, changed_prompt: false, default_check, default_prompt: None, default_result: None, show_shortcuts: false, recording: None, shortcut_error: None, identify: None, controls_height: 260.0, info_width, show_folder, folder_width, browse: None, window_size, theme_error, ai: ai::Ai::default(), file_clipboard: None, player, t0, timings, shown: false }))
    }))
}

/// The UI's fonts, from lightview: Apache-2.0 and MIT only, since egui's default set isn't
/// clearly GPL-compatible. The system's CJK font, once read, goes in before the icons.
fn fonts(cjk: Option<Arc<egui::FontData>>) -> egui::FontDefinitions {
    use egui::{FontData, FontFamily, FontTweak};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("Roboto".into(), Arc::new(FontData::from_static(include_bytes!("../fonts/Roboto-Regular.ttf"))));
    fonts.font_data.insert("Droid Sans Mono".into(), Arc::new(FontData::from_static(include_bytes!("../fonts/DroidSansMono.ttf"))));
    let icons = FontData::from_static(include_bytes!("../fonts/emoji-icon-font.ttf")).tweak(FontTweak { scale: 0.90, ..Default::default() });
    fonts.font_data.insert("emoji-icon-font".into(), Arc::new(icons));
    let family = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, family(&["Roboto", "emoji-icon-font"]));
    fonts.families.insert(FontFamily::Monospace, family(&["Droid Sans Mono", "Roboto", "emoji-icon-font"]));
    if let Some(cjk) = cjk {
        fonts.font_data.insert("cjk".into(), cjk);
        fonts.families.values_mut().for_each(|f| f.insert(f.len() - 1, "cjk".into()));
    }
    fonts
}

/// The system's font for Chinese, Japanese and Korean tags, in the Chinese style. Reads ~20 MB:
/// call it off the UI thread. shortcut: one regional style for all three; pick by the tag's script if users notice.
fn cjk_font() -> Option<egui::FontData> {
    #[cfg(target_os = "linux")]
    let (path, index) = {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}\n%{lang}", "sans-serif:lang=zh-cn"]).output().ok()?;
        parse_fc_match(&String::from_utf8_lossy(&out.stdout))?
    };
    // Every Windows 10/11 has it, whatever its language (lightview's choice).
    #[cfg(windows)]
    let (path, index) = (PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts").join("msyh.ttc"), 0);
    #[cfg(not(any(target_os = "linux", windows)))]
    let (path, index): (PathBuf, u32) = return None;
    let mut font = egui::FontData::from_owned(std::fs::read(path).ok()?);
    font.index = index;
    Some(font)
}

/// fc-match's file, face index and languages: the font only if it covers Chinese, since
/// fontconfig answers with some font even when none does.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_fc_match(out: &str) -> Option<(PathBuf, u32)> {
    let mut lines = out.lines();
    let (file, index, langs) = (lines.next()?, lines.next()?.parse().ok()?, lines.next()?);
    langs.split('|').any(|l| l == "zh-cn").then(|| (PathBuf::from(file), index))
}

/// What the window shows with no file open: nothing to play, with no path.
fn no_track() -> Arc<Track> {
    Track::from_samples(48000, 2, Vec::new())
}

/// Wakes the window, once it exists, from another thread.
fn repaint(ctx: &Arc<OnceLock<egui::Context>>) -> impl Fn() + Send + 'static {
    let ctx = ctx.clone();
    move || {
        if let Some(c) = ctx.get() {
            c.request_repaint();
        }
    }
}

struct App {
    track: Arc<Track>,
    /// The next file in the folder, decoded ahead and queued for gapless playback.
    next: Option<Arc<Track>>,
    files: Vec<PathBuf>,
    index: usize,
    /// The Shuffle order's seed and the file it starts from; see `shuffled`.
    shuffle: (u64, usize),
    /// The player's `advanced` count last seen, to notice when it moved into `next` by itself.
    advanced: u64,
    ctx: Arc<OnceLock<egui::Context>>,
    /// The cover texture and the track (by address) it belongs to.
    cover: Option<(usize, egui::TextureHandle)>,
    /// The visualiser shown instead of the cover when `settings.visualiser`.
    visual: visual::Visual,
    settings: settings::Settings,
    /// The volume slider's position, 0 to 1; see `volume_gain`.
    volume: f32,
    speed: wsola::Speed,
    /// The track (by address) the title bar shows, whether its length was exact then, and whether it had unsaved edits.
    titled: Option<(usize, bool, bool)>,
    recent: Vec<PathBuf>,
    /// The Custom speed or pitch dialog's setting and value while it is open.
    custom_speed: Option<(Knob, f64)>,
    /// The Custom… effect dialog's effect and value.
    custom_fx: Option<(Fx, f64)>,
    show_settings: bool,
    /// Why the chosen theme didn't load, shown in Settings.
    theme_error: Option<String>,
    show_info: bool,
    show_edit: bool,
    /// The Edit panel's height as last shown.
    edit_height: f32,
    edit_view: EditView,
    /// The Spectrogram window: whether it's open, its own zoom, and its picture.
    show_spectrum: bool,
    show_spectrum_settings: bool,
    spectrum_view: EditView,
    spectrum: spectrum::Spectrogram,
    /// Edits since the track was opened; `None` until the first one.
    history: Option<edit::History>,
    looping: bool,
    /// The waveform columns of the strip and of the Edit panel, kept until their view changes.
    strip_cache: WaveCache,
    panel_cache: WaveCache,
    /// The selection and Loop last sent to the player, to send them again only when they change.
    sent_region: Option<(Option<(u64, u64)>, bool)>,
    /// The Metronome (volume popup, or Practice → Metronome): clicks at the track's found beats.
    metronome: bool,
    /// Its slider position, 0 to 1; like the volume, not saved.
    click_volume: f32,
    /// What the player was last sent for the Metronome: track, edit and beats, or None for off.
    sent_clicks: Option<Option<(usize, Option<usize>, usize)>>,
    /// The last found beats seen (their Arc's address): new ones turn the Metronome on.
    beats_seen: usize,
    /// Waits for Save As / Discard / Cancel while there are unsaved edits.
    pending: Option<Pending>,
    show_save: bool,
    /// The running save: what it reports on, the edit it writes, the file's name, and whether
    /// the name was claimed by Save Copy (removed again if the save fails).
    save_job: Option<(SaveResult, Arc<Edit>, PathBuf, bool)>,
    /// The last save's result and when it came, shown for a few seconds (errors until the next save).
    save_notice: Option<(Result<String, String>, f64)>,
    /// Holds File → Copy File's clipboard: on Linux its contents go when it's dropped.
    file_clipboard: Option<arboard::Clipboard>,
    /// Save As's last choice, kept while soundglass runs.
    save_choice: save::Choice,
    /// Track Info's tag editor while it is open.
    tag_editor: Option<TagEditor>,
    /// The running write of the tags (or a folder's cover): the notice to show, or why it failed.
    tag_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// The track (by address) last checked for a folder image to offer as its cover.
    cover_checked: usize,
    /// The folder image found for that track, from the check's thread.
    cover_found: Option<std::sync::mpsc::Receiver<FoundCover>>,
    /// The cover being offered (or removed) while asking: from the Edit menu or the folder's lone image.
    cover_offer: Option<CoverOffer>,
    /// Folders already asked about this run, so Next doesn't ask again.
    cover_asked: std::collections::HashSet<PathBuf>,
    /// File → Rename from Tags… while open: the pattern being edited, and the file's tags.
    rename: Option<(String, Result<tags::TagEdit, String>)>,
    /// File → Rename… while open: the name being typed, whether the window just opened, and the last error.
    rename_file: Option<(String, bool, Option<String>)>,
    /// File → Delete…'s question is showing.
    delete_prompt: bool,
    /// Files moved to the Trash this session, newest last; Ctrl+Z brings them back once the edits are undone.
    deleted: Vec<PathBuf>,
    /// The file the watcher checks every second, its changes, the version soundglass itself last
    /// wrote (not asked about), and the "changed on disk" question while it shows.
    watched: folder::Watched,
    changes: std::sync::mpsc::Receiver<(PathBuf, folder::Stamp)>,
    own_stamp: Option<folder::Stamp>,
    changed_prompt: bool,
    /// First start: whether soundglass is the default player, being found out on a thread; then the
    /// welcome question's tick while it shows; and the last Make default's outcome, for Settings.
    default_check: Option<std::sync::mpsc::Receiver<bool>>,
    default_prompt: Option<bool>,
    default_result: Option<Result<(), String>>,
    /// Settings → Change Keyboard Shortcuts… is open; the command waiting for its new key; why the last key was refused.
    show_shortcuts: bool,
    recording: Option<Command>,
    shortcut_error: Option<String>,
    identify: Option<Identify>,
    /// Local AI: the AI Models window, licence prompts and downloads.
    ai: ai::Ai,
    /// Height of everything below the cover, as measured last frame.
    controls_height: f32,
    /// Track Info's width as last shown.
    info_width: f32,
    /// The Folder panel, its width, and what it lists (read when its folder changes, not each frame).
    show_folder: bool,
    folder_width: f32,
    browse: Option<Browse>,
    /// The window's size in points, last seen while not maximized or fullscreen.
    window_size: Option<[f32; 2]>,
    player: Result<Player, String>,
    t0: Instant,
    timings: bool,
    /// Whether the window is on screen; playback starts then.
    shown: bool,
}

/// File → Identify Track… while open.
/// The Folder panel's folder and its subfolders and audio files.
struct Browse {
    dir: PathBuf,
    listing: Result<(Vec<PathBuf>, Vec<PathBuf>), String>,
    /// The track last scrolled into view, so it scrolls once per change.
    scrolled: Option<PathBuf>,
}

impl Browse {
    fn new(dir: PathBuf) -> Browse {
        let listing = folder::list_dir(&dir).map_err(|e| trf("Can't read this folder: {e}", &[("e", &e)]));
        Browse { dir, listing, scrolled: None }
    }
}

struct Identify {
    /// The file it's for: it closes when another one plays.
    path: PathBuf,
    /// The matches, or why there are none; `None` before Look up.
    found: Option<Result<Vec<identify::Match>, String>>,
    pick: usize,
    only_empty: bool,
    /// The running lookup, or the genre fetch for the picked match.
    job: Option<std::sync::mpsc::Receiver<IdentifyJob>>,
}

enum IdentifyJob {
    Found(Result<Vec<identify::Match>, String>),
    Genre(Result<Option<String>, String>),
}

/// An image offered as the cover (or `Cover::Remove`), with its 256 px preview and the folder's tracks.
struct CoverOffer {
    /// The image file; empty when removing.
    image: PathBuf,
    cover: tags::Cover,
    files: Vec<PathBuf>,
    texture: Option<egui::TextureHandle>,
    /// Picked from the Edit menu, so "This Track" is a choice too.
    manual: bool,
}

/// Track Info's tag fields: the tags as the file has them, and as edited.
struct TagEditor {
    read: tags::TagEdit,
    draft: tags::TagEdit,
    /// Waiting for "Write the tags into …?" to be answered.
    confirm: bool,
    error: Option<String>,
}

impl TagEditor {
    /// The edits, when there are any.
    fn changes(&self) -> Option<&tags::TagEdit> {
        (self.draft != self.read).then_some(&self.draft)
    }
}

/// What the widgets asked for this frame, applied once drawing is done.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Previous,
    Next,
    Toggle,
    Order(Order),
    Seek(u64),
    /// Slider position, 0 to 1.
    Volume(f32),
    Metronome(bool),
    /// The Metronome's slider position, 0 to 1.
    ClickVolume(f32),
    Speed(wsola::Speed),
    Undo,
    Redo,
    /// Removes the selection.
    Cut,
    /// Keeps only the selection.
    Crop,
    Silence,
    Fx(Fx),
}

/// A quick fix from the Effects menu, on the selection or, without one, the whole track.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fx {
    /// Over this many seconds from the start, or the whole selection with `None`.
    FadeIn(Option<f64>),
    /// Over this many seconds before the end, or the whole selection with `None`.
    FadeOut(Option<f64>),
    /// By this many dB.
    Gain(f64),
    /// The peak to this many dBFS.
    Peak(f64),
    /// The integrated loudness to this many LUFS.
    Loudness(f64),
    /// Removes what is quieter than this many dB at the start and end.
    Trim(f64),
    Reverse,
    Mono,
    Stereo,
    Swap,
}

impl Fx {
    /// The Custom… dialog's title, unit, range, first value and a hint ("" for none).
    fn custom(self) -> (&'static str, &'static str, std::ops::RangeInclusive<f64>, f64, &'static str) {
        match self {
            Fx::FadeIn(_) => (tr("Fade in"), " s", 0.1..=600.0, 3.0, ""),
            Fx::FadeOut(_) => (tr("Fade out"), " s", 0.1..=600.0, 3.0, ""),
            Fx::Gain(_) => (tr("Gain"), " dB", -40.0..=40.0, 2.0, ""),
            Fx::Peak(_) => (tr("Normalize peak"), " dBFS", -40.0..=0.0, -1.0, tr("0 = max")),
            Fx::Loudness(_) => (tr("Normalize loudness"), " LUFS", -40.0..=-5.0, -14.0, tr("−14 streaming, −23 TV")),
            _ => (tr("Trim silence below"), " dB", -90.0..=-10.0, -60.0, tr("−60 silence, −40 hiss")),
        }
    }

    /// The same effect by `v`.
    fn with(self, v: f64) -> Fx {
        match self {
            Fx::FadeIn(_) => Fx::FadeIn(Some(v)),
            Fx::FadeOut(_) => Fx::FadeOut(Some(v)),
            Fx::Gain(_) => Fx::Gain(v),
            Fx::Peak(_) => Fx::Peak(v),
            Fx::Loudness(_) => Fx::Loudness(v),
            _ => Fx::Trim(v),
        }
    }
}

/// The presets the Effects menu offers.
const FADES: [f64; 3] = [1.0, 3.0, 5.0];
const GAINS: [f64; 6] = [-6.0, -3.0, -1.0, 1.0, 3.0, 6.0];
const PEAKS: [(f64, &str); 2] = [(-1.0, tr_key("Peak near max")), (0.0, tr_key("Peak at max"))];
const LOUDNESS: [(f64, &str); 3] = [(-14.0, tr_key("As streaming")), (-16.0, tr_key("As podcasts")), (-23.0, tr_key("As TV"))];
const TRIMS: [(f64, &str); 3] = [(-60.0, tr_key("Silence")), (-50.0, tr_key("Faint noise")), (-40.0, tr_key("Hiss"))];

/// `x` with a real minus sign.
fn signed(x: f64) -> String {
    format!("{x}").replace('-', "−")
}

/// Where to go once unsaved edits are saved or discarded.
enum Pending {
    Go(usize),
    Open(PathBuf),
    Quit,
}

/// Ok(whether the tags were copied), or why the save failed.
type SaveResult = std::sync::mpsc::Receiver<Result<bool, String>>;

/// The speeds Playback's two speed submenus offer directly.
const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Custom speeds move in this step.
const SPEED_STEP: f64 = 0.05;
/// The pitch changes, in semitones, Playback → Time-preserved pitch change offers directly.
const PITCHES: [f64; 5] = [-2.0, -1.0, 0.0, 1.0, 2.0];
/// Custom pitch changes move in this step, up to an octave either way.
const PITCH_STEP: f64 = 0.1;

/// `x` rounded to the nearest 0.05 and kept within 0.5×–2×.
fn snap_speed(x: f64) -> f64 {
    ((x / SPEED_STEP).round() * SPEED_STEP).clamp(0.5, 2.0)
}

/// One of Playback's three speed and pitch settings.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Knob {
    Tape,
    Tempo,
    Pitch,
}

impl Knob {
    const ALL: [Knob; 3] = [Knob::Tape, Knob::Tempo, Knob::Pitch];

    fn title(self) -> &'static str {
        match self {
            Knob::Tape => tr("Change speed and pitch"),
            Knob::Tempo => tr("Pitch-preserved speed change"),
            Knob::Pitch => tr("Time-preserved pitch change"),
        }
    }

    fn presets(self) -> &'static [f64] {
        if self == Knob::Pitch { &PITCHES } else { &SPEEDS }
    }

    fn get(self, s: wsola::Speed) -> f64 {
        match self {
            Knob::Tape => s.tape,
            Knob::Tempo => s.tempo,
            Knob::Pitch => s.semitones,
        }
    }

    fn set(self, s: wsola::Speed, v: f64) -> wsola::Speed {
        match self {
            Knob::Tape => wsola::Speed { tape: v, ..s },
            Knob::Tempo => wsola::Speed { tempo: v, ..s },
            Knob::Pitch => wsola::Speed { semitones: v, ..s },
        }
    }

    fn label(self, v: f64) -> String {
        match self {
            Knob::Pitch if v == 0.0 => tr("0 (Normal)").into(),
            Knob::Pitch => {
                let (sign, n) = (if v > 0.0 { "+" } else { "−" }, v.abs());
                if n == 1.0 { trf("{sign}{n} semitone", &[("sign", &sign), ("n", &n)]) } else { trf("{sign}{n} semitones", &[("sign", &sign), ("n", &n)]) }
            }
            _ if v == 1.0 => tr("1× (Normal)").into(),
            _ => format!("{v}×"),
        }
    }

    fn snap(self, v: f64) -> f64 {
        if self == Knob::Pitch { ((v / PITCH_STEP).round() * PITCH_STEP).clamp(-12.0, 12.0) } else { snap_speed(v) }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let recent: Vec<String> = self.recent.iter().map(|p| p.display().to_string()).collect();
        storage.set_string("recent", recent.join("\n"));
        // eframe restores a "window" entry even with persist_window off, and an old one would win
        // over ours; an empty one no longer parses, so eframe leaves the size to us.
        storage.set_string("window", String::new());
        if self.settings.remember_window_size {
            (self.settings.window_size, self.settings.info_panel, self.settings.info_width) = (self.window_size, self.show_info, self.info_width);
            (self.settings.edit_panel, self.settings.edit_height) = (self.show_edit, self.edit_height);
            (self.settings.folder_panel, self.settings.folder_width) = (self.show_folder, self.folder_width);
        }
        self.settings.volume = self.volume;
        eframe::set_value(storage, settings::KEY, &self.settings);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // eframe keeps the window hidden until the first frame is painted, so the window is
        // visible from the second frame on. shortcut: the compositor maps it a few ms after that;
        // winit has no "now on screen" event, so this is the closest signal there is.
        if ctx.cumulative_frame_nr() == 0 {
            ctx.request_repaint();
        } else if !self.shown {
            self.shown = true;
            if self.timings {
                eprintln!("window      {:.1} ms", ms(self.t0));
            }
            if let Ok(p) = &self.player {
                p.send(Cmd::Play);
            }
        }
        self.follow_player();
        self.follow_changes();
        if self.settings.practice {
            self.track.analyse(repaint(&self.ctx));
        }
        // Re-set when the track changes and again once its exact length is known.
        if self.titled != Some((Arc::as_ptr(&self.track) as usize, self.track.done(), self.dirty())) {
            self.set_title(&ctx);
        }

        let player = self.player.as_ref().ok();
        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        let edit = self.edit();
        let playing = player.is_some_and(|p| p.shared.playing.load(Relaxed));
        let len = edit.len();
        // Moved on by the time since the audio callback last said, so the playhead glides.
        let pos = player.map_or(0, |p| {
            let (pos, ago) = p.position();
            if playing { display_pos(pos, ago, rate, self.speed.source(), self.edit_view.selection, self.looping, len) } else { pos }
        });
        if playing && (self.show_edit || self.show_spectrum || self.settings.show_waveform || self.settings.visualiser) {
            // About 30 fps: smooth enough for the playhead, cheaper than redrawing at the screen's rate (user's choice).
            ctx.request_repaint_after(Duration::from_millis(WAVE_REPAINT_MS));
        } else if playing {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        let mut actions = Vec::new();
        let keys = keys::shortcuts(&self.settings.shortcuts);
        let key_text = |c: Command| keys.iter().find(|(_, k)| *k == c).map(|(sc, _)| ctx.format_shortcut(sc)).unwrap_or_default();
        // While Settings records a shortcut, the key goes there and nowhere else.
        let mut commands = if self.recording.is_some() {
            self.record_shortcut(&ctx);
            Vec::new()
        } else {
            self.read_shortcuts(&ctx, &keys)
        };
        self.player_keys(&ctx, pos, len, rate, &mut actions);
        let loaded = self.has_track() && self.track.done();
        let mut save_copy = false;
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }
        let (mut toggle_edit, mut open_file, mut toggle_info, mut toggle_folder) = (false, false, false, false);
        let (mut pick_cover, mut remove_cover) = (false, false);
        let mut open_recent = None;
        let viewport = ctx.input(|i| i.viewport().clone());
        let resizable = !viewport.maximized.unwrap_or(false) && !viewport.fullscreen.unwrap_or(false);
        // Not `viewport.inner_rect`: it needs the window's position, which Wayland never gives,
        // so it is always None there. The canvas size is the inner size on every platform.
        if resizable {
            let size = ctx.viewport_rect().size();
            self.window_size = Some([size.x, size.y]);
        }
        let fullscreen = viewport.fullscreen.unwrap_or(false);

        // The mouse's back and forward buttons, as lightview.
        if self.no_dialog(&ctx) {
            for (button, action) in [(egui::PointerButton::Extra1, Action::Previous), (egui::PointerButton::Extra2, Action::Next)] {
                if ctx.input(|i| i.pointer.button_pressed(button)) {
                    actions.push(action);
                }
            }
        }
        // shortcut: winit gets no dropped files on Wayland yet; it works on X11 and Windows.
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            self.open_dropped(path, &ctx);
        }
        let why_not_rename = if !self.has_track() { Some(tr("no file open")) } else if !self.track.done() { Some(tr("after loading")) } else if self.tag_job.is_some() { Some(tr("saving tags…")) } else { None };
        let why_not_identify = if !identify::ready(&self.settings.acoustid_key) { Some(tr("add an AcoustID key in Settings")) } else { why_not_rename.filter(|&w| w != "saving tags…") };
        // The write closes the tag editor, which would drop its unsaved edits.
        let why_not_cover = if !self.has_track() { Some(tr("no file open")) } else if self.tag_job.is_some() { Some(tr("saving tags…")) } else if self.tag_editor.is_some() { Some(tr("close the tag editor first")) } else { None };
        let beats_found = self.track.beats.lock().unwrap_or_else(|e| e.into_inner()).is_some();

        let mut ai_tool = None;
        // A menu item: greyed out with `why` on hover while it can't run; clicked, it runs `command`.
        let item = |ui: &mut egui::Ui, commands: &mut Vec<Command>, label: &str, command: Command, why: Option<&str>| {
            if ui.add_enabled(why.is_none(), egui::Button::new(label).shortcut_text(key_text(command))).on_disabled_hover_text(why.unwrap_or_default()).clicked() {
                commands.push(command);
                ui.close();
            }
        };
        let no_file = (!self.has_track()).then_some(tr("no file open"));
        // No menu bar in full screen.
        (!fullscreen).then(|| egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(tr("File"), |ui| {
                    item(ui, &mut commands, tr("Open…"), Command::Open, None);
                    ui.add_enabled_ui(!self.recent.is_empty(), |ui| {
                        ui.menu_button(tr("Open Recent"), |ui| {
                            self.recent.retain(|p| p.exists());
                            for path in &self.recent {
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
                                    open_recent = Some(path.clone());
                                    ui.close();
                                }
                            }
                        });
                    });
                    item(ui, &mut commands, tr("Close"), Command::Close, None);
                    ui.separator();
                    item(ui, &mut commands, if cfg!(windows) { tr("Show in Explorer") } else { tr("Show in File Manager") }, Command::ShowInFolder, no_file);
                    item(ui, &mut commands, tr("Copy Path"), Command::CopyPath, no_file);
                    item(ui, &mut commands, tr("Copy File"), Command::CopyFile, no_file);
                    let why = (!self.can_save_copy()).then_some(tr("with unsaved edits, once loaded"));
                    item(ui, &mut commands, tr("Save Copy"), Command::SaveCopy, why);
                    item(ui, &mut commands, tr("Save As…"), Command::SaveAs, (!loaded).then_some(no_file.unwrap_or(tr("after loading"))));
                    ui.separator();
                    item(ui, &mut commands, tr("Rename…"), Command::Rename, why_not_rename);
                    item(ui, &mut commands, tr("Delete…"), Command::Delete, no_file);
                    item(ui, &mut commands, tr("Rename from Tags…"), Command::RenameFromTags, why_not_rename);
                    item(ui, &mut commands, tr("Identify Track…"), Command::Identify, why_not_identify);
                    ui.separator();
                    item(ui, &mut commands, tr("Settings…"), Command::Settings, None);
                    item(ui, &mut commands, tr("Quit"), Command::Quit, None);
                });
                ui.menu_button(tr("Edit"), |ui| {
                    let (history, selected) = (self.history.as_ref(), self.edit_view.selection.is_some() && self.track.done());
                    let nothing = |on: bool, why| (!on).then_some(why);
                    item(ui, &mut commands, tr("Undo"), Command::Undo, nothing(history.is_some_and(|h| h.can_undo()) || !self.deleted.is_empty(), tr("nothing to undo")));
                    item(ui, &mut commands, tr("Redo"), Command::Redo, nothing(history.is_some_and(|h| h.can_redo()), tr("nothing to redo")));
                    ui.separator();
                    for (label, command) in [(tr("Cut out"), Command::Cut), (tr("Crop"), Command::Crop), (tr("Silence"), Command::Silence)] {
                        item(ui, &mut commands, label, command, nothing(selected, tr("select a part in the Edit panel")));
                    }
                    ui.separator();
                    item(ui, &mut commands, tr("Add/Replace Album Cover…"), Command::AddCover, why_not_cover);
                    item(ui, &mut commands, tr("Remove Album Cover…"), Command::RemoveCover, why_not_cover);
                });
                ui.add_enabled_ui(self.has_track(), |ui| ui.menu_button(tr("Effects"), |ui| self.effects_menu(ui, &mut commands, &key_text)));
                ui.menu_button(tr("View"), |ui| {
                    for (on, label, command) in [(self.show_info, tr("Track Info"), Command::TrackInfo), (self.show_folder, tr("Folder"), Command::Folder), (self.show_edit, tr("Edit"), Command::EditPanel)] {
                        let mut on = on;
                        if ui.add(egui::Checkbox::new(&mut on, label)).on_hover_text(key_text(command)).clicked() {
                            commands.push(command);
                            ui.close();
                        }
                    }
                    item(ui, &mut commands, tr("Full Screen"), Command::FullScreen, None);
                    ui.separator();
                    let mut on = self.settings.practice;
                    if ui.checkbox(&mut on, tr("Practice mode")).on_hover_text(tr("Tools for musicians: loops, the spectrogram, tempo, key and chords")).clicked() {
                        commands.push(Command::Practice);
                        ui.close();
                    }
                });
                ui.menu_button(tr("Playback"), |ui| {
                    for knob in Knob::ALL {
                        ui.menu_button(knob.title(), |ui| {
                            let now = knob.get(self.speed);
                            for &v in knob.presets() {
                                if ui.radio((now - v).abs() < 1e-6, knob.label(v)).on_hover_text(key_text(Command::Speed(knob, v))).clicked() {
                                    commands.push(Command::Speed(knob, v));
                                    ui.close();
                                }
                            }
                            let custom = !knob.presets().iter().any(|v| (now - v).abs() < 1e-6);
                            let label = if custom { trf("Custom ({value})…", &[("value", &knob.label((now * 100.0).round() / 100.0))]) } else { tr("Custom…").into() };
                            if ui.radio(custom, label).on_hover_text(key_text(Command::CustomSpeed(knob))).clicked() {
                                commands.push(Command::CustomSpeed(knob));
                                ui.close();
                            }
                        });
                    }
                    ui.separator();
                    item(ui, &mut commands, tr("Reset"), Command::ResetSpeed, (self.speed == wsola::Speed::NORMAL).then_some(tr("already normal")));
                });
                ui.menu_button(tr("Local AI"), |ui| {
                    let key = |task: Option<Task>| key_text(task.map_or(Command::AiModels, Command::Ai));
                    if let Some(task) = self.ai.menu_items(ui, &self.settings.ai, loaded, self.edit_view.selection.is_some(), &key) {
                        commands.push(Command::Ai(task));
                    }
                });
                if self.settings.practice {
                    ui.menu_button(tr("Practice"), |ui| {
                        ui.checkbox(&mut self.looping, tr("Loop selection")).on_hover_text("L");
                        let mut on = self.metronome;
                        if ui.add_enabled(beats_found, egui::Checkbox::new(&mut on, tr("Metronome"))).on_hover_text(format!("Clicks on the found beats {}", key_text(Command::Metronome))).on_disabled_hover_text(tr("after Local AI → Find Beats")).clicked() {
                            commands.push(Command::Metronome);
                        }
                        let mut on = self.show_spectrum;
                        if ui.checkbox(&mut on, tr("Spectrogram")).on_hover_text(key_text(Command::Spectrogram)).clicked() {
                            commands.push(Command::Spectrogram);
                            ui.close();
                        }
                    });
                }
            });
        }));

        // What the menus and keys asked for, each only while its menu item would be enabled.
        for command in commands {
            match command {
                Command::Open => open_file = true,
                Command::Close | Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Command::ShowInFolder if self.has_track() => folder::show_in_folder(&self.track.path),
                Command::CopyPath if self.has_track() => ctx.copy_text(self.track.path.display().to_string()),
                Command::CopyFile if self.has_track() => self.copy_file(&ctx),
                Command::SaveCopy => save_copy = true,
                Command::SaveAs if loaded => self.show_save = true,
                Command::Rename if why_not_rename.is_none() => self.start_rename(),
                Command::Delete if self.has_track() => self.delete_prompt = true,
                Command::RenameFromTags if why_not_rename.is_none() => self.rename = Some((self.settings.rename_pattern.clone(), tags::read(&self.track.path))),
                Command::Identify if why_not_identify.is_none() => self.identify = Some(Identify { path: self.track.path.clone(), found: None, pick: 0, only_empty: true, job: None }),
                Command::Settings => self.show_settings = true,
                Command::Undo => actions.push(Action::Undo),
                Command::Redo => actions.push(Action::Redo),
                Command::Cut => actions.push(Action::Cut),
                Command::Crop => actions.push(Action::Crop),
                Command::Silence => actions.push(Action::Silence),
                Command::AddCover if why_not_cover.is_none() => pick_cover = true,
                Command::RemoveCover if why_not_cover.is_none() => remove_cover = true,
                Command::Fx(fx) if self.fx_enabled(fx) => actions.push(Action::Fx(fx)),
                Command::CustomFx(fx) if loaded => self.custom_fx = Some((fx, fx.custom().3)),
                Command::TrackInfo => toggle_info = true,
                Command::Folder => toggle_folder = true,
                Command::EditPanel => toggle_edit = true,
                Command::FullScreen => ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen)),
                Command::Practice => {
                    self.settings.practice = !self.settings.practice;
                    if !self.settings.practice {
                        self.looping = false;
                        self.show_spectrum = false;
                    }
                }
                Command::Speed(knob, v) => actions.push(Action::Speed(knob.set(self.speed, v))),
                Command::CustomSpeed(knob) => self.custom_speed = Some((knob, knob.get(self.speed))),
                Command::ResetSpeed => actions.push(Action::Speed(wsola::Speed::NORMAL)),
                Command::Ai(task) if self.ai.why_not(task, &self.settings.ai, loaded).is_none() => ai_tool = Some(task),
                Command::AiModels => self.ai.show_models = true,
                Command::Metronome if self.settings.practice && beats_found => self.metronome = !self.metronome,
                Command::Spectrogram if self.settings.practice => self.show_spectrum = !self.show_spectrum,
                _ => {}
            }
        }

        let cover = if self.settings.visualiser { Some(self.visual.frame(&ctx, &self.track, &edit, pos, rate, playing)) } else { self.cover_texture(&ctx) };
        let tags = self.track.tags.get().cloned().unwrap_or_default();
        let error = self.track.error.lock().unwrap().clone().or_else(|| self.player.as_ref().err().cloned());
        let has_next = self.neighbour(false).is_some();
        // A match taken into the tag editor shows Track Info, where it is.
        if self.identify_window(&ctx) && !self.show_info {
            toggle_info = true;
        }
        if toggle_info {
            self.show_info = !self.show_info;
            // Grow or shrink by the panel, so the player view keeps its size.
            if resizable && let Some(size) = self.window_size {
                let size = resize_for_panel(size, self.show_info, self.info_width, 0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
            }
        }
        if toggle_folder {
            self.show_folder = !self.show_folder;
            if resizable && let Some(size) = self.window_size {
                let size = resize_for_panel(size, self.show_folder, self.folder_width, 0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
            }
            // Opens on the playing track's folder, read afresh.
            self.browse = None;
        }
        if toggle_edit {
            self.show_edit = !self.show_edit;
            if resizable && let Some(size) = self.window_size {
                let size = resize_for_panel(size, self.show_edit, self.edit_height, 1);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
            }
        }
        // Before Track Info, so it spans the whole width.
        if self.show_edit {
            let panel = egui::Panel::bottom("edit").resizable(true).size_range(100.0..=400.0).default_size(self.edit_height).show(ui, |ui| {
                edit_panel(ui, &mut self.edit_view, &mut self.panel_cache, &self.track, &edit, pos, len, rate, playing)
            });
            self.edit_height = panel.response.rect.height();
            if let Some(frame) = panel.inner {
                actions.push(Action::Seek(frame));
            }
        }
        if self.show_info {
            let size = std::fs::metadata(&self.track.path).map(|m| m.len()).ok();
            let panel = egui::Panel::left("info").resizable(true).size_range(240.0..=360.0).default_size(self.info_width).show(ui, |ui| {
                let can_edit = if !self.has_track() { Err(tr("no file open")) } else if !self.track.done() { Err(tr("after loading")) } else if self.tag_job.is_some() { Err(tr("saving tags…")) } else { Ok(()) };
                info_panel(ui, &self.track, size, &mut self.tag_editor, can_edit, self.settings.practice)
            });
            match panel.inner {
                Some(TagClick::Edit) => match tags::read(&self.track.path) {
                    Ok(t) => self.tag_editor = Some(TagEditor { read: t.clone(), draft: t, confirm: false, error: None }),
                    Err(e) => self.save_notice = Some((Err(e), ui.input(|i| i.time))),
                },
                Some(TagClick::Save) => {
                    if let Some(e) = &mut self.tag_editor {
                        e.confirm = true;
                    }
                }
                None => {}
            }
            self.info_width = panel.response.rect.width();
        }
        if self.show_folder {
            let panel = egui::Panel::right("folder").resizable(true).size_range(200.0..=400.0).default_size(self.folder_width).show(ui, |ui| self.folder_panel(ui));
            self.folder_width = panel.response.rect.width();
            if let Some(path) = panel.inner {
                match self.files.iter().position(|p| *p == path) {
                    Some(i) => self.go(i),
                    None => self.open(path),
                }
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if !self.has_track() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.heading(tr("No file open"));
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(tr("Open…")).shortcut_text(key_text(Command::Open))).clicked() {
                        open_file = true;
                    }
                    notice(ui, self.save_notice.as_ref(), Some(ui.input(|i| i.time)));
                });
                return;
            }
            ui.vertical_centered(|ui| {
                // The cover takes the width, but leaves room below for what was measured last frame.
                let gap = 16.0 + 2.0 * ui.spacing().item_spacing.y;
                let side = cover_side(ui.available_width(), ui.available_height(), self.controls_height, gap);
                cover_art(ui, cover.as_ref(), side, &mut self.settings.cover_fit, &mut self.settings.visualiser);
                ui.add_space(16.0);
                let controls = ui.scope(|ui| {
                    let title = tags.title.unwrap_or_else(|| self.track.path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                    let time = ui.input(|i| i.time);
                    marquee_label(ui, &title, egui::TextStyle::Heading.resolve(ui.style()), ui.visuals().strong_text_color(), time);
                    for line in [tags.artist, tags.album].into_iter().flatten() {
                        marquee_label(ui, &line, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color(), time);
                    }
                    if let Some(e) = &error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    notice(ui, self.save_notice.as_ref(), Some(time));
                    ui.add_space(12.0);
                    let wave = self.settings.show_waveform.then_some((&*self.track, &*edit, self.edit_view.selection, &mut self.strip_cache));
                    if let Some(frame) = seek_bar(ui, pos, len, rate, wave) {
                        actions.push(Action::Seek(frame));
                    }
                    ui.add_space(8.0);
                    let metronome = self.track.beats.lock().unwrap_or_else(|e| e.into_inner()).is_some().then_some((self.metronome, self.click_volume));
                    actions.extend(transport(ui, playing, has_next, self.volume, metronome, self.settings.order));
                });
                let height = controls.response.rect.height();
                if (height - self.controls_height).abs() > 0.5 {
                    // Settles in one extra frame when tags, the waveform or an error line change it.
                    self.controls_height = height;
                    ui.ctx().request_repaint();
                }
            });
        });

        self.settings_window(&ctx);
        if let Some(speed) = self.speed_window(&ctx) {
            actions.push(Action::Speed(speed));
        }
        if let Some(fx) = self.fx_window(&ctx) {
            actions.push(Action::Fx(fx));
        }
        if save_copy {
            self.save_copy();
        }
        self.follow_save(&ctx);
        self.unsaved_window(&ctx);
        self.save_window(&ctx);
        self.tags_confirm(&ctx);
        self.cover_offer_modal(&ctx);
        self.follow_tags(&ctx);
        self.rename_window(&ctx);
        self.rename_file_window(&ctx);
        self.delete_window(&ctx);
        self.changed_window(&ctx, pos);
        self.default_window(&ctx);
        self.shortcuts_window(&ctx);
        if let Some(task) = ai_tool {
            self.ai.start_tool(task, self.track.clone(), edit.clone(), self.edit_view.selection, &self.settings.ai, &ctx);
        }
        if let Some(path) = self.ai.windows(&ctx, &mut self.settings.ai) {
            self.open(path);
        }
        actions.extend(self.spectrum_window(&ctx, &edit, pos, len, rate, playing));
        if let Some(path) = open_recent {
            self.open(path);
        }
        if open_file && let Some(path) = rfd::FileDialog::new().add_filter(tr("Audio"), &folder::EXTENSIONS).pick_file() {
            self.open(path);
        }
        let dir = self.track.path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        if pick_cover && let Some(image) = rfd::FileDialog::new().add_filter(tr("Image"), folder::IMAGE_EXTS).set_directory(dir).pick_file()
            && let Err(e) = self.offer_cover(&ctx, image, true)
        {
            self.save_notice = Some((Err(e), ctx.input(|i| i.time)));
        }
        if remove_cover {
            // The preview is the current track's own cover, if it shows one.
            let texture = self.cover_texture(&ctx);
            self.cover_offer = Some(CoverOffer { image: PathBuf::new(), cover: tags::Cover::Remove, files: folder::list_folder(&self.track.path), texture, manual: true });
        }
        for action in actions {
            self.apply(action, pos, len);
        }
        // The Metronome follows the track, its edits and its beats.
        let beats = self.track.beats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(b) = &beats
            && Arc::as_ptr(b) as usize != self.beats_seen
        {
            self.beats_seen = Arc::as_ptr(b) as usize;
            self.metronome = true;
            self.tag_bpm(b);
        }
        let want = beats.as_ref().filter(|_| self.metronome).map(|b| (Arc::as_ptr(&self.track) as usize, self.history.as_ref().map(|h| Arc::as_ptr(h.current()) as usize), Arc::as_ptr(b) as usize));
        if self.sent_clicks != Some(want) && let Ok(p) = &self.player {
            let clicks = beats.filter(|_| want.is_some()).map(|b| Arc::new(player::Clicks { beats: ai::edited_beats(&b.beats, &edit) }));
            p.send(Cmd::ClickVolume(volume_gain(self.click_volume)));
            p.send(Cmd::Clicks(clicks));
            self.sent_clicks = Some(want);
        }
        let region = (self.edit_view.selection, self.looping);
        if self.sent_region != Some(region) && let Ok(p) = &self.player {
            p.send(Cmd::Region(region.0, region.1));
            self.sent_region = Some(region);
        }
    }
}

/// `name — 3:12 — MP3`, leaving out what isn't known.
fn title(name: &str, length: Option<&str>, format: &str) -> String {
    [Some(name), length, (!format.is_empty()).then_some(format)].into_iter().flatten().collect::<Vec<_>>().join(" — ")
}

/// Track Info's rows; "—" for what isn't known, "measuring…" while loudness is computed.
/// Grouped as Track Info shows them: General, Audio, Tags.
type Rows = Vec<(&'static str, String)>;
fn info_rows(track: &Track, file_size: Option<u64>, practice: bool) -> [(&'static str, Rows); 3] {
    let unknown = || "—".to_string();
    let tags = track.tags.get().cloned().unwrap_or_default();
    let info = track.info.get().cloned().unwrap_or_default();
    let format = track.format.get();
    let rate = format.map_or(0, |f| f.rate as u64);
    let seconds = (track.done() && rate > 0).then(|| track.len() as f64 / rate as f64);
    let or = |v: String| if v.is_empty() { unknown() } else { v };
    let channels = format.map_or_else(unknown, |f| match f.channels {
        1 => tr("1 (mono)").into(),
        2 => tr("2 (stereo)").into(),
        n => n.to_string(),
    });
    let bitrate = seconds.zip(file_size).filter(|(s, _)| *s > 0.0).map_or_else(unknown, |(s, b)| format!("{:.0} kb/s", b as f64 * 8.0 / s / 1000.0));
    let loudness = match (track.loudness.get(), track.done()) {
        (Some(l), _) if l.lufs.is_finite() => (format!("{:.1} LUFS", l.lufs), format!("{:.1} dBFS", l.peak_db)),
        (Some(_), _) => (tr("silent").into(), tr("silent").into()),
        (None, true) => (tr("measuring…").into(), tr("measuring…").into()),
        (None, false) => (tr("after loading").into(), tr("after loading").into()),
    };
    let (tempo, key) = match (track.tempo.get(), track.done()) {
        (Some(e), _) => (e.bpm.map_or_else(unknown, |b| format!("≈ {b:.0} BPM")), e.key.map_or_else(unknown, |k| format!("≈ {}", k.name()))),
        (None, true) => (tr("measuring…").into(), tr("measuring…").into()),
        (None, false) => (tr("after loading").into(), tr("after loading").into()),
    };
    let path = &track.path;
    let general = vec![
        (tr_key("Name"), path.file_name().unwrap_or_default().to_string_lossy().into_owned()),
        (tr_key("Folder"), path.parent().map_or_else(unknown, |p| p.display().to_string())),
        (tr_key("Format"), or(info.container)),
        (tr_key("Codec"), or(info.codec)),
        (tr_key("File size"), file_size.map_or_else(unknown, |b| format!("{:.1} MB", b as f64 / 1e6))),
        (tr_key("Length"), seconds.map_or_else(unknown, |_| clock(track.len(), rate))),
        (tr_key("Bitrate"), bitrate),
    ];
    let mut audio = vec![
        (tr_key("Sample rate"), if rate > 0 { format!("{:.1} kHz", rate as f64 / 1000.0) } else { unknown() }),
        (tr_key("Channels"), channels),
        (tr_key("Bit depth"), info.bits.map_or_else(unknown, |b| format!("{b}-bit"))),
        (tr_key("Loudness"), loudness.0),
        (tr_key("Peak"), loudness.1),
    ];
    if practice {
        audio.extend([(tr_key("Tempo"), tempo), (tr_key("Key"), key)]);
    }
    let tags = vec![
        (tr_key("Title"), tags.title.unwrap_or_else(unknown)),
        (tr_key("Artist"), tags.artist.unwrap_or_else(unknown)),
        (tr_key("Album"), tags.album.unwrap_or_else(unknown)),
    ];
    [(tr_key("General"), general), (tr_key("Audio"), audio), (tr_key("Tags"), tags)]
}

/// Track Info's body, laid out like lightview's inspector: collapsing sections of two-column grids.
#[derive(Clone, Copy)]
enum TagClick {
    Edit,
    Save,
}

/// The tag fields of Track Info's editor, with Save and Cancel; Cancel closes it here.
/// `estimate` offers the measured tempo and key for the BPM and Key fields.
fn tag_fields(ui: &mut egui::Ui, editor: &mut Option<TagEditor>, estimate: tempo::Estimate) -> Option<TagClick> {
    let e = editor.as_mut()?;
    let mut click = None;
    egui::Grid::new("tag fields").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        let d = &mut e.draft;
        for (name, field) in [(tr("Title"), &mut d.title), (tr("Artist"), &mut d.artist), (tr("Album"), &mut d.album), (tr("Year"), &mut d.year), (tr("Track"), &mut d.track), (tr("Genre"), &mut d.genre)] {
            ui.label(name);
            ui.add(egui::TextEdit::singleline(field).desired_width(f32::INFINITY));
            ui.end_row();
        }
        ui.label(tr("BPM"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut d.bpm).desired_width(48.0));
            if let Some(bpm) = estimate.bpm
                && ui.button(format!("≈ {bpm:.0}")).on_hover_text(tr("Use the estimated tempo")).clicked()
            {
                d.bpm = format!("{bpm:.0}");
            }
            // Any estimate can be an octave off: a slow song counted at double time, or the reverse.
            let n = d.bpm.trim().parse::<u32>().ok().filter(|&n| n > 0);
            if ui.add_enabled(n.is_some(), egui::Button::new("½×")).on_hover_text(tr("Half the tempo")).clicked() {
                d.bpm = n.unwrap_or(0).div_ceil(2).to_string();
            }
            if ui.add_enabled(n.is_some(), egui::Button::new("2×")).on_hover_text(tr("Double the tempo")).clicked() {
                d.bpm = (n.unwrap_or(0) * 2).to_string();
            }
        });
        ui.end_row();
        ui.label(tr("Key"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut d.key).desired_width(48.0));
            if let Some(key) = estimate.key
                && ui.button(format!("≈ {}", key.short_name())).on_hover_text(trf("Use the estimated key, {key}", &[("key", &key.name())])).clicked()
            {
                d.key = key.short_name();
            }
        });
        ui.end_row();
        ui.label(tr("Cover"));
        ui.horizontal(|ui| {
            ui.label(match d.cover {
                tags::Cover::Keep => tr("as it is"),
                tags::Cover::Remove => tr("removed"),
                tags::Cover::New(_) => tr("new image"),
            });
            if ui.button(tr("Replace…")).clicked() && let Some(path) = rfd::FileDialog::new().add_filter(tr("Image"), folder::IMAGE_EXTS).pick_file() {
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(tags::cover) {
                    Ok(c) => (d.cover, e.error) = (c, None),
                    Err(err) => e.error = Some(err),
                }
            }
            if ui.button(tr("Remove")).clicked() {
                d.cover = tags::Cover::Remove;
            }
        });
        ui.end_row();
    });
    if let Some(err) = &e.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
    }
    let (changed, mut cancel) = (e.changes().is_some(), false);
    ui.horizontal(|ui| {
        if ui.add_enabled(changed, egui::Button::new(tr("Save…"))).on_hover_text(tr("Writes the tags into the file; the audio isn't changed")).clicked() {
            click = Some(TagClick::Save);
        }
        cancel = ui.button(tr("Cancel")).clicked();
    });
    if cancel {
        *editor = None;
    }
    click
}

/// `can_edit` is why the tags can't be edited now, if they can't.
fn info_panel(ui: &mut egui::Ui, track: &Track, file_size: Option<u64>, editor: &mut Option<TagEditor>, can_edit: Result<(), &str>, practice: bool) -> Option<TagClick> {
    let mut click = None;
    // Fill the panel's width: shrinking to the content made it shrink with it (as in lightview).
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for (section, rows) in info_rows(track, file_size, practice) {
            egui::CollapsingHeader::new(tr(section)).default_open(true).show(ui, |ui| {
                if section == "Tags" {
                    if editor.is_some() {
                        click = tag_fields(ui, editor, track.tempo.get().copied().filter(|_| practice).unwrap_or_default());
                        return;
                    }
                    let edit = ui.add_enabled(can_edit.is_ok(), egui::Button::new(tr("Edit")));
                    let edit = if let Err(why) = can_edit { edit.on_disabled_hover_text(why) } else { edit };
                    if edit.clicked() {
                        click = Some(TagClick::Edit);
                    }
                }
                egui::Grid::new(section).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    // Tags ends with every tag as the file has it.
                    let raw = if section == "Tags" { track.info.get().map(|i| i.raw.clone()).unwrap_or_default() } else { Vec::new() };
                    for (name, value) in rows.into_iter().map(|(n, v)| (tr(n).to_string(), v)).chain(raw) {
                        ui.label(name);
                        ui.add(egui::Label::new(value).wrap());
                        ui.end_row();
                    }
                });
            });
        }
    });
    click
}

/// The window size after a panel opens (bigger by `extent` along `axis`, 0 = width) or closes.
fn resize_for_panel(mut size: [f32; 2], opening: bool, extent: f32, axis: usize) -> [f32; 2] {
    size[axis] = if opening { size[axis] + extent } else { (size[axis] - extent).max(1.0) };
    size
}



/// A folder image offered as the cover: its path, the cover to write and a thumbnail to show.
type FoundCover = (PathBuf, tags::Cover, egui::ColorImage);

/// Reads and checks `image` for the cover offer: the cover to write and a 256 px thumbnail.
fn read_cover(image: &std::path::Path) -> Result<(tags::Cover, egui::ColorImage), String> {
    let bytes = std::fs::read(image).map_err(|e| e.to_string())?;
    let rgba = image::load_from_memory(&bytes).map_err(|_| tr("That image can't be read."))?.thumbnail(256, 256).to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok((tags::cover(bytes)?, egui::ColorImage::from_rgba_unmultiplied(size, &rgba)))
}

/// A name typed for a file: not empty, not `.` or `..`, and no folder separators.
fn valid_file_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(tr("That isn't a valid file name.").into());
    }
    Ok(())
}

/// How many files Open Recent lists.
const RECENT_MAX: usize = 10;

/// Moves `path` to the front of `recent`, keeping at most `RECENT_MAX`. From lightview.
fn push_recent(recent: &mut Vec<PathBuf>, path: &std::path::Path) {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_path_buf());
    recent.truncate(RECENT_MAX);
}

impl App {
    /// Notices the player moving into the queued track, and decodes the next file ahead.
    fn follow_player(&mut self) {
        let Ok(p) = &self.player else { return };
        // Tracks the audio callback is done with are freed here, not in the callback.
        while p.retired.try_recv().is_ok() {}
        let advanced = p.shared.advanced.load(Relaxed);
        if advanced != self.advanced {
            self.advanced = advanced;
            if let Some(next) = self.next.take() {
                let index = self.files.iter().position(|p| *p == next.path).unwrap_or(self.index);
                (self.track, self.index) = (next, index);
                (self.history, self.edit_view, self.sent_region) = (None, EditView::default(), None);
            }
        }
        // Decode ahead once this track is in memory, so Next and the gapless move are instant.
        // Never with no file: Repeat track would queue the empty track after itself forever.
        if self.has_track() && self.track.done() && self.next.is_none() && let Some(index) = self.upcoming() {
            // Repeat track queues the same, already decoded track.
            let next = if index == self.index { self.track.clone() } else { Track::load_ahead(self.files[index].clone(), repaint(&self.ctx)) };
            p.send(Cmd::Queue(Some(next.clone())));
            self.next = Some(next);
        }
    }

    /// The file to play when this track ends, by the play order.
    fn upcoming(&self) -> Option<usize> {
        match self.settings.order {
            Order::RepeatOne => Some(self.index),
            order => neighbour(order, self.shuffle, self.files.len(), self.index, false),
        }
    }

    /// The file Next (or Previous, with `back`) moves to; Repeat track moves through the folder.
    fn neighbour(&self, back: bool) -> Option<usize> {
        neighbour(self.settings.order, self.shuffle, self.files.len(), self.index, back)
    }

    fn apply(&mut self, action: Action, pos: u64, len: u64) {
        let Ok(p) = &self.player else { return };
        match action {
            // Past the track's own edits, Undo brings back the last file moved to the Trash.
            Action::Undo if !self.history.as_ref().is_some_and(|h| h.can_undo()) && !self.deleted.is_empty() => self.restore_deleted(),
            Action::Undo | Action::Redo | Action::Cut | Action::Crop | Action::Silence | Action::Fx(_) => self.change(action, pos),
            Action::Previous => self.previous(),
            Action::Next => {
                if let Some(index) = self.neighbour(false) {
                    self.go(index);
                }
            }
            Action::Order(order) => {
                if order == Order::Shuffle {
                    self.shuffle = (random_seed(), self.index);
                }
                self.settings.order = order;
                // A track queued for the old order is dropped; follow_player queues the new one.
                // shortcut: should the player move into the old one before Queue(None) arrives, the UI misses it.
                if self.next.as_ref().map(|n| &n.path) != self.upcoming().map(|i| &self.files[i]) {
                    self.next = None;
                    p.send(Cmd::Queue(None));
                }
            }
            Action::Seek(frame) => p.send(Cmd::Seek(frame)),
            Action::Speed(speed) => {
                self.speed = speed;
                p.send(Cmd::Speed(speed));
            }
            Action::Volume(v) => {
                self.volume = v;
                p.send(Cmd::Volume(volume_gain(v)));
            }
            Action::Metronome(on) => self.metronome = on,
            Action::ClickVolume(v) => {
                self.click_volume = v;
                p.send(Cmd::ClickVolume(volume_gain(v)));
            }
            Action::Toggle if p.shared.playing.load(Relaxed) => p.send(Cmd::Pause),
            Action::Toggle => {
                // With a selection, Play starts it unless the playhead is already inside it;
                // at the end, Play starts the track again.
                if let Some((a, b)) = self.edit_view.selection && !(a..b).contains(&pos) {
                    p.send(Cmd::Seek(a));
                } else if self.track.done() && pos >= len {
                    p.send(Cmd::Seek(0));
                }
                p.send(Cmd::Play);
            }
        }
    }

    /// The Custom speed or pitch dialog; returns the new settings when OK is pressed.
    /// The Folder panel: `..`, the subfolders, then the audio files. A folder click browses
    /// there; returns the file clicked, to play.
    fn folder_panel(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        // With no file open, the home folder.
        let home = || std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        let here = self.track.path.parent().map(PathBuf::from).unwrap_or_else(home);
        let b = self.browse.get_or_insert_with(|| Browse::new(here));
        let name = |p: &std::path::Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
        ui.strong(name(&b.dir)).on_hover_text(b.dir.display().to_string());
        ui.separator();
        let (mut enter, mut play) = (None, None);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if let Some(parent) = b.dir.parent() && ui.selectable_label(false, "..").on_hover_text(tr("Up one folder")).clicked() {
                enter = Some(parent.to_path_buf());
            }
            match &b.listing {
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Ok((dirs, files)) => {
                    for d in dirs {
                        if ui.selectable_label(false, format!("🗀 {}", name(d))).clicked() {
                            enter = Some(std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()));
                        }
                    }
                    for f in files {
                        let current = *f == self.track.path;
                        let row = ui.selectable_label(current, format!("♪ {}", name(f)));
                        if current && b.scrolled.as_ref() != Some(f) {
                            row.scroll_to_me(Some(egui::Align::Center));
                            b.scrolled = Some(f.clone());
                        }
                        if row.clicked() && !current {
                            play = Some(f.clone());
                        }
                    }
                    if dirs.is_empty() && files.is_empty() {
                        ui.weak(tr("No audio files or folders here"));
                    }
                }
            }
        });
        if let Some(dir) = enter {
            *b = Browse::new(dir);
        }
        play
    }

    fn speed_window(&mut self, ctx: &egui::Context) -> Option<wsola::Speed> {
        let (knob, mut draft) = self.custom_speed?;
        let (mut chosen, mut close) = (None, false);
        let (step, range, decimals, unit) = if knob == Knob::Pitch { (PITCH_STEP, -12.0..=12.0, 1, tr(" semitones")) } else { (SPEED_STEP, 0.5..=2.0, 2, "×") };
        egui::Window::new(knob.title()).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("−").clicked() {
                    draft -= step;
                }
                ui.add(egui::DragValue::new(&mut draft).range(range).speed(step / 5.0).fixed_decimals(decimals).suffix(unit));
                if ui.button("+").clicked() {
                    draft += step;
                }
            });
            ui.horizontal(|ui| {
                if ui.button(tr("OK")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = Some(knob.set(self.speed, knob.snap(draft)));
                    close = true;
                }
                if ui.button(tr("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        self.custom_speed = (!close).then_some((knob, knob.snap(draft)));
        chosen
    }

    /// Whether the effect `fx` can run now: decoded, "Whole selection" fades need a selection, and
    /// Channels only offers what changes the track.
    fn fx_enabled(&self, fx: Fx) -> bool {
        let out = self.edit().channels(self.track.format.get().map_or(1, |f| f.channels));
        self.has_track() && self.track.done() && match fx {
            Fx::FadeIn(None) | Fx::FadeOut(None) => self.edit_view.selection.is_some(),
            Fx::Mono => out >= 2,
            Fx::Stereo => out == 1,
            Fx::Swap => out == 2,
            _ => true,
        }
    }

    /// The Effects menu: presets, then Custom… in each submenu.
    fn effects_menu(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>, key_text: &dyn Fn(Command) -> String) {
        let selected = self.edit_view.selection.is_some();
        let mut pick = |ui: &mut egui::Ui, label: &str, command: Command| {
            let fx = match command {
                Command::Fx(fx) | Command::CustomFx(fx) => fx,
                _ => unreachable!("only effects here"),
            };
            // Custom… opens a dialog with the value, so it only needs the track decoded.
            let enabled = if matches!(command, Command::CustomFx(_)) { self.has_track() && self.track.done() } else { self.fx_enabled(fx) };
            if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key_text(command))).clicked() {
                commands.push(command);
                ui.close();
            }
        };
        ui.weak(if selected { tr("On the selection") } else { tr("On the whole track") });
        for (name, fade) in [(tr("Fade in"), Fx::FadeIn as fn(Option<f64>) -> Fx), (tr("Fade out"), Fx::FadeOut)] {
            ui.menu_button(name, |ui| {
                pick(ui, tr("Whole selection"), Command::Fx(fade(None)));
                for secs in FADES {
                    pick(ui, &trf("{secs} s", &[("secs", &secs)]), Command::Fx(fade(Some(secs))));
                }
                pick(ui, tr("Custom…"), Command::CustomFx(fade(None)));
            });
        }
        ui.menu_button(tr("Gain"), |ui| {
            for db in GAINS {
                pick(ui, &format!("{}{} dB", if db > 0.0 { "+" } else { "" }, signed(db)), Command::Fx(Fx::Gain(db)));
            }
            pick(ui, tr("Custom…"), Command::CustomFx(Fx::Gain(0.0)));
        });
        ui.menu_button(tr("Normalize"), |ui| {
            for (db, name) in PEAKS {
                pick(ui, &format!("{} ({} dBFS)", tr(name), signed(db)), Command::Fx(Fx::Peak(db)));
            }
            for (lufs, name) in LOUDNESS {
                pick(ui, &format!("{} ({} LUFS)", tr(name), signed(lufs)), Command::Fx(Fx::Loudness(lufs)));
            }
            pick(ui, tr("Custom peak…"), Command::CustomFx(Fx::Peak(0.0)));
            pick(ui, tr("Custom loudness…"), Command::CustomFx(Fx::Loudness(0.0)));
        });
        ui.menu_button(tr("Trim silence"), |ui| {
            for (db, name) in TRIMS {
                pick(ui, &format!("{} ({} dB)", tr(name), signed(db)), Command::Fx(Fx::Trim(db)));
            }
            pick(ui, tr("Custom…"), Command::CustomFx(Fx::Trim(0.0)));
        });
        pick(ui, tr("Reverse"), Command::Fx(Fx::Reverse));
        ui.separator();
        ui.menu_button(tr("Channels"), |ui| {
            ui.weak(tr("On the whole track"));
            for (label, fx) in [(tr("Mono"), Fx::Mono), (tr("Stereo"), Fx::Stereo), (tr("Swap left and right"), Fx::Swap)] {
                pick(ui, label, Command::Fx(fx));
            }
        });
    }

    /// The Custom… effect dialog; returns the effect when OK is pressed.
    fn fx_window(&mut self, ctx: &egui::Context) -> Option<Fx> {
        let (fx, mut draft) = self.custom_fx?;
        let (title, unit, range, _, hint) = fx.custom();
        let (mut chosen, mut close) = (None, false);
        egui::Window::new(title).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.add(egui::DragValue::new(&mut draft).range(range).speed(0.1).fixed_decimals(1).suffix(unit));
            if !hint.is_empty() {
                ui.weak(hint);
            }
            ui.horizontal(|ui| {
                if ui.button(tr("OK")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = Some(fx.with(draft));
                    close = true;
                }
                if ui.button(tr("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        self.custom_fx = (!close).then_some((fx, draft));
        chosen
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new(tr("Settings")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Seek bar:"));
                ui.radio_value(&mut self.settings.show_waveform, false, tr("Line"));
                ui.radio_value(&mut self.settings.show_waveform, true, tr("Waveform"));
            });
            ui.horizontal(|ui| {
                ui.label(tr("Theme:"));
                let mut chosen = false;
                egui::ComboBox::from_id_salt("theme").selected_text(theme::name(&self.settings.theme)).show_ui(ui, |ui| {
                    // Read only while the list is open, so a theme just copied in shows up.
                    for id in [theme::SYSTEM.to_string()].into_iter().chain(theme::user_themes()) {
                        if ui.selectable_label(self.settings.theme == id, theme::name(&id)).clicked() {
                            (self.settings.theme, chosen) = (id, true);
                        }
                    }
                }).response.on_hover_text(tr("Your own themes go in the themes folder. Choosing a theme again reloads its file."));
                if chosen {
                    self.theme_error = theme::apply(ctx, &self.settings.theme);
                }
                if ui.button(tr("Open themes folder")).clicked() {
                    theme::open_folder();
                }
            });
            if let Some(e) = &self.theme_error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| {
                ui.label(tr("Language:"));
                let current = self.settings.language.unwrap_or_default();
                egui::ComboBox::from_id_salt("language").selected_text(current.name()).show_ui(ui, |ui| {
                    for language in i18n::Language::ALL {
                        if ui.selectable_label(current == language, language.name()).clicked() {
                            self.settings.language = Some(language);
                            i18n::set(language);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr("Font size:"));
                egui::ComboBox::from_id_salt("font-size").selected_text(self.settings.font_size.to_string()).show_ui(ui, |ui| {
                    for size in settings::FONT_SIZES {
                        if ui.selectable_value(&mut self.settings.font_size, size, size.to_string()).clicked() {
                            settings::apply_font_size(ctx, size);
                        }
                    }
                });
            });
            ui.checkbox(&mut self.settings.remember_window_size, tr("Remember window size"));
            ui.weak(tr("Applies the next time soundglass opens; the window, Track Info and the Edit panel as you quit are kept."));
            ui.add_enabled_ui(!self.settings.remember_window_size, |ui| {
                ui.horizontal(|ui| {
                    let [w, h] = &mut self.settings.default_size;
                    ui.label(tr("Window size:"));
                    ui.add(egui::DragValue::new(w).range(settings::MIN_SIZE[0]..=7680.0));
                    ui.label("×");
                    ui.add(egui::DragValue::new(h).range(settings::MIN_SIZE[1]..=4320.0));
                });
            });
            if ui.button(tr("Change Keyboard Shortcuts…")).clicked() {
                self.show_shortcuts = true;
            }
            ui.separator();
            ui.strong(tr("AcoustID Settings"));
            ui.horizontal(|ui| {
                ui.label(tr("Application key:"));
                ui.text_edit_singleline(&mut self.settings.acoustid_key);
            });
            ui.horizontal(|ui| {
                ui.weak(tr("Identify Track needs one; it is free for non-commercial use."));
                if ui.link(tr("Get a key")).clicked() {
                    folder::open_url(identify::KEY_PAGE);
                }
            });
            if folder::desktop_installed() {
                ui.separator();
                if ui.button(tr("Make soundglass the default player")).clicked() {
                    self.default_result = Some(folder::make_default());
                }
                if let Some(result) = &self.default_result {
                    ui.weak(match result {
                        Ok(()) => tr("soundglass is now the default player"),
                        Err(e) => e,
                    });
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.weak(format!("soundglass {}", env!("CARGO_PKG_VERSION")));
                // Only opens the page: soundglass never checks on its own.
                if ui.button(tr("Check for updates")).clicked() {
                    folder::open_url("https://github.com/pyubero/soundglass/releases");
                }
            });
        });
        self.show_settings = open;
    }

    /// The current track's cover as a texture, uploaded once per track.
    fn cover_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let key = Arc::as_ptr(&self.track) as usize;
        if self.cover.as_ref().is_none_or(|(k, _)| *k != key) {
            let image = self.track.cover.get()?.clone();
            self.cover = Some((key, ctx.load_texture("cover", image, egui::TextureOptions::LINEAR)));
        }
        self.cover.as_ref().map(|(_, t)| t.clone())
    }

    /// Plays `path` and makes its folder the one Previous and Next move through.
    fn open(&mut self, path: PathBuf) {
        if self.dirty() {
            self.pending = Some(Pending::Open(path));
            return;
        }
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        // The Folder panel follows a file opened elsewhere (a tool's output, Recent, Open…).
        if self.browse.as_ref().is_some_and(|b| Some(b.dir.as_path()) != path.parent()) {
            self.browse = None;
        }
        self.remember(&path);
        self.files = folder::list_folder(&path);
        self.next = None;
        let index = self.files.iter().position(|p| *p == path).unwrap_or(0);
        self.shuffle.1 = index;
        self.go(index);
    }

    /// Plays file `index` of the folder, using the track decoded ahead when it is that one.
    fn go(&mut self, index: usize) {
        if self.dirty() {
            self.pending = Some(Pending::Go(index));
            return;
        }
        let (Some(path), Ok(p)) = (self.files.get(index), &self.player) else { return };
        let track = match self.next.take() {
            Some(next) if next.path == *path => next,
            _ => Track::load(path.clone(), repaint(&self.ctx)),
        };
        p.send(Cmd::Load(track.clone()));
        if self.shown {
            p.send(Cmd::Play);
        }
        (self.track, self.index) = (track, index);
        self.forget_edits();
    }

    /// A file is open (not the empty window).
    fn has_track(&self) -> bool {
        !self.track.path.as_os_str().is_empty()
    }

    /// Starts afresh on a new track: no edits, no selection, the whole track in view.
    fn forget_edits(&mut self) {
        (self.history, self.edit_view, self.sent_region) = (None, EditView::default(), None);
    }

    fn dirty(&self) -> bool {
        self.history.as_ref().is_some_and(|h| h.dirty())
    }

    /// Edits the track, or undoes or redoes an edit, and tells the player.
    fn change(&mut self, action: Action, pos: u64) {
        let Ok(p) = &self.player else { return };
        // Edits only once decoded, so they never point past the audio there is.
        if !self.has_track() || !self.track.done() {
            return;
        }
        let history = self.history.get_or_insert_with(|| edit::History::new(Edit::whole(self.track.len())));
        let sel = self.edit_view.selection;
        let (cur, view) = (history.current().clone(), &mut self.edit_view);
        let seek = match (action, sel) {
            (Action::Undo, _) => {
                history.undo();
                view.selection = None;
                pos.min(history.current().len())
            }
            (Action::Redo, _) => {
                history.redo();
                view.selection = None;
                pos.min(history.current().len())
            }
            (Action::Cut, Some((a, b))) => {
                history.push(cur.cut(a, b));
                view.selection = None;
                a
            }
            (Action::Crop, Some((a, b))) => {
                history.push(cur.crop(a, b));
                view.selection = None;
                0
            }
            (Action::Silence, Some((a, b))) => {
                history.push(cur.silence(a, b));
                pos
            }
            (Action::Fx(fx), _) => {
                let (rate, src) = self.track.format.get().map_or((1, 1), |f| (f.rate, f.channels));
                let (a, b) = sel.unwrap_or((0, cur.len()));
                let frames = |secs: f64| (secs * rate as f64) as u64;
                // shortcut: peak, loudness and trim measure on the UI thread (~0.1 s per 5 min); move to a thread if long recordings stutter.
                let edited = match fx {
                    Fx::FadeIn(secs) => Ok(cur.fade(a, secs.map_or(b, |s| (a + frames(s)).min(b)), false)),
                    Fx::FadeOut(secs) => Ok(cur.fade(secs.map_or(a, |s| b.saturating_sub(frames(s)).max(a)), b, true)),
                    Fx::Gain(db) => Ok(cur.gain(a, b, db as f32, db as f32)),
                    Fx::Peak(target) => match edit::peak(&self.track, &cur, a, b) {
                        0.0 => Err(tr("Nothing to normalize: it's silent")),
                        peak => {
                            let db = target as f32 - 20.0 * peak.log10();
                            Ok(cur.gain(a, b, db, db))
                        }
                    },
                    Fx::Loudness(target) => match edit::loudness(&self.track, &cur, a, b) {
                        lufs if lufs.is_finite() => Ok(cur.gain(a, b, (target - lufs) as f32, (target - lufs) as f32)),
                        _ => Err(tr("Too short or too quiet to measure its loudness")),
                    },
                    Fx::Trim(db) => match edit::audible(&self.track, &cur, a, b, db as f32) {
                        Some((first, last)) if (first, last) != (a, b) => Ok(cur.cut(last, b).cut(a, first)),
                        Some(_) => Err(tr("No silence at the start or end to trim")),
                        None => Err(tr("It's all quieter than that; nothing trimmed")),
                    },
                    Fx::Reverse => Ok(cur.reverse(a, b)),
                    Fx::Mono => Ok(cur.mono(src)),
                    Fx::Stereo => Ok(cur.stereo(src)),
                    Fx::Swap => Ok(cur.swap(src)),
                };
                match edited {
                    Ok(e) => history.push(e),
                    Err(m) => {
                        self.save_notice = Some((Ok(m.into()), self.ctx.get().map_or(0.0, |c| c.input(|i| i.time))));
                        return;
                    }
                }
                if let Fx::Trim(_) = fx {
                    view.selection = None;
                    a
                } else {
                    pos
                }
            }
            _ => return,
        };
        // Back at the first step the track plays as it is, and moves on gaplessly again.
        p.send(Cmd::Edit(history.can_undo().then(|| history.current().clone())));
        p.send(Cmd::Seek(seek));
    }

    /// Asks what to do with unsaved edits before leaving the track.
    fn unsaved_window(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() || self.show_save {
            return;
        }
        let (mut save, mut discard, mut cancel) = (false, false, false);
        let modal = egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
            ui.heading(tr("Unsaved edits"));
            ui.label(tr("This track has edits that aren't saved. The original file is never changed."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = ui.button(tr("Save As…")).clicked();
                discard = ui.button(tr("Discard edits")).clicked();
                cancel = ui.button(tr("Cancel")).clicked();
            });
        });
        if save {
            self.show_save = true;
        } else if cancel || modal.should_close() {
            self.pending = None;
        } else if discard {
            self.history = None;
            self.run_pending(ctx);
        }
    }

    /// Goes where the unsaved-edits prompt was holding up.
    fn run_pending(&mut self, ctx: &egui::Context) {
        match self.pending.take() {
            Some(Pending::Go(index)) => self.go(index),
            Some(Pending::Open(path)) => self.open(path),
            Some(Pending::Quit) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }

    /// Save Copy has something to write: unsaved edits on a fully decoded track.
    fn can_save_copy(&self) -> bool {
        self.dirty() && self.track.done() && self.save_job.is_none()
    }

    /// Saves the edits next to the original under a new name, with no questions (as lightview does).
    fn save_copy(&mut self) {
        if !self.can_save_copy() {
            return;
        }
        let (codec, bits) = self.track.info.get().map_or((String::new(), None), |i| (i.codec.clone(), i.bits));
        let choice = save::copy_choice(&self.track.path, &codec, bits, self.save_choice);
        match save::reserve_copy(&self.track.path, choice.kind.ext()) {
            Ok(target) => self.start_save(choice, target, true),
            Err(e) => self.save_notice = Some((Err(e), self.ctx.get().map_or(0.0, |c| c.input(|i| i.time)))),
        }
    }

    /// Saves the current edits to `target` on its own thread; `reserved` names a file Save Copy
    /// claimed, removed again if the save fails.
    fn start_save(&mut self, choice: save::Choice, target: PathBuf, reserved: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (track, edit, notify) = (self.track.clone(), self.edit(), repaint(&self.ctx));
        let (written, path) = (edit.clone(), target.clone());
        let draft = self.tag_editor.as_ref().and_then(|e| e.changes()).cloned();
        std::thread::spawn(move || {
            track::background();
            let result = save::save(&track, &written, choice, &path, draft.as_ref());
            if result.is_err() && reserved {
                let _ = std::fs::remove_file(&path);
            }
            let _ = tx.send(result);
            notify();
        });
        self.save_job = Some((rx, edit, target, reserved));
    }

    /// Picks up the running save's result: marks the edits saved and, if the unsaved-edits
    /// prompt was waiting, carries on where it was going.
    fn follow_save(&mut self, ctx: &egui::Context) {
        let Some((rx, edit, target, reserved)) = &self.save_job else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            // The save thread died (a panic) without a word.
            Err(_) => {
                if *reserved {
                    let _ = std::fs::remove_file(target);
                }
                Err(tr("Saving failed").into())
            }
        };
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if result.is_ok() && let Some(h) = &mut self.history {
            h.mark_saved(edit);
        }
        let ok = result.is_ok();
        let message = result.map(|tags| if tags { trf("Saved {name}", &[("name", &name)]) } else { trf("Saved {name}, but the tags could not be copied", &[("name", &name)]) });
        (self.save_job, self.save_notice) = (None, Some((message, ctx.input(|i| i.time))));
        if ok && self.pending.is_some() {
            self.show_save = false;
            self.run_pending(ctx);
        }
    }

    /// No text field has focus and no dialog waits for an answer, so plain keys are shortcuts.
    fn no_dialog(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.focused().is_none()) && self.custom_speed.is_none() && self.custom_fx.is_none() && self.pending.is_none()
    }

    /// The player's keys in the window `ctx` is drawing (the main one or the Spectrogram):
    /// Space, PageUp/PageDown, ←/→, and Esc and L.
    fn player_keys(&mut self, ctx: &egui::Context, pos: u64, len: u64, rate: u64, actions: &mut Vec<Action>) {
        let key = |k| ctx.input(|i| i.key_pressed(k));
        // A focused button already takes Space as its own click; toggling here too would undo it.
        if key(egui::Key::Space) && ctx.memory(|m| m.focused().is_none()) {
            actions.push(Action::Toggle);
        }
        for (k, action) in [(egui::Key::PageUp, Action::Previous), (egui::Key::PageDown, Action::Next)] {
            if key(k) {
                actions.push(action);
            }
        }
        let step = STEP_SECONDS * rate;
        if key(egui::Key::ArrowLeft) {
            actions.push(Action::Seek(pos.saturating_sub(step)));
        }
        if key(egui::Key::ArrowRight) {
            actions.push(Action::Seek((pos + step).min(len)));
        }
        if self.no_dialog(ctx) {
            let plain = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
            if plain(egui::Key::Escape) {
                // The selection first, then full screen (this window's).
                if self.edit_view.selection.take().is_none() && ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
            }
            if plain(egui::Key::L) && self.settings.practice {
                self.looping = !self.looping;
            }
        }
    }

    /// Practice → Spectrogram: its own window, closed with its ✕ or the menu. Returns what its keys
    /// and clicks asked for.
    fn spectrum_window(&mut self, ctx: &egui::Context, edit: &Arc<Edit>, pos: u64, len: u64, rate: u64, playing: bool) -> Vec<Action> {
        // Hidden, not closed, with no file open.
        if !self.show_spectrum || !self.has_track() {
            return Vec::new();
        }
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy();
        let builder = egui::ViewportBuilder::default().with_title(trf("Spectrogram — {name}", &[("name", &name)])).with_inner_size([900.0, 400.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("spectrum"), builder, |ui, _| {
            if ui.input(|i| i.viewport().close_requested()) {
                self.show_spectrum = false;
            }
            let mut actions = Vec::new();
            let ctx = ui.ctx().clone();
            egui::Panel::top("spectrum-bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let notes = ui.selectable_label(self.settings.spectrum_notes, tr("Notes")).on_hover_text(tr("Show or hide the note lines"));
                    if notes.clicked() {
                        self.settings.spectrum_notes = !self.settings.spectrum_notes;
                    }
                    let chords = ui.selectable_label(self.settings.spectrum_chords, tr("Chords")).on_hover_text(tr("Show or hide the chords along the top"));
                    if chords.clicked() {
                        self.settings.spectrum_chords = !self.settings.spectrum_chords;
                    }
                    if let Some(key) = self.track.tempo.get().and_then(|e| e.key) {
                        let scale = tempo::major_scale(key);
                        let major = spectrum::KEYS[scale].0;
                        let why = if key.is_minor() { trf("Show the notes of {major} major, which {key} shares", &[("major", &major), ("key", &key.name())]) } else { trf("Show the notes of {major} major", &[("major", &major)]) };
                        if ui.selectable_label(self.settings.note_scale == Some(scale), trf("Key ≈ {key}", &[("key", &key.name())])).on_hover_text(why).clicked() {
                            (self.settings.note_scale, self.settings.spectrum_notes) = (Some(scale), true);
                        }
                    }
                    if let Some(hop) = self.spectrum.hop().filter(|h| *h != self.settings.spectrum.hop as u64) {
                        ui.weak(trf("hop {hop} for this length", &[("hop", &hop)])).on_hover_text(tr("A long track: the hop is raised so the spectrogram fits in memory"));
                    }
                    if ui.button(tr("Fit")).on_hover_text(tr("Show the whole track and all frequencies")).clicked() {
                        (self.spectrum_view.start, self.spectrum_view.frames, self.spectrum.band) = (0, 0, [0.0, 1.0]);
                    }
                    if ui.selectable_label(self.show_spectrum_settings, tr("Settings")).clicked() {
                        self.show_spectrum_settings = !self.show_spectrum_settings;
                    }
                });
            });
            let mut open = self.show_spectrum_settings;
            egui::Window::new(tr("Spectrogram settings")).open(&mut open).collapsible(false).resizable(false).show(&ctx, |ui| spectrum_settings(ui, &mut self.settings, rate));
            self.show_spectrum_settings = open;
            let seek = egui::CentralPanel::default()
                .show(ui, |ui| spectrum_panel(ui, &mut self.spectrum_view, &mut self.edit_view.selection, &mut self.spectrum, &self.settings, &self.track, edit, pos, len, rate, playing))
                .inner;
            actions.extend(seek.map(Action::Seek));
            self.player_keys(&ctx, pos, len, rate, &mut actions);
            actions
        })
    }

    /// Save As: the format and quality, then the system's save dialog; saves on its own thread.
    fn save_window(&mut self, ctx: &egui::Context) {
        if !self.show_save {
            return;
        }
        let (mut open, mut pick) = (true, false);
        let busy = self.save_job.is_some();
        egui::Window::new(tr("Save As")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            let rate = self.track.format.get().map_or(0, |f| f.rate);
            ui.add_enabled_ui(!busy, |ui| {
                save_options(ui, &mut self.save_choice, rate);
                if self.speed != wsola::Speed::NORMAL {
                    let mut apply = self.save_choice.speed.is_some();
                    ui.checkbox(&mut apply, tr("Apply speed and pitch"));
                    self.save_choice.speed = apply.then_some(self.speed);
                }
            });
            ui.weak(tr("The original file is never changed."));
            ui.add_space(4.0);
            if let Some((_, _, target, _)) = &self.save_job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(trf("Saving {name}…", &[("name", &target.file_name().unwrap_or_default().to_string_lossy())]));
                });
            } else {
                notice(ui, self.save_notice.as_ref(), None);
                pick = ui.button(tr("Save…")).clicked();
            }
        });
        if pick {
            let kind = self.save_choice.kind;
            let mut picker = rfd::FileDialog::new().add_filter(kind.name(), &[kind.ext()]).set_file_name(save::suggested_name(&self.track.path, kind));
            if let Some(dir) = self.track.path.parent() {
                picker = picker.set_directory(dir);
            }
            // The system dialog asks before replacing an existing file.
            if let Some(target) = picker.save_file() {
                // The settings as they are now, and only while they aren't Normal (the box is hidden then).
                let speed = self.save_choice.speed.filter(|_| self.speed != wsola::Speed::NORMAL).map(|_| self.speed);
                self.start_save(save::Choice { speed, ..self.save_choice }, target, false);
            }
        }
        // Closing it while saving only hides it; the save still finishes.
        if !open {
            self.show_save = false;
        }
    }

    /// The current edits, or the whole track when there are none.
    fn edit(&self) -> Arc<Edit> {
        self.history.as_ref().map_or_else(|| Arc::new(Edit::whole(self.track.len())), |h| h.current().clone())
    }

    fn previous(&mut self) {
        let Ok(p) = &self.player else { return };
        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        match self.neighbour(true) {
            Some(index) if p.shared.pos.load(Relaxed) <= RESTART_SECONDS * rate => self.go(index),
            _ => p.send(Cmd::Seek(0)),
        }
    }

    /// Shows the file name, length and format in the title bar; the length once it is exact.
    fn set_title(&mut self, ctx: &egui::Context) {
        let path = &self.track.path;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let format = path.extension().unwrap_or_default().to_string_lossy().to_uppercase();
        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        let length = (self.track.done() && rate > 0).then(|| clock(self.track.len(), rate));
        let mark = if self.dirty() { "• " } else { "" };
        let text = if self.has_track() { format!("{mark}{}", title(&name, length.as_deref(), &format)) } else { "soundglass".into() };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(text));
        self.titled = Some((Arc::as_ptr(&self.track) as usize, self.track.done(), self.dirty()));
    }

    /// "Write the tags into …?"; Write starts writing them on its own thread.
    fn tags_confirm(&mut self, ctx: &egui::Context) {
        let Some(editor) = self.tag_editor.as_mut().filter(|e| e.confirm) else { return };
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (mut write, mut cancel) = (false, false);
        let modal = egui::Modal::new(egui::Id::unique("write tags")).show(ctx, |ui| {
            ui.heading(tr("Write tags"));
            ui.label(trf("Write the tags into {name}? The audio isn't changed.", &[("name", &name)]));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                write = ui.button(tr("Write")).clicked();
                cancel = ui.button(tr("Cancel")).clicked();
            });
        });
        if write {
            let (tx, rx) = std::sync::mpsc::channel();
            let (path, draft, notify) = (self.track.path.clone(), editor.draft.clone(), repaint(&self.ctx));
            std::thread::spawn(move || {
                let _ = tx.send(tags::write_in_place(&path, &draft).map(|()| tr("Tags saved").into()));
                notify();
            });
            (editor.confirm, self.tag_job) = (false, Some(rx));
        } else if cancel || modal.should_close() {
            editor.confirm = false;
        }
    }

    /// Asks about `image` as the cover of this track or its folder's tracks; `manual` when picked
    /// from the Edit menu (this track or all), else the folder's lone image (all or not now).
    fn offer_cover(&mut self, ctx: &egui::Context, image: PathBuf, manual: bool) -> Result<(), String> {
        let (cover, thumbnail) = read_cover(&image)?;
        self.show_cover_offer(ctx, (image, cover, thumbnail), manual);
        Ok(())
    }

    fn show_cover_offer(&mut self, ctx: &egui::Context, (image, cover, thumbnail): FoundCover, manual: bool) {
        let texture = ctx.load_texture("offered cover", thumbnail, egui::TextureOptions::LINEAR);
        self.cover_offer = Some(CoverOffer { image, cover, files: folder::list_folder(&self.track.path), texture: Some(texture), manual });
    }

    /// "Add album cover?" with the image, asked when a track without a cover opens
    /// in a folder with exactly one image (`folder::IMAGE_EXTS`), or from the Edit menu's cover items; the chosen
    /// tracks are written on a thread, as the tag job.
    fn cover_offer_modal(&mut self, ctx: &egui::Context) {
        let key = Arc::as_ptr(&self.track) as usize;
        if key != self.cover_checked && self.has_track() && self.tag_editor.is_none() && self.tag_job.is_none() {
            self.cover_checked = key;
            let dir = self.track.path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            self.cover_found = None;
            if !self.cover_asked.contains(&dir) {
                // The file's tags, the folder and the image (up to 10 MB) are read off the UI thread.
                let (tx, rx) = std::sync::mpsc::channel();
                let (path, notify) = (self.track.path.clone(), repaint(&self.ctx));
                std::thread::spawn(move || {
                    track::background();
                    // An unreadable image just isn't offered.
                    let found = (tags::has_cover(&path) == Ok(false)).then(|| folder::lone_image(&dir)).flatten().and_then(|image| Some((image.clone(), read_cover(&image).ok()?)));
                    if let Some((image, (cover, thumbnail))) = found {
                        let _ = tx.send((image, cover, thumbnail));
                        notify();
                    }
                });
                self.cover_found = Some(rx);
            }
        }
        // Only for the track it was found for, and not over the tag editor or another offer.
        if key == self.cover_checked && self.tag_editor.is_none() && self.tag_job.is_none() && self.cover_offer.is_none()
            && let Some(found) = self.cover_found.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            self.cover_found = None;
            self.show_cover_offer(ctx, found, false);
        }
        let Some(CoverOffer { image, cover, files, texture, manual }) = &self.cover_offer else { return };
        let remove = matches!(cover, tags::Cover::Remove);
        let name = image.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let n = files.len();
        let what = if n == 1 { tr("this track").to_string() } else { trf("all {n} tracks in this folder", &[("n", &n)]) };
        let (mut yes, mut one, mut no) = (false, false, false);
        let modal = egui::Modal::new(egui::Id::unique("folder cover")).show(ctx, |ui| {
            ui.set_max_width(320.0);
            ui.heading(if remove { tr("Remove album cover") } else if *manual { tr("Album cover") } else { tr("Add album cover?") });
            ui.add_space(4.0);
            if let Some(texture) = texture {
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::from_texture(texture).max_size(egui::vec2(256.0, 256.0)).corner_radius(8.0).alt_text(if remove { tr("Current cover") } else { &name }));
                });
                ui.add_space(4.0);
            }
            if remove && n == 1 {
                ui.label(tr("Remove the cover from this track? The music itself isn't changed."));
            } else if remove {
                ui.label(trf("Remove the cover from this track only, or from {what}? The music itself isn't changed.", &[("what", &what)]));
            } else if !*manual {
                ui.label(trf("This track has no cover. Save {name} as the cover of {what}?", &[("name", &name), ("what", &what)]));
                ui.label(tr("Tracks that already have a cover get this one instead. The music itself isn't changed."));
            } else if n == 1 {
                ui.label(trf("Save {name} as the cover of this track? Any cover it has now is replaced. The music itself isn't changed.", &[("name", &name)]));
            } else {
                ui.label(trf("Save {name} as the cover of this track, or of {what}? Any cover they have now is replaced. The music itself isn't changed.", &[("name", &name), ("what", &what)]));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| match (*manual, n) {
                (false, _) => {
                    yes = ui.button(tr("Add Cover")).clicked();
                    no = ui.button(tr("Not Now")).clicked();
                }
                (true, 1) => {
                    yes = ui.button(if remove { tr("Remove Cover") } else { tr("Save Cover") }).clicked();
                    no = ui.button(tr("Cancel")).clicked();
                }
                (true, _) => {
                    yes = ui.button(trf("All {n} Tracks", &[("n", &n)])).clicked();
                    one = ui.button(tr("This Track")).clicked();
                    no = ui.button(tr("Cancel")).clicked();
                }
            });
        });
        if !(yes || one || no || modal.should_close()) {
            return;
        }
        let Some(CoverOffer { image, cover, mut files, manual, .. }) = self.cover_offer.take() else { return };
        if !manual {
            self.cover_asked.insert(image.parent().map(|d| d.to_path_buf()).unwrap_or_default());
        }
        if one {
            files = vec![self.track.path.clone()];
        }
        if yes || one {
            let (tx, rx) = std::sync::mpsc::channel();
            let notify = repaint(&self.ctx);
            let removed = matches!(cover, tags::Cover::Remove);
            std::thread::spawn(move || {
                let failed: Vec<String> = files.iter().filter(|f| tags::write_cover(f, &cover).is_err()).map(|f| f.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
                let done = files.len() - failed.len();
                let _ = tx.send(match (done, failed.is_empty()) {
                    (0, _) => Err(trf("The cover couldn't be written ({failed})", &[("failed", &failed.join(", "))])),
                    (1, true) if removed => Ok(tr("Cover removed from 1 track").into()),
                    (1, true) => Ok(tr("Cover added to 1 track").into()),
                    (n, true) if removed => Ok(trf("Cover removed from {n} tracks", &[("n", &n)])),
                    (n, true) => Ok(trf("Cover added to {n} tracks", &[("n", &n)])),
                    (n, false) => {
                        let args: [(&str, &dyn std::fmt::Display); 3] = [("n", &n), ("total", &files.len()), ("failed", &failed.join(", "))];
                        Ok(if removed { trf("Cover removed from {n} of {total} tracks (not: {failed})", &args) } else { trf("Cover added to {n} of {total} tracks (not: {failed})", &args) })
                    }
                });
                notify();
            });
            self.tag_job = Some(rx);
        }
    }

    /// After Find Beats: writes the found tempo as the file's BPM tag, unless it has one.
    fn tag_bpm(&mut self, beats: &ai::Beats) {
        // shortcut: skipped while the tag editor is open or tags are being written; queue it if that's missed.
        let Some(rate) = self.track.format.get().map(|f| f.rate) else { return };
        let Some(bpm) = ai::bpm(&beats.beats, rate) else { return };
        if self.tag_editor.is_some() || self.tag_job.is_some() || !tags::read(&self.track.path).is_ok_and(|t| t.bpm.is_empty()) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (path, notify) = (self.track.path.clone(), repaint(&self.ctx));
        std::thread::spawn(move || {
            let _ = tx.send(tags::write_bpm(&path, bpm).map(|()| trf("BPM tag saved ({bpm})", &[("bpm", &bpm)])));
            notify();
        });
        self.tag_job = Some(rx);
    }

    /// Picks up the tag write: on success the track is read afresh under its tags, playing on.
    fn follow_tags(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.tag_job else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(_) => Err(tr("Writing the tags failed").into()),
        };
        self.tag_job = None;
        self.own_stamp = folder::stamp(&self.track.path);
        let result = result.and_then(|notice| self.relabel(self.track.path.clone()).map(|()| notice));
        match result {
            Ok(notice) => {
                self.tag_editor = None;
                // A track decoded ahead would show its old cover.
                self.next = None;
                self.save_notice = Some((Ok(notice), ctx.input(|i| i.time)));
            }
            Err(e) => match &mut self.tag_editor {
                Some(editor) => editor.error = Some(e),
                None => self.save_notice = Some((Err(e), ctx.input(|i| i.time))),
            },
        }
    }

    /// Plays on with the same audio under `path`'s tags and name.
    fn relabel(&mut self, path: PathBuf) -> Result<(), String> {
        let track = self.track.relabel(path, repaint(&self.ctx))?;
        if let Ok(p) = &self.player {
            p.send(Cmd::Swap(track.clone()));
        }
        self.track = track;
        Ok(())
    }

    /// File → Rename from Tags…: the pattern, a preview of the name, Rename and Cancel.
    /// File → Identify Track…: Look up, pick a match, and Use Match fills the tag editor
    /// (nothing is written here). True when the tag editor was filled.
    fn identify_window(&mut self, ctx: &egui::Context) -> bool {
        let Some(id) = &mut self.identify else { return false };
        if id.path != self.track.path {
            self.identify = None;
            return false;
        }
        let mut genre = None;
        if let Some(rx) = &id.job {
            match rx.try_recv() {
                Ok(IdentifyJob::Found(found)) => (id.found, id.pick, id.job) = (Some(found), 0, None),
                Ok(IdentifyJob::Genre(g)) => (genre, id.job) = (Some(g), None),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => (id.found, id.job) = (Some(Err(tr("The lookup stopped.").into())), None),
            }
        }
        let busy = id.job.is_some();
        let saving = self.tag_job.is_some();
        let (mut look, mut take, mut close) = (false, false, false);
        egui::Window::new(tr("Identify Track")).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(tr("Sends this file's fingerprint and length to AcoustID, and the chosen match's ID to MusicBrainz. Nothing else leaves this computer."));
            ui.add_space(6.0);
            let mut found = 0;
            match &id.found {
                None => {}
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(m)) if m.is_empty() => {
                    ui.label(tr("No match found."));
                }
                Some(Ok(matches)) => {
                    found = matches.len();
                    for (i, m) in matches.iter().enumerate() {
                        ui.radio_value(&mut id.pick, i, format!("{:.0} %  {} — {}", m.score * 100.0, m.title, if m.artist.is_empty() { "?" } else { &m.artist }));
                        let release = [m.album.clone(), m.year.clone(), if m.track.is_empty() { String::new() } else { trf("track {n}", &[("n", &m.track)]) }];
                        let release: Vec<String> = release.into_iter().filter(|s| !s.is_empty()).collect();
                        if !release.is_empty() {
                            ui.indent(i, |ui| ui.weak(release.join(", ")));
                        }
                    }
                }
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if id.found.is_some() { tr("Fetching the genre…") } else { tr("Looking up…") });
                });
            }
            ui.checkbox(&mut id.only_empty, tr("Only fill empty tags"));
            ui.horizontal(|ui| {
                look = ui.add_enabled(!busy, egui::Button::new(tr("Look up"))).clicked();
                let item = ui.add_enabled(!busy && !saving && found > 0, egui::Button::new(tr("Use Match"))).on_hover_text(tr("Fills Track Info's tag editor; Save… there writes the tags"));
                take = item.on_disabled_hover_text(if saving { tr("saving tags…") } else { "" }).clicked();
                close = ui.button(tr("Close")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if look {
            let (tx, rx) = std::sync::mpsc::channel();
            let (track, ctx, key) = (self.track.clone(), ctx.clone(), self.settings.acoustid_key.clone());
            std::thread::spawn(move || {
                track::background();
                let found = identify::fingerprint(&track).and_then(|(fp, secs)| identify::lookup(&key, &fp, secs));
                let _ = tx.send(IdentifyJob::Found(found));
                ctx.request_repaint();
            });
            (id.found, id.job) = (None, Some(rx));
        }
        let picked = || match &id.found {
            Some(Ok(m)) => m.get(id.pick).cloned(),
            _ => None,
        };
        if take && let Some(m) = picked() {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(IdentifyJob::Genre(identify::genre(&m.recording)));
                ctx.request_repaint();
            });
            id.job = Some(rx);
        }
        if close {
            self.identify = None;
            return false;
        }
        let (Some(genre), Some(m)) = (genre, picked()) else { return false };
        let only_empty = id.only_empty;
        // Over the tag editor's own unsaved edits, if it's open: they aren't dropped.
        let base = match &self.tag_editor {
            Some(e) => Ok((e.read.clone(), e.draft.clone())),
            None => tags::read(&self.track.path).map(|t| (t.clone(), t)),
        };
        match base {
            Ok((read, base)) => {
                // Without a genre (MusicBrainz unreachable, or none voted) the rest is still filled.
                let draft = identify::fill(&base, &m, genre.ok().flatten().as_deref(), only_empty);
                self.tag_editor = Some(TagEditor { read, draft, confirm: false, error: None });
                self.identify = None;
                true
            }
            Err(e) => {
                self.save_notice = Some((Err(e), ctx.input(|i| i.time)));
                false
            }
        }
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some((pattern, read)) = &mut self.rename else { return };
        let path = self.track.path.clone();
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let (mut go, mut close) = (None, false);
        egui::Window::new(tr("Rename from Tags")).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(pattern).desired_width(320.0));
            ui.weak(tags::PLACEHOLDERS.map(|p| format!("{{{p}}}")).join(" "));
            ui.add_space(4.0);
            let name = read.as_ref().map(|t| tags::file_name(pattern, t, &ext));
            let why: Option<String> = match &name {
                Err(e) => Some(e.to_string()),
                Ok(n) if n.is_empty() => Some(tr("None of these tags is set.").into()),
                Ok(n) if path.file_name().is_some_and(|f| f.to_string_lossy() == *n) => Some(tr("It already has that name.").into()),
                Ok(n) if path.with_file_name(n).exists() => Some(tr("A file with that name already exists.").into()),
                Ok(_) => None,
            };
            if let Ok(n) = &name && !n.is_empty() {
                ui.label(trf("→ {n}", &[("n", &n)]));
            }
            if let Some(why) = &why {
                ui.colored_label(ui.visuals().warn_fg_color, why.as_str());
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(why.is_none(), egui::Button::new(tr("Rename"))).clicked() {
                    go = name.ok();
                }
                if ui.button(tr("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        if let Some(name) = go {
            let pattern = pattern.clone();
            match self.rename_track(&name, ctx) {
                Ok(()) => {
                    self.settings.rename_pattern = pattern;
                    close = true;
                }
                Err(e) => self.save_notice = Some((Err(e), ctx.input(|i| i.time))),
            }
        }
        if close {
            self.rename = None;
        }
    }

    /// Renames the current file to `name` in its folder, never replacing another file; playback
    /// and edits carry on, and the folder, Open Recent and the Folder panel follow.
    fn rename_track(&mut self, name: &str, ctx: &egui::Context) -> Result<(), String> {
        let path = self.track.path.clone();
        let new = tags::rename(&path, name)?;
        self.relabel(new.clone())?;
        self.files = folder::list_folder(&new);
        self.index = self.files.iter().position(|p| *p == new).unwrap_or(0);
        if let Some(b) = &mut self.browse {
            *b = Browse::new(b.dir.clone());
        }
        for p in &mut self.recent {
            if *p == path {
                *p = new.clone();
            }
        }
        self.remember(&new);
        self.save_notice = Some((Ok(trf("Renamed to {name}", &[("name", &name)])), ctx.input(|i| i.time)));
        Ok(())
    }

    /// "Move to the Trash?"; Continue (or Enter) moves it and goes on to the next file, stopped.
    fn delete_window(&mut self, ctx: &egui::Context) {
        if !self.delete_prompt {
            return;
        }
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dirty = self.dirty();
        let modal = egui::Modal::new(egui::Id::unique("delete")).show(ctx, |ui| {
            ui.heading(tr("Delete file?"));
            ui.label(trf("{name} will be moved to the Trash.", &[("name", &name)]));
            if dirty {
                ui.label(tr("Its unsaved edits will be lost."));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (accept, cancel) = (ui.button(tr("Continue")), ui.button(tr("Cancel")));
                if !cancel.has_focus() {
                    accept.request_focus();
                }
                (accept.clicked(), cancel.clicked())
            }).inner
        });
        let (accept, cancel) = modal.inner;
        if accept {
            self.delete_track(ctx);
        }
        if accept || cancel || modal.should_close() {
            self.delete_prompt = false;
        }
    }

    /// Moves the current file to the Trash, then shows the next file in the folder (the previous one
    /// at its end), stopped, or no file when it was the last.
    fn delete_track(&mut self, ctx: &egui::Context) {
        let path = self.track.path.clone();
        if let Err(e) = trash::delete(&path) {
            self.save_notice = Some((Err(trf("Couldn't delete the file: {e}", &[("e", &e)])), ctx.input(|i| i.time)));
            return;
        }
        self.files.retain(|p| *p != path);
        self.recent.retain(|p| *p != path);
        if let Some(b) = &mut self.browse {
            *b = Browse::new(b.dir.clone());
        }
        self.deleted.push(path);
        self.history = None;
        self.next = None;
        let Ok(p) = &self.player else { return };
        if self.files.is_empty() {
            (self.track, self.index) = (no_track(), 0);
            p.send(Cmd::Load(self.track.clone()));
            self.forget_edits();
        } else {
            self.go(self.index.min(self.files.len() - 1));
        }
        if let Ok(p) = &self.player {
            p.send(Cmd::Pause);
        }
        self.save_notice = Some((Ok(tr("Moved to the Trash").into()), ctx.input(|i| i.time)));
    }

    /// Puts the last file moved to the Trash back and opens it.
    fn restore_deleted(&mut self) {
        let Some(path) = self.deleted.pop() else { return };
        let time = self.ctx.get().map_or(0.0, |c| c.input(|i| i.time));
        match folder::restore_trashed(&path) {
            Ok(()) => self.open(path),
            Err(e) => self.save_notice = Some((Err(trf("Couldn't bring back {name}: {e}", &[("name", &path.file_name().unwrap_or_default().to_string_lossy()), ("e", &e)])), time)),
        }
    }

    /// The shortcuts pressed this frame. Keys without Ctrl, Alt or Shift only when no text field or
    /// dialog wants them, and Undo and Redo belong to a text field while one has focus.
    fn read_shortcuts(&self, ctx: &egui::Context, keys: &[(egui::KeyboardShortcut, Command)]) -> Vec<Command> {
        let typing = ctx.egui_wants_keyboard_input();
        let plain_ok = !typing && self.custom_speed.is_none() && self.custom_fx.is_none() && self.pending.is_none() && ctx.memory(|m| m.top_modal_layer().is_none());
        ctx.input_mut(|i| {
            keys.iter()
                .filter(|(sc, c)| (plain_ok || !sc.modifiers.is_none()) && !(typing && matches!(c, Command::Undo | Command::Redo)) && i.consume_shortcut(sc))
                .map(|(_, c)| *c)
                .collect()
        })
    }

    /// Change Keyboard Shortcuts…'s Change: the next key press, with its modifiers, becomes
    /// `recording`'s shortcut unless it's taken; Esc cancels. Nothing else sees the key. From lightview.
    fn record_shortcut(&mut self, ctx: &egui::Context) {
        let Some(command) = self.recording else { return };
        let pressed = ctx.input_mut(|i| {
            let pressed = i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some(egui::KeyboardShortcut::new(*modifiers, *key)),
                _ => None,
            });
            i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) | egui::Event::Text(_)));
            pressed
        });
        let Some(sc) = pressed else { return };
        self.recording = None;
        if sc == egui::KeyboardShortcut::new(egui::Modifiers::NONE, egui::Key::Escape) {
            return;
        }
        match keys::taken(&keys::shortcuts(&self.settings.shortcuts), sc, command) {
            Some(other) => self.shortcut_error = Some(trf("{keys} is already used by {other}", &[("keys", &ctx.format_shortcut(&sc)), ("other", &other)])),
            None => self.set_shortcut(command, Some(sc)),
        }
    }

    /// Gives `command` the shortcut `sc`, or none.
    fn set_shortcut(&mut self, command: Command, sc: Option<egui::KeyboardShortcut>) {
        let id = keys::id(command);
        self.settings.shortcuts.retain(|(n, _)| *n != id);
        self.settings.shortcuts.push((id, sc));
        self.shortcut_error = None;
    }

    /// Keyboard Shortcuts, over Settings: every menu command by menu, its keys, Change and Remove. From lightview.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let height = ctx.content_rect().height() * 0.6;
        let modal = egui::Modal::new(egui::Id::unique("shortcuts")).show(ctx, |ui| {
            ui.heading(tr("Keyboard Shortcuts"));
            ui.weak(tr("Space, ←, →, Page Up, Page Down, L (Loop selection), Esc and Enter are fixed."));
            if let Some(e) = &self.shortcut_error {
                ui.colored_label(ui.visuals().warn_fg_color, e);
            }
            let keys = keys::shortcuts(&self.settings.shortcuts);
            egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                for (menu, items) in keys::bindable_menus() {
                    ui.add_space(6.0);
                    ui.strong(tr(menu));
                    egui::Grid::new(("shortcuts", menu)).num_columns(3).striped(true).min_col_width(120.0).show(ui, |ui| {
                        for (command, name) in items {
                            ui.label(name);
                            let current: Vec<String> = keys.iter().filter(|(_, c)| *c == command).map(|(sc, _)| ui.ctx().format_shortcut(sc)).collect();
                            if self.recording == Some(command) {
                                ui.strong(tr("Press keys… (Esc to cancel)"));
                            } else {
                                ui.label(current.join(", "));
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button(tr("Change")).clicked() {
                                    (self.recording, self.shortcut_error) = (Some(command), None);
                                }
                                if !current.is_empty() && ui.small_button(tr("Remove")).clicked() {
                                    self.set_shortcut(command, None);
                                }
                            });
                            ui.end_row();
                        }
                    });
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Reset All Shortcuts")).clicked() {
                    (self.settings.shortcuts, self.recording, self.shortcut_error) = (Vec::new(), None, None);
                }
                ui.button(tr("Close")).clicked()
            })
            .inner
        });
        if modal.inner || modal.should_close() {
            (self.show_shortcuts, self.recording, self.shortcut_error) = (false, None, None);
        }
    }

    /// First start, when soundglass isn't the default player yet: a welcome with "Make soundglass my
    /// default player" ticked. Asked once; already the default, nothing is asked.
    fn default_window(&mut self, ctx: &egui::Context) {
        if let Some(is_default) = self.default_check.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.default_check = None;
            if is_default {
                self.settings.default_asked = true;
            } else {
                self.default_prompt = Some(true);
            }
        }
        let Some(mut tick) = self.default_prompt else { return };
        let modal = egui::Modal::new(egui::Id::unique("default-player")).show(ctx, |ui| {
            ui.heading(tr("Welcome to soundglass"));
            ui.add_space(8.0);
            ui.checkbox(&mut tick, tr("Make soundglass my default player"));
            ui.add_space(8.0);
            ui.button(tr("OK")).clicked()
        });
        self.default_prompt = Some(tick);
        if modal.inner || modal.should_close() {
            (self.default_prompt, self.settings.default_asked) = (None, true);
            if modal.inner && tick {
                let result = folder::make_default();
                // Settings is where a failure is shown.
                self.show_settings = result.is_err();
                self.default_result = Some(result);
            }
        }
    }

    /// Points the watcher at the current file and picks up its changes: one made by another program
    /// asks whether to reload. Not while soundglass is writing the tags itself.
    fn follow_changes(&mut self) {
        if let Ok(mut w) = self.watched.lock() {
            let now = self.has_track().then(|| self.track.path.clone());
            if *w != now {
                *w = now;
            }
        }
        while let Ok((path, stamp)) = self.changes.try_recv() {
            if path == self.track.path && self.tag_job.is_none() && self.own_stamp != Some(stamp) {
                self.changed_prompt = true;
            }
        }
    }

    /// "Changed on disk. Reload it?"; Reload reads it again at the same place, dropping the edits.
    fn changed_window(&mut self, ctx: &egui::Context, pos: u64) {
        if !self.changed_prompt {
            return;
        }
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dirty = self.dirty();
        let modal = egui::Modal::new(egui::Id::unique("changed")).show(ctx, |ui| {
            ui.heading(tr("File changed"));
            ui.label(trf("{name} was changed by another program. Reload it?", &[("name", &name)]));
            if dirty {
                ui.label(tr("Its unsaved edits will be lost."));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| (ui.button(tr("Reload")).clicked(), ui.button(tr("Keep current")).clicked())).inner
        });
        let (reload, keep) = modal.inner;
        if reload {
            self.reload(pos);
        }
        if reload || keep || modal.should_close() {
            self.changed_prompt = false;
        }
    }

    /// Reads the current file again and goes on from `pos`, playing or not as before.
    fn reload(&mut self, pos: u64) {
        let Ok(p) = &self.player else { return };
        let track = Track::load(self.track.path.clone(), repaint(&self.ctx));
        p.send(Cmd::Load(track.clone()));
        p.send(Cmd::Seek(pos));
        // Repeat track queued the old version.
        (self.track, self.next) = (track, None);
        self.forget_edits();
    }

    /// A file dropped on the window plays (asking first about unsaved edits); a folder plays its first file.
    fn open_dropped(&mut self, path: PathBuf, ctx: &egui::Context) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let found = if path.is_dir() {
            folder::list_dir(&path).ok().and_then(|(_, files)| files.into_iter().next()).ok_or(trf("No audio files in {name}", &[("name", &name)]))
        } else if folder::is_audio(&path) {
            Ok(path)
        } else {
            Err(trf("{name} isn't an audio file soundglass plays", &[("name", &name)]))
        };
        match found {
            Ok(path) => self.open(path),
            Err(e) => self.save_notice = Some((Err(e), ctx.input(|i| i.time))),
        }
    }

    fn start_rename(&mut self) {
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.rename_file = Some((name, true, None));
    }

    /// File → Rename…: the name with only its stem selected; Enter or Rename applies it. From lightview.
    fn rename_file_window(&mut self, ctx: &egui::Context) {
        use egui::text::{CCursor, CCursorRange};
        let Some((mut name, fresh, error)) = self.rename_file.take() else { return };
        let modal = egui::Modal::new(egui::Id::unique("rename-file")).show(ctx, |ui| {
            ui.heading(tr("Rename"));
            ui.add_space(6.0);
            let mut edit = egui::TextEdit::singleline(&mut name).desired_width(320.0).show(ui);
            if fresh {
                edit.response.request_focus();
                let stem = name.rsplit_once('.').map_or(&name[..], |(stem, _)| stem).chars().count();
                edit.state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(stem))));
                edit.state.store(ui.ctx(), edit.response.id);
            }
            let enter = edit.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(e) = &error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.add_space(6.0);
            let (rename, cancel) = ui.horizontal(|ui| (ui.button(tr("Rename")).clicked(), ui.button(tr("Cancel")).clicked())).inner;
            (enter || rename, cancel)
        });
        let (accept, cancel) = modal.inner;
        if cancel || modal.should_close() {
            return;
        }
        let unchanged = self.track.path.file_name().is_some_and(|f| f.to_string_lossy() == name);
        if !accept {
            self.rename_file = Some((name, false, error));
        } else if !unchanged && let Err(e) = valid_file_name(&name).and_then(|()| self.rename_track(&name, ctx)) {
            self.rename_file = Some((name, true, Some(e)));
        }
    }

    /// Puts the current file on the clipboard as a file. From lightview.
    // shortcut: GNOME Files may want x-special/gnome-copied-files, which arboard doesn't offer; add it if that's reported.
    fn copy_file(&mut self, ctx: &egui::Context) {
        let clipboard = match self.file_clipboard.take() {
            Some(c) => Ok(c),
            None => arboard::Clipboard::new(),
        };
        let result = clipboard.and_then(|mut c| {
            let copied = c.set().file_list(&[&self.track.path]);
            self.file_clipboard = Some(c);
            copied
        });
        let notice = result.map(|()| tr("Copied the file").to_string()).map_err(|e| trf("Couldn't copy the file: {e}", &[("e", &e)]));
        self.save_notice = Some((notice, ctx.input(|i| i.time)));
    }

    /// Puts `path` first in Open Recent.
    fn remember(&mut self, path: &std::path::Path) {
        push_recent(&mut self.recent, path);
    }
}

/// How long a "Saved …" message stays in the player view.
const NOTICE_SECONDS: f64 = 4.0;

/// The last save's result: an error, or "Saved …" (only for `NOTICE_SECONDS` when `now` is given).
fn notice(ui: &mut egui::Ui, notice: Option<&(Result<String, String>, f64)>, now: Option<f64>) {
    match notice {
        Some((Err(e), _)) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        Some((Ok(m), at)) if now.is_none_or(|now| now - at < NOTICE_SECONDS) => {
            ui.label(m);
            if let Some(now) = now {
                ui.ctx().request_repaint_after(Duration::from_secs_f64(NOTICE_SECONDS - (now - at)));
            }
        }
        _ => {}
    }
}

/// Save As's format and quality choices.
/// The sample rates Save As offers besides the track's own.
const RATES: [u32; 6] = [22050, 32000, 44100, 48000, 88200, 96000];

fn khz(rate: u32) -> String {
    format!("{} kHz", rate as f64 / 1000.0)
}

/// `rate` is the track's own sample rate.
fn save_options(ui: &mut egui::Ui, choice: &mut save::Choice, rate: u32) {
    ui.horizontal(|ui| {
        for kind in save::Kind::ALL {
            ui.radio_value(&mut choice.kind, kind, kind.name());
        }
    });
    ui.horizontal(|ui| match choice.kind {
        save::Kind::Wav => {
            for (bits, label) in [(16, tr("16-bit")), (24, tr("24-bit")), (32, tr("32-bit float"))] {
                ui.radio_value(&mut choice.wav_bits, bits, label);
            }
        }
        save::Kind::Flac => {
            for (bits, label) in [(16, tr("16-bit")), (24, tr("24-bit"))] {
                ui.radio_value(&mut choice.flac_bits, bits, label);
            }
        }
        save::Kind::Mp3 => {
            for kbps in [128, 192, 256, 320] {
                ui.radio_value(&mut choice.mp3_kbps, kbps, trf("{kbps} kb/s", &[("kbps", &kbps)]));
            }
        }
        save::Kind::Vorbis => {
            ui.add(egui::Slider::new(&mut choice.vorbis_quality, 0..=10).text(tr("quality")));
        }
    });
    let same = trf("Same ({rate})", &[("rate", &khz(rate))]);
    egui::ComboBox::from_label(tr("Sample rate")).selected_text(choice.rate.map_or(same.clone(), khz)).show_ui(ui, |ui| {
        ui.selectable_value(&mut choice.rate, None, same);
        for r in RATES.into_iter().filter(|&r| r != rate) {
            ui.selectable_value(&mut choice.rate, Some(r), khz(r));
        }
    });
}

/// The cover's side: the full width, unless the height left after the controls (`controls`
/// tall, `gap` above them) is less; never below 64 points or wider than the window.
fn cover_side(width: f32, height: f32, controls: f32, gap: f32) -> f32 {
    (height - controls - gap).min(width).max(64.0).min(width)
}

/// The side the cover image is fitted into: the whole space, or at most 256 px.
fn cover_fit_side(side: f32, fit: bool) -> f32 {
    if fit { side } else { side.min(256.0) }
}

/// The cover in a `side` × `side` space (filling it when `fit`, else at most 256 px), with a
/// corner button that flips `fit`; or a placeholder note when there is none.
fn cover_art(ui: &mut egui::Ui, cover: Option<&egui::TextureHandle>, side: f32, fit: &mut bool, visual: &mut bool) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    // A small square button `n` places in from the bottom-right corner, drawn by `icon`.
    let corner_button = |n: f32, id: &str, tip: &str, icon: &dyn Fn(egui::Pos2, egui::Stroke)| {
        let br = rect.right_bottom() - egui::vec2(6.0 + 24.0 * n, 6.0);
        let button = egui::Rect::from_min_max(br - egui::vec2(20.0, 20.0), br);
        let click = ui.interact(button, egui::Id::unique(id), egui::Sense::click());
        click.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
        let visuals = ui.visuals();
        painter.rect_filled(button, 4.0, visuals.extreme_bg_color.gamma_multiply(0.7));
        let color = if click.hovered() { visuals.strong_text_color() } else { visuals.weak_text_color() };
        icon(button.center(), egui::Stroke::new(1.5, color));
        click.on_hover_text(tip).clicked()
    };
    match cover {
        Some(tex) => {
            let size = tex.size_vec2();
            let fitted = egui::Rect::from_center_size(rect.center(), size * (cover_fit_side(side, *fit) / size.x.max(size.y)));
            egui::Image::from_texture(tex).corner_radius(8.0).paint_at(ui, fitted);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, if *visual { tr("Visualiser") } else { tr("Album cover") }));
            let tip = if *fit { tr("Cover at 256 px") } else { tr("Fit cover space") };
            // Two arrows on the diagonal: out to the corners to fit, in to the middle to shrink.
            let arrows = |c: egui::Pos2, stroke: egui::Stroke| {
                for dir in [egui::vec2(1.0, -1.0), egui::vec2(-1.0, 1.0)] {
                    let (inner, outer) = (c + dir * 1.5, c + dir * 6.0);
                    let (tip_at, back) = if *fit { (inner, dir) } else { (outer, -dir) };
                    painter.line_segment([inner, outer], stroke);
                    painter.line_segment([tip_at, tip_at + egui::vec2(back.x * 3.5, 0.0)], stroke);
                    painter.line_segment([tip_at, tip_at + egui::vec2(0.0, back.y * 3.5)], stroke);
                }
            };
            if corner_button(0.0, "cover fit", tip, &arrows) {
                *fit = !*fit;
            }
        }
        None => {
            let visuals = ui.visuals();
            painter.rect_filled(rect, 8.0, visuals.faint_bg_color);
            // A note drawn from shapes, so it doesn't depend on the font having a glyph for it.
            let (u, c) = (side / 10.0, rect.center());
            let color = visuals.weak_text_color();
            painter.circle_filled(c + egui::vec2(-0.6 * u, 1.6 * u), 0.9 * u, color);
            painter.line_segment([c + egui::vec2(0.2 * u, 1.6 * u), c + egui::vec2(0.2 * u, -2.2 * u)], egui::Stroke::new(0.3 * u, color));
            painter.line_segment([c + egui::vec2(0.2 * u, -2.2 * u), c + egui::vec2(1.4 * u, -1.2 * u)], egui::Stroke::new(0.3 * u, color));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, tr("No album cover")));
        }
    }
    // Shows the cover: a framed picture; shows the visualiser: three level bars.
    let tip = if *visual { tr("Album cover") } else { tr("Visualiser") };
    let icon = |c: egui::Pos2, stroke: egui::Stroke| {
        if *visual {
            painter.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(12.0, 12.0)), 1.0, stroke, egui::StrokeKind::Middle);
            painter.line_segment([c + egui::vec2(-6.0, 4.0), c + egui::vec2(-1.0, -1.0)], stroke);
            painter.line_segment([c + egui::vec2(-1.0, -1.0), c + egui::vec2(6.0, 5.0)], stroke);
        } else {
            for (x, h) in [(-4.0, 4.0), (0.0, 8.0), (4.0, 6.0)] {
                painter.line_segment([c + egui::vec2(x, 5.0), c + egui::vec2(x, 5.0 - h)], stroke);
            }
        }
    };
    let n = if cover.is_some() { 1.0 } else { 0.0 };
    if corner_button(n, "cover visualiser", tip, &icon) {
        *visual = !*visual;
    }
}

/// One line of text, centred; scrolls back and forth when it is wider than the window.
fn marquee_label(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: egui::Color32, time: f64) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), galley.size().y), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    let offset = marquee(galley.size().x, rect.width(), time);
    let x = if galley.size().x <= rect.width() { rect.center().x - galley.size().x / 2.0 } else { rect.left() - offset };
    if galley.size().x > rect.width() {
        ui.ctx().request_repaint();
    }
    ui.painter_at(rect).galley(egui::pos2(x, rect.top()), galley, color);
}

/// Points per second the marquee moves, and seconds it rests at each end.
const MARQUEE_SPEED: f64 = 30.0;
const MARQUEE_PAUSE: f64 = 1.5;

/// How far to shift text `text_w` wide in a space `avail_w` wide at `time` seconds: rest,
/// scroll to the end, rest, scroll back.
fn marquee(text_w: f32, avail_w: f32, time: f64) -> f32 {
    let overflow = (text_w - avail_w) as f64;
    if overflow <= 0.0 {
        return 0.0;
    }
    let travel = overflow / MARQUEE_SPEED;
    let phase = time.rem_euclid(2.0 * (MARQUEE_PAUSE + travel));
    let shift = if phase < MARQUEE_PAUSE {
        0.0
    } else if phase < MARQUEE_PAUSE + travel {
        (phase - MARQUEE_PAUSE) * MARQUEE_SPEED
    } else if phase < 2.0 * MARQUEE_PAUSE + travel {
        overflow
    } else {
        overflow - (phase - 2.0 * MARQUEE_PAUSE - travel) * MARQUEE_SPEED
    };
    shift.clamp(0.0, overflow) as f32
}

/// A slim bar, or the waveform (`track` through `edit`, with the selection), with the times at
/// its ends; returns the frame to seek to when clicked or dragged.
type Wave<'a> = (&'a Track, &'a Edit, Option<(u64, u64)>, &'a mut WaveCache);
fn seek_bar(ui: &mut egui::Ui, pos: u64, len: u64, rate: u64, wave: Option<Wave>) -> Option<u64> {
    let height = if wave.is_some() { 48.0 } else { 18.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    let seconds = pos.checked_div(rate).unwrap_or(0) as f64;
    response.widget_info(|| egui::WidgetInfo::slider(true, seconds, tr("Position, seconds")));
    let visuals = ui.visuals();
    let fraction = (pos as f32 / len.max(1) as f32).min(1.0);
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let x = bar.left() + bar.width() * fraction;
    let painter = ui.painter_at(rect);
    if let Some((track, edit, selection, cache)) = wave {
        draw_waveform(&painter, visuals, rect, cache.columns(track, edit, 0, len, rect.width() as u64), (0, len), pos, selection);
    } else {
        painter.rect_filled(bar, 2.0, visuals.widgets.inactive.bg_fill);
        painter.rect_filled(egui::Rect::from_min_max(bar.min, egui::pos2(x, bar.max.y)), 2.0, theme::played(ui.ctx()));
        let knob = if response.hovered() || response.dragged() { 7.0 } else { 5.0 };
        // Not clipped to the bar's rect, so the knob can overhang its ends.
        ui.painter().circle_filled(egui::pos2(x, bar.center().y), knob, visuals.strong_text_color());
    }
    ui.horizontal(|ui| {
        ui.label(clock(pos, rate));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(clock(len, rate)));
    });
    let p = response.interact_pointer_pos().filter(|_| response.clicked() || response.dragged())?;
    Some((((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64 * len as f64) as u64)
}

/// One waveform's columns (lowest and highest sample per pixel), worked out again only when the
/// view, the width, the edits or the decoded audio change: the playhead moving costs nothing.
#[derive(Default)]
struct WaveCache {
    /// Width in columns, first frame, frames shown, decoded frames, and the edit list.
    key: Option<(u64, u64, u64, u64, Edit)>,
    cols: Vec<(f32, f32)>,
}

impl WaveCache {
    fn columns(&mut self, track: &Track, edit: &Edit, start: u64, frames: u64, columns: u64) -> &[(f32, f32)] {
        let (frames, decoded) = (frames.max(1), track.decoded());
        let same = self.key.as_ref().is_some_and(|k| (k.0, k.1, k.2, k.3) == (columns, start, frames, decoded) && k.4 == *edit);
        if !same {
            let peaks = track.peaks.lock().unwrap();
            // Stops where nothing is decoded yet.
            self.cols = (0..columns)
                .map_while(|c| {
                    let a = start + c * frames / columns;
                    edit::min_max(track, &peaks, edit, a, (start + (c + 1) * frames / columns).max(a + 1))
                })
                .collect();
            self.key = Some((columns, start, frames, decoded, edit.clone()));
        }
        &self.cols
    }
}

/// Waveform columns `cols` for edited frames `view` (start, count) across `rect`: the played part
/// highlighted, the selection shaded, and the playhead at `pos`.
fn draw_waveform(painter: &egui::Painter, visuals: &egui::Visuals, rect: egui::Rect, cols: &[(f32, f32)], (start, frames): (u64, u64), pos: u64, selection: Option<(u64, u64)>) {
    let frames = frames.max(1);
    let x_of = |f: u64| rect.left() + ((f as f64 - start as f64) / frames as f64 * rect.width() as f64) as f32;
    if let Some((a, b)) = selection {
        let shade = egui::Rect::from_x_y_ranges(x_of(a).max(rect.left())..=x_of(b).min(rect.right()), rect.y_range());
        painter.rect_filled(shade, 0.0, visuals.selection.bg_fill.gamma_multiply(0.35));
    }
    let (mid, half, played, played_color) = (rect.center().y, rect.height() / 2.0, x_of(pos), theme::played(painter.ctx()));
    for (c, &(lo, hi)) in cols.iter().enumerate() {
        let x = rect.left() + c as f32 + 0.5;
        let color = if x < played { played_color } else { visuals.text_color() };
        // At least one point tall, so silence still shows as a line.
        let (top, bottom) = ((mid - hi * half).min(mid - 0.5), (mid - lo * half).max(mid + 0.5));
        painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], egui::Stroke::new(1.0, color));
    }
    if rect.x_range().contains(played) {
        painter.vline(played, rect.y_range(), egui::Stroke::new(1.5, visuals.strong_text_color()));
    }
}

/// Where playback is `ago` after the callback said `pos`: moved on at `rate` × `speed`, wrapping
/// inside a looped region, stopping at an unlooped region's end and at the track's end `len`.
fn display_pos(pos: u64, ago: Duration, rate: u64, speed: f64, region: Option<(u64, u64)>, looping: bool, len: u64) -> u64 {
    // At most 0.1 s ahead, in case the callback stalls.
    let p = pos + (ago.as_secs_f64().min(0.1) * rate as f64 * speed) as u64;
    match region {
        // As the player does: the region counts only once playing inside it, before its end.
        Some((a, b)) if pos < b && p >= b => if looping && b > a { a + (p - a) % (b - a) } else { b },
        _ => p.min(len),
    }
}

/// The Edit panel's view and selection, in edited frames.
#[derive(Default)]
struct EditView {
    start: u64,
    /// Frames shown; 0 shows the whole track, following it as it grows while decoding.
    frames: u64,
    selection: Option<(u64, u64)>,
    /// The fixed end of the selection while dragging.
    anchor: Option<u64>,
    /// The playhead last frame, to notice it crossing the view's right edge.
    last_pos: u64,
}

/// The closest zoom: a few points per frame.
const MIN_VIEW: u64 = 64;

/// The view (start, frames) zoomed in by `factor` (2 = twice as close) with frame `at` staying
/// where it is on screen; (0, 0) once it shows the whole track.
fn zoom_view(start: u64, frames: u64, len: u64, factor: f32, at: u64) -> (u64, u64) {
    let new = ((frames as f64 / factor as f64) as u64).max(MIN_VIEW);
    if new >= len {
        return (0, 0);
    }
    let rel = at.saturating_sub(start) as f64 / frames.max(1) as f64;
    let start = (at as f64 - rel * new as f64).max(0.0) as u64;
    (start.min(len - new), new)
}

/// Selection, zoom and the waveform of the visible part; returns the frame to seek to on a click.
#[allow(clippy::too_many_arguments)]
fn edit_panel(ui: &mut egui::Ui, view: &mut EditView, cache: &mut WaveCache, track: &Track, edit: &Edit, pos: u64, len: u64, rate: u64, playing: bool) -> Option<u64> {
    let frames = follow(view, len, pos, playing);
    let centre = view.start + frames / 2;
    ui.horizontal(|ui| {
        match view.selection {
            Some((a, b)) => ui.label(format!("{} – {} ({:.3} s)", clock_ms(a, rate), clock_ms(b, rate), (b - a) as f64 / rate.max(1) as f64)),
            None => ui.weak(tr("Drag on the waveform to select")),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Fit")).on_hover_text(tr("Show the whole track")).clicked() {
                (view.start, view.frames) = (0, 0);
            }
            if ui.button("+").on_hover_text(tr("Zoom in (scroll; right-drag to move along)")).clicked() {
                (view.start, view.frames) = zoom_view(view.start, frames, len, 2.0, centre);
            }
            if ui.button("−").on_hover_text(tr("Zoom out")).clicked() {
                (view.start, view.frames) = zoom_view(view.start, frames, len, 0.5, centre);
            }
        });
    });
    let zoomed = view.frames != 0;
    let bar = if zoomed { SCROLLBAR + ui.spacing().item_spacing.y } else { 0.0 };
    let size = egui::vec2(ui.available_width(), (ui.available_height() - bar).max(40.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, tr("Waveform: click to play from there, drag to select, scroll to zoom, right-drag to move along")));
    let seek = select(ui, &response, rect, (view.start, frames), &mut view.selection, &mut view.anchor);
    zoom_pan(ui, &response, rect, view, len, frames, true);
    let frames = if view.frames == 0 { len } else { view.frames };
    let cols = cache.columns(track, edit, view.start, frames, rect.width() as u64);
    draw_waveform(&ui.painter_at(rect), ui.visuals(), rect, cols, (view.start, frames), pos, view.selection);
    if zoomed {
        scrollbar(ui, view, len);
    }
    seek
}

/// The Edit panel's scrollbar height.
const SCROLLBAR: f32 = 10.0;

/// Zoomed in, where `view` sits in the track `len`: drag the thumb or click the bar to move along.
fn scrollbar(ui: &mut egui::Ui, view: &mut EditView, len: u64) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), SCROLLBAR), egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ScrollBar, true, tr("Waveform position")));
    // Zoomed out to the whole track this frame (by the wheel): nothing to scroll.
    if view.frames == 0 || len == 0 {
        return;
    }
    let frames = view.frames.min(len);
    let per_px = len as f64 / rect.width().max(1.0) as f64;
    let start = if response.dragged() {
        view.start as f64 + response.drag_delta().x as f64 * per_px
    } else if response.clicked() && let Some(p) = response.interact_pointer_pos() {
        (p.x - rect.left()) as f64 * per_px - frames as f64 / 2.0
    } else {
        view.start as f64
    };
    view.start = start.clamp(0.0, (len - frames) as f64) as u64;
    let x = |f: u64| rect.left() + (f as f64 / per_px) as f32;
    let (a, b) = (x(view.start), x(view.start + frames));
    let thumb = egui::Rect::from_x_y_ranges(a..=b.max(a + 6.0), rect.y_range());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, SCROLLBAR / 2.0, ui.visuals().extreme_bg_color);
    painter.rect_filled(thumb, SCROLLBAR / 2.0, ui.style().interact(&response).bg_fill);
}

/// Keeps `view` inside the track `len` and, zoomed in, turns the page when the playhead `pos`
/// runs off its right edge (one moved out of view by panning is left alone); the frames shown.
fn follow(view: &mut EditView, len: u64, pos: u64, playing: bool) -> u64 {
    let frames = if view.frames == 0 { len } else { view.frames.min(len) }.max(1);
    view.start = view.start.min(len.saturating_sub(frames));
    let end = view.start + frames;
    if playing && view.frames != 0 && view.anchor.is_none() && (view.start..end).contains(&view.last_pos) && pos >= end {
        view.start = pos.min(len.saturating_sub(frames));
    }
    view.last_pos = pos;
    frames
}

/// Drag to select frames of `(start, frames)` shown across `rect` (Shift moves the nearer end),
/// click to play from there (Shift+click extends the selection); returns the frame to seek to.
/// `anchor` is the selection's fixed end while dragging.
fn select(ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, (start, frames): (u64, u64), selection: &mut Option<(u64, u64)>, anchor: &mut Option<u64>) -> Option<u64> {
    let frame_at = |x: f32| start + (((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * frames as f64) as u64;
    let shift = ui.input(|i| i.modifiers.shift);
    let mut seek = None;
    if response.drag_started_by(egui::PointerButton::Primary) {
        let from = ui.input(|i| i.pointer.press_origin()).map_or(0, |p| frame_at(p.x));
        // Shift+drag moves the nearer end of the selection.
        *anchor = Some(match *selection {
            Some((a, b)) if shift => if from.abs_diff(a) < from.abs_diff(b) { b } else { a },
            _ => from,
        });
    }
    if response.dragged_by(egui::PointerButton::Primary) && let (Some(anchor), Some(p)) = (*anchor, response.interact_pointer_pos()) {
        let f = frame_at(p.x);
        *selection = (f != anchor).then(|| (anchor.min(f), anchor.max(f)));
    }
    if response.drag_stopped() {
        *anchor = None;
    }
    if response.clicked() && let Some(p) = response.interact_pointer_pos() {
        let f = frame_at(p.x);
        match *selection {
            Some((a, b)) if shift => *selection = Some((a.min(f), b.max(f))),
            _ => (*selection, seek) = (None, Some(f)),
        }
    }
    seek
}

/// The wheel zooms `view` (showing `frames` across `rect`) around the pointer, and Ctrl+wheel or
/// a pinch too when `zoom_delta`; right-drag and sideways scrolling (touchpads, Shift+wheel) move along.
fn zoom_pan(ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, view: &mut EditView, len: u64, frames: u64, zoom_delta: bool) {
    let frame_at = |x: f32| view.start + (((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * frames as f64) as u64;
    let mut pan = if response.dragged_by(egui::PointerButton::Secondary) { response.drag_delta().x } else { 0.0 };
    if response.hovered() {
        // egui turns Ctrl+scroll into zoom_delta; plain vertical scrolling zooms too.
        let (zoom, scroll, pointer) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        let zoom = if zoom_delta { zoom } else { 1.0 } * (scroll.y / 200.0).exp();
        if zoom != 1.0 {
            let at = pointer.map_or(view.start + frames / 2, |p| frame_at(p.x));
            (view.start, view.frames) = zoom_view(view.start, frames, len, zoom, at);
        }
        pan += scroll.x;
    }
    if view.frames != 0 && pan != 0.0 {
        let shift = (-pan / rect.width()) as f64 * view.frames.min(len) as f64;
        view.start = (view.start as f64 + shift).clamp(0.0, len.saturating_sub(view.frames.min(len)) as f64) as u64;
    }
}

/// The Spectrogram settings window: the notes' tuning, names and look.
fn spectrum_settings(ui: &mut egui::Ui, s: &mut settings::Settings, rate: u64) {
    let ms = |frames: usize| format!("{frames} ({:.1} ms)", frames as f64 * 1000.0 / rate.max(1) as f64);
    let p = &mut s.spectrum;
    ui.strong(tr("Analysis"));
    egui::Grid::new("analysis").num_columns(2).show(ui, |ui| {
        ui.label(tr("FFT size"));
        egui::ComboBox::from_id_salt("fft").selected_text(ms(p.fft)).show_ui(ui, |ui| {
            for n in (9..=15).map(|b| 1usize << b) {
                ui.selectable_value(&mut p.fft, n, ms(n));
            }
        });
        ui.end_row();
        ui.label(tr("Window size"));
        egui::ComboBox::from_id_salt("window").selected_text(ms(p.window)).show_ui(ui, |ui| {
            for n in (8..=15).map(|b| 1usize << b).filter(|n| *n <= p.fft) {
                ui.selectable_value(&mut p.window, n, ms(n));
            }
        });
        ui.end_row();
        ui.label(tr("Hop"));
        egui::ComboBox::from_id_salt("hop").selected_text(ms(p.hop)).show_ui(ui, |ui| {
            for n in spectrum::HOPS {
                ui.selectable_value(&mut p.hop, n, ms(n));
            }
        });
        ui.end_row();
        ui.label(tr("Window function"));
        egui::ComboBox::from_id_salt("shape").selected_text(p.shape.name()).show_ui(ui, |ui| {
            for shape in spectrum::Shape::ALL {
                ui.selectable_value(&mut p.shape, shape, shape.name());
            }
        });
        ui.end_row();
        ui.label(tr("Levels"));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut p.floor).range(-160.0..=-20.0).speed(0.5).suffix(" dB"));
            ui.label(tr("to"));
            ui.add(egui::DragValue::new(&mut p.ceiling).range(-60.0..=6.0).speed(0.5).suffix(" dBFS"));
        });
        ui.end_row();
        ui.label(tr("Lowest frequency"));
        ui.add(egui::DragValue::new(&mut p.low).range(10.0..=200.0).speed(0.5).suffix(" Hz"));
        ui.end_row();
        ui.label(tr("Colours"));
        egui::ComboBox::from_id_salt("colours").selected_text(p.map.name()).show_ui(ui, |ui| {
            for map in spectrum::ColourMap::ALL {
                ui.selectable_value(&mut p.map, map, map.name());
            }
        });
        ui.end_row();
    });
    if ui.add_enabled(*p != spectrum::Params::default(), egui::Button::new(tr("Reset to defaults"))).clicked() {
        *p = spectrum::Params::default();
    }
    *p = p.checked();
    ui.separator();
    ui.strong(tr("Notes"));
    ui.horizontal(|ui| {
        ui.label(tr("Tuning A4"));
        ui.add(egui::DragValue::new(&mut s.note_a4).range(400.0..=480.0).speed(0.1).suffix(" Hz"));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.note_solfege, false, tr("C D E"));
        ui.radio_value(&mut s.note_solfege, true, tr("Do Ré Mi"));
    });
    egui::Grid::new("notes").num_columns(2).show(ui, |ui| {
        ui.label(tr("Scale shown"));
        let scale = |k: Option<usize>| k.map_or(tr("All notes").to_string(), |k| trf("{key} major", &[("key", &spectrum::KEYS[k].0)]));
        egui::ComboBox::from_id_salt("scale").selected_text(scale(s.note_scale)).show_ui(ui, |ui| {
            for k in std::iter::once(None).chain((0..12).map(Some)) {
                ui.selectable_value(&mut s.note_scale, k, scale(k));
            }
        });
        ui.end_row();
        ui.label(tr("Instrument range"));
        let range = |i: spectrum::Instrument| match i.range() {
            None => tr("None (whole range)").to_string(),
            Some(_) => {
                let n = spectrum::notes(440.0, 1.0, f32::MAX, None, true, i);
                let name = |n: Option<&spectrum::Note>| n.map_or(String::new(), |n| spectrum::note_name(n, false, true));
                format!("{} ({}–{})", i.name(), name(n.first()), name(n.last()))
            }
        };
        egui::ComboBox::from_id_salt("instrument").selected_text(range(s.note_instrument)).show_ui(ui, |ui| {
            for i in spectrum::Instrument::ALL {
                ui.selectable_value(&mut s.note_instrument, i, range(i));
            }
        });
        ui.end_row();
    });
    ui.add_enabled_ui(s.note_scale.is_none(), |ui| {
        ui.horizontal(|ui| {
            ui.radio_value(&mut s.note_flats, false, tr("Sharps (C#)"));
            ui.radio_value(&mut s.note_flats, true, tr("Flats (Db)"));
        })
        .response
        .on_disabled_hover_text(tr("The key spells its notes"));
    });
    ui.checkbox(&mut s.note_octaves, tr("Octave numbers"));
    ui.separator();
    for (colour, label) in [(&mut s.note_c_color, tr("Tonic lines (C for all notes)")), (&mut s.note_color, tr("Other lines"))] {
        ui.horizontal(|ui| {
            egui::color_picker::color_edit_button_srgba(ui, colour, egui::color_picker::Alpha::OnlyBlend);
            ui.label(label);
        });
    }
    ui.add(egui::Slider::new(&mut s.note_c_width, 1.0..=6.0).text(tr("Tonic line width")));
    ui.add(egui::Slider::new(&mut s.note_label_size, 6.0..=24.0).text(tr("Label size")));
}

/// The spectrogram of edited frames shown in `view`, with note lines or frequency marks and the
/// playhead; returns the frame to seek to on a click.
#[allow(clippy::too_many_arguments)]
fn spectrum_panel(ui: &mut egui::Ui, view: &mut EditView, selection: &mut Option<(u64, u64)>, spectrogram: &mut spectrum::Spectrogram, s: &settings::Settings, track: &Arc<Track>, edit: &Arc<Edit>, pos: u64, len: u64, rate: u64, playing: bool) -> Option<u64> {
    let frames = follow(view, len, pos, playing);
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, tr("Spectrogram: click to play from there, drag to select, scroll to zoom, Ctrl+scroll to zoom the frequencies, right-drag to move around")));
    let seek = select(ui, &response, rect, (view.start, frames), selection, &mut view.anchor);
    zoom_pan(ui, &response, rect, view, len, frames, false);
    // Frequencies: Ctrl+wheel (or a pinch) zooms around the pointer, right-drag up and down moves.
    if response.hovered() {
        let (zoom, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if zoom != 1.0 {
            let at = pointer.map_or(0.5, |p| 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0));
            spectrogram.band = spectrum::zoom_band(spectrogram.band, zoom, at);
        }
    }
    if response.dragged_by(egui::PointerButton::Secondary) {
        spectrogram.band = spectrum::pan_band(spectrogram.band, response.drag_delta().y / rect.height());
    }
    let frames = if view.frames == 0 { len } else { view.frames }.max(1);
    let painter = ui.painter_at(rect);
    let x_of = |f: u64| rect.left() + ((f as f64 - view.start as f64) / frames as f64 * rect.width() as f64) as f32;
    let (band, low, nyquist) = (spectrogram.band, s.spectrum.low, rate as f32 / 2.0);
    let y_of = |freq: f32| rect.top() + spectrum::height_of(freq, rate as f32, band, low) * rect.height();
    spectrogram.update(ui.ctx(), track, edit, s.spectrum);
    // Worked out once for the whole track; zooming and panning only move and stretch the tiles.
    let x_at = |f: f64| rect.left() + ((f - view.start as f64) / frames as f64 * rect.width() as f64) as f32;
    // The bins are evenly spaced in Hz, the axis isn't: each bin is its own strip of a mesh.
    let fft = spectrogram.fft();
    for (from, to, slab, texture) in spectrogram.tiles() {
        let x = [x_at(from), x_at(to)];
        if x[1] >= rect.left() && x[0] <= rect.right() {
            painter.add(spectrum::mesh(texture.id(), x, slab, fft, rate as f32, y_of));
        }
    }
    let status = if !track.done() { Some(tr("Decoding…").to_string()) } else { spectrogram.progress().map(|done| format!("Computing… {:.0} %", done * 100.0)) };
    if let Some(text) = status {
        // Below the chord bar when it's shown (13 pt text plus 8).
        let top = if s.spectrum_chords { 25.0 } else { 4.0 };
        painter.text(rect.right_top() + egui::vec2(-6.0, top), egui::Align2::RIGHT_TOP, text, egui::FontId::proportional(12.0), egui::Color32::from_white_alpha(220));
    }
    if let Some((a, b)) = *selection {
        let shade = egui::Rect::from_x_y_ranges(x_of(a).max(rect.left())..=x_of(b).min(rect.right()), rect.y_range());
        painter.rect_filled(shade, 0.0, egui::Color32::from_white_alpha(40));
        painter.vline(shade.left(), rect.y_range(), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)));
        painter.vline(shade.right(), rect.y_range(), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)));
    }
    if s.spectrum_notes && nyquist > low {
        // Points between neighbouring lines: a semitone, or about two in a scale (7 notes an octave).
        let per_octave = if s.note_scale.is_some() { 7.0 } else { 12.0 };
        let step = rect.height() * 2f32.ln() / per_octave / ((nyquist / low).ln() * (band[1] - band[0]));
        let (every, one_an_octave) = (step >= s.note_label_size, step * per_octave >= s.note_label_size);
        // Short of room, one name an octave: on C, or on the tonic in a scale (D major has no C).
        // The tonal centre: the scale's tonic, or C with all notes. Its lines stand out and, short of room, carry the names.
        let tonic = s.note_scale.map_or(0, |k| spectrum::KEYS[k].1);
        let font = egui::FontId::proportional(s.note_label_size);
        for note in spectrum::notes(s.note_a4, low, nyquist, s.note_scale, s.note_flats, s.note_instrument) {
            let (freq, pc) = (note.freq, note.pc);
            let y = y_of(freq);
            if !rect.y_range().contains(y) {
                continue;
            }
            let stroke = if pc == tonic { egui::Stroke::new(s.note_c_width, s.note_c_color) } else { egui::Stroke::new(1.0, s.note_color) };
            painter.hline(rect.x_range(), y, stroke);
            if every || (one_an_octave && pc == tonic) {
                let name = spectrum::note_name(&note, s.note_solfege, s.note_octaves);
                let galley = painter.layout_no_wrap(name, font.clone(), egui::Color32::WHITE);
                let at = egui::pos2(rect.left() + 4.0, y - galley.size().y / 2.0);
                painter.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand(1.0), 2.0, egui::Color32::from_black_alpha(160));
                painter.galley(at, galley, egui::Color32::WHITE);
            }
        }
    }
    for (freq, label) in [(100.0, "100 Hz"), (1000.0, "1 kHz"), (10000.0, "10 kHz")] {
        if !s.spectrum_notes && freq < nyquist {
            let y = y_of(freq);
            painter.hline(rect.x_range(), y, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)));
            painter.text(egui::pos2(rect.left() + 4.0, y - 2.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), egui::Color32::from_white_alpha(200));
        }
    }
    if s.spectrum_chords {
        chord_bar(&painter, rect, &x_of, s, track, edit);
    }
    // Found beats as ticks along the bottom, the first of each bar taller.
    if let Some(b) = track.beats.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        let down = ai::edited_beats(&b.downbeats, edit);
        for f in ai::edited_beats(&b.beats, edit) {
            let x = x_of(f);
            if rect.x_range().contains(x) {
                let h = if down.binary_search(&f).is_ok() { 14.0 } else { 7.0 };
                painter.vline(x, (rect.bottom() - h)..=rect.bottom(), egui::Stroke::new(1.5, egui::Color32::from_white_alpha(220)));
            }
        }
    }
    let x = x_of(pos);
    if rect.x_range().contains(x) {
        painter.vline(x, rect.y_range(), egui::Stroke::new(1.5, egui::Color32::WHITE));
    }
    seek
}

/// The Spectrogram's chord bar: a strip along the top, a cell per chord where the edits play it.
fn chord_bar(painter: &egui::Painter, rect: egui::Rect, x_of: &dyn Fn(u64) -> f32, s: &settings::Settings, track: &Track, edit: &Edit) {
    let font = egui::FontId::proportional(13.0);
    let strip = egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), font.size + 8.0));
    painter.rect_filled(strip, 0.0, egui::Color32::from_black_alpha(140));
    let Some(chords) = track.chords.get() else {
        let text = if track.done() { tr("Chords: measuring…") } else { tr("Chords: after loading") };
        painter.text(strip.left_center() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, text, font, egui::Color32::from_white_alpha(200));
        return;
    };
    for (i, (a, b, chord)) in chords::edited(chords, edit).into_iter().enumerate() {
        let (x0, x1) = (x_of(a).max(strip.left()), x_of(b).min(strip.right()));
        if x1 <= x0 {
            continue;
        }
        let cell = egui::Rect::from_x_y_ranges(x0..=x1, strip.y_range());
        painter.rect_filled(cell.shrink2(egui::vec2(0.5, 0.0)), 0.0, egui::Color32::from_black_alpha(if i % 2 == 0 { 200 } else { 160 }));
        let kind = match chord.kind {
            chords::Kind::Major => "",
            chords::Kind::Minor => "m",
            chords::Kind::Seventh => "7",
        };
        let name = spectrum::pc_name(chord.root, s.note_flats, s.note_scale, s.note_solfege) + kind;
        let galley = painter.layout_no_wrap(name, font.clone(), egui::Color32::WHITE);
        if galley.size().x + 6.0 <= cell.width() {
            painter.galley(egui::pos2(cell.left() + 3.0, cell.center().y - galley.size().y / 2.0), galley, egui::Color32::WHITE);
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Volume { muted: bool },
    Previous,
    Play,
    Pause,
    Next,
    Order(Order),
}

/// Volume, then previous, play/pause and next, then the play order, with play/pause centred.
/// `metronome`: whether it's on and its slider, once the track's beats are found.
fn transport(ui: &mut egui::Ui, playing: bool, has_next: bool, volume: f32, metronome: Option<(bool, f32)>, order: Order) -> Vec<Action> {
    let (size, small, gap) = (56.0, 40.0, 16.0);
    let buttons = [
        (Icon::Previous, tr("Previous"), true, Action::Previous),
        (if playing { Icon::Pause } else { Icon::Play }, if playing { tr("Pause") } else { tr("Play") }, true, Action::Toggle),
        (Icon::Next, tr("Next"), has_next, Action::Next),
    ];
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        // The volume and play-order buttons are the same size, so play/pause stays centred.
        let row = 3.0 * size + 2.0 * gap + 2.0 * (small + gap);
        ui.add_space(((ui.available_width() - row) / 2.0).max(0.0));
        let percent = format!("{:.0}", volume * 100.0);
        let label = if metronome.is_some_and(|m| m.0) { trf("Volume, {percent} %, metronome on", &[("percent", &percent)]) } else { trf("Volume, {percent} %", &[("percent", &percent)]) };
        let button = icon_button(ui, Icon::Volume { muted: volume == 0.0 }, &label, true, small);
        let mut v = volume;
        let (mut on, mut clicks) = metronome.unwrap_or_default();
        egui::Popup::from_toggle_button_response(&button)
            .align(egui::RectAlign::TOP)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false).text(tr("Volume")));
                match metronome {
                    Some(_) => {
                        ui.checkbox(&mut on, tr("Metronome")).on_hover_text(tr("Clicks on the found beats"));
                        ui.add_enabled(on, egui::Slider::new(&mut clicks, 0.0..=1.0).show_value(false).text(tr("Clicks")));
                    }
                    None => drop(ui.add_enabled(false, egui::Checkbox::new(&mut false, tr("Metronome"))).on_disabled_hover_text(tr("after Local AI → Find Beats"))),
                }
            });
        if v != volume {
            actions.push(Action::Volume(v));
        }
        if let Some((was_on, was)) = metronome {
            if on != was_on {
                actions.push(Action::Metronome(on));
            }
            if clicks != was {
                actions.push(Action::ClickVolume(clicks));
            }
        }
        for (icon, label, enabled, action) in buttons {
            if icon_button(ui, icon, label, enabled, size).clicked() {
                actions.push(action);
            }
        }
        let (name, then) = match order {
            Order::InOrder => (tr("In order"), Order::RepeatOne),
            Order::RepeatOne => (tr("Repeat track"), Order::Shuffle),
            Order::Shuffle => (tr("Shuffle"), Order::InOrder),
        };
        if icon_button(ui, Icon::Order(order), &trf("Play order, {name}", &[("name", &name)]), true, small).on_hover_text(name).clicked() {
            actions.push(Action::Order(then));
        }
    });
    actions
}

/// The track before or after `index` of `len`: the folder's order, or the shuffled one in Shuffle.
fn neighbour(order: Order, shuffle: (u64, usize), len: usize, index: usize, back: bool) -> Option<usize> {
    let list: Vec<usize> = if order == Order::Shuffle { shuffled(len, shuffle.0, shuffle.1) } else { (0..len).collect() };
    let k = list.iter().position(|&i| i == index)?;
    list.get(if back { k.checked_sub(1)? } else { k + 1 }).copied()
}

/// 0..len in a random order set by `seed` (Fisher–Yates on a xorshift), with `start` first.
fn shuffled(len: usize, seed: u64, start: usize) -> Vec<usize> {
    let mut list: Vec<usize> = (0..len).collect();
    let mut x = seed | 1;
    for i in (1..len).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        list.swap(i, (x % (i as u64 + 1)) as usize);
    }
    if let Some(k) = list.iter().position(|&i| i == start) {
        list.swap(0, k);
    }
    list
}

/// A new random number from std's per-process hash keys, so no random-number crate is needed.
fn random_seed() -> u64 {
    use std::hash::BuildHasher;
    std::hash::RandomState::new().hash_one(Instant::now())
}

/// Gain for slider position `v` (0 to 1): a cube curve, so equal steps sound about equally loud.
fn volume_gain(v: f32) -> f32 {
    v.clamp(0.0, 1.0).powi(3)
}

/// A round button with a drawn icon; screen readers get `label`.
fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool, size: f32) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let visuals = ui.visuals();
    let main = matches!(icon, Icon::Play | Icon::Pause);
    let color = if !enabled { visuals.weak_text_color() } else if main { visuals.panel_fill } else { visuals.strong_text_color() };
    let painter = ui.painter();
    if main {
        painter.circle_filled(rect.center(), size / 2.0, visuals.strong_text_color());
    } else if response.hovered() && enabled {
        painter.circle_filled(rect.center(), size / 2.0, visuals.widgets.hovered.bg_fill);
    }
    if response.has_focus() {
        painter.circle_stroke(rect.center(), size / 2.0, visuals.selection.stroke);
    }
    let (c, u) = (rect.center(), size / 8.0);
    let triangle = |tip: f32, base: f32| {
        // Points right when tip > base.
        vec![egui::pos2(c.x + base, c.y - 1.2 * u), egui::pos2(c.x + tip, c.y), egui::pos2(c.x + base, c.y + 1.2 * u)]
    };
    let fill = |points| egui::Shape::convex_polygon(points, color, egui::Stroke::NONE);
    let bar = |x: f32| egui::Rect::from_center_size(egui::pos2(x, c.y), egui::vec2(0.35 * u, 2.4 * u));
    match icon {
        Icon::Volume { muted } => {
            // A speaker: a box and a cone, then sound waves or a slash.
            let body = egui::Rect::from_center_size(egui::pos2(c.x - 1.4 * u, c.y), egui::vec2(0.8 * u, 1.2 * u));
            painter.rect_filled(body, 0.0, color);
            painter.add(fill(vec![egui::pos2(body.right(), body.top()), egui::pos2(c.x + 0.2 * u, c.y - 1.5 * u), egui::pos2(c.x + 0.2 * u, c.y + 1.5 * u), egui::pos2(body.right(), body.bottom())]));
            let stroke = egui::Stroke::new(0.3 * u, color);
            if muted {
                painter.line_segment([egui::pos2(c.x + 0.7 * u, c.y - 0.8 * u), egui::pos2(c.x + 2.0 * u, c.y + 0.8 * u)], stroke);
                painter.line_segment([egui::pos2(c.x + 0.7 * u, c.y + 0.8 * u), egui::pos2(c.x + 2.0 * u, c.y - 0.8 * u)], stroke);
            } else {
                for r in [1.1 * u, 1.9 * u] {
                    let arc = (-6..=6).map(|i| {
                        let a = i as f32 * 0.12;
                        egui::pos2(c.x + 0.2 * u + r * a.cos(), c.y + r * a.sin())
                    });
                    painter.add(egui::Shape::line(arc.collect(), stroke));
                }
            }
        }
        Icon::Play => {
            painter.add(fill(vec![egui::pos2(c.x - 0.8 * u, c.y - 1.4 * u), egui::pos2(c.x + 1.4 * u, c.y), egui::pos2(c.x - 0.8 * u, c.y + 1.4 * u)]));
        }
        Icon::Pause => {
            for x in [c.x - 0.6 * u, c.x + 0.6 * u] {
                painter.rect_filled(egui::Rect::from_center_size(egui::pos2(x, c.y), egui::vec2(0.5 * u, 2.6 * u)), 1.0, color);
            }
        }
        Icon::Previous => {
            painter.add(fill(triangle(-0.9 * u, 1.0 * u)));
            painter.rect_filled(bar(c.x - 1.1 * u), 0.0, color);
        }
        Icon::Next => {
            painter.add(fill(triangle(0.9 * u, -1.0 * u)));
            painter.rect_filled(bar(c.x + 1.1 * u), 0.0, color);
        }
        Icon::Order(order) => {
            let stroke = egui::Stroke::new(0.3 * u, color);
            // An arrowhead at `tip` pointing along `dir` (a unit vector).
            let head = |tip: egui::Pos2, dir: egui::Vec2| {
                let (back, side) = (tip - 0.8 * u * dir, 0.55 * u * dir.rot90());
                fill(vec![tip, back + side, back - side])
            };
            let right = egui::vec2(1.0, 0.0);
            match order {
                Order::InOrder => {
                    painter.line_segment([egui::pos2(c.x - 1.5 * u, c.y), egui::pos2(c.x + 0.9 * u, c.y)], stroke);
                    painter.add(head(egui::pos2(c.x + 1.5 * u, c.y), right));
                }
                Order::RepeatOne => {
                    // A circle open at the right, its end arrowed, around a 1.
                    let (r, a0, a1) = (1.5 * u, 0.4_f32, std::f32::consts::TAU - 0.5);
                    let arc = (0..=24).map(|i| {
                        let a = a0 + (a1 - a0) * i as f32 / 24.0;
                        c + r * egui::vec2(a.cos(), a.sin())
                    });
                    painter.add(egui::Shape::line(arc.collect(), stroke));
                    let a = a1 + 0.25;
                    painter.add(head(c + r * egui::vec2(a.cos(), a.sin()), egui::vec2(-a.sin(), a.cos())));
                    painter.text(c, egui::Align2::CENTER_CENTER, "1", egui::FontId::proportional(1.7 * u), color);
                }
                Order::Shuffle => {
                    for (y0, y1) in [(-0.9, 0.9), (0.9, -0.9)] {
                        let (from, to) = (egui::pos2(c.x - 1.5 * u, c.y + y0 * u), egui::pos2(c.x + 1.5 * u, c.y + y1 * u));
                        let dir = (to - from).normalized();
                        painter.line_segment([from, to - 0.6 * u * dir], stroke);
                        painter.add(head(to, dir));
                    }
                }
            }
        }
    }
    response
}

/// `frames` at `rate` as m:ss.
fn clock(frames: u64, rate: u64) -> String {
    let s = frames.checked_div(rate).unwrap_or(0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// `frames` at `rate` as m:ss.mmm, for selection edges.
fn clock_ms(frames: u64, rate: u64) -> String {
    let ms = (frames * 1000).checked_div(rate).unwrap_or(0);
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::{parse_fc_match, valid_file_name, clock_ms, cover_fit_side, cover_side, display_pos, info_rows, marquee, neighbour, push_recent, resize_for_panel, shuffled, snap_speed, title, volume_gain, zoom_view};
    use crate::settings::Order;
    use std::path::PathBuf;

    #[test]
    fn typed_file_names() {
        assert!(valid_file_name("01 - Song.mp3").is_ok());
        for bad in ["", "  ", ".", "..", "a/b.mp3", "a\\b.mp3"] {
            assert!(valid_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cjk_font_only_when_it_covers_chinese() {
        assert_eq!(parse_fc_match("/f/NotoSansCJK.ttc\n1\nen|ja|zh-cn|zh-tw"), Some(("/f/NotoSansCJK.ttc".into(), 1)));
        assert_eq!(parse_fc_match("/f/DejaVuSans.ttf\n0\nen|de|ru"), None);
        assert_eq!(parse_fc_match(""), None);
    }

    #[test]
    fn track_info_rows() {
        // 2 s of stereo at 44.1 kHz in a 352 800-byte file: 1411 kb/s, like a CD.
        let track = crate::track::Track::from_samples(44100, 2, vec![0.0; 2 * 88200]);
        let groups = info_rows(&track, Some(352_800), true);
        assert_eq!(groups.each_ref().map(|(s, _)| *s), ["General", "Audio", "Tags"]);
        let rows: Vec<_> = groups.into_iter().flat_map(|(_, r)| r).collect();
        let row = |name| rows.iter().find(|(n, _)| *n == name).unwrap().1.clone();
        assert_eq!(row("Length"), "0:02");
        assert_eq!(row("Sample rate"), "44.1 kHz");
        assert_eq!(row("Channels"), "2 (stereo)");
        assert_eq!(row("Bitrate"), "1411 kb/s");
        assert_eq!(row("File size"), "0.4 MB");
        assert_eq!(row("Title"), "—");
        assert_eq!(row("Loudness"), "measuring…");
        assert_eq!(row("Tempo"), "measuring…");
        let off: Vec<_> = info_rows(&track, None, false).into_iter().flat_map(|(_, r)| r).map(|(n, _)| n).collect();
        assert!(!off.contains(&"Tempo") && !off.contains(&"Key") && off.contains(&"Peak"));
    }

    #[test]
    fn panel_grows_and_shrinks_the_window() {
        assert_eq!(resize_for_panel([380.0, 680.0], true, 320.0, 0), [700.0, 680.0]);
        assert_eq!(resize_for_panel([700.0, 680.0], false, 320.0, 0), [380.0, 680.0]);
        assert_eq!(resize_for_panel([380.0, 680.0], true, 160.0, 1), [380.0, 840.0]);
    }

    #[test]
    fn playhead_glides_wraps_and_stops() {
        let ms = |n| std::time::Duration::from_millis(n);
        assert_eq!(display_pos(1000, ms(10), 48000, 1.0, None, false, 100_000), 1480);
        assert_eq!(display_pos(1000, ms(10), 48000, 2.0, None, false, 100_000), 1960);
        assert_eq!(display_pos(99_900, ms(10), 48000, 1.0, None, false, 100_000), 100_000, "not past the end");
        // Region 1000..1200: looping wraps back in, without a loop it rests at the end.
        assert_eq!(display_pos(1100, ms(10), 48000, 1.0, Some((1000, 1200)), true, 100_000), 1000 + (1580 - 1000) % 200);
        assert_eq!(display_pos(1100, ms(10), 48000, 1.0, Some((1000, 1200)), false, 100_000), 1200);
        // Past the region's end it doesn't count.
        assert_eq!(display_pos(5000, ms(10), 48000, 1.0, Some((1000, 1200)), true, 100_000), 5480);
        assert_eq!(display_pos(1000, ms(500), 48000, 1.0, None, false, 100_000), 5800, "capped at 0.1 s");
    }

    #[test]
    fn shuffle_plays_each_track_once_from_the_start() {
        let list = shuffled(16, 12345, 7);
        let mut sorted = list.clone();
        sorted.sort();
        assert_eq!(sorted, (0..16).collect::<Vec<_>>());
        assert_eq!(list[0], 7);
        assert_eq!(list, shuffled(16, 12345, 7), "the same seed, the same order");
        assert_ne!(list, shuffled(16, 999, 7));
        assert!(shuffled(0, 1, 0).is_empty());
        // Walking it with Next visits every track, then stops.
        let (mut at, mut seen) = (7, vec![7]);
        while let Some(i) = neighbour(Order::Shuffle, (12345, 7), 16, at, false) {
            (at, seen) = (i, [seen, vec![i]].concat());
        }
        assert_eq!(seen, list);
        assert_eq!(neighbour(Order::Shuffle, (12345, 7), 16, list[3], true), Some(list[2]));
        assert_eq!(neighbour(Order::Shuffle, (12345, 7), 16, 7, true), None);
    }

    #[test]
    fn in_order_and_repeat_move_through_the_folder() {
        for order in [Order::InOrder, Order::RepeatOne] {
            assert_eq!(neighbour(order, (1, 0), 3, 1, false), Some(2));
            assert_eq!(neighbour(order, (1, 0), 3, 2, false), None);
            assert_eq!(neighbour(order, (1, 0), 3, 0, true), None);
        }
    }

    #[test]
    fn zoom_keeps_the_frame_in_place() {
        // Twice as close around frame 500 of 0..1000: 500 stays in the middle.
        assert_eq!(zoom_view(0, 1000, 1000, 2.0, 500), (250, 500));
        // Around the left edge, the left edge stays.
        assert_eq!(zoom_view(0, 1000, 1000, 2.0, 0), (0, 500));
        // Out past the whole track shows it all; never closer than MIN_VIEW.
        assert_eq!(zoom_view(250, 500, 1000, 0.25, 500), (0, 0));
        assert_eq!(zoom_view(0, 100, 1000, 100.0, 50).1, super::MIN_VIEW);
        assert_eq!(clock_ms(48000 * 61 + 24000, 48000), "1:01.500");
    }

    #[test]
    fn cover_leaves_room_for_the_controls() {
        assert_eq!(cover_side(1200.0, 680.0, 250.0, 22.0), 408.0, "wide: limited by height");
        assert_eq!(cover_side(364.0, 680.0, 250.0, 22.0), 364.0, "narrow: limited by width");
        assert_eq!(cover_side(364.0, 300.0, 250.0, 22.0), 64.0, "tiny: smallest cover");
        assert_eq!(cover_side(40.0, 300.0, 250.0, 22.0), 40.0, "never wider than the window");
        assert_eq!((cover_fit_side(400.0, false), cover_fit_side(180.0, false), cover_fit_side(400.0, true)), (256.0, 180.0, 400.0));
    }

    #[test]
    fn title_parts() {
        assert_eq!(title("Long Gone.mp3", Some("3:12"), "MP3"), "Long Gone.mp3 — 3:12 — MP3");
        assert_eq!(title("Long Gone.mp3", None, "MP3"), "Long Gone.mp3 — MP3");
        assert_eq!(title("noext", None, ""), "noext");
    }

    #[test]
    fn recent_moves_to_front_and_caps() {
        let mut recent: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!("/{i}.mp3"))).collect();
        push_recent(&mut recent, &PathBuf::from("/5.mp3"));
        assert_eq!(recent.len(), 10);
        assert_eq!(recent[0], PathBuf::from("/5.mp3"));
        assert_eq!(recent.iter().filter(|p| **p == PathBuf::from("/5.mp3")).count(), 1);
    }

    #[test]
    fn speed_snaps_to_the_grid() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(snap_speed(1.37), 1.35));
        assert!(close(snap_speed(1.026), 1.05));
        assert!(close(snap_speed(0.3), 0.5));
        assert!(close(snap_speed(2.6), 2.0));
    }

    #[test]
    fn volume_curve() {
        assert_eq!(volume_gain(0.0), 0.0);
        assert_eq!(volume_gain(1.0), 1.0);
        assert!(volume_gain(0.5) > volume_gain(0.4) && volume_gain(0.5) < 0.5);
        assert_eq!(volume_gain(2.0), 1.0);
    }

    #[test]
    fn marquee_rests_scrolls_and_returns() {
        assert_eq!(marquee(100.0, 200.0, 5.0), 0.0, "fits: never moves");
        // 60 pt too wide: rest 1.5 s, 2 s out, rest 1.5 s, 2 s back; a 7 s cycle.
        assert_eq!(marquee(260.0, 200.0, 0.0), 0.0);
        assert_eq!(marquee(260.0, 200.0, 1.4), 0.0);
        assert_eq!(marquee(260.0, 200.0, 2.5), 30.0);
        assert_eq!(marquee(260.0, 200.0, 4.0), 60.0);
        assert_eq!(marquee(260.0, 200.0, 6.0), 30.0);
        assert_eq!(marquee(260.0, 200.0, 7.0), 0.0);
        for i in 0..200 {
            let x = marquee(260.0, 200.0, i as f64 * 0.05);
            assert!((0.0..=60.0).contains(&x), "{x}");
        }
    }
}
