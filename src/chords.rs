//! Chords over time: chroma per frame, major/minor/7th templates, Viterbi smoothing.
//! A first try (docs/CHORDS.md): it finds the main chords better than the changes.

use bpm_key::advanced::{resample, stft, tonal};

use crate::edit::Edit;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Major,
    Minor,
    Seventh,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chord {
    /// Pitch class, C = 0.
    pub root: u8,
    pub kind: Kind,
}

/// A chord from frame to frame (end excluded); gaps between spans are "no chord".
pub type Span = (u64, u64, Chord);

const SR: u32 = 22_050;
const NFFT: usize = 8192;
const HOP: usize = 2048;
const KINDS: [(Kind, &[usize]); 3] = [(Kind::Major, &[0, 4, 7]), (Kind::Minor, &[0, 3, 7]), (Kind::Seventh, &[0, 4, 7, 10])];
/// States: kind × 12 + root, then "no chord".
const STATES: usize = 37;
const NONE: usize = 36;
/// Tuned on the test album (docs/CHORDS.md).
const NO_CHORD: f32 = 0.25;
const PENALTY: f32 = 5.0;

/// The chords of `mono` at `rate`, in its frames.
pub fn find(mono: &[f32], rate: u32) -> Vec<Span> {
    if rate == 0 || mono.len() < NFFT * rate as usize / SR as usize || !mono.iter().all(|v| v.is_finite()) {
        return Vec::new();
    }
    // A hostile file must not take soundglass down with it.
    std::panic::catch_unwind(|| {
        let x = resample(mono, rate, SR);
        let scores: Vec<[f32; STATES]> = chroma(&x).iter().map(scores).collect();
        let path = viterbi(&scores);
        // Frame t is centred on sample t·HOP + NFFT/2 and stands for the hop around it.
        let frame = |t: usize| (((t * HOP + NFFT / 2) as f64 - HOP as f64 / 2.0) / SR as f64 * rate as f64).max(0.0) as u64;
        let mut spans: Vec<Span> = Vec::new();
        for (t, &s) in path.iter().enumerate() {
            if s == NONE {
                continue;
            }
            let chord = Chord { root: (s % 12) as u8, kind: KINDS[s / 12].0 };
            match spans.last_mut() {
                Some(last) if last.2 == chord && last.1 == frame(t) => last.1 = frame(t + 1),
                _ => spans.push((frame(t), frame(t + 1), chord)),
            }
        }
        spans
    })
    .unwrap_or_default()
}

/// Unit chroma per frame (C = 0), `None` for frames near silence.
fn chroma(x: &[f32]) -> Vec<Option<[f32; 12]>> {
    let spec = stft(x, SR as f32, NFFT, HOP);
    let tuning = tonal(&spec).map_or(0.0, |t| t.tuning_cents);
    let (lo, hi) = (((55.0 / spec.bin_hz(1.0)) as usize).max(1), ((1760.0 / spec.bin_hz(1.0)) as usize).min(spec.bins - 1));
    let raw: Vec<([f32; 12], f32)> = (0..spec.frames)
        .map(|t| {
            let m = spec.frame(t);
            let mut c = [0f32; 12];
            for (k, &mag) in m.iter().enumerate().take(hi + 1).skip(lo) {
                let midi = 69.0 + 12.0 * (spec.bin_hz(k as f32) / 440.0).log2() - tuning / 100.0;
                c[(midi.round() as i32).rem_euclid(12) as usize] += (1.0 + 100.0 * mag).ln();
            }
            (c, c.iter().map(|v| v * v).sum::<f32>().sqrt())
        })
        .collect();
    let loudest = raw.iter().fold(0f32, |a, r| a.max(r.1));
    raw.into_iter().map(|(c, n)| (n > 0.05 * loudest && n > 0.0).then(|| c.map(|v| v / n))).collect()
}

/// Each state's score for a frame: the Pearson correlation with the chord's notes (both
/// centred, so a four-note chord isn't favoured), or the fixed "no chord" score.
fn scores(c: &Option<[f32; 12]>) -> [f32; STATES] {
    let mut s = [0f32; STATES];
    let Some(c) = c else {
        s[NONE] = 1.0;
        return s;
    };
    s[NONE] = NO_CHORD;
    let cm = c.iter().sum::<f32>() / 12.0;
    for (kind, (_, notes)) in KINDS.iter().enumerate() {
        let tm = notes.len() as f32 / 12.0;
        for root in 0..12 {
            let mut t = [0f32; 12];
            for n in notes.iter() {
                t[(root + n) % 12] = 1.0;
            }
            let (mut num, mut cc, mut tt) = (0f32, 0f32, 0f32);
            for i in 0..12 {
                num += (c[i] - cm) * (t[i] - tm);
                cc += (c[i] - cm).powi(2);
                tt += (t[i] - tm).powi(2);
            }
            s[kind * 12 + root] = if cc > 0.0 { num / (cc * tt).sqrt() } else { 0.0 };
        }
    }
    s
}

