#!/usr/bin/env bash
#
# Fetch the ONNX Runtime dylib that a macOS release bundles into
# Murmur.app/Contents/Frameworks. Signing re-signs it there with the app's Team
# ID, so a signed app loads it with library validation on. The version and
# archive checksum are read from runtime.rs, the same pins the in-app download
# uses, so the two cannot drift.
#
# Usage: fetch-ort-macos.sh <dest-dir>   (writes <dest-dir>/libonnxruntime.dylib)
set -euo pipefail

dest="${1:?usage: fetch-ort-macos.sh <dest-dir>}"
runtime_rs="$(git rev-parse --show-toplevel)/crates/murmur-core/src/stt/runtime.rs"

version="$(sed -nE 's/^const ORT_VERSION: &str = "([0-9.]+)";$/\1/p' "$runtime_rs")"
sha256="$(grep -A1 '^#\[cfg(target_os = "macos")\]$' "$runtime_rs" |
  sed -nE 's/^const ORT_ARCHIVE_SHA256: &str = "([0-9a-f]{64})";$/\1/p')"
if [ -z "$version" ] || [ "${#sha256}" -ne 64 ]; then
  echo "could not read the macOS ORT_VERSION / ORT_ARCHIVE_SHA256 pins from $runtime_rs" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
url="https://github.com/microsoft/onnxruntime/releases/download/v${version}/onnxruntime-osx-universal2-${version}.tgz"
curl -fsSL --retry 3 -o "$tmp/ort.tgz" "$url"
echo "${sha256}  $tmp/ort.tgz" | shasum -a 256 -c -

tar -xzf "$tmp/ort.tgz" -C "$tmp"
dylib="$tmp/onnxruntime-osx-universal2-${version}/lib/libonnxruntime.dylib"
# The app is a universal binary; a single-arch dylib would load on one slice only.
archs=" $(lipo -archs "$dylib") "
if [[ "$archs" != *" arm64 "* || "$archs" != *" x86_64 "* ]]; then
  echo "expected a universal (arm64 + x86_64) dylib, got:${archs}" >&2
  exit 1
fi

mkdir -p "$dest"
cp "$dylib" "$dest/libonnxruntime.dylib"
echo "ONNX Runtime ${version} ready at $dest/libonnxruntime.dylib"
