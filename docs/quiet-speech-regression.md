# Quiet-speech regression audit (2026-09-28)

The fixed RMS floor introduced by transcription hardening could discard usable
quiet speech. Lower volume also made boundary trimming remove words that the
same Parakeet model recognized from the complete clip. This is a demonstrated
preparation defect; it does not explain every reported substitution or establish
that close-mic speech is now reliable.

## Reproduction

Ten synthetic speech clips (five sentences in Windows David and Zira voices)
were decoded with the locally installed Parakeet v3 model at five amplitudes.
One sentence is "What are some processes I can close out?". The test compares
frozen pre-hardening relaxed preparation, current preparation, and an untrimmed
control with the same capped normalization. It bypasses VAD and text cleanup to
isolate preparation from other causes. Each amplitude has 88 reference words.

At 1% amplitude, pre-hardening preparation admitted all ten clips with one word
error; version 0.28.0 rejected all ten as `too_short_trimmed`. The untrimmed
control also had one word error. At 2%, the current trimmer dropped "out" from
the Zira sentence while the untrimmed control preserved it. Neither synthetic
voice reproduced the reported substitution of "call out" for "close out".

The original audit's quiet condition was 20% amplitude. A quieter synthetic
voice does not reproduce human soft speech, whispering, microphone placement,
or Windows voice capture's noise suppression and gain processing. Local history
contains text rather than labeled source audio and cannot establish accuracy.

## Change

Trimming uses the smaller of the profile's absolute threshold and 10% of the
loudest 32 ms frame's RMS. It preserves 160 ms of context at each boundary.
The separate raw peak/RMS admission floors, two-audible-frame requirement,
minimum durations, non-finite rejection, and normalization cap stay in place.
Live preview and final transcription both use this preparation function.

This separates deciding where to trim from deciding whether to decode a clip.
Quiet words need not reach a fixed amplitude to remain attached to an otherwise
usable phrase. Silence and isolated clicks still cannot be promoted into speech
by the longer padding. Amplitude alone is not proof of speech; VAD remains
necessary, and these checks do not guarantee rejection of arbitrary noise.

| Relative amplitude | 0.28.0 word errors / 88 | Updated word errors / 88 | Rejected clips before / after |
| --- | ---: | ---: | ---: |
| 100% | 3 | 3 | 0 / 0 |
| 20% | 5 | 3 | 0 / 0 |
| 5% | 5 | 4 | 0 / 0 |
| 2% | 12 | 3 | 0 / 0 |
| 1% | 88 | 45 | 10 / 5 |

Both voices preserve "close out" through 2% amplitude after the change. At 1%,
the David clip survives but the Zira clip still fails the raw peak threshold.
Five clips at that level remain rejected as `too_quiet`. This is a partial
recovery of faint speech, not restoration of all pre-hardening sensitivity.
Reducing those admission floors needs representative speech and room-noise
recordings to measure the false-insertion tradeoff; this change leaves them
intact. None of these synthetic results establish real-microphone accuracy.

## Full pipeline results

The expanded run completed 161 cases with Parakeet v3: 22 clips across seven
speech conditions and seven non-speech clips. All seven non-speech clips were
blocked by Silero and produced no text (21 seconds total, not a real-room false
insertion rate). The "close out" sentence was correct in 13 of 14 combinations;
the remaining 1%-amplitude Zira clip was rejected as `too_quiet`.

| Condition | Word errors / 132 | WER |
| --- | ---: | ---: |
| Clean | 9 | 6.82% |
| 20% amplitude | 8 | 6.06% |
| 5% amplitude | 10 | 7.58% |
| 2% amplitude | 9 | 6.82% |
| 1% amplitude | 70 | 53.03% |
| Added noise | 10 | 7.58% |
| Leading/trailing silence | 7 | 5.30% |

These rates include the existing blacklist's false rejection of real "Okay"
and "Thank you" replies. At 1% amplitude, 12 clips hit the raw signal floor and
one hit the blacklist. Identifier formatting succeeded in 52 of 56 cases;
the four misses were at 1% amplitude. This run exercises VAD and delivery text
processing, but still excludes physical capture, preview timing, and keystrokes.

## Running the comparison

Generate fixtures with `scripts/transcription-fixtures.ps1`. They now include
11 sentences per installed Windows voice. Set the same explicit local manifest,
model, VAD, and report paths described in the original audit. Then run:

```powershell
cargo test --offline -p murmur-app --lib compare_quiet_preparation -- --ignored
```

The comparison selects non-identifier sentences longer than two words. Set
`MURMUR_STRESS_PREPARATION` to `current`, `pre_hardening`, or `untrimmed` to run
one path; omit it to compare all three. Use separate `MURMUR_STRESS_REPORT`
filenames before and after a source change. Models are never downloaded.
Reports contain fixture text and are written only to the explicit local path.

`stress_local_transcription` additionally exercises Silero at 0.50, the phrase
splitter, vocabulary correction, and text cleanup. Its conditions now include
5%, 2%, and 1% amplitude as well as the original clean, 20%, added-noise, and
padded cases. It also runs seven synthetic non-speech clips. An evaluation test
passing means it completed; inspect its word errors and rejections separately.

## Validation

- Formatting, whitespace checks, and `cargo clippy --offline --workspace
  --all-targets -- -D warnings` passed.
- `cargo test --offline --workspace`: 832 passed, 0 failed, 11 explicitly ignored.
- The direct preparation comparisons completed 150 before-change and 50
  after-change observations. The eight preparation unit tests passed.
- The expanded full-pipeline evaluation completed all 161 observations;
  accuracy and rejection results are reported above.
- `cargo test --offline --workspace --all-features` could not reach the tests:
  the optional `llama-cpp-sys-2` CMake/MSBuild install step failed with exit code
  1, as in the previous audit. The default app features include the actual
  Whisper, Parakeet, and Silero paths; no all-features pass is claimed.

The installed application and its settings were not replaced or changed.
