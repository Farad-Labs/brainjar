use brainjar::graph::{Entity, KnowledgeGraph, Relationship};

fn main() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join("brainjar_graph_test");
    std::fs::create_dir_all(&tmp)?;

    let kg = KnowledgeGraph::open(&tmp, "test_kb")?;

    let entities = vec![
        Entity {
            name: "Alice".to_string(),
            entity_type: "person".to_string(),
            description: "Lead dev".to_string(),
        },
        Entity {
            name: "brainjar".to_string(),
            entity_type: "project".to_string(),
            description: "Memory tool".to_string(),
        },
    ];
    let relationships = vec![Relationship {
        source: "Alice".to_string(),
        target: "brainjar".to_string(),
        relation: "created_by".to_string(),
        description: "Alice built it".to_string(),
    }];

    kg.ingest_entities("memory/2026-04-01.md", &entities, &relationships)?;

    let stats = kg.stats()?;
    println!("Nodes: {}, Edges: {}", stats.node_count, stats.edge_count);
    assert!(stats.node_count >= 3, "Expected doc node + 2 entity nodes");
    assert!(stats.edge_count >= 3, "Expected MENTIONS×2 + CREATED_BY");

    let results = kg.search("Alice", 5)?;
    println!("Graph search 'Alice': {} result(s)", results.len());
    assert!(!results.is_empty(), "Expected at least one result for Alice");
    println!("  file={}, entity={}", results[0].file, results[0].entity);

    kg.remove_document("memory/2026-04-01.md")?;
    println!("remove_document: OK");

    // cleanup
    let db_path = tmp.join(".brainjar").join("test_kb_graph.db");
    let _ = std::fs::remove_file(&db_path);

    println!("✓ All graph smoke tests passed!");
    Ok(())
}
