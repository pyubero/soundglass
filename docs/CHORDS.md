# Chord recognition: the first try

The idea: a bar over the Spectrogram, switched by a Chords button, with the chords written against the track's time. This note keeps the first attempt so a later one can start from it. Tried 2026-11-12 on the test album "1954-Louis Armstrong Plays W.C.Handy", which is mostly 12-bar blues (I–IV–V, usually as 7th chords), so a good result should show three chords taking turns every one to four bars.

## What was tried

The classic method, with no new dependency (only bpm-key 0.1.0's `advanced` module):

- Mono audio resampled to 22 050 Hz, STFT 8192 / hop 2048 (`advanced::stft`): 93 ms frames, 2.7 Hz bins.
- Chroma: every bin from 55 to 1760 Hz added to its pitch class as `ln(1 + 100·magnitude)`, shifted by the track's tuning (`advanced::tonal`), then each frame scaled to unit length. A frame under 5 % of the loudest frame's level is "no chord".
- Vocabulary: major, minor and dominant 7th on all 12 roots, plus "no chord" (37 states).
- Score: the Pearson correlation of the frame's chroma with each chord's binary template, and a fixed score (0.25) for "no chord".
- Smoothing: Viterbi over the 37 states, maximising the summed scores minus a penalty (5) for each change.

### Tuning notes

- Plain cosine similarity with binary templates (the first version) called almost everything a 7th chord, the spoken tracks included (Alligator Story 89 % Ab7): a four-note template collects more of the background energy than a triad. Pearson correlation centres both and fixes that; the speech then comes out as "no chord".
- With Pearson the scores are smaller, so the change penalty went up from 1–2 to 5; at 3 the median chord lasted 2.8 s, at 8 it lasted 9 s and too much became "no chord".
- The "no chord" score decides how much music is left unnamed: 0.35 already left 54 % of Yellow Dog Blues blank, 0.25 left 8 %.

## Results on the test album (penalty 5, no chord 0.25)

Changes per minute, median chord length, and the five chords taking most of the time:

- 01 St. Louis Blues: 8/min, 5.7 s; Db 46 %, N 16 %, Bbm 7 %, Fm 6 %, Bb 6 %
- 02 Yellow Dog Blues: 12/min, 4.0 s; Eb 38 %, Bb7 17 %, Ab7 10 %, N 8 %, Fm 7 %
- 03 Loveless Love: 5/min, 4.0 s; Ab7 26 %, Fm 19 %, N 18 %, Ab 17 %, C 10 %
- 04 Aunt Hagar's Blues: 10/min, 3.9 s; Eb 39 %, Ab7 11 %, Fm 10 %, N 8 %, Ebm 7 %
- 05 Long Gone: 4/min, 6.5 s; N 60 %, Ab7 18 %, Fm 12 %, Cm 4 %, Ab 2 %
- 06 The Memphis Blues: 7/min, 5.6 s; Bb7 26 %, Fm 19 %, Db 15 %, N 11 %, C 7 %
- 07 Beale Street Blues: 11/min, 3.9 s; Eb 29 %, Dm 20 %, Cm 10 %, Fm 9 %, N 9 %
- 08 Ole Miss Blues: 6/min, 5.9 s; Fm 48 %, C 15 %, N 13 %, Cm 7 %, Db 6 %
- 09 Chantez Les Bas: 9/min, 4.6 s; Eb 22 %, Cm 19 %, Ab 11 %, Ab7 10 %, N 10 %
- 10 Hesitating Blues: 11/min, 4.7 s; F 37 %, N 14 %, Fm 13 %, C 12 %, D7 7 %
- 11 Atlanta Blues: 6/min, 5.9 s; F 45 %, Dm 17 %, N 13 %, Fm 12 %, Am 4 %
- 12 Interview (speech): no changes; N 100 %
- 13 Lovelass Love: 6/min, 4.7 s; N 33 %, Fm 16 %, Cm 16 %, Ab 13 %, C 10 %
- 14 Hesitating Blues: 9/min, 5.3 s; N 35 %, F 25 %, C 10 %, Fm 8 %, D7 7 %
- 15 Alligator Story (speech): no changes; N 100 %
- 16 Long Gone: 1/min, 8.5 s; N 93 %, Fm 4 %, Ab 2 %

The first minute of three tracks:

- 02 Yellow Dog Blues: N to 0:00.6, Eb to 0:10.1, Bb7 to 0:11.7, N to 0:20.1, Cm to 0:22.8, Ab7 to 0:29.4, Bb7 to 0:54.9, Cm to 1:06.5
- 06 The Memphis Blues: C to 0:04.0, F to 0:09.6, C to 0:15.5, F to 0:21.6, C to 0:24.9, A7 to 0:30.3, N to 0:49.5, C7 to 0:51.8, Fm to 1:08.4
- 11 Atlanta Blues: A7 to 0:03.0, then F to 1:24.1

What it shows:

- It finds each track's tonal centre and its main chords (Eb, Ab7, Bb7 in Yellow Dog Blues; Eb with Ab7 in Aunt Hagar's Blues), and speech is left blank.
- It does not follow the changes: a 25 s Bb7 in Yellow Dog Blues and 81 s of F in Atlanta Blues are many bars of a 12-bar blues each, and Long Gone (16) is 93 % blank.
- Relative chords get swapped (Fm or Cm for Ab or Eb, Dm for F), the usual weakness of triad templates when a 7th or 6th is played.

