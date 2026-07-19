//! Live smoke test of the ADK engine against a running LM Studio server.
//!
//! Exercises the same seam the app uses: AgentEngine::run_stream -> EngineEvent.
//!
//! ```bash
//! cargo run -p essentio-core --example adk_smoke
//! ```

use essentio_core::{
    engine::{AgentEngine, EngineEvent, RunOptions, UserInput, adk::AdkEngine},
    providers::{ProviderConfig, openai_compat::OpenAiCompatClient},
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

    let engine = AdkEngine::new(provider, None);

    let mut opts = RunOptions::new("lmstudio-local", &model.id);
    opts.system_prompt = Some("You are terse. Answer in one short sentence.".to_string());

    let mut stream = engine
        .run_stream(
            "smoke-session".into(),
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
    println!("\nOK — ADK engine streaming verified through the AgentEngine seam.");

    Ok(())
}
