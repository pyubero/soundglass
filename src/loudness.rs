//! Integrated loudness (LUFS) per ITU-R BS.1770-4 / EBU R128, written here instead of adding a crate.

/// A biquad filter, transposed direct form II.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for `rate`: a high shelf (the head) and a high-pass. These are the
/// standard's analogue prototypes, so any sample rate works; at 48 kHz they give its table values.
fn k_weighting(rate: u32) -> [Biquad; 2] {
    let fs = rate as f64;
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, high_pass]
}

/// Feeds interleaved audio through K-weighting and keeps the energy of each 100 ms segment;
/// gating blocks of 400 ms are 4 segments with 75 % overlap.
pub struct Meter {
    channels: usize,
    filters: Vec<[Biquad; 2]>,
    /// Frames per segment, and how many are in the current one.
    segment: usize,
    filled: usize,
    /// Sum of squares per channel of the current segment.
    current: Vec<f64>,
    /// Channel-summed mean square of each finished segment.
    segments: Vec<f64>,
}

impl Meter {
    pub fn new(rate: u32, channels: usize) -> Meter {
        Meter {
            channels,
            filters: vec![k_weighting(rate); channels],
            segment: (rate as usize / 10).max(1),
            filled: 0,
            current: vec![0.0; channels],
            segments: Vec::new(),
        }
    }

    // shortcut: every channel weighs 1.0; BS.1770 gives surround channels 1.41 and skips LFE — add for 5.1 files.
    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.channels) {
            for ((s, f), sum) in frame.iter().zip(&mut self.filters).zip(&mut self.current) {
                let shelved = f[0].run(*s as f64);
                let y = f[1].run(shelved);
                *sum += y * y;
            }
            self.filled += 1;
            if self.filled == self.segment {
                self.segments.push(self.current.iter().sum::<f64>() / self.segment as f64);
                self.current.fill(0.0);
                self.filled = 0;
            }
        }
    }

    /// Integrated loudness in LUFS; −∞ for silence or less than 400 ms of audio.
    pub fn integrated(&self) -> f64 {
        let blocks: Vec<f64> = self.segments.windows(4).map(|w| w.iter().sum::<f64>() / 4.0).collect();
        let lufs = |power: f64| -0.691 + 10.0 * power.log10();
        let mean = |powers: &mut dyn Iterator<Item = f64>| {
            let (sum, n) = powers.fold((0.0, 0), |(s, n), p| (s + p, n + 1));
            (n > 0).then(|| sum / n as f64)
        };
        // Absolute gate at −70 LUFS, then a relative gate 10 LU below what passed it.
        let Some(absolute) = mean(&mut blocks.iter().copied().filter(|&p| lufs(p) > -70.0)) else { return f64::NEG_INFINITY };
        let relative = lufs(absolute) - 10.0;
        mean(&mut blocks.iter().copied().filter(|&p| lufs(p) > -70.0 && lufs(p) > relative)).map_or(f64::NEG_INFINITY, lufs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a 997 Hz sine at `db` dBFS, in the channels marked true.
    fn sine(rate: u32, seconds: f64, db: f64, on: &[bool]) -> Vec<f32> {
        let amp = 10f64.powf(db / 20.0);
        (0..(rate as f64 * seconds) as usize)
            .flat_map(|i| {
                let s = amp * (2.0 * std::f64::consts::PI * 997.0 * i as f64 / rate as f64).sin();
                on.iter().map(move |&c| if c { s as f32 } else { 0.0 })
            })
            .collect()
    }

    fn measure(rate: u32, channels: usize, samples: &[f32]) -> f64 {
        let mut m = Meter::new(rate, channels);
        m.push(samples);
        m.integrated()
    }

    #[test]
    fn reference_sine_in_one_channel_is_minus_3_01() {
        // BS.1770-4: a 0 dBFS 997 Hz sine in one channel reads −3.01 LUFS.
        for rate in [44100, 48000] {
            let lufs = measure(rate, 2, &sine(rate, 10.0, 0.0, &[true, false]));
            assert!((lufs + 3.01).abs() < 0.05, "{rate} Hz: {lufs}");
        }
    }

    #[test]
    fn stereo_sine_at_minus_20() {
        let lufs = measure(48000, 2, &sine(48000, 10.0, -20.0, &[true, true]));
        assert!((lufs + 20.0).abs() < 0.05, "{lufs}");
    }

    #[test]
    fn silence_and_short_clips_are_minus_infinity() {
        assert_eq!(measure(48000, 2, &vec![0.0; 96000 * 2]), f64::NEG_INFINITY);
        assert_eq!(measure(48000, 1, &sine(48000, 0.3, 0.0, &[true])), f64::NEG_INFINITY);
    }
}
