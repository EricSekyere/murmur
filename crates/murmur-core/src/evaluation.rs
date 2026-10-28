//! Explicitly saved local correction examples. Never called during dictation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid example ID")]
    InvalidId,
    #[error("Corrected text must contain 1-8000 characters")]
    InvalidText,
    #[error("Audio must be finite mono samples, at a valid rate, no longer than 90 seconds")]
    InvalidAudio,
    #[error("Example already exists")]
    Exists,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WAV error: {0}")]
    Wav(#[from] hound::Error),
    #[error("Example metadata error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub id: String,
    pub file: String,
    pub expected: String,
    pub recognized: String,
    pub model_at_save: String,
}

fn directory(root: &Path, id: &str) -> Result<PathBuf, Error> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidId);
    }
    Ok(root.join(id))
}

/// Save an entire pair before exposing it in the collection. No overwrites.
pub fn save(root: &Path, example: &Example, samples: &[f32], rate: u32) -> Result<PathBuf, Error> {
    let destination = directory(root, &example.id)?;
    if example.expected.trim().is_empty() || example.expected.chars().count() > 8000 {
        return Err(Error::InvalidText);
    }
    if !(8000..=192000).contains(&rate)
        || samples.is_empty()
        || samples.len() > rate as usize * 90
        || samples.iter().any(|s| !s.is_finite())
    {
        return Err(Error::InvalidAudio);
    }
    if destination.exists() {
        return Err(Error::Exists);
    }
    std::fs::create_dir_all(root)?;
    let pending = root.join(format!(".{}.pending", example.id));
    std::fs::create_dir(&pending)?;
    let result = write_pair(&pending, example, samples, rate)
        .and_then(|()| std::fs::rename(&pending, &destination).map_err(Error::from));
    if result.is_err()
        && let Err(error) = std::fs::remove_dir_all(&pending)
    {
        tracing::warn!(%error, "could not remove incomplete correction example");
    }
    result?;
    Ok(destination)
}

fn write_pair(dir: &Path, example: &Example, samples: &[f32], rate: u32) -> Result<(), Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.join("audio.wav"), spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    let mut record = example.clone();
    record.file = "audio.wav".into();
    let bytes = serde_json::to_vec_pretty(&[record])?;
    crate::fsutil::atomic_write(&dir.join("manifest.json"), &bytes)?;
    Ok(())
}

/// Enumerate only complete examples; unrelated files and pending saves are ignored.
pub fn list(root: &Path) -> Result<Vec<Example>, Error> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut examples = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if directory(root, &id).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        let records: Result<Vec<Example>, _> = std::fs::read(entry.path().join("manifest.json"))
            .map_err(Error::from)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(Error::from));
        if let Ok(records) = records {
            examples.extend(records.into_iter().filter(|record| record.id == id));
        }
    }
    examples.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(examples)
}

/// Delete one user-selected example; caller-supplied paths are never accepted.
pub fn delete(root: &Path, id: &str) -> Result<(), Error> {
    let path = directory(root, id)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::InvalidId);
    }
    if path.canonicalize()?.parent() != Some(root.canonicalize()?.as_path()) {
        return Err(Error::InvalidId);
    }
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Example {
        Example {
            id: "0123456789abcdef0123456789abcdef".into(),
            file: "ignored".into(),
            expected: "do not delete it".into(),
            recognized: "do delete it".into(),
            model_at_save: "test".into(),
        }
    }

    #[test]
    fn pair_round_trip_preserves_audio_and_correction_and_can_be_deleted() {
        let root = tempfile::tempdir().expect("tempdir");
        let samples = vec![0.1, -0.2, 0.0];
        let example = example();
        let dir = save(root.path(), &example, &samples, 16000).expect("save");
        let mut wav = hound::WavReader::open(dir.join("audio.wav")).expect("wav");
        assert_eq!(
            wav.samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .expect("samples"),
            samples
        );
        assert_eq!(
            list(root.path()).expect("list")[0].expected,
            example.expected
        );
        assert!(matches!(
            save(root.path(), &example, &samples, 16000),
            Err(Error::Exists)
        ));
        delete(root.path(), &example.id).expect("delete");
        assert!(list(root.path()).expect("list").is_empty());
    }

    #[test]
    fn invalid_requests_do_not_create_files_or_escape_the_collection() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut example = example();
        example.id = "../outside".into();
        assert!(matches!(
            save(root.path(), &example, &[0.1], 16000),
            Err(Error::InvalidId)
        ));
        assert!(matches!(delete(root.path(), ".."), Err(Error::InvalidId)));
        example.id = "a".repeat(32);
        assert!(matches!(
            save(root.path(), &example, &[f32::NAN], 16000),
            Err(Error::InvalidAudio)
        ));
        example.expected.clear();
        assert!(matches!(
            save(root.path(), &example, &[0.1], 16000),
            Err(Error::InvalidText)
        ));
        assert_eq!(
            std::fs::read_dir(root.path()).expect("directory").count(),
            0
        );
    }
}
