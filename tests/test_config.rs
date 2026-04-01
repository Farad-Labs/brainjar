/// Integration tests for config loading from temp files.
use brainjar::config::load_config;

#[test]
fn test_load_config_full_toml() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("brainjar.toml");
    let content = r#"
[knowledge_bases.notes]
watch_paths = ["notes"]
auto_sync = true

[knowledge_bases.code]
watch_paths = ["src"]
auto_sync = false

[providers.openai]
api_key = "sk-test"

[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536

[extraction]
provider = "openai"
model = "gpt-4o-mini"
enabled = false
"#;
    std::fs::write(&config_path, content).unwrap();

    let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
    assert!(config.knowledge_bases.contains_key("notes"));
    assert!(config.knowledge_bases.contains_key("code"));
    assert!(config.knowledge_bases["notes"].auto_sync);
    assert!(!config.knowledge_bases["code"].auto_sync);

    let emb = config.embeddings.as_ref().unwrap();
    assert_eq!(emb.provider, "openai");
    assert_eq!(emb.dimensions, 1536);

    let ext = config.extraction.as_ref().unwrap();
    assert!(!ext.enabled);

    let key = config.resolve_api_key("openai", None);
    assert_eq!(key.as_deref(), Some("sk-test"));
}

#[test]
fn test_load_config_minimal() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("brainjar.toml");
    std::fs::write(&config_path, "[knowledge_bases.kb]\nwatch_paths = [\"notes\"]\nauto_sync = true\n").unwrap();

    let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
    assert!(config.embeddings.is_none());
    assert!(config.extraction.is_none());
    assert!(config.providers.is_empty());
}

#[test]
fn test_load_config_env_var_expansion() {
    unsafe { std::env::set_var("BRAINJAR_INTEG_TEST_KEY", "expanded-value"); }
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("brainjar.toml");
    let content = r#"
[knowledge_bases.kb]
watch_paths = ["notes"]
auto_sync = true

[providers.gemini]
api_key = "${BRAINJAR_INTEG_TEST_KEY}"
"#;
    std::fs::write(&config_path, content).unwrap();
    let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
    let key = config.resolve_api_key("gemini", None);
    assert_eq!(key.as_deref(), Some("expanded-value"));
    unsafe { std::env::remove_var("BRAINJAR_INTEG_TEST_KEY"); }
}

#[test]
fn test_load_config_backward_compat_inline_key() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("brainjar.toml");
    let content = r#"
[knowledge_bases.kb]
watch_paths = ["notes"]
auto_sync = true

[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
api_key = "legacy-inline-key"
"#;
    std::fs::write(&config_path, content).unwrap();
    let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
    let emb = config.embeddings.as_ref().unwrap();
    let key = config.resolve_api_key(&emb.provider, emb.api_key.as_deref());
    assert_eq!(key.as_deref(), Some("legacy-inline-key"));
}

#[test]
fn test_config_dir_is_parent_of_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    let config_path = sub.join("brainjar.toml");
    std::fs::write(&config_path, "[knowledge_bases.kb]\nwatch_paths=[\".\"] \nauto_sync=true\n").unwrap();

    let config = load_config(Some(config_path.to_str().unwrap())).unwrap();
    assert_eq!(config.config_dir, sub);
}
