# Diagnostics

The Diagnostics view shows what Murmur hears and why phrases are accepted or
rejected. Use it to confirm your microphone is working and to understand dropped
sessions.

## Signal levels

The Signal section shows your live and peak input level (RMS) while you speak. If
these stay near zero when you talk, Murmur is not receiving audio, so check your
input device, mute switch, and input volume.

## Accepted and rejected counts

Murmur tallies how many phrases it accepted and delivered versus how many it
rejected. A few rejections are normal: silent or noise-only audio is dropped on
purpose. A high reject count while you are clearly speaking points to a mic or
threshold problem.

## Rejection reasons

Rejections are grouped so you can see the cause. Too Short means the utterance was
too brief to transcribe. Too Quiet means it fell below the speech threshold.
Hallucination means the output looked like a known noise artifact and was
filtered. Engine covers transcription errors or empty results. No Signal means no
audio came through. Other catches anything left, such as text that became empty
after cleanup.

## Reset counters

Click Reset Counters to zero the accepted, rejected, and reason tallies. This is
handy when testing a change, like a new microphone or a different sensitivity, so
you measure only what happens next.

## Signal-to-noise and latency

Recognition is best with a clean signal and low latency. A clean signal comes from
a good microphone in a quiet space; high latency usually means a heavy model on a
CPU-only machine, where switching to a smaller model (or Parakeet) speeds things
up.
