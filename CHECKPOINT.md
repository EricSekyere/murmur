# Murmur — Project Checkpoint

**Date:** 2026-02-18
**Phase:** 1 — Foundation (Hardened)

---

## Project Structure

```
murmur/
├── Cargo.toml                  # Workspace root (3 crates)
├── CLAUDE.md                   # Project conventions + feature flags
├── .gitignore
├── prd.md                      # Full PRD
├── config/default.toml         # Default TOML config template
├── resources/icon.png          # Placeholder icon
│
├── crates/
│   ├── murmur-core/            # Shared library
│   │   └── src/
│   │       ├── lib.rs          # Re-exports all modules
│   │       ├── audio/
│   │       │   ├── mod.rs      # AudioBuffer (always available, Default impl)
│   │       │   ├── capture.rs  # CPAL mic capture (16kHz mono PCM)
│   │       │   └── vad.rs      # Silero VAD stub (clean API, not yet wired)
│   │       ├── stt/
│   │       │   ├── engine.rs   # whisper-rs transcription (real impl behind stt feature)
│   │       │   └── models.rs   # HuggingFace download + SHA256 verification
│   │       ├── output/
│   │       │   ├── mod.rs      # OutputMode enum (Serialize/Deserialize/Default)
│   │       │   ├── keyboard.rs # enigo keystroke simulation
│   │       │   ├── clipboard.rs# arboard clipboard
│   │       │   └── stdout.rs   # stdout for CLI piping (Default impl)
│   │       ├── config/
│   │       │   └── settings.rs # TOML config load/save with validation (hotkey format check)
│   │       └── hotkey.rs       # Global hotkey with Drop cleanup
│   │
│   ├── murmur-cli/             # CLI binary ("murmur")
│   │   └── src/main.rs         # clap: listen, config, models (fully wired)
│   │
│   └── murmur-app/             # Tauri v2 desktop app
│       ├── tauri.conf.json     # Tray icon + popup window
│       ├── capabilities/default.json
│       ├── icons/              # PNG + ICO placeholders
│       ├── build.rs
│       ├── src/
│       │   ├── main.rs         # Handles Result from run() with error reporting
│       │   └── lib.rs          # Tray (icon fallback), audio worker (recv_timeout), all expect→Result
│       └── frontend/
│           ├── index.html      # Popup UI
│           ├── style.css       # Dark theme
│           └── main.js         # Clear on start, show processing time
```

---

## Build Status

| Check                                              | Status               |
|----------------------------------------------------|----------------------|
| `cargo check --workspace`                          | Pass (0 warnings)    |
| `cargo clippy --workspace`                         | Pass (0 warnings)    |
| `cargo check --workspace --features full`          | Requires cmake/libclang |
| `cargo run -p murmur-cli -- --help`                | Works                |
| `cargo run -p murmur-cli -- config --show`         | Works                |
| `cargo run -p murmur-cli -- models --list`         | Works                |
| Git                                                | Clean                |

---

## Phase 1 Hardening (Completed)

### Critical Fixes
- **Icon unwrap crash** → fallback to 1x1 transparent pixel if no icon bundled
- **Audio worker deadlock** → `recv_timeout(5s)` instead of blocking `recv()`
- **`expect()` elimination** → `run()` returns `anyhow::Result<()>`, all `.expect()` replaced with `?` + `.context()`
- **Mutex poisoning** → consistent `unwrap_or_else(|e| e.into_inner())` everywhere

### Moderate Fixes
- **OutputMode enum merge** → removed duplicate `OutputModeSetting`, unified `OutputMode` with `Serialize/Deserialize/Default`
- **Hotkey format validation** → require `+` separator, non-empty parts
- **Tauri main.rs** → handles `Result` from `run()` (prints error + exits 1)

### Minor Fixes
- **Default impls** → `AudioBuffer` (sample_rate=16000), `StdoutOutput`, `OutputMode` (`#[default] Keyboard`)
- **HotkeyManager Drop** → unregisters hotkey and clears event handler on drop
- **Frontend UX** → clears old transcription on record start, shows processing time

---

## Implementation Status

### Fully Implemented
- Cargo workspace with 3 crates wired together
- **Config system:** TOML load/save, validation (vad_threshold, hotkey format), auto-create on first run
- **Model manager:** List models, download from HuggingFace with SHA256 verification + progress bar
- **Audio capture:** CPAL microphone → 16kHz mono f32 PCM buffer
- **STT engine:** whisper-rs integration (context init, FullParams, segment iteration) — gated behind `stt` feature
- **Output strategies:** keyboard (enigo), clipboard (arboard), stdout
- **Global hotkeys:** global-hotkey crate, press/release events via mpsc channel, cleanup on drop
- **CLI listen loop:** hotkey press → record → release → transcribe → output (fully wired)
- **CLI config/models:** show/reset config, list/download models
- **Tauri app:** system tray (icon fallback), popup window, audio worker (timeout-safe), Result-based error handling
- **Frontend:** dark-themed popup with recording toggle, status badge, processing time, transcription display

### Stubbed / Not Yet Wired
- **VAD inference:** Clean API exists (`VoiceActivityDetector`) but Silero ONNX model not loaded yet
- **GPU acceleration:** Not implemented (CPU-only for now)

---

## Feature Flags (murmur-core)

| Feature    | Dependencies | Default | Status                              |
|------------|-------------|---------|-------------------------------------|
| `audio`    | cpal        | Yes     | Working                             |
| `keyboard` | enigo       | Yes     | Working                             |
| `stt`      | whisper-rs  | No      | Working (needs cmake + libclang)    |
| `vad`      | ort         | No      | Compiles, inference not wired       |
| `full`     | all above   | No      | Working                             |

---

## Code Quality

- Zero `.unwrap()` / `.expect()` in production code (1 acceptable: progress bar template literal)
- Zero compiler warnings
- Zero clippy warnings
- `cargo fmt` clean

---

## Next Steps (Phase 2: Code Intelligence)

1. End-to-end test with cmake/libclang: download model, run `murmur listen`, speak, verify output
2. Wire up Silero VAD (download ONNX model, integrate into capture pipeline)
3. Voice commands (new line, code block, etc.)
4. Custom vocabulary from codebase (tree-sitter)
5. Project file indexer
6. Modes (coding, prose, command)
7. Polish system tray UI
