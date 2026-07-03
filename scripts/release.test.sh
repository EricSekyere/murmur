#!/usr/bin/env bash
# Tests for the What's New generation in scripts/release.sh, focused on the
# `Whats-New: Title | body` trailer support. Each case builds a throwaway git
# repo, commits, runs release.sh, and asserts on the generated whatsnew.data.js.
#
# Run:  bash scripts/release.test.sh
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")" && pwd)/release.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0

assert_contains() {
  local file="$1" needle="$2" label="$3"
  if grep -qF -- "$needle" "$file"; then
    echo "PASS: $label"
    pass=$((pass + 1))
  else
    echo "FAIL: $label"
    echo "  expected to find: $needle"
    echo "  in: $(cat "$file")"
    fail=$((fail + 1))
  fi
}

assert_not_contains() {
  local file="$1" needle="$2" label="$3"
  if grep -qF -- "$needle" "$file"; then
    echo "FAIL: $label"
    echo "  expected NOT to find: $needle"
    echo "  in: $(cat "$file")"
    fail=$((fail + 1))
  else
    echo "PASS: $label"
    pass=$((pass + 1))
  fi
}

new_repo() {
  local dir="$WORK/$1"
  mkdir -p "$dir"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email test@example.com
  git -C "$dir" config user.name Test
  git -C "$dir" commit -q --allow-empty -m 'chore: init'
  git -C "$dir" tag v0.1.0
  echo "$dir"
}

run_script() {
  local dir="$1"
  (cd "$dir" && bash "$SCRIPT" notes.md whatsnew.data.js >/dev/null)
}

# --- Case 1: feat subject without trailer still becomes a bullet ---
repo="$(new_repo case1)"
git -C "$repo" commit -q --allow-empty -m 'feat: add voice commands'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '"title":"Add voice commands"' \
  'feat subject becomes a highlight'

# --- Case 2: fix commit with trailer contributes title + body ---
repo="$(new_repo case2)"
git -C "$repo" commit -q --allow-empty -m 'fix(stt): rework model loading

Whats-New: Faster startup | Model loading now happens in the background.'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '"title":"Faster startup"' \
  'trailer on a fix produces a highlight title'
assert_contains "$repo/whatsnew.data.js" '"body":"Model loading now happens in the background."' \
  'trailer body after | is captured'
assert_not_contains "$repo/whatsnew.data.js" 'Bug fixes and improvements' \
  'trailer suppresses the generic fallback'

# --- Case 3: fix without trailer contributes nothing (generic fallback) ---
repo="$(new_repo case3)"
git -C "$repo" commit -q --allow-empty -m 'fix(app): allow unused fields'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" 'Bug fixes and improvements' \
  'fixes-only release without trailers keeps the generic line'
assert_not_contains "$repo/whatsnew.data.js" 'Allow unused fields' \
  'untagged fix subject does not leak into highlights'

# --- Case 4: one squash commit with two trailers yields two bullets ---
repo="$(new_repo case4)"
git -C "$repo" commit -q --allow-empty -m 'feat: big roadmap squash

Whats-New: Ask for help by voice | A new Help tab answers questions offline.
Whats-New: A fresh new look | The app and pill share a redesigned interface.'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '"title":"Ask for help by voice"' \
  'first trailer of a squash commit'
assert_contains "$repo/whatsnew.data.js" '"title":"A fresh new look"' \
  'second trailer of the same commit'

# --- Case 5: feat with trailer uses the trailer, not the subject ---
repo="$(new_repo case5)"
git -C "$repo" commit -q --allow-empty -m 'feat: internal plumbing subject

Whats-New: Nicer wording for users'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '"title":"Nicer wording for users"' \
  'trailer overrides the feat subject'
assert_not_contains "$repo/whatsnew.data.js" 'Internal plumbing subject' \
  'feat subject suppressed when a trailer exists'

# --- Case 6: trailer text with quotes/backslashes is JSON-escaped ---
repo="$(new_repo case6)"
git -C "$repo" commit -q --allow-empty -m 'feat: escaping

Whats-New: Say "hello" | Path C:\Models now works.'
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '\"hello\"' \
  'quotes in trailer are escaped'
assert_contains "$repo/whatsnew.data.js" 'C:\\Models' \
  'backslashes in trailer are escaped'

# --- Case 7: trailer on a chore does not gate a release by itself ---
repo="$(new_repo case7)"
git -C "$repo" commit -q --allow-empty -m 'chore: housekeeping

Whats-New: Should not release'
out="$(cd "$repo" && bash "$SCRIPT" notes.md whatsnew.data.js)"
if printf '%s' "$out" | grep -q 'release=false'; then
  echo 'PASS: chore with trailer still releases nothing'
  pass=$((pass + 1))
else
  echo "FAIL: chore with trailer still releases nothing"
  echo "  output: $out"
  fail=$((fail + 1))
fi

echo
echo "passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
