# Voice commands and editing

Murmur recognizes a small set of spoken editing commands while you dictate. A
command only fires when it is the entire phrase, so dictating it inside a sentence
just types the words.

## Line breaks

Say "new line" to insert a single line break, or "new paragraph" to insert a
blank line (two breaks). These let you structure text without reaching for the
keyboard.

## Scratch that

Say "scratch that" or "delete that" to remove the phrase you just delivered. It
is the quickest way to undo a misrecognized sentence and try again.

## Undo and redo

Say "undo that" to undo (Ctrl+Z / Cmd+Z) and "redo that" to redo. The two-word
form is required on purpose: a bare "undo" or "redo" is too easy to misrecognize
and would destroy real edits, so it is treated as plain text.

## Copy, Tab and Escape

Say "copy that" or "copy selection" to copy (Ctrl+C / Cmd+C). Say "press tab" or
"tab key" to press Tab, and "press escape" or "escape key" to press Escape. These
act only when spoken as the whole phrase.

## Commands that are deliberately not voice triggered

Paste, cut, and select all are intentionally never voice commands. A single
misrecognition could inject your clipboard or wipe a document, so saying "paste"
or "select all" simply types those words instead.

## Type a command literally

To type a command's words instead of running it, prefix the phrase with
"literally" or "literal". For example, "literally scratch that" types the text
"scratch that". This escape only kicks in when the phrase would otherwise act, so
ordinary prose that happens to start with "literally" is untouched.

## Why commands need the whole phrase

Commands and snippets match only after normalizing the full phrase (ignoring
case, surrounding spaces, and trailing punctuation). Saying "press the new line
button" or "scratch that itch" is delivered as plain text, because the command is
not the entire phrase.
