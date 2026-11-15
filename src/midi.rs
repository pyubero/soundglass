//! Standard MIDI Files played with our own small piano: each note a few decaying harmonics.

use crate::i18n::{tr, trf};
use crate::track::{Info, Tags};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::path::Path;

const RATE: u32 = 44100;
const BLOCK: usize = 4096;
/// Real MIDI files are a few hundred kB; this bounds the event list a hostile file can make.
const MAX_BYTES: u64 = 4 << 20;
const MAX_SECONDS: f64 = 3600.0;
const VOICES: usize = 64;
const HARMONICS: usize = 8;
/// Added after the last event, so the last notes ring out.
const TAIL: f64 = 1.5;
/// Strings' stiffness: partial n sits at n·f·√(1 + B·n²), as on a piano.
const INHARMONICITY: f32 = 0.0004;
const ATTACK: f32 = 0.003;
const RELEASE: f32 = 0.12;

#[derive(Clone, Copy)]
enum Ev {
    Tempo(u32),
    Off(u8, u8),
    Pedal(u8, bool),
    On(u8, u8, u8),
}

impl Ev {
    /// At the same tick: the tempo first, then note offs, so a repeated note restrikes.
    fn order(self) -> u8 {
        match self {
            Ev::Tempo(_) => 0,
            Ev::Off(..) => 1,
            Ev::Pedal(..) => 2,
            Ev::On(..) => 3,
        }
    }
}

struct Voice {
    channel: u8,
    key: u8,
    /// The key is still down; a released key keeps sounding while the pedal is down.
    held: bool,
    releasing: bool,
    age: u32,
    level: f32,
    /// Each harmonic is a phasor `z` turned by `w` every sample; |w| < 1 makes it decay.
    z: [(f32, f32); HARMONICS],
    w: [(f32, f32); HARMONICS],
}

impl Voice {
    fn new(channel: u8, key: u8, velocity: u8) -> Self {
        let f = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
        let v = velocity as f32 / 127.0;
        // Time to fall 60 dB: ~8 s for the lowest key, under 1 s for the highest.
        let t60 = 8.0 * 2f32.powf(-(key as f32 - 21.0).max(0.0) / 24.0);
        // Harder notes are brighter: higher partials fall off less.
        let tilt = 2.2 - v;
        let gains: Vec<f32> = (1..=HARMONICS).map(|n| 1.0 / (n as f32).powf(tilt)).collect();
        let norm = 0.3 * v.powf(1.5) / gains.iter().sum::<f32>();
        let (mut z, mut w) = ([(0.0, 0.0); HARMONICS], [(0.0, 0.0); HARMONICS]);
        for n in 1..=HARMONICS {
            let nf = n as f32;
            let freq = nf * f * (1.0 + INHARMONICITY * nf * nf).sqrt();
            if freq >= RATE as f32 / 2.0 {
                break;
            }
            let r = (-6.9 / (t60 / (1.0 + 0.6 * (nf - 1.0)) * RATE as f32)).exp();
            let theta = std::f32::consts::TAU * freq / RATE as f32;
            z[n - 1] = (gains[n - 1] * norm, 0.0);
            w[n - 1] = (r * theta.cos(), r * theta.sin());
        }
        Voice { channel, key, held: true, releasing: false, age: 0, level: 1.0, z, w }
    }

    fn add_to(&mut self, out: &mut [f32]) {
        let attack = ATTACK * RATE as f32;
        let release = (-6.9 / (RELEASE * RATE as f32)).exp();
        for s in out {
            let mut sum = 0.0;
            for (z, w) in self.z.iter_mut().zip(&self.w) {
                *z = (z.0 * w.0 - z.1 * w.1, z.0 * w.1 + z.1 * w.0);
                sum += z.1;
            }
            if self.releasing {
                self.level *= release;
            }
            *s += sum * self.level * (self.age as f32 / attack).min(1.0);
            self.age = self.age.saturating_add(1);
        }
    }

    fn silent(&self) -> bool {
        self.level * self.z.iter().map(|z| z.0.abs() + z.1.abs()).sum::<f32>() < 1e-5
    }
}

