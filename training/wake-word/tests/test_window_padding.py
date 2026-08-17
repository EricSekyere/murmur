"""A synthesized phrase is shorter than one head window, so it must be padded."""

from __future__ import annotations

import numpy as np
import pytest

from wake_word_training.audio import SAMPLE_RATE, pad_for_window
from wake_word_training.features import (
    MEL_FRAMES_PER_FRAME,
    MIN_HEAD_WINDOW_MELS,
    MIN_HEAD_WINDOW_SAMPLES,
)


def test_window_length_is_derived_from_the_backbone_shape():
    # 76 mel frames for the first embedding, then 8 per additional one, over
    # the 16 a head scores. At 5 mel frames per 1280 samples that is 3.2s.
    assert MIN_HEAD_WINDOW_MELS == 196
    assert MIN_HEAD_WINDOW_SAMPLES == 51_200
    assert MIN_HEAD_WINDOW_SAMPLES / SAMPLE_RATE == 3.2


def test_a_piper_length_clip_is_padded_to_a_scorable_window():
    # Piper returned ~0.65s for "Hey Murmur", which produced no windows.
    clip = np.full(int(0.65 * SAMPLE_RATE), 0.5, dtype=np.float32)
    out = pad_for_window(clip, MIN_HEAD_WINDOW_SAMPLES)
    assert len(out) == MIN_HEAD_WINDOW_SAMPLES


def test_the_phrase_ends_near_the_window_end():
    # The head scores the most recent embeddings, so at inference the phrase
    # completes at the end of the window. Training must match.
    clip = np.full(int(0.65 * SAMPLE_RATE), 0.5, dtype=np.float32)
    out = pad_for_window(clip, MIN_HEAD_WINDOW_SAMPLES)
    voiced = np.flatnonzero(np.abs(out) > 0.1)
    trailing = (len(out) - voiced[-1]) / SAMPLE_RATE
    assert trailing < 0.35, f"phrase ends {trailing:.2f}s before the window end"
    assert voiced[0] > 0, "expected leading pad, not a phrase at sample zero"


def test_a_long_clip_is_left_alone():
    clip = np.zeros(MIN_HEAD_WINDOW_SAMPLES * 2, dtype=np.float32)
    assert len(pad_for_window(clip, MIN_HEAD_WINDOW_SAMPLES)) == len(clip)


def test_padding_stays_in_range_with_a_noise_bed():
    clip = np.full(int(0.65 * SAMPLE_RATE), 0.9, dtype=np.float32)
    noise = np.random.default_rng(0).normal(0, 0.3, 4000).astype(np.float32)
    out = pad_for_window(clip, MIN_HEAD_WINDOW_SAMPLES, filler=noise)
    assert np.max(np.abs(out)) <= 1.0 + 1e-6


@pytest.mark.skipif(
    not (
        __import__("pathlib").Path(__file__).parents[1] / "data" / "backbone"
    ).is_dir(),
    reason="backbone models not downloaded",
)
def test_padded_clip_yields_windows_from_the_real_backbone():
    # Ask the backbone rather than trusting the arithmetic: the unpadded clip
    # must yield nothing and the padded one must yield at least one window.
    from pathlib import Path

    from wake_word_training.features import Backbone

    root = Path(__file__).parents[1] / "data" / "backbone"
    backbone = Backbone(root / "melspectrogram.onnx", root / "embedding_model.onnx")
    rng = np.random.default_rng(0)
    clip = rng.normal(0, 0.2, int(0.65 * SAMPLE_RATE)).astype(np.float32)

    assert len(backbone.windows(clip)) == 0
    assert len(backbone.windows(pad_for_window(clip, MIN_HEAD_WINDOW_SAMPLES))) >= 1


@pytest.mark.skipif(
    not (
        __import__("pathlib").Path(__file__).parents[1] / "data" / "backbone"
    ).is_dir(),
    reason="backbone models not downloaded",
)
def test_evaluation_pads_short_held_out_clips(tmp_path):
    # Same bug on the eval side: an unpadded held-out clip yields no windows,
    # so _score_dir would report 0.0 for every positive and recall would be 0
    # no matter how good the head is.
    from pathlib import Path

    from wake_word_training.audio import write_wav
    from wake_word_training.evaluate_lib import _score_dir
    from wake_word_training.features import Backbone

    class _ConstantSession:
        def get_inputs(self):
            class _Input:
                name = "embeddings"

            return [_Input()]

        def run(self, _outputs, feed):
            batch = next(iter(feed.values()))
            return [np.full((len(batch), 1), 0.9, dtype=np.float32)]

    root = Path(__file__).parents[1] / "data" / "backbone"
    backbone = Backbone(root / "melspectrogram.onnx", root / "embedding_model.onnx")
    clip = np.random.default_rng(0).normal(0, 0.2, int(0.65 * SAMPLE_RATE))
    write_wav(tmp_path / "clip.wav", clip.astype(np.float32))

    scores = _score_dir(backbone, _ConstantSession(), tmp_path)
    assert scores == [pytest.approx(0.9)]


@pytest.mark.skipif(
    not (
        __import__("pathlib").Path(__file__).parents[1] / "data" / "backbone"
    ).is_dir(),
    reason="backbone models not downloaded",
)
def test_backbone_still_emits_five_mel_frames():
    # MIN_HEAD_WINDOW_SAMPLES is derived from this rate. Assuming the 10ms hop
    # gave 8 and silently produced clips too short to train on, so pin it.
    from pathlib import Path

    from wake_word_training.features import FRAME_SAMPLES, Backbone

    root = Path(__file__).parents[1] / "data" / "backbone"
    backbone = Backbone(root / "melspectrogram.onnx", root / "embedding_model.onnx")
    one = np.zeros(FRAME_SAMPLES, dtype=np.float32)
    out = backbone._run(backbone._mel, one.reshape(1, -1))
    assert out.reshape(-1, 32).shape[0] == MEL_FRAMES_PER_FRAME
