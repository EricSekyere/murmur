"""CLI entry points: validate-only and check-report."""

from __future__ import annotations

from pathlib import Path

from wake_word_training.evaluate_lib import run as evaluate_run
from wake_word_training.gates import EvalReport, OperatingPoint, write_report
from wake_word_training.intervals import (
    poisson_rate_interval,
    wilson_proportion_interval,
)
from wake_word_training.train_lib import run as train_run

ROOT = Path(__file__).resolve().parents[1]


def test_train_validate_only_accepts_shipped_allowlist() -> None:
    assert train_run(["--validate-only", "--allowlist", str(ROOT / "allowlist.toml")]) == 0


def test_evaluate_check_report_refuses_failing_gates(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    write_report(
        path,
        EvalReport(
            false_accepts_per_hour=2.0,
            recall=0.5,
            manifest=[{"id": "x", "kind": "voice", "licence": "CC-BY-NC-4.0"}],
            operating_points=[
                OperatingPoint("Low", 0.8, 1.0, 0.4),
                OperatingPoint("Medium", 0.5, 2.0, 0.5),
                OperatingPoint("High", 0.2, 5.0, 0.7),
            ],
        ),
    )
    assert evaluate_run(["--check-report", str(path)]) == 2


def _measured(name: str, threshold: float, fa: float, recall: float) -> OperatingPoint:
    """A point with the intervals a real evaluation reports.

    500 h of background and 20 000 clips, so the conservative end of each
    interval is inside its gate and the CLI is exercised on a shippable report
    rather than on one the gates would refuse for lacking intervals.
    """
    hours, clips = 500.0, 20_000
    events = round(fa * hours)
    return OperatingPoint(
        name=name,
        threshold=threshold,
        false_accepts_per_hour=fa,
        recall=recall,
        false_accept_events=events,
        measured_hours=hours,
        measured_clips=clips,
        false_accepts_ci95=list(poisson_rate_interval(events, hours)),
        recall_ci95=list(wilson_proportion_interval(round(recall * clips), clips)),
    )


def _passing_report() -> EvalReport:
    return EvalReport(
        false_accepts_per_hour=0.1,
        recall=0.95,
        manifest=[{"id": "en_US-libritts-high", "kind": "voice", "licence": "CC-BY-4.0"}],
        operating_points=[
            _measured("Low", 0.7, 0.01, 0.92),
            _measured("Medium", 0.5, 0.1, 0.95),
            _measured("High", 0.3, 0.4, 0.99),
        ],
    )


def test_evaluate_without_report_flag_writes_output_dir_report(
    tmp_path: Path, monkeypatch
) -> None:
    # Omitting --report must still satisfy the spec path output-dir/report.json.
    out = tmp_path / "output"
    monkeypatch.setattr(
        "wake_word_training.evaluate_lib._evaluate",
        lambda allowlist, args: _passing_report(),
    )
    rc = evaluate_run(
        [
            "--allowlist",
            str(ROOT / "allowlist.toml"),
            "--output-dir",
            str(out),
            "--data-dir",
            str(tmp_path),
        ]
    )
    assert rc == 0
    assert (out / "report.json").is_file()


def test_evaluate_check_report_accepts_passing_gates(tmp_path: Path) -> None:
    path = tmp_path / "report.json"
    write_report(path, _passing_report())
    assert evaluate_run(["--check-report", str(path)]) == 0
