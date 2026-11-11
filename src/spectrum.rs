//! The spectrogram: a short-time Fourier transform of the edited audio, mixed to mono, every
//! `hop` frames, each pixel column the loudest of the frames inside it, drawn on a logarithmic
//! frequency axis from `Params::low` to half the sample rate. Made on a worker thread.

use crate::edit::{self, Edit};
use crate::track::Track;
use eframe::egui;
use realfft::num_complex::Complex;
use realfft::RealToComplex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// The window's shape.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    Hann,
    Hamming,
    BlackmanHarris,
    Rectangular,
}

impl Shape {
    pub const ALL: [Shape; 4] = [Shape::Hann, Shape::Hamming, Shape::BlackmanHarris, Shape::Rectangular];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Hann => "Hann",
            Shape::Hamming => "Hamming",
            Shape::BlackmanHarris => "Blackman–Harris",
            Shape::Rectangular => "Rectangular",
        }
    }

    pub fn from_name(name: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The window's `n` weights.
    fn table(self, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / n as f32;
                match self {
                    Shape::Hann => 0.5 - 0.5 * x.cos(),
                    Shape::Hamming => 0.54 - 0.46 * x.cos(),
                    Shape::BlackmanHarris => 0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos(),
                    Shape::Rectangular => 1.0,
                }
            })
            .collect()
    }
}

/// The colours from quiet to loud.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColourMap {
    Magma,
    Gray,
    BlueRed,
}

impl ColourMap {
    pub const ALL: [ColourMap; 3] = [ColourMap::Magma, ColourMap::Gray, ColourMap::BlueRed];

    pub fn name(self) -> &'static str {
        match self {
            ColourMap::Magma => "Magma",
            ColourMap::Gray => "Gray",
            ColourMap::BlueRed => "Blue to red",
        }
    }

    pub fn from_name(name: &str) -> Option<ColourMap> {
        ColourMap::ALL.into_iter().find(|m| m.name() == name)
    }

    fn stops(self) -> &'static [[f32; 3]] {
        match self {
            ColourMap::Magma => &[[0.0, 0.0, 4.0], [80.0, 18.0, 123.0], [182.0, 54.0, 121.0], [251.0, 136.0, 97.0], [252.0, 253.0, 191.0]],
            ColourMap::Gray => &[[0.0, 0.0, 0.0], [255.0, 255.0, 255.0]],
            ColourMap::BlueRed => &[[0.0, 0.0, 40.0], [0.0, 60.0, 200.0], [0.0, 200.0, 200.0], [240.0, 230.0, 0.0], [220.0, 20.0, 0.0]],
        }
    }
}

/// How the spectrogram is analysed and coloured; kept in settings.txt.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Params {
    /// FFT length and window length in frames (the window zero-padded to the FFT), and the step between windows.
    pub fft: usize,
    pub window: usize,
    pub hop: usize,
    pub shape: Shape,
    /// The quietest and loudest levels shown, in dBFS.
    pub floor: f32,
    pub ceiling: f32,
    /// The lowest frequency shown, at the bottom.
    pub low: f32,
    pub map: ColourMap,
}

impl Default for Params {
    fn default() -> Self {
        Params { fft: 4096, window: 4096, hop: 512, shape: Shape::Hann, floor: -100.0, ceiling: 0.0, low: 20.0, map: ColourMap::Magma }
    }
}

impl Params {
    /// The same with each value made valid: powers of two in range, the window no longer than the
    /// FFT, the ceiling at least 10 dB above the floor.
    pub fn checked(self) -> Params {
        let fft = self.fft.clamp(512, 32768).next_power_of_two();
        let window = self.window.clamp(256, fft).next_power_of_two().min(fft);
        let floor = if self.floor.is_finite() { self.floor.clamp(-160.0, -20.0) } else { -100.0 };
        let ceiling = if self.ceiling.is_finite() { self.ceiling.clamp(-60.0, 6.0).max(floor + 10.0) } else { 0.0 };
        let low = if self.low.is_finite() { self.low.clamp(10.0, 200.0) } else { 20.0 };
        Params { fft, window, hop: self.hop.clamp(32, 8192), floor, ceiling, low, ..self }
    }
}

/// One FFT's worth of state for a set of `Params`.
struct Analyser {
    params: Params,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
}

