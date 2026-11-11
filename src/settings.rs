//! Settings in `settings.txt` next to eframe's own storage, read before the window opens.

use eframe::egui::Color32;
use std::path::{Path, PathBuf};

/// Track Info's width when nothing is saved.
pub const INFO_WIDTH: f32 = 320.0;
/// The Edit panel's height when nothing is saved.
pub const EDIT_HEIGHT: f32 = 160.0;

// shortcut: hand-written key=value lines; move to serde + ron (new dependencies, ask first) when settings grow.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Reopen with the window, Track Info and the Edit panel as they were at exit, instead of the phone-shaped default.
    pub remember_window_size: bool,
    /// Show a waveform strip instead of the plain seek bar.
    pub show_waveform: bool,
    /// The whole window at exit in points, Track Info and the Edit panel included when they were open.
    pub window_size: Option<[f32; 2]>,
    /// Whether Track Info was open, and its width.
    pub info_panel: bool,
    pub info_width: f32,
    /// Whether the Edit panel was open, and its height.
    pub edit_panel: bool,
    pub edit_height: f32,
    /// File → Rename from Tags…'s pattern, as last used.
    pub rename_pattern: String,
    /// The Spectrogram's note lines: shown or not, the tuning, how they're named and drawn.
    pub spectrum_notes: bool,
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            remember_window_size: false, show_waveform: false, window_size: None, info_panel: false, info_width: INFO_WIDTH, edit_panel: false, edit_height: EDIT_HEIGHT, rename_pattern: crate::tags::DEFAULT_PATTERN.into(),
            spectrum_notes: false,
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
        }
    }
}

/// The note lines' colours when nothing is saved: C a warm amber at 60 %, the others white at 30 %.
const NOTE_C_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(255, 200, 90, 153);
const NOTE_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 77);

/// `rrggbbaa`, unmultiplied.
fn to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    format!("{r:02x}{g:02x}{b:02x}{a:02x}")
}

fn hex(v: &str) -> Option<Color32> {
    let byte = |i: usize| u8::from_str_radix(v.get(i..i + 2)?, 16).ok();
    (v.len() == 8).then_some(())?;
    Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, byte(6)?))
}

fn path() -> Option<PathBuf> {
    eframe::storage_dir("soundglass").map(|d| d.join("settings.txt"))
}

/// The saved settings; anything missing or unreadable is the default.
pub fn load() -> Settings {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| parse(&s)).unwrap_or_default()
}

fn parse(text: &str) -> Settings {
    use crate::spectrum::{ColourMap, Instrument, Shape, KEYS};
    let mut s = Settings::default();
    // "Natural notes only", before Scale shown replaced it, is C major.
    let (mut naturals, mut scale) = (false, None);
    let float = |v: &str| v.trim().parse::<f32>().ok().filter(|n| n.is_finite());
    let count = |v: &str| v.trim().parse::<usize>().ok();
    for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
        let (value, on) = (value.trim(), value.trim() == "true");
        let number = |v: &str| v.trim().parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0);
        match key.trim() {
            "remember_window_size" => s.remember_window_size = on,
            "show_waveform" => s.show_waveform = on,
            "window_size" => s.window_size = value.split_once(',').and_then(|(w, h)| Some([number(w)?, number(h)?])),
            "info_panel" => s.info_panel = on,
            "info_width" => s.info_width = number(value).unwrap_or(INFO_WIDTH),
            "edit_panel" => s.edit_panel = on,
            "edit_height" => s.edit_height = number(value).unwrap_or(EDIT_HEIGHT),
            "rename_pattern" if !value.is_empty() => s.rename_pattern = value.into(),
            "spectrum_notes" => s.spectrum_notes = on,
            "note_a4" => s.note_a4 = number(value).map_or(440.0, |n| n.clamp(400.0, 480.0)),
            "note_solfege" => s.note_solfege = on,
            "note_flats" => s.note_flats = on,
            "note_octaves" => s.note_octaves = on,
            "note_naturals" => naturals = on,
            "note_scale" => scale = Some(KEYS.iter().position(|k| k.0 == value)),
            "note_instrument" => s.note_instrument = Instrument::from_name(value).unwrap_or(Instrument::None),
            "note_c_color" => s.note_c_color = hex(value).unwrap_or(NOTE_C_COLOR),
            "note_color" => s.note_color = hex(value).unwrap_or(NOTE_COLOR),
            "note_c_width" => s.note_c_width = number(value).map_or(2.0, |n| n.min(6.0)),
            "spec_fft" => s.spectrum.fft = count(value).unwrap_or(s.spectrum.fft),
            "spec_window" => s.spectrum.window = count(value).unwrap_or(s.spectrum.window),
            "spec_hop" => s.spectrum.hop = count(value).unwrap_or(s.spectrum.hop),
            "spec_rows" => s.spectrum.rows = count(value).unwrap_or(s.spectrum.rows),
            "spec_shape" => s.spectrum.shape = Shape::from_name(value).unwrap_or(s.spectrum.shape),
            "spec_floor" => s.spectrum.floor = float(value).unwrap_or(s.spectrum.floor),
            "spec_ceiling" => s.spectrum.ceiling = float(value).unwrap_or(s.spectrum.ceiling),
            "spec_low" => s.spectrum.low = float(value).unwrap_or(s.spectrum.low),
            "spec_colours" => s.spectrum.map = ColourMap::from_name(value).unwrap_or(s.spectrum.map),
            "note_label_size" => s.note_label_size = number(value).map_or(11.0, |n| n.clamp(6.0, 24.0)),
            _ => {}
        }
    }
    s.spectrum = s.spectrum.checked();
    s.note_scale = scale.unwrap_or(naturals.then_some(0));
    s
}

