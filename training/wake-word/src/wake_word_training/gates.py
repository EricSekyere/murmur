"""Release gates for a trained Hey Murmur head.

All three must pass or the artifact does not ship:

1. False-accepts/hour at Medium ≤ 0.5
2. Recall at Medium ≥ 0.9
3. Training-input manifest is entirely MIT / Apache-2.0 / CC0 / CC-BY
   (no NC, no SA).

Gates 1 and 2 are counts, and the gate judges the **conservative end of the
95% interval**, not the point estimate: the false-accept ceiling under-promises
nothing to a user, so a rate whose interval still reaches past 0.5/hour has
not been shown to meet it. 17 events over 46 h read as 0.3684/hour and as a
95% interval of 0.215 to 0.590, and only one of those two answers the question
the ceiling asks. A report without intervals cannot be judged and fails closed.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path

from wake_word_training.allowlist import licence_is_permissive

FALSE_ACCEPT_MAX_PER_HOUR = 0.5
RECALL_MIN = 0.9
OPERATING_POINT_NAMES = ("Low", "Medium", "High")
# Each named point is the most sensitive threshold that stays inside a
# false-accept budget, so the three are what a user actually chooses between.
# Medium is the gate; Low is the quiet setting and High the eager one.
FALSE_ACCEPT_BUDGETS = {
    "Low": 0.1,
    "Medium": FALSE_ACCEPT_MAX_PER_HOUR,
    "High": 2.0,
}


class GateFailure(Exception):
    """One or more release gates failed; the artifact must not ship."""


def _agrees(stored: list, expected: tuple[float, float]) -> bool:
    """Whether a stored interval matches one recomputed from the counts.

    Tolerance is loose enough for JSON round-tripping and tight enough that no
    edit large enough to change a verdict survives.
    """
    return all(abs(float(s) - e) <= 1e-9 for s, e in zip(stored, expected))


@dataclass
class OperatingPoint:
    name: str
    threshold: float
    false_accepts_per_hour: float
    recall: float
    # Measured on the held-out half, at a threshold frozen on validation. The
    # raw event count and exposure are kept because they, not the ratio, are
    # what the interval is computed from and what a reader can re-derive.
    false_accept_events: int = 0
    measured_hours: float = 0.0
    measured_clips: int = 0
    false_accepts_ci95: list = field(default_factory=list)
    recall_ci95: list = field(default_factory=list)
    # What this threshold did on the validation data that chose it. A large gap
    # between the two is the calibration failing to generalise, which the old
    # single-sample protocol could not show at all.
    calibration: dict = field(default_factory=dict)

    def judged_false_accepts(self) -> float | None:
        """Upper end of the false-accept interval, or None if unverifiable.

        Recomputed from the counts stored beside it rather than trusted. A
        stored interval is just a number in a file, so `--check-report` on a
        hand-edited report would otherwise judge whatever it was told.
        """
        if len(self.false_accepts_ci95) != 2 or self.measured_hours <= 0.0:
            return None
        from wake_word_training.intervals import poisson_rate_interval

        expected = poisson_rate_interval(self.false_accept_events, self.measured_hours)
        if not _agrees(self.false_accepts_ci95, expected):
            return None
        return float(expected[1])

    def judged_recall(self) -> float | None:
        """Lower end of the recall interval, or None if unverifiable."""
        if len(self.recall_ci95) != 2 or self.measured_clips <= 0:
            return None
        from wake_word_training.intervals import wilson_proportion_interval

        hits = round(self.recall * self.measured_clips)
        expected = wilson_proportion_interval(hits, self.measured_clips)
        if not _agrees(self.recall_ci95, expected):
            return None
        return float(expected[0])


@dataclass
class EvalReport:
    false_accepts_per_hour: float
    recall: float
    manifest: list[dict] = field(default_factory=list)
    operating_points: list[OperatingPoint] = field(default_factory=list)
    phrase: str = "Hey Murmur"
    # Score spread, the recall/false-accept curve and the per-voice recall
    # breakdown. Not gated, but a head whose scores pin to 0 and 1 passes or
    # fails for reasons no single Medium number shows.
    diagnostics: dict = field(default_factory=dict)

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


def _point_from_dict(name: str, item: dict) -> OperatingPoint:
    """One operating point, tolerating a report written before the intervals.

    Missing intervals stay empty rather than defaulting to something passable:
    `gate_failures` refuses to judge a point it cannot bound.
    """
    return OperatingPoint(
        name=name,
        threshold=float(item["threshold"]),
        false_accepts_per_hour=float(item["false_accepts_per_hour"]),
        recall=float(item["recall"]),
        false_accept_events=int(item.get("false_accept_events") or 0),
        measured_hours=float(item.get("measured_hours") or 0.0),
        measured_clips=int(item.get("measured_clips") or 0),
        false_accepts_ci95=[float(v) for v in (item.get("false_accepts_ci95") or [])],
        recall_ci95=[float(v) for v in (item.get("recall_ci95") or [])],
        calibration=dict(item.get("calibration") or {}),
    )


def load_report(path: Path) -> EvalReport:
    data = json.loads(path.read_text(encoding="utf-8"))
    points_raw = data.get("operating_points") or {}
    if isinstance(points_raw, dict):
        points = [_point_from_dict(name, item) for name, item in points_raw.items()]
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
        diagnostics=dict(data.get("diagnostics") or {}),
    )


def gate_failures(report: EvalReport) -> list[str]:
    """Return human-readable failures; empty means all three gates passed."""
    medium = report.medium()
    failures = (
        ["no Medium operating point; there is nothing to judge"]
        if medium is None
        else _false_accept_failures(medium) + _recall_failures(medium)
    )
    failures += _manifest_failures(report)
    names = {p.name for p in report.operating_points}
    missing = [n for n in OPERATING_POINT_NAMES if n not in names]
    if missing:
        failures.append(f"operating points missing {missing}")
    return failures


def _false_accept_failures(medium: OperatingPoint) -> list[str]:
    judged = medium.judged_false_accepts()
    if judged is None:
        return [
            "false-accept ceiling cannot be judged at Medium: the report "
            "carries no 95% interval, and a point estimate over a few dozen "
            "events says little about the rate a user will meet"
        ]
    if judged <= FALSE_ACCEPT_MAX_PER_HOUR:
        return []
    low, high = medium.false_accepts_ci95
    return [
        f"false-accept ceiling failed at Medium: {medium.false_accept_events} events "
        f"over {medium.measured_hours:.2f} h = {medium.false_accepts_per_hour:.4f}/hour, "
        f"95% interval {low:.4f} to {high:.4f}; the gate judges the upper end "
        f"against <= {FALSE_ACCEPT_MAX_PER_HOUR}/hour"
    ]


def _recall_failures(medium: OperatingPoint) -> list[str]:
    judged = medium.judged_recall()
    if judged is None:
        return [
            "recall floor cannot be judged at Medium: the report carries no "
            "95% interval"
        ]
    if judged >= RECALL_MIN:
        return []
    low, high = medium.recall_ci95
    return [
        f"recall floor failed at Medium: {medium.recall:.4f} over "
        f"{medium.measured_clips} clips, 95% interval {low:.4f} to {high:.4f}; "
        f"the gate judges the lower end against >= {RECALL_MIN}"
    ]


def _manifest_failures(report: EvalReport) -> list[str]:
    failures: list[str] = []
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
            "false_accept_ceiling": _false_accept_gate(medium, fa),
            "recall_floor": _recall_gate(medium, recall),
            "manifest_all_permissive": {
                "passed": not any(
                    "manifest" in f or "all-permissive" in f for f in failures
                )
                and bool(report.manifest),
            },
            "all_passed": not failures,
        },
        "operating_points": points,
        "diagnostics": report.diagnostics,
        "manifest": report.manifest,
        "release_gates": [
            f"false-accept ≤ {FALSE_ACCEPT_MAX_PER_HOUR}/hour at Medium, "
            "judged on the upper end of the 95% interval",
            f"recall ≥ {RECALL_MIN} at Medium, judged on the lower end of the "
            "95% interval",
            "manifest all-permissive (MIT / Apache-2.0 / CC0 / CC-BY; no NC, no SA)",
        ],
    }


def _false_accept_gate(medium: OperatingPoint | None, fa: float) -> dict:
    judged = medium.judged_false_accepts() if medium else None
    return {
        "limit_per_hour": FALSE_ACCEPT_MAX_PER_HOUR,
        "value": fa,
        "judges": "ci95_upper",
        "judged_value": judged,
        "ci95": list(medium.false_accepts_ci95) if medium else [],
        "events": medium.false_accept_events if medium else 0,
        "hours": medium.measured_hours if medium else 0.0,
        "passed": judged is not None and judged <= FALSE_ACCEPT_MAX_PER_HOUR,
    }


def _recall_gate(medium: OperatingPoint | None, recall: float) -> dict:
    judged = medium.judged_recall() if medium else None
    return {
        "limit": RECALL_MIN,
        "value": recall,
        "judges": "ci95_lower",
        "judged_value": judged,
        "ci95": list(medium.recall_ci95) if medium else [],
        "clips": medium.measured_clips if medium else 0,
        "passed": judged is not None and judged >= RECALL_MIN,
    }
