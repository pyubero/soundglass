//! Identify Track: a Chromaprint fingerprint looked up in AcoustID, the genre from MusicBrainz.
//! Online only when asked: AcoustID gets the fingerprint, the length and the app key;
//! MusicBrainz gets the chosen recording's ID. Nothing else leaves the computer.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};
use serde_json::Value;

use crate::tags::TagEdit;
use crate::track::Track;
use crate::i18n::{tr, trf};

/// A URL for soundglass, sent in the User-Agent as MusicBrainz asks.
const CONTACT: &str = "https://github.com/pyubero/soundglass";
/// Where a user registers the AcoustID application key that Settings asks for.
pub const KEY_PAGE: &str = "https://acoustid.org/new-application";

/// Seconds of audio fingerprinted, as fpcalc does.
const SECONDS: usize = 120;
/// The most results shown.
const MAX_MATCHES: usize = 10;
const REPLY_LIMIT: u64 = 2_000_000;

/// Whether Settings has an AcoustID key, so lookups may run.
pub fn ready(key: &str) -> bool {
    !key.trim().is_empty()
}

/// One way the file could be tagged: a recording on one release.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    /// AcoustID's score, 0 to 1.
    pub score: f32,
    /// The MusicBrainz recording ID.
    pub recording: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track: String,
}

/// The compressed, base64 fingerprint (as fpcalc prints it) of the first two minutes, and the
/// track's length in whole seconds.
pub fn fingerprint(track: &Track) -> Result<(String, u32), String> {
    let format = *track.format.get().ok_or(tr("The file has no audio."))?;
    let config = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&config);
    printer.start(format.rate, format.channels as u32).map_err(|e| trf("Can't fingerprint this audio ({e})", &[("e", &format!("{e:?}"))]))?;
    let max = SECONDS * format.rate as usize * format.channels;
    let mut fed = 0;
    let mut buf = Vec::new();
    for chunk in track.chunks() {
        let take = chunk.len().min(max - fed);
        buf.clear();
        buf.extend(chunk[..take].iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        printer.consume(&buf);
        fed += take;
        if fed == max {
            break;
        }
    }
    printer.finish();
    if printer.fingerprint().is_empty() {
        return Err(tr("The file is too short to identify.").into());
    }
    let compressed = FingerprintCompressor::from(&config).compress(printer.fingerprint());
    let secs = (track.len() / format.rate.max(1) as u64) as u32;
    Ok((base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed), secs))
}

/// Looks the fingerprint up in AcoustID: the matches, best first.
pub fn lookup(key: &str, fingerprint: &str, secs: u32) -> Result<Vec<Match>, String> {
    wait_turn();
    let form = [("client", key.trim()), ("duration", &secs.to_string()), ("fingerprint", fingerprint), ("meta", "recordings releases tracks")];
    let reply = agent().post("https://api.acoustid.org/v2/lookup").send_form(form).map_err(|e| format!("AcoustID can't be reached ({e})"))?;
    matches(&read(reply)?)
}

/// The genre of MusicBrainz recording `id` with the most votes.
pub fn genre(id: &str) -> Result<Option<String>, String> {
    // It goes into the URL: a MusicBrainz ID is hex digits and dashes.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Ok(None);
    }
    wait_turn();
    let url = format!("https://musicbrainz.org/ws/2/recording/{id}?inc=genres&fmt=json");
    let reply = agent().get(&url).call().map_err(|e| trf("MusicBrainz can't be reached ({e})", &[("e", &e)]))?;
    Ok(top_genre(&read(reply)?))
}

