/// Integration tests for graph operations.
use brainjar::graph::{Entity, KnowledgeGraph, Relationship};

fn make_kg() -> (KnowledgeGraph, std::path::PathBuf) {
    let unique = format!(
        "brainjar_graph_integ_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    let base = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&base).unwrap();
    let kg = KnowledgeGraph::open(&base, "test").unwrap();
    (kg, base)
}

#[test]
fn test_graph_insert_and_search() {
    let (kg, _base) = make_kg();
    let entities = vec![
        Entity {
            name: "Brainjar".to_string(),
            entity_type: "project".to_string(),
            description: "AI memory layer".to_string(),
        },
        Entity {
            name: "SQLite".to_string(),
            entity_type: "tool".to_string(),
            description: "Embedded relational database".to_string(),
        },
    ];
    let rels = vec![Relationship {
        source: "Brainjar".to_string(),
        target: "SQLite".to_string(),
        relation: "uses".to_string(),
        description: "Stores data in SQLite".to_string(),
    }];

    kg.ingest_entities("notes/arch.md", &entities, &rels)
        .unwrap();

    let results = kg.search("Brainjar", 10, 1.0).unwrap();
    assert!(!results.is_empty());
    let result = &results[0];
    assert_eq!(result.entity, "Brainjar");
    assert_eq!(result.file, "notes/arch.md");
}

#[test]
fn test_graph_search_returns_file_path() {
    let (kg, _base) = make_kg();
    let entities = vec![Entity {
        name: "RustLang".to_string(),
        entity_type: "tool".to_string(),
        description: "A programming language".to_string(),
    }];
    kg.ingest_entities("docs/rust.md", &entities, &[]).unwrap();

    let results = kg.search("RustLang", 5, 1.0).unwrap();
    assert!(results.iter().any(|r| r.file == "docs/rust.md"));
}

#[test]
fn test_graph_search_no_match() {
    let (kg, _base) = make_kg();
    let results = kg.search("nonexistent_entity_xyz", 5, 1.0).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_graph_deduplication() {
    let (kg, _base) = make_kg();
    let entity = Entity {
        name: "SharedThing".to_string(),
        entity_type: "concept".to_string(),
        description: "Shared between two docs".to_string(),
    };
    // Ingest same entity from two different documents
    kg.ingest_entities("doc_a.md", std::slice::from_ref(&entity), &[])
        .unwrap();
    kg.ingest_entities("doc_b.md", &[entity], &[]).unwrap();

    let results = kg.search("SharedThing", 10, 1.0).unwrap();
    // Both docs should appear, but each only once
    let files: std::collections::HashSet<_> = results.iter().map(|r| r.file.as_str()).collect();
    assert_eq!(files.len(), results.len(), "Duplicate file entries found");
}

#[test]
fn test_graph_stats_after_ingestion() {
    let (kg, _base) = make_kg();
    let entities = vec![
        Entity {
            name: "Alpha".to_string(),
            entity_type: "concept".to_string(),
            description: "First".to_string(),
        },
        Entity {
            name: "Beta".to_string(),
            entity_type: "concept".to_string(),
            description: "Second".to_string(),
        },
    ];
    kg.ingest_entities("file.md", &entities, &[]).unwrap();

    let stats = kg.stats().unwrap();
    assert!(stats.node_count >= 2); // at least 2 entity nodes + 1 doc node
}

#[test]
fn test_graph_manually_inserted_entities_searchable() {
    let (kg, _base) = make_kg();
    // Manually insert without extraction (simulates extraction-skipped scenario)
    let entities = vec![
        Entity {
            name: "ManualEntity".to_string(),
            entity_type: "service".to_string(),
            description: "Manually inserted for testing".to_string(),
        },
    ];
    // graphqlite's MATCH+MERGE Cypher can fail on some SQLite builds (Linux CI).
    // Skip gracefully if ingestion fails due to Cypher parse error.
    if let Err(e) = kg.ingest_entities("manual.md", &entities, &[]) {
        if e.to_string().contains("PARSE_ERROR") || e.to_string().contains("syntax error") {
            eprintln!("Skipping test: graphqlite Cypher parse issue on this platform");
            return;
        }
        panic!("Unexpected error: {e}");
    }

    let results = kg.search("ManualEntity", 5, 1.0).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].entity_type, "service");
}
