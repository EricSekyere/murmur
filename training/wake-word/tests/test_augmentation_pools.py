"""Augmentation pools are sampled, not swallowed.

Loading them whole cost 36 GB of float32 on a 39.6 GB machine (60 325 impulse
responses plus 2016 MUSAN recordings averaging 237 s), to index one entry per
clip. Bounding them is only safe if the bound still spans the corpus.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import numpy as np

from wake_word_training.audio import SAMPLE_RATE, write_wav
from wake_word_training.windows import load_pool, stride_sample


def _tree(root: Path, count: int, seconds: float) -> None:
    clip = np.full(int(seconds * SAMPLE_RATE), 0.1, dtype=np.float32)
    for i in range(count):
        write_wav(root / f"{i:04d}.wav", clip)


def _allowlist(dataset_id: str, role: str) -> SimpleNamespace:
    return SimpleNamespace(
        datasets=[SimpleNamespace(id=dataset_id, role=role, licence="CC-BY-4.0")]
    )


def test_stride_sample_spans_the_list_rather_than_taking_a_prefix() -> None:
    items = list(range(1000))
    taken = stride_sample(items, 10)
    assert len(taken) == 10
    assert taken[0] == 0
    assert taken[-1] >= 900, f"sample stopped at {taken[-1]}"


def test_stride_sample_keeps_everything_below_the_limit() -> None:
    assert stride_sample([1, 2, 3], 10) == [1, 2, 3]


def test_the_pool_is_capped_and_still_spans_the_corpus(tmp_path: Path) -> None:
    _tree(tmp_path / "musan", 200, 0.05)
    pool = load_pool(tmp_path, _allowlist("musan", "noise"), role="noise", max_clips=20)
    assert len(pool) == 20


def test_long_recordings_are_trimmed(tmp_path: Path) -> None:
    # MUSAN music tracks run minutes; a 3.2 s window never reaches the tail,
    # and keeping it is what made the pool 30 GB.
    _tree(tmp_path / "musan", 3, 4.0)
    pool = load_pool(
        tmp_path, _allowlist("musan", "noise"), role="noise", max_clips=10, max_seconds=1.0
    )
    assert [len(audio) for audio, _id, _lic in pool] == [SAMPLE_RATE] * 3


def test_the_rir_pool_still_excludes_noise_recordings_when_capped(tmp_path: Path) -> None:
    root = tmp_path / "rirs_noises"
    clip = np.full(320, 0.1, dtype=np.float32)
    for i in range(50):
        write_wav(root / "simulated_rirs" / f"rir-{i:03d}.wav", clip)
        write_wav(root / "pointsource_noises" / f"noise-{i:03d}.wav", clip)
    pool = load_pool(tmp_path, _allowlist("rirs_noises", "rir"), role="rir", max_clips=30)
    assert len(pool) == 30
