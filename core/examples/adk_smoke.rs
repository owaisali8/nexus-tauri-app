//! Live smoke test of the ADK engine against a running LM Studio server.
//!
//! Exercises the same seam the app uses: AgentEngine::run_stream -> EngineEvent.
//!
//! ```bash
//! cargo run -p essentio-core --example adk_smoke
//! ```

use essentio_core::{
    engine::{AgentEngine, EngineEvent, EngineKind, RunOptions, UserInput, adk::AdkEngine},
    memory::Store,
    providers::{ChatTransport, ProviderConfig, openai_compat::OpenAiCompatClient},
};
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ProviderConfig::lm_studio();
    provider.validate()?;

    // Reuse the direct client purely to pick a loaded chat model.
    let models = OpenAiCompatClient::new(&provider, None)?
        .list_models()
        .await?;
    let model = models
        .iter()
        .find(|model| !model.id.contains("embed"))
        .ok_or("no non-embedding model loaded in LM Studio")?;
    println!(
        "== ADK engine via {} (model: {}) ==",
        provider.label, model.id
    );

    // In-memory store: this exercises the engine, not durability.
    let store = Store::open_in_memory()?;
    let session = store.create_session("smoke", "lmstudio-local", &model.id, EngineKind::Adk)?;
    let engine = AdkEngine::new(provider, None, store.clone());

    let mut opts = RunOptions::new("lmstudio-local", &model.id);
    opts.system_prompt = Some("You are terse. Answer in one short sentence.".to_string());

    let mut stream = engine
        .run_stream(
            session.id.clone().into(),
            UserInput::text("What is a Cargo workspace?"),
            opts,
        )
        .await?;

    let mut tokens = 0usize;
    let mut text = String::new();
    let mut terminal = None;

    while let Some(event) = stream.next().await {
        match event {
            EngineEvent::Token { text: chunk } => {
                tokens += 1;
                print!("{chunk}");
                std::io::stdout().flush().ok();
                text.push_str(&chunk);
            }
            EngineEvent::ToolCall { name, .. } => println!("\n[tool call: {name}]"),
            EngineEvent::ToolResult { id, ok, .. } => {
                println!("\n[tool result {id}: ok={ok}]")
            }
            EngineEvent::Done { usage } => terminal = Some(format!("Done (usage: {usage:?})")),
            EngineEvent::Error { message } => terminal = Some(format!("Error: {message}")),
            EngineEvent::Citation { source, .. } => println!("\n[citation: {source}]"),
        }
    }

    println!("\n\n== RESULT ==");
    println!("  token events : {tokens}");
    println!("  chars        : {}", text.len());
    println!(
        "  terminal     : {}",
        terminal.as_deref().unwrap_or("MISSING")
    );

    assert!(!text.is_empty(), "engine produced no assistant text");
    assert!(terminal.is_some(), "stream ended without a terminal event");

    // The transcript should now hold both sides of the turn.
    let stored = store.load_messages(&session.id)?;
    println!("  persisted    : {} messages", stored.len());
    for message in &stored {
        println!("    [{}] {}", message.role, message.content);
    }
    assert_eq!(stored.len(), 2, "expected the user and assistant turns");
    assert_eq!(stored[0].role, "user");
    assert_eq!(stored[1].role, "assistant");

    // Regression: ADK's own transport buffers on a bare `<` or `[` (partial
    // tool-call markers) and flushes after turn_complete, silently dropping
    // everything after the first one. See engine/adk/model.rs.
    println!("\n== bracket regression ==");
    let mut opts = RunOptions::new("lmstudio-local", &model.id);
    opts.system_prompt = Some("You are a Rust expert. Answer directly.".to_string());
    opts.temperature = Some(0.3);

    let bracket_session =
        store.create_session("brackets", "lmstudio-local", &model.id, EngineKind::Adk)?;
    let mut stream = engine
        .run_stream(
            bracket_session.id.into(),
            UserInput::text(
                "Explain Rust generics in two sentences. \
                 Mention Vec<T> and #[derive(Debug)] explicitly.",
            ),
            opts,
        )
        .await?;

    let mut bracket_text = String::new();
    while let Some(event) = stream.next().await {
        if let EngineEvent::Token { text: chunk } = event {
            bracket_text.push_str(&chunk);
        }
    }

    let angles = bracket_text.matches('<').count();
    let squares = bracket_text.matches('[').count();
    println!("  chars     : {}", bracket_text.len());
    println!("  '<' count : {angles}");
    println!("  '[' count : {squares}");
    println!("  text      : {bracket_text}");

    assert!(
        angles > 0 || squares > 0,
        "no angle or square brackets survived the stream — ADK tail-loss has regressed"
    );

    println!("\nOK — ADK streaming, persistence, and bracket handling verified.");

    Ok(())
}
