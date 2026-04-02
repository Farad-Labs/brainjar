use anyhow::{Context, Result};
use std::path::Path;

use crate::config::EmbeddingConfig;

/// Gemini embedding task types for optimized retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// Embedding stored documents for retrieval
    RetrievalDocument,
    /// Embedding a search query
    RetrievalQuery,
    /// Embedding a code-related search query
    CodeRetrievalQuery,
}

impl TaskType {
    /// Returns the Gemini API task type string.
    pub fn as_gemini_str(self) -> &'static str {
        match self {
            Self::RetrievalDocument => "RETRIEVAL_DOCUMENT",
            Self::RetrievalQuery => "RETRIEVAL_QUERY",
            Self::CodeRetrievalQuery => "CODE_RETRIEVAL_QUERY",
        }
    }
}

/// Code file extensions that indicate source code content.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "c", "cpp", "h",
    "cs", "rb", "swift", "zig", "lua", "sh", "bash", "zsh", "ps1",
    "sql", "toml", "yaml", "yml", "json", "xml", "html", "css", "scss",
];

/// Determine the embedding task type for a document based on file extension.
/// Code files are embedded as RETRIEVAL_DOCUMENT (same as other docs — the
/// distinction matters at query time, not index time).
pub fn task_type_for_document(_path: &str) -> TaskType {
    TaskType::RetrievalDocument
}

