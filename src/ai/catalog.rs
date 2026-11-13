//! The models soundglass knows: downloadable, pinned by revision and SHA-256, with their licences.
use super::*;

/// A model soundglass knows by file name.
pub struct Known {
    pub file: &'static str,
    pub label: &'static str,
    pub task: Task,
    /// Size, all files together.
    pub mb: u32,
    pub(super) sha256: &'static str,
    /// Where `file` (and `data`) are downloaded from, pinned to a revision.
    pub(super) base: &'static str,
    /// The other files it needs, by name and sha256 (weights kept apart, a vocabulary, a second network).
    pub(super) data: &'static [(&'static str, &'static str)],
    /// Who made the ONNX file, when it isn't the authors.
    pub converted_by: Option<&'static str>,
    /// Shown, and accepted by the user, before the download starts.
    pub licenses: &'static [License],
}

const DEEPFILTERNET_LICENSE: License = License { project: "DeepFilterNet", spdx: "MIT OR Apache-2.0", url: "https://github.com/Rikorose/DeepFilterNet", text: include_str!("../../licenses/deepfilternet.txt") };

/// In order of preference per task.
pub const CATALOG: [Known; 1] = [Known {
    file: "denoiser_model.onnx",
    label: "DeepFilterNet3 — speech, 48 kHz",
    task: Task::Denoise,
    mb: 16,
    sha256: "fe5eb64fa2e4154c83f8e4935e82871c850c154387ee892e0ab65fe179e7d8c9",
    // Pinned to a revision, so the file (and its checksum) can't change under us.
    base: "https://huggingface.co/kimtos-labs/denoiser-dfn3/resolve/888f33c41851d7168ae31c3637c323b9e2f3c2a4",
    data: &[],
    converted_by: Some("The ONNX file is grazder's torchDF streaming export, hosted by KimtOS on Hugging Face."),
    licenses: &[DEEPFILTERNET_LICENSE],
}];

pub(super) fn models_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("models"))
}

/// The catalog models whose files are all in the models folder.
pub fn installed() -> Vec<&'static str> {
    models_dir().map(|d| installed_in(&d)).unwrap_or_default()
}

pub(super) fn installed_in(dir: &Path) -> Vec<&'static str> {
    CATALOG.iter().filter(|k| std::iter::once(k.file).chain(k.data.iter().map(|d| d.0)).all(|f| dir.join(f).is_file())).map(|k| k.file).collect()
}

/// The model `task` uses: the user's choice if it's installed, else the first installed one for it.
pub fn choose(choice: Option<&String>, task: Task, installed: &[&'static str]) -> Option<&'static Known> {
    let ok = |k: &&Known| k.task == task && installed.contains(&k.file);
    CATALOG.iter().filter(ok).find(|k| choice.is_some_and(|c| c == k.file)).or_else(|| CATALOG.iter().find(ok))
}

/// The path of one of an installed model's files.
pub(super) fn model_path(file: &str) -> Result<PathBuf, String> {
    Ok(models_dir()?.join(file))
}

/// Downloads a catalog model (blocking). Every file goes to a temporary name first and is
/// checked, so a failed or tampered download never leaves a model behind.
pub fn download(k: &Known, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    download_to(&models_dir()?, k, progress, cancel)
}

pub(super) fn download_to(dir: &Path, k: &Known, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let files: Vec<(&str, &str)> = std::iter::once((k.file, k.sha256)).chain(k.data.iter().copied()).collect();
    let part = |name: &str| dir.join(format!("{name}.part"));
    let n = files.len() as f32;
    // Fetch and check every file before installing any, so a failure leaves nothing half-installed.
    for (i, (name, sha)) in files.iter().enumerate() {
        if let Err(e) = fetch(&format!("{}/{name}", k.base), sha, &part(name), &|p| progress((i as f32 + p) / n), cancel) {
            files.iter().for_each(|(n, _)| drop(std::fs::remove_file(part(n))));
            return Err(e);
        }
    }
    // The extra files go in first: the model only counts as installed once its main file is there.
    for (name, _) in files.iter().rev() {
        std::fs::rename(part(name), dir.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Opens the models folder in the file manager.
pub fn open_folder() {
    if let Ok(dir) = models_dir() {
        let _ = std::fs::create_dir_all(&dir);
        open(dir);
    }
}
