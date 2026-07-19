//! ADK-Rust implementation of [`AgentEngine`].
//!
//! **This is the only module permitted to import `adk_*` crates.** Everything
//! outside it depends on the trait and `core`'s own DTOs, which is what keeps
//! swapping engines cheap. `core/tests/adk_boundary.rs` enforces this.

mod event_map;

use std::{collections::HashMap, sync::Arc};

use adk_agent::LlmAgentBuilder;
use adk_core::{Content, Part, UserId};
use adk_model::openai::{OpenAIClient, OpenAIConfig};
use adk_runner::Runner;
use adk_session::{CreateRequest, GetRequest, InMemorySessionService, SessionService};
use futures::{StreamExt, stream::BoxStream};

use crate::{
    Error, Result,
    engine::{AgentEngine, EngineEvent, RunOptions, SessionId, UserInput},
    providers::{LOCAL_PLACEHOLDER_KEY, ProviderConfig, ProviderKind},
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
    sessions: Arc<InMemorySessionService>,
}

impl AdkEngine {
    pub fn new(provider: ProviderConfig, api_key: Option<String>) -> Self {
        Self {
            provider,
            api_key,
            sessions: Arc::new(InMemorySessionService::new()),
        }
    }

    /// Build the ADK model client for this provider.
    ///
    /// Only OpenAI-compatible endpoints are wired for now — that covers
    /// LM Studio, Ollama and vLLM, which is what the app is built against.
    /// Cloud kinds land when their DoD does.
    fn model(&self, model_name: &str) -> Result<OpenAIClient> {
        match self.provider.kind {
            ProviderKind::OpenAiCompatible => {
                let base_url = self.provider.effective_base_url().ok_or_else(|| {
                    Error::ProviderMisconfigured {
                        provider_id: self.provider.id.clone(),
                        reason: "base_url is required".to_string(),
                    }
                })?;
                let api_key = self
                    .api_key
                    .clone()
                    .unwrap_or_else(|| LOCAL_PLACEHOLDER_KEY.to_string());

                OpenAIClient::new(OpenAIConfig::compatible(api_key, base_url, model_name))
                    .map_err(|error| Error::Engine(error.to_string()))
            }
            ProviderKind::OpenAi => {
                let api_key = self
                    .api_key
                    .clone()
                    .ok_or_else(|| Error::ProviderMisconfigured {
                        provider_id: self.provider.id.clone(),
                        reason: "an API key is required".to_string(),
                    })?;
                OpenAIClient::new(OpenAIConfig::new(api_key, model_name))
                    .map_err(|error| Error::Engine(error.to_string()))
            }
            other => Err(Error::Engine(format!(
                "provider kind {other:?} is not wired into the ADK engine yet"
            ))),
        }
    }

    /// Ensure the ADK session exists.
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
            .map(|_| ())
            .map_err(|error| Error::Engine(error.to_string()))
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

        self.ensure_session(&session_id.0).await?;

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

        Ok(crate::engine::ensure_terminal(mapped))
    }
}
