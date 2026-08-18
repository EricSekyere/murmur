#!/usr/bin/env bash
# Fetch the three allowlisted corpora into data/<id>/.
# Only OpenSLR sources named in allowlist.toml. No ACAV100M, no MIT IR Survey.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DATA="$ROOT/data"
mkdir -p "$DATA"

# id|url|archive
ENTRIES=(
  "musan|https://www.openslr.org/resources/17/musan.tar.gz|musan.tar.gz"
  "rirs_noises|https://www.openslr.org/resources/28/rirs_noises.zip|rirs_noises.zip"
  "librispeech|https://www.openslr.org/resources/12/train-clean-100.tar.gz|train-clean-100.tar.gz"
)

for entry in "${ENTRIES[@]}"; do
  IFS='|' read -r id url archive <<<"$entry"
  dest="$DATA/$id"
  mkdir -p "$dest"
  out="$dest/$archive"

  if [[ -f "$dest/.extracted" ]]; then
    echo ":: $id already extracted, skipping"
    continue
  fi

  echo ":: fetching $id from $url"
  # -C - resumes a partial file so an interrupted run does not restart.
  if ! curl -fL -C - --retry 5 --retry-delay 10 -o "$out" "$url"; then
    echo "!! download failed: $id"
    continue
  fi

  echo ":: extracting $id"
  case "$archive" in
    *.tar.gz) tar -xzf "$out" -C "$dest" && touch "$dest/.extracted" ;;
    *.zip)    unzip -q -o "$out" -d "$dest" && touch "$dest/.extracted" ;;
  esac

  if [[ -f "$dest/.extracted" ]]; then
    rm -f "$out"
    echo ":: $id done"
  else
    echo "!! extract failed: $id"
  fi
done

echo ":: corpora summary"
du -sh "$DATA"/* 2>/dev/null
