# Spec: Integration Test Harness

**Status:** Draft
**Date:** 2026-04-02

## Overview

A repeatable integration test flow that validates all search modes work correctly against a golden test corpus. Runs after any change to embedding, storage, or retrieval logic.

## Test Flow

```bash
# 1. Cleanup
rm -rf test-corpus/.brainjar test-corpus/brainjar.toml

# 2. Create local config
cat > test-corpus/brainjar.toml << 'EOF'
[providers.gemini]
api_key = "${GOOGLE_API_KEY}"

[embeddings]
provider = "gemini"
model = "gemini-embedding-001"
dimensions = 3072

[extraction]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
enabled = true

[knowledge_bases.test-corpus]
watch_paths = ["./"]
auto_sync = true
description = "Golden test corpus for search validation"
EOF

# 3. Sync
cd test-corpus && brainjar sync --config brainjar.toml

# 4. Run integration tests
cargo test --test search_integration -- --ignored
```

## Test Corpus Design

Each document is crafted to plant specific signals that ONLY a particular search engine should find.

### FTS5 Signals (exact keyword match)
- Unique technical terms that only appear in one document
- Exact phrases that FTS5 tokenization handles well
- Multi-word queries that require proximity

### Graph Signals (entity/relationship traversal)
- Person A mentioned in doc 1, Person A's project mentioned in doc 2
- Implicit relationships: "Sarah designed X" in one doc, "X was redesigned by the new architect" in another — only graph connects Sarah to the redesign
- Entity chains: A→B→C across 3 docs, query about A↔C connection

### Vector/Semantic Signals (meaning, not keywords)
- Synonyms: doc uses "cost reduction" but query uses "saving money"
- Paraphrased concepts: doc says "the system breaks under heavy load" but query is "scalability problems"
- Domain equivalence: doc says "HIPAA compliance" but query is "healthcare data regulations"

### Fuzzy Signals (typo tolerance)
- Intentional misspellings in docs that fuzzy should correct
- Technical terms with common typos (e.g., "kuberentes" for "kubernetes")
- Abbreviations vs full names

### Cross-mode Signals (tests that multiple modes find the same thing)
- A document that should rank high in BOTH FTS and vector
- A query where graph provides context that improves ranking

## Expanded Test Corpus (15 documents)

### Existing (10) — Nexus Labs / Atlas project
Keep as-is. These already have good entity density for graph testing.

### New Documents (5) — planted signals

**11. `semantic-traps.md`** — Vector search validation
Contains concepts described with unusual phrasing that keyword search won't match:
- "The financial hemorrhage from our cloud provider" (query: "cost overruns")
- "Our deployment cadence accelerated" (query: "shipping faster")
- "The system exhibited thermal throttling under sustained load" (query: "performance problems")

**12. `typo-corpus.md`** — Fuzzy search validation  
Contains intentional misspellings and variations:
- "kuberentes" (kubernetes), "postgress" (postgres), "archetecture" (architecture)
- Abbreviations: "k8s", "pg", "tf" alongside full names
- Mixed case: "clickHouse", "PostgreSQL", "ClickHouse"

**13. `hidden-connections.md`** — Graph traversal validation
Introduces new entities that ONLY connect to existing entities via graph:
- "Elena Vasquez joined the team as Marcus Webb's replacement"
- "The Cobalt Finance deal was handed off from James Liu to Elena"
- Query "Elena's projects" should return Cobalt Finance + Atlas via graph

**14. `synonym-concepts.md`** — Vector + negative FTS test
Describes concepts using ONLY synonyms (never the "obvious" keyword):
- Describes "caching" without ever using the word "cache" (uses "memoization", "hot data store", "lookup acceleration")
- Describes "authentication" without the word (uses "identity verification", "credential validation", "access control")
- FTS for "cache" should NOT find this; vector search for "caching strategy" SHOULD

**15. `cross-reference-chain.md`** — Multi-hop graph test
Creates a 3-hop entity chain:
- "Project Helix" → managed by "Dr. Yuki Tanaka" → reports to "James Liu" → sponsors "SOC 2 audit"
- Query: "What's the connection between Project Helix and SOC 2?" 
- Only graph traversal (Helix → Tanaka → Liu → SOC 2) can connect these

