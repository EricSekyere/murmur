#!/usr/bin/env python3
"""Generate the Rust/Python parity fixture for murmur-core.

`crates/murmur-core/src/audio/wake_onnx.rs` scores in Rust and this package
scores in Python, against thresholds whose named operating points are 0.006
apart. Any numeric drift between the two silently invalidates both release
gates, and nothing compared them. This writes the two files that let a Rust
test compare actual numbers:

  crates/murmur-core/tests/fixtures/wake_parity.wav
  crates/murmur-core/tests/fixtures/wake_parity_scores.json

The audio is one real held-out "Hey Murmur" clip seated exactly as the
evaluator seats it, with real background speech in front so the stream crosses
low-scoring territory as well as the decision band. Scores are read back from
the written WAV rather than taken from the in-memory float array, so the
expected numbers belong to the committed bytes and not to a rounding of them.

Run from `training/wake-word`:

    uv run --extra train python tools/make_parity_fixture.py

Re-run it whenever the head is retrained: the JSON records the SHA256 of all
three models and the Rust test refuses a fixture that describes another head.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
_SRC = ROOT / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from wake_word_training.audio import read_audio, resample_16k, serve_window, write_wav
from wake_word_training.features import (
    FRAME_SAMPLES,
    MIN_HEAD_WINDOW_SAMPLES,
    WINDOW_STRIDE_SECONDS,
)
from wake_word_training.scoring import load_models, score_windows

# Real speech ahead of the phrase, so the fixture exercises the low end of the
# score range too. A whole number of frames keeps the streaming loop aligned
# with the file rather than with a partial frame at the end.
LEAD_FRAMES = 20
DEST = Path("crates/murmur-core/tests/fixtures")


def main(argv: list[str] | None = None) -> int:
    args = _parse(argv)
    positive = _first_audio(args.clips)
    background = _first_audio(args.background)
    if positive is None or background is None:
        print(
            f"error: need one clip under {args.clips} and one under {args.background}",
            file=sys.stderr,
        )
        return 2
    audio = _compose(positive, background)
    args.dest.mkdir(parents=True, exist_ok=True)
    wav_path = args.dest / "wake_parity.wav"
    write_wav(wav_path, audio)

    decoded, rate = read_audio(wav_path)
    models = load_models(args.backbone_dir, args.head)
    scores = score_windows(models.head, models.backbone.windows(decoded))
    _write_scores(args.dest / "wake_parity_scores.json", args, wav_path, rate, decoded, scores)
    print(
        f"wrote {wav_path} ({wav_path.stat().st_size} bytes, "
        f"{len(decoded) / rate:.2f}s) and {len(scores)} expected scores "
        f"spanning {min(scores):.6f} to {max(scores):.6f}"
    )
    return 0


def _parse(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--head", type=Path, default=ROOT / "output" / "hey_murmur.onnx")
    parser.add_argument("--backbone-dir", type=Path, default=ROOT / "data" / "backbone")
    parser.add_argument(
        "--clips",
        type=Path,
        default=ROOT / "output" / "clips" / "held_out",
        help="Held-out positives; the first in sorted order becomes the fixture.",
    )
    parser.add_argument(
        "--background", type=Path, default=ROOT / "data" / "librispeech"
    )
    parser.add_argument(
        "--dest", type=Path, default=ROOT.parents[1] / DEST, help="Fixture directory."
    )
    return parser.parse_args(argv)


def _first_audio(root: Path) -> Path | None:
    from wake_word_training.audio import iter_audio_files

    return next(iter(iter_audio_files(root)), None)


def _compose(positive: Path, background: Path) -> np.ndarray:
    """Background speech, then the positive clip seated as the evaluator seats it."""
    samples, rate = read_audio(positive)
    seated = serve_window(resample_16k(samples, rate), MIN_HEAD_WINDOW_SAMPLES)
    samples, rate = read_audio(background)
    lead = resample_16k(samples, rate)[: LEAD_FRAMES * FRAME_SAMPLES]
    if len(lead) < LEAD_FRAMES * FRAME_SAMPLES:
        raise SystemExit(f"{background} is shorter than {LEAD_FRAMES} frames")
    return np.concatenate([lead, seated]).astype(np.float32)


def _write_scores(path: Path, args, wav_path: Path, rate: int, decoded, scores) -> None:
    import onnxruntime as ort

    payload = {
        "generated_by": "training/wake-word/tools/make_parity_fixture.py",
        "audio": wav_path.name,
        "sample_rate": rate,
        "samples": len(decoded),
        "frame_samples": FRAME_SAMPLES,
        "window_stride_seconds": WINDOW_STRIDE_SECONDS,
        "onnxruntime": ort.__version__,
        "head_sha256": _digest(args.head),
        "melspectrogram_sha256": _digest(args.backbone_dir / "melspectrogram.onnx"),
        "embedding_sha256": _digest(args.backbone_dir / "embedding_model.onnx"),
        "scores": [round(float(s), 9) for s in scores],
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


if __name__ == "__main__":
    raise SystemExit(main())
