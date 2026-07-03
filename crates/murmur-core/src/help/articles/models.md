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
option, the heaviest, and the only multilingual model.

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

English only models (the Whisper .en models and Parakeet) always transcribe
English. To dictate another language, switch to the multilingual model (Large v3
Turbo) and set your Speech Language in Settings, or leave it on Auto-detect.
Murmur ships language options for Spanish, French, German, and many more.

## Translate to English

Turn on Translate to English to speak any supported language and have English
typed out. This works only on the multilingual model. With an English-only model
selected, the translate toggle has no effect.

## Model and language mismatch

If you set a non-English language or turn on translation while an English-only
model is selected, Murmur warns you. The English models cannot transcribe other
languages, so non-English speech would otherwise come out as garbled English.
Switch to Large v3 Turbo to fix it.

## Where models are stored and verified

Downloaded models live in your app data folder under murmur/models. Every file is
checked against a pinned SHA256 checksum before use, so a corrupted, incomplete,
or tampered download is rejected and refetched rather than loaded.
