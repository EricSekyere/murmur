# Murmur

Fast, private, on-device dictation for Windows. Press a hotkey, speak, and your words appear in whatever app has focus — fully offline.

## Features

- **Local-first** — speech recognition runs on your machine. No cloud, no telemetry.
- **Fast** — Whisper (CUDA-accelerated on NVIDIA GPUs) or Parakeet (DirectML) transcribe each phrase in well under a second.
- **Types anywhere** — text is delivered to the focused window via paste or keystroke simulation, with fallbacks for terminals and elevated windows.
- **Noise-robust** — Silero VAD plus decoder-confidence gating keep sighs, breaths, and background noise from becoming phantom words.
- **Floating pill widget** — always-on-top control with live waveform and recording timer.
- **Dashboard** — history, session analytics, diagnostics, and settings.
- **Developer mode** — dictate code: tech-term correction, spoken symbols (`fat arrow` → `=>`), filler removal, casing commands (camel, snake, pascal, kebab).

## Install

Download the installer from [Releases](https://github.com/EricSekyere/murmur/releases) and run it. The default model (~490 MB) downloads on first launch.

Requires Windows 10/11 x64 with an AVX2-capable CPU (any Intel/AMD CPU from ~2013 onward). NVIDIA GPU optional.

## Usage

| Action | How |
|---|---|
| Start/stop dictation | Double-tap **right Ctrl**, press `Ctrl+Q`, or click the pill |
| Choose a model | Settings → STT Model (smaller = faster, larger = more accurate) |
| Phrase splitting | Settings → Phrase Pause (silence duration that ends a phrase) |
| Filtering | Settings → Transcription Profile (Relaxed / Strict) |

Each phrase is transcribed when you pause and typed into the active window; stopping flushes the final phrase.

## Building from source

### Windows

Prerequisites: Rust 1.93+, CMake, LLVM/libclang, `cargo install tauri-cli --version '^2'`. Optional: CUDA Toolkit 12.x (auto-detected, enables GPU Whisper), NSIS (installer bundling).

```sh
./build.sh
```

`build.sh` handles the non-obvious parts. It forces optimized MSVC flags for whisper.cpp and wires up CUDA. See its comments for details.

### macOS

Run this on a Mac (the app cannot cross-compile from another OS). Prerequisites: Xcode command line tools (`xcode-select --install`), CMake (`brew install cmake`), Rust (rustup.rs), and `cargo install tauri-cli --version '^2'`.

```sh
./build-macos.sh
```

This builds an unsigned `Murmur.app` and `.dmg` for the host architecture. Whisper runs on Metal and Accelerate; the Parakeet backend is Windows-only and is excluded. On first launch, right-click the app and choose Open to get past Gatekeeper, then grant Microphone, Accessibility, and Input Monitoring under System Settings, Privacy and Security.

### Common

```sh
cargo check --workspace                   # default features
cargo test -p murmur-core -p murmur-app   # unit tests
cargo run -p murmur-app                   # desktop app (debug)
```

## Architecture

Cargo workspace, three crates:

- `murmur-core` — audio capture (CPAL), Silero VAD (ONNX Runtime), STT engines (whisper.cpp, Parakeet), output strategies, config.
- `murmur-app` — Tauri v2 desktop app: tray, dashboard, pill widget, session orchestration.
- `murmur-cli` — command-line transcription.

Capture, phrase detection, and inference run on dedicated threads connected by channels; the realtime audio callback never blocks.

## Privacy

Everything runs locally. The only network access is the one-time, checksum-verified download of model files. Audio and transcripts never leave your machine.

## License

[MIT](LICENSE)
