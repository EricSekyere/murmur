# Integrations and updates

Murmur can share your recent dictation with AI coding tools through a local MCP
server, and it keeps itself current with a signed auto-updater.

## The MCP server

Murmur includes a built-in Model Context Protocol server that lets Claude and
Cursor work with your dictation. It exposes two read-only tools, one that
returns your most recent transcripts and one that searches them by text, plus
an optional tool that lets an agent ask you to dictate an answer (see below).
It is fully local and talks over standard input and output, so nothing leaves
your machine.

## Connect your editor

In Settings, under Connect to Cursor / Claude, click Connect editors. Murmur
detects installed clients (Cursor and Claude Desktop) and adds the server entry to
their config in one click. Restart the editor afterward to finish. The entry
points at the Murmur app, which serves MCP when relaunched, so there is no
separate program to install.

## What the editor can see

The history tools are read-only: they can read your stored dictation history
and nothing else, and they can never change anything. If you turn Save History
off, there is nothing for them to read. Whether an agent may also start a
dictation session is a separate, explicit toggle, described next.

## Voice answers for coding agents

With "Allow agents to start dictation" on (the default), a connected coding
agent can ask you a question mid-task through a dictation-request tool: Murmur
starts an ordinary dictation session, you answer by speaking, and the
transcribed answer is returned to the agent. The session looks and behaves like
any other, so you always see when it is listening. Turn the toggle off in
Settings to keep the MCP connection strictly read-only.

## Local API for editor plugins

Editor plugins, such as the Murmur VS Code extension, can connect to a local
WebSocket API that streams live dictation events and can start and stop
dictation. It is off by default; turn on "Local API for editor plugins" in
Settings and restart Murmur to use it. The API listens on localhost only, so
nothing outside your machine can reach it, every client must authenticate with
a token that changes on each start, and connections from web pages are
refused.

## Automatic updates

Murmur checks for new versions and shows an update banner when one is available.
Click Update and Restart to download and install it, then Murmur relaunches into
the new version. Updates are cryptographically signed and verified before they are
applied, so a tampered update is refused.

## What's new

After an update, the What's New panel highlights what changed. It opens once per
update, and you can reopen it any time from the button in Settings.

## Platform support

Murmur ships today on Windows (signed, auto-updating) and Linux. macOS support is
in progress and not yet signed or notarized. On Linux, prefer an X11 session: the
global hotkey works, but double-tap activation and direct typing into other apps
are limited under Wayland, where Murmur falls back to clipboard and paste.
