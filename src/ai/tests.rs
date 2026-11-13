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
    let out = denoise::denoise(&model_path(k.file).unwrap(), vec![noisy.clone()], &|_| {}, &AtomicBool::new(false)).unwrap().remove(0);
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
    let out = ui::run_tool(Task::Denoise, &CATALOG[0], &track, &edit, &|_| {}, &AtomicBool::new(false)).unwrap();
    eprintln!("{} s of audio in {:.1} s → {}", track.len() / 44_100, t0.elapsed().as_secs_f32(), out.display());
    assert_eq!(out, dir.join("song-denoised.flac"));
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
