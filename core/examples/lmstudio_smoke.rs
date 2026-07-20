//! Live smoke test against a running LM Studio server.
//!
//! Not a unit test: it needs a real server, so it stays out of `cargo test`.
//!
//! ```bash
//! cargo run -p essentio-core --example lmstudio_smoke
//! ```

use essentio_core::{
    engine::EngineEvent,
    providers::{ChatMessage, ChatTransport, ProviderConfig, openai_compat::OpenAiCompatClient},
};
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ProviderConfig::lm_studio();
    config.validate()?;
    let client = OpenAiCompatClient::new(&config, None)?;

    println!("== GET /models ==");
    let models = client.list_models().await?;
    for model in &models {
        println!("  - {}", model.id);
    }

    let chat_model = models
        .iter()
        .find(|model| !model.id.contains("embed"))
        .ok_or("no non-embedding model loaded in LM Studio")?;
    println!("\n== POST /chat/completions (model: {}) ==", chat_model.id);

    let mut stream = client
        .chat_stream(
            &chat_model.id,
            vec![
                ChatMessage::system("You are terse. Answer in one short sentence."),
                ChatMessage::user("What is a Tauri sidecar?"),
            ],
            Some(0.2),
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
            EngineEvent::Done { usage } => {
                terminal = Some(format!("Done (usage: {usage:?})"));
            }
            EngineEvent::Error { message } => {
                terminal = Some(format!("Error: {message}"));
            }
            other => println!("\n[unexpected event: {other:?}]"),
        }
    }

    println!("\n\n== RESULT ==");
    println!("  token events : {tokens}");
    println!("  chars        : {}", text.len());
    println!(
        "  terminal     : {}",
        terminal.as_deref().unwrap_or("MISSING")
    );

    assert!(
        tokens > 1,
        "expected incremental streaming, got {tokens} event(s)"
    );
    assert!(terminal.is_some(), "stream ended without a terminal event");
    println!("\nOK — streaming verified against LM Studio, zero cloud calls.");

    Ok(())
}
