# Beat tracking

The second try, Beat This!, is what the Metronome uses (2026-11-13); the first try follows it.

## Second try: Beat This! (Local AI → Find Beats)

Beat This! (Foscarin, Schlüter, Widmer, ISMIR 2024; JKU Linz, MIT), checkpoint final0, as musetric's ONNX export (191 MB, checked by its author against torch on real music), with our own log-mel front end and the reference's "minimal" postprocessing (see `src/ai/beats.rs`). Runs on the original audio; the beats are mapped through the edits.

- songbird's `final0.onnx` (83 MB) was tried first and is not used: on a 30 s window of track 02 its beat logits were up to 13.8 off musetric's, and it found 161 beats on 02 against 359.
- Our log-mel matches a numpy rebuild of torchaudio's MelSpectrogram (slaney, `normalized="frame_length"`, `log1p(1000 x)`) within 6e-5.

Beats found, median tempo between neighbouring beats, the 5th–95th percentile of those tempos, the mean over the first and last fifth, and the run time (12-thread CPU):

- 01 St. Louis Blues: 1042 beats, 120.0 BPM, 115–125, 118.6 → 119.4, 32 s
- 02 Yellow Dog Blues: 359, 88.2, 86–91, 87.0 → 84.9, 17 s
- 03 Loveless Love: 680, 157.9, 143–158, 148.7 → 154.0, 19 s
- 04 Aunt Hagar's Blues: 368, 75.0, 73–79, 76.1 → 84.5, 19 s
- 05 Long Gone: 669, 142.9, 71–150, 137.3 → 144.7, 21 s
- 06 The Memphis Blues: 292, 100.0, 97–103, 98.8 → 105.5, 13 s
- 07 Beale Street Blues: 367, 75.0, 73–79, 74.7 → 75.3, 19 s
- 08 Ole Miss Blues: 649, 187.5, 176–200, 192.2 → 196.5, 13 s
- 09 Chantez Les Bas: 463, 96.8, 94–100, 97.2 → 98.2, 19 s
- 10 Hesitating Blues: 416, 78.9, 77–81, 77.6 → 79.0, 20 s
- 11 Atlanta Blues: 802, 187.5, 97–200, 200.1 → 191.8, 19 s
- 12 Interview (speech): no beats, 12 s
- 13 Lovelass Love: 712, 157.9, 81–167, 123.3 → 157.3, 23 s
- 14 Hesitating Blues: 419, 76.9, 75–81, 77.0 → 77.7, 22 s
- 15 Alligator Story (speech): no beats, 4 s
- 16 Long Gone: 457, 136.4, 70–143, 123.8 → 139.9, 30 s

Against the first try: the gaps between beats hold within about ±5 % of the tempo on most tracks (the first try swung ±35 %), and speech gets no beats instead of made-up ones. 08 and 11 come out at twice the first try's tempo (eighth notes, or a two-beat feel). Awaiting a check by ear: `cargo test --release -- --ignored beats_on_the_album` with `SG_CLICKS=dir` writes each track with clicks.

