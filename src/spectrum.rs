//! The spectrogram: a short-time Fourier transform of the whole edited track, mixed to mono,
//! one column every `hop` frames on a logarithmic frequency axis from `Params::low` to half the
//! sample rate. Worked out once on a worker thread, kept as levels in tiles of textures, then
//! only scaled to zoom and pan.

use crate::edit::{self, Edit};
use crate::track::Track;
use eframe::egui;
use realfft::num_complex::Complex;
use realfft::RealToComplex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// The window's shape.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
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

    fn stops(self) -> &'static [[f32; 3]] {
        match self {
            ColourMap::Magma => &[[0.0, 0.0, 4.0], [80.0, 18.0, 123.0], [182.0, 54.0, 121.0], [251.0, 136.0, 97.0], [252.0, 253.0, 191.0]],
            ColourMap::Gray => &[[0.0, 0.0, 0.0], [255.0, 255.0, 255.0]],
            ColourMap::BlueRed => &[[0.0, 0.0, 40.0], [0.0, 60.0, 200.0], [0.0, 200.0, 200.0], [240.0, 230.0, 0.0], [220.0, 20.0, 0.0]],
        }
    }
}

/// The hop choices.
pub const HOPS: [usize; 3] = [256, 512, 1024];

/// How the spectrogram is analysed and coloured; kept in the settings.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
        let nearest = |v: usize, choices: [usize; 3]| choices.into_iter().min_by_key(|c| c.abs_diff(v)).unwrap();
        Params { fft, window, hop: nearest(self.hop, HOPS), floor, ceiling, low, ..self }
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

    /// The level in dBFS of each FFT bin (0 Hz up to half the sample rate, `fft / 2 + 1` of
    /// them) around edited frame `centre`; `None` where the audio isn't decoded yet.
    fn bins(&mut self, track: &Track, edit: &Edit, centre: u64) -> Option<Vec<f32>> {
        let format = track.format.get()?;
        let channels = edit.channels(format.channels).max(1);
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
        Some(self.output.iter().map(|c| 20.0 * (c.norm() * scale).max(1e-10).log10()).collect())
    }
}

/// The FFT bins of `params`: one row of the picture each.
pub fn bins(params: &Params) -> usize {
    params.fft / 2 + 1
}

/// Columns per tile, and bins per texture: textures larger than this may not fit the graphics card.
const TILE: usize = 2048;
const SLAB: usize = 4096;
/// At most this many levels (columns × bins) are kept: about 300 MB with their textures.
const BUDGET: u64 = 48_000_000;
/// The levels kept: `u8` steps of about 0.65 dB from `LEVEL_LOW` to `LEVEL_HIGH` dBFS.
const LEVEL_LOW: f32 = -160.0;
const LEVEL_HIGH: f32 = 6.0;

fn level(db: f32) -> u8 {
    ((db.clamp(LEVEL_LOW, LEVEL_HIGH) - LEVEL_LOW) / (LEVEL_HIGH - LEVEL_LOW) * 255.0).round() as u8
}

fn db(level: u8) -> f32 {
    LEVEL_LOW + level as f32 / 255.0 * (LEVEL_HIGH - LEVEL_LOW)
}

/// The hop for a track of `len` frames: the chosen one, doubled until the levels fit `BUDGET`.
fn hop_for(len: u64, params: &Params) -> u64 {
    let mut hop = params.hop as u64;
    while len.div_ceil(hop) * bins(params) as u64 > BUDGET {
        hop *= 2;
    }
    hop
}

/// The whole edited track's columns, centred on frames 0, `hop`, 2·`hop`…, each the levels of
/// the FFT's bins (lowest first), handed to `send` a tile at a time as (tile, levels column by
/// column, columns). Stops when `cancel` is set; counts finished columns in `done`.
fn analyse(track: &Track, edit: &Edit, params: Params, hop: u64, cancel: &AtomicBool, done: &AtomicUsize, mut send: impl FnMut(usize, Vec<u8>, usize)) {
    let (rows, columns) = (bins(&params), edit.len().div_ceil(hop) as usize);
    let mut analyser = Analyser::new(params);
    for tile in 0..columns.div_ceil(TILE) {
        let width = TILE.min(columns - tile * TILE);
        let mut levels = Vec::with_capacity(width * rows);
        for c in tile * TILE..tile * TILE + width {
            if cancel.load(Relaxed) {
                return;
            }
            // The decoded chunks are only locked for a moment; a column that still can't be read stays at the floor.
            let column = (0..100).find_map(|_| analyser.bins(track, edit, c as u64 * hop).or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                None
            }));
            match column {
                Some(column) => levels.extend(column.into_iter().map(level)),
                None => levels.resize(levels.len() + rows, 0),
            }
            done.store(c + 1, Relaxed);
        }
        send(tile, levels, width);
    }
}

