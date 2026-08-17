"""Synthetic 'Hey Murmur' generation and noise/RIR augmentation."""

from __future__ import annotations

import wave
from pathlib import Path

import numpy as np

PHRASE = "Hey Murmur"
SAMPLE_RATE = 16_000
# LibriSpeech train-other-500 is ~500 h; never load that as float32.
DEFAULT_MAX_BACKGROUND_HOURS = 20.0
DEFAULT_MAX_NEGATIVE_WINDOWS = 50_000
AUDIO_GLOBS = ("*.wav", "*.flac")


def write_wav(path: Path, samples: np.ndarray, sample_rate: int = SAMPLE_RATE) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    clipped = np.clip(samples, -1.0, 1.0)
    pcm = (clipped * 32767.0).astype(np.int16)
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(pcm.tobytes())


def read_wav(path: Path) -> tuple[np.ndarray, int]:
    """Decode WAV or FLAC (OpenSLR LibriSpeech) to mono float32."""
    return read_audio(path)


def read_audio(path: Path) -> tuple[np.ndarray, int]:
    suffix = path.suffix.lower()
    if suffix == ".wav":
        try:
            return _read_pcm_wav(path)
        except (wave.Error, ValueError):
            return _read_soundfile(path)
    return _read_soundfile(path)


def _read_pcm_wav(path: Path) -> tuple[np.ndarray, int]:
    with wave.open(str(path), "rb") as wav:
        rate = wav.getframerate()
        n = wav.getnchannels()
        width = wav.getsampwidth()
        raw = wav.readframes(wav.getnframes())
    if width != 2:
        raise ValueError(f"{path}: unsupported sample width {width}")
    data = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    if n > 1:
        data = data.reshape(-1, n).mean(axis=1)
    return data, rate


def _read_soundfile(path: Path) -> tuple[np.ndarray, int]:
    import soundfile as sf

    data, rate = sf.read(str(path), dtype="float32", always_2d=False)
    if getattr(data, "ndim", 1) > 1:
        data = np.mean(data, axis=1)
    return np.asarray(data, dtype=np.float32), int(rate)


def iter_audio_files(root: Path):
    if not root.is_dir():
        return
    found: set[Path] = set()
    for pattern in AUDIO_GLOBS:
        found.update(root.rglob(pattern))
    yield from sorted(found)


def iter_clips_capped(root: Path, *, max_hours: float):
    """Yield (audio_16k, path) until `max_hours` of audio has been read.

    Stops after the clip that reaches the cap (at most one-file overshoot).
    Does not accumulate the rest of the tree.
    """
    hours = 0.0
    for path in iter_audio_files(root):
        if hours >= max_hours:
            return
        samples, rate = read_audio(path)
        audio = resample_16k(samples, rate)
        hours += len(audio) / SAMPLE_RATE / 3600.0
        yield audio, path


def resample_16k(samples: np.ndarray, rate: int) -> np.ndarray:
    if rate == SAMPLE_RATE:
        return samples.astype(np.float32, copy=False)
    import math

    n_out = int(math.floor(len(samples) * SAMPLE_RATE / rate))
    if n_out <= 0:
        return np.zeros(0, dtype=np.float32)
    x_old = np.linspace(0.0, 1.0, num=len(samples), endpoint=False)
    x_new = np.linspace(0.0, 1.0, num=n_out, endpoint=False)
    return np.interp(x_new, x_old, samples).astype(np.float32)


def mix_noise(speech: np.ndarray, noise: np.ndarray, snr_db: float) -> np.ndarray:
    if len(noise) == 0:
        return speech
    if len(noise) < len(speech):
        reps = int(np.ceil(len(speech) / len(noise)))
        noise = np.tile(noise, reps)
    start = 0 if len(noise) == len(speech) else int(
        np.random.randint(0, len(noise) - len(speech) + 1)
    )
    noise = noise[start : start + len(speech)]
    p_sig = float(np.mean(speech**2) + 1e-9)
    p_noise = float(np.mean(noise**2) + 1e-9)
    scale = np.sqrt(p_sig / (p_noise * (10.0 ** (snr_db / 10.0))))
    return (speech + noise * scale).astype(np.float32)


