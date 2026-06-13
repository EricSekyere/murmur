#!/usr/bin/env bash
set -euo pipefail

# Build Murmur for macOS. Run this on a Mac (it cannot cross-compile from
# Windows). Produces an unsigned .app and .dmg under
# target/<arch>-apple-darwin/release/bundle/.

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

red()    { printf '\033[1;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[1;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[1;33m%s\033[0m\n' "$*"; }
info()   { printf '\033[1;36m:: %s\033[0m\n' "$*"; }

if [[ "$(uname)" != "Darwin" ]]; then
  red "This script must run on macOS. (Murmur cannot cross-compile from another OS.)"
  exit 1
fi

# Prerequisites
FAIL=0
need() { command -v "$1" >/dev/null 2>&1 || { red "Missing: $1 — $2"; FAIL=1; }; }

need cargo "Install Rust from https://rustup.rs"
need cmake "Install with: brew install cmake"
if ! cargo tauri --version >/dev/null 2>&1; then
  red "Missing: cargo-tauri CLI"
  echo "  Install with: cargo install tauri-cli --version '^2'"
  FAIL=1
fi
if ! xcode-select -p >/dev/null 2>&1; then
  red "Missing: Xcode command line tools (clang/libclang)"
  echo "  Install with: xcode-select --install"
  FAIL=1
fi
[[ "$FAIL" -ne 0 ]] && { red "Fix the issues above and retry."; exit 1; }

# Build for the host architecture (arm64 on Apple Silicon, x86_64 on Intel).
ARCH="$(uname -m)"
case "$ARCH" in
  arm64)  TARGET="aarch64-apple-darwin" ;;
  x86_64) TARGET="x86_64-apple-darwin" ;;
  *)      red "Unsupported architecture: $ARCH"; exit 1 ;;
esac
rustup target add "$TARGET" >/dev/null 2>&1 || true

info "Formatting..."
cargo fmt --all

info "Building Murmur for macOS ($TARGET)..."
cd "$ROOT/crates/murmur-app"
# Parakeet is excluded (its DirectML provider is Windows-only); Whisper runs
# on Metal/Accelerate. cargo flags go after `--`.
cargo tauri build --target "$TARGET" --bundles app,dmg -- \
  --no-default-features --features macos

echo ""
green "Build complete!"
echo "  App:  target/$TARGET/release/bundle/macos/Murmur.app"
echo "  DMG:  target/$TARGET/release/bundle/dmg/"
echo ""
yellow "First launch: the build is unsigned, so right-click the app and choose Open"
yellow "to bypass Gatekeeper, then grant Microphone and Accessibility permissions"
yellow "in System Settings > Privacy & Security."
