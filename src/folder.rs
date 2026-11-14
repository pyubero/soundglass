//! The audio files next to the one playing, in file-manager order (from lightview's `files.rs`),
//! and the Folder panel's listing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What symphonia 0.6 decodes with the `all` feature. It has no Opus decoder, so no .opus.
pub const EXTENSIONS: [&str; 14] = ["mp3", "mp2", "mp1", "flac", "wav", "ogg", "oga", "m4a", "m4b", "aac", "aif", "aiff", "caf", "mka"];

pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The audio files in `path`'s folder by name, or just `path` if the folder can't be read.
pub fn list_folder(path: &Path) -> Vec<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![path.to_path_buf()] };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).filter(|p| is_audio(p)).collect();
    sort_natural(&mut files);
    // A file opened by its full path is in the list even if its extension isn't one of ours.
    if !files.iter().any(|p| p == path) {
        files.insert(0, path.to_path_buf());
    }
    files
}

/// Image files offered or accepted as covers; all but JPEG and PNG are converted to JPEG by `tags::cover`.
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp", "bmp", "gif"];

/// The one image (`IMAGE_EXTS`) in `dir` (hidden ones left out), or None when there are none or several.
pub fn lone_image(dir: &Path) -> Option<PathBuf> {
    let image = |p: &PathBuf| {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        IMAGE_EXTS.contains(&ext.as_str()) && p.file_name().is_some_and(|n| !n.to_string_lossy().starts_with('.')) && p.is_file()
    };
    let mut images = std::fs::read_dir(dir).ok()?.filter_map(|e| Some(e.ok()?.path())).filter(image);
    let first = images.next()?;
    images.next().is_none().then_some(first)
}

/// The Folder panel's listing of `dir`: its subfolders and its audio files, each by name,
/// hidden ones (".name") left out.
pub fn list_dir(dir: &Path) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for path in std::fs::read_dir(dir)?.filter_map(|e| Some(e.ok()?.path())) {
        if path.file_name().is_none_or(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if path.is_dir() {
            dirs.push(path);
        } else if is_audio(&path) {
            files.push(path);
        }
    }
    sort_natural(&mut dirs);
    sort_natural(&mut files);
    Ok((dirs, files))
}

fn sort_natural(paths: &mut [PathBuf]) {
    let name = |p: &PathBuf| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    paths.sort_by(|a, b| {
        let (a, b) = (name(a), name(b));
        natural_cmp(&a, &b).then_with(|| a.cmp(&b))
    });
}

/// Case-insensitive comparison where runs of digits compare as numbers ("track2" < "track10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let digits = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                        s.push(c);
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (x, y) = (digits(&mut a), digits(&mut b));
                let ord = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// `path` as a `file://` URI, escaped so that a comma can't split `dbus-send`'s array.
#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> String {
    use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
    use std::os::unix::ffi::OsStrExt;
    const ESCAPED: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    format!("file://{}", percent_encode(path.as_os_str().as_bytes(), ESCAPED))
}

/// Whether a Trash item is `path`. When Windows hides known extensions, `item.name` has none,
/// but `item.id` (like `$RAB12CD.mp3`) keeps it. From lightview.
fn is_trashed_copy(item: &trash::TrashItem, path: &Path) -> bool {
    let same_name = Some(item.name.as_os_str()) == path.file_name() || (Some(item.name.as_os_str()) == path.file_stem() && Path::new(&item.id).extension() == path.extension());
    // Compared canonically, as `path` is: the Trash may spell the folder differently.
    same_name && std::fs::canonicalize(&item.original_parent).is_ok_and(|p| Some(p.as_path()) == path.parent())
}

/// Puts a file back from the Trash, to the place it was deleted from. From lightview.
pub fn restore_trashed(path: &Path) -> Result<(), String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let newest = items.into_iter().filter(|i| is_trashed_copy(i, path)).max_by_key(|i| i.time_deleted);
    let item = newest.ok_or("It's no longer in the Trash.")?;
    // The full name, or the file would come back without its extension.
    let name = path.file_name().unwrap_or_default().to_owned();
    trash::os_limited::restore_all([trash::TrashItem { name, ..item }]).map_err(|e| e.to_string())
}

/// What tells two versions of a file apart: when it was modified, and its length.
pub type Stamp = (std::time::SystemTime, u64);

pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Watches one file for changes made by other programs. From lightview.
#[derive(Default)]
struct Watch {
    path: Option<PathBuf>,
    /// The version last reported (or first seen), and the one seen at the previous check.
    known: Option<Stamp>,
    last: Option<Stamp>,
}

