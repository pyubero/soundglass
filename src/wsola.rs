//! Time-stretching by WSOLA: overlapping windows of the input laid down at a new spacing, each one
//! nudged (within ±10 ms) to where it best continues the last, then crossfaded. Changes the speed
//! without the pitch; pitch changes are a stretch plus the resampler. Streams, and allocates only
//! in `new`, so it can run in the audio callback.

/// Window length in seconds; the synthesis hop is half of it.
const WINDOW: f64 = 0.04;
/// How far each window may move to line up, in seconds.
const TOLERANCE: f64 = 0.01;
/// Buffers are sized for this sample rate; above it the window is just shorter in time.
const MAX_RATE: f64 = 192_000.0;
/// The stretch ratio is held within 1/MAX_TEMPO to MAX_TEMPO.
const MAX_TEMPO: f64 = 4.0;

/// The three Playback settings: tape speed (pitch moves with it), pitch-preserved speed, and
/// time-preserved pitch in semitones. They combine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    pub tape: f64,
    pub tempo: f64,
    pub semitones: f64,
}

impl Speed {
    pub const NORMAL: Speed = Speed { tape: 1.0, tempo: 1.0, semitones: 0.0 };

    /// How much faster the stretched audio is played: tape speed times the pitch ratio.
    pub fn rate(&self) -> f64 {
        self.tape * 2f64.powf(self.semitones / 12.0)
    }

    /// How fast WSOLA moves through the input: the pitch ratio is undone here, so pitch alone keeps the length.
    pub fn stretch(&self) -> f64 {
        self.tempo / 2f64.powf(self.semitones / 12.0)
    }

    /// Whether WSOLA has any work to do.
    pub fn stretches(&self) -> bool {
        (self.stretch() - 1.0).abs() > 1e-9
    }

    /// How fast the original moves under playback.
    pub fn source(&self) -> f64 {
        self.tape * self.tempo
    }
}

pub struct Wsola {
    channels: usize,
    /// Window length and hop in frames, for the current rate.
    n: usize,
    hop: usize,
    tol: usize,
    tempo: f64,
    window: Vec<f32>,
    /// Input frames from absolute frame `base`, `len` of them.
    buf: Vec<f32>,
    base: u64,
    len: usize,
    /// Where the next window is taken from, ideally (absolute input frame).
    ana: f64,
    /// Where the last window was actually taken from.
    prev: Option<u64>,
    /// Overlap-add output; its first `ready` frames are finished, `sent` of them handed out.
    ola: Vec<f32>,
    ready: usize,
    sent: usize,
    /// The search's scratch: the channels' sum over the searched frames, and its running energy.
    mono: Vec<f32>,
    energy: Vec<f64>,
}

impl Wsola {
    pub fn new(channels: usize, rate: u32) -> Wsola {
        let max_n = (MAX_RATE * WINDOW) as usize;
        let mut w = Wsola {
            channels,
            n: 0,
            hop: 0,
            tol: 0,
            tempo: 1.0,
            window: vec![0.0; max_n],
            // Enough for the widest span one step needs: a 4× jump plus the search on both sides.
            buf: vec![0.0; 6 * max_n * channels],
            base: 0,
            len: 0,
            ana: 0.0,
            prev: None,
            ola: vec![0.0; max_n * channels],
            ready: 0,
            sent: 0,
            mono: vec![0.0; 6 * max_n],
            energy: vec![0.0; 6 * max_n + 1],
        };
        w.set_rate(rate);
        w
    }

