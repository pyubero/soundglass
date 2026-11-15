//! Edits as a list of pieces of the original track: its audio in memory is never changed, and an
//! undo step is just an older list. Effects are fields on the pieces (a gain ramp, played
//! backwards) and a channel mix on the whole list.

use crate::track::{PEAK, Track};
use std::sync::Arc;

/// At or below this gain a frame is silent; fades run from here to 0 dB.
pub const FLOOR_DB: f32 = -60.0;

/// `len` frames of the original from frame `src` (backwards if `rev`), or `len` frames of silence.
/// `db` is the gain at the piece's start and end, linear in dB in between, so gains and fades
/// stacked on a piece add up to one ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub src: u64,
    pub len: u64,
    pub silent: bool,
    pub rev: bool,
    pub db: (f32, f32),
}

impl Seg {
    fn plain(src: u64, len: u64) -> Seg {
        Seg { src, len, silent: false, rev: false, db: (0.0, 0.0) }
    }

    fn quiet(len: u64) -> Seg {
        Seg { silent: true, ..Seg::plain(0, len) }
    }

    /// The gain in dB `off` frames into the piece (`len` is its end).
    fn db_at(&self, off: u64) -> f32 {
        if self.len == 0 { self.db.0 } else { self.db.0 + (self.db.1 - self.db.0) * (off as f64 / self.len as f64) as f32 }
    }

    /// The frames `lo..hi` of the piece, as played.
    fn part(&self, lo: u64, hi: u64) -> Seg {
        let src = if self.rev { self.src + (self.len - hi) } else { self.src + lo };
        Seg { src, len: hi - lo, db: (self.db_at(lo), self.db_at(hi)), ..*self }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub segs: Vec<Seg>,
    /// Output channels × source channels; `None` keeps the source's channels.
    pub mix: Option<Vec<Vec<f32>>>,
}

impl Edit {
    /// The whole track, unedited.
    pub fn whole(len: u64) -> Edit {
        Edit { segs: joined(vec![Seg::plain(0, len)]), mix: None }
    }

    pub fn len(&self) -> u64 {
        self.segs.iter().map(|s| s.len).sum()
    }

    /// The channels it plays with, from a track with `src` channels.
    pub fn channels(&self, src: usize) -> usize {
        self.mix.as_ref().map_or(src, |m| m.len())
    }

    /// The same mix with other pieces.
    fn with(&self, segs: Vec<Seg>) -> Edit {
        Edit { segs: joined(segs), mix: self.mix.clone() }
    }

    /// The pieces that make up frames `a..b`.
    fn slice(&self, a: u64, b: u64) -> Vec<Seg> {
        let (mut out, mut start) = (Vec::new(), 0);
        for s in &self.segs {
            let (lo, hi) = (a.max(start), b.min(start + s.len));
            if lo < hi {
                out.push(s.part(lo - start, hi - start));
            }
            start += s.len;
        }
        out
    }

    /// Without frames `a..b`.
    pub fn cut(&self, a: u64, b: u64) -> Edit {
        let mut segs = self.slice(0, a);
        segs.extend(self.slice(b, self.len()));
        self.with(segs)
    }

    /// The original's frame played at edited frame `pos`; None in silence or past the end.
    pub fn source_of(&self, pos: u64) -> Option<u64> {
        let mut at = 0;
        for seg in &self.segs {
            if pos < at + seg.len {
                let off = pos - at;
                return (!seg.silent).then_some(if seg.rev { seg.src + seg.len - 1 - off } else { seg.src + off });
            }
            at += seg.len;
        }
        None
    }

    /// Only frames `a..b`.
    pub fn crop(&self, a: u64, b: u64) -> Edit {
        self.with(self.slice(a, b))
    }

    /// Frames `a..b` replaced with silence of the same length.
    pub fn silence(&self, a: u64, b: u64) -> Edit {
        let (len, b) = (self.len(), b.min(self.len()));
        let mut segs = self.slice(0, a);
        segs.push(Seg::quiet(b.saturating_sub(a)));
        segs.extend(self.slice(b, len));
        self.with(segs)
    }

