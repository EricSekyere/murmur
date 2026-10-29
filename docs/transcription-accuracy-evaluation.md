# Murmur transcription accuracy evaluation

Evaluated October 28, 2026. The priority is reliable everyday dictation: requests,
explanations, conversations, numbers, and longer thoughts. Owneli and Supabase
are two small regression cases, not the basis for choosing a model.

Murmur has reproducible accuracy problems outside the recognizer. Audio
preparation can change which words survive recognition; vocabulary correction
can replace correct ordinary words; and text filters discard legitimate short
replies. Address these alongside model evaluation. A larger model alone cannot
repair mistakes introduced after recognition.

**Current configuration and dictionary behavior**

The installed app is version 0.29.1, using Parakeet TDT 0.6B v3 with relaxed
transcription, prose cleanup enabled, and a 600 ms phrase pause. The inspected
recognition, preparation, correction, and preview source matches that tag.
Echo cancellation is enabled; its effective VAD threshold is at least 0.50,
even though the saved threshold is 0.25. The tests use 0.50 but bypass physical
microphone capture and echo cancellation.

The saved vocabulary contains Claude, Fresha, Owneli, X, waitlist, and no-show.
Supabase is absent from the saved file inspected during this evaluation. The
final transcription path loads the vocabulary, so this is not simply a case of
the whole dictionary being disconnected.

However, the current Parakeet integration passes only audio to its recognizer.
Its dictionary works afterward through spelling, phonetic, casing, and word
joining heuristics. Whisper additionally receives the vocabulary as decoder
context. These hints still do not guarantee correct recognition.

The distinction matters: a dictionary entry cannot teach this Parakeet decoder
how a name sounds. Its later correction also deliberately protects many common
words, which prevents some unwanted substitutions but leaves some intended
corrections unresolved. In these tests, the recognizer correctly produced
"a fresh approach" and the saved Fresha entry changed it to "a Fresha approach"
in both voices. Blindly strengthening fuzzy matching would make that worse.

Live preview does not run the same vocabulary correction as final output and
does not refresh vocabulary itself. The existing history learning mines
distinctive terms from delivered transcripts; it does not learn from paired
audio and user corrections. Project indexing is disabled in the saved settings.

Relevant implementation: [final transcription](../crates/murmur-app/src/transcribe.rs),
[recognizer integration](../crates/murmur-core/src/stt/engine.rs),
[vocabulary correction](../crates/murmur-core/src/vocab_correct.rs), and
[preview](../crates/murmur-app/src/preview.rs).

**Broad stress test**

The [corpus](../scripts/transcription-broad-phrases.json) contains 30 sentences,
spoken by both installed Windows voices, David and Zira: 60 synthetic recordings.
It covers everyday prose, negations, similar sounding words, numbers, long
instructions, restarts, short replies, technical terms, and dictionary collisions.
The completed matrix contains 1,108 scenario evaluations: 427 each for Parakeet
v3 and v2, 187 for Whisper Small, and 67 for Whisper Turbo. These reuse the same
recordings across conditions; they are not 1,108 independent speakers or phrases.

Each recording runs through actual Silero VAD, phrase splitting, audio
preparation, recognition, vocabulary correction, and final text filtering.
The harness records three text stages separately. "Raw" below means recognizer
output after VAD and audio preparation, not a model-only benchmark. The test
does not exercise live preview timing, typing into another app, or real acoustics.

The main word-error score excludes the three number sentences and two
disfluency sentences per voice: writing "15" for "fifteen" or intentionally
removing a restart should not inflate recognition errors. Those ten clips are
still tested and retained for separate inspection. The remaining 50 clips have
734 reference words. Scoring ignores punctuation and capitalization.

| Model, clean audio | Raw word errors | After dictionary | Final word errors | Final WER |
| --- | ---: | ---: | ---: | ---: |
| Parakeet v3, currently selected | 10 | 12 | 18 | 2.45% |
| Parakeet v2 | 5 | 7 | 13 | 1.77% |
| Whisper small.en | 5 | 7 | 12 | 1.63% |
| Whisper large-v3-turbo | 5 | 7 | 13 | 1.77% |

Removing the two named-product sentences as well leaves 46 clips and 688 words.
Raw/final errors are 6/14 for v3, 0/8 for v2, 3/10 for Whisper Small, and 3/11
for Whisper Turbo. This keeps the general-language comparison separate from the
example names. Whisper Small's raw count includes one harmless "OK"/"Okay"
variant; inspect actual output as well as the score. The small, synthetic corpus
does not establish personal accuracy or a universal model ranking; v2 is a
worthwhile English comparison, not an automatic upgrade.

