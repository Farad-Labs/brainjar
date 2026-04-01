use anyhow::{Context, Result};
use graphqlite::Graph;
use std::path::Path;

// ─── Data types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entity {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Relationship {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphStats {
    pub node_count: i64,
    pub edge_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSearchResult {
    pub file: String,
    pub entity: String,
    pub entity_type: String,
    pub related_entities: Vec<String>,
    pub score: f64,
}

// ─── KnowledgeGraph ──────────────────────────────────────────────────────────

pub struct KnowledgeGraph {
    graph: Graph,
}

impl KnowledgeGraph {
    /// Open (or create) the graph database for a knowledge base.
    pub fn open(config_dir: &Path, kb_name: &str) -> Result<Self> {
        let graph_dir = config_dir.join(".brainjar");
        std::fs::create_dir_all(&graph_dir)
            .with_context(|| format!("Failed to create graph dir: {}", graph_dir.display()))?;
        let path = graph_dir.join(format!("{kb_name}_graph.db"));
        let graph = Graph::open(&path)
            .with_context(|| format!("Failed to open graph DB: {}", path.display()))?;
        Ok(Self { graph })
    }

    /// Returns true if a graph DB file already exists for this KB.
    pub fn exists(config_dir: &Path, kb_name: &str) -> bool {
        config_dir
            .join(".brainjar")
            .join(format!("{kb_name}_graph.db"))
            .exists()
    }

    /// Upsert entities and relationships extracted from a document.
    pub fn ingest_entities(
        &self,
        doc_path: &str,
        entities: &[Entity],
        relationships: &[Relationship],
    ) -> Result<()> {
        let doc_id = sanitize_id(doc_path);

        self.graph
            .upsert_node(&doc_id, [("path", doc_path), ("type", "document")], "Document")
            .with_context(|| format!("Failed to upsert document node: {doc_path}"))?;

        for entity in entities {
            let node_id = sanitize_id(&entity.name);
            self.graph
                .upsert_node(
                    &node_id,
                    [
                        ("name", entity.name.as_str()),
                        ("type", entity.entity_type.as_str()),
                        ("description", entity.description.as_str()),
                    ],
                    "Entity",
                )
                .with_context(|| format!("Failed to upsert entity: {}", entity.name))?;

            self.graph
                .upsert_edge(&doc_id, &node_id, [("source_doc", doc_path)], "MENTIONS")
                .with_context(|| {
                    format!("Failed to upsert MENTIONS edge for: {}", entity.name)
                })?;
        }

        for rel in relationships {
            let source_id = sanitize_id(&rel.source);
            let target_id = sanitize_id(&rel.target);

            // Ensure source and target entity nodes exist
            self.graph
                .upsert_node(&source_id, [("name", rel.source.as_str())], "Entity")
                .with_context(|| format!("Failed to upsert entity: {}", rel.source))?;
            self.graph
                .upsert_node(&target_id, [("name", rel.target.as_str())], "Entity")
                .with_context(|| format!("Failed to upsert entity: {}", rel.target))?;

            let rel_type = graphqlite::sanitize_rel_type(&rel.relation);
            self.graph
                .upsert_edge(
                    &source_id,
                    &target_id,
                    [
                        ("description", rel.description.as_str()),
                        ("source_doc", doc_path),
                    ],
                    &rel_type,
                )
                .with_context(|| {
                    format!(
                        "Failed to upsert edge: {} -[{}]-> {}",
                        rel.source, rel.relation, rel.target
                    )
                })?;
        }

        Ok(())
    }

    /// Search: find entities matching a query, then surface the documents that
    /// mention them (plus 1-hop neighbors for context).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<GraphSearchResult>> {
        let query_lower = query.to_lowercase();

        // Return individual properties — avoids needing to navigate the node wrapper
        let all_entities = self
            .graph
            .query("MATCH (e:Entity) RETURN e.id AS eid, e.name AS ename, e.type AS etype")
            .context("Failed to query entity nodes")?;

        // (id, name, type) of matching entities
        let mut matched: Vec<(String, String, String)> = Vec::new();

        for row in all_entities.iter() {
            let name: String = row.get("ename").unwrap_or_default();
            if !name.to_lowercase().contains(&query_lower) {
                continue;
            }
            let id: String = row.get("eid").unwrap_or_default();
            let etype: String = row.get("etype").unwrap_or_default();
            if !id.is_empty() {
                matched.push((id, name, etype));
            }
        }

        let mut raw_results: Vec<GraphSearchResult> = Vec::new();

        for (entity_id, entity_name, entity_type) in &matched {
            // 1-hop neighbors for context
            // get_neighbors returns full node objects: {properties: {name, ...}, labels: [...], id: N}
            let neighbors = self.graph.get_neighbors(entity_id).unwrap_or_default();
            let related: Vec<String> = neighbors
                .iter()
                .filter_map(|n| {
                    // Try direct "name" key first, then "properties.name"
                    n.get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .or_else(|| {
                            n.get("properties")
                                .and_then(|p| p.get("name"))
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                        })
                })
                .collect();

            // Documents that MENTION this entity
            let doc_rows = self
                .graph
                .query_builder(
                    "MATCH (d:Document)-[:MENTIONS]->(e {id: $eid}) RETURN d.path AS dpath",
                )
                .param("eid", entity_id.as_str())
                .run()
                .unwrap_or_else(|_| graphqlite::CypherResult::empty());

            for doc_row in doc_rows.iter() {
                let file = doc_row.get::<String>("dpath").unwrap_or_default();
                if file.is_empty() {
                    continue;
                }
                raw_results.push(GraphSearchResult {
                    file,
                    entity: entity_name.clone(),
                    entity_type: entity_type.clone(),
                    related_entities: related.clone(),
                    score: 1.0,
                });
            }

            if raw_results.len() >= limit * 4 {
                break;
            }
        }

        // Deduplicate by file (keep first occurrence = highest relevance)
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<GraphSearchResult> = raw_results
            .into_iter()
            .filter(|r| seen.insert(r.file.clone()))
            .take(limit)
            .collect();

        Ok(deduped)
    }

    /// Get node and edge counts for the graph.
    pub fn stats(&self) -> Result<GraphStats> {
        let s = self.graph.stats().context("Failed to get graph stats")?;
        Ok(GraphStats {
            node_count: s.node_count,
            edge_count: s.edge_count,
        })
    }

    /// Remove the document node and all its MENTIONS edges (call before re-extraction).
    pub fn remove_document(&self, doc_path: &str) -> Result<()> {
        let doc_id = sanitize_id(doc_path);
        // DETACH DELETE removes the node plus all connected edges
        self.graph
            .query_builder("MATCH (d:Document {id: $id}) DETACH DELETE d")
            .param("id", doc_id.as_str())
            .run()
            .context("Failed to remove document node from graph")?;
        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Create a stable, Cypher-safe node ID from an arbitrary string.
fn sanitize_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}
