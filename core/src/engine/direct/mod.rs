//! Framework-free [`AgentEngine`] over the OpenAI-compatible client.
//!
//! No agent framework and no tool loop — just streamed completions. It exists
//! so plain chat has a path with minimal moving parts, and so the ADK engine
//! has something to be compared against.

use futures::{StreamExt, stream::BoxStream};

use crate::{
    Result,
    engine::{AgentEngine, EngineEvent, RunOptions, SessionId, UserInput},
    memory::Store,
    providers::{
        ProviderConfig,
        openai_compat::{ChatMessage, OpenAiCompatClient},
    },
};

pub struct DirectEngine {
    provider: ProviderConfig,
    api_key: Option<String>,
    /// Transcripts live in SQLite, so conversations survive restart and the
    /// engine holds no conversation state of its own.
    store: Store,
}

impl DirectEngine {
    pub fn new(provider: ProviderConfig, api_key: Option<String>, store: Store) -> Self {
        Self {
            provider,
            api_key,
            store,
        }
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

        // Persist the user turn before the request goes out, so a failed or
        // cancelled run still leaves the question in the transcript.
        self.store
            .append_message(&session_id.0, "user", &input.text)?;

        let mut messages = Vec::new();
        if let Some(system_prompt) = opts.system_prompt.as_deref() {
            messages.push(ChatMessage::system(system_prompt));
        }
        messages.extend(
            self.store
                .load_messages(&session_id.0)?
                .iter()
                .map(super::super::memory::Message::to_chat_message),
        );

        let inner = client
            .chat_stream(&opts.model, messages, opts.temperature)
            .await?;

        // Accumulate the reply so it can be written once the run terminates.
        let store = self.store.clone();
        let key = session_id.0.clone();
        let mut reply = String::new();

        let stream = inner.map(move |event| {
            match &event {
                EngineEvent::Token { text } => reply.push_str(text),
                EngineEvent::Done { .. } if !reply.is_empty() => {
                    let text = std::mem::take(&mut reply);
                    // A persistence failure must not truncate the stream the
                    // user is watching; log and carry on.
                    if let Err(error) = store.append_message(&key, "assistant", &text) {
                        tracing::error!(%error, "failed to persist assistant message");
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
    use crate::engine::EngineKind;

    #[tokio::test]
    async fn user_turn_is_persisted_even_when_the_server_is_unreachable() {
        let store = Store::open_in_memory().unwrap();
        let session = store
            .create_session("s", "lmstudio-local", "any", EngineKind::Direct)
            .unwrap();

        let mut provider = ProviderConfig::lm_studio();
        // Port chosen to be closed so the request fails fast.
        provider.base_url = Some("http://127.0.0.1:1/v1".to_string());
        let engine = DirectEngine::new(provider, None, store.clone());

        let result = engine
            .run_stream(
                session.id.clone().into(),
                UserInput::text("hello"),
                RunOptions::new("lmstudio-local", "any-model"),
            )
            .await;

        assert!(result.is_err(), "expected a transport error");

        let messages = store.load_messages(&session.id).unwrap();
        assert_eq!(messages.len(), 1, "the user turn should still be recorded");
        assert_eq!(messages[0].role, "user");
    }
}
