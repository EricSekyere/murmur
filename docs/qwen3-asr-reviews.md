# Qwen3 ASR review evidence and local trial

Reviewed October 28, 2026. Qwen3-ASR 1.7B is a credible local transcription
candidate. Independent results justify a trial, but do not establish that it
will beat Murmur's existing models on everyday English dictation. This is the
speech-recognition model, not the general Qwen3 chat model or Qwen3-TTS.

| Review or evaluation | What it found | How much to trust the comparison |
| --- | --- | --- |
| [OpenCaptions measurements](https://docs.opencaptions.app/project/speech-model-benchmarks/) | On English FLEURS, Qwen 1.7B scored 3.0% WER against Whisper Turbo's 5.2%. It also found decoding loops on difficult audio and material differences between original and quantized exports. | Independent implementation with reproduction scripts, but only 20 clips per language. CPU/runtime differences matter; its performance numbers do not predict this laptop's GPU latency. |
| [Neosophie Japanese IT speech benchmark](https://neosophie.com/en/blog/20260414-it-asr-benchmark) | Qwen had the lowest character error rate on 15 roughly 30-second clips from different speakers. The author still preferred Whisper for overall balance and identified semantic-error risks. | Useful real-speech comparison, but Japanese technical speech is a different task from general English dictation. |
| [Hands-on LocalLLaMA discussion](https://www.reddit.com/r/LocalLLaMA/comments/1rq118c/qwen3_asr_seems_to_outperform_whisper_in_almost/) | The tester reported strong Korean accuracy and good speed. Other users described missing English TV dialogue and hallucinations on unclear audio. The original tester also reported problems during extended vLLM streaming. | First-person experience and linked implementation, not a controlled shared benchmark. Different users, audio, GPUs, and runtimes explain some disagreement. |
| [Qwen's model card](https://huggingface.co/Qwen/Qwen3-ASR-1.7B) | Apache 2.0 weights, 30 languages plus 22 Chinese dialects, offline and streaming support. | Primary source for capabilities and licensing; vendor benchmark claims are not independent reviews. |

The evidence supports describing Qwen as promising for accuracy, with integration
and difficult-audio caveats. It does not support calling it universally best.
For Murmur, preserving negations and whole phrases matters more than making
the output look fluent.

## Completed local trial

The original 1.7B weights ran offline on this RTX 4050 laptop, with verified
checksums and a pinned revision. The experiment uses the official Transformers
runtime in an isolated Python environment. It does not install a new Murmur
backend or change the selected model. See the [reproduction instructions](../scripts/qwen-asr/README.md).

Sixty original synthetic recordings ran twice, with and without context,
followed by seven non-speech signals in both conditions: 134 observations.
Scoring follows the baseline's punctuation/case normalization and excludes
number-formatting and disfluency cases. This leaves 50 clips and 734 words.

| Original complete audio | Word errors | WER | General-only errors (688 words) |
| --- | ---: | ---: | ---: |
| Qwen, no glossary | 7 | 0.95% | 5 |
| Qwen, relevant glossary | 5 | 0.68% | 5 |

The glossary repaired Owneli in two clips; the general-speech score was
unchanged. One voice's “Use async await” became “You say sync a wait” with and
without hints. The original email clips were complete in both voices.
These synthetic results do not establish personal microphone or accent accuracy.

**All seven direct non-speech inputs produced unwanted text in both conditions.**
Without context, silence became an invented sentence and noise often became
“Okay” or “Oh.” With context, every non-speech output repeated the glossary.
This makes VAD and acceptance checks a prerequisite for integration.
These results bypass Murmur's VAD, preparation, correction and filtering;
Murmur's zero-output noise results used those checks, so this is not an equal
comparison of the underlying models' resistance to hallucination.

The GPU memory budget was 3500 MiB to leave room for the running app; some
weights were offloaded to CPU. Average speech inference was 13.3 seconds without
context and 14.0 with context, approximately 2.45 and 2.58 times audio duration.
Concurrent native builds and CPU offloading make this unsuitable for ranking
production latency. It does show that this configuration was too slow for
responsive dictation. A smaller model or quantized runtime needs a separate
accuracy and speed evaluation; neither is assumed equivalent to these weights.

**Recommendation:** retain Qwen as an experimental candidate. Do not replace
Murmur's default model yet. Test a real held-out microphone set through the same
complete pipeline, with bounded context and non-speech rejection, then measure
latency in isolation. The [recorded observations](evaluations/qwen3-asr-2026-10-28.json)
contain only the synthetic corpus.

GPT-Transcribe is excluded from this implementation scope following the decision
to keep speech processing local. Its API requires cloud processing and paid
usage; an API key does not make it an offline model.

Pipeline fixes remain independently reviewable: conservative
dictionary correction, preview vocabulary consistency, short-reply filtering,
and explicit local collection of corrected examples. A Qwen experiment does not
replace those fixes or justify changing the default model before evaluation.

The untrimmed-audio experiment was also tested through the full Parakeet v3
pipeline on 427 cases. It did not repair the email omission there and increased
clean raw errors from 10 to 11, and padded-audio errors from 5 to 9. The change
is not included in the fixes. This is evidence to investigate phrase boundaries
with real recordings, not justification to remove audio admission checks.
