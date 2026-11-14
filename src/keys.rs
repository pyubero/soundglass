//! Keyboard shortcuts: every menu command, its default keys, and the user's changes from Settings →
//! Change Keyboard Shortcuts…, as lightview's `menu.rs`.

use crate::ai::Task;
use crate::{FADES, Fx, GAINS, Knob, LOUDNESS, PEAKS, TRIMS};
use crate::i18n::{tr, tr_key, trf};
use eframe::egui::{Key, KeyboardShortcut, Modifiers};

/// A menu item, as a key can run it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Open,
    Close,
    ShowInFolder,
    CopyPath,
    CopyFile,
    SaveCopy,
    SaveAs,
    Rename,
    Delete,
    RenameFromTags,
    Identify,
    Settings,
    Quit,
    Undo,
    Redo,
    Cut,
    Crop,
    Silence,
    AddCover,
    RemoveCover,
    Fx(Fx),
    /// The Custom… item that opens with this effect.
    CustomFx(Fx),
    TrackInfo,
    Folder,
    EditPanel,
    FullScreen,
    Practice,
    Speed(Knob, f64),
    CustomSpeed(Knob),
    ResetSpeed,
    Ai(Task),
    AiModels,
    Metronome,
    Spectrogram,
}

const fn key(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}
const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
const NONE: Modifiers = Modifiers::NONE;

/// The keys out of the box.
const DEFAULTS: [(KeyboardShortcut, Command); 17] = [
    (key(CTRL, Key::O), Command::Open),
    (key(CTRL, Key::W), Command::Close),
    (key(CTRL, Key::S), Command::SaveCopy),
    (key(CTRL_SHIFT, Key::S), Command::SaveAs),
    (key(NONE, Key::F2), Command::Rename),
    (key(CTRL, Key::Comma), Command::Settings),
    (key(CTRL, Key::Q), Command::Quit),
    (key(CTRL, Key::Z), Command::Undo),
    (key(CTRL_SHIFT, Key::Z), Command::Redo),
    (key(CTRL, Key::Y), Command::Redo),
    (key(NONE, Key::Delete), Command::Cut),
    (key(CTRL, Key::T), Command::Crop),
    (key(CTRL, Key::L), Command::Silence),
    (key(CTRL, Key::I), Command::TrackInfo),
    (key(CTRL, Key::B), Command::Folder),
    (key(CTRL, Key::E), Command::EditPanel),
    (key(NONE, Key::F11), Command::FullScreen),
];

/// Keys no command can have: the player's (Space, ←, →, Page Up/Down, L), Esc, Enter, and text copy and paste.
const FIXED: [KeyboardShortcut; 11] = [
    key(NONE, Key::Space),
    key(NONE, Key::ArrowLeft),
    key(NONE, Key::ArrowRight),
    key(NONE, Key::PageUp),
    key(NONE, Key::PageDown),
    key(NONE, Key::L),
    key(NONE, Key::Escape),
    key(NONE, Key::Enter),
    key(CTRL, Key::C),
    key(CTRL, Key::X),
    key(CTRL, Key::V),
];

/// Every command by menu, in menu order, with the name the shortcuts list shows.
pub fn bindable_menus() -> Vec<(&'static str, Vec<(Command, String)>)> {
    use Command::*;
    let named = |items: &[(Command, &str)]| items.iter().map(|&(c, n)| (c, n.to_string())).collect::<Vec<_>>();
    let mut effects = Vec::new();
    for (name, fade) in [(tr("Fade in"), crate::Fx::FadeIn as fn(Option<f64>) -> crate::Fx), (tr("Fade out"), crate::Fx::FadeOut)] {
        effects.push((Fx(fade(None)), trf("{name}: whole selection", &[("name", &name)])));
        effects.extend(FADES.map(|s| (Fx(fade(Some(s))), trf("{name}: {s} s", &[("name", &name), ("s", &s)]))));
        effects.push((CustomFx(fade(None)), trf("{name}: custom…", &[("name", &name)])));
    }
    effects.extend(GAINS.map(|db| (Fx(crate::Fx::Gain(db)), trf("Gain: {db} dB", &[("db", &format!("{}{}", if db > 0.0 { "+" } else { "" }, crate::signed(db)))]))));
    effects.push((CustomFx(crate::Fx::Gain(0.0)), tr("Gain: custom…").into()));
    effects.extend(PEAKS.map(|(db, n)| (Fx(crate::Fx::Peak(db)), trf("Normalize: {name} ({db} dBFS)", &[("name", &tr(n)), ("db", &crate::signed(db))]))));
    effects.extend(LOUDNESS.map(|(l, n)| (Fx(crate::Fx::Loudness(l)), trf("Normalize: {name} ({lufs} LUFS)", &[("name", &tr(n)), ("lufs", &crate::signed(l))]))));
    effects.push((CustomFx(crate::Fx::Peak(0.0)), tr("Normalize: custom peak…").into()));
    effects.push((CustomFx(crate::Fx::Loudness(0.0)), tr("Normalize: custom loudness…").into()));
    effects.extend(TRIMS.map(|(db, n)| (Fx(crate::Fx::Trim(db)), trf("Trim silence: {name} ({db} dB)", &[("name", &tr(n)), ("db", &crate::signed(db))]))));
    effects.push((CustomFx(crate::Fx::Trim(0.0)), tr("Trim silence: custom…").into()));
    effects.extend(named(&[(Fx(crate::Fx::Reverse), tr("Reverse")), (Fx(crate::Fx::Mono), tr("Channels: mono")), (Fx(crate::Fx::Stereo), tr("Channels: stereo")), (Fx(crate::Fx::Swap), tr("Channels: swap left and right"))]));
    let mut playback = Vec::new();
    for knob in Knob::ALL {
        playback.extend(knob.presets().iter().map(|&v| (Speed(knob, v), format!("{}: {}", knob.title(), knob.label(v)))));
        playback.push((CustomSpeed(knob), trf("{name}: custom…", &[("name", &knob.title())])));
    }
    playback.push((ResetSpeed, tr("Reset").into()));
    let mut ai: Vec<(Command, String)> = Task::ALL.iter().map(|&t| (Ai(t), t.title().to_string())).collect();
    ai.push((AiModels, tr("AI Models…").into()));
    vec![
        (tr_key("File"), named(&[(Open, tr("Open…")), (Close, tr("Close")), (ShowInFolder, if cfg!(windows) { tr("Show in Explorer") } else { tr("Show in File Manager") }), (CopyPath, tr("Copy Path")), (CopyFile, tr("Copy File")), (SaveCopy, tr("Save Copy")), (SaveAs, tr("Save As…")), (Rename, tr("Rename…")), (Delete, tr("Delete…")), (RenameFromTags, tr("Rename from Tags…")), (Identify, tr("Identify Track…")), (Settings, tr("Settings…")), (Quit, tr("Quit"))])),
        (tr_key("Edit"), named(&[(Undo, tr("Undo")), (Redo, tr("Redo")), (Cut, tr("Cut out")), (Crop, tr("Crop")), (Silence, tr("Silence")), (AddCover, tr("Add/Replace Album Cover…")), (RemoveCover, tr("Remove Album Cover…"))])),
        (tr_key("Effects"), effects),
        (tr_key("View"), named(&[(TrackInfo, tr("Track Info")), (Folder, tr("Folder")), (EditPanel, tr("Edit")), (FullScreen, tr("Full Screen")), (Practice, tr("Practice mode"))])),
        (tr_key("Playback"), playback),
        (tr_key("Local AI"), ai),
        (tr_key("Practice"), named(&[(Metronome, tr("Metronome")), (Spectrogram, tr("Spectrogram"))])),
    ]
}

