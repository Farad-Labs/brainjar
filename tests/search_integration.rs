/// Integration tests for search modes against the golden test corpus
/// 
/// These tests validate that each search engine (FTS, vector, graph, fuzzy)
/// correctly finds planted signals in the test corpus.
/// 
/// Run with: cargo test --test search_integration -- --ignored
/// (Requires GOOGLE_API_KEY and synced test-corpus)

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn test_corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-corpus")
}

fn run_search(query: &str, mode: &str) -> String {
    let output = Command::new("brainjar")
        .arg("search")
        .arg(query)
        .arg("--mode")
        .arg(mode)
        .arg("--config")
        .arg(test_corpus_path().join("brainjar.toml"))
        .arg("--json")
        .output()
        .expect("Failed to run brainjar search");

    String::from_utf8_lossy(&output.stdout).to_string()
}

fn results_contain_file(results: &str, filename: &str) -> bool {
    results.contains(filename)
}

fn get_score_for_file(results: &str, filename: &str) -> Option<f64> {
    // Parse JSON and find score for the given file
    // (Simplified: assumes one result per file)
    if !results_contain_file(results, filename) {
        return None;
    }
    // TODO: Proper JSON parsing when we implement --json output
    Some(0.5) // placeholder
}

#[test]
#[ignore] // requires synced corpus + API keys
fn test_fts_finds_exact_terms() {
    // "thermal throttling" appears only in semantic-traps.md
    // FTS should find it by exact keyword match
    let results = run_search("thermal throttling", "text");
    assert!(
        results_contain_file(&results, "semantic-traps.md"),
        "FTS should find exact phrase 'thermal throttling' in semantic-traps.md"
    );
}

#[test]
#[ignore]
fn test_fts_misses_synonyms() {
    // "caching strategy" does NOT appear in synonym-concepts.md (only synonyms)
    let results = run_search("caching strategy", "text");
    assert!(
        !results_contain_file(&results, "synonym-concepts.md"),
        "FTS should NOT find synonym-concepts.md for 'caching strategy' (no keyword match)"
    );
}

#[test]
#[ignore]
fn test_vector_finds_synonyms() {
    // Vector search should find synonym-concepts.md for "caching strategy"
    // even though it only contains synonyms (memoization, hot data store, etc.)
    let results = run_search("caching strategy", "vector");
    assert!(
        results_contain_file(&results, "synonym-concepts.md"),
        "Vector search should find synonym-concepts.md via semantic similarity"
    );
}

#[test]
#[ignore]
fn test_vector_finds_paraphrased_concepts() {
    // "saving money on infrastructure" → synonym-concepts.md talks about cost reduction
    // but uses phrases like "deferred database costs" and "net benefit"
    let results = run_search("saving money on infrastructure", "vector");
    assert!(
        results_contain_file(&results, "synonym-concepts.md"),
        "Vector should find cost-related content via semantic similarity"
    );
}

#[test]
#[ignore]
fn test_fuzzy_corrects_typos() {
    // "kubernetes" should match "kuberentes" (typo) in typo-corpus.md
    let results = run_search("kubernetes", "fuzzy");
    assert!(
        results_contain_file(&results, "typo-corpus.md"),
        "Fuzzy search should correct 'kuberentes' typo to match 'kubernetes' query"
    );
}

#[test]
#[ignore]
fn test_fuzzy_handles_abbreviations() {
    // "PostgreSQL" should match "pg" and "postgress" in typo-corpus.md
    let results = run_search("PostgreSQL", "fuzzy");
    assert!(
        results_contain_file(&results, "typo-corpus.md"),
        "Fuzzy search should match PostgreSQL to pg/postgress variations"
    );
}

#[test]
#[ignore]
fn test_graph_traverses_relationships() {
    // "Elena Vasquez Atlas" → Elena replaced Marcus Webb, Marcus led Atlas
    // Graph should connect Elena → Marcus → Atlas
    let results = run_search("Elena Vasquez Atlas", "graph");
    assert!(
        results_contain_file(&results, "hidden-connections.md"),
        "Graph should connect Elena to Atlas via Marcus Webb replacement relationship"
    );
}

