//! Settings in `settings.txt` next to eframe's own storage, read before the window opens.

use std::path::{Path, PathBuf};

// shortcut: hand-written key=value lines; move to serde + ron (new dependencies, ask first) when settings grow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// Reopen at the last window size instead of the phone-shaped default.
    pub remember_window_size: bool,
    /// Show a waveform strip instead of the plain seek bar.
    pub show_waveform: bool,
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
        let on = value.trim() == "true";
        match key.trim() {
            "remember_window_size" => s.remember_window_size = on,
            "show_waveform" => s.show_waveform = on,
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

    /// Writes a temporary file and renames it over the old one, so a crash never leaves half a file.
    fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, format!("remember_window_size={}\nshow_waveform={}\n", self.remember_window_size, self.show_waveform))?;
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
        let s = Settings { remember_window_size: true, show_waveform: true };
        s.write_to(&path).unwrap();
        let back = parse(&std::fs::read_to_string(&path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, s);
        assert_eq!(parse("garbage\n=\nremember_window_size=maybe"), Settings::default());
        assert_eq!(parse(""), Settings::default());
    }
}
