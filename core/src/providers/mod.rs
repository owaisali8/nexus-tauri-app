//! Provider configuration.
//!
//! A [`ProviderConfig`] never contains a secret. `api_key_ref` names an OS
//! keychain entry; the secret is fetched at use time and is not serialized to
//! disk, logs, or the frontend.

pub mod anthropic;
pub mod gemini;
pub mod openai_compat;

use std::sync::Arc;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, engine::EngineEvent};

/// Re-exported so callers depend on `providers::ChatMessage` rather than on
/// the OpenAI module, which is one transport among several.
pub use openai_compat::ChatMessage;

/// Default LM Studio OpenAI-compatible endpoint.
pub const LM_STUDIO_DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";

/// Placeholder credential for local servers that ignore authentication.
pub const LOCAL_PLACEHOLDER_KEY: &str = "lm-studio";

/// A model advertised by a provider's model-listing endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
}

/// A provider's chat wire format.
///
/// Each implementation owns one API shape — OpenAI-compatible, Anthropic
/// Messages, Gemini generateContent — and normalizes it onto [`EngineEvent`].
/// Engines depend on this trait, not on any particular provider.
#[async_trait::async_trait]
pub trait ChatTransport: Send + Sync {
    /// Models the provider reports as available.
    ///
    /// Doubles as the connection test, so it must fail rather than return an
    /// empty list when the endpoint or credential is wrong.
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// Stream one completion.
    ///
    /// The returned stream must terminate with exactly one
    /// [`EngineEvent::Done`] or [`EngineEvent::Error`].
    async fn chat_stream(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
    ) -> Result<BoxStream<'static, EngineEvent>>;
}

/// Build the transport for a provider.
///
/// `api_key` is the resolved secret from the keychain, or `None` for local
/// servers that ignore authentication.
pub fn build_transport(
    config: &ProviderConfig,
    api_key: Option<String>,
) -> Result<Arc<dyn ChatTransport>> {
    match config.kind {
        ProviderKind::Anthropic => Ok(Arc::new(anthropic::AnthropicClient::new(config, api_key)?)),
        ProviderKind::Gemini => Ok(Arc::new(gemini::GeminiClient::new(config, api_key)?)),
        // OpenAI and DeepSeek both speak the OpenAI wire format; the only
        // difference is the default base URL, which config already carries.
        ProviderKind::OpenAi | ProviderKind::DeepSeek | ProviderKind::OpenAiCompatible => Ok(
            Arc::new(openai_compat::OpenAiCompatClient::new(config, api_key)?),
        ),
    }
}

/// Split a message list into an optional system prompt and the rest.
///
/// Anthropic and Gemini both take system instructions as a separate top-level
/// field rather than a message with `role: "system"`.
pub(crate) fn split_system(messages: Vec<ChatMessage>) -> (Option<String>, Vec<ChatMessage>) {
    let mut system = Vec::new();
    let mut rest = Vec::new();

    for message in messages {
        if message.role == "system" {
            system.push(message.content);
        } else {
            rest.push(message);
        }
    }

    let system = if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    };

    (system, rest)
}

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
    ///
    /// Only generic OpenAI-compatible servers need one, since there is no
    /// sensible default for "some endpoint the user is running". The named
    /// providers fall back to [`ProviderKind::default_base_url`].
    pub fn requires_base_url(&self) -> bool {
        matches!(self, Self::OpenAiCompatible)
    }

    /// Whether a real credential is mandatory. Local servers accept a
    /// placeholder, so they are exempt.
    pub fn requires_api_key(&self) -> bool {
        !matches!(self, Self::OpenAiCompatible)
    }

    /// Endpoint used when a provider has no explicit `base_url`.
    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::DeepSeek => Some("https://api.deepseek.com/v1"),
            Self::Anthropic => Some("https://api.anthropic.com"),
            Self::Gemini => Some("https://generativelanguage.googleapis.com"),
            Self::OpenAiCompatible => None,
        }
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

    /// Base URL with surrounding whitespace and any trailing slash removed,
    /// falling back to the kind's default when none is configured.
    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .map(|url| url.trim().trim_end_matches('/'))
            .filter(|url| !url.is_empty())
            .or_else(|| self.kind.default_base_url())
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
