//! ADK-Rust implementation of [`AgentEngine`].
//!
//! **This is the only module permitted to import `adk_*` crates.** Everything
//! outside it depends on the trait and `core`'s own DTOs, which is what keeps
//! swapping engines cheap. `core/tests/adk_boundary.rs` enforces this.

mod event_map;
mod model;

use std::{collections::HashMap, sync::Arc};

use adk_agent::LlmAgentBuilder;
use adk_core::{Content, Event, Part, UserId};
use adk_runner::Runner;
use adk_session::{
    CreateRequest, DeleteRequest, GetRequest, InMemorySessionService, SessionService,
};
use futures::{StreamExt, stream::BoxStream};

use crate::{
    Error, Result,
    engine::{AgentEngine, EngineEvent, RunOptions, SessionId, UserInput, adk::model::CompatModel},
    memory::Store,
    providers::ProviderConfig,
};

const APP_NAME: &str = "essentio";
const AGENT_NAME: &str = "assistant";
const DEFAULT_USER: &str = "local";

/// Runs turns through ADK-Rust.
///
/// Holds resolved provider config; secrets are passed in at construction and
/// never read from the environment.
pub struct AdkEngine {
    provider: ProviderConfig,
    api_key: Option<String>,
    /// ADK's own session state. It is a cache, not the source of truth —
    /// SQLite is, and this is rehydrated from it on first touch after start.
    sessions: Arc<InMemorySessionService>,
    store: Store,
}

impl AdkEngine {
    pub fn new(provider: ProviderConfig, api_key: Option<String>, store: Store) -> Self {
        Self {
            provider,
            api_key,
            sessions: Arc::new(InMemorySessionService::new()),
            store,
        }
    }

    /// Build the model ADK will drive.
    ///
    /// This is [`CompatModel`], backed by our own transports rather than
    /// `adk_model`'s clients — see `model.rs` for the data-loss bug that makes
    /// ADK's OpenAI transport unusable for text containing `<` or `[`. Going
    /// through `build_transport` means every provider kind works here, not
    /// just OpenAI-compatible ones.
    fn model(&self, model_name: &str) -> Result<CompatModel> {
        CompatModel::new(&self.provider, self.api_key.clone(), model_name)
    }

    /// Ensure the ADK session exists, rehydrating it from SQLite if new.
    ///
    /// `Runner::run` does *not* create a missing session despite the docs
    /// saying it retrieves or creates one — it fails the stream with
    /// "session not found". Creating it up front is the fix.
    async fn ensure_session(&self, session_id: &str) -> Result<()> {
        let existing = self
            .sessions
            .get(GetRequest {
                app_name: APP_NAME.to_string(),
                user_id: DEFAULT_USER.to_string(),
                session_id: session_id.to_string(),
                num_recent_events: None,
                after: None,
            })
            .await;

        if existing.is_ok() {
            return Ok(());
        }

        self.sessions
            .create(CreateRequest {
                app_name: APP_NAME.to_string(),
                user_id: DEFAULT_USER.to_string(),
                session_id: Some(session_id.to_string()),
                state: HashMap::new(),
            })
            .await
            .map_err(|error| Error::Engine(error.to_string()))?;

        // Replay the stored transcript so a session resumed after restart has
        // its context back. Without this, ADK would start every conversation
        // blank while the UI showed the full history.
        for message in self.store.load_messages(session_id)? {
            let author = if message.role == "user" {
                "user"
            } else {
                AGENT_NAME
            };

            let mut event = Event::new(format!("hydrate-{}", message.id));
            event.author = author.to_string();
            event.set_content(Content {
                role: if message.role == "user" {
                    "user".to_string()
                } else {
                    "model".to_string()
                },
                parts: vec![Part::Text {
                    text: message.content,
                }],
            });

            self.sessions
                .append_event(session_id, event)
                .await
                .map_err(|error| Error::Engine(error.to_string()))?;
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentEngine for AdkEngine {
    async fn run_stream(
        &self,
        session_id: SessionId,
        input: UserInput,
        opts: RunOptions,
    ) -> Result<BoxStream<'static, EngineEvent>> {
        let model = self.model(&opts.model)?;

        let mut builder = LlmAgentBuilder::new(AGENT_NAME).model(Arc::new(model));
        if let Some(system_prompt) = opts.system_prompt.as_deref() {
            builder = builder.instruction(system_prompt);
        }
        let agent = builder
            .build()
            .map_err(|error| Error::Engine(error.to_string()))?;

        let runner = Runner::builder()
            .app_name(APP_NAME)
            .agent(Arc::new(agent))
            .session_service(self.sessions.clone())
            .build()
            .map_err(|error| Error::Engine(error.to_string()))?;

        // Hydrate before the user turn is persisted, or the replay would
        // duplicate the message ADK is about to receive as input.
        self.ensure_session(&session_id.0).await?;
        self.store
            .append_message(&session_id.0, "user", &input.text)?;

        let user_id =
            UserId::new(DEFAULT_USER).map_err(|error| Error::Engine(error.to_string()))?;
        let adk_session = adk_core::SessionId::new(session_id.0.clone())
            .map_err(|error| Error::Engine(error.to_string()))?;

        let content = Content {
            role: "user".to_string(),
            parts: vec![Part::Text { text: input.text }],
        };

        let events = runner
            .run(user_id, adk_session, content)
            .await
            .map_err(|error| Error::Engine(error.to_string()))?;

        // Flatten ADK events into EngineEvents. One ADK event can carry several
        // parts (text plus a tool call), so this is a flat_map, not a map.
        //
        // ADK emits no terminal event of its own, so `ensure_terminal` supplies
        // one — without overwriting an Error the run already produced.
        let mapped = events
            .flat_map(|item| futures::stream::iter(event_map::to_engine_events(item)))
            .boxed();

        // ADK emits no terminal event of its own, so the Done that signals
        // "reply complete" is the one ensure_terminal appends. The persist
        // step must therefore sit downstream of it — upstream it would never
        // observe a Done and the assistant turn would never be written.
        let terminated = crate::engine::ensure_terminal(mapped);

        let store = self.store.clone();
        let key = session_id.0.clone();
        let mut reply = String::new();

        let persisted = terminated.map(move |event| {
            match &event {
                EngineEvent::Token { text } => reply.push_str(text),
                EngineEvent::Done { .. } if !reply.is_empty() => {
                    let text = std::mem::take(&mut reply);
                    if let Err(error) = store.append_message(&key, "assistant", &text) {
                        tracing::error!(%error, "failed to persist assistant message");
                    }
                }
                _ => {}
            }
            event
        });

        Ok(persisted.boxed())
    }

    /// Drop ADK's cached session so the next run rehydrates from SQLite.
    ///
    /// ADK accumulates its own event list as the conversation proceeds. After
    /// a regenerate or an edit, that copy still holds the turns the user
    /// removed, so deleting it is what makes the truncation take effect.
    async fn forget_session(&self, session_id: &SessionId) -> Result<()> {
        match self
            .sessions
            .delete(DeleteRequest {
                app_name: APP_NAME.to_string(),
                user_id: DEFAULT_USER.to_string(),
                session_id: session_id.0.clone(),
            })
            .await
        {
            Ok(()) => Ok(()),
            // Nothing cached is the desired end state, not a failure.
            Err(error) => {
                tracing::debug!(%error, "no cached ADK session to forget");
                Ok(())
            }
        }
    }
}
