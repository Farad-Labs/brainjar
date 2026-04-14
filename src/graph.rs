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
    /// `db_dir` is the directory that contains brainjar databases
    /// (typically `config.effective_db_dir()`).
    pub fn open(db_dir: &Path, kb_name: &str) -> Result<Self> {
        std::fs::create_dir_all(db_dir)
            .with_context(|| format!("Failed to create db dir: {}", db_dir.display()))?;
        let path = db_dir.join(format!("{kb_name}_graph.db"));
        let graph = Graph::open(&path)
            .with_context(|| format!("Failed to open graph DB: {}", path.display()))?;
        Ok(Self { graph })
    }

    /// Returns true if a graph DB file already exists for this KB.
    pub fn exists(db_dir: &Path, kb_name: &str) -> bool {
        db_dir
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

    /// Upsert code entities and relationships extracted by tree-sitter.
    /// This is analogous to `ingest_entities` but works with `CodeEntity`/`CodeRelationship`
    /// types from the `treesitter` module, enabling zero-LLM-cost graph construction.
    #[cfg(feature = "tree-sitter")]
    pub fn ingest_code_entities(
        &self,
        entities: &[crate::treesitter::CodeEntity],
        relationships: &[crate::treesitter::CodeRelationship],
        file_path: &str,
    ) -> Result<()> {
        let doc_id = sanitize_id(file_path);

        self.graph
            .upsert_node(&doc_id, [("path", file_path), ("type", "document")], "Document")
            .with_context(|| format!("Failed to upsert document node: {file_path}"))?;

        for entity in entities {
            let node_id = sanitize_id(&format!("{}::{}", file_path, entity.name));
            self.graph
                .upsert_node(
                    &node_id,
                    [
                        ("name", entity.name.as_str()),
                        ("type", entity.entity_type.as_str()),
                        ("description", entity.description.as_str()),
                        ("file", entity.file_path.as_str()),
                    ],
                    "Entity",
                )
                .with_context(|| format!("Failed to upsert code entity: {}", entity.name))?;

            self.graph
                .upsert_edge(&doc_id, &node_id, [("source_doc", file_path)], "MENTIONS")
                .with_context(|| {
                    format!("Failed to upsert MENTIONS edge for: {}", entity.name)
                })?;
        }

        for rel in relationships {
            let source_id = sanitize_id(&format!("{}::{}", file_path, rel.source));
            let target_id = sanitize_id(&rel.target);

            self.graph
                .upsert_node(&source_id, [("name", rel.source.as_str())], "Entity")
                .with_context(|| format!("Failed to upsert source entity: {}", rel.source))?;
            self.graph
                .upsert_node(&target_id, [("name", rel.target.as_str())], "Entity")
                .with_context(|| format!("Failed to upsert target entity: {}", rel.target))?;

            let rel_type = graphqlite::sanitize_rel_type(&rel.relation);
            self.graph
                .upsert_edge(
                    &source_id,
                    &target_id,
                    [("source_doc", file_path)],
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
        // Split query into words for multi-word matching
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        // Return individual properties — avoids needing to navigate the node wrapper
        let all_entities = self
            .graph
            .query("MATCH (e:Entity) RETURN e.id AS eid, e.name AS ename, e.type AS etype")
            .context("Failed to query entity nodes")?;

        // (id, name, type, match_quality) of matching entities
        let mut matched: Vec<(String, String, String, f64)> = Vec::new();

        for row in all_entities.iter() {
            let name: String = row.get("ename").unwrap_or_default();
            let name_lower = name.to_lowercase();

            // Determine match quality: check each query word against the entity name
            let mut best_quality: Option<f64> = None;
            for word in &query_words {
                let quality = if name_lower == *word {
                    Some(1.0_f64)
                } else if name_lower.starts_with(word) {
                    Some(0.7_f64)
                } else if name_lower.contains(word) {
                    Some(0.5_f64)
                } else {
                    None
                };
                if let Some(q) = quality {
                    best_quality = Some(best_quality.map_or(q, |prev: f64| prev.max(q)));
                }
            }

            let match_quality = match best_quality {
                Some(q) => q,
                None => continue,
            };

            let id: String = row.get("eid").unwrap_or_default();
            let etype: String = row.get("etype").unwrap_or_default();
            if !id.is_empty() {
                matched.push((id, name, etype, match_quality));
            }
        }

        let mut raw_results: Vec<GraphSearchResult> = Vec::new();

        for (entity_id, entity_name, entity_type, match_quality) in &matched {
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

            // Count documents that MENTION this entity (for authority scoring)
            let mention_count: f64 = self
                .graph
                .query_builder(
                    "MATCH (d:Document)-[:MENTIONS]->(e {id: $eid}) RETURN count(d) AS cnt",
                )
                .param("eid", entity_id.as_str())
                .run()
                .ok()
                .and_then(|rows| rows.iter().next().and_then(|r| r.get::<i64>("cnt").ok()))
                .unwrap_or(1) as f64;

            let neighbor_count = related.len();

            // Composite entity score: match quality × logarithmic mention authority × connectivity boost
            let entity_score = match_quality
                * (1.0 + mention_count.ln().max(0.0))
                * (1.0 + 0.1 * neighbor_count as f64);

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
                    score: entity_score,
                });
            }

            if raw_results.len() >= limit * 4 {
                break;
            }
        }

        // Normalize scores to [0.0, 1.0] by dividing by the max score
        let max_score = raw_results
            .iter()
            .map(|r| r.score)
            .fold(0.0_f64, f64::max);
        if max_score > 0.0 {
            for r in &mut raw_results {
                r.score /= max_score;
            }
        }

        // Deduplicate by file: keep the highest-scoring entry per file
        let mut best_per_file: std::collections::HashMap<String, GraphSearchResult> =
            std::collections::HashMap::new();
        for result in raw_results {
            let entry = best_per_file
                .entry(result.file.clone())
                .or_insert_with(|| result.clone());
            if result.score > entry.score {
                *entry = result;
            }
        }

        let mut deduped: Vec<GraphSearchResult> = best_per_file.into_values().collect();
        deduped.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        deduped.truncate(limit);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_kg() -> (KnowledgeGraph, std::path::PathBuf) {
        let unique = format!(
            "brainjar_graph_test_{}",
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
    fn test_sanitize_id_lowercases() {
        assert_eq!(sanitize_id("Rust"), "rust");
    }

    #[test]
    fn test_sanitize_id_replaces_special_chars() {
        assert_eq!(sanitize_id("notes/hello.md"), "notes_hello_md");
    }

    #[test]
    fn test_sanitize_id_alphanumeric_unchanged() {
        assert_eq!(sanitize_id("abc123"), "abc123");
    }

    #[test]
    fn test_ingest_entities_and_search() {
        let (kg, _base) = make_kg();
        let entities = vec![
            Entity {
                name: "Brainjar".to_string(),
                entity_type: "project".to_string(),
                description: "AI memory tool".to_string(),
            },
        ];
        kg.ingest_entities("notes/intro.md", &entities, &[]).unwrap();

        let results = kg.search("Brainjar", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.entity.contains("Brainjar")));
    }

    #[test]
    fn test_ingest_entities_with_relationships() {
        let (kg, _base) = make_kg();
        let entities = vec![
            Entity {
                name: "Brainjar".to_string(),
                entity_type: "project".to_string(),
                description: "AI memory tool".to_string(),
            },
            Entity {
                name: "SQLite".to_string(),
                entity_type: "tool".to_string(),
                description: "Embedded DB".to_string(),
            },
        ];
        let rels = vec![
            Relationship {
                source: "Brainjar".to_string(),
                target: "SQLite".to_string(),
                relation: "uses".to_string(),
                description: "stores data in sqlite".to_string(),
            },
        ];
        kg.ingest_entities("notes/arch.md", &entities, &rels).unwrap();
        let stats = kg.stats().unwrap();
        assert!(stats.node_count > 0);
        assert!(stats.edge_count > 0);
    }

    #[test]
    fn test_search_case_insensitive() {
        let (kg, _base) = make_kg();
        let entities = vec![
            Entity {
                name: "MyProject".to_string(),
                entity_type: "project".to_string(),
                description: "Some project".to_string(),
            },
        ];
        kg.ingest_entities("doc.md", &entities, &[]).unwrap();
        let results = kg.search("myproject", 5).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_search_no_match_returns_empty() {
        let (kg, _base) = make_kg();
        let results = kg.search("xyzzy_nonexistent", 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_remove_document_does_not_error() {
        let (kg, _base) = make_kg();
        let entities = vec![
            Entity {
                name: "Rust".to_string(),
                entity_type: "tool".to_string(),
                description: "Systems language".to_string(),
            },
        ];
        kg.ingest_entities("notes/rust.md", &entities, &[]).unwrap();
        kg.remove_document("notes/rust.md").unwrap();
    }

    #[test]
    fn test_deduplication_in_search() {
        let (kg, _base) = make_kg();
        let entity = Entity {
            name: "SharedEntity".to_string(),
            entity_type: "concept".to_string(),
            description: "Appears in multiple docs".to_string(),
        };
        kg.ingest_entities("doc_a.md", std::slice::from_ref(&entity), &[]).unwrap();
        kg.ingest_entities("doc_b.md", &[entity], &[]).unwrap();

        let results = kg.search("SharedEntity", 10).unwrap();
        // No duplicate files in results
        let mut files: Vec<&str> = results.iter().map(|r| r.file.as_str()).collect();
        files.sort();
        files.dedup();
        assert_eq!(files.len(), results.len());
    }

    #[test]
    fn test_exists_before_and_after_creation() {
        let unique = format!(
            "brainjar_exists_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        );
        let base = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&base).unwrap();
        assert!(!KnowledgeGraph::exists(&base, "mydb"));
        KnowledgeGraph::open(&base, "mydb").unwrap();
        assert!(KnowledgeGraph::exists(&base, "mydb"));
    }

    #[test]
    fn test_stats_empty_graph() {
        let (kg, _base) = make_kg();
        let stats = kg.stats().unwrap();
        assert!(stats.node_count >= 0);
        assert!(stats.edge_count >= 0);
    }
}