## What to try next

- Beat-synchronous chroma (average the frames between beats, then smooth): chords change on beats, and it removes most of the flicker without a big penalty. This waits on a working beat tracker (docs/BEAT_TRACKING.md).
- A bass chroma (under ~250 Hz) besides the treble one, scored against each chord's root: it tells F from Dm and Ab from Fm.
- Templates with weighted harmonics (each note's 2nd and 3rd harmonics added at lower weight) instead of binary masks.
- A key-aware transition matrix in the Viterbi (changes to I, IV, V cheaper than to far chords), using the key bpm-key already estimates.
- An HMM with a "no chord" state learnt from silence and speech, instead of a fixed score.
- Check it against something known: a track with a published chord chart, or a synthetic I–IV–V test signal.

## The code

The functions of the scratch prototype, fed mono f32 at 44 100 Hz from ffmpeg (`ffmpeg -v error -i TRACK.mp3 -f f32le -ac 1 -ar 44100 -`) and resampled with `advanced::resample(&s, 44100, SR)`. `no_chord()` returned 0.25 (an argument in the prototype); `viterbi` was called with penalty 5.

```rust
use bpm_key::advanced::{resample, stft, tonal};
use std::io::Read;

const SR: u32 = 22_050;
const NFFT: usize = 8192;
const HOP: usize = 2048;
const ROOTS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const KINDS: [(&str, &[usize]); 3] = [("", &[0, 4, 7]), ("m", &[0, 3, 7]), ("7", &[0, 4, 7, 10])];
const N: usize = 36; // "no chord"; chords are kind * 12 + root

fn name(s: usize) -> String {
    if s == N { "N".into() } else { format!("{}{}", ROOTS[s % 12], KINDS[s / 12].0) }
}

/// Unit chroma per frame (None for near-silent frames), C = 0.
fn chroma(x: &[f32]) -> Vec<Option<[f32; 12]>> {
    let spec = stft(x, SR as f32, NFFT, HOP);
    let tuning = tonal(&spec).map_or(0.0, |t| t.tuning_cents);
    let (lo, hi) = ((55.0 / spec.bin_hz(1.0)) as usize, (1760.0 / spec.bin_hz(1.0)) as usize);
    let raw: Vec<([f32; 12], f32)> = (0..spec.frames)
        .map(|t| {
            let m = spec.frame(t);
            let mut c = [0f32; 12];
            for k in lo.max(1)..=hi.min(spec.bins - 1) {
                let midi = 69.0 + 12.0 * (spec.bin_hz(k as f32) / 440.0).log2() - tuning / 100.0;
                c[(midi.round() as i32).rem_euclid(12) as usize] += (1.0 + 100.0 * m[k]).ln();
            }
            let norm = c.iter().map(|v| v * v).sum::<f32>().sqrt();
            (c, norm)
        })
        .collect();
    let loudest = raw.iter().fold(0f32, |a, r| a.max(r.1));
    raw.into_iter().map(|(c, n)| (n > 0.05 * loudest && n > 0.0).then(|| c.map(|v| v / n))).collect()
}

/// Frame scores for the 37 states.
fn no_chord() -> f32 {
    std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(0.4)
}

fn scores(c: &Option<[f32; 12]>) -> [f32; 37] {
    let mut s = [0f32; 37];
    match c {
        None => s[N] = 1.0,
        Some(c) => {
            s[N] = no_chord();
            for (kind, (_, notes)) in KINDS.iter().enumerate() {
                for root in 0..12 {
                    // Pearson correlation: both centred, so a fuller template isn't favoured.
                    let mut t = [0f32; 12];
                    for n in notes.iter() {
                        t[(root + n) % 12] = 1.0;
                    }
                    let (cm, tm) = (c.iter().sum::<f32>() / 12.0, notes.len() as f32 / 12.0);
                    let (mut num, mut cc, mut tt) = (0f32, 0f32, 0f32);
                    for i in 0..12 {
                        num += (c[i] - cm) * (t[i] - tm);
                        cc += (c[i] - cm).powi(2);
                        tt += (t[i] - tm).powi(2);
                    }
                    s[kind * 12 + root] = if cc > 0.0 { num / (cc * tt).sqrt() } else { 0.0 };
                }
            }
        }
    }
    s
}

/// Best state path: the sum of scores, minus `penalty` per change.
fn viterbi(frames: &[[f32; 37]], penalty: f32) -> Vec<usize> {
    let n = frames.len();
    let mut back = vec![[0u8; 37]; n];
    let mut cur = frames.first().copied().unwrap_or([0.0; 37]);
    for t in 1..n {
        let (best, arg) = cur.iter().enumerate().fold((f32::MIN, 0), |b, (i, &v)| if v > b.0 { (v, i) } else { b });
        let mut next = [0f32; 37];
        for s in 0..37 {
            let (v, from) = if cur[s] >= best - penalty { (cur[s], s) } else { (best - penalty, arg) };
            next[s] = v + frames[t][s];
            back[t][s] = from as u8;
        }
        cur = next;
    }
    let mut s = cur.iter().enumerate().fold((f32::MIN, 0), |b, (i, &v)| if v > b.0 { (v, i) } else { b }).1;
    let mut path = vec![0; n];
    for t in (0..n).rev() {
        path[t] = s;
        s = back[t][s] as usize;
    }
    path
}

```
