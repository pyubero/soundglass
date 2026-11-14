//! Settings, kept like lightview's: a RON value under `KEY` in eframe's storage
//! (`~/.local/share/soundglass/app.ron`), read before the window opens.

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Key in eframe's storage.
pub const KEY: &str = "settings";

/// Track Info's width when nothing is saved.
pub const INFO_WIDTH: f32 = 320.0;
/// The Edit panel's height when nothing is saved.
pub const EDIT_HEIGHT: f32 = 160.0;
/// The interface's font size choices and default, in points (lightview's).
pub const FONT_SIZES: [f32; 8] = [10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0];
pub const DEFAULT_FONT_SIZE: f32 = 13.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Reopen with the window, Track Info and the Edit panel as they were at exit, instead of the phone-shaped default.
    pub remember_window_size: bool,
    /// Show a waveform strip instead of the plain seek bar.
    pub show_waveform: bool,
    /// Practice mode: the v2 tools (loop, spectrogram, tempo, key and chords) shown and analysed.
    pub practice: bool,
    /// The whole window at exit in points, Track Info and the Edit panel included when they were open.
    pub window_size: Option<[f32; 2]>,
    /// Whether Track Info was open, and its width.
    pub info_panel: bool,
    pub info_width: f32,
    /// Whether the Folder panel was open, and its width.
    pub folder_panel: bool,
    pub folder_width: f32,
    /// Whether the Edit panel was open, and its height.
    pub edit_panel: bool,
    pub edit_height: f32,
    /// File → Rename from Tags…'s pattern, as last used.
    pub rename_pattern: String,
    /// The Spectrogram's note lines: shown or not, the tuning, how they're named and drawn.
    pub spectrum_notes: bool,
    /// The Spectrogram's chord bar.
    pub spectrum_chords: bool,
    pub note_a4: f32,
    /// Do Ré Mi instead of C D E.
    pub note_solfege: bool,
    pub note_flats: bool,
    pub note_octaves: bool,
    /// Only the notes of this major scale (an index into `spectrum::KEYS`); `None` for all notes.
    pub note_scale: Option<usize>,
    /// Only the notes in this instrument's range.
    pub note_instrument: crate::spectrum::Instrument,
    /// The colours of the C lines and of the others, opacity included.
    pub note_c_color: Color32,
    pub note_color: Color32,
    pub note_c_width: f32,
    pub note_label_size: f32,
    /// How the Spectrogram is analysed and coloured.
    pub spectrum: crate::spectrum::Params,
    /// `theme::SYSTEM`, a shipped theme's id, or the file name (without `.xml`) of a user theme.
    pub theme: String,
    /// The cover fills its space; otherwise it is shown at most 256 px.
    pub cover_fit: bool,
    /// The visualiser shows in place of the cover.
    pub visualiser: bool,
    /// Local AI: the model each tool uses, and whether to use the GPU.
    pub ai: AiSettings,
    /// What plays when a track ends, and what Next and Previous move to.
    pub order: Order,
    /// The volume slider's position, 0 to 1.
    pub volume: f32,
    /// The interface's font size in points, one of `FONT_SIZES`.
    pub font_size: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Order {
    /// The folder's tracks one after another, then stop.
    #[default]
    InOrder,
    /// The same track again and again.
    RepeatOne,
    /// Each of the folder's tracks once, in a random order, then stop.
    Shuffle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// The chosen model file per tool; missing means the first installed one.
    pub models: HashMap<crate::ai::Task, String>,
    pub use_gpu: bool,
    /// Each model's settings, by model file; missing means its defaults.
    pub tune: HashMap<String, crate::ai::Tune>,
    /// The tools write 24-bit FLAC, else Ogg Vorbis q6.
    pub lossless: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings { models: HashMap::new(), use_gpu: true, tune: HashMap::new(), lossless: false }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            remember_window_size: false, show_waveform: false, practice: false, window_size: None, info_panel: false, info_width: INFO_WIDTH, folder_panel: false, folder_width: INFO_WIDTH, edit_panel: false, edit_height: EDIT_HEIGHT, rename_pattern: crate::tags::DEFAULT_PATTERN.into(),
            spectrum_notes: false,
            spectrum_chords: false,
            note_a4: 440.0,
            note_solfege: false,
            note_flats: false,
            note_octaves: true,
            note_scale: None,
            note_instrument: crate::spectrum::Instrument::None,
            note_c_color: NOTE_C_COLOR,
            note_color: NOTE_COLOR,
            note_c_width: 2.0,
            note_label_size: 11.0,
            spectrum: Default::default(),
            theme: crate::theme::SYSTEM.into(),
            cover_fit: false,
            visualiser: false,
            ai: AiSettings::default(),
            order: Order::InOrder,
            volume: 1.0,
            font_size: DEFAULT_FONT_SIZE,
        }
    }
}

/// The note lines' colours when nothing is saved: C a warm amber at 60 %, the others white at 30 %.
const NOTE_C_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(255, 200, 90, 153);
const NOTE_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 77);

/// Scales every text style from egui's defaults, so calling it again never compounds. From lightview.
pub fn apply_font_size(ctx: &eframe::egui::Context, size: f32) {
    // The saved file can be edited by hand.
    let factor = size.clamp(FONT_SIZES[0], FONT_SIZES[FONT_SIZES.len() - 1]) / DEFAULT_FONT_SIZE;
    ctx.all_styles_mut(|style| {
        style.text_styles = eframe::egui::style::default_text_styles();
        style.text_styles.values_mut().for_each(|font| font.size *= factor);
    });
}

