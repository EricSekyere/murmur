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
- Targets: Windows (x64) and Linux (x64) ship today; macOS (Apple Silicon + Intel) is in progress

## Feature Flags (murmur-core)

| Feature    | Dep        | Default | Notes                                |
|------------|------------|---------|--------------------------------------|
| `audio`    | cpal       | Yes     | Microphone capture                   |
| `keyboard` | enigo      | Yes     | Keystroke simulation                 |
| `stt`      | whisper-rs | No      | Requires cmake + libclang            |
| `vad`      | ort        | No      | ONNX Runtime for Silero VAD          |
| `indexer`  | ignore, regex | No   | Codebase-derived vocabulary (lexical scan) |
| `treesitter` | tree-sitter + grammars | No | AST-accurate indexer extraction (needs a C compiler) |
| `full`     | all above  | No      | Enables everything; needs native tools|

## Build Prerequisites

- Rust 1.93+
- For `stt` feature: cmake + LLVM/libclang (set `LIBCLANG_PATH` on Windows)
- For Tauri app: WebView2 (Windows), webkit2gtk (Linux)
- Use `uv` for any Python tooling (not bare python/python3)

## Conventions

- **Error handling:** `anyhow::Result` in binaries, `thiserror` in murmur-core; never `unwrap()` or `expect()` in production code.
- **Logging:** `tracing` crate only (no `println!`, `eprintln!`, or the `log` crate). Prefer structured fields (`info!(?model_path, "loading model")`) over interpolated messages.
- **Async:** tokio (multi-threaded). Keep CPU-heavy work (STT inference) off the async reactor — it runs on a dedicated worker thread (see `audio_worker`); use `spawn_blocking` if you need blocking work inside an async task. Avoid `block_on` outside binary entry points.
- **Config:** TOML via serde, stored in `dirs::config_dir()/murmur/`. Every field carries `#[serde(default)]` so old and new configs load across versions — do NOT add `#[serde(deny_unknown_fields)]` (it breaks forward/backward compatibility). Validate ranges on load and recover a corrupt config to defaults rather than failing startup.
- **Licensing:** All dependencies must be MIT or Apache 2.0 (no GPL).
- **Formatting:** `cargo fmt` before committing; `cargo clippy -- -D warnings` in CI.
- **No `unwrap()`/`expect()` in production code** — propagate with `?` and `.context()`. In libraries, return `Result` rather than panicking.

## Architecture Principles

- **Single Responsibility:** Each module (`audio::capture`, `stt::engine`, etc.) has one well-defined purpose. Split files at ~500 lines, functions at ~50 lines.
- **Open/Closed:** Use traits (e.g., `OutputStrategy`) to extend behaviour without modifying existing code. Prefer `dyn Trait` over sprawling match expressions when adding variants is the common change.
- **Dependency Inversion:** High-level modules depend on abstractions (traits), not concrete types. Inject dependencies via constructors.
- **DRY:** Share common logic in `murmur-core`. Use functions, generics, and extension traits before reaching for macros. Don't copy-paste significant code.
- **KISS:** Prefer straightforward solutions. Avoid premature abstraction — wait until duplication actually appears. A simple `if`/`else` usually beats a custom framework.
- **YAGNI:** Don't build for hypothetical futures. Feature flags keep heavy deps optional until actually needed.

## Coding Standards

### Error Handling
- **murmur-core:** Define domain error enums with `thiserror`. Keep variants specific (e.g., `#[error("IO error: {0}")] Io(#[from] std::io::Error)`) rather than one catch-all.
- **murmur-cli / murmur-app:** Use `anyhow::Result`. Add context with `.context()` / `.with_context(|| …)` on fallible operations.
- **Never** ignore a `Result`. `let _ = …` only with a comment explaining why the error is safe to drop.
- **Never** call `unwrap()`, `expect()`, or `panic!` in non-test code unless the invariant is provably unreachable and a comment says why. For lock poisoning, recover with `unwrap_or_else(|e| e.into_inner())`.

### Logging
- Use `tracing` exclusively. No `println!`, `eprintln!`, or `log` macros.
- **Levels:** `error` (unrecoverable), `warn` (recoverable or unexpected), `info` (lifecycle events), `debug` (diagnostics), `trace` (audio buffers, etc.).
- **Spans:** Correlate related work with spans (e.g., a transcription request); `#[tracing::instrument]` on key entry points.
- **Fields:** Prefer structured key-value pairs (`debug!(audio_len = buffer.len(), "captured")`) over string interpolation.
- **Privacy:** Never log transcript contents above `trace`, and never log secrets. This is a local-first app — keep recognized text out of the debug log.

