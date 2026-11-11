// No console window behind the app on Windows (debug builds keep it, for --timings output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod folder;
mod player;
mod settings;
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
    // Started from the app menu, with no file: ask for one.
    // shortcut: no empty window to open from; give App an Option<Track> if an empty start is wanted.
    let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter("Audio", &folder::EXTENSIONS).pick_file()) else {
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
    // A vertical phone's shape, about 9:16.
    let viewport = egui::ViewportBuilder::default().with_title(&name).with_inner_size([380.0, 680.0]).with_min_inner_size([260.0, 420.0]);
    let settings = settings::load();
    let options = eframe::NativeOptions { viewport, persist_window: settings.remember_window_size, ..Default::default() };
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        cc.egui_ctx.set_fonts(fonts());
        let player = opening.join().unwrap_or_else(|_| Err("the sound device failed to open".into()));
        if let Ok(p) = &player {
            p.send(Cmd::Load(track.clone()));
        }
        Ok(Box::new(App { track, next: None, files, index, advanced: 0, ctx, cover: None, settings, volume: 1.0, speed: 1.0, custom_speed: None, show_settings: false, settings_error: None, player, t0, timings, shown: false }))
    }))
}

/// The UI's fonts, from lightview: Apache-2.0 and MIT only, since egui's default set isn't
/// clearly GPL-compatible. shortcut: no CJK font, so Chinese/Japanese/Korean tags show as boxes;
/// copy lightview's system CJK font lookup (`markup::read_cjk_font`) when that matters.
fn fonts() -> egui::FontDefinitions {
    use egui::{FontData, FontFamily, FontTweak};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("Roboto".into(), Arc::new(FontData::from_static(include_bytes!("../fonts/Roboto-Regular.ttf"))));
    fonts.font_data.insert("Droid Sans Mono".into(), Arc::new(FontData::from_static(include_bytes!("../fonts/DroidSansMono.ttf"))));
    let icons = FontData::from_static(include_bytes!("../fonts/emoji-icon-font.ttf")).tweak(FontTweak { scale: 0.90, ..Default::default() });
    fonts.font_data.insert("emoji-icon-font".into(), Arc::new(icons));
    let family = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, family(&["Roboto", "emoji-icon-font"]));
    fonts.families.insert(FontFamily::Monospace, family(&["Droid Sans Mono", "Roboto", "emoji-icon-font"]));
    fonts
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
    settings: settings::Settings,
    /// The volume slider's position, 0 to 1; see `volume_gain`.
    volume: f32,
    speed: f64,
    /// The Custom speed dialog's value while it is open.
    custom_speed: Option<f64>,
    show_settings: bool,
    settings_error: Option<String>,
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
    /// Slider position, 0 to 1.
    Volume(f32),
    Speed(f64),
}

/// The speeds Playback → Change speed offers directly.
const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Custom speeds move in this step.
const SPEED_STEP: f64 = 0.05;

