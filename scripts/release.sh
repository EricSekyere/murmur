#!/usr/bin/env bash
#
# Derive the next release version and notes from Conventional Commits made since
# the last `v*` tag. The git tag is the source of truth for the released
# version; the manifest files are bumped to match only at build time (see
# .github/workflows/release.yml), so this never needs a manual version edit.
#
# Bump rules (Conventional Commits):
#   feat                       -> minor
#   fix | perf                 -> patch
#   <type>! / "BREAKING CHANGE" -> major (but minor while still 0.x, so a single
#                                         breaking change can't silently jump to
#                                         1.0.0 before you mean it to)
# Only feat/fix/perf/breaking gate a release. A range of only chore/docs/ci/etc.
# yields release=false, matching the old "promoting docs never re-releases".
#
# Usage:  scripts/release.sh [notes-output-file]   (default: release-notes.md)
# Prints `version=`, `bump=`, `release=` to stdout; also appends them to
# $GITHUB_OUTPUT when set (CI). Run it locally to preview the next release.
set -euo pipefail

notes_file="${1:-release-notes.md}"
repo_url="https://github.com/EricSekyere/murmur"

last_tag="$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)"
if [ -n "$last_tag" ]; then
  base="${last_tag#v}"
  range="${last_tag}..HEAD"
else
  base="0.0.0"
  range="HEAD"
fi
IFS=. read -r major minor patch <<<"$base"

has_break=0
has_feat=0
has_patch=0
feats=""
fixes=""
perfs=""

while IFS= read -r hash; do
  [ -n "$hash" ] || continue
  subject="$(git show -s --format=%s "$hash")"
  body="$(git show -s --format=%b "$hash")"

  type=""
  bang=""
  if [[ "$subject" =~ ^([A-Za-z]+)(\([^\)]*\))?(!)?: ]]; then
    type="$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
    bang="${BASH_REMATCH[3]}"
  fi

  if [ -n "$bang" ] || printf '%s' "$body" | grep -q 'BREAKING CHANGE'; then
    has_break=1
  fi

  desc="${subject#*: }"
  short="${hash:0:7}"
  case "$type" in
  feat) has_feat=1 && feats+="- ${desc} (${short})"$'\n' ;;
  fix) has_patch=1 && fixes+="- ${desc} (${short})"$'\n' ;;
  perf) has_patch=1 && perfs+="- ${desc} (${short})"$'\n' ;;
  esac
done < <(git log --no-merges --format=%H "$range")

level="none"
if [ "$has_break" -eq 1 ]; then
  level="major"
elif [ "$has_feat" -eq 1 ]; then
  level="minor"
elif [ "$has_patch" -eq 1 ]; then
  level="patch"
fi

if [ "$level" = "none" ]; then
  release="false"
  version="$base"
else
  release="true"
  # Pre-1.0: a breaking change bumps minor, not major.
  if [ "$level" = "major" ] && [ "$major" -eq 0 ]; then
    level="minor"
  fi
  case "$level" in
  major)
    major=$((major + 1))
    minor=0
    patch=0
    ;;
  minor)
    minor=$((minor + 1))
    patch=0
    ;;
  patch) patch=$((patch + 1)) ;;
  esac
  version="${major}.${minor}.${patch}"
fi

{
  echo "## What's new in v${version}"
  echo
  if [ -n "$feats" ]; then
    echo "### Features"
    printf '%s\n' "$feats"
  fi
  if [ -n "$fixes" ]; then
    echo "### Fixes"
    printf '%s\n' "$fixes"
  fi
  if [ -n "$perfs" ]; then
    echo "### Performance"
    printf '%s\n' "$perfs"
  fi
  if [ -n "$last_tag" ]; then
    echo "**Full changelog:** ${repo_url}/compare/${last_tag}...v${version}"
    echo
  fi
  cat <<'INSTALL'
## Install

**Windows:** download the `.exe` installer and run it. Requires Windows 10/11 x64 with an AVX2-capable CPU (any Intel or AMD CPU from about 2013 onward). An NVIDIA GPU is optional and accelerates the larger models.

**Linux (X11):** download the `.AppImage`, `chmod +x`, and run it, or install the `.deb`. Best on an X11 session; on Wayland, typing into other apps is limited.

Models download automatically on first launch. Installed copies update themselves.
INSTALL
} >"$notes_file"

echo "version=${version}"
echo "bump=${level}"
echo "release=${release}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=${version}"
    echo "bump=${level}"
    echo "release=${release}"
  } >>"$GITHUB_OUTPUT"
fi