    /// Sizes the window for `rate` and starts afresh.
    pub fn set_rate(&mut self, rate: u32) {
        let max_n = self.window.len();
        self.n = (((rate as f64 * WINDOW) as usize) & !1).clamp(16, max_n);
        self.hop = self.n / 2;
        self.tol = ((rate as f64 * TOLERANCE) as usize).min(self.hop);
        // Periodic Hann: two of them half a window apart sum to exactly 1.
        for (k, w) in self.window[..self.n].iter_mut().enumerate() {
            *w = (0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / self.n as f64).cos()) as f32;
        }
        self.reset();
    }

    /// Input frames per output frame.
    pub fn set_tempo(&mut self, tempo: f64) {
        self.tempo = tempo.clamp(1.0 / MAX_TEMPO, MAX_TEMPO);
    }

    /// Forgets the input, for a seek.
    pub fn reset(&mut self) {
        (self.base, self.len, self.ana, self.prev, self.ready, self.sent) = (0, 0, 0.0, None, 0, 0);
        self.ola.fill(0.0);
    }

    /// Fills `out` (interleaved) with stretched audio, asking `source` to fill buffers with input.
    /// False when `source` has nothing (returned false) before any output; a later end pads with
    /// silence. shortcut: the last ~50 ms still in the buffers are dropped at the end, fine for playback.
    pub fn process(&mut self, out: &mut [f32], mut source: impl FnMut(&mut [f32]) -> bool) -> bool {
        let (ch, frames) = (self.channels, out.len() / self.channels);
        let mut i = 0;
        while i < frames {
            if self.sent == self.ready && !self.step(&mut source) {
                if i == 0 {
                    return false;
                }
                out[i * ch..].fill(0.0);
                return true;
            }
            let k = (self.ready - self.sent).min(frames - i);
            out[i * ch..(i + k) * ch].copy_from_slice(&self.ola[self.sent * ch..(self.sent + k) * ch]);
            (self.sent, i) = (self.sent + k, i + k);
        }
        true
    }

    /// Adds one window to the output.
    fn step(&mut self, source: &mut impl FnMut(&mut [f32]) -> bool) -> bool {
        let (ch, n, hop, tol) = (self.channels, self.n, self.hop, self.tol as u64);
        let ana = self.ana.round() as u64;
        let natural = self.prev.map(|p| p + hop as u64);
        if !self.fill((ana + tol).max(natural.unwrap_or(0)) + n as u64, source) {
            return false;
        }
        let start = match natural {
            Some(natural) => self.best(natural, ana.saturating_sub(tol).max(self.base), ana + tol),
            None => ana,
        };
        if self.ready > 0 {
            self.ola.copy_within(hop * ch..n * ch, 0);
            self.ola[(n - hop) * ch..n * ch].fill(0.0);
        }
        let off = (start - self.base) as usize;
        for (k, &w) in self.window[..n].iter().enumerate() {
            for c in 0..ch {
                self.ola[k * ch + c] += w * self.buf[(off + k) * ch + c];
            }
        }
        (self.prev, self.ready, self.sent) = (Some(start), hop, 0);
        self.ana += hop as f64 * self.tempo;
        // Keep from the earliest frame the next step can look at.
        let keep = (self.ana.round() as u64).saturating_sub(tol).min(start + hop as u64);
        let drop = (keep.saturating_sub(self.base) as usize).min(self.len);
        self.buf.copy_within(drop * ch..self.len * ch, 0);
        (self.base, self.len) = (self.base + drop as u64, self.len - drop);
        true
    }

    /// Reads input until frame `end` (absolute) is buffered.
    fn fill(&mut self, end: u64, source: &mut impl FnMut(&mut [f32]) -> bool) -> bool {
        let ch = self.channels;
        let want = (end.saturating_sub(self.base + self.len as u64) as usize).min(self.buf.len() / ch - self.len);
        if want > 0 {
            if !source(&mut self.buf[self.len * ch..(self.len + want) * ch]) {
                return false;
            }
            self.len += want;
        }
        true
    }

    /// The start in `lo..=hi` whose first half best matches the half window from `natural`,
    /// by normalised cross-correlation of the channels' sum: every 4th start, then around the best.
    fn best(&mut self, natural: u64, lo: u64, hi: u64) -> u64 {
        let (ch, half) = (self.channels, self.hop);
        let from = lo.min(natural);
        let count = (hi.max(natural) + half as u64 - from) as usize;
        let first = (from - self.base) as usize;
        for (k, m) in self.mono[..count].iter_mut().enumerate() {
            *m = self.buf[(first + k) * ch..][..ch].iter().sum();
        }
        for k in 0..count {
            self.energy[k + 1] = self.energy[k] + (self.mono[k] as f64).powi(2);
        }
        let (mono, energy) = (&self.mono[..count], &self.energy[..=count]);
        let target = &mono[(natural - from) as usize..][..half];
        let score = |s: u64| {
            let i = (s - from) as usize;
            dot(&mono[i..i + half], target) / ((energy[i + half] - energy[i]).max(0.0) as f32 + 1e-9).sqrt()
        };
        let pick = |range: &mut dyn Iterator<Item = u64>| range.map(|s| (s, score(s))).fold((lo, f32::MIN), |b, c| if c.1 > b.1 { c } else { b }).0;
        let coarse = pick(&mut (lo..=hi).step_by(4));
        pick(&mut (coarse.saturating_sub(3).max(lo)..=(coarse + 3).min(hi)))
    }
}

