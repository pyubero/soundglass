//! A track decoded fully into memory on a background thread, readable while it fills in.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag, StandardVisualKey};

/// Frames per chunk: about 1.5 s at 44.1 kHz. The player reads whole chunks without copying the list.
pub const CHUNK: usize = 65536;

/// Frames per waveform peak: a 5-minute song gives ~13 000 peaks, more than a window is wide.
pub const PEAK: usize = 1024;

/// Sample rate and channel count, known once the first packet is decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub rate: u32,
    pub channels: usize,
}

/// What the file's tags say, each `None` when missing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// What Track Info shows besides the tags.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub container: String,
    pub codec: String,
    pub bits: Option<u32>,
    /// Every tag as the file has it, key and value.
    pub raw: Vec<(String, String)>,
}

/// Integrated loudness (LUFS) and sample peak (dBFS), measured once decoding is done.
#[derive(Clone, Copy, Debug)]
pub struct Loudness {
    pub lufs: f64,
    pub peak_db: f64,
}

/// Largest cover side kept, in pixels; bigger embedded pictures are shrunk on load.
const COVER_MAX: u32 = 512;

// shortcut: whole track in memory (~1.4 GB per hour at 48 kHz stereo); a RAM limit in Settings comes later (user's choice).
pub struct Track {
    pub path: PathBuf,
    pub format: OnceLock<Format>,
    /// Length the container reports, before decoding finishes.
    pub expected: OnceLock<u64>,
    pub tags: OnceLock<Tags>,
    pub info: OnceLock<Info>,
    pub loudness: OnceLock<Loudness>,
    /// The embedded front cover (or first picture), decoded off the UI thread.
    pub cover: OnceLock<eframe::egui::ColorImage>,
    /// Interleaved samples, `CHUNK` frames each except the last.
    chunks: Mutex<Vec<Arc<[f32]>>>,
    /// Frames in `chunks`.
    decoded: AtomicU64,
    /// Lowest and highest sample of every `PEAK` frames, all channels together, for the waveform.
    pub peaks: Mutex<Vec<(f32, f32)>>,
    done: AtomicBool,
    pub error: Mutex<Option<String>>,
}