/// The colour of each of the 256 levels.
fn palette(params: &Params) -> Vec<egui::Color32> {
    (0..=255).map(|l| colour(db(l), params)).collect()
}

/// The picture of bins `from..to` of a tile's levels (column by column, `rows` bins each,
/// lowest first), highest bin at the top.
fn tile_image(levels: &[u8], columns: usize, rows: usize, (from, to): (usize, usize), palette: &[egui::Color32]) -> egui::ColorImage {
    let height = to - from;
    let mut pixels = vec![egui::Color32::TRANSPARENT; columns * height];
    for (c, column) in levels.chunks_exact(rows).enumerate() {
        for (k, &l) in column[from..to].iter().enumerate() {
            pixels[(height - 1 - k) * columns + c] = palette[l as usize];
        }
    }
    egui::ColorImage::new([columns, height], pixels)
}

/// The bins of each texture: `SLAB` at a time.
fn slabs(rows: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..rows.div_ceil(SLAB)).map(move |s| (s * SLAB, ((s + 1) * SLAB).min(rows)))
}

/// A slab of bins `from..to` drawn between `x` left and right, each bin a strip from half a bin
/// below to half above its frequency, placed by `y_of` (for the log axis); `fft` and `rate` give
/// the bins' frequencies.
pub fn mesh(texture: egui::TextureId, [left, right]: [f32; 2], (from, to): (usize, usize), fft: usize, rate: f32, y_of: impl Fn(f32) -> f32) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    let span = (to - from) as f32;
    for (i, edge) in (from..=to).enumerate() {
        // The 0 Hz bin's lower edge is below 0; any small frequency puts it off the bottom.
        let freq = ((edge as f32 - 0.5) * rate / fft as f32).max(1e-3);
        let (y, v) = (y_of(freq), 1.0 - i as f32 / span);
        mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(left, y), uv: egui::pos2(0.0, v), color: egui::Color32::WHITE });
        mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(right, y), uv: egui::pos2(1.0, v), color: egui::Color32::WHITE });
        if i > 0 {
            let k = 2 * i as u32;
            mesh.indices.extend_from_slice(&[k - 2, k - 1, k, k - 1, k + 1, k]);
        }
    }
    mesh
}

/// Finished tiles turned into textures per frame.
const TILES_PER_FRAME: usize = 2;
/// Mipmapped, so a zoomed-out picture is averaged rather than shimmering.
const TEXTURE: egui::TextureOptions = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };

struct Tile {
    levels: Vec<u8>,
    columns: usize,
    /// One per `SLAB` bins, lowest first.
    textures: Vec<egui::TextureHandle>,
}

/// The analysis running on the worker thread.
struct Job {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    columns: usize,
    rx: Receiver<(usize, Vec<u8>, usize)>,
}

/// A job dropped (no longer wanted, or the window gone) stops at its next column.
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
    }
}

/// What the analysis depends on: the track (by address), the edits, and the parameters with
/// the colours (those only recolour) and the lowest frequency (only drawn) left out.
type Key = (usize, Arc<Edit>, Params);

pub struct Spectrogram {
    /// The part of the frequency axis shown, bottom and top, on the log scale where 0 is
    /// `Params::low` and 1 half the sample rate.
    pub band: [f32; 2],
    key: Option<Key>,
    /// The hop used: the chosen one, or more for a long track.
    hop: u64,
    /// Bins per column, and the FFT length they come from.
    rows: usize,
    fft: usize,
    tiles: Vec<Tile>,
    /// The floor, ceiling and colour map the tiles are coloured with.
    colours: Option<(f32, f32, ColourMap)>,
    job: Option<Job>,
}

impl Spectrogram {
    pub fn new() -> Spectrogram {
        Spectrogram { band: [0.0, 1.0], key: None, hop: 0, rows: 0, fft: 0, tiles: Vec::new(), colours: None, job: None }
    }

