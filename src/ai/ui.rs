//! The Local AI menu, the AI Models window, licence prompts and downloads.
use super::*;
use crate::settings::AiSettings;
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, TryRecvError};

/// What a licence prompt is for.
#[derive(Clone, Copy, PartialEq)]
enum Download {
    Runtime,
    Gpu,
    /// An index into `CATALOG`.
    Model(usize),
}

impl Download {
    fn title(self) -> &'static str {
        match self {
            Download::Runtime => "AI runtime (ONNX Runtime)",
            Download::Gpu => "GPU support",
            Download::Model(i) => CATALOG[i].label,
        }
    }

    fn licenses(self) -> &'static [License] {
        match self {
            Download::Runtime => std::slice::from_ref(&RUNTIME_LICENSE),
            Download::Gpu => &GPU_LICENSES,
            Download::Model(i) => CATALOG[i].licenses,
        }
    }

    fn note(self) -> Option<&'static str> {
        match self {
            Download::Runtime => None,
            Download::Gpu => Some("Downloaded from Microsoft (ONNX Runtime) and NVIDIA (CUDA libraries, via their official PyPI packages)."),
            Download::Model(i) => CATALOG[i].converted_by,
        }
    }
}

/// A download running on its own thread.
struct Job {
    what: Download,
    /// 0–1 as f32 bits.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<(), String>>,
}

/// What a tool made: a file to open, or something to say (a MIDI file saved, beats found).
#[derive(Debug, PartialEq)]
pub(super) enum Done {
    Open(PathBuf),
    Said(String),
    /// A transcript, shown to copy or save; true when it's subtitles (.srt).
    Text(String, bool),
}

/// An AI tool running on its own thread.
struct Run {
    task: Task,
    /// The track it runs on.
    source: PathBuf,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<Done, String>>,
}

/// The Local AI windows' state.
#[derive(Default)]
pub struct Ai {
    pub show_models: bool,
    run: Option<Run>,
    /// A tool's outcome when nothing opens (a saved MIDI file, an error), shown until closed.
    run_error: Option<(Task, Result<String, String>)>,
    /// Transcribe Speech's text, the track it came from (for the file's name), and whether it's subtitles.
    transcript: Option<(String, PathBuf, bool)>,
    /// A licence being read: what for, and which of its licences.
    license: Option<(Download, usize)>,
    jobs: Vec<Job>,
    /// The last download's outcome, shown in AI Models.
    notice: Option<Result<String, String>>,
}

impl Ai {
    fn busy(&self, what: Download) -> Option<&Job> {
        self.jobs.iter().find(|j| j.what == what)
    }

    /// The Local AI menu: every tool, then AI Models… and, while it's missing, Download AI
    /// Runtime…. Returns a tool to run.
    /// With `selected`, the items say they work on the selection.
    pub fn menu_items(&mut self, ui: &mut egui::Ui, s: &AiSettings, loaded: bool, selected: bool) -> Option<Task> {
        let mut chosen = None;
        for task in Task::ALL {
            let why = Ai::ready(task, s).err().or((!loaded).then_some("after loading")).or(self.run.is_some().then_some("another AI tool is running"));
            if ui.add_enabled(why.is_none(), egui::Button::new(if selected { format!("{} (selection)", task.title()) } else { task.title().to_string() })).on_disabled_hover_text(why.unwrap_or_default()).clicked() {
                chosen = Some(task);
                ui.close();
            }
        }
        ui.separator();
        if ui.button("AI Models…").clicked() {
            self.show_models = true;
            ui.close();
        }
        if !runtime_installed() {
            let busy = self.busy(Download::Runtime).is_some();
            if ui.add_enabled(!busy, egui::Button::new(format!("Download AI Runtime ({RUNTIME_DOWNLOAD_MB} MB)…"))).clicked() {
                self.license = Some((Download::Runtime, 0));
                ui.close();
            }
        }
        chosen
    }

