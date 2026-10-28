"""Offline, model-only Qwen evaluation; not a Murmur pipeline benchmark."""

import argparse
import json
import logging
import os
import re
import time
from pathlib import Path

from model_files import verify


def tokens(text):
    return re.findall(r"[^\W_]+", text.casefold())


def word_errors(expected, actual):
    reference, hypothesis = tokens(expected), tokens(actual)
    row = list(range(len(hypothesis) + 1))
    for i, word in enumerate(reference):
        next_row = [i + 1]
        for j, received in enumerate(hypothesis):
            next_row.append(min(row[j] + (word != received), row[j + 1] + 1,
                                next_row[j] + 1))
        row = next_row
    return row[-1]


def non_speech(np):
    rng = np.random.default_rng(42)
    yield "silence", np.zeros(48000, dtype="float32")
    for name, amplitude in [("quiet_noise", .002), ("noise", .03), ("loud_noise", .2)]:
        yield name, (rng.uniform(-1, 1, 48000) * amplitude).astype("float32")
    for name, hz in [("hum", 60), ("tone", 440)]:
        yield name, (.03 * np.sin(2 * np.pi * hz * np.arange(48000) / 16000)).astype("float32")
    clicks = np.zeros(48000, dtype="float32")
    clicks[::1600] = .8
    yield "clicks", clicks


def run(args):
    for key in ("HF_HUB_OFFLINE", "TRANSFORMERS_OFFLINE", "HF_HUB_DISABLE_TELEMETRY", "DO_NOT_TRACK"):
        os.environ[key] = "1"
    os.environ["GRADIO_ANALYTICS_ENABLED"] = "False"
    import numpy as np
    import soundfile as sf
    import torch
    from qwen_asr import Qwen3ASRModel

    pins = verify(args.model)
    torch.set_num_threads(args.threads)
    cuda = torch.cuda.is_available() and not args.cpu
    options = {"device_map": "auto", "max_memory": {0: args.gpu_memory, "cpu": "8GiB"}} if cuda else {"device_map": "cpu"}
    model = Qwen3ASRModel.from_pretrained(
        str(args.model), dtype=torch.bfloat16 if cuda else torch.float32,
        attn_implementation="sdpa", local_files_only=True,
        max_inference_batch_size=1, max_new_tokens=256, **options,
    )
    observations = []
    report = {"model": pins["repository"], "revision": pins["revision"],
              "pipeline": "model-only, complete original clips, no VAD or text correction",
              "device_map": {k: str(v) for k, v in model.model.hf_device_map.items()},
              "context": args.context, "observations": observations}
    contexts = [("no_glossary", "")]
    if args.context:
        contexts.append(("glossary", args.context))

    def evaluate(case, samples, rate):
        for label, context in contexts:
            start = time.monotonic()
            result = model.transcribe(audio=(samples, rate), context=context, language=args.language)
            elapsed = time.monotonic() - start
            text = result[0].text
            observations.append({**case, "condition": label, "raw_actual": text,
                                 "raw_word_errors": word_errors(case["expected"], text),
                                 "reference_words": len(tokens(case["expected"])),
                                 "seconds": elapsed, "audio_seconds": len(samples) / rate})
            temporary = args.report.with_suffix(".json.tmp")
            temporary.write_text(json.dumps(report, indent=2), encoding="utf-8")
            temporary.replace(args.report)
            logging.info("Completed observation %d (%s)", len(observations), label)

    cases = json.loads(args.manifest.read_text(encoding="utf-8-sig"))
    for case in cases[:args.limit]:
        samples, rate = sf.read(args.manifest.parent / case["file"], dtype="float32")
        evaluate(case, samples, rate)
    if not args.skip_non_speech:
        for name, samples in non_speech(np):
            evaluate({"id": name, "expected": ""}, samples, 16000)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--context", default="")
    parser.add_argument("--language", default="English")
    parser.add_argument("--gpu-memory", default="3500MiB")
    parser.add_argument("--cpu", action="store_true")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--limit", type=int, help="Limit speech clips for a smoke test")
    parser.add_argument("--skip-non-speech", action="store_true", help="Speech-only smoke test")
    args = parser.parse_args()
    if args.threads < 1 or (args.limit is not None and args.limit < 1) or args.report.exists():
        parser.error("threads and limit must be positive and report must be a new file")
    args.model = args.model.resolve()
    args.manifest = args.manifest.resolve()
    args.report = args.report.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    return args


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    run(arguments())
