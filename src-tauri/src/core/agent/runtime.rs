use rig_core::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
    providers::{anthropic, ollama, openai, openrouter},
};
use serde::{Deserialize, Serialize};

use crate::core::agent::AgentProfile;

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

pub fn build_runtime_plan(agent: &AgentProfile) -> RuntimePlan {
    let missing_configuration = missing_configuration(&agent.provider_id);

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

    let runtime_plan = build_runtime_plan(agent);
    if !runtime_plan.ready {
        return Err(format!(
            "agent runtime is missing configuration: {}",
            runtime_plan.missing_configuration.join(", ")
        ));
    }

    let output = match agent.provider_id.as_str() {
        "openai" => run_openai(agent, &request.prompt).await,
        "anthropic" => run_anthropic(agent, &request.prompt).await,
        "openrouter" => run_openrouter(agent, &request.prompt).await,
        "ollama" => run_ollama(agent, &request.prompt).await,
        "lmstudio" => run_lmstudio(agent, &request.prompt).await,
        provider => Err(format!("unsupported provider: {provider}")),
    }?;

    Ok(AgentPromptResponse {
        agent_id: agent.id.clone(),
        provider_id: agent.provider_id.clone(),
        model: agent.model.clone(),
        output,
        runtime_plan,
    })
}

async fn run_openai(agent: &AgentProfile, prompt: &str) -> Result<String, String> {
    let client = openai::Client::from_env().map_err(|error| error.to_string())?;
    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_anthropic(agent: &AgentProfile, prompt: &str) -> Result<String, String> {
    let client = anthropic::Client::from_env().map_err(|error| error.to_string())?;
    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_openrouter(agent: &AgentProfile, prompt: &str) -> Result<String, String> {
    let client = openrouter::Client::from_env().map_err(|error| error.to_string())?;
    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_ollama(agent: &AgentProfile, prompt: &str) -> Result<String, String> {
    let client = ollama::Client::from_env().map_err(|error| error.to_string())?;
    client
        .agent(&agent.model)
        .preamble(&agent.system_instructions)
        .build()
        .prompt(prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn run_lmstudio(agent: &AgentProfile, prompt: &str) -> Result<String, String> {
    let base_url = std::env::var("LMSTUDIO_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:1234/v1".to_string());
    let api_key = std::env::var("LMSTUDIO_API_KEY").unwrap_or_else(|_| "lm-studio".to_string());
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

fn missing_configuration(provider_id: &str) -> Vec<String> {
    match provider_id {
        "openai" => missing_env("OPENAI_API_KEY"),
        "anthropic" => missing_env("ANTHROPIC_API_KEY"),
        "openrouter" => missing_env("OPENROUTER_API_KEY"),
        "ollama" => Vec::new(),
        "lmstudio" => Vec::new(),
        _ => vec!["supported provider".to_string()],
    }
}

fn missing_env(key: &str) -> Vec<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Vec::new(),
        _ => vec![key.to_string()],
    }
}

pub fn rig_marker() -> &'static str {
    rig_core::providers::openai::GPT_5_2
}
