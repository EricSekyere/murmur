"""openWakeWord-style fully-connected wake head and ONNX export.

Input: float32 [batch, 16, 96] (16 consecutive 96-dim embeddings).
Output: float32 [batch, 1] wake score in [0, 1].
Matches the streaming contract in murmur-core's OnnxWakeScorer.
"""

from __future__ import annotations

import copy
from dataclasses import dataclass
from pathlib import Path
from typing import NamedTuple

HEAD_WINDOW = 16
EMB_DIM = 96
HIDDEN = 128
# The first head trained 10 000 unregularised Adam steps over ~4000 positive
# windows, roughly 640 passes per window, and its sigmoid pinned: every clip
# of every voice scored exactly 1.0000 or 0.0000, so no threshold in
# [0.05, 0.95] could trade recall against false accepts. Dropout, decoupled
# weight decay, softened targets and a stop on held-back speakers keep the
# head in the range where a threshold sweep means something.
DROPOUT = 0.3
WEIGHT_DECAY = 1e-2
LABEL_SMOOTHING = 0.02
VALIDATION_EVERY = 100
PATIENCE = 12
# Half-and-half batches cycled 8000 positive windows 29 times by the step
# early stopping picked, while the 200 000 negatives had been seen 1.15
# times: the head stopped because it had memorised the positives, long before
# it had learnt the background. A quarter-positive batch moves the balance to
# the class that is actually still being learnt.
POSITIVE_BATCH_FRACTION = 0.25
# Share of the negative half drawn from mined hard negatives when there are
# any. High enough to matter against 200 000 ordinary windows, low enough that
# a few hundred mined clips are not the only background the head sees.
HARD_NEGATIVE_SHARE = 0.34


@dataclass
class TrainingTrace:
    """What the run did, so a saturating head is visible without re-scoring."""

    steps_run: int
    best_step: int
    best_validation_loss: float
    stopped_early: bool
    # Split by class: a run that stops because the positives are memorised
    # looks nothing like one that stops because the background is learnt, and
    # the combined number cannot tell them apart.
    positive_validation_loss: float = float("nan")
    negative_validation_loss: float = float("nan")
    hard_negatives: int = 0


def build_head(dropout: float = DROPOUT):
    """Return an untrained WakeHead. Imports torch lazily."""
    import torch
    from torch import nn

    class WakeHead(nn.Module):
        def __init__(self) -> None:
            super().__init__()
            n_in = HEAD_WINDOW * EMB_DIM
            self.net = nn.Sequential(
                nn.Flatten(),
                nn.Linear(n_in, HIDDEN),
                nn.LayerNorm(HIDDEN),
                nn.ReLU(),
                nn.Dropout(dropout),
                nn.Linear(HIDDEN, HIDDEN),
                nn.LayerNorm(HIDDEN),
                nn.ReLU(),
                nn.Dropout(dropout),
                nn.Linear(HIDDEN, 1),
            )

        def logits(self, embeddings):  # noqa: ANN001
            return self.net(embeddings)

        def forward(self, embeddings):  # noqa: ANN001
            # The exported graph owes murmur-core a score in [0, 1]; training
            # works on the logits so the loss stays numerically stable.
            return torch.sigmoid(self.net(embeddings))

    return WakeHead()


def export_onnx(model, dest: Path) -> None:
    import torch

    dest.parent.mkdir(parents=True, exist_ok=True)
    dummy = torch.zeros(1, HEAD_WINDOW, EMB_DIM)
    model.eval()
    torch.onnx.export(
        model,
        dummy,
        str(dest),
        input_names=["embeddings"],
        output_names=["score"],
        opset_version=17,
        dynamo=False,
    )


def train_head(positives, negatives, **kwargs):
    """Train on embedding windows. Arrays are float32 [N, 16, 96].

    See `train_head_with_trace` for the keyword arguments; this returns only
    the model.
    """
    return train_head_with_trace(positives, negatives, **kwargs)[0]


