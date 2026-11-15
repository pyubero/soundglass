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
use crate::i18n::{tr, trf};

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

/// Lowers the calling thread's priority, so the window and the sound come first: for work that
/// can wait (loudness, decoding ahead, analyses, saving, AI tools).
pub fn background() {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        }
        // On Linux, PRIO_PROCESS (0) with who = 0 sets the calling thread's nice value only (setpriority(2), BUGS).
        unsafe { setpriority(0, 0, 10) };
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

// shortcut: whole track in memory (~1.4 GB per hour at 48 kHz stereo); a RAM limit in Settings comes later (user's choice).
pub struct Track {
    pub path: PathBuf,
    pub format: OnceLock<Format>,
    /// Length the container reports, before decoding finishes.
    pub expected: OnceLock<u64>,
    pub tags: OnceLock<Tags>,
    pub info: OnceLock<Info>,
    pub loudness: OnceLock<Loudness>,
    /// Tempo and key, estimated by `analyse` (Practice mode only).
    pub tempo: OnceLock<crate::tempo::Estimate>,
    /// Chords over time, in the original's frames, found with the tempo.
    pub chords: OnceLock<Arc<[crate::chords::Span]>>,
    /// Beats in the original's frames, from Local AI → Find Beats.
    pub beats: Mutex<Option<Arc<crate::ai::Beats>>>,
    /// The embedded front cover (or first picture), decoded off the UI thread.
    pub cover: OnceLock<eframe::egui::ColorImage>,
    /// Interleaved samples, `CHUNK` frames each except the last.
    chunks: Mutex<Vec<Arc<[f32]>>>,
    /// Frames in `chunks`.
    decoded: AtomicU64,
    /// Lowest and highest sample of every `PEAK` frames, all channels together, for the waveform.
    pub peaks: Mutex<Vec<(f32, f32)>>,
    done: AtomicBool,
    /// `analyse` has started (or the results came over with `relabel`).
    analysing: AtomicBool,
    pub error: Mutex<Option<String>>,
}

