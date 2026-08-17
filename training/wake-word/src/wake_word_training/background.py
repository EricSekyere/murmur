"""Which background recordings train the head and which measure false accepts.

Training took background clips in sorted order until the window cap, which
stopped after 690 files: 2.4 hours drawn from 7 of LibriSpeech's 251
speakers. Evaluation then counted false accepts over the whole corpus, so the
head met 244 unseen background speakers for the first time at gate time and
scored 6.02 false accepts an hour against a 0.5 ceiling.

Two fixes live here, and both sides import this module so they cannot drift:
recordings are split by speaker, so the false-accept gate is measured on
speakers the head never trained against, and the training order interleaves
speakers, so a window cap yields a broad sample instead of a prefix.
"""

from __future__ import annotations

from collections.abc import Iterator
from hashlib import blake2b
from pathlib import Path

from wake_word_training.audio import iter_audio_files

TRAIN = "train"
VALIDATION = "validation"
EVALUATION = "evaluation"
ROLES = (TRAIN, VALIDATION, EVALUATION)
DEFAULT_SPLIT_SEED = "hey-murmur-background-v1"
# Training and validation together take the smaller half; the false-accept
# gate needs enough hours to resolve a 0.5/hour ceiling, and at ~100 h of
# LibriSpeech this leaves ~45 h and ~22 events of headroom at the ceiling.
_ROLE_WEIGHTS = ((TRAIN, 0.45), (VALIDATION, 0.10), (EVALUATION, 0.45))


class BackgroundSplitError(Exception):
    """The background corpus cannot be split into train and gate halves."""


def background_group(path: Path) -> str:
    """The recording group a background file belongs to.

    LibriSpeech nests as `<speaker>/<chapter>/<utterance>.flac`, so the
    grandparent directory names the speaker. Anything shallower falls back to
    its parent, which keeps a flat corpus splittable by directory.
    """
    parent = path.parent
    return parent.parent.name if parent.parent.name else parent.name


def background_role(group: str, *, seed: str = DEFAULT_SPLIT_SEED) -> str:
    """Deterministic role for a group, stable across runs and machines."""
    digest = blake2b(f"{seed}:{group}".encode(), digest_size=8).hexdigest()
    position = int(digest, 16) / float(1 << 64)
    cumulative = 0.0
    for role, weight in _ROLE_WEIGHTS:
        cumulative += weight
        if position < cumulative:
            return role
    return EVALUATION


def background_paths(
    root: Path, *, role: str, seed: str = DEFAULT_SPLIT_SEED
) -> list[Path]:
    """Files of one role, ordered so consecutive files come from different groups.

    The interleave is what makes a window cap representative: taking the first
    N files of a sorted corpus takes N files from a handful of speakers.
    """
    if role not in ROLES:
        raise BackgroundSplitError(f"unknown background role {role!r}")
    grouped: dict[str, list[Path]] = {}
    for path in iter_audio_files(root):
        group = background_group(path)
        if background_role(group, seed=seed) != role:
            continue
        grouped.setdefault(group, []).append(path)
    return list(_interleave(grouped))


def _interleave(grouped: dict[str, list[Path]]) -> Iterator[Path]:
    order = sorted(grouped)
    depth = 0
    remaining = True
    while remaining:
        remaining = False
        for group in order:
            files = grouped[group]
            if depth < len(files):
                remaining = True
                yield files[depth]
        depth += 1
