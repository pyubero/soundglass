use super::*;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("soundglass-test-{}-{name}", std::process::id()))
}

fn sha(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn every_download_has_its_licence() {
    for k in &CATALOG {
        assert!(!k.licenses.is_empty(), "{}", k.file);
    }
    for l in CATALOG.iter().flat_map(|k| k.licenses).chain([&RUNTIME_LICENSE]) {
        let ok = match l.spdx {
            "Apache-2.0" => l.text.contains("Apache License") && l.text.contains("Version 2.0"),
            "MIT" | "MIT OR Apache-2.0" => l.text.contains("Permission is hereby granted") && l.text.contains("Copyright"),
            "CC-BY-4.0" => l.text.contains("Attribution 4.0 International"),
            other => panic!("unexpected licence {other}"),
        };
        assert!(ok, "{} text doesn't look like {}", l.project, l.spdx);
    }
}

#[test]
fn fetch_keeps_only_a_matching_file() {
    let (src, dest) = (temp("fetch-src"), temp("fetch-dest"));
    std::fs::write(&src, b"model bytes").unwrap();
    let url = format!("file://{}", src.display());
    let no = AtomicBool::new(false);
    let last = std::sync::Mutex::new(0.0);
    assert!(fetch(&url, "0000", &dest, &|_| {}, &no).is_err());
    let refused = !dest.exists();
    fetch(&url, &sha(b"model bytes"), &dest, &|p| *last.lock().unwrap() = p, &no).unwrap();
    let kept = std::fs::read(&dest).unwrap() == b"model bytes";
    // Cancelled before the first read: nothing left behind.
    std::fs::remove_file(&dest).unwrap();
    let cancelled = fetch(&url, &sha(b"model bytes"), &dest, &|_| {}, &AtomicBool::new(true)).is_err() && !dest.exists();
    std::fs::remove_file(&src).unwrap();
    assert!(refused && kept && cancelled);
    assert_eq!(*last.lock().unwrap(), 1.0);
}

#[test]
fn two_file_download_is_all_or_nothing() {
    let root = temp("dl");
    let (src, dest) = (root.join("src"), root.join("models"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("m.onnx"), b"graph").unwrap();
    std::fs::write(src.join("m.onnx.data"), b"weights").unwrap();
    let base = format!("file://{}", src.display()).leak();
    let known = |data_sha: &'static str| Known { file: "m.onnx", label: "", task: Task::Denoise, mb: 0, sha256: sha(b"graph").leak(), base, data: vec![("m.onnx.data", data_sha)].leak(), converted_by: None, licenses: &[] };
    let no = AtomicBool::new(false);
    // A bad checksum on the second file installs nothing, not even the good first one.
    assert!(download_to(&dest, &known("0000"), &|_| {}, &no).is_err());
    let leftovers: Vec<_> = std::fs::read_dir(&dest).unwrap().collect();
    download_to(&dest, &known(sha(b"weights").leak()), &|_| {}, &no).unwrap();
    let ok = dest.join("m.onnx").is_file() && dest.join("m.onnx.data").is_file();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(ok);
}

#[test]
fn short_gpu_errors() {
    assert_eq!(short_error("blah blah CUDA failure 100: no CUDA-capable device ; GPU=0 ; host=x"), "CUDA failure 100: no CUDA-capable device");
    assert_eq!(short_error(&"x".repeat(200)).chars().count(), 161);
}

/// Online: `XDG_DATA_HOME=target/aitest cargo test --release -- --ignored install_runtime`.
#[test]
#[ignore]
pub(super) fn install_runtime() {
    assert!(data_dir().unwrap().starts_with(std::env::current_dir().unwrap().join("target")), "use a throwaway XDG_DATA_HOME");
    if !runtime_path().unwrap().is_file() {
        download_runtime(&|_| {}, &AtomicBool::new(false)).unwrap();
    }
    environment().unwrap();
}

/// Online, then on the CPU: `XDG_DATA_HOME=$PWD/target/aitest cargo test --release -- --ignored denoise_`.
#[test]
#[ignore]
fn denoise_lowers_noise() {
    install_runtime();
    let k = &CATALOG[0];
    if !model_path(k.file).unwrap().is_file() {
        download(k, &|_| {}, &AtomicBool::new(false)).unwrap();
    }
    // 3 s of white noise at −30 dBFS RMS, a harmonic "voice" in the middle second.
    let rate = denoise::RATE as f32;
    let mut seed = 1u32;
    let noisy: Vec<f32> = (0..3 * denoise::RATE as usize)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.055;
            let t = i as f32 / rate;
            let voice = if (1.0..2.0).contains(&t) { (1..6).map(|h| (std::f32::consts::TAU * 180.0 * h as f32 * t).sin() * 0.2 / h as f32).sum() } else { 0.0 };
            noise + voice
        })
        .collect();
    let t0 = std::time::Instant::now();
    let out = denoise::denoise(&model_path(k.file).unwrap(), vec![noisy.clone()], 0.0, &|_| {}, &AtomicBool::new(false)).unwrap().remove(0);
    eprintln!("3 s denoised in {:.2} s", t0.elapsed().as_secs_f32());
    let db = |x: &[f32]| 10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).log10();
    let (noise_in, noise_out) = (db(&noisy[24_000..44_000]), db(&out[24_000..44_000]));
    let (voice_in, voice_out) = (db(&noisy[60_000..90_000]), db(&out[60_000..90_000]));
    eprintln!("noise {noise_in:.1} → {noise_out:.1} dB, voice {voice_in:.1} → {voice_out:.1} dB");
    assert_eq!(out.len(), noisy.len());
    assert!(noise_out < noise_in - 15.0);
    assert!(voice_out > voice_in - 6.0);
}