impl Settings {
    pub fn save(&self) -> Result<(), String> {
        let path = path().ok_or("no folder for settings")?;
        self.write_to(&path).map_err(|e| format!("could not save settings: {e}"))
    }

    fn text(&self) -> String {
        let mut text = format!(
            "remember_window_size={}\nshow_waveform={}\ninfo_panel={}\ninfo_width={}\nedit_panel={}\nedit_height={}\n",
            self.remember_window_size, self.show_waveform, self.info_panel, self.info_width, self.edit_panel, self.edit_height
        );
        // One line: a pattern can't hold a line break, the dialog's field is single-line.
        text += &format!("rename_pattern={}\n", self.rename_pattern.replace(['\n', '\r'], " "));
        text += &format!(
            "spectrum_notes={}\nnote_a4={}\nnote_solfege={}\nnote_flats={}\nnote_octaves={}\nnote_scale={}\nnote_instrument={}\nnote_c_color={}\nnote_color={}\nnote_c_width={}\nnote_label_size={}\n",
            self.spectrum_notes, self.note_a4, self.note_solfege, self.note_flats, self.note_octaves, self.note_scale.map_or("all", |k| crate::spectrum::KEYS[k % 12].0), self.note_instrument.name(), to_hex(self.note_c_color), to_hex(self.note_color), self.note_c_width, self.note_label_size
        );
        let p = self.spectrum;
        text += &format!(
            "spec_fft={}\nspec_window={}\nspec_hop={}\nspec_rows={}\nspec_shape={}\nspec_floor={}\nspec_ceiling={}\nspec_low={}\nspec_colours={}\n",
            p.fft, p.window, p.hop, p.rows, p.shape.name(), p.floor, p.ceiling, p.low, p.map.name()
        );
        if let Some([w, h]) = self.window_size {
            text += &format!("window_size={w},{h}\n");
        }
        text
    }

    /// Writes a temporary file and renames it over the old one, so a crash never leaves half a file.
    fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, self.text())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("soundglass-settings-{}", std::process::id()));
        let path = dir.join("settings.txt");
        let s = Settings {
            remember_window_size: true, show_waveform: true, window_size: Some([700.0, 680.0]), info_panel: true, info_width: 300.0, edit_panel: true, edit_height: 200.0, rename_pattern: "{artist} - {title}".into(),
            spectrum_notes: true,
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
            spectrum: crate::spectrum::Params {
                fft: 8192,
                window: 2048,
                hop: 1024,
                rows: 2048,
                shape: crate::spectrum::Shape::BlackmanHarris,
                floor: -120.0,
                ceiling: -6.0,
                low: 40.0,
                map: crate::spectrum::ColourMap::BlueRed,
            },
        };
        s.write_to(&path).unwrap();
        let back = parse(&std::fs::read_to_string(&path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, s);
        assert_eq!(parse("garbage\n=\nremember_window_size=maybe\nwindow_size=wide,-3\ninfo_width=nan"), Settings::default());
        let defaults = parse("");
        assert_eq!((defaults.window_size, defaults.info_panel, defaults.info_width), (None, false, INFO_WIDTH));
        assert_eq!((defaults.edit_panel, defaults.edit_height), (false, EDIT_HEIGHT));
        assert_eq!((defaults.spectrum_notes, defaults.note_a4, defaults.note_octaves), (false, 440.0, true));
        assert_eq!(parse("note_a4=1000\nnote_opacity=-1").note_a4, 480.0);
        assert_eq!(parse("note_naturals=true").note_scale, Some(0));
        assert_eq!(parse("note_naturals=true\nnote_scale=all").note_scale, None);
        assert_eq!((parse("note_scale=D").note_scale, parse("note_scale=H").note_scale), (Some(2), None));
        assert_eq!(parse("note_color=zz00ff80\nnote_c_color=fff").note_color, NOTE_COLOR);
        assert_eq!(parse("spec_fft=3000\nspec_window=99999\nspec_shape=Kaiser").spectrum, crate::spectrum::Params::default().checked());
    }
}
