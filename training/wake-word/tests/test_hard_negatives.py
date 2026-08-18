"""Mined hard negatives must come from the training half and behave like negatives.

Run 5 reached 0.999 held-out recall at threshold 0.98 while roughly one
background window an hour scored above the entire positive band, so the
false-accept ceiling forced a threshold above every positive and recall fell
to 0.0005. Mining those windows is the fix; mining the gate half would train
on the measurement instead.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest

from wake_word_training import train_lib
from wake_word_training.audio import SAMPLE_RATE, write_wav
from wake_word_training.background import (
    EVALUATION,
    TRAIN,
    background_group,
    background_paths,
    background_role,
)
from wake_word_training.hard_negatives import mine_hard_clips


class _OneWindowBackbone:
    def windows(self, audio: np.ndarray) -> np.ndarray:
        return np.zeros((1, 16, 96), dtype=np.float32)


def _corpus(root: Path, speakers: int = 40) -> None:
    clip = np.full(SAMPLE_RATE, 0.05, dtype=np.float32)
    for speaker in range(speakers):
        write_wav(root / f"{speaker:03d}" / "c" / "0.wav", clip)


def _scorer(scores: list[float]):
    """Returns each queued score in turn, one clip at a time."""
    it = iter(scores)

    def score(_windows: np.ndarray) -> np.ndarray:
        return np.array([next(it)], dtype=np.float32)

    return score


def test_only_clips_the_head_scores_high_are_kept(tmp_path: Path) -> None:
    _corpus(tmp_path, speakers=4)
    paths = sorted(tmp_path.rglob("*.wav"))
    clips = mine_hard_clips(
        _OneWindowBackbone(),
        _scorer([0.9, 0.1, 0.6, 0.05]),
        paths,
        min_score=0.5,
        max_hours=10.0,
    )
    assert len(clips) == 2


def test_mining_respects_the_clip_cap(tmp_path: Path) -> None:
    _corpus(tmp_path, speakers=6)
    clips = mine_hard_clips(
        _OneWindowBackbone(),
        _scorer([0.99] * 6),
        sorted(tmp_path.rglob("*.wav")),
        min_score=0.5,
        max_hours=10.0,
        max_clips=3,
    )
    assert len(clips) == 3


def test_mining_stops_at_the_hour_cap(tmp_path: Path) -> None:
    _corpus(tmp_path, speakers=20)
    clips = mine_hard_clips(
        _OneWindowBackbone(),
        _scorer([0.99] * 20),
        sorted(tmp_path.rglob("*.wav")),
        min_score=0.5,
        max_hours=3 / 3600,
    )
    assert len(clips) <= 4


def test_mining_reads_only_training_half_background(tmp_path: Path, monkeypatch) -> None:
    # What the pipeline hands the miner decides what it can ever see. Mining
    # the gate half would train the head on its own measurement.
    from wake_word_training import hard_negatives

    _corpus(tmp_path / "librispeech")
    allowlist = SimpleNamespace(
        datasets=[SimpleNamespace(id="librispeech", role="background")]
    )
    seen: list[Path] = []

    def spy(_backbone, _score, paths, **_kwargs):
        seen.extend(paths)
        return []

    monkeypatch.setattr(hard_negatives, "mine_hard_clips", spy)
    monkeypatch.setattr(train_lib, "augmentation_pools", lambda *a, **k: ([], []))
    monkeypatch.setattr(
        train_lib,
        "windows_from_clips",
        lambda *a, **k: np.zeros((0, 16, 96), np.float32),
    )
    args = SimpleNamespace(
        data_dir=tmp_path, hard_negative_hours=1.0, hard_negative_min_score=0.5
    )
    empty = np.zeros((1, 16, 96), np.float32)
    train_lib._mine_hard_negatives(
        None, None, allowlist, args, train_lib._Windows(empty, empty, empty, empty)
    )

    gate = background_paths(tmp_path / "librispeech", role=EVALUATION)
    assert seen and gate
    assert {background_group(p) for p in seen}.isdisjoint(
        {background_group(p) for p in gate}
    )
    assert all(background_role(background_group(p)) == TRAIN for p in seen)


torch_missing = importlib.util.find_spec("torch") is None


@pytest.mark.skipif(torch_missing, reason="torch not installed")
def test_hard_negatives_take_their_share_of_every_batch() -> None:
    from wake_word_training.model import _batch, _batch_shape

    shape = _batch_shape(512, 0.25, 0.34, has_hard=True)
    assert shape.positives == 128
    assert shape.hard > 0
    assert shape.positives + shape.negatives + shape.hard == 512

    marker = 7.0
    pos = np.zeros((10, 16, 96), np.float32)
    neg = np.zeros((10, 16, 96), np.float32)
    hard = np.full((10, 16, 96), marker, np.float32)
    import torch

    xb, _yb = _batch(
        pos, neg, hard, np.random.default_rng(0), shape, (0.98, 0.02), torch.device("cpu")
    )
    drawn = int((xb.numpy().reshape(len(xb), -1)[:, 0] == marker).sum())
    assert drawn == shape.hard


@pytest.mark.skipif(torch_missing, reason="torch not installed")
def test_batches_are_negative_heavy_so_the_background_is_learnt() -> None:
    # Half-and-half batches cycled 8000 positives 29 times by the step early
    # stopping chose, against 1.15 passes over 200 000 negatives.
    from wake_word_training.model import (
        HARD_NEGATIVE_SHARE,
        POSITIVE_BATCH_FRACTION,
        _batch_shape,
    )

    shape = _batch_shape(512, POSITIVE_BATCH_FRACTION, HARD_NEGATIVE_SHARE, has_hard=False)
    assert shape.negatives > 2 * shape.positives


@pytest.mark.skipif(torch_missing, reason="torch not installed")
def test_mined_negatives_move_the_head_off_them() -> None:
    from wake_word_training.model import score_windows, train_head_with_trace

    rng = np.random.default_rng(3)
    pos = rng.normal(1.0, 0.4, (300, 16, 96)).astype(np.float32)
    neg = rng.normal(-1.0, 0.4, (300, 16, 96)).astype(np.float32)
    # Background that sits on the positive side: exactly the population that
    # outranked every positive in run 5.
    confusable = rng.normal(0.9, 0.1, (60, 16, 96)).astype(np.float32)

    plain, _ = train_head_with_trace(pos, neg, steps=800)
    mined, _ = train_head_with_trace(pos, neg, hard_negatives=confusable, steps=800)

    before = float(np.mean(score_windows(plain, confusable)))
    after = float(np.mean(score_windows(mined, confusable)))
    assert after < before - 0.2, f"confusable score only moved {before:.3f} -> {after:.3f}"
    assert float(np.mean(score_windows(mined, pos))) > 0.5, "recall was traded away"
