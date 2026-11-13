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

/// An AI tool running on its own thread.
struct Run {
    task: Task,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<PathBuf, String>>,
}

/// The Local AI windows' state.
#[derive(Default)]
pub struct Ai {
    pub show_models: bool,
    run: Option<Run>,
    /// A tool's error, shown in its window until closed.
    run_error: Option<(Task, String)>,
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

    /// The Local AI menu: the tools that have a model in the catalog (Practice mode's only with
    /// it on), then AI Models… and, while it's missing, Download AI Runtime…. Returns a tool to run.
    pub fn menu_items(&mut self, ui: &mut egui::Ui, s: &AiSettings, practice: bool, loaded: bool) -> Option<Task> {
        let mut chosen = None;
        for task in Task::ALL.into_iter().filter(|t| (practice || !t.practice()) && CATALOG.iter().any(|k| k.task == *t)) {
            let why = Ai::ready(task, s).err().or((!loaded).then_some("after loading")).or(self.run.is_some().then_some("another AI tool is running"));
            if ui.add_enabled(why.is_none(), egui::Button::new(task.title())).on_disabled_hover_text(why.unwrap_or_default()).clicked() {
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
    pub fn start_tool(&mut self, task: Task, track: Arc<crate::track::Track>, edit: Arc<crate::edit::Edit>, s: &AiSettings, ctx: &egui::Context) {
        let Ok(model) = Ai::ready(task, s) else { return };
        let (progress, cancel) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
        let (tx, done) = std::sync::mpsc::channel();
        let (p, c, ctx) = (progress.clone(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = |x: f32| p.store(x.to_bits(), Ordering::Relaxed);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_tool(task, model, &track, &edit, &report, &c))).unwrap_or_else(|_| Err("the AI tool failed".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        (self.run, self.run_error) = (Some(Run { task, progress, cancel, done }), None);
    }

    /// The running tool's window; the file it made, once it's done.
    fn run_window(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if let Some(run) = &self.run {
            match run.done.try_recv() {
                Err(TryRecvError::Empty) => {
                    let p = f32::from_bits(run.progress.load(Ordering::Relaxed));
                    egui::Window::new(run.task.title()).collapsible(false).resizable(false).show(ctx, |ui| {
                        ui.label(if on_gpu() { "Running on the GPU…" } else { "Running on the CPU…" });
                        ui.add(egui::ProgressBar::new(p).desired_width(240.0).show_percentage());
                        if ui.button("Cancel").clicked() {
                            run.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Ok(Ok(path)) => {
                    self.run = None;
                    return Some(path);
                }
                Ok(Err(e)) => {
                    let task = run.task;
                    let cancelled = run.cancel.load(Ordering::Relaxed);
                    self.run = None;
                    if !cancelled {
                        self.run_error = Some((task, e));
                    }
                }
                Err(TryRecvError::Disconnected) => (self.run, self.run_error) = (None, Some((run.task, "the AI tool stopped".into()))),
            }
        }
        if let Some((task, e)) = &self.run_error {
            let mut open = true;
            egui::Window::new(task.title()).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| ui.colored_label(ui.visuals().error_fg_color, e));
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
                if ui.checkbox(&mut s.use_gpu, "Use the GPU").changed() {
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
            for task in Task::ALL.into_iter().filter(|t| CATALOG.iter().any(|k| k.task == *t)) {
                ui.add_space(8.0);
                ui.strong(task.title());
                if task.practice() {
                    ui.weak("In Practice mode.");
                }
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
pub(super) fn run_tool(task: Task, model: &Known, track: &crate::track::Track, edit: &crate::edit::Edit, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let own = track.format.get().ok_or("the track has no audio")?.rate;
    match task {
        Task::Denoise => {
            let target = crate::save::reserve_named(&track.path, "denoised", "flac")?;
            let result = (|| {
                let channels = audio::planar(track, edit, denoise::RATE)?;
                let clean = denoise::denoise(&model_path(model.file)?, channels, &|p| progress(0.95 * p), cancel)?;
                audio::save_flac(&clean, denoise::RATE, own, &target)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&target);
            }
            result.map(|()| target)
        }
        _ => Err("not built yet".into()),
    }
}