impl Analyser {
    fn new(params: Params) -> Analyser {
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(params.fft);
        Analyser { params, window: params.shape.table(params.window), input: fft.make_input_vec(), output: fft.make_output_vec(), fft }
    }

    /// The level in dBFS of each of `height` rows (top first) of `band` around edited frame
    /// `centre`, each the loudest FFT bin in its range; `None` where the audio isn't decoded yet.
    fn rows(&mut self, track: &Track, edit: &Edit, centre: u64, height: usize, band: [f32; 2]) -> Option<Vec<f32>> {
        let format = track.format.get()?;
        let (rate, channels) = (format.rate as f32, edit.channels(format.channels).max(1));
        let (n, half) = (self.params.window, self.params.window as u64 / 2);
        let (a, b) = (centre.saturating_sub(half), (centre + half).min(edit.len()));
        // Zeros where the window runs past the track's start or end, and after it up to the FFT's length.
        self.input.fill(0.0);
        let mut i = (a + half - centre) as usize;
        let ok = edit::read(track, edit, a, b, |s| {
            for f in s.chunks_exact(channels) {
                if i < n {
                    self.input[i] = f.iter().sum::<f32>() / channels as f32 * self.window[i];
                }
                i += 1;
            }
        });
        if !ok {
            return None;
        }
        self.fft.process(&mut self.input, &mut self.output).ok()?;
        // A full-scale sine comes out at 0 dB.
        let scale = 2.0 / self.window.iter().sum::<f32>();
        let (fft, low, top) = (self.params.fft, self.params.low, self.output.len() - 1);
        let bin = |f: f32| (f / rate * fft as f32).round() as usize;
        let [lo, hi] = band;
        let freq = |y: f32| freq_at(lo + (hi - lo) * (1.0 - y / height as f32), rate, low);
        Some(
            (0..height)
                .map(|y| {
                    let (from, to) = (bin(freq(y as f32 + 1.0)).min(top), bin(freq(y as f32)).min(top));
                    let peak = self.output[from..=to.max(from)].iter().map(|c| c.norm()).fold(0.0, f32::max);
                    20.0 * (peak * scale).max(1e-10).log10()
                })
                .collect(),
        )
    }
}

/// What a picture is made from: its size in pixels, the edited frames and band it shows, the
/// analysis, how much was decoded, and the edits.
#[derive(Clone, PartialEq)]
struct Key {
    width: usize,
    height: usize,
    start: u64,
    frames: u64,
    band: [f32; 2],
    params: Params,
    decoded: u64,
    edit: Arc<Edit>,
}

/// The picture for `key`: column by column, the loudest of the hops inside each (the nearest
/// hop when none is). `None` once `cancel` is set; columns from the first undecoded one on stay
/// transparent. Counts finished columns in `done`.
fn render(track: &Track, key: &Key, cancel: &AtomicBool, done: &AtomicUsize) -> Option<egui::ColorImage> {
    let (w, h, hop) = (key.width, key.height, key.params.hop as u64);
    let mut analyser = Analyser::new(key.params);
    let mut pixels = vec![egui::Color32::TRANSPARENT; w * h];
    // The last hop computed, reused while zoomed in past the hop.
    let mut last: Option<(u64, Vec<f32>)> = None;
    'columns: for c in 0..w {
        if cancel.load(Relaxed) {
            return None;
        }
        let edge = |c: usize| key.start + (c as f64 * key.frames as f64 / w as f64) as u64;
        let (a, b) = (edge(c), edge(c + 1).max(edge(c) + 1));
        let (first, end) = (a.div_ceil(hop), b.div_ceil(hop));
        let hops = if first < end { first..end } else { let k = ((a + b) / 2 + hop / 2) / hop; k..k + 1 };
        let mut loudest: Option<Vec<f32>> = None;
        for k in hops {
            let rows = match &last {
                Some((at, rows)) if *at == k => rows.clone(),
                _ => {
                    let Some(rows) = analyser.rows(track, &key.edit, k * hop, h, key.band) else { break 'columns };
                    last = Some((k, rows.clone()));
                    rows
                }
            };
            match &mut loudest {
                Some(l) => l.iter_mut().zip(&rows).for_each(|(l, r)| *l = l.max(*r)),
                None => loudest = Some(rows),
            }
        }
        for (y, db) in loudest.unwrap_or_default().into_iter().enumerate() {
            pixels[y * w + c] = colour(db, &key.params);
        }
        done.store(c + 1, Relaxed);
    }
    Some(egui::ColorImage::new([w, h], pixels))
}

