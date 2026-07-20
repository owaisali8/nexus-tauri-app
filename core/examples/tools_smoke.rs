//! Live check of the tool loop against a running LM Studio server.
//!
//! Verifies the full round trip: the model requests a tool, the engine runs
//! it, feeds the result back, and the model answers using it.
//!
//! ```bash
//! cargo run -p essentio-core --example tools_smoke
//! ```

use essentio_core::{
    engine::{AgentEngine, EngineEvent, EngineKind, RunOptions, UserInput, direct::DirectEngine},
    memory::Store,
    providers::{ChatTransport, ProviderConfig, openai_compat::OpenAiCompatClient},
    tools::{AutoApprove, builtin::default_registry},
};
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ProviderConfig::lm_studio();
    let models = OpenAiCompatClient::new(&provider, None)?
        .list_models()
        .await?;
    let model = models
        .iter()
        .find(|model| !model.id.contains("embed"))
        .ok_or("no non-embedding model loaded in LM Studio")?;

    println!(
        "== tool loop via {} (model: {}) ==",
        provider.label, model.id
    );

    let store = Store::open_in_memory()?;
    let session = store.create_session("tools", "lmstudio-local", &model.id, EngineKind::Direct)?;

    let registry = default_registry();
    println!(
        "  tools offered : {:?}",
        registry
            .specs()
            .iter()
            .map(|spec| spec.name.clone())
            .collect::<Vec<_>>()
    );

    // Read-only tools bypass the gate anyway; AutoApprove keeps this
    // non-interactive.
    let engine = DirectEngine::new(provider, None, store.clone())
        .with_tools(registry, Arc::new(AutoApprove));

    let mut opts = RunOptions::new("lmstudio-local", &model.id);
    opts.system_prompt =
        Some("You have tools. Use them when the question needs live data.".to_string());
    opts.temperature = Some(0.1);
    opts.tool_ids = vec!["current_time".to_string()];

    let mut stream = engine
        .run_stream(
            session.id.clone().into(),
            UserInput::text("What is the current unix timestamp? Use your tools, then state it."),
            opts,
        )
        .await?;

    let mut calls = 0usize;
    let mut results = 0usize;
    let mut text = String::new();
    let mut terminal = None;

    while let Some(event) = stream.next().await {
        match event {
            EngineEvent::Token { text: chunk } => text.push_str(&chunk),
            EngineEvent::ToolCall { id, name, args } => {
                calls += 1;
                println!("  -> call    : {name} (id {id}) args={args}");
            }
            EngineEvent::ToolResult { id, ok, output } => {
                results += 1;
                println!("  <- result  : id {id} ok={ok} output={output}");
            }
            EngineEvent::Done { usage } => terminal = Some(format!("Done (usage: {usage:?})")),
            EngineEvent::Error { message } => terminal = Some(format!("Error: {message}")),
            EngineEvent::Citation { source, .. } => println!("  citation: {source}"),
        }
    }

    println!("\n== RESULT ==");
    println!("  tool calls   : {calls}");
    println!("  tool results : {results}");
    println!(
        "  terminal     : {}",
        terminal.as_deref().unwrap_or("MISSING")
    );
    println!("  answer       : {text}");

    assert!(
        calls > 0,
        "the model never requested a tool — the tools array may not be reaching it"
    );
    assert_eq!(calls, results, "every call must produce a result");
    assert!(!text.is_empty(), "no answer after the tool round");

    // The transcript keeps the question and the final answer; tool traffic is
    // in-run only.
    let stored = store.load_messages(&session.id)?;
    println!("  persisted    : {} messages", stored.len());
    assert_eq!(stored.len(), 2, "expected the user turn and the answer");

    println!("\nOK — tool call, execution, and follow-up answer verified.");
    Ok(())
}
