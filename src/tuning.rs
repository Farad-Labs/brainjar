use serde::{Deserialize, Serialize};

/// Tunable scoring parameters for search quality.
/// These can be overridden via the `[tuning]` section in `brainjar.toml`.
/// If the section is omitted, all fields fall back to their hardcoded defaults,
/// preserving identical behaviour to earlier versions of BrainJar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningParams {
    // ── Weighted Score Fusion (WSF) engine weights ────────────────────────────
    /// Weight applied to FTS5 BM25 results (default: 0.35).
    #[serde(default = "default_wsf_fts")]
    pub wsf_fts_weight: f64,
    /// Weight applied to vector KNN results (default: 0.25).
    #[serde(default = "default_wsf_vector")]
    pub wsf_vector_weight: f64,
    /// Weight applied to knowledge-graph entity results (default: 0.2).
    #[serde(default = "default_wsf_graph")]
    pub wsf_graph_weight: f64,
    /// Weight applied to filename stem results (default: 0.1).
    #[serde(default = "default_wsf_filename")]
    pub wsf_filename_weight: f64,
    /// Weight applied to local fuzzy results (default: 0.1).
    #[serde(default = "default_wsf_local")]
    pub wsf_local_weight: f64,

    // ── Filename search scoring ───────────────────────────────────────────────
    /// Score added when a query word exactly matches the filename stem (default: 1.0).
    #[serde(default = "default_filename_exact")]
    pub filename_exact_score: f64,
    /// Score added when a query word is a substring of the filename stem (default: 0.5).
    #[serde(default = "default_filename_substring")]
    pub filename_substring_score: f64,

    // ── Graph entity scoring ──────────────────────────────────────────────────
    /// Base score assigned to every graph search result (default: 1.0).
    /// Will be replaced by composite scoring in a future PR.
    #[serde(default = "default_graph_base_score")]
    pub graph_base_score: f64,
}

// ── Default functions (required by serde) ────────────────────────────────────

fn default_wsf_fts() -> f64 {
    0.35
}
fn default_wsf_vector() -> f64 {
    0.25
}
fn default_wsf_graph() -> f64 {
    0.2
}
fn default_wsf_filename() -> f64 {
    0.1
}
fn default_wsf_local() -> f64 {
    0.1
}
fn default_filename_exact() -> f64 {
    1.0
}
fn default_filename_substring() -> f64 {
    0.5
}
fn default_graph_base_score() -> f64 {
    1.0
}

impl Default for TuningParams {
    fn default() -> Self {
        Self {
            wsf_fts_weight: default_wsf_fts(),
            wsf_vector_weight: default_wsf_vector(),
            wsf_graph_weight: default_wsf_graph(),
            wsf_filename_weight: default_wsf_filename(),
            wsf_local_weight: default_wsf_local(),
            filename_exact_score: default_filename_exact(),
            filename_substring_score: default_filename_substring(),
            graph_base_score: default_graph_base_score(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All default values must match the constants that were previously hardcoded
    /// in search.rs and graph.rs.
    #[test]
    fn test_tuning_params_default_values() {
        let t = TuningParams::default();
        assert!((t.wsf_fts_weight - 0.35).abs() < f64::EPSILON);
        assert!((t.wsf_vector_weight - 0.25).abs() < f64::EPSILON);
        assert!((t.wsf_graph_weight - 0.2).abs() < f64::EPSILON);
        assert!((t.wsf_filename_weight - 0.1).abs() < f64::EPSILON);
        assert!((t.wsf_local_weight - 0.1).abs() < f64::EPSILON);
        assert!((t.filename_exact_score - 1.0).abs() < f64::EPSILON);
        assert!((t.filename_substring_score - 0.5).abs() < f64::EPSILON);
        assert!((t.graph_base_score - 1.0).abs() < f64::EPSILON);
    }

    /// A full `[tuning]` override in TOML should propagate to all fields.
    #[test]
    fn test_tuning_params_config_override() {
        let toml_str = r#"
[tuning]
wsf_fts_weight = 0.4
wsf_vector_weight = 0.3
wsf_graph_weight = 0.15
wsf_filename_weight = 0.08
wsf_local_weight = 0.07
filename_exact_score = 2.0
filename_substring_score = 0.8
graph_base_score = 0.9
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            tuning: TuningParams,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("should parse");
        let t = w.tuning;
        assert!((t.wsf_fts_weight - 0.4).abs() < f64::EPSILON);
        assert!((t.wsf_vector_weight - 0.3).abs() < f64::EPSILON);
        assert!((t.wsf_graph_weight - 0.15).abs() < f64::EPSILON);
        assert!((t.wsf_filename_weight - 0.08).abs() < f64::EPSILON);
        assert!((t.wsf_local_weight - 0.07).abs() < f64::EPSILON);
        assert!((t.filename_exact_score - 2.0).abs() < f64::EPSILON);
        assert!((t.filename_substring_score - 0.8).abs() < f64::EPSILON);
        assert!((t.graph_base_score - 0.9).abs() < f64::EPSILON);
    }

    /// Partial override: only some fields set; unset fields must use defaults.
    #[test]
    fn test_tuning_params_partial_override() {
        let toml_str = r#"
[tuning]
wsf_fts_weight = 0.5
graph_base_score = 0.75
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            tuning: TuningParams,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("should parse");
        let t = w.tuning;
        // Overridden fields
        assert!((t.wsf_fts_weight - 0.5).abs() < f64::EPSILON);
        assert!((t.graph_base_score - 0.75).abs() < f64::EPSILON);
        // Unchanged fields should be at their defaults
        assert!((t.wsf_vector_weight - 0.25).abs() < f64::EPSILON);
        assert!((t.wsf_graph_weight - 0.2).abs() < f64::EPSILON);
        assert!((t.wsf_filename_weight - 0.1).abs() < f64::EPSILON);
        assert!((t.wsf_local_weight - 0.1).abs() < f64::EPSILON);
        assert!((t.filename_exact_score - 1.0).abs() < f64::EPSILON);
        assert!((t.filename_substring_score - 0.5).abs() < f64::EPSILON);
    }

    /// Omitting `[tuning]` entirely deserializes as `Default`.
    #[test]
    fn test_tuning_params_absent_uses_defaults() {
        let toml_str = r#"
[knowledge_bases.test]
watch_paths = ["notes"]
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            tuning: TuningParams,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("should parse");
        let t = w.tuning;
        assert!((t.wsf_fts_weight - 0.35).abs() < f64::EPSILON);
        assert!((t.filename_substring_score - 0.5).abs() < f64::EPSILON);
    }
}
