use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProvider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub models: Vec<String>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProvider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub status: ProviderStatus,
    pub models: Vec<String>,
    pub base_url: Option<String>,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Configured,
    NeedsKey,
    LocalRequired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplate {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub default_models: Vec<String>,
    pub default_base_url: Option<String>,
    pub requires_api_key: bool,
}

pub fn provider_templates() -> Vec<ProviderTemplate> {
    vec![
        template(
            "openai",
            "OpenAI",
            ProviderKind::Cloud,
            true,
            None,
            &["gpt-4.1", "gpt-4.1-mini", "o4-mini"],
        ),
        template(
            "anthropic",
            "Anthropic",
            ProviderKind::Cloud,
            true,
            None,
            &["claude-sonnet-4", "claude-haiku-3.5"],
        ),
        template(
            "openrouter",
            "OpenRouter",
            ProviderKind::Cloud,
            true,
            None,
            &["openrouter/auto", "anthropic/claude-sonnet-4"],
        ),
        template(
            "ollama",
            "Ollama",
            ProviderKind::Local,
            false,
            Some("http://localhost:11434"),
            &["llama3.1", "qwen2.5-coder", "mistral"],
        ),
        template(
            "lmstudio",
            "LM Studio",
            ProviderKind::Local,
            false,
            Some("http://localhost:1234/v1"),
            &["local-model"],
        ),
    ]
}

pub fn merge_providers(stored: &[StoredProvider]) -> Vec<LlmProvider> {
    let templates = provider_templates();
    let mut providers = Vec::new();

    for entry in stored.iter().filter(|item| item.is_enabled) {
        providers.push(stored_to_llm(entry));
    }

    for template in templates {
        if stored.iter().any(|item| item.id == template.id) {
            continue;
        }

        providers.push(LlmProvider {
            id: template.id.clone(),
            name: template.name,
            kind: template.kind,
            status: if template.requires_api_key {
                ProviderStatus::NeedsKey
            } else {
                ProviderStatus::LocalRequired
            },
            models: template.default_models,
            base_url: template.default_base_url,
            has_api_key: false,
        });
    }

    providers
}

pub fn resolve_provider<'a>(
    provider_id: &str,
    stored: &'a [StoredProvider],
) -> Option<&'a StoredProvider> {
    stored
        .iter()
        .find(|provider| provider.id == provider_id && provider.is_enabled)
}

pub fn provider_is_ready(provider: &StoredProvider) -> bool {
    match provider.kind {
        ProviderKind::Cloud => provider
            .api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty()),
        ProviderKind::Local => true,
    }
}

pub fn missing_configuration(provider_id: &str, stored: &[StoredProvider]) -> Vec<String> {
    if let Some(provider) = resolve_provider(provider_id, stored) {
        if provider_is_ready(provider) {
            return Vec::new();
        }

        return match provider.kind {
            ProviderKind::Cloud => vec![format!("{} API key", provider.name)],
            ProviderKind::Local => vec![format!("{} base URL", provider.name)],
        };
    }

    match provider_id {
        "openai" => missing_env("OPENAI_API_KEY"),
        "anthropic" => missing_env("ANTHROPIC_API_KEY"),
        "openrouter" => missing_env("OPENROUTER_API_KEY"),
        "ollama" | "lmstudio" => Vec::new(),
        _ => vec!["configured provider".to_string()],
    }
}

fn stored_to_llm(provider: &StoredProvider) -> LlmProvider {
    LlmProvider {
        id: provider.id.clone(),
        name: provider.name.clone(),
        kind: provider.kind.clone(),
        status: if provider_is_ready(provider) {
            ProviderStatus::Configured
        } else if matches!(provider.kind, ProviderKind::Local) {
            ProviderStatus::LocalRequired
        } else {
            ProviderStatus::NeedsKey
        },
        models: provider.models.clone(),
        base_url: provider.base_url.clone(),
        has_api_key: provider
            .api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty()),
    }
}

fn template(
    id: &str,
    name: &str,
    kind: ProviderKind,
    requires_api_key: bool,
    default_base_url: Option<&str>,
    models: &[&str],
) -> ProviderTemplate {
    ProviderTemplate {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        default_models: models.iter().map(|model| model.to_string()).collect(),
        default_base_url: default_base_url.map(str::to_string),
        requires_api_key,
    }
}

fn missing_env(key: &str) -> Vec<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Vec::new(),
        _ => vec![key.to_string()],
    }
}
