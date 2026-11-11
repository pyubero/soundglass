//! Decoding (symphonia) and playback (cpal).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Decodes the default audio track packet by packet, handing interleaved f32 samples,
/// the channel count and the sample rate to `sink`. Stops at the first error `sink` returns.
pub fn decode(path: &Path, mut sink: impl FnMut(&[f32], usize, u32) -> Result<(), String>) -> Result<(), String> {
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
    let track_id = track.id;
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
                sink(&samples, buf.spec().channels().count(), buf.spec().rate())?;
            }
            // A damaged packet is skipped, as players usually do.
            Err(Error::DecodeError(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Plays `path` on a background thread. Errors go to `error`, and the window is woken to show them.
pub fn play(path: PathBuf, t0: Instant, timings: bool, bench: bool, error: Arc<Mutex<Option<String>>>, ctx: Arc<OnceLock<eframe::egui::Context>>) {
    std::thread::spawn(move || {
        let (mut stream, mut tx) = (None, None);
        let result = decode(&path, |samples, channels, rate| {
            if stream.is_none() {
                // About 1 s of MP3 packets or 3 s of FLAC: enough to ride out a slow disk.
                let (t, rx) = sync_channel(32);
                stream = Some(open(channels, rate, rx, t0, timings, bench)?);
                tx = Some(t);
            }
            // shortcut: rate or channel changes mid-file (chained Ogg) are ignored, handle them with resampling in milestone 2.
            let _ = tx.as_ref().unwrap().send(samples.to_vec());
            Ok(())
        });
        if let Err(e) = result {
            eprintln!("soundglass: {}: {e}", path.display());
            *error.lock().unwrap() = Some(e);
            if let Some(ctx) = ctx.get() {
                ctx.request_repaint();
            }
        }
        drop(tx);
        // The stream plays out what is queued; keep it alive until the window closes.
        let _stream = stream;
        loop {
            std::thread::park();
        }
    });
}

fn open(channels: usize, rate: u32, rx: Receiver<Vec<f32>>, t0: Instant, timings: bool, bench: bool) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_output_device().ok_or("no sound output device")?;
    // shortcut: no resampling, the device must accept the file's rate (PipeWire and PulseAudio do); add rubato in milestone 2, before Windows.
    let config = cpal::StreamConfig { channels: channels as u16, sample_rate: rate, buffer_size: cpal::BufferSize::Default };
    let (mut buf, mut pos, mut started) = (Vec::new(), 0, false);
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut i = 0;
                while i < out.len() {
                    if pos == buf.len() {
                        // shortcut: the old Vec is freed here, inside the audio callback; switch to a ring buffer if playback glitches.
                        match rx.try_recv() {
                            Ok(next) => (buf, pos) = (next, 0),
                            Err(_) => break,
                        }
                    }
                    let n = (out.len() - i).min(buf.len() - pos);
                    out[i..i + n].copy_from_slice(&buf[pos..pos + n]);
                    (i, pos) = (i + n, pos + n);
                }
                out[i..].fill(0.0);
                if i > 0 && !started {
                    started = true;
                    // Time the first real samples reach the device; the device's own latency comes on top.
                    if timings {
                        eprintln!("first sound {:.1} ms", crate::ms(t0));
                    }
                    if bench {
                        std::process::exit(0);
                    }
                }
            },
            |e| eprintln!("soundglass: sound output: {e}"),
            None,
        )
        .map_err(|e| format!("device does not support {channels} channels at {rate} Hz ({e})"))?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::decode;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("soundglass-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn decodes_a_wav() {
        let (frames, rate) = (1000u32, 22050u32);
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
        let path = temp_file("tone.wav", &wav);

        let mut total = 0;
        decode(&path, |samples, channels, r| {
            assert_eq!((channels, r), (2, rate));
            total += samples.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(total, frames as usize * 2);
    }

    #[test]
    fn junk_is_an_error_not_a_panic() {
        let path = temp_file("junk.mp3", &[0x5a; 4096]);
        assert!(decode(&path, |_, _, _| Ok(())).is_err());
    }
}
