//! A track decoded fully into memory on a background thread, readable while it fills in.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

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

// shortcut: whole track in memory (~1.4 GB per hour at 48 kHz stereo); a RAM limit in Settings comes later (user's choice).
pub struct Track {
    pub path: PathBuf,
    pub format: OnceLock<Format>,
    /// Length the container reports, before decoding finishes.
    pub expected: OnceLock<u64>,
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
    pub fn load(path: PathBuf, notify: impl Fn() + Send + 'static) -> Arc<Track> {
        let track = Arc::new(Track::empty(path));
        let t = track.clone();
        std::thread::spawn(move || {
            let mut cur: Vec<f32> = Vec::new();
            let (mut peak, mut peak_frames) = ((f32::MAX, f32::MIN), 0);
            let result = decode(&t.path.clone(), |samples, channels, rate, expected| {
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
        });
        track
    }

    fn empty(path: PathBuf) -> Track {
        Track {
            path,
            format: OnceLock::new(),
            expected: OnceLock::new(),
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

    #[cfg(test)]
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
pub fn decode(path: &Path, mut sink: impl FnMut(&[f32], usize, u32, Option<u64>) -> Result<(), String>) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("not a supported audio file ({e})"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let (track_id, expected) = (track.id, track.num_frames);
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio track")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| e.to_string())?;
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

    fn wait(track: &Track) {
        while !track.done() {
            std::thread::yield_now();
        }
    }

    #[test]
    fn decodes_a_wav() {
        let mut total = 0;
        decode(&temp_file("tone.wav", &wav(1000, 22050)), |samples, channels, rate, _| {
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
        assert!(decode(&path, |_, _, _, _| Ok(())).is_err());
        let track = Track::load(path, || {});
        wait(&track);
        assert!(track.error.lock().unwrap().is_some());
    }
}
