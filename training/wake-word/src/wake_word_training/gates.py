"""Release gates for a trained Hey Murmur head.

All three must pass or the artifact does not ship:

1. False-accepts/hour at Medium ≤ 0.5
2. Recall at Medium ≥ 0.9
3. Training-input manifest is entirely MIT / Apache-2.0 / CC0 / CC-BY
   (no NC, no SA).
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path

from wake_word_training.allowlist import licence_is_permissive

FALSE_ACCEPT_MAX_PER_HOUR = 0.5
RECALL_MIN = 0.9
OPERATING_POINT_NAMES = ("Low", "Medium", "High")


class GateFailure(Exception):
    """One or more release gates failed; the artifact must not ship."""


@dataclass
class OperatingPoint:
    name: str
    threshold: float
    false_accepts_per_hour: float
    recall: float


@dataclass
class EvalReport:
    false_accepts_per_hour: float
    recall: float
    manifest: list[dict] = field(default_factory=list)
    operating_points: list[OperatingPoint] = field(default_factory=list)
    phrase: str = "Hey Murmur"

    def medium(self) -> OperatingPoint | None:
        for point in self.operating_points:
            if point.name == "Medium":
                return point
        return None


def write_report(path: Path, report: EvalReport) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(_report_to_dict(report), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def load_report(path: Path) -> EvalReport:
    data = json.loads(path.read_text(encoding="utf-8"))
    points_raw = data.get("operating_points") or {}
    if isinstance(points_raw, dict):
        points = [
            OperatingPoint(
                name=name,
                threshold=float(item["threshold"]),
                false_accepts_per_hour=float(item["false_accepts_per_hour"]),
                recall=float(item["recall"]),
            )
            for name, item in points_raw.items()
        ]
    else:
        points = [OperatingPoint(**item) for item in points_raw]
    medium = next((p for p in points if p.name == "Medium"), None)
    fa = float(
        data.get(
            "false_accepts_per_hour",
            medium.false_accepts_per_hour if medium else 0.0,
        )
    )
    recall = float(data.get("recall", medium.recall if medium else 0.0))
    return EvalReport(
        false_accepts_per_hour=fa,
        recall=recall,
        manifest=list(data.get("manifest") or []),
        operating_points=points,
        phrase=str(data.get("phrase") or "Hey Murmur"),
    )


def gate_failures(report: EvalReport) -> list[str]:
    """Return human-readable failures; empty means all three gates passed."""
    failures: list[str] = []
    medium = report.medium()
    fa = medium.false_accepts_per_hour if medium else report.false_accepts_per_hour
    recall = medium.recall if medium else report.recall
    if fa > FALSE_ACCEPT_MAX_PER_HOUR:
        failures.append(
            f"false-accept ceiling failed at Medium: {fa}/hour "
            f"(limit <= {FALSE_ACCEPT_MAX_PER_HOUR}/hour)"
        )
    if recall < RECALL_MIN:
        failures.append(
            f"recall floor failed at Medium: {recall} (limit >= {RECALL_MIN})"
        )
    bad = [
        item
        for item in report.manifest
        if not licence_is_permissive(str(item.get("licence") or ""))
    ]
    if not report.manifest:
        failures.append("manifest is empty; licence audit cannot pass")
    elif bad:
        shown = ", ".join(
            f"{item.get('id')!r} ({item.get('licence')})" for item in bad[:8]
        )
        failures.append(f"manifest is not all-permissive: {shown}")
    names = {p.name for p in report.operating_points}
    missing = [n for n in OPERATING_POINT_NAMES if n not in names]
    if missing:
        failures.append(f"operating points missing {missing}")
    return failures


def assert_report_shippable(path: Path) -> EvalReport:
    if not path.is_file():
        raise GateFailure(f"no evaluation report at {path}; refusing to ship")
    report = load_report(path)
    failures = gate_failures(report)
    if failures:
        raise GateFailure(
            "release gates failed; artifact must not ship:\n- "
            + "\n- ".join(failures)
        )
    return report


def _report_to_dict(report: EvalReport) -> dict:
    points = {p.name: asdict(p) for p in report.operating_points}
    for item in points.values():
        item.pop("name", None)
    failures = gate_failures(report)
    medium = report.medium()
    fa = medium.false_accepts_per_hour if medium else report.false_accepts_per_hour
    recall = medium.recall if medium else report.recall
    return {
        "phrase": report.phrase,
        "false_accepts_per_hour": fa,
        "recall": recall,
        "gates": {
            "false_accept_ceiling": {
                "limit_per_hour": FALSE_ACCEPT_MAX_PER_HOUR,
                "value": fa,
                "passed": fa <= FALSE_ACCEPT_MAX_PER_HOUR,
            },
            "recall_floor": {
                "limit": RECALL_MIN,
                "value": recall,
                "passed": recall >= RECALL_MIN,
            },
            "manifest_all_permissive": {
                "passed": not any(
                    "manifest" in f or "all-permissive" in f for f in failures
                )
                and bool(report.manifest),
            },
            "all_passed": not failures,
        },
        "operating_points": points,
        "manifest": report.manifest,
        "release_gates": [
            f"false-accept ≤ {FALSE_ACCEPT_MAX_PER_HOUR}/hour at Medium",
            f"recall ≥ {RECALL_MIN} at Medium",
            "manifest all-permissive (MIT / Apache-2.0 / CC0 / CC-BY; no NC, no SA)",
        ],
    }