/// Determine the embedding task type for a search query.
/// If the KB contains code files, uses CODE_RETRIEVAL_QUERY for better
/// code-aware semantic matching.
pub fn task_type_for_query(paths: &[String]) -> TaskType {
    let has_code = paths.iter().any(|p| {
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| CODE_EXTENSIONS.contains(&ext))
    });
    if has_code {
        TaskType::CodeRetrievalQuery
    } else {
        TaskType::RetrievalQuery
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Pluggable embedding provider.
pub struct Embedder {
    config: EmbeddingConfig,
    /// Resolved API key (already env-expanded, if applicable).
    api_key: Option<String>,
    /// Resolved base URL (used by Ollama).
    base_url: Option<String>,
    client: reqwest::Client,
}

impl Embedder {
    /// Create an Embedder.
    ///
    /// `api_key` and `base_url` should already be resolved from the provider
    /// config (see `Config::resolve_api_key` / `Config::resolve_base_url`).
    pub fn new(config: &EmbeddingConfig, api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            config: config.clone(),
            api_key,
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Embed a batch of texts and return one vector per input.
    /// Uses RETRIEVAL_DOCUMENT task type (for indexing).
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch_with_task(texts, TaskType::RetrievalDocument).await
    }

    /// Embed a batch of texts with a specific task type.
    pub async fn embed_batch_with_task(&self, texts: &[&str], task_type: TaskType) -> Result<Vec<Vec<f32>>> {
        match self.config.provider.as_str() {
            "gemini" => self.embed_gemini(texts, task_type).await,
            "openai" => self.embed_openai(texts).await,
            "ollama" => self.embed_ollama(texts).await,
            p => anyhow::bail!("Unknown embedding provider: {}", p),
        }
    }

    /// The dimensionality of the embeddings produced by this provider.
    pub fn dimensions(&self) -> usize {
        self.config.dimensions
    }
}

// ─── Provider implementations ────────────────────────────────────────────────

impl Embedder {
    async fn embed_gemini(&self, texts: &[&str], task_type: TaskType) -> Result<Vec<Vec<f32>>> {
        let api_key = self.require_api_key()?;

        let mut all_embeddings = Vec::with_capacity(texts.len());

        // Gemini embedContent works one text at a time (batchEmbedContents also
        // exists but for simplicity we loop here — fine for Phase 2).
        for text in texts {
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:embedContent?key={}",
                self.config.model, api_key
            );
            let body = serde_json::json!({
                "model": format!("models/{}", self.config.model),
                "content": {"parts": [{"text": text}]},
                "taskType": task_type.as_gemini_str()
            });

            let resp = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("Gemini embedContent request failed")?;

            let status = resp.status();
            let json: serde_json::Value =
                resp.json().await.context("Failed to parse Gemini embed response")?;

            if !status.is_success() {
                let err_msg = json["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error");
                anyhow::bail!("Gemini API error ({}): {}", status, err_msg);
            }

            let values = json["embedding"]["values"]
                .as_array()
                .context("Gemini: missing embedding.values in response")?;

            let embedding: Vec<f32> = values
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            all_embeddings.push(embedding);
        }

        Ok(all_embeddings)
    }

    async fn embed_openai(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let api_key = self.require_api_key()?;
        let url = "https://api.openai.com/v1/embeddings";

        let body = serde_json::json!({
            "model": self.config.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .context("OpenAI embeddings request failed")?;

        let json: serde_json::Value =
            resp.json().await.context("Failed to parse OpenAI embed response")?;

        let data = json["data"]
            .as_array()
            .context("OpenAI: missing data array in response")?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let values = item["embedding"]
                .as_array()
                .context("OpenAI: missing embedding array for item")?;
            let vec: Vec<f32> = values
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            embeddings.push(vec);
        }

        Ok(embeddings)
    }

    async fn embed_ollama(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let base_url = self
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:11434");
        let url = format!("{}/api/embed", base_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "input": texts,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Ollama embed request failed")?;

        let json: serde_json::Value =
            resp.json().await.context("Failed to parse Ollama embed response")?;

        let embeddings_raw = json["embeddings"]
            .as_array()
            .context("Ollama: missing embeddings array in response")?;

        let mut result = Vec::with_capacity(embeddings_raw.len());
        for item in embeddings_raw {
            let vec: Vec<f32> = item
                .as_array()
                .context("Ollama: embedding item is not an array")?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            result.push(vec);
        }

        Ok(result)
    }

    fn require_api_key(&self) -> Result<&str> {
        self.api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No api_key configured for embedding provider '{}'. \
                     Set it under [providers.{}] in brainjar.toml.",
                    self.config.provider,
                    self.config.provider
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(provider: &str, model: &str, dims: usize) -> EmbeddingConfig {
        EmbeddingConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: None,
            base_url: None,
            dimensions: dims,
        }
    }

    #[test]
    fn test_embedder_creation_gemini() {
        let cfg = make_config("gemini", "gemini-embedding-001", 3072);
        let embedder = Embedder::new(&cfg, Some("fake-key".to_string()), None);
        assert_eq!(embedder.dimensions(), 3072);
    }

    #[test]
    fn test_embedder_creation_openai() {
        let cfg = make_config("openai", "text-embedding-3-small", 1536);
        let embedder = Embedder::new(&cfg, Some("sk-fake".to_string()), None);
        assert_eq!(embedder.dimensions(), 1536);
    }

    #[test]
    fn test_embedder_creation_ollama() {
        let cfg = make_config("ollama", "nomic-embed-text", 384);
        let embedder = Embedder::new(&cfg, None, Some("http://localhost:11434".to_string()));
        assert_eq!(embedder.dimensions(), 384);
    }

    #[test]
    fn test_embedder_requires_api_key_when_missing() {
        let cfg = make_config("gemini", "gemini-embedding-001", 3072);
        let embedder = Embedder::new(&cfg, None, None);
        let result = embedder.require_api_key();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("gemini"));
    }

    #[test]
    fn test_embedder_requires_api_key_when_empty() {
        let cfg = make_config("openai", "text-embedding-3-small", 1536);
        let embedder = Embedder::new(&cfg, Some("".to_string()), None);
        let result = embedder.require_api_key();
        assert!(result.is_err());
    }

    #[test]
    fn test_embedder_api_key_present() {
        let cfg = make_config("openai", "text-embedding-3-small", 1536);
        let embedder = Embedder::new(&cfg, Some("sk-real-key".to_string()), None);
        let key = embedder.require_api_key().unwrap();
        assert_eq!(key, "sk-real-key");
    }

    #[test]
    fn test_embedder_dimensions_zero_allowed() {
        let cfg = make_config("ollama", "nomic-embed-text", 0);
        let embedder = Embedder::new(&cfg, None, None);
        assert_eq!(embedder.dimensions(), 0);
    }

    #[tokio::test]
    async fn test_embed_batch_unknown_provider_errors() {
        let cfg = make_config("unknown_provider", "some-model", 128);
        let embedder = Embedder::new(&cfg, Some("key".to_string()), None);
        let result = embedder.embed_batch(&["hello"]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown embedding provider"));
    }

    #[test]
    fn test_task_type_for_query_with_code() {
        let paths = vec!["main.rs".to_string(), "README.md".to_string()];
        assert_eq!(task_type_for_query(&paths), TaskType::CodeRetrievalQuery);
    }

    #[test]
    fn test_task_type_for_query_without_code() {
        let paths = vec!["notes.md".to_string(), "plan.txt".to_string()];
        assert_eq!(task_type_for_query(&paths), TaskType::RetrievalQuery);
    }

    #[test]
    fn test_task_type_gemini_strings() {
        assert_eq!(TaskType::RetrievalDocument.as_gemini_str(), "RETRIEVAL_DOCUMENT");
        assert_eq!(TaskType::RetrievalQuery.as_gemini_str(), "RETRIEVAL_QUERY");
        assert_eq!(TaskType::CodeRetrievalQuery.as_gemini_str(), "CODE_RETRIEVAL_QUERY");
    }
}
