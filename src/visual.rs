//! The visualiser shown in place of the album cover: a plasma of colour waves computed on a
//! small grid and stretched smooth by the GPU, moved by the music under the playhead (bass for
//! brightness and speed, treble for ripple size, a sudden rise in bass for a flash and a hue jump).

use crate::edit::{self, Edit};
use crate::track::Track;
use eframe::egui;
use realfft::num_complex::Complex;
use realfft::RealToComplex;
use std::sync::Arc;
use std::time::Instant;

/// Side of the colour grid; LINEAR filtering stretches it to the cover's size.
const GRID: usize = 96;
/// Frames analysed each redraw (about 23 ms at 44.1 kHz).
const FFT: usize = 1024;

pub struct Visual {
    fft: Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    texture: Option<egui::TextureHandle>,
    last: Option<Instant>,
    /// Plasma phase and base hue, advanced by time and the music.
    phase: f32,
    hue: f32,
    /// Band levels 0..1 after auto-gain, the loudest seen per band (slowly forgotten),
    /// the bass's running average for onsets, and the flash left from the last onset.
    level: [f32; 3],
    peak: [f32; 3],
    bass_avg: f32,
    flash: f32,
}

impl Visual {
    pub fn new() -> Visual {
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        Visual { input: fft.make_input_vec(), output: fft.make_output_vec(), fft, texture: None, last: None, phase: 0.0, hue: 0.0, level: [0.0; 3], peak: [1e-9; 3], bass_avg: 0.0, flash: 0.0 }
    }

    /// The frame for edited position `pos`; while paused, the last one again (nothing computed).
    pub fn frame(&mut self, ctx: &egui::Context, track: &Track, edit: &Edit, pos: u64, rate: u64, playing: bool) -> egui::TextureHandle {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |l| (now - l).as_secs_f32().min(0.1));
        self.last = playing.then_some(now);
        if let Some(t) = &self.texture && !playing {
            return t.clone();
        }
        if playing && rate > 0 {
            self.listen(track, edit, pos, rate as f32, dt);
        }
        let [bass, mid, treble] = self.level;
        self.phase += dt * (0.6 + 2.5 * bass);
        self.hue = (self.hue + dt * (0.02 + 0.1 * mid)).fract();
        let image = plasma(self.phase, self.hue, bass, mid, treble, self.flash);
        match &mut self.texture {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("visualiser", image, egui::TextureOptions::LINEAR)),
        }
        self.texture.clone().expect("set above")
    }

    /// Updates the band levels and onset flash from the `FFT` frames before `pos`.
    fn listen(&mut self, track: &Track, edit: &Edit, pos: u64, rate: f32, dt: f32) {
        let a = pos.saturating_sub(FFT as u64);
        let ch = edit.channels(track.format.get().map_or(1, |f| f.channels)).max(1);
        let (input, mut n) = (&mut self.input, 0);
        input.fill(0.0);
        // Never blocks: a busy or undecoded part just leaves zeros.
        edit::read(track, edit, a, pos, |block| {
            for frame in block.chunks_exact(ch) {
                if n < FFT {
                    input[n] = frame.iter().sum::<f32>() / ch as f32;
                    n += 1;
                }
            }
        });
        if self.fft.process(input, &mut self.output).is_err() {
            return;
        }
        let hz = rate / FFT as f32;
        let mut power = [0f32; 3];
        for (i, c) in self.output.iter().enumerate().skip(1) {
            let f = i as f32 * hz;
            power[if f < 150.0 { 0 } else if f < 2000.0 { 1 } else { 2 }] += c.norm_sqr();
        }
        let fade = (-dt * 6.0).exp();
        for b in 0..3 {
            let e = power[b].sqrt();
            // Auto-gain: quiet and loud tracks move alike; the peak halves in about 7 s.
            self.peak[b] = e.max(self.peak[b] * (-dt * 0.1).exp()).max(1e-9);
            self.level[b] = (e / self.peak[b]).max(self.level[b] * fade);
        }
        let bass = self.level[0];
        if bass > 0.3 && bass > 1.4 * self.bass_avg && self.flash < 0.5 {
            self.flash = 1.0;
            self.hue = (self.hue + 0.13).fract();
        }
        self.flash *= fade;
        self.bass_avg += (bass - self.bass_avg) * (1.0 - (-dt * 4.0).exp());
    }
}

/// The GRID×GRID picture: four sine waves summed, read as a hue around `hue`.
fn plasma(phase: f32, hue: f32, bass: f32, mid: f32, treble: f32, flash: f32) -> egui::ColorImage {
    let k = 3.0 + 4.0 * treble;
    let value = (0.2 + 0.3 * bass + 0.25 * flash).min(0.75);
    let spread = 0.08 + 0.1 * mid;
    // Saturation and value are the same for the whole frame, so convert 256 hues once.
    let palette: Vec<egui::Color32> = (0..256).map(|i| egui::ecolor::Hsva::new(i as f32 / 256.0, 0.85 - 0.3 * flash, value, 1.0).into()).collect();
    let coord = |i: usize| i as f32 / GRID as f32 * 2.0 - 1.0;
    let cols: Vec<f32> = (0..GRID).map(|i| (coord(i) * k + phase).sin()).collect();
    let mut pixels = Vec::with_capacity(GRID * GRID);
    for j in 0..GRID {
        let y = coord(j);
        let row = (y * k * 0.8 - phase * 0.7).sin();
        for (i, col) in cols.iter().enumerate() {
            let x = coord(i);
            let v = col + row + ((x + y) * k * 0.6 + phase * 1.3).sin() + ((x * x + y * y).sqrt() * k * 1.5 - phase * 2.0).sin();
            pixels.push(palette[((hue + v * spread + 1.0).fract() * 256.0) as usize & 255]);
        }
    }
    egui::ColorImage::new([GRID, GRID], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plasma_fills_the_grid_and_moves() {
        let a = plasma(0.0, 0.2, 0.5, 0.5, 0.5, 0.0);
        let b = plasma(1.0, 0.2, 0.5, 0.5, 0.5, 0.0);
        assert_eq!(a.pixels.len(), GRID * GRID);
        assert_ne!(a.pixels, b.pixels);
    }
}