def score_windows(model, windows, batch_size: int = 8192):
    """Wake scores for [N, 16, 96] windows, batched, with the model in eval mode."""
    import numpy as np
    import torch

    model.eval()
    out: list[np.ndarray] = []
    with torch.no_grad():
        for start in range(0, len(windows), batch_size):
            batch = torch.from_numpy(
                np.asarray(windows[start : start + batch_size], dtype=np.float32)
            )
            out.append(model(batch).numpy().reshape(-1))
    return np.concatenate(out) if out else np.zeros(0, dtype=np.float32)


def train_head_with_trace(
    positives,
    negatives,
    *,
    hard_negatives=None,
    validation: tuple | None = None,
    steps: int = 10_000,
    lr: float = 1e-3,
    batch_size: int = 512,
    positive_fraction: float = POSITIVE_BATCH_FRACTION,
    hard_negative_share: float = HARD_NEGATIVE_SHARE,
    weight_decay: float = WEIGHT_DECAY,
    dropout: float = DROPOUT,
    label_smoothing: float = LABEL_SMOOTHING,
    validation_every: int = VALIDATION_EVERY,
    patience: int = PATIENCE,
):
    """Train the head and report what the run did.

    `validation` is an optional (positives, negatives) pair drawn from
    speakers and background recordings held back from training. When given,
    training keeps the lowest-validation-loss weights and stops after
    `patience` checks without improvement, so the head ships from the step it
    generalised best rather than the step it memorised most.

    `hard_negatives` are background windows a previous pass scored like the
    phrase; they take `hard_negative_share` of the negative part of each batch.
    """
    import numpy as np
    import torch
    from torch import nn

    pos = np.asarray(positives, dtype=np.float32)
    neg = np.asarray(negatives, dtype=np.float32)
    hard = None if hard_negatives is None else np.asarray(hard_negatives, dtype=np.float32)
    if pos.ndim != 3 or neg.ndim != 3:
        raise ValueError("positives and negatives must be [N, 16, 96]")
    if hard is not None and len(hard) == 0:
        hard = None
    model = build_head(dropout)
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    model.to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=lr, weight_decay=weight_decay)
    loss_fn = nn.BCEWithLogitsLoss()
    rng = np.random.default_rng(0)
    targets = (1.0 - label_smoothing, label_smoothing)
    shape = _batch_shape(batch_size, positive_fraction, hard_negative_share, hard is not None)
    checker = _Validator(model, validation, device, *targets)
    stopper = _EarlyStopping(model, patience)

    step = 0
    best_parts = (float("nan"), float("nan"))
    for step in range(1, steps + 1):
        model.train()
        xb, yb = _batch(pos, neg, hard, rng, shape, targets, device)
        opt.zero_grad()
        loss_fn(model.logits(xb), yb).backward()
        opt.step()
        if not checker.active or step % validation_every:
            continue
        stop = stopper.consider(step, checker.loss())
        if stopper.best_step == step:
            best_parts = (checker.positive_loss, checker.negative_loss)
        if stop:
            break
    if checker.active:
        stopper.restore()
    model.eval()
    return model.cpu(), TrainingTrace(
        steps_run=step,
        best_step=stopper.best_step,
        best_validation_loss=stopper.best_loss if checker.active else float("nan"),
        stopped_early=checker.active and step < steps,
        positive_validation_loss=best_parts[0],
        negative_validation_loss=best_parts[1],
        hard_negatives=0 if hard is None else len(hard),
    )


class _BatchShape(NamedTuple):
    positives: int
    negatives: int
    hard: int


def _batch_shape(
    batch_size: int, positive_fraction: float, hard_share: float, has_hard: bool
) -> _BatchShape:
    n_pos = max(1, int(round(batch_size * positive_fraction)))
    n_neg = max(1, batch_size - n_pos)
    n_hard = max(1, int(round(n_neg * hard_share))) if has_hard else 0
    return _BatchShape(n_pos, n_neg - n_hard, n_hard)


