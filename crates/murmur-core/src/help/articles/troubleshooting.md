# Troubleshooting

Most problems are a microphone, hotkey, or model issue. Start here.

## Recording stops right after it starts

If a session ends after a second or two with nothing transcribed, the microphone
is delivering silence. The most common cause is echo cancellation: the Windows
voice-processing path can hand back digital silence on some audio setups. Murmur
detects this automatically, immediately if the voice path fails to start or
after a few silent sessions in a row, and falls back to your raw microphone.
You can also turn off Echo Cancellation in Settings to use the plain microphone
directly.

## My microphone is not picked up

Check that the right input device is selected in Settings (or as your system
default), that it is not muted, and that the input volume is up. Another app
holding the microphone in exclusive mode can starve Murmur. Unplugging and
replugging a USB mic, or restarting Murmur, clears most stuck capture states.

## Nothing was typed

Murmur delivers to the window focused when the session started. If direct typing
could not reach it, the text may have gone to the clipboard as a fallback instead,
so try pasting. Idle or silent sessions are dropped on purpose and are not an
error. Check the Diagnostics view to see whether the phrase was rejected.

## The hotkey does nothing

The shortcut may already be in use by another application, or registration failed
at startup. Open Settings and rebind it to a free combination. Avoid Ctrl plus a
single letter, which collides with common app shortcuts. On Linux, global hotkeys
need an X11 session.

## Double-tap does not work

Double-tap activation works on Windows and macOS. It is not available on Linux, so
use the global hotkey there. On Windows, the default double-tap key is Right Ctrl,
which never types a character; if you remapped it to a letter, taps only count
when no other key is involved.

## Words come out wrong or repeated

Switch to a more accurate model, or add the tricky terms to your personal
dictionary so they are biased toward the right spelling. Background noise and very
short utterances are the usual cause of garbled or repeated output. The strict
transcription profile filters more aggressively if hallucinations persist.

## Output is slow to appear

A heavy Whisper model on a CPU-only machine adds latency. Switch to Parakeet (the
default) or a small Whisper model for low latency, and turn off Live Preview for
the absolute fastest delivery. The Diagnostics view shows model latency.

## Typing fails on Linux

Under Wayland, the system blocks one app from typing into another, so direct
keystroke output does not work. Murmur falls back to clipboard and paste there.
For full direct typing, use an X11 session.

## Echo cancellation

Echo cancellation removes the speaker audio your microphone would otherwise pick
up, which is useful on calls and is Windows only. If it causes silence or you do
not need it, turn it off in Settings; transcription quality is otherwise
unaffected.

## A corrupt config or history

If your config file becomes unreadable, Murmur backs it up and starts from
defaults rather than failing to launch, so you never get stuck. A history file
that cannot be parsed is likewise backed up and a fresh one is started.
