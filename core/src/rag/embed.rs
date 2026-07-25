//! Turning text into vectors.
//!
//! The OpenAI-compatible `/embeddings` endpoint covers LM Studio, Ollama and
//! OpenAI itself, which means embedding can run entirely on the user's machine
//! — the point of a local-first workspace.

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    providers::{LOCAL_PLACEHOLDER_KEY, ProviderConfig},
};

/// A unit-length vector.
///
/// Normalizing on the way in means similarity is a plain dot product, and no
/// caller can forget to do it.
pub type Embedding = Vec<f32>;

#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts, returning one vector per input in order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>>;

    /// Model identifier, recorded alongside stored vectors.
    ///
    /// Vectors from different models are not comparable, so a corpus embedded
    /// with one model must not be searched with another.
    fn model(&self) -> &str;
}

/// Scale a vector to unit length.
///
/// A zero vector is returned unchanged: dividing by its length would produce
/// NaNs that then poison every comparison.
pub fn normalize(mut vector: Embedding) -> Embedding {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude > f32::EPSILON {
        for value in &mut vector {
            *value /= magnitude;
        }
    }
    vector
}

/// Cosine similarity of two unit vectors, which is their dot product.
///
/// Mismatched lengths return 0 rather than panicking: a corpus embedded with a
/// different model should rank as unrelated, not crash the search.
pub fn similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    left.iter()
        .zip(right)
        .map(|(a, b)| a * b)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    /// Servers may return results out of order; this puts them back.
    #[serde(default)]
    index: usize,
}

pub struct OpenAiCompatEmbedder {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatEmbedder {
    pub fn new(
        config: &ProviderConfig,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Result<Self> {
        let base_url = config
            .effective_base_url()
            .ok_or_else(|| Error::ProviderMisconfigured {
                provider_id: config.id.clone(),
                reason: "base_url is required for embeddings".to_string(),
            })?
            .to_string();

        Ok(Self {
            http: reqwest::Client::builder()
                .build()
                .map_err(|error| Error::Transport(error.to_string()))?,
            base_url,
            api_key: api_key.unwrap_or_else(|| LOCAL_PLACEHOLDER_KEY.to_string()),
            model: model.into(),
        })
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAiCompatEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/embeddings", self.base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&EmbeddingRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(|error| {
                if error.is_connect() {
                    Error::Transport(format!(
                        "could not reach {} — is the embedding server running?",
                        self.base_url
                    ))
                } else {
                    Error::Transport(error.to_string())
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Transport(format!(
                "{url} returned {status}: {}",
                body.trim()
            )));
        }

        let mut body: EmbeddingResponse = response
            .json()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        if body.data.len() != texts.len() {
            return Err(Error::Transport(format!(
                "expected {} embeddings, got {}",
                texts.len(),
                body.data.len()
            )));
        }

        body.data.sort_by_key(|item| item.index);
        Ok(body
            .data
            .into_iter()
            .map(|item| normalize(item.embedding))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizing_gives_unit_length() {
        let unit = normalize(vec![3.0, 4.0]);
        let magnitude = unit.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 1e-6, "got {magnitude}");
    }

    /// Dividing by zero magnitude would produce NaNs that spread through
    /// every later comparison.
    #[test]
    fn a_zero_vector_survives_normalization() {
        let zero = normalize(vec![0.0, 0.0, 0.0]);
        assert!(zero.iter().all(|value| value.is_finite()));
        assert_eq!(zero, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn identical_vectors_are_maximally_similar() {
        let vector = normalize(vec![1.0, 2.0, 3.0]);
        assert!((similarity(&vector, &vector) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        let left = normalize(vec![1.0, 0.0]);
        let right = normalize(vec![0.0, 1.0]);
        assert!(similarity(&left, &right).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_score_minus_one() {
        let left = normalize(vec![1.0, 0.0]);
        let right = normalize(vec![-1.0, 0.0]);
        assert!((similarity(&left, &right) + 1.0).abs() < 1e-6);
    }

    /// Vectors from different models have different dimensions and are not
    /// comparable; scoring them as unrelated beats panicking.
    #[test]
    fn mismatched_dimensions_score_zero() {
        assert_eq!(similarity(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn similarity_stays_in_range() {
        // Floating point can push a dot product just outside [-1, 1].
        let value = similarity(&[1.0, 1.0, 1.0], &[1.0, 1.0, 1.0]);
        assert!((-1.0..=1.0).contains(&value), "got {value}");
    }
}
