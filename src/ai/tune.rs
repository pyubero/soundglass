//! Each model's few settings (AI Models → Settings), saved per model file, as lightview's
//! preprocessing overrides. Every field falls back to its default and is clamped on use.
use super::*;
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DenoiseTune {
    /// At most this much noise reduction, in dB; 0 is the model's full effect.
    pub limit_db: f32,
}

impl Default for DenoiseTune {
    fn default() -> Self {
        DenoiseTune { limit_db: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StemsTune {
    /// How much neighbouring segments overlap, 0.1–0.5.
    pub overlap: f32,
    /// Which of drums, bass, other and vocals to write.
    pub write: [bool; 4],
}

impl Default for StemsTune {
    fn default() -> Self {
        StemsTune { overlap: 0.25, write: [true; 4] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiTune {
    pub onset: f32,
    pub frame: f32,
    pub min_note_ms: f32,
    /// MIDI numbers, A0 (21) to C8 (108).
    pub low: u8,
    pub high: u8,
    pub melodia: bool,
    pub tempo: f32,
}

impl Default for MidiTune {
    fn default() -> Self {
        MidiTune { onset: 0.5, frame: 0.3, min_note_ms: 127.7, low: 21, high: 108, melodia: true, tempo: 120.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeatsTune {
    /// The logit a peak must pass, −3…3; lower finds more beats.
    pub threshold: f32,
}

impl Default for BeatsTune {
    fn default() -> Self {
        BeatsTune { threshold: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechTune {
    /// Whisper: a language code ("en", "es"…); empty to detect it.
    pub language: String,
    /// Whisper: translate into English instead of transcribing.
    pub translate: bool,
    /// Pieces quieter than this (RMS, dBFS) are skipped.
    pub silence_db: f32,
    /// The transcript as subtitles (.srt), with each stretch's start and end.
    pub time_marks: bool,
}

impl Default for SpeechTune {
    fn default() -> Self {
        SpeechTune { language: String::new(), translate: false, silence_db: -60.0, time_marks: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tune {
    Denoise(DenoiseTune),
    Stems(StemsTune),
    Midi(MidiTune),
    Beats(BeatsTune),
    Speech(SpeechTune),
}

impl Tune {
    fn default_for(task: Task) -> Tune {
        match task {
            Task::Denoise => Tune::Denoise(Default::default()),
            Task::Stems => Tune::Stems(Default::default()),
            Task::Midi => Tune::Midi(Default::default()),
            Task::Beats => Tune::Beats(Default::default()),
            Task::Transcribe => Tune::Speech(Default::default()),
        }
    }

    /// Back within range: the file can be edited by hand.
    fn checked(self) -> Tune {
        match self {
            Tune::Denoise(t) => Tune::Denoise(DenoiseTune { limit_db: if t.limit_db.is_finite() && t.limit_db > 0.0 { t.limit_db.clamp(6.0, 60.0) } else { 0.0 } }),
            Tune::Stems(t) => Tune::Stems(StemsTune { overlap: if t.overlap.is_finite() { t.overlap.clamp(0.1, 0.5) } else { 0.25 }, ..t }),
            Tune::Midi(t) => {
                let d = MidiTune::default();
                let fit = |x: f32, lo: f32, hi: f32, or: f32| if x.is_finite() { x.clamp(lo, hi) } else { or };
                let (low, high) = (t.low.clamp(21, 108), t.high.clamp(21, 108));
                Tune::Midi(MidiTune {
                    onset: fit(t.onset, 0.05, 0.95, d.onset),
                    frame: fit(t.frame, 0.05, 0.95, d.frame),
                    min_note_ms: fit(t.min_note_ms, 30.0, 500.0, d.min_note_ms),
                    low: low.min(high),
                    high: high.max(low),
                    melodia: t.melodia,
                    tempo: fit(t.tempo, 40.0, 240.0, d.tempo),
                })
            }
            Tune::Beats(t) => Tune::Beats(BeatsTune { threshold: if t.threshold.is_finite() { t.threshold.clamp(-3.0, 3.0) } else { 0.0 } }),
            Tune::Speech(t) => {
                let language: String = t.language.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_lowercase()).take(8).collect();
                Tune::Speech(SpeechTune { language, silence_db: if t.silence_db.is_finite() { t.silence_db.clamp(-80.0, -30.0) } else { -60.0 }, ..t })
            }
        }
    }
}

/// The settings `model` runs with: the saved ones when they're for its kind, else the defaults.
pub fn tune_for(model: &Known, s: &crate::settings::AiSettings) -> Tune {
    let default = Tune::default_for(model.task);
    s.tune.get(model.file).filter(|t| std::mem::discriminant(*t) == std::mem::discriminant(&default)).cloned().unwrap_or(default).checked()
}

/// One model's settings; a change is saved for its file, Reset to defaults removes it.
pub(super) fn tune_ui(ui: &mut egui::Ui, s: &mut crate::settings::AiSettings, model: &Known) {
    let before = tune_for(model, s);
    let mut t = before.clone();
    let whisper = model.data.len() == 2;
    egui::Grid::new(("tune", model.file)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        match &mut t {
            Tune::Denoise(d) => {
                ui.label("Strength");
                ui.horizontal(|ui| {
                    let mut full = d.limit_db == 0.0;
                    if ui.checkbox(&mut full, "Full").changed() {
                        d.limit_db = if full { 0.0 } else { 24.0 };
                    }
                    if !full {
                        ui.add(egui::DragValue::new(&mut d.limit_db).range(6.0..=60.0).suffix(" dB at most"));
                    }
                })
                .response
                .on_hover_text("Full takes away all the noise it finds; a limit keeps some, for a more natural sound");
            }
            Tune::Stems(st) => {
                ui.label("Overlap");
                ui.add(egui::Slider::new(&mut st.overlap, 0.1..=0.5).custom_formatter(|v, _| format!("{:.0} %", v * 100.0))).on_hover_text("More overlap between the 7.8 s segments: smoother joins, slower");
                ui.end_row();
                ui.label("Write");
                ui.horizontal(|ui| {
                    for (on, name) in st.write.iter_mut().zip(stems::NAMES) {
                        ui.checkbox(on, name);
                    }
                });
            }
            Tune::Midi(m) => {
                ui.label("Onset threshold");
                ui.add(egui::Slider::new(&mut m.onset, 0.05..=0.95)).on_hover_text("Higher: fewer, surer notes start");
                ui.end_row();
                ui.label("Note threshold");
                ui.add(egui::Slider::new(&mut m.frame, 0.05..=0.95)).on_hover_text("Higher: notes end sooner, quiet ones are left out");
                ui.end_row();
                ui.label("Shortest note");
                ui.add(egui::DragValue::new(&mut m.min_note_ms).range(30.0..=500.0).suffix(" ms"));
                ui.end_row();
                ui.label("Notes from");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut m.low).range(21..=108).custom_formatter(|v, _| note_name(v as u8)));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut m.high).range(21..=108).custom_formatter(|v, _| note_name(v as u8)));
                });
                ui.end_row();
                ui.label("Held notes");
                ui.checkbox(&mut m.melodia, "Also find notes without a clear start").on_hover_text("Basic Pitch's melodia trick");
                ui.end_row();
                ui.label("MIDI tempo");
                ui.add(egui::DragValue::new(&mut m.tempo).range(40.0..=240.0).suffix(" bpm")).on_hover_text("Only how the file's beats are counted; the notes keep their times");
            }
            Tune::Beats(b) => {
                ui.label("Sensitivity");
                ui.add(egui::Slider::new(&mut b.threshold, -3.0..=3.0)).on_hover_text("Lower finds more beats, also where the model is unsure");
            }
            Tune::Speech(sp) => {
                if whisper {
                    ui.label("Language");
                    ui.add(egui::TextEdit::singleline(&mut sp.language).hint_text("detect").desired_width(60.0)).on_hover_text("A code such as en, es, fr, de; empty to detect it from the first speech");
                    ui.end_row();
                    ui.label("Task");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut sp.translate, false, "Transcribe");
                        ui.radio_value(&mut sp.translate, true, "Translate to English");
                    });
                    ui.end_row();
                }
                ui.label("Time marks");
                ui.checkbox(&mut sp.time_marks, "As subtitles (.srt)").on_hover_text("Each stretch of speech with its start and end, to save as an .srt file");
                ui.end_row();
                ui.label("Skip quieter than");
                ui.add(egui::DragValue::new(&mut sp.silence_db).range(-80.0..=-30.0).suffix(" dBFS")).on_hover_text("Silent pieces are left out, so nothing is made up there");
            }
        }
        ui.end_row();
    });
    if t != before {
        s.tune.insert(model.file.to_string(), t);
    }
    if ui.add_enabled(s.tune.contains_key(model.file), egui::Button::new("Reset to defaults")).clicked() {
        s.tune.remove(model.file);
    }
}

/// "C4", "A♯0"… with sharps written as # (the bundled font has no ♯).
fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AiSettings;

    #[test]
    fn tunes_are_per_model_clamped_and_of_their_kind() {
        let midi = CATALOG.iter().find(|k| k.task == Task::Midi).unwrap();
        let mut s = AiSettings::default();
        assert_eq!(tune_for(midi, &s), Tune::Midi(MidiTune::default()));
        s.tune.insert(midi.file.into(), Tune::Midi(MidiTune { onset: 9.0, low: 100, high: 30, tempo: f32::NAN, ..Default::default() }));
        let Tune::Midi(m) = tune_for(midi, &s) else { panic!() };
        assert_eq!((m.onset, m.low, m.high, m.tempo), (0.95, 30, 100, 120.0));
        // A setting of the wrong kind (edited by hand) gives the defaults.
        s.tune.insert(midi.file.into(), Tune::Beats(BeatsTune { threshold: 1.0 }));
        assert_eq!(tune_for(midi, &s), Tune::Midi(MidiTune::default()));
        let speech = CATALOG.iter().find(|k| k.task == Task::Transcribe).unwrap();
        s.tune.insert(speech.file.into(), Tune::Speech(SpeechTune { language: " EN ".into(), silence_db: -200.0, ..Default::default() }));
        assert_eq!(tune_for(speech, &s), Tune::Speech(SpeechTune { language: "en".into(), silence_db: -80.0, translate: false, time_marks: false }));
        assert_eq!((note_name(21), note_name(60), note_name(108)), ("A0".to_string(), "C4".to_string(), "C8".to_string()));
    }
}
