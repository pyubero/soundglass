//! Sound output: one device stream for the whole run, fed from `Track`s through a resampler.
//!
//! The audio callback owns all playback state (`Engine`). The UI steers it with `Cmd`s and reads
//! back the position from `Shared`. The callback never blocks, allocates or frees a track: tracks
//! it is done with go back to the UI on `retired`.

use crate::edit::{self, Edit};
use crate::track::Track;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::time::Instant;

pub enum Cmd {
    /// Play this track from the start; drops the queued one.
    Load(Arc<Track>),
    /// Play this track when the current one ends, without a gap.
    Queue(Arc<Track>),
    Play,
    Pause,
    Seek(u64),
    /// Linear gain.
    Volume(f32),
    Speed(f64),
    /// The current track's edits; `None` plays it as it is. `Load` clears them.
    Edit(Option<Arc<Edit>>),
    /// Frames to stop at the end of (or loop, when true), once playing inside them.
    Region(Option<(u64, u64)>, bool),
}

/// What the audio callback is done with, sent back so it is freed on the UI thread.
#[allow(dead_code)] // held only to be dropped
pub enum Retired {
    Track(Arc<Track>),
    Edit(Arc<Edit>),
}

#[derive(Default)]
pub struct Shared {
    /// Frame of the current track being played.
    pub pos: AtomicU64,
    /// Goes up by one each time playback moves on to the queued track.
    pub advanced: AtomicU64,
    pub playing: AtomicBool,
    /// When `pos` was stored: nanoseconds since `Player::epoch`.
    pub stamp: AtomicU64,
}

pub struct Player {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
    pub retired: Receiver<Retired>,
    epoch: Instant,
    _stream: cpal::Stream,
}

