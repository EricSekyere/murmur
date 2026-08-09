# Models, languages and translation

Murmur transcribes entirely on your machine using a speech model you
choose. The first time you pick a model it downloads once, then runs
offline. You can switch models any time in Settings.

## The models at a glance

| Model | Download | Notes |
| --- | --- | --- |
| Parakeet TDT 0.6B v2 (default) | 661 MB | Best accuracy, fast on CPU, native punctuation and capitalization; English only |
| Parakeet TDT 0.6B v3 | 670 MB | Same engine, 25 European languages with automatic detection |
| Whisper Base (English) | 148 MB | Smallest and fastest, least accurate |
| Whisper Small (English) | 488 MB | Good balance of speed and accuracy |
| Whisper Medium (English) | 1.5 GB | More accurate, slower; needs 4 GB+ RAM |
| Whisper Large v3 Turbo | 1.6 GB | Most accurate Whisper option, the only model that can translate to English; needs 6 GB+ RAM |

## Switching models

1. Open Settings and pick a model card.
2. If it is not downloaded yet, Murmur fetches it and shows the size and progress.
3. Your current session keeps working while the new model loads; new sessions wait until the swap is done.

## Speed and accuracy tradeoff

Smaller models are faster but less accurate; larger models are the
reverse. On a machine without a GPU, prefer Parakeet or a small Whisper
model for low latency. The medium and large Whisper models are really
only practical with a GPU.

## GPU acceleration

Some builds of Murmur run Whisper models on your graphics card: CUDA
builds use NVIDIA GPUs, and Vulkan builds work on any modern GPU (NVIDIA,
AMD, or Intel). When a GPU build is running, Settings shows a note under
the STT Model list naming the backend.

Only Whisper models use the GPU. Parakeet always runs on the CPU, so with
Parakeet selected the GPU backend sits idle. To put your graphics card to
work, pick a Whisper model; the GPU is what makes the medium and large
variants fast enough for real-time dictation.

> **Note:** On Vulkan builds, the first phrase after launching the app can
> take a few extra seconds while GPU shaders compile. That happens once
> per launch; every phrase after it is fast.

## Languages

| Model | Other languages | Speech Language setting |
| --- | --- | --- |
| Whisper .en models, Parakeet v2 | No, English only | Ignored |
| Parakeet v3 | 25 European languages | Ignored (always auto-detects) |
| Whisper Large v3 Turbo | Yes | Honored, including Auto-detect |

Murmur ships language options for Spanish, French, German, and many more.

## Translate to English

Turn on Translate to English to speak any supported language and have
English typed out. This works only on Whisper Large v3 Turbo.
English-only models and Parakeet v3 ignore the toggle; Parakeet v3 always
transcribes in the language you spoke.

## Model and language mismatch

If a language or translation setting will not do what it says on the
active model, Murmur warns you. English-only models cannot transcribe
other languages (non-English speech would come out as garbled English),
and Parakeet v3 ignores a forced Speech Language and the translate
toggle. Switch to Large v3 Turbo for translation or a forced language.

## Unload the model when idle

The speech model stays in memory so dictation starts instantly, which can
hold hundreds of megabytes of RAM while you are not dictating. Set
"Unload Model When Idle" in Settings to free that memory after a period
without dictation, from five minutes up to a day. The model reloads
automatically the next time you dictate; the only cost is a short delay
on the first phrase after a long idle stretch. The default is Never,
which keeps the model loaded.

## Where models are stored and verified

Downloaded models live in your app data folder under murmur/models. If a
download is interrupted, the part already fetched is kept, and the next
attempt resumes where it left off instead of starting over.

> **Privacy:** Every file is checked against a pinned SHA256 checksum
> before use, so a corrupted, incomplete, or tampered download is rejected
> and refetched rather than loaded. A file without a pinned checksum is
> never downloaded at all; there is no unverified path.
