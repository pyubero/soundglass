// No console window behind the app on Windows (debug builds keep it, for --timings output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod edit;
mod folder;
mod loudness;
mod player;
mod save;
mod settings;
mod track;

use eframe::egui;
use player::{Cmd, Player};
use std::sync::atomic::Ordering::Relaxed;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use edit::Edit;
use track::Track;

/// The window's size when nothing is remembered: a vertical phone's shape, about 9:16.
const DEFAULT_SIZE: [f32; 2] = [380.0, 680.0];

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
    let settings = settings::load();
    let remembered = settings.window_size.filter(|_| settings.remember_window_size);
    let viewport = egui::ViewportBuilder::default().with_title(&name).with_inner_size(remembered.unwrap_or(DEFAULT_SIZE)).with_min_inner_size([260.0, 420.0]);
    // soundglass keeps the window size itself (see `App::save`): eframe's own saving can't follow Track Info.
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    let (show_info, show_edit) = (remembered.is_some() && settings.info_panel, remembered.is_some() && settings.edit_panel);
    let (window_size, info_width, edit_height) = (remembered, settings.info_width, settings.edit_height);
    eframe::run_native("soundglass", options, Box::new(move |cc| {
        let _ = ctx.set(cc.egui_ctx.clone());
        let mut recent: Vec<PathBuf> = cc.storage.and_then(|s| s.get_string("recent")).unwrap_or_default().lines().map(PathBuf::from).collect();
        push_recent(&mut recent, &track.path);
        cc.egui_ctx.set_fonts(fonts());
        let player = opening.join().unwrap_or_else(|_| Err("the sound device failed to open".into()));
        if let Ok(p) = &player {
            p.send(Cmd::Load(track.clone()));
        }
        Ok(Box::new(App { track, next: None, files, index, advanced: 0, ctx, cover: None, settings, volume: 1.0, speed: 1.0, titled: None, recent, custom_speed: None, show_settings: false, show_info, show_edit, edit_height, edit_view: EditView::default(), history: None, looping: false, sent_region: None, strip_cache: WaveCache::default(), panel_cache: WaveCache::default(), pending: None, show_save: false, save_job: None, save_notice: None, save_choice: save::Choice::default(), controls_height: 260.0, info_width, window_size, settings_error: None, player, t0, timings, shown: false }))
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
    /// The track (by address) the title bar shows, whether its length was exact then, and whether it had unsaved edits.
    titled: Option<(usize, bool, bool)>,
    recent: Vec<PathBuf>,
    /// The Custom speed dialog's value while it is open.
    custom_speed: Option<f64>,
    show_settings: bool,
    show_info: bool,
    show_edit: bool,
    /// The Edit panel's height as last shown.
    edit_height: f32,
    edit_view: EditView,
    /// Edits since the track was opened; `None` until the first one.
    history: Option<edit::History>,
    looping: bool,
    /// The waveform columns of the strip and of the Edit panel, kept until their view changes.
    strip_cache: WaveCache,
    panel_cache: WaveCache,
    /// The selection and Loop last sent to the player, to send them again only when they change.
    sent_region: Option<(Option<(u64, u64)>, bool)>,
    /// Waits for Save As / Discard / Cancel while there are unsaved edits.
    pending: Option<Pending>,
    show_save: bool,
    /// The running save: what it reports on, the edit it writes, the file's name, and whether
    /// the name was claimed by Save Copy (removed again if the save fails).
    save_job: Option<(SaveResult, Arc<Edit>, PathBuf, bool)>,
    /// The last save's result and when it came, shown for a few seconds (errors until the next save).
    save_notice: Option<(Result<String, String>, f64)>,
    /// Save As's last choice, kept while soundglass runs.
    save_choice: save::Choice,
    /// Height of everything below the cover, as measured last frame.
    controls_height: f32,
    /// Track Info's width as last shown.
    info_width: f32,
    /// The window's size in points, last seen while not maximized or fullscreen.
    window_size: Option<[f32; 2]>,
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
    Undo,
    Redo,
    /// Removes the selection.
    Cut,
    /// Keeps only the selection.
    Crop,
    Silence,
}

/// Where to go once unsaved edits are saved or discarded.
enum Pending {
    Go(usize),
    Open(PathBuf),
    Quit,
}

/// Ok(whether the tags were copied), or why the save failed.
type SaveResult = std::sync::mpsc::Receiver<Result<bool, String>>;

/// The speeds Playback → Change speed offers directly.
const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Custom speeds move in this step.
const SPEED_STEP: f64 = 0.05;

/// `x` rounded to the nearest 0.05 and kept within 0.5×–2×.
fn snap_speed(x: f64) -> f64 {
    ((x / SPEED_STEP).round() * SPEED_STEP).clamp(0.5, 2.0)
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
            self.settings_error = self.settings.save().err();
        }
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
            if playing { display_pos(pos, ago, rate, self.speed, self.edit_view.selection, self.looping, len) } else { pos }
        });
        if playing && (self.show_edit || self.settings.show_waveform) {
            // About 30 fps: smooth enough for the playhead, cheaper than redrawing at the screen's rate (user's choice).
            ctx.request_repaint_after(Duration::from_millis(WAVE_REPAINT_MS));
        } else if playing {
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
        let shortcut = |m, k| egui::KeyboardShortcut::new(m, k);
        let (undo_key, redo_key, redo_key2) = (shortcut(egui::Modifiers::COMMAND, egui::Key::Z), shortcut(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z), shortcut(egui::Modifiers::COMMAND, egui::Key::Y));
        let (cut_key, crop_key, silence_key) = (shortcut(egui::Modifiers::NONE, egui::Key::Delete), shortcut(egui::Modifiers::COMMAND, egui::Key::T), shortcut(egui::Modifiers::COMMAND, egui::Key::L));
        // Ctrl+Shift+Z first: Ctrl+Z would match it too, since a shortcut without Shift ignores Shift.
        for (keys, action) in [(&[redo_key, redo_key2][..], Action::Redo), (&[undo_key], Action::Undo), (&[crop_key], Action::Crop), (&[silence_key], Action::Silence)] {
            if keys.iter().any(|k| ctx.input_mut(|i| i.consume_shortcut(k))) {
                actions.push(action);
            }
        }
        if ctx.memory(|m| m.focused().is_none()) && self.custom_speed.is_none() && self.pending.is_none() {
            let plain = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
            if plain(egui::Key::Escape) {
                self.edit_view.selection = None;
            }
            if plain(egui::Key::L) {
                self.looping = !self.looping;
            }
            if plain(egui::Key::Delete) {
                actions.push(Action::Cut);
            }
        }
        let save_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::S);
        let copy_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        // Ctrl+Shift+S first: Ctrl+S would match it too.
        if ctx.input_mut(|i| i.consume_shortcut(&save_key)) && self.track.done() {
            self.show_save = true;
        }
        let mut save_copy = ctx.input_mut(|i| i.consume_shortcut(&copy_key));
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }
        let edit_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::E);
        let mut toggle_edit = ctx.input_mut(|i| i.consume_shortcut(&edit_key));
        let open_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);
        let mut open_file = ctx.input_mut(|i| i.consume_shortcut(&open_key));
        let mut open_recent = None;
        let info_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::I);
        let viewport = ctx.input(|i| i.viewport().clone());
        let resizable = !viewport.maximized.unwrap_or(false) && !viewport.fullscreen.unwrap_or(false);
        // Not `viewport.inner_rect`: it needs the window's position, which Wayland never gives,
        // so it is always None there. The canvas size is the inner size on every platform.
        if resizable {
            let size = ctx.viewport_rect().size();
            self.window_size = Some([size.x, size.y]);
        }
        let mut toggle_info = ctx.input_mut(|i| i.consume_shortcut(&info_key));
        if ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("Open…").shortcut_text(ctx.format_shortcut(&open_key))).clicked() {
                        open_file = true;
                    }
                    ui.add_enabled_ui(!self.recent.is_empty(), |ui| {
                        ui.menu_button("Open Recent", |ui| {
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
                    if ui.add_enabled(self.can_save_copy(), egui::Button::new("Save Copy").shortcut_text(ctx.format_shortcut(&copy_key))).on_hover_text("Saves the edits next to the original, in its format, as name-edited").clicked() {
                        save_copy = true;
                        ui.close();
                    }
                    if ui.add_enabled(self.track.done(), egui::Button::new("Save As…").shortcut_text(ctx.format_shortcut(&save_key))).clicked() {
                        self.show_save = true;
                        ui.close();
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
                ui.menu_button("Edit", |ui| {
                    let (history, selected) = (self.history.as_ref(), self.edit_view.selection.is_some() && self.track.done());
                    let items = [
                        ("Undo", &undo_key, history.is_some_and(|h| h.can_undo()), Action::Undo),
                        ("Redo", &redo_key, history.is_some_and(|h| h.can_redo()), Action::Redo),
                        ("Cut out", &cut_key, selected, Action::Cut),
                        ("Crop", &crop_key, selected, Action::Crop),
                        ("Silence", &silence_key, selected, Action::Silence),
                    ];
                    for (i, (label, key, enabled, action)) in items.into_iter().enumerate() {
                        if i == 2 {
                            ui.separator();
                        }
                        if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(ctx.format_shortcut(key))).clicked() {
                            actions.push(action);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("View", |ui| {
                    let mut on = self.show_info;
                    if ui.add(egui::Checkbox::new(&mut on, "Track Info")).on_hover_text(ctx.format_shortcut(&info_key)).clicked() {
                        toggle_info = true;
                        ui.close();
                    }
                    let mut on = self.show_edit;
                    if ui.add(egui::Checkbox::new(&mut on, "Edit")).on_hover_text(ctx.format_shortcut(&edit_key)).clicked() {
                        toggle_edit = true;
                        ui.close();
                    }
                    if ui.checkbox(&mut self.settings.show_waveform, "Waveform").changed() {
                        self.settings_error = self.settings.save().err();
                    }
                });
                ui.menu_button("Playback", |ui| {
                    ui.checkbox(&mut self.looping, "Loop selection").on_hover_text("L");
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
        if toggle_info {
            self.show_info = !self.show_info;
            // Grow or shrink by the panel, so the player view keeps its size.
            if resizable && let Some(size) = self.window_size {
                let size = resize_for_panel(size, self.show_info, self.info_width, 0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
            }
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
            let panel = egui::Panel::right("info").resizable(true).size_range(240.0..=360.0).default_size(self.info_width).show(ui, |ui| info_panel(ui, &self.track, size));
            self.info_width = panel.response.rect.width();
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                // The cover takes the width, but leaves room below for what was measured last frame.
                let gap = 16.0 + 2.0 * ui.spacing().item_spacing.y;
                let side = cover_side(ui.available_width(), ui.available_height(), self.controls_height, gap);
                cover_art(ui, cover.as_ref(), side);
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
                    actions.extend(transport(ui, playing, has_next, self.volume));
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
        if save_copy {
            self.save_copy();
        }
        self.follow_save(&ctx);
        self.unsaved_window(&ctx);
        self.save_window(&ctx);
        if let Some(path) = open_recent {
            self.open(path);
        }
        if open_file && let Some(path) = rfd::FileDialog::new().add_filter("Audio", &folder::EXTENSIONS).pick_file() {
            self.open(path);
        }
        for action in actions {
            self.apply(action, pos, len);
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
fn info_rows(track: &Track, file_size: Option<u64>) -> [(&'static str, Rows); 3] {
    let unknown = || "—".to_string();
    let tags = track.tags.get().cloned().unwrap_or_default();
    let info = track.info.get().cloned().unwrap_or_default();
    let format = track.format.get();
    let rate = format.map_or(0, |f| f.rate as u64);
    let seconds = (track.done() && rate > 0).then(|| track.len() as f64 / rate as f64);
    let or = |v: String| if v.is_empty() { unknown() } else { v };
    let channels = format.map_or_else(unknown, |f| match f.channels {
        1 => "1 (mono)".into(),
        2 => "2 (stereo)".into(),
        n => n.to_string(),
    });
    let bitrate = seconds.zip(file_size).filter(|(s, _)| *s > 0.0).map_or_else(unknown, |(s, b)| format!("{:.0} kb/s", b as f64 * 8.0 / s / 1000.0));
    let loudness = match (track.loudness.get(), track.done()) {
        (Some(l), _) if l.lufs.is_finite() => (format!("{:.1} LUFS", l.lufs), format!("{:.1} dBFS", l.peak_db)),
        (Some(_), _) => ("silent".into(), "silent".into()),
        (None, true) => ("measuring…".into(), "measuring…".into()),
        (None, false) => ("after loading".into(), "after loading".into()),
    };
    let path = &track.path;
    let general = vec![
        ("Name", path.file_name().unwrap_or_default().to_string_lossy().into_owned()),
        ("Folder", path.parent().map_or_else(unknown, |p| p.display().to_string())),
        ("Format", or(info.container)),
        ("Codec", or(info.codec)),
        ("File size", file_size.map_or_else(unknown, |b| format!("{:.1} MB", b as f64 / 1e6))),
        ("Length", seconds.map_or_else(unknown, |_| clock(track.len(), rate))),
        ("Bitrate", bitrate),
    ];
    let audio = vec![
        ("Sample rate", if rate > 0 { format!("{:.1} kHz", rate as f64 / 1000.0) } else { unknown() }),
        ("Channels", channels),
        ("Bit depth", info.bits.map_or_else(unknown, |b| format!("{b}-bit"))),
        ("Loudness", loudness.0),
        ("Peak", loudness.1),
    ];
    let tags = vec![
        ("Title", tags.title.unwrap_or_else(unknown)),
        ("Artist", tags.artist.unwrap_or_else(unknown)),
        ("Album", tags.album.unwrap_or_else(unknown)),
    ];
    [("General", general), ("Audio", audio), ("Tags", tags)]
}

/// Track Info's body, laid out like lightview's inspector: collapsing sections of two-column grids.
fn info_panel(ui: &mut egui::Ui, track: &Track, file_size: Option<u64>) {
    // Fill the panel's width: shrinking to the content made it shrink with it (as in lightview).
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for (section, rows) in info_rows(track, file_size) {
            egui::CollapsingHeader::new(section).default_open(true).show(ui, |ui| {
                egui::Grid::new(section).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    // Tags ends with every tag as the file has it.
                    let raw = if section == "Tags" { track.info.get().map(|i| i.raw.clone()).unwrap_or_default() } else { Vec::new() };
                    for (name, value) in rows.into_iter().map(|(n, v)| (n.to_string(), v)).chain(raw) {
                        ui.label(name);
                        ui.add(egui::Label::new(value).wrap());
                        ui.end_row();
                    }
                });
            });
        }
    });
}

/// The window size after a panel opens (bigger by `extent` along `axis`, 0 = width) or closes.
fn resize_for_panel(mut size: [f32; 2], opening: bool, extent: f32, axis: usize) -> [f32; 2] {
    size[axis] = if opening { size[axis] + extent } else { (size[axis] - extent).max(1.0) };
    size
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
                (self.track, self.index) = (next, self.index + 1);
                (self.history, self.edit_view, self.sent_region) = (None, EditView::default(), None);
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
            Action::Undo | Action::Redo | Action::Cut | Action::Crop | Action::Silence => self.change(action, pos),
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
            ui.weak("Applies the next time soundglass opens; the window, Track Info and the Edit panel as you quit are kept.");
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
        if self.dirty() {
            self.pending = Some(Pending::Open(path));
            return;
        }
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.remember(&path);
        self.files = folder::list_folder(&path);
        self.next = None;
        let index = self.files.iter().position(|p| *p == path).unwrap_or(0);
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
        if !self.track.done() {
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
            ui.heading("Unsaved edits");
            ui.label("This track has edits that aren't saved. The original file is never changed.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = ui.button("Save As…").clicked();
                discard = ui.button("Discard edits").clicked();
                cancel = ui.button("Cancel").clicked();
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
        std::thread::spawn(move || {
            let result = save::save(&track, &written, choice, &path);
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
                Err("Saving failed".into())
            }
        };
        let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if result.is_ok() && let Some(h) = &mut self.history {
            h.mark_saved(edit);
        }
        let ok = result.is_ok();
        let message = result.map(|tags| if tags { format!("Saved {name}") } else { format!("Saved {name}, but the tags could not be copied") });
        (self.save_job, self.save_notice) = (None, Some((message, ctx.input(|i| i.time))));
        if ok && self.pending.is_some() {
            self.show_save = false;
            self.run_pending(ctx);
        }
    }

    /// Save As: the format and quality, then the system's save dialog; saves on its own thread.
    fn save_window(&mut self, ctx: &egui::Context) {
        if !self.show_save {
            return;
        }
        let (mut open, mut pick) = (true, false);
        let busy = self.save_job.is_some();
        egui::Window::new("Save As").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.add_enabled_ui(!busy, |ui| save_options(ui, &mut self.save_choice));
            ui.weak("The original file is never changed.");
            ui.add_space(4.0);
            if let Some((_, _, target, _)) = &self.save_job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Saving {}…", target.file_name().unwrap_or_default().to_string_lossy()));
                });
            } else {
                notice(ui, self.save_notice.as_ref(), None);
                pick = ui.button("Save…").clicked();
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
                self.start_save(self.save_choice, target, false);
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
        if p.shared.pos.load(Relaxed) > RESTART_SECONDS * rate || self.index == 0 {
            p.send(Cmd::Seek(0));
        } else {
            self.go(self.index - 1);
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{mark}{}", title(&name, length.as_deref(), &format))));
        self.titled = Some((Arc::as_ptr(&self.track) as usize, self.track.done(), self.dirty()));
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
fn save_options(ui: &mut egui::Ui, choice: &mut save::Choice) {
    ui.horizontal(|ui| {
        for kind in save::Kind::ALL {
            ui.radio_value(&mut choice.kind, kind, kind.name());
        }
    });
    ui.horizontal(|ui| match choice.kind {
        save::Kind::Wav => {
            for (bits, label) in [(16, "16-bit"), (24, "24-bit"), (32, "32-bit float")] {
                ui.radio_value(&mut choice.wav_bits, bits, label);
            }
        }
        save::Kind::Flac => {
            for (bits, label) in [(16, "16-bit"), (24, "24-bit")] {
                ui.radio_value(&mut choice.flac_bits, bits, label);
            }
        }
        save::Kind::Mp3 => {
            for kbps in [128, 192, 256, 320] {
                ui.radio_value(&mut choice.mp3_kbps, kbps, format!("{kbps} kb/s"));
            }
        }
        save::Kind::Vorbis => {
            ui.add(egui::Slider::new(&mut choice.vorbis_quality, 0..=10).text("quality"));
        }
    });
}

/// The cover's side: the full width, unless the height left after the controls (`controls`
/// tall, `gap` above them) is less; never below 64 points or wider than the window.
fn cover_side(width: f32, height: f32, controls: f32, gap: f32) -> f32 {
    (height - controls - gap).min(width).max(64.0).min(width)
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

/// A slim bar, or the waveform (`track` through `edit`, with the selection), with the times at
/// its ends; returns the frame to seek to when clicked or dragged.
type Wave<'a> = (&'a Track, &'a Edit, Option<(u64, u64)>, &'a mut WaveCache);
fn seek_bar(ui: &mut egui::Ui, pos: u64, len: u64, rate: u64, wave: Option<Wave>) -> Option<u64> {
    let height = if wave.is_some() { 48.0 } else { 18.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    let seconds = pos.checked_div(rate).unwrap_or(0) as f64;
    response.widget_info(|| egui::WidgetInfo::slider(true, seconds, "Position, seconds"));
    let visuals = ui.visuals();
    let fraction = (pos as f32 / len.max(1) as f32).min(1.0);
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let x = bar.left() + bar.width() * fraction;
    let painter = ui.painter_at(rect);
    if let Some((track, edit, selection, cache)) = wave {
        draw_waveform(&painter, visuals, rect, cache.columns(track, edit, 0, len, rect.width() as u64), (0, len), pos, selection);
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
    let (mid, half, played) = (rect.center().y, rect.height() / 2.0, x_of(pos));
    for (c, &(lo, hi)) in cols.iter().enumerate() {
        let x = rect.left() + c as f32 + 0.5;
        let color = if x < played { visuals.selection.bg_fill } else { visuals.text_color() };
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
    let frames = if view.frames == 0 { len } else { view.frames.min(len) }.max(1);
    view.start = view.start.min(len.saturating_sub(frames));
    // Zoomed in, the view turns the page when the playhead runs off its right edge; one moved
    // out of view by panning is left alone.
    let end = view.start + frames;
    if playing && view.frames != 0 && view.anchor.is_none() && (view.start..end).contains(&view.last_pos) && pos >= end {
        view.start = pos.min(len.saturating_sub(frames));
    }
    view.last_pos = pos;
    let centre = view.start + frames / 2;
    ui.horizontal(|ui| {
        match view.selection {
            Some((a, b)) => ui.label(format!("{} – {} ({:.3} s)", clock_ms(a, rate), clock_ms(b, rate), (b - a) as f64 / rate.max(1) as f64)),
            None => ui.weak("Drag on the waveform to select"),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Fit").on_hover_text("Show the whole track").clicked() {
                (view.start, view.frames) = (0, 0);
            }
            if ui.button("+").on_hover_text("Zoom in (scroll; right-drag to move along)").clicked() {
                (view.start, view.frames) = zoom_view(view.start, frames, len, 2.0, centre);
            }
            if ui.button("−").on_hover_text("Zoom out").clicked() {
                (view.start, view.frames) = zoom_view(view.start, frames, len, 0.5, centre);
            }
        });
    });
    let size = egui::vec2(ui.available_width(), ui.available_height().max(40.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Waveform: click to play from there, drag to select, scroll to zoom, right-drag to move along"));
    let frame_at = |x: f32| view.start + (((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * frames as f64) as u64;
    let shift = ui.input(|i| i.modifiers.shift);
    let mut seek = None;
    if response.drag_started_by(egui::PointerButton::Primary) {
        let from = ui.input(|i| i.pointer.press_origin()).map_or(0, |p| frame_at(p.x));
        // Shift+drag moves the nearer end of the selection.
        view.anchor = Some(match view.selection {
            Some((a, b)) if shift => if from.abs_diff(a) < from.abs_diff(b) { b } else { a },
            _ => from,
        });
    }
    if response.dragged_by(egui::PointerButton::Primary) && let (Some(anchor), Some(p)) = (view.anchor, response.interact_pointer_pos()) {
        let f = frame_at(p.x);
        view.selection = (f != anchor).then(|| (anchor.min(f), anchor.max(f)));
    }
    if response.drag_stopped() {
        view.anchor = None;
    }
    if response.clicked() && let Some(p) = response.interact_pointer_pos() {
        let f = frame_at(p.x);
        match view.selection {
            Some((a, b)) if shift => view.selection = Some((a.min(f), b.max(f))),
            _ => (view.selection, seek) = (None, Some(f)),
        }
    }
    // Right-drag and sideways scrolling (touchpads, Shift+wheel) move along; the wheel zooms.
    let mut pan = if response.dragged_by(egui::PointerButton::Secondary) { response.drag_delta().x } else { 0.0 };
    if response.hovered() {
        // egui turns Ctrl+scroll into zoom_delta; plain vertical scrolling zooms too.
        let (zoom, scroll, pointer) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        let zoom = zoom * (scroll.y / 200.0).exp();
        if zoom != 1.0 {
            let at = pointer.map_or(centre, |p| frame_at(p.x));
            (view.start, view.frames) = zoom_view(view.start, frames, len, zoom, at);
        }
        pan += scroll.x;
    }
    if view.frames != 0 && pan != 0.0 {
        let shift = (-pan / rect.width()) as f64 * view.frames.min(len) as f64;
        view.start = (view.start as f64 + shift).clamp(0.0, len.saturating_sub(view.frames.min(len)) as f64) as u64;
    }
    let frames = if view.frames == 0 { len } else { view.frames };
    let cols = cache.columns(track, edit, view.start, frames, rect.width() as u64);
    draw_waveform(&ui.painter_at(rect), ui.visuals(), rect, cols, (view.start, frames), pos, view.selection);
    seek
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
    use super::{clock_ms, cover_side, display_pos, info_rows, marquee, push_recent, resize_for_panel, snap_speed, title, volume_gain, zoom_view};
    use std::path::PathBuf;

    #[test]
    fn track_info_rows() {
        // 2 s of stereo at 44.1 kHz in a 352 800-byte file: 1411 kb/s, like a CD.
        let track = crate::track::Track::from_samples(44100, 2, vec![0.0; 2 * 88200]);
        let groups = info_rows(&track, Some(352_800));
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
