# Voitex — Project Checkpoint

**Date:** 2026-02-18
**Phase:** 1 — Foundation (In Progress)

---

## Project Structure

```
voitex/
├── Cargo.toml                  # Workspace root (3 crates)
├── CLAUDE.md                   # Project conventions + feature flags
├── .gitignore
├── prd.md                      # Full PRD
├── config/default.toml         # Default TOML config template
├── resources/icon.png          # Placeholder icon
│
├── crates/
│   ├── voitex-core/            # Shared library
│   │   └── src/
│   │       ├── lib.rs          # Re-exports all modules
│   │       ├── audio/
│   │       │   ├── mod.rs      # AudioBuffer (always available)
│   │       │   ├── capture.rs  # CPAL mic capture (16kHz mono PCM)
│   │       │   └── vad.rs      # Silero VAD stub (clean API, not yet wired)
│   │       ├── stt/
│   │       │   ├── engine.rs   # whisper-rs transcription (real impl behind stt feature)
│   │       │   └── models.rs   # HuggingFace download + SHA256 verification
│   │       ├── output/
│   │       │   ├── keyboard.rs # enigo keystroke simulation
│   │       │   ├── clipboard.rs# arboard clipboard
│   │       │   └── stdout.rs   # stdout for CLI piping
│   │       ├── config/
│   │       │   └── settings.rs # TOML config load/save with validation
│   │       └── hotkey.rs       # Global hotkey (real impl with global-hotkey crate)
│   │
│   ├── voitex-cli/             # CLI binary ("voitex")
│   │   └── src/main.rs         # clap: listen, config, models (fully wired)
│   │
│   └── voitex-app/             # Tauri v2 desktop app
│       ├── tauri.conf.json     # Tray icon + popup window
│       ├── capabilities/default.json
│       ├── icons/              # PNG + ICO placeholders
│       ├── build.rs
│       ├── src/
│       │   ├── main.rs         # Windows subsystem entry
│       │   └── lib.rs          # Tray, audio worker thread, 4 Tauri commands (wired)
│       └── frontend/
│           ├── index.html      # Popup UI
│           ├── style.css       # Dark theme
│           └── main.js         # Tauri invoke + event listener
```

---

## Build Status

| Check                                              | Status               |
|----------------------------------------------------|----------------------|
| `cargo check --workspace`                          | Pass (0 warnings)    |
| `cargo check --workspace --features full`          | Pass (0 warnings)    |
| `cargo run -p voitex-cli -- --help`                | Works                |
| `cargo check -p voitex-app --features full`        | Pass (0 warnings)    |
| Git                                                | Initial commit done  |

---

## Implementation Status

### Fully Implemented
- Cargo workspace with 3 crates wired together
- **Config system:** TOML load/save, validation (vad_threshold range, empty hotkey), auto-create on first run
- **Model manager:** List models, download from HuggingFace with SHA256 verification + progress bar
- **Audio capture:** CPAL microphone → 16kHz mono f32 PCM buffer
- **STT engine:** whisper-rs integration (context init, FullParams, segment iteration) — gated behind `stt` feature
- **Output strategies:** keyboard (enigo), clipboard (arboard), stdout
- **Global hotkeys:** global-hotkey crate, press/release events via mpsc channel
- **CLI listen loop:** hotkey press → record → release → transcribe → output (fully wired)
- **CLI config/models:** show/reset config, list/download models
- **Tauri app:** system tray, popup window, audio worker thread (Send-safe), 4 commands wired to voitex-core
- **Frontend:** dark-themed popup with recording toggle, status badge, transcription display, event listener

### Stubbed / Not Yet Wired
- **VAD inference:** Clean API exists (`VoiceActivityDetector`) but Silero ONNX model not loaded yet
- **GPU acceleration:** Not implemented (CPU-only for now)

---

## Feature Flags (voitex-core)

| Feature    | Dependencies | Default | Status                              |
|------------|-------------|---------|-------------------------------------|
| `audio`    | cpal        | Yes     | Working                             |
| `keyboard` | enigo       | Yes     | Working                             |
| `stt`      | whisper-rs  | No      | Working (needs cmake + libclang)    |
| `vad`      | ort         | No      | Compiles, inference not wired       |
| `full`     | all above   | No      | Working                             |

---

## Next Steps (Phase 1 Completion)

1. End-to-end test: download a model, run `voitex listen`, speak, verify output
2. Test Tauri app launch (`cargo run -p voitex-app --features full`)
3. Wire up Silero VAD (download ONNX model, integrate into capture pipeline)
4. Git commit current progress

## Future (Phase 2: Code Intelligence)

- Voice commands (new line, code block, etc.)
- Custom vocabulary from codebase (tree-sitter)
- Project file indexer
- Modes (coding, prose, command)
- Polish system tray UI
