# Always listening

Always listening is an opt-in mode that starts a normal dictation
session when you say the wake phrase "Hey Murmur". It is off by default.
The first time you turn it on, Murmur downloads three checksum-verified
model files and then arms the detector.

Turn it on under Settings as Always listening, or from the tray menu.
Turning it off disarms immediately (or after the current session ends, if
one is already running).

## What arming means

When the mode is armed, the microphone stays open. Incoming audio frames
are scored by a local wake-word detector and then discarded. Scoring is
not dictation: there is no session, no calibration, and no speech-to-text
engine loaded. Only a detection of "Hey Murmur" starts the same full
session a hotkey press would.

The OS mic-in-use indicator stays lit while armed, because the stream is
actually open. A meeting takes the microphone for itself and disarms
until the meeting ends.

## How the indicators look

Every tray and widget state is driven by the worker that holds the
stream, so the indicator cannot show armed unless the detector is
actually running. Colour may reinforce a state; shape is what tells them
apart, including in greyscale.

| Where | State | What you see |
| --- | --- | --- |
| Tray | Idle | The Murmur logo (waveform / "W" / arrow), not a mic |
| Tray | Armed | Hollow outline mic with a small dot badge |
| Tray | Recording | Solid filled mic glyph |
| Floating pill | Armed | Static outlined orb; no waveform, no pulse |
| Floating pill | Recording | Filled orb with a waveform |
| Sound | On wake | The same start chime as a hotkey session |

The two states you must never confuse are armed and recording: hollow mic
plus badge versus solid mic on the tray, and a static outlined orb versus
a filled orb with a waveform on the pill.

> **Tip:** Sound Cues play the start chime when a wake is detected and
> the stop chime if that session ends. Turn them off in Settings if you
> prefer silent operation.

## What stays in memory

While armed, Murmur holds in memory only a rolling audio window of at
most one second and a few seconds of derived 96-dimensional audio
embeddings (not reconstructible audio); nothing is written to disk,
nothing leaves the machine, and all of it is discarded the moment the
mode is disarmed or the app exits.

> **Privacy:** Those embeddings are audio fingerprints, not a recording
> you could play back. Model files are verified against a pinned SHA256
> checksum before they are used, the same as every other Murmur download.

## Sensitivity

Wake sensitivity is Low, Medium, or High. Medium is the recommended
default. The setting changes how readily "Hey Murmur" is accepted; it
does not change microphone gain.

| Level | What it does |
| --- | --- |
| Low | Fewer false triggers; may miss a quiet or distant phrase |
| Medium | The recommended operating point |
| High | Better pickup in noise; more likely to fire on similar speech |

If the TV, a call, or someone nearby keeps starting sessions you did not
mean, drop to Low. If your own "Hey Murmur" is ignored in a noisy room,
raise it to High.

## False triggers

A false trigger looks like a real start: the start chime plays, the tray
and pill switch to recording, then nobody speaks. Within about 4 seconds
the session silently aborts, the stop chime plays, and Murmur re-arms.
Nothing is transcribed or delivered.

That cutoff is a silent abort, a fixed ~4 second window for wake-started
sessions only. It is not the Session Timeout (which is a minute of
inactivity by default, and still applies to hotkey sessions). If you do
speak after the wake, the abort is cancelled and dictation continues
normally.

False triggers are expected occasionally. The short abort is what keeps
each one from sitting on an open mic until the session timeout.

## Why the phrase is Hey Murmur

The wake phrase is "Hey Murmur" and cannot be changed in v1. The detector
uses a model trained specifically for that phrase; a different phrase
would be a different trained artifact, not a setting.

Custom phrases are a future training-pipeline feature, not something you
can type in today.
