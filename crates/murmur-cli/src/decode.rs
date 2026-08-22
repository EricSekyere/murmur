//! Decoding audio files to raw samples for `transcribe`.
//!
//! WAV stays on `hound`, which has handled it here for a long time and is
//! already proven against odd bit depths. Everything else goes through
//! Symphonia, which is pure Rust, so the command gains mp3, m4a, mp4 and the
//! rest without depending on an ffmpeg binary the user has to install.

use anyhow::{Context, Result};
use std::path::Path;

/// Interleaved samples in [-1, 1], with the source's own rate and channel
/// count. Callers resample and downmix through `AudioBuffer::from_raw`.
pub(crate) struct Decoded {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u16,
}

impl std::fmt::Debug for Decoded {
    /// Shape only. A failure message carrying a million samples is unreadable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoded")
            .field("samples", &self.samples.len())
            .field("rate", &self.rate)
            .field("channels", &self.channels)
            .finish()
    }
}

/// Decode any supported audio file.
///
/// The extension only picks the decoder; Symphonia probes the actual content,
/// so a misnamed file still decodes when its real format is supported.
pub(crate) fn decode(path: &Path) -> Result<Decoded> {
    let is_wav = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if is_wav {
        return read_wav(path);
    }
    let mut decoded = read_with_symphonia(path)?;
    quantize_to_16_bit(&mut decoded.samples);
    Ok(decoded)
}

/// Round samples to the 16-bit PCM grid.
///
/// A compressed file decodes to full-precision float, while a WAV, which is
/// almost always 16-bit PCM, arrives already on this grid. That difference is
/// tiny, around 1.5e-5, and it is not tiny in its effect: decoding one mp3 and
/// feeding Parakeet the raw float returned an empty transcript, while the same
/// samples rounded here returned the full 140 characters. Putting every input
/// path in the same numeric domain removes a failure that depends on nothing
/// but the container the audio arrived in.
fn quantize_to_16_bit(samples: &mut [f32]) {
    const SCALE: f32 = 32768.0;
    for s in samples {
        *s = (*s * SCALE).round().clamp(-SCALE, SCALE - 1.0) / SCALE;
    }
}

/// Decode a WAV file, handling both float and integer PCM.
fn read_wav(path: &Path) -> Result<Decoded> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open WAV file {}", path.display()))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<Vec<f32>, _>>()
            .context("Failed to read float WAV samples")?,
        hound::SampleFormat::Int => {
            // Full-scale magnitude for the sample width, so any bit depth
            // (16/24/32-bit PCM) normalizes to [-1, 1].
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<Vec<f32>, _>>()
                .context("Failed to read integer WAV samples")?
        }
    };
    Ok(Decoded {
        samples,
        rate: spec.sample_rate,
        channels: spec.channels,
    })
}

