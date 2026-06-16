# Murmur

Fast, private, on-device dictation for Windows. Press a hotkey, speak, and your words appear in whatever app has focus — fully offline.

## Features

- **Local-first** — speech recognition runs on your machine. No cloud, no telemetry.
- **Fast** — Whisper (CUDA-accelerated on NVIDIA GPUs) or Parakeet (DirectML) transcribe each phrase in well under a second.
- **Live preview** — watch your words appear as you speak, both in the dashboard and as a caption under the floating pill, before each phrase is final.
- **Types anywhere** — text is delivered to the focused window via paste or keystroke simulation, with fallbacks for terminals and elevated windows.
- **Voice editing commands** — say "new line", "new paragraph", "scratch that", "select all", "copy that", "cut", "paste", "undo", "redo", "press tab", or "press escape" as a whole phrase.
- **Text snippets** — define `trigger = expansion` pairs; say the trigger to type the expansion (emails, sign-offs, boilerplate).
- **Multilingual and translate** — transcribe dozens of languages, or translate your speech straight to English, with the multilingual model.
- **Noise-robust** — Silero VAD plus decoder-confidence gating keep sighs, breaths, and background noise from becoming phantom words.
- **Searchable history** — every delivered phrase is saved locally, tagged with the app it landed in, and filterable from the dashboard.
- **Per-app profiles** — automatically switch output mode and developer mode based on the focused application.
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
| Live preview | Settings → Live Preview (interim text as you speak; off for lowest latency) |
| Language / translate | Settings → Speech Language and Translate to English (needs the multilingual model) |
| Text snippets | Settings → Text Snippets (`trigger = expansion`, one per line) |
| App profiles | Settings → App Profiles (`app = options`, e.g. `code = dev`) |
| Phrase splitting | Settings → Phrase Pause (silence duration that ends a phrase) |
| Filtering | Settings → Transcription Profile (Relaxed / Strict) |

Each phrase is transcribed when you pause and typed into the active window; stopping flushes the final phrase.

## Building from source

Prerequisites: Rust 1.93+, CMake, LLVM/libclang, `cargo install tauri-cli --version '^2'`. Optional: CUDA Toolkit 12.x (auto-detected, enables GPU Whisper), NSIS (installer bundling).

```sh
./build.sh
```

`build.sh` handles the non-obvious parts — forcing optimized MSVC flags for whisper.cpp and wiring up CUDA — see its comments for details.

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
