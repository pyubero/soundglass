//! Tempo and key, estimated with bpm-key once a track is decoded.

use std::sync::Arc;

use bpm_key::{Analysis, Config, Key};

use crate::track::Format;

/// What was found; either can be missing (silence, speech, too short).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub bpm: Option<f32>,
    pub key: Option<Key>,
}

// shortcut: only the first 10 minutes are analysed (a mono copy is made); raise it if long pieces need it.
const MAX_SECONDS: usize = 600;

/// Estimates the tempo and key of interleaved `chunks`, at the tempo people tap along to.
pub fn estimate(chunks: &[Arc<[f32]>], format: Format) -> Estimate {
    let channels = format.channels.max(1);
    let max = MAX_SECONDS * format.rate as usize;
    let mono: Vec<f32> = chunks.iter().flat_map(|c| c.chunks_exact(channels)).take(max).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
    let Ok(cfg) = Config::human_tempo().to_builder().only([Analysis::Tempo, Analysis::Key]).build() else { return Estimate::default() };
    // A hostile file must not take soundglass down with it.
    match std::panic::catch_unwind(|| bpm_key::analyze(&mono, 1, format.rate, &cfg)) {
        Ok(Ok(f)) => Estimate { bpm: f.tempo.map(|t| t.bpm), key: f.key.map(|k| k.key) },
        _ => Estimate::default(),
    }
}

/// The major scale (an index into `spectrum::KEYS`, which is its tonic's pitch class) with the
/// notes of `key`: its own, or its relative major's for a minor key.
pub fn major_scale(key: Key) -> usize {
    (key.tonic.index() as usize + if key.is_minor() { 3 } else { 0 }) % 12
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(samples: Vec<f32>) -> Vec<Arc<[f32]>> {
        samples.chunks(crate::track::CHUNK).map(Arc::from).collect()
    }

    #[test]
    fn clicks_at_120_bpm() {
        let rate = 44100;
        // 30 s of 10 ms decaying 1 kHz bursts every half second.
        let s: Vec<f32> = (0..rate * 30)
            .map(|i| {
                let t = (i % (rate / 2)) as f32 / rate as f32;
                if t < 0.01 { (t * 6283.0).sin() * (1.0 - t * 100.0) } else { 0.0 }
            })
            .collect();
        let bpm = estimate(&chunks(s), Format { rate: rate as u32, channels: 1 }).bpm.unwrap();
        assert!((bpm - 120.0).abs() < 2.0, "{bpm}");
    }

    #[test]
    fn garbage_gives_nothing() {
        let s = vec![f32::NAN, f32::INFINITY, 1.0, -1.0].repeat(44100);
        assert_eq!(estimate(&chunks(s), Format { rate: 44100, channels: 2 }), Estimate::default());
        assert_eq!(estimate(&[], Format { rate: 44100, channels: 2 }), Estimate::default());
    }

    #[test]
    fn minor_keys_use_their_relative_major() {
        use bpm_key::{Mode, PitchClass};
        assert_eq!(major_scale(Key::new(PitchClass::A, Mode::Minor)), 0);
        assert_eq!(major_scale(Key::new(PitchClass::F, Mode::Minor)), 8);
        assert_eq!(major_scale(Key::new(PitchClass::EFlat, Mode::Major)), 3);
    }
}
