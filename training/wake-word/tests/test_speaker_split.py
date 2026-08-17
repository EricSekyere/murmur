"""The recall gate is only meaningful if held-out people were never trained on.

Run 4 held out whole voices, so recall could only be 0, 0.5 or 1 and sat at
0.5 at every threshold. Splitting by person needs the aliasing to be right:
`en_US-libritts-high` and `en_US-libritts_r-medium` ship the same 904 LibriTTS
people under different labels (`p3922` against `3922`) and, for 185 of them,
under different `speaker_id` slots, so a split keyed on the slot leaks a
held-out person straight back into training.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wake_word_training.speakers import (
    SplitError,
    build_inventory,
    split_speakers,
    voice_family,
    voice_speakers,
)

DATA = Path(__file__).parents[1] / "data" / "voices"
LIBRITTS = "en_US-libritts-high"
LIBRITTS_R = "en_US-libritts_r-medium"


def _people(count: int = 60) -> list[str]:
    return [str(3000 + i) for i in range(count)]


def _libritts_config(people: list[str]) -> dict:
    # Piper prefixes LibriTTS labels with `p` on the high voice only.
    return {
        "num_speakers": len(people),
        "speaker_id_map": {f"p{label}": slot for slot, label in enumerate(people)},
    }


def _libritts_r_config(people: list[str]) -> dict:
    # LibriTTS-R carries the same cast in a different slot order, as shipped.
    shuffled = list(people)
    for i in range(0, len(shuffled) - 1, 2):
        shuffled[i], shuffled[i + 1] = shuffled[i + 1], shuffled[i]
    return {
        "num_speakers": len(shuffled),
        "speaker_id_map": {label: slot for slot, label in enumerate(shuffled)},
    }


def _inventory(people: list[str]):
    return voice_speakers(LIBRITTS, _libritts_config(people)) + voice_speakers(
        LIBRITTS_R, _libritts_r_config(people)
    )


def _person_at(config: dict, speaker_id: int) -> str:
    """Who a slot renders, read straight off the config, not from the split code."""
    for label, slot in config["speaker_id_map"].items():
        if slot == speaker_id:
            return label.lstrip("p")
    raise AssertionError(f"no speaker {speaker_id}")


def test_voice_family_folds_libritts_r_onto_libritts() -> None:
    assert voice_family(LIBRITTS) == voice_family(LIBRITTS_R) == "libritts"
    assert voice_family("en_GB-vctk-medium") == "vctk"
    assert voice_family("en_US-ljspeech-high") == "ljspeech"


def test_vctk_and_libritts_labels_do_not_collide() -> None:
    # Both corpora use numeric-looking labels; namespacing by family is what
    # keeps VCTK's p239 from being mistaken for LibriTTS speaker 239.
    libritts = voice_speakers(LIBRITTS, _libritts_config(["239"]))
    vctk = voice_speakers(
        "en_GB-vctk-medium", {"num_speakers": 2, "speaker_id_map": {"p239": 0, "p240": 1}}
    )
    assert {s.identity for s in libritts}.isdisjoint({s.identity for s in vctk})


def test_single_speaker_voices_share_one_stratum_so_a_whole_voice_is_held_out() -> None:
    solo = voice_speakers("en_US-norman-medium", {"num_speakers": 1})
    assert len(solo) == 1
    assert solo[0].speaker_id is None
    assert solo[0].stratum == "solo"


def test_a_held_out_person_is_absent_from_training_under_every_voice() -> None:
    people = _people()
    configs = {LIBRITTS: _libritts_config(people), LIBRITTS_R: _libritts_r_config(people)}
    split = split_speakers(_inventory(people))

    def rendered(part: str) -> set[str]:
        return {
            _person_at(configs[s.voice_id], s.speaker_id) for s in split.part(part)
        }

    held = rendered("held_out")
    assert held, "nothing was held out"
    assert held.isdisjoint(rendered("train"))
    assert held.isdisjoint(rendered("validation"))
    assert rendered("validation").isdisjoint(rendered("train"))


def test_both_voices_render_a_held_out_person_when_it_is_held_out() -> None:
    # Aliasing must be symmetric: holding a person out of one voice while the
    # sibling voice still renders them is the leak this split exists to stop.
    people = _people()
    split = split_speakers(_inventory(people))
    for part in ("train", "validation", "held_out"):
        voices = {s.voice_id for s in split.part(part)}
        assert voices == {LIBRITTS, LIBRITTS_R}, part


def test_every_stratum_reaches_every_split() -> None:
    inventory = (
        _inventory(_people())
        + voice_speakers(
            "en_GB-vctk-medium",
            {"num_speakers": 20, "speaker_id_map": {f"p{200 + i}": i for i in range(20)}},
        )
        + voice_speakers("en_US-norman-medium", {"num_speakers": 1})
        + voice_speakers("en_US-ljspeech-high", {"num_speakers": 1})
        + voice_speakers("en_US-joe-medium", {"num_speakers": 1})
        + voice_speakers("en_GB-alba-medium", {"num_speakers": 1})
        + voice_speakers("en_GB-cori-medium", {"num_speakers": 1})
    )
    split = split_speakers(inventory)
    for part in ("train", "validation", "held_out"):
        strata = {s.stratum for s in split.part(part)}
        assert {"libritts", "vctk", "solo"} <= strata, (part, strata)


def test_the_split_is_reproducible_and_seed_sensitive() -> None:
    people = _people()
    first = split_speakers(_inventory(people))
    again = split_speakers(_inventory(people))
    other = split_speakers(_inventory(people), seed="a-different-seed")
    assert first.identities("held_out") == again.identities("held_out")
    assert first.identities("held_out") != other.identities("held_out")


def test_a_split_that_leaves_no_training_speakers_is_refused() -> None:
    with pytest.raises(SplitError):
        split_speakers(_inventory(_people()), held_out_fraction=0.6, validation_fraction=0.5)


@pytest.mark.skipif(not DATA.is_dir(), reason="Piper voices not downloaded")
def test_shipped_libritts_voices_really_do_alias_the_same_cast() -> None:
    # The measurement the whole split rests on, taken from the shipped
    # configs rather than assumed: 904 shared people, 185 in different slots.
    def config(voice_id: str) -> dict:
        return json.loads((DATA / f"{voice_id}.onnx.json").read_text(encoding="utf-8"))

    high = {k.lstrip("p"): v for k, v in config(LIBRITTS)["speaker_id_map"].items()}
    medium = {k.lstrip("p"): v for k, v in config(LIBRITTS_R)["speaker_id_map"].items()}
    shared = set(high) & set(medium)
    assert len(shared) == len(high) == len(medium) == 904
    assert sum(1 for k in shared if high[k] != medium[k]) > 0, (
        "if the slots agreed, splitting on speaker_id would be safe"
    )


@pytest.mark.skipif(not DATA.is_dir(), reason="Piper voices not downloaded")
def test_the_shipped_allowlist_yields_a_measurable_held_out_set() -> None:
    voices = {p.name[: -len(".onnx")]: p for p in sorted(DATA.glob("*.onnx"))}
    split = split_speakers(build_inventory(voices))
    # Recall used to take three values because two voices were held out. The
    # gate needs enough people for it to move.
    assert len(split.identities("held_out")) >= 50
    assert len(split.identities("train")) >= 400
    assert len(split.voices("held_out")) >= 4
