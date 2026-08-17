"""Evaluate a trained head and enforce the three release gates."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from wake_word_training.allowlist import AllowlistError, validate_allowlist
from wake_word_training.audio import (
    iter_audio_files,
    pad_for_window,
    read_audio,
    resample_16k,
)
from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES
from wake_word_training.gates import (
    FALSE_ACCEPT_MAX_PER_HOUR,
    RECALL_MIN,
    EvalReport,
    GateFailure,
    OperatingPoint,
    assert_report_shippable,
    write_report,
)
from wake_word_training.train_lib import default_allowlist

ROOT = Path(__file__).resolve().parents[2]
REFRACTORY = 25


def run(argv: list[str] | None = None) -> int:
    args = _parse(argv)
    try:
        if args.check_report:
            assert_report_shippable(args.check_report)
            print(f"release gates passed: {args.check_report}")
            return 0
        allowlist = validate_allowlist(args.allowlist)
        report = _evaluate(allowlist, args)
        report_path = args.report
        write_report(report_path, report)
        assert_report_shippable(report_path)
        print(f"release gates passed: {report_path}")
    except (AllowlistError, GateFailure) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    return 0


def _parse(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Evaluate hey_murmur.onnx. Ships only if false-accepts/hour at Medium "
            f"≤ {FALSE_ACCEPT_MAX_PER_HOUR}, recall at Medium ≥ {RECALL_MIN}, "
            "and the input manifest is all-permissive."
        )
    )
    parser.add_argument("--allowlist", type=Path, default=default_allowlist())
    parser.add_argument("--output-dir", type=Path, default=ROOT / "output")
    parser.add_argument("--data-dir", type=Path, default=ROOT / "data")
    parser.add_argument("--model", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument(
        "--check-report",
        type=Path,
        default=None,
        help="Re-check an existing report.json; refuse to ship if any gate fails.",
    )
    args = parser.parse_args(argv)
    if args.report is None:
        args.report = args.output_dir / "report.json"
    return args


def _evaluate(allowlist, args: argparse.Namespace) -> EvalReport:
    import numpy as np
    import onnxruntime as ort

    from wake_word_training.features import Backbone
    from wake_word_training.allowlist import refuse_forbidden_heads

    out = args.output_dir
    model_path = args.model or (out / "hey_murmur.onnx")
    refuse_forbidden_heads(str(model_path))
    if not model_path.is_file():
        raise AllowlistError(f"missing trained head {model_path}")
    backbone_dir = args.data_dir / "backbone"
    mel = backbone_dir / "melspectrogram.onnx"
    emb = backbone_dir / "embedding_model.onnx"
    backbone = Backbone(mel, emb)
    session = ort.InferenceSession(str(model_path), providers=["CPUExecutionProvider"])
    held_out = out / "clips" / "held_out"
    pos_scores = _score_dir(backbone, session, held_out)
    bg_scores, bg_hours = _score_background(backbone, session, allowlist, args.data_dir)
    if not pos_scores:
        raise AllowlistError(f"no held-out positives under {held_out}")
    if bg_hours <= 0:
        raise AllowlistError("background corpus has zero duration")
    points = _operating_points(pos_scores, bg_scores, bg_hours)
    medium = next(p for p in points if p.name == "Medium")
    manifest = _manifest(allowlist, out)
    return EvalReport(
        false_accepts_per_hour=medium.false_accepts_per_hour,
        recall=medium.recall,
        manifest=manifest,
        operating_points=points,
    )


def _score_windows(session, windows) -> list[float]:
    import numpy as np

    if len(windows) == 0:
        return []
    name = session.get_inputs()[0].name
    scores: list[float] = []
    # The exported head fixes the batch dimension at 1, matching the
    # streaming contract in murmur-core's OnnxWakeScorer ([1, 16, 96]),
    # so windows are scored one at a time rather than as one batch.
    for window in np.asarray(windows, dtype=np.float32):
        out = session.run(None, {name: window[None, ...]})[0]
        scores.append(float(np.asarray(out).reshape(-1)[0]))
    return scores


def _score_dir(backbone, session, clips_dir: Path) -> list[float]:
    scores: list[float] = []
    for path in iter_audio_files(clips_dir):
        samples, rate = read_audio(path)
        # Held-out clips are bare Piper phrases, shorter than one head window;
        # seat them like training does or every positive scores 0.0.
        audio = pad_for_window(resample_16k(samples, rate), MIN_HEAD_WINDOW_SAMPLES)
        windows = backbone.windows(audio)
        clip_scores = _score_windows(session, windows)
        scores.append(max(clip_scores) if clip_scores else 0.0)
    return scores


def _score_background(backbone, session, allowlist, data_root: Path):
    scores: list[float] = []
    hours = 0.0
    for dataset in allowlist.datasets:
        if dataset.role != "background":
            continue
        root = data_root / dataset.id
        for path in iter_audio_files(root):
            samples, rate = read_audio(path)
            audio = resample_16k(samples, rate)
            hours += len(audio) / 16_000 / 3600.0
            windows = backbone.windows(audio)
            scores.extend(_score_windows(session, windows))
    return scores, hours


def _operating_points(pos_scores, bg_scores, bg_hours: float) -> list[OperatingPoint]:
    import numpy as np

    curve = []
    for threshold in np.linspace(0.05, 0.95, 19):
        t = float(threshold)
        recall = sum(1 for s in pos_scores if s >= t) / len(pos_scores)
        fa = _false_accepts_per_hour(bg_scores, t, bg_hours)
        curve.append((t, fa, recall))
    passing = [c for c in curve if c[1] <= FALSE_ACCEPT_MAX_PER_HOUR]
    if passing:
        medium = min(passing, key=lambda c: c[0])
    else:
        medium = min(curve, key=lambda c: c[1])
    stricter = [c for c in curve if c[0] > medium[0]]
    looser = [c for c in curve if c[0] < medium[0]]
    low = max(stricter, key=lambda c: c[0]) if stricter else medium
    high = min(looser, key=lambda c: c[0]) if looser else medium
    return [
        OperatingPoint("Low", low[0], low[1], low[2]),
        OperatingPoint("Medium", medium[0], medium[1], medium[2]),
        OperatingPoint("High", high[0], high[1], high[2]),
    ]


def _false_accepts_per_hour(scores: list[float], threshold: float, hours: float) -> float:
    if hours <= 0:
        return float("inf")
    hits = 0
    refractory = 0
    for score in scores:
        if refractory > 0:
            refractory -= 1
            continue
        if score >= threshold:
            hits += 1
            refractory = REFRACTORY
    return hits / hours


def _manifest(allowlist, out: Path) -> list[dict]:
    run_path = out / "run_manifest.json"
    if run_path.is_file():
        data = json.loads(run_path.read_text(encoding="utf-8"))
        return list(data.get("manifest") or [])
    return [
        {
            "id": e.id,
            "kind": e.kind,
            "licence": e.licence,
            "url": e.url,
            "role": e.role,
            "notes": e.notes,
        }
        for e in allowlist.all_entries()
    ]
