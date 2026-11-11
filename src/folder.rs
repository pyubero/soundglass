//! The audio files next to the one playing, in file-manager order. From lightview's `files.rs`.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// What symphonia 0.6 decodes with the `all` feature. It has no Opus decoder, so no .opus.
const EXTENSIONS: [&str; 14] = ["mp3", "mp2", "mp1", "flac", "wav", "ogg", "oga", "m4a", "m4b", "aac", "aif", "aiff", "caf", "mka"];

pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The audio files in `path`'s folder by name, or just `path` if the folder can't be read.
pub fn list_folder(path: &Path) -> Vec<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![path.to_path_buf()] };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).filter(|p| is_audio(p)).collect();
    let name = |p: &PathBuf| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    files.sort_by(|a, b| {
        let (a, b) = (name(a), name(b));
        natural_cmp(&a, &b).then_with(|| a.cmp(&b))
    });
    // A file opened by its full path is in the list even if its extension isn't one of ours.
    if !files.iter().any(|p| p == path) {
        files.insert(0, path.to_path_buf());
    }
    files
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
}
