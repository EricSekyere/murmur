#!/usr/bin/env python3
"""Train a Hey Murmur openWakeWord head from allowlisted Piper voices.

Refuses any input not listed in allowlist.toml. Fails closed if the allowlist
is empty or a voice's licence is missing. Never downloads upstream
CC BY-NC-SA pre-trained heads (Hey Jarvis, Alexa, …).

Full training downloads Piper voices and expects allowlisted MUSAN / RIR /
LibriSpeech corpora under --data-dir. Use --validate-only to audit licences
without starting a multi-hour job.

Release (hash pins + GitHub `wake-models-v1`) is a separate, human-approved
step — this script does not publish artifacts.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parent / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from wake_word_training.train_lib import run

if __name__ == "__main__":
    raise SystemExit(run())
