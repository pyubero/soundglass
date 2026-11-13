//! Remove Noise: DeepFilterNet3 as grazder's torchDF streaming export, one ONNX file that holds
//! the whole pipeline (STFT, features, the three networks, deep filtering): 480 samples at
//! 48 kHz in, 480 out, with its state carried from frame to frame.
use super::*;

pub const RATE: u32 = 48_000;
const HOP: usize = 480;
const FFT: usize = 960;
/// The state is ~45k floats; a model asking for far more isn't this one.
const MAX_STATES: i64 = 1 << 20;

/// Each channel denoised on its own thread, with its own session and state; the same length
/// out as in. Two ONNX threads per channel: 30% faster than six on this tiny network.
/// `limit_db`: at most this much reduction (the model's attenuation limit), 0 for no limit.
pub(super) fn denoise(model: &Path, channels: Vec<Vec<f32>>, limit_db: f32, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    let total = channels.iter().map(|c| c.len() / HOP + 3).sum::<usize>().max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let runs: Vec<_> = channels.into_iter().map(|ch| scope.spawn(|| channel(model, ch, limit_db, &done, cancel))).collect();
        while !runs.iter().all(|r| r.is_finished()) {
            progress(done.load(Ordering::Relaxed) as f32 / total as f32);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        runs.into_iter().map(|r| r.join().unwrap_or_else(|_| Err("the AI tool failed".into()))).collect()
    })
}

fn channel(model: &Path, mut input: Vec<f32>, limit_db: f32, done: &std::sync::atomic::AtomicUsize, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    use ort::value::Tensor;
    let mut session = session_with(model, 2, false)?;
    let states_len = session
        .inputs()
        .iter()
        .find(|i| i.name() == "states")
        .and_then(|i| i.dtype().tensor_shape())
        .and_then(|s| (s.len() == 1).then(|| s[0]))
        .filter(|&n| n > 0 && n <= MAX_STATES)
        .ok_or("the model isn't the DeepFilterNet3 streaming export")? as usize;
    let err = |e: ort::Error| e.to_string();
    let len = input.len();
    // As torchDF's pipeline: padded to whole frames plus one FFT, and the output read
    // FFT − HOP samples later (the model's delay).
    input.resize(len.div_ceil(HOP) * HOP + FFT, 0.0);
    let mut states = vec![0f32; states_len];
    let mut out = Vec::with_capacity(input.len());
    for frame in input.chunks_exact(HOP) {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let outputs = session
            .run(ort::inputs![
                "input_frame" => Tensor::from_array(([HOP], frame.to_vec())).map_err(err)?,
                "states" => Tensor::from_array(([states_len], states)).map_err(err)?,
                "atten_lim_db" => Tensor::from_array((Vec::<usize>::new(), vec![limit_db])).map_err(err)?,
            ])
            .map_err(err)?;
        let (_, enhanced) = output(&outputs, "enhanced_audio_frame")?;
        let (_, next) = output(&outputs, "new_states")?;
        if enhanced.len() != HOP || next.len() != states_len {
            return Err("the model gave an unexpected output".into());
        }
        out.extend_from_slice(enhanced);
        states = next.to_vec();
        done.fetch_add(1, Ordering::Relaxed);
    }
    out.drain(..FFT - HOP);
    out.truncate(len);
    Ok(out)
}
