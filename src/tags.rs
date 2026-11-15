//! Editing a file's tags (with lofty) and naming files from them. Tags are written into a copy
//! next to the file, the copy's audio is checked against the original's, and only then does the
//! copy replace the file — so the audio is never changed.

use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use std::path::{Path, PathBuf};
use crate::i18n::{tr, trf};

/// Largest cover accepted, in bytes.
pub const COVER_MAX_BYTES: usize = 10_000_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Cover {
    #[default]
    Keep,
    Remove,
    /// A JPEG or PNG, checked (and converted to JPEG from other formats) by `cover`.
    New(Vec<u8>),
}

/// The editable tags; an empty field means no tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagEdit {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track: String,
    pub genre: String,
    /// Whole beats per minute.
    pub bpm: String,
    /// The musical key, as "Am" or "Eb".
    pub key: String,
    pub cover: Cover,
}

/// The file's tags as editable fields. Err when lofty can't read (so can't write) the format.
pub fn read(path: &Path) -> Result<TagEdit, String> {
    // A hostile file must not take soundglass down with it.
    std::panic::catch_unwind(|| read_tags(path)).unwrap_or_else(|_| Err(tr("This file's tags can't be read.").into()))
}

fn read_tags(path: &Path) -> Result<TagEdit, String> {
    let file = lofty::read_from_path(path).map_err(|e| trf("Can't write tags to this format ({e})", &[("e", &e)]))?;
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else { return Ok(TagEdit::default()) };
    let text = |v: Option<std::borrow::Cow<str>>| v.map(|v| v.trim().to_string()).unwrap_or_default();
    Ok(TagEdit {
        title: text(tag.title()),
        artist: text(tag.artist()),
        album: text(tag.album()),
        year: tag.date().map(|d| d.year.to_string()).unwrap_or_default(),
        track: tag.track().map(|t| t.to_string()).unwrap_or_default(),
        genre: text(tag.genre()),
        bpm: text(tag.get_string(ItemKey::IntegerBpm).or(tag.get_string(ItemKey::Bpm)).map(Into::into)),
        key: text(tag.get_string(ItemKey::InitialKey).map(Into::into)),
        cover: Cover::Keep,
    })
}

/// Title, artist and album as lofty reads them: for files whose tags symphonia doesn't see
/// (a WAV's tags after its audio, where lofty writes them).
pub fn basic(path: &Path) -> Option<crate::track::Tags> {
    let t = read(path).ok()?;
    let some = |v: String| Some(v).filter(|v| !v.is_empty());
    Some(crate::track::Tags { title: some(t.title), artist: some(t.artist), album: some(t.album) })
}

/// A picked image as a new cover, if it decodes: a JPEG or PNG as it is (up to 10 MB), any other
/// format shrunk to 1000 px at most and converted to JPEG, since players often show only those two.
pub fn cover(bytes: Vec<u8>) -> Result<Cover, String> {
    use image::ImageFormat::{Jpeg, Png};
    let keep = matches!(image::guess_format(&bytes), Ok(Jpeg | Png));
    if keep && bytes.len() > COVER_MAX_BYTES {
        return Err(tr("The image is larger than 10 MB.").into());
    }
    // A big TIFF scan is decoded whole; the image crate's default limits cap a hostile file's cost.
    let img = image::load_from_memory(&bytes).map_err(|_| tr("That image can't be read."))?;
    let bytes = if keep {
        bytes
    } else {
        let img = if img.width() > 1000 || img.height() > 1000 { img.resize(1000, 1000, image::imageops::FilterType::Lanczos3) } else { img };
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
        out
    };
    Picture::from_reader(&mut &bytes[..]).map_err(|_| tr("That image can't be used as a cover."))?;
    Ok(Cover::New(bytes))
}

/// Sets `tag`'s fields to `edit`'s; the one place that does, for in-place saves and copies.
pub fn apply(edit: &TagEdit, tag: &mut Tag) {
    let set = |tag: &mut Tag, v: &str, set: fn(&mut Tag, String), remove: fn(&mut Tag)| {
        if v.trim().is_empty() { remove(tag) } else { set(tag, v.trim().to_string()) }
    };
    set(tag, &edit.title, Tag::set_title, Tag::remove_title);
    set(tag, &edit.artist, Tag::set_artist, Tag::remove_artist);
    set(tag, &edit.album, Tag::set_album, Tag::remove_album);
    set(tag, &edit.genre, Tag::set_genre, Tag::remove_genre);
    match edit.year.trim().parse::<u16>() {
        Ok(year) => tag.set_date(lofty::tag::items::Timestamp { year, ..Default::default() }),
        Err(_) => tag.remove_date(),
    }
    match edit.track.trim().parse::<u32>() {
        Ok(n) => tag.set_track(n),
        Err(_) => tag.remove_track(),
    }
    tag.remove_key(ItemKey::IntegerBpm);
    tag.remove_key(ItemKey::Bpm);
    // shortcut: RIFF INFO and AIFF text have no BPM or key field, so those are dropped there.
    if let Ok(bpm) = edit.bpm.trim().parse::<u32>() {
        set_bpm(tag, bpm);
    }
    if edit.key.trim().is_empty() {
        tag.remove_key(ItemKey::InitialKey);
    } else {
        tag.insert_text(ItemKey::InitialKey, edit.key.trim().to_string());
    }
    set_cover(tag, &edit.cover);
}

