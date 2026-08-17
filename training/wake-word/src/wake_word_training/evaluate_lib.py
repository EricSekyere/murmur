"""Evaluate a trained head and enforce the three release gates.

Two halves, and nothing crosses between them:

* **calibration** - validation speakers (`clips/validation`) and the validation
  background split. Chooses the Low / Medium / High thresholds.
* **measurement** - held-out speakers (`clips/held_out`) and the gate
  background split. Reports recall and false accepts at those frozen
  thresholds, with a 95% interval, and never moves one.

Scoring both halves is the expensive part, so the raw scores are cached next to
the report: `--reuse-scores` recomputes the whole protocol from them in a
second, and refuses if the head has changed since they were written.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from wake_word_training.allowlist import AllowlistError, validate_allowlist
from wake_word_training.background import EVALUATION, VALIDATION
from wake_word_training.calibration import (
    REFRACTORY_WINDOWS,
    SERVE_REFRACTORY_SECONDS,
    calibrate,
    measure,
    threshold_curve,
)
from wake_word_training.features import WINDOW_STRIDE_SECONDS
from wake_word_training.gates import (
    FALSE_ACCEPT_MAX_PER_HOUR,
    RECALL_MIN,
    EvalReport,
    GateFailure,
    assert_report_shippable,
    write_report,
)
from wake_word_training.scores import ScoreSets, load_scores, save_scores, score_half
from wake_word_training.train_lib import default_allowlist

ROOT = Path(__file__).resolve().parents[2]


def run(argv: list[str] | None = None) -> int:
    args = _parse(argv)
    try:
        if args.check_report:
            assert_report_shippable(args.check_report)
            print(f"release gates passed: {args.check_report}")
            return 0
        allowlist = validate_allowlist(args.allowlist)
        report = _evaluate(allowlist, args)
        write_report(args.report, report)
        assert_report_shippable(args.report)
        print(f"release gates passed: {args.report}")
    except (AllowlistError, GateFailure) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    return 0


def _parse(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Evaluate hey_murmur.onnx. Thresholds are calibrated on validation "
            "speakers and the validation background, then measured once on the "
            "held-out speakers and the gate background. Ships only if the 95% "
            f"interval puts false-accepts/hour at Medium under "
            f"{FALSE_ACCEPT_MAX_PER_HOUR} and recall over {RECALL_MIN}, and "
            "the input manifest is all-permissive."
        )
    )
    parser.add_argument("--allowlist", type=Path, default=default_allowlist())
    parser.add_argument("--output-dir", type=Path, default=ROOT / "output")
    parser.add_argument("--data-dir", type=Path, default=ROOT / "data")
    parser.add_argument("--model", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--scores", type=Path, default=None)
    parser.add_argument(
        "--workers",
        type=int,
        default=None,
        help="Scoring processes (default: cores - 1). Scores do not depend on it.",
    )
    parser.add_argument(
        "--reuse-scores",
        action="store_true",
        help="Recompute the report from cached scores instead of re-scoring audio.",
    )
    parser.add_argument(
        "--check-report",
        type=Path,
        default=None,
        help="Re-check an existing report.json; refuse to ship if any gate fails.",
    )
    args = parser.parse_args(argv)
    if args.report is None:
        args.report = args.output_dir / "report.json"
    if args.scores is None:
        args.scores = args.output_dir / "scores.npz"
    return args


def _evaluate(allowlist, args: argparse.Namespace) -> EvalReport:
    from wake_word_training.allowlist import refuse_forbidden_heads

    out = args.output_dir
    model_path = args.model or (out / "hey_murmur.onnx")
    refuse_forbidden_heads(str(model_path))
    if not model_path.is_file():
        raise AllowlistError(f"missing trained head {model_path}")
    sets = (
        load_scores(args.scores, model_path)
        if args.reuse_scores
        else _score_both_halves(allowlist, args, model_path)
    )
    _assert_scorable(sets, out)
    cal = sets.calibration
    gate = sets.gate
    curve = threshold_curve(cal.positive_scores(), cal.background, cal.background_hours)
    chosen = calibrate(
        cal.positive_scores(), cal.background, cal.background_hours, curve=curve
    )
    points = [
        measure(one, gate.positive_scores(), gate.background, gate.background_hours)
        for one in chosen
    ]
    medium = next(p for p in points if p.name == "Medium")
    return EvalReport(
        false_accepts_per_hour=medium.false_accepts_per_hour,
        recall=medium.recall,
        manifest=_manifest(allowlist, out),
        operating_points=points,
        diagnostics=_diagnostics(sets, curve, medium),
    )


def _score_both_halves(allowlist, args: argparse.Namespace, model_path: Path) -> ScoreSets:
    backbone_dir = args.data_dir / "backbone"
    clips = args.output_dir / "clips"
    sets = ScoreSets(
        calibration=score_half(
            backbone_dir,
            model_path,
            clips / "validation",
            allowlist,
            args.data_dir,
            role=VALIDATION,
            workers=args.workers,
        ),
        gate=score_half(
            backbone_dir,
            model_path,
            clips / "held_out",
            allowlist,
            args.data_dir,
            role=EVALUATION,
            workers=args.workers,
        ),
    )
    save_scores(args.scores, sets, model_path)
    return sets


def _assert_scorable(sets: ScoreSets, out: Path) -> None:
    for name, half in (("validation", sets.calibration), ("held_out", sets.gate)):
        if not half.positives:
            raise AllowlistError(f"no positives under {out / 'clips' / name}")
        if half.background_hours <= 0:
            raise AllowlistError(f"the {name} background split has zero duration")


def _diagnostics(sets: ScoreSets, curve, medium) -> dict:
    cal = sets.calibration
    return {
        "protocol": (
            "thresholds calibrated on validation speakers and the validation "
            "background split; recall and false accepts measured once on the "
            "held-out speakers and the gate background split"
        ),
        "refractory": {
            "serve_seconds": SERVE_REFRACTORY_SECONDS,
            "window_stride_seconds": WINDOW_STRIDE_SECONDS,
            "eval_windows": REFRACTORY_WINDOWS,
            "resets_per_file": True,
        },
        "calibration": {
            "clips": len(cal.positives),
            "background_hours": cal.background_hours,
            "background_files": len(cal.background),
            # One event over this exposure is the finest rate the calibration
            # half can resolve, so a budget is only ever met to within it.
            "rate_resolution_per_hour": 1.0 / cal.background_hours
            if cal.background_hours
            else float("inf"),
        },
        "calibration_curve": [
            {
                "threshold": round(point.threshold, 6),
                "recall": round(point.recall, 6),
                "false_accepts_per_hour": round(point.false_accepts_per_hour, 6),
            }
            for point in curve
        ],
        **_gate_diagnostics(sets.gate, medium),
    }


def _gate_diagnostics(gate, medium) -> dict:
    """The measured half's spread. Not gated, but a head whose scores pin to 0
    and 1 passes or fails for reasons no single Medium number shows."""
    import numpy as np

    positives = gate.positive_scores()
    background = [s for scores in gate.background for s in scores]
    return {
        "held_out_clips": len(positives),
        "background_hours": gate.background_hours,
        "background_files": len(gate.background),
        "background_windows": len(background),
        "positive_score_quantiles": _quantile_map(positives),
        "background_score_quantiles": _quantile_map(background),
        "positive_scores_on_the_rails": float(
            np.mean([1.0 if (s <= 0.001 or s >= 0.999) else 0.0 for s in positives])
        ),
        "recall_by_voice_at_medium": _recall_by_group(gate.positives, 0, medium.threshold),
        "speaker_recall_at_medium": _speaker_recall(gate.positives, medium.threshold),
    }


def _quantile_map(scores) -> dict:
    import numpy as np

    if not len(scores):
        return {}
    values = np.asarray(scores, dtype=np.float64)
    return {
        f"p{int(q * 100):02d}": float(np.quantile(values, q))
        for q in (0.0, 0.01, 0.05, 0.25, 0.5, 0.75, 0.95, 0.99, 1.0)
    }


def _group_of(name: str, depth: int) -> str:
    """One component of a clip's `<voice>/<speaker>/<clip>.wav` path."""
    parts = Path(name).parts
    return parts[depth] if len(parts) > depth else "?"


def _recall_by_group(scored, depth: int, threshold: float) -> dict:
    hits: dict[str, list[int]] = {}
    for name, score in scored:
        bucket = hits.setdefault(_group_of(name, depth), [])
        bucket.append(1 if score >= threshold else 0)
    return {
        name: {"clips": len(flags), "recall": sum(flags) / len(flags)}
        for name, flags in sorted(hits.items())
    }


def _speaker_recall(scored, threshold: float) -> dict:
    """How recall is spread over people, not just over voices.

    An aggregate can clear the floor while a whole speaker scores zero, which
    is the failure the old two-voice split could only ever report as 0.5.
    """
    import numpy as np

    per_speaker: dict[str, list[int]] = {}
    for name, score in scored:
        # Keyed on the speaker directory alone, so the same LibriTTS person
        # rendered by both sibling voices counts once.
        per_speaker.setdefault(_group_of(name, 1), []).append(
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
