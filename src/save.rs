//! Save As: the edited track encoded into a new file with the original's tags, never over the
//! original, written to a temp file, checked, then renamed into place.

use crate::edit::{self, Edit};
use crate::track::Track;
use crate::tags::TagEdit;
use crate::wsola::{Speed, Wsola};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Wav,
    Flac,
    Mp3,
    Vorbis,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Wav, Kind::Flac, Kind::Mp3, Kind::Vorbis];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Wav => "WAV",
            Kind::Flac => "FLAC",
            Kind::Mp3 => "MP3",
            Kind::Vorbis => "Ogg Vorbis",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Kind::Wav => "wav",
            Kind::Flac => "flac",
            Kind::Mp3 => "mp3",
            Kind::Vorbis => "ogg",
        }
    }

    fn file_type(self) -> lofty::file::FileType {
        use lofty::file::FileType;
        match self {
            Kind::Wav => FileType::Wav,
            Kind::Flac => FileType::Flac,
            Kind::Mp3 => FileType::Mpeg,
            Kind::Vorbis => FileType::Vorbis,
        }
    }
}

/// The sample rates LAME encodes.
const MP3_RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

/// The format and, for each format, its quality, so switching formats keeps each one's choice.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub kind: Kind,
    /// 16, 24, or 32 for 32-bit float.
    pub wav_bits: u16,
    /// 16 or 24.
    pub flac_bits: u16,
    /// Constant bitrate, 128 to 320.
    pub mp3_kbps: u32,
    /// 0 to 10, as oggenc counts.
    pub vorbis_quality: u8,
    /// The sample rate to save at; `None` keeps the track's.
    pub rate: Option<u32>,
    /// Playback's speed and pitch, applied to the audio; `None` leaves them out.
    pub speed: Option<Speed>,
}

impl Default for Choice {
    fn default() -> Choice {
        Choice { kind: Kind::Flac, wav_bits: 16, flac_bits: 16, mp3_kbps: 256, vorbis_quality: 6, rate: None, speed: None }
    }
}

/// The name Save As suggests: `<stem>-edited.<ext>`, as Save Copy names its first copy.
pub fn suggested_name(original: &Path, kind: Kind) -> String {
    format!("{}-edited.{}", original.file_stem().unwrap_or_default().to_string_lossy(), kind.ext())
}

/// Claims the first free name for a copy next to `source` (`song-edited.ext`, then
/// `song-edited-2.ext`…) by creating it empty, so nothing is overwritten and no two saves pick
/// the same name. From lightview's `save::reserve_copy`.
pub fn reserve_copy(source: &Path, ext: &str) -> Result<PathBuf, String> {
    reserve_named(source, "edited", ext)
}

/// Like `reserve_copy`, named `<stem>-<what>.<ext>` (`song-denoised.flac`).
pub fn reserve_named(source: &Path, what: &str, ext: &str) -> Result<PathBuf, String> {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    for n in 1.. {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let dest = source.with_file_name(format!("{stem}-{what}{suffix}.{ext}"));
        match std::fs::File::options().write(true).create_new(true).open(&dest) {
            Ok(_) => return Ok(dest),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("can't save a copy next to the original ({e})")),
        }
    }
    unreachable!()
}

/// What Save Copy writes: the original's format when soundglass can write it (FLAC otherwise),
/// at its bit depth (`bits`) and sample rate; lossy formats at Save As's `last` quality.
pub fn copy_choice(source: &Path, codec: &str, bits: Option<u32>, last: Choice) -> Choice {
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let kind = match ext.as_str() {
        "wav" => Kind::Wav,
        "flac" => Kind::Flac,
        "mp3" => Kind::Mp3,
        // An .ogg can hold FLAC or Opus too.
        "ogg" | "oga" if codec.to_ascii_lowercase().contains("vorbis") => Kind::Vorbis,
        _ => Kind::Flac,
    };
    let deep = bits.is_some_and(|b| b > 16);
    Choice { kind, wav_bits: if bits == Some(32) { 32 } else if deep { 24 } else { 16 }, flac_bits: if deep { 24 } else { 16 }, rate: None, speed: None, ..last }
}

