# Security Policy

Murmur handles microphone audio and everything you dictate, so security reports are taken seriously.

## Supported versions

Only the [latest release](https://github.com/EricSekyere/murmur/releases/latest) is supported. The app auto-updates on Windows and Linux (AppImage), so most users are on the current version.

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

- Email **eric@ericsekyere.ca** with a description, reproduction steps, and the version affected.
- You should receive an acknowledgement within a few days. Please allow a reasonable window for a fix and coordinated disclosure before publishing details.

## Scope: what is security-sensitive here

Reports in these areas are especially valuable:

- **Audio and transcript confidentiality** — anything that causes audio or dictated text to leave the machine, persist unexpectedly, or appear in logs above `trace` level.
- **The local WebSocket API** — bypassing its token authentication, Origin rejection, or localhost-only binding (see `docs/local-api.md` and `crates/murmur-app/src/local_api/`).
- **Download integrity** — bypassing SHA-256 verification of model/runtime downloads or minisign verification of updates.
- **Keystroke injection** — crafted speech or transcripts that escape the text-delivery path and trigger unintended actions in the focused application.
- **The meeting audio spool** — meeting audio surviving on disk outside its documented lifecycle (deleted after processing; swept at next launch after a crash).

## What Murmur intentionally does

To save triage time, the following is by design, not a vulnerability:

- Network requests to Hugging Face and GitHub for model/runtime downloads and to GitHub Releases for update checks.
- Transcription history stored in plaintext JSON in the user's config directory (local, capped, user-clearable, and disableable).
- Keystroke simulation into whichever window has focus — that is the product.
