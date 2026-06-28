use serde::{Deserialize, Serialize};

use crate::core::{mcp::MCP_SERVERS, skills::AGENT_SKILLS, tools::CORE_TOOLS};

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
    vec![
        AgentProfile {
            id: "job-application-agent".to_string(),
            name: "Job Application Agent".to_string(),
            description: "Browser agent for applying to jobs with resume upload support.".to_string(),
            system_instructions: [
                "You are an employment application assistant.",
                "Use the browser only for the user-specified job search and application workflow.",
                "Extract requirements, tailor answers from local notes and resume files, and attach approved PDF files.",
                "Never submit a final application, sign legal attestations, or message recruiters without explicit human approval.",
            ]
            .join("\n"),
            provider_id: "openai".to_string(),
            model: "gpt-4.1-mini".to_string(),
            tools: CORE_TOOLS[0..5]
                .iter()
                .map(|tool| tool.to_string())
                .collect(),
            mcps: MCP_SERVERS.iter().map(|mcp| mcp.to_string()).collect(),
            skills: AGENT_SKILLS[0..2]
                .iter()
                .map(|skill| skill.to_string())
                .collect(),
        },
        AgentProfile {
            id: "research-agent".to_string(),
            name: "Research Agent".to_string(),
            description: "Collects notes, files, and source summaries into local folders.".to_string(),
            system_instructions: "Research thoroughly, cite saved files, and keep local memory organized.".to_string(),
            provider_id: "ollama".to_string(),
            model: "qwen2.5-coder".to_string(),
            tools: CORE_TOOLS[4..6]
                .iter()
                .map(|tool| tool.to_string())
                .collect(),
            mcps: vec![MCP_SERVERS[0].to_string()],
            skills: vec![AGENT_SKILLS[2].to_string()],
        },
    ]
}
