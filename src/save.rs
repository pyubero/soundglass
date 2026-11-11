//! Save As: the edited track encoded into a new file with the original's tags, never over the
//! original, written to a temp file, checked, then renamed into place.

use crate::edit::{self, Edit};
use crate::track::Track;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Wav,
}

impl Kind {
    pub const ALL: [Kind; 1] = [Kind::Wav];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Wav => "WAV",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Kind::Wav => "wav",
        }
    }

    fn file_type(self) -> lofty::file::FileType {
        match self {
            Kind::Wav => lofty::file::FileType::Wav,
        }
    }
}

/// The format and, for each format, its quality, so switching formats keeps each one's choice.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub kind: Kind,
    /// 16, 24, or 32 for 32-bit float.
    pub wav_bits: u16,
}

impl Default for Choice {
    fn default() -> Choice {
        Choice { kind: Kind::Wav, wav_bits: 16 }
    }
}

/// The name Save As suggests: `<stem> (edited).<ext>` next to the original.
pub fn suggested_name(original: &Path, kind: Kind) -> String {
    format!("{} (edited).{}", original.file_stem().unwrap_or_default().to_string_lossy(), kind.ext())
}

/// Writes frames `edit` of `track` (fully decoded) to `target` as `choice` says. Ok(false) when
/// the file is saved but the original's tags could not be copied.
pub fn save(track: &Track, edit: &Edit, choice: Choice, target: &Path) -> Result<bool, String> {
    if same_file(target, &track.path) {
        return Err("That is the original file; choose another name. The original is never changed.".into());
    }
    let name = target.file_name().ok_or("no file name")?.to_string_lossy();
    // Hidden, next to the target so the rename stays on one disk, with the real extension so it decodes.
    let temp = target.with_file_name(format!(".{name}.soundglass-part.{}", choice.kind.ext()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        encode(track, edit, choice, &temp)?;
        let tags = std::panic::catch_unwind(|| copy_tags(&track.path, &temp, choice.kind)).unwrap_or(false);
        verify(&temp, track, edit)?;
        std::fs::rename(&temp, target).map_err(|e| e.to_string())?;
        Ok(tags)
    }))
    .unwrap_or_else(|_| Err("the encoder failed".into()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Whether `a` (which may not exist yet) is the file at `b`.
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| -> Option<PathBuf> {
        std::fs::canonicalize(p).ok().or_else(|| Some(std::fs::canonicalize(p.parent()?).ok()?.join(p.file_name()?)))
    };
    canon(a).is_some_and(|a| canon(b) == Some(a))
}

fn encode(track: &Track, edit: &Edit, choice: Choice, path: &Path) -> Result<(), String> {
    let format = track.format.get().ok_or("the track has no audio")?;
    let (channels, rate) = (format.channels, format.rate);
    let err = |e: hound::Error| e.to_string();
    let mut ok = true;
    match choice.kind {
        Kind::Wav => {
            let float = choice.wav_bits == 32;
            let spec = hound::WavSpec {
                channels: channels as u16,
                sample_rate: rate,
                bits_per_sample: choice.wav_bits,
                sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
            };
            let mut w = hound::WavWriter::create(path, spec).map_err(err)?;
            let scale = ((1u32 << (choice.wav_bits.min(24) - 1)) - 1) as f32;
            let mut result = Ok(());
            ok &= edit::read(track, edit, 0, edit.len(), |block| {
                for &x in block {
                    // shortcut: no dither when going down to 16/24 bits; add TPDF dither if quiet fades sound grainy.
                    let r = if float { w.write_sample(x) } else { w.write_sample((x.clamp(-1.0, 1.0) * scale).round() as i32) };
                    if result.is_ok() {
                        result = r;
                    }
                }
            });
            result.map_err(err)?;
            w.finalize().map_err(err)?;
        }
    }
    if ok { Ok(()) } else { Err("part of the track could not be read".into()) }
}

/// Copies the original's main tag (cover included where the format holds one) into `to`. True
/// when there was nothing to copy, too.
fn copy_tags(from: &Path, to: &Path, kind: Kind) -> bool {
    use lofty::file::TaggedFileExt;
    use lofty::tag::TagExt;
    let Ok(file) = lofty::read_from_path(from) else { return false };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else { return true };
    let mut tag = tag.clone();
    tag.re_map(kind.file_type().primary_tag_type());
    tag.save_to_path(to, lofty::config::WriteOptions::default()).is_ok()
}

/// Decodes the written file: it must have the track's rate and channels and the edit's length,
/// give or take 0.1 s (lossy encoders pad the end).
fn verify(path: &Path, track: &Track, edit: &Edit) -> Result<(), String> {
    let want = track.format.get().copied().ok_or("the track has no audio")?;
    let (mut frames, mut got) = (0u64, None);
    crate::track::decode(path, |_, _, _| {}, |samples, channels, rate, _| {
        got = Some((rate, channels));
        frames += (samples.len() / channels) as u64;
        Ok(())
    })
    .map_err(|e| format!("the saved file doesn't read back ({e})"))?;
    if got.is_some_and(|(r, c)| r != want.rate || c != want.channels) || frames.abs_diff(edit.len()) > want.rate as u64 / 10 {
        return Err(format!("the saved file reads back wrong ({frames} frames, expected {})", edit.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A fresh, empty folder of its own for each test, outside the repo.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sine(rate: u32, channels: usize, frames: usize) -> Arc<Track> {
        Track::from_samples(rate, channels, (0..frames * channels).map(|i| ((i / channels) as f32 * 0.05).sin() * 0.5).collect())
    }

    fn read_back(path: &Path) -> (Vec<f32>, usize, u32) {
        let (mut all, mut fmt) = (Vec::new(), (0, 0));
        crate::track::decode(path, |_, _, _| {}, |s, c, r, _| {
            all.extend_from_slice(s);
            fmt = (c, r);
            Ok(())
        })
        .unwrap();
        (all, fmt.0, fmt.1)
    }

    #[test]
    fn wav_round_trips_at_every_depth() {
        let dir = scratch("wav");
        let track = sine(44100, 2, 30_000);
        let edit = Edit::whole(30_000).cut(1000, 2000);
        let mut want = Vec::new();
        edit::read(&track, &edit, 0, edit.len(), |b| want.extend_from_slice(b));
        for (bits, tolerance) in [(16, 1.0 / 32767.0), (24, 1.0 / 8_388_607.0), (32, 0.0)] {
            let path = dir.join(format!("out{bits}.wav"));
            // Ok(false): a made-up track has no original to copy tags from.
            assert_eq!(save(&track, &edit, Choice { kind: Kind::Wav, wav_bits: bits }, &path), Ok(false));
            let (got, channels, rate) = read_back(&path);
            assert_eq!((channels, rate, got.len()), (2, 44100, want.len()));
            let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(worst <= tolerance, "{bits}-bit off by {worst}");
        }
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    }

    #[test]
    fn refuses_to_write_over_the_original() {
        let dir = scratch("original");
        let path = dir.join("song.wav");
        std::fs::write(&path, b"not really audio").unwrap();
        let mut t = Arc::into_inner(sine(44100, 1, 100)).unwrap();
        t.path = path.clone();
        // Through another spelling of the same path, too.
        let other = dir.join(".").join("song.wav");
        assert!(save(&t, &Edit::whole(100), Choice::default(), &other).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not really audio");
    }

    #[test]
    fn keeps_the_tags_of_a_copy_of_an_album_track() {
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;
        let album = Path::new(env!("CARGO_MANIFEST_DIR")).join("1954-Louis Armstrong Plays W.C.Handy");
        let Some(mp3) = std::fs::read_dir(&album).ok().and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "mp3"))) else {
            eprintln!("skipped: the test album isn't here");
            return;
        };
        // Work on a copy: the album folder is never written to.
        let dir = scratch("tags");
        let copy = dir.join("song.mp3");
        std::fs::copy(&mp3, &copy).unwrap();
        let track = Track::load(copy.clone(), || {});
        while !track.done() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The album has only ID3v1 tags, so this also checks the fallback to the first tag.
        let read = |p: &Path| {
            let f = lofty::read_from_path(p).unwrap();
            f.primary_tag().or_else(|| f.first_tag()).and_then(|t| t.title().map(|s| s.into_owned()))
        };
        let title = read(&copy);
        let out = dir.join("out.wav");
        // Five seconds are enough to check the tags.
        let rate = track.format.get().unwrap().rate as u64;
        assert_eq!(save(&track, &Edit::whole(track.len()).crop(0, 5 * rate), Choice::default(), &out), Ok(true));
        assert_eq!(read(&out), title);
        assert!(title.is_some());
    }
}
