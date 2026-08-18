"""Audio files to wake scores, through the one serve-side pipeline.

Every score in the report comes from `score_file`, so the calibration half and
the gate half cannot drift apart in preprocessing. Scores stay grouped by file
because the false-accept count applies a refractory window, and a refractory
that carries across a file boundary suppresses hits in a recording that has
nothing to do with the one that fired.
"""

from __future__ import annotations

import os
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import NamedTuple

from wake_word_training.audio import SAMPLE_RATE, read_audio, resample_16k, serve_window
from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

# The backbone runs at ~4.5x realtime and does not scale past one core (mel and
# embedding are too small for intra-op threading; measured identical scores at
# 1, 2, 4 and default threads). 54 h of audio is therefore ~7 h in one process
# and under an hour across the machine, which is what makes re-running the
# protocol cheap enough to check.
WORKER_THREADS = 1


class Models(NamedTuple):
    """The two backbone sessions plus the trained head."""

    backbone: object
    head: object


class FileScores(NamedTuple):
    path: Path
    scores: list[float]
    seconds: float


def load_models(backbone_dir: Path, head_path: Path, *, threads: int | None = None) -> Models:
    import onnxruntime as ort

    from wake_word_training.features import Backbone

    options = None
    if threads:
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
    backbone = Backbone(
        backbone_dir / "melspectrogram.onnx",
        backbone_dir / "embedding_model.onnx",
        options=options,
    )
    head = ort.InferenceSession(
        str(head_path), options, providers=["CPUExecutionProvider"]
    )
    return Models(backbone, head)


def score_windows(session, windows) -> list[float]:
    import numpy as np

    if len(windows) == 0:
        return []
    name = session.get_inputs()[0].name
    scores: list[float] = []
    # The exported head fixes the batch dimension at 1, matching the
    # streaming contract in murmur-core's OnnxWakeScorer ([1, 16, 96]),
    # so windows are scored one at a time rather than as one batch.
    for window in np.asarray(windows, dtype=np.float32):
        out = session.run(None, {name: window[None, ...]})[0]
        scores.append(float(np.asarray(out).reshape(-1)[0]))
    return scores


def score_file(models: Models, path: Path) -> FileScores:
    """One file's window scores and its duration.

    Held-out clips are bare Piper phrases shorter than one head window, so
    they are seated exactly as the deployed scorer would see them; without
    that every positive scores 0.0. Background files go through the same call
    so a short recording still yields windows instead of only counting hours.
    """
    samples, rate = read_audio(path)
    audio = resample_16k(samples, rate)
    windows = models.backbone.windows(serve_window(audio, MIN_HEAD_WINDOW_SAMPLES))
    return FileScores(path, score_windows(models.head, windows), len(audio) / SAMPLE_RATE)


def score_files(models: Models, paths) -> list[FileScores]:
    return [score_file(models, path) for path in paths]


def default_workers() -> int:
    # One spare core so a long scoring run leaves the machine usable.
    return max(1, (os.cpu_count() or 2) - 1)


def score_files_parallel(
    backbone_dir: Path, head_path: Path, paths, *, workers: int | None = None
) -> list[FileScores]:
    """`score_files` spread over worker processes, in the same order.

    Identical per-file work: each worker calls `score_file`, so parallelism
    cannot change a score, only when it is computed.
    """
    paths = list(paths)
    count = workers if workers is not None else default_workers()
    if count <= 1 or len(paths) <= 1:
        return score_files(load_models(backbone_dir, head_path, threads=WORKER_THREADS), paths)
    with ProcessPoolExecutor(
        max_workers=count,
        initializer=_init_worker,
        initargs=(backbone_dir, head_path),
    ) as pool:
        return list(pool.map(_score_in_worker, paths, chunksize=8))


_WORKER_MODELS: Models | None = None


def _init_worker(backbone_dir: Path, head_path: Path) -> None:
    global _WORKER_MODELS
    _WORKER_MODELS = load_models(backbone_dir, head_path, threads=WORKER_THREADS)


def _score_in_worker(path: Path) -> FileScores:
    if _WORKER_MODELS is None:
        raise RuntimeError("worker was not initialised with models")
    return score_file(_WORKER_MODELS, path)
