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
    // A vertical phone's shape, about 9:19.5.
    let viewport = egui::ViewportBuilder::default().with_title(&name).with_inner_size([380.0, 780.0]).with_min_inner_size([260.0, 420.0]);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        let player = opening.join().unwrap_or_else(|_| Err("the sound device failed to open".into()));
        if let Ok(p) = &player {
            p.send(Cmd::Load(track.clone()));
        }
        Ok(Box::new(App { track, next: None, files, index, advanced: 0, ctx, cover: None, player, t0, timings, shown: false }))
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
    /// The cover texture and the track (by address) it belongs to.
    cover: Option<(usize, egui::TextureHandle)>,
    player: Result<Player, String>,
    t0: Instant,
    timings: bool,
    /// Whether the window is on screen; playback starts then.
    shown: bool,
}

/// What the widgets asked for this frame, applied once drawing is done.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Previous,
    Next,
    Toggle,
    Seek(u64),
}

impl eframe::App for App {
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
        self.follow_player(&ctx);

        let player = self.player.as_ref().ok();
        let rate = self.track.format.get().map_or(0, |f| f.rate as u64);
        let (pos, len) = (player.map_or(0, |p| p.shared.pos.load(Relaxed)), self.track.len());
        let playing = player.is_some_and(|p| p.shared.playing.load(Relaxed));
        if playing {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        let mut actions = Vec::new();
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
        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("Quit").shortcut_text(ctx.format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q)))).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });

        let cover = self.cover_texture(&ctx);
        let tags = self.track.tags.get().cloned().unwrap_or_default();
        let error = self.track.error.lock().unwrap().clone().or_else(|| self.player.as_ref().err().cloned());
        let has_next = self.index + 1 < self.files.len();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                // The cover takes the width, but leaves room below for text, bar and buttons.
                let side = ui.available_width().min(ui.available_height() - 190.0).max(64.0);
                cover_art(ui, cover.as_ref(), side);
                ui.add_space(16.0);
                let title = tags.title.unwrap_or_else(|| self.track.path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                let time = ui.input(|i| i.time);
                marquee_label(ui, &title, egui::TextStyle::Heading.resolve(ui.style()), ui.visuals().strong_text_color(), time);
                for line in [tags.artist, tags.album].into_iter().flatten() {
                    marquee_label(ui, &line, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color(), time);
                }
                if let Some(e) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(12.0);
                if let Some(frame) = seek_bar(ui, pos, len, rate) {
                    actions.push(Action::Seek(frame));
                }
                ui.add_space(8.0);
                actions.extend(transport(ui, playing, has_next));
            });
        });

        for action in actions {
            self.apply(action, pos, len);
        }
    }
}

impl App {
    /// Notices the player moving into the queued track, and decodes the next file ahead.
    fn follow_player(&mut self, ctx: &egui::Context) {
        let Ok(p) = &self.player else { return };
        // Tracks the audio callback is done with are freed here, not in the callback.
        while p.retired.try_recv().is_ok() {}
        let advanced = p.shared.advanced.load(Relaxed);
        if advanced != self.advanced {
            self.advanced = advanced;
            if let Some(next) = self.next.take() {
                (self.track, self.index) = (next, self.index + 1);
                self.set_title(ctx);
            }
        }
        // Decode ahead once this track is in memory, so Next and the gapless move are instant.
        if self.track.done() && self.next.is_none() && let Some(path) = self.files.get(self.index + 1) {
            let next = Track::load(path.clone(), repaint(&self.ctx));
            p.send(Cmd::Queue(next.clone()));
            self.next = Some(next);
        }
    }

    fn apply(&mut self, action: Action, pos: u64, len: u64) {
        let Ok(p) = &self.player else { return };
        match action {
            Action::Previous => self.previous(),
            Action::Next => self.go(self.index + 1),
            Action::Seek(frame) => p.send(Cmd::Seek(frame)),
            Action::Toggle if p.shared.playing.load(Relaxed) => p.send(Cmd::Pause),
            Action::Toggle => {
                // At the end, Play starts the track again.
                if self.track.done() && pos >= len {
                    p.send(Cmd::Seek(0));
                }
                p.send(Cmd::Play);
            }
        }
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

/// The cover, fitted in a `side` × `side` square, or a placeholder note when there is none.
fn cover_art(ui: &mut egui::Ui, cover: Option<&egui::TextureHandle>, side: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    match cover {
        Some(tex) => {
            let size = tex.size_vec2();
            let fitted = egui::Rect::from_center_size(rect.center(), size * (side / size.x.max(size.y)));
            egui::Image::from_texture(tex).corner_radius(8.0).paint_at(ui, fitted);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Album cover"));
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
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "No album cover"));
        }
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

/// A slim bar with the times at its ends; returns the frame to seek to when clicked or dragged.
fn seek_bar(ui: &mut egui::Ui, pos: u64, len: u64, rate: u64) -> Option<u64> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::click_and_drag());
    let seconds = pos.checked_div(rate).unwrap_or(0) as f64;
    response.widget_info(|| egui::WidgetInfo::slider(true, seconds, "Position, seconds"));
    let visuals = ui.visuals();
    let fraction = (pos as f32 / len.max(1) as f32).min(1.0);
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let x = bar.left() + bar.width() * fraction;
    let painter = ui.painter();
    painter.rect_filled(bar, 2.0, visuals.widgets.inactive.bg_fill);
    painter.rect_filled(egui::Rect::from_min_max(bar.min, egui::pos2(x, bar.max.y)), 2.0, visuals.selection.bg_fill);
    let knob = if response.hovered() || response.dragged() { 7.0 } else { 5.0 };
    painter.circle_filled(egui::pos2(x, bar.center().y), knob, visuals.strong_text_color());
    ui.horizontal(|ui| {
        ui.label(clock(pos, rate));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(clock(len, rate)));
    });
    let p = response.interact_pointer_pos().filter(|_| response.clicked() || response.dragged())?;
    Some((((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64 * len as f64) as u64)
}

#[derive(Clone, Copy)]
enum Icon {
    Previous,
    Play,
    Pause,
    Next,
}

/// Previous, play/pause and next, centred.
fn transport(ui: &mut egui::Ui, playing: bool, has_next: bool) -> Vec<Action> {
    let (size, gap) = (56.0, 16.0);
    let buttons = [
        (Icon::Previous, "Previous", true, Action::Previous),
        (if playing { Icon::Pause } else { Icon::Play }, if playing { "Pause" } else { "Play" }, true, Action::Toggle),
        (Icon::Next, "Next", has_next, Action::Next),
    ];
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.add_space(((ui.available_width() - 3.0 * size - 2.0 * gap) / 2.0).max(0.0));
        for (icon, label, enabled, action) in buttons {
            if icon_button(ui, icon, label, enabled, size).clicked() {
                actions.push(action);
            }
        }
    });
    actions
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
    }
    response
}

/// `frames` at `rate` as m:ss.
fn clock(frames: u64, rate: u64) -> String {
    let s = frames.checked_div(rate).unwrap_or(0);
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::marquee;

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
