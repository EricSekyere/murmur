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
# In-app "What's New" copy: any commit may carry one or more trailers of the
# form `Whats-New: Title | optional body` (body after the first `|`). Trailers
# are the curated source for the dialog and replace the commit's subject there;
# without one, feat/perf subjects are used as before. Trailers never affect the
# version bump. A squash commit can carry several trailers, one per highlight.
# `Whats-New: skip` keeps the commit's own subject out of the dialog, and
# `Whats-New-Skip: <7+ hash chars>` on any commit retracts everything an
# already-pushed commit contributed. Skips affect only the dialog, never the
# GitHub notes or the bump.
#
# Usage:  scripts/release.sh [notes-output-file]   (default: release-notes.md)
# Prints `version=`, `bump=`, `release=` to stdout; also appends them to
# $GITHUB_OUTPUT when set (CI). Run it locally to preview the next release.
set -euo pipefail

notes_file="${1:-release-notes.md}"
# Optional: emit the bundled in-app "What's New" data (window.WHATS_NEW_DATA)
# here. The release build copies it into the frontend so the dialog always
# reflects the release without hand-editing whatsnew.js.
whatsnew_file="${2:-}"
repo_url="https://github.com/EricSekyere/murmur"

# Uppercase the first character (commit descriptions are usually lowercased).
cap() { printf '%s%s' "$(printf '%s' "${1:0:1}" | tr '[:lower:]' '[:upper:]')" "${1:1}"; }

# Escape a string for a JSON/JS double-quoted literal. The file is loaded as an
# external script (not inline HTML), so only `\` and `"` need escaping; commit
# subjects and unfolded trailer values are single-line, so there are no newlines
# to handle. sed (not bash replacement) because bash's `${//}` backslash
# handling is version-dependent. Backslashes first, then quotes, so the
# quote-escape's own `\` is not doubled.
json_esc() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

trim() {
  local s="$1"
  s="${s#"${s%%[![:space:]]*}"}"
  printf '%s' "${s%"${s##*[![:space:]]}"}"
}

last_tag="$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)"
if [ -n "$last_tag" ]; then
  base="${last_tag#v}"
  range="${last_tag}..HEAD"
else
  base="0.0.0"
  range="HEAD"
fi
IFS=. read -r major minor patch <<<"$base"

# After-the-fact retraction: `Whats-New-Skip: <hash>` on any commit in the
# range removes everything the referenced commit contributed to the dialog
# (subject and trailers), for copy that can no longer be amended into history.
# At least 7 hash characters; shorter values are ignored with a warning. Skips
# never touch the GitHub notes or the version bump.
skip_prefixes=()
while IFS= read -r line; do
  line="$(trim "$line")"
  [ -n "$line" ] || continue
  if [ "${#line}" -lt 7 ]; then
    echo "warning: ignoring Whats-New-Skip '${line}' (need at least 7 hash characters)" >&2
    continue
  fi
  skip_prefixes+=("${line,,}")
done <<<"$(git log --no-merges --format='%(trailers:key=Whats-New-Skip,valueonly,unfold)' "$range")"

has_break=0
has_feat=0
has_patch=0
feats=""
fixes=""
perfs=""
# User-facing highlights for the in-app dialog. Curated `Whats-New:` trailers
# win; otherwise feat/perf subjects (every untagged fix would be too noisy for
# a "What's New").
hl_titles=()
hl_bodies=()

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

  # Anchored to a footer line per the Conventional Commits spec (either
  # spelling, colon required): prose merely mentioning "BREAKING CHANGE"
  # must not trigger a major bump.
  if [ -n "$bang" ] || printf '%s' "$body" | grep -qE '^BREAKING[- ]CHANGE:'; then
    has_break=1
  fi

  suppressed=0
  for p in "${skip_prefixes[@]}"; do
    if [[ "$hash" == "$p"* ]]; then
      suppressed=1
    fi
  done

  # `Whats-New: skip` is not a bullet, but still marks the commit as curated
  # so its subject stays out of the dialog.
  had_trailer=0
  if [ "$suppressed" -eq 0 ]; then
    while IFS= read -r line; do
      line="$(trim "$line")"
      [ -n "$line" ] || continue
      had_trailer=1
      [ "${line,,}" = "skip" ] && continue
      if [[ "$line" == *"|"* ]]; then
        hl_titles+=("$(cap "$(trim "${line%%|*}")")")
        hl_bodies+=("$(trim "${line#*|}")")
      else
        hl_titles+=("$(cap "$line")")
        hl_bodies+=("")
      fi
    done <<<"$(git show -s --format='%(trailers:key=Whats-New,valueonly,unfold)' "$hash")"
  fi

  desc="${subject#*: }"
  short="${hash:0:7}"
  case "$type" in
  feat)
    has_feat=1
    feats+="- ${desc} (${short})"$'\n'
    if [ "$suppressed" -eq 0 ] && [ "$had_trailer" -eq 0 ]; then
      hl_titles+=("$(cap "$desc")")
      hl_bodies+=("")
    fi
    ;;
  fix) has_patch=1 && fixes+="- ${desc} (${short})"$'\n' ;;
  perf)
    has_patch=1
    perfs+="- ${desc} (${short})"$'\n'
    if [ "$suppressed" -eq 0 ] && [ "$had_trailer" -eq 0 ]; then
      hl_titles+=("$(cap "$desc")")
      hl_bodies+=("")
    fi
    ;;
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

# Generate the in-app "What's New" data, one bullet per highlight. Copy quality
# follows commit quality: add `Whats-New: Title | body` trailers for curated
# bullets, or at least write user-facing feat/perf subjects. A release with
# neither falls back to a generic line.
if [ -n "$whatsnew_file" ]; then
  {
    printf 'window.WHATS_NEW_DATA = {"version":"%s","items":[' "$version"
    if [ "${#hl_titles[@]}" -eq 0 ]; then
      printf '{"title":"Bug fixes and improvements","body":"This update focuses on stability and polish. The full list of changes is in the release notes on GitHub."}'
    else
      sep=""
      for i in "${!hl_titles[@]}"; do
        printf '%s{"title":"%s","body":"%s"}' "$sep" "$(json_esc "${hl_titles[$i]}")" "$(json_esc "${hl_bodies[$i]}")"
        sep=","
      done
    fi
    printf ']};\n'
  } >"$whatsnew_file"
fi

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
