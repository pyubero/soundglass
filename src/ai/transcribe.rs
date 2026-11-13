//! Transcribe Speech: Whisper (sherpa-onnx's export: our log-mel, encoder → cross K/V, decoder
//! with self-attention caches, greedy, as sherpa-onnx's scripts/whisper/test.py) or NVIDIA's
//! Parakeet TDT (istupakov's export with its own feature graph, greedy TDT as onnx-asr). Long
//! audio is cut into pieces of at most 29 s at quiet moments; silent pieces are skipped.
use super::*;
use ort::value::{DynValue, Tensor};

pub const RATE: u32 = 16_000;
const MAX_PIECE: usize = 29 * RATE as usize;

fn err(e: ort::Error) -> String {
    e.to_string()
}

/// Pieces of at most 29 s, each cut at the quietest 100 ms of its last 5 s.
pub(super) fn pieces(x: &[f32]) -> Vec<std::ops::Range<usize>> {
    let win = RATE as usize / 10;
    let energy = |a: usize| x[a..a + win].iter().map(|v| v * v).sum::<f32>();
    let (mut out, mut start) = (Vec::new(), 0);
    while x.len() - start > MAX_PIECE {
        let lo = start + MAX_PIECE - 5 * RATE as usize;
        let quiet = (lo..start + MAX_PIECE - win).step_by(win / 2).min_by(|&a, &b| energy(a).total_cmp(&energy(b))).unwrap_or(lo);
        out.push(start..quiet + win / 2);
        start = quiet + win / 2;
    }
    if start < x.len() {
        out.push(start..x.len());
    }
    out
}

/// Quieter than `db` (RMS, dBFS): left out, since Whisper makes up words in silence.
fn silent(x: &[f32], db: f32) -> bool {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt() < 10f32.powf(db / 20.0)
}

/// librosa.filters.mel(sr=16000, n_fft=400, n_mels): slaney scale, slaney (area) norm; rows of 201.
pub(super) fn mel_filters(n_mels: usize) -> Vec<f32> {
    let (fsp, min_log_hz, logstep) = (200.0 / 3.0, 1000.0f64, 6.4f64.ln() / 27.0);
    let min_log_mel = min_log_hz / fsp;
    let to_mel = |f: f64| if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / logstep } else { f / fsp };
    let to_hz = |m: f64| if m >= min_log_mel { min_log_hz * ((m - min_log_mel) * logstep).exp() } else { fsp * m };
    let top = to_mel(RATE as f64 / 2.0);
    let pts: Vec<f64> = (0..n_mels + 2).map(|i| to_hz(top * i as f64 / (n_mels + 1) as f64)).collect();
    let mut out = vec![0f32; n_mels * 201];
    for m in 0..n_mels {
        let norm = 2.0 / (pts[m + 2] - pts[m]);
        for k in 0..201 {
            let f = k as f64 * RATE as f64 / 400.0;
            let w = ((f - pts[m]) / (pts[m + 1] - pts[m])).min((pts[m + 2] - f) / (pts[m + 2] - pts[m + 1])).max(0.0);
            out[m * 201 + k] = (w * norm) as f32;
        }
    }
    out
}

