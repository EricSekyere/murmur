"""Audio corpora to embedding windows, through the one shared augmentation.

Everything that turns files on disk into `[N, 16, 96]` arrays lives here:
the bounded augmentation pools, the two class extractors, and the level guard
that refuses a corpus whose classes differ in loudness rather than content.
Both classes go through `train_window`, and the parity tests assert it.
"""

from __future__ import annotations

from pathlib import Path

from wake_word_training.allowlist import Allowlist, AllowlistError
from wake_word_training.audio import (
    DEFAULT_MAX_NOISE_CLIPS,
    DEFAULT_MAX_NOISE_SECONDS,
    DEFAULT_MAX_RIR_CLIPS,
    DEFAULT_MAX_RIR_SECONDS,
    SAMPLE_RATE,
    iter_audio_files,
    iter_clips_from,
    read_audio,
    resample_16k,
    train_window,
)
from wake_word_training.background import background_paths

EMPTY_SHAPE = (0, 16, 96)
# Positives were peak-normalised and negatives were not, so the head learned
# level instead of the phrase. Anything past this ratio is that bug returning.
MAX_CLASS_LEVEL_RATIO = 1.5


def augmentation_pools(data_root: Path, allowlist: Allowlist) -> tuple[list, list]:
    """The (noise, rir) pools a run augments with."""
    noise = load_pool(
        data_root,
        allowlist,
        role="noise",
        max_clips=DEFAULT_MAX_NOISE_CLIPS,
        max_seconds=DEFAULT_MAX_NOISE_SECONDS,
    )
    rir = load_pool(
        data_root,
        allowlist,
        role="rir",
        max_clips=DEFAULT_MAX_RIR_CLIPS,
        max_seconds=DEFAULT_MAX_RIR_SECONDS,
    )
    return noise, rir


def load_pool(
    data_root: Path,
    allowlist: Allowlist,
    *,
    role: str,
    max_clips: int = DEFAULT_MAX_RIR_CLIPS,
    max_seconds: float = DEFAULT_MAX_NOISE_SECONDS,
) -> list[tuple]:
    """An evenly-spread, bounded sample of one augmentation corpus.

    Loading the corpora whole cost 36 GB of float32 on a 39.6 GB machine and
    most of the run's wall clock, to index one entry per clip. Sampling with a
    stride keeps the spread of a full pool without the resident set.
    """
    pool: list[tuple] = []
    limit = int(max_seconds * SAMPLE_RATE)
    for dataset in allowlist.datasets:
        if dataset.role != role:
            continue
        paths = [
            path
            for path in iter_audio_files(data_root / dataset.id)
            # RIRS_NOISES ships pointsource/isotropic noise recordings in the
            # same tree as the impulse responses, named *noise*. Globbing them
            # into the RIR pool convolved 23% of positives with up to 71s of
            # noise, burying the phrase in clips still labelled positive.
            if not (role == "rir" and "noise" in path.name.lower())
        ]
        for path in stride_sample(paths, max_clips):
            samples, rate = read_audio(path)
            pool.append((resample_16k(samples, rate)[:limit], dataset.id, dataset.licence))
    return pool


def stride_sample(items: list, limit: int) -> list:
    """`limit` items spread across the list, not its first `limit`."""
    if limit <= 0 or len(items) <= limit:
        return items
    step = len(items) / limit
    return [items[int(i * step)] for i in range(limit)]


def windows_from_dir(backbone, clips_dir: Path, *, noise, rir):
    """Every clip under a directory, augmented, as head windows."""
    import numpy as np

    from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

    windows = []
    wavs = list(iter_audio_files(clips_dir))
    for i, wav in enumerate(wavs):
        samples, rate = read_audio(wav)
        audio = resample_16k(samples, rate)
        augmented = train_window(
            audio,
            MIN_HEAD_WINDOW_SAMPLES,
            index=i,
            noise=_pick(noise, i),
            rir=_pick(rir, i),
        )
        windows.append(backbone.windows(augmented))
    if not windows:
        raise AllowlistError(f"no clips under {clips_dir}")
    stacked = [w for w in windows if len(w)]
    if not stacked:
        # read_audio returns clips at their native rate (Piper writes 22050 Hz).
        shortest = min(
            len(samples) / rate for samples, rate in (read_audio(w) for w in wavs)
        )
        raise AllowlistError(
            "clips produced no embedding windows: a head window needs "
            f"{MIN_HEAD_WINDOW_SAMPLES / SAMPLE_RATE:.2f}s of audio and the "
            f"shortest clip under {clips_dir} is {shortest:.2f}s"
        )
    return np.concatenate(stacked, axis=0)


