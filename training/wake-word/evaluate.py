#!/usr/bin/env python3
"""Evaluate hey_murmur.onnx and write report.json.

Thresholds are calibrated on the validation speakers and the validation
background split, then measured once on the held-out speakers and the gate
background split. Nothing in the measurement can move a threshold.

report.json contains:
  1. false-accepts/hour on the gate background, with its 95% Poisson interval
     and the event count and exposure behind it
  2. recall on held-out synthetic speakers, with its 95% Wilson interval
  3. the full input manifest with licences
  4. Low / Medium / High operating points, each carrying its calibration
     numbers alongside its measured ones

Release gates — all three must pass or the process fails (artifact does not
ship). The two count-based gates judge the conservative end of the interval,
not the point estimate:
  * false-accept ≤ 0.5/hour at Medium, on the interval's upper end
  * recall ≥ 0.9 at Medium, on the interval's lower end
  * manifest all-permissive (MIT / Apache-2.0 / CC0 / CC-BY; no NC, no SA)

`--reuse-scores` re-derives the report from the cached scores of a previous
run; `--check-report path` re-validates an existing report without any audio.
Publishing `wake-models-v1` is a separate step.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parent / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from wake_word_training.evaluate_lib import run

if __name__ == "__main__":
    raise SystemExit(run())
