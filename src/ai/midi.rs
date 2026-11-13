//! Melody to MIDI: Spotify's Basic Pitch (nmp.onnx), its windowing and note decoding ported from
//! basic_pitch/inference.py and note_creation.py (defaults: onset 0.5, frame 0.3, 127.7 ms
//! shortest note, inferred onsets, the "melodia trick"). No pitch bends.
use super::*;

pub const RATE: u32 = 22_050;
const HOP: usize = 256;
/// Samples per window, and frames per window out of the model.
const WINDOW: usize = RATE as usize * 2 - HOP;
const WINDOW_FRAMES: usize = 172;
const OVERLAP_FRAMES: usize = 30;
const OVERLAP: usize = OVERLAP_FRAMES * HOP;
const STRIDE: usize = WINDOW - OVERLAP;
const KEYS: usize = 88;
/// The lowest key's MIDI number (A0).
const MIDI_OFFSET: u8 = 21;
const ONSET: f32 = 0.5;
const FRAME: f32 = 0.3;
/// round(127.7 ms × 22050 / 256).
const MIN_NOTE: usize = 11;
const ENERGY_TOL: usize = 11;

/// A note: start and end in model frames, MIDI number, mean activation (0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Note {
    pub start: usize,
    pub end: usize,
    pub key: u8,
    pub amp: f32,
}

