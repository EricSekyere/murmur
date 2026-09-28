# Transcription quality audit (2026-09-27)

Murmur supports local dictation and explicit developer formatting commands. It
cannot guarantee exact recognition, arbitrary speech-to-program conversion, or
zero hallucinations. Formatter correctness and recognition accuracy are different
properties and need separate tests.

The [quiet-speech follow-up](quiet-speech-regression.md) found false rejections
and clipped words missed by this initial audit. In particular, quieter synthetic
speech at 20% amplitude did not cover the new trimmer's failure at lower levels.
The original measurements below are historical, not a claim that the hardening
was free of recognition regressions.

## Website claims

Reviewed https://www.ericsekyere.ca/projects/murmur on 2026-09-27.

- The identifier demo represents implemented Rust formatting rules. The page
  correctly discloses that its JavaScript demo starts after recognition and does
  not use a microphone. It is not an acoustic accuracy demonstration.
- Identifier, symbol, path, and commit-message features exist, but require their
  documented modes/grammar and correct recognition of the spoken command.
- Project vocabulary is optional. Whisper can bias its decoder; the current
  Parakeet integration instead applies vocabulary corrections after recognition.
- Local processing is supported by the implementation. The page acknowledges
  downloads and update checks. Model/runtime downloads are prerequisites for
  offline use.
- The advertised first speech-model download of about 490 MB is stale: the
  configured default is Parakeet v2, whose installed model files total about
  661 MB (runtime downloads are additional).
- Universal accuracy, arbitrary code generation, and uniformly low latency are
  not established by the implementation or this evaluation. The PRD's accuracy
  and 300 ms targets are requirements, not measured guarantees.

Suggested copy:

> Dictate text locally and use explicit voice commands to format identifiers,
> symbols, and paths. Accuracy varies with the model, voice, microphone, and
> background noise. Review dictated code before using it.

The public website was not modified in this repository.

## Local-install inspection

At inspection, the install used Parakeet v3, relaxed transcription, English,
600 ms phrase pauses, echo cancellation, and speech cleanup. Developer mode and
project indexing were disabled. The saved VAD threshold was 0.25, but the
installed echo-cancellation path raises the effective threshold to 0.50; logs
confirmed this. Changing the saved threshold alone would not describe the
actual speech gate.

The history contained 500 recent entries. History holds delivered text, not
labeled source audio, so it cannot measure the user's word error rate or prove
which phrases were hallucinations. No personal transcript contents are included
in this report or the fixtures. Recent logs showed no inference errors in the
three days inspected; this is not evidence that the words were correct.

## Reproduced defects and changes

1. The old trimmer returned the whole clip when no frame exceeded its threshold.
   A constant 0.002-level noise input therefore survived. No audible frames now
   yields no audio.
2. The old signal gate measured after up to 5x amplification, letting below-floor
   inputs pass. Peak and RMS checks now use the signal before normalization.
3. Trimming stopped at the last loud frame and could remove a quieter word
   ending. It now retains 64 ms on both ends. At least two audible frames are
   required so added context does not promote an isolated click into a phrase.
4. Live preview skipped signal and decoder-quality checks. It now shares those
   checks with final delivery.
5. Hallucination checks ran after developer formatting, rejecting valid spoken
   punctuation or parenthesized code. They now inspect the recognized words
   before formatting. Raw noise annotations remain rejected.

6. The build without default features exposed an existing file-transcription
   error: its pure-Rust windowing helper was incorrectly gated on Parakeet. The
   helper is now available independently of the native inference features.
7. The acoustic tests found that Whisper can emit joined style words such as
   "CamelCase" and "Snakecase". The command parser now accepts these spellings;
   regression tests use the actual recognized phrases.

Three isolated regression tests against the original audio functions failed;
these cover no-audible-frame handling, amplification admitting noise, and
preserving a quiet word ending. The new checks also cover invalid floating-point
samples, quiet usable speech, clicks, preview confidence, and spoken symbols.

## Repeatable local evaluation

The opt-in test `transcribe::stress::stress_local_transcription` uses installed
models without downloading anything. It feeds 16 kHz audio in 50 ms chunks through
Silero (0.50), the real phrase splitter (600 ms pause), audio preparation,
recognition, vocabulary correction, speech cleanup, and final rejection checks.
It separately applies the real command identifier parser to delivered text.

The fixture script uses Windows' offline David and Zira voices: 10 phrases per
voice, with ordinary prose, negation, technical terms, names, four casing
commands, and short replies. Each is tested clean, at 20% amplitude, with seeded
white noise of amplitude 0.025, and with leading/trailing silence. Seven additional
three-second signals cover silence, three noise levels, hum, a tone, and clicks.
That is 87 cases per model, including 32 identifier cases. The glossary is fixed
to Claude and Fresha to match the inspected install.

