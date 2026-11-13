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
fn install_runtime() {
    assert!(data_dir().unwrap().starts_with(std::env::current_dir().unwrap().join("target")), "use a throwaway XDG_DATA_HOME");
    if !runtime_path().unwrap().is_file() {
        download_runtime(&|_| {}, &AtomicBool::new(false)).unwrap();
    }
    environment().unwrap();
}
