"""Pinned, checksum-verified artifacts for the optional local ASR experiment."""

import hashlib
import json
import logging
import os
from pathlib import Path


def pins():
    return json.loads(Path(__file__).with_name("model-sha256.json").read_text())


def verify(directory):
    manifest = pins()
    for name, expected in manifest["files"].items():
        path = directory / name
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            raise ValueError(f"Model checksum mismatch: {name}; download it again")
    return manifest


def download(directory):
    os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
    from huggingface_hub import hf_hub_download

    manifest = pins()
    for name, expected in manifest["files"].items():
        path = Path(hf_hub_download(
            repo_id=manifest["repository"], revision=manifest["revision"],
            filename=name, local_dir=directory, token=False,
        ))
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            path.unlink()
            logging.warning("Removed corrupt model artifact: %s", name)
            raise ValueError(f"Model checksum mismatch: {name}; retry the download")
    verify(directory)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    download(args.directory.resolve())
