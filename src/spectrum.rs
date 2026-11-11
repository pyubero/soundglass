//! The spectrogram: one FFT per pixel column of the edited audio, mixed to mono, drawn on a
//! logarithmic frequency axis from `LOW` Hz to half the sample rate.

use crate::edit::{self, Edit};
use crate::track::Track;
use eframe::egui;
use realfft::num_complex::Complex;
use realfft::RealToComplex;
use std::sync::Arc;

/// FFT length in frames: about 93 ms at 44.1 kHz, 11 Hz between bins.
const N: usize = 4096;
/// The lowest frequency shown, at the bottom.
pub const LOW: f32 = 20.0;
/// The quietest level shown, in dBFS; anything below is the darkest colour.
const FLOOR: f32 = -100.0;

pub struct Spectrogram {
    /// Width and height in pixels, first frame, frames shown, decoded frames, and the edit list.
    key: Option<(usize, usize, u64, u64, u64, Edit)>,
    texture: Option<egui::TextureHandle>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
}

impl Default for Spectrogram {
    fn default() -> Self {
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(N);
        let window = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
        Spectrogram { key: None, texture: None, input: fft.make_input_vec(), output: fft.make_output_vec(), fft, window }
    }
}

impl Spectrogram {
    /// The picture of edited frames `start..start + frames` at `width` × `height` pixels, made
    /// again only when one of those, the edits or the decoded audio change.
    // shortcut: about one FFT per column on the UI thread whenever the view changes (and every frame while decoding); move to a thread if zooming stutters.
    #[allow(clippy::too_many_arguments)]
    pub fn texture(&mut self, ctx: &egui::Context, track: &Track, edit: &Edit, start: u64, frames: u64, width: usize, height: usize) -> Option<&egui::TextureHandle> {
        let (width, height, frames, decoded) = (width.max(1), height.max(1), frames.max(1), track.decoded());
        let same = self.key.as_ref().is_some_and(|k| (k.0, k.1, k.2, k.3, k.4) == (width, height, start, frames, decoded) && k.5 == *edit);
        if !same {
            let rate = track.format.get()?.rate as f32;
            // Undecoded columns stay transparent.
            let mut pixels = vec![egui::Color32::TRANSPARENT; width * height];
            for c in 0..width {
                let centre = start + ((c as f64 + 0.5) * frames as f64 / width as f64) as u64;
                let Some(rows) = self.rows(track, edit, centre, height, rate) else { break };
                for (y, db) in rows.into_iter().enumerate() {
                    pixels[y * width + c] = colour(db);
                }
            }
            let image = egui::ColorImage::new([width, height], pixels);
            match &mut self.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("spectrogram", image, egui::TextureOptions::LINEAR)),
            }
            self.key = Some((width, height, start, frames, decoded, edit.clone()));
        }
        self.texture.as_ref()
    }

    /// The level in dBFS of each of `height` rows (top first) around edited frame `centre`,
    /// each the loudest FFT bin in its band; `None` where the audio isn't decoded yet.
    fn rows(&mut self, track: &Track, edit: &Edit, centre: u64, height: usize, rate: f32) -> Option<Vec<f32>> {
        let channels = edit.channels(track.format.get().map_or(1, |f| f.channels)).max(1);
        let (len, half) = (edit.len(), N as u64 / 2);
        let (a, b) = (centre.saturating_sub(half), (centre + half).min(len));
        // Zeros where the window runs past the track's start or end.
        self.input.fill(0.0);
        let mut i = (a + half - centre) as usize;
        let ok = edit::read(track, edit, a, b, |s| {
            for f in s.chunks_exact(channels) {
                self.input[i] = f.iter().sum::<f32>() / channels as f32 * self.window[i];
                i += 1;
            }
        });
        if !ok {
            return None;
        }
        self.fft.process(&mut self.input, &mut self.output).ok()?;
        // A full-scale sine comes out at 0 dB.
        let scale = 2.0 / self.window.iter().sum::<f32>();
        let nyquist = rate / 2.0;
        let bin = |f: f32| f / rate * N as f32;
        Some(
            (0..height)
                .map(|y| {
                    let freq = |y: f32| LOW * (nyquist / LOW).powf(1.0 - y / height as f32);
                    let (lo, hi) = (bin(freq(y as f32 + 1.0)).round() as usize, bin(freq(y as f32)).round() as usize);
                    let peak = self.output[lo.min(N / 2)..=hi.clamp(lo, N / 2)].iter().map(|c| c.norm()).fold(0.0, f32::max);
                    20.0 * (peak * scale).max(1e-10).log10()
                })
                .collect(),
        )
    }
}

/// From black through purple and orange to pale yellow as `db` goes from `FLOOR` to 0 dBFS.
fn colour(db: f32) -> egui::Color32 {
    const STOPS: [[f32; 3]; 5] = [[0.0, 0.0, 4.0], [80.0, 18.0, 123.0], [182.0, 54.0, 121.0], [251.0, 136.0, 97.0], [252.0, 253.0, 191.0]];
    let t = ((db - FLOOR) / -FLOOR).clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t as usize).min(STOPS.len() - 2);
    let k = t - i as f32;
    let c = |j: usize| (STOPS[i][j] + (STOPS[i + 1][j] - STOPS[i][j]) * k) as u8;
    egui::Color32::from_rgb(c(0), c(1), c(2))
}

/// Where frequency `freq` sits, from 0 at the top (half of `rate`) to 1 at the bottom (`LOW`).
pub fn height_of(freq: f32, rate: f32) -> f32 {
    1.0 - (freq / LOW).ln() / (rate / 2.0 / LOW).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_is_brightest_at_its_frequency_and_silence_is_the_floor() {
        let (rate, height) = (44100, 200);
        let sine: Vec<f32> = (0..44100).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate as f32).sin() * 0.5).collect();
        let track = Track::from_samples(rate, 1, sine);
        let edit = Edit::whole(44100);
        let mut s = Spectrogram::default();
        let rows = s.rows(&track, &edit, 22050, height, rate as f32).unwrap();
        let loudest = (0..height).max_by(|&a, &b| rows[a].total_cmp(&rows[b])).unwrap();
        let want = (height_of(1000.0, rate as f32) * height as f32) as usize;
        assert!(loudest.abs_diff(want) <= 1, "loudest row {loudest}, 1 kHz is row {want}");
        // Half scale is −6 dBFS.
        assert!((rows[loudest] + 6.0).abs() < 1.0, "{} dB", rows[loudest]);

        let silent = Track::from_samples(rate, 2, vec![0.0; 2 * 44100]);
        let rows = s.rows(&silent, &edit, 0, height, rate as f32).unwrap();
        assert!(rows.iter().all(|&db| colour(db) == colour(FLOOR)));
    }
}
