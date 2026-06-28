use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProvider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub status: ProviderStatus,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
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

pub fn default_providers() -> Vec<LlmProvider> {
    vec![
        provider(
            "openai",
            "OpenAI",
            ProviderKind::Cloud,
            cloud_status("OPENAI_API_KEY"),
            &["gpt-4.1", "gpt-4.1-mini", "o4-mini"],
        ),
        provider(
            "anthropic",
            "Anthropic",
            ProviderKind::Cloud,
            cloud_status("ANTHROPIC_API_KEY"),
            &["claude-sonnet-4", "claude-haiku-3.5"],
        ),
        provider(
            "openrouter",
            "OpenRouter",
            ProviderKind::Cloud,
            cloud_status("OPENROUTER_API_KEY"),
            &["openrouter/auto", "anthropic/claude-sonnet-4"],
        ),
        provider(
            "ollama",
            "Ollama",
            ProviderKind::Local,
            ProviderStatus::LocalRequired,
            &["llama3.1", "qwen2.5-coder", "mistral"],
        ),
        provider(
            "lmstudio",
            "LM Studio",
            ProviderKind::Local,
            ProviderStatus::LocalRequired,
            &["local-model"],
        ),
    ]
}

fn provider(
    id: &str,
    name: &str,
    kind: ProviderKind,
    status: ProviderStatus,
    models: &[&str],
) -> LlmProvider {
    LlmProvider {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        status,
        models: models.iter().map(|model| model.to_string()).collect(),
    }
}

fn cloud_status(env_key: &str) -> ProviderStatus {
    match std::env::var(env_key) {
        Ok(value) if !value.trim().is_empty() => ProviderStatus::Configured,
        _ => ProviderStatus::NeedsKey,
    }
}
