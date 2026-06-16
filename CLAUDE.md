# Murmur

Voice-to-text desktop tool for developers. See `prd.md` for full requirements and current implementation status.

## Project Structure

- **Cargo workspace** with three crates:
  - `crates/murmur-core` — shared library (audio, STT, output, config, hotkeys)
  - `crates/murmur-cli` — CLI binary using clap
  - `crates/murmur-app` — Tauri v2 desktop app (system tray + popup)

## Tech Stack

- Rust 1.93, edition 2024
- Tauri v2 for desktop app (vanilla HTML/CSS/JS frontend)
- whisper-rs (whisper.cpp) for STT
- CPAL for audio capture
- ort (ONNX Runtime) for Silero VAD
- enigo for keystroke simulation, arboard for clipboard
- global-hotkey for system-wide hotkeys
- Targets: macOS (Apple Silicon + Intel) and Windows (x64)

## Feature Flags (murmur-core)

| Feature    | Dep        | Default | Notes                                |
|------------|------------|---------|--------------------------------------|
| `audio`    | cpal       | Yes     | Microphone capture                   |
| `keyboard` | enigo      | Yes     | Keystroke simulation                 |
| `stt`      | whisper-rs | No      | Requires cmake + libclang            |
| `vad`      | ort        | No      | ONNX Runtime for Silero VAD          |
| `full`     | all above  | No      | Enables everything; needs native tools|

## Build Prerequisites

- Rust 1.93+
- For `stt` feature: cmake + LLVM/libclang (set `LIBCLANG_PATH` on Windows)
- For Tauri app: WebView2 (Windows), webkit2gtk (Linux)
- Use `uv` for any Python tooling (not bare python/python3)

## Conventions

- **Error handling:** `anyhow::Result` in binaries, `thiserror` in murmur-core
- **Logging:** `tracing` crate only (not `println!` or `log`)
- **Async:** tokio (multi-threaded)
- **Config:** TOML via serde, stored in `dirs::config_dir()/murmur/`
- **Licensing:** All deps must be MIT or Apache 2.0 (no GPL)
- **Formatting:** `cargo fmt` before committing, `cargo clippy -- -D warnings` in CI
- **No `unwrap()`/`expect()` in production code** — propagate with `?` and `.context()`

## Architecture Principles

- **Single Responsibility:** Each module (`audio::capture`, `stt::engine`, etc.) has one purpose.
- **Open/Closed:** Use traits (e.g., `OutputStrategy`) to extend behaviour without modifying existing code.
- **Dependency Inversion:** High-level modules depend on abstractions (traits), not concrete types. Inject via constructors.
- **DRY:** Share common logic in `murmur-core`. Prefer functions/generics over macros.
- **KISS:** Prefer straightforward solutions. Avoid premature abstraction — wait until duplication actually appears.
- **YAGNI:** Don't build for hypothetical futures. Feature flags keep heavy deps optional until needed.

## Coding Standards

### Error Handling
- **murmur-core:** Define domain error enums with `thiserror`.
- **murmur-cli / murmur-app:** Use `anyhow::Result`. Add context with `.context()` / `.with_context()`.
- **No `unwrap()`/`expect()` in production code** (tests and truly unrecoverable panics excepted).

### Logging
- `tracing` crate only. Log levels: `error` (unrecoverable), `warn` (recoverable), `info` (lifecycle events), `debug` (diagnostics), `trace` (audio buffers, etc.).
- Use spans to correlate operations (e.g., a transcription request).

### Async
- Runtime: tokio (multi-threaded). Use `tokio::spawn` for CPU-heavy work (STT inference) to avoid blocking the reactor.

### Code Quality
- `cargo fmt` before committing. `cargo clippy -- -D warnings` in CI.
- **No god files or god functions.** Split files over ~500 lines and functions over ~50 lines. Each file/function should have a single clear responsibility.
- Avoid: deep nesting, magic numbers, unnecessary clones, ignored `Result`s.
- Prefer early returns, combinators (`map`, `and_then`), and constants with meaningful names.

### File Operations
- Atomic config writes: write to tempfile, then rename.
- Validate config on load. Verify model SHA256 checksums after download.

## UI Guidelines (Tauri App)

- Dark theme by default. CSS variables for colours, 8px spacing grid.
- Vanilla JS + HTML/CSS — no framework. Communicate with backend via Tauri `invoke`/`listen`.
- Semantic HTML, keyboard navigable, ARIA labels on icon buttons.
- Debounce rapid events (audio levels). Use `requestAnimationFrame` for animations.

## Testing

- **Unit tests:** Same file, `#[cfg(test)]`. Mock external deps with `mockall`.
- **Integration tests:** `tests/` directory at crate level.
- **Benchmarks:** `benches/` with `cargo bench` for STT/VAD hot paths.

## Common Commands

```
cargo check --workspace                          # check default features
cargo check --workspace --features full          # check with STT/VAD
cargo run -p murmur-cli -- --help                # CLI help
cargo run -p murmur-app                          # launch Tauri app
cargo fmt --all && cargo clippy --workspace      # lint
```

## Commit Style

Conventional Commits: `feat(core): add VAD pipeline`, `fix(cli): handle missing config`
Scopes: `core`, `cli`, `app`, `audio`, `stt`, `config`
**Never include AI attribution (Co-Authored-By, "generated by", etc.) in commits, code comments, or any files.**

## Security

- All processing local by default — no network calls, no telemetry
- Verify SHA256 checksums on model downloads
- Atomic config writes (tempfile + rename)
- `cargo audit` in CI
