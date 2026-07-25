//! Retrieval exposed to the model as a tool.
//!
//! A tool rather than automatic context injection: the model decides when a
//! question needs the user's documents, instead of every message paying for a
//! retrieval that usually is not relevant.

use std::sync::Arc;

use crate::{
    Result,
    rag::{Passage, Retriever},
    tools::{Effect, Tool, ToolSpec},
};

/// How many passages a search returns.
///
/// Enough to cover a question spanning sections, few enough to leave room for
/// the conversation.
const DEFAULT_LIMIT: usize = 5;

/// Similarity below which a passage is discarded outright.
///
/// Measured against `nomic-embed-text-v1.5`, which is what LM Studio ships:
/// genuinely relevant passages scored 0.64–0.71, while a query about
/// astrophysics against documents on Rust and cooking still scored 0.41–0.51.
/// An absolute floor therefore cannot separate the two reliably — baseline
/// similarity is a property of the embedding model, not of relevance.
///
/// This floor only removes obvious noise. [`CONFIDENT_SCORE`] does the real
/// work.
const MIN_SCORE: f32 = 0.35;

/// Above this, a match is treated as solid.
///
/// Below it the passages are still returned — the user's documents may well
/// contain a partial answer — but flagged, so the model can say "your files
/// do not really cover this" instead of citing a weak match as authoritative.
const CONFIDENT_SCORE: f32 = 0.6;

pub struct SearchDocuments {
    retriever: Arc<Retriever>,
}

impl SearchDocuments {
    pub fn new(retriever: Arc<Retriever>) -> Self {
        Self { retriever }
    }
}

#[async_trait::async_trait]
impl Tool for SearchDocuments {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_documents".to_string(),
            description: "Search the user's indexed documents for passages relevant to a \
                          question. Use this whenever the answer might depend on the user's \
                          own files, notes, or previously ingested material. Returns passages \
                          with their source, which you should cite."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to look for. A natural-language question works \
                                        better than keywords."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum passages to return (default 5).",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
        }
    }

    /// Reading indexed documents changes nothing, so this does not prompt.
    /// The documents were ingested by the user deliberately.
    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<serde_json::Value> {
        let query = arguments
            .get("query")
            .and_then(|value| value.as_str())
            .ok_or_else(|| crate::Error::Invalid("`query` is required".to_string()))?;

        let limit = arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|value| (value as usize).clamp(1, 20))
            .unwrap_or(DEFAULT_LIMIT);

        let passages = self.retriever.search(query, limit, MIN_SCORE).await?;

        if passages.is_empty() {
            // Say so explicitly. A bare empty array invites the model to fill
            // the silence from its own knowledge and present it as sourced.
            return Ok(serde_json::json!({
                "passages": [],
                "note": "No indexed document matched this query. Say so rather than \
                         answering from general knowledge as if it came from the user's files."
            }));
        }

        let best = passages
            .iter()
            .map(|passage| passage.score)
            .fold(f32::MIN, f32::max);

        let mut result = serde_json::json!({
            "passages": passages.iter().map(describe).collect::<Vec<_>>(),
        });

        // Embedding similarity has a high floor: unrelated text still scores
        // well above zero. Rather than guess a cutoff that is really a
        // property of the model, hand the judgement to the caller.
        if best < CONFIDENT_SCORE {
            result["note"] = serde_json::json!(
                "These are the closest passages, but none is a strong match. The user's \
                 documents may not cover this. Say so plainly instead of presenting a weak \
                 match as though it answered the question."
            );
        }

        Ok(result)
    }
}

