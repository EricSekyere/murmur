# Integrations and updates

Murmur can share your recent dictation with AI coding tools through a local MCP
server, and it keeps itself current with a signed auto-updater.

## The MCP server

Murmur includes a built-in Model Context Protocol server that lets Claude and
Cursor read your recent dictation. It exposes two read-only tools: one returns
your most recent transcripts, and one searches them by text. It is fully local and
talks over standard input and output, so nothing leaves your machine.

## Connect your editor

In Settings, under Connect to Cursor / Claude, click Connect editors. Murmur
detects installed clients (Cursor and Claude Desktop) and adds the server entry to
their config in one click. Restart the editor afterward to finish. The entry
points at the Murmur app, which serves MCP when relaunched, so there is no
separate program to install.

## What the editor can see

The integration is read-only and can only read your stored dictation history. It
cannot start recording or change anything. If you turn Save History off, there is
nothing for it to read.

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