Word error rate counts word substitutions, insertions, and deletions, ignoring
case and punctuation. It is measured on delivered text, so intentional cleanup
and false rejections count as errors. Each speech condition contains 116
reference words. These are synthetic smoke tests, not a representative benchmark
of accents, microphones, rooms, meetings, or long dictation. The tests do not
exercise physical capture, echo cancellation, live preview timing, keystroke
insertion, or command execution. Timing from this debug build is not a product
latency benchmark. A successful test run means the evaluation completed; inspect
the report to assess accuracy.

| Model | Clean WER | Quiet WER | Added noise WER | Padded WER | Correct identifiers | Noise-only output |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Parakeet v3 | 7.76% | 9.48% | 8.62% | 8.62% | 32/32 | 0/7 |
| Parakeet v2 | 5.17% | 5.17% | 8.62% | 5.17% | 31/32 | 0/7 |
| Whisper small.en | 10.34% | 10.34% | 10.34% | 10.34% | 24/32 before parser fix | 0/7 |

All three models lose genuine standalone "Thank you" and "Okay" to the existing
phrase blacklist: 16 rejected speech cases per model across the conditions.
This is a known tradeoff, not a model-recognition failure. Removing the blacklist
without testing breaths, sighs, music, and real room noise could restore unwanted
text; this audit leaves that policy in place. V3 also misrecognized a technical
phrase in clean audio; V2 missed one identifier with added noise. These results
do not justify declaring any model universally better. Whisper recognized
"CamelCase" and "Snakecase" as joined words, causing eight command failures
across the four conditions. Unit tests now format those observed spellings
correctly; the acoustic run was not repeated after that parser-only fix. Its
remaining clean technical error split "TypeScript" into two words.

The three completed runs comprise 261 cases. All 21 noise-only cases were
blocked by VAD; this is only 63 seconds of synthetic non-speech, not an estimate
of false insertions per hour in real rooms.

### Reproduction on Windows

From the repository root:

```powershell
./scripts/transcription-fixtures.ps1
$env:MURMUR_STRESS_MANIFEST = "$PWD/target/transcription-stress/manifest.json"
$env:MURMUR_STRESS_VAD = "$env:APPDATA/murmur/vad/silero_vad.onnx"
$env:MURMUR_STRESS_MODEL = "$env:APPDATA/murmur/models/parakeet-tdt-0.6b-v3"
$env:MURMUR_STRESS_BACKEND = 'parakeet-v3'
$env:MURMUR_STRESS_REPORT = "$PWD/target/transcription-stress/parakeet-v3.json"
cargo test -p murmur-app --lib transcribe::stress::stress_local_transcription -- --ignored --exact
```

For V2, change the backend to `parakeet-v2`, model directory to
`parakeet-tdt-0.6b-v2`, and report filename. For Whisper small, use backend
`whisper-small` and model file `ggml-small.en.bin`. All paths must already exist.
Speech synthesis requires access to installed Windows voices. Fixtures/reports
stay in ignored `target/`; personal evaluation audio should also remain local.

A custom manifest is a JSON array of `{ "id": "example", "file": "example.wav",
"expected": "verified spoken words", "identifier": "optionalExpectedIdentifier" }`.
Files resolve relative to the manifest. Omit `identifier` for prose. Use separately
recorded, human-checked speech for any release accuracy claim; retain a held-out
set to avoid tuning exclusively to these synthetic phrases.

The current Parakeet wrapper exposes neither token confidence nor no-speech
probabilities. The confidence filters available for Whisper therefore do not
operate on Parakeet output. A stricter profile cannot create that missing
evidence or guarantee correct technical words.

## Model candidates

Before adding a backend, compare exact technical identifiers, false insertions
on non-speech, missed words, short replies, and phrase latency on the same local
recordings and hardware.

- [Qwen3-ASR 1.7B](https://huggingface.co/Qwen/Qwen3-ASR-1.7B): Apache 2.0;
  supports context vocabulary, multilingual recognition, and varied English
  accents. A candidate to evaluate, not an established Murmur improvement.
- [Voxtral Realtime](https://mistral.ai/news/voxtral-transcribe-2/): Apache 2.0
  open weights and a native streaming architecture; 4B parameters make local
  memory and latency important integration measurements.

Neither backend is implemented or evaluated here. A new model would not replace
correct capture, phrase boundaries, or conservative output handling.

## Validation

- `cargo fmt --all --check`, `git diff --check`, and
  `cargo clippy --workspace --offline --all-targets -- -D warnings` passed.
- Standard workspace suite: 828 passed, 10 explicitly ignored. The new joined
  casing-command regression is included in this run.
- Workspace without default features: 802 passed, 4 explicitly ignored after
  fixing the windowing helper's feature gate. This run preceded the final
  joined-spelling parser addition, which was checked in the standard suite.
- The model evaluations are three separate, explicitly invoked ignored tests;
  their successful completion is not an assertion of perfect transcription.
- `cargo test --workspace --all-features --offline` was attempted but did not
  reach the tests: the optional `llama-cpp-sys-2` CMake/MSBuild install step exited
  with code 1. No all-features pass is claimed. The standard app build already
  enables Whisper, Parakeet, and Silero.

The fixes are source changes. This audit did not replace or restart the installed
app, modify its settings, publish a release, or change the public website.