    /// Runs `task` on the edited track, on its own thread; the file it makes is opened when done.
    /// `selection`: edited frames to work on, else the whole track.
    pub fn start_tool(&mut self, task: Task, track: Arc<crate::track::Track>, edit: Arc<crate::edit::Edit>, selection: Option<(u64, u64)>, s: &AiSettings, ctx: &egui::Context) {
        let Ok(model) = Ai::ready(task, s) else { return };
        let tune = tune_for(model, s);
        let source = track.path.clone();
        let (progress, cancel) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (tx, done) = std::sync::mpsc::channel();
        let (p, c, ctx) = (progress.clone(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = |x: f32| p.store(x.to_bits(), Ordering::Relaxed);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_tool(task, model, &tune, &track, &edit, selection, &report, &c))).unwrap_or_else(|_| Err("the AI tool failed".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        (self.run, self.run_error) = (Some(Run { task, source, progress, cancel, done }), None);
    }

    /// The running tool's window; the file it made, once it's done.
    fn run_window(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if let Some(run) = &self.run {
            match run.done.try_recv() {
                Err(TryRecvError::Empty) => {
                    let p = f32::from_bits(run.progress.load(Ordering::Relaxed));
                    egui::Window::new(run.task.title()).collapsible(false).resizable(false).show(ctx, |ui| {
                        ui.label(if on_gpu() && matches!(run.task, Task::Stems | Task::Beats) { "Running on the GPU…" } else { "Running on the CPU…" });
                        ui.add(egui::ProgressBar::new(p).desired_width(240.0).show_percentage());
                        if ui.button("Cancel").clicked() {
                            run.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Ok(Ok(Done::Open(path))) => {
                    self.run = None;
                    return Some(path);
                }
                Ok(Ok(Done::Said(m))) => (self.run, self.run_error) = (None, Some((run.task, Ok(m)))),
                Ok(Ok(Done::Text(t, subtitles))) => (self.transcript, self.run) = (Some((t, run.source.clone(), subtitles)), None),
                Ok(Err(e)) => {
                    let task = run.task;
                    let cancelled = run.cancel.load(Ordering::Relaxed);
                    self.run = None;
                    if !cancelled {
                        self.run_error = Some((task, Err(e)));
                    }
                }
                Err(TryRecvError::Disconnected) => (self.run, self.run_error) = (None, Some((run.task, Err("the AI tool stopped".into())))),
            }
        }
        self.transcript_window(ctx);
        if let Some((task, e)) = &self.run_error {
            let mut open = true;
            egui::Window::new(task.title()).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| match e {
                Ok(m) => ui.label(m),
                Err(e) => ui.colored_label(ui.visuals().error_fg_color, e),
            });
            if !open {
                self.run_error = None;
            }
        }
        None
    }

    /// Whether `task` can run now: the runtime and a model for it installed.
    pub fn ready(task: Task, s: &AiSettings) -> Result<&'static Known, &'static str> {
        if !runtime_installed() {
            return Err("needs the AI runtime: Local AI → Download AI Runtime…");
        }
        choose(s.models.get(&task), task, &installed()).ok_or("needs a model: Local AI → AI Models…")
    }

    /// Transcribe Speech's result: the text to read or select, Copy, and Save as Text….
    fn transcript_window(&mut self, ctx: &egui::Context) {
        let Some((text, source, subtitles)) = &mut self.transcript else { return };
        let (button, kind, ext) = if *subtitles { ("Save as Subtitles…", "Subtitles", "srt") } else { ("Save as Text…", "Text", "txt") };
        let mut open = true;
        let mut saved = None;
        egui::Window::new("Transcript").open(&mut open).collapsible(false).default_size([480.0, 360.0]).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                // Read-only but selectable: edits to the text are thrown away each frame.
                let mut shown = text.as_str();
                ui.add(egui::TextEdit::multiline(&mut shown).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    ctx.copy_text(text.clone());
                }
                if ui.button(button).clicked() {
                    let name = format!("{}.{ext}", source.file_stem().unwrap_or_default().to_string_lossy());
                    let mut dialog = rfd::FileDialog::new().add_filter(kind, &[ext]).set_file_name(name);
                    if let Some(dir) = source.parent() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(path) = dialog.save_file() {
                        saved = Some(save_text(&path, text));
                    }
                }
            });
        });
        if let Some(r) = saved {
            self.run_error = Some((Task::Transcribe, r.map(|p| format!("Saved {}", p.file_name().unwrap_or_default().to_string_lossy()))));
        }
        if !open {
            self.transcript = None;
        }
    }

    /// The AI Models window, the licence prompt and the running tool; follows the downloads.
    /// Returns a file a tool made, to open.
    pub fn windows(&mut self, ctx: &egui::Context, s: &mut AiSettings) -> Option<PathBuf> {
        self.follow(ctx);
        let made = self.run_window(ctx);
        if self.show_models {
            let mut open = true;
            egui::Window::new("AI Models").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| self.models_ui(ui, s));
            self.show_models &= open;
        }
        if let Some((what, step)) = self.license {
            let licenses = what.licenses();
            let modal = egui::Modal::new(egui::Id::unique("ai-license")).show(ctx, |ui| license_ui(ui, what.title(), &licenses[step], step, licenses.len(), what.note()));
            match modal.inner {
                Some(true) if step + 1 < licenses.len() => self.license = Some((what, step + 1)),
                Some(true) => {
                    self.license = None;
                    self.start(what, ctx);
                }
                Some(false) => self.license = None,
                None if modal.should_close() => self.license = None,
                None => {}
            }
        }
        made
    }

