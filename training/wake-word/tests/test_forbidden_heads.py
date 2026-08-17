"""Forbidden upstream heads must never enter training."""

from __future__ import annotations

import pytest

from wake_word_training.allowlist import AllowlistError, refuse_forbidden_heads


def test_refuses_openwakeword_hey_jarvis_head() -> None:
    with pytest.raises(AllowlistError, match="CC BY-NC-SA"):
        refuse_forbidden_heads(
            "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/hey_jarvis_v0.1.onnx"
        )


def test_refuses_local_alexa_head_path() -> None:
    with pytest.raises(AllowlistError, match="alexa"):
        refuse_forbidden_heads("/models/alexa_v0.1.onnx")


def test_allows_backbone_melspectrogram() -> None:
    refuse_forbidden_heads(
        "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1/melspectrogram.onnx"
    )