/// Writes frames `edit` of `track` (fully decoded) to `target` as `choice` says. Ok(false) when
/// the file is saved but the original's tags could not be copied.
/// `draft` holds tag edits not yet written to the original, to put in the copy instead.
pub fn save(track: &Track, edit: &Edit, choice: Choice, target: &Path, draft: Option<&TagEdit>) -> Result<bool, String> {
    if same_file(target, &track.path) {
        return Err("That is the original file; choose another name. The original is never changed.".into());
    }
    let name = target.file_name().ok_or("no file name")?.to_string_lossy();
    // Hidden, next to the target so the rename stays on one disk, with the real extension so it decodes.
    let temp = target.with_file_name(format!(".{name}.soundglass-part.{}", choice.kind.ext()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let own = track.format.get().map_or(0, |f| f.rate);
        let (rate, speed) = (choice.rate.unwrap_or(own), choice.speed.filter(|s| *s != Speed::NORMAL));
        let resampled = if rate != own || speed.is_some() { Some(resample(track, edit, rate, speed)?) } else { None };
        let (audio, edit) = resampled.as_ref().map_or((track, edit), |(t, e)| (&**t, e));
        encode(audio, edit, choice, &temp)?;
        let tags = std::panic::catch_unwind(|| copy_tags(&track.path, &temp, choice.kind, draft)).unwrap_or(false);
        verify(&temp, audio, edit, matches!(choice.kind, Kind::Wav | Kind::Flac))?;
        std::fs::rename(&temp, target).map_err(|e| e.to_string())?;
        Ok(tags)
    }))
    .unwrap_or_else(|_| Err("the encoder failed".into()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The edited track at another sample rate and with `speed` applied, as a track of its own with no edits.
pub fn resample(track: &Track, edit: &Edit, rate: u32, speed: Option<Speed>) -> Result<(Arc<Track>, Edit), String> {
    use rubato::Resampler;
    let format = track.format.get().ok_or("the track has no audio")?;
    let channels = edit.channels(format.channels);
    let mut samples = Vec::with_capacity(edit.len() as usize * channels);
    edit::read(track, edit, 0, edit.len(), |b| samples.extend_from_slice(b));
    let speed = speed.unwrap_or(Speed::NORMAL);
    if speed.stretches() {
        let mut w = Wsola::new(channels, format.rate);
        w.set_tempo(speed.stretch());
        let mut out = vec![0.0; (samples.len() as f64 / speed.stretch()) as usize / channels * channels];
        let mut pos = 0;
        w.process(&mut out, |dst| {
            // Past the end, silence: WSOLA's last window still needs input after it.
            let rest = samples.get(pos..).unwrap_or_default();
            let n = dst.len().min(rest.len());
            dst[..n].copy_from_slice(&rest[..n]);
            dst[n..].fill(0.0);
            pos += dst.len();
            true
        });
        samples = out;
    }
    let frames = samples.len() / channels;
    let err = |e: &dyn std::fmt::Display| format!("resampling: {e}");
    let mut resampler: Box<dyn Resampler<f32>> = if speed.rate() == 1.0 {
        Box::new(rubato::Fft::<f32>::new(format.rate as usize, rate as usize, 1024, channels, rubato::FixedSync::Both).map_err(|e| err(&e))?)
    } else {
        // Played faster by `rate()`: not a ratio of whole numbers, so the sinc resampler.
        let params = rubato::SincInterpolationParameters::new(256, rubato::WindowFunction::Blackman2)
            .oversampling_factor(256)
            .interpolation(rubato::SincInterpolationType::Cubic);
        let ratio = rate as f64 / (format.rate as f64 * speed.rate());
        Box::new(rubato::Async::<f32>::new_sinc(ratio, 1.0, &params, 1024, channels, rubato::FixedAsync::Input).map_err(|e| err(&e))?)
    };
    let input = rubato::audioadapter_buffers::direct::InterleavedSlice::new(&samples, channels, frames).map_err(|e| err(&e))?;
    // process_all trims the resampler's delay, so the length scales exactly.
    let out = resampler.process_all(&input, frames, None).map_err(|e| err(&e))?.take_data();
    let len = (out.len() / channels) as u64;
    Ok((Track::from_samples(rate, channels, out), Edit::whole(len)))
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
    let (channels, rate) = (edit.channels(format.channels), format.rate);
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
        Kind::Flac => {
            let bits = choice.flac_bits;
            let scale = ((1u32 << (bits - 1)) - 1) as f32;
            let mut samples: Vec<i32> = Vec::with_capacity(edit.len() as usize * channels);
            ok &= edit::read(track, edit, 0, edit.len(), |block| samples.extend(block.iter().map(|&x| (x.clamp(-1.0, 1.0) * scale).round() as i32)));
            let dbg = |e: &dyn std::fmt::Debug| format!("FLAC: {e:?}");
            use flacenc::error::Verify;
            let mut config = flacenc::config::Encoder::default();
            // flacenc 0.5.1's fixed predictors made a 33 MB frame of one 4096-frame stereo block
            // (a Demucs stem), which symphonia then skipped; LPC alone codes that block in 20 KB.
            config.subframe_coding.use_fixed = false;
            let config = config.into_verified().map_err(|e| dbg(&e))?;
            let source = flacenc::source::MemSource::from_samples(&samples, channels, bits as usize, rate as usize);
            let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).map_err(|e| dbg(&e))?;
            let mut sink = flacenc::bitsink::ByteSink::new();
            flacenc::component::BitRepr::write(&stream, &mut sink).map_err(|e| dbg(&e))?;
            let mut bytes = sink.into_inner();
            // STREAMINFO's minimum block size must leave out the last, shorter block (the FLAC spec);
            // flacenc counts it, and symphonia then reads the stream as variable-block and fails.
            if bytes.len() >= 12 && &bytes[..4] == b"fLaC" {
                bytes.copy_within(10..12, 8);
            }
            std::fs::write(path, &bytes).map_err(|e| e.to_string())?;
        }
        Kind::Mp3 => {
            use mp3lame_encoder::{Builder, FlushGap, InterleavedPcm, MonoPcm};
            if channels > 2 {
                return Err("MP3 holds mono or stereo only; save as FLAC or WAV instead".into());
            }
            if !MP3_RATES.contains(&rate) {
                return Err(format!("MP3 can't hold {:.1} kHz audio; save as FLAC or WAV instead", rate as f64 / 1000.0));
            }
            let dbg = |e: &dyn std::fmt::Debug| format!("MP3: {e:?}");
            let mut b = Builder::new().ok_or("MP3: LAME failed to start")?;
            b.set_num_channels(channels as u8).map_err(|e| dbg(&e))?;
            b.set_sample_rate(rate).map_err(|e| dbg(&e))?;
            b.set_brate(mp3_bitrate(choice.mp3_kbps)).map_err(|e| dbg(&e))?;
            b.set_quality(mp3lame_encoder::Quality::Best).map_err(|e| dbg(&e))?;
            let mut enc = b.build().map_err(|e| dbg(&e))?;
            let mut out = Vec::new();
            let mut result = Ok(0);
            // Blocks of at most 4096 frames, to keep LAME's output buffer small.
            ok &= edit::read(track, edit, 0, edit.len(), |block| {
                for part in block.chunks(4096 * channels) {
                    out.reserve(mp3lame_encoder::max_required_buffer_size(part.len()));
                    let r = if channels == 1 { enc.encode_to_vec(MonoPcm(part), &mut out) } else { enc.encode_to_vec(InterleavedPcm(part), &mut out) };
                    if result.is_ok() {
                        result = r;
                    }
                }
            });
            result.map_err(|e| dbg(&e))?;
            out.reserve(7200);
            enc.flush_to_vec::<FlushGap>(&mut out).map_err(|e| dbg(&e))?;
            // The LAME tag frame goes over the empty frame LAME put first (after any ID3v2 tag): it
            // carries the exact length, so players (symphonia too) trim the encoder's delay and padding.
            // It is written into spare capacity, so room is reserved first.
            let mut tag = Vec::with_capacity(enc.lame_tag_size());
            let at = enc.id3v2_tag_size();
            if enc.lame_tag_encode_to_vec(&mut tag).is_some() && at + tag.len() <= out.len() {
                out[at..at + tag.len()].copy_from_slice(&tag);
            }
            std::fs::write(path, &out).map_err(|e| e.to_string())?;
        }
        Kind::Vorbis => {
            use std::num::{NonZeroU8, NonZeroU32};
            let dbg = |e: vorbis_rs::VorbisError| format!("Ogg Vorbis: {e}");
            let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
            let (r, c) = (NonZeroU32::new(rate).ok_or("no sample rate")?, NonZeroU8::new(channels as u8).ok_or("no channels")?);
            let mut b = vorbis_rs::VorbisEncoderBuilder::new(r, c, file).map_err(dbg)?;
            b.bitrate_management_strategy(vorbis_rs::VorbisBitrateManagementStrategy::QualityVbr { target_quality: choice.vorbis_quality as f32 / 10.0 });
            let mut enc = b.build().map_err(dbg)?;
            let mut planar = vec![Vec::new(); channels];
            let mut result = Ok(());
            ok &= edit::read(track, edit, 0, edit.len(), |block| {
                // libvorbis works best with blocks of about a thousand frames.
                for part in block.chunks(1024 * channels) {
                    for (ch, plane) in planar.iter_mut().enumerate() {
                        plane.clear();
                        plane.extend(part.iter().skip(ch).step_by(channels));
                    }
                    if result.is_ok() {
                        result = enc.encode_audio_block(&planar);
                    }
                }
            });
            result.map_err(dbg)?;
            use std::io::Write;
            enc.finish().map_err(dbg)?.flush().map_err(|e| e.to_string())?;
        }
    }
    if ok { Ok(()) } else { Err("part of the track could not be read".into()) }
}

