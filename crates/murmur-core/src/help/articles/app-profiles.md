# Per-app profiles

App Profiles let you override settings automatically based on the app you are
dictating into. For example, turn on developer mode in your editor and off in
chat, or use clipboard paste only in your terminal. Set them in Settings.

## How to write a profile

Enter one profile per line as app = options. The options are dev or plain for
developer mode, and auto, keyboard, clipboard_paste, or clipboard for output
mode. For example: code = dev, or windowsterminal = dev, clipboard, or
slack = plain.

## How matching works

The app name is matched against the focused window's process name when a session
starts. Matching is case-insensitive and matches a whole word, so code matches
Code.exe and code-insiders.exe but not unicode.exe. A multi-word pattern such as
visual studio matches as a substring instead.

## Which profile wins

The first profile whose pattern matches the foreground app is applied for that
session. Fields you leave out fall back to your global settings, so a profile that
only sets dev keeps your normal output mode.

## When a profile applies

A profile is evaluated at the moment a session starts, based on whatever window is
focused then. If no profile matches, your global developer mode and output mode
are used. You can store up to 50 profiles.

## Common uses

Use dev for editors and terminals so code transcribes cleanly, and plain for
Slack, email, and docs so prose stays natural. Use clipboard_paste for a terminal
that drops directly typed characters, while leaving everything else on auto.
