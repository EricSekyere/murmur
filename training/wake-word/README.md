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
20; `--max-negative-windows`, default 50000). The pipeline will not load
`train-other-500` (~500 h) into RAM.

This generates "Hey Murmur" with allowlisted Piper voices, augments with
allowlisted noise/RIR, trains the head, and writes `output/hey_murmur.onnx`.

## Evaluate

```bash
uv run --extra train python evaluate.py --output-dir ./output --data-dir ./data
```

Writes `output/report.json` with:

1. false-accepts/hour on allowlisted background
2. recall on held-out synthetic voices
3. the full input manifest with licences
4. Low / Medium / High operating points

Re-check a report without scoring audio:

```bash
uv run python evaluate.py --check-report ./output/report.json
```

### Release gates

All three must pass or `evaluate.py` exits non-zero and the artifact **must
not ship**:

| Gate | Medium operating point |
|---|---|
| False-accept ceiling | ≤ 0.5 / hour |
| Recall floor | ≥ 0.9 |
| Manifest | every input MIT / Apache-2.0 / CC0 / CC-BY (no NC, no SA) |

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

Pytest lives here so it does not need the Rust `wake` feature.