/// Whisper's log_mel_spectrogram (n_fft 400, hop 160, periodic Hann, centred with reflect
/// padding, power, last frame dropped, log10 floored 8 below its peak, (x + 4) / 4), then
/// padded with zeros as sherpa-onnx does: by 1500 frames, at most to 3000. Mels × frames.
pub(super) fn whisper_mel(x: &[f32], filters: &[f32], n_mels: usize) -> (Vec<f32>, usize) {
    const N: usize = 400;
    let n = x.len();
    let frames = n / 160;
    let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let at = |i: isize| -> f32 {
        let m = n as isize;
        let j = if i < 0 { -i } else if i >= m { 2 * (m - 1) - i } else { i };
        if (0..m).contains(&j) { x[j as usize] } else { 0.0 }
    };
    let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    let mut logs = vec![0f32; n_mels * frames];
    for f in 0..frames {
        let start = (f * 160) as isize - (N / 2) as isize;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = at(start + i as isize) * window[i];
        }
        let _ = fft.process(&mut buf, &mut spec);
        for m in 0..n_mels {
            let e: f32 = spec.iter().zip(&filters[m * 201..(m + 1) * 201]).map(|(c, w)| c.norm_sqr() * w).sum();
            logs[m * frames + f] = e.max(1e-10).log10();
        }
    }
    let peak = logs.iter().copied().fold(f32::MIN, f32::max);
    let total = (frames + 1500).min(3000);
    let mut mel = vec![0f32; n_mels * total];
    for m in 0..n_mels {
        for f in 0..frames.min(total) {
            mel[m * total + f] = (logs[m * frames + f].max(peak - 8.0) + 4.0) / 4.0;
        }
    }
    (mel, total)
}

/// A stretch of speech: its start and end in seconds, and its words.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The cues `by` seconds later (a selection's start, so the times are the track's).
pub(super) fn shifted(cues: Vec<Cue>, by: f64) -> Vec<Cue> {
    cues.into_iter().map(|c| Cue { start: c.start + by, end: c.end + by, ..c }).collect()
}

/// The cues' words as one text.
pub(super) fn plain(cues: &[Cue]) -> String {
    cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// The cues as an .srt file: number, `hh:mm:ss,mmm --> hh:mm:ss,mmm`, text, blank line.
pub(super) fn srt(cues: &[Cue]) -> String {
    let time = |s: f64| {
        let ms = (s.max(0.0) * 1000.0).round() as u64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    };
    cues.iter().enumerate().map(|(i, c)| format!("{}\n{} --> {}\n{}\n\n", i + 1, time(c.start), time(c.end.max(c.start)), c.text)).collect()
}

/// Whisper's ApplyTimestampRules (decoding.py) for the next token after `said`: timestamps
/// (ids from `ts_begin`, 20 ms each) come first (at most 1 s in), in pairs, never back in time,
/// and are taken whenever all of them together are likelier than the best text token.
pub(super) fn timestamp_rules(logits: &mut [f32], said: &[i64], ts_begin: usize, eot: usize) {
    let n = logits.len();
    let ts_begin = ts_begin.min(n);
    let is_ts = |t: &i64| *t as usize >= ts_begin;
    let last = said.last().is_some_and(is_ts);
    let penultimate = said.len() < 2 || is_ts(&said[said.len() - 2]);
    if last {
        let range = if penultimate { ts_begin..n } else { 0..eot.min(n) };
        logits[range].fill(f32::NEG_INFINITY);
    }
    if let Some(&t) = said.iter().rev().find(|t| is_ts(t)) {
        let floor = if last && !penultimate { t as usize } else { t as usize + 1 };
        logits[ts_begin..floor.min(n)].fill(f32::NEG_INFINITY);
    }
    if said.is_empty() {
        logits[..ts_begin].fill(f32::NEG_INFINITY);
        logits[(ts_begin + 51).min(n)..].fill(f32::NEG_INFINITY);
    }
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if max == f32::NEG_INFINITY {
        return;
    }
    // log-sum-exp of the timestamps against the best text token, both relative to the max.
    let ts_mass: f32 = logits[ts_begin..].iter().map(|&x| (x - max).exp()).sum();
    let best_text = logits[..ts_begin].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if ts_mass > 0.0 && ts_mass.ln() > best_text - max {
        logits[..ts_begin].fill(f32::NEG_INFINITY);
    }
}

/// Words (start, end in seconds, text with its leading space) grouped into subtitle cues: a cue
/// ends after . ? ! once it's 1 s long, before a word that would take it past 7 s or 84
/// characters, and at a pause of more than 1 s.
pub(super) fn cues_from_words(words: &[(f64, f64, String)]) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut open: Option<Cue> = None;
    for (start, end, word) in words {
        if let Some(c) = &open
            && (start - c.end > 1.0 || end - c.start > 7.0 || c.text.len() + word.len() > 84)
        {
            cues.extend(open.take());
        }
        let c = open.get_or_insert(Cue { start: *start, end: *end, text: String::new() });
        c.text.push_str(word);
        c.end = *end;
        if c.end - c.start >= 1.0 && c.text.trim_end().ends_with(['.', '?', '!']) {
            cues.extend(open.take());
        }
    }
    cues.extend(open);
    cues.iter_mut().for_each(|c| c.text = tidy_spaces(&c.text));
    cues.retain(|c| !c.text.is_empty());
    cues
}

