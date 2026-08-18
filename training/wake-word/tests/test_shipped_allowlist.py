"""Shipped allowlist.toml must itself pass the licence gate."""

from pathlib import Path

from wake_word_training.allowlist import validate_allowlist

ROOT = Path(__file__).resolve().parents[1]


def test_shipped_allowlist_validates() -> None:
    parsed = validate_allowlist(ROOT / "allowlist.toml")
    assert parsed.voices, "allowlist must list at least one Piper voice"
    assert parsed.datasets, "allowlist must list at least one dataset"
    roles = {d.role for d in parsed.datasets}
    assert {"noise", "rir", "background"} <= roles
    assert parsed.backbone, "allowlist must pin the Apache-2.0 backbone"
