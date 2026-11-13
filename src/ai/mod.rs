//! Local AI, as in lightview: ONNX Runtime and the models aren't shipped. Local AI → Download
//! AI Runtime… and AI Models… install them on request, each file pinned by SHA-256.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

mod audio;
mod beats;
mod catalog;
mod denoise;
mod midi;
mod runtime;
mod stems;
mod transcribe;
mod tune;
mod ui;

pub use beats::{Beats, edited as edited_beats};
pub use catalog::*;
pub use runtime::*;
pub use tune::{Tune, tune_for};
pub use ui::*;

fn data_dir() -> Result<PathBuf, String> {
    eframe::storage_dir("soundglass").ok_or_else(|| "no data folder".to_string())
}

/// What a model is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Task {
    Denoise,
    Stems,
    Beats,
    Midi,
    Transcribe,
}

impl Task {
    pub const ALL: [Task; 5] = [Task::Denoise, Task::Transcribe, Task::Stems, Task::Midi, Task::Beats];

    pub fn title(self) -> &'static str {
        match self {
            Task::Denoise => "Remove Noise",
            Task::Transcribe => "Transcribe Speech",
            Task::Stems => "Split into Stems",
            Task::Midi => "Melody to MIDI",
            Task::Beats => "Find Beats",
        }
    }
}

/// A third-party licence, embedded so it can be read before downloading (and offline).
pub struct License {
    pub project: &'static str,
    pub spdx: &'static str,
    pub url: &'static str,
    pub text: &'static str,
}

/// Runs a helper program (tar, unzip, PowerShell); a failure gives its error output.
pub fn run(cmd: &mut std::process::Command) -> Result<(), String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd.output().map_err(|e| format!("{name}: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("{name}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// A command without a console window on Windows.
fn command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Unpacks the zip entries matching `patterns` (`*` wildcards) straight into `dest`, without
/// their folders. PowerShell, as in lightview: not every Windows has tar.exe or unzip.
#[cfg(windows)]
fn unzip(archive: &Path, dest: &Path, patterns: &[&str]) -> Result<(), String> {
    let script = "$ErrorActionPreference = 'Stop'; try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($env:SG_ARCHIVE)
        try {
            foreach ($entry in $zip.Entries) {
                if ($env:SG_PATTERNS -split '\\|' | Where-Object { $entry.Name -and $entry.FullName -like $_ }) {
                    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, [IO.Path]::Combine($env:SG_DEST, $entry.Name), $true)
                }
            }
        } finally { $zip.Dispose() }
    } catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }";
    run(command("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).env("SG_ARCHIVE", archive).env("SG_DEST", dest).env("SG_PATTERNS", patterns.join("|")))
}

/// Opens a folder or URL with the desktop's default app.
pub fn open(target: impl AsRef<std::ffi::OsStr>) {
    drop(command(if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(target).spawn());
}

#[cfg(test)]
mod tests;
