//! Retrieval-augmented generation: ingest documents, retrieve passages.
//!
//! # Storage
//!
//! Vectors live in the same SQLite database as everything else and are
//! compared by brute force. LanceDB — what the architecture doc originally
//! called for — nearly tripled the dependency tree (753 to 2157 crates,
//! pulling in Arrow and DataFusion) for a workspace that will hold thousands
//! of chunks, not millions.
//!
//! Scanning N vectors of d dimensions is N·d multiply-adds: ~1ms for 1,000
//! chunks at 768 dimensions, ~10ms for 10,000. Past roughly 100,000 chunks
//! this stops being reasonable and wants a real index. [`Retriever`] is narrow
//! enough to swap at that point without touching callers.

pub mod chunk;
pub mod embed;
pub mod tool;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    Result,
    memory::Store,
    rag::embed::{Embedder, similarity},
};

/// An ingested document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: String,
    pub title: String,
    pub source: String,
    pub mime_type: String,
    pub byte_count: i64,
    pub created_at: i64,
    /// How many chunks this document produced.
    #[serde(default)]
    pub chunk_count: i64,
}

/// A retrieved passage and how well it matched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Passage {
    pub document_id: String,
    pub document_title: String,
    pub source: String,
    pub text: String,
    pub seq: i64,
    /// Cosine similarity in [-1, 1]; higher is closer.
    pub score: f32,
}

/// Ingests documents and retrieves passages.
pub struct Retriever {
    store: Store,
    embedder: Arc<dyn Embedder>,
}

impl Retriever {
    pub fn new(store: Store, embedder: Arc<dyn Embedder>) -> Self {
        Self { store, embedder }
    }

    /// Chunk, embed and store a document, replacing any earlier version.
    ///
    /// Returns the number of chunks stored.
    pub async fn ingest(
        &self,
        title: &str,
        source: &str,
        mime_type: &str,
        text: &str,
    ) -> Result<usize> {
        let chunks = chunk::split(text);
        if chunks.is_empty() {
            return Err(crate::Error::Invalid(
                "the document has no text to index".to_string(),
            ));
        }

        let texts: Vec<String> = chunks.iter().map(|chunk| chunk.text.clone()).collect();
        let embeddings = self.embedder.embed(&texts).await?;

        if embeddings.len() != chunks.len() {
            return Err(crate::Error::Transport(format!(
                "embedded {} of {} chunks",
                embeddings.len(),
                chunks.len()
            )));
        }

        // Re-ingesting the same source replaces it rather than accumulating
        // duplicates that would all match the same query.
        self.store.delete_document_by_source(source)?;

        let document = Document {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            source: source.to_string(),
            mime_type: mime_type.to_string(),
            byte_count: text.len() as i64,
            created_at: 0,
            chunk_count: chunks.len() as i64,
        };

        self.store
            .insert_document(&document, &chunks, &embeddings, self.embedder.model())?;

        Ok(chunks.len())
    }

    /// Find the `limit` passages closest to `query`.
    ///
    /// `min_score` filters weak matches: without it a query unrelated to the
    /// corpus still returns the least-unrelated chunks, which reads as the
    /// model inventing sources.
    pub async fn search(&self, query: &str, limit: usize, min_score: f32) -> Result<Vec<Passage>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let embedded = self.embedder.embed(&[query.to_string()]).await?;
        let Some(vector) = embedded.into_iter().next() else {
            return Ok(Vec::new());
        };

        // Only compare against vectors from the same model.
        let candidates = self.store.load_chunks_for_model(self.embedder.model())?;

        let mut scored: Vec<Passage> = candidates
            .into_iter()
            .map(|candidate| {
                let score = similarity(&vector, &candidate.embedding);
                Passage {
                    document_id: candidate.document_id,
                    document_title: candidate.document_title,
                    source: candidate.source,
                    text: candidate.text,
                    seq: candidate.seq,
                    score,
                }
            })
            .filter(|passage| passage.score >= min_score)
            .collect();

        // Descending by score; NaN cannot occur since inputs are normalized,
        // but total_cmp avoids relying on that.
        scored.sort_by(|left, right| right.score.total_cmp(&left.score));
        scored.truncate(limit);

        Ok(scored)
    }

    pub fn embedder_model(&self) -> &str {
        self.embedder.model()
    }
}

/// A stored chunk with its vector, as loaded for scoring.
#[derive(Debug, Clone)]
pub struct StoredChunk {
    pub document_id: String,
    pub document_title: String,
    pub source: String,
    pub text: String,
    pub seq: i64,
    pub embedding: Vec<f32>,
}

/// Pack a vector into little-endian bytes for storage.
pub fn encode_embedding(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Unpack a stored vector.
///
/// Trailing bytes that do not form a whole f32 are ignored rather than
/// panicking — a truncated row should degrade, not crash a search.
pub fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|quad| f32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_round_trip_through_bytes() {
        let original = vec![0.5, -0.25, 1.0, 0.0];
        let decoded = decode_embedding(&encode_embedding(&original));
        assert_eq!(decoded, original);
    }

    #[test]
    fn an_empty_vector_round_trips() {
        assert!(decode_embedding(&encode_embedding(&[])).is_empty());
    }

    /// A truncated BLOB must not panic a search.
    #[test]
    fn trailing_bytes_are_ignored() {
        let mut bytes = encode_embedding(&[1.0, 2.0]);
        bytes.push(0xff);
        assert_eq!(decode_embedding(&bytes).len(), 2);
    }

    #[test]
    fn negative_and_fractional_values_survive() {
        let original = vec![-0.123_456_79, 0.987_654_3, -1.0];
        assert_eq!(decode_embedding(&encode_embedding(&original)), original);
    }
}