    /// Starts the analysis of `track` with `edit` once it's fully decoded (its length is exact
    /// then), again when the edits or the analysis change; picks up finished tiles; recolours
    /// when only the colours change.
    pub fn update(&mut self, ctx: &egui::Context, track: &Arc<Track>, edit: &Arc<Edit>, params: Params) {
        if !track.done() || track.format.get().is_none() {
            return;
        }
        let key = (Arc::as_ptr(track) as usize, edit.clone(), Params { floor: 0.0, ceiling: 0.0, map: ColourMap::Magma, low: 0.0, ..params });
        if self.key.as_ref() != Some(&key) {
            (self.hop, self.rows, self.fft, self.tiles, self.job) = (hop_for(edit.len(), &params), bins(&params), params.fft, Vec::new(), None);
            let (tx, rx) = std::sync::mpsc::channel();
            let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
            let columns = edit.len().div_ceil(self.hop) as usize;
            let (track, edit, ctx, hop) = (track.clone(), edit.clone(), ctx.clone(), self.hop);
            let (c, d) = (cancel.clone(), done.clone());
            std::thread::spawn(move || {
                crate::track::background();
                analyse(&track, &edit, params, hop, &c, &d, |tile, levels, columns| {
                    let _ = tx.send((tile, levels, columns));
                    ctx.request_repaint();
                })
            });
            self.job = Some(Job { cancel, done, columns, rx });
            self.key = Some(key);
        }
        let colours = Some((params.floor, params.ceiling, params.map));
        let palette = palette(&params);
        if self.colours != colours {
            for t in &mut self.tiles {
                for (texture, slab) in t.textures.iter_mut().zip(slabs(self.rows)) {
                    texture.set(tile_image(&t.levels, t.columns, self.rows, slab, &palette), TEXTURE);
                }
            }
            self.colours = colours;
        }
        if let Some(job) = &self.job {
            // A few tiles per frame: uploading a backlog at once would stall the window.
            for n in 0.. {
                if n == TILES_PER_FRAME {
                    ctx.request_repaint();
                    break;
                }
                match job.rx.try_recv() {
                    Ok((tile, levels, columns)) => {
                        let textures = slabs(self.rows)
                            .map(|slab| ctx.load_texture(format!("spectrogram-{tile}-{}", slab.0), tile_image(&levels, columns, self.rows, slab, &palette), TEXTURE))
                            .collect();
                        self.tiles.push(Tile { levels, columns, textures });
                    }
                    Err(TryRecvError::Empty) => break,
                    // Finished (or the worker died): keep what came.
                    Err(TryRecvError::Disconnected) => {
                        self.job = None;
                        break;
                    }
                }
            }
        }
    }

    /// The textures so far, each as the edited frames it spans (from, to), its bins (from, to)
    /// and the texture.
    pub fn tiles(&self) -> impl Iterator<Item = (f64, f64, (usize, usize), &egui::TextureHandle)> {
        let hop = self.hop as f64;
        self.tiles.iter().enumerate().flat_map(move |(i, t)| {
            let first = (i * TILE) as f64 * hop - hop / 2.0;
            t.textures.iter().zip(slabs(self.rows)).map(move |(texture, slab)| (first, first + t.columns as f64 * hop, slab, texture))
        })
    }

    /// The FFT length the tiles were made with.
    pub fn fft(&self) -> usize {
        self.fft
    }

    /// The hop used, once the analysis started.
    pub fn hop(&self) -> Option<u64> {
        self.key.is_some().then_some(self.hop)
    }

