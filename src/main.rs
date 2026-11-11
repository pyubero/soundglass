// No console window behind the app on Windows (debug builds keep it, for --timings output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod folder;
mod player;
mod track;

use eframe::egui;
use player::{Cmd, Player};
use std::sync::atomic::Ordering::Relaxed;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use track::Track;

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
    let Some(path) = path else {
        eprintln!("usage: soundglass FILE [--timings | --bench]");
        std::process::exit(2);
    };
    let path = std::fs::canonicalize(&path).unwrap_or_else(|e| {
        eprintln!("soundglass: {}: {e}", path.display());
        std::process::exit(1);
    });

    // Decoding and opening the sound device overlap the window's OpenGL setup.
    let ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let track = Track::load(path.clone(), repaint(&ctx));
    let files = folder::list_folder(&path);
    let index = files.iter().position(|p| *p == path).unwrap_or(0);
    let opening = std::thread::spawn(move || Player::open(t0, timings, bench));

    let name = track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_title(&name), ..Default::default() };
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        let player = opening.join().unwrap_or_else(|_| Err("the sound device failed to open".into()));
        if let Ok(p) = &player {
            p.send(Cmd::Load(track.clone()));
        }
        Ok(Box::new(App { track, next: None, files, index, advanced: 0, ctx, player, t0, timings, shown: false }))
    }))
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
    /// The player's `advanced` count last seen, to notice when it moved into `next` by itself.
    advanced: u64,
    ctx: Arc<OnceLock<egui::Context>>,
    player: Result<Player, String>,
    t0: Instant,
    timings: bool,
    /// Whether the window is on screen; playback starts then.
    shown: bool,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
            self.previous();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::PageDown)) {
            self.go(self.index + 1);
        }
        let player = self.player.as_ref().ok();
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
            if let Some(p) = player {
                p.send(Cmd::Play);
            }
        }

        if let Some(p) = player {
            let advanced = p.shared.advanced.load(Relaxed);
            if advanced != self.advanced {
                self.advanced = advanced;
                if let Some(next) = self.next.take() {
                    (self.track, self.index) = (next, self.index + 1);
                    self.set_title(&ctx);
                }
            }
            // Decode ahead once this track is in memory, so Next and the gapless move are instant.
            if self.track.done() && self.next.is_none() && let Some(path) = self.files.get(self.index + 1) {
                let next = Track::load(path.clone(), repaint(&self.ctx));
                p.send(Cmd::Queue(next.clone()));
                self.next = Some(next);
            }
        }

        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        let (pos, len) = (player.map_or(0, |p| p.shared.pos.load(Relaxed)), self.track.len());
        let playing = player.is_some_and(|p| p.shared.playing.load(Relaxed));
        if let Some(p) = player {
            // Tracks the audio callback is done with are freed here, not in the callback.
            while p.retired.try_recv().is_ok() {}
            let key = |k| ctx.input(|i| i.key_pressed(k));
            let (space, left, right) = (key(egui::Key::Space), key(egui::Key::ArrowLeft), key(egui::Key::ArrowRight));

            if space {
                if playing {
                    p.send(Cmd::Pause);
                } else {
                    if self.track.done() && pos >= len {
                        p.send(Cmd::Seek(0));
                    }
                    p.send(Cmd::Play);
                }
            }
            let step = STEP_SECONDS * rate;
            if left {
                p.send(Cmd::Seek(pos.saturating_sub(step)));
            }
            if right {
                p.send(Cmd::Seek((pos + step).min(len)));
            }
            if playing {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        let error = self.track.error.lock().unwrap().clone().or_else(|| self.player.as_ref().err().cloned());
        let status = match error {
            Some(e) => e,
            None if playing => "Playing".into(),
            None => "Paused".into(),
        };
        egui::Panel::bottom("controls").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} / {}", clock(pos, rate), clock(len, rate)));
                ui.label(status);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(frame) = waveform(ui, &self.track, pos)
                && let Some(p) = player
            {
                p.send(Cmd::Seek(frame));
            }
        });
    }
}

impl App {
    /// Plays file `index` of the folder, using the track decoded ahead when it is that one.
    fn go(&mut self, index: usize) {
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
        if let Some(ctx) = self.ctx.get() {
            self.set_title(ctx);
        }
    }

    fn previous(&mut self) {
        let Ok(p) = &self.player else { return };
        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        if p.shared.pos.load(Relaxed) > RESTART_SECONDS * rate || self.index == 0 {
            p.send(Cmd::Seek(0));
        } else {
            self.go(self.index - 1);
        }
    }

    fn set_title(&self, ctx: &egui::Context) {
        let name = self.track.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(name));
    }
}

/// Draws the whole track across `ui`, played part highlighted, with the playhead. Returns the
/// frame to seek to when it is clicked or dragged.
fn waveform(ui: &mut egui::Ui, track: &Track, pos: u64) -> Option<u64> {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Position"));
    let len = track.len().max(1);
    let (painter, visuals) = (ui.painter_at(rect), ui.visuals());
    let (mid, half) = (rect.center().y, rect.height() / 2.0);
    let played = rect.left() + rect.width() * (pos as f32 / len as f32).min(1.0);
    let columns = rect.width() as u64;
    let peaks = track.peaks.lock().unwrap();
    for c in 0..columns {
        let a = (c * len / columns) as usize / track::PEAK;
        let b = ((c + 1) * len / columns) as usize / track::PEAK;
        // Fills in as decoding goes; columns past what is decoded stay empty.
        let Some(slice) = peaks.get(a..(b.max(a + 1)).min(peaks.len())).filter(|s| !s.is_empty()) else { break };
        let (lo, hi) = slice.iter().fold((0f32, 0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
        let x = rect.left() + c as f32 + 0.5;
        let color = if x < played { visuals.selection.bg_fill } else { visuals.text_color() };
        // At least one point tall, so silence still shows as a line.
        let (top, bottom) = ((mid - hi * half).min(mid - 0.5), (mid - lo * half).max(mid + 0.5));
        painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], egui::Stroke::new(1.0, color));
    }
    painter.vline(played, rect.y_range(), egui::Stroke::new(1.5, visuals.strong_text_color()));
    let p = response.interact_pointer_pos().filter(|_| response.clicked() || response.dragged())?;
    Some((((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * len as f64) as u64)
}

/// `frames` at `rate` as m:ss.
fn clock(frames: u64, rate: u64) -> String {
    let s = frames.checked_div(rate).unwrap_or(0);
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
