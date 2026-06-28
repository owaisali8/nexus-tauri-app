use serde::{Deserialize, Serialize};

pub mod runtime;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_instructions: String,
    pub provider_id: String,
    pub model: String,
    pub tools: Vec<String>,
    pub mcps: Vec<String>,
    pub skills: Vec<String>,
}

pub fn default_agents() -> Vec<AgentProfile> {
    Vec::new()
}
