# Meetings

Meetings record both sides of a call: your microphone and the system audio your
speakers play. Murmur transcribes while the meeting runs and saves a transcript
you can label by speaker, summarize, and export. Everything stays on your
machine, and recording starts only when you press Start.

## Start and stop a meeting

Open the Meetings section on the Dictate view and click Start Meeting. A timer
and an audio badge show that recording is running, and the live transcript
grows as people talk. Click Stop to finish; the transcript is processed and
saved automatically to your meetings list.

## What gets recorded

A meeting captures your microphone and, on Windows, the system audio, which is
what the other participants say through your speakers. Both sides of a call
are transcribed without any bot joining the meeting. The badge next to the
timer reads "mic + system audio" when both are captured and "mic only" when
system audio is unavailable, such as on Linux or when the output device refuses
loopback capture. A meeting never fails just because system audio could not
start.

## The live transcript

Speech is transcribed in chunks of roughly twenty seconds while the meeting
runs, so text appears with a short delay rather than all at the end. Murmur
cuts each chunk at the quietest nearby moment instead of mid-word, and the
transcript is saved incrementally as it grows, so an interruption loses at most
the last chunk.

## Speaker labels

Click "Enable speaker labels" in the Meetings section to download the speaker
model (about 469 MB, one time). Once it is on disk, new meetings get
per-speaker transcripts: when a meeting stops, Murmur works out who spoke when
and labels the turns Speaker 1, Speaker 2, and so on. It distinguishes voices
but does not know anyone's name. Speaker labels are available on builds that
include the feature, and any problem simply produces a transcript without
labels rather than losing the meeting.

## Meeting summaries

On builds that include the local rewrite model, every saved meeting has a
Summarize button. Click it to generate a short summary on your machine; the
summary is stored with the record and included in the Markdown export. Nothing
is sent anywhere. The same local model that powers Rewrite selection writes the
summary.

## Saved meetings, export and delete

Finished meetings are listed newest first in the Meetings section. View shows
the transcript, with speaker labels when available. Export writes a Markdown
file next to the record, and Delete removes a meeting permanently after a
confirmation. Records live in your config directory under murmur/meetings, one
file per meeting. Meetings are your data, not a rolling log: clearing dictation
history or turning Save History off never touches them.

## Meetings and dictation

A meeting and a dictation session cannot run at the same time. While a meeting
is recording, starting dictation is refused so the meeting keeps the
microphone; stop the meeting to dictate again. Likewise, a meeting will not
start while you are mid-dictation.

## Meeting privacy

Meetings are processed locally like everything else in Murmur. Audio is
transcribed and discarded in memory, with one exception: speaker labels need
the whole meeting's audio at once, so with labels enabled the audio is spooled
to a temporary file during the meeting. That file is deleted the moment
processing starts, and also on any failure, crash, or the next launch, so no
recording is ever kept. Without speaker labels, meeting audio never touches
disk at all.