/// Reads the settings straight from eframe's storage file, so the window size is known before
/// the window is created; `App::save` writes them back through eframe. Anything missing or
/// unreadable falls back to the defaults.
pub fn load() -> Settings {
    let path = eframe::storage_dir("soundglass").map(|d| d.join("app.ron"));
    path.and_then(|p| std::fs::read_to_string(p).ok()).map(|s| parse(&s)).unwrap_or_default()
}

fn parse(app_ron: &str) -> Settings {
    ron::from_str::<HashMap<String, String>>(app_ron).ok().and_then(|kv| ron::from_str::<Settings>(kv.get(KEY)?).ok()).unwrap_or_default().checked()
}

impl Settings {
    /// Back within range: the file can be edited by hand.
    fn checked(mut self) -> Settings {
        let d = Settings::default();
        let positive = |n: f32| n.is_finite() && n > 0.0;
        self.window_size = self.window_size.filter(|s| s.iter().all(|&n| positive(n)));
        self.info_width = if positive(self.info_width) { self.info_width } else { d.info_width };
        self.folder_width = if positive(self.folder_width) { self.folder_width } else { d.folder_width };
        self.edit_height = if positive(self.edit_height) { self.edit_height } else { d.edit_height };
        self.note_a4 = if positive(self.note_a4) { self.note_a4.clamp(400.0, 480.0) } else { d.note_a4 };
        self.note_c_width = if positive(self.note_c_width) { self.note_c_width.min(6.0) } else { d.note_c_width };
        self.note_label_size = if positive(self.note_label_size) { self.note_label_size.clamp(6.0, 24.0) } else { d.note_label_size };
        self.note_scale = self.note_scale.filter(|&k| k < 12);
        if self.theme.is_empty() {
            self.theme = d.theme.clone();
        }
        if self.rename_pattern.is_empty() {
            self.rename_pattern = d.rename_pattern;
        }
        self.spectrum = self.spectrum.checked();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let s = Settings {
            remember_window_size: true, show_waveform: true, practice: true, window_size: Some([700.0, 680.0]), info_panel: true, info_width: 300.0, folder_panel: true, folder_width: 250.0, edit_panel: true, edit_height: 200.0, rename_pattern: "{artist} - {title}".into(),
            spectrum_notes: true,
            spectrum_chords: true,
            note_a4: 432.0,
            note_solfege: true,
            note_flats: true,
            note_octaves: false,
            note_scale: Some(10),
            note_instrument: crate::spectrum::Instrument::TenorSax,
            note_c_color: Color32::from_rgba_unmultiplied(10, 200, 30, 255),
            note_color: Color32::from_rgba_unmultiplied(0, 0, 255, 255),
            note_c_width: 3.0,
            note_label_size: 14.0,
            cover_fit: true,
            visualiser: true,
            spectrum: crate::spectrum::Params {
                fft: 8192,
                window: 2048,
                hop: 1024,
                shape: crate::spectrum::Shape::BlackmanHarris,
                floor: -120.0,
                ceiling: -6.0,
                low: 40.0,
                map: crate::spectrum::ColourMap::BlueRed,
            },
            theme: "high-contrast".into(),
            ai: AiSettings { models: HashMap::from([(crate::ai::Task::Stems, "m.onnx".into())]), use_gpu: false, lossless: true, tune: HashMap::from([("nmp.onnx".to_string(), crate::ai::Tune::Beats(Default::default()))]) },
            order: Order::Shuffle,
            volume: 0.4,
        };
        let app_ron = |v: &str| ron::to_string(&HashMap::from([(KEY.to_string(), v.to_string())])).unwrap();
        assert_eq!(parse(&app_ron(&ron::to_string(&s).unwrap())), s);
        for junk in ["", "garbage", "{\"recent\": \"x\"}"] {
            assert_eq!(parse(junk), Settings::default());
        }
        assert_eq!(parse(&app_ron("not ron")), Settings::default());
        // Only some fields: the rest keep their defaults.
        let some = parse(&app_ron("(practice: true, spectrum: (fft: 8192))"));
        assert_eq!((some.practice, some.spectrum.fft, some.note_a4, some.spectrum.hop), (true, 8192, 440.0, crate::spectrum::Params::default().hop));
        let bad = parse(&app_ron("(note_a4: 1000, info_width: -3, edit_height: 0, window_size: Some((700, -1)), note_scale: Some(12), note_label_size: 99, rename_pattern: \"\", theme: \"\", spectrum: (fft: 3000, window: 99999))"));
        assert_eq!((bad.note_a4, bad.info_width, bad.edit_height, bad.window_size, bad.note_scale, bad.note_label_size), (480.0, INFO_WIDTH, EDIT_HEIGHT, None, None, 24.0));
        assert_eq!((bad.rename_pattern.as_str(), bad.theme.as_str()), (crate::tags::DEFAULT_PATTERN, crate::theme::SYSTEM));
        assert_eq!(bad.spectrum, crate::spectrum::Params { fft: 3000, window: 99999, ..Default::default() }.checked());
    }
}
