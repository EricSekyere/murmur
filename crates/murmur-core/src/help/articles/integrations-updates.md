# Integrations and updates

Murmur connects to coding agents and editor plugins, and keeps itself
current with a signed auto-updater.

## Coding agents

Murmur includes a built-in MCP server with four tools that let Claude
and Cursor read and search your recent dictation, and ask you questions
you answer by voice. It is fully local, and one click in Settings
(Connect editors) wires it up.

> **Tip:** See the "Coding agents (Claude and Cursor)" article for the
> tool reference, setup steps, privacy controls, and troubleshooting.

## Local API for editor plugins

Editor plugins, such as the Murmur VS Code extension, can connect to a
local WebSocket API that streams live dictation events and can start and
stop dictation. It is off by default; turn on "Local API for editor
plugins" in Settings and restart Murmur to use it.

> **Note:** The API listens on localhost only, so nothing outside your
> machine can reach it, every client must authenticate with a token that
> changes on each start, and connections from web pages are refused.

## Automatic updates

Murmur checks for new versions and shows an update banner when one is
available. Click Update and Restart to download and install it, then
Murmur relaunches into the new version. Updates are cryptographically
signed and verified before they are applied, so a tampered update is
refused.

## What's new

After an update, the What's New panel highlights what changed. It opens
once per update, and you can reopen it any time from the button in
Settings.

## Platform support

| Platform | Status |
| --- | --- |
| Windows | Ships today, signed, auto-updating |
| Linux | Ships today; prefer an X11 session |
| macOS | In progress, not yet signed or notarized |

On Linux, the global hotkey works, but double-tap activation and direct
typing into other apps are limited under Wayland, where Murmur falls back
to clipboard and paste.