/// A picture being made on the worker thread.
struct Job {
    key: Key,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    rx: Receiver<Option<egui::ColorImage>>,
}

/// A picture ready to draw, and what it shows (which may be an older view than asked for).
pub struct Shown<'a> {
    pub texture: &'a egui::TextureHandle,
    pub start: u64,
    pub frames: u64,
    pub band: [f32; 2],
    pub low: f32,
}

pub struct Spectrogram {
    /// The part of the frequency axis shown, bottom and top, on the log scale where 0 is
    /// `Params::low` and 1 half the sample rate.
    pub band: [f32; 2],
    shown: Option<Key>,
    texture: Option<egui::TextureHandle>,
    job: Option<Job>,
}

impl Spectrogram {
    pub fn new() -> Spectrogram {
        Spectrogram { band: [0.0, 1.0], shown: None, texture: None, job: None }
    }

    /// The picture of edited frames `start..start + frames` at `width` × `height` pixels: the
    /// latest one finished, while a worker makes the one asked for if that differs. Also the
    /// worker's progress (0 to 1) while it runs.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, ctx: &egui::Context, track: &Arc<Track>, edit: &Arc<Edit>, start: u64, frames: u64, width: usize, height: usize, params: Params) -> (Option<Shown<'_>>, Option<f32>) {
        let want = Key { width: width.max(1), height: height.max(1), start, frames: frames.max(1), band: self.band, params, decoded: track.decoded(), edit: edit.clone() };
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(image) => {
                    if let Some(image) = image {
                        match &mut self.texture {
                            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                            None => self.texture = Some(ctx.load_texture("spectrogram", image, egui::TextureOptions::LINEAR)),
                        }
                    }
                    self.shown = Some(job.key.clone());
                    self.job = None;
                }
                // The worker died: keep the old picture and don't try that view again.
                Err(TryRecvError::Disconnected) => self.shown = self.job.take().map(|j| j.key.clone()),
                Err(TryRecvError::Empty) => {
                    // Only more decoded audio: let it finish, the next one picks that up.
                    if (Key { decoded: want.decoded, ..job.key.clone() }) != want {
                        self.job = None;
                    }
                }
            }
        }
        if self.job.is_none() && self.shown.as_ref() != Some(&want) && track.format.get().is_some() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
            let (track, key, ctx) = (track.clone(), want.clone(), ctx.clone());
            let (c, d) = (cancel.clone(), done.clone());
            std::thread::spawn(move || {
                let image = render(&track, &key, &c, &d);
                if !c.load(Relaxed) {
                    let _ = tx.send(image);
                    ctx.request_repaint();
                }
            });
            self.job = Some(Job { key: want, cancel, done, rx });
        }
        let progress = self.job.as_ref().map(|j| j.done.load(Relaxed) as f32 / j.key.width as f32);
        let shown = self.texture.as_ref().zip(self.shown.as_ref()).map(|(texture, k)| Shown { texture, start: k.start, frames: k.frames, band: k.band, low: k.params.low });
        (shown, progress)
    }
}

/// A job dropped (no longer wanted, or the window gone) stops at its next column.
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
    }
}

/// From `floor` to `ceiling` (dBFS) through the colour map's stops.
fn colour(db: f32, params: &Params) -> egui::Color32 {
    let stops = params.map.stops();
    let t = ((db - params.floor) / (params.ceiling - params.floor)).clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (t as usize).min(stops.len() - 2);
    let k = t - i as f32;
    let c = |j: usize| (stops[i][j] + (stops[i + 1][j] - stops[i][j]) * k) as u8;
    egui::Color32::from_rgb(c(0), c(1), c(2))
}

/// The frequency at `at` on the whole axis (0 is `low`, 1 half of `rate`).
pub fn freq_at(at: f32, rate: f32, low: f32) -> f32 {
    low * (rate / 2.0 / low).powf(at)
}

/// Where frequency `freq` sits within `band` of an axis from `low`, from 0 at its top to 1 at
/// its bottom (outside: beyond those).
pub fn height_of(freq: f32, rate: f32, [lo, hi]: [f32; 2], low: f32) -> f32 {
    let at = (freq / low).ln() / (rate / 2.0 / low).ln();
    1.0 - (at - lo) / (hi - lo)
}

