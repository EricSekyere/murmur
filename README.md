# Murmur

Fast, private, on-device dictation for Windows. Press a hotkey, speak, and your words appear in whatever app has focus — fully offline.

## Features

- **Local-first** — speech recognition runs on your machine. No cloud, no telemetry.
- **Fast** — Whisper (CUDA-accelerated on NVIDIA GPUs) or Parakeet (DirectML) transcribe each phrase in well under a second.
- **Live preview** — watch your words appear as you speak, both in the dashboard and as a caption under the floating pill, before each phrase is final.
- **Types anywhere** — text is delivered to the focused window via paste or keystroke simulation, with fallbacks for terminals and elevated windows.
- **Voice editing commands** — say "new line", "new paragraph", "scratch that", "copy that", "undo", "redo", "press tab", or "press escape" as a whole phrase.
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
| Find the pill | Home view → Find pill (flashes the widget and pulls it back on-screen) |
| Phrase splitting | Settings → Phrase Pause (silence duration that ends a phrase) |
| Filtering | Settings → Transcription Profile (Relaxed / Strict) |

Each phrase is transcribed when you pause and typed into the active window; stopping flushes the final phrase.

## Editor integration (MCP)

Let Claude and Cursor read your recent dictation through Murmur's built-in MCP server. Two read-only tools become available: `get_recent_transcripts` and `search_transcripts`. Everything stays local: the editor spawns Murmur and talks to it over stdin/stdout, no network.

**Desktop app:** Settings → Connect to Cursor / Claude → Connect editors. It writes the server into every detected editor's config; restart the editor to finish.

**CLI:**

```sh
murmur mcp install                    # configure every detected client (Cursor, Claude Desktop)
murmur mcp install --client cursor    # just one
claude mcp add murmur -- murmur mcp   # Claude Code
```

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

Everything runs locally. The only network access is the one-time, checksum-verified download of model files, plus update checks against GitHub Releases. Audio and transcripts never leave your machine. See the full [Privacy Policy](PRIVACY.md).

## License & terms

[MIT](LICENSE). Use of the app is also covered by the [Terms of Use](TERMS.md).
