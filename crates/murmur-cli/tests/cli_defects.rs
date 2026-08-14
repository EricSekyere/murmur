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
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_murmur"));
        cmd.args(args)
            .env("MURMUR_CONFIG_DIR", self.config_base())
            .env("MURMUR_DATA_DIR", self.data_base())
            .env_remove("MURMUR_NO_DOWNLOAD");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().expect("spawn murmur binary")
    }

    fn config_file(&self) -> PathBuf {
        self.config_base().join("murmur").join("config.toml")
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
#[test]
fn stub_build_refuses_to_transcribe() {
    // Ask the engine rather than this crate's cfg: `cargo test --workspace`
    // unifies features, so murmur-app pulls murmur-core/full and the CLI gets
    // a working backend while murmur-cli's own `full` stays off.
    let model = murmur_core::config::Settings::default().model;
    if murmur_core::stt::engine::SttEngine::backend_available(model.backend()) {
        return;
    }

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

/// `transcribe` is a read path: it must never create a config file, even on
/// a first run with no config at all.
#[test]
fn transcribe_does_not_create_a_config_file() {
    let sb = Sandbox::new("transcribe-no-config");
    let wav = sb.data_base().join("clip.wav");
    std::fs::write(&wav, b"RIFF").expect("write fixture");

    // --no-download keeps the full-featured build off the network; the stub
    // build fails earlier at the backend check. Both fail after config load.
    let out = sb.run(&[
        "transcribe",
        wav.to_str().expect("utf8 path"),
        "--no-download",
    ]);
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    assert!(
        !sb.config_file().exists(),
        "transcribe must not create a config file"
    );
}

/// `config --show` on a corrupt file must report the problem and leave the
/// file byte-for-byte intact: no rewrite, no .bak, non-zero exit.
#[test]
fn config_show_on_corrupt_file_neither_rewrites_nor_backs_up() {
    let sb = Sandbox::new("show-corrupt");
    let file = sb.config_file();
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, "not [valid toml").expect("write corrupt config");

    let out = sb.run(&["config", "--show"]);
    assert!(
        !out.status.success(),
        "showing a corrupt config must exit non-zero: {out:?}"
    );
    assert!(
        stderr_text(&out).contains("unreadable or invalid"),
        "error must name the broken file: {}",
        stderr_text(&out)
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "not [valid toml",
        "the corrupt file must be left untouched"
    );
    assert!(
        !file.with_extension("toml.bak").exists(),
        "no backup may be created by a read command"
    );
}

/// `config --show` with no file prints defaults, says so, and creates nothing.
#[test]
fn config_show_without_a_file_prints_defaults_and_creates_nothing() {
    let sb = Sandbox::new("show-missing");
    let out = sb.run(&["config", "--show"]);
    assert!(
        out.status.success(),
        "missing config is not an error: {out:?}"
    );
    assert!(
        stdout_text(&out).contains("hotkey"),
        "defaults must be printed: {}",
        stdout_text(&out)
    );
    assert!(
        stderr_text(&out).contains("defaults"),
        "the user must be told these are defaults: {}",
        stderr_text(&out)
    );
    assert!(!sb.config_file().exists(), "no config file may be created");
}

/// With downloads refused, a missing model is a clean, actionable failure:
/// non-zero exit, no fetch, and a message naming the model, size, and command.
#[test]
fn no_download_refuses_model_fetch_with_actionable_error() {
    let sb = Sandbox::new("no-download");
    let wav = sb.data_base().join("clip.wav");
    std::fs::write(&wav, b"RIFF").expect("write fixture");
    let wav = wav.to_str().expect("utf8 path");

    for (args, env) in [
        (&["transcribe", wav, "--no-download"][..], &[][..]),
        (&["transcribe", wav][..], &[("MURMUR_NO_DOWNLOAD", "1")][..]),
    ] {
        let out = sb.run_with_env(args, env);
        assert!(
            !out.status.success(),
            "{args:?} must exit non-zero: {out:?}"
        );
        assert!(
            !sb.data_base().join("murmur").exists(),
            "no download may start for {args:?}"
        );

        // The refusal message only fires when a backend exists; a stub build
        // correctly fails earlier with its own rebuild instruction.
        let model = murmur_core::config::Settings::default().model;
        if murmur_core::stt::engine::SttEngine::backend_available(model.backend()) {
            let err = stderr_text(&out);
            assert!(err.contains(model.id()), "must name the model: {err}");
            assert!(
                err.contains(&format!("{} MB", model.size_mb())),
                "must state the size: {err}"
            );
            assert!(
                err.contains(&format!("murmur models --download {}", model.id())),
                "must give the exact fetch command: {err}"
            );
        }
    }
}

/// Purely informational invocations must not touch the filesystem: the
/// voitex migration only runs for commands that actually load settings.
#[test]
fn help_and_version_do_not_run_the_voitex_migration() {
    let sb = Sandbox::new("help-no-migrate");
    let voitex = sb.config_base().join("voitex");
    std::fs::create_dir_all(&voitex).expect("mkdir");
    std::fs::write(voitex.join("config.toml"), "save_history = true").expect("write");

    for args in [&["--help"][..], &["--version"][..]] {
        let out = sb.run(args);
        assert!(out.status.success(), "{args:?} must succeed");
        assert!(
            voitex.exists() && !sb.config_base().join("murmur").exists(),
            "{args:?} must not migrate or create config dirs"
        );
    }

    // A command that loads settings still migrates the legacy directory.
    let out = sb.run(&["config", "--show"]);
    assert!(out.status.success(), "show after migration: {out:?}");
    assert!(!voitex.exists(), "config --show must run the migration");
    assert!(
        stdout_text(&out).contains("save_history = true"),
        "migrated settings must be shown: {}",
        stdout_text(&out)
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