impl Watch {
    /// One check of `now` (the path to watch), whose stamp is `current`: a change once it has
    /// held for a whole interval, so a half-written file isn't reported. A file that's gone reports nothing.
    fn step(&mut self, now: Option<PathBuf>, current: Option<Stamp>) -> Option<(PathBuf, Stamp)> {
        if now != self.path {
            (self.path, self.known, self.last) = (now, current, current);
            return None;
        }
        let mut changed = None;
        if let (Some(p), Some(new)) = (&self.path, current) {
            if self.known.is_none() {
                self.known = current;
            } else if current != self.known && current == self.last {
                self.known = current;
                changed = Some((p.clone(), new));
            }
        }
        self.last = current;
        changed
    }
}

/// Checks the path put in the returned slot every `interval` on its own thread, so a slow network
/// folder can't hold up the window, and sends each change, calling `notify`. Ends with the slot.
pub fn watch(notify: impl Fn() + Send + 'static, interval: std::time::Duration) -> (Arc<Mutex<Option<PathBuf>>>, std::sync::mpsc::Receiver<(PathBuf, Stamp)>) {
    let (slot, (tx, rx)) = (Arc::new(Mutex::new(None)), std::sync::mpsc::channel());
    let shared = Arc::clone(&slot);
    std::thread::spawn(move || {
        let mut watch = Watch::default();
        while Arc::strong_count(&shared) > 1 {
            std::thread::sleep(interval);
            let now: Option<PathBuf> = shared.lock().ok().and_then(|p| p.clone());
            let current = now.as_deref().and_then(stamp);
            if let Some(change) = watch.step(now, current) && tx.send(change).is_ok() {
                notify();
            }
        }
    });
    (slot, rx)
}

/// The desktop entry `xdg-mime` points at, which also lists the types soundglass plays (one source with install.sh).
const DESKTOP_FILE: &str = "soundglass.desktop";
const DESKTOP_ENTRY: &str = include_str!("../soundglass.desktop");

/// The MimeType list of the desktop entry.
fn mime_types() -> Vec<&'static str> {
    DESKTOP_ENTRY.lines().find_map(|l| l.strip_prefix("MimeType=")).unwrap_or_default().split(';').filter(|t| !t.is_empty()).collect()
}

/// Linux, with a desktop entry for `xdg-mime` to point at (the .deb or install.sh put it there).
/// Windows sets its defaults in its own Settings, so never there.
pub fn desktop_installed() -> bool {
    let home = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share/applications").join(DESKTOP_FILE));
    cfg!(target_os = "linux") && (Path::new("/usr/share/applications").join(DESKTOP_FILE).exists() || home.is_some_and(|p| p.exists()))
}