Both Parakeet versions ran all seven audio conditions. Each row below uses the
same 734 reference words; percentages are final-output word error rates.

| Audio condition | Parakeet v3 | Parakeet v2 |
| --- | ---: | ---: |
| Clean | 2.45% | 1.77% |
| 20% amplitude | 2.59% | 1.77% |
| 5% amplitude | 2.45% | 1.77% |
| 2% amplitude | 2.32% | 1.77% |
| 1% amplitude | 51.23% | 50.95% |
| Added deterministic noise | 2.59% | 1.77% |
| Leading and trailing silence | 1.77% | 1.77% |

Whisper Small ran clean, 20% amplitude, and added-noise conditions, with final
WER of 1.63% in each. Whisper Turbo ran clean speech only. The unequal condition
coverage must not be read as evidence that either Whisper model matches
Parakeet on the extreme-amplitude cases.

At 1% amplitude, 32 of 60 clips have at least one rejected phrase for each
Parakeet model. Most rejections are the audio admission floor, not word
substitutions. Scaling synthesized speech is not equivalent to whispering or
speaking softly into a microphone, so it identifies a threshold boundary
without establishing that the threshold should simply be lowered.

Seven additional synthetic non-speech clips cover silence, noise at three
levels, hum, a tone, and clicks. Each is three seconds long. They all produce
no text in the completed model runs. Repeating these same clips across models
does not establish a false-insertion rate for real rooms.

**Failures that a model switch does not fully solve**

1. **Audio preparation affects recognition.** With Parakeet v3, both voices
   lose "for getting" from "Thanks for getting back to me" in the broad clean
   run. A separate controlled comparison bypasses VAD and text cleanup: the
   complete clean clip is correct in both voices, while current preparation
   reproduces the omission. This demonstrates sensitivity to preparation; it
   does not show that trimming physically removed those internal words.
   At lower amplitudes, untrimmed audio does not consistently fix the omission.
2. **Dictionary correction damages correct speech.** The Fresha substitution
   adds two errors per clean model run. Preserve ordinary words and evaluate
   explicit, user-reviewed aliases separately from speculative fuzzy matches.
3. **Legitimate short replies disappear.** "Okay" and "Thank you" are recognized
   in both voices (including the spelling "OK"), then rejected by the exact-text
   hallucination filter: four lost clips and six missing reference words per
   clean model run. Replace unconditional rejection with a decision using speech
   evidence, while
   checking real noise and breath recordings for false output.

The clean v3 run grows from 10 raw errors to 18 delivered errors. Eight are
introduced after recognition. Neither an external API nor a larger local model
can prevent those eight if its transcript passes through the same logic.

**Model and provider options**

Murmur already supports local Whisper models. The first comparison should use
the installed Parakeet v2 and Whisper Turbo on identical human recordings,
including real microphone and pause behavior. This evaluation used a CPU
Whisper test build, with other checks running concurrently; its elapsed times
are unsuitable for ranking production GPU latency. The machine has an RTX 4050
laptop GPU with roughly 6 GB VRAM, so memory and latency need measurement before
keeping multiple larger models loaded.

