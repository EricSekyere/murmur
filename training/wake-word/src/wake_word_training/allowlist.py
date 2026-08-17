"""Parse and enforce the training-input allowlist.

Every Piper voice and every negative/background/RIR dataset must appear in
`allowlist.toml` with an individual licence string. Only MIT, Apache-2.0, CC0,
and CC-BY (no NC, no SA) pass. ACAV100M-derived material is excluded by
default even if someone stamps a permissive licence on it.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

PERMITTED_FAMILIES = ("MIT", "Apache-2.0", "CC0", "CC-BY")

# Substrings that identify ACAV100M-derived material regardless of the
# licence field. Upstream openWakeWord heads are CC BY-NC-SA because of this.
_ACAV_MARKERS = ("acav100m", "acav_100m", "acav-100m")

# Pre-trained openWakeWord heads (CC BY-NC-SA). Never download or fine-tune.
FORBIDDEN_HEAD_STEMS = (
    "alexa",
    "hey_jarvis",
    "hey_mycroft",
    "hey_rhasspy",
    "timer_v0",
    "weather_v0",
)


class AllowlistError(Exception):
    """Training input is missing, empty, or not permissively licensed."""


@dataclass(frozen=True)
class AllowlistEntry:
    id: str
    licence: str
    url: str
    role: str = ""
    kind: str = "voice"
    notes: str = ""
    sha256: str = ""


@dataclass(frozen=True)
class Allowlist:
    path: Path
    voices: list[AllowlistEntry] = field(default_factory=list)
    datasets: list[AllowlistEntry] = field(default_factory=list)
    backbone: list[AllowlistEntry] = field(default_factory=list)

    def voice_ids(self) -> set[str]:
        return {v.id for v in self.voices}

    def dataset_ids(self) -> set[str]:
        return {d.id for d in self.datasets}

    def all_entries(self) -> list[AllowlistEntry]:
        return [*self.voices, *self.datasets, *self.backbone]


def validate_allowlist(path: Path) -> Allowlist:
    """Load `allowlist.toml` and fail closed on empty, missing, or bad licences."""
    raw = path.read_text(encoding="utf-8")
    data = tomllib.loads(raw)
    voices = [_entry(row, kind="voice", required_role=False) for row in data.get("voices", [])]
    datasets = [
        _entry(row, kind="dataset", required_role=True) for row in data.get("datasets", [])
    ]
    backbone = [
        _entry(row, kind="backbone", required_role=False) for row in data.get("backbone", [])
    ]
    if not voices and not datasets:
        raise AllowlistError(
            f"{path}: allowlist is empty (no voices or datasets); refusing to train"
        )
    for entry in [*voices, *datasets, *backbone]:
        _reject_acav(entry)
        _require_permissive_licence(entry)
    return Allowlist(path=path, voices=voices, datasets=datasets, backbone=backbone)


def assert_inputs_allowlisted(
    allowlist: Allowlist,
    *,
    voice_ids: list[str] | None = None,
    dataset_ids: list[str] | None = None,
) -> None:
    """Refuse any training input that is not an allowlisted id."""
    permitted_voices = allowlist.voice_ids()
    permitted_datasets = allowlist.dataset_ids()
    for vid in voice_ids or []:
        if vid not in permitted_voices:
            raise AllowlistError(f"voice {vid!r} is not in the allowlist")
    for did in dataset_ids or []:
        if did not in permitted_datasets:
            raise AllowlistError(f"dataset {did!r} is not in the allowlist")


def refuse_forbidden_heads(path_or_url: str) -> None:
    """Fail closed if a path or URL names an upstream CC BY-NC-SA head."""
    lowered = path_or_url.lower().replace("-", "_")
    for stem in FORBIDDEN_HEAD_STEMS:
        if stem in lowered:
            raise AllowlistError(
                f"refusing upstream CC BY-NC-SA pre-trained head {path_or_url!r} "
                f"(matched {stem!r})"
            )


def _entry(row: dict, *, kind: str, required_role: bool) -> AllowlistEntry:
    ident = str(row.get("id") or "").strip()
    if not ident:
        raise AllowlistError(f"{kind} entry is missing id")
    licence = str(row.get("licence") or "").strip()
    if not licence:
        raise AllowlistError(f"{kind} {ident!r} is missing a licence string")
    url = str(row.get("url") or "").strip()
    role = str(row.get("role") or "").strip()
    if required_role and not role:
        raise AllowlistError(f"dataset {ident!r} is missing role (noise|rir|background)")
    return AllowlistEntry(
        id=ident,
        licence=licence,
        url=url,
        role=role,
        kind=kind,
        notes=str(row.get("notes") or "").strip(),
        sha256=str(row.get("sha256") or "").strip().lower(),
    )


def _reject_acav(entry: AllowlistEntry) -> None:
    blob = f"{entry.id} {entry.url}".lower()
    if any(marker in blob for marker in _ACAV_MARKERS):
        raise AllowlistError(
            f"{entry.kind} {entry.id!r}: ACAV100M-derived material is excluded by default"
        )


def _require_permissive_licence(entry: AllowlistEntry) -> None:
    family = classify_licence(entry.licence)
    if family is None:
        raise AllowlistError(
            f"{entry.kind} {entry.id!r} licence {entry.licence!r} is not permitted "
            f"(need MIT / Apache-2.0 / CC0 / CC-BY; no NC, no SA)"
        )


def classify_licence(licence: str) -> str | None:
    """Return a permitted family name, or None if the licence is forbidden."""
    compact = re.sub(r"[\s_]+", "-", licence.strip().upper())
    compact = compact.replace("ATTRIBUTION", "BY")
    if not compact:
        return None
    if "NC" in compact.split("-") or compact.endswith("-NC") or "-NC-" in compact:
        return None
    if re.search(r"(^|-)SA(-|$)", compact):
        return None
    if compact in {"MIT", "X11"}:
        return "MIT"
    if compact in {"APACHE-2.0", "APACHE-2", "APACHE2.0", "APACHE2"} or compact.startswith(
        "APACHE-2"
    ):
        return "Apache-2.0"
    if compact in {"CC0", "CC0-1.0", "CC-0"} or compact.startswith("CC0"):
        return "CC0"
    if compact in {"PUBLIC-DOMAIN", "PUBLICDOMAIN", "PD"}:
        return "CC0"
    if compact == "CC-BY" or compact.startswith("CC-BY-"):
        # NC and SA already rejected above; remaining CC-BY-* is attribution-only.
        return "CC-BY"
    if compact.startswith("CC-BY"):
        return "CC-BY"
    return None


def licence_is_permissive(licence: str) -> bool:
    return classify_licence(licence) is not None