/// ID3 and MP4 have a whole-number BPM field; Vorbis comments and APE only the text one.
fn set_bpm(tag: &mut Tag, bpm: u32) {
    let _ = tag.insert_text(ItemKey::IntegerBpm, bpm.to_string()) || tag.insert_text(ItemKey::Bpm, bpm.to_string());
}

fn set_cover(tag: &mut Tag, cover: &Cover) {
    match cover {
        Cover::Keep => {}
        Cover::Remove => {
            while tag.picture_count() > 0 {
                tag.remove_picture(0);
            }
        }
        Cover::New(bytes) => {
            if let Ok(mut picture) = Picture::from_reader(&mut &bytes[..]) {
                picture.set_pic_type(PictureType::CoverFront);
                tag.remove_picture_type(PictureType::CoverFront);
                tag.push_picture(picture);
            }
        }
    }
}

/// Writes `edit` into the file at `path`: into a hidden copy next to it first, whose audio must
/// decode to exactly the original's samples, then renamed over it.
pub fn write_in_place(path: &Path, edit: &TagEdit) -> Result<(), String> {
    write_with(path, |tag| apply(edit, tag))
}

/// Puts `cover` into the file at `path`, the other tags untouched; written as `write_in_place` does.
pub fn write_cover(path: &Path, cover: &Cover) -> Result<(), String> {
    write_with(path, |tag| set_cover(tag, cover))
}

/// Puts `bpm` into the file at `path`, the other tags untouched; written as `write_in_place` does.
pub fn write_bpm(path: &Path, bpm: u32) -> Result<(), String> {
    write_with(path, |tag| set_bpm(tag, bpm))
}

/// Whether the file embeds a picture. Err when lofty can't read (so can't write) the format.
pub fn has_cover(path: &Path) -> Result<bool, String> {
    let count = || -> Result<bool, String> {
        let file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
        Ok(file.tags().iter().any(|t| t.picture_count() > 0))
    };
    std::panic::catch_unwind(count).unwrap_or_else(|_| Err(tr("This file's tags can't be read.").into()))
}

