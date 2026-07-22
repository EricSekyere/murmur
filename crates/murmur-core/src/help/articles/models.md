# Models, languages and translation

Murmur transcribes entirely on your machine using a speech model you choose. The
first time you pick a model it downloads once, then runs offline. You can switch
models any time in Settings.

## The default model

Parakeet TDT 0.6B v2 is the default. It is fast on the CPU, has the best
accuracy, and produces punctuation and capitalization on its own, so it is the
best choice for most people. It is English only and is about 661 MB to download.

## Whisper model variants

Whisper models are also available. Base (English) is the smallest and fastest but
least accurate. Small (English) is a good balance. Medium (English) is more
accurate but needs about 4 GB of RAM. Large v3 Turbo is the most accurate Whisper
option, the heaviest, and the only model that can translate to English.

## Switching models

Open Settings and pick a model card. If it is not downloaded yet, Murmur fetches
it and shows the size and progress. Your current session keeps working while the
new model loads, and new sessions wait until the swap is done.

## Speed and accuracy tradeoff

Smaller models are faster but less accurate; larger models are the reverse. On a
machine without a GPU, prefer Parakeet or a small Whisper model for low latency.
The medium and large Whisper models are really only practical with a GPU.

## GPU acceleration

Some builds of Murmur run Whisper models on your graphics card: CUDA builds use
NVIDIA GPUs, and Vulkan builds work on any modern GPU (NVIDIA, AMD, or Intel).
When a GPU build is running, Settings shows a note under the STT Model list
naming the backend.

Only Whisper models use the GPU. Parakeet always runs on the CPU, so with
Parakeet selected the GPU backend sits idle. To put your graphics card to work,
pick a Whisper model; the GPU is what makes the medium and large variants fast
enough for real-time dictation.

On Vulkan builds, the first phrase after launching the app can take a few extra
seconds while GPU shaders compile. That happens once per launch; every phrase
after it is fast.

## Languages

English only models (the Whisper .en models and Parakeet v2) always transcribe
English. Two models understand other languages: Whisper Large v3 Turbo honors
the Speech Language setting (or Auto-detect), and Parakeet v3 covers 25
European languages with automatic detection. Parakeet v3 always detects the
language itself, so the Speech Language setting has no effect on it. Murmur
ships language options for Spanish, French, German, and many more.

## Translate to English

Turn on Translate to English to speak any supported language and have English
typed out. This works only on Whisper Large v3 Turbo. English-only models and
Parakeet v3 ignore the toggle; Parakeet v3 always transcribes in the language
you spoke.

## Model and language mismatch

If a language or translation setting will not do what it says on the active
model, Murmur warns you. English-only models cannot transcribe other languages
(non-English speech would come out as garbled English), and Parakeet v3
ignores a forced Speech Language and the translate toggle. Switch to Large v3
Turbo for translation or a forced language.

## Unload the model when idle

The speech model stays in memory so dictation starts instantly, which can hold
hundreds of megabytes of RAM while you are not dictating. Set "Unload Model
When Idle" in Settings to free that memory after a period without dictation,
from five minutes up to a day. The model reloads automatically the next time
you dictate; the only cost is a short delay on the first phrase after a long
idle stretch. The default is Never, which keeps the model loaded.

## Where models are stored and verified

Downloaded models live in your app data folder under murmur/models. Every file is
checked against a pinned SHA256 checksum before use, so a corrupted, incomplete,
or tampered download is rejected and refetched rather than loaded.
