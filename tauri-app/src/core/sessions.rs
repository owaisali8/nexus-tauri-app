use serde::{Deserialize, Serialize};

use crate::core::{
    agent::{runtime, AgentProfile},
    llm::StoredProvider,
    memory::{AgentSession, MemoryEntry, MessageRole, SessionMessage},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub agent_id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendChatMessageRequest {
    pub session_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageResponse {
    pub session_id: String,
    pub user_message: SessionMessage,
    pub assistant_message: SessionMessage,
    pub output: String,
}

pub fn create_session(agent: &AgentProfile, title: Option<String>) -> AgentSession {
    let now = now_unix();
    let session_title = title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("Chat with {}", agent.name));

    AgentSession {
        id: format!("session-{}", now),
        agent_id: agent.id.clone(),
        title: session_title,
        created_at: now,
        updated_at: now,
    }
}

pub fn build_user_message(session_id: &str, content: &str) -> SessionMessage {
    let now = now_unix();
    SessionMessage {
        id: format!("msg-{now}-user"),
        session_id: session_id.to_string(),
        role: MessageRole::User,
        content: content.to_string(),
        created_at: now,
    }
}

pub fn build_assistant_message(session_id: &str, content: &str) -> SessionMessage {
    let now = now_unix();
    SessionMessage {
        id: format!("msg-{now}-assistant"),
        session_id: session_id.to_string(),
        role: MessageRole::Assistant,
        content: content.to_string(),
        created_at: now,
    }
}

pub async fn run_chat_turn(
    agent: &AgentProfile,
    provider: Option<&StoredProvider>,
    memory: &[MemoryEntry],
    history: &[SessionMessage],
    prompt: &str,
) -> Result<String, String> {
    let composed_prompt = compose_chat_prompt(memory, history, prompt);
    runtime::run_agent_prompt_with_provider(agent, provider, &composed_prompt).await
}

fn compose_chat_prompt(
    memory: &[MemoryEntry],
    history: &[SessionMessage],
    prompt: &str,
) -> String {
    let mut sections = Vec::new();

    if !memory.is_empty() {
        let memory_block = memory
            .iter()
            .map(|entry| format!("- {}: {}", entry.key, entry.value))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Long-term memory:\n{memory_block}"));
    }

    if !history.is_empty() {
        let transcript = history
            .iter()
            .map(|message| {
                let speaker = match message.role {
                    MessageRole::User => "User",
                    MessageRole::Assistant => "Assistant",
                    MessageRole::System => "System",
                };
                format!("{speaker}: {}", message.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Conversation so far:\n{transcript}"));
    }

    sections.push(format!("Latest user message:\n{prompt}"));
    sections.join("\n\n")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