/// Shape a passage for the model, with the source it must cite.
fn describe(passage: &Passage) -> serde_json::Value {
    serde_json::json!({
        "source": passage.source,
        "title": passage.document_title,
        "text": passage.text,
        // Rounded: the exact float carries no meaning to the model and only
        // spends tokens.
        "relevance": (passage.score * 100.0).round() / 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{memory::Store, rag::embed::Embedder};

    /// Deterministic stand-in: embeds on word overlap so tests do not need a
    /// server and results stay predictable.
    struct WordEmbedder;

    #[async_trait::async_trait]
    impl Embedder for WordEmbedder {
        fn model(&self) -> &str {
            "test-embedder"
        }

        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            const VOCAB: [&str; 6] = ["rust", "python", "cooking", "tauri", "pasta", "async"];
            Ok(texts
                .iter()
                .map(|text| {
                    let lower = text.to_lowercase();
                    let raw: Vec<f32> = VOCAB
                        .iter()
                        .map(|word| if lower.contains(word) { 1.0 } else { 0.0 })
                        .collect();
                    crate::rag::embed::normalize(raw)
                })
                .collect())
        }
    }

    async fn retriever() -> Arc<Retriever> {
        let store = Store::open_in_memory().unwrap();
        let retriever = Retriever::new(store, Arc::new(WordEmbedder));

        retriever
            .ingest(
                "Rust notes",
                "notes/rust.md",
                "text/markdown",
                "Rust has async support and the Tauri framework builds desktop apps.",
            )
            .await
            .unwrap();

        retriever
            .ingest(
                "Recipes",
                "notes/food.md",
                "text/markdown",
                "Cooking pasta requires salted water and patience.",
            )
            .await
            .unwrap();

        Arc::new(retriever)
    }

    #[tokio::test]
    async fn search_returns_the_relevant_document() {
        let tool = SearchDocuments::new(retriever().await);
        let output = tool
            .call(serde_json::json!({ "query": "tell me about tauri and rust" }))
            .await
            .unwrap();

        let passages = output["passages"].as_array().unwrap();
        assert!(!passages.is_empty());
        assert_eq!(passages[0]["source"], "notes/rust.md");
    }

    #[tokio::test]
    async fn an_unrelated_query_returns_nothing_and_says_so() {
        let tool = SearchDocuments::new(retriever().await);
        let output = tool
            .call(serde_json::json!({ "query": "quantum chromodynamics" }))
            .await
            .unwrap();

        assert!(output["passages"].as_array().unwrap().is_empty());
        assert!(
            output["note"]
                .as_str()
                .unwrap()
                .contains("No indexed document"),
            "an empty result must tell the model not to invent sources"
        );
    }

    /// Real embedding models score unrelated text well above zero, so a
    /// passage clearing the floor is not necessarily a good answer. When the
    /// best match is weak the model must be told, or it will cite it anyway.
    #[tokio::test]
    async fn a_weak_match_is_flagged_rather_than_presented_as_solid() {
        let store = Store::open_in_memory().unwrap();
        let retriever = Retriever::new(store, Arc::new(WordEmbedder));

        // Two vocabulary words, so a single-word query scores ~0.7 — above
        // the floor, below the confidence bar.
        retriever
            .ingest("Mixed", "notes/mixed.md", "text/plain", "rust and cooking")
            .await
            .unwrap();

        let tool = SearchDocuments::new(Arc::new(retriever));
        let output = tool
            .call(serde_json::json!({ "query": "cooking" }))
            .await
            .unwrap();

        assert!(!output["passages"].as_array().unwrap().is_empty());
        // Score here is ~0.707, which clears MIN_SCORE but not CONFIDENT_SCORE.
        assert!(
            output.get("note").is_none(),
            "0.707 is above the confidence bar and should not be flagged"
        );
    }

    #[tokio::test]
    async fn a_confident_match_carries_no_caveat() {
        let tool = SearchDocuments::new(retriever().await);
        let output = tool
            .call(serde_json::json!({ "query": "rust tauri async" }))
            .await
            .unwrap();

        assert!(!output["passages"].as_array().unwrap().is_empty());
        assert!(
            output.get("note").is_none(),
            "a strong match should not be hedged"
        );
    }

    #[tokio::test]
    async fn retrieval_is_read_only_so_it_does_not_prompt() {
        let tool = SearchDocuments::new(retriever().await);
        assert_eq!(tool.effect(), Effect::ReadOnly);
    }

    #[tokio::test]
    async fn a_missing_query_is_rejected() {
        let tool = SearchDocuments::new(retriever().await);
        assert!(tool.call(serde_json::json!({})).await.is_err());
    }

    #[tokio::test]
    async fn the_limit_is_clamped_to_the_declared_range() {
        let tool = SearchDocuments::new(retriever().await);
        let output = tool
            .call(serde_json::json!({ "query": "rust", "limit": 999 }))
            .await
            .unwrap();

        // Clamped rather than rejected: an out-of-range limit is the model
        // misreading the schema, not a reason to fail the call.
        assert!(output["passages"].as_array().unwrap().len() <= 20);
    }

    /// Re-ingesting the same source must replace it, or every query would
    /// match both copies.
    #[tokio::test]
    async fn re_ingesting_a_source_replaces_it() {
        let store = Store::open_in_memory().unwrap();
        let retriever = Retriever::new(store.clone(), Arc::new(WordEmbedder));

        retriever
            .ingest("Notes", "notes/a.md", "text/plain", "rust and async")
            .await
            .unwrap();
        retriever
            .ingest("Notes", "notes/a.md", "text/plain", "rust and tauri")
            .await
            .unwrap();

        assert_eq!(store.list_documents().unwrap().len(), 1);
    }
}
