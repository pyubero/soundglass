//! Identify Track: a Chromaprint fingerprint looked up in AcoustID, the genre from MusicBrainz.
//! Online only when asked: AcoustID gets the fingerprint, the length and the app key;
//! MusicBrainz gets the chosen recording's ID. Nothing else leaves the computer.
//! Fill Metadata From: the title and artist searched in MusicBrainz, Discogs or English Wikipedia.

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
    /// The MusicBrainz release ID; empty for a recording on no release.
    pub release: String,
    /// How many tracks the release has, if AcoustID says.
    pub release_tracks: Option<u64>,
    /// Tells editions apart: the release's country and format, as "US, CD" or "Europe, 2×CD".
    pub edition: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track: String,
    /// Filled by `complete`, or by the search when the source has it.
    pub genre: String,
}

/// Where Identify Track's window looks: AcoustID by fingerprint, the rest by title and artist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    AcoustId,
    MusicBrainz,
    Discogs,
    Wikipedia,
}

impl Source {
    pub fn title(self) -> &'static str {
        match self {
            Self::AcoustId => tr("Identify Track"),
            Self::MusicBrainz => tr("Fill from MusicBrainz"),
            Self::Discogs => tr("Fill from Discogs"),
            Self::Wikipedia => tr("Fill from Wikipedia"),
        }
    }

    pub fn privacy(self) -> &'static str {
        match self {
            Self::AcoustId => tr("Sends this file's fingerprint and length to AcoustID, and the chosen match's ID to MusicBrainz. Nothing else leaves this computer."),
            Self::MusicBrainz => tr("Sends the title and artist to MusicBrainz, and the chosen match's ID for its genre. Nothing else leaves this computer."),
            Self::Discogs => tr("Sends the title and artist to Discogs, and the chosen release's ID for its track list. Nothing else leaves this computer."),
            Self::Wikipedia => tr("Sends the title and artist to English Wikipedia. Nothing else leaves this computer."),
        }
    }

    /// Whether its matches have a score worth showing.
    pub fn scored(self) -> bool {
        matches!(self, Self::AcoustId | Self::MusicBrainz)
    }
}

/// What to search for: the title and artist tags; without a title tag, the file's name less a
/// leading track number ("01 - ", "03-", "7. ") and with underscores as spaces.
pub fn query(tags: Option<&TagEdit>, path: &std::path::Path) -> (String, String) {
    let artist = tags.map(|t| t.artist.trim().to_string()).unwrap_or_default();
    if let Some(t) = tags.filter(|t| !t.title.trim().is_empty()) {
        return (t.title.trim().to_string(), artist);
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let title = if rest.len() < stem.len() && rest.starts_with([' ', '-', '.']) { rest.trim_start_matches([' ', '-', '.']) } else { &stem };
    (title.trim().to_string(), artist)
}

/// The matches for `title` by `artist`, best first.
pub fn search(source: Source, title: &str, artist: &str) -> Result<Vec<Match>, String> {
    let (title, artist) = (title.trim(), artist.trim());
    if title.is_empty() {
        return Err(tr("Type a title to search for.").into());
    }
    wait_turn();
    match source {
        Source::AcoustId => Ok(Vec::new()),
        Source::MusicBrainz => {
            // Lucene phrases: a quote or backslash in them escaped.
            let phrase = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
            let mut q = format!("recording:{}", phrase(title));
            if !artist.is_empty() {
                q += &format!(" AND artist:{}", phrase(artist));
            }
            let reply = agent().get("https://musicbrainz.org/ws/2/recording").query("query", &q).query("limit", "25").query("fmt", "json").call().map_err(|e| trf("MusicBrainz can't be reached ({e})", &[("e", &e)]))?;
            Ok(shown(mb_matches(&read(reply)?)))
        }
        Source::Discogs => {
            let mut req = agent().get("https://api.discogs.com/database/search").query("type", "release").query("track", title).query("per_page", "10");
            if !artist.is_empty() {
                req = req.query("artist", artist);
            }
            Ok(discogs_matches(&read(req.call().map_err(discogs_error)?)?, title))
        }
        Source::Wikipedia => {
            let search = format!("{title} {artist} hastemplate:\"Infobox song\"");
            let pairs = [("action", "query"), ("format", "json"), ("formatversion", "2"), ("generator", "search"), ("gsrsearch", &search), ("gsrlimit", "5"), ("prop", "revisions"), ("rvprop", "content"), ("rvslots", "main"), ("rvsection", "0")];
            let reply = agent().get("https://en.wikipedia.org/w/api.php").query_pairs(pairs).call().map_err(|e| trf("Wikipedia can't be reached ({e})", &[("e", &e)]))?;
            Ok(wiki_matches(&read(reply)?))
        }
    }
}

/// The picked match with what the search didn't give: the genre (AcoustID, MusicBrainz), the
/// track's title and number on the release (Discogs). A failure there keeps the match as it is.
pub fn complete(source: Source, mut m: Match) -> Match {
    match source {
        Source::AcoustId | Source::MusicBrainz => m.genre = genre(&m.recording).ok().flatten().unwrap_or_default(),
        // It goes into the URL: a Discogs ID is digits.
        Source::Discogs if !m.release.is_empty() && m.release.bytes().all(|b| b.is_ascii_digit()) => {
            wait_turn();
            let url = format!("https://api.discogs.com/releases/{}", m.release);
            if let Ok(rel) = agent().get(&url).call().map_err(discogs_error).and_then(read)
                && let Some((title, n)) = discogs_track(&rel, &m.title)
            {
                (m.title, m.track) = (title, n);
            }
        }
        Source::Discogs | Source::Wikipedia => {}
    }
    m
}

fn discogs_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401 | 403) => tr("Discogs now asks for a token; this lookup can't run.").into(),
        ureq::Error::StatusCode(429) => tr("Discogs is busy; try again in a minute.").into(),
        e => trf("Discogs can't be reached ({e})", &[("e", &e)]),
    }
}

