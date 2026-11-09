#!/usr/bin/env bash
# Pin the CPU baseline that whisper.cpp (ggml) compiles for in Linux builds
# that leave the runner: release bundles and build-linux's test bundles.
#
# ggml compiles with -march=native unless SOURCE_DATE_EPOCH is set, so a build
# inherits whatever CPU the hosted runner had; v0.29.1 picked up AVX-512. With
# native off ggml enables no SIMD by itself, so CFLAGS supply x86-64-v3 (AVX2,
# FMA, F16C, BMI2), the baseline Windows gets from /arch:AVX2.
# check-cpu-baseline.sh verifies the result.
set -euo pipefail

baseline=-march=x86-64-v3
{
    echo "SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)"
    echo "CFLAGS=${CFLAGS:+$CFLAGS }$baseline"
    echo "CXXFLAGS=${CXXFLAGS:+$CXXFLAGS }$baseline"
} >> "$GITHUB_ENV"
echo "C/C++ baseline: $baseline, ggml native detection off"
