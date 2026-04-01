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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_extraction_config(provider: &str) -> ExtractionConfig {
        ExtractionConfig {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            api_key: None,
            base_url: None,
            enabled: true,
        }
    }

    #[test]
    fn test_extractor_creation() {
        let cfg = make_extraction_config("gemini");
        let extractor = Extractor::new(&cfg, Some("key".to_string()), None);
        assert_eq!(extractor.config.provider, "gemini");
    }

    #[test]
    fn test_extractor_requires_api_key_when_missing() {
        let cfg = make_extraction_config("openai");
        let extractor = Extractor::new(&cfg, None, None);
        let result = extractor.require_api_key();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("openai"));
    }

    #[test]
    fn test_extractor_requires_api_key_when_empty_string() {
        let cfg = make_extraction_config("gemini");
        let extractor = Extractor::new(&cfg, Some("".to_string()), None);
        assert!(extractor.require_api_key().is_err());
    }

    #[test]
    fn test_extractor_api_key_ok() {
        let cfg = make_extraction_config("gemini");
        let extractor = Extractor::new(&cfg, Some("real-key".to_string()), None);
        assert_eq!(extractor.require_api_key().unwrap(), "real-key");
    }

    #[test]
    fn test_build_prompt_contains_content() {
        let content = "This document talks about Rust and SQLite.";
        let file_path = "notes/rust.md";
        let prompt = build_prompt(content, file_path);
        assert!(prompt.contains(content));
        assert!(prompt.contains(file_path));
    }

    #[test]
    fn test_build_prompt_contains_entity_types() {
        let prompt = build_prompt("content", "file.md");
        assert!(prompt.contains("person"));
        assert!(prompt.contains("project"));
        assert!(prompt.contains("tool"));
    }

    #[test]
    fn test_build_prompt_contains_relationship_types() {
        let prompt = build_prompt("content", "file.md");
        assert!(prompt.contains("depends_on"));
        assert!(prompt.contains("relates_to"));
    }

    #[test]
    fn test_build_prompt_instructs_json_only() {
        let prompt = build_prompt("content", "file.md");
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_parse_extraction_result_valid_json() {
        let json = r#"{"entities": [{"name": "Rust", "type": "tool", "description": "A language"}], "relationships": []}"#;
        let result = parse_extraction_result(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "Rust");
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn test_parse_extraction_result_strips_markdown_fences() {
        let json = "```json\n{\"entities\": [], \"relationships\": []}\n```";
        let result = parse_extraction_result(json).unwrap();
        assert!(result.entities.is_empty());
    }

    #[test]
    fn test_parse_extraction_result_invalid_json_returns_empty() {
        let result = parse_extraction_result("not valid json at all").unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn test_parse_extraction_result_empty_string() {
        let result = parse_extraction_result("").unwrap();
        assert!(result.entities.is_empty());
    }

    #[test]
    fn test_parse_extraction_result_with_relationships() {
        let json = r#"{
            "entities": [
                {"name": "Brainjar", "type": "project", "description": "AI memory"}
            ],
            "relationships": [
                {"source": "Brainjar", "target": "SQLite", "relation": "uses", "description": "stores data"}
            ]
        }"#;
        let result = parse_extraction_result(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.relationships.len(), 1);
        assert_eq!(result.relationships[0].source, "Brainjar");
        assert_eq!(result.relationships[0].relation, "uses");
    }

    #[tokio::test]
    async fn test_extract_unknown_provider_errors() {
        let cfg = make_extraction_config("unknown_llm");
        let extractor = Extractor::new(&cfg, Some("key".to_string()), None);
        let result = extractor.extract("content", "file.md").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown extraction provider"));
    }
}
