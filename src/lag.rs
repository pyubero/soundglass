//! Lag, freezes and dropouts, for the log: slow UI frames with where their time went, a watchdog
//! for a frame that doesn't end, frames missing while playing, and the audio callback's troubles.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A frame slower than this is logged, with its phases.
const SLOW: Duration = Duration::from_millis(50);
/// The watchdog logs a frame still running after this, then after twice as long, and so on.
const STUCK: Duration = Duration::from_secs(1);
/// At most one slow-frame line and one sound line per this; the rest are counted.
const EVERY: Duration = Duration::from_secs(1);

struct State {
    frame: u64,
    start: Option<Instant>,
    phase: &'static str,
    /// Each phase and when it ended.
    marks: Vec<(&'static str, Instant)>,
    /// When the last frame ended, and how soon it asked for the next one while focused.
    end: Option<(Instant, Option<Duration>)>,
    slow_logged: Option<Instant>,
    slow_unlogged: u32,
    sound_logged: Option<Instant>,
}

static STATE: Mutex<State> =
    Mutex::new(State { frame: 0, start: None, phase: "", marks: Vec::new(), end: None, slow_logged: None, slow_unlogged: 0, sound_logged: None });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// At the start of each frame. A frame that came much later than asked for while focused is
/// logged: the event loop, the compositor or the GPU held it up, outside our own code.
pub fn frame_start(focused: bool) {
    let now = Instant::now();
    let mut s = state();
    if let Some((end, Some(asked))) = s.end
        && focused
        && now - end > asked + Duration::from_millis(500)
    {
        log::warn!("no frame for {} ms while playing (asked for one after {} ms){}", (now - end).as_millis(), asked.as_millis(), memory());
    }
    s.frame += 1;
    (s.start, s.phase) = (Some(now), "start");
    s.marks.clear();
}

/// Marks where the frame is now; a slow frame's line and the watchdog name the phase.
pub fn phase(name: &'static str) {
    let mut s = state();
    let ended = s.phase;
    s.marks.push((ended, Instant::now()));
    s.phase = name;
}

/// At the end of each frame, with the repaint interval it asked for while focused, if any.
pub fn frame_end(asked: Option<Duration>) {
    let now = Instant::now();
    let mut s = state();
    let Some(start) = s.start.take() else { return };
    let ended = s.phase;
    s.marks.push((ended, now));
    s.end = Some((now, asked));
    if now - start < SLOW {
        return;
    }
    if s.slow_logged.is_some_and(|t| now - t < EVERY) {
        s.slow_unlogged += 1;
        return;
    }
    let (phases, more) = (breakdown(start, &s.marks), std::mem::take(&mut s.slow_unlogged));
    s.slow_logged = Some(now);
    drop(s);
    let more = if more > 0 { format!(" (and {more} slow frames since the last line)") } else { String::new() };
    log::warn!("slow frame {} ms: {phases}{more}", (now - start).as_millis());
}

/// `waveform 80, panels 12`: each phase of 5 ms or more, in ms.
fn breakdown(start: Instant, marks: &[(&str, Instant)]) -> String {
    let mut from = start;
    let parts: Vec<String> = marks
        .iter()
        .filter_map(|&(name, end)| {
            let d = end.saturating_duration_since(from);
            from = end;
            (d >= Duration::from_millis(5)).then(|| format!("{name} {}", d.as_millis()))
        })
        .collect();
    if parts.is_empty() { "spread over every phase".into() } else { parts.join(", ") }
}

/// Starts the thread that logs a frame that doesn't end: a freeze, while it happens.
pub fn watch() {
    let _ = std::thread::Builder::new().name("watchdog".into()).spawn(|| {
        let (mut frame, mut next) = (0, STUCK);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let (f, start, phase) = {
                let s = state();
                (s.frame, s.start, s.phase)
            };
            if f != frame {
                (frame, next) = (f, STUCK);
            }
            if let Some(start) = start
                && start.elapsed() >= next
            {
                log::warn!("UI stuck for {:.1} s in {phase}{}", start.elapsed().as_secs_f32(), memory());
                next *= 2;
            }
        }
    });
}

