"""Allowlist enforcement: non-permissive licences must not train."""

from __future__ import annotations

from pathlib import Path
from textwrap import dedent

import pytest

from wake_word_training.allowlist import (
    AllowlistError,
    assert_inputs_allowlisted,
    validate_allowlist,
)


def _write_toml(tmp_path: Path, body: str) -> Path:
    path = tmp_path / "allowlist.toml"
    path.write_text(dedent(body).lstrip(), encoding="utf-8")
    return path


def _ok_voice() -> str:
    return """
        [[voices]]
        id = "en_US-libritts-high"
        licence = "CC-BY-4.0"
        url = "https://example.invalid/libritts.onnx"
        """


def test_rejects_voice_tagged_cc_by_nc(tmp_path: Path) -> None:
    # Dropping the NC check would accept this voice and taint the head.
    path = _write_toml(
        tmp_path,
        """
        [[voices]]
        id = "en_US-amy-medium"
        licence = "CC-BY-NC-4.0"
        url = "https://example.invalid/amy.onnx"
        """,
    )
    with pytest.raises(AllowlistError, match="CC-BY-NC"):
        validate_allowlist(path)


def test_rejects_share_alike_voice(tmp_path: Path) -> None:
    path = _write_toml(
        tmp_path,
        """
        [[voices]]
        id = "en_GB-northern_english_male-medium"
        licence = "CC-BY-SA-4.0"
        url = "https://example.invalid/nem.onnx"
        """,
    )
    with pytest.raises(AllowlistError, match="CC-BY-SA"):
        validate_allowlist(path)


def test_fails_closed_when_allowlist_has_no_voices_or_datasets(tmp_path: Path) -> None:
    path = _write_toml(tmp_path, "# empty on purpose\n")
    with pytest.raises(AllowlistError, match="empty"):
        validate_allowlist(path)


def test_fails_closed_when_voice_licence_missing(tmp_path: Path) -> None:
    path = _write_toml(
        tmp_path,
        """
        [[voices]]
        id = "en_US-mystery-medium"
        url = "https://example.invalid/mystery.onnx"
        """,
    )
    with pytest.raises(AllowlistError, match="licence"):
        validate_allowlist(path)


def test_excludes_acav100m_even_if_labelled_apache(tmp_path: Path) -> None:
    path = _write_toml(
        tmp_path,
        """
        [[voices]]
        id = "en_US-libritts-high"
        licence = "CC-BY-4.0"
        url = "https://example.invalid/libritts.onnx"

        [[datasets]]
        id = "acav100m-features"
        role = "background"
        licence = "Apache-2.0"
        url = "https://huggingface.co/datasets/davidscripka/openwakeword_features"
        """,
    )
    with pytest.raises(AllowlistError, match="ACAV100M"):
        validate_allowlist(path)


def test_accepts_permissive_voice_and_dataset(tmp_path: Path) -> None:
    path = _write_toml(
        tmp_path,
        _ok_voice()
        + """
        [[datasets]]
        id = "musan"
        role = "noise"
        licence = "CC-BY-4.0"
        url = "https://www.openslr.org/17/"
        """,
    )
    parsed = validate_allowlist(path)
    assert [v.id for v in parsed.voices] == ["en_US-libritts-high"]
    assert [d.id for d in parsed.datasets] == ["musan"]


def test_refuses_input_not_in_allowlist(tmp_path: Path) -> None:
    path = _write_toml(tmp_path, _ok_voice())
    parsed = validate_allowlist(path)
    with pytest.raises(AllowlistError, match="not in the allowlist"):
        assert_inputs_allowlisted(parsed, voice_ids=["en_US-ryan-medium"])
