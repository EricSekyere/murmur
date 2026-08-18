"""Synthesis must render the split it was given, and only that split."""

from __future__ import annotations

from pathlib import Path

import pytest

from wake_word_training import corpus
from wake_word_training.corpus import (
    assert_no_stale_clips,
    speaker_dir_name,
    synthesize_split,
)
from wake_word_training.speakers import SplitError, split_speakers, voice_speakers

MULTI = "en_US-libritts-high"
SOLO_A = "en_US-norman-medium"
SOLO_B = "en_US-ljspeech-high"
SOLO_C = "en_US-joe-medium"
SOLO_D = "en_GB-alba-medium"


def _split():
    inventory = voice_speakers(
        MULTI,
        {"num_speakers": 40, "speaker_id_map": {f"p{3000 + i}": i for i in range(40)}},
    )
    for solo in (SOLO_A, SOLO_B, SOLO_C, SOLO_D):
        inventory += voice_speakers(solo, {"num_speakers": 1})
    return split_speakers(inventory)


def _voice_paths(root: Path) -> dict[str, Path]:
    return {
        vid: root / f"{vid}.onnx" for vid in (MULTI, SOLO_A, SOLO_B, SOLO_C, SOLO_D)
    }


def _record(monkeypatch) -> list[tuple[Path, dict, Path]]:
    calls: list[tuple[Path, dict, Path]] = []

    def fake(voice_onnx: Path, phrase: str, dest: Path, *, variation=None):
        calls.append((voice_onnx, variation or {}, dest))
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(b"")

    monkeypatch.setattr(corpus, "synthesize_with_piper", fake)
    return calls


def _rendered(calls, voice_paths) -> set[tuple[str, int | None]]:
    by_path = {str(p): vid for vid, p in voice_paths.items()}
    return {(by_path[str(v)], var.get("speaker_id")) for v, var, _dest in calls}


def test_a_held_out_speaker_is_never_rendered_into_training(tmp_path, monkeypatch) -> None:
    split = _split()
    voice_paths = _voice_paths(tmp_path)
    calls = _record(monkeypatch)

    synthesize_split(
        voice_paths, split, "train", phrase="Hey Murmur", clips_root=tmp_path / "c", n_clips=400
    )
    trained = _rendered(calls, voice_paths)
    calls.clear()
    synthesize_split(
        voice_paths, split, "held_out", phrase="Hey Murmur", clips_root=tmp_path / "c", n_clips=200
    )
    held = _rendered(calls, voice_paths)

    assert held and trained
    assert held.isdisjoint(trained)


def test_synthesis_spreads_over_the_split_speakers(tmp_path, monkeypatch) -> None:
    # 400 clips of speaker 0 is what the corpus used to be; the point of the
    # multi-speaker voices is that they render many different people.
    split = _split()
    voice_paths = _voice_paths(tmp_path)
    calls = _record(monkeypatch)
    synthesize_split(
        voice_paths, split, "train", phrase="Hey Murmur", clips_root=tmp_path / "c", n_clips=400
    )
    slots = {
        var["speaker_id"] for v, var, _d in calls if str(v).endswith(f"{MULTI}.onnx")
    }
    assert slots == {s.speaker_id for s in split.by_voice("train", MULTI)}
    assert len(slots) > 10


def test_clips_land_in_per_speaker_directories(tmp_path, monkeypatch) -> None:
    split = _split()
    calls = _record(monkeypatch)
    synthesize_split(
        _voice_paths(tmp_path),
        split,
        "held_out",
        phrase="Hey Murmur",
        clips_root=tmp_path / "c",
        n_clips=100,
    )
    expected = {speaker_dir_name(s) for s in split.part("held_out")}
    assert {dest.parent.name for _v, _var, dest in calls} <= expected
    assert {dest.parent.parent.name for _v, _var, dest in calls} == set(
        split.voices("held_out")
    )


def test_a_corpus_from_an_older_split_is_refused(tmp_path) -> None:
    split = _split()
    clips = tmp_path / "clips"
    stranger = clips / "held_out" / MULTI / "libritts_9999"
    stranger.mkdir(parents=True)
    (stranger / "00000.wav").write_bytes(b"")
    with pytest.raises(SplitError, match="belong to no held_out speaker"):
        assert_no_stale_clips(clips, split)


def test_a_flat_pre_split_corpus_is_refused(tmp_path) -> None:
    clips = tmp_path / "clips"
    flat = clips / "held_out" / MULTI
    flat.mkdir(parents=True)
    (flat / "00000.wav").write_bytes(b"")
    with pytest.raises(SplitError, match="per-speaker"):
        assert_no_stale_clips(clips, _split())


def test_a_matching_corpus_passes(tmp_path, monkeypatch) -> None:
    split = _split()
    _record(monkeypatch)
    clips = tmp_path / "clips"
    for name in ("train", "validation", "held_out"):
        synthesize_split(
            _voice_paths(tmp_path),
            split,
            name,
            phrase="Hey Murmur",
            clips_root=clips,
            n_clips=100,
        )
    assert_no_stale_clips(clips, split)
