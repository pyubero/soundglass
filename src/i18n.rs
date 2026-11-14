//! UI translations, as lightview's: the English text is the key, `tr("Save As…")`. English only
//! for now (user's choice). To add a language: a `Language` variant and its name, a match arm in
//! `from_locale` and `source`, a `locales/xx.tsv` with one `English<TAB>translation` line per key
//! (`\n` for a newline), and the language in the tests' `TRANSLATED`; a test then fails until the
//! file has exactly the keys in the code.
//! shortcut: left in English on purpose: units (dB, Hz, kb/s), note names, the AI models' internal
//! errors and licence texts; wrap them too if a translator asks.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
}

impl Language {
    pub const ALL: [Self; 1] = [Self::English];

    /// In its own language, so it can be found whatever the current one is.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
        }
    }

    /// A locale such as `es_ES.UTF-8` or a `LANGUAGE` list such as `de:en`; English if it isn't one of ours.
    fn from_locale(_locale: &str) -> Self {
        Self::English
    }

    /// The translation table, built into the program.
    fn source(self) -> &'static str {
        match self {
            Self::English => "",
        }
    }
}

/// The desktop's language, the way gettext picks it; English if it isn't one of ours.
pub fn system_language() -> Language {
    let locale = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty());
    locale.map_or(Language::English, |l| Language::from_locale(&l))
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static TABLES: [OnceLock<HashMap<String, String>>; Language::ALL.len()] = [const { OnceLock::new() }; Language::ALL.len()];

pub fn set(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

fn parse(source: &str) -> HashMap<String, String> {
    source.lines().filter_map(|l| l.split_once('\t')).map(|(k, v)| (k.replace("\\n", "\n"), v.replace("\\n", "\n"))).collect()
}

fn lookup(language: Language, key: &'static str) -> &'static str {
    if language == Language::English {
        return key;
    }
    let table = TABLES[language as usize].get_or_init(|| parse(language.source()));
    table.get(key).map_or(key, String::as_str)
}

/// Marks text in a `const` table for translation; show it through `tr`.
pub const fn tr_key(key: &'static str) -> &'static str {
    key
}

pub fn tr(key: &'static str) -> &'static str {
    lookup(Language::ALL[CURRENT.load(Ordering::Relaxed) as usize], key)
}

/// `tr` with `{name}` placeholders, so a translation can reorder them.
pub fn trf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(tr(key), args)
}

/// In one pass: a file name containing `{name}` isn't a placeholder.
fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        let value = from_brace.find('}').and_then(|close| args.iter().find(|(n, _)| *n == &from_brace[1..close]).map(|(_, v)| (close, v)));
        match value {
            Some((close, v)) => {
                out.push_str(&v.to_string());
                rest = &from_brace[close + 1..];
            }
            None => {
                out.push('{');
                rest = &from_brace[1..];
            }
        }
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The languages with a table; none yet.
    const TRANSLATED: [Language; 0] = [];

    /// Every literal passed to `tr`/`trf`/`tr_key` in the sources, as written there.
    fn keys_in_sources() -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        let mut files: Vec<_> = std::fs::read_dir("src").unwrap().map(|e| e.unwrap().path()).collect();
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if path.ends_with("i18n.rs") {
                continue;
            }
            let source = std::fs::read_to_string(path).unwrap();
            let code = source.split("#[cfg(test)]").next().unwrap();
            for call in ["tr(\"", "trf(\"", "tr_key(\""] {
                for (at, _) in code.match_indices(call) {
                    if code[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &code[at + call.len()..];
                    let mut end = 0;
                    while rest.as_bytes()[end] != b'"' {
                        end += if rest.as_bytes()[end] == b'\\' { 2 } else { 1 };
                    }
                    keys.insert(rest[..end].replace("\\\"", "\""));
                }
            }
        }
        keys
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{').skip(1).filter_map(|p| p.split_once('}')).map(|(name, _)| name).collect()
    }

    #[test]
    fn every_language_has_exactly_the_strings_in_the_code() {
        let keys = keys_in_sources();
        assert!(keys.len() > 300, "the source scan found only {}", keys.len());
        for language in TRANSLATED {
            let table: BTreeSet<String> = language.source().lines().filter_map(|l| l.split_once('\t')).map(|(k, _)| k.to_string()).collect();
            assert_eq!(table.len(), language.source().lines().count(), "{language:?}: a line is malformed or repeated");
            let missing: Vec<_> = keys.difference(&table).collect();
            let stale: Vec<_> = table.difference(&keys).collect();
            assert!(missing.is_empty() && stale.is_empty(), "{language:?}: missing {missing:#?}, stale {stale:#?}");
        }
    }

    #[test]
    fn keys_name_their_placeholders() {
        // trf's arguments are found by name: `{}` or `{:.1}` would never be filled.
        for key in keys_in_sources() {
            assert!(placeholders(&key).iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_')), "{key}");
        }
    }

    #[test]
    fn unknown_text_stays_english() {
        assert_eq!(lookup(Language::English, "Save As…"), "Save As…");
        assert_eq!(parse("One\\nTwo\tUno\\nDos")["One\nTwo"], "Uno\nDos");
    }

    #[test]
    fn placeholders_fill_once_in_any_order() {
        assert_eq!(fill("{b} then {a}", &[("a", &1), ("b", &"x")]), "x then 1");
        assert_eq!(fill("Saved {name}", &[("name", &"{name}.mp3")]), "Saved {name}.mp3");
        assert_eq!(fill("keeps {other} and {", &[("name", &1)]), "keeps {other} and {");
    }

    #[test]
    fn languages_are_in_declaration_order() {
        assert_eq!(Language::ALL.map(|l| l as usize), std::array::from_fn(|i| i));
        assert_eq!(Language::from_locale("es_ES.UTF-8"), Language::English);
    }
}