/// Whisper's text of `mono` (at `RATE`), the language detected from the first piece with sound.
/// `tune`: a fixed language (else detected), translate, and the silence level.
/// With `tune.time_marks`, Whisper writes its own segment times; else one cue a piece.
pub(super) fn whisper(encoder: &Path, decoder: &Path, tokens: &Path, mono: &[f32], tune: &super::tune::SpeechTune, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<Cue>, String> {
    let mut enc = session_with(encoder, cores(), false)?;
    let mut dec = session_with(decoder, cores(), false)?;
    let meta = enc.metadata().map_err(err)?;
    let get = |k: &str| meta.custom(k).ok_or_else(|| format!("the model has no {k}"));
    let num = |k: &str, max: i64| get(k)?.trim().parse::<i64>().ok().filter(|v| (0..=max).contains(v)).ok_or_else(|| format!("the model's {k} is out of range"));
    let list = |k: &str| get(k)?.split(',').map(|v| v.trim().parse::<i64>().map_err(|_| format!("the model's {k} isn't a list"))).collect::<Result<Vec<i64>, String>>();
    let (n_mels, n_layer, n_ctx, n_state) = (num("n_mels", 128)? as usize, num("n_text_layer", 64)? as usize, num("n_text_ctx", 4096)? as usize, num("n_text_state", 8192)? as usize);
    let (eot, sot, no_timestamps, no_speech, translate, blank) = (num("eot", 1 << 20)?, num("sot", 1 << 20)?, num("no_timestamps", 1 << 20)?, num("no_speech", 1 << 20)?, num("translate", 1 << 20)?, num("blank_id", 1 << 20)?);
    let multilingual = num("is_multilingual", 1)? == 1;
    let mut start = list("sot_sequence")?;
    if !tune.time_marks {
        start.push(no_timestamps);
    }
    // The timestamp tokens follow <|notimestamps|>: <|0.00|>, <|0.02|>…
    let ts_begin = no_timestamps as usize + 1;
    let languages = if multilingual { list("all_language_tokens")? } else { Vec::new() };
    let codes: Vec<String> = if multilingual { get("all_language_codes")?.split(',').map(|c| c.trim().to_string()).collect() } else { Vec::new() };
    drop(meta);
    // A chosen language is set from the start; detection then never runs.
    let mut language: Option<i64> = None;
    if !tune.language.is_empty() {
        let i = codes.iter().position(|c| *c == tune.language).ok_or_else(|| if multilingual { format!("this model doesn't know the language \"{}\"; it knows {}", tune.language, codes.join(", ")) } else { "this model only knows English: leave the language empty".to_string() })?;
        language = languages.get(i).copied();
    }
    if tune.translate && !multilingual {
        return Err("this model only knows English, so it can't translate".into());
    }
    if !matches!(n_mels, 80 | 128) || start.len() < 3 {
        return Err("the model isn't a Whisper export soundglass knows".into());
    }
    let table = read_tokens(tokens)?;
    let filters = mel_filters(n_mels);
    let (enc_in, dec_in): (String, Vec<String>) = (enc.inputs()[0].name().to_string(), dec.inputs().iter().map(|i| i.name().to_string()).collect());
    let dec_out: Vec<String> = dec.outputs().iter().map(|o| o.name().to_string()).collect();
    if dec_in.len() < 6 || dec_out.len() < 3 || enc.outputs().len() < 2 {
        return Err("the model isn't a Whisper export soundglass knows".into());
    }
    let (cross_k_name, cross_v_name) = (enc.outputs()[0].name().to_string(), enc.outputs()[1].name().to_string());
    let cache_len = n_layer * n_ctx * n_state;
    let mut cues = Vec::new();
    let parts = pieces(mono);
    for (p, range) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let piece = &mono[range.clone()];
        let (at, len) = (range.start as f64 / RATE as f64, piece.len() as f64 / RATE as f64);
        if silent(piece, tune.silence_db) {
            continue;
        }
        let (mel, frames) = whisper_mel(piece, &filters, n_mels);
        let mut out = enc.run(ort::inputs![enc_in.as_str() => Tensor::from_array(([1usize, n_mels, frames], mel)).map_err(err)?]).map_err(err)?;
        let cross_k = out.remove(&cross_k_name).ok_or("the model gave no cross attention")?;
        let cross_v = out.remove(&cross_v_name).ok_or("the model gave no cross attention")?;
        drop(out);
        // One decoder step: logits for the last token, and the caches for the next step.
        let mut step = |tokens: Vec<i64>, k: DynValue, v: DynValue, offset: i64| -> Result<(Vec<f32>, DynValue, DynValue), String> {
            let n = tokens.len();
            let mut out = dec
                .run(ort::inputs![
                    dec_in[0].as_str() => Tensor::from_array(([1usize, n], tokens)).map_err(err)?,
                    dec_in[1].as_str() => k,
                    dec_in[2].as_str() => v,
                    dec_in[3].as_str() => &cross_k,
                    dec_in[4].as_str() => &cross_v,
                    dec_in[5].as_str() => Tensor::from_array(([1usize], vec![offset])).map_err(err)?,
                ])
                .map_err(err)?;
            let (_, logits) = output(&out, &dec_out[0])?;
            let vocab = logits.len() / n.max(1);
            if vocab == 0 || logits.len() != vocab * n {
                return Err("the model gave an unexpected output".into());
            }
            let last = logits[(n - 1) * vocab..].to_vec();
            let k = out.remove(&dec_out[1]).ok_or("the model gave no cache")?;
            let v = out.remove(&dec_out[2]).ok_or("the model gave no cache")?;
            Ok((last, k, v))
        };
        let empty = || -> Result<(DynValue, DynValue), String> {
            let zeros = || Tensor::from_array(([n_layer, 1, n_ctx, n_state], vec![0f32; cache_len])).map(|t| t.into_dyn()).map_err(err);
            Ok((zeros()?, zeros()?))
        };
        if multilingual && language.is_none() {
            let (k, v) = empty()?;
            let (logits, ..) = step(vec![sot], k, v, 0)?;
            language = languages.iter().copied().filter(|&l| (l as usize) < logits.len()).max_by(|&a, &b| logits[a as usize].total_cmp(&logits[b as usize]));
        }
        let mut seq = start.clone();
        if let Some(l) = language {
            seq[1] = l;
        }
        if tune.translate {
            seq[2] = translate;
        }
        let (k, v) = empty()?;
        let n = seq.len() as i64;
        let (mut logits, mut k, mut v) = step(seq, k, v, 0)?;
        let mut offset = n;
        let mut said: Vec<i64> = Vec::new();
        // Whisper's sample_len: at most half the context per piece, which also stops a repeating loop.
        while said.len() < n_ctx / 2 {
            for t in [no_timestamps, sot, no_speech, translate] {
                if let Some(x) = logits.get_mut(t as usize) {
                    *x = f32::NEG_INFINITY;
                }
            }
            if said.is_empty() {
                for t in [eot, blank] {
                    if let Some(x) = logits.get_mut(t as usize) {
                        *x = f32::NEG_INFINITY;
                    }
                }
            }
            if tune.time_marks {
                timestamp_rules(&mut logits, &said, ts_begin, eot as usize);
            }
            let next = (0..logits.len()).max_by(|&a, &b| logits[a].total_cmp(&logits[b])).unwrap_or(eot as usize) as i64;
            if next == eot || offset as usize >= n_ctx {
                break;
            }
            said.push(next);
            (logits, k, v) = step(vec![next], k, v, offset)?;
            offset += 1;
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
        }
        let words = |toks: &[i64]| String::from_utf8_lossy(&toks.iter().filter(|&&t| t < eot).filter_map(|t| table.get(t)).flatten().copied().collect::<Vec<u8>>()).trim().to_string();
        // <|a|> text <|b|>, then the next pair; text after the last time runs to the piece's end.
        let (mut from, mut text) = (0.0, Vec::new());
        for &t in &said {
            if t as usize >= ts_begin {
                let time = (t as usize - ts_begin) as f64 * 0.02;
                if !text.is_empty() {
                    cues.push(Cue { start: at + from, end: at + time.min(len), text: words(&text) });
                    text.clear();
                }
                from = time.min(len);
            } else {
                text.push(t);
            }
        }
        if !text.is_empty() {
            cues.push(Cue { start: at + from, end: at + len, text: words(&text) });
        }
        progress((p + 1) as f32 / parts.len() as f32);
    }
    cues.retain(|c| !c.text.is_empty());
    Ok(cues)
}

