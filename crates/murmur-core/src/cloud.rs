//! Opt-in BYO-key cloud rewrite backend (roadmap feature 10, command-mode
//! phase 5). Strictly off by default, three gates deep: the `cloud` cargo
//! feature is off, [`CloudConfig::enabled`] defaults to false, and even with
//! both on, no request leaves the device unless the user has exported
//! [`CLOUD_API_KEY_ENV`]. Every gate is checked before any HTTP client is
//! constructed.
//!
//! [`CloudConfig`] is plain data and stays unconditional so config files
//! round-trip without the feature, mirroring how `llm::RewriteMode` loads
//! without llama.cpp. All network code sits behind `#[cfg(feature = "cloud")]`.
//!
//! Security invariants:
//! - The API key is read from the environment at call time only. It is never
//!   stored in the config file and never logged; a platform keyring is the
//!   intended future home.
//! - The transcript is never logged above `trace` (and only its length there).
//! - The app layer must show a visible "speech leaving device" indicator
//!   whenever a cloud rewrite is active; that UI is not implemented in core.
//!
//! [`cloud_rewrite`] takes the same (system, user) pair the local
//! `llm::rewrite` path generates with, so it can slot into the existing
//! rewrite flow as an alternative backend.

use serde::{Deserialize, Serialize};

/// Environment variable holding the user's API key. Read at call time only,
/// never persisted (a platform keyring is the future home) and never logged.
pub const CLOUD_API_KEY_ENV: &str = "MURMUR_CLOUD_API_KEY";

/// BYO-key cloud rewrite settings for an OpenAI-compatible chat-completions
/// endpoint. Deliberately has no API key field: the key comes from
/// [`CLOUD_API_KEY_ENV`] so it can never end up in the plaintext config.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Master opt-in switch. False (the default) means no network call, ever.
    #[serde(default)]
    pub enabled: bool,
    /// Endpoint base, e.g. "https://api.openai.com/v1"; requests go to
    /// `{base_url}/chat/completions`. Empty (the default) is not configured.
    #[serde(default)]
    pub base_url: String,
    /// Model name the endpoint expects, e.g. "gpt-4o-mini".
    #[serde(default)]
    pub model: String,
}

#[cfg(feature = "cloud")]
const REQUEST_TIMEOUT_SECS: u64 = 60;

/// Errors from the cloud rewrite path. The skip variants (`Disabled`,
/// `MissingApiKey`, `NotConfigured`) are returned before any client exists.
#[cfg(feature = "cloud")]
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("cloud rewrite is disabled in the config")]
    Disabled,
    #[error("no API key: set the {CLOUD_API_KEY_ENV} environment variable")]
    MissingApiKey,
    #[error("cloud endpoint not configured: base_url and model must be set")]
    NotConfigured,
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("cloud API returned HTTP {status}")]
    Api { status: u16 },
    #[error("malformed cloud response: {0}")]
    MalformedResponse(&'static str),
}

/// Opt-in guard, evaluated before any HTTP client is constructed. Failing
/// here is what makes "off by default means no network call" provable:
/// [`cloud_rewrite`] only builds a client after this returns Ok.
#[cfg(feature = "cloud")]
fn preflight(cfg: &CloudConfig, api_key: Option<&str>) -> Result<String, CloudError> {
    if !cfg.enabled {
        return Err(CloudError::Disabled);
    }
    let key = api_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or(CloudError::MissingApiKey)?;
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err(CloudError::NotConfigured);
    }
    Ok(key.to_string())
}

/// OpenAI chat-completions request body. Pure so the request shape is
/// unit-testable without sending anything.
#[cfg(feature = "cloud")]
pub fn build_request_body(model: &str, system: &str, user: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    })
}

/// Pull the assistant text out of a chat-completions response.
#[cfg(feature = "cloud")]
fn extract_content(response: &serde_json::Value) -> Result<String, CloudError> {
    response["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or(CloudError::MalformedResponse(
            "missing choices[0].message.content",
        ))
}

