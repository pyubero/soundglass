# Decisions

- 2026-11-11 — crate and binary are named soundglass — free on crates.io, apt and Flathub; the app's final name is still open (user's choice)
- 2026-11-11 — licence GPL-3.0-or-later — same as lightview, so its code can be copied over (user's choice)
- 2026-11-11 — eframe 0.37 (glow, wayland, x11, accesskit) for the UI — carried over from lightview (user's choice)
- 2026-11-11 — symphonia with all codecs for decoding — pure Rust, MPL-2.0 (user's choice)
- 2026-11-11 — cpal with default features (ALSA on Linux) for sound output — Apache-2.0 (user's choice)
- 2026-11-11 — no resampling in the spike; the device must accept the file's rate — PipeWire and PulseAudio accept any rate; rubato comes in milestone 2, before Windows (user's choice)
- 2026-11-11 — std mpsc sync_channel of Vec<f32> between decoder and audio callback, no ring-buffer crate — no new dependency; accepted limit: memory is freed inside the audio callback
- 2026-11-11 — audio starts before the window opens — decoding and device setup overlap OpenGL setup, as lightview does for images
- 2026-11-11 — "first sound" is timed when the first real samples reach the cpal callback — the device's own output latency is not included
