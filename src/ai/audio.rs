//! Audio in and out of the models: the edited track at a model's rate, one Vec per channel,
//! and results saved through save.rs (temp file, decoded back, renamed into place).
use super::*;
use crate::edit::Edit;
use crate::track::Track;

/// The edited track's channels at `rate`.
pub(super) fn planar(track: &Track, edit: &Edit, rate: u32) -> Result<Vec<Vec<f32>>, String> {
    let own = track.format.get().ok_or("the track has no audio")?.rate;
    let resampled = if own != rate { Some(crate::save::resample(track, edit, rate, None)?) } else { None };
    let (track, edit) = resampled.as_ref().map_or((track, edit), |(t, e)| (&**t, e));
    let channels = edit.channels(track.format.get().ok_or("the track has no audio")?.channels);
    let mut out = vec![Vec::with_capacity(edit.len() as usize); channels];
    let whole = crate::edit::read(track, edit, 0, edit.len(), |block| {
        for frame in block.chunks_exact(channels) {
            out.iter_mut().zip(frame).for_each(|(c, &x)| c.push(x));
        }
    });
    if whole { Ok(out) } else { Err("the track isn't fully loaded".into()) }
}

/// Saves `channels` (at `rate`) to `target` at `keep_rate`, the original's rate, as `kind`
/// (24-bit FLAC or Ogg Vorbis q6).
pub(super) fn save_audio(channels: &[Vec<f32>], rate: u32, keep_rate: u32, kind: crate::save::Kind, target: &Path) -> Result<(), String> {
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let interleaved: Vec<f32> = (0..frames).flat_map(|i| channels.iter().map(move |c| c[i])).collect();
    let track = Track::from_samples(rate, channels.len(), interleaved);
    let choice = crate::save::Choice { kind, flac_bits: 24, vorbis_quality: 6, rate: Some(keep_rate), ..Default::default() };
    // Ok(false) only says the tags weren't copied: these files start without any.
    crate::save::save(&track, &Edit::whole(frames as u64), choice, target, None).map(drop)
}