/// Eight lanes at a time, so it vectorises.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum::<f32>() + tail
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 8000;

    fn sine(freq: f64, frames: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| {
            let x = (std::f64::consts::TAU * freq * i as f64 / RATE as f64).sin() as f32 * 0.5;
            [x, x]
        }).collect()
    }

    /// Stretches `input` (stereo) at `tempo`, reading until it runs out.
    fn stretch(input: &[f32], tempo: f64) -> Vec<f32> {
        let mut w = Wsola::new(2, RATE);
        w.set_tempo(tempo);
        let mut pos = 0;
        let mut out = Vec::new();
        let mut block = vec![0.0; 512];
        while w.process(&mut block, |dst| {
            if pos + dst.len() > input.len() {
                return false;
            }
            dst.copy_from_slice(&input[pos..pos + dst.len()]);
            pos += dst.len();
            true
        }) {
            out.extend_from_slice(&block);
        }
        out
    }

    /// Frequency from upward zero crossings of the left channel, skipping the edges.
    fn freq(stereo: &[f32]) -> f64 {
        let left: Vec<f32> = stereo.chunks(2).map(|f| f[0]).collect();
        let mid = &left[left.len() / 10..left.len() * 9 / 10];
        let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f64 * RATE as f64 / mid.len() as f64
    }

    #[test]
    fn speed_keeps_the_pitch() {
        let input = sine(440.0, RATE as usize * 4);
        for tempo in [0.5, 0.75, 1.5, 2.0] {
            let out = stretch(&input, tempo);
            let expected = input.len() as f64 / tempo;
            assert!((out.len() as f64 / expected - 1.0).abs() < 0.05, "tempo {tempo}: {} vs {expected}", out.len());
            let f = freq(&out);
            assert!((f - 440.0).abs() < 5.0, "tempo {tempo}: {f} Hz");
        }
    }

    #[test]
    fn a_steady_sine_stays_smooth() {
        // Well-aligned windows add up to the same amplitude, without dips at the seams.
        let out = stretch(&sine(440.0, RATE as usize * 2), 0.6);
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        for block in left[RATE as usize / 4..left.len() - RATE as usize / 4].chunks(40) {
            let peak = block.iter().fold(0f32, |p, x| p.max(x.abs()));
            assert!(peak > 0.45 && peak < 0.55, "peak {peak}");
        }
    }

    #[test]
    fn speed_settings_combine() {
        let up = Speed { tape: 1.0, tempo: 1.0, semitones: 12.0 };
        assert!((up.rate() - 2.0).abs() < 1e-12 && (up.stretch() - 0.5).abs() < 1e-12);
        assert!(!Speed { tape: 1.5, ..Speed::NORMAL }.stretches());
        assert!((Speed { tape: 1.5, tempo: 0.5, semitones: 0.0 }.source() - 0.75).abs() < 1e-12);
    }
}
