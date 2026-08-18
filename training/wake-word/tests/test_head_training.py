"""The head must stay in the range where a threshold sweep means something.

Run 4's head trained 10 000 unregularised Adam steps over ~4000 positive
windows and pinned: every held-out clip scored exactly 1.0000 or 0.0000, so
recall was 0.5 at every threshold from 0.05 to 0.95 and the sweep had nothing
to trade. These tests pin the three mechanisms that stop that: softened
targets, a stop on held-back data, and shipping the best step rather than the
last.
"""

from __future__ import annotations

import importlib.util

import numpy as np
import pytest

from wake_word_training.model import LABEL_SMOOTHING, train_head_with_trace

pytestmark = pytest.mark.skipif(
    importlib.util.find_spec("torch") is None, reason="torch not installed"
)

RAILS = 0.001


def _blobs(n: int, centre: float, seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    return (rng.normal(centre, 0.5, (n, 16, 96))).astype(np.float32)


def _overlapping(n: int, sign: float, seed: int) -> np.ndarray:
    """Classes that genuinely overlap, so a calibrated head must be unsure.

    A shifted 1536-dim blob is separable at 27 sigma however little the shift;
    the signal has to sit in one feature for the decision to be a real one.
    """
    rng = np.random.default_rng(seed)
    windows = rng.normal(0.0, 1.0, (n, 16, 96)).astype(np.float32)
    windows[:, 0, 0] += np.float32(sign * 0.6)
    return windows


def _scores(model, windows) -> np.ndarray:
    import torch

    with torch.no_grad():
        return model(torch.from_numpy(windows)).numpy().reshape(-1)


def test_scores_stay_off_the_rails_on_separable_classes() -> None:
    # Perfectly separable input, so the head certainly converges. Without
    # softened targets it drives the sigmoid to exactly 1.0 and 0.0, which is
    # what left no threshold able to trade recall for false accepts.
    pos, neg = _blobs(400, 1.5, 1), _blobs(400, -1.5, 2)
    model, _trace = train_head_with_trace(pos, neg, steps=1500)
    scores = np.concatenate([_scores(model, pos), _scores(model, neg)])
    on_rails = np.mean((scores <= RAILS) | (scores >= 1.0 - RAILS))
    assert on_rails == 0.0, f"{on_rails:.1%} of scores pinned to 0 or 1"
    assert scores.max() > 0.9 and scores.min() < 0.1, "the head learned nothing"


def test_an_ambiguous_case_gets_an_uncertain_score() -> None:
    # A head that answers 1.0000 or 0.0000 to everything cannot rank, and
    # ranking is what the threshold sweep trades on.
    pos, neg = _overlapping(400, 1.0, 3), _overlapping(400, -1.0, 4)
    model, _trace = train_head_with_trace(
        pos,
        neg,
        validation=(_overlapping(200, 1.0, 5), _overlapping(200, -1.0, 6)),
        steps=4000,
    )
    unseen = _scores(model, _overlapping(400, 1.0, 7))
    assert float(np.mean((unseen <= RAILS) | (unseen >= 1.0 - RAILS))) == 0.0
    assert 0.05 < float(np.median(unseen)) < 0.99


def test_training_stops_once_validation_stops_improving() -> None:
    pos, neg = _blobs(120, 0.3, 8), _blobs(120, -0.3, 9)
    validation = (_blobs(120, 0.3, 10), _blobs(120, -0.3, 11))
    _model, trace = train_head_with_trace(
        pos, neg, validation=validation, steps=20_000, validation_every=50, patience=5
    )
    assert trace.stopped_early
    assert trace.steps_run < 20_000
    assert trace.best_step <= trace.steps_run


def test_the_exported_weights_are_the_best_validation_step() -> None:
    # Keeping the last step is how a head that has started memorising ships.
    # The returned model must score the loss the trace claims was best.
    import torch

    pos, neg = _blobs(120, 0.3, 12), _blobs(120, -0.3, 13)
    val_pos, val_neg = _blobs(120, 0.3, 14), _blobs(120, -0.3, 15)
    model, trace = train_head_with_trace(
        pos,
        neg,
        validation=(val_pos, val_neg),
        steps=20_000,
        validation_every=50,
        patience=5,
    )
    assert trace.stopped_early
    with torch.no_grad():
        x = torch.from_numpy(np.concatenate([val_pos, val_neg]))
        y = torch.from_numpy(
            np.concatenate(
                [
                    np.full((len(val_pos), 1), 1.0 - LABEL_SMOOTHING, np.float32),
                    np.full((len(val_neg), 1), LABEL_SMOOTHING, np.float32),
                ]
            )
        )
        per_item = torch.nn.functional.binary_cross_entropy_with_logits(
            model.logits(x), y, reduction="none"
        )
    weights = np.concatenate(
        [
            np.full((len(val_pos), 1), 0.5 / len(val_pos), np.float32),
            np.full((len(val_neg), 1), 0.5 / len(val_neg), np.float32),
        ]
    )
    restored = float((per_item.numpy() * weights).sum())
    assert restored == pytest.approx(trace.best_validation_loss, abs=1e-5)


def test_no_validation_means_no_early_stop() -> None:
    pos, neg = _blobs(60, 1.0, 16), _blobs(60, -1.0, 17)
    _model, trace = train_head_with_trace(pos, neg, steps=200)
    assert trace.steps_run == 200
    assert not trace.stopped_early


def test_the_exported_graph_keeps_the_murmur_core_contract(tmp_path) -> None:
    import onnxruntime as ort

    from wake_word_training.model import export_onnx

    model, _trace = train_head_with_trace(_blobs(40, 1.0, 18), _blobs(40, -1.0, 19), steps=50)
    dest = tmp_path / "head.onnx"
    export_onnx(model, dest)
    session = ort.InferenceSession(str(dest), providers=["CPUExecutionProvider"])
    out = session.run(None, {session.get_inputs()[0].name: np.zeros((1, 16, 96), np.float32)})[0]
    assert np.asarray(out).shape == (1, 1)
    assert 0.0 <= float(np.asarray(out).reshape(-1)[0]) <= 1.0