impl Track {
    /// Starts decoding `path` on its own thread; `notify` runs whenever more audio is ready.
    pub fn load(path: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Arc<Track> {
        Track::start(path, notify, false)
    }

    /// `load` for the file after this one: decoded at low priority, since nothing plays it yet.
    // shortcut: if Next moves to it before it's decoded, it finishes at low priority; raise it if that ever stutters.
    pub fn load_ahead(path: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Arc<Track> {
        Track::start(path, notify, true)
    }

    fn start(path: PathBuf, notify: impl Fn() + Send + Sync + 'static, ahead: bool) -> Arc<Track> {
        let notify = Arc::new(notify);
        let track = Arc::new(Track::empty(path));
        let t = track.clone();
        std::thread::spawn(move || {
            if ahead {
                background();
            }
            let mut cur: Vec<f32> = Vec::new();
            let (mut peak, mut peak_frames) = ((f32::MAX, f32::MIN), 0);
            let on_meta = |tags, info, picture| t.set_meta(tags, info, picture, notify.clone());
            // A hostile file that panics the decoder must still end as an error, not "Decoding…" forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&t.path.clone(), on_meta, |samples, channels, rate, expected| {
                if let Some(n) = expected {
                    let _ = t.expected.set(n);
                }
                let format = *t.format.get_or_init(|| Format { rate, channels });
                // shortcut: a rate change mid-file (chained Ogg) plays at the first rate; resample per segment if it matters.
                if format.channels != channels {
                    return Err(tr("the channel count changes mid-file").into());
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
            })))
            .unwrap_or_else(|_| Err(tr("this file can't be decoded").into()));
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
            background();
            if let Some(format) = t.format.get() {
                let mut meter = crate::loudness::Meter::new(format.rate, format.channels);
                let chunks = t.chunks.lock().unwrap().clone();
                for chunk in &chunks {
                    meter.push(chunk);
                }
                let peak = t.peaks.lock().unwrap().iter().fold(0f32, |m, &(lo, hi)| m.max(-lo).max(hi));
                let _ = t.loudness.set(Loudness { lufs: meter.integrated(), peak_db: 20.0 * (peak as f64).log10() });
                notify();
            }
        });
        track
    }

    /// Estimates the tempo, key and chords on their own thread, once, after decoding is done.
    pub fn analyse(self: &Arc<Self>, notify: impl Fn() + Send + 'static) {
        if !self.done() || self.analysing.swap(true, Relaxed) {
            return;
        }
        let t = self.clone();
        std::thread::spawn(move || {
            background();
            let Some(format) = t.format.get() else { return };
            let mono = crate::tempo::mono(&t.chunks(), *format);
            // Only this thread holds the track: nobody will look at it.
            if Arc::strong_count(&t) == 1 {
                return;
            }
            let _ = t.tempo.set(crate::tempo::estimate(&mono, format.rate));
            notify();
            if Arc::strong_count(&t) > 1 {
                let _ = t.chords.set(crate::chords::find(&mono, format.rate).into());
                notify();
            }
        });
    }

    fn set_meta(self: &Arc<Self>, tags: Tags, info: Info, picture: Option<Box<[u8]>>, notify: Arc<dyn Fn() + Send + Sync>) {
        // Untagged to symphonia: maybe tagged where only lofty looks (a WAV's tags after its audio).
        let tags = if tags == Tags::default() { crate::tags::basic(&self.path).unwrap_or(tags) } else { tags };
        let _ = self.tags.set(tags);
        let _ = self.info.set(info);
        // On its own thread: a large JPEG must not hold up the first sound.
        if let Some(data) = picture {
            let t = self.clone();
            std::thread::spawn(move || {
                background();
                if let Ok(img) = image::load_from_memory(&data) {
                    let rgba = img.thumbnail(COVER_MAX, COVER_MAX).to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let _ = t.cover.set(eframe::egui::ColorImage::from_rgba_unmultiplied(size, &rgba));
                    notify();
                }
            });
        }
    }

    /// The same decoded audio under `path`, with the tags, info and cover read afresh from it:
    /// after its tags are rewritten or it is renamed. The audio itself must be unchanged.
    pub fn relabel(&self, path: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Result<Arc<Track>, String> {
        // Its decoder fills only this track: a copy taken now would stay part-decoded forever.
        if !self.done() {
            return Err(tr("the track is still loading").into());
        }
        let t = Arc::new(Track::empty(path));
        if let Some(&f) = self.format.get() {
            let _ = t.format.set(f);
        }
        if let Some(&l) = self.loudness.get() {
            let _ = t.loudness.set(l);
        }
        if let Some(&e) = self.tempo.get() {
            let _ = t.tempo.set(e);
        }
        if let Some(c) = self.chords.get() {
            let _ = t.chords.set(c.clone());
            t.analysing.store(true, Relaxed);
        }
        *t.beats.lock().unwrap_or_else(|e| e.into_inner()) = self.beats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        *t.chunks.lock().unwrap() = self.chunks.lock().unwrap().clone();
        *t.peaks.lock().unwrap() = self.peaks.lock().unwrap().clone();
        t.decoded.store(self.decoded(), Relaxed);
        t.done.store(self.done(), Relaxed);
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
        let mut meta = None;
        // Stops at the first packet: only the metadata is wanted.
        let result = decode(&t.path, |tags, info, picture| meta = Some((tags, info, picture)), |_, _, _, _| Err(String::new()));
        let (tags, info, picture) = meta.ok_or_else(|| result.err().unwrap_or_default())?;
        t.set_meta(tags, info, picture, notify);
        Ok(t)
    }

    fn empty(path: PathBuf) -> Track {
        Track {
            path,
            format: OnceLock::new(),
            expected: OnceLock::new(),
            tags: OnceLock::new(),
            info: OnceLock::new(),
            loudness: OnceLock::new(),
            tempo: OnceLock::new(),
            chords: OnceLock::new(),
            beats: Mutex::new(None),
            cover: OnceLock::new(),
            chunks: Mutex::new(Vec::new()),
            decoded: AtomicU64::new(0),
            peaks: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
            analysing: AtomicBool::new(false),
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

    /// The decoded chunks so far (shared, not copied): for analyses off the audio thread.
    pub fn chunks(&self) -> Vec<Arc<[f32]>> {
        self.chunks.lock().unwrap().clone()
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
        self.find(&self.chunks.try_lock().ok()?, pos)
    }

    /// `chunk_at` for readers off the audio thread (save, AI tools): waits for the list instead of
    /// missing it, so it is `None` only where nothing is decoded.
    pub fn chunk_at_wait(&self, pos: u64) -> Option<(Arc<[f32]>, usize)> {
        self.find(&self.chunks.lock().unwrap_or_else(|e| e.into_inner()), pos)
    }

    fn find(&self, chunks: &[Arc<[f32]>], pos: u64) -> Option<(Arc<[f32]>, usize)> {
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
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")) {
        return crate::midi::decode(path, on_meta, sink);
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| trf("not a supported audio file ({e})", &[("e", &e)]))?;
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
    let track = format.default_track(TrackType::Audio).ok_or(tr("no audio track"))?;
    let (track_id, expected) = (track.id, track.num_frames);
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or(tr("no audio track"))?;
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
    fn relabel_shares_the_audio_and_reads_new_tags() {
        let path = temp_file("relabel.wav", &wav(1000, 44100));
        let track = Track::load(path.clone(), || {});
        wait(&track);
        crate::tags::write_in_place(&path, &crate::tags::TagEdit { title: "New name".into(), ..Default::default() }).unwrap();
        let fresh = track.relabel(path, || {}).unwrap();
        assert!(Arc::ptr_eq(&track.chunk_at(0).unwrap().0, &fresh.chunk_at(0).unwrap().0));
        assert_eq!((fresh.decoded(), fresh.done()), (1000, true));
        assert_eq!(fresh.tags.get().and_then(|t| t.title.as_deref()), Some("New name"));
    }

    /// Runs `f` while another thread holds the chunk list for 100 ms, as the loudness pass,
    /// Practice analysis and Identify briefly do.
    fn while_busy(track: &Track, f: impl FnOnce()) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                let _held = track.chunks.lock().unwrap();
                tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(100));
            });
            rx.recv().unwrap();
            f();
        });
    }

    #[test]
    fn offline_readers_wait_for_a_busy_track() {
        use crate::edit::{self, Edit};
        let track = Track::from_samples(44100, 2, vec![0.25; 3 * CHUNK * 2]);
        let edit = Edit::whole(track.len());
        while_busy(&track, || {
            assert!(!edit::read(&track, &edit, 0, edit.len(), |_| {}), "the callback's reader never waits");
            let mut n = 0;
            assert!(edit::read_all(&track, &edit, 0, edit.len(), |b| n += b.len()));
            assert_eq!(n as u64, edit.len() * 2);
        });
        // A save that resamples reads the whole track too, instead of a shorter one.
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}-busy", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("48k.wav");
        let choice = crate::save::Choice { kind: crate::save::Kind::Wav, rate: Some(48000), ..Default::default() };
        while_busy(&track, || {
            crate::save::save(&track, &edit, choice, &path, None).unwrap();
        });
        let mut frames = 0;
        decode(&path, |_, _, _| {}, |s, c, _, _| Ok(frames += s.len() / c)).unwrap();
        let want = edit.len() * 48000 / 44100;
        assert!((frames as u64).abs_diff(want) <= 2, "{frames} frames, want {want}");
    }

    /// This thread's nice value, from /proc (field 19 of stat, after the parenthesised name).
    #[cfg(target_os = "linux")]
    fn nice() -> i32 {
        let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
        stat.rsplit(')').next().unwrap().split_whitespace().nth(16).unwrap().parse().unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn background_lowers_only_the_calling_thread() {
        let before = nice();
        assert_eq!(std::thread::spawn(|| {
            background();
            nice()
        }).join().unwrap(), 10.max(before));
        assert_eq!(nice(), before);
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
