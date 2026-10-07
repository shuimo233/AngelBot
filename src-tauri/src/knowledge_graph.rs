//! Knowledge Graph — entity extraction, storage, and traversal.
//! Issue #37 Phase 4: Enables entity-relationship reasoning during conversations.

use rusqlite::{params, Connection};

/// Insert or update a knowledge node (entity).
pub fn upsert_node(
    conn: &Connection,
    id: &str,
    entity_type: &str,
    label: &str,
    properties: Option<&str>,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO knowledge_nodes (id, entity_type, label, properties, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, entity_type, label, properties, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert a relationship edge between two nodes.
pub fn insert_edge(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
    predicate: &str,
    weight: f64,
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR IGNORE INTO knowledge_edges (id, source_id, target_id, predicate, weight, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, source_id, target_id, predicate, weight, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// Extract entities from text using simple heuristic (Noun phrase detection).
/// Returns list of (entity, entity_type) pairs for the LLM to refine.
pub fn heuristic_extract(text: &str) -> Vec<(String, String)> {
    let mut entities = Vec::new();
    // Simple extraction: detect capitalized words, quoted phrases, known patterns
    for word in text.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        if cleaned.len() >= 2 && cleaned.chars().next().map_or(false, |c| c.is_uppercase()) {
            entities.push((cleaned.to_string(), "topic".to_string()));
        }
    }
    // Detect quoted phrases as entities
    let mut in_quote = false;
    let mut current_quote = String::new();
    for ch in text.chars() {
        if ch == '"' || ch == '\u{201C}' || ch == '\u{201D}' {
            if in_quote && !current_quote.is_empty() {
                entities.push((current_quote.clone(), "topic".to_string()));
                current_quote.clear();
            }
            in_quote = !in_quote;
        } else if in_quote {
            current_quote.push(ch);
        }
    }
    entities
}

/// Get graph context for injection into system prompt.
/// Returns a textual description of entities related to the query.
pub fn get_graph_context(conn: &Connection, query: &str, max_hops: i32) -> String {
    let sql = format!(
        "WITH RECURSIVE walk AS (
            SELECT n.id, n.entity_type, n.label, CAST('' AS TEXT) as predicate, CAST('' AS TEXT) as target_label, 1 as hop
            FROM knowledge_nodes n WHERE n.label LIKE '%' || ?1 || '%'
            UNION ALL
            SELECT n2.id, n2.entity_type, n2.label, e.predicate, n3.label, w.hop + 1
            FROM walk w
            JOIN knowledge_edges e ON w.id = e.source_id
            JOIN knowledge_nodes n2 ON e.target_id = n2.id
            LEFT JOIN knowledge_nodes n3 ON e.source_id = n3.id
            WHERE w.hop < {}
        )
        SELECT DISTINCT entity_type, label, predicate, target_label, hop FROM walk ORDER BY hop", max_hops
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let rows: Vec<String> = stmt
        .query_map(params![query], |r| {
            let etype = r.get::<_, String>(0)?;
            let label = r.get::<_, String>(1)?;
            let pred = r.get::<_, Option<String>>(2)?.unwrap_or_default();
            let target = r.get::<_, Option<String>>(3)?.unwrap_or_default();
            if pred.is_empty() {
                Ok(format!("[{}] {}", etype, label))
            } else {
                Ok(format!(
                    "[{}] {} → {} → [{}] {}",
                    etype, label, pred, etype, target
                ))
            }
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    if rows.is_empty() {
        String::new()
    } else {
        format!(
            "## 知识图谱上下文\n以下是与当前对话相关的知识图谱节点：\n{}",
            rows.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_extract_capitalized() {
        let entities = heuristic_extract("I talked to Alice about Python and Docker");
        // Should detect Alice, Python, Docker
        assert!(!entities.is_empty());
    }

    #[test]
    fn test_heuristic_extract_quoted() {
        let entities = heuristic_extract("The project \"AngelBot\" uses Rust and Tauri");
        let has_angelbot = entities.iter().any(|(name, _)| name == "AngelBot");
        assert!(has_angelbot);
    }
}