/// Whether soundglass already plays MP3s by default. Runs xdg-mime: off the UI thread.
pub fn is_default() -> bool {
    std::process::Command::new("xdg-mime").args(["query", "default", "audio/mpeg"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == DESKTOP_FILE)
}

/// Makes soundglass this user's default for every type in its desktop entry.
pub fn make_default() -> Result<(), String> {
    crate::ai::run(std::process::Command::new("xdg-mime").args(["default", DESKTOP_FILE]).args(mime_types()))
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    drop(std::process::Command::new(program).arg(url).spawn());
}

/// Opens the file manager with `path` highlighted, or just its folder if the desktop can't do that.
#[cfg(target_os = "linux")]
pub fn show_in_folder(path: &Path) {
    let (uri, dir) = (file_uri(path), path.parent().unwrap_or(Path::new("/")).to_path_buf());
    std::thread::spawn(move || {
        let call = ["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"];
        let shown = std::process::Command::new("dbus-send").args(call).arg(format!("array:string:{uri}")).arg("string:").output();
        if !shown.is_ok_and(|o| o.status.success()) {
            drop(std::process::Command::new("xdg-open").arg(dir).spawn());
        }
    });
}

/// Opens Explorer with `path` highlighted. It takes `/select,` and the quoted path as one argument,
/// which `Command::arg`'s own quoting would break, and doesn't understand `canonicalize`'s `\\?\` form.
#[cfg(windows)]
pub fn show_in_folder(path: &Path) {
    use std::os::windows::process::CommandExt;
    let path = path.to_string_lossy();
    let path = match path.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => path.strip_prefix(r"\\?\").unwrap_or(&path).to_string(),
    };
    drop(std::process::Command::new("explorer").raw_arg(format!("/select,\"{path}\"")).spawn());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_types_come_from_the_desktop_entry() {
        let types = mime_types();
        assert!(types.contains(&"audio/mpeg") && types.contains(&"audio/flac"), "{types:?}");
        assert!(types.iter().all(|t| t.starts_with("audio/")));
    }

    #[test]
    fn a_change_is_reported_once_it_holds() {
        let t = |s: u64| Some((std::time::UNIX_EPOCH + std::time::Duration::from_secs(s), 10));
        let (mut w, p) = (Watch::default(), Some(PathBuf::from("/a.mp3")));
        assert_eq!(w.step(p.clone(), t(1)), None);
        assert_eq!(w.step(p.clone(), t(1)), None);
        // Still being written: reported only when the next check sees the same.
        assert_eq!(w.step(p.clone(), t(2)), None);
        assert_eq!(w.step(p.clone(), t(3)), None);
        assert_eq!(w.step(p.clone(), t(3)), Some(("/a.mp3".into(), t(3).unwrap())));
        assert_eq!(w.step(p.clone(), t(3)), None);
        // Gone, then another file: nothing.
        assert_eq!(w.step(p.clone(), None), None);
        assert_eq!(w.step(Some("/b.mp3".into()), t(9)), None);
        assert_eq!(w.step(Some("/b.mp3".into()), t(9)), None);
    }

    #[test]
    fn finds_trashed_file_with_hidden_extension() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = dir.join("song.mp3");
        let item = |name: &str, id: &str| trash::TrashItem { id: id.into(), name: name.into(), original_parent: dir.clone(), time_deleted: 0 };
        assert!(is_trashed_copy(&item("song.mp3", "/trash/info/song.mp3.trashinfo"), &path));
        assert!(is_trashed_copy(&item("song", r"C:\$Recycle.Bin\S-1\$RAB12CD.mp3"), &path));
        assert!(!is_trashed_copy(&item("song", r"C:\$Recycle.Bin\S-1\$RAB12CD.flac"), &path));
        assert!(!is_trashed_copy(&item("other.mp3", "/trash/info/other.mp3.trashinfo"), &path));
        assert!(!is_trashed_copy(&trash::TrashItem { original_parent: dir.join("elsewhere"), ..item("song.mp3", "x") }, &path));
    }

    // Uses a Trash under target/: `XDG_DATA_HOME=$PWD/target/trashtest cargo test --release -- --ignored trash_round_trip`.
    #[test]
    #[ignore]
    fn trash_round_trip() {
        let data = std::env::var("XDG_DATA_HOME").expect("set XDG_DATA_HOME to a folder under target/");
        assert!(data.contains("/target/"), "XDG_DATA_HOME must be under target/");
        std::fs::create_dir_all(&data).unwrap();
        let path = std::fs::canonicalize(&data).unwrap().join(format!("soundglass-trash-{}.mp3", std::process::id()));
        std::fs::write(&path, "audio").unwrap();
        trash::delete(&path).unwrap();
        assert!(!path.exists());
        restore_trashed(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "audio");
        std::fs::remove_file(&path).unwrap();
        assert!(restore_trashed(&path).is_err());
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("img2.jpg", "img10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("IMG_b.jpg", "img_A.jpg"), Ordering::Greater);
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Equal);
        assert_eq!(natural_cmp("photo", "photo1"), Ordering::Less);
    }

    /// The Windows installer registers every extension the player opens (the .desktop file lists MIME types instead).
    #[test]
    fn installer_lists_every_extension() {
        let wxs = include_str!("../packaging/windows/soundglass.wxs");
        for e in EXTENSIONS {
            assert!(wxs.contains(&format!(r#"Classes\.{e}\OpenWithProgids""#)), ".{e} missing from Open with in soundglass.wxs");
            assert!(wxs.contains(&format!(r#"FileAssociations" Name=".{e}""#)), ".{e} missing from Default apps in soundglass.wxs");
        }
    }

    #[test]
    fn folder_filter_and_order() {
        let dir = std::env::temp_dir().join(format!("soundglass-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["10 b.MP3", "2 b.flac", "1 a.wav", "notes.txt", "cover.jpg", "x.opus"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let names: Vec<String> = list_folder(&dir.join("1 a.wav")).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["1 a.wav", "2 b.flac", "10 b.MP3"]);
    }

    #[test]
    fn one_image_only() {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-image", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a.mp3", "notes.txt", ".hidden.png"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let none = lone_image(&dir);
        std::fs::write(dir.join("Cover.JPG"), b"").unwrap();
        let one = lone_image(&dir);
        std::fs::write(dir.join("back.png"), b"").unwrap();
        let two = lone_image(&dir);
        std::fs::remove_file(dir.join("Cover.JPG")).unwrap();
        std::fs::remove_file(dir.join("back.png")).unwrap();
        std::fs::write(dir.join("scan.TIF"), b"").unwrap();
        let tif = lone_image(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((none, one, two, tif), (None, Some(dir.join("Cover.JPG")), None, Some(dir.join("scan.TIF"))));
    }

    #[test]
    fn browse_listing() {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-browse", std::process::id()));
        for d in ["sub 10", "sub 9", ".hidden"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["b10.mp3", "b2.flac", "notes.txt", ".x.mp3"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let names = |v: Vec<PathBuf>| v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let (dirs, files) = list_dir(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((names(dirs), names(files)), (vec!["sub 9".to_string(), "sub 10".into()], vec!["b2.flac".to_string(), "b10.mp3".into()]));
        assert!(list_dir(&dir).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn uri_escapes_spaces_and_commas() {
        assert_eq!(file_uri(Path::new("/a b/c,d.mp3")), "file:///a%20b/c%2Cd.mp3");
    }
}
