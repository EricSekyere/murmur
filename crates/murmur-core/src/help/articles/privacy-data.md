# Privacy and your data

Murmur is local-first by design. Speech recognition runs entirely on your
machine, and nothing you dictate is sent anywhere by default.

## Everything runs on device

Audio capture, voice activity detection, and transcription all happen
locally using a model on your own machine. There are no cloud calls for
dictation and no telemetry. Your speech and the resulting text stay on
your computer.

## No audio is kept

Audio is processed and discarded; Murmur does not save recordings. Only
the text of delivered phrases is ever stored, and only if you leave
history on. To power Re-transcribe, the last few utterances (about ten,
up to a minute of audio in total) stay in memory while the app runs, and
only while history is on; that audio is never written to disk and
vanishes when the app exits, when you clear history, or when you turn
Save History off.

> **Privacy:** The one exception is meeting speaker labels: with labels
> enabled, a meeting's audio is spooled to a temporary file so speakers
> can be told apart. That file is deleted as soon as the meeting is
> processed, on any failure, when you quit the app mid-meeting (the
> transcript is still saved; only the speaker labels are skipped), and at
> the next launch. Without speaker labels, meeting audio never touches
> disk either.

## History is optional

Murmur keeps a local, searchable log of delivered phrases so you can find
and reuse them. Turn off Save History in Settings to store nothing on
disk. Turning history off also purges what is already stored, deleting
the history file rather than leaving an empty one behind.

## Where your data lives

| Data | Location |
| --- | --- |
| Settings | `config.toml` in your config directory under murmur |
| History | `history.json` next to it |
| Meeting transcripts | A meetings folder alongside |
| Downloaded models | Your app data folder under murmur/models |

These are ordinary files protected by your normal account permissions.

## Safe config handling

Config and history are written atomically (to a temporary file, then
renamed), so a crash mid-write cannot corrupt them. A config that is
somehow unreadable is backed up and replaced with defaults rather than
blocking startup, so Murmur always launches.

## Download integrity

Every model file is verified against a pinned SHA256 checksum before it
is used. A corrupted or tampered download is rejected and refetched, so
you only ever run the expected model bytes. Downloads fail closed: a file
with no pinned checksum is refused before any network request is made, so
there is no unverified download path.

## Editor integration stays local

The optional MCP integration that lets Claude and Cursor read your recent
dictation runs locally over standard input and output and never leaves
your machine. Its history tools are read-only, and a separate "Allow
agents to start dictation" toggle controls whether an agent may also
request a spoken answer from you. The optional editor-plugin API listens
on localhost only and requires a per-start token, so no other machine can
connect. If you turn history off, there is nothing for either to read.
