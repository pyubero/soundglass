// No console window behind the app on Windows (debug builds keep it, for --timings output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;

use eframe::egui;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

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
    let Some(path) = path else {
        eprintln!("usage: soundglass FILE [--timings | --bench]");
        std::process::exit(2);
    };
    let path = std::fs::canonicalize(&path).unwrap_or_else(|e| {
        eprintln!("soundglass: {}: {e}", path.display());
        std::process::exit(1);
    });

    // Decoding and opening the sound device overlap the window's OpenGL setup.
    let (error, ctx) = (Arc::new(Mutex::new(None)), Arc::new(OnceLock::new()));
    audio::play(path.clone(), t0, timings, bench, error.clone(), ctx.clone());

    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_title(&name), ..Default::default() };
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        Ok(Box::new(App { name, error, t0, timings, window_reported: false }))
    }))
}

struct App {
    name: String,
    error: Arc<Mutex<Option<String>>>,
    t0: Instant,
    timings: bool,
    window_reported: bool,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.timings && !self.window_reported {
            eprintln!("window      {:.1} ms", ms(self.t0));
            self.window_reported = true;
        }
        ui.heading(&self.name);
        ui.label(self.error.lock().unwrap().as_deref().unwrap_or("Playing"));
    }
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
