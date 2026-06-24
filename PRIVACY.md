# Privacy Policy

**Effective date:** June 18, 2026

This Privacy Policy explains how **Murmur** (the "Software"), a local-first
desktop dictation application provided by **Eric S.** ("we," "us," "our"),
handles your information.

**The short version: Murmur does not collect, transmit, store on our servers,
or sell any of your data.** There are no accounts, no analytics, and no
telemetry. Everything happens on your device.

## What we collect

Nothing. We operate no servers, run no analytics or tracking, and receive no
information about you or how you use Murmur.

## Audio and transcripts

- Your microphone audio is captured locally, transcribed locally by the bundled
  speech-to-text engine (whisper.cpp or NVIDIA Parakeet), and delivered as text
  on your machine.
- Audio is held in memory only for the duration of processing. Murmur does not
  save audio recordings to disk.
- Transcribed text is typed into whichever application you have focused. It is
  never transmitted to us or any third party.

## What is stored on your device

The following is stored **only on your own computer**, never in the cloud:

- **Settings** — your configuration file, in your operating system's standard
  config directory.
- **Transcription history** — by default, delivered phrases are saved to a
  local, searchable history. You can turn this off ("Save History") in Settings
  and clear it at any time.
- **Diagnostic logs** — local log files for troubleshooting. Transcript text is
  not written to logs at the default log level.
- **Models** — downloaded speech-to-text model files.

You can remove any of this by clearing history in the app, deleting Murmur's
config directory, or uninstalling the Software.

## Network activity

Murmur connects to the internet only for the following purposes:

1. **Model and runtime downloads** — on first use, or when you select a new
   model, Murmur downloads the required model and runtime files from their
   hosting providers (such as Hugging Face and GitHub) and verifies their
   integrity with a SHA-256 checksum.
2. **Update checks** — Murmur checks GitHub Releases for new versions and can
   download and install updates.

These requests are made directly to those third-party services. We do not
operate any intermediary servers and do not receive any data about you.

## Third-party services

When Murmur downloads models or updates, those requests are served by the
relevant provider (for example, GitHub for application updates and Hugging Face
for models), each subject to its own privacy policy. We do not share your data
with them beyond the standard request required to fetch a file.

## Children's privacy

Murmur is a developer tool, is not directed at children, and collects no
personal information from anyone.

## Changes to this policy

We may update this Privacy Policy from time to time. The effective date above
will be updated, and material changes will be noted in the project's release
notes.

## Contact

Questions about this policy? Contact **eric@ericsekyere.ca**.

© 2026 Eric S. All rights reserved.
