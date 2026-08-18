"""The RIR pool must hold impulse responses, not the corpus's noise tracks.

RIRS_NOISES ships pointsource and isotropic noise recordings (up to 71s)
beside the RIRs, named *noise*. Globbing the whole tree convolved 23% of
positives with noise, producing unintelligible clips still labelled
"Hey Murmur", which feeds false accepts directly.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import numpy as np

from wake_word_training.audio import write_wav
from wake_word_training.windows import load_pool


def _dataset_tree(tmp_path: Path, dataset_id: str) -> Path:
    root = tmp_path / dataset_id
    clip = np.full(320, 0.1, dtype=np.float32)
    for name in (
        "pointsource_noises/noise-free-sound-0000.wav",
        "real_rirs_isotropic_noises/RVB2014_type1_noise_largeroom1_1.wav",
        "real_rirs_isotropic_noises/RVB2014_type1_rir_largeroom1_1.wav",
        "simulated_rirs/smallroom/Room001/Room001-00001.wav",
    ):
        write_wav(root / name, clip)
    return root


def _allowlist(dataset_id: str, role: str) -> SimpleNamespace:
    return SimpleNamespace(
        datasets=[SimpleNamespace(id=dataset_id, role=role, licence="Apache-2.0")]
    )


def test_rir_pool_excludes_noise_recordings(tmp_path: Path) -> None:
    _dataset_tree(tmp_path, "rirs_noises")
    pool = load_pool(tmp_path, _allowlist("rirs_noises", "rir"), role="rir")
    assert len(pool) == 2


def test_noise_pool_keeps_noise_recordings(tmp_path: Path) -> None:
    # The filter is about what a convolution kernel may be; an additive noise
    # pool obviously keeps its noise files.
    _dataset_tree(tmp_path, "musan")
    pool = load_pool(tmp_path, _allowlist("musan", "noise"), role="noise")
    assert len(pool) == 4
