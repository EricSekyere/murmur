# Local Qwen3-ASR trial

This developer experiment runs the official Qwen3-ASR 1.7B model independently
of Murmur. It adds no app dependency, cloud service, API key, or model selector.
Download first; evaluation verifies every pinned model file and forces offline
inference. Reports contain transcripts, so keep personal recordings and reports
in an ignored local directory.

Tested with Python 3.12.6, qwen-asr 0.0.6, torch 2.6.0+cu124,
transformers 4.57.6, accelerate 1.12.0, soundfile 0.14.0 and numpy 2.5.4.
Use an isolated environment, not Murmur's runtime:

```powershell
uv venv --python 3.12 target/qwen-trial/.venv
uv pip install --python target/qwen-trial/.venv/Scripts/python.exe torch==2.6.0 --index-url https://download.pytorch.org/whl/cu124
uv pip install --python target/qwen-trial/.venv/Scripts/python.exe qwen-asr==0.0.6 transformers==4.57.6 accelerate==1.12.0 soundfile==0.14.0 numpy==2.5.4
uv run --no-project --python target/qwen-trial/.venv/Scripts/python.exe scripts/qwen-asr/model_files.py target/qwen-trial/model
./scripts/transcription-fixtures.ps1 -OutputDirectory target/qwen-audio -PhraseManifest scripts/transcription-broad-phrases.json
uv run --no-project --python target/qwen-trial/.venv/Scripts/python.exe scripts/qwen-asr/evaluate.py --model target/qwen-trial/model --manifest target/qwen-audio/manifest.json --report target/qwen-trial/results.json --context 'Relevant terms: Claude, Fresha, Owneli, Supabase, TypeScript, npm'
```

Use `bin/python` instead of `Scripts/python.exe` on Unix. The CUDA wheel is for
compatible NVIDIA systems; choose a supported CPU torch wheel and `--cpu` for
CPU inference. Expect substantial memory use. The default 3500 MiB GPU budget
allows CPU offloading; override with `--gpu-memory` only if sufficient VRAM is
free. A trial is not a production latency benchmark when other work is running.

The manifest is the same JSON array of `id`, `file`, `expected` objects used by
Murmur's stress harness. Relative audio paths resolve beside the manifest. Each
original recording runs without context, and again with context when supplied.
Seven synthetic non-speech cases follow. Their seeded NumPy noise is different
from the Rust harness's noise. Output is saved after each completed observation;
an existing report is never overwritten at startup.

This runner measures **model output on complete audio**. It bypasses Murmur's
VAD, splitting, preparation, correction, and delivery. Compare only equal input
paths; do not rank its WER directly against Murmur's complete-pipeline scores.
Numbers and deliberate speech cleanup need separate inspection. The synthetic
corpus does not establish accuracy for a person's microphone or accent.

Run lightweight verification without loading a model:

```powershell
uv run --no-project --python target/qwen-trial/.venv/Scripts/python.exe -m unittest discover -s scripts/qwen-asr
```

Model checksums are pinned to official repository revision
`7278e1e70fe206f11671096ffdd38061171dd6e5`. Weight SHA256 values match its published
LFS metadata; other files were verified against that revision's Git blob IDs
before their SHA256 values were recorded. No remote Python code is loaded.
See [review evidence](../../docs/qwen3-asr-reviews.md).
