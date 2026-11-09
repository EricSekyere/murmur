#!/usr/bin/env bash
# Fail if a Linux x86-64 binary needs more than the baseline that
# cpu-baseline-env.sh pins, or if whisper.cpp (ggml) lost its SIMD.
#
# ggml has no runtime CPU dispatch in our build, so AVX-512 anywhere in it
# crashes with "illegal instruction" on CPUs without AVX-512, as v0.29.1 did.
# The AVX2 check catches the opposite slip: native off without the CFLAGS
# leaves a portable but SSE2-only ggml, several times slower.
set -euo pipefail

binary=$1
read -r avx512 ggml_avx2 < <(
    objdump -d --no-show-raw-insn "$binary" | awk '
        /^[0-9a-f]+ <.*>:$/ { in_ggml = ($2 ~ /ggml/) }
        /%zmm/ { avx512++ }
        in_ggml && /%ymm/ { ggml_avx2++ }
        END { print avx512 + 0, ggml_avx2 + 0 }'
)
echo "AVX-512 instructions: $avx512; AVX/AVX2 instructions in ggml: $ggml_avx2"

if [ "$avx512" -gt 0 ]; then
    echo "::error::$binary uses AVX-512, so it crashes on CPUs without it. Was it built without cpu-baseline-env.sh?"
    exit 1
fi
if [ "$ggml_avx2" -eq 0 ]; then
    echo "::error::ggml in $binary has no AVX2 code. Are CFLAGS from cpu-baseline-env.sh reaching the whisper.cpp build?"
    exit 1
fi