def apply_rir(speech: np.ndarray, rir: np.ndarray) -> np.ndarray:
    if len(rir) == 0:
        return speech
    wet = np.convolve(speech, rir, mode="full")[: len(speech)]
    peak = float(np.max(np.abs(wet)) + 1e-9)
    return (wet / peak).astype(np.float32)


def augment_clip(
    speech: np.ndarray,
    *,
    noise: np.ndarray | None = None,
    rir: np.ndarray | None = None,
    snr_db: float = 10.0,
) -> np.ndarray:
    out = speech.astype(np.float32, copy=True)
    if rir is not None and len(rir) > 0:
        out = apply_rir(out, rir)
    if noise is not None and len(noise) > 0:
        out = mix_noise(out, noise, snr_db)
    peak = float(np.max(np.abs(out)) + 1e-9)
    return (out / max(peak, 1.0)).astype(np.float32)


def pad_for_window(
    speech: np.ndarray,
    target_len: int,
    *,
    filler: np.ndarray | None = None,
    tail_samples: int = SAMPLE_RATE // 5,
) -> np.ndarray:
    """Seat a short phrase in a window long enough to score.

    Piper returns just the utterance, and "Hey Murmur" is well under the
    audio one head window needs, so an unpadded clip produces no training
    windows at all. Most of the padding leads and a short tail follows,
    matching inference: the head scores the most recent embeddings, so the
    phrase completes near the end of the window. Padding uses background
    audio when there is any, so the model does not learn that the phrase is
    whatever follows silence.
    """
    speech = speech.astype(np.float32, copy=False)
    if len(speech) >= target_len:
        return speech
    pad_total = target_len - len(speech)
    # A fixed tail, not a fraction: the phrase must finish just before the
    # window ends however long the window is, so the head sees it where it
    # will see it at inference.
    tail = min(tail_samples, pad_total)
    head = pad_total - tail
    out = np.zeros(target_len, dtype=np.float32)
    if filler is not None and len(filler) > 0:
        reps = int(np.ceil(target_len / len(filler)))
        out += np.tile(filler, reps)[:target_len].astype(np.float32) * 0.05
    out[head : head + len(speech)] += speech
    peak = float(np.max(np.abs(out)) + 1e-9)
    return (out / max(peak, 1.0)).astype(np.float32)


def synthesis_variation(index: int, num_speakers: int) -> dict:
    """Deterministic per-clip synthesis settings.

    Piper is deterministic, so repeating one call yields byte-identical audio:
    the first trained head saw 10 unique waveforms behind 4000 files and
    memorised two of them (recall stuck at 0.5 across every threshold). Vary
    the speaker where a voice has more than one, and jitter prosody either
    way, so clips differ as renditions rather than as copies. Seeded by index
    so a rerun reproduces the same corpus.
    """
    rng = np.random.default_rng(index)
    return {
        "speaker_id": int(index % num_speakers) if num_speakers > 1 else None,
        "length_scale": float(rng.uniform(0.85, 1.25)),
        "noise_scale": float(rng.uniform(0.55, 0.85)),
        "noise_w_scale": float(rng.uniform(0.6, 1.0)),
    }


def synthesize_with_piper(
    voice_onnx: Path,
    phrase: str,
    dest: Path,
    *,
    variation: dict | None = None,
) -> None:
    """Generate one clip with an allowlisted Piper voice. Requires piper-tts."""
    from piper import PiperVoice

    voice = PiperVoice.load(str(voice_onnx))
    dest.parent.mkdir(parents=True, exist_ok=True)
    rate = getattr(getattr(voice, "config", None), "sample_rate", SAMPLE_RATE)
    syn_config = None
    if variation:
        from piper import SynthesisConfig

        syn_config = SynthesisConfig(**variation)
    with wave.open(str(dest), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        if syn_config is not None:
            voice.synthesize_wav(phrase, wav, syn_config=syn_config)
        elif hasattr(voice, "synthesize_wav"):
            voice.synthesize_wav(phrase, wav)
        else:
            voice.synthesize(phrase, wav)
