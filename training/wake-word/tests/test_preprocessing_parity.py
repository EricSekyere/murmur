"""Training and evaluation must feed the model through one shared pipeline.

Run 3 regression: training levelled both classes to a fixed RMS while
evaluation and the Rust runtime fed natural-level audio, and false accepts
went from 6.6/hour to 493/hour. These tests pin the properties that prevent
a recurrence: the training view of a clip is the serve view at some level,
the level applied is a clip-independent gain rather than a normalisation,
and both train_lib and evaluate_lib actually call the shared functions.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import numpy as np

from wake_word_training import evaluate_lib, windows as windows_lib
from wake_word_training.background import EVALUATION, TRAIN, background_role
from wake_word_training.audio import (
    GAIN_DB_MAX,
    GAIN_DB_MIN,
    SAMPLE_RATE,
    random_gain,
    read_audio,
    serve_window,
    train_window,
    write_wav,
)
from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES


def _clip(scale: float, seed: int = 11) -> np.ndarray:
    rng = np.random.default_rng(seed)
    return rng.normal(0.0, scale, int(0.7 * SAMPLE_RATE)).astype(np.float32)


def _rms(audio: np.ndarray) -> float:
    return float(np.sqrt(np.mean(audio.astype(np.float64) ** 2)))


def _group_with_role(role: str) -> str:
    """A LibriSpeech-shaped speaker directory that the split assigns to `role`."""
    for i in range(500):
        name = f"spk{i:03d}"
        if background_role(name) == role:
            return name
    raise AssertionError(f"no group hashes to {role}")


def _background_clip(root: Path, role: str, audio: np.ndarray) -> Path:
    path = root / _group_with_role(role) / "chapter" / "clip.wav"
    write_wav(path, audio)
    return path


class _RecordingBackbone:
    """Stands in for the ONNX backbone; captures exactly what it was fed."""

    def __init__(self) -> None:
        self.audios: list[np.ndarray] = []

    def windows(self, audio: np.ndarray) -> np.ndarray:
        self.audios.append(np.asarray(audio, dtype=np.float32))
        return np.zeros((1, 16, 96), dtype=np.float32)


class _ConstantSession:
    def get_inputs(self):
        class _Input:
            name = "embeddings"

        return [_Input()]

    def run(self, _outputs, feed):
        batch = next(iter(feed.values()))
        return [np.full((len(batch), 1), 0.5, dtype=np.float32)]


def test_train_window_is_serve_window_at_some_level() -> None:
    # With augmentation off, training must see exactly what evaluation sees,
    # times one scalar. Any step added to either side breaks this.
    clip = _clip(0.05)
    trained = train_window(clip, MIN_HEAD_WINDOW_SAMPLES, index=3)
    served = serve_window(clip, MIN_HEAD_WINDOW_SAMPLES)
    hot = np.abs(served) > 1e-4
    gains = trained[hot] / served[hot]
    assert float(np.ptp(gains)) < 1e-5, "gain varies within one clip"
    assert np.allclose(trained, served * gains[0], atol=1e-6)


def test_training_gain_is_not_a_normaliser() -> None:
    # The same waveform at two levels must come out at two levels in the same
    # ratio. RMS levelling (the run 3 bug) collapses the ratio towards 1.
    quiet = _clip(0.005)
    loud = _clip(0.05)
    ratio = _rms(random_gain(loud, 7)) / _rms(random_gain(quiet, 7))
    assert 9.0 < ratio < 11.0, f"relative level not preserved: {ratio:.2f}x"


def test_gain_spread_dominates_the_class_level_gap() -> None:
    # Measured corpus gap: positives RMS 0.1220 vs negatives 0.0268. A
    # multiplicative gain preserves that gap, so the spread must be at least
    # twice as wide or the class level distributions cannot overlap enough
    # to price the loudness shortcut out of the loss.
    gap_db = 20.0 * np.log10(0.1220 / 0.0268)
    assert GAIN_DB_MAX - GAIN_DB_MIN >= 2.0 * gap_db


def test_evaluation_feeds_the_backbone_serve_window_output(tmp_path: Path) -> None:
    clip = _clip(0.05)
    write_wav(tmp_path / "clip.wav", clip)
    decoded, rate = read_audio(tmp_path / "clip.wav")
    assert rate == SAMPLE_RATE

    backbone = _RecordingBackbone()
    evaluate_lib._score_dir(backbone, _ConstantSession(), tmp_path)
    expected = serve_window(decoded, MIN_HEAD_WINDOW_SAMPLES)
    assert len(backbone.audios) == 1
    assert np.array_equal(backbone.audios[0], expected)


def test_background_scoring_feeds_the_backbone_serve_window_output(
    tmp_path: Path,
) -> None:
    path = _background_clip(tmp_path / "bg", EVALUATION, _clip(0.02, seed=5))
    decoded, _rate = read_audio(path)
    allowlist = SimpleNamespace(
        datasets=[SimpleNamespace(id="bg", role="background")]
    )

    backbone = _RecordingBackbone()
    evaluate_lib._score_background(backbone, _ConstantSession(), allowlist, tmp_path)
    expected = serve_window(decoded, MIN_HEAD_WINDOW_SAMPLES)
    assert len(backbone.audios) == 1
    assert np.array_equal(backbone.audios[0], expected)


def test_both_training_classes_go_through_train_window(
    tmp_path: Path, monkeypatch
) -> None:
    # If either class path hand-composes its own steps again, the shared
    # function stops being the single source of truth. Spy on the call sites.
    calls: list[int] = []
    from wake_word_training import audio as audio_mod

    def spy(audio, target_len, **kwargs):
        calls.append(kwargs["index"])
        return audio_mod.train_window(audio, target_len, **kwargs)

    monkeypatch.setattr(windows_lib, "train_window", spy)

    pos_dir = tmp_path / "positives"
    pos_dir.mkdir()
    write_wav(pos_dir / "a.wav", _clip(0.05))
    windows_lib.windows_from_dir(_RecordingBackbone(), pos_dir, noise=[], rir=[])
    assert len(calls) == 1

    calls.clear()
    _background_clip(tmp_path / "bg", TRAIN, _clip(0.02, seed=5))
    allowlist = SimpleNamespace(
        datasets=[SimpleNamespace(id="bg", role="background")]
    )
    windows_lib.sample_background_windows(
        _RecordingBackbone(),
        tmp_path,
        allowlist,
        role=TRAIN,
        max_hours=1.0,
        max_windows=10,
        noise=[],
        rir=[],
    )
    assert len(calls) == 1

    calls.clear()
    windows_lib.windows_from_clips(
        _RecordingBackbone(), [_clip(0.02, seed=6)], noise=[], rir=[], max_windows=10
    )
    assert len(calls) == 1, "mined hard negatives bypassed the shared pipeline"
