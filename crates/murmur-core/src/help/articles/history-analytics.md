# History and analytics

Murmur keeps a local log of delivered phrases and turns it into usage
stats, all on your machine. Both are off limits to the network and depend
on Save History being on.

## Searchable history

The History section on the Dictate view lists your delivered phrases
newest first. Type in the search box to filter by text or app
(case-insensitive substring). The log keeps your most recent phrases and
drops the oldest past its cap, so it stays a convenience rather than an
archive.

## Re-transcribe a recent phrase

If a fresh phrase came out mangled, you can run it through the engine
again without repeating yourself. Recent entries keep their audio in
memory (roughly the last ten utterances, up to about a minute in total),
and those entries show a Re-transcribe button:

1. Open History on the Dictate view and find the entry.
2. Click Re-transcribe. If you switched to a better model since, the
   re-run uses the current model.
3. The result appears as a new entry at the top of the list and is
   copied to your clipboard; nothing is typed into any window, and the
   original entry stays put.

The button only appears while the audio is still held: it disappears
after a restart, once older utterances are evicted, or if the re-run
does not pass the usual quality checks (you get a notice and the
original entry is left unchanged). Re-transcription waits its turn:
while you are dictating or recording a meeting it refuses rather than
slowing live speech.

> **Privacy:** That audio lives in RAM only. It is never written to
> disk, and it is dropped on exit, on Clear, and when Save History is
> turned off.

## Clear history

Click Clear to wipe the stored log.

> **Privacy:** If you turn off Save History in Settings, the existing log
> is purged and the file is removed, so nothing lingers on disk.

## Tagged by app

Each stored phrase records the app it was delivered to when known. That
tagging powers the top-apps breakdown in analytics and lets you search
history by app name.

## The analytics dashboard

The Analytics view summarizes your usage entirely from local history:
total words and phrases, words this week, your day streak, top apps, and
recent per-session stats like words per minute, duration, and average
latency. It also shows today, all-time, and peak-hour summaries, plus a
vocabulary card with your distinct word count and a richness score (the
share of unique words in your recent dictation).

## Day streak

The day streak counts consecutive days with at least one phrase, ending
today (or yesterday if you have not dictated yet today, so the streak is
not lost before the day is over). Dictate something each day to keep it
going.

## No analytics without history

Because every stat is derived from the history log, turning Save History
off means there is nothing to summarize.

> **Tip:** Leave history on if you want the dashboard, search, and
> learn-from-history to work.
