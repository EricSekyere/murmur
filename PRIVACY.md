# Privacy Policy

**Effective date:** August 14, 2026

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
- During dictation, audio is held in memory only for the duration of
  processing and is never written to disk. The most recent few utterances may
  also be kept in memory until the app exits so a phrase can be re-transcribed
  from its history entry; that audio is likewise never written to disk.
- **Meeting mode** is the one exception: when speaker labels are enabled (the
  diarization model is downloaded), meeting audio is temporarily spooled to a
  file in Murmur's config directory so it can be processed after the meeting.
  That file is deleted immediately after processing, on any error, and, if the app crashed mid-meeting, swept on the next launch. It never leaves your
  machine and there is no setting that retains it.
- **Always-listening mode** (off by default): while armed, Murmur holds in
  memory a rolling audio window of at most one second, less than one further
  frame of audio (under 80 milliseconds) waiting to be scored, and a few
  seconds of derived audio fingerprints (mel-spectrogram frames and
  96-dimensional embeddings, which are not reconstructible audio) so it can
  hear the wake phrase. Nothing is written
  to disk, nothing leaves your machine, and all of it is discarded the moment
  the mode is disarmed or the app exits. The wake-word model files are
  downloaded and checksum-verified like any other model.
- Transcribed text is typed into whichever application you have focused. It is
  never transmitted to us or any third party.

## What is stored on your device

The following is stored **only on your own computer**, never in the cloud:

- **Settings:** your configuration file, in your operating system's standard
  config directory.
- **Transcription history:** by default, delivered phrases are saved to a
  local, searchable history (capped at the most recent 500 entries). You can
  turn this off ("Save History") in Settings and clear it at any time.
- **Meeting records:** transcripts, speaker labels, and summaries from
  meetings you record are saved locally alongside the config directory.
- **Diagnostic logs:** local log files for troubleshooting. Transcript text is
  not written to logs at the default log level.
- **Models:** downloaded speech-to-text model files.

You can remove any of this by clearing history in the app, deleting Murmur's
config directory, or uninstalling the Software.

## Network activity

Murmur connects to the internet only for the following purposes:

1. **Model and runtime downloads:** on first use, or when you select a new
   model, Murmur downloads the required model and runtime files from their
   hosting providers (such as Hugging Face and GitHub) and verifies their
   integrity with a SHA-256 checksum.
2. **Update checks:** at startup, Murmur checks GitHub Releases for new
   versions and can download and install updates. Update packages are
   signature-verified before install.
3. **Optional cloud rewrite (off by default):** builds compiled with the
   `cloud` feature can send text you explicitly rewrite to an
   OpenAI-compatible endpoint that **you** configure with **your own** API
   key. This requires the feature to be compiled in, the setting to be
   enabled, and the `MURMUR_CLOUD_API_KEY` environment variable to be set;
   absent any one of those, no such request is ever made. Your API key is
   read from the environment at call time and is never stored in the config
   file or logged.

These requests are made directly to those third-party services. We do not
operate any intermediary servers and do not receive any data about you.

Two local integration surfaces exist, neither of which uses the internet: an
opt-in WebSocket API for editor plugins (localhost-only, token-authenticated,
off by default; see `docs/local-api.md`) and an MCP server that editors spawn
and talk to over stdin/stdout. Both only ever exchange data with software
running on your own machine.

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
