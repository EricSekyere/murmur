"""Augmentation helpers keep clips in range without pulling Piper."""

from pathlib import Path

import numpy as np
import soundfile as sf

from wake_word_training.audio import apply_rir, augment_clip, mix_noise, read_wav


def test_mix_noise_does_not_mute_speech() -> None:
    speech = np.ones(1600, dtype=np.float32) * 0.2
    noise = np.ones(3200, dtype=np.float32) * 0.05
    mixed = mix_noise(speech, noise, snr_db=10.0)
    assert mixed.shape == speech.shape
    assert float(np.max(np.abs(mixed))) > 0.0


def test_rir_convolution_preserves_length() -> None:
    speech = np.random.default_rng(0).standard_normal(800).astype(np.float32)
    rir = np.array([1.0, 0.3, 0.1], dtype=np.float32)
    wet = apply_rir(speech, rir)
    assert wet.shape == speech.shape


def test_augment_clip_normalizes() -> None:
    speech = np.ones(400, dtype=np.float32)
    out = augment_clip(speech, noise=None, rir=None)
    assert float(np.max(np.abs(out))) <= 1.0 + 1e-5


def test_read_wav_accepts_flac(tmp_path: Path) -> None:
    # OpenSLR LibriSpeech ships FLAC; stdlib wave cannot ingest it.
    path = tmp_path / "clip.flac"
    expected = np.zeros(1600, dtype=np.float32)
    expected[::16] = 0.25
    sf.write(str(path), expected, 16_000)
    decoded, rate = read_wav(path)
    assert rate == 16_000
    assert decoded.shape[0] == expected.shape[0]
    assert float(np.max(np.abs(decoded))) > 0.0
