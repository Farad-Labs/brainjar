/// Integration tests for smart data_dir resolution in brainjar init.
/// Tests the three main scenarios:
/// 1. Default config in ~/.brainjar/ → data_dir = ~/.brainjar
/// 2. Custom config in regular dir → data_dir = {parent}/.brainjar
/// 3. Custom config already in .brainjar → data_dir = {parent} (no double nesting)
use brainjar::config::load_config;
use brainjar::init::{resolve_data_dir, resolve_data_dir_string, generate_brainjar_toml};
use std::path::{Path, PathBuf};

#[test]
fn test_resolve_data_dir_prevents_double_nesting() {
    // Config at /tmp/proj/.brainjar/brainjar.toml
    // Should resolve to /tmp/proj/.brainjar, NOT /tmp/proj/.brainjar/.brainjar
    let config = Path::new("/tmp/proj/.brainjar/brainjar.toml");
    let result = resolve_data_dir(config);
    assert_eq!(result, Path::new("/tmp/proj/.brainjar"));
    assert!(!result.to_string_lossy().contains(".brainjar/.brainjar"));
}

#[test]
fn test_resolve_data_dir_creates_subdir_for_regular_path() {
    // Config at /tmp/test-custom/test.toml
    // Should resolve to /tmp/test-custom/.brainjar
    let config = Path::new("/tmp/test-custom/test.toml");
    let result = resolve_data_dir(config);
    assert_eq!(result, Path::new("/tmp/test-custom/.brainjar"));
}

#[test]
fn test_smart_default_with_default_config_location() {
    // When user runs init with no --config, it uses ~/.brainjar/brainjar.toml
    // The smart default should be ~/.brainjar
    if let Some(home) = dirs::home_dir() {
        let config = home.join(".brainjar").join("brainjar.toml");
        let result = resolve_data_dir(&config);
        assert_eq!(result, home.join(".brainjar"));
    }
}

#[test]
fn test_smart_default_with_custom_config_in_subdir() {
    // Config at ~/experiments/test.toml
    // Should suggest ~/experiments/.brainjar
    if let Some(home) = dirs::home_dir() {
        let config = home.join("experiments").join("test.toml");
        let result = resolve_data_dir(&config);
        assert_eq!(result, home.join("experiments").join(".brainjar"));

        // Verify string representation uses ~
        let result_str = resolve_data_dir_string(&config);
        assert!(result_str.starts_with("~/"), "Expected ~ prefix, got: {}", result_str);
        assert!(
            result_str.contains(".brainjar"),
            "Expected .brainjar in path, got: {}",
            result_str
        );
    }
}

#[test]
fn test_two_configs_different_paths_separate_data_dirs() {
    // Create two configs with the same KB name but different paths
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();

    let config1 = dir1.path().join("brainjar.toml");
    let config2 = dir2.path().join("brainjar.toml");

    let data_dir1 = resolve_data_dir(&config1);
    let data_dir2 = resolve_data_dir(&config2);

    // They should be different
    assert_ne!(data_dir1, data_dir2);
    // Both should end with .brainjar
    assert!(data_dir1.ends_with(".brainjar"));
    assert!(data_dir2.ends_with(".brainjar"));
}

#[test]
fn test_generated_toml_contains_resolved_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("custom.toml");

    // Simulate what happens when user chooses the smart default
    let smart_data_dir = resolve_data_dir_string(&config_path);

    generate_brainjar_toml(
        &config_path,
        &smart_data_dir,
        &[],
        None,
        None,
        None,
        None,
        &[],
    )
    .expect("Failed to generate toml");

    let content = std::fs::read_to_string(&config_path).expect("Failed to read generated toml");

    // Verify the data_dir line is in the TOML
    assert!(
        content.contains(&format!("data_dir = \"{}\"", smart_data_dir)),
        "Generated TOML should contain data_dir = \"{}\". Got:\n{}",
        smart_data_dir,
        content
    );
}

#[test]
fn test_effective_db_dir_respects_resolved_data_dir() {
    // Generate a config with a custom data_dir, then load it back
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("brainjar.toml");

    // Resolve smart default
    let smart_dir = resolve_data_dir_string(&config_path);

    // Generate config with this data_dir
    generate_brainjar_toml(
        &config_path,
        &smart_dir,
        &[],
        None,
        None,
        None,
        None,
        &[],
    )
    .expect("Failed to generate toml");

    // Load the config back
    let config = load_config(Some(config_path.to_str().unwrap()))
        .expect("Failed to load config");

    let effective_dir = config.effective_db_dir();

    // Effective dir should match what we specified
    // (need to expand ~ and environment variables)
    let expected = brainjar::config::expand_tilde(&smart_dir);
    assert_eq!(effective_dir, expected, 
        "effective_db_dir should match resolved data_dir. Expected: {:?}, got: {:?}",
        expected, effective_dir);
}

#[test]
fn test_nested_brainjar_config_parent_is_dotbrainjar() {
    // If config is at ~/.brainjar/brainjar.toml, the parent is ~/.brainjar
    if let Some(home) = dirs::home_dir() {
        let brainjar_dir = home.join(".brainjar");
        let config = brainjar_dir.join("brainjar.toml");

        let parent = config.parent().unwrap();
        let parent_name = parent.file_name().map(|n| n.to_string_lossy().into_owned());
        assert_eq!(parent_name.as_deref(), Some(".brainjar"));

        // And resolve_data_dir should return the parent directly
        let result = resolve_data_dir(&config);
        assert_eq!(result, brainjar_dir);
    }
}

#[test]
fn test_resolve_data_dir_string_absolute_path_outside_home() {
    // For paths outside home, should return absolute path (no ~)
    let config = Path::new("/var/lib/brainjar/config.toml");
    let result = resolve_data_dir_string(config);
    assert_eq!(result, "/var/lib/brainjar/.brainjar");
    assert!(!result.contains("~"), "Absolute paths outside home should not use ~");
}

#[test]
fn test_resolve_data_dir_handles_relative_paths() {
    // Relative path like ./brainjar.toml should work
    let config = Path::new("./brainjar.toml");
    let result = resolve_data_dir(config);
    assert_eq!(result, Path::new("./.brainjar"));
}

#[test]
fn test_resolve_data_dir_handles_deeply_nested_paths() {
    // Verify it works with many parent levels
    let config = Path::new("/a/b/c/d/e/f/g/brainjar.toml");
    let result = resolve_data_dir(config);
    assert_eq!(result, Path::new("/a/b/c/d/e/f/g/.brainjar"));
}

#[test]
fn test_multiple_configs_same_project_different_names() {
    // Two configs in the same project directory with different filenames
    // Should both resolve to the same data_dir (parent/.brainjar)
    let dir = tempfile::tempdir().unwrap();
    let config1 = dir.path().join("brainjar.toml");
    let config2 = dir.path().join("brainjar-alt.toml");

    let data_dir1 = resolve_data_dir(&config1);
    let data_dir2 = resolve_data_dir(&config2);

    assert_eq!(data_dir1, data_dir2, 
        "Configs in same dir should resolve to same data_dir");
}
