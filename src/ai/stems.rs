//! Split into Stems: HTDemucs v4 (Meta), StemSplit's single-file ONNX export with the STFT
//! inside: `mix` [1, 2, 343980] at 44.1 kHz in, `stems` [1, 4, 2, 343980] out. Run in
//! overlapping segments, cross-faded, as StemSplit's infer.py and Demucs' apply_model do.
use super::*;

pub const RATE: u32 = 44_100;
/// The model's fixed segment, 7.8 s.
const SEGMENT: usize = 343_980;
pub const NAMES: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// The four stems (in `NAMES` order) of a mono or stereo mix, each stereo, as long as the mix.
/// shortcut: holds all four stems in memory (~170 MB a minute of audio is the 4×2 stems' f32s);
/// stream them to the encoders if long tracks run out of memory.
pub(super) fn separate(model: &Path, mut mix: Vec<Vec<f32>>, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<[[Vec<f32>; 2]; 4], String> {
    use ort::value::Tensor;
    let err = |e: ort::Error| e.to_string();
    if mix.len() == 1 {
        mix.push(mix[0].clone());
    }
    if mix.len() != 2 {
        return Err("Split into Stems takes mono or stereo tracks".into());
    }
    let total = mix[0].len();
    // As Demucs' api.py: the mix scaled by its mono mean and deviation, the stems scaled back.
    let mono: Vec<f32> = mix[0].iter().zip(&mix[1]).map(|(l, r)| (l + r) / 2.0).collect();
    let mean = mono.iter().map(|&x| x as f64).sum::<f64>() / total.max(1) as f64;
    // torch's std is the unbiased one.
    let std = (mono.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / (total.max(2) - 1) as f64).sqrt() + 1e-8;
    let (mean, std) = (mean as f32, std as f32);
    let mut session = session(model)?;
    let overlap = SEGMENT / 4;
    let stride = SEGMENT - overlap;
    let fade: Vec<f32> = (0..SEGMENT).map(|i| ((i.min(SEGMENT - 1 - i) + 1) as f32 / overlap as f32).min(1.0)).collect();
    let mut out: [[Vec<f32>; 2]; 4] = std::array::from_fn(|_| [vec![0f32; total], vec![0f32; total]]);
    let mut weight = vec![0f32; total];
    let chunks = total.div_ceil(stride).max(1);
    let mut input = vec![0f32; 2 * SEGMENT];
    for i in 0..chunks {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let start = i * stride;
        let len = (total - start.min(total)).min(SEGMENT);
        input.fill(0.0);
        for (c, ch) in mix.iter().enumerate() {
            for (dst, &x) in input[c * SEGMENT..c * SEGMENT + len].iter_mut().zip(&ch[start..start + len]) {
                *dst = (x - mean) / std;
            }
        }
        let outputs = session.run(ort::inputs!["mix" => Tensor::from_array(([1usize, 2, SEGMENT], input.clone())).map_err(err)?]).map_err(err)?;
        let (_, stems) = output(&outputs, "stems")?;
        if stems.len() != 4 * 2 * SEGMENT {
            return Err("the model gave an unexpected output".into());
        }
        for (s, stem) in out.iter_mut().enumerate() {
            for (c, ch) in stem.iter_mut().enumerate() {
                let part = &stems[(s * 2 + c) * SEGMENT..][..len];
                for ((dst, &y), &w) in ch[start..start + len].iter_mut().zip(part).zip(&fade) {
                    *dst += y * w;
                }
            }
        }
        weight[start..start + len].iter_mut().zip(&fade).for_each(|(a, &w)| *a += w);
        progress((i + 1) as f32 / chunks as f32);
    }
    for ch in out.iter_mut().flatten() {
        for (y, &w) in ch.iter_mut().zip(&weight) {
            *y = *y / w.max(1e-8) * std + mean;
        }
    }
    Ok(out)
}
