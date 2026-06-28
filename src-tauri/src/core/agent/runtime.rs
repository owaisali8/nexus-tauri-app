use rig_core::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
    providers::{anthropic, ollama, openai, openrouter},
};
use serde::{Deserialize, Serialize};

use crate::core::{agent::AgentProfile, llm::StoredProvider};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePlan {
    pub agent_id: String,
    pub provider_id: String,
    pub model: String,
    pub rig_provider: String,
    pub ready: bool,
    pub missing_configuration: Vec<String>,
    pub tools: Vec<String>,
    pub mcps: Vec<String>,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptRequest {
    pub agent_id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptResponse {
    pub agent_id: String,
    pub provider_id: String,
    pub model: String,
    pub output: String,
    pub runtime_plan: RuntimePlan,
}

pub fn build_runtime_plan(agent: &AgentProfile, providers: &[StoredProvider]) -> RuntimePlan {
    let missing_configuration = crate::core::llm::missing_configuration(&agent.provider_id, providers);

    RuntimePlan {
        agent_id: agent.id.clone(),
        provider_id: agent.provider_id.clone(),
        model: agent.model.clone(),
        rig_provider: rig_provider_name(&agent.provider_id).to_string(),
        ready: missing_configuration.is_empty(),
        missing_configuration,
        tools: agent.tools.clone(),
        mcps: agent.mcps.clone(),
        skills: agent.skills.clone(),
    }
}

pub async fn run_agent_prompt(
    agent: &AgentProfile,
    providers: &[StoredProvider],
    request: AgentPromptRequest,
) -> Result<AgentPromptResponse, String> {
    if request.prompt.trim().is_empty() {
        return Err("prompt is required".to_string());
    }

    if request.agent_id != agent.id {
        return Err(format!(
            "request agent_id {} does not match loaded agent {}",
            request.agent_id, agent.id
        ));
    }

    let runtime_plan = build_runtime_plan(agent, providers);
    if !runtime_plan.ready {
        return Err(format!(
            "agent runtime is missing configuration: {}",
            runtime_plan.missing_configuration.join(", ")
        ));
    }

    let provider = crate::core::llm::resolve_provider(&agent.provider_id, providers);
    let output = run_agent_prompt_with_provider(agent, provider, &request.prompt).await?;

    Ok(AgentPromptResponse {
        agent_id: agent.id.clone(),
        provider_id: agent.provider_id.clone(),
        model: agent.model.clone(),
        output,
        runtime_plan,
    })
}

pub async fn run_agent_prompt_with_provider(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    match agent.provider_id.as_str() {
        "openai" => run_openai(agent, provider, prompt).await,
        "anthropic" => run_anthropic(agent, provider, prompt).await,
        "openrouter" => run_openrouter(agent, provider, prompt).await,
        "ollama" => run_ollama(agent, provider, prompt).await,
        "lmstudio" => run_lmstudio(agent, provider, prompt).await,
        provider_id => Err(format!("unsupported provider: {provider_id}")),
    }
}

async fn run_openai(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    let client = if let Some(config) = provider.and_then(openai_api_key) {
        let mut builder = openai::Client::builder().api_key(config);
        if let Some(base_url) = provider.and_then(|item| item.base_url.as_deref()) {
            builder = builder.base_url(base_url);
        }
        builder.build().map_err(|error| error.to_string())?
    } else {
        openai::Client::from_env().map_err(|error| error.to_string())?
    };

    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_anthropic(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    let client = if let Some(api_key) = provider.and_then(cloud_api_key) {
        anthropic::Client::builder()
            .api_key(api_key)
            .build()
            .map_err(|error| error.to_string())?
    } else {
        anthropic::Client::from_env().map_err(|error| error.to_string())?
    };

    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_openrouter(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    let client = if let Some(api_key) = provider.and_then(cloud_api_key) {
        openrouter::Client::builder()
            .api_key(api_key)
            .build()
            .map_err(|error| error.to_string())?
    } else {
        openrouter::Client::from_env().map_err(|error| error.to_string())?
    };

    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_ollama(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    let client = if let Some(base_url) = provider
        .and_then(|item| item.base_url.as_deref())
        .filter(|url| !url.trim().is_empty())
    {
        let api_key = provider
            .and_then(|item| item.api_key.as_deref())
            .unwrap_or("");
        ollama::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|error| error.to_string())?
    } else {
        ollama::Client::from_env().map_err(|error| error.to_string())?
    };

    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_lmstudio(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    prompt: &str,
) -> Result<String, String> {
    let base_url = provider
        .and_then(|item| item.base_url.clone())
        .or_else(|| std::env::var("LMSTUDIO_BASE_URL").ok())
        .unwrap_or_else(|| "http://localhost:1234/v1".to_string());
    let api_key = provider
        .and_then(cloud_api_key)
        .or_else(|| std::env::var("LMSTUDIO_API_KEY").ok())
        .unwrap_or_else(|| "lm-studio".to_string());

    let client = openai::CompletionsClient::builder()
        .api_key(&api_key)
        .base_url(&base_url)
        .build()
        .map_err(|error| error.to_string())?;

    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

fn cloud_api_key(provider: &StoredProvider) -> Option<String> {
    provider
        .api_key
        .as_ref()
        .filter(|key| !key.trim().is_empty())
        .cloned()
}

fn openai_api_key(provider: &StoredProvider) -> Option<&str> {
    provider.api_key.as_deref().filter(|key| !key.trim().is_empty())
}

fn rig_provider_name(provider_id: &str) -> &'static str {
    match provider_id {
        "anthropic" => "rig_core::providers::anthropic",
        "ollama" => "rig_core::providers::ollama",
        "openrouter" => "rig_core::providers::openrouter",
        "openai" => "rig_core::providers::openai",
        "lmstudio" => "rig_core::providers::openai::CompletionsClient",
        _ => "unsupported",
    }
}

pub fn rig_marker() -> &'static str {
    rig_core::providers::openai::GPT_5_2
}