## Test Assertions

```rust
#[test]
#[ignore] // requires API keys + synced corpus
fn test_fts_finds_exact_terms() {
    // "thermal throttling" appears only in semantic-traps.md
    // But FTS should find it by exact keyword
    let results = search("thermal throttling", Mode::Text);
    assert!(results.iter().any(|r| r.file.contains("semantic-traps")));
}

#[test]
#[ignore]
fn test_fts_misses_synonyms() {
    // "caching strategy" does NOT appear in synonym-concepts.md (only synonyms)
    let results = search("caching strategy", Mode::Text);
    assert!(!results.iter().any(|r| r.file.contains("synonym-concepts")));
}

#[test]
#[ignore]
fn test_vector_finds_synonyms() {
    // Vector search should find synonym-concepts.md for "caching strategy"
    let results = search("caching strategy", Mode::Vector);
    assert!(results.iter().any(|r| r.file.contains("synonym-concepts")));
}

#[test]
#[ignore]
fn test_fuzzy_corrects_typos() {
    // "kubernetes" should match "kuberentes" in typo-corpus.md
    let results = search("kubernetes", Mode::Fuzzy);
    assert!(results.iter().any(|r| r.file.contains("typo-corpus")));
}

#[test]
#[ignore]
fn test_graph_traverses_relationships() {
    // "Elena Vasquez" → connects to Atlas via Marcus Webb replacement
    let results = search("Elena Atlas", Mode::Graph);
    assert!(results.iter().any(|r| r.file.contains("hidden-connections")));
}

#[test]
#[ignore]
fn test_graph_multi_hop() {
    // Project Helix → Tanaka → Liu → SOC 2 (3 hops)
    let results = search("Project Helix SOC 2", Mode::Graph);
    assert!(!results.is_empty());
}

#[test]
#[ignore]
fn test_all_mode_merges_signals() {
    // "Marcus replacement" should find hidden-connections via:
    // - FTS (keyword "replacement")
    // - Graph (Marcus Webb → Elena Vasquez relationship)
    let results = search("Marcus replacement", Mode::All);
    assert!(results.iter().any(|r| r.file.contains("hidden-connections")));
    // Should rank higher than FTS-only because graph reinforces
    let fts_results = search("Marcus replacement", Mode::Text);
    let all_score = results.iter().find(|r| r.file.contains("hidden-connections")).unwrap().score;
    let fts_score = fts_results.iter().find(|r| r.file.contains("hidden-connections")).map(|r| r.score).unwrap_or(0.0);
    assert!(all_score >= fts_score);
}

#[test]
#[ignore]
fn test_vector_beats_fts_on_semantic_queries() {
    // "saving money on infrastructure" → synonym-concepts.md via vector
    // FTS won't find it (no keyword match for "saving money")
    let fts = search("saving money on infrastructure", Mode::Text);
    let vec = search("saving money on infrastructure", Mode::Vector);
    let fts_has_it = fts.iter().any(|r| r.file.contains("synonym-concepts"));
    let vec_has_it = vec.iter().any(|r| r.file.contains("synonym-concepts"));
    assert!(!fts_has_it, "FTS should NOT find synonym doc");
    assert!(vec_has_it, "Vector SHOULD find synonym doc");
}
```

## CI Integration

The integration tests are `#[ignore]` by default (require API keys). Run manually or in a CI step with secrets:

```yaml
# .github/workflows/integration.yml
name: Integration Tests
on: workflow_dispatch  # manual trigger only
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install --path .
      - run: |
          cd test-corpus
          brainjar sync --config brainjar.toml
        env:
          GOOGLE_API_KEY: ${{ secrets.GOOGLE_API_KEY }}
      - run: cargo test --test search_integration -- --ignored
        env:
          GOOGLE_API_KEY: ${{ secrets.GOOGLE_API_KEY }}
```

## Maintenance

When adding new search features or changing embedding/retrieval:
1. Add a new planted signal to the corpus
2. Add a corresponding test assertion
3. Run the full harness
4. If a test fails, the feature broke something — fix before merging
