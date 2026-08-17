"""Synthesize the three speaker-disjoint clip splits on disk.

Layout is `output/clips/<split>/<voice id>/<speaker>/NNNNN.wav`. The speaker
directory is what makes the split auditable after the fact: the evaluator can
group recall by voice and by person, and a stale corpus from an older split is
caught rather than silently scored.
"""

from __future__ import annotations

from pathlib import Path

from wake_word_training.audio import synthesis_variation, synthesize_with_piper
from wake_word_training.speakers import Speaker, SpeakerSplit, SplitError

SPLITS = ("train", "validation", "held_out")


def speaker_dir_name(speaker: Speaker) -> str:
    """Filesystem-safe form of a speaker identity (`:` is illegal on Windows)."""
    return speaker.identity.replace(":", "_")


def synthesize_split(
    voice_paths: dict[str, Path],
    split: SpeakerSplit,
    name: str,
    *,
    phrase: str,
    clips_root: Path,
    n_clips: int,
) -> dict[str, int]:
    """Render `n_clips` for one split, balanced over its voices and speakers.

    Clips are shared equally between the voices that have speakers in this
    split, and within a voice they cycle its speakers, so a voice contributing
    660 people renders 660 different people rather than one of them 660 times.
    Existing files are kept, so an interrupted run resumes.
    """
    voices = [v for v in split.voices(name) if v in voice_paths]
    if not voices:
        raise SplitError(f"the {name} split has no voices among the selected ones")
    per_voice = max(1, n_clips // len(voices))
    written: dict[str, int] = {}
    for voice_id in voices:
        speakers = split.by_voice(name, voice_id)
        for index in range(per_voice):
            speaker = speakers[index % len(speakers)]
            dest = (
                clips_root
                / name
                / voice_id
                / speaker_dir_name(speaker)
                / f"{index:05d}.wav"
            )
            if not dest.is_file():
                synthesize_with_piper(
                    voice_paths[voice_id],
                    phrase,
                    dest,
                    variation=synthesis_variation(index, speaker.speaker_id),
                )
        written[voice_id] = per_voice
    return written


def assert_no_stale_clips(clips_root: Path, split: SpeakerSplit) -> None:
    """Refuse a corpus left over from a different split.

    Synthesis skips clips that already exist, so an earlier split's held-out
    directory would otherwise be scored as if it belonged to this one, and the
    recall gate would silently measure speakers that are now in training.
    """
    for name in SPLITS:
        root = clips_root / name
        if not root.is_dir():
            continue
        expected = {
            (speaker.voice_id, speaker_dir_name(speaker))
            for speaker in split.part(name)
        }
        for voice_dir in sorted(p for p in root.iterdir() if p.is_dir()):
            found = {p.name for p in voice_dir.iterdir() if p.is_dir()}
            if not found and any(voice_dir.iterdir()):
                raise SplitError(
                    f"{voice_dir} holds clips directly, not per-speaker directories; "
                    "it predates the speaker split. Delete the clips tree and resynthesize."
                )
            stale = {
                (voice_dir.name, speaker)
                for speaker in found
                if (voice_dir.name, speaker) not in expected
            }
            if stale:
                sample = ", ".join(f"{v}/{s}" for v, s in sorted(stale)[:5])
                raise SplitError(
                    f"{len(stale)} clip director(ies) under {root} belong to no "
                    f"{name} speaker in the current split: {sample}. "
                    "Delete the clips tree and resynthesize."
                )