/// The whole tool on a temp copy of an album track: a FLAC next to it, as long, at its rate.
#[test]
#[ignore]
fn denoise_album_track() {
    denoise_lowers_noise();
    let album = std::env::current_dir().unwrap().join("1954-Louis Armstrong Plays W.C.Handy");
    let src = crate::folder::list_dir(&album).unwrap().1.into_iter().next().expect("the test album");
    let dir = temp("denoise-album");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("song.mp3");
    std::fs::copy(&src, &copy).unwrap();
    let track = crate::track::Track::load(copy.clone(), || {});
    while !track.done() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let edit = crate::edit::Edit::whole(track.len());
    let t0 = std::time::Instant::now();
    let out = dir.join("song-denoised.flac");
    assert_eq!(ui::run_tool(Task::Denoise, &CATALOG[0], &tune_for(&CATALOG[0], &Default::default()), &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap(), ui::Done::Open(out.clone()));
    eprintln!("{} s of audio in {:.1} s → {}", track.len() / 44_100, t0.elapsed().as_secs_f32(), out.display());
    let back = crate::track::Track::load(out, || {});
    while !back.done() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (a, b) = (track.format.get().unwrap(), back.format.get().unwrap());
    assert_eq!((a.rate, a.channels), (b.rate, b.channels));
    let near = track.len().abs_diff(back.len()) < 100;
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(near, "{} vs {}", track.len(), back.len());
}

/// A temp copy of the album's first track, loaded; and its folder.
fn album_copy(name: &str) -> (std::sync::Arc<crate::track::Track>, PathBuf) {
    let album = std::env::current_dir().unwrap().join("1954-Louis Armstrong Plays W.C.Handy");
    let src = crate::folder::list_dir(&album).unwrap().1.into_iter().next().expect("the test album");
    let dir = temp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("song.mp3");
    std::fs::copy(&src, &copy).unwrap();
    let track = crate::track::Track::load(copy, || {});
    while !track.done() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    (track, dir)
}

fn installed_model(task: Task) -> &'static Known {
    install_runtime();
    let k = CATALOG.iter().find(|k| k.task == task).unwrap();
    if !model_path(k.file).unwrap().is_file() {
        download(k, &|_| {}, &AtomicBool::new(false)).unwrap();
    }
    k
}