/// The saved name of a command: stable as long as the command's variant and values are.
pub fn id(command: Command) -> String {
    format!("{command:?}")
}

/// The keys in use: the defaults of unchanged commands, then the user's, most modifiers first, so
/// Ctrl+Shift+Z is matched before Ctrl+Z (egui lets a shortcut without Shift match with it).
pub fn shortcuts(changes: &[(String, Option<KeyboardShortcut>)]) -> Vec<(KeyboardShortcut, Command)> {
    let change = |c: Command| changes.iter().find(|(n, _)| *n == id(c)).map(|(_, sc)| *sc);
    let mut keys: Vec<(KeyboardShortcut, Command)> = DEFAULTS.iter().filter(|(_, c)| change(*c).is_none()).copied().collect();
    keys.extend(bindable_menus().into_iter().flat_map(|(_, items)| items).filter_map(|(c, _)| Some((change(c)??, c))));
    let count = |m: Modifiers| m.alt as u8 + m.shift as u8 + (m.ctrl || m.command) as u8 + m.mac_cmd as u8;
    keys.sort_by_key(|(sc, _)| std::cmp::Reverse(count(sc.modifiers)));
    keys
}

/// Why `sc` can't be given to `command`: it's fixed, or the name of the command that has it.
pub fn taken(keys: &[(KeyboardShortcut, Command)], sc: KeyboardShortcut, command: Command) -> Option<String> {
    if FIXED.contains(&sc) {
        return Some(tr("Space, ←, →, Page Up, Page Down, L, Esc, Enter and Ctrl+C/X/V").into());
    }
    let (_, other) = keys.iter().find(|(k, c)| *k == sc && *c != command)?;
    Some(bindable_menus().into_iter().flat_map(|(_, items)| items).find(|(c, _)| c == other).map_or_else(|| id(*other), |(_, name)| name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_shortcuts_first_and_changes_replace_defaults() {
        let keys = shortcuts(&[]);
        let at = |c: Command| keys.iter().position(|(_, k)| *k == c).unwrap();
        assert!(at(Command::Redo) < at(Command::Undo) && at(Command::SaveAs) < at(Command::SaveCopy));
        // A changed command loses all its defaults (Redo has two); a removed one has none.
        let mine = key(Modifiers::ALT, Key::R);
        let keys = shortcuts(&[(id(Command::Redo), Some(mine)), (id(Command::Cut), None), (id(Command::Fx(Fx::Reverse)), Some(key(CTRL, Key::R)))]);
        let of = |c: Command| keys.iter().filter(|(_, k)| *k == c).map(|(sc, _)| *sc).collect::<Vec<_>>();
        assert_eq!((of(Command::Redo), of(Command::Cut), of(Command::Fx(Fx::Reverse))), (vec![mine], vec![], vec![key(CTRL, Key::R)]));
    }

    #[test]
    fn taken_keys_are_refused() {
        let keys = shortcuts(&[]);
        assert_eq!(taken(&keys, key(CTRL, Key::O), Command::Quit).as_deref(), Some("Open…"));
        assert_eq!(taken(&keys, key(CTRL, Key::O), Command::Open), None);
        assert!(taken(&keys, key(NONE, Key::Space), Command::Quit).is_some());
        assert_eq!(taken(&keys, key(CTRL, Key::J), Command::Quit), None);
    }

    #[test]
    fn every_default_is_listed_and_names_are_unique() {
        let all: Vec<(Command, String)> = bindable_menus().into_iter().flat_map(|(_, items)| items).collect();
        assert!(DEFAULTS.iter().all(|(_, c)| all.iter().any(|(a, _)| a == c)));
        for (i, (c, _)) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|(d, _)| id(*d) != id(*c)), "{c:?} twice");
        }
    }
}