/// `read`'s tags with match `m` (and `genre`) filled in: only the empty fields, or all of them.
pub fn fill(read: &TagEdit, m: &Match, genre: Option<&str>, only_empty: bool) -> TagEdit {
    let mut t = read.clone();
    let found = [(&mut t.title, &m.title), (&mut t.artist, &m.artist), (&mut t.album, &m.album), (&mut t.year, &m.year), (&mut t.track, &m.track)];
    for (field, value) in found.into_iter().chain([(&mut t.genre, &genre.unwrap_or_default().to_string())]) {
        if !value.is_empty() && (!only_empty || field.trim().is_empty()) {
            field.clone_from(value);
        }
    }
    t
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("soundglass/{} ( {CONTACT} )", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The reply as JSON, at most `REPLY_LIMIT` bytes.
fn read(mut reply: ureq::http::Response<ureq::Body>) -> Result<Value, String> {
    let text = reply.body_mut().with_config().limit(REPLY_LIMIT).read_to_string().map_err(|e| trf("The reply couldn't be read ({e})", &[("e", &e)]))?;
    serde_json::from_str(&text).map_err(|_| tr("The reply wasn't understood.").to_string())
}

/// Keeps requests a second apart: AcoustID allows 3 a second, MusicBrainz 1.
fn wait_turn() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        std::thread::sleep(Duration::from_secs(1).saturating_sub(t.elapsed()));
    }
    *last = Some(Instant::now());
}

/// AcoustID's reply as matches: one per recording and release, best score first.
fn matches(reply: &Value) -> Result<Vec<Match>, String> {
    if reply["status"] != "ok" {
        let why = reply["error"]["message"].as_str().unwrap_or(tr("unknown error"));
        return Err(trf("AcoustID: {why}", &[("why", &why)]));
    }
    let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_string();
    let mut out: Vec<Match> = Vec::new();
    for result in reply["results"].as_array().into_iter().flatten() {
        let score = result["score"].as_f64().unwrap_or(0.0) as f32;
        for rec in result["recordings"].as_array().into_iter().flatten() {
            let base = Match { score, recording: text(&rec["id"]), title: text(&rec["title"]), artist: artists(&rec["artists"]), ..Match::default() };
            let releases = rec["releases"].as_array().map_or(&[][..], Vec::as_slice);
            if releases.is_empty() {
                out.push(base.clone());
            }
            for rel in releases {
                let year = rel["date"]["year"].as_u64().map(|y| y.to_string()).unwrap_or_default();
                // The track's position on its medium, from the first medium that has the recording.
                let track = rel["mediums"].as_array().into_iter().flatten().flat_map(|m| m["tracks"].as_array().into_iter().flatten()).find_map(|t| t["position"].as_u64());
                out.push(Match { album: text(&rel["title"]), year, track: track.map(|n| n.to_string()).unwrap_or_default(), ..base.clone() });
            }
        }
    }
    out.retain(|m| !m.title.is_empty());
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut seen = Vec::new();
    out.retain(|m| {
        let key = (m.title.clone(), m.artist.clone(), m.album.clone(), m.year.clone(), m.track.clone());
        !seen.contains(&key) && {
            seen.push(key);
            true
        }
    });
    out.truncate(MAX_MATCHES);
    Ok(out)
}

/// An artist credit as written: names with their join phrases ("A feat. B").
fn artists(credit: &Value) -> String {
    let parts = credit.as_array().into_iter().flatten();
    let joined: String = parts.map(|a| format!("{}{}", a["name"].as_str().unwrap_or_default(), a["joinphrase"].as_str().unwrap_or(" & "))).collect();
    joined.trim_end_matches(" & ").trim().to_string()
}

