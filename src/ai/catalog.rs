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

const DEMUCS_LICENSE: License = License { project: "Demucs (HTDemucs v4)", spdx: "MIT", url: "https://github.com/facebookresearch/demucs", text: include_str!("../../licenses/demucs.txt") };

const BASIC_PITCH_LICENSE: License = License { project: "Basic Pitch (Spotify)", spdx: "Apache-2.0", url: "https://github.com/spotify/basic-pitch", text: include_str!("../../licenses/basic-pitch.txt") };

const BEAT_THIS_LICENSE: License = License { project: "Beat This! (JKU Linz)", spdx: "MIT", url: "https://github.com/CPJKU/beat_this", text: include_str!("../../licenses/beat-this.txt") };

const WHISPER_LICENSE: License = License { project: "Whisper (OpenAI)", spdx: "MIT", url: "https://github.com/openai/whisper", text: include_str!("../../licenses/whisper.txt") };
const PARAKEET_LICENSE: License = License { project: "NVIDIA Parakeet TDT 0.6B v3", spdx: "CC-BY-4.0", url: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3", text: include_str!("../../licenses/parakeet.txt") };
const SHERPA_WHISPER: &str = "The ONNX files are sherpa-onnx's int8 export (k2-fsa), on Hugging Face.";

/// In order of preference per task.
pub const CATALOG: [Known; 7] = [Known {
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
}, Known {
    file: "htdemucs_fp16weights.onnx",
    label: "HTDemucs v4 — drums, bass, other, vocals",
    task: Task::Stems,
    mb: 166,
    sha256: "d05c269d0178d2a72ad484b10b11dd370193fc923201c3b27a99f848745db70a",
    base: "https://huggingface.co/StemSplitio/htdemucs-onnx/resolve/d54ed9eb60e258ea82131c6ee14578628816456a",
    data: &[],
    converted_by: Some("The ONNX file is StemSplit's export (weights stored at half precision), on Hugging Face."),
    licenses: &[DEMUCS_LICENSE],
}, Known {
    file: "nmp.onnx",
    label: "Basic Pitch — any instrument, several notes at once",
    task: Task::Midi,
    mb: 1,
    sha256: "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec",
    // Spotify's own ONNX file, from the repository at a fixed commit.
    base: "https://raw.githubusercontent.com/spotify/basic-pitch/dfb20ef559dff1792e11e022f3f0c7008c1dee6d/basic_pitch/saved_models/icassp_2022",
    data: &[],
    converted_by: None,
    licenses: &[BASIC_PITCH_LICENSE],
}, Known {
    file: "beat_this.onnx",
    label: "Beat This! — beats and downbeats",
    task: Task::Beats,
    mb: 191,
    sha256: "1337dc6c21257ed6b803418efc320df42b5d8eb296ef16648ef60457cd4b9e55",
    base: "https://huggingface.co/musetric/beat-this-onnx/resolve/25308706dda545c0d5950065bb0604ad71bcf1f5",
    // torchaudio's mel filterbank, for the front end.
    data: &[("mel-filterbank.bin", "1ee975d96f44ccf2c3bfe37825c1c1f0b089f5703c7a12a84b1f0a3bce004533")],
    converted_by: Some("The ONNX file is musetric's export of JKU's final0 checkpoint, on Hugging Face."),
    licenses: &[BEAT_THIS_LICENSE],
}, Known {
    file: "small-encoder.int8.onnx",
    label: "Whisper small — 99 languages",
    task: Task::Transcribe,
    mb: 375,
    sha256: "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9",
    base: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/8f3c18b358db4d1f2fc1eae49d75cd20989e4309",
    data: &[
        ("small-decoder.int8.onnx", "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee"),
        ("small-tokens.txt", "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126"),
    ],
    converted_by: Some(SHERPA_WHISPER),
    licenses: &[WHISPER_LICENSE],
}, Known {
    file: "encoder-model.int8.onnx",
    label: "Parakeet TDT 0.6B v3 — 25 European languages, more accurate",
    task: Task::Transcribe,
    mb: 671,
    sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    base: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
    data: &[
        ("decoder_joint-model.int8.onnx", "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70"),
        ("nemo128.onnx", "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f"),
        ("vocab.txt", "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
    ],
    converted_by: Some("The ONNX files are istupakov's int8 conversion for onnx-asr, on Hugging Face."),
    licenses: &[PARAKEET_LICENSE],
}, Known {
    file: "turbo-encoder.int8.onnx",
    label: "Whisper large-v3-turbo — 99 languages, best, slow on a CPU",
    task: Task::Transcribe,
    mb: 1037,
    sha256: "b02dcdf54f348741e93fe732b67d933c8dcb6735655f710640143081db38878b",
    base: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/resolve/2ca6ff69fc878651b770880507669577ac41c2ff",
    data: &[
        ("turbo-decoder.int8.onnx", "20accd02388482eb3a46bd615631adfdc85e1eb2c7db9ea3f02a40ffe6b81547"),
        ("turbo-tokens.txt", "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126"),
    ],
    converted_by: Some(SHERPA_WHISPER),
    licenses: &[WHISPER_LICENSE],
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