/// Renders the file as mono 44.1 kHz, through `sink` in blocks, as `track::decode` does for audio.
pub fn decode(
    path: &Path,
    on_meta: impl FnOnce(Tags, Info, Option<Box<[u8]>>),
    mut sink: impl FnMut(&[f32], usize, u32, Option<u64>) -> Result<(), String>,
) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err(tr("MIDI file is too big").into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&bytes).map_err(|e| trf("not a supported audio file ({e})", &[("e", &e)]))?;
    let events = timeline(&smf)?;
    let total = events.last().map_or(0, |e| e.0 + (TAIL * RATE as f64) as u64);
    on_meta(Tags::default(), Info { container: "Standard MIDI File".into(), codec: tr("Piano synth").into(), ..Info::default() }, None);

    let mut voices: Vec<Voice> = Vec::with_capacity(VOICES);
    let mut pedal = [false; 16];
    let mut next = events.iter().peekable();
    let mut buf = vec![0.0; BLOCK];
    let mut pos = 0;
    while pos < total {
        let end = (pos + BLOCK as u64).min(total);
        let block = &mut buf[..(end - pos) as usize];
        block.fill(0.0);
        let mut at = pos;
        while at < end {
            while let Some(&(_, ev)) = next.next_if(|e| e.0 <= at) {
                match ev {
                    Ev::On(ch, key, vel) => {
                        if voices.len() == VOICES {
                            voices.remove(0);
                        }
                        voices.push(Voice::new(ch, key, vel));
                    }
                    Ev::Off(ch, key) => {
                        for v in voices.iter_mut().filter(|v| v.channel == ch && v.key == key && v.held) {
                            v.held = false;
                            v.releasing |= !pedal[ch as usize];
                        }
                    }
                    Ev::Pedal(ch, down) => {
                        pedal[ch as usize] = down;
                        for v in voices.iter_mut().filter(|v| v.channel == ch && !v.held) {
                            v.releasing |= !down;
                        }
                    }
                    Ev::Tempo(_) => {}
                }
            }
            let stop = next.peek().map_or(end, |e| e.0.min(end));
            for v in &mut voices {
                v.add_to(&mut block[(at - pos) as usize..(stop - pos) as usize]);
            }
            at = stop;
        }
        // A soft limit, so dense chords bend instead of clipping.
        block.iter_mut().for_each(|s| *s = s.tanh());
        sink(block, 1, RATE, Some(total))?;
        voices.retain(|v| !v.silent());
        pos = end;
    }
    Ok(())
}

