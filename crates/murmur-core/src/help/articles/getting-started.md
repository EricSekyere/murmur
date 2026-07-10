# Getting started

Murmur turns your voice into text in any app. Press or hold your hotkey, speak,
and the words appear wherever your cursor is. Everything runs on your machine.

## Dictate with the hotkey

The default hotkey is Ctrl+Shift+Space (Super+Shift+Space on macOS). Press it to
start a session, speak, then press it again to stop, or just pause and let it
auto-stop. The floating pill shows the current state as you go.

## Double-tap to start and stop

You can also start and stop without a key combo by double-tapping a single key.
The default is Right Ctrl on Windows (either Ctrl on macOS and Linux). Right Ctrl
never types a character and is almost never part of a shortcut, so a double-tap
cannot collide with copy or paste. Set this under Settings as the Activation Key.

## Push to talk vs toggle

In toggle mode one press (or double-tap) starts recording and the next stops it.
In hold (push to talk) mode, recording lasts only while you hold the key. Choose
between them under Settings as the Activation mode. Toggle is the default.

## Change your hotkey

Open Settings, find Hotkey, click into the field, and press your desired key
combination, then Save. If a hotkey fails to register, another app is usually
already using it, so pick a different combination. Avoid Ctrl plus a single
letter, which collides with common app shortcuts.

## Where your text goes

Text is delivered to the window that was focused when you started the session, so
switching windows mid sentence does not send your words to the wrong place. By
default Murmur types the characters directly, which never touches your clipboard.

## Auto-stop and timeouts

A session ends on its own after a short silence once you have spoken, and a whole
session ends after a longer stretch of total inactivity (60 seconds by default).
During shorter pauses the session stays live: the pill dims to a "waiting" look
and wakes again the moment you resume speaking. Set Session Timeout to 0 to keep
listening until you stop it yourself, which is useful for hands-free dictation.

## First-run setup

The first time you open Murmur, a short onboarding walks you through the hotkey,
a microphone test, and a few tips. The default model downloads once in the
background, and the mic test shows your words on screen without typing them
anywhere.
