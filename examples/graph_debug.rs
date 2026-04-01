use graphqlite::Graph;

fn main() -> anyhow::Result<()> {
    let g = Graph::open(":memory:")?;

    g.upsert_node("alice", [("name", "Alice"), ("type", "person"), ("description", "Lead dev")], "Entity")?;
    g.upsert_node("brainjar", [("name", "brainjar"), ("type", "project"), ("description", "Memory tool")], "Entity")?;
    g.upsert_node("doc1", [("path", "memory/test.md"), ("type", "document")], "Document")?;
    g.upsert_edge("doc1", "alice", [("source_doc", "memory/test.md")], "MENTIONS")?;

    // Test 1: Return full node object
    let r1 = g.query("MATCH (e:Entity) RETURN e")?;
    println!("=== MATCH (e:Entity) RETURN e ===");
    for row in r1.iter() {
        println!("  row columns: {:?}", row.columns());
        if let Some(v) = row.get_value("e") {
            println!("  e = {:?}", v);
        }
    }

    // Test 2: Return individual properties
    let r2 = g.query("MATCH (e:Entity) RETURN e.id AS id, e.name AS name, e.type AS etype")?;
    println!("\n=== RETURN individual props ===");
    for row in r2.iter() {
        let id: String = row.get("id")?;
        let name: String = row.get("name")?;
        let etype: String = row.get("etype")?;
        println!("  id={} name={} type={}", id, name, etype);
    }

    // Test 3: find docs mentioning alice
    let r3 = g.query_builder("MATCH (d:Document)-[:MENTIONS]->(e {id: $eid}) RETURN d.path AS dpath")
        .param("eid", "alice")
        .run()?;
    println!("\n=== Docs mentioning alice ===");
    for row in r3.iter() {
        let path: String = row.get("dpath")?;
        println!("  path={}", path);
    }

    Ok(())
}
