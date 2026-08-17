"""openWakeWord-style fully-connected wake head and ONNX export.

Input: float32 [batch, 16, 96] (16 consecutive 96-dim embeddings).
Output: float32 [batch, 1] wake score in [0, 1].
Matches the streaming contract in murmur-core's OnnxWakeScorer.
"""

from __future__ import annotations

from pathlib import Path

HEAD_WINDOW = 16
EMB_DIM = 96
HIDDEN = 128


def build_head():
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
                nn.Linear(HIDDEN, HIDDEN),
                nn.LayerNorm(HIDDEN),
                nn.ReLU(),
                nn.Linear(HIDDEN, 1),
                nn.Sigmoid(),
            )

        def forward(self, embeddings):  # noqa: ANN001
            return self.net(embeddings)

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


def train_head(
    positives,
    negatives,
    *,
    steps: int = 10_000,
    lr: float = 1e-3,
    batch_size: int = 512,
):
    """Train on embedding windows. Arrays are float32 [N, 16, 96]."""
    import numpy as np
    import torch
    from torch import nn

    model = build_head()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    model.to(device)
    opt = torch.optim.Adam(model.parameters(), lr=lr)
    loss_fn = nn.BCELoss()
    rng = np.random.default_rng(0)
    pos = np.asarray(positives, dtype=np.float32)
    neg = np.asarray(negatives, dtype=np.float32)
    if pos.ndim != 3 or neg.ndim != 3:
        raise ValueError("positives and negatives must be [N, 16, 96]")
    half = max(1, batch_size // 2)
    model.train()
    for _ in range(steps):
        pi = rng.integers(0, len(pos), size=min(half, len(pos)))
        ni = rng.integers(0, len(neg), size=min(half, len(neg)))
        x = np.concatenate([pos[pi], neg[ni]], axis=0)
        y = np.concatenate(
            [np.ones((len(pi), 1), np.float32), np.zeros((len(ni), 1), np.float32)]
        )
        perm = rng.permutation(len(x))
        xb = torch.from_numpy(x[perm]).to(device)
        yb = torch.from_numpy(y[perm]).to(device)
        opt.zero_grad()
        pred = model(xb)
        loss = loss_fn(pred, yb)
        loss.backward()
        opt.step()
    model.eval()
    return model.cpu()
