"""Release gates: a failing report must not ship."""

from __future__ import annotations

from pathlib import Path

import pytest

from wake_word_training.gates import (
    FALSE_ACCEPT_MAX_PER_HOUR,
    RECALL_MIN,
    EvalReport,
    GateFailure,
    OperatingPoint,
    assert_report_shippable,
    write_report,
)


def _point(name: str, *, fa: float, recall: float, threshold: float) -> OperatingPoint:
    return OperatingPoint(
        name=name,
        threshold=threshold,
        false_accepts_per_hour=fa,
        recall=recall,
    )


def _report(*, fa: float = 0.1, recall: float = 0.95, licence: str = "CC-BY-4.0") -> EvalReport:
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
            _point("Low", fa=0.01, recall=0.8, threshold=0.7),
            _point("Medium", fa=fa, recall=recall, threshold=0.5),
            _point("High", fa=1.0, recall=0.99, threshold=0.3),
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


def test_passing_report_is_shippable(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    write_report(path, _report())
    loaded = assert_report_shippable(path)
    assert loaded.false_accepts_per_hour <= FALSE_ACCEPT_MAX_PER_HOUR
    assert loaded.recall >= RECALL_MIN
