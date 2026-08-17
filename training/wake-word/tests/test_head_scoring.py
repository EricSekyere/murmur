"""The exported head fixes batch at 1; the evaluator must feed it that way."""

from __future__ import annotations

import numpy as np

from wake_word_training.evaluate_lib import _score_windows


class _Batch1Session:
    """Rejects batches like the exported head does (onnxruntime INVALID_ARGUMENT)."""

    def __init__(self) -> None:
        self.seen: list[tuple] = []

    def get_inputs(self):
        class _Input:
            name = "embeddings"

        return [_Input()]

    def run(self, _outputs, feed):
        batch = next(iter(feed.values()))
        self.seen.append(batch.shape)
        if batch.shape[0] != 1:
            raise ValueError(f"expected batch 1, got {batch.shape[0]}")
        return [np.full((1, 1), 0.5, dtype=np.float32)]


def test_score_windows_feeds_the_streaming_batch_of_one():
    session = _Batch1Session()
    windows = np.zeros((3, 16, 96), dtype=np.float32)

    assert _score_windows(session, windows) == [0.5, 0.5, 0.5]
    assert session.seen == [(1, 16, 96)] * 3


def test_score_windows_handles_empty_input():
    assert _score_windows(_Batch1Session(), np.zeros((0, 16, 96), np.float32)) == []
