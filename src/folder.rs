//! The audio files next to the one playing, in file-manager order (from lightview's `files.rs`),
//! and the Folder panel's listing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

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
