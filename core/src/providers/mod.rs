//! Provider configuration.
//!
//! A [`ProviderConfig`] never contains a secret. `api_key_ref` names an OS
//! keychain entry; the secret is fetched at use time and is not serialized to
//! disk, logs, or the frontend.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Default LM Studio OpenAI-compatible endpoint.
pub const LM_STUDIO_DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";

/// Placeholder credential for local servers that ignore authentication.
pub const LOCAL_PLACEHOLDER_KEY: &str = "lm-studio";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    DeepSeek,
    /// LM Studio, Ollama, vLLM — anything speaking the OpenAI wire format.
    OpenAiCompatible,
}

impl ProviderKind {
    /// Whether a `base_url` is mandatory for this kind.
    pub fn requires_base_url(&self) -> bool {
        matches!(self, Self::OpenAiCompatible)
    }

    /// Whether a real credential is mandatory. Local servers accept a
    /// placeholder, so they are exempt.
    pub fn requires_api_key(&self) -> bool {
        !matches!(self, Self::OpenAiCompatible)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Stable id, e.g. `lmstudio-local`.
    pub id: String,
    /// Human label, e.g. `LM Studio (local)`.
    pub label: String,
    pub kind: ProviderKind,
    /// Required for [`ProviderKind::OpenAiCompatible`].
    #[serde(default)]
    pub base_url: Option<String>,
    /// Keychain entry name — never the secret itself.
    #[serde(default)]
    pub api_key_ref: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
}

impl ProviderConfig {
    /// A ready-to-use LM Studio provider pointing at the default local port.
    pub fn lm_studio() -> Self {
        Self {
            id: "lmstudio-local".to_string(),
            label: "LM Studio (local)".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some(LM_STUDIO_DEFAULT_BASE_URL.to_string()),
            api_key_ref: None,
            default_model: None,
        }
    }

    /// Validate structural requirements. Does not touch the keychain, so this
    /// is safe to call on untrusted input from the frontend.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::Invalid("provider id is required".to_string()));
        }
        if self.label.trim().is_empty() {
            return Err(Error::Invalid("provider label is required".to_string()));
        }

        if self.kind.requires_base_url() && self.effective_base_url().is_none() {
            return Err(Error::ProviderMisconfigured {
                provider_id: self.id.clone(),
                reason: "base_url is required for OpenAI-compatible providers".to_string(),
            });
        }

        if self.kind.requires_api_key()
            && self
                .api_key_ref
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(Error::ProviderMisconfigured {
                provider_id: self.id.clone(),
                reason: "an API key is required for cloud providers".to_string(),
            });
        }

        Ok(())
    }

    /// Base URL with surrounding whitespace and any trailing slash removed.
    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .map(|url| url.trim().trim_end_matches('/'))
            .filter(|url| !url.is_empty())
    }

    /// Build `{base_url}/{path}` for OpenAI-compatible calls.
    pub fn endpoint(&self, path: &str) -> Result<String> {
        let base = self
            .effective_base_url()
            .ok_or_else(|| Error::ProviderMisconfigured {
                provider_id: self.id.clone(),
                reason: "no base_url configured".to_string(),
            })?;
        Ok(format!("{base}/{}", path.trim_start_matches('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lm_studio_default_is_valid() {
        ProviderConfig::lm_studio().validate().unwrap();
    }

    #[test]
    fn openai_compatible_requires_base_url() {
        let mut config = ProviderConfig::lm_studio();
        config.base_url = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn cloud_provider_requires_key_ref() {
        let config = ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            kind: ProviderKind::OpenAi,
            base_url: None,
            api_key_ref: None,
            default_model: Some("gpt-4.1".to_string()),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn endpoint_normalizes_slashes() {
        let mut config = ProviderConfig::lm_studio();
        config.base_url = Some("http://localhost:1234/v1/".to_string());
        assert_eq!(
            config.endpoint("/models").unwrap(),
            "http://localhost:1234/v1/models"
        );
    }

    #[test]
    fn config_never_serializes_a_secret() {
        let json = serde_json::to_string(&ProviderConfig::lm_studio()).unwrap();
        assert!(json.contains("apiKeyRef"));
        assert!(!json.contains("api_key\""));
    }
}
