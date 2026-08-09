# Contributing to Murmur

Thanks for your interest. This document covers what you need to build, test, and land a change.

## Prerequisites

- **Rust 1.93+** (edition 2024)
- **CMake:** whisper.cpp is built from source by `whisper-rs`
- **LLVM/libclang:** required by bindgen. On Windows, set `LIBCLANG_PATH` (typically `C:\Program Files\LLVM\bin`)
- **Tauri CLI:** `cargo install tauri-cli --version '^2'` (desktop app only)

Platform extras:

- **Windows:** MSVC toolchain (Visual Studio Build Tools). Optional: CUDA Toolkit 12.x for GPU Whisper (auto-detected by `build.sh`), NSIS for installer bundling.
- **Linux:** WebView2 equivalent is `webkit2gtk`; see the apt packages installed in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the exact list (ALSA headers, GTK, etc.).
- **macOS:** Xcode command-line tools.

## Building

```sh
cargo check --workspace                  # fast check, default features
cargo run -p murmur-app                  # desktop app (debug)
cargo run -p murmur-cli -- --help        # CLI
./build.sh                               # release build; handles MSVC flags + CUDA detection
```

### Feature flags (murmur-core)

Heavy native dependencies are optional so a default `cargo check` stays light:

| Feature | Dependency | Default | Notes |
|---|---|---|---|
| `audio` | cpal | yes | Microphone capture |
| `keyboard` | enigo | yes | Keystroke simulation |
| `stt` | whisper-rs | no | Needs cmake + libclang |
| `vad` | ort | no | ONNX Runtime for Silero VAD |
| `indexer` | ignore, regex | no | Codebase-derived vocabulary |
| `treesitter` | tree-sitter | no | AST-accurate indexing (needs a C compiler) |
| `full` | all above | no | Everything; needs the native tools |

`cargo check --workspace --features full` exercises the STT/VAD path. CI builds `full`, not `--all-features` (GPU SDK features like `cuda`/`vulkan` need their toolkits installed).

## Testing

```sh
cargo test --workspace                   # default features
cargo test -p murmur-core --features full
```

Unit tests live next to the code behind `#[cfg(test)]`; integration tests in each crate's `tests/`. Don't sleep on real time in tests; use `tempfile` for filesystem fixtures.

## Before you open a PR

- `cargo fmt --all`
- `cargo clippy --workspace -- -D warnings` (CI denies warnings)
- `cargo test --workspace`
- CI also runs `cargo audit` and a `cargo deny` license/ban gate; all dependencies must be MIT or Apache 2.0 licensed (no GPL).

## Conventions

The full engineering conventions live in [CLAUDE.md](CLAUDE.md) and [AGENTS.md](AGENTS.md); the short version:

- **Errors:** `thiserror` enums in `murmur-core`, `anyhow` with `.context()` in binaries. No `unwrap()`/`expect()`/`panic!` in production code.
- **Logging:** the `tracing` crate only, never `println!` in library code. Never log transcript contents above `trace` level; this is a privacy-sensitive app.
- **Async:** tokio. CPU-heavy work (STT inference) runs on dedicated worker threads, never on the async reactor.
- **Config:** TOML with `#[serde(default)]` on every field (old and new configs must load across versions), atomic tempfile-and-rename writes, and recovery to defaults on a corrupt file.
- **Downloads:** every model/runtime download is verified against a pinned SHA-256 before use.
- **Commits:** [Conventional Commits](https://www.conventionalcommits.org/): `feat(core): ...`, `fix(app): ...`. Scopes: `core`, `cli`, `app`, `audio`, `stt`, `config`. Releases are versioned automatically from commit subjects on the release branch, and a `Whats-New: Title | body` trailer on a user-visible commit becomes an in-app What's New bullet.

## Project layout

```
crates/murmur-core    shared library (audio, VAD, STT, meetings, LLM, output, config)
crates/murmur-app     Tauri v2 desktop app (vanilla HTML/CSS/JS frontend)
crates/murmur-cli     CLI binary
crates/murmur-mcp     stdio MCP server + editor client registration
editors/vscode        reference VS Code extension over the local WebSocket API
docs/                 deeper docs (local-api.md)
```

## Questions

Open a [discussion or issue](https://github.com/EricSekyere/murmur/issues). For security reports, see [SECURITY.md](SECURITY.md). Please do not open public issues for vulnerabilities.