/// The note and onset activations, frames × 88, for a mono signal at `RATE`.
pub(super) fn activations(model: &Path, mono: &[f32], progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(Vec<[f32; KEYS]>, Vec<[f32; KEYS]>), String> {
    use ort::value::Tensor;
    let err = |e: ort::Error| e.to_string();
    let mut session = session_with(model, cores(), false)?;
    // Half an overlap of silence first, as inference.py does.
    let mut audio = vec![0f32; OVERLAP / 2];
    audio.extend_from_slice(mono);
    let windows = audio.len().div_ceil(STRIDE).max(1);
    let (mut notes, mut onsets) = (Vec::new(), Vec::new());
    for w in 0..windows {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let start = w * STRIDE;
        let mut input = vec![0f32; WINDOW];
        let part = &audio[start.min(audio.len())..(start + WINDOW).min(audio.len())];
        input[..part.len()].copy_from_slice(part);
        let outputs = session.run(ort::inputs!["serving_default_input_2:0" => Tensor::from_array(([1usize, WINDOW, 1], input)).map_err(err)?]).map_err(err)?;
        for (name, out) in [("StatefulPartitionedCall:1", &mut notes), ("StatefulPartitionedCall:2", &mut onsets)] {
            let (_, data) = output(&outputs, name)?;
            if data.len() != WINDOW_FRAMES * KEYS {
                return Err("the model gave an unexpected output".into());
            }
            // Half the overlap off each end of every window.
            out.extend(data.chunks_exact(KEYS).skip(OVERLAP_FRAMES / 2).take(WINDOW_FRAMES - OVERLAP_FRAMES).map(|f| <[f32; KEYS]>::try_from(f).unwrap()));
        }
        progress((w + 1) as f32 / windows as f32);
    }
    let keep = (mono.len() as f64 / STRIDE as f64 * (WINDOW_FRAMES - OVERLAP_FRAMES) as f64) as usize;
    notes.truncate(keep);
    onsets.truncate(keep);
    Ok((notes, onsets))
}

/// Notes from the activations, as output_to_notes_polyphonic.
pub(super) fn notes(frames: &[[f32; KEYS]], onsets: &[[f32; KEYS]]) -> Vec<Note> {
    let n = frames.len();
    // Onsets inferred from rises in the frames, scaled to the predicted onsets' peak.
    let rise: Vec<[f32; KEYS]> = (0..n)
        .map(|t| std::array::from_fn(|k| if t < 2 { 0.0 } else { (frames[t][k] - frames[t - 1][k]).min(frames[t][k] - frames[t - 2][k]).max(0.0) }))
        .collect();
    let peak = |m: &[[f32; KEYS]]| m.iter().flatten().fold(0f32, |a, &b| a.max(b));
    let (onset_max, rise_max) = (peak(onsets), peak(&rise));
    let onsets: Vec<[f32; KEYS]> = onsets.iter().zip(&rise).map(|(o, r)| std::array::from_fn(|k| o[k].max(if rise_max > 0.0 { onset_max * r[k] / rise_max } else { 0.0 }))).collect();
    // Peaks in time over the threshold, latest first.
    let mut starts: Vec<(usize, usize)> = (1..n.saturating_sub(1)).flat_map(|t| (0..KEYS).map(move |k| (t, k))).filter(|&(t, k)| onsets[t][k] >= ONSET && onsets[t][k] > onsets[t - 1][k] && onsets[t][k] > onsets[t + 1][k]).collect();
    starts.reverse();
    let mut left: Vec<[f32; KEYS]> = frames.to_vec();
    let mut out = Vec::new();
    let clear = |left: &mut Vec<[f32; KEYS]>, t: usize, k: usize| {
        left[t][k] = 0.0;
        if k + 1 < KEYS {
            left[t][k + 1] = 0.0;
        }
        if k > 0 {
            left[t][k - 1] = 0.0;
        }
    };
    let mean = |a: usize, b: usize, k: usize| frames[a..b].iter().map(|f| f[k]).sum::<f32>() / (b - a).max(1) as f32;
    for (start, k) in starts {
        if start >= n - 1 {
            continue;
        }
        // On until the frames stay under the threshold for ENERGY_TOL frames.
        let (mut i, mut below) = (start + 1, 0);
        while i < n - 1 && below < ENERGY_TOL {
            below = if left[i][k] < FRAME { below + 1 } else { 0 };
            i += 1;
        }
        i -= below;
        if i - start <= MIN_NOTE {
            continue;
        }
        (start..i).for_each(|t| clear(&mut left, t, k));
        out.push(Note { start, end: i, key: k as u8 + MIDI_OFFSET, amp: mean(start, i, k) });
    }
    // The melodia trick: notes grown both ways from the strongest frame left, again and again.
    // Values only ever drop to 0, so the strongest left is the next one in this list not yet
    // cleared (ties in time, then key order, as numpy's argmax).
    let mut seeds: Vec<(usize, usize)> = (0..n).flat_map(|t| (0..KEYS).map(move |k| (t, k))).filter(|&(t, k)| left[t][k] > FRAME).collect();
    seeds.sort_by(|a, b| left[b.0][b.1].total_cmp(&left[a.0][a.1]).then(a.cmp(b)));
    for (mid, k) in seeds {
        if left[mid][k] == 0.0 {
            continue;
        }
        left[mid][k] = 0.0;
        let (mut i, mut below) = (mid + 1, 0);
        while i < n - 1 && below < ENERGY_TOL {
            below = if left[i][k] < FRAME { below + 1 } else { 0 };
            clear(&mut left, i, k);
            i += 1;
        }
        let end = i - 1 - below;
        let (mut i, mut below) = (mid as isize - 1, 0);
        while i > 0 && below < ENERGY_TOL {
            below = if left[i as usize][k] < FRAME { below + 1 } else { 0 };
            clear(&mut left, i as usize, k);
            i -= 1;
        }
        let start = (i + 1 + below as isize) as usize;
        if end <= start || end - start <= MIN_NOTE {
            continue;
        }
        out.push(Note { start, end, key: k as u8 + MIDI_OFFSET, amp: mean(start, end, k) });
    }
    out
}

/// A model frame's time in seconds, as model_frames_to_time (with its alignment offset per window).
fn seconds(frame: usize) -> f64 {
    let hop = HOP as f64 / RATE as f64;
    let window_offset = hop * (WINDOW_FRAMES as f64 - WINDOW as f64 / HOP as f64) + 0.0018;
    frame as f64 * hop - window_offset * (frame / WINDOW_FRAMES) as f64
}

/// A format 0 Standard MIDI File at 120 bpm, 220 ticks a beat (as pretty_midi), Electric Piano 1.
pub(super) fn smf(notes: &[Note]) -> Vec<u8> {
    const PPQ: f64 = 220.0;
    let tick = |frame: usize| (seconds(frame).max(0.0) * 2.0 * PPQ).round() as u32;
    // (tick, off before on at the same tick, bytes)
    let mut events: Vec<(u32, u8, [u8; 3])> = Vec::with_capacity(notes.len() * 2);
    for n in notes {
        let velocity = (127.0 * n.amp).round().clamp(1.0, 127.0) as u8;
        events.push((tick(n.start), 1, [0x90, n.key, velocity]));
        events.push((tick(n.end), 0, [0x80, n.key, 0]));
    }
    events.sort_by_key(|e| (e.0, e.1));
    let mut track = vec![0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0xC0, 0x04];
    let mut last = 0;
    for (t, _, bytes) in events {
        push_varlen(&mut track, t - last);
        track.extend_from_slice(&bytes);
        last = t;
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let mut file = b"MThd\0\0\0\x06\0\0\0\x01".to_vec();
    file.extend_from_slice(&(PPQ as u16).to_be_bytes());
    file.extend_from_slice(b"MTrk");
    file.extend_from_slice(&(track.len() as u32).to_be_bytes());
    file.extend(track);
    file
}

fn push_varlen(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.extend(bytes.iter().rev());
}
