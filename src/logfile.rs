//! The debug log: soundglass.log in the data folder, rotated to soundglass.log.1 at `LIMIT`,
//! so the two hold the most recent 0.5–1 MB. Never log from the cpal data callback or
//! `edit::read`: logging locks and allocates.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 512 * 1024;
/// How many names `private` remembers; the oldest go first.
const NAMES: usize = 2000;
/// Shorter names are left alone: they would blank ordinary words.
const SHORTEST: usize = 4;

/// File and folder names kept out of the log, with what's written instead.
static PRIVATE: Mutex<VecDeque<(String, &'static str)>> = Mutex::new(VecDeque::new());

/// Keeps `path` out of the log: its full path, name, stem and folder are written as `<file>` or
/// `<folder>`, wherever they turn up (our messages, errors, panics, the libraries' warnings).
pub fn private(path: &Path) {
    let lossy = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
    let dir = path.parent();
    let names = [
        Some((path.to_string_lossy().into_owned(), "<file>")),
        dir.map(|d| (d.to_string_lossy().into_owned(), "<folder>")),
        path.file_name().map(|n| (lossy(n), "<file>")),
        path.file_stem().map(|n| (lossy(n), "<file>")),
        dir.and_then(|d| d.file_name()).map(|n| (lossy(n), "<folder>")),
    ];
    let mut known = PRIVATE.lock().unwrap_or_else(|e| e.into_inner());
    for (name, tag) in names.into_iter().flatten() {
        if name.chars().count() >= SHORTEST && !known.iter().any(|(k, _)| *k == name) {
            known.push_back((name, tag));
        }
    }
    while known.len() > NAMES {
        known.pop_front();
    }
}

/// `line` with the private names blanked, longest first so a full path goes before the names in
/// it, and the home folder as `~`.
fn scrub(line: String) -> String {
    let known = PRIVATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<&(String, &str)> = known.iter().filter(|(k, _)| line.contains(k.as_str())).collect();
    names.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    let mut line = names.into_iter().fold(line, |l, (k, tag)| l.replace(k.as_str(), tag));
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| h.to_string_lossy().into_owned())
        && home.len() > 1
    {
        line = line.replace(&home, "~");
    }
    line
}

pub fn path() -> Option<PathBuf> {
    eframe::storage_dir("soundglass").map(|d| d.join("soundglass.log"))
}

/// The open file and its size; rotated when a line would take it past the limit.
struct Writer {
    path: PathBuf,
    limit: u64,
    file: File,
    len: u64,
}

impl Writer {
    fn open(path: PathBuf, limit: u64) -> std::io::Result<Writer> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let len = file.metadata()?.len();
        Ok(Writer { path, limit, file, len })
    }

    fn write(&mut self, line: &str) {
        if self.len > 0 && self.len + line.len() as u64 > self.limit {
            let old = self.path.with_extension("log.1");
            if std::fs::rename(&self.path, &old).is_ok()
                && let Ok(file) = File::create(&self.path)
            {
                (self.file, self.len) = (file, 0);
            }
        }
        // A full disk or a removed folder must never stop the app.
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.len += line.len() as u64;
        }
    }
}

struct Logger(Mutex<Writer>);

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= if m.target().starts_with("soundglass") { log::Level::Info } else { log::Level::Warn }
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        // Only ours, as before the log; dependencies' warnings and panics (printed by the default hook) stay in the file.
        if r.level() <= log::Level::Warn && r.target().starts_with("soundglass") {
            eprintln!("soundglass: {}", r.args());
        }
        let line = scrub(format!("{} {} {}: {}\n", timestamp(SystemTime::now()), r.level(), r.target(), r.args()));
        if let Ok(mut w) = self.0.lock() {
            w.write(&line);
        }
    }

    fn flush(&self) {}
}

/// ureq middleware: logs each request's host and path (not its query, which can hold a key),
/// its status and how long the reply took to start.
pub fn request(req: ureq::http::Request<ureq::SendBody>, next: ureq::middleware::MiddlewareNext) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let (method, host, path) = (req.method().clone(), req.uri().host().unwrap_or_default().to_string(), req.uri().path().to_string());
    let t0 = std::time::Instant::now();
    let reply = next.handle(req);
    let status = reply.as_ref().map_or_else(|e| e.to_string(), |r| r.status().to_string());
    log::info!("{method} {host}{path}: {status} in {} ms", t0.elapsed().as_millis());
    reply
}

/// Starts the log and logs panics, including the ones `catch_unwind` turns into messages.
pub fn init() {
    let Some(writer) = path().and_then(|p| Writer::open(p, LIMIT).ok()) else { return };
    if log::set_boxed_logger(Box::new(Logger(Mutex::new(writer)))).is_err() {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(target: "panic", "{info}");
        default(info);
    }));
}

/// `2026-11-15 10:04:31Z`, in UTC.
fn timestamp(t: SystemTime) -> String {
    let s = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil(s / 86400);
    format!("{y}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", s % 86400 / 3600, s % 3600 / 60, s % 60)
}

/// Days since 1970-01-01 to year, month, day (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + u64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01 00:00:00Z");
        // 2024-02-29 12:34:56 UTC, a leap day.
        assert_eq!(timestamp(UNIX_EPOCH + std::time::Duration::from_secs(1_709_210_096)), "2024-02-29 12:34:56Z");
    }

    #[test]
    fn private_names_are_blanked() {
        private(Path::new("/music/Some Album/03 - A Song.flac"));
        private(Path::new("/x/ab.mp3"));
        let line = scrub("can't read /music/Some Album/03 - A Song.flac; 03 - A Song.flac in Some Album; 03 - A Song again; ab stays\n".into());
        assert_eq!(line, "can't read <file>; <file> in <folder>; <file> again; ab stays\n");
        assert!(!scrub("/music/Some Album/other.mp3".into()).contains("Some Album"));
    }

    #[test]
    fn rotates_at_the_limit() {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-log", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("soundglass.log");
        let mut w = Writer::open(path.clone(), 1000).unwrap();
        for i in 0..500 {
            w.write(&format!("line {i:04} of the test log\n"));
        }
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        assert_eq!(names, ["soundglass.log", "soundglass.log.1"]);
        assert!(std::fs::metadata(&path).unwrap().len() <= 1000);
        assert!(std::fs::metadata(path.with_extension("log.1")).unwrap().len() <= 1000);
        assert!(std::fs::read_to_string(&path).unwrap().ends_with("line 0499 of the test log\n"));
        // Reopening appends to what's there.
        let before = std::fs::metadata(&path).unwrap().len();
        Writer::open(path.clone(), 1000).unwrap().write("x\n");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before + 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