/// Rewrite `user` with the configured cloud model, instructed by `system`.
///
/// Skips without touching the network (no client is built) unless the config
/// opt-in is on, [`CLOUD_API_KEY_ENV`] is set, and the endpoint is configured.
/// The key and transcript are never logged.
#[cfg(feature = "cloud")]
pub async fn cloud_rewrite(
    cfg: &CloudConfig,
    system: &str,
    user: &str,
) -> Result<String, CloudError> {
    let env_key = std::env::var(CLOUD_API_KEY_ENV).ok();
    let key = preflight(cfg, env_key.as_deref())?;

    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let body = build_request_body(&cfg.model, system, user);
    tracing::debug!(%url, model = %cfg.model, "sending cloud rewrite request");
    tracing::trace!(user_chars = user.len(), "cloud rewrite payload size");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;
    let response = client
        .post(&url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(status = status.as_u16(), "cloud rewrite request failed");
        return Err(CloudError::Api {
            status: status.as_u16(),
        });
    }
    let value: serde_json::Value = response.json().await?;
    extract_content(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_config_defaults_to_disabled_and_unconfigured() {
        let cfg = CloudConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.base_url.is_empty());
        assert!(cfg.model.is_empty());
    }

    #[test]
    fn cloud_config_round_trips_through_toml_without_a_key_field() {
        let cfg = CloudConfig {
            enabled: true,
            base_url: "https://api.example.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        let text = toml::to_string(&cfg).unwrap();
        assert!(!text.contains("key"), "config must never carry a key field");
        let reloaded: CloudConfig = toml::from_str(&text).unwrap();
        assert_eq!(reloaded, cfg);
    }

    #[test]
    fn empty_toml_table_deserializes_to_disabled() {
        let cfg: CloudConfig = toml::from_str("").unwrap();
        assert!(!cfg.enabled);
    }

    #[cfg(feature = "cloud")]
    fn enabled_cfg() -> CloudConfig {
        CloudConfig {
            enabled: true,
            base_url: "https://api.example.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
        }
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn preflight_rejects_disabled_config_even_with_a_key() {
        let cfg = CloudConfig {
            enabled: false,
            ..enabled_cfg()
        };
        assert!(matches!(
            preflight(&cfg, Some("sk-test")),
            Err(CloudError::Disabled)
        ));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn preflight_rejects_missing_or_blank_key() {
        let cfg = enabled_cfg();
        assert!(matches!(
            preflight(&cfg, None),
            Err(CloudError::MissingApiKey)
        ));
        assert!(matches!(
            preflight(&cfg, Some("   ")),
            Err(CloudError::MissingApiKey)
        ));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn preflight_rejects_unconfigured_endpoint() {
        let cfg = CloudConfig {
            base_url: String::new(),
            ..enabled_cfg()
        };
        assert!(matches!(
            preflight(&cfg, Some("sk-test")),
            Err(CloudError::NotConfigured)
        ));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn preflight_passes_through_a_trimmed_key() {
        let key = preflight(&enabled_cfg(), Some("  sk-test  ")).unwrap();
        assert_eq!(key, "sk-test");
    }

    #[cfg(feature = "cloud")]
    #[tokio::test]
    async fn disabled_config_short_circuits_before_any_request() {
        // base_url points at a closed local port: if a request were attempted
        // the error would be Http (connection refused), not Disabled.
        let cfg = CloudConfig {
            enabled: false,
            base_url: "http://127.0.0.1:9".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        let err = cloud_rewrite(&cfg, "system", "user").await.unwrap_err();
        assert!(matches!(err, CloudError::Disabled));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn request_body_has_openai_chat_shape() {
        let body = build_request_body("gpt-4o-mini", "be terse", "hello world");
        assert_eq!(body["model"], "gpt-4o-mini");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be terse");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "hello world");
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn extract_content_reads_the_first_choice() {
        let response = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "  Hello.  " } }]
        });
        assert_eq!(extract_content(&response).unwrap(), "Hello.");
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn extract_content_rejects_a_malformed_response() {
        let response = serde_json::json!({ "error": { "message": "nope" } });
        assert!(matches!(
            extract_content(&response),
            Err(CloudError::MalformedResponse(_))
        ));
    }
}