Ideas: the "dbn" postprocessing (madmom's DBN, 55–215 BPM, 3 or 4 beats a bar) keeps one tempo where the network is unsure, where "minimal" can click at random; the tempo octave could be offered as ½× / 2× as for BPM.

# The first try

The Metronome needs beat times, not just a tempo: a fixed grid (one offset, then every 60/BPM s) drifts off a live band within a minute or two. This note keeps the first attempt so the next one can start from it. Tried 2026-11-12 on the test album "1954-Louis Armstrong Plays W.C.Handy"; the verdict on listening was that the clicks were placed too badly to use, so the Metronome is postponed (see docs/DECISIONS.md).

## What was tried

The Ellis (2007) dynamic-programming beat tracker, as in librosa's `beat_track`, on bpm-key 0.1.0's onset envelope:

- Mono audio resampled to 22 050 Hz (`bpm_key::advanced::resample`), STFT 1024 / hop 256 (`advanced::stft`, the same as bpm-key's own tempo), onset envelope from `advanced::onset_envelope`: about 86 frames per second, 11.6 ms per frame.
- Tempo from `bpm_key::analyze` with `Config::human_tempo()` (centred on 100 BPM).
- Local score: the envelope divided by its standard deviation, smoothed with a Gaussian a 32nd of a beat period wide.
- Each frame's score is its local score plus the best earlier beat 0.5 to 2 periods back, minus `tightness · ln²(lag / period)` with tightness 100; then backtrack from the last strong score peak (at least half the median peak).
- Weak beats at both ends (local score under half the beats' RMS) are dropped; a beat's time is the centre of its STFT window, `(frame · 256 + 512) / 22 050` s.

## Results on the test album

Tempo, beats found, run time, and the tempo between neighbouring beats (lowest–highest, then the mean over the first and last fifth of the track):

- 01 St. Louis Blues: 118.8 BPM, 1042 beats, 1.9 s; 86–161, 119.1 → 120.0
- 02 Yellow Dog Blues: 87.2 BPM, 371 beats, 0.9 s; 64–126, 87.2 → 88.4
- 03 Loveless Love: 152.0 BPM, 678 beats, 1.0 s; 117–215, 149.0 → 154.4
- 04 Aunt Hagar's Blues: 75.4 BPM, 373 beats, 1.1 s; 59–108, 78.1 → 78.3
- 05 Long Gone: 143.5 BPM, 723 beats, 1.1 s; 99–191, 143.4 → 145.3
- 06 The Memphis Blues: 98.9 BPM, 292 beats, 0.6 s; 78–133, 101.0 → 99.8
- 07 Beale Street Blues: 75.7 BPM, 365 beats, 1.1 s; 62–108, 75.2 → 76.3
- 08 Ole Miss Blues: 93.1 BPM, 317 beats, 0.7 s; 72–123, 96.3 → 94.5
- 09 Chantez Les Bas: 98.4 BPM, 464 beats, 1.0 s; 69–129, 98.7 → 99.8
- 10 Hesitating Blues: 78.9 BPM, 416 beats, 1.2 s; 58–105, 78.2 → 79.9
- 11 Atlanta Blues: 96.3 BPM, 435 beats, 1.0 s; 67–126, 97.7 → 98.8
- 12 Interview (speech): 91.1 BPM, 249 beats, 0.6 s; 69–117, 94.6 → 92.9
- 13 Lovelass Love: 156.6 BPM, 907 beats, 1.2 s; 123–215, 155.4 → 159.0
- 14 Hesitating Blues: 78.3 BPM, 442 beats, 1.2 s; 59–112, 78.8 → 79.0
- 15 Alligator Story: 88.1 BPM, 67 beats, 0.2 s; 73–115, 90.9 → 92.8
- 16 Long Gone: 137.2 BPM, 1081 beats, 1.7 s; 105–185, 137.4 → 140.7

The averages follow the band's drift (03 speeds up from 149 to 154), but single gaps swing ±35 % around the tempo, and by ear the clicks were often off the beat (checked on 02, 06, 11 and 12).

## Why it may have failed, and what to try next

- The album is close to a worst case: 1950s recordings, brushed drums, the pulse carried by bass and banjo. An envelope over all bands up to 11 kHz is dominated by horns and vocals; try one weighted to the low end (under ~200 Hz) or a sum of band-wise envelopes.
- Tightness 100 lets the path jump between 0.5 and 2 periods; try a higher value, a narrower lag window (e.g. 0.8–1.25 periods), or a tightness from how steady the track is.
- The tempo itself can be an octave off; a wrong period makes the tracker click on every other beat or twice per beat.
- Speech and free-time passages (intros, breaks, stops) have no pulse: the tracker should leave gaps there rather than fill them.
- Check by ear, not only by numbers: render the track with clicks, as below.
- If our own attempts don't get there: a neural tracker (Beat This!, ISMIR 2024, ported to Rust in disctale-bpm-key-detector) is more accurate, but it's a new dependency with model weights.

## How it was checked

A scratch crate with `bpm-key = { version = "=0.1.0", default-features = false }`, reading mono f32 at 44 100 Hz from stdin and writing the track back with a 25 ms 1.5 kHz decaying click mixed in at each beat:

```sh
ffmpeg -v error -i TRACK.mp3 -f f32le -ac 1 -ar 44100 - | target/release/beats > out.raw
ffmpeg -f f32le -ar 44100 -ac 1 -i out.raw out-clicks.wav
```

## The code

```rust
use bpm_key::advanced::{onset_envelope, resample, stft};

const SR: u32 = 22_050;
const HOP: usize = 256;
const NFFT: usize = 1024;

/// Ellis (2007) dynamic-programming beat tracker; beat times in seconds.
fn beats(mono: &[f32], rate: u32, bpm: f32) -> Vec<f64> {
    let x = resample(mono, rate, SR);
    let env = onset_envelope(&stft(&x, SR as f32, NFFT, HOP));
    let fps = SR as f32 / HOP as f32;
    let period = 60.0 * fps / bpm;
    let n = env.len();
    if n < 4 * period as usize {
        return Vec::new();
    }
    let mean = env.iter().sum::<f32>() / n as f32;
    let std = (env.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt();
    if !(std > 0.0) {
        return Vec::new();
    }
    // Onsets smoothed by a Gaussian a 32nd of a period wide.
    let half = period.round() as isize;
    let kernel: Vec<f32> = (-half..=half).map(|t| (-0.5 * (t as f32 * 32.0 / period).powi(2)).exp()).collect();
    let local: Vec<f32> = (0..n as isize)
        .map(|i| kernel.iter().enumerate().map(|(k, w)| {
            let j = i + k as isize - half;
            if j >= 0 && (j as usize) < n { w * env[j as usize] / std } else { 0.0 }
        }).sum())
        .collect();
    let tightness = 100.0;
    let (lo, hi) = ((period / 2.0).round() as usize, (2.0 * period).round() as usize);
    let mut score = vec![0f32; n];
    let mut back = vec![usize::MAX; n];
    for i in 0..n {
        let mut best = (0f32, usize::MAX);
        for lag in lo..=hi.min(i) {
            let p = i - lag;
            let cost = -tightness * (lag as f32 / period).ln().powi(2);
            let s = score[p] + cost;
            if best.1 == usize::MAX || s > best.0 { best = (s, p); }
        }
        score[i] = local[i] + if best.1 == usize::MAX { 0.0 } else { best.0 };
        back[i] = best.1;
    }
    // The last beat: the last local maximum of the score above half the median of them.
    let peaks: Vec<usize> = (1..n - 1).filter(|&i| score[i] > score[i - 1] && score[i] >= score[i + 1]).collect();
    let mut sorted: Vec<f32> = peaks.iter().map(|&i| score[i]).collect();
    sorted.sort_by(f32::total_cmp);
    let Some(&median) = sorted.get(sorted.len() / 2) else { return Vec::new() };
    let Some(&last) = peaks.iter().rev().find(|&&i| score[i] >= 0.5 * median) else { return Vec::new() };
    let mut out = vec![last];
    while let Some(&b) = out.last() {
        if back[b] == usize::MAX { break; }
        out.push(back[b]);
    }
    out.reverse();
    // Drop weak beats at both ends (silence before and after the music).
    let rms = (out.iter().map(|&b| local[b].powi(2)).sum::<f32>() / out.len() as f32).sqrt();
    let strong = |b: &&usize| local[**b] >= 0.5 * rms;
    let first = out.iter().position(|b| strong(&b)).unwrap_or(0);
    let end = out.len() - out.iter().rev().position(|b| strong(&b)).unwrap_or(0);
    // Frame t's onset sits at the centre of window t.
    out[first..end].iter().map(|&b| (b * HOP + NFFT / 2) as f64 / SR as f64).collect()
}
```