    /// How far the analysis is (0 to 1) while it runs.
    pub fn progress(&self) -> Option<f32> {
        self.job.as_ref().map(|j| j.done.load(Relaxed) as f32 / j.columns.max(1) as f32)
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

/// The major scales the notes can be limited to, as (name, tonic's pitch class with C = 0,
/// tonic's letter with C = 0): the usual spellings, F# rather than Gb and B rather than Cb.
pub const KEYS: [(&str, u8, usize); 12] =
    [("C", 0, 0), ("Db", 1, 1), ("D", 2, 1), ("Eb", 3, 2), ("E", 4, 2), ("F", 5, 3), ("F#", 6, 3), ("G", 7, 4), ("Ab", 8, 5), ("A", 9, 5), ("Bb", 10, 6), ("B", 11, 6)];

/// The pitch class of each natural note, C to B.
const NATURALS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

/// How pitch class `pc` is written, as (letter with C = 0, accidental): in major scale `key`
/// (an index into `KEYS`) as that key spells it, `None` when it isn't in the scale; with no
/// key, with sharps or `flats`.
fn spell(pc: u8, flats: bool, key: Option<usize>) -> Option<(usize, &'static str)> {
    let Some(key) = key else {
        let natural = NATURALS.iter().position(|&n| n == pc);
        return Some(match natural {
            Some(letter) => (letter, ""),
            None if flats => (NATURALS.iter().position(|&n| n == pc + 1)?, "b"),
            None => (NATURALS.iter().position(|&n| n + 1 == pc)?, "#"),
        });
    };
    let (_, tonic, letter) = KEYS[key % 12];
    (0..7).find_map(|degree| {
        let note = (tonic + NATURALS[degree]) % 12;
        let letter = (letter + degree) % 7;
        (note == pc).then(|| (letter, match (note + 12 - NATURALS[letter]) % 12 { 1 => "#", 11 => "b", _ => "" }))
    })
}

/// An instrument whose usual range (in concert pitch) the note lines can be limited to.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Instrument {
    None,
    Trumpet,
    TenorSax,
}

impl Instrument {
    pub const ALL: [Instrument; 3] = [Instrument::None, Instrument::Trumpet, Instrument::TenorSax];

    pub fn name(self) -> &'static str {
        match self {
            Instrument::None => "None",
            Instrument::Trumpet => "Trumpet",
            Instrument::TenorSax => "Tenor sax",
        }
    }

    /// The lowest and highest sounding notes as MIDI numbers: B♭ trumpet written F#3–C6 sounds
    /// a tone lower, B♭ tenor sax written Bb3–F6 a major ninth lower.
    pub fn range(self) -> Option<(i32, i32)> {
        match self {
            Instrument::None => None,
            Instrument::Trumpet => Some((52, 82)),
            Instrument::TenorSax => Some((44, 75)),
        }
    }
}

/// A note line: its frequency, pitch class (C = 0), how it's written, and its octave.
pub struct Note {
    pub freq: f32,
    pub pc: u8,
    pub letter: usize,
    pub mark: &'static str,
    pub octave: i32,
}

/// The notes of equal temperament tuned to `a4` from `low` Hz up to `nyquist`, lowest first:
/// only those of major scale `key` when there is one, and within `instrument`'s range.
pub fn notes(a4: f32, low: f32, nyquist: f32, key: Option<usize>, flats: bool, instrument: Instrument) -> Vec<Note> {
    let (lowest, highest) = instrument.range().unwrap_or((0, 135));
    (lowest..=highest)
        .filter_map(|midi: i32| {
            let (freq, pc) = (a4 * 2f32.powf((midi - 69) as f32 / 12.0), midi.rem_euclid(12) as u8);
            let (letter, mark) = spell(pc, flats, key)?;
            (low..=nyquist).contains(&freq).then_some(Note { freq, pc, letter, mark, octave: midi / 12 - 1 })
        })
        .collect()
}

/// A note's name: "C#4", "Db4", "Do#4", "Réb4"… with `#` and `b`, as the bundled font has no ♯ or ♭.
pub fn note_name(note: &Note, solfege: bool, octaves: bool) -> String {
    const LETTERS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];
    const SOLFEGE: [&str; 7] = ["Do", "Ré", "Mi", "Fa", "Sol", "La", "Si"];
    let base = if solfege { SOLFEGE } else { LETTERS }[note.letter];
    if octaves { format!("{base}{}{}", note.mark, note.octave) } else { format!("{base}{}", note.mark) }
}