/// Decode a non-WAV container by probing its content.
fn read_with_symphonia(path: &Path) -> Result<Decoded> {
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open audio file {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    // The extension is a hint only; probing decides. A file named .mp3 that
    // is really m4a still decodes.
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| {
            format!(
                "Unsupported or unreadable audio file {}: no decoder matched its contents",
                path.display()
            )
        })?;

    // Copy what the loop needs before borrowing `format` mutably. A video
    // container carries several tracks, so take the first audio one.
    let (track_id, params) = format
        .tracks()
        .iter()
        .find_map(|t| match &t.codec_params {
            Some(CodecParameters::Audio(a)) => Some((t.id, a.clone())),
            _ => None,
        })
        .context("File contains no decodable audio track")?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .context("No decoder available for this audio codec")?;

    // Rate and channel count are only certain once a packet has been decoded,
    // since some containers omit them from the header.
    let mut rate = params.sample_rate.unwrap_or(0);
    let mut channels = 0u16;

    let mut samples: Vec<f32> = Vec::new();
    let mut chunk: Vec<f32> = Vec::new();
    while let Some(packet) = format
        .next_packet()
        .context("Failed to read an audio packet")?
    {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                let spec = audio.spec();
                rate = spec.rate();
                channels = spec.channels().count() as u16;
                audio.copy_to_vec_interleaved(&mut chunk);
                samples.append(&mut chunk);
            }
            // A corrupt packet mid-file should not discard everything already
            // decoded; skip it and keep going.
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                tracing::warn!(error = %e, "skipping undecodable audio packet");
            }
            Err(e) => return Err(anyhow::Error::new(e).context("Failed to decode audio")),
        }
    }

    if samples.is_empty() {
        anyhow::bail!("Decoded no audio from {}", path.display());
    }
    if rate == 0 || channels == 0 {
        anyhow::bail!(
            "Audio file {} did not report a usable sample rate or channel count",
            path.display()
        );
    }
    Ok(Decoded {
        samples,
        rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_fixture(dir: &Path, name: &str, rate: u32, channels: u16) -> std::path::PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).expect("writer");
        for i in 0..(rate as usize * channels as usize) {
            let v = ((i as f32 * 0.01).sin() * 8000.0) as i16;
            w.write_sample(v).expect("sample");
        }
        w.finalize().expect("finalize");
        path
    }

    #[test]
    fn wav_still_decodes_through_hound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = wav_fixture(dir.path(), "a.wav", 16_000, 1);
        let d = decode(&path).expect("decode");
        assert_eq!(d.rate, 16_000);
        assert_eq!(d.channels, 1);
        assert_eq!(d.samples.len(), 16_000);
        assert!(d.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn wav_extension_matching_ignores_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = wav_fixture(dir.path(), "b.WAV", 8_000, 2);
        let d = decode(&path).expect("decode");
        assert_eq!(d.rate, 8_000);
        assert_eq!(d.channels, 2);
    }

    #[test]
    fn quantization_snaps_samples_onto_the_16_bit_grid() {
        // The regression this guards: full-precision float decoded from a
        // compressed file made Parakeet return an empty transcript for audio
        // it transcribed correctly once rounded.
        let mut v = vec![0.001_128_525_f32, -0.5, 0.0, 0.28199];
        quantize_to_16_bit(&mut v);
        for s in &v {
            let grid = s * 32768.0;
            assert!(
                (grid - grid.round()).abs() < 1e-3,
                "{s} is not on the 16-bit grid"
            );
        }
    }

    #[test]
    fn quantization_is_idempotent_and_stays_in_range() {
        let mut v = vec![0.7_f32, -0.7, 1.0, -1.0, 0.0];
        quantize_to_16_bit(&mut v);
        let once = v.clone();
        quantize_to_16_bit(&mut v);
        assert_eq!(v, once, "second pass moved the samples");
        assert!(
            v.iter().all(|s| (-1.0..=1.0).contains(s)),
            "out of range: {v:?}"
        );
    }

    #[test]
    fn quantization_leaves_wav_style_samples_untouched() {
        // 16-bit WAV already arrives on the grid, so this must be a no-op
        // there and cannot degrade the path that already worked.
        let mut v: Vec<f32> = (-3..=3).map(|i| i as f32 * 1024.0 / 32768.0).collect();
        let before = v.clone();
        quantize_to_16_bit(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn an_unsupported_file_fails_with_a_readable_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"this is not audio").expect("write");
        let err = decode(&path).expect_err("must not decode");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Unsupported or unreadable"),
            "unhelpful error: {msg}"
        );
    }

    #[test]
    fn a_missing_file_reports_the_path() {
        let err = decode(Path::new("no/such/file.mp3")).expect_err("must fail");
        assert!(format!("{err:#}").contains("no/such/file.mp3"));
    }

    #[test]
    fn an_empty_file_does_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.mp3");
        std::fs::write(&path, b"").expect("write");
        assert!(decode(&path).is_err());
    }
}
