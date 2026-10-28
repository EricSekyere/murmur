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

The local trial should use the original 1.7B weights first, pin the model
revision, verify downloaded weight hashes, and run inference offline. Keep the
runtime outside Murmur's default installation while measuring correctness,
memory, and latency on the RTX 4050 laptop. Test the same recordings with and
without a small relevant glossary, including ordinary words that could be
incorrectly attracted to glossary terms. Count omissions and false output on
non-speech separately from word error rate. Compare any smaller or quantized
variant independently rather than assuming it retains the original accuracy.

GPT-Transcribe is excluded from this implementation scope following the decision
to keep speech processing local. Its API requires cloud processing and paid
usage; an API key does not make it an offline model.

Pipeline fixes remain independently reviewable: audio preparation, conservative
dictionary correction, preview vocabulary consistency, short-reply filtering,
and explicit local collection of corrected examples. A Qwen experiment does not
replace those fixes or justify changing the default model before evaluation.