/// Pitch class `pc` as the note lines write it, without an octave: as major scale `key` spells
/// it when it's in the scale, otherwise with sharps or `flats`.
pub fn pc_name(pc: u8, flats: bool, key: Option<usize>, solfege: bool) -> String {
    let (letter, mark) = spell(pc, flats, key).or_else(|| spell(pc, flats, None)).unwrap_or((0, ""));
    note_name(&Note { freq: 0.0, pc, letter, mark, octave: 0 }, solfege, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_pitch_classes_in_and_out_of_a_scale() {
        // F# major spells pitch class 5 as E#; outside the scale, sharps or flats as chosen.
        assert_eq!(pc_name(5, false, Some(6), false), "E#");
        assert_eq!(pc_name(1, true, Some(0), false), "Db");
        assert_eq!(pc_name(10, false, None, true), "La#");
    }

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
        let all = notes(440.0, 20.0, 22050.0, None, false, Instrument::None);
        let find = |n: &[Note], pc, oct| n.iter().find(|x| x.pc == pc && x.octave == oct).unwrap().freq;
        assert!((find(&all, 9, 4) - 440.0).abs() < 1e-3);
        assert!((find(&all, 0, 4) - 261.626).abs() < 0.01);
        assert!((find(&notes(432.0, 20.0, 22050.0, None, false, Instrument::None), 0, 4) - 256.87).abs() < 0.01);
        assert!(all.first().unwrap().freq >= 20.0 && all.last().unwrap().freq <= 22050.0);
        assert!(notes(440.0, 100.0, 22050.0, None, false, Instrument::None).first().unwrap().freq >= 100.0);
        let names = |key: Option<usize>, flats: bool, oct: i32| -> Vec<String> {
            notes(440.0, 20.0, 22050.0, key, flats, Instrument::None).iter().filter(|n| n.octave == oct).map(|n| note_name(n, false, false)).collect()
        };
        assert_eq!(names(None, false, 4).len(), 12);
        // C major is the natural notes.
        assert_eq!(names(Some(0), false, 4), ["C", "D", "E", "F", "G", "A", "B"]);
        assert_eq!(names(Some(5), false, 4), ["C", "D", "E", "F", "G", "A", "Bb"]);
        assert_eq!(names(Some(2), true, 4), ["C#", "D", "E", "F#", "G", "A", "B"]);
        assert_eq!(names(Some(6), false, 4), ["C#", "D#", "E#", "F#", "G#", "A#", "B"]);
        assert_eq!(names(Some(1), false, 4), ["C", "Db", "Eb", "F", "Gb", "Ab", "Bb"]);
        let c_sharp = notes(440.0, 20.0, 22050.0, None, false, Instrument::None).into_iter().find(|n| n.pc == 1 && n.octave == 4).unwrap();
        assert_eq!(note_name(&c_sharp, false, true), "C#4");
        assert_eq!(note_name(&c_sharp, true, false), "Do#");
        let d_flat = notes(440.0, 20.0, 22050.0, None, true, Instrument::None).into_iter().find(|n| n.pc == 1 && n.octave == 4).unwrap();
        assert_eq!(note_name(&d_flat, true, true), "Réb4");
    }

    #[test]
    fn instruments_limit_the_notes_to_their_range() {
        let ends = |i: Instrument| {
            let n = notes(440.0, 20.0, 22050.0, None, true, i);
            (note_name(n.first().unwrap(), false, true), n.first().unwrap().freq, note_name(n.last().unwrap(), false, true), n.last().unwrap().freq)
        };
        let (lo, f_lo, hi, f_hi) = ends(Instrument::Trumpet);
        assert_eq!((lo.as_str(), hi.as_str()), ("E3", "Bb5"));
        assert!((f_lo - 164.81).abs() < 0.01 && (f_hi - 932.33).abs() < 0.01);
        let (lo, f_lo, hi, f_hi) = ends(Instrument::TenorSax);
        assert_eq!((lo.as_str(), hi.as_str()), ("Ab2", "Eb5"));
        assert!((f_lo - 103.83).abs() < 0.01 && (f_hi - 622.25).abs() < 0.01);
    }

    fn sine(rate: u32, freq: f32, amp: f32) -> Arc<Track> {
        Track::from_samples(rate, 1, (0..rate).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin() * amp).collect())
    }

    #[test]
    fn a_sine_is_brightest_at_its_frequency_and_silence_is_the_floor() {
        let rate = 44100;
        let (track, edit) = (sine(rate, 1000.0, 0.5), Edit::whole(rate as u64));
        for shape in Shape::ALL {
            for (fft, window) in [(4096, 4096), (8192, 2048)] {
                let params = Params { fft, window, shape, ..Params::default() };
                let levels = Analyser::new(params).bins(&track, &edit, 22050).unwrap();
                assert_eq!(levels.len(), bins(&params));
                let loudest = (0..levels.len()).max_by(|&a, &b| levels[a].total_cmp(&levels[b])).unwrap();
                let want = (1000.0 * fft as f32 / rate as f32).round() as usize;
                assert!(loudest.abs_diff(want) <= 1, "{shape:?} {fft}/{window}: loudest bin {loudest}, 1 kHz is bin {want}");
                // Half scale is −6 dBFS.
                assert!((levels[loudest] + 6.0).abs() < 1.5, "{shape:?} {fft}/{window}: {} dB", levels[loudest]);
            }
        }
        let silent = Track::from_samples(rate, 2, vec![0.0; 2 * rate as usize]);
        let p = Params::default();
        let levels = Analyser::new(p).bins(&silent, &edit, 0).unwrap();
        assert!(levels.iter().all(|&db| colour(db, &p) == colour(p.floor, &p)));
    }

    #[test]
    fn big_ffts_are_split_into_textures_and_drawn_by_frequency() {
        assert_eq!(slabs(2049).collect::<Vec<_>>(), [(0, 2049)]);
        assert_eq!(slabs(16385).collect::<Vec<_>>(), [(0, 4096), (4096, 8192), (8192, 12288), (12288, 16384), (16384, 16385)]);
        // Bins of an 8-point FFT at 8 kHz are 1 kHz apart: bin 2's strip spans 1.5–2.5 kHz.
        let m = mesh(egui::TextureId::default(), [0.0, 10.0], (0, 5), 8, 8000.0, |f| -f);
        assert_eq!((m.vertices.len(), m.indices.len()), (12, 30));
        assert_eq!((m.vertices[4].pos.y, m.vertices[6].pos.y), (-1500.0, -2500.0));
        assert_eq!((m.vertices[0].uv.y, m.vertices[10].uv.y), (1.0, 0.0));
    }

    #[test]
    fn the_track_is_analysed_once_into_columns_every_hop() {
        // One second of silence with a click: only the column centred nearest it lights up.
        let rate = 8000;
        let mut samples = vec![0.0; rate];
        samples[5100] = 1.0;
        let track = Track::from_samples(rate as u32, 1, samples);
        let params = Params { fft: 512, window: 256, hop: 256, ..Params::default() };
        let mut got = Vec::new();
        analyse(&track, &Edit::whole(rate as u64), params, 256, &AtomicBool::new(false), &AtomicUsize::new(0), |tile, levels, columns| got.push((tile, levels, columns)));
        assert_eq!(got.len(), 1);
        let (tile, levels, columns) = &got[0];
        assert_eq!((*tile, *columns, levels.len()), (0, 32, 32 * 257));
        let lit: Vec<usize> = (0..32).filter(|c| levels[c * 257..(c + 1) * 257].iter().any(|&l| db(l) > params.floor)).collect();
        assert_eq!(lit, [20]);
        let mut none = 0;
        analyse(&track, &Edit::whole(rate as u64), params, 256, &AtomicBool::new(true), &AtomicUsize::new(0), |_, _, _| none += 1);
        assert_eq!(none, 0);
    }

    #[test]
    fn levels_round_trip_and_long_tracks_get_a_longer_hop() {
        for d in [-150.0, -100.0, -6.0, 0.0, 5.9] {
            assert!((db(level(d)) - d).abs() < 0.4, "{d}");
        }
        let p = Params::default();
        let minutes = |m: u64| m * 60 * 44100;
        assert_eq!(hop_for(minutes(3), &p), 512);
        assert_eq!(hop_for(minutes(3), &Params { fft: 2048, hop: 256, ..p }), 256);
        assert_eq!(hop_for(minutes(3), &Params { hop: 256, ..p }), 512);
        assert!(hop_for(minutes(60), &p) > 512);
        assert!(minutes(60).div_ceil(hop_for(minutes(60), &p)) * bins(&p) as u64 <= BUDGET);
    }

    #[test]
    fn params_are_made_valid() {
        let p = Params { fft: 3000, window: 100_000, hop: 1, floor: -10.0, ceiling: -40.0, low: f32::NAN, ..Params::default() }.checked();
        assert_eq!((p.fft, p.window, p.hop, p.floor, p.ceiling, p.low), (4096, 4096, 256, -20.0, -10.0, 20.0));
        assert_eq!(Params { hop: 300, ..Params::default() }.checked().hop, 256);
        assert_eq!(Params { hop: 100, ..Params::default() }.checked().hop, 256);
        assert_eq!(Params::default().checked(), Params::default());
        assert_eq!(Params { window: 300, ..Params::default() }.checked().window, 512);
    }
}