/// What went wrong in the audio callback since the last look. The callback only adds to these.
#[derive(Default)]
pub struct Sound {
    /// Underruns or overruns cpal reported.
    pub xruns: AtomicU64,
    /// Callbacks that came more than two buffers after the one before, while playing.
    pub late: AtomicU64,
    /// Callbacks that took more than half their buffer's time.
    pub slow: AtomicU64,
    /// Callbacks that had to play silence because the track wasn't decoded there yet or was busy.
    pub starved: AtomicU64,
    pub longest_gap_us: AtomicU64,
    pub slowest_us: AtomicU64,
    pub buffer_us: AtomicU64,
}

impl Sound {
    /// In the callback: notes its start, the last one's start and how long it took.
    pub fn callback(&self, start: Duration, last: Option<Duration>, busy: Duration, buffer: Duration) {
        let us = |d: Duration| d.as_micros() as u64;
        self.buffer_us.store(us(buffer), Relaxed);
        self.slowest_us.fetch_max(us(busy), Relaxed);
        if busy > buffer / 2 {
            self.slow.fetch_add(1, Relaxed);
        }
        if let Some(last) = last {
            let gap = start.saturating_sub(last);
            self.longest_gap_us.fetch_max(us(gap), Relaxed);
            if gap > buffer * 2 + Duration::from_millis(5) {
                self.late.fetch_add(1, Relaxed);
            }
        }
    }

    /// On the UI thread, each frame: logs the troubles, at most once per second.
    pub fn report(&self) {
        {
            let mut s = state();
            if s.sound_logged.is_some_and(|t| t.elapsed() < EVERY) {
                return;
            }
            s.sound_logged = Some(Instant::now());
        }
        let take = |a: &AtomicU64| a.swap(0, Relaxed);
        let (xruns, late, slow, starved) = (take(&self.xruns), take(&self.late), take(&self.slow), take(&self.starved));
        let (gap, slowest) = (take(&self.longest_gap_us), take(&self.slowest_us));
        if xruns + late + slow + starved > 0 {
            let ms = |us: u64| us as f64 / 1000.0;
            log::warn!(
                "sound: underruns {xruns}, late callbacks {late} (longest gap {:.1} ms), slow callbacks {slow} (slowest {:.1} ms), starved callbacks {starved}; buffer {:.1} ms",
                ms(gap),
                ms(slowest),
                ms(self.buffer_us.load(Relaxed))
            );
        }
    }
}

/// `, memory 312 MB`: the app's resident memory, where the system says (Linux).
pub fn memory() -> String {
    // shortcut: Linux only, assuming 4 KB pages; add GetProcessMemoryInfo if Windows lag reports need it.
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(String::new(), |pages| format!(", memory {} MB", pages * 4096 >> 20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_names_the_slow_phases() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        assert_eq!(breakdown(t, &[("start", ms(2)), ("waveform", ms(82)), ("panels", ms(84)), ("windows", ms(96))]), "waveform 80, windows 12");
        assert_eq!(breakdown(t, &[("start", ms(4)), ("panels", ms(8))]), "spread over every phase");
    }

    #[test]
    fn callbacks_count_late_and_slow() {
        let s = Sound::default();
        let (buffer, ms) = (Duration::from_millis(10), Duration::from_millis);
        s.callback(ms(100), Some(ms(90)), ms(1), buffer);
        assert_eq!((s.late.load(Relaxed), s.slow.load(Relaxed)), (0, 0));
        s.callback(ms(140), Some(ms(100)), ms(6), buffer);
        assert_eq!((s.late.load(Relaxed), s.slow.load(Relaxed)), (1, 1));
        assert_eq!((s.longest_gap_us.load(Relaxed), s.slowest_us.load(Relaxed)), (40_000, 6_000));
    }
}