impl Player {
    /// Opens the default output device. `--timings` prints when the first real samples reach it,
    /// and `--bench` exits there.
    pub fn open(t0: Instant, timings: bool, bench: bool) -> Result<Player, String> {
        let device = cpal::default_host().default_output_device().ok_or("no sound output device")?;
        let config: cpal::StreamConfig = device.default_output_config().map_err(|e| e.to_string())?.into();
        let channels = config.channels as usize;
        let (tx, rx) = channel();
        let (retire, retired) = channel();
        let shared = Arc::new(Shared::default());
        let mut engine = Engine::new(config.sample_rate, retire);
        let (s, mut started, epoch) = (shared.clone(), false, Instant::now());
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    while let Ok(cmd) = rx.try_recv() {
                        engine.handle(cmd);
                    }
                    engine.render(out, channels);
                    s.pos.store(engine.pos, Relaxed);
                    s.stamp.store(epoch.elapsed().as_nanos() as u64, Relaxed);
                    s.advanced.store(engine.advanced, Relaxed);
                    s.playing.store(engine.playing, Relaxed);
                    if engine.heard && !started {
                        started = true;
                        // Time the first real samples reach the device; its own latency comes on top.
                        if timings {
                            eprintln!("first sound {:.1} ms", crate::ms(t0));
                        }
                        if bench {
                            std::process::exit(0);
                        }
                    }
                },
                |e| eprintln!("soundglass: sound output: {e}"),
                None,
            )
            .map_err(|e| format!("cannot open the sound device ({e})"))?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Player { tx, shared, retired, epoch, _stream: stream })
    }

    /// The frame being played and how long ago the callback said so.
    pub fn position(&self) -> (u64, std::time::Duration) {
        let stamp = std::time::Duration::from_nanos(self.shared.stamp.load(Relaxed));
        (self.shared.pos.load(Relaxed), self.epoch.elapsed().saturating_sub(stamp))
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Output frames the resampler makes per call.
const BLOCK: usize = 512;
/// The resampler's ratio may range from 1/MAX_RATIO to MAX_RATIO: 192 kHz files at 2× still fit.
const MAX_RATIO: f64 = 10.0;

struct Engine {
    cur: Option<Arc<Track>>,
    next: Option<Arc<Track>>,
    edit: Option<Arc<Edit>>,
    region: Option<(u64, u64)>,
    looping: bool,
    pos: u64,
    advanced: u64,
    playing: bool,
    /// Whether real samples (not silence) have been played yet.
    heard: bool,
    volume: f32,
    speed: f64,
    dev_rate: u32,
    /// The input rate the resampler is set for.
    rate: u32,
    resampler: Async<f32>,
    /// Stereo frames waiting to go into the resampler, and coming out of it.
    input: Vec<f32>,
    output: Vec<f32>,
    out_pos: usize,
    out_len: usize,
    retire: Sender<Retired>,
}

impl Engine {
    fn new(dev_rate: u32, retire: Sender<Retired>) -> Engine {
        let params = SincInterpolationParameters::new(128, WindowFunction::Blackman2)
            .oversampling_factor(128)
            .interpolation(SincInterpolationType::Linear);
        let resampler = Async::<f32>::new_sinc(1.0, MAX_RATIO, &params, BLOCK, 2, FixedAsync::Output).unwrap();
        let input = vec![0.0; resampler.input_frames_max() * 2];
        let output = vec![0.0; resampler.output_frames_max() * 2];
        Engine {
            cur: None, next: None, edit: None, region: None, looping: false, pos: 0, advanced: 0, playing: false, heard: false, volume: 1.0, speed: 1.0,
            dev_rate, rate: dev_rate, resampler, input, output, out_pos: 0, out_len: 0, retire,
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Load(track) => {
                let next = self.next.take();
                self.retire(next);
                let old = self.cur.replace(track);
                self.retire(old);
                let edit = self.edit.take();
                self.retire_edit(edit);
                self.region = None;
                self.seek(0);
            }
            Cmd::Queue(track) => {
                let old = self.next.replace(track);
                self.retire(old);
            }
            Cmd::Play => self.playing = true,
            Cmd::Pause => self.playing = false,
            Cmd::Seek(pos) => self.seek(pos),
            Cmd::Volume(v) => self.volume = v,
            Cmd::Speed(s) => {
                self.speed = s;
                self.set_ratio();
            }
            Cmd::Edit(edit) => {
                let old = std::mem::replace(&mut self.edit, edit);
                self.retire_edit(old);
            }
            Cmd::Region(region, looping) => (self.region, self.looping) = (region, looping),
        }
    }

    fn retire(&self, track: Option<Arc<Track>>) {
        if let Some(t) = track {
            let _ = self.retire.send(Retired::Track(t));
        }
    }

    fn retire_edit(&self, edit: Option<Arc<Edit>>) {
        if let Some(e) = edit {
            let _ = self.retire.send(Retired::Edit(e));
        }
    }

    fn seek(&mut self, pos: u64) {
        self.pos = pos;
        self.resampler.reset();
        (self.out_pos, self.out_len) = (0, 0);
    }

    fn set_ratio(&mut self) {
        let ratio = self.dev_rate as f64 / (self.rate as f64 * self.speed);
        let _ = self.resampler.set_resample_ratio(ratio.clamp(1.0 / MAX_RATIO * 1.001, MAX_RATIO * 0.999), false);
    }

    /// Fills `out` (interleaved, `channels` per frame) with sound or silence.
    fn render(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_exact_mut(channels) {
            if self.out_pos == self.out_len && !(self.playing && self.produce()) {
                frame.fill(0.0);
                continue;
            }
            let (l, r) = (self.output[self.out_pos * 2] * self.volume, self.output[self.out_pos * 2 + 1] * self.volume);
            self.out_pos += 1;
            match frame {
                [m] => *m = (l + r) * 0.5,
                [a, b, rest @ ..] => {
                    (*a, *b) = (l, r);
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
    }

    /// Runs the resampler once. False when there is nothing to play.
    fn produce(&mut self) -> bool {
        let Some(format) = self.cur.as_ref().and_then(|t| t.format.get().copied()) else { return false };
        if format.rate != self.rate {
            self.rate = format.rate;
            self.set_ratio();
        }
        let frames = self.resampler.input_frames_next();
        if !self.pull(frames) {
            return false;
        }
        let input = InterleavedSlice::new(&self.input[..frames * 2], 2, frames).unwrap();
        let capacity = self.output.len() / 2;
        let mut output = InterleavedSlice::new_mut(&mut self.output, 2, capacity).unwrap();
        let (_, written) = self.resampler.process_into_buffer(&input, &mut output, None).unwrap_or((0, 0));
        (self.out_pos, self.out_len) = (0, written);
        written > 0
    }

    /// Copies `frames` stereo frames from the current track (then the queued one) into `input`,
    /// padding with silence where audio isn't decoded yet. False at the end with nothing queued.
    fn pull(&mut self, frames: usize) -> bool {
        let mut i = 0;
        while i < frames {
            let Some(track) = &self.cur else { break };
            let end = self.edit.as_ref().map_or(track.decoded(), |e| e.len());
            if track.done() && self.pos >= end {
                // The end of the track: move on to the queued one, in the same block, so there is no gap.
                // Not while edited: the edits would be left behind unsaved without a word.
                match self.next.take() {
                    Some(next) if self.edit.is_none() && next.format.get().is_some() => {
                        let old = self.cur.replace(next);
                        self.retire(old);
                        (self.pos, self.advanced) = (0, self.advanced + 1);
                        continue;
                    }
                    next => {
                        self.next = next;
                        if i == 0 {
                            self.playing = false;
                            return false;
                        }
                        break;
                    }
                }
            }
            let mut want = (frames - i) as u64;
            let region = self.region.filter(|&(_, b)| self.pos < b);
            if let Some((_, b)) = region {
                want = want.min(b - self.pos);
            }
            let want = want as usize;
            let channels = track.format.get().map_or(1, |f| f.channels);
            let n = match &self.edit {
                Some(e) => {
                    let (out_ch, end) = (e.channels(channels), (self.pos + want as u64).min(e.len()));
                    let (input, mut k) = (&mut self.input[i * 2..], 0);
                    edit::read(track, e, self.pos, end, |s| {
                        for frame in s.chunks_exact(out_ch) {
                            (input[k * 2], input[k * 2 + 1]) = stereo(frame);
                            k += 1;
                        }
                    });
                    if k == 0 {
                        break;
                    }
                    k
                }
                None => {
                    let Some((chunk, offset)) = track.chunk_at(self.pos) else { break };
                    let n = want.min(chunk.len() / channels - offset);
                    for (k, frame) in chunk[offset * channels..(offset + n) * channels].chunks_exact(channels).enumerate() {
                        (self.input[(i + k) * 2], self.input[(i + k) * 2 + 1]) = stereo(frame);
                    }
                    n
                }
            };
            (i, self.pos) = (i + n, self.pos + n as u64);
            self.heard = true;
            if let Some((a, b)) = region
                && self.pos == b
            {
                // Back to the start within the block, so the loop has no gap.
                if self.looping {
                    self.pos = a;
                } else {
                    self.playing = false;
                    break;
                }
            }
        }
        self.input[i * 2..frames * 2].fill(0.0);
        true
    }
}

/// A frame as left and right.
fn stereo(frame: &[f32]) -> (f32, f32) {
    // shortcut: files with more than 2 channels play their first two; mix down properly if surround files show up.
    if frame.len() == 1 { (frame[0], frame[0]) } else { (frame[0], frame[1]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::CHUNK;

    fn ramp(frames: usize, start: f32) -> Vec<f32> {
        (0..frames).flat_map(|i| [start + i as f32, -(start + i as f32)]).collect()
    }

    fn engine() -> Engine {
        Engine::new(48000, channel().0)
    }

    #[test]
    fn seek_lands_on_the_frame() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 2, ramp(CHUNK + 2000, 0.0))));
        // Across the chunk boundary, too.
        e.handle(Cmd::Seek(CHUNK as u64 - 5));
        assert!(e.pull(10));
        assert_eq!(e.input[..20], ramp(10, CHUNK as f32 - 5.0)[..]);
        assert_eq!(e.pos, CHUNK as u64 + 5);
    }

    #[test]
    fn moves_into_the_queued_track_without_a_gap() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 2, ramp(100, 0.0))));
        e.handle(Cmd::Queue(Track::from_samples(48000, 1, (1000..1100).map(|i| i as f32).collect())));
        assert!(e.pull(150));
        let mut want = ramp(100, 0.0);
        want.extend((1000..1050).flat_map(|i| [i as f32, i as f32]));
        assert_eq!(e.input[..300], want[..]);
        assert_eq!((e.pos, e.advanced), (50, 1));
    }

    #[test]
    fn stops_at_the_end() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 2, ramp(10, 0.0))));
        e.handle(Cmd::Play);
        assert!(e.pull(20));
        assert_eq!(e.input[20..40], [0.0; 20]);
        assert!(!e.pull(20));
        assert!(!e.playing);
    }

    fn left(e: &Engine, frames: usize) -> Vec<f32> {
        e.input[..frames * 2].iter().step_by(2).copied().collect()
    }

    #[test]
    fn plays_through_cuts_and_silence() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 1, (0..10).map(|i| i as f32 + 1.0).collect())));
        e.handle(Cmd::Edit(Some(Arc::new(Edit::whole(10).cut(2, 4).silence(3, 5)))));
        assert!(e.pull(10));
        assert_eq!(left(&e, 10), [1., 2., 5., 0., 0., 8., 9., 10., 0., 0.]);
        assert_eq!(e.pos, 8);
    }

    #[test]
    fn plays_reversed_gained_and_mixed_edits() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 2, (0..10).flat_map(|i| [i as f32, -(i as f32)]).collect())));
        let edit = Edit::whole(5).reverse(0, 5).gain(0, 1, edit::FLOOR_DB, edit::FLOOR_DB);
        e.handle(Cmd::Edit(Some(Arc::new(edit.swap(2)))));
        assert!(e.pull(5));
        assert_eq!(e.input[..10], [0., 0., -3., 3., -2., 2., -1., 1., 0., 0.]);
        e.handle(Cmd::Edit(Some(Arc::new(edit.mono(2)))));
        e.handle(Cmd::Seek(1));
        assert!(e.pull(2));
        assert_eq!(e.input[..4], [0.; 4]);
    }

    #[test]
    fn loops_a_region_without_a_gap_and_stops_without_loop() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 1, (0..10).map(|i| i as f32).collect())));
        e.handle(Cmd::Region(Some((2, 5)), true));
        e.handle(Cmd::Seek(3));
        e.handle(Cmd::Play);
        assert!(e.pull(6));
        assert_eq!(left(&e, 6), [3., 4., 2., 3., 4., 2.]);
        e.handle(Cmd::Region(Some((2, 5)), false));
        e.handle(Cmd::Seek(3));
        assert!(e.pull(6));
        assert_eq!(left(&e, 6), [3., 4., 0., 0., 0., 0.]);
        assert!(!e.playing);
        // Past the region's end it plays on.
        e.handle(Cmd::Seek(6));
        e.handle(Cmd::Play);
        assert!(e.pull(2));
        assert_eq!(left(&e, 2), [6., 7.]);
    }

    #[test]
    fn does_not_move_on_while_edited() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 1, vec![1.0; 10])));
        e.handle(Cmd::Queue(Track::from_samples(48000, 1, vec![2.0; 10])));
        e.handle(Cmd::Edit(Some(Arc::new(Edit::whole(10).cut(0, 5)))));
        e.handle(Cmd::Play);
        assert!(e.pull(10));
        assert_eq!(left(&e, 10), [1., 1., 1., 1., 1., 0., 0., 0., 0., 0.]);
        assert!(!e.pull(10));
        assert_eq!(e.advanced, 0);
    }

    #[test]
    fn double_speed_uses_twice_the_input() {
        let mut e = engine();
        e.handle(Cmd::Load(Track::from_samples(48000, 2, vec![0.1; 200_000])));
        e.handle(Cmd::Play);
        let mut out = vec![0.0; 2 * 48000];
        e.render(&mut out, 2);
        let normal = e.pos;
        e.handle(Cmd::Seek(0));
        e.handle(Cmd::Speed(2.0));
        e.render(&mut out, 2);
        let ratio = e.pos as f64 / normal as f64;
        assert!((ratio - 2.0).abs() < 0.02, "ratio {ratio}");
        assert!(out.iter().any(|&s| s != 0.0));
    }
}
