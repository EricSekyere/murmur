//! Regression tests for CLI defects, run against the real binary. Every spawn
//! points `MURMUR_CONFIG_DIR` and `MURMUR_DATA_DIR` at per-test temp dirs so
//! the user's real config and models are never touched.

use std::path::PathBuf;
use std::process::{Command, Output};

use murmur_core::stt::models::SttModel;

/// Per-test config/data sandbox, removed on drop.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("murmur-cli-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("create config dir");
        std::fs::create_dir_all(root.join("data")).expect("create data dir");
        Self { root }
    }

    fn config_base(&self) -> PathBuf {
        self.root.join("config")
    }

    fn data_base(&self) -> PathBuf {
        self.root.join("data")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_murmur"))
            .args(args)
            .env("MURMUR_CONFIG_DIR", self.config_base())
            .env("MURMUR_DATA_DIR", self.data_base())
            .output()
            .expect("spawn murmur binary")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn invalid_hotkey_is_rejected_and_config_preserved() {
    let sb = Sandbox::new("hotkey-invalid");

    let ok = sb.run(&["config", "--hotkey", "ctrl+alt+m"]);
    assert!(ok.status.success(), "valid hotkey must save: {ok:?}");

    for bad in ["", "   ", "ctrl++space"] {
        let out = sb.run(&["config", "--hotkey", bad]);
        assert!(
            !out.status.success(),
            "invalid hotkey {bad:?} must exit non-zero"
        );
        assert!(
            stderr_text(&out).to_lowercase().contains("hotkey"),
            "error must explain the hotkey rejection: {}",
            stderr_text(&out)
        );
    }

    // The earlier valid hotkey must survive, and no recovery backup may exist
    // (a .bak means a bad save reached disk and load-time recovery fired).
    let show = sb.run(&["config", "--show"]);
    assert!(show.status.success());
    assert!(
        stdout_text(&show).contains("ctrl+alt+m"),
        "config must be intact after rejected saves: {}",
        stdout_text(&show)
    );
    let murmur_dir = sb.config_base().join("murmur");
    let baks: Vec<_> = std::fs::read_dir(&murmur_dir)
        .expect("config dir exists")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".bak"))
        .collect();
    assert!(baks.is_empty(), "no backup file may be created: {baks:?}");
}

#[test]
fn config_flags_are_mutually_exclusive() {
    let sb = Sandbox::new("config-flags");
    for args in [
        &["config", "--show", "--reset"][..],
        &["config", "--reset", "--hotkey", "ctrl+alt+m"][..],
        &["config", "--show", "--hotkey", "ctrl+alt+m"][..],
    ] {
        let out = sb.run(args);
        assert!(!out.status.success(), "{args:?} must be rejected");
        assert!(
            stderr_text(&out).contains("cannot be used with"),
            "clap must report the conflict for {args:?}: {}",
            stderr_text(&out)
        );
    }
}

#[test]
fn index_on_a_file_is_an_error() {
    let sb = Sandbox::new("index-file");
    let file = sb.data_base().join("not-a-project.txt");
    std::fs::write(&file, "hello").expect("write fixture");

    let out = sb.run(&["index", file.to_str().expect("utf8 path")]);
    assert!(!out.status.success(), "indexing a file must exit non-zero");
    assert!(
        stderr_text(&out).contains("expects a project directory"),
        "error must say a directory is expected: {}",
        stderr_text(&out)
    );

    let missing = sb.data_base().join("does-not-exist");
    let out = sb.run(&["index", missing.to_str().expect("utf8 path")]);
    assert!(!out.status.success(), "missing path must still error");
}

#[test]
fn transcribe_on_a_directory_is_a_clear_error() {
    let sb = Sandbox::new("transcribe-dir");
    let dir = sb.data_base().join("some-dir");
    std::fs::create_dir_all(&dir).expect("create fixture dir");

    let out = sb.run(&["transcribe", dir.to_str().expect("utf8 path")]);
    assert!(!out.status.success(), "transcribing a directory must fail");
    assert!(
        stderr_text(&out).contains("is a directory"),
        "error must name the problem: {}",
        stderr_text(&out)
    );
}

/// A build without an STT backend must refuse to transcribe instead of
/// printing an empty transcript with exit 0, and must not start a download.
#[cfg(not(any(feature = "full", feature = "cuda", feature = "vulkan")))]
#[test]
fn stub_build_refuses_to_transcribe() {
    let sb = Sandbox::new("stub-transcribe");
    let wav = sb.data_base().join("clip.wav");
    std::fs::write(&wav, b"RIFF").expect("write fixture");

    let out = sb.run(&["transcribe", wav.to_str().expect("utf8 path")]);
    assert!(
        !out.status.success(),
        "stub build must exit non-zero, got: {out:?}"
    );
    assert!(
        stdout_text(&out).trim().is_empty(),
        "no fake transcript may reach stdout: {}",
        stdout_text(&out)
    );
    assert!(
        stderr_text(&out).contains("--features full"),
        "error must say how to get a working build: {}",
        stderr_text(&out)
    );
    assert!(
        !sb.data_base().join("murmur").exists(),
        "no model download may start on a stub build"
    );
}

#[test]
fn models_list_respects_data_dir_override() {
    let sb = Sandbox::new("models-dir");

    // Empty override dir: nothing may be reported as downloaded even if the
    // user's real data dir has models.
    let out = sb.run(&["models", "--list"]);
    assert!(out.status.success());
    for line in stdout_text(&out).lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.contains("not downloaded"),
            "empty data dir must list everything as not downloaded: {line}"
        );
    }

    // Fake the base model's files inside the override dir; the CLI must see it.
    let model = SttModel::WhisperBaseEn;
    let models_dir = sb.data_base().join("murmur").join("models");
    std::fs::create_dir_all(&models_dir).expect("create models dir");
    std::fs::write(models_dir.join("ggml-base.en.bin"), b"stub").expect("write model stub");

    let out = sb.run(&["models", "--list"]);
    assert!(out.status.success());
    let text = stdout_text(&out);
    let line = text
        .lines()
        .find(|l| l.contains(model.name()))
        .expect("base model listed");
    assert!(
        !line.contains("not downloaded"),
        "override dir contents must be reported as downloaded: {line}"
    );
}

#[test]
fn redirected_logs_contain_no_ansi_escapes() {
    let sb = Sandbox::new("no-ansi");
    // Piped stderr is not a terminal, so log lines must be plain text.
    let out = sb.run(&["config", "--hotkey", "ctrl+alt+m"]);
    assert!(out.status.success());
    assert!(
        !out.stderr.contains(&0x1b),
        "captured logs must not contain ANSI escapes: {:?}",
        stderr_text(&out)
    );
}
