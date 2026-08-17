"""CLI entry points: validate-only and check-report."""

from __future__ import annotations

from pathlib import Path

from wake_word_training.evaluate_lib import run as evaluate_run
from wake_word_training.gates import EvalReport, OperatingPoint, write_report
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


def _passing_report() -> EvalReport:
    return EvalReport(
        false_accepts_per_hour=0.1,
        recall=0.95,
        manifest=[{"id": "en_US-libritts-high", "kind": "voice", "licence": "CC-BY-4.0"}],
        operating_points=[
            OperatingPoint("Low", 0.7, 0.01, 0.8),
            OperatingPoint("Medium", 0.5, 0.1, 0.95),
            OperatingPoint("High", 0.3, 0.4, 0.99),
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
