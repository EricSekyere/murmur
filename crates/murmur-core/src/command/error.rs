use thiserror::Error;

/// Errors from command-mode permission persistence.
#[derive(Debug, Error)]
pub enum CommandError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("TOML parse error: {0}")]
    Deserialize(#[from] toml::de::Error),

    #[error("could not determine config directory")]
    ConfigDirUnavailable,
}