    fn start(&mut self, what: Download, ctx: &egui::Context) {
        let (progress, cancel) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (tx, done) = std::sync::mpsc::channel();
        let (p, c, ctx) = (progress.clone(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = |x: f32| p.store(x.to_bits(), Ordering::Relaxed);
            let result = match what {
                Download::Runtime => download_runtime(&report, &c),
                Download::Gpu => download_gpu(&report, &c),
                Download::Model(i) => download(&CATALOG[i], &report, &c),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.jobs.push(Job { what, progress, cancel, done });
        // Progress and the outcome are shown there.
        self.show_models = true;
    }

    fn follow(&mut self, ctx: &egui::Context) {
        let mut notice = None;
        self.jobs.retain(|j| match j.done.try_recv() {
            Err(TryRecvError::Empty) => true,
            Ok(r) => {
                notice = Some(r.map(|()| format!("{} installed.", j.what.title())).map_err(|e| format!("{}: {e}", j.what.title())));
                false
            }
            Err(TryRecvError::Disconnected) => {
                notice = Some(Err(format!("{}: the download stopped.", j.what.title())));
                false
            }
        });
        if notice.is_some() {
            self.notice = notice;
        }
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// A download's button, or its progress bar and Cancel while it runs.
    fn download_button(&mut self, ui: &mut egui::Ui, what: Download, label: &str) {
        match self.busy(what) {
            Some(job) => {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(egui::ProgressBar::new(p).desired_width(120.0).show_percentage());
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            None => {
                if ui.add_enabled(self.license.is_none(), egui::Button::new(label)).clicked() {
                    self.license = Some((what, 0));
                }
            }
        }
    }

    fn models_ui(&mut self, ui: &mut egui::Ui, s: &mut AiSettings) {
        let runtime = runtime_installed();
        if !runtime {
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, "The AI runtime isn't installed yet.");
                self.download_button(ui, Download::Runtime, &format!("Download AI Runtime ({RUNTIME_DOWNLOAD_MB} MB)"));
            });
        }
        if nvidia_driver() || gpu_installed() {
            ui.add_space(4.0);
            ui.strong("GPU (NVIDIA)");
            if !gpu_installed() {
                ui.horizontal(|ui| {
                    ui.label(format!("Run the AI tools on the graphics card ({GPU_DOWNLOAD_MB} MB, NVIDIA's CUDA libraries)."));
                    self.download_button(ui, Download::Gpu, "Download GPU Support");
                });
            } else {
                if ui.checkbox(&mut s.use_gpu, "Use the GPU").on_hover_text("For Split into Stems and Find Beats, 2.6–3× faster on a GTX 1050; the other tools are faster on the CPU").changed() {
                    set_use_gpu(s.use_gpu);
                }
                if gpu_needs_restart() {
                    ui.colored_label(ui.visuals().warn_fg_color, "Restart soundglass to start using the GPU.");
                } else if let Some(e) = gpu_error().filter(|_| s.use_gpu) {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("The GPU couldn't be used, so the CPU is: {}", short_error(&e)));
                }
            }
        }
        let installed = installed();
        egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
            for task in Task::ALL {
                ui.add_space(8.0);
                ui.strong(task.title());
                let models: Vec<&Known> = CATALOG.iter().filter(|k| k.task == task && installed.contains(&k.file)).collect();
                if models.len() > 1 {
                    let current = choose(s.models.get(&task), task, &installed);
                    egui::ComboBox::from_id_salt(("ai-model", task.title())).selected_text(current.map_or("", |k| k.label)).show_ui(ui, |ui| {
                        for k in models {
                            if ui.selectable_label(current.is_some_and(|c| c.file == k.file), k.label).clicked() {
                                s.models.insert(task, k.file.to_string());
                            }
                        }
                    });
                } else if let Some(k) = models.first() {
                    ui.label(format!("{} (installed)", k.label));
                }
                for k in CATALOG.iter().filter(|k| k.task == task && installed.contains(&k.file)) {
                    egui::CollapsingHeader::new(format!("Settings: {}", k.label)).id_salt(("tune", k.file)).show(ui, |ui| super::tune::tune_ui(ui, s, k));
                }
                egui::Grid::new(("catalog", task.title())).num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                    for (i, k) in CATALOG.iter().enumerate().filter(|(_, k)| k.task == task && !installed.contains(&k.file)) {
                        ui.label(k.label);
                        ui.label(format!("{} MB", k.mb));
                        ui.horizontal(|ui| self.download_button(ui, Download::Model(i), "Download"));
                        ui.end_row();
                    }
                });
            }
        });
        match &self.notice {
            Some(Ok(m)) => drop(ui.label(m)),
            Some(Err(e)) => drop(ui.colored_label(ui.visuals().error_fg_color, e)),
            None => {}
        }
        ui.add_space(8.0);
        if ui.button("Open models folder").clicked() {
            open_folder();
        }
    }
}