impl Track {
    /// Starts decoding `path` on its own thread; `notify` runs whenever more audio is ready.
    pub fn load(path: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Arc<Track> {
        let notify = Arc::new(notify);
        let track = Arc::new(Track::empty(path));
        let t = track.clone();
        std::thread::spawn(move || {
            let mut cur: Vec<f32> = Vec::new();
            let (mut peak, mut peak_frames) = ((f32::MAX, f32::MIN), 0);
            let on_meta = |tags, info, picture: Option<Box<[u8]>>| {
                let _ = t.tags.set(tags);
                let _ = t.info.set(info);
                // On its own thread: a large JPEG must not hold up the first sound.
                if let Some(data) = picture {
                    let (t, notify) = (t.clone(), notify.clone());
                    std::thread::spawn(move || {
                        if let Ok(img) = image::load_from_memory(&data) {
                            let rgba = img.thumbnail(COVER_MAX, COVER_MAX).to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let _ = t.cover.set(eframe::egui::ColorImage::from_rgba_unmultiplied(size, &rgba));
                            notify();
                        }
                    });
                }
            };
            let result = decode(&t.path.clone(), on_meta, |samples, channels, rate, expected| {
                if let Some(n) = expected {
                    let _ = t.expected.set(n);
                }
                let format = *t.format.get_or_init(|| Format { rate, channels });
                // shortcut: a rate change mid-file (chained Ogg) plays at the first rate; resample per segment if it matters.
                if format.channels != channels {
                    return Err("the channel count changes mid-file".into());
                }
                // Only the decoder holds the track: nobody will play it, so stop.
                if Arc::strong_count(&t) == 1 {
                    return Err(String::new());
                }
                for frame in samples.chunks_exact(channels) {
                    cur.extend_from_slice(frame);
                    for &s in frame {
                        peak = (peak.0.min(s), peak.1.max(s));
                    }
                    peak_frames += 1;
                    if peak_frames == PEAK {
                        t.peaks.lock().unwrap().push(std::mem::replace(&mut peak, (f32::MAX, f32::MIN)));
                        peak_frames = 0;
                    }
                    if cur.len() == CHUNK * channels {
                        t.push(std::mem::take(&mut cur), channels);
                        notify();
                    }
                }
                Ok(())
            });
            if peak_frames > 0 {
                t.peaks.lock().unwrap().push(peak);
            }
            if !cur.is_empty() {
                let channels = t.format.get().map_or(1, |f| f.channels);
                t.push(cur, channels);
            }
            if let Err(e) = result.and_then(|_| t.format.get().map(|_| ()).ok_or_else(|| "the file has no audio".to_string()))
                && !e.is_empty()
            {
                eprintln!("soundglass: {}: {e}", t.path.display());
                *t.error.lock().unwrap() = Some(e);
            }
            t.done.store(true, Relaxed);
            notify();
            if let Some(format) = t.format.get() {
                let mut meter = crate::loudness::Meter::new(format.rate, format.channels);
                let chunks = t.chunks.lock().unwrap().clone();
                for chunk in chunks {
                    meter.push(&chunk);
                }
                let peak = t.peaks.lock().unwrap().iter().fold(0f32, |m, &(lo, hi)| m.max(-lo).max(hi));
                let _ = t.loudness.set(Loudness { lufs: meter.integrated(), peak_db: 20.0 * (peak as f64).log10() });
                notify();
            }
        });
        track
    }

    fn empty(path: PathBuf) -> Track {
        Track {
            path,
            format: OnceLock::new(),
            expected: OnceLock::new(),
            tags: OnceLock::new(),
            info: OnceLock::new(),
            loudness: OnceLock::new(),
            cover: OnceLock::new(),
            chunks: Mutex::new(Vec::new()),
            decoded: AtomicU64::new(0),
            peaks: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
        }
    }

    fn push(&self, samples: Vec<f32>, channels: usize) {
        let frames = (samples.len() / channels) as u64;
        self.chunks.lock().unwrap().push(samples.into());
        self.decoded.fetch_add(frames, Relaxed);
    }

    /// Frames ready to play.
    pub fn decoded(&self) -> u64 {
        self.decoded.load(Relaxed)
    }

    pub fn done(&self) -> bool {
        self.done.load(Relaxed)
    }

    /// The length in frames: exact once decoded, the container's figure before that.
    pub fn len(&self) -> u64 {
        if self.done() { self.decoded() } else { self.expected.get().copied().unwrap_or(0).max(self.decoded()) }
    }

    /// The chunk holding frame `pos` and the frame's offset in it, or `None` while that part is
    /// still decoding or the list is locked. Never blocks, so the audio callback can call it.
    pub fn chunk_at(&self, pos: u64) -> Option<(Arc<[f32]>, usize)> {
        let chunks = self.chunks.try_lock().ok()?;
        let chunk = chunks.get(pos as usize / CHUNK)?.clone();
        let channels = self.format.get()?.channels;
        let offset = pos as usize % CHUNK;
        (offset < chunk.len() / channels).then_some((chunk, offset))
    }

    /// A fully decoded track made from samples in memory (Save As's resampled audio, and tests).
    pub fn from_samples(rate: u32, channels: usize, samples: Vec<f32>) -> Arc<Track> {
        let t = Track::empty(PathBuf::new());
        t.format.set(Format { rate, channels }).unwrap();
        for part in samples.chunks(CHUNK * channels) {
            t.push(part.to_vec(), channels);
        }
        t.done.store(true, Relaxed);
        Arc::new(t)
    }
}

/// Decodes the default audio track packet by packet, handing interleaved f32 samples, the channel
/// count, the sample rate and the container's length to `sink`. Stops at the first error `sink`
/// returns. symphonia's decoders already trim encoder delay and padding, so albums are gapless.
/// `on_meta` gets the tags, the info and the cover's encoded bytes once, before the first samples.
pub fn decode(
    path: &Path,
    on_meta: impl FnOnce(Tags, Info, Option<Box<[u8]>>),
    mut sink: impl FnMut(&[f32], usize, u32, Option<u64>) -> Result<(), String>,
) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("not a supported audio file ({e})"))?;
    let (mut tags, mut picture) = (Tags::default(), None);
    let mut info = Info { container: format.format_info().long_name.to_string(), ..Info::default() };
    if let Some(rev) = format.metadata().skip_to_latest() {
        for tag in &rev.media.tags {
            info.raw.push((tag.raw.key.clone(), tag.raw.value.to_string()));
            // ID3v1 pads its fixed-width fields with spaces or NULs.
            let text = |v: &str| Some(v.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_string()).filter(|t| !t.is_empty());
            match &tag.std {
                Some(StandardTag::TrackTitle(v)) => tags.title = text(v),
                Some(StandardTag::Artist(v)) => tags.artist = text(v),
                Some(StandardTag::Album(v)) => tags.album = text(v),
                _ => {}
            }
        }
        let visuals = &rev.media.visuals;
        let front = visuals.iter().find(|v| v.usage == Some(StandardVisualKey::FrontCover)).or(visuals.first());
        picture = front.map(|v| v.data.clone());
    }
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let (track_id, expected) = (track.id, track.num_frames);
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio track")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| e.to_string())?;
    info.codec = symphonia::default::get_codecs().get_audio_decoder(params.codec).map_or_else(String::new, |d| d.codec.info.long_name.to_string());
    info.bits = params.bits_per_sample;
    on_meta(tags, info, picture);
    let mut samples = Vec::new();
    while let Some(packet) = format.next_packet().map_err(|e| e.to_string())? {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                samples.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut samples);
                sink(&samples, buf.spec().channels().count(), buf.spec().rate(), expected)?;
            }
            // A damaged packet is skipped, as players usually do.
            Err(Error::DecodeError(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// A 16-bit stereo WAV whose samples count up from 0.
    pub fn wav(frames: u32, rate: u32) -> Vec<u8> {
        let data_len = frames * 4;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(2u16.to_le_bytes()); // stereo
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_len.to_le_bytes());
        wav.extend((0..frames * 2).flat_map(|i| (i as i16).to_le_bytes()));
        wav
    }

    /// `wav` behind an ID3v2.4 tag carrying title, artist and album, as MP3s have.
    /// (symphonia 0.6.1 reads a WAV's own INFO chunk but then drops it, so that can't be used.)
    fn tagged_wav() -> Vec<u8> {
        let mut frames = Vec::new();
        for (id, text) in [(b"TIT2", "Long Gone   "), (b"TPE1", "Some Artist"), (b"TALB", "An Album")] {
            let body: Vec<u8> = [3u8].into_iter().chain(text.bytes()).collect(); // 3 = UTF-8
            frames.extend(id);
            frames.extend(syncsafe(body.len()));
            frames.extend([0, 0]);
            frames.extend(body);
        }
        let mut tag = b"ID3\x04\x00\x00".to_vec();
        tag.extend(syncsafe(frames.len()));
        tag.extend(frames);
        tag.extend(wav(100, 8000));
        tag
    }

    /// ID3v2 sizes: 7 bits per byte.
    fn syncsafe(n: usize) -> [u8; 4] {
        [(n >> 21) as u8 & 0x7f, (n >> 14) as u8 & 0x7f, (n >> 7) as u8 & 0x7f, n as u8 & 0x7f]
    }

    #[test]
    fn reads_tags() {
        let mut got = None;
        decode(&temp_file("tagged.wav", &tagged_wav()), |tags, info, _| got = Some((tags, info.raw.len())), |_, _, _, _| Ok(())).unwrap();
        let want = Tags { title: Some("Long Gone".into()), artist: Some("Some Artist".into()), album: Some("An Album".into()) };
        assert_eq!(got, Some((want, 3)));
    }

    /// Until the decode thread is finished, loudness pass included: it briefly locks the
    /// chunk list, and `chunk_at` (try_lock, for the audio callback) would see None meanwhile.
    fn wait(track: &Track) {
        while !(track.done() && (track.format.get().is_none() || track.loudness.get().is_some())) {
            std::thread::yield_now();
        }
    }

    #[test]
    fn decodes_a_wav() {
        let mut total = 0;
        decode(&temp_file("tone.wav", &wav(1000, 22050)), |tags, info, cover| {
            assert_eq!((tags, cover), (Tags::default(), None));
            assert_eq!((info.bits, info.raw.len()), (Some(16), 0));
            assert!(!info.codec.is_empty() && !info.container.is_empty(), "{info:?}");
        }, |samples, channels, rate, _| {
            assert_eq!((channels, rate), (2, 22050));
            total += samples.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(total, 2000);
    }

    #[test]
    fn loads_into_chunks() {
        let frames = CHUNK as u32 + 500;
        let track = Track::load(temp_file("long.wav", &wav(frames, 44100)), || {});
        wait(&track);
        assert_eq!(track.decoded(), frames as u64);
        assert_eq!(track.format.get(), Some(&Format { rate: 44100, channels: 2 }));
        // Frame CHUNK + 10 is the 11th frame of the second chunk; its left sample counts up from 0.
        let (chunk, offset) = track.chunk_at(CHUNK as u64 + 10).unwrap();
        assert_eq!(offset, 10);
        assert_eq!(chunk[offset * 2], ((CHUNK as i32 + 10) * 2) as i16 as f32 / 32768.0);
        assert!(track.chunk_at(frames as u64).is_none());
        // 65 536 + 500 frames: 64 full peaks and a partial one. Samples count up, so each peak
        // runs from its block's first left sample to its last right sample.
        let peaks = track.peaks.lock().unwrap();
        assert_eq!(peaks.len(), CHUNK / PEAK + 1);
        assert_eq!(peaks[1], (2048.0 / 32768.0, 4095.0 / 32768.0));
        assert!(track.error.lock().unwrap().is_none());
    }

    #[test]
    fn junk_is_an_error_not_a_panic() {
        let path = temp_file("junk.mp3", &[0x5a; 4096]);
        assert!(decode(&path, |_, _, _| {}, |_, _, _, _| Ok(())).is_err());
        let track = Track::load(path, || {});
        wait(&track);
        assert!(track.error.lock().unwrap().is_some());
    }
}
