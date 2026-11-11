//! Settings in `settings.txt` next to eframe's own storage, read before the window opens.

use std::path::{Path, PathBuf};

/// Track Info's width when nothing is saved.
pub const INFO_WIDTH: f32 = 320.0;

// shortcut: hand-written key=value lines; move to serde + ron (new dependencies, ask first) when settings grow.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Reopen with the window and Track Info as they were at exit, instead of the phone-shaped default.
    pub remember_window_size: bool,
    /// Show a waveform strip instead of the plain seek bar.
    pub show_waveform: bool,
    /// The whole window at exit in points, Track Info included when it was open.
    pub window_size: Option<[f32; 2]>,
    /// Whether Track Info was open, and its width.
    pub info_panel: bool,
    pub info_width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { remember_window_size: false, show_waveform: false, window_size: None, info_panel: false, info_width: INFO_WIDTH }
    }
}

fn path() -> Option<PathBuf> {
    eframe::storage_dir("soundglass").map(|d| d.join("settings.txt"))
}

/// The saved settings; anything missing or unreadable is the default.
pub fn load() -> Settings {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| parse(&s)).unwrap_or_default()
}

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
        let (value, on) = (value.trim(), value.trim() == "true");
        let number = |v: &str| v.trim().parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0);
        match key.trim() {
            "remember_window_size" => s.remember_window_size = on,
            "show_waveform" => s.show_waveform = on,
            "window_size" => s.window_size = value.split_once(',').and_then(|(w, h)| Some([number(w)?, number(h)?])),
            "info_panel" => s.info_panel = on,
            "info_width" => s.info_width = number(value).unwrap_or(INFO_WIDTH),
            _ => {}
        }
    }
    s
}

impl Settings {
    pub fn save(&self) -> Result<(), String> {
        let path = path().ok_or("no folder for settings")?;
        self.write_to(&path).map_err(|e| format!("could not save settings: {e}"))
    }

    fn text(&self) -> String {
        let mut text = format!(
            "remember_window_size={}\nshow_waveform={}\ninfo_panel={}\ninfo_width={}\n",
            self.remember_window_size, self.show_waveform, self.info_panel, self.info_width
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
        let s = Settings { remember_window_size: true, show_waveform: true, window_size: Some([700.0, 680.0]), info_panel: true, info_width: 300.0 };
        s.write_to(&path).unwrap();
        let back = parse(&std::fs::read_to_string(&path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, s);
        assert_eq!(parse("garbage\n=\nremember_window_size=maybe\nwindow_size=wide,-3\ninfo_width=nan"), Settings::default());
        let defaults = parse("");
        assert_eq!((defaults.window_size, defaults.info_panel, defaults.info_width), (None, false, INFO_WIDTH));
    }
}