/// sherpa-onnx's tokens.txt: a token's bytes in base64, then its id, per line.
fn read_tokens(path: &Path) -> Result<std::collections::HashMap<i64, Vec<u8>>, String> {
    use base64::Engine;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let (t, id) = l.rsplit_once(' ')?;
            Some((id.trim().parse().ok()?, base64::engine::general_purpose::STANDARD.decode(t).ok()?))
        })
        .collect())
}

/// Parakeet TDT's speech in `mono` (at `RATE`), as cues of its words' times (80 ms an encoder
/// frame). `vocab` is "token id" per line, `<blk>` the blank.
pub(super) fn parakeet(encoder: &Path, decoder_joint: &Path, features: &Path, vocab: &Path, mono: &[f32], silence_db: f32, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<Cue>, String> {
    let text = std::fs::read_to_string(vocab).map_err(|e| e.to_string())?;
    let mut tokens: Vec<(usize, String)> = text.lines().filter_map(|l| l.rsplit_once(' ').and_then(|(t, id)| Some((id.trim().parse().ok()?, t.replace('\u{2581}', " "))))).collect();
    tokens.sort_by_key(|t| t.0);
    let vocab: Vec<String> = tokens.into_iter().map(|t| t.1).collect();
    let blank = vocab.iter().position(|t| t == "<blk>").ok_or("the vocabulary has no blank")?;
    let mut pre = session_with(features, cores(), false)?;
    let mut enc = session_with(encoder, cores(), false)?;
    let mut dj = session_with(decoder_joint, cores(), false)?;
    let state_shape = |name: &str| -> Result<(usize, usize), String> {
        let s = dj.inputs().iter().find(|i| i.name() == name).and_then(|i| i.dtype().tensor_shape()).ok_or("the model isn't Parakeet TDT")?;
        let ok = |v: i64| (1..=4096).contains(&v);
        if s.len() == 3 && ok(s[0]) && ok(s[2]) { Ok((s[0] as usize, s[2] as usize)) } else { Err("the model isn't Parakeet TDT".into()) }
    };
    let ((l1, h1), (l2, h2)) = (state_shape("input_states_1")?, state_shape("input_states_2")?);
    // onnx-asr: window_step 0.01 s × subsampling 8.
    const FRAME: f64 = 0.08;
    let mut cues = Vec::new();
    let parts = pieces(mono);
    for (p, range) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let piece = &mono[range.clone()];
        if silent(piece, silence_db) {
            continue;
        }
        let mut f = pre
            .run(ort::inputs![
                "waveforms" => Tensor::from_array(([1usize, piece.len()], piece.to_vec())).map_err(err)?,
                "waveforms_lens" => Tensor::from_array(([1usize], vec![piece.len() as i64])).map_err(err)?,
            ])
            .map_err(err)?;
        let (feats, lens) = (f.remove("features").ok_or("no features")?, f.remove("features_lens").ok_or("no features")?);
        drop(f);
        let out = enc.run(ort::inputs!["audio_signal" => &feats, "length" => &lens]).map_err(err)?;
        let (shape, encoded) = output(&out, "outputs")?;
        let (dim, frames) = (shape.get(1).copied().unwrap_or(0) as usize, shape.get(2).copied().unwrap_or(0) as usize);
        let len = out.get("encoded_lengths").and_then(|v| v.try_extract_tensor::<i64>().ok()).and_then(|(_, l)| l.first().copied()).unwrap_or(frames as i64).clamp(0, frames as i64) as usize;
        if shape.len() != 3 || dim == 0 || encoded.len() != dim * frames {
            return Err("the model gave an unexpected output".into());
        }
        let encoded = encoded.to_vec();
        drop(out);
        let mut states = (vec![0f32; l1 * h1], vec![0f32; l2 * h2]);
        // Each token with its frame and how many frames it lasts.
        let mut said: Vec<(usize, usize, usize)> = Vec::new();
        let (mut t, mut emitted) = (0usize, 0);
        while t < len {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let frame: Vec<f32> = (0..dim).map(|d| encoded[d * frames + t]).collect();
            let last = said.last().map_or(blank, |s| s.0) as i32;
            let out = dj
                .run(ort::inputs![
                    "encoder_outputs" => Tensor::from_array(([1usize, dim, 1], frame)).map_err(err)?,
                    "targets" => Tensor::from_array(([1usize, 1], vec![last])).map_err(err)?,
                    "target_length" => Tensor::from_array(([1usize], vec![1i32])).map_err(err)?,
                    "input_states_1" => Tensor::from_array(([l1, 1, h1], states.0.clone())).map_err(err)?,
                    "input_states_2" => Tensor::from_array(([l2, 1, h2], states.1.clone())).map_err(err)?,
                ])
                .map_err(err)?;
            let (_, logits) = output(&out, "outputs")?;
            if logits.len() <= vocab.len() {
                return Err("the model gave an unexpected output".into());
            }
            let argmax = |x: &[f32]| (0..x.len()).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(0);
            let (token, skip) = (argmax(&logits[..vocab.len()]), argmax(&logits[vocab.len()..]));
            if token != blank {
                states = (output(&out, "output_states_1")?.1.to_vec(), output(&out, "output_states_2")?.1.to_vec());
                said.push((token, t, skip.max(1)));
                emitted += 1;
            }
            if skip > 0 {
                (t, emitted) = (t + skip, 0);
            } else if token == blank || emitted == 10 {
                (t, emitted) = (t + 1, 0);
            }
        }
        // Words: a token starting with a space (▁) starts one.
        let at = range.start as f64 / RATE as f64;
        let mut words: Vec<(f64, f64, String)> = Vec::new();
        for &(token, frame, dur) in &said {
            let text = vocab.get(token).map_or("", |s| s.as_str());
            let (start, end) = (at + frame as f64 * FRAME, at + (frame + dur) as f64 * FRAME);
            match words.last_mut() {
                Some(w) if !text.starts_with(' ') => (w.1, w.2) = (end, w.2.clone() + text),
                _ => words.push((start, end, text.to_string())),
            }
        }
        // Cues never run from one piece into the next.
        cues.extend(cues_from_words(&words));
        progress((p + 1) as f32 / parts.len() as f32);
    }
    Ok(cues)
}