/// One licence of a download (`step` of `count`, from 0). Some(true) accepts it, Some(false) cancels.
/// The text scrolls within the window's height, so the buttons stay on screen.
fn license_ui(ui: &mut egui::Ui, title: &str, l: &License, step: usize, count: usize, note: Option<&str>) -> Option<bool> {
    ui.heading(if count > 1 { format!("Licence ({} of {count}) — {title}", step + 1) } else { format!("Licence — {title}") });
    ui.label("Please read and accept the licence before downloading.");
    if let Some(note) = note {
        ui.weak(note);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.strong(format!("{} — {}", l.project, l.spdx));
        if ui.button("Open project page").clicked() {
            open(l.url);
        }
    });
    // Room for the heading, notes and buttons around the text.
    let height = (ui.ctx().content_rect().height() - 260.0).clamp(100.0, 360.0);
    egui::ScrollArea::vertical().id_salt(("license", step)).max_height(height).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(l.text).monospace()).wrap());
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let accept = ui.button(if step + 1 < count { "Accept and Continue" } else { "Accept and Download" }).clicked();
        let cancel = ui.button("Cancel").clicked();
        accept.then_some(true).or(cancel.then_some(false))
    })
    .inner
}

/// Runs a tool to the end (blocking): its output file, named after the track, next to it.
/// With `selection` (edited frames), only that stretch: a clip for the files it writes, the
/// track's own times for subtitles and beats.
pub(super) fn run_tool(task: Task, model: &Known, tune: &Tune, track: &crate::track::Track, edit: &crate::edit::Edit, selection: Option<(u64, u64)>, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Done, String> {
    let own = track.format.get().ok_or("the track has no audio")?.rate;
    if selection.is_some_and(|(a, b)| b.saturating_sub(a) < own as u64 / 2) {
        return Err("select at least half a second, or nothing for the whole track".into());
    }
    let (full, clip) = (edit, selection.map(|(a, b)| edit.crop(a, b)));
    let edit = clip.as_ref().unwrap_or(full);
    let offset = selection.map_or(0, |s| s.0);
    match (task, tune) {
        (Task::Denoise, Tune::Denoise(t)) => {
            let target = crate::save::reserve_named(&track.path, "denoised", "flac")?;
            let result = (|| {
                let channels = audio::planar(track, edit, denoise::RATE)?;
                let clean = denoise::denoise(&model_path(model.file)?, channels, t.limit_db, &|p| progress(0.95 * p), cancel)?;
                audio::save_flac(&clean, denoise::RATE, own, &target)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&target);
            }
            result.map(|()| Done::Open(target))
        }
        (Task::Stems, Tune::Stems(t)) => {
            if !t.write.contains(&true) {
                return Err("choose at least one stem to write (AI Models → Settings)".into());
            }
            let dir = reserve_dir(&track.path, "stems")?;
            let result = (|| {
                let mix = audio::planar(track, edit, stems::RATE)?;
                let parts = stems::separate(&model_path(model.file)?, mix, t.overlap, &|p| progress(0.9 * p), cancel)?;
                let mut written = Vec::new();
                for ((name, part), _) in stems::NAMES.iter().zip(&parts).zip(t.write).filter(|(_, w)| *w) {
                    let path = dir.join(format!("{name}.flac"));
                    audio::save_flac(part, stems::RATE, own, &path)?;
                    written.push(path);
                }
                // The vocals, or the last one written (they're written in NAMES order).
                Ok(Done::Open(written.pop().unwrap()))
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            result
        }
        (Task::Midi, Tune::Midi(t)) => {
            let target = crate::save::reserve_named(&track.path, "melody", "mid")?;
            let result = (|| {
                let channels = audio::planar(track, edit, midi::RATE)?;
                let mono: Vec<f32> = (0..channels[0].len()).map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32).collect();
                let (frames, onsets) = midi::activations(&model_path(model.file)?, &mono, &|p| progress(0.9 * p), cancel)?;
                let notes = midi::notes(&frames, &onsets, t);
                if notes.is_empty() {
                    return Err("no notes were found".to_string());
                }
                // Written beside it, then renamed over the reserved name.
                let part = target.with_extension("mid.part");
                std::fs::write(&part, midi::smf(&notes, t.tempo)).and_then(|()| std::fs::rename(&part, &target)).map_err(|e| {
                    let _ = std::fs::remove_file(&part);
                    e.to_string()
                })
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&target);
            }
            result.map(|()| Done::Said(format!("Saved {}", target.file_name().unwrap_or_default().to_string_lossy())))
        }
        (Task::Beats, Tune::Beats(t)) => {
            // On the original, not the edits, so the beats can be mapped through them as they
            // change; a selection is read as edited, its beats mapped back to the original.
            let whole = crate::edit::Edit::whole(track.len());
            let channels = audio::planar(track, if selection.is_some() { edit } else { &whole }, beats::RATE)?;
            // The mean of the channels, as Beat This! downmixes.
            let mono: Vec<f32> = (0..channels[0].len()).map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32).collect();
            let mut found = beats::track(&model_path(model.file)?, &model_path(model.data[0].0)?, mono, own, t.threshold, progress, cancel)?;
            if selection.is_some() {
                let back = |v: &[u64]| {
                    let mut v: Vec<u64> = v.iter().filter_map(|&f| full.source_of(offset + f)).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                found = beats::Beats { beats: back(&found.beats), downbeats: back(&found.downbeats) };
            }
            let said = format!("Found {} beats and {} downbeats. The volume button's Metronome clicks along.", found.beats.len(), found.downbeats.len());
            *track.beats.lock().unwrap() = Some(Arc::new(found));
            Ok(Done::Said(said))
        }
        (Task::Transcribe, Tune::Speech(t)) => {
            let channels = audio::planar(track, edit, transcribe::RATE)?;
            let mono: Vec<f32> = (0..channels[0].len()).map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32).collect();
            let path = |f: &str| model_path(f);
            let cues = match model.data {
                [(dj, _), (features, _), (vocab, _)] => transcribe::parakeet(&path(model.file)?, &path(dj)?, &path(features)?, &path(vocab)?, &mono, t.silence_db, progress, cancel)?,
                [(decoder, _), (tokens, _)] => transcribe::whisper(&path(model.file)?, &path(decoder)?, &path(tokens)?, &mono, t, progress, cancel)?,
                _ => return Err("not a transcription model".into()),
            };
            if cues.is_empty() {
                return Err("no speech was found".into());
            }
            let cues = transcribe::shifted(cues, offset as f64 / own as f64);
            Ok(Done::Text(if t.time_marks { transcribe::srt(&cues) } else { transcribe::plain(&cues) }, t.time_marks))
        }
        _ => Err("the settings aren't for this tool".into()),
    }
}

/// Creates the first free folder `<stem>-<what>` (then `-2`…) next to `source`, so nothing is overwritten.
fn reserve_dir(source: &Path, what: &str) -> Result<PathBuf, String> {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    for n in 1.. {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let dir = source.with_file_name(format!("{stem}-{what}{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("can't make a folder next to the track ({e})")),
        }
    }
    unreachable!()
}

/// Writes `text` to `path` through a temporary file beside it, so a failed write leaves no half file.
/// The system dialog has already asked before replacing an existing file.
fn save_text(path: &Path, text: &str) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("no file name")?.to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.soundglass-part"));
    let result = std::fs::write(&temp, text.as_bytes()).and_then(|()| std::fs::rename(&temp, path)).map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map(|()| path.to_path_buf())
}