#[test]
#[ignore]
fn test_graph_finds_account_transfer() {
    // "Elena Cobalt" → hidden-connections.md documents Cobalt handoff from James to Elena
    let results = run_search("Elena Cobalt", "graph");
    assert!(
        results_contain_file(&results, "hidden-connections.md"),
        "Graph should find Elena-Cobalt connection via account transfer"
    );
}

#[test]
#[ignore]
fn test_graph_multi_hop() {
    // Project Helix → Tanaka → Liu → SOC 2 (3-hop chain)
    // Query: "Project Helix SOC 2" should connect via graph
    let results = run_search("Project Helix SOC 2", "graph");
    assert!(
        results_contain_file(&results, "cross-reference-chain.md"),
        "Graph should traverse multi-hop: Helix → Tanaka → Liu → SOC 2"
    );
}

#[test]
#[ignore]
fn test_all_mode_merges_signals() {
    // "Marcus replacement" should find hidden-connections via both FTS and graph
    // FTS: keyword "replacement"
    // Graph: Marcus Webb → Elena Vasquez relationship
    let all_results = run_search("Marcus replacement", "all");
    let fts_results = run_search("Marcus replacement", "text");
    
    assert!(
        results_contain_file(&all_results, "hidden-connections.md"),
        "All mode should find hidden-connections.md"
    );
    assert!(
        results_contain_file(&fts_results, "hidden-connections.md"),
        "FTS mode should also find it (keyword match)"
    );
    
    // TODO: Verify that all_mode score >= fts_mode score (graph boost)
}

#[test]
#[ignore]
fn test_vector_semantic_query_beats_fts() {
    // "cost overruns" → semantic-traps.md uses "financial hemorrhage"
    // Vector should find it, FTS should not
    let fts = run_search("cost overruns", "text");
    let vec = run_search("cost overruns", "vector");
    
    assert!(
        !results_contain_file(&fts, "semantic-traps.md"),
        "FTS should NOT find 'cost overruns' (no keyword match)"
    );
    assert!(
        results_contain_file(&vec, "semantic-traps.md"),
        "Vector SHOULD find via semantic similarity to 'financial hemorrhage'"
    );
}

#[test]
#[ignore]
fn test_vector_understands_performance_synonyms() {
    // "performance problems" → semantic-traps.md uses "thermal throttling", "buckled", "latency balloon"
    let results = run_search("performance problems", "vector");
    assert!(
        results_contain_file(&results, "semantic-traps.md"),
        "Vector should connect 'performance problems' to load/throttling descriptions"
    );
}

#[test]
#[ignore]
fn test_fuzzy_bidirectional() {
    // Corpus has typo "archetecture", query has correct "architecture"
    // Fuzzy should still match (bidirectional correction)
    let results = run_search("architecture", "fuzzy");
    assert!(
        results_contain_file(&results, "typo-corpus.md"),
        "Fuzzy should match query 'architecture' to corpus typo 'archetecture'"
    );
}

#[test]
#[ignore]
fn test_graph_finds_indirect_relationships() {
    // Sarah Chen → Elena Vasquez (reporting relationship)
    // Elena → Cobalt Finance (account owner)
    // Query "Sarah Cobalt" should connect via Elena
    let results = run_search("Sarah Cobalt", "graph");
    assert!(
        results_contain_file(&results, "hidden-connections.md"),
        "Graph should connect Sarah to Cobalt via Elena's reporting relationship"
    );
}

#[test]
#[ignore]
fn test_graph_executive_sponsorship_chain() {
    // James Liu sponsors SOC 2 audit
    // Dr. Yuki Tanaka reports to James Liu
    // Query "Tanaka audit" should connect via James
    let results = run_search("Tanaka SOC 2 audit", "graph");
    assert!(
        results_contain_file(&results, "cross-reference-chain.md"),
        "Graph should connect Tanaka to SOC 2 audit via James Liu sponsorship"
    );
}
