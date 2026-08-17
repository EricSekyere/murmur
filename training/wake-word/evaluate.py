#!/usr/bin/env python3
"""Evaluate hey_murmur.onnx and write report.json.

report.json contains:
  1. false-accepts/hour on the allowlisted background corpus
  2. recall on held-out synthetic voices
  3. the full input manifest with licences
  4. measured Low / Medium / High operating points

Release gates — all three must pass or the process fails (artifact does not
ship):
  * false-accept ≤ 0.5/hour at Medium
  * recall ≥ 0.9 at Medium
  * manifest all-permissive (MIT / Apache-2.0 / CC0 / CC-BY; no NC, no SA)

`--check-report path` re-validates an existing report without scoring audio.
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