/// MusicBrainz's recording search as matches: one per recording and release.
fn mb_matches(reply: &Value) -> Vec<Match> {
    let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_string();
    let mut out = Vec::new();
    for rec in reply["recordings"].as_array().into_iter().flatten() {
        let score = rec["score"].as_f64().unwrap_or(0.0) as f32 / 100.0;
        let base = Match { score, recording: text(&rec["id"]), title: text(&rec["title"]), artist: artists(&rec["artist-credit"]), ..Match::default() };
        let releases = rec["releases"].as_array().map_or(&[][..], Vec::as_slice);
        if releases.is_empty() {
            out.push(base.clone());
        }
        for rel in releases {
            let media = &rel["media"];
            let as_acoustid = serde_json::json!({"country": rel["country"], "medium_count": media.as_array().map_or(1, Vec::len), "mediums": media});
            let track = media.as_array().into_iter().flatten().flat_map(|m| m["track"].as_array().into_iter().flatten()).find_map(|t| t["number"].as_str());
            let year: String = text(&rel["date"]).chars().take(4).collect();
            out.push(Match { release: text(&rel["id"]), release_tracks: rel["track-count"].as_u64(), edition: edition(&as_acoustid), album: text(&rel["title"]), year, track: track.unwrap_or_default().to_string(), ..base.clone() });
        }
    }
    out.retain(|m| !m.title.is_empty());
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// Discogs's release search as matches; the track's own title and number come from `complete`.
fn discogs_matches(reply: &Value, title: &str) -> Vec<Match> {
    let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_string();
    let mut out = Vec::new();
    for r in reply["results"].as_array().into_iter().flatten() {
        let Some(id) = r["id"].as_u64() else { continue };
        // "Artist - Album", as Discogs titles a release.
        let full = text(&r["title"]);
        let (artist, album) = full.split_once(" - ").map_or((String::new(), full.clone()), |(a, b)| (a.trim().to_string(), b.trim().to_string()));
        let year = match &r["year"] {
            Value::String(y) => y.trim().to_string(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let list = |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect() };
        let edition = [text(&r["country"]), list(&r["format"]).join(", ")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        let genre = list(&r["genre"]).into_iter().next().unwrap_or_default();
        out.push(Match { release: id.to_string(), title: title.to_string(), artist, album, year, edition, genre, ..Match::default() });
    }
    out.truncate(MAX_MATCHES);
    out
}

/// The track on a Discogs release titled like `title`: its title as written and its number
/// (the position if it's a number, else its place among the tracks, so "A", "B" → 1, 2).
fn discogs_track(release: &Value, title: &str) -> Option<(String, String)> {
    let tracks: Vec<&Value> = release["tracklist"].as_array()?.iter().filter(|t| t["type_"].as_str().unwrap_or("track") == "track").collect();
    let fold = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let want = fold(title);
    let named = |t: &&Value| fold(t["title"].as_str().unwrap_or_default());
    let i = tracks.iter().position(|t| named(t) == want).or_else(|| tracks.iter().position(|t| !want.is_empty() && named(t).contains(&want)))?;
    let position = tracks[i]["position"].as_str().unwrap_or_default().trim();
    let n = if !position.is_empty() && position.bytes().all(|b| b.is_ascii_digit()) { position.trim_start_matches('0').to_string() } else { (i + 1).to_string() };
    Some((tracks[i]["title"].as_str().unwrap_or_default().trim().to_string(), n))
}

/// Wikipedia's search results as matches, from each article's song infobox, in search order.
fn wiki_matches(reply: &Value) -> Vec<Match> {
    let mut pages: Vec<&Value> = reply["query"]["pages"].as_array().into_iter().flatten().collect();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX));
    let mut out = Vec::new();
    for p in pages {
        let source = p["revisions"][0]["slots"]["main"]["content"].as_str().unwrap_or_default();
        let Some(fields) = infobox(source) else { continue };
        let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or_default();
        let page = p["title"].as_str().unwrap_or_default();
        let name = wikitext(get("name"));
        let title = if name.is_empty() { page.trim_end_matches(" (song)").trim().to_string() } else { name.trim_matches('"').to_string() };
        let year = ["released", "recorded", "published"].iter().find_map(|k| first_year(get(k))).unwrap_or_default();
        let genre = wikitext(get("genre")).split(',').next().unwrap_or_default().trim().to_string();
        let mut genre_chars = genre.chars();
        let genre = genre_chars.next().map(|f| f.to_uppercase().chain(genre_chars).collect()).unwrap_or_default();
        let track = wikitext(get("track_no"));
        out.push(Match { title, artist: wikitext(get("artist")), album: wikitext(get("album")), year, genre, track: if track.bytes().all(|b| b.is_ascii_digit()) { track } else { String::new() }, ..Match::default() });
    }
    out.retain(|m| !m.title.is_empty());
    out
}

/// The `| name = value` fields of an article's `{{Infobox song}}` (or single), names lowercased.
fn infobox(source: &str) -> Option<Vec<(String, String)>> {
    let lower = source.to_ascii_lowercase();
    let start = ["{{infobox song", "{{infobox single"].iter().filter_map(|k| lower.find(k)).min()?;
    // To the matching "}}": templates inside the fields nest.
    let bytes = source.as_bytes();
    let (mut depth, mut i, mut end) = (0i32, start, source.len());
    while i + 1 < bytes.len() {
        match &bytes[i..i + 2] {
            b"{{" => (depth, i) = (depth + 1, i + 2),
            b"}}" => {
                (depth, i) = (depth - 1, i + 2);
                if depth == 0 {
                    end = i - 2;
                    break;
                }
            }
            _ => i += 1,
        }
    }
    let fields = split_top(&source[start + 2..end]);
    Some(fields.iter().skip(1).filter_map(|f| f.split_once('=')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect())
}

/// `s` split at each "|" outside nested `{{…}}` and `[[…]]`.
fn split_top(s: &str) -> Vec<String> {
    let (mut parts, mut cur, mut nest) = (Vec::new(), String::new(), 0i32);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let doubled = matches!(c, '{' | '[' | '}' | ']') && chars.peek() == Some(&c);
        if doubled {
            nest += if matches!(c, '{' | '[') { 1 } else { -1 };
            cur.push(c);
            cur.push(chars.next().unwrap_or(c));
        } else if c == '|' && nest <= 0 {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts
}

/// The first year (1000–2999) in a field, whatever its date template.
fn first_year(field: &str) -> Option<String> {
    let field = drop_between(field, "<ref", "</ref>");
    let b = field.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let digits = &b[i..i + 4];
        let alone = (i == 0 || !b[i - 1].is_ascii_digit()) && b.get(i + 4).is_none_or(|c| !c.is_ascii_digit());
        (alone && matches!(digits[0], b'1' | b'2') && digits.iter().all(u8::is_ascii_digit)).then(|| field[i..i + 4].to_string())
    })
}

/// A field's wikitext as plain text: comments, references and templates dropped (the items of a
/// list template kept), links as their shown text, line breaks and list items as ", ".
fn wikitext(field: &str) -> String {
    let mut s = drop_between(field, "<!--", "-->");
    s = drop_between(&s, "<ref", "</ref>");
    for list in ["{{hlist", "{{flatlist", "{{plainlist", "{{ubl", "{{unbulleted list"] {
        while let Some(at) = s.to_ascii_lowercase().find(list) {
            let inner_start = at + list.len();
            let Some(close) = s[inner_start..].find("}}").map(|c| inner_start + c) else { break };
            let items = split_top(&s[inner_start..close]).join(", ");
            s = format!("{}{}{}", &s[..at], items, &s[close + 2..]);
        }
    }
    // Remaining templates go, innermost first.
    while let Some(close) = s.find("}}") {
        let Some(open) = s[..close].rfind("{{") else { break };
        s.replace_range(open..close + 2, "");
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(open) = rest.find("[[") {
        out += &rest[..open];
        rest = &rest[open + 2..];
        // An unclosed link loses only its "[[".
        let Some(close) = rest.find("]]") else { continue };
        out += rest[..close].rsplit('|').next().unwrap_or_default();
        rest = &rest[close + 2..];
    }
    out += rest;
    let out = out.replace("<br />", ", ").replace("<br/>", ", ").replace("<br>", ", ").replace("\n*", ", ").replace("''", "");
    let parts: Vec<&str> = out.split(',').map(|p| p.trim().trim_start_matches('*').trim()).filter(|p| !p.is_empty()).collect();
    parts.join(", ")
}

/// `s` without each span from `open` to `close` (or to its end; `<ref name=x />` closes itself).
fn drop_between(s: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find(open) {
        out += &rest[..at];
        let after = &rest[at..];
        let self_closed = after.find('>').filter(|&g| after[..g].ends_with('/'));
        rest = match self_closed {
            Some(g) => &after[g + 1..],
            None => after.find(close).map_or("", |c| &after[c + close.len()..]),
        };
    }
    out + rest
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

/// Looks the fingerprint up in AcoustID: the best matches, best first.
pub fn lookup(key: &str, fingerprint: &str, secs: u32) -> Result<Vec<Match>, String> {
    lookup_all(key, fingerprint, secs).map(shown)
}

/// Every match AcoustID has for the fingerprint, best first; a rate-limit reply is retried once.
pub fn lookup_all(key: &str, fingerprint: &str, secs: u32) -> Result<Vec<Match>, String> {
    let ask = || {
        wait_turn();
        let form = [("client", key.trim()), ("duration", &secs.to_string()), ("fingerprint", fingerprint), ("meta", "recordings releases tracks")];
        let reply = agent().post("https://api.acoustid.org/v2/lookup").send_form(form).map_err(|e| format!("AcoustID can't be reached ({e})"))?;
        matches(&read(reply)?)
    };
    match ask() {
        // AcoustID's message for its error 14 ("rate limit exceeded").
        Err(e) if e.to_lowercase().contains("rate limit") => {
            std::thread::sleep(Duration::from_secs(3));
            ask()
        }
        other => other,
    }
}

/// An album the folder's files could belong to: one MusicBrainz release.
#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub release: String,
    pub album: String,
    pub artist: String,
    pub year: String,
    /// How many of the files have a match on it.
    pub files: usize,
    pub tracks: Option<u64>,
    pub edition: String,
}

/// The releases the files' matches point to, the one covering the most files first, then the one
/// whose track count is nearest the folder's (an album before an anthology holding it), then the
/// highest summed score; at most `MAX_MATCHES`.
pub fn albums(per_file: &[Vec<Match>]) -> Vec<Album> {
    let mut out: Vec<(Album, f32)> = Vec::new();
    for matches in per_file {
        let mut counted: Vec<&str> = Vec::new();
        // Best first, so each release takes this file's best match on it.
        for m in matches.iter().filter(|m| !m.release.is_empty()) {
            if counted.contains(&m.release.as_str()) {
                continue;
            }
            counted.push(&m.release);
            match out.iter_mut().find(|(a, _)| a.release == m.release) {
                Some((a, score)) => (a.files, *score) = (a.files + 1, *score + m.score),
                None => out.push((Album { release: m.release.clone(), album: m.album.clone(), artist: m.artist.clone(), year: m.year.clone(), files: 1, tracks: m.release_tracks, edition: m.edition.clone() }, m.score)),
            }
        }
    }
    let off = |a: &Album| a.tracks.map_or(u64::MAX, |t| t.abs_diff(per_file.len() as u64));
    out.sort_by(|(a, x), (b, y)| b.files.cmp(&a.files).then(off(a).cmp(&off(b))).then(y.total_cmp(x)));
    out.into_iter().take(MAX_MATCHES).map(|(a, _)| a).collect()
}

/// Each file's best match on `release`, if it has one.
pub fn pick(per_file: &[Vec<Match>], release: &str) -> Vec<Option<Match>> {
    per_file.iter().map(|ms| ms.iter().filter(|m| m.release == release).max_by(|a, b| a.score.total_cmp(&b.score)).cloned()).collect()
}

/// `fingerprint` of the file at `path`, decoding only the first two minutes when its length is known.
pub fn fingerprint_file(path: &std::path::Path) -> Result<(String, u32), String> {
    let (mut samples, mut spec, mut frames, mut total) = (Vec::new(), (0, 0), 0u64, None);
    let stop = "stop";
    let done = crate::track::decode(path, |_, _, _| {}, |buf, channels, rate, expected| {
        if channels == 0 || rate == 0 || (spec != (0, 0) && spec != (rate, channels)) {
            return Err(tr("The audio changes format partway.").into());
        }
        (spec, total) = ((rate, channels), expected);
        frames += (buf.len() / channels) as u64;
        let max = SECONDS * rate as usize * channels;
        samples.extend_from_slice(&buf[..buf.len().min(max.saturating_sub(samples.len()))]);
        // With the length known, the rest needn't be decoded.
        if samples.len() >= max && expected.is_some() { Err(stop.into()) } else { Ok(()) }
    });
    match done {
        Err(e) if e != stop => return Err(e),
        _ => {}
    }
    let (rate, channels) = spec;
    if rate == 0 {
        return Err(tr("The file has no audio.").into());
    }
    let (fp, _) = fingerprint(&Track::from_samples(rate, channels, samples))?;
    Ok((fp, (total.unwrap_or(frames) / rate as u64) as u32))
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
/// With `genre` None, the match's own genre.
pub fn fill(read: &TagEdit, m: &Match, genre: Option<&str>, only_empty: bool) -> TagEdit {
    let mut t = read.clone();
    let genre = genre.or(Some(m.genre.as_str()));
    let found = [(&mut t.title, &m.title), (&mut t.artist, &m.artist), (&mut t.album, &m.album), (&mut t.year, &m.year), (&mut t.track, &m.track)];
    for (field, value) in found.into_iter().chain([(&mut t.genre, &genre.unwrap_or_default().to_string())]) {
        if !value.is_empty() && (!only_empty || field.trim().is_empty()) {
            field.clone_from(value);
        }
    }
    t
}

/// One agent for every request, so the connection is kept between them.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            // ureq shares this between the host's addresses: 10 s let two dead IPv6 ones take 8 s.
            .timeout_connect(Some(Duration::from_secs(2)))
            .user_agent(format!("soundglass/{} ( {CONTACT} )", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
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
                out.push(Match { release: text(&rel["id"]), release_tracks: rel["track_count"].as_u64(), edition: edition(rel), album: text(&rel["title"]), year, track: track.map(|n| n.to_string()).unwrap_or_default(), ..base.clone() });
            }
        }
    }
    out.retain(|m| !m.title.is_empty());
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Every release kept: Identify Album votes with all of them.
    let mut seen = Vec::new();
    out.retain(|m| {
        let key = (m.recording.clone(), m.release.clone());
        !seen.contains(&key) && {
            seen.push(key);
            true
        }
    });
    Ok(out)
}

/// The best `MAX_MATCHES` matches as Identify Track lists them: editions alike in every shown field once.
fn shown(mut all: Vec<Match>) -> Vec<Match> {
    let mut seen = Vec::new();
    all.retain(|m| {
        let key = (m.title.clone(), m.artist.clone(), m.album.clone(), m.year.clone(), m.track.clone());
        !seen.contains(&key) && {
            seen.push(key);
            true
        }
    });
    all.truncate(MAX_MATCHES);
    all
}

/// A release's country and format: MusicBrainz's codes for Europe and Worldwide spelled out, and
/// "2×" before the format of a release on several media alike.
fn edition(rel: &Value) -> String {
    let country = match rel["country"].as_str().unwrap_or_default().trim() {
        "XE" => tr("Europe"),
        "XW" => tr("Worldwide"),
        c => c,
    };
    let formats: Vec<&str> = rel["mediums"].as_array().into_iter().flatten().filter_map(|m| m["format"].as_str()).map(str::trim).filter(|f| !f.is_empty()).collect();
    let count = rel["medium_count"].as_u64().unwrap_or(1);
    let format = match formats.first() {
        Some(f) if count > 1 && formats.iter().all(|g| g == f) => format!("{count}×{f}"),
        Some(_) => formats.join(" + "),
        None => String::new(),
    };
    [country.to_string(), format].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
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
                    {"id": "r1", "track_count": 16, "country": "US", "medium_count": 1, "title": "Plays W.C. Handy", "date": {"year": 1954}, "mediums": [{"format": "CD", "tracks": [{"position": 1}]}]},
                    {"id": "r2", "title": "Plays W.C. Handy", "date": {"year": 1954}, "mediums": [{"tracks": [{"position": 1}]}]},
                    {"title": 7, "date": "soon", "mediums": "none"}
                ]}]},
            {"score": "high", "recordings": [{"title": 3}]}
        ]});
        let all = matches(&reply).unwrap();
        // Both editions kept for the album vote; shown once in Identify Track.
        assert_eq!(all.iter().filter(|m| m.album == "Plays W.C. Handy").count(), 2);
        let m = shown(all);
        assert_eq!(m.len(), 3, "{m:?}");
        assert_eq!(m[0], Match { score: 0.97, recording: "a".into(), release: "r1".into(), release_tracks: Some(16), edition: "US, CD".into(), title: "St. Louis Blues".into(), artist: "Louis Armstrong and His All-Stars".into(), album: "Plays W.C. Handy".into(), year: "1954".into(), track: "1".into(), genre: String::new() });
        // Odd types drop the value, not the match.
        assert_eq!((m[1].album.as_str(), m[1].year.as_str(), m[1].track.as_str()), ("", "", ""));
        assert_eq!((m[2].title.as_str(), m[2].album.as_str()), ("Long Gone", ""));
    }

    #[test]
    fn suggests_the_album_most_files_are_on() {
        let on = |release: &str, track: &str, score: f32| Match { score, release: release.into(), album: format!("album {release}"), title: format!("t{track}"), track: track.into(), ..Match::default() };
        // 15 files on the album (each also on a compilation for the first two), one file not found.
        let mut per_file: Vec<Vec<Match>> = (1..=15).map(|n| vec![on("lp", &n.to_string(), 0.9)]).collect();
        per_file[0].insert(0, on("best-of", "7", 0.95));
        per_file[1].insert(0, on("best-of", "3", 0.95));
        // A file listed twice on one release counts once.
        per_file[2].push(on("lp", "3", 0.5));
        per_file.push(vec![]);
        let a = albums(&per_file);
        assert_eq!(a.iter().map(|a| (a.release.as_str(), a.files)).collect::<Vec<_>>(), [("lp", 15), ("best-of", 2)]);
        let picked = pick(&per_file, "lp");
        assert_eq!(picked.len(), 16);
        assert_eq!(picked[2].as_ref().map(|m| (m.track.as_str(), m.score)), Some(("3", 0.9)));
        assert_eq!(picked[0].as_ref().map(|m| m.track.as_str()), Some("1"));
        assert!(picked[15].is_none());
        assert!(albums(&[]).is_empty());
        // Both cover every file: the 16-track album before the 300-track anthology, whatever the scores.
        let mut both: Vec<Vec<Match>> = (1..=16).map(|n| vec![Match { release_tracks: Some(300), ..on("anthology", &n.to_string(), 0.99) }, Match { release_tracks: Some(16), ..on("lp", &n.to_string(), 0.9) }]).collect();
        assert_eq!(albums(&both)[0].release, "lp");
        both.iter_mut().for_each(|ms| ms.iter_mut().for_each(|m| m.release_tracks = None));
        assert_eq!(albums(&both)[0].release, "anthology");
    }

    #[test]
    fn fingerprints_files_and_survives_garbage() {
        let path = crate::track::tests::temp_file("identify-file.wav", &crate::track::tests::wav(44100 * 30, 44100));
        let (fp, secs) = fingerprint_file(&path).unwrap();
        assert_eq!(secs, 30);
        assert!(!fp.is_empty());
        let junk = crate::track::tests::temp_file("identify-junk.mp3", b"not audio at all");
        assert!(fingerprint_file(&junk).is_err());
        let _ = (std::fs::remove_file(path), std::fs::remove_file(junk));
    }

    #[test]
    fn names_editions() {
        assert_eq!(edition(&json!({"country": "XE", "medium_count": 2, "mediums": [{"format": "CD"}, {"format": "CD"}]})), "Europe, 2×CD");
        assert_eq!(edition(&json!({"country": "GB", "medium_count": 2, "mediums": [{"format": "12\" Vinyl"}]})), "GB, 2×12\" Vinyl");
        assert_eq!(edition(&json!({"country": "US", "mediums": [{"format": "CD"}, {"format": "DVD"}]})), "US, CD + DVD");
        assert_eq!(edition(&json!({"mediums": [{"format": "SACD"}]})), "SACD");
        assert_eq!(edition(&json!({"country": 3, "mediums": "none"})), "");
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
    fn queries_tags_then_the_file_name() {
        let p = std::path::Path::new("/music/01 - St. Louis Blues.mp3");
        assert_eq!(query(None, p), ("St. Louis Blues".into(), String::new()));
        let tags = TagEdit { title: " ".into(), artist: "Louis Armstrong".into(), ..TagEdit::default() };
        assert_eq!(query(Some(&tags), std::path::Path::new("03-Loveless_Love.flac")), ("Loveless Love".into(), "Louis Armstrong".into()));
        assert_eq!(query(Some(&TagEdit { title: "Long Gone".into(), ..tags }), p).0, "Long Gone");
        assert_eq!(query(None, std::path::Path::new("7. Memphis Blues.ogg")).0, "Memphis Blues");
        // A number that is the title stays.
        assert_eq!(query(None, std::path::Path::new("1999.mp3")).0, "1999");
        assert_eq!(query(None, std::path::Path::new("")).0, "");
    }

    #[test]
    fn reads_musicbrainz_search() {
        let reply = json!({"recordings": [
            {"id": "r", "score": 100, "title": "St. Louis Blues", "artist-credit": [{"name": "Louis Armstrong", "joinphrase": " & "}, {"name": "His All-Stars"}],
             "releases": [{"id": "rel", "title": "Plays W.C. Handy", "date": "1954-08", "country": "US", "track-count": 11,
                           "media": [{"format": "12\" Vinyl", "track": [{"number": "A1"}]}]}]},
            {"id": "s", "score": 40, "title": "St. Louis Blues", "artist-credit": "odd"},
            {"score": "x", "title": 5}
        ]});
        let m = mb_matches(&reply);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!((m[0].score, m[0].artist.as_str(), m[0].album.as_str(), m[0].year.as_str(), m[0].track.as_str(), m[0].edition.as_str(), m[0].release_tracks), (1.0, "Louis Armstrong & His All-Stars", "Plays W.C. Handy", "1954", "A1", "US, 12\" Vinyl", Some(11)));
        assert_eq!((m[1].recording.as_str(), m[1].album.as_str()), ("s", ""));
    }

    #[test]
    fn reads_discogs_search_and_tracklist() {
        let reply = json!({"results": [
            {"id": 7680029, "title": "Louis Armstrong - St Louis Blues / Basin Street Blues", "year": "1955", "genre": ["Jazz"], "country": "Germany", "format": ["Vinyl", "7\""]},
            {"id": 2, "title": "No Dash Here", "year": 1960},
            {"title": "no id"}
        ]});
        let m = discogs_matches(&reply, "St. Louis Blues");
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].release.as_str(), m[0].artist.as_str(), m[0].album.as_str(), m[0].year.as_str(), m[0].genre.as_str(), m[0].edition.as_str()), ("7680029", "Louis Armstrong", "St Louis Blues / Basin Street Blues", "1955", "Jazz", "Germany, Vinyl, 7\""));
        assert_eq!((m[1].artist.as_str(), m[1].album.as_str(), m[1].year.as_str()), ("", "No Dash Here", "1960"));
        let release = json!({"tracklist": [
            {"position": "", "type_": "heading", "title": "Side One"},
            {"position": "A", "type_": "track", "title": "St Louis Blues"},
            {"position": "B", "type_": "track", "title": "Basin Street Blues"}
        ]});
        assert_eq!(discogs_track(&release, "St. Louis Blues"), Some(("St Louis Blues".into(), "1".into())));
        assert_eq!(discogs_track(&release, "basin street"), Some(("Basin Street Blues".into(), "2".into())));
        assert_eq!(discogs_track(&json!({"tracklist": [{"position": "03", "title": "X"}]}), "x"), Some(("X".into(), "3".into())));
        assert_eq!(discogs_track(&release, "Hello Dolly"), None);
        assert_eq!(discogs_track(&json!({"tracklist": 3}), "x"), None);
        // A release ID that isn't digits never reaches a URL.
        let odd = Match { release: "../x".into(), title: "t".into(), ..Match::default() };
        assert_eq!(complete(Source::Discogs, odd.clone()), odd);
    }

    #[test]
    fn reads_wikipedia_infoboxes() {
        let georgia = "Intro {{Short description|x}}\n{{Infobox song\n| name     = Georgia Grind\n| type     = single\n| artist   = [[Louis Armstrong and his Hot Five|Louis Armstrong & Hot Five]]\n| album    =\n| released = April 1926<ref>{{cite web|date=2001}}</ref>\n| recorded = {{Start date|1926|2|26}}\n| genre    = {{hlist|[[jazz]]|[[Dirty blues|dirty blues]]}}\n| track_no = 2\n}}\nText.";
        let st = "{{Infobox Song\n| name = <!-- none -->\n| artist = W. C. Handy<br />Bessie Smith\n| published = {{start date|1914|9|11}} by [[Harry Pace|Pace]]\n| genre = Blues, jazz\n| track_no = A1\n}}";
        let reply = json!({"query": {"pages": [
            {"title": "Saint Louis Blues (song)", "index": 2, "revisions": [{"slots": {"main": {"content": st}}}]},
            {"title": "Georgia Grind", "index": 1, "revisions": [{"slots": {"main": {"content": georgia}}}]},
            {"title": "Louis Armstrong", "index": 3, "revisions": [{"slots": {"main": {"content": "{{Infobox person|name=Louis}}"}}}]},
            {"title": "Broken", "index": 4, "revisions": [{"slots": {"main": {"content": "{{Infobox song| name = Ünïcode [[İstanbul"}}}]}
        ]}});
        let m = wiki_matches(&reply);
        assert_eq!(m.len(), 3, "{m:?}");
        assert_eq!((m[0].title.as_str(), m[0].artist.as_str(), m[0].album.as_str(), m[0].year.as_str(), m[0].genre.as_str(), m[0].track.as_str()), ("Georgia Grind", "Louis Armstrong & Hot Five", "", "1926", "Jazz", "2"));
        assert_eq!((m[1].title.as_str(), m[1].artist.as_str(), m[1].year.as_str(), m[1].genre.as_str(), m[1].track.as_str()), ("Saint Louis Blues", "W. C. Handy, Bessie Smith", "1914", "Blues", ""));
        assert_eq!(m[2].title, "Ünïcode İstanbul");
        assert_eq!(wikitext("{{hlist|Jazz|Blues}}"), "Jazz, Blues");
        assert_eq!(wikitext("\n* [[Swing music|Swing]]\n* Pop<ref name=a />"), "Swing, Pop");
        assert_eq!(first_year("{{Duration|m=3}} 12345 c. 1954"), Some("1954".into()));
        assert_eq!(first_year("unknown"), None);
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

/// Online, by hand: `cargo test --release -- --ignored --nocapture fill_from_`.
#[cfg(test)]
#[test]
#[ignore]
fn fill_from_each_source() {
    for source in [Source::MusicBrainz, Source::Discogs, Source::Wikipedia] {
        let found = search(source, "St. Louis Blues", "Louis Armstrong").unwrap();
        assert!(!found.is_empty(), "{source:?}");
        let best = complete(source, found[0].clone());
        println!("{source:?}: {} found; {best:?}", found.len());
    }
}

/// Online, by hand: `SG_ACOUSTID_KEY=… cargo test --release -- --ignored --nocapture album_lookup_on_the_test_folder`.
#[cfg(test)]
#[test]
#[ignore]
fn album_lookup_on_the_test_folder() {
    let key = std::env::var("SG_ACOUSTID_KEY").expect("SG_ACOUSTID_KEY");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("1954-Louis Armstrong Plays W.C.Handy");
    let first = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| crate::folder::is_audio(p)).unwrap();
    let files = crate::folder::list_folder(&first);
    let mut per_file = Vec::new();
    for f in &files {
        let t = std::time::Instant::now();
        let (fp, secs) = fingerprint_file(f).unwrap();
        let t_fp = t.elapsed();
        let m = lookup_all(&key, &fp, secs).unwrap();
        println!("  fingerprint {t_fp:.1?}, lookup {:.1?}", t.elapsed() - t_fp);
        let releases: std::collections::BTreeSet<_> = m.iter().map(|m| format!("{} {} {}", m.release, m.year, m.album)).collect();
        println!("{} — {} matches, {} releases, recordings {:?}", f.file_name().unwrap().to_string_lossy(), m.len(), releases.len(), m.iter().map(|m| &m.recording[..8]).collect::<std::collections::BTreeSet<_>>());
        per_file.push(m);
    }
    for a in albums(&per_file) {
        println!("{} files, {:?} tracks, {}: {} {} {} ({})", a.files, a.tracks, a.edition, a.artist, a.album, a.year, a.release);
    }
}