### Async
- Runtime: tokio (multi-threaded). CPU-bound work (STT inference) runs on a dedicated thread, not the reactor; use `spawn_blocking` for blocking work that must live inside an async task.
- Anything that might be spawned must be `Send`; avoid holding `!Send` types across `.await` points.
- Long-running tasks shut down cleanly via the existing stop signal (an `AtomicBool` flag plus an mpsc command channel — see `audio_worker`), not by detaching and leaking.

### Code Quality
- `cargo fmt` and `cargo clippy -- -D warnings` are mandatory before commit.
- **No god files or god functions.** Split files over ~500 lines and functions over ~50 lines. Each file/function has a single clear responsibility.
- **Naming:** Descriptive, self-documenting names; avoid abbreviations unless widely known (`stt`, `vad`). A boolean parameter is a smell — consider an enum when the call site would otherwise read `foo(true, false)`.
- **Avoid:** deep nesting (>3 levels), magic numbers, unnecessary clones, ignored `Result`s, `match` on `bool`.
- **Patterns:** Prefer early returns, combinators (`map`, `and_then`), and `if let`/`while let`. Name constants with `const`; prefer std `OnceLock`/`LazyLock` over `lazy_static`/`once_cell`.
- **Hot paths:** In the audio loop and STT path, avoid allocations — pre-allocate and reuse buffers (`Vec::clear`), and pass slices (`&[f32]`) rather than owned vectors where possible.

### Comments
- **Comment the *why*, not the *what*.** Don't restate what the code plainly does or narrate each step. Reserve comments for non-obvious rationale, invariants, gotchas, and edge cases.
- Prefer self-documenting names over explanatory comments. No redundant, decorative, or restating comments.
- Keep it lean: a single line of context beats a paragraph. A `///` doc comment on public items (note panic/error conditions); sparse `//` notes only where the reason isn't obvious from the code.

### File Operations
- **Config writes:** Write to a tempfile, then atomically rename. Never write the target file in place.
- **Config reads:** Validate structure and field ranges immediately after deserialization; clamp or reject out-of-range values, and fall back to defaults on a corrupt file instead of failing startup.
- **Model downloads:** Verify the SHA256 checksum before using a downloaded file; delete corrupted artifacts and log a warning.

### Dependencies
- Prefer well-maintained crates; avoid yanked versions. `Cargo.lock` is committed for reproducible builds — change it deliberately, not via an incidental full re-resolve.
- Keep the dependency tree minimal; prefer pure-Rust implementations over FFI wrappers unless performance or ecosystem dictates otherwise.
- Run `cargo audit` in CI (`cargo deny` optional for license + advisory gating).

## UI Guidelines (Tauri App)

- **Dark theme** by default. CSS custom properties for colours; spacing on an 8px grid.
- **Tech:** Vanilla JS + HTML/CSS, no framework. Communicate with the backend via Tauri `invoke`/`listen`.
- **Accessibility:** Semantic HTML (`<button>`, `<nav>`), keyboard navigable (tab order, visible focus), ARIA labels on icon-only buttons.
- **Performance:** Debounce rapid events (audio levels); use `requestAnimationFrame` for animations and avoid layout thrashing.
- **Memory:** Remove event listeners on teardown/window close, and abort in-flight fetches that are no longer needed.

## Testing

- **Unit tests:** Same file, behind `#[cfg(test)]`. Mock external deps with `mockall` (or hand-rolled traits). Don't sleep on real time; use `tempfile` for filesystem fixtures.
- **Integration tests:** `tests/` directory at crate level, exercising public APIs.
- **Benchmarks:** `benches/` with `cargo bench` (criterion for STT/VAD hot paths); compare before/after on performance changes.
- **CI:** Run the suite with both `--all-features` and `--no-default-features` to catch feature-gate regressions, alongside `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo audit`.

## Common Commands

```
cargo check --workspace                          # check default features
cargo check --workspace --features full          # check with STT/VAD
cargo run -p murmur-cli -- --help                # CLI help
cargo run -p murmur-app                          # launch Tauri app
cargo fmt --all && cargo clippy --workspace      # lint
cargo test --workspace --all-features            # full test suite
cargo bench -p murmur-core                       # benchmarks
```

## Commit Style

Conventional Commits: `feat(core): add VAD pipeline`, `fix(cli): handle missing config`
Scopes: `core`, `cli`, `app`, `audio`, `stt`, `config`
**Never include AI attribution (Co-Authored-By, "generated by", etc.) in commits, code comments, or any files.**

## Security

- **Local only** by default — no network calls, no telemetry. Any optional cloud feature must be explicit opt-in behind a feature flag.
- **Checksums:** Verify SHA256 on every downloaded model and artifact against an expected hash; delete and re-fetch on mismatch.
- **Config:** Atomic writes (tempfile + rename). No secrets in plain text; the app stores none today, and any future secret belongs in a platform keyring.
- **CI:** Run `cargo audit` on every push.
