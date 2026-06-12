# Murmur

Fast, private, on-device dictation for Windows. Press a hotkey, speak, and your words appear in whatever app has focus — like macOS dictation, but for Windows and fully offline.

## Features

- **Local-first** — all speech recognition runs on your machine. No cloud, no network calls, no telemetry.
- **Fast** — GPU-accelerated Whisper (CUDA) or Parakeet models transcribe phrases in well under a second; text lands moments after you pause.
- **Types anywhere** — output goes straight to the focused window via paste or keystroke simulation, with smart fallbacks for terminals and elevated windows.
- **Smart voice detection** — Silero VAD plus decoder-confidence gating means sighs, breaths, and background noise don't turn into phantom words.
- **Floating pill widget** — an always-on-top mini control with live waveform, recording timer, and one-click toggle.
- **Dashboard** — transcription history, session analytics (WPM, words/day), live diagnostics, and full settings.
- **Developer mode** — optional post-processing for dictating code: tech-term correction, spoken symbols (`fat arrow` → `=>`), filler removal, and casing commands (camel, snake, pascal, kebab).

## Install

Download the latest installer from [Releases](https://github.com/EricSekyere/murmur/releases) and run it. Models download automatically on first launch (~500 MB for the default model).

Requirements: Windows 10/11 x64, a CPU with AVX2 (any Intel/AMD CPU from ~2013 onward). An NVIDIA GPU is optional — CUDA builds are currently local-build only.

## Usage

| Action | How |
|---|---|
| Start/stop dictation | `Ctrl+Q` (configurable), double-tap **right Ctrl**, or click the pill |
| Choose a model | Settings → STT Model (smaller = faster, larger = more accurate) |
| Tune phrase splitting | Settings → Phrase Pause (how long a silence ends a phrase) |
| Filter aggressiveness | Settings → Transcription Profile (Relaxed / Strict) |

Speak naturally; each phrase is transcribed when you pause and typed into the active window. Stop dictating and the final phrase flushes automatically.

## Building from source

Prerequisites:

- Rust 1.93+ ([rustup.rs](https://rustup.rs))
- CMake and LLVM/libclang (for whisper.cpp)
- `cargo install tauri-cli --version '^2'`
- Optional: CUDA Toolkit 12.x for GPU-accelerated Whisper (auto-detected)
- Optional: NSIS for installer bundling

```sh
./build.sh
```

The script handles the important build details — notably forcing optimized MSVC flags for whisper.cpp (see the comments in `build.sh` for why) and enabling CUDA when the toolkit is present.

```
cargo check --workspace                  # check default features
cargo test -p murmur-core -p murmur-app # unit tests
cargo run -p murmur-app                  # run the desktop app (debug)
```

## Architecture

Cargo workspace with three crates:

- `murmur-core` — audio capture (CPAL), voice activity detection (Silero via ONNX Runtime), STT engines (whisper.cpp, Parakeet), output strategies, config.
- `murmur-app` — Tauri v2 desktop app: system tray, dashboard window, floating pill widget, dictation session orchestration.
- `murmur-cli` — command-line interface for one-shot transcription.

Audio capture, phrase detection, and inference each run on dedicated threads connected by channels; the realtime audio callback never blocks on anything.

## Privacy

Everything runs locally. The only network access is downloading model files (from Hugging Face/GitHub) on first use, verified by checksum. Your audio and transcripts never leave your machine.

## License

[MIT](LICENSE)
