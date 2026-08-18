"""Background loading must not materialize a 500-hour corpus."""

from __future__ import annotations

from pathlib import Path

import numpy as np

from wake_word_training.audio import DEFAULT_MAX_BACKGROUND_HOURS, iter_clips_capped


def test_default_background_cap_is_documented() -> None:
    assert DEFAULT_MAX_BACKGROUND_HOURS > 0
    assert DEFAULT_MAX_BACKGROUND_HOURS < 500


def test_iter_clips_capped_does_not_materialize_full_tree(
    tmp_path: Path, monkeypatch
) -> None:
    root = tmp_path / "librispeech"
    root.mkdir()
    for i in range(40):
        (root / f"{i:02d}.flac").write_bytes(b"not-audio")

    reads: list[Path] = []

    def fake_read(path: Path):
        reads.append(path)
        return np.ones(16_000, dtype=np.float32), 16_000

    monkeypatch.setattr("wake_word_training.audio.read_audio", fake_read)
    clips = list(iter_clips_capped(root, max_hours=5 / 3600))
    assert len(reads) < 40
    assert 5 <= len(reads) <= 6
    assert len(clips) == len(reads)
    assert sum(len(audio) for audio, _path in clips) / 16_000 <= 6
