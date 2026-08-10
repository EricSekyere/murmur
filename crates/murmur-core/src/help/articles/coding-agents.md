# Coding agents (Claude and Cursor)

Murmur ships a built-in MCP server that lets Claude, Cursor, and other
MCP clients work with your dictation: read what you recently said, search
it, and even ask you a question you answer by voice. Everything runs
locally over standard input and output; nothing leaves your machine.

## What this gives you

Dictate a thought while you work, then ask your agent "do what I just
said". Ask it to find the ticket number you dictated yesterday. Or let a
long-running agent pause mid-task, ask "should I migrate the tests too?",
and take your spoken answer as the reply, without you touching the
keyboard.

> **Privacy:** The server only ever reads the same local history file the
> History view shows. It never captures audio itself, never writes to
> your history, and makes no network connections.

## The four tools

| Tool | What it does | Typical use |
| --- | --- | --- |
| `get_recent_transcripts` | Returns your latest dictations, newest first (default 20, max 100) | "Act on what I just dictated" |
| `search_transcripts` | Case-insensitive substring search over history, newest first | "Find where I mentioned the auth bug" |
| `wait_for_next_dictation` | Waits for the next phrase you dictate and returns it; never starts recording | The agent asks a question, you press your hotkey and answer |
| `request_dictation` | Starts a recording session in the running app and returns what you say | Hands-free voice answers, with an optional on-screen prompt |

The waiting tools take a `timeout_secs` parameter (default 30 seconds,
capped at 300) and reply with a tagged status: `received` with the
transcript text, `timed_out`, or `history_disabled` when Save History is
off.

## Set up from the app

1. Open Settings and find the editor integration section.
2. Click **Connect editors**. Murmur detects installed clients (Cursor and Claude Desktop) and adds a `murmur` entry to each one's MCP config. Existing servers and settings in those files are preserved.
3. Restart the editor (or toggle the server in its MCP settings) to load it.

The entry points at the Murmur app itself, which serves MCP when the
editor relaunches it, so there is no separate program to install.

## Set up from the CLI

1. Run `murmur mcp install` to configure every detected client, or name one explicitly with `murmur mcp install cursor` or `murmur mcp install claude-desktop` (naming a client writes its config even if it was not detected).
2. For Claude Code, the install command prints a ready-made `claude mcp add murmur ...` one-liner to run yourself.
3. `murmur mcp` with no action runs the stdio server directly; that is what the editor invokes, so you never need to run it by hand.

## What an agent can and cannot do

The agent sees exactly what the History view stores: transcript text, a
timestamp, and (when app-aware history is on) the name of the app the
text was delivered to. Two settings control everything:

| Setting | Default | What it gates |
| --- | --- | --- |
| Save History | On | All four tools. Off means the read tools return an empty list and the waiting tools answer `history_disabled`, because there is nothing to read back. |
| Allow agents to start dictation | On | Only `request_dictation`. Off means the connection is strictly read-only: the app ignores start requests, and recording only ever begins from your own hotkey. |

An agent can never turn the microphone on by itself: the app owns
capture, and even `request_dictation` only asks the running app to start
an ordinary session, with the pill visibly recording. It cannot read
anything you deleted from history, cannot modify or delete history, and
cannot see live audio.

> **Note:** Turning Save History off also purges the existing log, so
> there is no stale file left for a tool to read.

## Voice answers, from your side of the desk

Here is what a `request_dictation` round trip looks like:

1. Your agent hits a fork in the road and calls the tool, for example with the prompt "Keep the old config format too?".
2. The Murmur pill appears and starts recording, showing the agent's question, exactly like a session you started yourself.
3. You say "yes, keep reading the old format but always write the new one" and stop as usual (or let auto-stop end it).
4. The transcript is delivered like any dictation, and the agent receives the same text as its answer and carries on.

If you would rather stay in control of the microphone, the agent can use
`wait_for_next_dictation` instead: it only watches for the next phrase
you dictate, and nothing happens until you press your own hotkey.

A start request is only honored when the app considers it safe: the
allow-agents toggle must be on, no session may already be recording, and
the request must be recent (requests older than five minutes are
ignored). Every request retires itself once it finishes, whether you
answered or it timed out, so a finished request cannot open the
microphone later. Two editors can each have a request outstanding without
cancelling one another.

> **Tip:** Give the agent a generous `timeout_secs` when you expect to
> think before answering; the default 30 seconds passes quickly.

## Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| Tools return an empty list or `history_disabled` | Save History is off. Turn it on in Settings; dictate something new for the waiting tools to observe. |
| The editor does not list Murmur's tools | The editor caches MCP config at startup. Restart it after Connect editors, or toggle the server in its MCP settings. |
| `request_dictation` always times out | The Murmur app is not running (the trigger is only acted on by the live app), or "Allow agents to start dictation" is off. |
| A request arrived while you were already dictating | Murmur never interrupts a live session; the request is dropped and the agent's call times out. Ask it to retry. |
| You answered, but the agent reported `timed_out` | Your session ended after the agent's timeout expired. The transcript is still saved, so the agent can fetch it with `get_recent_transcripts`; next time have it pass a larger `timeout_secs`. |
