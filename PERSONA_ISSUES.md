# Persona Review — Issues & Implementation Plan

## P0 — Blocking

### 1. Click-to-stop on any mouse click (M3, R4)
**Problem:** Global mouse listener stops recording on ANY click, preventing users from interacting with other apps while dictating.
**Fix:** Make click-to-stop an opt-in setting (`click_to_stop: bool` in config, default `false`). Remove from default behavior. Add toggle in Settings UI.

### 2. No hotkey customization in UI (M1)
**Problem:** Hotkey only changeable via TOML file. Default Ctrl+Q conflicts with common shortcuts.
**Fix:** Add hotkey display + edit control in Settings panel. Show current hotkey, allow typing a new combo, validate, save, and re-register.

### 3. No output mode selection in UI (M2)
**Problem:** `output_mode` exists in config but not exposed in Settings panel. Users don't know keyboard mode exists.
**Fix:** Add output mode selector (Keyboard / Clipboard / Stdout) in Settings panel. Save to config on change.

## P1 — Major Gap

### 4. No punctuation in Whisper output (D1, D2)
**Problem:** Whisper produces no punctuation or capitalization. Parakeet handles this natively.
**Fix:** Use `set_initial_prompt()` with a punctuation-encouraging prompt for Whisper. Add note in model descriptions that Parakeet has native punctuation.

### 5. Crashes on low-RAM hardware (R1)
**Problem:** No warning when selecting large models on systems with limited RAM.
**Fix:** Show estimated memory usage per model in the model selector UI. Add a warning badge/label for models >1GB.

### 6. Poor laptop mic handling / No mic level indicator (R2, R5, X11)
**Problem:** No visual feedback for mic input quality. Quiet mics produce bad results with no explanation.
**Fix:** Add a real-time RMS level indicator visible during recording. Show "too quiet" / "good" zones. Emit audio-level events from the audio worker.

### 7. Fragmented long dictation — phrase pause not tunable in UI (D3, M5, X8)
**Problem:** 1.8s phrase pause and 10s session timeout are hard-coded defaults only in config file. No UI control.
**Fix:** Add sliders/inputs for phrase pause and session timeout in Settings panel. Save to config.

## P2 — Important

### 8. No audio device selector in UI (R7)
**Problem:** `audio_device` config field exists but no UI dropdown.
**Fix:** Add Tauri command to list audio devices. Add dropdown in Settings panel.

### 9. Widget can't be hidden (M8, R6)
**Problem:** Widget is always visible. Can't hide during screen sharing. No keyboard control.
**Fix:** Add a "Show widget" toggle in Settings. Persist to config. Add tray menu item to toggle widget.

### 10. No `prefers-reduced-motion` support (R9)
**Problem:** Animations (voice bars, pulse, gradient drift) run even when OS accessibility setting is on.
**Fix:** Add `@media (prefers-reduced-motion: reduce)` to CSS that disables/simplifies animations.

### 11. CLI improvements: JSON output (X2)
**Problem:** CLI only outputs plain text. No structured output for scripting.
**Fix:** Add `--format json` flag to `listen` command that outputs JSON lines.

## P3 — Nice to Have (Deferred)
- Text formatting commands ("new paragraph", "new line")
- Undo last output
- Session-level history grouping
- Notification sounds
- Shell completions
- CLI status/health command
- Review-before-output mode
- CLI streaming mode
- CLI toggle mode