/// `x` rounded to the nearest 0.05 and kept within 0.5×–2×.
fn snap_speed(x: f64) -> f64 {
    ((x / SPEED_STEP).round() * SPEED_STEP).clamp(0.5, 2.0)
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
        let open_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);
        let mut open_file = ctx.input_mut(|i| i.consume_shortcut(&open_key));
        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("Open…").shortcut_text(ctx.format_shortcut(&open_key))).clicked() {
                        open_file = true;
                    }
                    if ui.button("Settings…").clicked() {
                        self.show_settings = true;
                    }
                    ui.separator();
                    ui.separator();
                    if ui.add(egui::Button::new("Quit").shortcut_text(ctx.format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q)))).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.checkbox(&mut self.settings.show_waveform, "Waveform").changed() {
                        self.settings_error = self.settings.save().err();
                    }
                });
                ui.menu_button("Playback", |ui| {
                    ui.menu_button("Change speed", |ui| {
                        for speed in SPEEDS {
                            let label = if speed == 1.0 { "1× (Normal)".to_string() } else { format!("{speed}×") };
                            if ui.radio((self.speed - speed).abs() < 1e-6, label).clicked() {
                                actions.push(Action::Speed(speed));
                                ui.close();
                            }
                        }
                        let custom = !SPEEDS.iter().any(|s| (self.speed - s).abs() < 1e-6);
                        let label = if custom { format!("Custom ({:.2}×)…", self.speed) } else { "Custom…".into() };
                        if ui.radio(custom, label).clicked() {
                            self.custom_speed = Some(self.speed);
                            ui.close();
                        }
                    });
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
                let wave = self.settings.show_waveform.then_some(&*self.track);
                if let Some(frame) = seek_bar(ui, pos, len, rate, wave) {
                    actions.push(Action::Seek(frame));
                }
                ui.add_space(8.0);
                actions.extend(transport(ui, playing, has_next, self.volume));
            });
        });

        self.settings_window(&ctx);
        if let Some(speed) = self.speed_window(&ctx) {
            actions.push(Action::Speed(speed));
        }
        if open_file && let Some(path) = rfd::FileDialog::new().add_filter("Audio", &folder::EXTENSIONS).pick_file() {
            self.open(path);
        }
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
            Action::Speed(speed) => {
                self.speed = speed;
                p.send(Cmd::Speed(speed));
            }
            Action::Volume(v) => {
                self.volume = v;
                p.send(Cmd::Volume(volume_gain(v)));
            }
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

    /// The Custom speed dialog; returns the speed when OK is pressed.
    fn speed_window(&mut self, ctx: &egui::Context) -> Option<f64> {
        let mut draft = self.custom_speed?;
        let (mut chosen, mut close) = (None, false);
        egui::Window::new("Custom speed").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("−").clicked() {
                    draft -= SPEED_STEP;
                }
                ui.add(egui::DragValue::new(&mut draft).range(0.5..=2.0).speed(0.01).fixed_decimals(2).suffix("×"));
                if ui.button("+").clicked() {
                    draft += SPEED_STEP;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = Some(snap_speed(draft));
                    close = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        self.custom_speed = (!close).then_some(snap_speed(draft));
        chosen
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            if ui.checkbox(&mut self.settings.remember_window_size, "Remember window size").changed() {
                self.settings_error = self.settings.save().err();
            }
            ui.weak("Takes effect the next time soundglass opens.");
            if let Some(e) = &self.settings_error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
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
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.files = folder::list_folder(&path);
        self.next = None;
        let index = self.files.iter().position(|p| *p == path).unwrap_or(0);
        self.go(index);
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

/// A slim bar, or `wave`'s waveform, with the times at its ends; returns the frame to seek to
/// when clicked or dragged.
fn seek_bar(ui: &mut egui::Ui, pos: u64, len: u64, rate: u64, wave: Option<&Track>) -> Option<u64> {
    let height = if wave.is_some() { 48.0 } else { 18.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    let seconds = pos.checked_div(rate).unwrap_or(0) as f64;
    response.widget_info(|| egui::WidgetInfo::slider(true, seconds, "Position, seconds"));
    let visuals = ui.visuals();
    let fraction = (pos as f32 / len.max(1) as f32).min(1.0);
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let x = bar.left() + bar.width() * fraction;
    let painter = ui.painter_at(rect);
    if let Some(track) = wave {
        draw_waveform(&painter, visuals, rect, track, x);
    } else {
        painter.rect_filled(bar, 2.0, visuals.widgets.inactive.bg_fill);
        painter.rect_filled(egui::Rect::from_min_max(bar.min, egui::pos2(x, bar.max.y)), 2.0, visuals.selection.bg_fill);
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

/// The whole track across `rect`, played part (left of `played`) highlighted, with the playhead.
/// Fills in as decoding goes. From the first waveform view (a61e538).
fn draw_waveform(painter: &egui::Painter, visuals: &egui::Visuals, rect: egui::Rect, track: &Track, played: f32) {
    let len = track.len().max(1);
    let (mid, half) = (rect.center().y, rect.height() / 2.0);
    let columns = rect.width() as u64;
    let peaks = track.peaks.lock().unwrap();
    for c in 0..columns {
        let a = (c * len / columns) as usize / track::PEAK;
        let b = ((c + 1) * len / columns) as usize / track::PEAK;
        let Some(slice) = peaks.get(a..(b.max(a + 1)).min(peaks.len())).filter(|s| !s.is_empty()) else { break };
        let (lo, hi) = slice.iter().fold((0f32, 0f32), |(l, h), &(a, b)| (l.min(a), h.max(b)));
        let x = rect.left() + c as f32 + 0.5;
        let color = if x < played { visuals.selection.bg_fill } else { visuals.text_color() };
        // At least one point tall, so silence still shows as a line.
        let (top, bottom) = ((mid - hi * half).min(mid - 0.5), (mid - lo * half).max(mid + 0.5));
        painter.line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], egui::Stroke::new(1.0, color));
    }
    painter.vline(played, rect.y_range(), egui::Stroke::new(1.5, visuals.strong_text_color()));
}

#[derive(Clone, Copy)]
enum Icon {
    Volume { muted: bool },
    Previous,
    Play,
    Pause,
    Next,
}

/// Volume, then previous, play/pause and next, with play/pause centred.
fn transport(ui: &mut egui::Ui, playing: bool, has_next: bool, volume: f32) -> Vec<Action> {
    let (size, small, gap) = (56.0, 40.0, 16.0);
    let buttons = [
        (Icon::Previous, "Previous", true, Action::Previous),
        (if playing { Icon::Pause } else { Icon::Play }, if playing { "Pause" } else { "Play" }, true, Action::Toggle),
        (Icon::Next, "Next", has_next, Action::Next),
    ];
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        // Counted as if the volume button had a twin on the right, so play/pause stays centred.
        let row = 3.0 * size + 2.0 * gap + 2.0 * (small + gap);
        ui.add_space(((ui.available_width() - row) / 2.0).max(0.0));
        let label = format!("Volume, {:.0} %", volume * 100.0);
        let button = icon_button(ui, Icon::Volume { muted: volume == 0.0 }, &label, true, small);
        let mut v = volume;
        egui::Popup::from_toggle_button_response(&button)
            .align(egui::RectAlign::TOP)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false).text("Volume")));
        if v != volume {
            actions.push(Action::Volume(v));
        }
        for (icon, label, enabled, action) in buttons {
            if icon_button(ui, icon, label, enabled, size).clicked() {
                actions.push(action);
            }
        }
    });
    actions
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
    use super::{marquee, snap_speed, volume_gain};

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
