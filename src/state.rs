use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    pub version: u32,
    pub knowledge_bases: HashMap<String, KbState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KbState {
    pub last_sync: Option<DateTime<Utc>>,
    pub last_ingestion_job_id: Option<String>,
    pub last_ingestion_status: Option<String>,
    pub files: HashMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub content_hash: String,
    pub s3_key: String,
    pub last_modified: DateTime<Utc>,
}

impl State {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = state_path(config_dir);
        if !path.exists() {
            return Ok(State {
                version: 1,
                knowledge_bases: HashMap::new(),
            });
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read state file: {}", path.display()))?;
        let state: State = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse state file: {}", path.display()))?;
        Ok(state)
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let path = state_path(config_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write state file: {}", path.display()))?;
        Ok(())
    }

    pub fn kb_state(&self, kb_name: &str) -> KbState {
        self.knowledge_bases
            .get(kb_name)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_kb_state(&mut self, kb_name: &str, kb_state: KbState) {
        self.knowledge_bases.insert(kb_name.to_string(), kb_state);
    }
}

fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".brainjar").join("state.json")
}
