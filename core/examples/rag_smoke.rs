//! Live RAG check against LM Studio's embedding model.
//!
//! Ingests two documents, then confirms a query retrieves the right one and
//! an unrelated query retrieves nothing.
//!
//! ```bash
//! cargo run -p nexus-core --example rag_smoke
//! ```

use nexus_core::{
    memory::Store,
    providers::{ChatTransport, ProviderConfig, openai_compat::OpenAiCompatClient},
    rag::{Retriever, embed::OpenAiCompatEmbedder},
};
use std::sync::Arc;

const RUST_DOC: &str = "
Tauri builds desktop applications with a Rust backend and a web frontend.
Unlike Electron it uses the operating system's own webview, so binaries are
far smaller and start faster. The Rust side exposes commands the frontend
invokes over IPC.

Ownership in Rust means each value has a single owner. When the owner goes
out of scope the value is dropped. Borrowing lets code reference a value
without taking ownership, and the borrow checker enforces the rules at
compile time.
";

const COOKING_DOC: &str = "
A good risotto needs constant attention. Toast the rice in butter until the
grains turn translucent, then add warm stock one ladle at a time, stirring
until each addition is absorbed before adding the next.

Bread dough develops gluten through kneading. A wetter dough produces a more
open crumb but is harder to shape by hand.
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ProviderConfig::lm_studio();

    let models = OpenAiCompatClient::new(&provider, None)?
        .list_models()
        .await?;
    let embedding_model = models
        .iter()
        .find(|model| model.id.contains("embed"))
        .ok_or("no embedding model loaded in LM Studio")?;

    println!("== embeddings via {} ==", embedding_model.id);

    let store = Store::open_in_memory()?;
    let embedder = OpenAiCompatEmbedder::new(&provider, None, &embedding_model.id)?;
    let retriever = Retriever::new(store.clone(), Arc::new(embedder));

    let rust_chunks = retriever
        .ingest("Tauri and Rust", "docs/rust.md", "text/markdown", RUST_DOC)
        .await?;
    let cooking_chunks = retriever
        .ingest("Cooking", "docs/cooking.md", "text/markdown", COOKING_DOC)
        .await?;

    println!("  ingested : {rust_chunks} + {cooking_chunks} chunks");

    // --- a query that should hit the Rust document ---
    println!("\n== query: \"how does borrowing work?\" ==");
    let hits = retriever
        .search("how does borrowing work?", 3, 0.25)
        .await?;
    for hit in &hits {
        println!(
            "  [{:.3}] {} — {}",
            hit.score,
            hit.source,
            first_line(&hit.text)
        );
    }
    assert!(!hits.is_empty(), "expected a match for a Rust question");
    assert_eq!(
        hits[0].source, "docs/rust.md",
        "the closest passage should come from the Rust document"
    );

    // --- a query that should hit the cooking document ---
    println!("\n== query: \"how do I cook rice properly?\" ==");
    let hits = retriever
        .search("how do I cook rice properly?", 3, 0.25)
        .await?;
    for hit in &hits {
        println!(
            "  [{:.3}] {} — {}",
            hit.score,
            hit.source,
            first_line(&hit.text)
        );
    }
    assert_eq!(
        hits.first().map(|hit| hit.source.as_str()),
        Some("docs/cooking.md"),
        "the closest passage should come from the cooking document"
    );

    // --- a query unrelated to everything ---
    //
    // The point is not that this returns nothing — embedding similarity has a
    // high floor, so it usually will return something — but that the best
    // score stays below the confidence bar the tool uses to warn the model.
    println!("\n== query: \"orbital mechanics of binary stars\" ==");
    let hits = retriever
        .search("orbital mechanics of binary stars", 3, 0.35)
        .await?;
    let best = hits.iter().map(|hit| hit.score).fold(f32::MIN, f32::max);
    println!("  matches: {}, best score: {best:.3}", hits.len());
    for hit in &hits {
        println!("  [{:.3}] {}", hit.score, hit.source);
    }
    assert!(
        hits.is_empty() || best < 0.6,
        "an unrelated query scored {best:.3}, at or above the confidence bar — \
         the model would be told this is a solid match"
    );

    // --- re-ingest replaces rather than duplicates ---
    retriever
        .ingest("Tauri and Rust", "docs/rust.md", "text/markdown", RUST_DOC)
        .await?;
    let documents = store.list_documents()?;
    println!("\n== documents after re-ingest ==");
    for document in &documents {
        println!("  {} ({} chunks)", document.source, document.chunk_count);
    }
    assert_eq!(
        documents.len(),
        2,
        "re-ingesting must replace, not duplicate"
    );

    println!("\nOK — ingestion, retrieval, and ranking verified against real embeddings.");
    Ok(())
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    line.chars().take(60).collect()
}
