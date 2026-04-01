use anyhow::{Context, Result};

use crate::config::EmbeddingConfig;

// ─── Public API ──────────────────────────────────────────────────────────────

/// Pluggable embedding provider.
pub struct Embedder {
    config: EmbeddingConfig,
    client: reqwest::Client,
}

impl Embedder {
    pub fn new(config: &EmbeddingConfig) -> Self {
        Self {
            config: config.clone(),
            client: reqwest::Client::new(),
        }
    }

    /// Embed a batch of texts and return one vector per input.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        match self.config.provider.as_str() {
            "gemini" => self.embed_gemini(texts).await,
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
    async fn embed_gemini(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let api_key = self.resolve_api_key()?;

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
                "content": {"parts": [{"text": text}]}
            });

            let resp = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("Gemini embedContent request failed")?;

            let json: serde_json::Value =
                resp.json().await.context("Failed to parse Gemini embed response")?;

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
        let api_key = self.resolve_api_key()?;
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
            .config
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

    fn resolve_api_key(&self) -> Result<String> {
        match &self.config.api_key {
            Some(key) if key.starts_with("${") && key.ends_with('}') => {
                let var_name = &key[2..key.len() - 1];
                std::env::var(var_name)
                    .with_context(|| format!("Env var {} not set (required by embedding config)", var_name))
            }
            Some(key) => Ok(key.clone()),
            None => anyhow::bail!(
                "No api_key configured for embedding provider '{}'",
                self.config.provider
            ),
        }
    }
}
