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
            watch_paths: vec![watch_path.to_string_lossy().to_string()],
            auto_sync: true,
        },
    );
    Config {
        providers: HashMap::new(),
        knowledge_bases: kbs,
        embeddings: None,
        extraction: None,
        config_dir: config_dir.to_path_buf(),
    }
}
