use anyhow::{Context, Result};
use std::path::Path;

use crate::config::EmbeddingConfig;

/// Embedding task types for optimized retrieval.
/// Different models handle these differently:
/// - gemini-embedding-001: uses `taskType` API parameter
/// - gemini-embedding-2-preview: uses text prefix format (e.g., "task: search result | query: ...")
/// - openai (text-embedding-3-*): no task type support — task_type parameter is ignored
/// - ollama: no task type support — task_type parameter is ignored
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
    /// Returns the Gemini embedding-001 API task type string.
    pub fn as_gemini_v1_str(self) -> &'static str {
        match self {
            Self::RetrievalDocument => "RETRIEVAL_DOCUMENT",
            Self::RetrievalQuery => "RETRIEVAL_QUERY",
            Self::CodeRetrievalQuery => "CODE_RETRIEVAL_QUERY",
        }
    }
}

/// Returns true if the model uses text prefix format instead of taskType parameter.
/// gemini-embedding-2-preview (and future v2+ models) use prefix format.
fn is_gemini_v2_embedding(model: &str) -> bool {
    model.contains("embedding-2") || model.contains("embedding-3")
}

/// Format text with the appropriate task prefix for gemini-embedding-2.
/// See: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/embeddings/get-multimodal-embeddings
fn format_gemini_v2_text(text: &str, task_type: TaskType, title: Option<&str>) -> String {
    match task_type {
        TaskType::RetrievalDocument => {
            let t = title.unwrap_or("none");
            format!("title: {} | text: {}", t, text)
        }
        TaskType::RetrievalQuery => {
            format!("task: search result | query: {}", text)
        }
        TaskType::CodeRetrievalQuery => {
            format!("task: code retrieval | query: {}", text)
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

    /// Embed a batch of document texts with optional titles for better v2 embeddings.
    /// For gemini-embedding-2, titles are prepended as "title: X | text: Y".
    pub async fn embed_documents(&self, docs: &[(&str, Option<&str>)]) -> Result<Vec<Vec<f32>>> {
        if self.config.provider == "gemini" && is_gemini_v2_embedding(&self.config.model) {
            // For v2, format each text with its title prefix
            let formatted: Vec<String> = docs
                .iter()
                .map(|(text, title)| format_gemini_v2_text(text, TaskType::RetrievalDocument, *title))
                .collect();
            let refs: Vec<&str> = formatted.iter().map(|s| s.as_str()).collect();
            self.embed_gemini_raw(&refs).await
        } else {
            // For v1 / non-gemini, just use the text with task type
            let texts: Vec<&str> = docs.iter().map(|(text, _)| *text).collect();
            self.embed_batch_with_task(&texts, TaskType::RetrievalDocument).await
        }
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
        if is_gemini_v2_embedding(&self.config.model) {
            // v2: format with text prefix, no taskType parameter
            let formatted: Vec<String> = texts
                .iter()
                .map(|text| format_gemini_v2_text(text, task_type, None))
                .collect();
            let refs: Vec<&str> = formatted.iter().map(|s| s.as_str()).collect();
            self.embed_gemini_raw(&refs).await
        } else {
            // v1: use taskType parameter
            self.embed_gemini_v1(texts, task_type).await
        }
    }

    /// Send a single batchEmbedContents request and return one Vec<f32> per input.
    /// `requests` must be a JSON array of individual request objects.
    async fn batch_embed_gemini_request(&self, requests: serde_json::Value) -> Result<Vec<Vec<f32>>> {
        let api_key = self.require_api_key()?;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:batchEmbedContents?key={}",
            self.config.model, api_key
        );
        let body = serde_json::json!({ "requests": requests });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini batchEmbedContents request failed")?;

        let status = resp.status();
        let json: serde_json::Value =
            resp.json().await.context("Failed to parse Gemini batch embed response")?;

        if !status.is_success() {
            let err_msg = json["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            anyhow::bail!("Gemini API error ({}): {}", status, err_msg);
        }

        let embeddings = json["embeddings"]
            .as_array()
            .context("Gemini: missing embeddings array in batch response")?;

        embeddings
            .iter()
            .map(|emb| {
                let values = emb["values"]
                    .as_array()
                    .context("Gemini: missing values in batch embedding")?;
                Ok(values
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect())
            })
            .collect()
    }

    /// Gemini embedding-001 style: uses taskType API parameter.
    /// Sends up to 100 texts per batchEmbedContents request.
    async fn embed_gemini_v1(&self, texts: &[&str], task_type: TaskType) -> Result<Vec<Vec<f32>>> {
        let model = &self.config.model;
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch in texts.chunks(100) {
            let requests: Vec<serde_json::Value> = batch
                .iter()
                .map(|text| serde_json::json!({
                    "model": format!("models/{}", model),
                    "content": {"parts": [{"text": text}]},
                    "taskType": task_type.as_gemini_v1_str()
                }))
                .collect();

            let mut batch_embeddings =
                self.batch_embed_gemini_request(serde_json::Value::Array(requests)).await?;
            all_embeddings.append(&mut batch_embeddings);
        }

        Ok(all_embeddings)
    }

    /// Gemini embedding-2 style: text is already formatted with prefixes, no taskType param.
    /// Sends up to 100 texts per batchEmbedContents request.
    async fn embed_gemini_raw(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let model = &self.config.model;
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch in texts.chunks(100) {
            // No taskType for v2 — task is encoded in the text prefix
            let requests: Vec<serde_json::Value> = batch
                .iter()
                .map(|text| serde_json::json!({
                    "model": format!("models/{}", model),
                    "content": {"parts": [{"text": text}]}
                }))
                .collect();

            let mut batch_embeddings =
                self.batch_embed_gemini_request(serde_json::Value::Array(requests)).await?;
            all_embeddings.append(&mut batch_embeddings);
        }

        Ok(all_embeddings)
    }

    async fn embed_openai(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let api_key = self.require_api_key()?;
        let url = "https://api.openai.com/v1/embeddings";

        let mut body = serde_json::json!({
            "model": self.config.model,
            "input": texts,
        });

        // Pass dimensions if configured — text-embedding-3-* support Matryoshka
        // dimension reduction. Without this, the API returns the model's full
        // default dimensions regardless of what brainjar.toml specifies.
        if self.config.dimensions > 0 {
            body["dimensions"] = serde_json::json!(self.config.dimensions);
        }

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .context("OpenAI embeddings request failed")?;

        // Check HTTP status before attempting to parse the response body.
        // Without this, a 429 rate-limit response produces a confusing
        // "missing data array" error instead of a clear rate-limit message.
        let status = resp.status();
        let json: serde_json::Value =
            resp.json().await.context("Failed to parse OpenAI embed response")?;

        if !status.is_success() {
            let err_msg = json["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            anyhow::bail!("OpenAI API error ({}): {}", status, err_msg);
        }

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
    fn test_task_type_gemini_v1_strings() {
        assert_eq!(TaskType::RetrievalDocument.as_gemini_v1_str(), "RETRIEVAL_DOCUMENT");
        assert_eq!(TaskType::RetrievalQuery.as_gemini_v1_str(), "RETRIEVAL_QUERY");
        assert_eq!(TaskType::CodeRetrievalQuery.as_gemini_v1_str(), "CODE_RETRIEVAL_QUERY");
    }

    #[test]
    fn test_is_gemini_v2_embedding() {
        assert!(is_gemini_v2_embedding("gemini-embedding-2-preview"));
        assert!(is_gemini_v2_embedding("gemini-embedding-3-something"));
        assert!(!is_gemini_v2_embedding("gemini-embedding-001"));
        assert!(!is_gemini_v2_embedding("text-embedding-004"));
    }

    #[test]
    fn test_format_gemini_v2_document() {
        let result = format_gemini_v2_text("hello world", TaskType::RetrievalDocument, Some("My Doc"));
        assert_eq!(result, "title: My Doc | text: hello world");
    }

    #[test]
    fn test_format_gemini_v2_document_no_title() {
        let result = format_gemini_v2_text("hello world", TaskType::RetrievalDocument, None);
        assert_eq!(result, "title: none | text: hello world");
    }

    #[test]
    fn test_format_gemini_v2_query() {
        let result = format_gemini_v2_text("find me stuff", TaskType::RetrievalQuery, None);
        assert_eq!(result, "task: search result | query: find me stuff");
    }

    #[test]
    fn test_format_gemini_v2_code_query() {
        let result = format_gemini_v2_text("parse json", TaskType::CodeRetrievalQuery, None);
        assert_eq!(result, "task: code retrieval | query: parse json");
    }

    #[test]
    fn test_gemini_v1_batch_request_structure() {
        let texts = ["text1", "text2", "text3"];
        let model = "gemini-embedding-001";
        let task_type_str = TaskType::RetrievalDocument.as_gemini_v1_str();

        let requests: Vec<serde_json::Value> = texts
            .iter()
            .map(|text| serde_json::json!({
                "model": format!("models/{}", model),
                "content": {"parts": [{"text": text}]},
                "taskType": task_type_str
            }))
            .collect();
        let body = serde_json::json!({ "requests": requests });

        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0]["model"], "models/gemini-embedding-001");
        assert_eq!(reqs[0]["taskType"], "RETRIEVAL_DOCUMENT");
        assert_eq!(reqs[0]["content"]["parts"][0]["text"], "text1");
        assert_eq!(reqs[2]["content"]["parts"][0]["text"], "text3");
    }

    #[test]
    fn test_gemini_raw_batch_request_structure() {
        let texts = ["title: Doc | text: hello", "title: Other | text: world"];
        let model = "gemini-embedding-2-preview";

        let requests: Vec<serde_json::Value> = texts
            .iter()
            .map(|text| serde_json::json!({
                "model": format!("models/{}", model),
                "content": {"parts": [{"text": text}]}
            }))
            .collect();
        let body = serde_json::json!({ "requests": requests });

        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0]["model"], "models/gemini-embedding-2-preview");
        // No taskType field for raw/v2 requests
        assert!(reqs[0].get("taskType").is_none() || reqs[0]["taskType"].is_null());
        assert_eq!(reqs[0]["content"]["parts"][0]["text"], "title: Doc | text: hello");
        assert_eq!(reqs[1]["content"]["parts"][0]["text"], "title: Other | text: world");
    }

    #[test]
    fn test_gemini_batch_chunks_at_100() {
        // Verify that .chunks(100) correctly partitions a 101-element slice
        let texts: Vec<&str> = (0..101).map(|_| "x").collect();
        let mut batch_sizes = Vec::new();
        for batch in texts.chunks(100) {
            batch_sizes.push(batch.len());
        }
        assert_eq!(batch_sizes, vec![100, 1]);
    }
}