fn mp3_bitrate(kbps: u32) -> mp3lame_encoder::Bitrate {
    use mp3lame_encoder::Bitrate::*;
    match kbps {
        ..=128 => Kbps128,
        129..=192 => Kbps192,
        193..=256 => Kbps256,
        _ => Kbps320,
    }
}

/// Copies the original's main tag (cover included where the format holds one) into `to`. True
/// when there was nothing to copy, too.
fn copy_tags(from: &Path, to: &Path, kind: Kind, draft: Option<&TagEdit>) -> bool {
    use lofty::file::TaggedFileExt;
    use lofty::tag::TagExt;
    let file = lofty::read_from_path(from).ok();
    let found = file.as_ref().and_then(|f| f.primary_tag().or_else(|| f.first_tag())).cloned();
    let kind = kind.file_type().primary_tag_type();
    let mut tag = match (found, draft) {
        (Some(tag), _) => tag,
        (None, Some(_)) => lofty::tag::Tag::new(kind),
        (None, None) => return file.is_some(),
    };
    tag.re_map(kind);
    if let Some(d) = draft {
        crate::tags::apply(d, &mut tag);
    }
    tag.save_to_path(to, lofty::config::WriteOptions::default()).is_ok()
}

/// Decodes the written file: it must have the track's rate, the edit's channels and its length,
/// give or take 0.1 s (lossy encoders pad the end).
/// `exact`: a lossless format must read back frame for frame; a lossy one within 0.1 s (encoder delay).
fn verify(path: &Path, track: &Track, edit: &Edit, exact: bool) -> Result<(), String> {
    let want = track.format.get().copied().ok_or("the track has no audio")?;
    let (mut frames, mut got) = (0u64, None);
    crate::track::decode(path, |_, _, _| {}, |samples, channels, rate, _| {
        got = Some((rate, channels));
        frames += (samples.len() / channels) as u64;
        Ok(())
    })
    .map_err(|e| format!("the saved file doesn't read back ({e})"))?;
    if got.is_some_and(|(r, c)| r != want.rate || c != edit.channels(want.channels)) || frames.abs_diff(edit.len()) > if exact { 0 } else { want.rate as u64 / 10 } {
        return Err(format!("the saved file reads back wrong ({frames} frames, expected {})", edit.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(save(&track, &edit, Choice { kind: Kind::Wav, wav_bits: bits, ..Choice::default() }, &path, None), Ok(false));
            let (got, channels, rate) = read_back(&path);
            assert_eq!((channels, rate, got.len()), (2, 44100, want.len()));
            let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(worst <= tolerance, "{bits}-bit off by {worst}");
        }
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    }

    #[test]
    fn flac_is_exact_and_lossy_formats_keep_the_length() {
        let dir = scratch("formats");
        for channels in [1, 2] {
            let track = sine(44100, channels, 100_000);
            let edit = Edit::whole(100_000).silence(0, 5000);
            let mut want = Vec::new();
            edit::read(&track, &edit, 0, edit.len(), |b| want.extend_from_slice(b));
            for bits in [16, 24] {
                let path = dir.join(format!("{channels}-{bits}.flac"));
                save(&track, &edit, Choice { kind: Kind::Flac, flac_bits: bits, ..Choice::default() }, &path, None).unwrap();
                let (got, ..) = read_back(&path);
                let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
                assert_eq!(got.len(), want.len());
                assert!(worst <= 1.0 / ((1u32 << (bits - 1)) - 1) as f32, "{bits}-bit FLAC off by {worst}");
            }
            for kind in [Kind::Mp3, Kind::Vorbis] {
                let path = dir.join(format!("{channels}.{}", kind.ext()));
                save(&track, &edit, Choice { kind, ..Choice::default() }, &path, None).unwrap();
                let (got, c, r) = read_back(&path);
                assert_eq!((c, r), (channels, 44100));
                // The length is exact or within a few frames: the encoder's delay is trimmed on decode.
                let frames = (got.len() / channels) as i64;
                assert!((frames - edit.len() as i64).abs() <= 2, "{} {channels} ch: {frames} frames, want {}", kind.name(), edit.len());
            }
        }
    }

    #[test]
    fn saves_the_channel_mix() {
        let dir = scratch("mix");
        let track = sine(44100, 2, 10_000);
        let edit = Edit::whole(10_000).mono(2);
        let mut want = Vec::new();
        edit::read(&track, &edit, 0, edit.len(), |b| want.extend_from_slice(b));
        let path = dir.join("mono.flac");
        save(&track, &edit, Choice { flac_bits: 24, ..Choice::default() }, &path, None).unwrap();
        let (got, c, _) = read_back(&path);
        assert_eq!((c, got.len()), (1, want.len()));
        let path = dir.join("mono.mp3");
        save(&track, &edit, Choice { kind: Kind::Mp3, ..Choice::default() }, &path, None).unwrap();
        assert_eq!(read_back(&path).1, 1);
    }

    #[test]
    fn saves_with_speed_and_pitch() {
        let dir = scratch("speed");
        let track = sine(44100, 2, 44100);
        let path = dir.join("fast.wav");
        let speed = Speed { tape: 1.0, tempo: 2.0, semitones: 2.0 };
        save(&track, &Edit::whole(44100), Choice { kind: Kind::Wav, speed: Some(speed), ..Choice::default() }, &path, None).unwrap();
        let (got, c, r) = read_back(&path);
        assert_eq!((c, r), (2, 44100));
        assert!((got.len() / 2).abs_diff(22050) <= 50, "{} frames", got.len() / 2);
        // Tape speed changes the length too.
        let tape = dir.join("tape.wav");
        save(&track, &Edit::whole(44100), Choice { kind: Kind::Wav, speed: Some(Speed { tape: 1.5, ..Speed::NORMAL }), ..Choice::default() }, &tape, None).unwrap();
        assert!((read_back(&tape).0.len() / 2).abs_diff(29400) <= 50);
    }

    #[test]
    fn copies_get_the_unsaved_tag_edits() {
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;
        let dir = scratch("draft");
        let track = sine(44100, 2, 4410);
        let path = dir.join("draft.flac");
        let draft = TagEdit { title: "Draft title".into(), ..TagEdit::default() };
        save(&track, &Edit::whole(4410), Choice::default(), &path, Some(&draft)).unwrap();
        let file = lofty::read_from_path(&path).unwrap();
        assert_eq!(file.primary_tag().unwrap().title().as_deref(), Some("Draft title"));
    }

    #[test]
    fn saves_at_another_sample_rate() {
        let dir = scratch("rate");
        let track = sine(44100, 2, 44100);
        let edit = Edit::whole(44100).cut(0, 4410).mono(2);
        let path = dir.join("48k.wav");
        save(&track, &edit, Choice { kind: Kind::Wav, rate: Some(48000), ..Choice::default() }, &path, None).unwrap();
        let (got, c, r) = read_back(&path);
        assert_eq!((c, r), (1, 48000));
        assert!(got.len().abs_diff(43200) <= 2, "{} frames", got.len());
        // The sine survives: the middle peaks near its 0.5 amplitude.
        let peak = got[1000..40000].iter().fold(0f32, |p, x| p.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
        // Save Copy never changes the rate.
        assert_eq!(copy_choice(&path, "", None, Choice { rate: Some(22050), ..Choice::default() }).rate, None);
    }

    #[test]
    fn mp3_refuses_rates_lame_has_not() {
        let dir = scratch("mp3rate");
        let track = sine(96000, 2, 1000);
        let err = save(&track, &Edit::whole(1000), Choice { kind: Kind::Mp3, ..Choice::default() }, &dir.join("x.mp3"), None).unwrap_err();
        assert!(err.contains("96.0 kHz"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no temp file left");
    }

    #[test]
    fn copies_get_free_names_and_never_overwrite() {
        let dir = scratch("names");
        let song = dir.join("song.mp3");
        std::fs::write(&song, b"original").unwrap();
        std::fs::write(dir.join("song-edited-2.flac"), b"someone's file").unwrap();
        assert_eq!(reserve_copy(&song, "flac").unwrap(), dir.join("song-edited.flac"));
        assert_eq!(reserve_copy(&song, "flac").unwrap(), dir.join("song-edited-3.flac"));
        assert_eq!(std::fs::read(dir.join("song-edited-2.flac")).unwrap(), b"someone's file");
        assert_eq!(std::fs::read(&song).unwrap(), b"original");
    }

    #[test]
    fn copy_keeps_the_format_when_it_can() {
        let last = Choice { mp3_kbps: 320, ..Choice::default() };
        let c = copy_choice(Path::new("a.mp3"), "MPEG Audio Layer 3", None, last);
        assert_eq!((c.kind, c.mp3_kbps), (Kind::Mp3, 320));
        assert_eq!(copy_choice(Path::new("a.M4A"), "AAC", Some(16), last).kind, Kind::Flac);
        assert_eq!(copy_choice(Path::new("a.ogg"), "Opus", None, last).kind, Kind::Flac);
        assert_eq!(copy_choice(Path::new("a.ogg"), "Vorbis", None, last).kind, Kind::Vorbis);
        let c = copy_choice(Path::new("a.flac"), "FLAC", Some(24), last);
        assert_eq!((c.kind, c.flac_bits), (Kind::Flac, 24));
        assert_eq!(copy_choice(Path::new("a.wav"), "PCM", Some(32), last).wav_bits, 32);
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
        assert!(save(&t, &Edit::whole(100), Choice::default(), &other, None).is_err());
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
        let out = dir.join("out.flac");
        // Five seconds are enough to check the tags.
        let rate = track.format.get().unwrap().rate as u64;
        assert_eq!(save(&track, &Edit::whole(track.len()).crop(0, 5 * rate), Choice::default(), &out, None), Ok(true));
        assert_eq!(read(&out), title);
        assert!(title.is_some());
    }
}
