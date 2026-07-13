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

## Conventional Commit by voice

Say "commit" followed by a commit type (feat, fix, docs, chore, and the other
Conventional Commit types) to type a formatted commit line instead of prose:
"commit feat scope core add the vocabulary metric" types
`feat(core): add the vocabulary metric`. Say "scope" plus one word for the
scope, and "breaking" right before the description for the `!` marker. Murmur
only types the line; it never runs git. Phrases like "commit the changes" are
unaffected because a valid type must follow "commit".

## Spoken emoji

Say "emoji" followed by a name to insert the character inline: "great work
emoji fire" delivers "great work 🔥", and "emoji thumbs up" delivers 👍. The
explicit "emoji" keyword keeps words like "fire" safe in ordinary prose, and an
unknown name is simply typed as-is.

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
