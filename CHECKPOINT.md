# Voitex — Project Checkpoint

**Date:** 2026-02-18
**Phase:** 1 — Project Scaffolding (Complete)

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
├── models/                     # .gitignored (for downloaded whisper models)
│
├── crates/
│   ├── voitex-core/            # Shared library
│   │   └── src/
│   │       ├── lib.rs          # Re-exports all modules
│   │       ├── audio/
│   │       │   ├── capture.rs  # CPAL mic capture (16kHz mono PCM)
│   │       │   └── vad.rs      # Silero VAD stub
│   │       ├── stt/
│   │       │   ├── engine.rs   # Whisper transcription stub
│   │       │   └── models.rs   # Model manager (download, list, paths)
│   │       ├── output/
│   │       │   ├── keyboard.rs # enigo keystroke simulation
│   │       │   ├── clipboard.rs# arboard clipboard
│   │       │   └── stdout.rs   # stdout for CLI piping
│   │       ├── config/
│   │       │   └── settings.rs # TOML config load/save with defaults
│   │       └── hotkey.rs       # Global hotkey stub
│   │
│   ├── voitex-cli/             # CLI binary ("voitex")
│   │   └── src/main.rs         # clap: listen, config, models
│   │
│   └── voitex-app/             # Tauri v2 desktop app
│       ├── tauri.conf.json     # Tray icon + popup window
│       ├── capabilities/default.json
│       ├── icons/              # PNG + ICO placeholders
│       ├── build.rs
│       ├── src/
│       │   ├── main.rs         # Windows subsystem entry
│       │   └── lib.rs          # Tray setup, 4 Tauri commands
│       └── frontend/
│           ├── index.html      # Popup UI
│           ├── style.css       # Dark theme
│           └── main.js         # Tauri invoke calls
```

---

## Build Status

| Check                              | Status                                  |
|------------------------------------|-----------------------------------------|
| `cargo check --workspace`          | Pass (0 warnings)                       |
| `cargo run -p voitex-cli -- --help`| Works — shows listen/config/models      |
| `cargo check -p voitex-app`        | Pass (0 warnings)                       |
| Git repo initialized               | Yes (no commits yet)                    |

---

## Dependency Versions (Pinned)

| Crate        | Version       | Notes                              |
|--------------|---------------|------------------------------------|
| whisper-rs   | 0.15          | Requires cmake + libclang          |
| ort          | 2.0.0-rc.11   | Pre-release, must pin exact version|
| enigo        | 0.6           | Keystroke simulation               |
| cpal         | 0.15          | Audio capture                      |
| arboard      | 3             | Clipboard                          |
| global-hotkey| 0.6           | System-wide hotkeys                |
| tauri        | 2 (2.10.2)    | Desktop framework                  |
| tauri-build  | 2 (2.5.5)     | Build-time code generation         |
| clap         | 4             | CLI parsing                        |
| tokio        | 1             | Async runtime                      |
| serde        | 1             | Serialization                      |
| toml         | 0.8           | Config format                      |
| tracing      | 0.1           | Structured logging                 |
| reqwest      | 0.12          | HTTP (rustls, no openssl)          |
| dirs         | 6             | OS-standard directories            |

---

## Feature Flags (voitex-core)

| Feature    | Dependencies | Default | Status                              |
|------------|-------------|---------|-------------------------------------|
| `audio`    | cpal        | Yes     | Compiles, working stub              |
| `keyboard` | enigo       | Yes     | Compiles, working stub              |
| `stt`      | whisper-rs  | No      | Blocked — needs cmake + libclang    |
| `vad`      | ort         | No      | Blocked — needs ONNX Runtime        |
| `full`     | all above   | No      | Blocked — needs native build tools  |

Default features (`audio` + `keyboard`) compile without native C/C++ build tools.
The `stt` and `vad` features require cmake and LLVM/libclang on PATH.

---

## Implementation Status

### Implemented
- Cargo workspace with 3 crates wired together
- voitex-core module structure (audio, stt, output, config, hotkey)
- Config system: TOML load/save with sensible defaults
- Model manager: list models, check downloaded, path resolution
- Audio capture: CPAL microphone → PCM buffer (16kHz mono f32)
- Output strategies: keyboard (enigo), clipboard (arboard), stdout
- CLI: `voitex listen`, `voitex config`, `voitex models` with clap
- Tauri v2 app: system tray icon, popup window, 4 Tauri commands
- Frontend: dark-themed popup UI with recording toggle, status badge, audio level bar
- Tauri capabilities/permissions for tray, window, events

### Stubbed (TODO)
- VAD inference (Silero ONNX model loading + processing)
- Whisper transcription (whisper-rs context init + inference)
- Model download from HuggingFace (reqwest + SHA256 checksum)
- Global hotkey registration (global-hotkey crate wiring)
- Listen loop (audio capture → VAD → STT → output pipeline)
- Frontend ↔ backend event communication for real-time status

---

## Known Issues / Blockers

1. **cmake + libclang not on PATH** — whisper-rs-sys build fails without these.
   Install LLVM and cmake, then set `LIBCLANG_PATH` to enable `stt` feature.
2. **No git commits yet** — repo initialized but nothing committed.
3. **Placeholder icons** — 32x32 green circle PNG, needs real microphone icon.
4. **ort v2 is pre-release** — pinned to `2.0.0-rc.11`, may need updating when stable releases.

---

## Next Steps

1. Install cmake + LLVM/libclang on Windows to unblock `stt` and `vad` features
2. Enable `full` features and verify `cargo check --workspace --features full` passes
3. Wire up the listen loop: audio capture → VAD → STT → output
4. Implement model download from HuggingFace
5. Register global hotkeys (push-to-talk)
6. Initial git commit
7. Test `cargo run -p voitex-app` launches tray icon