/// The narrowest band: about a third of an octave at 44.1 kHz.
const MIN_BAND: f32 = 0.03;

/// `band` zoomed in by `factor` (2 = twice as close) keeping point `at` (0 bottom, 1 top of the
/// band, as on screen) in place, within the whole axis.
pub fn zoom_band([lo, hi]: [f32; 2], factor: f32, at: f32) -> [f32; 2] {
    let width = ((hi - lo) / factor).clamp(MIN_BAND, 1.0);
    let fixed = lo + at * (hi - lo);
    let lo = (fixed - at * width).clamp(0.0, 1.0 - width);
    [lo, lo + width]
}

/// `band` moved up by `delta` of its own width, stopping at the ends of the axis.
pub fn pan_band([lo, hi]: [f32; 2], delta: f32) -> [f32; 2] {
    let width = hi - lo;
    let lo = (lo + delta * width).clamp(0.0, 1.0 - width);
    [lo, lo + width]
}

/// The notes of equal temperament tuned to `a4` from `low` Hz up to `nyquist`, lowest first,
/// as (frequency, pitch class with C = 0, octave); `naturals` keeps only the white keys.
pub fn notes(a4: f32, low: f32, nyquist: f32, naturals: bool) -> Vec<(f32, u8, i32)> {
    (0..=135)
        .map(|midi: i32| (a4 * 2f32.powf((midi - 69) as f32 / 12.0), midi.rem_euclid(12) as u8, midi / 12 - 1))
        .filter(|&(f, pc, _)| (low..=nyquist).contains(&f) && !(naturals && [1, 3, 6, 8, 10].contains(&pc)))
        .collect()
}