fn top_genre(reply: &Value) -> Option<String> {
    let genres = reply["genres"].as_array()?;
    let best = genres.iter().filter(|g| g["name"].is_string()).max_by_key(|g| g["count"].as_u64().unwrap_or(0))?;
    let name = best["name"].as_str()?.trim();
    // As tags usually write it: "jazz" → "Jazz".
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_acoustid_matches() {
        let reply = json!({"status": "ok", "results": [
            {"score": 0.5, "recordings": [{"id": "b", "title": "Long Gone", "artists": [{"name": "Louis Armstrong"}]}]},
            {"score": 0.97, "recordings": [{"id": "a", "title": "St. Louis Blues",
                "artists": [{"name": "Louis Armstrong", "joinphrase": " and "}, {"name": "His All-Stars"}],
                "releases": [
                    {"title": "Plays W.C. Handy", "date": {"year": 1954}, "mediums": [{"tracks": [{"position": 1}]}]},
                    {"title": "Plays W.C. Handy", "date": {"year": 1954}, "mediums": [{"tracks": [{"position": 1}]}]},
                    {"title": 7, "date": "soon", "mediums": "none"}
                ]}]},
            {"score": "high", "recordings": [{"title": 3}]}
        ]});
        let m = matches(&reply).unwrap();
        assert_eq!(m.len(), 3, "{m:?}");
        assert_eq!(m[0], Match { score: 0.97, recording: "a".into(), title: "St. Louis Blues".into(), artist: "Louis Armstrong and His All-Stars".into(), album: "Plays W.C. Handy".into(), year: "1954".into(), track: "1".into() });
        // Odd types drop the value, not the match.
        assert_eq!((m[1].album.as_str(), m[1].year.as_str(), m[1].track.as_str()), ("", "", ""));
        assert_eq!((m[2].title.as_str(), m[2].album.as_str()), ("Long Gone", ""));
    }

    #[test]
    fn reports_acoustid_errors() {
        assert_eq!(matches(&json!({"status": "error", "error": {"code": 4, "message": "invalid API key"}})), Err("AcoustID: invalid API key".into()));
        assert_eq!(matches(&json!([1, 2])), Err("AcoustID: unknown error".into()));
        assert_eq!(matches(&json!({"status": "ok"})), Ok(vec![]));
    }

    #[test]
    fn picks_the_most_voted_genre() {
        assert_eq!(top_genre(&json!({"genres": [{"name": "blues", "count": 1}, {"name": "jazz", "count": 4}]})), Some("Jazz".into()));
        assert_eq!(top_genre(&json!({"genres": []})), None);
        assert_eq!(top_genre(&json!({"genres": "jazz"})), None);
    }

    #[test]
    fn fills_empty_tags_or_all() {
        let read = TagEdit { title: "track01".into(), year: "1954".into(), ..TagEdit::default() };
        let m = Match { title: "St. Louis Blues".into(), artist: "Louis Armstrong".into(), year: "1955".into(), ..Match::default() };
        let only = fill(&read, &m, Some("Jazz"), true);
        assert_eq!((only.title.as_str(), only.artist.as_str(), only.year.as_str(), only.genre.as_str()), ("track01", "Louis Armstrong", "1954", "Jazz"));
        let all = fill(&read, &m, None, false);
        assert_eq!((all.title.as_str(), all.year.as_str(), all.genre.as_str()), ("St. Louis Blues", "1955", ""));
    }

    #[test]
    fn genre_ids_are_checked_before_going_into_a_url() {
        assert_eq!(genre("../../x?y"), Ok(None));
        assert_eq!(genre(""), Ok(None));
    }

    #[test]
    fn fingerprints_are_stable_and_follow_the_audio() {
        let tone = |f: f32| (0..44100 * 20).flat_map(|i| [(i as f32 * f / 44100.0 * std::f32::consts::TAU).sin() * 0.3 * (1.0 + (i as f32 / 4410.0).sin()); 2]).collect::<Vec<f32>>();
        let (a, secs) = fingerprint(&Track::from_samples(44100, 2, tone(440.0))).unwrap();
        assert_eq!(secs, 20);
        assert_eq!(fingerprint(&Track::from_samples(44100, 2, tone(440.0))).unwrap().0, a);
        assert_ne!(fingerprint(&Track::from_samples(44100, 2, tone(660.0))).unwrap().0, a);
        assert!(fingerprint(&Track::from_samples(44100, 2, vec![0.0; 200])).is_err());
    }
}
