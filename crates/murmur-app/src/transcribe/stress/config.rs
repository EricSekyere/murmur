use super::*;

const CONDITIONS: &[&str] = &[
    "clean",
    "quiet",
    "amplitude_5_percent",
    "amplitude_2_percent",
    "amplitude_1_percent",
    "noise",
    "padded",
];

pub(super) fn vocabulary() -> Result<Vec<String>> {
    match std::env::var("MURMUR_STRESS_VOCABULARY") {
        Ok(value) => serde_json::from_str(&value)
            .context("MURMUR_STRESS_VOCABULARY must be a JSON array of strings"),
        Err(std::env::VarError::NotPresent) => Ok(vec!["Claude".into(), "Fresha".into()]),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn conditions() -> Result<Vec<String>> {
    let selected: Vec<String> = match std::env::var("MURMUR_STRESS_CONDITIONS") {
        Ok(value) => value.split(',').map(|s| s.trim().to_string()).collect(),
        Err(std::env::VarError::NotPresent) => CONDITIONS.iter().map(|s| s.to_string()).collect(),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        selected.iter().all(|s| CONDITIONS.contains(&s.as_str())),
        "MURMUR_STRESS_CONDITIONS contains an unsupported condition"
    );
    Ok(selected)
}

pub(super) fn model(name: &str) -> Result<SttModel> {
    match name {
        "parakeet-v2" => Ok(SttModel::ParakeetTdt06bV2),
        "parakeet-v3" => Ok(SttModel::ParakeetTdt06bV3),
        "whisper-base" => Ok(SttModel::WhisperBaseEn),
        "whisper-small" => Ok(SttModel::WhisperSmallEn),
        "whisper-medium" => Ok(SttModel::WhisperMediumEn),
        "whisper-turbo" => Ok(SttModel::WhisperLargeV3Turbo),
        _ => anyhow::bail!("unsupported stress backend: {name}"),
    }
}