def _batch(pos, neg, hard, rng, shape: _BatchShape, targets, device):
    """One shuffled batch of windows and softened labels in the given shape."""
    import numpy as np
    import torch

    hot, cold = targets
    groups = [(pos, shape.positives, hot), (neg, shape.negatives, cold)]
    if hard is not None and shape.hard:
        groups.append((hard, shape.hard, cold))
    xs, ys = [], []
    for source, count, target in groups:
        # With replacement, so a small group still fills its share: the mined
        # hard negatives are deliberately few and must not shrink the batch.
        picked = rng.integers(0, len(source), size=count)
        xs.append(source[picked])
        ys.append(np.full((len(picked), 1), target, np.float32))
    x = np.concatenate(xs, axis=0)
    y = np.concatenate(ys, axis=0)
    perm = rng.permutation(len(x))
    return torch.from_numpy(x[perm]).to(device), torch.from_numpy(y[perm]).to(device)


class _EarlyStopping:
    """Keeps the lowest-validation-loss weights and calls time on the run."""

    def __init__(self, model, patience: int) -> None:
        self._model = model
        self._patience = patience
        self._state = copy.deepcopy(model.state_dict())
        self._stale = 0
        self.best_loss = float("inf")
        self.best_step = 0

    def consider(self, step: int, loss: float) -> bool:
        """Record a validation reading; True means training should stop."""
        if loss < self.best_loss - 1e-5:
            self.best_loss, self.best_step, self._stale = loss, step, 0
            self._state = copy.deepcopy(self._model.state_dict())
            return False
        self._stale += 1
        return self._stale >= self._patience

    def restore(self) -> None:
        self._model.load_state_dict(self._state)


class _Validator:
    """Balanced validation loss over held-back speakers and background."""

    def __init__(self, model, validation, device, hot: float, cold: float) -> None:
        import numpy as np
        import torch

        self.active = False
        self.positive_loss = float("nan")
        self.negative_loss = float("nan")
        if validation is None:
            return
        val_pos, val_neg = validation
        val_pos = np.asarray(val_pos, dtype=np.float32)
        val_neg = np.asarray(val_neg, dtype=np.float32)
        if len(val_pos) == 0 or len(val_neg) == 0:
            return
        self.active = True
        self._model = model
        self._torch = torch
        self._split = len(val_pos)
        self._x = torch.from_numpy(np.concatenate([val_pos, val_neg], axis=0)).to(device)
        self._y = torch.from_numpy(
            np.concatenate(
                [
                    np.full((len(val_pos), 1), hot, np.float32),
                    np.full((len(val_neg), 1), cold, np.float32),
                ]
            )
        ).to(device)
        # Positives are far rarer than background windows, so an unweighted
        # mean would let the negatives alone decide when to stop.
        weights = np.concatenate(
            [
                np.full((len(val_pos), 1), 0.5 / len(val_pos), np.float32),
                np.full((len(val_neg), 1), 0.5 / len(val_neg), np.float32),
            ]
        )
        self._w = torch.from_numpy(weights).to(device)

    def loss(self) -> float:
        torch = self._torch
        self._model.eval()
        positive = 0.0
        negative = 0.0
        with torch.no_grad():
            for start in range(0, len(self._x), 4096):
                stop = start + 4096
                weighted = (
                    torch.nn.functional.binary_cross_entropy_with_logits(
                        self._model.logits(self._x[start:stop]),
                        self._y[start:stop],
                        reduction="none",
                    )
                    * self._w[start:stop]
                )
                cut = min(max(self._split - start, 0), len(weighted))
                positive += float(weighted[:cut].sum())
                negative += float(weighted[cut:].sum())
        self.positive_loss, self.negative_loss = positive, negative
        return positive + negative
