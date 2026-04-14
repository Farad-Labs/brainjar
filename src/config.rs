use crate::tuning::TuningParams;
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
    /// Where brainjar stores its databases.
    /// Defaults to `~/.brainjar` when not set.
    pub data_dir: Option<String>,
    /// Path to the config file (not serialized)
    #[serde(skip)]
    pub config_dir: PathBuf,
    /// Watch mode configuration
    pub watch: Option<WatchConfig>,
    /// Tunable scoring parameters (optional `[tuning]` section).
    /// Falls back to hardcoded defaults when the section is absent.
    #[serde(default)]
    pub tuning: TuningParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    pub horizon_days: u32,
    #[serde(default = "default_floor")]
    pub floor: f64,
    #[serde(default = "default_shape")]
    pub shape: f64,
}

fn default_floor() -> f64 { 0.3 }
fn default_shape() -> f64 { 1.0 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum KbType {
    #[default]
    Docs,
    Code,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderConfig {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub weight_boost: f64,
    #[serde(default)]
    pub decay: Option<DecayConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBaseConfig {
    #[serde(default, rename = "type")]
    pub kb_type: KbType,
    #[serde(default)]
    pub watch_paths: Vec<String>,
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub description: Option<String>,
}

impl KnowledgeBaseConfig {
    /// Returns the effective list of folders to watch.
    /// If `folders` is non-empty, returns those directly.
    /// If only `watch_paths` are set (legacy), converts each to a default `FolderConfig`.
    pub fn effective_folders(&self) -> Vec<FolderConfig> {
        if !self.folders.is_empty() {
            self.folders.clone()
        } else {
            self.watch_paths
                .iter()
                .map(|p| FolderConfig {
                    path: p.clone(),
                    title: None,
                    weight_boost: 0.0,
                    decay: None,
                })
                .collect()
        }
    }
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WatchConfig {
    /// Polling interval in seconds (default: 300)
    pub interval: Option<u64>,
}

pub fn load_config(config_path: Option<&str>) -> Result<Config> {
    let path = if let Some(p) = config_path {
        let raw = PathBuf::from(p);
        raw.canonicalize().unwrap_or(raw)
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

    // Check ~/.brainjar/brainjar.toml
    if let Some(home) = dirs::home_dir() {
        let home_config = home.join(".brainjar").join("brainjar.toml");
        if home_config.exists() {
            return Ok(home_config);
        }
    }

    anyhow::bail!(
        "No brainjar.toml found. Checked: current directory (and ancestors), ~/.brainjar/.\n\nRun `brainjar init` to create a new config."
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

    /// Expand folder paths relative to the config dir, with ~ support.
    /// Returns `(absolute_path, folder_config)` pairs.
    pub fn expand_watch_paths(&self, kb: &KnowledgeBaseConfig) -> Vec<(PathBuf, FolderConfig)> {
        kb.effective_folders()
            .into_iter()
            .map(|f| {
                let path = self.expand_path(&f.path);
                (path, f)
            })
            .collect()
    }

    /// Return the directory where KB databases are stored.
    /// Uses `data_dir` from config (with `~` expansion), defaulting to `~/.brainjar`.
    pub fn effective_db_dir(&self) -> PathBuf {
        let raw = self
            .data_dir
            .as_deref()
            .unwrap_or("~/.brainjar");
        // Expand environment variables (${HOME}, ${USER}, etc) then tilde
        let expanded = expand_env_var(raw);
        expand_tilde(&expanded)
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

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p.starts_with('~')
        && let Some(home) = dirs::home_dir()
    {
        // Handle `~/...` and bare `~`
        let rest = p.trim_start_matches('~').trim_start_matches('/');
        return if rest.is_empty() { home } else { home.join(rest) };
    }
    PathBuf::from(p)
}

/// Expand `${VAR_NAME}` env-var references in an API key value.
pub(crate) fn expand_env_var(key: &str) -> String {
    if key.starts_with("${") && key.ends_with('}') {
        let var_name = &key[2..key.len() - 1];
        std::env::var(var_name).unwrap_or_default()
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(extra_toml: &str) -> Config {
        let toml_str = format!(
            r#"
[knowledge_bases.test]
watch_paths = ["notes"]
auto_sync = true
{extra_toml}
"#
        );
        let mut config: Config = toml::from_str(&toml_str).unwrap();
        config.config_dir = std::path::PathBuf::from("/tmp");
        config
    }

    #[test]
    fn test_parse_valid_toml_minimal() {
        let config = make_config("");
        assert!(config.knowledge_bases.contains_key("test"));
        assert!(config.embeddings.is_none());
        assert!(config.extraction.is_none());
    }

    #[test]
    fn test_parse_providers_section() {
        let config = make_config(
            r#"
[providers.gemini]
api_key = "gk-123"
base_url = "https://gemini.example.com"
"#,
        );
        let p = config.providers.get("gemini").unwrap();
        assert_eq!(p.api_key.as_deref(), Some("gk-123"));
        assert_eq!(p.base_url.as_deref(), Some("https://gemini.example.com"));
    }

    #[test]
    fn test_parse_embedding_config() {
        let config = make_config(
            r#"
[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
"#,
        );
        let emb = config.embeddings.as_ref().unwrap();
        assert_eq!(emb.provider, "openai");
        assert_eq!(emb.model, "text-embedding-3-small");
        assert_eq!(emb.dimensions, 1536);
    }

    #[test]
    fn test_parse_extraction_config() {
        let config = make_config(
            r#"
[extraction]
provider = "gemini"
model = "gemini-pro"
enabled = true
"#,
        );
        let ext = config.extraction.as_ref().unwrap();
        assert_eq!(ext.provider, "gemini");
        assert!(ext.enabled);
    }

    #[test]
    fn test_expand_env_var_present() {
        unsafe { std::env::set_var("BRAINJAR_TEST_KEY_CFGTEST", "secret"); }
        let result = expand_env_var("${BRAINJAR_TEST_KEY_CFGTEST}");
        assert_eq!(result, "secret");
        unsafe { std::env::remove_var("BRAINJAR_TEST_KEY_CFGTEST"); }
    }

    #[test]
    fn test_expand_env_var_missing() {
        // Unset var → empty string
        unsafe { std::env::remove_var("BRAINJAR_NONEXISTENT_XYZ"); }
        let result = expand_env_var("${BRAINJAR_NONEXISTENT_XYZ}");
        assert_eq!(result, "");
    }

    #[test]
    fn test_expand_env_var_literal() {
        let result = expand_env_var("sk-literal-key");
        assert_eq!(result, "sk-literal-key");
    }

    #[test]
    fn test_resolve_api_key_providers_section_takes_priority() {
        let config = make_config(
            r#"
[providers.gemini]
api_key = "from-providers"
"#,
        );
        let key = config.resolve_api_key("gemini", Some("legacy-key"));
        assert_eq!(key.as_deref(), Some("from-providers"));
    }

    #[test]
    fn test_resolve_api_key_legacy_fallback() {
        let config = make_config(""); // no providers section
        let key = config.resolve_api_key("openai", Some("legacy-key"));
        assert_eq!(key.as_deref(), Some("legacy-key"));
    }

    #[test]
    fn test_resolve_api_key_none_when_missing() {
        let config = make_config("");
        let key = config.resolve_api_key("openai", None);
        assert!(key.is_none());
    }

    #[test]
    fn test_resolve_api_key_env_var_expansion() {
        unsafe { std::env::set_var("BRAINJAR_API_CFG_TEST", "expanded-key"); }
        let config = make_config(
            r#"
[providers.openai]
api_key = "${BRAINJAR_API_CFG_TEST}"
"#,
        );
        let key = config.resolve_api_key("openai", None);
        assert_eq!(key.as_deref(), Some("expanded-key"));
        unsafe { std::env::remove_var("BRAINJAR_API_CFG_TEST"); }
    }

    #[test]
    fn test_resolve_base_url_providers_priority() {
        let config = make_config(
            r#"
[providers.ollama]
base_url = "http://gpu:11434"
"#,
        );
        let url = config.resolve_base_url("ollama", Some("http://localhost:11434"));
        assert_eq!(url.as_deref(), Some("http://gpu:11434"));
    }

    #[test]
    fn test_resolve_base_url_legacy_fallback() {
        let config = make_config("");
        let url = config.resolve_base_url("ollama", Some("http://localhost:11434"));
        assert_eq!(url.as_deref(), Some("http://localhost:11434"));
    }

    #[test]
    fn test_backward_compat_inline_api_key() {
        let config = make_config(
            r#"
[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
api_key = "inline-key"
"#,
        );
        let emb = config.embeddings.as_ref().unwrap();
        assert_eq!(emb.api_key.as_deref(), Some("inline-key"));
        // Resolve should return it as fallback when no providers entry
        let resolved = config.resolve_api_key(&emb.provider, emb.api_key.as_deref());
        assert_eq!(resolved.as_deref(), Some("inline-key"));
    }

    #[test]
    fn test_expand_watch_paths_relative() {
        let config = make_config("");
        let kb = config.knowledge_bases.get("test").unwrap().clone();
        let paths = config.expand_watch_paths(&kb);
        assert_eq!(paths.len(), 1);
        // Relative path should be joined to config_dir (/tmp)
        assert!(paths[0].0.to_string_lossy().contains("notes"));
    }

    #[test]
    fn test_expand_path_absolute() {
        let mut config = make_config("");
        config.config_dir = std::path::PathBuf::from("/some/dir");
        let expanded = config.expand_path("/absolute/path");
        assert_eq!(expanded, std::path::PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_load_config_from_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("brainjar.toml");
        let content = r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true
"#;
        std::fs::write(&config_path, content).unwrap();
        let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
        assert!(config.knowledge_bases.contains_key("main"));
        let expected = dir.path().canonicalize().unwrap_or(dir.path().to_path_buf());
        assert_eq!(config.config_dir, expected);
    }

    #[test]
    fn test_load_config_missing_file_errors() {
        let result = load_config(Some("/nonexistent/path/brainjar.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_invalid_toml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("brainjar.toml");
        std::fs::write(&config_path, "this is not { valid toml [").unwrap();
        let result = load_config(Some(config_path.to_str().unwrap()));
        assert!(result.is_err());
    }

    // ─── FolderConfig / effective_folders ────────────────────────────────────

    #[test]
    fn test_effective_folders_uses_folders_when_present() {
        let toml_str = r#"
[knowledge_bases.test]
auto_sync = false

[[knowledge_bases.test.folders]]
path = "docs"
weight_boost = 0.5

[[knowledge_bases.test.folders]]
path = "src"
weight_boost = 1.0
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let kb = config.knowledge_bases.get("test").unwrap();
        let folders = kb.effective_folders();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].path, "docs");
        assert!((folders[0].weight_boost - 0.5).abs() < f64::EPSILON);
        assert_eq!(folders[1].path, "src");
        assert!((folders[1].weight_boost - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_effective_folders_converts_watch_paths_when_folders_empty() {
        let toml_str = r#"
[knowledge_bases.test]
watch_paths = ["notes", "docs"]
auto_sync = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let kb = config.knowledge_bases.get("test").unwrap();
        let folders = kb.effective_folders();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].path, "notes");
        assert!((folders[0].weight_boost).abs() < f64::EPSILON);
        assert!(folders[0].decay.is_none());
    }

    #[test]
    fn test_config_folders_with_decay() {
        let toml_str = r#"
[knowledge_bases.test]
auto_sync = false

[[knowledge_bases.test.folders]]
path = "news"
weight_boost = 0.2

[knowledge_bases.test.folders.decay]
horizon_days = 30
floor = 0.1
shape = 2.0
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let kb = config.knowledge_bases.get("test").unwrap();
        let folders = kb.effective_folders();
        assert_eq!(folders.len(), 1);
        let decay = folders[0].decay.as_ref().unwrap();
        assert_eq!(decay.horizon_days, 30);
        assert!((decay.floor - 0.1).abs() < 1e-9);
        assert!((decay.shape - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_legacy_watch_paths_config_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("brainjar.toml");
        let content = r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true
"#;
        std::fs::write(&config_path, content).unwrap();
        let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
        let kb = config.knowledge_bases.get("main").unwrap();
        assert_eq!(kb.watch_paths.len(), 1);
        assert!(kb.folders.is_empty());
        let folders = kb.effective_folders();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "notes");
    }
}
