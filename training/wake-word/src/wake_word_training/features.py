"""Frozen openWakeWord backbone: audio → 96-dim embeddings."""

from __future__ import annotations

from pathlib import Path

import numpy as np

from wake_word_training.allowlist import AllowlistError, refuse_forbidden_heads
from wake_word_training.audio import SAMPLE_RATE
from wake_word_training.model import EMB_DIM, HEAD_WINDOW

FRAME_SAMPLES = 1280
MEL_BINS = 32
EMB_WINDOW = 76
EMB_STEP = 8
# Mel frames the melspectrogram model emits per FRAME_SAMPLES of audio,
# measured from the shipped backbone rather than inferred from the hop size.
# `test_backbone_still_emits_five_mel_frames` fails if a future model changes
# it, because everything below is derived from this number.
MEL_FRAMES_PER_FRAME = 5

# Audio needed for a single head window, which scores HEAD_WINDOW embeddings:
# EMB_WINDOW mel frames for the first embedding, then EMB_STEP more for each
# of the rest. Clips shorter than this yield no training windows at all, which
# is silent unless it is measured, so it is derived here rather than assumed.
MIN_HEAD_WINDOW_MELS = EMB_WINDOW + (HEAD_WINDOW - 1) * EMB_STEP
MIN_HEAD_WINDOW_SAMPLES = (
    -(-MIN_HEAD_WINDOW_MELS // MEL_FRAMES_PER_FRAME) * FRAME_SAMPLES
)
# Audio between consecutive head windows: `windows` slides by one embedding,
# and one embedding step is EMB_STEP mel frames. 0.128 s, not the 0.08 s of an
# audio frame, which is why the eval refractory is not the serve frame count.
MEL_FRAME_SAMPLES = FRAME_SAMPLES // MEL_FRAMES_PER_FRAME
WINDOW_STRIDE_SECONDS = EMB_STEP * MEL_FRAME_SAMPLES / SAMPLE_RATE


class Backbone:
    """melspectrogram.onnx + embedding_model.onnx via onnxruntime."""

    def __init__(self, mel_path: Path, emb_path: Path, *, options=None) -> None:
        import onnxruntime as ort

        refuse_forbidden_heads(str(mel_path))
        refuse_forbidden_heads(str(emb_path))
        if "hey_murmur" in mel_path.name.lower():
            raise AllowlistError("melspectrogram path looks like a head")
        # `options` exists so a worker process can hold the models to one
        # thread; measured bit-identical embeddings at 1, 2, 4 and default
        # intra-op threads, so it changes throughput and nothing else.
        self._mel = ort.InferenceSession(
            str(mel_path), options, providers=["CPUExecutionProvider"]
        )
        self._emb = ort.InferenceSession(
            str(emb_path), options, providers=["CPUExecutionProvider"]
        )

    def embeddings(self, audio: np.ndarray) -> np.ndarray:
        """Return [T, 96] embeddings for 16 kHz mono float audio."""
        frames = _frame(audio, FRAME_SAMPLES)
        mels: list[np.ndarray] = []
        for frame in frames:
            out = self._run(self._mel, frame.reshape(1, -1))
            scaled = out.reshape(-1, MEL_BINS) / 10.0 + 2.0
            mels.extend(scaled)
        if len(mels) < EMB_WINDOW:
            return np.zeros((0, EMB_DIM), dtype=np.float32)
        stacked = np.stack(mels, axis=0)
        embs: list[np.ndarray] = []
        start = 0
        while start + EMB_WINDOW <= len(stacked):
            window = stacked[start : start + EMB_WINDOW]
            inp = window.reshape(1, EMB_WINDOW, MEL_BINS, 1).astype(np.float32)
            vec = self._run(self._emb, inp).reshape(-1)
            if vec.size != EMB_DIM:
                raise AllowlistError(
                    f"embedding output has {vec.size} dims, expected {EMB_DIM}"
                )
            embs.append(vec.astype(np.float32))
            start += EMB_STEP
        if not embs:
            return np.zeros((0, EMB_DIM), dtype=np.float32)
        return np.stack(embs, axis=0)

    def windows(self, audio: np.ndarray) -> np.ndarray:
        seq = self.embeddings(audio)
        if len(seq) < HEAD_WINDOW:
            return np.zeros((0, HEAD_WINDOW, EMB_DIM), dtype=np.float32)
        out = [
            seq[i : i + HEAD_WINDOW]
            for i in range(0, len(seq) - HEAD_WINDOW + 1)
        ]
        return np.stack(out, axis=0)

    def _run(self, session, array: np.ndarray) -> np.ndarray:
        name = session.get_inputs()[0].name
        result = session.run(None, {name: array.astype(np.float32)})[0]
        return np.asarray(result)


def _frame(audio: np.ndarray, size: int) -> np.ndarray:
    n = (len(audio) // size) * size
    if n == 0:
        return np.zeros((0, size), dtype=np.float32)
    return audio[:n].reshape(-1, size).astype(np.float32)
