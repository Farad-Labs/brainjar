/// Shared helpers for integration tests.
use std::collections::HashMap;
use std::path::Path;

use brainjar::config::{Config, KnowledgeBaseConfig};

/// Build a minimal Config pointing at `config_dir` with one KB watching `watch_path`.
pub fn make_config(config_dir: &Path, watch_path: &Path) -> Config {
    let mut kbs = HashMap::new();
    kbs.insert(
        "test".to_string(),
        KnowledgeBaseConfig {
            kb_type: brainjar::config::KbType::Docs,
            watch_paths: vec![watch_path.to_string_lossy().to_string()],
            folders: vec![],
            auto_sync: true,
            description: None,
        },
    );
    Config {
        providers: HashMap::new(),
        knowledge_bases: kbs,
        embeddings: None,
        extraction: None,
        data_dir: Some(config_dir.to_string_lossy().to_string()),
        config_dir: config_dir.to_path_buf(),
        watch: None,
    }
}
