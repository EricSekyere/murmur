"""Download allowlisted artifacts with SHA-256 verification."""

from __future__ import annotations

import hashlib
import urllib.request
from pathlib import Path

from wake_word_training.allowlist import AllowlistEntry, AllowlistError, refuse_forbidden_heads


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch_verified(url: str, dest: Path, *, sha256: str, label: str) -> Path:
    refuse_forbidden_heads(url)
    refuse_forbidden_heads(str(dest))
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.is_file() and sha256:
        actual = sha256_file(dest)
        if actual == sha256.lower():
            return dest
        dest.unlink()
    tmp = dest.with_suffix(dest.suffix + ".part")
    urllib.request.urlretrieve(url, tmp)
    if sha256:
        actual = sha256_file(tmp)
        if actual != sha256.lower():
            tmp.unlink(missing_ok=True)
            raise AllowlistError(
                f"{label}: sha256 mismatch for {url} (got {actual}, expected {sha256})"
            )
    tmp.replace(dest)
    return dest


def fetch_entry(entry: AllowlistEntry, dest_dir: Path, *, require_hash: bool) -> Path:
    if not entry.url:
        raise AllowlistError(f"{entry.kind} {entry.id!r} has no url")
    if require_hash and not entry.sha256:
        raise AllowlistError(
            f"{entry.kind} {entry.id!r} is missing sha256; refusing unverified download"
        )
    suffix = Path(entry.url.split("?", 1)[0]).suffix or ".bin"
    dest = dest_dir / f"{entry.id}{suffix}"
    return fetch_verified(
        entry.url, dest, sha256=entry.sha256, label=f"{entry.kind} {entry.id}"
    )
