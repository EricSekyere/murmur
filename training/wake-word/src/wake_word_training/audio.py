"""Synthetic 'Hey Murmur' generation and noise/RIR augmentation."""

from __future__ import annotations

import wave
from pathlib import Path

import numpy as np

PHRASE = "Hey Murmur"
SAMPLE_RATE = 16_000
# Per-clip training gain range, identical for both classes and log-uniform so
# every level in the range is equally likely. Natural class levels sit 13 dB
# apart (positives RMS 0.1220, negatives 0.0268) and a multiplicative gain
# preserves that gap, so the spread must be far wider than the gap for the
# class level distributions to genuinely overlap; tests pin this margin.
GAIN_DB_MIN = -24.0
GAIN_DB_MAX = 6.0
# LibriSpeech train-other-500 is ~500 h; never load that as float32.
DEFAULT_MAX_BACKGROUND_HOURS = 20.0
DEFAULT_MAX_NEGATIVE_WINDOWS = 200_000
DEFAULT_MAX_VALIDATION_WINDOWS = 20_000
# Augmentation pools are sampled, not loaded whole. Measured on the shipped
# corpora: RIRS_NOISES holds 60325 impulse responses (5.9 GB as float32) and
# MUSAN 2016 recordings averaging 237 s (30.6 GB), against 39.6 GB of RAM,
# and a run only ever indexes one entry per clip. Trimming bounds the tail of
# long MUSAN music tracks, which a 3.2 s window never reaches anyway.
DEFAULT_MAX_RIR_CLIPS = 3_000
DEFAULT_MAX_RIR_SECONDS = 4.0
DEFAULT_MAX_NOISE_CLIPS = 1_200
DEFAULT_MAX_NOISE_SECONDS = 15.0
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
    yield from iter_clips_from(iter_audio_files(root), max_hours=max_hours)


def iter_clips_from(paths, *, max_hours: float):
    """`iter_clips_capped` over an explicit, already-ordered file list.

    Training reads a speaker-interleaved order (see `background.py`), so the
    caller supplies the sequence rather than letting the cap take a prefix of
    the sorted tree.
    """
    hours = 0.0
    for path in paths:
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
    try:
        # Direct convolution is quadratic and dominates the training wall
        # clock; scipy ships with the train extra but not the base install.
        from scipy.signal import fftconvolve

        wet = fftconvolve(speech, rir, mode="full")[: len(speech)]
    except ImportError:
        wet = np.convolve(speech, rir, mode="full")[: len(speech)]
    # Reverb must not re-level the clip. Peak-normalising here pushed every
    # reverberant clip to full scale, a normalisation inference never applies,
    # and the loudness shortcut the head kept learning started as exactly
    # that. Matching the dry RMS keeps the room and discards the gain.
    dry = float(np.sqrt(np.mean(speech**2)))
    wet_rms = float(np.sqrt(np.mean(wet**2)))
    if wet_rms < 1e-9:
        return wet.astype(np.float32)
    return (wet * (dry / wet_rms)).astype(np.float32)


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


def random_gain(audio: np.ndarray, index: int) -> np.ndarray:
    """Scale a clip by a random gain, seeded per clip for reproducible reruns.

    A gain around the clip's natural level, never a level target: levelling
    every training clip to one RMS taught the head a loudness band that raw
    evaluation and the raw Rust microphone feed never reproduce, and false
    accepts went from 6.6/hour to 493/hour. Level invariance comes from
    seeing each clip at many levels, not from a normalisation inference
    cannot repeat.
    """
    rng = np.random.default_rng(index + 1_000_003)
    gain = 10.0 ** (float(rng.uniform(GAIN_DB_MIN, GAIN_DB_MAX)) / 20.0)
    out = audio.astype(np.float32) * np.float32(gain)
    # Clip like the ADC would. Rescaling loud clips to fit would pile them
    # onto one level, which is normalisation sneaking back in.
    return np.clip(out, -1.0, 1.0)


def serve_window(
    audio: np.ndarray,
    target_len: int,
    *,
    filler: np.ndarray | None = None,
) -> np.ndarray:
    """Audio exactly as the deployed scorer receives it: natural level,
    seated in a scorable window.

    This is the single serve-side pipeline. The Rust armed loop
    (murmur-core `audio/wake_onnx.rs`) streams raw microphone samples into
    the same backbone, so any transform added here must be mirrored there
    before it is real. Training must wrap this function (`train_window`)
    rather than composing its own steps; tests/test_preprocessing_parity.py
    fails when the two chains diverge.
    """
    return pad_for_window(audio, target_len, filler=filler)


def train_window(
    audio: np.ndarray,
    target_len: int,
    *,
    index: int,
    noise: np.ndarray | None = None,
    rir: np.ndarray | None = None,
) -> np.ndarray:
    """The serve pipeline plus randomised augmentation, for either class.

    Augmentation may only spread the distribution the model sees (rooms,
    noise, levels), never move it somewhere inference cannot follow. That is
    why there is no normalisation step, and why positives and negatives both
    go through this one function.
    """
    # Some clips skip the room and some skip the noise bed, so the dry quiet
    # case the evaluator and a close microphone produce sits inside the
    # training distribution rather than being an edge the head never saw.
    if index % 4 == 3:
        rir = None
    if index % 3 == 2:
        noise = None
    out = serve_window(audio, target_len, filler=noise)
    out = augment_clip(out, noise=noise, rir=rir, snr_db=5.0 + (index % 16))
    return random_gain(out, index)


def synthesis_variation(index: int, speaker_id: int | None) -> dict:
    """Deterministic per-clip synthesis settings for one chosen speaker.

    Piper is deterministic, so repeating one call yields byte-identical audio:
    the first trained head saw 10 unique waveforms behind 4000 files and
    memorised two of them (recall stuck at 0.5 across every threshold). The
    speaker is chosen by the caller, because which people may be rendered into
    which split is the split's decision, not this function's. Prosody is
    jittered on top so clips differ as renditions rather than as copies,
    seeded by index so a rerun reproduces the same corpus.
    """
    rng = np.random.default_rng(index)
    return {
        "speaker_id": None if speaker_id is None else int(speaker_id),
        "length_scale": float(rng.uniform(0.85, 1.25)),
        "noise_scale": float(rng.uniform(0.55, 0.85)),
        "noise_w_scale": float(rng.uniform(0.6, 1.0)),
    }


_LOADED_VOICE: tuple[str, object] | None = None


def _piper_voice(voice_onnx: Path):
    """The most recently used Piper voice, reloaded only when it changes.

    Loading the ONNX voice per clip cost 1.51 s against 0.25 s once loaded, a
    6x tax on a corpus of thousands of clips. Synthesis walks one voice at a
    time, so holding a single voice is enough and keeps peak memory to one
    model.
    """
    from piper import PiperVoice

    global _LOADED_VOICE
    key = str(voice_onnx)
    if _LOADED_VOICE is None or _LOADED_VOICE[0] != key:
        _LOADED_VOICE = (key, PiperVoice.load(key))
    return _LOADED_VOICE[1]


def synthesize_with_piper(
    voice_onnx: Path,
    phrase: str,
    dest: Path,
    *,
    variation: dict | None = None,
) -> None:
    """Generate one clip with an allowlisted Piper voice. Requires piper-tts."""
    voice = _piper_voice(voice_onnx)
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