/// Every track's notes, pedal and tempo changes in one list, each at its sample frame.
fn timeline(smf: &Smf) -> Result<Vec<(u64, Ev)>, String> {
    let mut ticked: Vec<(u64, Ev)> = Vec::new();
    for track in &smf.tracks {
        let mut tick = 0u64;
        for e in track {
            tick += e.delta.as_int() as u64;
            let ev = match e.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(t)) => Ev::Tempo(t.as_int().max(1)),
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => Ev::On(ch, key.as_int(), vel.as_int()),
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => Ev::Off(ch, key.as_int()),
                        MidiMessage::Controller { controller, value } if controller.as_int() == 64 => Ev::Pedal(ch, value.as_int() >= 64),
                        _ => continue,
                    }
                }
                _ => continue,
            };
            ticked.push((tick, ev));
        }
    }
    ticked.sort_by_key(|&(tick, ev)| (tick, ev.order()));
    // Seconds a tick: fixed for SMPTE timing, following the tempo (µs a beat, 120 BPM at start) otherwise.
    let (ppq, mut tick_seconds) = match smf.header.timing {
        Timing::Metrical(ppq) => (Some(ppq.as_int().max(1) as f64), 0.5 / ppq.as_int().max(1) as f64),
        Timing::Timecode(fps, sub) => (None, 1.0 / (fps.as_f32() as f64 * sub.max(1) as f64)),
    };
    let (mut last_tick, mut seconds) = (0, 0.0);
    let mut out = Vec::with_capacity(ticked.len());
    for (tick, ev) in ticked {
        seconds += (tick - last_tick) as f64 * tick_seconds;
        last_tick = tick;
        if seconds > MAX_SECONDS {
            return Err(tr("MIDI file is too long").into());
        }
        if let (Ev::Tempo(t), Some(ppq)) = (ev, ppq) {
            tick_seconds = t as f64 / 1e6 / ppq;
        }
        out.push(((seconds * RATE as f64).round() as u64, ev));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::tests::temp_file;

    /// A format 0 file: `ppq` ticks a beat, each event its delta ticks and bytes.
    fn smf(ppq: u16, events: &[(u32, &[u8])]) -> Vec<u8> {
        let mut track = Vec::new();
        for &(delta, bytes) in events.iter().chain(&[(0, &[0xFF, 0x2F, 0x00][..])]) {
            let mut v = vec![(delta & 0x7F) as u8];
            let mut d = delta >> 7;
            while d > 0 {
                v.insert(0, (d & 0x7F) as u8 | 0x80);
                d >>= 7;
            }
            track.extend(v);
            track.extend_from_slice(bytes);
        }
        let mut file = b"MThd\0\0\0\x06\0\0\0\x01".to_vec();
        file.extend_from_slice(&ppq.to_be_bytes());
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(track.len() as u32).to_be_bytes());
        file.extend(track);
        file
    }

    fn render(name: &str, bytes: &[u8]) -> Result<Vec<f32>, String> {
        let mut out = Vec::new();
        decode(&temp_file(name, bytes), |_, info, _| assert_eq!(info.container, "Standard MIDI File"), |s, channels, rate, total| {
            assert_eq!((channels, rate), (1, RATE));
            out.extend_from_slice(s);
            assert!(out.len() as u64 <= total.unwrap());
            Ok(())
        })?;
        Ok(out)
    }

    fn first_sound(s: &[f32]) -> usize {
        s.iter().position(|x| x.abs() > 1e-4).unwrap()
    }

    #[test]
    fn plays_a_note_on_its_beat() {
        // 120 BPM: the note starts at 0.5 s and stops at 1 s.
        let s = render("note.mid", &smf(480, &[(480, &[0x90, 60, 100]), (480, &[0x80, 60, 0])])).unwrap();
        assert_eq!(s.len(), ((1.0 + TAIL) * RATE as f64) as usize);
        assert!((22050..22100).contains(&first_sound(&s)));
        assert!(s[23000..44000].iter().any(|x| x.abs() > 0.05));
        assert!(s[s.len() - 1000..].iter().all(|x| x.abs() < 1e-3), "released");
    }

    #[test]
    fn follows_tempo_changes() {
        // 240 BPM from the start: the note starts at 0.25 s.
        let s = render("tempo.mid", &smf(480, &[(0, &[0xFF, 0x51, 0x03, 0x03, 0xD0, 0x90]), (480, &[0x90, 60, 100]), (480, &[0x80, 60, 0])])).unwrap();
        assert!((11025..11075).contains(&first_sound(&s)));
    }

    #[test]
    fn the_pedal_holds_released_notes() {
        let held = |pedal_up: u32| {
            let s = render("pedal.mid", &smf(480, &[(0, &[0xB0, 64, 127]), (0, &[0x90, 48, 100]), (480, &[0x80, 48, 0]), (pedal_up, &[0xB0, 64, 0])])).unwrap();
            s[RATE as usize..RATE as usize + 2000].iter().map(|x| x.abs()).fold(0.0, f32::max)
        };
        assert!(held(960) > 100.0 * held(0), "sounding at 1 s only while the pedal is down");
    }

    #[test]
    fn dense_chords_stay_in_range() {
        let on: Vec<[u8; 3]> = (0..200).map(|i| [0x90 | (i % 16) as u8, 21 + (i % 88) as u8, 127]).collect();
        let events: Vec<(u32, &[u8])> = on.iter().map(|e| (0, &e[..])).collect();
        let s = render("dense.mid", &smf(480, &events)).unwrap();
        assert!(s.iter().all(|x| x.abs() <= 1.0) && s.iter().any(|x| x.abs() > 0.1));
    }

    #[test]
    fn bad_files_are_errors() {
        assert!(render("junk.mid", b"MThd not really a midi file").is_err());
        let file = smf(480, &[(480, &[0x90, 60, 100])]);
        assert!(render("cut.mid", &file[..10]).is_err());
        // 1 tick a beat at 120 BPM, a quarter of a million beats apart: hours of silence.
        let far = smf(1, &[(0x0FFF_FFFF, &[0x90, 60, 100])]);
        assert_eq!(render("far.mid", &far).unwrap_err(), "MIDI file is too long");
        assert!(render("empty.mid", &smf(480, &[])).unwrap().is_empty());
    }
}