OpenAI currently recommends `gpt-transcribe` for recorded speech. Its API accepts
context, keyword hints, and language hints, which provides a more direct route
for vocabulary guidance. These remain hints, not forced substitutions.
[OpenAI file transcription documentation](https://developers.openai.com/api/docs/guides/speech-to-text).
The listed price is $0.0045 per audio minute, or $0.27 per hour by multiplication.
It also supports Realtime input transcription.
[GPT-Transcribe model documentation](https://developers.openai.com/api/docs/models/gpt-transcribe).
This is a candidate for an explicit opt-in cloud mode, with a feature flag and
credentials in the platform keyring. It was not used in the local evaluations.
An API model name does not establish the exact backend of every ChatGPT voice UI.

Anthropic's Claude Code documentation says dictation streams audio to its
servers and adds project and branch names as recognition hints. It does not
identify the underlying speech model, so there is no basis here to claim it uses
Whisper or another particular recognizer. The transferable idea is providing
relevant context during recognition.
[Anthropic voice dictation documentation](https://code.claude.com/docs/en/voice-dictation).

Qwen3-ASR 1.7B is a local candidate worth evaluating: its Apache 2.0 model supports
multiple languages and English accents, and the official inference code accepts
context. It would require a new backend and measured memory, latency, and
accuracy; it is not an evaluated replacement in Murmur.
[Qwen model card](https://huggingface.co/Qwen/Qwen3-ASR-1.7B),
[Qwen context input implementation](https://github.com/QwenLM/Qwen3-ASR/blob/main/qwen_asr/inference/qwen3_asr.py).

**Recommended implementation order**

1. Retain enough original audio in memory to compare conservative preparation
   and phrase boundaries. Evaluate an untrimmed final pass without removing
   speech admission checks. Check first and last words, pauses, negations, and
   quiet speech independently.
2. Fix incorrect dictionary substitutions and short-reply rejection. Make
   preview and final vocabulary behavior consistent. Add reviewed correction
   pairs and scoped aliases rather than treating every edit as a global rule.
3. Add an explicit local "save this mistake" action that retains a clip and its
   corrected transcript for evaluation. Use a held-out human set covering
   ordinary conversation, varied pace, actual background noise, and the user's
   microphone. Track missed phrases, changed negations, numbers, unwanted
   corrections, false output on non-speech, and correction time alongside WER.
4. Compare the existing local models on that set, then evaluate Qwen and an
   optional OpenAI mode. A fast preview plus a more accurate final pass is a
   viable design, but should not type provisional text that later needs repair.
   Parakeet's current wrapper exposes no token confidence, so a confidence-based
   fallback first needs real confidence support or another tested trigger.

Do not add a general language-model rewrite as the default accuracy fix. It can
make a wrong transcript read fluently while changing what was actually said.
Keep optional stylistic cleanup distinct from faithful transcription.

**Reproduction and validation**

From the repository root, with the models and ONNX runtime already installed:

```powershell
./scripts/transcription-fixtures.ps1 `
  -OutputDirectory target/transcription-broad-2026-10-28 `
  -PhraseManifest scripts/transcription-broad-phrases.json
$env:ORT_DYLIB_PATH = "$env:APPDATA/murmur/onnxruntime/onnxruntime.dll"
$env:MURMUR_STRESS_MANIFEST = "$PWD/target/transcription-broad-2026-10-28/manifest.json"
$env:MURMUR_STRESS_VAD = "$env:APPDATA/murmur/vad/silero_vad.onnx"
$env:MURMUR_STRESS_MODEL = "$env:APPDATA/murmur/models/parakeet-tdt-0.6b-v3"
$env:MURMUR_STRESS_BACKEND = 'parakeet-v3'
$env:MURMUR_STRESS_VOCABULARY = '["Claude","Fresha","Owneli","X","waitlist","no-show"]'
$env:MURMUR_STRESS_CONDITIONS = 'clean,quiet,amplitude_5_percent,amplitude_2_percent,amplitude_1_percent,noise,padded'
$env:MURMUR_STRESS_REPORT = "$PWD/target/transcription-broad-2026-10-28/parakeet-v3.json"
cargo test --offline -p murmur-app --lib transcribe::stress::stress_local_transcription -- --ignored --exact
./scripts/transcription-summary.ps1 -ReportDirectory target/transcription-broad-2026-10-28
```

Change the backend, model path, and report filename together. Parakeet v2 uses
`parakeet-v2` and directory `parakeet-tdt-0.6b-v2`; Whisper Turbo uses
`whisper-turbo` and file `ggml-large-v3-turbo.bin`. Its completed evaluation uses
`clean` only. Whisper Small uses `whisper-small`, file `ggml-small.en.bin`, and
conditions `clean,quiet,noise`. The harness also accepts `whisper-base` and
`whisper-medium`. Model files are never downloaded by these tests.

Raw reports and audio remain under ignored
`target/transcription-broad-2026-10-28/`. The isolated preparation comparison
uses `email-manifest.json`, `email-prepared.json`, and `email-untrimmed.json`;
its existing runner is documented in [the quiet-speech audit](quiet-speech-regression.md).
An evaluation test passing means it completed and wrote observations, not that
every phrase was correct.

Formatting, whitespace checks, and workspace Clippy passed. The standard
workspace suite passed 836 tests with 13 explicitly ignored. The
all-features check was attempted but failed before tests in whisper.cpp's Vulkan
shader-generator build: MSBuild FileTracker FTK1011 could not create tracking
files in deeply nested build paths. No all-features pass is claimed. The standard
app build used here already enables Whisper, Parakeet, and Silero.

Changes in this evaluation are the corpus, fixture options, stage-level report
fields, model/condition selection, and report summarizer. Production recognition
behavior and installed settings have not been changed; the fixes above remain
proposed work.
