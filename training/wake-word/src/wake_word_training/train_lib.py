"""Licence-gated training orchestration. Heavy deps are imported lazily."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import NamedTuple

from wake_word_training.allowlist import (
    Allowlist,
    AllowlistError,
    assert_inputs_allowlisted,
    refuse_forbidden_heads,
    validate_allowlist,
)
from wake_word_training.audio import (
    DEFAULT_MAX_BACKGROUND_HOURS,
    DEFAULT_MAX_NEGATIVE_WINDOWS,
    DEFAULT_MAX_VALIDATION_WINDOWS,
    PHRASE,
)
from wake_word_training.background import background_paths
from wake_word_training.corpus import assert_no_stale_clips, synthesize_split
from wake_word_training.download import fetch_entry
from wake_word_training.hard_negatives import (
    DEFAULT_MAX_HOURS as DEFAULT_HARD_NEGATIVE_HOURS,
)
from wake_word_training.hard_negatives import (
    DEFAULT_MIN_SCORE as DEFAULT_HARD_NEGATIVE_MIN_SCORE,
)
from wake_word_training.speakers import (
    DEFAULT_HELD_OUT_FRACTION,
    DEFAULT_SPLIT_SEED,
    DEFAULT_VALIDATION_FRACTION,
    SpeakerSplit,
    build_inventory,
    split_speakers,
)
from wake_word_training.windows import (
    assert_classes_are_not_separable_by_level,
    augmentation_pools,
    sample_background_windows,
    windows_from_clips,
    windows_from_dir,
)

ROOT = Path(__file__).resolve().parents[2]


def default_allowlist() -> Path:
    return ROOT / "allowlist.toml"


def run(argv: list[str] | None = None) -> int:
    args = _parse(argv)
    try:
        allowlist = validate_allowlist(args.allowlist)
        _refuse_init_from(args.init_from)
        selected = _selected_voices(allowlist, args.voices)
        if args.validate_only:
            print(
                f"allowlist ok: {len(allowlist.voices)} voices, "
                f"{len(allowlist.datasets)} datasets, {len(allowlist.backbone)} backbone files"
            )
            return 0
        _train(allowlist, selected, args)
    except AllowlistError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    return 0


def _parse(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Train a Hey Murmur openWakeWord head from allowlisted inputs."
    )
    parser.add_argument("--allowlist", type=Path, default=default_allowlist())
    parser.add_argument("--output-dir", type=Path, default=ROOT / "output")
    parser.add_argument("--data-dir", type=Path, default=ROOT / "data")
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--phrase", default=PHRASE)
    parser.add_argument("--n-samples", type=int, default=8000)
    parser.add_argument("--n-validation", type=int, default=1200)
    parser.add_argument("--n-held-out", type=int, default=2000)
    parser.add_argument("--steps", type=int, default=10_000)
    parser.add_argument(
        "--voices",
        nargs="*",
        default=None,
        help="Subset of allowlisted voice ids (default: all).",
    )
    parser.add_argument(
        "--init-from",
        type=Path,
        default=None,
        help="Optional checkpoint. Upstream CC BY-NC-SA heads are rejected.",
    )
    parser.add_argument(
        "--held-out-fraction",
        type=float,
        default=DEFAULT_HELD_OUT_FRACTION,
        help="Fraction of speakers (not voices) reserved for the recall gate.",
    )
    parser.add_argument(
        "--validation-fraction",
        type=float,
        default=DEFAULT_VALIDATION_FRACTION,
        help="Fraction of speakers reserved for early stopping.",
    )
    parser.add_argument("--split-seed", default=DEFAULT_SPLIT_SEED)
    parser.add_argument(
        "--max-background-hours",
        type=float,
        default=DEFAULT_MAX_BACKGROUND_HOURS,
        help="Cap on background/negative audio hours loaded during training.",
    )
    parser.add_argument(
        "--max-negative-windows",
        type=int,
        default=DEFAULT_MAX_NEGATIVE_WINDOWS,
        help="Cap on embedding windows taken from background audio.",
    )
    parser.add_argument(
        "--max-validation-windows",
        type=int,
        default=DEFAULT_MAX_VALIDATION_WINDOWS,
        help="Cap on background windows held back for early stopping.",
    )
    parser.add_argument(
        "--hard-negative-hours",
        type=float,
        default=DEFAULT_HARD_NEGATIVE_HOURS,
        help="Training-half background searched for false accepts; 0 disables the pass.",
    )
    parser.add_argument(
        "--hard-negative-min-score",
        type=float,
        default=DEFAULT_HARD_NEGATIVE_MIN_SCORE,
        help="Score at which a background clip counts as a hard negative.",
    )
    return parser.parse_args(argv)


def _refuse_init_from(path: Path | None) -> None:
    if path is not None:
        refuse_forbidden_heads(str(path))


def _selected_voices(allowlist: Allowlist, requested: list[str] | None) -> list[str]:
    ids = requested if requested else sorted(allowlist.voice_ids())
    assert_inputs_allowlisted(allowlist, voice_ids=ids)
    if not ids:
        raise AllowlistError("no Piper voices selected")
    return ids


class _Windows(NamedTuple):
    positives: object
    negatives: object
    validation_positives: object
    validation_negatives: object


def _train(allowlist: Allowlist, voice_ids: list[str], args: argparse.Namespace) -> None:
    from wake_word_training.features import Backbone
    from wake_word_training.model import export_onnx, train_head_with_trace

    out = args.output_dir
    _require_dataset_dirs(allowlist, args.data_dir)
    backbone_paths = _fetch_backbone(allowlist, args.data_dir / "backbone")
    voice_paths = _fetch_voices(allowlist, voice_ids, args.data_dir / "voices")
    split = split_speakers(
        build_inventory(voice_paths),
        held_out_fraction=args.held_out_fraction,
        validation_fraction=args.validation_fraction,
        seed=args.split_seed,
    )
    _report_split(split)
    _synthesize_corpus(voice_paths, split, args)
    backbone = Backbone(backbone_paths["melspectrogram"], backbone_paths["embedding_model"])
    windows = _collect_windows(backbone, allowlist, args)
    assert_classes_are_not_separable_by_level(windows.positives, windows.negatives)
    print(
        f"windows: train {len(windows.positives)}+/{len(windows.negatives)}-, "
        f"validation {len(windows.validation_positives)}+/"
        f"{len(windows.validation_negatives)}-"
    )
    validation = (windows.validation_positives, windows.validation_negatives)
    model, trace = train_head_with_trace(
        windows.positives, windows.negatives, validation=validation, steps=args.steps
    )
    _report_trace("first pass", trace)
    hard = _mine_hard_negatives(backbone, model, allowlist, args, windows)
    if len(hard):
        model, trace = train_head_with_trace(
            windows.positives,
            windows.negatives,
            hard_negatives=hard,
            validation=validation,
            steps=args.steps,
        )
        _report_trace("hard-negative pass", trace)
    dest = out / "hey_murmur.onnx"
    export_onnx(model, dest)
    _write_run_manifest(out, allowlist, split, trace, dest)
    print(f"wrote {dest}")


def _synthesize_corpus(
    voice_paths: dict[str, Path], split: SpeakerSplit, args: argparse.Namespace
) -> None:
    clips = args.output_dir / "clips"
    assert_no_stale_clips(clips, split)
    for name, count in (
        ("train", args.n_samples),
        ("validation", args.n_validation),
        ("held_out", args.n_held_out),
    ):
        synthesize_split(
            voice_paths, split, name, phrase=args.phrase, clips_root=clips, n_clips=count
        )


def _collect_windows(backbone, allowlist: Allowlist, args: argparse.Namespace) -> _Windows:
    data = args.data_dir
    clips = args.output_dir / "clips"
    noise, rir = augmentation_pools(data, allowlist)

    def background(role: str, max_windows: int):
        return sample_background_windows(
            backbone,
            data,
            allowlist,
            role=role,
            max_hours=args.max_background_hours,
            max_windows=max_windows,
            noise=noise,
            rir=rir,
        )

    negatives = background("train", args.max_negative_windows)
    if negatives is None or len(negatives) == 0:
        raise AllowlistError(
            "no background windows; place allowlisted background audio under data/<id>/"
        )
    return _Windows(
        positives=windows_from_dir(backbone, clips / "train", noise=noise, rir=rir),
        negatives=negatives,
        validation_positives=windows_from_dir(
            backbone, clips / "validation", noise=noise, rir=rir
        ),
        validation_negatives=background("validation", args.max_validation_windows),
    )


def _mine_hard_negatives(backbone, model, allowlist: Allowlist, args, windows: _Windows):
    """Background clips the first pass scores like the phrase, as extra negatives.

    Fed back through the same augmentation as every other negative, so the
    head cannot separate them on level or cleanliness, and mined only from the
    training half of the background split so the false-accept gate stays a
    measurement rather than a recital.
    """
    import numpy as np

    from wake_word_training.hard_negatives import (
        DEFAULT_MAX_HARD_WINDOWS,
        mine_hard_clips,
    )
    from wake_word_training.model import score_windows

    if args.hard_negative_hours <= 0:
        return np.zeros((0, 16, 96), dtype=np.float32)
    clips = mine_hard_clips(
        backbone,
        lambda w: score_windows(model, w),
        _background_paths_for(allowlist, args.data_dir, role="train"),
        max_hours=args.hard_negative_hours,
        min_score=args.hard_negative_min_score,
    )
    noise, rir = augmentation_pools(args.data_dir, allowlist)
    hard = windows_from_clips(
        backbone, clips, noise=noise, rir=rir, max_windows=DEFAULT_MAX_HARD_WINDOWS
    )
    print(f"mined {len(clips)} hard background clips -> {len(hard)} windows")
    if len(hard):
        assert_classes_are_not_separable_by_level(
            windows.positives, np.concatenate([windows.negatives, hard], axis=0)
        )
    return hard


def _background_paths_for(allowlist: Allowlist, data_root: Path, *, role: str) -> list[Path]:
    paths: list[Path] = []
    for dataset in allowlist.datasets:
        if dataset.role == "background":
            paths.extend(background_paths(data_root / dataset.id, role=role))
    return paths


def _report_trace(label: str, trace) -> None:
    print(
        f"{label}: stopped at step {trace.steps_run} (best {trace.best_step}), "
        f"validation loss {trace.best_validation_loss:.5f} "
        f"(positives {trace.positive_validation_loss:.5f}, "
        f"negatives {trace.negative_validation_loss:.5f}), "
        f"hard negatives {trace.hard_negatives}"
    )


def _report_split(split: SpeakerSplit) -> None:
    for name in ("train", "validation", "held_out"):
        print(
            f"{name:11s} {len(split.identities(name)):5d} speakers "
            f"over {len(split.voices(name)):2d} voices "
            f"({len(split.part(name))} voice/speaker slots)"
        )


def _require_dataset_dirs(allowlist: Allowlist, data_root: Path) -> None:
    for dataset in allowlist.datasets:
        path = data_root / dataset.id
        if not path.is_dir():
            raise AllowlistError(
                f"dataset {dataset.id!r} ({dataset.licence}) is allowlisted but "
                f"{path} is missing. Download from {dataset.url} into that directory. "
                "ACAV100M is not an acceptable substitute."
            )


def _fetch_backbone(allowlist: Allowlist, dest_dir: Path) -> dict[str, Path]:
    paths: dict[str, Path] = {}
    for entry in allowlist.backbone:
        refuse_forbidden_heads(entry.url)
        paths[entry.id] = fetch_entry(entry, dest_dir, require_hash=True)
    for needed in ("melspectrogram", "embedding_model"):
        if needed not in paths:
            raise AllowlistError(f"allowlist backbone is missing {needed}")
    return paths


def _fetch_voices(
    allowlist: Allowlist, voice_ids: list[str], dest_dir: Path
) -> dict[str, Path]:
    by_id = {v.id: v for v in allowlist.voices}
    paths: dict[str, Path] = {}
    for vid in voice_ids:
        entry = by_id[vid]
        onnx = fetch_entry(entry, dest_dir, require_hash=bool(entry.sha256))
        cfg_url = entry.url + ".json"
        cfg_dest = onnx.with_suffix(onnx.suffix + ".json")
        if not cfg_dest.is_file():
            from wake_word_training.download import fetch_verified

            fetch_verified(cfg_url, cfg_dest, sha256="", label=f"voice config {vid}")
        paths[vid] = onnx
    return paths




def _write_run_manifest(
    out: Path,
    allowlist: Allowlist,
    split: SpeakerSplit,
    trace,
    model_path: Path,
) -> None:
    payload = {
        "phrase": PHRASE,
        "model": str(model_path),
        "speaker_split": {
            name: {
                "voices": split.voices(name),
                "speakers": len(split.identities(name)),
            }
            for name in ("train", "validation", "held_out")
        },
        "training": {
            "steps_run": trace.steps_run,
            "best_step": trace.best_step,
            "best_validation_loss": trace.best_validation_loss,
            "positive_validation_loss": trace.positive_validation_loss,
            "negative_validation_loss": trace.negative_validation_loss,
            "hard_negative_windows": trace.hard_negatives,
            "stopped_early": trace.stopped_early,
        },
        "manifest": [
            {
                "id": e.id,
                "kind": e.kind,
                "licence": e.licence,
                "url": e.url,
                "role": e.role,
                "notes": e.notes,
            }
            for e in allowlist.all_entries()
        ],
    }
    (out / "run_manifest.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
