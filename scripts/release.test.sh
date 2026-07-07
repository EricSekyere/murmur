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

# --- Case 8: `Whats-New: skip` keeps the commit's own subject out ---
repo="$(new_repo case8)"
git -C "$repo" commit -q --allow-empty -m 'feat: internal refactor subject

Whats-New: skip'
run_script "$repo"
assert_not_contains "$repo/whatsnew.data.js" 'Internal refactor subject' \
  'skip suppresses the commit subject'
assert_not_contains "$repo/whatsnew.data.js" '"title":"Skip"' \
  'skip itself is not a bullet'
assert_contains "$repo/whatsnew.data.js" 'Bug fixes and improvements' \
  'release with everything skipped falls back to the generic line'

# --- Case 9: Whats-New-Skip retracts an earlier commit's subject ---
repo="$(new_repo case9)"
git -C "$repo" commit -q --allow-empty -m 'feat: clunky squash subject'
target="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" commit -q --allow-empty -m "chore: curate release notes

Whats-New-Skip: ${target:0:7}
Whats-New: Nice curated bullet | Something readable."
out="$(cd "$repo" && bash "$SCRIPT" notes.md whatsnew.data.js)"
assert_not_contains "$repo/whatsnew.data.js" 'Clunky squash subject' \
  'a 7-char Whats-New-Skip prefix retracts the subject'
assert_contains "$repo/whatsnew.data.js" '"title":"Nice curated bullet"' \
  'the curating commit adds its own bullets'
assert_contains "$repo/notes.md" 'clunky squash subject' \
  'GitHub notes still list the retracted commit'
if printf '%s' "$out" | grep -q 'release=true'; then
  echo 'PASS: retracted feat still gates the release'
  pass=$((pass + 1))
else
  echo 'FAIL: retracted feat still gates the release'
  echo "  output: $out"
  fail=$((fail + 1))
fi

# --- Case 10: Whats-New-Skip also retracts the target's trailers ---
repo="$(new_repo case10)"
git -C "$repo" commit -q --allow-empty -m 'feat: something

Whats-New: Regretted wording'
target="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" commit -q --allow-empty -m "chore: retract

Whats-New-Skip: ${target}"
run_script "$repo"
assert_not_contains "$repo/whatsnew.data.js" 'Regretted wording' \
  'a full-hash skip retracts curated trailers too'

# --- Case 11: too-short skip values are ignored ---
repo="$(new_repo case11)"
git -C "$repo" commit -q --allow-empty -m 'feat: keep me visible'
target="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" commit -q --allow-empty -m "chore: sloppy retract

Whats-New-Skip: ${target:0:6}"
run_script "$repo"
assert_contains "$repo/whatsnew.data.js" '"title":"Keep me visible"' \
  'skip values under 7 chars are ignored'

# --- Case 12: mentioning "BREAKING CHANGE" mid-sentence is not breaking ---
repo="$(new_repo case12)"
git -C "$repo" commit -q --allow-empty -m 'fix: tighten release detection

This commit documents how the BREAKING CHANGE footer is parsed but is
not itself breaking.'
out="$(cd "$repo" && bash "$SCRIPT" notes.md whatsnew.data.js)"
if printf '%s' "$out" | grep -q 'version=0.1.1'; then
  echo 'PASS: body prose mentioning BREAKING CHANGE stays a patch'
  pass=$((pass + 1))
else
  echo 'FAIL: body prose mentioning BREAKING CHANGE stays a patch'
  echo "  output: $out"
  fail=$((fail + 1))
fi

# --- Case 13: a real BREAKING CHANGE footer bumps (minor while pre-1.0) ---
repo="$(new_repo case13)"
git -C "$repo" commit -q --allow-empty -m 'fix: change config format

BREAKING CHANGE: old configs no longer load.'
out="$(cd "$repo" && bash "$SCRIPT" notes.md whatsnew.data.js)"
if printf '%s' "$out" | grep -q 'version=0.2.0'; then
  echo 'PASS: BREAKING CHANGE footer bumps the version'
  pass=$((pass + 1))
else
  echo 'FAIL: BREAKING CHANGE footer bumps the version'
  echo "  output: $out"
  fail=$((fail + 1))
fi

# --- Case 14: the spec's hyphenated BREAKING-CHANGE synonym also bumps ---
repo="$(new_repo case14)"
git -C "$repo" commit -q --allow-empty -m 'fix: change config format

BREAKING-CHANGE: old configs no longer load.'
out="$(cd "$repo" && bash "$SCRIPT" notes.md whatsnew.data.js)"
if printf '%s' "$out" | grep -q 'version=0.2.0'; then
  echo 'PASS: hyphenated BREAKING-CHANGE also bumps'
  pass=$((pass + 1))
else
  echo 'FAIL: hyphenated BREAKING-CHANGE also bumps'
  echo "  output: $out"
  fail=$((fail + 1))
fi

echo
echo "passed=$pass failed=$fail"
[ "$fail" -eq 0 ]
