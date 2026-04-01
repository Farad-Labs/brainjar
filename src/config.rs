use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Per-provider credentials (api_key, base_url).
    /// Keys: "gemini", "openai", "ollama", etc.
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub knowledge_bases: HashMap<String, KnowledgeBaseConfig>,
    pub embeddings: Option<EmbeddingConfig>,
    pub extraction: Option<ExtractionConfig>,
    /// Path to the config file (not serialized)
    #[serde(skip)]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBaseConfig {
    pub watch_paths: Vec<String>,
    #[serde(default)]
    pub auto_sync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String, // "gemini", "openai", "ollama"
    pub model: String,
    /// Backward-compat: api_key directly on embeddings section.
    /// Prefer [providers.<name>].api_key over this.
    pub api_key: Option<String>,
    /// Backward-compat: base_url directly on embeddings section.
    pub base_url: Option<String>,
    pub dimensions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    pub provider: String,
    pub model: String,
    /// Backward-compat: api_key directly on extraction section.
    pub api_key: Option<String>,
    /// Backward-compat: base_url directly on extraction section.
    pub base_url: Option<String>,
    pub enabled: bool,
}

pub fn load_config(config_path: Option<&str>) -> Result<Config> {
    let path = if let Some(p) = config_path {
        PathBuf::from(p)
    } else {
        find_config()?
    };

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config file: {}\n\nRun `brainjar init` to create a new config.", path.display()))?;

    let mut config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

    config.config_dir = path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    Ok(config)
}

fn find_config() -> Result<PathBuf> {
    // Check current directory and parents
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = dir.join("brainjar.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }

    // Check global config
    if let Some(config_dir) = dirs::config_dir() {
        let global = config_dir.join("brainjar").join("config.toml");
        if global.exists() {
            return Ok(global);
        }
    }

    anyhow::bail!(
        "No brainjar.toml found in current directory or ancestors, and no global config at ~/.config/brainjar/config.toml.\n\nRun `brainjar init` to create a new config."
    )
}

impl Config {
    /// Resolve the API key for a given provider name.
    /// Prefers `[providers.<name>].api_key`, falls back to the supplied
    /// `legacy_key` (the `api_key` field on `[embeddings]` / `[extraction]`).
    pub fn resolve_api_key(&self, provider: &str, legacy_key: Option<&str>) -> Option<String> {
        // Prefer providers section
        if let Some(p) = self.providers.get(provider)
            && p.api_key.is_some() {
                return p.api_key.as_ref().map(|k| expand_env_var(k));
            }
        // Fall back to legacy inline key
        legacy_key.map(expand_env_var)
    }

    /// Resolve the base_url for a given provider name.
    pub fn resolve_base_url(&self, provider: &str, legacy_url: Option<&str>) -> Option<String> {
        if let Some(p) = self.providers.get(provider)
            && p.base_url.is_some() {
                return p.base_url.clone();
            }
        legacy_url.map(|s| s.to_string())
    }

    /// Expand watch paths relative to the config dir, with ~ support
    pub fn expand_watch_paths(&self, kb: &KnowledgeBaseConfig) -> Vec<PathBuf> {
        kb.watch_paths
            .iter()
            .map(|p| self.expand_path(p))
            .collect()
    }

    pub fn expand_path(&self, p: &str) -> PathBuf {
        if p.starts_with('~')
            && let Some(home) = dirs::home_dir() {
                return home.join(&p[2..]);
            }
        if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            self.config_dir.join(p)
        }
    }
}

/// Expand `${VAR_NAME}` env-var references in an API key value.
fn expand_env_var(key: &str) -> String {
    if key.starts_with("${") && key.ends_with('}') {
        let var_name = &key[2..key.len() - 1];
        std::env::var(var_name).unwrap_or_default()
    } else {
        key.to_string()
    }
}
