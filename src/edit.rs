//! Edits as a list of pieces of the original track: its audio in memory is never changed, and an
//! undo step is just an older list.

use crate::track::{PEAK, Track};
use std::sync::Arc;

/// `len` frames of the original from frame `src`, or `len` frames of silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub src: u64,
    pub len: u64,
    pub silent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub segs: Vec<Seg>,
}

impl Edit {
    /// The whole track, unedited.
    pub fn whole(len: u64) -> Edit {
        Edit::joined(vec![Seg { src: 0, len, silent: false }])
    }

    pub fn len(&self) -> u64 {
        self.segs.iter().map(|s| s.len).sum()
    }

    /// The original's frame at `pos` (`None` in silence) and the frames left in its piece;
    /// `None` past the end.
    pub fn locate(&self, pos: u64) -> Option<(Option<u64>, u64)> {
        let mut start = 0;
        for s in &self.segs {
            if pos < start + s.len {
                let off = pos - start;
                return Some(((!s.silent).then_some(s.src + off), s.len - off));
            }
            start += s.len;
        }
        None
    }

    /// The pieces that make up frames `a..b`.
    fn slice(&self, a: u64, b: u64) -> Vec<Seg> {
        let (mut out, mut start) = (Vec::new(), 0);
        for s in &self.segs {
            let (lo, hi) = (a.max(start), b.min(start + s.len));
            if lo < hi {
                out.push(Seg { src: s.src + (lo - start), len: hi - lo, silent: s.silent });
            }
            start += s.len;
        }
        out
    }

    /// Without frames `a..b`.
    pub fn cut(&self, a: u64, b: u64) -> Edit {
        let mut segs = self.slice(0, a);
        segs.extend(self.slice(b, self.len()));
        Edit::joined(segs)
    }

    /// Only frames `a..b`.
    pub fn crop(&self, a: u64, b: u64) -> Edit {
        Edit::joined(self.slice(a, b))
    }

    /// Frames `a..b` replaced with silence of the same length.
    pub fn silence(&self, a: u64, b: u64) -> Edit {
        let (len, b) = (self.len(), b.min(self.len()));
        let mut segs = self.slice(0, a);
        segs.push(Seg { src: 0, len: b.saturating_sub(a), silent: true });
        segs.extend(self.slice(b, len));
        Edit::joined(segs)
    }

    /// Merges neighbours that continue each other and drops empty pieces.
    fn joined(segs: Vec<Seg>) -> Edit {
        let mut out: Vec<Seg> = Vec::new();
        for s in segs.into_iter().filter(|s| s.len > 0) {
            match out.last_mut() {
                Some(l) if l.silent && s.silent => l.len += s.len,
                Some(l) if !l.silent && !s.silent && l.src + l.len == s.src => l.len += s.len,
                _ => out.push(s),
            }
        }
        Edit { segs: out }
    }
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

/// Hands the interleaved samples of edited frames `a..b` to `sink` in blocks, silence as zeros.
/// False if part of it isn't decoded (or the track is busy), with what came before already handed over.
pub fn read(track: &Track, edit: &Edit, a: u64, b: u64, mut sink: impl FnMut(&[f32])) -> bool {
    let channels = track.format.get().map_or(1, |f| f.channels);
    let zeros = vec![0.0; 4096 * channels];
    for seg in edit.slice(a, b) {
        let mut left = seg.len as usize;
        while left > 0 {
            let n = if seg.silent {
                let n = left.min(4096);
                sink(&zeros[..n * channels]);
                n
            } else {
                let src = seg.src + (seg.len - left as u64);
                let Some((chunk, offset)) = track.chunk_at(src) else { return false };
                let n = left.min(chunk.len() / channels - offset);
                sink(&chunk[offset * channels..(offset + n) * channels]);
                n
            };
            left -= n;
        }
    }
    true
}

/// Lowest and highest sample of edited frames `a..b`, from `peaks` (the track's) for long pieces
/// and from the samples themselves for short ones, so zoomed-in edges are exact. `None` where
/// nothing is decoded yet.
pub fn min_max(track: &Track, peaks: &[(f32, f32)], edit: &Edit, a: u64, b: u64) -> Option<(f32, f32)> {
    let mut range: Option<(f32, f32)> = None;
    let mut add = |lo: f32, hi: f32| {
        let r = range.get_or_insert((lo, hi));
        *r = (r.0.min(lo), r.1.max(hi));
    };
    for seg in edit.slice(a, b) {
        if seg.silent {
            add(0.0, 0.0);
        } else if seg.len >= 4 * PEAK as u64 {
            let from = (seg.src / PEAK as u64) as usize;
            let to = (seg.src + seg.len).div_ceil(PEAK as u64) as usize;
            for &(lo, hi) in peaks.get(from..to.min(peaks.len())).unwrap_or(&[]) {
                add(lo, hi);
            }
        } else {
            read(track, &Edit { segs: vec![seg] }, 0, seg.len, |s| {
                for &x in s {
                    add(x, x);
                }
            });
        }
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(src: u64, len: u64) -> Seg {
        Seg { src, len, silent: false }
    }

    fn quiet(len: u64) -> Seg {
        Seg { src: 0, len, silent: true }
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
    fn locate_maps_through_the_pieces() {
        let e = Edit::whole(100).silence(10, 20).cut(30, 40);
        assert_eq!(e.locate(5), Some((Some(5), 5)));
        assert_eq!(e.locate(12), Some((None, 8)));
        assert_eq!(e.locate(30), Some((Some(40), 60)));
        assert_eq!(e.locate(90), None);
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
        let mut got = Vec::new();
        assert!(read(&track, &e, 0, e.len(), |s| got.extend_from_slice(s)));
        assert_eq!(got, [0., 0., 2., 3., 8., 9., 10., 11., 12., 13., 14., 15., 16., 17., 18., 19.]);
    }
}
