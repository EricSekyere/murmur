# Hey Murmur training pipeline

Licence-gated synthetic-data training for the always-listening wake head.
Adapts [openWakeWord](https://github.com/dscripka/openWakeWord)'s procedure
(Piper TTS positives → noise/RIR augmentation → classifier on a frozen
Apache-2.0 backbone) into scripts. **Upstream CC BY-NC-SA pre-trained heads
("Hey Jarvis", "Alexa", …) are never downloaded or used.**

This directory is a `uv` project. The rest of Murmur uses `uv` for Python;
do not call bare `python` / `pip`.

## Licence audit is a gate

`allowlist.toml` is the only permitted input set. Every Piper voice and every
negative/background/RIR dataset has an **individual** licence string.
Allowed: MIT, Apache-2.0, CC0, CC-BY. **NC and SA voices are excluded.**
ACAV100M-derived material is excluded by default (that corpus is why
upstream heads are CC BY-NC-SA).

`train.py` fails closed if:

- the allowlist is empty
- a voice or dataset is missing its licence
- an input id is not on the allowlist
- a path/URL names a forbidden upstream head

## Setup

```bash
cd training/wake-word
uv sync --group dev          # tests / licence audit
uv sync --extra train --group dev   # full training stack (torch, piper-tts, onnxruntime)
```

## Validate the allowlist (cheap)

```bash
uv run python train.py --validate-only
uv run pytest
```

## Train (hours; large downloads)

Place allowlisted corpora under `data/<id>/` first. Do **not** substitute
ACAV100M or the MIT IR Survey.

| Directory | Dataset | Licence | Source |
|---|---|---|---|
| `data/musan/` | MUSAN noise/music | CC-BY-4.0 | https://www.openslr.org/17/ |
| `data/rirs_noises/` | OpenSLR RIR+noise | Apache-2.0 | https://www.openslr.org/28/ |
| `data/librispeech/` | LibriSpeech (background, FLAC) | CC-BY-4.0 | https://www.openslr.org/12/ |

WAV and FLAC under those directories are ingested (`soundfile` for FLAC). Do
not convert the corpus first.

Then:

```bash
uv run --extra train python train.py --output-dir ./output --data-dir ./data
```

Background audio is streamed with a cap (`--max-background-hours`, default
20; `--max-negative-windows`, default 200000). The pipeline will not load
`train-other-500` (~500 h) into RAM, and the augmentation pools are sampled
rather than read whole (loading them cost 36 GB of float32).

This generates "Hey Murmur" with allowlisted Piper voices, augments with
allowlisted noise/RIR, trains the head, and writes `output/hey_murmur.onnx`.

## Two splits hold the gates up

**Speakers.** The allowlist carries 1937 speaker slots over 1033 distinct
people, so training holds out *people*, not voices. `en_US-libritts-high` and
`en_US-libritts_r-medium` render the same 904 LibriTTS people under different
labels and, for 185 of them, under different `speaker_id` slots, so
`speakers.py` normalises identities before splitting and refuses a split where
one person reaches two parts. Clips land in
`output/clips/<split>/<voice>/<speaker>/`, and a corpus left over from an
earlier split is refused rather than scored.

**Background.** LibriSpeech speakers are split too (`background.py`): training
draws negatives from one half, the false-accept gate counts them on the other,
and the training order interleaves speakers so the window cap yields a broad
sample instead of the first few speaker directories.

The head trains with dropout, decoupled weight decay and softened targets, and
stops on a validation split of held-back speakers, keeping the
best-validation weights. Without that it converged to exactly 1.0000 and
0.0000 and no threshold could trade recall against false accepts.

A second pass then trains against **hard negatives**: training-half background
the first pass scored like the phrase, fed back through the same augmentation
as every other negative. Separating the classes was not enough on its own,
because a thin tail of background outranked the whole positive band.

## Evaluate

```bash
uv run --extra train python evaluate.py --output-dir ./output --data-dir ./data
```

### Calibration and measurement are separate halves

A threshold chosen on the data that then reports on it is not measured, it is
fitted. The old protocol drew candidate thresholds from the held-out positives
and the gate background, picked the loosest threshold whose rate on *those same
background scores* fitted the budget, and published that rate: the best of ~126
tries on the sample it was scored against.

So the run has two halves and nothing crosses between them:

| Half | Positives | Background | Decides |
|---|---|---|---|
| Calibration | `clips/validation` (123 speakers) | validation split, ~7.4 h | the Low / Medium / High thresholds |
| Measurement | `clips/held_out` (155 speakers) | gate split, ~46 h | the reported recall and false accepts |

Selection uses the validation **point estimate**, because 7.4 h resolves a rate
only to 0.135/hour and an interval-based rule there would accept nothing but a
zero-event threshold. The interval is applied where the claim is made, on the
held-out measurement.

Each named point is still the most sensitive threshold inside its budget (Low
0.1/h, Medium 0.5/h, High 2.0/h), with candidates drawn from the measured
positive scores and the background's upper tail as well as a fixed grid. A
fixed [0.05, 0.95] grid collapsed Low and Medium onto one threshold whenever
the head's scores sat outside it.

Writes `output/report.json` with:

1. false-accepts/hour on the gate half, with its 95% Poisson interval and the
   raw event count and exposure it came from
2. recall on held-out synthetic **speakers**, with its 95% Wilson interval
3. the full input manifest with licences
4. Low / Medium / High operating points, each carrying both what it did on the
   calibration half and what it did on the gate half
5. `diagnostics`: score quantiles for both classes, the calibration curve,
   recall per voice, the spread of recall over individual speakers, and the
   refractory the false-accept count used

Scoring 54 h of audio is the whole cost of a run, so the raw scores are cached
in `output/scores.npz` and the protocol can be re-derived from them in a
second. The cache records the head's SHA256 and refuses to load against a
different head:

```bash
uv run --extra train python evaluate.py --reuse-scores
uv run python evaluate.py --check-report ./output/report.json   # no scores at all
```

### The gates judge intervals, not point estimates

Both count-based gates are decided on the **conservative end of the 95%
interval**. 17 false accepts over 46 h reads as 0.3684/hour, comfortably under
the ceiling, and as an interval of 0.215 to 0.590, which is not: at that event
count the data cannot tell the two apart, and only one of the two answers the
question the ceiling asks a user. A report carrying no interval fails closed
rather than being judged on the ratio alone.

| Gate | Medium operating point | Judged on |
|---|---|---|
| False-accept ceiling | ≤ 0.5 / hour | upper end of the 95% Poisson interval |
| Recall floor | ≥ 0.9 | lower end of the 95% Wilson interval |
| Manifest | every input MIT / Apache-2.0 / CC0 / CC-BY (no NC, no SA) | exactly |

At 46 h of gate background the ceiling admits at most **13 events** (upper end
0.482/hour). More background is the only way to buy resolution: the point
estimate is the same claim either way, the interval simply says how well it is
known.

### The refractory matches the shipped detector

`murmur-core`'s armed loop ignores 25 frames of 80 ms after a hit, so one
utterance cannot double-trigger: 2.0 s. The evaluator slides one head window
per 128 ms, not per 80 ms, so counting 25 *windows* suppressed 3.2 s and
undercounted false accepts by a third. It now counts 15 windows (1.92 s,
rounded down so eval never suppresses longer than serve) and resets at every
file boundary, because a hit at the end of one recording has no business
hiding one at the start of an unrelated recording.

### Rust and Python must agree on the numbers

The Rust runtime scores the same three models on the same audio, against
thresholds 0.006 apart, so numeric drift between the two would invalidate both
gates silently. `crates/murmur-core/tests/wake_parity.rs` compares actual
scores on a committed 4.8 s fixture (real background speech followed by a real
held-out "Hey Murmur" clip, seated exactly as the evaluator seats it) and
agrees to 1e-5, 200x inside the narrowest decision margin. Measured
disagreement is currently 0.0 across ONNX Runtime 1.21 and 1.23.

Regenerate the fixture whenever the head is retrained; the test fails rather
than skipping if the fixture describes a different model:

```bash
uv run --extra train python tools/make_parity_fixture.py
```

## Release is a separate step

This pipeline does **not** publish models. Hash pins in
`crates/murmur-core/src/audio/wake.rs` stay `pending-release-upload` until a
human-approved GitHub release `wake-models-v1` exists. After that release:

1. SHA-256 the three ONNX files
2. replace the fail-closed sentinels and `TOTAL_DOWNLOAD_BYTES`
3. map Low/Medium/High thresholds from `report.json` operating points

Do not run `gh release create` / `gh release upload` from this tree as part
of ordinary training.

## Tests

```bash
uv run pytest
```

Pytest lives here so it does not need the Rust `wake` feature. Two of the files
exist because a reviewer broke the code and the suite stayed green:

- `test_false_accept_count.py` pins the false-accept arithmetic absolutely.
  Every other test asserts a *relation* (this budget holds, that point is
  looser), and `hits / hours / 2.0` preserves every relation in the suite.
- `test_preprocessing_parity.py::test_serve_window_*` constrain the shared
  preprocessing function on its own. The rest of that file compares training
  against serving, so a transform added *inside* the shared function is
  invisible to all of it, and that is the bug class that produced 493 false
  accepts an hour.

The real-model Rust tests live in `crates/murmur-core/tests/` and skip when the
model artifacts are absent:

```bash
MURMUR_WAKE_MODEL_DIR=/path/with/three/onnx/files \
  cargo test -p murmur-core --features wake --test wake_parity -- --nocapture
```
