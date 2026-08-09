# Voice commands and editing

Murmur recognizes a small set of spoken editing commands while you dictate.
A command only fires when it is the entire phrase, so dictating it inside a
sentence just types the words.

## The commands at a glance

| Say | Get |
| --- | --- |
| "new line" or "newline" | A single line break |
| "new paragraph" | A blank line (two breaks) |
| "scratch that" or "delete that" | Removes the phrase you just delivered |
| "undo that" | Undo ([[Ctrl+Z]] / [[Cmd+Z]]) |
| "redo that" | Redo ([[Ctrl+Y]] / [[Cmd+Shift+Z]]) |
| "copy that" or "copy selection" | Copy ([[Ctrl+C]] / [[Cmd+C]]) |
| "press tab" or "tab key" | Presses [[Tab]] |
| "press escape" or "escape key" | Presses [[Escape]] |

> **Note:** The two-word "undo that" and "redo that" forms are required on
> purpose: a bare "undo" or "redo" is too easy to misrecognize and would
> destroy real edits, so it is treated as plain text.

## Commands that are deliberately not voice triggered

Paste, cut, and select all are intentionally never voice commands. A single
misrecognition could inject your clipboard or wipe a document, so saying
"paste" or "select all" simply types those words instead.

## Conventional Commit by voice

Say "commit" followed by a Conventional Commit type to type a formatted
commit line instead of prose. Murmur only types the line; it never runs git.

| Say | Get |
| --- | --- |
| "commit fix handle the missing config" | `fix: handle the missing config` |
| "commit feat scope core add the vocabulary metric" | `feat(core): add the vocabulary metric` |
| "commit feat breaking drop the old api" | `feat!: drop the old api` |

- Valid types: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert.
- Say "scope" plus one word for the scope, and "breaking" right before the description for the `!` marker.
- Phrases like "commit the changes" are unaffected because a valid type must follow "commit".

## Spoken emoji

Say "emoji" followed by a name to insert the character inline.

| Say | Get |
| --- | --- |
| "great work emoji fire" | great work 🔥 |
| "emoji thumbs up" | 👍 |

The explicit "emoji" keyword keeps words like "fire" safe in ordinary prose,
and an unknown name is simply typed as-is.

## Type a command literally

To type a command's words instead of running it, prefix the phrase with
"literally" or "literal".

| Say | Get |
| --- | --- |
| "literally scratch that" | The text "scratch that" |

This escape only kicks in when the phrase would otherwise act, so ordinary
prose that happens to start with "literally" is untouched.

## Why commands need the whole phrase

Commands and snippets match only after normalizing the full phrase (ignoring
case, surrounding spaces, and trailing punctuation). Saying "press the new
line button" or "scratch that itch" is delivered as plain text, because the
command is not the entire phrase.

## Command mode

Command mode is a separate voice mode for acting on your machine instead of
typing text. Press [[Ctrl+Shift+Period]] to enter it, then speak an action
such as "open the readme file" or "go to the source folder".

> **Warning:** Risky actions are never run from voice alone: they wait in a
> confirm dialog until you click or press a key, so a misheard phrase cannot
> act on its own.

## Choosing between close matches

Sometimes several paths fit what you said almost equally well, most often the
same filename living in two different folders. Rather than guess, Murmur shows
a short picker listing the near-tied paths.

- Click a path, or press its number from 1 to 5, to insert it.
- Press [[Escape]] or Cancel to dismiss the picker without inserting anything.
- Speaking again replaces the picker with the result of your new phrase.

If Murmur cannot hand focus back to the window you were dictating into, it
copies the chosen path to your clipboard instead of typing it, and tells you
so.

## Spoken path aliases

File and folder phrases in command mode are resolved against your indexed
project folders (see the codebase vocabulary help), and spoken aliases map
awkward spoken forms to real path segments first. These built-ins, and more,
are always active:

| Say | Means |
| --- | --- |
| "source" | `src` |
| "package json" | `package.json` |
| "dot env" | `.env` |
| "read me" | `README` |
| "cargo toml" | `Cargo.toml` |

Add your own in Settings under Spoken Path Aliases, one per line as
`spoken = path`, for example `end to end tests = tests/e2e`. An alias with
the same spoken form as a built-in overrides it.
