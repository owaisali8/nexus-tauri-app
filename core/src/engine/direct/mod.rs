//! Framework-free [`AgentEngine`] over the OpenAI-compatible client.
//!
//! No agent framework and no tool loop — just streamed completions. It exists
//! so plain chat has a path with minimal moving parts, and so the ADK engine
//! has something to be compared against.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use futures::{StreamExt, stream::BoxStream};

use crate::{
    Result,
    engine::{AgentEngine, EngineEvent, RunOptions, SessionId, UserInput},
    providers::{
        ProviderConfig,
        openai_compat::{ChatMessage, OpenAiCompatClient},
    },
};

type History = Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>;

pub struct DirectEngine {
    provider: ProviderConfig,
    api_key: Option<String>,
    /// Per-session transcript.
    ///
    /// The trait hands over a single turn, so conversation state is the
    /// engine's job — ADK keeps it in a session service, and this is the
    /// equivalent. Phase 1 replaces it with the SQLite store.
    history: History,
}

impl DirectEngine {
    pub fn new(provider: ProviderConfig, api_key: Option<String>) -> Self {
        Self {
            provider,
            api_key,
            history: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Number of stored turns for a session. Exposed for tests.
    pub fn turn_count(&self, session_id: &str) -> usize {
        self.history
            .lock()
            .map(|history| history.get(session_id).map_or(0, Vec::len))
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl AgentEngine for DirectEngine {
    async fn run_stream(
        &self,
        session_id: SessionId,
        input: UserInput,
        opts: RunOptions,
    ) -> Result<BoxStream<'static, EngineEvent>> {
        let client = OpenAiCompatClient::new(&self.provider, self.api_key.clone())?;
        let key = session_id.0.clone();

        let mut messages = Vec::new();
        if let Some(system_prompt) = opts.system_prompt.as_deref() {
            messages.push(ChatMessage::system(system_prompt));
        }

        {
            let mut history = self
                .history
                .lock()
                .map_err(|_| crate::Error::Engine("history lock poisoned".to_string()))?;
            let turns = history.entry(key.clone()).or_default();
            turns.push(ChatMessage::user(input.text));
            messages.extend(turns.iter().cloned());
        }

        let inner = client
            .chat_stream(&opts.model, messages, opts.temperature)
            .await?;

        // Accumulate the reply so it can be appended to history once the run
        // terminates. Without this the next turn would lose the assistant side
        // of the conversation.
        let history = Arc::clone(&self.history);
        let mut reply = String::new();

        let stream = inner.map(move |event| {
            match &event {
                EngineEvent::Token { text } => reply.push_str(text),
                EngineEvent::Done { .. } => {
                    if !reply.is_empty()
                        && let Ok(mut turns) = history.lock()
                    {
                        turns
                            .entry(key.clone())
                            .or_default()
                            .push(ChatMessage::assistant(std::mem::take(&mut reply)));
                    }
                }
                // On Error the partial reply is deliberately dropped: half a
                // sentence in history poisons every later turn.
                _ => {}
            }
            event
        });

        Ok(stream.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_starts_empty() {
        let engine = DirectEngine::new(ProviderConfig::lm_studio(), None);
        assert_eq!(engine.turn_count("session-1"), 0);
    }

    #[tokio::test]
    async fn user_turn_is_recorded_even_when_the_server_is_unreachable() {
        let mut provider = ProviderConfig::lm_studio();
        // Port chosen to be closed so the request fails fast.
        provider.base_url = Some("http://127.0.0.1:1/v1".to_string());
        let engine = DirectEngine::new(provider, None);

        let result = engine
            .run_stream(
                "session-1".into(),
                UserInput::text("hello"),
                RunOptions::new("lmstudio-local", "any-model"),
            )
            .await;

        assert!(result.is_err(), "expected a transport error");
        assert_eq!(
            engine.turn_count("session-1"),
            1,
            "the user turn should still be recorded"
        );
    }
}