/// Online, then the first 60 s of an album track: four stems that add back up to the mix.
#[test]
#[ignore]
fn stems_add_up_to_the_mix() {
    let k = installed_model(Task::Stems);
    let (track, dir) = album_copy("stems");
    let edit = crate::edit::Edit::whole(track.len()).crop(30 * 44_100, 90 * 44_100);
    let t0 = std::time::Instant::now();
    let done = ui::run_tool(Task::Stems, k, &tune_for(k, &Default::default()), &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap();
    eprintln!("60 s split in {:.1} s on the {}", t0.elapsed().as_secs_f32(), if on_gpu() { "GPU" } else { "CPU" });
    assert_eq!(done, ui::Done::Open(dir.join("song-stems").join("vocals.flac")));
    let mix = audio::planar(&track, &edit, 44_100).unwrap();
    let stems: Vec<Vec<Vec<f32>>> = stems::NAMES
        .iter()
        .map(|n| {
            let t = crate::track::Track::load(dir.join("song-stems").join(format!("{n}.flac")), || {});
            while !t.done() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            audio::planar(&t, &crate::edit::Edit::whole(t.len()), 44_100).unwrap()
        })
        .collect();
    let db = |x: f64| 10.0 * x.log10();
    let (mut signal, mut residual, mut voc) = (0f64, 0f64, 0f64);
    for c in 0..2 {
        for i in 0..stems.iter().map(|s| s[c].len()).min().unwrap() {
            let sum: f32 = stems.iter().map(|s| s[c][i]).sum();
            signal += (mix[c][i] as f64).powi(2);
            residual += ((mix[c][i] - sum) as f64).powi(2);
            voc += (stems[3][c][i] as f64).powi(2);
        }
    }
    eprintln!("mix − sum of stems: {:.1} dB below the mix; vocals {:.1} dB below the mix", db(signal / residual), db(signal / voc));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(db(signal / residual) > 15.0);
}

#[test]
fn saved_results_keep_their_length() {
    let dir = temp("save-flac");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = Vec::new();
    for frames in [2_646_000usize, 100_000] {
        let ch: Vec<Vec<f32>> = (0..2).map(|c| (0..frames).map(|i| ((i * (c + 3)) % 200) as f32 / 400.0).collect()).collect();
        let clipped: Vec<Vec<f32>> = (0..2).map(|c| (0..frames).map(|i| 2.0 * (i as f32 * 0.01 * (c + 1) as f32).sin()).collect()).collect();
        let path = dir.join(format!("{frames}-clipped.flac"));
        audio::save_flac(&clipped, 44_100, 44_100, &path).unwrap();
        let t = crate::track::Track::load(path, || {});
        while !t.done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        out.push((frames as u64, t.len()));
        let path = dir.join(format!("{frames}.flac"));
        audio::save_flac(&ch, 44_100, 44_100, &path).unwrap();
        let t = crate::track::Track::load(path, || {});
        while !t.done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        out.push((frames as u64, t.len()));
    }
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.iter().all(|(a, b)| a == b), "{out:?}");
}

#[test]
fn notes_from_activations_and_midi_bytes() {
    // Key 39 (C4) on from frame 10 to 40 with an onset at 10; key 43 too short (5 frames).
    let mut frames = vec![[0f32; 88]; 100];
    let mut onsets = vec![[0f32; 88]; 100];
    (10..40).for_each(|t| frames[t][39] = 0.8);
    (60..65).for_each(|t| frames[t][43] = 0.9);
    onsets[10][39] = 0.9;
    let notes = midi::notes(&frames, &onsets, &Default::default());
    assert_eq!(notes.len(), 1, "{notes:?}");
    let n = notes[0];
    assert_eq!((n.start, n.end, n.key), (10, 40, 60));
    assert!((n.amp - 0.8).abs() < 1e-6);
    // A held note without an onset is still found, by the melodia trick.
    let mut frames = vec![[0f32; 88]; 100];
    (20..50).for_each(|t| frames[t][50] = 0.5);
    let notes = midi::notes(&frames, &vec![[0f32; 88]; 100], &Default::default());
    assert_eq!(notes.iter().map(|n| n.key).collect::<Vec<_>>(), [71]);
    let smf = midi::smf(&notes, 120.0);
    assert_eq!(&smf[..14], b"MThd\0\0\0\x06\0\0\0\x01\0\xDC");
    let len = u32::from_be_bytes(smf[18..22].try_into().unwrap()) as usize;
    assert_eq!(smf.len(), 22 + len);
    assert_eq!(&smf[smf.len() - 3..], [0xFF, 0x2F, 0x00]);
    assert!(smf.windows(2).any(|w| w == [0x90, 71]) && smf.windows(2).any(|w| w == [0x80, 71]));
}

