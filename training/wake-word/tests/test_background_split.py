"""False accepts must be measured on background the head never trained on.

Training took background files in sorted order until the window cap, which
stopped after 690 files: 2.4 hours from 7 of LibriSpeech's 251 speakers. The
gate then counted false accepts over the whole corpus, so the head met 244
unseen speakers for the first time at gate time and scored 6.02/hour against a
0.5 ceiling.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import numpy as np

from wake_word_training import evaluate_lib
from wake_word_training.audio import write_wav
from wake_word_training.background import (
    EVALUATION,
    TRAIN,
    VALIDATION,
    background_group,
    background_paths,
    background_role,
)

SPEAKERS = 60
FILES_PER_SPEAKER = 8


def _corpus(root: Path) -> None:
    clip = np.full(1600, 0.05, dtype=np.float32)
    for speaker in range(SPEAKERS):
        for chapter in range(2):
            for utterance in range(FILES_PER_SPEAKER // 2):
                write_wav(
                    root / f"{speaker:03d}" / f"{chapter}" / f"{utterance}.wav", clip
                )


def test_group_is_the_librispeech_speaker_directory() -> None:
    path = Path("data/librispeech/LibriSpeech/train-clean-100/103/1240/103-1240-0000.flac")
    assert background_group(path) == "103"


def test_every_group_lands_in_exactly_one_role() -> None:
    roles = {f"{i:03d}": background_role(f"{i:03d}") for i in range(SPEAKERS)}
    assert set(roles.values()) == {TRAIN, VALIDATION, EVALUATION}


def test_training_and_gate_background_never_share_a_speaker(tmp_path: Path) -> None:
    _corpus(tmp_path)
    trained = {background_group(p) for p in background_paths(tmp_path, role=TRAIN)}
    gated = {background_group(p) for p in background_paths(tmp_path, role=EVALUATION)}
    validated = {background_group(p) for p in background_paths(tmp_path, role=VALIDATION)}
    assert trained and gated and validated
    assert trained.isdisjoint(gated)
    assert trained.isdisjoint(validated)
    assert validated.isdisjoint(gated)


def test_every_file_of_a_speaker_stays_on_one_side(tmp_path: Path) -> None:
    _corpus(tmp_path)
    by_group: dict[str, set[str]] = {}
    for role in (TRAIN, VALIDATION, EVALUATION):
        for path in background_paths(tmp_path, role=role):
            by_group.setdefault(background_group(path), set()).add(role)
    assert all(len(roles) == 1 for roles in by_group.values())


def test_the_window_cap_sees_many_speakers_not_a_prefix(tmp_path: Path) -> None:
    # Training stops at a window cap long before the corpus ends, so what the
    # head sees is decided by the order. Sorted order spends the whole budget
    # on the first speaker's files.
    _corpus(tmp_path)
    paths = background_paths(tmp_path, role=TRAIN)
    n_train_speakers = len({background_group(p) for p in paths})
    prefix = paths[:n_train_speakers]
    assert len({background_group(p) for p in prefix}) == n_train_speakers


def test_the_evaluator_scores_only_the_gate_half(tmp_path: Path, monkeypatch) -> None:
    _corpus(tmp_path / "bg")
    allowlist = SimpleNamespace(datasets=[SimpleNamespace(id="bg", role="background")])
    seen: list[Path] = []
    original = evaluate_lib.read_audio

    class _EmptyBackbone:
        def windows(self, audio: np.ndarray) -> np.ndarray:
            return np.zeros((0, 16, 96), dtype=np.float32)

    def spy(path: Path):
        seen.append(path)
        return original(path)

    monkeypatch.setattr(evaluate_lib, "read_audio", spy)
    evaluate_lib._score_background(_EmptyBackbone(), None, allowlist, tmp_path)

    assert seen, "the evaluator scored nothing"
    assert {background_group(p) for p in seen} == {
        background_group(p) for p in background_paths(tmp_path / "bg", role=EVALUATION)
    }
    assert {background_group(p) for p in seen}.isdisjoint(
        {background_group(p) for p in background_paths(tmp_path / "bg", role=TRAIN)}
    )
