//! Transcribe Speech: Whisper (sherpa-onnx's export: our log-mel, encoder → cross K/V, decoder
//! with self-attention caches, greedy, as sherpa-onnx's scripts/whisper/test.py) or NVIDIA's
//! Parakeet TDT (istupakov's export with its own feature graph, greedy TDT as onnx-asr). Long
//! audio is cut into pieces of at most 29 s at quiet moments; silent pieces are skipped.
use super::*;
use ort::value::{DynValue, Tensor};

pub const RATE: u32 = 16_000;
const MAX_PIECE: usize = 29 * RATE as usize;
/// Pieces quieter than this (RMS) are left out: Whisper makes up words in silence.
const SILENT_RMS: f32 = 0.001;

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

fn silent(x: &[f32]) -> bool {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt() < SILENT_RMS
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

/// Whisper's text of `mono` (at `RATE`), the language detected from the first piece with sound.
pub(super) fn whisper(encoder: &Path, decoder: &Path, tokens: &Path, mono: &[f32], progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<String, String> {
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
    start.push(no_timestamps);
    let languages = if multilingual { list("all_language_tokens")? } else { Vec::new() };
    drop(meta);
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
    let mut language: Option<i64> = None;
    let mut text = String::new();
    let parts = pieces(mono);
    for (p, range) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let piece = &mono[range.clone()];
        if silent(piece) {
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
        let bytes: Vec<u8> = said.iter().filter_map(|t| table.get(t)).flatten().copied().collect();
        let piece_text = String::from_utf8_lossy(&bytes);
        if !piece_text.trim().is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(piece_text.trim());
        }
        progress((p + 1) as f32 / parts.len() as f32);
    }
    Ok(text)
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

/// Parakeet TDT's text of `mono` (at `RATE`). `vocab` is "token id" per line, `<blk>` the blank.
pub(super) fn parakeet(encoder: &Path, decoder_joint: &Path, features: &Path, vocab: &Path, mono: &[f32], progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<String, String> {
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
    let mut words: Vec<usize> = Vec::new();
    let parts = pieces(mono);
    for (p, range) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let piece = &mono[range.clone()];
        if silent(piece) {
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
        let mut said: Vec<usize> = Vec::new();
        let (mut t, mut emitted) = (0usize, 0);
        while t < len {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let frame: Vec<f32> = (0..dim).map(|d| encoded[d * frames + t]).collect();
            let last = said.last().copied().unwrap_or(blank) as i32;
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
                said.push(token);
                emitted += 1;
            }
            if skip > 0 {
                (t, emitted) = (t + skip, 0);
            } else if token == blank || emitted == 10 {
                (t, emitted) = (t + 1, 0);
            }
        }
        // Pieces join with a space.
        words.extend(said);
        words.push(usize::MAX);
        progress((p + 1) as f32 / parts.len() as f32);
    }
    let joined: String = words.iter().map(|&w| vocab.get(w).map_or(" ", |s| s.as_str())).collect();
    Ok(tidy_spaces(&joined))
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
        assert!(silent(&[0.0; 100]) && !silent(&x[..1000]));
        assert_eq!(tidy_spaces(" Hello world , ok . Fine"), "Hello world, ok. Fine");
    }
}