def windows_from_clips(backbone, clips, *, noise, rir, max_windows: int):
    """Already-decoded 16 kHz clips, augmented, as head windows."""
    import numpy as np

    from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

    collected: list = []
    total = 0
    for i, audio in enumerate(clips):
        if total >= max_windows:
            break
        found = backbone.windows(
            train_window(
                audio,
                MIN_HEAD_WINDOW_SAMPLES,
                index=i,
                noise=_pick(noise, i),
                rir=_pick(rir, i),
            )
        )
        if len(found) == 0:
            continue
        collected.append(found)
        total += len(found)
    if not collected:
        return np.zeros(EMPTY_SHAPE, dtype=np.float32)
    return np.concatenate(collected, axis=0)[:max_windows]


def sample_background_windows(
    backbone,
    data_root: Path,
    allowlist: Allowlist,
    *,
    role: str = "train",
    max_hours: float,
    max_windows: int,
    noise,
    rir,
):
    """Stream background clips until hour or window caps; never load the full tree.

    `role` selects one side of the speaker split in `background.py`: the
    window cap is reached long before the corpus ends, so the order matters as
    much as the filter. Taking the sorted prefix gave the head 7 of 251
    LibriSpeech speakers, and the false-accept gate then measured the other
    244.
    """
    import numpy as np

    from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

    collected: list = []
    hours_left = max_hours
    n_windows = 0
    for dataset in allowlist.datasets:
        if dataset.role != "background":
            continue
        if hours_left <= 0 or n_windows >= max_windows:
            break
        paths = background_paths(data_root / dataset.id, role=role)
        for i, (audio, _path) in enumerate(iter_clips_from(paths, max_hours=hours_left)):
            hours_left -= len(audio) / SAMPLE_RATE / 3600.0
            # Identical treatment to positives, through the one shared
            # pipeline. Hand-composed per-class chains are how negatives once
            # stayed 4.5x quieter than positives and the head learned level
            # instead of the phrase.
            windows = backbone.windows(
                train_window(
                    audio,
                    MIN_HEAD_WINDOW_SAMPLES,
                    index=i,
                    noise=_pick(noise, i),
                    rir=_pick(rir, i),
                )
            )
            if len(windows) == 0:
                continue
            collected.append(windows)
            n_windows += len(windows)
            if hours_left <= 0 or n_windows >= max_windows:
                break
    if not collected:
        return np.zeros(EMPTY_SHAPE, dtype=np.float32)
    return np.concatenate(collected, axis=0)[:max_windows]


def assert_classes_are_not_separable_by_level(positives, negatives) -> None:
    """Refuse to train when the classes differ in loudness rather than content.

    Positives were peak-normalised and negatives were not, so the head learned
    level instead of the phrase: it converged, then scored chance recall on
    unseen voices. Cheap to check, and the failure is invisible until a full
    evaluation hours later.
    """
    import numpy as np

    pos = float(np.mean(np.abs(positives)))
    neg = float(np.mean(np.abs(negatives)))
    ratio = max(pos, neg) / max(min(pos, neg), 1e-9)
    if ratio > MAX_CLASS_LEVEL_RATIO:
        raise AllowlistError(
            "positive and negative windows differ in level by "
            f"{ratio:.1f}x (positives {pos:.4f}, negatives {neg:.4f}). "
            "Both classes must go through the same augmentation, otherwise "
            "the head separates them on loudness instead of the phrase."
        )


def _pick(pool, index: int):
    return pool[index % len(pool)][0] if pool else None
