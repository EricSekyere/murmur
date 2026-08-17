"""Speaker inventory and the speaker-disjoint split the recall gate rests on.

Holding out whole Piper voices made recall a three-valued quantity: with two
held-out voices it could only be 0, 0.5 or 1, and it sat at 0.5 at every
threshold from 0.05 to 0.95, so the threshold sweep had nothing to trade. The
allowlist actually carries 1937 speaker slots over 1033 distinct people, so
the split is by person instead.

`en_US-libritts-high` and `en_US-libritts_r-medium` render the same 904
LibriTTS people under different labels (`p3922` against `3922`) and, for 185
of them, under different `speaker_id` slots. Splitting on the raw slot leaks a
held-out person back into training through the sibling voice, so identities
are namespaced by corpus family and the label is normalised before hashing.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from hashlib import blake2b
from pathlib import Path

# Piper ships LibriTTS-R as a separate voice over the same cast, so the two
# share one identity namespace.
FAMILY_ALIASES = {"libritts_r": "libritts"}
# Voices with a single speaker are one person each, but splitting each of them
# on its own would round every fraction to zero and keep them all in training.
# They are stratified together so a whole unseen Piper voice reaches held-out.
SOLO_STRATUM = "solo"
DEFAULT_SPLIT_SEED = "hey-murmur-speakers-v1"
DEFAULT_HELD_OUT_FRACTION = 0.15
DEFAULT_VALIDATION_FRACTION = 0.12


class SplitError(Exception):
    """The speaker split is unusable, so training must not proceed."""


@dataclass(frozen=True)
class Speaker:
    """One renderable speaker slot on one Piper voice."""

    voice_id: str
    speaker_id: int | None
    identity: str
    stratum: str


@dataclass(frozen=True)
class SpeakerSplit:
    train: tuple[Speaker, ...]
    validation: tuple[Speaker, ...]
    held_out: tuple[Speaker, ...]

    def part(self, name: str) -> tuple[Speaker, ...]:
        parts = {
            "train": self.train,
            "validation": self.validation,
            "held_out": self.held_out,
        }
        if name not in parts:
            raise SplitError(f"unknown split {name!r}")
        return parts[name]

    def voices(self, name: str) -> list[str]:
        """Voice ids that have at least one speaker in this split, in order."""
        seen: dict[str, None] = {}
        for speaker in self.part(name):
            seen.setdefault(speaker.voice_id, None)
        return list(seen)

    def by_voice(self, name: str, voice_id: str) -> list[Speaker]:
        return [s for s in self.part(name) if s.voice_id == voice_id]

    def identities(self, name: str) -> set[str]:
        return {s.identity for s in self.part(name)}


def voice_family(voice_id: str) -> str:
    """The corpus a Piper voice draws its cast from (`en_US-libritts-high` -> libritts)."""
    parts = voice_id.split("-")
    name = parts[1] if len(parts) >= 3 else voice_id
    return FAMILY_ALIASES.get(name, name)


def normalise_label(label: str) -> str:
    """Strip Piper's optional `p` prefix so `p3922` and `3922` are one person.

    Only applied within a family, so VCTK's `p239` cannot collide with
    LibriTTS's `239`.
    """
    if label.startswith("p") and label[1:].isdigit():
        return label[1:]
    return label


def voice_speakers(voice_id: str, config: dict) -> list[Speaker]:
    """Speaker slots a Piper voice config exposes, as identities."""
    id_map = config.get("speaker_id_map") or {}
    if int(config.get("num_speakers", 1) or 1) <= 1 or not id_map:
        return [
            Speaker(
                voice_id=voice_id,
                speaker_id=None,
                identity=f"{SOLO_STRATUM}:{voice_id}",
                stratum=SOLO_STRATUM,
            )
        ]
    family = voice_family(voice_id)
    return [
        Speaker(
            voice_id=voice_id,
            speaker_id=int(slot),
            identity=f"{family}:{normalise_label(label)}",
            stratum=family,
        )
        for label, slot in sorted(id_map.items(), key=lambda kv: int(kv[1]))
    ]


def read_voice_speakers(voice_id: str, voice_onnx: Path) -> list[Speaker]:
    config_path = voice_onnx.with_suffix(voice_onnx.suffix + ".json")
    if not config_path.is_file():
        return voice_speakers(voice_id, {})
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise SplitError(f"unreadable voice config {config_path}: {exc}") from exc
    return voice_speakers(voice_id, config)


def build_inventory(voice_paths: dict[str, Path]) -> list[Speaker]:
    inventory: list[Speaker] = []
    for voice_id in sorted(voice_paths):
        inventory.extend(read_voice_speakers(voice_id, voice_paths[voice_id]))
    if not inventory:
        raise SplitError("no speakers found on the selected voices")
    return inventory


def split_speakers(
    speakers: list[Speaker],
    *,
    held_out_fraction: float = DEFAULT_HELD_OUT_FRACTION,
    validation_fraction: float = DEFAULT_VALIDATION_FRACTION,
    seed: str = DEFAULT_SPLIT_SEED,
) -> SpeakerSplit:
    """Partition people (not voice slots) into train / validation / held-out.

    Stratified per family so every split sees LibriTTS, VCTK, ARU and whole
    single-speaker voices, and keyed on a hash of the identity so a rerun with
    the same seed reproduces the split regardless of allowlist ordering.
    """
    if not 0.0 < held_out_fraction < 1.0 or not 0.0 <= validation_fraction < 1.0:
        raise SplitError("split fractions must lie in (0, 1)")
    if held_out_fraction + validation_fraction >= 1.0:
        raise SplitError("held-out plus validation must leave speakers for training")
    strata: dict[str, set[str]] = {}
    for speaker in speakers:
        strata.setdefault(speaker.stratum, set()).add(speaker.identity)
    roles: dict[str, str] = {}
    for stratum in sorted(strata):
        identities = sorted(
            strata[stratum], key=lambda identity: _rank(seed, identity)
        )
        n_held, n_val = _counts(
            len(identities), held_out_fraction, validation_fraction
        )
        for position, identity in enumerate(identities):
            if position < n_held:
                roles[identity] = "held_out"
            elif position < n_held + n_val:
                roles[identity] = "validation"
            else:
                roles[identity] = "train"
    buckets: dict[str, list[Speaker]] = {
        "train": [],
        "validation": [],
        "held_out": [],
    }
    for speaker in speakers:
        buckets[roles[speaker.identity]].append(speaker)
    split = SpeakerSplit(
        train=tuple(buckets["train"]),
        validation=tuple(buckets["validation"]),
        held_out=tuple(buckets["held_out"]),
    )
    assert_speaker_disjoint(split)
    return split


def assert_speaker_disjoint(split: SpeakerSplit) -> None:
    """Refuse a split where one person reaches two parts through any voice.

    The whole point of the split is that a held-out person was never trained
    on. LibriTTS and LibriTTS-R put the same people behind different slots, so
    this is checked rather than assumed.
    """
    parts = {name: split.identities(name) for name in ("train", "validation", "held_out")}
    for name, other in (
        ("train", "held_out"),
        ("train", "validation"),
        ("validation", "held_out"),
    ):
        shared = parts[name] & parts[other]
        if shared:
            sample = ", ".join(sorted(shared)[:5])
            raise SplitError(
                f"{len(shared)} speaker(s) appear in both {name} and {other}: {sample}. "
                "A held-out speaker must not be reachable under any voice."
            )
    for name, identities in parts.items():
        if not identities:
            raise SplitError(f"the {name} split has no speakers")


def _counts(total: int, held_out_fraction: float, validation_fraction: float) -> tuple[int, int]:
    if total < 3:
        # Too few people to spare any; the caller's other strata carry the split.
        return (1, 0) if total == 2 else (0, 0)
    n_held = max(1, round(total * held_out_fraction))
    n_val = max(1, round(total * validation_fraction))
    while total - n_held - n_val < 1 and n_held + n_val > 2:
        if n_val >= n_held:
            n_val -= 1
        else:
            n_held -= 1
    return n_held, n_val


def _rank(seed: str, identity: str) -> str:
    return blake2b(f"{seed}:{identity}".encode(), digest_size=16).hexdigest()
