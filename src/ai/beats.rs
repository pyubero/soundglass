//! Find Beats: Beat This! (JKU Linz, ISMIR 2024), final0, as musetric's export (checked against
//! torch on real music; songbird's final0.onnx wasn't: its logits were up to 13.8 off). The
//! log-mel front end is ours, with torchaudio's filterbank from the same repo. Chunking and the
//! "minimal" peak picking ported from beat_this/inference.py and model/postprocessor.py:
//! 1500-frame chunks, 6-frame borders, earlier chunks kept on overlaps, a peak is a positive
//! logit that is the largest within ±3 frames, neighbours merged.
use super::*;

pub const RATE: u32 = 22_050;
const FPS: f64 = 50.0;
const CHUNK: usize = 1500;
const BORDER: usize = 6;
const MELS: usize = 128;
const FFT: usize = 1024;
const HOP: usize = 441;
const BINS: usize = FFT / 2 + 1;

/// Beats and downbeats in the original's frames, in order; every downbeat is one of the beats.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beats {
    pub beats: Vec<u64>,
    pub downbeats: Vec<u64>,
}

/// Beats of `mono` (at `RATE`), as frames at the original's `rate`. `filterbank` is
/// torchaudio's slaney mel filterbank, [513, 128] row-major f32.
/// `threshold`: the logit a peak must pass (the reference's 0).
pub(super) fn track(model: &Path, filterbank: &Path, mono: Vec<f32>, rate: u32, threshold: f32, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Beats, String> {
    use ort::value::Tensor;
    let err = |e: ort::Error| e.to_string();
    let bank = std::fs::read(filterbank).map_err(|e| e.to_string())?;
    if bank.len() != BINS * MELS * 4 {
        return Err("the mel filterbank isn't Beat This!'s".into());
    }
    let bank: Vec<f32> = bank.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let mel = log_mel(&mono, &bank);
    let frames = mel.len() / MELS;
    progress(0.05);
    let mut session = session(model)?;
    let input_name = session.inputs().first().map(|i| i.name().to_string()).ok_or("the model has no input")?;
    let starts = chunk_starts(frames);
    let (mut beat, mut down) = (vec![-1000f32; frames], vec![-1000f32; frames]);
    // Latest first, so the earlier chunk's prediction is the one kept where they overlap.
    for (n, &start) in starts.iter().rev().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let (lo, hi) = (start.max(0) as usize, ((start + CHUNK as isize) as usize).min(frames));
        let (left, right) = ((-start).max(0) as usize, (start + CHUNK as isize - frames as isize).clamp(0, BORDER as isize) as usize);
        let t = left + (hi - lo) + right;
        // The export is fixed at 1500 frames: a short piece is padded with silence (zeros).
        let mut input = vec![0f32; CHUNK * MELS];
        input[left * MELS..(left + hi - lo) * MELS].copy_from_slice(&mel[lo * MELS..hi * MELS]);
        let out = session.run(ort::inputs![input_name.as_str() => Tensor::from_array(([1usize, CHUNK, MELS], input)).map_err(err)?]).map_err(err)?;
        let (_, b) = output(&out, "beat")?;
        let (_, d) = output(&out, "downbeat")?;
        if b.len() != CHUNK || d.len() != CHUNK {
            return Err("the model gave an unexpected output".into());
        }
        for j in BORDER..t.saturating_sub(BORDER) {
            let at = start + j as isize;
            if (0..frames as isize).contains(&at) {
                (beat[at as usize], down[at as usize]) = (b[j], d[j]);
            }
        }
        progress(0.05 + 0.95 * (n + 1) as f32 / starts.len() as f32);
    }
    let beats: Vec<f64> = peaks(&beat, threshold);
    // Each downbeat moved to its nearest beat.
    let mut downbeats: Vec<f64> = peaks(&down, threshold).into_iter().filter_map(|f| beats.iter().copied().min_by(|a, b| (a - f).abs().total_cmp(&(b - f).abs()))).collect();
    downbeats.dedup();
    let frame = |f: f64| (f / FPS * rate as f64).round() as u64;
    Ok(Beats { beats: beats.into_iter().map(frame).collect(), downbeats: downbeats.into_iter().map(frame).collect() })
}

/// `points` (in the original's frames, in order) where `edit` plays them: cut ones gone,
/// reversed parts mirrored, none in silence.
pub fn edited(points: &[u64], edit: &crate::edit::Edit) -> Vec<u64> {
    let mut out = Vec::new();
    let mut at = 0;
    for seg in &edit.segs {
        if !seg.silent {
            let (lo, hi) = (seg.src, seg.src + seg.len);
            let inside = &points[points.partition_point(|&p| p < lo)..points.partition_point(|&p| p < hi)];
            out.extend(inside.iter().map(|&p| at + if seg.rev { hi - 1 - p } else { p - lo }));
        }
        at += seg.len;
    }
    out.sort_unstable();
    out
}

