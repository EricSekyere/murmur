"""Licence-gated training orchestration. Heavy deps are imported lazily."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

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
    PHRASE,
    SAMPLE_RATE,
    augment_clip,
    iter_audio_files,
    iter_clips_capped,
    pad_for_window,
    read_audio,
    resample_16k,
    synthesis_variation,
    synthesize_with_piper,
)
from wake_word_training.download import fetch_entry

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
    parser.add_argument("--n-samples", type=int, default=4000)
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
    parser.add_argument("--held-out-fraction", type=float, default=0.2)
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


def _train(allowlist: Allowlist, voice_ids: list[str], args: argparse.Namespace) -> None:
    from wake_word_training.features import Backbone
    from wake_word_training.model import export_onnx, train_head

    out = args.output_dir
    data = args.data_dir
    _require_dataset_dirs(allowlist, data)
    backbone_dir = data / "backbone"
    voice_dir = data / "voices"
    backbone_paths = _fetch_backbone(allowlist, backbone_dir)
    voice_paths = _fetch_voices(allowlist, voice_ids, voice_dir)
    n_hold = max(1, int(round(len(voice_ids) * args.held_out_fraction)))
    held_out = voice_ids[-n_hold:]
    train_voices = voice_ids[:-n_hold] or voice_ids[:1]
    pos_dir = out / "clips" / "positives"
    hold_dir = out / "clips" / "held_out"
    _synthesize(voice_paths, train_voices, args.phrase, pos_dir, args.n_samples)
    _synthesize(
        voice_paths,
        held_out,
        args.phrase,
        hold_dir,
        max(20, args.n_samples // 10),
    )
    noise = _load_pool(data, allowlist, role="noise")
    rir = _load_pool(data, allowlist, role="rir")
    backbone = Backbone(backbone_paths["melspectrogram"], backbone_paths["embedding_model"])
    positives = _windows_from_dir(backbone, pos_dir, noise=noise, rir=rir)
    negatives = _sample_background_windows(
        backbone,
        data,
        allowlist,
        max_hours=args.max_background_hours,
        max_windows=args.max_negative_windows,
    )
    if negatives is None or len(negatives) == 0:
        raise AllowlistError(
            "no background windows; place allowlisted background audio under data/<id>/"
        )
    model = train_head(positives, negatives, steps=args.steps)
    dest = out / "hey_murmur.onnx"
    export_onnx(model, dest)
    _write_run_manifest(out, allowlist, train_voices, held_out, dest)
    print(f"wrote {dest}")


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


def _synthesize(
    voice_paths: dict[str, Path],
    voice_ids: list[str],
    phrase: str,
    dest_dir: Path,
    n_samples: int,
) -> None:
    if not voice_ids:
        raise AllowlistError("no voices to synthesize")
    per_voice = max(1, n_samples // len(voice_ids))
    for vid in voice_ids:
        speakers = _num_speakers(voice_paths[vid])
        for i in range(per_voice):
            dest = dest_dir / vid / f"{i:05d}.wav"
            if dest.is_file():
                continue
            synthesize_with_piper(
                voice_paths[vid],
                phrase,
                dest,
                variation=synthesis_variation(i, speakers),
            )


def _num_speakers(voice_onnx: Path) -> int:
    """Speakers in a Piper voice, from the config that ships beside it.

    libritts and vctk carry hundreds; using only speaker 0 threw that away.
    """
    config = voice_onnx.with_suffix(voice_onnx.suffix + ".json")
    if not config.is_file():
        return 1
    try:
        return max(1, int(json.loads(config.read_text(encoding="utf-8"))["num_speakers"]))
    except (ValueError, KeyError, OSError):
        return 1


def _load_pool(data_root: Path, allowlist: Allowlist, *, role: str) -> list[tuple]:
    pool: list[tuple] = []
    for dataset in allowlist.datasets:
        if dataset.role != role:
            continue
        root = data_root / dataset.id
        for path in iter_audio_files(root):
            samples, rate = read_audio(path)
            pool.append((resample_16k(samples, rate), dataset.id, dataset.licence))
    return pool


def _windows_from_dir(backbone, clips_dir: Path, *, noise, rir):
    import numpy as np

    from wake_word_training.features import MIN_HEAD_WINDOW_SAMPLES

    windows = []
    wavs = list(iter_audio_files(clips_dir))
    for i, wav in enumerate(wavs):
        samples, rate = read_audio(wav)
        audio = resample_16k(samples, rate)
        n = noise[i % len(noise)][0] if noise else None
        r = rir[i % len(rir)][0] if rir else None
        snr = 5.0 + (i % 16)
        # Pad before augmenting so reverb and noise cover the whole window.
        padded = pad_for_window(audio, MIN_HEAD_WINDOW_SAMPLES, filler=n)
        augmented = augment_clip(padded, noise=n, rir=r, snr_db=snr)
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


def _sample_background_windows(
    backbone,
    data_root: Path,
    allowlist: Allowlist,
    *,
    max_hours: float,
    max_windows: int,
):
    """Stream background clips until hour or window caps; never load the full tree."""
    import numpy as np

    collected: list = []
    hours_left = max_hours
    n_windows = 0
    for dataset in allowlist.datasets:
        if dataset.role != "background":
            continue
        if hours_left <= 0 or n_windows >= max_windows:
            break
        for audio, _path in iter_clips_capped(
            data_root / dataset.id, max_hours=hours_left
        ):
            hours_left -= len(audio) / SAMPLE_RATE / 3600.0
            windows = backbone.windows(audio)
            if len(windows) == 0:
                continue
            collected.append(windows)
            n_windows += len(windows)
            if hours_left <= 0 or n_windows >= max_windows:
                break
    if not collected:
        return np.zeros((0, 16, 96), dtype=np.float32)
    stacked = np.concatenate(collected, axis=0)
    if len(stacked) > max_windows:
        stacked = stacked[:max_windows]
    return stacked


def _write_run_manifest(
    out: Path,
    allowlist: Allowlist,
    train_voices: list[str],
    held_out: list[str],
    model_path: Path,
) -> None:
    payload = {
        "phrase": PHRASE,
        "model": str(model_path),
        "train_voices": train_voices,
        "held_out_voices": held_out,
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