fn write_with(path: &Path, change: impl FnOnce(&mut Tag)) -> Result<(), String> {
    // Through a symlink to the real file: renaming over the link would replace it with a copy.
    let path = &std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or(tr("no file name"))?.to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.soundglass-part.{ext}"));
    // The edit only reads its captures, so nothing is left half-changed after a panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        std::fs::copy(path, &temp).map_err(|e| e.to_string())?;
        let file = lofty::read_from_path(&temp).map_err(|e| e.to_string())?;
        // Only another tag (an MP3's ID3v1): start from its fields, or the new primary tag would hide them.
        let mut tag = file.primary_tag().or_else(|| file.first_tag()).cloned().unwrap_or_else(|| Tag::new(file.primary_tag_type()));
        tag.re_map(file.primary_tag_type());
        change(&mut tag);
        tag.save_to_path(&temp, lofty::config::WriteOptions::default()).map_err(|e| e.to_string())?;
        if audio(path)? != audio(&temp)? {
            return Err(tr("writing the tags would change the audio, so nothing was written").into());
        }
        crate::save::sync(&temp)?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    }))
    .unwrap_or_else(|_| Err(tr("the tag writer failed").into()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The decoded audio's frame count and a hash of every sample's bits.
fn audio(path: &Path) -> Result<(u64, u64), String> {
    let (mut n, mut hash) = (0u64, 0xcbf2_9ce4_8422_2325u64);
    crate::track::decode(path, |_, _, _| {}, |samples, _, _, _| {
        for s in samples {
            hash = (hash ^ s.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
        n += samples.len() as u64;
        Ok(())
    })?;
    Ok((n, hash))
}

/// The placeholders a rename pattern may use.
pub const PLACEHOLDERS: [&str; 6] = ["title", "artist", "album", "year", "track", "genre"];
pub const DEFAULT_PATTERN: &str = "{track} - {artist} - {title}";

/// The file name `pattern` gives with these tags and extension, or "" when no tag it uses is set.
/// A missing tag drops its placeholder and the text before it (or, for the first, after it).
pub fn file_name(pattern: &str, tags: &TagEdit, ext: &str) -> String {
    let value = |key: &str| -> Option<String> {
        let v = match key {
            "title" => &tags.title,
            "artist" => &tags.artist,
            "album" => &tags.album,
            "year" => &tags.year,
            "track" => return Some(tags.track.trim()).filter(|t| !t.is_empty()).map(|t| t.parse::<u32>().map_or(t.to_string(), |n| format!("{n:02}"))),
            "genre" => &tags.genre,
            _ => return None,
        };
        Some(v.trim().to_string()).filter(|v| !v.is_empty())
    };
    // Literal text and placeholders alternate: lit, {ph}, lit, {ph}, …, lit.
    let (mut lits, mut keys, mut rest) = (Vec::new(), Vec::new(), pattern);
    while let Some((before, after)) = rest.split_once('{').and_then(|(b, a)| Some((b, a.split_once('}')?))) {
        lits.push(before);
        keys.push(after.0);
        rest = after.1;
    }
    lits.push(rest);
    let mut stem = String::new();
    let mut any = false;
    for (i, key) in keys.iter().enumerate() {
        if let Some(v) = value(key) {
            // The text before this placeholder: the leading text for the first one shown, else its own separator.
            stem += if any { lits[i] } else { lits[0] };
            stem += &v;
            any = true;
        }
    }
    if !any {
        return String::new();
    }
    stem += lits[keys.len()];
    let clean: String = stem.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let mut clean = clean.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    while clean.len() > 200 {
        clean.pop();
    }
    // Windows can't make a file named CON, NUL, COM1… whatever its extension.
    let base = clean.split('.').next().unwrap_or_default().trim_end();
    let upper = base.to_ascii_uppercase();
    let numbered = |prefix: &str| upper.len() == 4 && upper.starts_with(prefix) && matches!(upper.as_bytes()[3], b'1'..=b'9');
    if ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str()) || numbered("COM") || numbered("LPT") {
        clean.insert(base.len(), '_');
    }
    if clean.is_empty() { clean } else { format!("{clean}.{ext}") }
}

/// Renames `from` to `name` in the same folder, never replacing a file that is there.
pub fn rename(from: &Path, name: &str) -> Result<PathBuf, String> {
    let to = from.with_file_name(name);
    if to.exists() {
        // On a disk that ignores case (Windows, macOS), a new name in other letters is the file itself.
        let same_letters = from.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase() == name.to_lowercase());
        if same_letters && crate::save::same_file(&to, from) {
            return std::fs::rename(from, &to).map(|()| to).map_err(|e| e.to_string());
        }
        return Err(tr("A file with that name already exists.").into());
    }
    // A hard link fails if the name was taken meanwhile; then the old name goes.
    match std::fs::hard_link(from, &to) {
        Ok(()) => {
            if let Err(e) = std::fs::remove_file(from) {
                let _ = std::fs::remove_file(&to);
                return Err(e.to_string());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(tr("A file with that name already exists.").into()),
        // Disks without hard links (FAT, exFAT): checked above, so rename.
        Err(_) => std::fs::rename(from, &to).map_err(|e| e.to_string())?,
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_converted_or_kept() {
        let encode = |w, h, format| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(w, h).write_to(&mut out, format).unwrap();
            out.into_inner()
        };
        let Ok(Cover::New(jpeg)) = cover(encode(2000, 1500, image::ImageFormat::Tiff)) else { panic!("TIFF refused") };
        let img = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((image::guess_format(&jpeg).unwrap(), img.width(), img.height()), (image::ImageFormat::Jpeg, 1000, 750));
        let png = encode(20, 10, image::ImageFormat::Png);
        assert_eq!(cover(png.clone()), Ok(Cover::New(png)));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-tags-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A short FLAC, written by our own Save As.
    fn flac(dir: &Path) -> PathBuf {
        let path = dir.join("song.flac");
        let track = crate::track::Track::from_samples(44100, 2, (0..44100 * 2).map(|i| (i as f32 * 0.01).sin() * 0.3).collect());
        crate::save::save(&track, &crate::edit::Edit::whole(44100), crate::save::Choice::default(), &path, None).unwrap();
        path
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 4).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    #[test]
    fn writes_tags_in_place_and_keeps_the_audio() {
        let dir = scratch("inplace");
        let path = flac(&dir);
        let before = audio(&path).unwrap();
        let edit = TagEdit { title: "St. Louis Blues".into(), artist: "Louis".into(), year: "1954".into(), track: "3".into(), bpm: "119".into(), key: "Fm".into(), cover: cover(png()).unwrap(), ..TagEdit::default() };
        write_in_place(&path, &edit).unwrap();
        assert_eq!(read(&path).unwrap(), TagEdit { cover: Cover::Keep, ..edit });
        assert_eq!(audio(&path).unwrap(), before);
        let file = lofty::read_from_path(&path).unwrap();
        assert_eq!(file.primary_tag().unwrap().picture_count(), 1);
        // Removing the cover and a field.
        write_in_place(&path, &TagEdit { title: "Only".into(), cover: Cover::Remove, ..TagEdit::default() }).unwrap();
        let file = lofty::read_from_path(&path).unwrap();
        assert_eq!(file.primary_tag().unwrap().picture_count(), 0);
        assert_eq!(read(&path).unwrap(), TagEdit { title: "Only".into(), ..TagEdit::default() });
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp file left");
    }

    #[test]
    fn writes_tags_into_a_wav() {
        let dir = scratch("wav");
        let path = dir.join("song.wav");
        let track = crate::track::Track::from_samples(44100, 2, vec![0.1; 4410 * 2]);
        crate::save::save(&track, &crate::edit::Edit::whole(4410), crate::save::Choice { kind: crate::save::Kind::Wav, ..Default::default() }, &path, None).unwrap();
        let before = audio(&path).unwrap();
        let edit = TagEdit { title: "Wave".into(), genre: "Jazz".into(), bpm: "96".into(), key: "Eb".into(), ..TagEdit::default() };
        write_in_place(&path, &edit).unwrap();
        assert_eq!(read(&path).unwrap(), edit);
        assert_eq!(audio(&path).unwrap(), before);
    }

    #[test]
    fn the_audio_check_tells_files_apart() {
        let dir = scratch("audio");
        let a = flac(&dir);
        let b = dir.join("other.wav");
        let track = crate::track::Track::from_samples(44100, 2, vec![0.1; 44100 * 2]);
        crate::save::save(&track, &crate::edit::Edit::whole(44100), crate::save::Choice { kind: crate::save::Kind::Wav, ..Default::default() }, &b, None).unwrap();
        assert_ne!(audio(&a).unwrap(), audio(&b).unwrap());
    }

    #[test]
    fn writes_a_bpm_and_keeps_the_other_tags() {
        let dir = scratch("bpm");
        let path = flac(&dir);
        write_in_place(&path, &TagEdit { title: "Kept".into(), year: "1954".into(), ..TagEdit::default() }).unwrap();
        let before = audio(&path).unwrap();
        write_bpm(&path, 119).unwrap();
        assert_eq!(read(&path).unwrap(), TagEdit { title: "Kept".into(), year: "1954".into(), bpm: "119".into(), ..TagEdit::default() });
        assert_eq!(audio(&path).unwrap(), before);
    }

    #[test]
    fn writes_a_cover_and_keeps_the_other_tags() {
        let dir = scratch("cover");
        let path = flac(&dir);
        write_in_place(&path, &TagEdit { title: "Kept".into(), ..TagEdit::default() }).unwrap();
        assert_eq!(has_cover(&path), Ok(false));
        let before = audio(&path).unwrap();
        write_cover(&path, &cover(png()).unwrap()).unwrap();
        write_cover(&path, &cover(png()).unwrap()).unwrap();
        assert_eq!(has_cover(&path), Ok(true));
        assert_eq!(lofty::read_from_path(&path).unwrap().primary_tag().unwrap().picture_count(), 1, "replaced, not added");
        assert_eq!(read(&path).unwrap().title, "Kept");
        assert_eq!(audio(&path).unwrap(), before);
        write_cover(&path, &Cover::Remove).unwrap();
        assert_eq!((has_cover(&path), read(&path).unwrap().title.as_str()), (Ok(false), "Kept"));
        assert!(has_cover(&dir.join("missing.mp3")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink() {
        let dir = scratch("symlink");
        let real = flac(&dir);
        let link = dir.join("link.flac");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_in_place(&link, &TagEdit { title: "Linked".into(), ..TagEdit::default() }).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(read(&real).unwrap().title, "Linked");
    }

    #[test]
    fn a_cover_keeps_the_tags_of_an_id3v1_only_mp3() {
        let dir = scratch("id3v1");
        let path = dir.join("old.mp3");
        let track = crate::track::Track::from_samples(44100, 2, vec![0.1; 44100 * 2]);
        crate::save::save(&track, &crate::edit::Edit::whole(44100), crate::save::Choice { kind: crate::save::Kind::Mp3, ..Default::default() }, &path, None).unwrap();
        lofty::tag::TagType::Id3v2.remove_from_path(&path, lofty::config::WriteOptions::default()).unwrap();
        let mut v1 = Tag::new(lofty::tag::TagType::Id3v1);
        v1.set_title("Old Title".into());
        v1.save_to_path(&path, lofty::config::WriteOptions::default()).unwrap();
        assert_eq!(read(&path).unwrap().title, "Old Title");
        write_cover(&path, &cover(png()).unwrap()).unwrap();
        assert_eq!(read(&path).unwrap().title, "Old Title", "the new ID3v2 tag carries the ID3v1 fields");
        assert_eq!(has_cover(&path), Ok(true));
    }

    #[test]
    fn rejects_bad_covers() {
        assert!(cover(b"not an image at all".to_vec()).is_err());
        assert!(cover(vec![0; COVER_MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn names_from_tags() {
        let tags = TagEdit { title: "St. Louis Blues".into(), artist: "Louis Armstrong".into(), track: "1".into(), ..TagEdit::default() };
        assert_eq!(file_name(DEFAULT_PATTERN, &tags, "mp3"), "01 - Louis Armstrong - St. Louis Blues.mp3");
        let no_artist = TagEdit { artist: String::new(), ..tags.clone() };
        assert_eq!(file_name(DEFAULT_PATTERN, &no_artist, "mp3"), "01 - St. Louis Blues.mp3");
        let no_track = TagEdit { track: String::new(), ..tags.clone() };
        assert_eq!(file_name(DEFAULT_PATTERN, &no_track, "mp3"), "Louis Armstrong - St. Louis Blues.mp3");
        assert_eq!(file_name(DEFAULT_PATTERN, &TagEdit::default(), "mp3"), "");
        let odd = TagEdit { title: "AC/DC: What?".into(), ..TagEdit::default() };
        assert_eq!(file_name("{title}", &odd, "flac"), "AC_DC_ What_.flac");
        assert_eq!(file_name("Disc {year} {nope}", &TagEdit { year: "1954".into(), ..TagEdit::default() }, "ogg"), "Disc 1954.ogg");
        assert!(file_name("{title}", &TagEdit { title: "x".repeat(300), ..TagEdit::default() }, "mp3").len() <= 204);
    }

    #[test]
    fn renames_without_replacing() {
        let dir = scratch("rename");
        let (a, b) = (dir.join("a.mp3"), dir.join("b.mp3"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        assert!(rename(&a, "b.mp3").is_err());
        assert_eq!((std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()), (b"a".to_vec(), b"b".to_vec()));
        let c = rename(&a, "c.mp3").unwrap();
        assert!(!a.exists() && std::fs::read(c).unwrap() == b"a");
    }

    #[test]
    fn renames_to_other_letter_case_but_never_over_another_file() {
        let dir = scratch("case");
        let song = dir.join("song.mp3");
        std::fs::write(&song, b"song").unwrap();
        // On Windows "Song.mp3" already "exists" (it is this file): the case-only path.
        let new = rename(&song, "Song.mp3").unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!((names, std::fs::read(new).unwrap()), (vec!["Song.mp3".to_string()], b"song".to_vec()));
        // Where case matters (Linux), "Other.mp3" is someone else's file and stays.
        if !dir.join("SONG.MP3").exists() {
            std::fs::write(dir.join("other.mp3"), b"mine").unwrap();
            std::fs::write(dir.join("Other.mp3"), b"theirs").unwrap();
            assert!(rename(&dir.join("other.mp3"), "Other.mp3").is_err());
            assert_eq!(std::fs::read(dir.join("Other.mp3")).unwrap(), b"theirs");
        }
    }

    #[test]
    fn reserved_windows_names_get_an_underscore() {
        let title = |t: &str| file_name("{title}", &TagEdit { title: t.into(), ..TagEdit::default() }, "mp3");
        assert_eq!(title("con"), "con_.mp3");
        assert_eq!(title("NUL"), "NUL_.mp3");
        assert_eq!(title("com7"), "com7_.mp3");
        assert_eq!(title("Aux.live"), "Aux_.live.mp3");
        assert_eq!(title("Concert"), "Concert.mp3");
        assert_eq!(title("COM0"), "COM0.mp3");
    }
}