/// LogMelSpect: centred STFT (reflect padding, periodic Hann) of 1024 with hop 441, magnitude
/// over √1024, onto the filterbank, log1p(1000 x). Frames × 128, row-major.
pub(super) fn log_mel(x: &[f32], bank: &[f32]) -> Vec<f32> {
    use realfft::num_complex::Complex;
    let n = x.len();
    let frames = n / HOP + 1;
    let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
    let window: Vec<f32> = (0..FFT).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).collect();
    // Reflect padding by FFT/2 on both sides (as torch), zeros for clips too short to reflect.
    let at = |i: isize| -> f32 {
        let m = n as isize;
        let j = if i < 0 { -i } else if i >= m { 2 * (m - 1) - i } else { i };
        if (0..m).contains(&j) { x[j as usize] } else { 0.0 }
    };
    let (mut buf, mut spec, mut mag) = (fft.make_input_vec(), fft.make_output_vec(), vec![0f32; BINS]);
    let mut out = vec![0f32; frames * MELS];
    for f in 0..frames {
        let start = (f * HOP) as isize - (FFT / 2) as isize;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = at(start + i as isize) * window[i];
        }
        let _ = fft.process(&mut buf, &mut spec);
        for (m, c) in mag.iter_mut().zip(&spec) {
            *m = Complex::norm(*c) / (FFT as f32).sqrt();
        }
        for (k, o) in out[f * MELS..(f + 1) * MELS].iter_mut().enumerate() {
            let e: f32 = mag.iter().enumerate().map(|(b, m)| m * bank[b * MELS + k]).sum();
            *o = (1000.0 * e).ln_1p();
        }
    }
    out
}

/// As split_piece with avoid_short_end: chunk starts (from −BORDER) a chunk less two borders apart,
/// the last one moved back to end a border past the end.
fn chunk_starts(frames: usize) -> Vec<isize> {
    let step = (CHUNK - 2 * BORDER) as isize;
    let mut starts: Vec<isize> = (-(BORDER as isize)..frames as isize - BORDER as isize).step_by(step as usize).collect();
    if frames > CHUNK - 2 * BORDER {
        *starts.last_mut().unwrap() = frames as isize - (CHUNK - BORDER) as isize;
    }
    starts
}

/// Frames (as fractions: merged neighbours are averaged) where the logit is over `threshold`
/// and the largest within ±3 frames.
fn peaks(logits: &[f32], threshold: f32) -> Vec<f64> {
    let n = logits.len();
    let on: Vec<usize> = (0..n).filter(|&i| logits[i] > threshold && logits[i.saturating_sub(3)..(i + 4).min(n)].iter().all(|&x| x <= logits[i])).collect();
    // deduplicate_peaks(width = 1): runs of neighbours become their running mean.
    let mut out = Vec::new();
    let mut iter = on.into_iter();
    let Some(first) = iter.next() else { return out };
    let (mut p, mut c) = (first as f64, 1.0);
    for q in iter {
        if q as f64 - p <= 1.0 {
            c += 1.0;
            p += (q as f64 - p) / c;
        } else {
            out.push(p);
            (p, c) = (q as f64, 1.0);
        }
    }
    out.push(p);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_the_piece_and_peaks_merge() {
        assert_eq!(chunk_starts(100), [-6]);
        assert_eq!(chunk_starts(3000), [-6, 1482, 1506]);
        let mut l = vec![-5f32; 40];
        (l[5], l[6], l[20], l[30]) = (2.0, 2.0, 1.0, -0.5);
        assert_eq!(peaks(&l, 0.0), [5.5, 20.0]);
        // Lower, the weaker peak at 30 counts too.
        assert_eq!(peaks(&l, -1.0), [5.5, 20.0, 30.0]);
    }

    #[test]
    fn beats_follow_the_edits() {
        use crate::edit::Edit;
        let beats = [10, 30, 50, 70, 90];
        assert_eq!(edited(&beats, &Edit::whole(100)), beats);
        assert_eq!(edited(&beats, &Edit::whole(100).cut(20, 60)), [10, 30, 50]);
        assert_eq!(edited(&beats, &Edit::whole(100).reverse(0, 100)), [9, 29, 49, 69, 89]);
        assert_eq!(edited(&beats, &Edit::whole(100).silence(0, 40)), [50, 70, 90]);
    }
}
