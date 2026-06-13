# Murmur v0.2.0

Private, on-device voice dictation for Windows. Press a key, speak, and your words appear in whatever app has focus. It runs fully offline, is GPU-accelerated, and uses no cloud.

This is the first public release. Models download automatically on first launch.

## Highlights

**Dictate anywhere.** Text is typed straight into the focused window (editor, browser, terminal, chat), with smart fallbacks for terminals and elevated apps. A floating, always-on-top pill gives you one-click control and a live level meter from any screen.

**Start how you like.** Double-tap Right Ctrl to start and stop, hold the key to talk and release to stop (push-to-talk), use your hotkey (default Ctrl+Q), or click the pill.

**Voice editing commands.** Say "new line", "new paragraph", or "scratch that" while dictating. They are recognized as commands only when spoken on their own, so normal sentences pass through untouched.

**Personal dictionary.** Add names, jargon, and product terms under Settings, Personal Dictionary so they transcribe with the right spelling.

**Noise-robust.** Silero voice-activity detection plus decoder-confidence checks keep sighs, breaths, and background noise from turning into phantom words.

**Sound cues.** A short tone plays when recording starts and stops. You can turn it off under Settings, Sound Cues.

**First-run onboarding.** A quick welcome, a live mic test, and tips to get you going.

**Auto-update.** From this release on, Murmur checks for new versions and offers a one-click "Update and Restart."

**Choose your model.** Swap between speed-oriented and accuracy-oriented models in Settings. On NVIDIA GPUs the local build uses CUDA for sub-second transcription.

## Install

Download the installer below and run it. The default speech model (about 490 MB) downloads automatically the first time you launch.

## Requirements

- Windows 10 or 11 (64-bit)
- A CPU with AVX2 (any Intel or AMD chip from roughly 2013 onward)
- NVIDIA GPU optional. It adds speed, but the released build runs on CPU and works on any machine.

## Privacy

Everything runs locally. The only network access is the one-time, checksum-verified download of model files. Your audio and transcripts never leave your machine.
