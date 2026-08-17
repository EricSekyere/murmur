"""Background the trained head mistakes for the phrase, fed back as negatives.

Run 5 separated the classes but not far enough: held-out recall reached 0.999
at threshold 0.98 across 155 unseen speakers, while roughly one background
window an hour scored *above* the entire positive band. The false-accept
ceiling then forced a threshold above every positive and recall fell to
0.0005. Those windows are a small, specific population, so the fix is to find
them and train against them.

Mining scores the serve view of the audio, because that is the view the
false-accept gate counts, but keeps the offending clip rather than the window:
the caller feeds it back through the same `train_window` augmentation as every
other negative, so hard negatives cannot become a class the head separates on
level or cleanliness.

Only training-half background is ever mined. Mining the gate half would train
on the measurement.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from pathlib import Path

import numpy as np

from wake_word_training.audio import SAMPLE_RATE, read_audio, resample_16k, serve_window
from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

DEFAULT_MIN_SCORE = 0.5
DEFAULT_MAX_HOURS = 35.0
DEFAULT_MAX_CLIPS = 3_000
DEFAULT_MAX_HARD_WINDOWS = 60_000


def mine_hard_clips(
    backbone,
    score: Callable[[np.ndarray], np.ndarray],
    paths: Iterable[Path],
    *,
    max_hours: float = DEFAULT_MAX_HOURS,
    min_score: float = DEFAULT_MIN_SCORE,
    max_clips: int = DEFAULT_MAX_CLIPS,
) -> list[np.ndarray]:
    """Audio of the background clips whose best serve window reaches `min_score`.

    `score` takes [N, 16, 96] windows and returns N scores in [0, 1]; injecting
    it keeps this module free of torch and lets a test drive the decision.
    """
    hard: list[np.ndarray] = []
    hours = 0.0
    for path in paths:
        if hours >= max_hours or len(hard) >= max_clips:
            break
        samples, rate = read_audio(path)
        audio = resample_16k(samples, rate)
        hours += len(audio) / SAMPLE_RATE / 3600.0
        windows = backbone.windows(serve_window(audio, MIN_HEAD_WINDOW_SAMPLES))
        if len(windows) == 0:
            continue
        if float(np.max(score(windows))) >= min_score:
            hard.append(audio)
    return hard