/// Online: a synthesised tune of known notes comes back as those notes.
#[test]
#[ignore]
fn midi_finds_a_played_tune() {
    let k = installed_model(Task::Midi);
    let tune = [60u8, 62, 64, 65, 67, 69, 71, 72];
    let rate = midi::RATE as f32;
    let mono: Vec<f32> = tune
        .iter()
        .flat_map(|&key| {
            let f = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
            (0..midi::RATE as usize / 2).map(move |i| {
                let t = i as f32 / rate;
                let env = (t * 40.0).min(1.0) * (-t * 3.0).exp();
                env * (1..5).map(|h| (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.3
            })
        })
        .collect();
    let (frames, onsets) = midi::activations(&model_path(k.file).unwrap(), &mono, &|_| {}, &AtomicBool::new(false)).unwrap();
    let mut notes = midi::notes(&frames, &onsets, &Default::default());
    notes.sort_by_key(|n| n.start);
    eprintln!("{:?}", notes.iter().map(|n| (n.key, format!("{:.2}", n.start as f32 * 256.0 / rate))).collect::<Vec<_>>());
    assert_eq!(frames.len(), (mono.len() as f64 / (midi_stride() as f64) * 142.0) as usize);
    // Every note played, in order, at its time (an overtone or so may come with them).
    let mut found = notes.iter();
    for (i, key) in tune.iter().enumerate() {
        let n = found.find(|n| n.key == *key).unwrap_or_else(|| panic!("{key} missing"));
        assert!((n.start as f32 * 256.0 / rate - i as f32 * 0.5).abs() < 0.05);
    }
    assert!(notes.len() <= tune.len() + 2);
}

fn midi_stride() -> usize {
    (22_050 * 2 - 256) - 30 * 256
}

/// Online: the whole tool on an album track; the MIDI file is checked by hand (ffprobe can't).
#[test]
#[ignore]
fn midi_album_track() {
    let k = installed_model(Task::Midi);
    let (track, dir) = album_copy("midi");
    let edit = crate::edit::Edit::whole(track.len()).crop(0, 60 * 44_100);
    let t0 = std::time::Instant::now();
    let done = ui::run_tool(Task::Midi, k, &tune_for(k, &Default::default()), &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap();
    let out = dir.join("song-melody.mid");
    eprintln!("60 s in {:.1} s, {} bytes", t0.elapsed().as_secs_f32(), std::fs::metadata(&out).unwrap().len());
    assert_eq!(done, ui::Done::Said("Saved song-melody.mid".into()));
    if let Ok(keep) = std::env::var("SG_KEEP") {
        std::fs::copy(&out, keep).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Online: Beat This! on album tracks (SG_TRACKS="02 06 11"), the numbers BEAT_TRACKING.md
/// keeps, and (with SG_CLICKS=dir) each track with clicks mixed in, to check by ear.
#[test]
#[ignore]
fn beats_on_the_album() {
    let k = installed_model(Task::Beats);
    let album = std::env::current_dir().unwrap().join("1954-Louis Armstrong Plays W.C.Handy");
    let files = crate::folder::list_dir(&album).unwrap().1;
    let wanted = std::env::var("SG_TRACKS").unwrap_or("02".into());
    for n in wanted.split_whitespace() {
        let src = files.iter().find(|f| f.file_name().unwrap().to_string_lossy().starts_with(n)).unwrap();
        let track = crate::track::Track::load(src.clone(), || {});
        while !track.done() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let edit = crate::edit::Edit::whole(track.len());
        let t0 = std::time::Instant::now();
        let said = match ui::run_tool(Task::Beats, k, &tune_for(k, &Default::default()), &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap() {
            ui::Done::Said(s) => s,
            other => panic!("{other:?}"),
        };
        let secs = t0.elapsed().as_secs_f32();
        let b = track.beats.lock().unwrap().clone().unwrap();
        let rate = track.format.get().unwrap().rate as f64;
        let bpm: Vec<f64> = b.beats.windows(2).map(|w| 60.0 * rate / (w[1] - w[0]) as f64).collect();
        if bpm.len() < 5 {
            eprintln!("{}: {} beats, {secs:.1} s", src.file_name().unwrap().to_string_lossy(), b.beats.len());
            continue;
        }
        let mut sorted = bpm.clone();
        sorted.sort_by(f64::total_cmp);
        let fifth = (bpm.len() / 5).max(1);
        let mean = |x: &[f64]| x.iter().sum::<f64>() / x.len() as f64;
        eprintln!(
            "{}: {} — median {:.1} BPM, {:.0}–{:.0} (5th–95th pct), {:.1} → {:.1}, {:.1} s; {said}",
            src.file_name().unwrap().to_string_lossy(), b.beats.len(), sorted[sorted.len() / 2], sorted[sorted.len() / 20], sorted[sorted.len() * 19 / 20], mean(&bpm[..fifth]), mean(&bpm[bpm.len() - fifth..]), secs
        );
        let per30: Vec<usize> = (0..track.len().div_ceil(30 * rate as u64)).map(|w| b.beats.iter().filter(|&&f| f / (30 * rate as u64) == w).count()).collect();
        eprintln!("  beats per 30 s: {per30:?}");
        if let Ok(dir) = std::env::var("SG_CLICKS") {
            let ch = audio::planar(&track, &edit, rate as u32).unwrap();
            let mut mono: Vec<f32> = (0..ch[0].len()).map(|i| ch.iter().map(|c| c[i]).sum::<f32>() / ch.len() as f32).collect();
            for &beat in &b.beats {
                let (f, amp) = if b.downbeats.binary_search(&beat).is_ok() { (2000.0, 0.5) } else { (1500.0, 0.35) };
                for j in 0..(0.025 * rate) as usize {
                    if let Some(x) = mono.get_mut(beat as usize + j) {
                        let t = j as f64 / rate;
                        *x += (amp * (std::f64::consts::TAU * f * t).sin() * (-t / 0.006).exp()) as f32;
                    }
                }
            }
            audio::save_flac(&[mono], rate as u32, rate as u32, &PathBuf::from(&dir).join(format!("{n}-clicks.flac"))).unwrap();
        }
    }
}

/// By hand: our log-mel against a dump of a reference (SG_DUMP holds audio.f32 and mel.f32).
#[test]
#[ignore]
fn log_mel_matches_the_reference() {
    let k = installed_model(Task::Beats);
    let dir = PathBuf::from(std::env::var("SG_DUMP").unwrap());
    let read = |p: PathBuf| std::fs::read(p).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<f32>>();
    let bank = read(model_path(k.data[0].0).unwrap());
    let ours = beats::log_mel(&read(dir.join("audio.f32")), &bank);
    let want = read(dir.join("mel.f32"));
    let diff = ours.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    eprintln!("{} vs {} values, max diff {diff}", ours.len(), want.len());
    assert!(ours.len() == want.len() && diff < 1e-3);
}

/// By hand: SG_DUMP gets our Whisper mel filters, to compare with OpenAI's mel_filters.npz.
#[test]
#[ignore]
fn dump_whisper_filters() {
    let dir = PathBuf::from(std::env::var("SG_DUMP").unwrap());
    for n in [80, 128] {
        let f = transcribe::mel_filters(n);
        std::fs::write(dir.join(format!("filters{n}.f32")), f.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    }
}

/// Online: each transcription model (or those whose file starts with SG_MODEL) on SG_WAV.
#[test]
#[ignore]
fn transcribe_a_recording() {
    install_runtime();
    let wav = PathBuf::from(std::env::var("SG_WAV").unwrap());
    let only = std::env::var("SG_MODEL").unwrap_or_default();
    let track = crate::track::Track::load(wav, || {});
    while !track.done() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let edit = crate::edit::Edit::whole(track.len());
    for k in CATALOG.iter().filter(|k| k.task == Task::Transcribe && k.file.starts_with(&only)) {
        if !std::iter::once(k.file).chain(k.data.iter().map(|d| d.0)).all(|f| model_path(f).unwrap().is_file()) {
            download(k, &|_| {}, &AtomicBool::new(false)).unwrap();
        }
        let t0 = std::time::Instant::now();
        let done = ui::run_tool(Task::Transcribe, k, &tune_for(k, &Default::default()), &track, &edit, &|_| {}, &AtomicBool::new(false));
        eprintln!("{} ({:.1} s): {done:?}", k.file, t0.elapsed().as_secs_f32());
    }
}

/// Online (the runtime): a garbage or truncated model file is an error, not a crash.
#[test]
#[ignore]
fn broken_models_are_errors() {
    install_runtime();
    let dir = temp("broken-model");
    std::fs::create_dir_all(&dir).unwrap();
    let garbage = dir.join("garbage.onnx");
    std::fs::write(&garbage, b"not a model at all").unwrap();
    let truncated = dir.join("truncated.onnx");
    let real = std::fs::read(model_path("nmp.onnx").unwrap()).unwrap();
    std::fs::write(&truncated, &real[..real.len() / 3]).unwrap();
    let results = [session(&garbage).is_err(), session(&truncated).is_err()];
    let midi = midi::activations(&garbage, &[0.0; 1000], &|_| {}, &AtomicBool::new(false)).is_err();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(results, [true, true]);
    assert!(midi);
}

/// Online, ~1.5 GB: `XDG_DATA_HOME=$PWD/target/aitest cargo test --release -- --ignored install_gpu`.
#[test]
#[ignore]
fn install_gpu() {
    assert!(data_dir().unwrap().starts_with(std::env::current_dir().unwrap().join("target")), "use a throwaway XDG_DATA_HOME");
    if !gpu_installed() {
        download_gpu(&|_| {}, &AtomicBool::new(false)).unwrap();
    }
    assert!(gpu_installed());
}

#[test]
fn midi_settings_change_the_notes() {
    use tune::MidiTune;
    // C4 from 50 to 80 (onset 0.9, level 0.5); G4 held from the very start to 40, at 0.4, so it
    // has no rise for an inferred onset: only the melodia trick finds it.
    let mut frames = vec![[0f32; 88]; 100];
    let mut onsets = vec![[0f32; 88]; 100];
    (50..80).for_each(|t| frames[t][39] = 0.5);
    onsets[50][39] = 0.9;
    (0..40).for_each(|t| frames[t][46] = 0.4);
    let keys = |t: MidiTune| {
        let mut k: Vec<u8> = midi::notes(&frames, &onsets, &t).iter().map(|n| n.key).collect();
        k.sort();
        k
    };
    let d = MidiTune::default();
    assert_eq!(keys(d), [60, 67]);
    // The note threshold over both levels: neither lasts.
    assert!(keys(MidiTune { frame: 0.6, ..d }).is_empty());
    // An onset threshold over 0.9 leaves C4 to the melodia trick, which still finds it.
    assert_eq!(keys(MidiTune { onset: 0.95, ..d }).len(), 2);
    assert_eq!(keys(MidiTune { melodia: false, ..d }), [60]);
    assert_eq!(keys(MidiTune { low: 62, ..d }), [67]);
    assert_eq!(keys(MidiTune { high: 64, ..d }), [60]);
    // C4's 30 frames are ~350 ms, too short at a 400 ms minimum; G4's 40 frames are long enough.
    assert_eq!(keys(MidiTune { min_note_ms: 400.0, ..d }), [67]);
    // The tempo is written as µs a beat: 500 000 at 120 bpm, 1 000 000 at 60.
    let notes = midi::notes(&frames, &onsets, &d);
    assert_eq!(midi::smf(&notes, 120.0)[22..30], [0, 0xFF, 0x51, 3, 0x07, 0xA1, 0x20, 0]);
    assert_eq!(midi::smf(&notes, 60.0)[22..30], [0, 0xFF, 0x51, 3, 0x0F, 0x42, 0x40, 0]);
}

#[test]
fn stems_need_one_chosen() {
    let k = CATALOG.iter().find(|k| k.task == Task::Stems).unwrap();
    let track = crate::track::Track::from_samples(44_100, 2, vec![0.0; 2000]);
    let none = Tune::Stems(tune::StemsTune { write: [false; 4], ..Default::default() });
    let r = ui::run_tool(Task::Stems, k, &none, &track, &crate::edit::Edit::whole(1000), &|_| {}, &AtomicBool::new(false));
    assert!(r.unwrap_err().contains("at least one stem"));
}

/// Online: the settings reach the models (denoise limit, chosen stems, Whisper's language).
#[test]
#[ignore]
fn settings_reach_the_models() {
    let k = installed_model(Task::Denoise);
    let mut seed = 1u32;
    let noise: Vec<f32> = (0..2 * denoise::RATE as usize)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 0.055
        })
        .collect();
    let db = |x: &[f32]| 10.0 * (x[20_000..].iter().map(|v| v * v).sum::<f32>() / (x.len() - 20_000) as f32).log10();
    let out = denoise::denoise(&model_path(k.file).unwrap(), vec![noise.clone()], 12.0, &|_| {}, &AtomicBool::new(false)).unwrap().remove(0);
    eprintln!("12 dB limit: noise {:.1} → {:.1} dB", db(&noise), db(&out));
    assert!((db(&noise) - db(&out) - 12.0).abs() < 3.0);

    let k = installed_model(Task::Stems);
    let (track, dir) = album_copy("stems-chosen");
    let edit = crate::edit::Edit::whole(track.len()).crop(0, 10 * 44_100);
    let only = Tune::Stems(tune::StemsTune { write: [false, true, false, false], ..Default::default() });
    let done = ui::run_tool(Task::Stems, k, &only, &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.join("song-stems")).unwrap().map(|e| e.unwrap().file_name()).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(done, ui::Done::Open(dir.join("song-stems").join("bass.flac")));
    assert_eq!(files, ["bass.flac"]);

    if let Ok(wav) = std::env::var("SG_WAV") {
        let k = CATALOG.iter().find(|k| k.file.starts_with("small")).unwrap();
        let track = crate::track::Track::load(wav.into(), || {});
        while !track.done() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let edit = crate::edit::Edit::whole(track.len());
        let run = |language: &str, translate: bool| ui::run_tool(Task::Transcribe, k, &Tune::Speech(tune::SpeechTune { language: language.into(), translate, ..Default::default() }), &track, &edit, &|_| {}, &AtomicBool::new(false));
        eprintln!("en: {:?}", run("en", false));
        eprintln!("fr: {:?}", run("fr", false));
        eprintln!("translate: {:?}", run("", true));
        assert!(run("xx", false).unwrap_err().contains("doesn't know the language"));
    }
}

/// Online: each transcription model (or those starting with SG_MODEL) on SG_WAV with time marks:
/// cues in order, inside the recording; the first ones are printed to check by ear.
#[test]
#[ignore]
fn subtitles_of_a_recording() {
    install_runtime();
    let only = std::env::var("SG_MODEL").unwrap_or_default();
    let track = crate::track::Track::load(std::env::var("SG_WAV").unwrap().into(), || {});
    while !track.done() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let secs = track.len() as f64 / track.format.get().unwrap().rate as f64;
    let edit = crate::edit::Edit::whole(track.len());
    for k in CATALOG.iter().filter(|k| k.task == Task::Transcribe && k.file.starts_with(&only)) {
        if !std::iter::once(k.file).chain(k.data.iter().map(|d| d.0)).all(|f| model_path(f).unwrap().is_file()) {
            download(k, &|_| {}, &AtomicBool::new(false)).unwrap();
        }
        let tune = Tune::Speech(tune::SpeechTune { time_marks: true, ..Default::default() });
        let ui::Done::Text(srt, true) = ui::run_tool(Task::Transcribe, k, &tune, &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap() else { panic!() };
        let time = |t: &str| {
            let (hms, ms) = t.trim().split_once(',').unwrap();
            let p: Vec<f64> = hms.split(':').map(|x| x.parse().unwrap()).collect();
            p[0] * 3600.0 + p[1] * 60.0 + p[2] + ms.parse::<f64>().unwrap() / 1000.0
        };
        let spans: Vec<(f64, f64)> = srt.lines().filter_map(|l| l.split_once(" --> ")).map(|(a, b)| (time(a), time(b))).collect();
        eprintln!("{}: {} cues over {secs:.1} s\n{}", k.file, spans.len(), srt.chars().take(700).collect::<String>());
        assert!(!spans.is_empty());
        assert!(spans.iter().all(|&(a, b)| a <= b && b <= secs + 0.1), "{spans:?}");
        assert!(spans.windows(2).all(|w| w[0].0 <= w[1].0), "{spans:?}");
    }
}
