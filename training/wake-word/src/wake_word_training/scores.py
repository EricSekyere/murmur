"""The two halves of the evaluation, scored and cached.

Scoring 54 h of audio is the whole cost of an evaluation, and the protocol it
feeds is what needed fixing, so the raw scores are written next to the report.
`--reuse-scores` then re-derives the report in a second. The cache records the
head's SHA256 and refuses to load against a different one: a threshold from one
model says nothing about another, and a stale cache would say it silently.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import NamedTuple

from wake_word_training.audio import iter_audio_files
from wake_word_training.background import background_paths
from wake_word_training.scoring import score_files_parallel

CACHE_VERSION = 2


class ScoredHalf(NamedTuple):
    """Positives per clip, background per file, and the background exposure.

    Background scores stay grouped by file because the refractory that
    de-duplicates a false accept must not carry into an unrelated recording.
    """

    positives: list[tuple[str, float]]
    background: list[list[float]]
    background_hours: float

    def positive_scores(self) -> list[float]:
        return [score for _name, score in self.positives]


class ScoreSets(NamedTuple):
    calibration: ScoredHalf
    gate: ScoredHalf


def score_clips(
    backbone_dir: Path, head_path: Path, clips_dir: Path, *, workers: int | None = None
) -> list[tuple[str, float]]:
    """Each clip's best window score, keyed by path relative to `clips_dir`."""
    paths = list(iter_audio_files(clips_dir))
    scored = score_files_parallel(backbone_dir, head_path, paths, workers=workers)
    return [
        (str(one.path.relative_to(clips_dir).as_posix()), max(one.scores, default=0.0))
        for one in scored
    ]


def score_background(
    backbone_dir: Path,
    head_path: Path,
    allowlist,
    data_root: Path,
    *,
    role: str,
    workers: int | None = None,
) -> tuple[list[list[float]], float]:
    """One score list per background file of `role`, plus the hours they cover.

    Training draws its negatives from the training third of the same split
    (see `background.py`), so neither the calibration nor the gate half is a
    recital of anything the head trained on.
    """
    paths: list[Path] = []
    for dataset in allowlist.datasets:
        if dataset.role == "background":
            paths.extend(background_paths(data_root / dataset.id, role=role))
    scored = score_files_parallel(backbone_dir, head_path, paths, workers=workers)
    hours = sum(one.seconds for one in scored) / 3600.0
    return [list(one.scores) for one in scored], hours


def score_half(
    backbone_dir: Path,
    head_path: Path,
    clips_dir: Path,
    allowlist,
    data_root: Path,
    *,
    role: str,
    workers: int | None = None,
) -> ScoredHalf:
    positives = score_clips(backbone_dir, head_path, clips_dir, workers=workers)
    background, hours = score_background(
        backbone_dir, head_path, allowlist, data_root, role=role, workers=workers
    )
    return ScoredHalf(positives, background, hours)


def head_digest(head_path: Path) -> str:
    return hashlib.sha256(head_path.read_bytes()).hexdigest()


def save_scores(path: Path, sets: ScoreSets, head_path: Path) -> None:
    import numpy as np

    path.parent.mkdir(parents=True, exist_ok=True)
    payload: dict[str, object] = {
        "meta": np.array(
            json.dumps(
                {
                    "version": CACHE_VERSION,
                    "head_sha256": head_digest(head_path),
                    "head": head_path.name,
                }
            )
        )
    }
    for name, half in (("calibration", sets.calibration), ("gate", sets.gate)):
        flat, offsets = _flatten(half.background)
        payload[f"{name}_positive_paths"] = np.array(
            json.dumps([p for p, _s in half.positives])
        )
        payload[f"{name}_positive_scores"] = np.asarray(
            [s for _p, s in half.positives], dtype=np.float32
        )
        payload[f"{name}_background_scores"] = flat
        payload[f"{name}_background_offsets"] = offsets
        payload[f"{name}_background_hours"] = np.asarray(half.background_hours)
    np.savez_compressed(path, **payload)


def load_scores(path: Path, head_path: Path) -> ScoreSets:
    import numpy as np

    from wake_word_training.allowlist import AllowlistError

    candidate = path if path.is_file() else path.with_suffix(path.suffix + ".npz")
    if not candidate.is_file():
        raise AllowlistError(f"no cached scores at {path}; run without --reuse-scores")
    with np.load(candidate) as data:
        meta = json.loads(str(data["meta"]))
        expected = head_digest(head_path)
        if meta.get("head_sha256") != expected:
            raise AllowlistError(
                f"cached scores in {candidate} were produced by a different head "
                f"({meta.get('head_sha256')}, now {expected}); re-score instead"
            )
        halves = {
            name: ScoredHalf(
                positives=list(
                    zip(
                        json.loads(str(data[f"{name}_positive_paths"])),
                        [float(v) for v in data[f"{name}_positive_scores"]],
                        strict=True,
                    )
                ),
                background=_unflatten(
                    data[f"{name}_background_scores"], data[f"{name}_background_offsets"]
                ),
                background_hours=float(data[f"{name}_background_hours"]),
            )
            for name in ("calibration", "gate")
        }
    return ScoreSets(calibration=halves["calibration"], gate=halves["gate"])


def _flatten(segments: list[list[float]]):
    import numpy as np

    offsets = np.cumsum([0] + [len(s) for s in segments], dtype=np.int64)
    flat = np.concatenate(
        [np.asarray(s, dtype=np.float32) for s in segments]
        or [np.zeros(0, dtype=np.float32)]
    )
    return flat, offsets


def _unflatten(flat, offsets) -> list[list[float]]:
    return [
        [float(v) for v in flat[start:stop]]
        for start, stop in zip(offsets[:-1], offsets[1:], strict=True)
    ]