    /// Frames `a..b` with a gain going from `from` to `to` dB added: a constant gain, or a fade.
    pub fn gain(&self, a: u64, b: u64, from: f32, to: f32) -> Edit {
        let (len, b) = (self.len(), b.min(self.len()));
        let at = |p: u64| from + (to - from) * (p as f64 / (b - a).max(1) as f64) as f32;
        let mut segs = self.slice(0, a);
        let mut p = a;
        for s in self.slice(a, b) {
            let ramp = (at(p - a), at(p - a + s.len));
            p += s.len;
            segs.push(if s.silent { s } else { Seg { db: (s.db.0 + ramp.0, s.db.1 + ramp.1), ..s } });
        }
        segs.extend(self.slice(b, len));
        self.with(segs)
    }

    /// Frames `a..b` faded in from silence (or out to it, with `out`) so loudness changes evenly
    /// to the ear: loudness goes with amplitude^0.6, so the amplitude follows t^(5/3). Built from
    /// steps of linear-dB ramps, so it stacks with other gains like any of them.
    pub fn fade(&self, a: u64, b: u64, out: bool) -> Edit {
        const STEPS: u64 = 32;
        let b = b.min(self.len());
        let db = |i: u64| {
            let t = if out { STEPS - i } else { i } as f32 / STEPS as f32;
            if t == 0.0 { FLOOR_DB } else { (20.0 * 5.0 / 3.0 * t.log10()).max(FLOOR_DB) }
        };
        let at = |i: u64| a + (b.saturating_sub(a)) * i / STEPS;
        (0..STEPS).fold(self.clone(), |e, i| e.gain(at(i), at(i + 1), db(i), db(i + 1)))
    }

    /// Frames `a..b` played backwards.
    pub fn reverse(&self, a: u64, b: u64) -> Edit {
        let mut segs = self.slice(0, a);
        segs.extend(self.slice(a, b).into_iter().rev().map(|s| if s.silent { s } else { Seg { rev: !s.rev, db: (s.db.1, s.db.0), ..s } }));
        segs.extend(self.slice(b, self.len()));
        self.with(segs)
    }

    /// The mix as rows, the source's own channels when there is none.
    fn rows(&self, src: usize) -> Vec<Vec<f32>> {
        self.mix.clone().unwrap_or_else(|| identity(src))
    }

    fn mixed(&self, rows: Vec<Vec<f32>>, src: usize) -> Edit {
        Edit { segs: self.segs.clone(), mix: (rows != identity(src)).then_some(rows) }
    }

    /// Every channel mixed into one.
    pub fn mono(&self, src: usize) -> Edit {
        let rows = self.rows(src);
        let row = (0..src).map(|j| rows.iter().map(|r| r[j]).sum::<f32>() / rows.len() as f32).collect();
        self.mixed(vec![row], src)
    }

    /// One channel played on two.
    pub fn stereo(&self, src: usize) -> Edit {
        let rows = self.rows(src);
        self.mixed(vec![rows[0].clone(), rows[0].clone()], src)
    }

    /// Left and right swapped.
    pub fn swap(&self, src: usize) -> Edit {
        let mut rows = self.rows(src);
        rows.swap(0, 1);
        self.mixed(rows, src)
    }
}

fn identity(n: usize) -> Vec<Vec<f32>> {
    (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect()
}

/// Merges neighbours that continue each other (same direction, same constant gain) and drops empty pieces.
fn joined(segs: Vec<Seg>) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    for s in segs.into_iter().filter(|s| s.len > 0) {
        match out.last_mut() {
            Some(l) if l.silent && s.silent => l.len += s.len,
            Some(l) if !l.silent && !s.silent && l.rev == s.rev && l.db.0 == l.db.1 && s.db == l.db => {
                if !l.rev && l.src + l.len == s.src {
                    l.len += s.len;
                } else if l.rev && s.src + s.len == l.src {
                    (l.src, l.len) = (s.src, l.len + s.len);
                } else {
                    out.push(s);
                }
            }
            _ => out.push(s),
        }
    }
    out
}

/// The gain factor of `db`.
fn factor(db: f32) -> f32 {
    if db <= FLOOR_DB { 0.0 } else { 10f32.powf(db / 20.0) }
}

/// Every edit since the track was opened, for undo and redo.
pub struct History {
    steps: Vec<Arc<Edit>>,
    at: usize,
    /// The step last saved; `usize::MAX` once that step was undone and overwritten.
    saved: usize,
}

impl History {
    pub fn new(first: Edit) -> History {
        History { steps: vec![Arc::new(first)], at: 0, saved: 0 }
    }