/// A note's name: "C#4", "Db4", "Do#4", "Réb4"… with `#` and `b`, as the bundled font has no ♯ or ♭.
pub fn note_name(pc: u8, octave: i32, solfege: bool, flats: bool, octaves: bool) -> String {
    const LETTERS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];
    const SOLFEGE: [&str; 7] = ["Do", "Ré", "Mi", "Fa", "Sol", "La", "Si"];
    // The natural note below (sharps) or above (flats) each pitch class, and its accidental.
    const SHARP: [(usize, &str); 12] = [(0, ""), (0, "#"), (1, ""), (1, "#"), (2, ""), (3, ""), (3, "#"), (4, ""), (4, "#"), (5, ""), (5, "#"), (6, "")];
    const FLAT: [(usize, &str); 12] = [(0, ""), (1, "b"), (1, ""), (2, "b"), (2, ""), (3, ""), (4, "b"), (4, ""), (5, "b"), (5, ""), (6, "b"), (6, "")];
    let (natural, mark) = if flats { FLAT } else { SHARP }[pc as usize % 12];
    let base = if solfege { SOLFEGE } else { LETTERS }[natural];
    if octaves { format!("{base}{mark}{octave}") } else { format!("{base}{mark}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_zooms_around_the_pointer_and_stays_on_the_axis() {
        let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        assert!(close(zoom_band([0.0, 1.0], 2.0, 0.5), [0.25, 0.75]));
        assert!(close(zoom_band([0.0, 1.0], 2.0, 0.0), [0.0, 0.5]));
        // The point under the pointer stays: 0.6 of the way up [0.2, 0.7] is 0.5.
        let z = zoom_band([0.2, 0.7], 4.0, 0.6);
        assert!((z[0] + 0.6 * (z[1] - z[0]) - 0.5).abs() < 1e-6);
        assert!(close(zoom_band([0.25, 0.75], 0.1, 0.5), [0.0, 1.0]));
        assert!((zoom_band([0.0, 1.0], 1000.0, 0.5)[1] - zoom_band([0.0, 1.0], 1000.0, 0.5)[0] - MIN_BAND).abs() < 1e-6);
        assert!(close(pan_band([0.5, 0.75], 10.0), [0.75, 1.0]));
        assert!(close(pan_band([0.5, 0.75], -10.0), [0.0, 0.25]));
        assert!(close(pan_band([0.0, 1.0], 0.3), [0.0, 1.0]));
    }

    #[test]
    fn notes_follow_the_tuning_and_are_named() {
        let all = notes(440.0, 20.0, 22050.0, false);
        let find = |n: &[(f32, u8, i32)], pc, oct| n.iter().find(|x| x.1 == pc && x.2 == oct).unwrap().0;
        assert!((find(&all, 9, 4) - 440.0).abs() < 1e-3);
        assert!((find(&all, 0, 4) - 261.626).abs() < 0.01);
        assert!((find(&notes(432.0, 20.0, 22050.0, false), 0, 4) - 256.87).abs() < 0.01);
        assert!(all.first().unwrap().0 >= 20.0 && all.last().unwrap().0 <= 22050.0);
        assert!(notes(440.0, 100.0, 22050.0, false).first().unwrap().0 >= 100.0);
        let octave4 = |n: Vec<(f32, u8, i32)>| n.iter().filter(|x| x.2 == 4).count();
        assert_eq!((octave4(all.clone()), octave4(notes(440.0, 20.0, 22050.0, true))), (12, 7));
        assert_eq!(note_name(1, 4, false, false, true), "C#4");
        assert_eq!(note_name(1, 4, false, true, true), "Db4");
        assert_eq!(note_name(1, 4, true, false, false), "Do#");
        assert_eq!(note_name(3, 2, true, true, true), "Mib2");
        assert_eq!(note_name(11, 0, true, false, true), "Si0");
    }

    fn sine(rate: u32, freq: f32, amp: f32) -> Arc<Track> {
        Track::from_samples(rate, 1, (0..rate).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin() * amp).collect())
    }

    #[test]
    fn a_sine_is_brightest_at_its_frequency_and_silence_is_the_floor() {
        let (rate, height) = (44100, 200);
        let (track, edit) = (sine(rate, 1000.0, 0.5), Edit::whole(rate as u64));
        let loudest = |rows: &[f32]| (0..rows.len()).max_by(|&a, &b| rows[a].total_cmp(&rows[b])).unwrap();
        for shape in Shape::ALL {
            for (fft, window) in [(4096, 4096), (8192, 2048)] {
                let params = Params { fft, window, shape, ..Params::default() };
                let mut a = Analyser::new(params);
                for band in [[0.0, 1.0], [0.5, 1.0]] {
                    let rows = a.rows(&track, &edit, 22050, height, band).unwrap();
                    let (got, want) = (loudest(&rows), (height_of(1000.0, rate as f32, band, params.low) * height as f32) as usize);
                    assert!(got.abs_diff(want) <= 1, "{shape:?} {fft}/{window} {band:?}: loudest row {got}, 1 kHz is row {want}");
                    // Half scale is −6 dBFS.
                    assert!((rows[got] + 6.0).abs() < 1.5, "{shape:?} {fft}/{window}: {} dB", rows[got]);
                }
            }
        }
        let silent = Track::from_samples(rate, 2, vec![0.0; 2 * rate as usize]);
        let p = Params::default();
        let rows = Analyser::new(p).rows(&silent, &edit, 0, height, [0.0, 1.0]).unwrap();
        assert!(rows.iter().all(|&db| colour(db, &p) == colour(p.floor, &p)));
    }

    #[test]
    fn each_column_is_the_loudest_of_its_hops() {
        // One second of silence with a click in the third quarter: only that column lights up.
        let rate = 8000;
        let mut samples = vec![0.0; rate];
        samples[5100] = 1.0;
        let track = Track::from_samples(rate as u32, 1, samples);
        let params = Params { fft: 512, window: 256, hop: 64, ..Params::default() };
        let key = Key { width: 4, height: 16, start: 0, frames: rate as u64, band: [0.0, 1.0], params, decoded: track.decoded(), edit: Arc::new(Edit::whole(rate as u64)) };
        let image = render(&track, &key, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap();
        let dark = colour(params.floor, &params);
        let lit: Vec<bool> = (0..4).map(|c| (0..16).any(|y| image.pixels[y * 4 + c] != dark)).collect();
        assert_eq!(lit, [false, false, true, false]);
        assert!(render(&track, &key, &AtomicBool::new(true), &AtomicUsize::new(0)).is_none());
    }

    #[test]
    fn params_are_made_valid() {
        let p = Params { fft: 3000, window: 100_000, hop: 1, floor: -10.0, ceiling: -40.0, low: f32::NAN, ..Params::default() }.checked();
        assert_eq!((p.fft, p.window, p.hop, p.floor, p.ceiling, p.low), (4096, 4096, 32, -20.0, -10.0, 20.0));
        assert_eq!(Params::default().checked(), Params::default());
        assert_eq!(Params { window: 300, ..Params::default() }.checked().window, 512);
    }
}
