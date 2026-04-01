use anyhow::{Context, Result};

use crate::config::ExtractionConfig;
use crate::graph::{Entity, Relationship};

// ─── Public API ──────────────────────────────────────────────────────────────

/// Result of entity extraction from a single document.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ExtractionResult {
    #[serde(default)]
    pub entities: Vec<Entity>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
}

/// Pluggable LLM-backed entity extractor.
pub struct Extractor {
    config: ExtractionConfig,
    /// Resolved API key (already env-expanded).
    api_key: Option<String>,
    /// Resolved base URL (used by Ollama).
    base_url: Option<String>,
    client: reqwest::Client,
}

impl Extractor {
    /// Create an Extractor.
    ///
    /// `api_key` and `base_url` should already be resolved from the provider
    /// config (see `Config::resolve_api_key` / `Config::resolve_base_url`).
    pub fn new(config: &ExtractionConfig, api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            config: config.clone(),
            api_key,
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Extract entities and relationships from `content`.
    pub async fn extract(&self, content: &str, file_path: &str) -> Result<ExtractionResult> {
        let prompt = build_prompt(content, file_path);
        match self.config.provider.as_str() {
            "gemini" => self.extract_gemini(&prompt).await,
            "openai" => self.extract_openai(&prompt).await,
            "ollama" => self.extract_ollama(&prompt).await,
            p => anyhow::bail!("Unknown extraction provider: {}", p),
        }
    }
}

// ─── Provider implementations ────────────────────────────────────────────────

impl Extractor {
    async fn extract_gemini(&self, prompt: &str) -> Result<ExtractionResult> {
        let api_key = self.require_api_key()?;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.config.model, api_key
        );
        let body = serde_json::json!({
            "contents": [{"parts": [{"text": prompt}]}],
            "generationConfig": {"responseMimeType": "application/json"}
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini extraction request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Gemini response")?;
        let text = json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("{}");
        parse_extraction_result(text)
    }

    async fn extract_openai(&self, prompt: &str) -> Result<ExtractionResult> {
        let api_key = self.require_api_key()?;
        let url = "https://api.openai.com/v1/chat/completions";
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [{"role": "user", "content": prompt}],
            "response_format": {"type": "json_object"}
        });

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .context("OpenAI extraction request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse OpenAI response")?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("{}");
        parse_extraction_result(text)
    }

    async fn extract_ollama(&self, prompt: &str) -> Result<ExtractionResult> {
        let base_url = self
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:11434");
        let url = format!("{}/api/generate", base_url);
        let body = serde_json::json!({
            "model": self.config.model,
            "prompt": prompt,
            "stream": false,
            "format": "json"
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Ollama extraction request failed")?;

        let json: serde_json::Value = resp.json().await.context("Failed to parse Ollama response")?;
        let text = json["response"].as_str().unwrap_or("{}");
        parse_extraction_result(text)
    }

    fn require_api_key(&self) -> Result<&str> {
        self.api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No api_key configured for extraction provider '{}'. \
                     Set it under [providers.{}] in brainjar.toml.",
                    self.config.provider,
                    self.config.provider
                )
            })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_prompt(content: &str, file_path: &str) -> String {
    format!(
        r#"Extract entities and relationships from this document.

Entity types: person, project, service, tool, config, decision, concept
Relationship types: depends_on, decided_by, deployed_to, relates_to, replaces, configures, uses, created_by

Return valid JSON only, no markdown fences:
{{"entities": [{{"name": "...", "type": "...", "description": "..."}}], "relationships": [{{"source": "...", "target": "...", "relation": "...", "description": "..."}}]}}

Document ({file_path}):
---
{content}
---"#
    )
}

fn parse_extraction_result(text: &str) -> Result<ExtractionResult> {
    let clean = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let result: ExtractionResult = serde_json::from_str(clean).unwrap_or_else(|_| ExtractionResult {
        entities: vec![],
        relationships: vec![],
    });

    Ok(result)
}
