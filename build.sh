#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# Colors
red()   { printf '\033[1;31m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[1;33m%s\033[0m\n' "$*"; }
info()  { printf '\033[1;36m:: %s\033[0m\n' "$*"; }

# Windows helper: clean stale whisper install artifacts that can fail with
# "file INSTALL cannot set permissions on .../whisper.lib" during repeated builds.
cleanup_whisper_artifacts() {
    if [[ "$OSTYPE" != msys* && "$OSTYPE" != cygwin* && "$OSTYPE" != win* ]]; then
        return
    fi

    local removed=0
    local lib
    shopt -s nullglob
    for lib in "$ROOT"/target/release/build/whisper-rs-sys-*/out/lib/whisper.lib \
               "$ROOT"/target/debug/build/whisper-rs-sys-*/out/lib/whisper.lib
    do
        if [[ -f "$lib" ]]; then
            rm -f "$lib"
            removed=1
        fi
    done
    shopt -u nullglob

    if [[ "$removed" -eq 1 ]]; then
        yellow "Removed stale whisper-rs-sys install artifacts"
    fi
}

# Prerequisite checks
FAIL=0

# Extra cargo feature flags (e.g. --features cuda), filled in by detection below.
CARGO_FEATURE_ARGS=()

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
        red "Missing: $1 - $2"
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

# LLVM / libclang (Windows only, needed for whisper-rs bindgen)
if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* || "$OSTYPE" == win* ]]; then
    # whisper.cpp MUST be compiled optimized. When CMAKE_GENERATOR is unset,
    # cmake-rs overrides CMAKE_C_FLAGS_RELEASE with flags it computed itself,
    # and it deliberately strips every -O*/​/O* flag while doing so ("let
    # cmake deal with optimization" — cmake-0.1.57 lib.rs:722). Net result:
    # MSVC compiles whisper/ggml with no /O2 and no /DNDEBUG, ~100x slower —
    # transcriptions take minutes and look like hangs.
    #
    # Setting CMAKE_GENERATOR explicitly disables that override path, so
    # CMake's default Release flags (/O2 /Ob2 /DNDEBUG) survive. The CFLAGS
    # below additionally enable AVX2 SIMD (GGML_NATIVE is a no-op under
    # MSVC, so AVX2 stays off without this). Note: /O-prefixed flags in
    # CFLAGS would be stripped by cmake-rs — do not put /O2 here, it must
    # come from the generator's Release config.
    export CMAKE_GENERATOR="${CMAKE_GENERATOR:-Visual Studio 16 2019}"
    export CFLAGS="${CFLAGS:-} /DNDEBUG /arch:AVX2"
    export CXXFLAGS="${CXXFLAGS:-} /DNDEBUG /arch:AVX2"
    yellow "Native C/C++ deps: generator='$CMAKE_GENERATOR', extra flags: /DNDEBUG /arch:AVX2"

    # CUDA (optional): build whisper with GPU acceleration when the toolkit
    # is installed. whisper-rs-sys reads CUDA_PATH; cmake reads CUDAARCHS.
    CUDA_ROOT=""
    for cuda_dir in "/c/Program Files/NVIDIA GPU Computing Toolkit/CUDA"/v*; do
        if [[ -f "$cuda_dir/bin/nvcc.exe" ]]; then
            CUDA_ROOT="$cuda_dir"
        fi
    done
    if [[ -n "$CUDA_ROOT" ]]; then
        export PATH="$CUDA_ROOT/bin:$PATH"
        CUDA_ROOT_WIN="$(cygpath -w "$CUDA_ROOT" 2>/dev/null || echo "$CUDA_ROOT")"
        if [[ -z "${CUDA_PATH:-}" ]]; then
            export CUDA_PATH="$CUDA_ROOT_WIN"
        fi
        # MSBuild's "CUDA <ver>.props" resolves CudaToolkitDir from the
        # VERSIONED env var (e.g. CUDA_PATH_V12_6), not CUDA_PATH. Shells
        # started before the toolkit install don't have it — derive it.
        cuda_ver="$(basename "$CUDA_ROOT")"          # e.g. v12.6
        cuda_ver_env="CUDA_PATH_${cuda_ver#v}"       # e.g. CUDA_PATH_12.6
        cuda_ver_env="${cuda_ver_env//./_}"          # e.g. CUDA_PATH_12_6
        cuda_ver_env="CUDA_PATH_V${cuda_ver_env#CUDA_PATH_}"
        export "$cuda_ver_env=$CUDA_ROOT_WIN"
        # RTX 4050 (Ada) = compute capability 8.9. Building a single arch
        # keeps the CUDA kernel compile to minutes instead of an hour.
        export CUDAARCHS="${CUDAARCHS:-89}"
        CARGO_FEATURE_ARGS+=(--features cuda)
        yellow "CUDA toolkit found at $CUDA_ROOT — building whisper with GPU acceleration (sm_$CUDAARCHS)"
    else
        yellow "CUDA toolkit not found — whisper will run on CPU"
    fi
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
        yellow "Warning: NSIS not found - installer (.exe) will not be created"
        echo "  Install with: choco install nsis  (or winget install NSIS.NSIS)"
        echo "  The standalone binary will still be built."
    fi
fi

if [[ "$FAIL" -ne 0 ]]; then
    red "Aborting: fix the issues above and retry."
    exit 1
fi

# Lint
info "Formatting..."
cargo fmt --all

info "Linting..."
if ! cargo clippy --workspace -- -D warnings; then
    yellow "Clippy warnings found (non-fatal, continuing build)"
fi

# Build
info "Building Tauri app (release)..."
cd "$ROOT/crates/murmur-app"
cleanup_whisper_artifacts

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

cargo tauri build "${TAURI_ARGS[@]}" "${CARGO_FEATURE_ARGS[@]}"

# When built with CUDA, place the CUDA runtime DLLs next to the exe so the
# app runs regardless of whether the CUDA bin dir is on PATH.
if [[ -n "${CUDA_ROOT:-}" ]]; then
    for dll in cudart64_12.dll cublas64_12.dll cublasLt64_12.dll; do
        src="$CUDA_ROOT/bin/$dll"
        dest="$ROOT/target/release/$dll"
        if [[ -f "$src" && ( ! -f "$dest" || "$src" -nt "$dest" ) ]]; then
            cp -f "$src" "$dest"
            yellow "Copied $dll next to murmur-app.exe"
        fi
    done
fi

info "Building CLI (release)..."
cd "$ROOT"
cleanup_whisper_artifacts
cargo build --release -p murmur-cli

# Summary
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