/// The state path with the highest summed score, less `PENALTY` per change.
fn viterbi(frames: &[[f32; STATES]]) -> Vec<usize> {
    let best = |v: &[f32; STATES]| v.iter().enumerate().fold((f32::MIN, 0), |b, (i, &x)| if x > b.0 { (x, i) } else { b });
    let Some(first) = frames.first() else { return Vec::new() };
    let mut back = vec![[0u8; STATES]; frames.len()];
    let mut cur = *first;
    for (t, frame) in frames.iter().enumerate().skip(1) {
        let (top, arg) = best(&cur);
        let mut next = [0f32; STATES];
        for s in 0..STATES {
            let (v, from) = if cur[s] >= top - PENALTY { (cur[s], s) } else { (top - PENALTY, arg) };
            next[s] = v + frame[s];
            back[t][s] = from as u8;
        }
        cur = next;
    }
    let mut s = best(&cur).1;
    let mut path = vec![0; frames.len()];
    for t in (0..frames.len()).rev() {
        path[t] = s;
        s = back[t][s] as usize;
    }
    path
}

/// `spans` (in the original's frames) where `edit` plays them: cut parts gone, reversed parts
/// mirrored, silence without chords.
pub fn edited(spans: &[Span], edit: &Edit) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0;
    for seg in &edit.segs {
        let (lo, hi) = (seg.src, seg.src + seg.len);
        for &(a, b, chord) in spans.iter().filter(|_| !seg.silent) {
            let (a, b) = (a.max(lo), b.min(hi));
            if a < b {
                let (x, y) = if seg.rev { (hi - b, hi - a) } else { (a - lo, b - lo) };
                out.push((at + x, at + y, chord));
            }
        }
        at += seg.len;
    }
    out.sort_by_key(|s| s.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 s of a chord's notes (with an octave above each), at 44.1 kHz.
    fn tones(pcs: &[u8]) -> Vec<f32> {
        let rate = 44100.0;
        (0..44100 * 8)
            .map(|i| {
                let t = i as f32 / rate;
                pcs.iter().map(|&pc| {
                    let f = 261.63 * 2f32.powf(pc as f32 / 12.0);
                    (t * f * std::f32::consts::TAU).sin() + 0.5 * (t * 2.0 * f * std::f32::consts::TAU).sin()
                }).sum::<f32>() * 0.1
            })
            .collect()
    }

    fn main_chord(spans: &[Span]) -> Chord {
        spans.iter().max_by_key(|s| s.1 - s.0).unwrap().2
    }

    #[test]
    fn names_major_minor_and_seventh_chords() {
        assert_eq!(main_chord(&find(&tones(&[0, 4, 7]), 44100)), Chord { root: 0, kind: Kind::Major });
        assert_eq!(main_chord(&find(&tones(&[9, 12, 16]), 44100)), Chord { root: 9, kind: Kind::Minor });
        assert_eq!(main_chord(&find(&tones(&[7, 11, 14, 17]), 44100)), Chord { root: 7, kind: Kind::Seventh });
    }

    #[test]
    fn silence_and_garbage_give_no_chords() {
        assert!(find(&vec![0.0; 44100 * 8], 44100).is_empty());
        assert!(find(&vec![f32::NAN; 44100 * 8], 44100).is_empty());
        assert!(find(&[], 44100).is_empty());
        assert!(find(&[0.5; 100], 44100).is_empty());
    }

    #[test]
    fn spans_follow_the_edits() {
        let c = Chord { root: 0, kind: Kind::Major };
        let g = Chord { root: 7, kind: Kind::Major };
        let spans = [(0, 50, c), (50, 100, g)];
        // Cut 40..60: C to 40, then G from 40.
        assert_eq!(edited(&spans, &Edit::whole(100).cut(40, 60)), vec![(0, 40, c), (40, 80, g)]);
        // Reversed: G first.
        assert_eq!(edited(&spans, &Edit::whole(100).reverse(0, 100)), vec![(0, 50, g), (50, 100, c)]);
        // Silenced: no chord there.
        assert_eq!(edited(&spans, &Edit::whole(100).silence(0, 50)), vec![(50, 100, g)]);
    }
}