/// onnx-asr's `\A\s|\s\B|(\s)\b` → keep a space only before a word character, never at the start.
fn tidy_spaces(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() && (out.is_empty() || !chars.get(i + 1).is_some_and(|n| n.is_alphanumeric() || *n == '_')) {
            continue;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_cut_in_quiet_places_and_spaces_are_tidied() {
        // 70 s of tone with silence at 27 s and 52 s: cut there, no piece over 29 s.
        let mut x: Vec<f32> = (0..70 * RATE as usize).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        for at in [27, 52] {
            x[at * RATE as usize..at * RATE as usize + 3200].fill(0.0);
        }
        let p = pieces(&x);
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|r| r.len() <= MAX_PIECE));
        assert!(p[0].end.abs_diff(27 * RATE as usize + 1600) < 1600, "{p:?}");
        assert_eq!(p.last().unwrap().end, x.len());
        assert!(silent(&[0.0; 100], -60.0) && !silent(&x[..1000], -60.0) && silent(&x[..1000], -6.0));
        assert_eq!(tidy_spaces(" Hello world , ok . Fine"), "Hello world, ok. Fine");
    }

    #[test]
    fn srt_and_cues() {
        let cues = [Cue { start: 1.24, end: 4.8, text: "Hello.".into() }, Cue { start: 3725.0, end: 3726.5005, text: "Later".into() }];
        assert_eq!(srt(&cues), "1\n00:00:01,240 --> 00:00:04,800\nHello.\n\n2\n01:02:05,000 --> 01:02:06,501\nLater\n\n");
        assert_eq!(plain(&cues), "Hello. Later");
        assert_eq!(shifted(cues[..1].to_vec(), 30.0), [Cue { start: 31.24, end: 34.8, text: "Hello.".into() }]);
        let w = |a: f64, b: f64, t: &str| (a, b, t.to_string());
        // A sentence end after 1 s, a pause over 1 s, and the 7 s limit each start a cue.
        let words = [w(0.0, 0.5, " Hi."), w(0.6, 1.2, " One"), w(1.2, 1.6, " two."), w(3.0, 3.4, " Three"), w(3.5, 9.0, " four"), w(9.0, 11.0, " five")];
        let got: Vec<(f64, f64, String)> = cues_from_words(&words).into_iter().map(|c| (c.start, c.end, c.text)).collect();
        assert_eq!(got, [w(0.0, 1.6, "Hi. One two."), w(3.0, 9.0, "Three four"), w(9.0, 11.0, "five")]);
    }

    #[test]
    fn whisper_timestamp_rules() {
        // Ids: text 0–9, eot 10, timestamps from 20 (0.00 s), 20 ms each; 100 ids in all.
        let (eot, ts) = (10, 20);
        let fresh = || {
            let mut l = vec![0f32; 100];
            l[3] = 5.0;
            l
        };
        let allowed = |l: &[f32]| (0..l.len()).filter(|&i| l[i] > f32::NEG_INFINITY).collect::<Vec<_>>();
        // First: only timestamps up to 1 s (ids 20–70).
        let mut l = fresh();
        timestamp_rules(&mut l, &[], ts, eot);
        assert_eq!(allowed(&l), (20..=70).collect::<Vec<_>>());
        // After <|0.10|> text <|0.40|>: no text, nothing before 0.40 (a pair may close on it).
        let mut l = fresh();
        timestamp_rules(&mut l, &[25, 3, 40], ts, eot);
        assert!(allowed(&l).iter().all(|&i| i == eot || i >= 40), "{:?}", allowed(&l));
        // After a pair: text (or eot), no timestamps.
        let mut l = fresh();
        timestamp_rules(&mut l, &[25, 3, 40, 40], ts, eot);
        assert!(allowed(&l).iter().all(|&i| i < ts));
        // After text: a timestamp later than the last one, or text; text wins here (5 against 0s)…
        let mut l = fresh();
        timestamp_rules(&mut l, &[25, 3], ts, eot);
        assert!(allowed(&l).contains(&3) && !allowed(&l).contains(&25) && allowed(&l).contains(&26));
        // …but when all timestamps together are likelier than the best text token, a timestamp it is.
        let mut l = vec![0f32; 100];
        l[3] = 2.0;
        timestamp_rules(&mut l, &[25, 3], ts, eot);
        assert!(allowed(&l).iter().all(|&i| i >= ts));
    }
}
