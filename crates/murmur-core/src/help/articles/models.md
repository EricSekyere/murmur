# Models

Murmur transcribes entirely on your machine using a speech model you choose. The
first time you pick a model it downloads once, then runs offline.

## Choosing a model

Parakeet is the default: fast on the CPU, good accuracy, and built in
punctuation, so it is the best choice for most people. Whisper models are also
available and are multilingual; the small and base English models are quick,
while large is the most accurate but the heaviest.

## Switch models

Open Settings and select a model card. If it is not downloaded yet, Murmur fetches
it and shows the size and progress. Your current session keeps working while a new
model loads.

## Languages and translation

English only models always transcribe English. To dictate another language, pick a
multilingual Whisper model and set the language in Settings, or turn on Translate
to English to speak any language and have English typed.

## Speed and accuracy

Smaller models are faster but less accurate; larger models are the reverse. On a
machine without a GPU, prefer Parakeet or a small Whisper model for low latency.
Live preview (the in progress caption) only runs on backends fast enough for it.

## Where models are stored

Downloaded models live in your app data folder and are verified against a pinned
checksum before use, so a corrupted or tampered download is rejected.
