# Privacy and your data

Murmur is local-first by design. Speech recognition runs entirely on your
machine, and nothing you dictate is sent anywhere by default.

## Everything runs on device

Audio capture, voice activity detection, and transcription all happen locally
using a model on your own machine. There are no cloud calls for dictation and no
telemetry. Your speech and the resulting text stay on your computer.

## No audio is kept

Audio is processed and discarded; Murmur does not save recordings. Only the text
of delivered phrases is ever stored, and only if you leave history on.

## History is optional

Murmur keeps a local, searchable log of delivered phrases so you can find and
reuse them. Turn off Save History in Settings to store nothing on disk. Turning
history off also purges what is already stored, deleting the history file rather
than leaving an empty one behind.

## Where your data lives

Settings are stored as a TOML config file in your config directory under murmur,
and history sits next to it as history.json. Downloaded models live in your app
data folder under murmur/models. These are ordinary files protected by your normal
account permissions.

## Safe config handling

Config and history are written atomically (to a temporary file, then renamed), so
a crash mid-write cannot corrupt them. A config that is somehow unreadable is
backed up and replaced with defaults rather than blocking startup, so Murmur
always launches.

## Download integrity

Every model file is verified against a pinned SHA256 checksum before it is used.
A corrupted or tampered download is rejected and refetched, so you only ever run
the expected model bytes.

## Editor integration stays local

The optional MCP integration that lets Claude and Cursor read your recent
dictation runs locally over standard input and output. It is read-only and never
leaves your machine. If you turn history off, there is nothing for it to read.
