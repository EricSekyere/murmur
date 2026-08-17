"""Evaluate a trained head and enforce the three release gates."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from wake_word_training.allowlist import AllowlistError, validate_allowlist
from wake_word_training.audio import (
    iter_audio_files,
    read_audio,
    resample_16k,
    serve_window,
)
from wake_word_training.background import EVALUATION, background_paths
from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES
from wake_word_training.gates import (
    FALSE_ACCEPT_BUDGETS,
    FALSE_ACCEPT_MAX_PER_HOUR,
    OPERATING_POINT_NAMES,
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
# Thresholds are drawn from the positive scores themselves as well as a fixed
# grid. A head whose scores all sat at 0.0000 or 1.0000 had nothing to trade
# on a [0.05, 0.95] grid, and the sweep reported the same recall at all 19
# points; quantile thresholds put a candidate wherever positives actually are.
SCORE_QUANTILES = 41
# The background's upper tail decides the false-accept budgets, and it is
# thin: 99% of windows sit below 0.04 while the handful that matter sit near
# 1.0. Sampling it logarithmically puts candidates between the top of the
# fixed grid and the positive band, where Low and Medium otherwise collapse
# onto one threshold despite budgets that differ fivefold.
TAIL_QUANTILES = 25
TAIL_DECADES = 5


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
    import onnxruntime as ort

    from wake_word_training.allowlist import refuse_forbidden_heads
    from wake_word_training.features import Backbone

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
    scored = _score_clips(backbone, session, held_out)
    pos_scores = [score for _path, score in scored]
    bg_scores, bg_hours = _score_background(backbone, session, allowlist, args.data_dir)
    if not pos_scores:
        raise AllowlistError(f"no held-out positives under {held_out}")
    if bg_hours <= 0:
        raise AllowlistError("background corpus has zero duration")
    curve = _threshold_curve(pos_scores, bg_scores, bg_hours)
    points = _operating_points(curve)
    medium = next(p for p in points if p.name == "Medium")
    return EvalReport(
        false_accepts_per_hour=medium.false_accepts_per_hour,
        recall=medium.recall,
        manifest=_manifest(allowlist, out),
        operating_points=points,
        diagnostics=_diagnostics(scored, held_out, bg_scores, bg_hours, curve, medium),
    )


def _diagnostics(scored, held_out: Path, bg_scores, bg_hours: float, curve, medium) -> dict:
    import numpy as np

    pos_scores = [score for _path, score in scored]
    return {
        "held_out_clips": len(pos_scores),
        "background_hours": bg_hours,
        "background_windows": len(bg_scores),
        "positive_score_quantiles": _quantile_map(pos_scores),
        "background_score_quantiles": _quantile_map(bg_scores),
        "positive_scores_on_the_rails": float(
            np.mean(
                [1.0 if (s <= 0.001 or s >= 0.999) else 0.0 for s in pos_scores]
            )
        ),
        "recall_by_voice_at_medium": _recall_by_group(scored, held_out, 0, medium.threshold),
        "speaker_recall_at_medium": _speaker_recall(scored, held_out, medium.threshold),
        "curve": [
            {
                "threshold": round(t, 6),
                "recall": round(recall, 6),
                "false_accepts_per_hour": round(fa, 6),
            }
            for t, fa, recall in curve
        ],
    }


def _quantile_map(scores) -> dict:
    import numpy as np

    if not scores:
        return {}
    values = np.asarray(scores, dtype=np.float64)
    return {
        f"p{int(q * 100):02d}": float(np.quantile(values, q))
        for q in (0.0, 0.01, 0.05, 0.25, 0.5, 0.75, 0.95, 0.99, 1.0)
    }


def _group_of(path: Path, root: Path, depth: int) -> str:
    parts = path.relative_to(root).parts
    return parts[depth] if len(parts) > depth else "?"


def _recall_by_group(scored, root: Path, depth: int, threshold: float) -> dict:
    hits: dict[str, list[int]] = {}
    for path, score in scored:
        bucket = hits.setdefault(_group_of(path, root, depth), [])
        bucket.append(1 if score >= threshold else 0)
    return {
        name: {"clips": len(flags), "recall": sum(flags) / len(flags)}
        for name, flags in sorted(hits.items())
    }


def _speaker_recall(scored, root: Path, threshold: float) -> dict:
    """How recall is spread over people, not just over voices.

    An aggregate can clear the floor while a whole speaker scores zero, which
    is the failure the old two-voice split could only ever report as 0.5.
    """
    import numpy as np

    per_speaker: dict[str, list[int]] = {}
    for path, score in scored:
        # Keyed on the speaker directory alone, so the same LibriTTS person
        # rendered by both sibling voices counts once.
        per_speaker.setdefault(_group_of(path, root, 1), []).append(
            1 if score >= threshold else 0
        )
    rates = np.array([sum(v) / len(v) for v in per_speaker.values()], dtype=np.float64)
    return {
        "speakers": len(rates),
        "min": float(rates.min()),
        "p05": float(np.quantile(rates, 0.05)),
        "median": float(np.median(rates)),
        "mean": float(rates.mean()),
        "fully_missed": int((rates == 0.0).sum()),
    }


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


def _score_clips(backbone, session, clips_dir: Path) -> list[tuple[Path, float]]:
    scored: list[tuple[Path, float]] = []
    for path in iter_audio_files(clips_dir):
        samples, rate = read_audio(path)
        # Held-out clips are bare Piper phrases, shorter than one head window;
        # seat them like training does or every positive scores 0.0.
        audio = serve_window(resample_16k(samples, rate), MIN_HEAD_WINDOW_SAMPLES)
        clip_scores = _score_windows(session, backbone.windows(audio))
        scored.append((path, max(clip_scores) if clip_scores else 0.0))
    return scored


def _score_dir(backbone, session, clips_dir: Path) -> list[float]:
    return [score for _path, score in _score_clips(backbone, session, clips_dir)]


def _score_background(backbone, session, allowlist, data_root: Path):
    """False accepts are counted only on background speakers training never saw.

    Training draws its negatives from the other side of the same split (see
    `background.py`), so this number is a generalisation measure rather than a
    recital.
    """
    scores: list[float] = []
    hours = 0.0
    for dataset in allowlist.datasets:
        if dataset.role != "background":
            continue
        for path in background_paths(data_root / dataset.id, role=EVALUATION):
            samples, rate = read_audio(path)
            audio = resample_16k(samples, rate)
            hours += len(audio) / 16_000 / 3600.0
            # Same serve pipeline as positives; also scores short background
            # files that used to count hours yet yield no windows.
            windows = backbone.windows(serve_window(audio, MIN_HEAD_WINDOW_SAMPLES))
            scores.extend(_score_windows(session, windows))
    return scores, hours


def _candidate_thresholds(pos_scores, bg_scores=()) -> list[float]:
    import numpy as np

    grid = list(np.linspace(0.05, 0.95, 19))
    positives = list(np.quantile(pos_scores, np.linspace(0.0, 1.0, SCORE_QUANTILES)))
    # A threshold exactly at a positive score keeps that clip (>=), so nudging
    # candidates down by an epsilon is what separates neighbouring recalls;
    # nudging a background candidate up is what excludes that window.
    candidates = grid + positives + [q - 1e-6 for q in positives]
    if len(bg_scores):
        tail = np.quantile(
            bg_scores, 1.0 - np.logspace(-TAIL_DECADES, -1, TAIL_QUANTILES)
        )
        candidates += [float(q) + 1e-6 for q in tail]
    return sorted(
        {round(float(np.clip(t, 1e-4, 1.0 - 1e-4)), 8) for t in candidates}
    )


def _threshold_curve(pos_scores, bg_scores, bg_hours: float) -> list[tuple[float, float, float]]:
    curve = []
    for threshold in _candidate_thresholds(pos_scores, bg_scores):
        recall = sum(1 for s in pos_scores if s >= threshold) / len(pos_scores)
        fa = _false_accepts_per_hour(bg_scores, threshold, bg_hours)
        curve.append((threshold, fa, recall))
    return curve


def _operating_points(curve: list[tuple[float, float, float]]) -> list[OperatingPoint]:
    """The most sensitive threshold inside each named false-accept budget."""
    points = []
    for name in OPERATING_POINT_NAMES:
        budget = FALSE_ACCEPT_BUDGETS[name]
        affordable = [c for c in curve if c[1] <= budget]
        # Nothing affordable means every threshold overspends; report the
        # cheapest so the gate fails on a real measured number.
        chosen = (
            min(affordable, key=lambda c: c[0])
            if affordable
            else min(curve, key=lambda c: (c[1], -c[0]))
        )
        points.append(OperatingPoint(name, chosen[0], chosen[1], chosen[2]))
    return points


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