    pub fn current(&self) -> &Arc<Edit> {
        &self.steps[self.at]
    }

    /// Adds an edit after the current one, dropping the steps that were undone.
    pub fn push(&mut self, edit: Edit) {
        self.steps.truncate(self.at + 1);
        if self.saved > self.at {
            self.saved = usize::MAX;
        }
        self.steps.push(Arc::new(edit));
        self.at += 1;
    }

    pub fn can_undo(&self) -> bool {
        self.at > 0
    }

    pub fn can_redo(&self) -> bool {
        self.at + 1 < self.steps.len()
    }

    pub fn undo(&mut self) {
        self.at = self.at.saturating_sub(1);
    }

    pub fn redo(&mut self) {
        if self.can_redo() {
            self.at += 1;
        }
    }

    pub fn dirty(&self) -> bool {
        self.at != self.saved
    }

    /// Marks `edit`'s step as the one saved (it may not be the current one: edits can go on while saving).
    pub fn mark_saved(&mut self, edit: &Arc<Edit>) {
        if let Some(i) = self.steps.iter().position(|s| Arc::ptr_eq(s, edit)) {
            self.saved = i;
        }
    }
}

/// Hands the interleaved samples of edited frames `a..b` to `sink` in blocks, with the gains,
/// directions and mix applied, silence as zeros. It doesn't allocate, so the player calls it.
/// False if part of it isn't decoded (or the track is busy), with what came before already handed over.
pub fn read(track: &Track, edit: &Edit, a: u64, b: u64, sink: impl FnMut(&[f32])) -> bool {
    read_with(track, edit, a, b, sink, |pos| track.chunk_at(pos))
}

/// `read` for everything off the audio thread (save, AI tools): waits while the track is busy,
/// so false means only that part of it isn't decoded.
pub fn read_all(track: &Track, edit: &Edit, a: u64, b: u64, sink: impl FnMut(&[f32])) -> bool {
    read_with(track, edit, a, b, sink, |pos| track.chunk_at_wait(pos))
}

fn read_with(track: &Track, edit: &Edit, a: u64, b: u64, mut sink: impl FnMut(&[f32]), fetch: impl Fn(u64) -> Option<(Arc<[f32]>, usize)>) -> bool {
    const BUF: usize = 8192;
    let src_ch = track.format.get().map_or(1, |f| f.channels);
    let out_ch = edit.channels(src_ch);
    // Symphonia reports at most 32 channels, so a block is at least 256 frames.
    let block = BUF / src_ch.max(out_ch).max(1);
    let (mut buf, mut mixed) = ([0f32; BUF], [0f32; BUF]);
    for seg in edit.slice(a, b) {
        let mut done = 0;
        while done < seg.len {
            let n = ((seg.len - done) as usize).min(block);
            let part = seg.part(done, done + n as u64);
            let frames = &mut buf[..n * src_ch];
            if seg.silent {
                frames.fill(0.0);
            } else {
                let mut got = 0;
                while got < n {
                    let Some((chunk, offset)) = fetch(part.src + got as u64) else { return false };
                    let k = (n - got).min(chunk.len() / src_ch - offset);
                    frames[got * src_ch..(got + k) * src_ch].copy_from_slice(&chunk[offset * src_ch..(offset + k) * src_ch]);
                    got += k;
                }
                if part.rev {
                    // Reversing the samples reverses each frame's channels too; put those back.
                    frames.reverse();
                    frames.chunks_exact_mut(src_ch).for_each(|f| f.reverse());
                }
                if part.db != (0.0, 0.0) {
                    for (j, f) in frames.chunks_exact_mut(src_ch).enumerate() {
                        let g = factor(part.db_at(j as u64));
                        f.iter_mut().for_each(|x| *x *= g);
                    }
                }
            }
            match &edit.mix {
                None => sink(frames),
                Some(rows) => {
                    for (f, out) in frames.chunks_exact(src_ch).zip(mixed.chunks_exact_mut(out_ch)) {
                        for (o, row) in out.iter_mut().zip(rows) {
                            *o = row.iter().zip(f).map(|(w, x)| w * x).sum();
                        }
                    }
                    sink(&mixed[..n * out_ch]);
                }
            }
            done += n as u64;
        }
    }
    true
}

// The measures below wait for the chunk list (read_all): a missed try_lock while playing would
// measure part of the selection, and normalize or trim from that.

/// The largest sample of edited frames `a..b`, as a factor (1.0 is full scale).
pub fn peak(track: &Track, edit: &Edit, a: u64, b: u64) -> f32 {
    let mut peak = 0f32;
    read_all(track, edit, a, b, |s| peak = s.iter().fold(peak, |p, x| p.max(x.abs())));
    peak
}

/// The integrated loudness of edited frames `a..b` in LUFS; −∞ when silent or under 400 ms.
pub fn loudness(track: &Track, edit: &Edit, a: u64, b: u64) -> f64 {
    let Some(format) = track.format.get() else { return f64::NEG_INFINITY };
    let mut meter = crate::loudness::Meter::new(format.rate, edit.channels(format.channels));
    read_all(track, edit, a, b, |s| meter.push(s));
    meter.integrated()
}

/// The frames from the first to the last one louder than `db` within `a..b`; `None` if all are quieter.
pub fn audible(track: &Track, edit: &Edit, a: u64, b: u64, db: f32) -> Option<(u64, u64)> {
    let channels = edit.channels(track.format.get().map_or(1, |f| f.channels));
    let (level, mut frame, mut found) = (10f32.powf(db / 20.0), a, None::<(u64, u64)>);
    read_all(track, edit, a, b, |s| {
        for f in s.chunks_exact(channels) {
            if f.iter().any(|x| x.abs() > level) {
                found = Some((found.map_or(frame, |(first, _)| first), frame + 1));
            }
            frame += 1;
        }
    });
    found
}

/// Lowest and highest sample of edited frames `a..b`, from `peaks` (the track's, one per `PEAK`
/// frames) for pieces at least that long and from the samples themselves for shorter ones, so
/// zoomed-in edges are exact. `None` where nothing is decoded yet.
pub fn min_max(track: &Track, peaks: &[(f32, f32)], edit: &Edit, a: u64, b: u64) -> Option<(f32, f32)> {
    let mut range: Option<(f32, f32)> = None;
    let mut add = |lo: f32, hi: f32| {
        let r = range.get_or_insert((lo, hi));
        *r = (r.0.min(lo), r.1.max(hi));
    };
    let mut pos = a;
    for seg in edit.slice(a, b) {
        if seg.silent {
            add(0.0, 0.0);
        } else if seg.len >= PEAK as u64 {
            // shortcut: peaks ignore the channel mix (a mono mix draws the louder channel); mix them if it looks wrong.
            let g = factor(seg.db.0).max(factor(seg.db.1));
            let from = (seg.src / PEAK as u64) as usize;
            let to = (seg.src + seg.len).div_ceil(PEAK as u64) as usize;
            for &(lo, hi) in peaks.get(from..to.min(peaks.len())).unwrap_or(&[]) {
                add(lo * g, hi * g);
            }
        } else {
            read(track, edit, pos, pos + seg.len, |s| {
                for &x in s {
                    add(x, x);
                }
            });
        }
        pos += seg.len;
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_of_undoes_the_edits() {
        let e = Edit::whole(100).cut(20, 40).silence(10, 15).reverse(50, 80);
        // Before the silence as it was; silence has no source; after the cut, 20 frames on.
        assert_eq!((e.source_of(5), e.source_of(12), e.source_of(30)), (Some(5), None, Some(50)));
        // Reversed 50..80 plays 99 down to 70.
        assert_eq!((e.source_of(50), e.source_of(79)), (Some(99), Some(70)));
        assert_eq!(e.source_of(80), None);
        // Every played frame maps back to itself through crate::ai::edited_beats.
        for pos in (0..80).filter(|&p| e.source_of(p).is_some()) {
            assert_eq!(crate::ai::edited_beats(&[e.source_of(pos).unwrap()], &e), [pos]);
        }
    }

    fn seg(src: u64, len: u64) -> Seg {
        Seg::plain(src, len)
    }

    fn quiet(len: u64) -> Seg {
        Seg::quiet(len)
    }

    /// The edited track read back whole.
    fn all(track: &Track, e: &Edit) -> Vec<f32> {
        let mut got = Vec::new();
        assert!(read(track, e, 0, e.len(), |s| got.extend_from_slice(s)));
        got
    }

    #[test]
    fn cut_crop_silence() {
        let e = Edit::whole(100);
        assert_eq!(e.cut(10, 20).segs, [seg(0, 10), seg(20, 80)]);
        assert_eq!(e.crop(10, 20).segs, [seg(10, 10)]);
        assert_eq!(e.silence(10, 20).segs, [seg(0, 10), quiet(10), seg(20, 80)]);
        // Across pieces, at their edges, and undoing a cut by its pieces joining again.
        let c = e.cut(10, 20);
        assert_eq!(c.cut(5, 15).segs, [seg(0, 5), seg(25, 75)]);
        assert_eq!(c.crop(0, 10).segs, [seg(0, 10)]);
        assert_eq!(c.silence(10, 15).silence(5, 12).segs, [seg(0, 5), quiet(10), seg(25, 75)]);
        assert_eq!(e.silence(90, 200).len(), 100);
        assert_eq!(e.cut(0, 100).segs, []);
    }

    #[test]
    fn gains_stack_into_one_ramp_and_slice() {
        let e = Edit::whole(100).gain(0, 100, -6.0, -6.0).gain(0, 100, FLOOR_DB, 0.0);
        assert_eq!(e.segs, [Seg { db: (-66.0, -6.0), ..seg(0, 100) }]);
        // A cut keeps each side's part of the ramp; undoing the gain joins the pieces again.
        let c = e.cut(25, 75);
        assert_eq!(c.segs[0].db, (-66.0, -51.0));
        assert_eq!(c.segs[1].db, (-21.0, -6.0));
        assert_eq!(Edit::whole(100).gain(10, 20, 3.0, 3.0).gain(10, 20, -3.0, -3.0).segs, [seg(0, 100)]);
        // Silence stays plain silence.
        assert_eq!(Edit::whole(10).silence(0, 10).gain(0, 10, 6.0, 6.0).segs, [quiet(10)]);
    }

    #[test]
    fn read_applies_gain_and_the_floor() {
        let track = Track::from_samples(48000, 1, vec![0.5; 10]);
        let got = all(&track, &Edit::whole(10).gain(0, 5, -6.0206, -6.0206).gain(5, 10, FLOOR_DB, FLOOR_DB));
        assert!(got[..5].iter().all(|&x| (x - 0.25).abs() < 1e-4), "{got:?}");
        assert_eq!(got[5..], [0.0; 5]);
        // A fade in starts silent and rises.
        let got = all(&track, &Edit::whole(10).gain(0, 10, FLOOR_DB, 0.0));
        assert!(got[0] == 0.0 && got.windows(2).all(|w| w[0] <= w[1]) && got[9] > 0.2, "{got:?}");
    }

    #[test]
    fn fades_change_loudness_evenly() {
        let track = Track::from_samples(48000, 1, vec![1.0; 48000]);
        let fade_in = all(&track, &Edit::whole(48000).fade(0, 48000, false));
        let fade_out = all(&track, &Edit::whole(48000).fade(0, 48000, true));
        assert!(fade_in[0] == 0.0 && fade_in.windows(2).all(|w| w[0] <= w[1]));
        for q in [0.25, 0.5, 0.75] {
            let i = (q * 48000.0) as usize;
            // amplitude^0.6 is the loudness heard.
            assert!((fade_in[i].powf(0.6) - q as f32).abs() < 0.03, "in at {q}: {}", fade_in[i]);
            assert!((fade_out[i].powf(0.6) - (1.0 - q) as f32).abs() < 0.03, "out at {q}: {}", fade_out[i]);
        }
        assert!(fade_out[47_999] < 0.01 && fade_out.windows(2).all(|w| w[0] >= w[1]));
        // A fade shorter than its steps still works.
        assert_eq!(Edit::whole(10).fade(0, 10, false).len(), 10);
    }

    #[test]
    fn reverse_plays_backwards_and_twice_undoes_itself() {
        let track = Track::from_samples(48000, 2, (0..20).map(|i| i as f32).collect());
        let e = Edit::whole(10).cut(2, 4);
        let r = e.reverse(1, 6);
        assert_eq!(all(&track, &r), [0., 1., 14., 15., 12., 13., 10., 11., 8., 9., 2., 3., 16., 17., 18., 19.]);
        assert_eq!(r.reverse(1, 6), e);
        // Parts of a reversed piece read the right frames.
        assert_eq!(all(&track, &Edit::whole(10).reverse(0, 10).crop(2, 4)), [14., 15., 12., 13.]);
    }

    #[test]
    fn mono_stereo_swap() {
        let track = Track::from_samples(48000, 2, vec![1.0, 3.0, 2.0, 4.0]);
        let e = Edit::whole(2);
        assert_eq!((all(&track, &e.swap(2)), e.swap(2).channels(2)), (vec![3., 1., 4., 2.], 2));
        assert_eq!(e.swap(2).swap(2), e);
        let m = e.mono(2);
        assert_eq!((all(&track, &m), m.channels(2)), (vec![2., 3.], 1));
        assert_eq!(all(&track, &m.stereo(2)), [2., 2., 3., 3.]);
        // Cuts keep the mix.
        assert_eq!(m.cut(0, 1).mix, m.mix);
    }

    #[test]
    fn measures_peak_loudness_and_the_audible_part() {
        // Silence, a 1 s sine at half scale, silence.
        let mut samples = vec![0.0; 48000];
        samples.extend((0..48000).map(|i| (i as f32 * 0.06).sin() * 0.5));
        samples.extend(vec![0.0; 48000]);
        let track = Track::from_samples(48000, 1, samples);
        let e = Edit::whole(144_000);
        assert!((peak(&track, &e, 0, e.len()) - 0.5).abs() < 1e-3);
        // Normalizing: the gain that takes the peak to −1 dBFS lands it there.
        let d = -1.0 - 20.0 * peak(&track, &e, 0, e.len()).log10();
        let n = e.gain(0, e.len(), d, d);
        assert!((20.0 * peak(&track, &n, 0, n.len()).log10() + 1.0).abs() < 0.01);
        let lufs = loudness(&track, &e, 0, e.len());
        assert!((loudness(&track, &e.gain(0, e.len(), 3.0, 3.0), 0, e.len()) - lufs - 3.0).abs() < 0.01);
        assert_eq!(loudness(&track, &e, 0, 1000), f64::NEG_INFINITY);
        let (first, last) = audible(&track, &e, 0, e.len(), -60.0).unwrap();
        assert!(first >= 48000 && first < 48010 && last > 95_990 && last <= 96_000, "{first} {last}");
        assert_eq!(audible(&track, &e, 0, 48000, -60.0), None);
    }

    #[test]
    fn history_undo_redo_dirty() {
        let mut h = History::new(Edit::whole(100));
        assert!(!h.dirty() && !h.can_undo());
        h.push(h.current().cut(0, 10));
        assert!(h.dirty());
        h.mark_saved(&h.current().clone());
        h.undo();
        assert!(h.dirty() && h.can_redo());
        h.redo();
        assert!(!h.dirty());
        h.undo();
        // A new edit after undo drops the saved step: it can't be reached any more.
        h.push(h.current().cut(0, 20));
        assert!(h.dirty() && !h.can_redo());
        h.undo();
        assert_eq!(h.current().len(), 100);
    }

    #[test]
    fn min_max_reads_samples_and_silence() {
        let samples: Vec<f32> = (0..10_000).map(|i| (i % 100) as f32 / 100.0 - 0.5).collect();
        let track = Track::from_samples(48000, 1, samples);
        let e = Edit::whole(10_000).silence(0, 50);
        let peaks = track.peaks.lock().unwrap().clone();
        assert_eq!(min_max(&track, &peaks, &e, 0, 50), Some((0.0, 0.0)));
        let at = |i: i32| i as f32 / 100.0 - 0.5;
        assert_eq!(min_max(&track, &peaks, &e, 60, 70), Some((at(60), at(69))));
        assert_eq!(min_max(&track, &peaks, &e, 20_000, 20_010), None);
    }

    #[test]
    fn read_hands_over_the_edited_samples() {
        let track = Track::from_samples(48000, 2, (0..20).map(|i| i as f32).collect());
        let e = Edit::whole(10).cut(2, 4).silence(0, 1);
        assert_eq!(all(&track, &e), [0., 0., 2., 3., 8., 9., 10., 11., 12., 13., 14., 15., 16., 17., 18., 19.]);
    }
}
