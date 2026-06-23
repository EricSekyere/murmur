# Troubleshooting

Most problems are a microphone, hotkey, or model issue. Start here.

## Recording stops right after it starts

If a session ends after a couple of seconds with nothing transcribed, the
microphone is delivering silence. The most common cause is echo cancellation: the
Windows voice processing path can hand back digital silence on some audio setups.
Murmur now detects this and falls back to your raw microphone automatically, but
you can also turn off echo cancellation in Settings to use the plain microphone.

## My microphone is not picked up

Check that the right input device is selected as your system default, that it is
not muted, and that the input volume is up. Another app holding the microphone in
exclusive mode can also starve Murmur. Unplugging and replugging a USB mic, or
restarting Murmur, clears most stuck capture states.

## Nothing was typed

Murmur delivers to the window focused when the session started. If you switched
windows, the text may have gone to the clipboard as a fallback instead. Idle or
silent sessions are dropped on purpose and are not an error.

## The hotkey does nothing

The shortcut may already be in use by another application, or the registration
failed at startup. Open Settings and rebind it to a free combination. On Linux,
global hotkeys and double tap need an X11 session.

## Words come out wrong or repeated

Switch to a more accurate model, or add tricky terms to your custom dictionary so
they are biased correctly. Background noise and very short utterances are the
usual cause of garbled or repeated output.

## Echo cancellation

Echo cancellation removes the speaker audio your microphone would otherwise pick
up, which is useful on calls. If it causes silence or you do not need it, turn it
off in Settings; transcription quality is otherwise unaffected.
