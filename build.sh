#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# ── Colors ────────────────────────────────────────────────────────────────────
red()   { printf '\033[1;31m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[1;33m%s\033[0m\n' "$*"; }
info()  { printf '\033[1;36m:: %s\033[0m\n' "$*"; }

# ── Prerequisite checks ──────────────────────────────────────────────────────
FAIL=0

has_cmd() {
    command -v "$1" &>/dev/null
}

has_any_cmd() {
    local cmd
    for cmd in "$@"; do
        if has_cmd "$cmd"; then
            return 0
        fi
    done
    return 1
}

check_cmd() {
    if ! command -v "$1" &>/dev/null; then
        red "Missing: $1 — $2"
        FAIL=1
    fi
}

check_cmd cargo    "Install Rust 1.93+ from https://rustup.rs"
check_cmd cmake    "Install CMake from https://cmake.org (needed for whisper.cpp)"

# cargo-tauri CLI
if ! cargo tauri --version &>/dev/null; then
    red "Missing: cargo-tauri CLI"
    echo "  Install with: cargo install tauri-cli --version '^2'"
    FAIL=1
fi

# LLVM / libclang (Windows only — needed for whisper-rs bindgen)
if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* || "$OSTYPE" == win* ]]; then
    if [[ -z "${LIBCLANG_PATH:-}" ]]; then
        # Auto-detect from common install locations
        for candidate in \
            "/c/Program Files/LLVM/bin" \
            "/c/Program Files (x86)/LLVM/bin" \
            "$PROGRAMFILES/LLVM/bin" \
        ; do
            if [[ -f "$candidate/libclang.dll" ]]; then
                export LIBCLANG_PATH="$candidate"
                yellow "Auto-detected LIBCLANG_PATH=$LIBCLANG_PATH"
                break
            fi
        done
        if [[ -z "${LIBCLANG_PATH:-}" ]]; then
            red "Missing: LIBCLANG_PATH is not set and LLVM was not found"
            echo "  Install LLVM from https://releases.llvm.org and set LIBCLANG_PATH"
            FAIL=1
        fi
    fi

    # NSIS (needed for Windows installer bundling)
    NSIS_FOUND=0
    if has_any_cmd makensis makensis.exe; then
        NSIS_FOUND=1
    else
        for nsis_dir in \
            "/c/Program Files (x86)/NSIS" \
            "/c/Program Files/NSIS" \
        ; do
            if [[ -f "$nsis_dir/makensis.exe" ]]; then
                export PATH="$nsis_dir:$PATH"
                NSIS_FOUND=1
                yellow "Added NSIS to PATH from $nsis_dir"
                break
            fi
        done
    fi
    if [[ "$NSIS_FOUND" -eq 0 ]]; then
        yellow "Warning: NSIS not found — installer (.exe) will not be created"
        echo "  Install with: choco install nsis  (or winget install NSIS.NSIS)"
        echo "  The standalone binary will still be built."
    fi
fi

if [[ "$FAIL" -ne 0 ]]; then
    red "Aborting: fix the issues above and retry."
    exit 1
fi

# ── Lint ──────────────────────────────────────────────────────────────────────
info "Formatting..."
cargo fmt --all

info "Linting..."
if ! cargo clippy --workspace -- -D warnings; then
    yellow "Clippy warnings found (non-fatal, continuing build)"
fi

# ── Build ─────────────────────────────────────────────────────────────────────
info "Building Tauri app (release)..."
cd "$ROOT/crates/murmur-app"

# On Windows, Tauri's --bundles flag only accepts "msi" or "nsis" (not "none").
# If NSIS is missing, we must skip bundling entirely via --no-bundle.
TAURI_ARGS=()
DID_BUNDLE=1
if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* || "$OSTYPE" == win* ]]; then
    if ! has_any_cmd makensis makensis.exe; then
        TAURI_ARGS+=(--no-bundle)
        DID_BUNDLE=0
    fi
fi

cargo tauri build "${TAURI_ARGS[@]}"

info "Building CLI (release)..."
cd "$ROOT"
cargo build --release -p murmur-cli

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
green "Build complete!"
echo ""

EXT=""
if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* || "$OSTYPE" == win* ]]; then
    EXT=".exe"
fi

echo "  App binary:  target/release/murmur-app${EXT}"
echo "  CLI binary:  target/release/murmur${EXT}"
if [[ "$DID_BUNDLE" -eq 1 && -d "target/release/bundle/nsis" ]]; then
    echo "  Installer:   target/release/bundle/nsis/"
elif [[ "$DID_BUNDLE" -eq 1 && -d "target/release/bundle/dmg" ]]; then
    echo "  Installer:   target/release/bundle/dmg/"
fi
echo ""
