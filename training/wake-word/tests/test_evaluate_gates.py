"""Release gates: a failing report must not ship."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wake_word_training.gates import (
    FALSE_ACCEPT_MAX_PER_HOUR,
    RECALL_MIN,
    EvalReport,
    GateFailure,
    OperatingPoint,
    assert_report_shippable,
    load_report,
    write_report,
)
from wake_word_training.intervals import (
    poisson_rate_interval,
    wilson_proportion_interval,
)


def _point(
    name: str,
    *,
    fa: float,
    recall: float,
    threshold: float,
    hours: float = 46.0,
    clips: int = 2000,
    intervals: bool = True,
) -> OperatingPoint:
    """One measured point, with the intervals a real evaluation reports.

    `intervals=False` builds the pre-interval shape, which the gate must refuse
    rather than wave through on the point estimate alone.
    """
    events = round(fa * hours)
    return OperatingPoint(
        name=name,
        threshold=threshold,
        false_accepts_per_hour=fa,
        recall=recall,
        false_accept_events=events,
        measured_hours=hours,
        measured_clips=clips,
        false_accepts_ci95=(
            list(poisson_rate_interval(events, hours)) if intervals else []
        ),
        recall_ci95=(
            list(wilson_proportion_interval(round(recall * clips), clips))
            if intervals
            else []
        ),
    )


def _report(
    *,
    fa: float = 0.1,
    recall: float = 0.95,
    licence: str = "CC-BY-4.0",
    hours: float = 46.0,
    intervals: bool = True,
) -> EvalReport:
    return EvalReport(
        false_accepts_per_hour=fa,
        recall=recall,
        manifest=[
            {
                "id": "en_US-libritts-high",
                "kind": "voice",
                "licence": licence,
            }
        ],
        operating_points=[
            _point("Low", fa=0.01, recall=0.8, threshold=0.7, intervals=intervals),
            _point(
                "Medium",
                fa=fa,
                recall=recall,
                threshold=0.5,
                hours=hours,
                intervals=intervals,
            ),
            _point("High", fa=1.0, recall=0.99, threshold=0.3, intervals=intervals),
        ],
    )


def test_refuses_to_ship_when_false_accepts_exceed_medium_ceiling(tmp_path: Path) -> None:
    report = _report(fa=FALSE_ACCEPT_MAX_PER_HOUR + 0.1)
    path = tmp_path / "report.json"
    write_report(path, report)
    with pytest.raises(GateFailure, match="false-accept"):
        assert_report_shippable(path)


def test_refuses_to_ship_when_recall_below_medium_floor(tmp_path: Path) -> None:
    report = _report(recall=RECALL_MIN - 0.05)
    path = tmp_path / "report.json"
    write_report(path, report)
    with pytest.raises(GateFailure, match="recall"):
        assert_report_shippable(path)


def test_refuses_to_ship_when_manifest_includes_non_permissive_licence(
    tmp_path: Path,
) -> None:
    report = _report(licence="CC-BY-NC-SA-4.0")
    path = tmp_path / "report.json"
    write_report(path, report)
    with pytest.raises(GateFailure, match="manifest"):
        assert_report_shippable(path)


def test_refuses_to_ship_when_the_interval_still_reaches_past_the_ceiling(
    tmp_path: Path,
) -> None:
    # The shipped measurement: 17 events over 46 h is 0.3684/hour, under the
    # ceiling, with a 95% interval of 0.215 to 0.590 that is not. The gate
    # judges the upper end, so this must not ship.
    report = _report(fa=17 / 46.144601597222255, hours=46.144601597222255)
    medium = report.medium()
    assert medium.false_accepts_per_hour < FALSE_ACCEPT_MAX_PER_HOUR
    assert medium.false_accepts_ci95[1] > FALSE_ACCEPT_MAX_PER_HOUR
    path = tmp_path / "report.json"
    write_report(path, report)
    with pytest.raises(GateFailure, match="upper end"):
        assert_report_shippable(path)


def test_refuses_to_ship_a_report_with_no_intervals(tmp_path: Path) -> None:
    # A point estimate over a few dozen events cannot be judged against the
    # ceiling at all, so a report that predates the intervals fails closed
    # instead of being waved through on a flattering ratio.
    path = tmp_path / "report.json"
    write_report(path, _report(fa=0.01, intervals=False))
    with pytest.raises(GateFailure, match="cannot be judged"):
        assert_report_shippable(path)


def test_the_report_records_which_end_of_the_interval_it_judged(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    write_report(path, _report())
    gates = json.loads(path.read_text(encoding="utf-8"))["gates"]
    assert gates["false_accept_ceiling"]["judges"] == "ci95_upper"
    assert gates["recall_floor"]["judges"] == "ci95_lower"
    ceiling = gates["false_accept_ceiling"]
    assert ceiling["judged_value"] == pytest.approx(ceiling["ci95"][1])
    assert ceiling["events"] > 0 and ceiling["hours"] > 0


def test_intervals_survive_a_report_round_trip(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    write_report(path, _report())
    medium = load_report(path).medium()
    assert len(medium.false_accepts_ci95) == 2
    assert len(medium.recall_ci95) == 2
    assert medium.measured_hours == 46.0
    assert medium.measured_clips == 2000


def test_passing_report_is_shippable(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    # Comfortably inside both gates at their conservative ends: 0.02/hour over
    # 500 h bounds the rate at 0.036, and 0.99 recall over 20000 clips at 0.988.
    write_report(path, _report(fa=0.02, recall=0.99, hours=500.0))
    loaded = assert_report_shippable(path)
    medium = loaded.medium()
    assert medium.false_accepts_ci95[1] <= FALSE_ACCEPT_MAX_PER_HOUR
    assert medium.recall_ci95[0] >= RECALL_MIN
