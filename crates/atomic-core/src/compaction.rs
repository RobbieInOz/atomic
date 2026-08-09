//! Tag compaction/consolidation
//!
//! This module handles LLM-assisted tag merging and categorization.

use crate::providers::structured::{call_structured, StructuredCall, GENERATIVE_CALL_ENFORCEMENT};
use crate::providers::types::{GenerationParams, Message};
use crate::providers::ProviderConfig;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
pub struct MergeResult {
    pub merges: Vec<TagMerge>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagMerge {
    pub winner_name: String,
    pub loser_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    pub tags_merged: i32,
    pub atoms_retagged: i32,
}

const MERGE_SYSTEM_PROMPT: &str = r#"You are a tag deduplication assistant. Your task is to identify tags that are duplicates or too similar and should be merged.

RULES:
1. Merge tags that refer to the same concept (e.g., "React" and "React.js" and "ReactJS")
2. Merge tags that are case variations (e.g., "AI" and "ai")
3. Merge tags with slight spelling differences (e.g., "Javascript" and "JavaScript")
4. The "winner" should be the more canonical/common name
5. Do NOT merge tags that are genuinely different concepts
6. Do NOT merge parent category tags with their children (e.g., "Topics" and "AI")
7. Do NOT merge a tag into one of its ancestors or descendants
8. Be conservative - only merge when you're highly confident they're the same thing

EXAMPLES of good merges:
- Winner: "React", Loser: "React.js" (same framework)
- Winner: "Machine Learning", Loser: "ML" (same concept, abbreviation)
- Winner: "JavaScript", Loser: "Javascript" (case variation)
- Winner: "United States", Loser: "USA" (same country)

EXAMPLES of BAD merges (don't do these):
- "AI" and "Machine Learning" (related but distinct concepts)
- "React" and "Vue" (different frameworks)
- "Topics" and "Concepts" (different organizational categories)
- "Programming" and "JavaScript" (one is broader than the other)

Return an empty merges array if no clear merges are warranted."#;

pub(crate) fn merge_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "merges": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "winner_name": {
                            "type": "string",
                            "description": "Name of the tag that should survive (canonical name)"
                        },
                        "loser_name": {
                            "type": "string",
                            "description": "Name of the tag to merge into the winner and delete"
                        },
                        "reason": {
                            "type": "string",
                            "description": "Brief explanation for why these tags should be merged"
                        }
                    },
                    "required": ["winner_name", "loser_name", "reason"],
                    "additionalProperties": false
                },
                "description": "List of tag pairs to merge"
            }
        },
        "required": ["merges"],
        "additionalProperties": false
    })
}

fn is_descendant_of(
    conn: &Connection,
    potential_child: &str,
    potential_parent: &str,
) -> Result<bool, String> {
    let mut current = potential_child.to_string();
    let mut visited = HashSet::new();

    loop {
        if current == potential_parent {
            return Ok(true);
        }
        if visited.contains(&current) {
            return Ok(false);
        }
        visited.insert(current.clone());

        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM tags WHERE id = ?1",
                [&current],
                |row| row.get(0),
            )
            .ok()
            .and_then(|opt| opt);

        match parent {
            Some(p) => current = p,
            None => return Ok(false),
        }
    }
}

fn get_tag_id_by_name(conn: &Connection, name: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM tags WHERE LOWER(name) = LOWER(?1)",
        [name.trim()],
        |row| row.get(0),
    )
    .ok()
}

fn get_all_tags_for_llm(conn: &Connection) -> Result<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.name, p.name as parent_name
             FROM tags t
             LEFT JOIN tags p ON t.parent_id = p.id
             ORDER BY COALESCE(p.name, t.name), t.name",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let tags: Vec<(String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("Failed to query tags: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect tags: {}", e))?;

    if tags.is_empty() {
        return Ok("(no existing tags)".to_string());
    }

    let mut result = String::new();
    let mut current_parent: Option<String> = None;

    for (name, parent) in tags {
        match (&parent, &current_parent) {
            (Some(p), Some(cp)) if p == cp => {
                result.push_str(&format!("  - {}\n", name));
            }
            (Some(p), _) => {
                result.push_str(&format!("{}\n", p));
                result.push_str(&format!("  - {}\n", name));
                current_parent = Some(p.clone());
            }
            (None, _) => {
                result.push_str(&format!("{}\n", name));
                current_parent = None;
            }
        }
    }

    Ok(result.trim_end().to_string())
}

async fn get_merge_suggestions(
    provider_config: &ProviderConfig,
    tag_tree: &str,
    model: &str,
    supported_params: Option<Vec<String>>,
) -> Result<MergeResult, String> {
    let user_content = format!(
        "FULL TAG HIERARCHY:\n{}\n\nAnalyze this tag hierarchy and identify any tags that are duplicates or too similar and should be merged together.",
        tag_tree
    );

    let messages = vec![
        Message::system(MERGE_SYSTEM_PROMPT),
        Message::user(user_content),
    ];

    let mut params = GenerationParams::new()
        .with_temperature(0.1)
        .with_minimize_reasoning(true);
    if let Some(supported) = supported_params {
        params = params.with_supported_parameters(supported);
    }

    let call = StructuredCall::<MergeResult>::new(
        provider_config,
        model,
        &messages,
        "merge_result",
        merge_schema(),
    )
    .with_params(params)
    // `{"merges": []}` is a legal answer, so a constrained decoder can return
    // it and compaction silently does nothing.
    .with_schema_enforcement(GENERATIVE_CALL_ENFORCEMENT)
    .with_max_retries(3);

    match call_structured::<MergeResult>(call).await {
        Ok(result) => Ok(result),
        Err(e) => {
            tracing::error!(model = %model, error = %e, "Tag merge call failed");
            Err(e.to_compact_string())
        }
    }
}

fn execute_tag_merge(conn: &Connection, merge: &TagMerge) -> Result<(bool, i32), String> {
    let winner_id = match get_tag_id_by_name(conn, &merge.winner_name) {
        Some(id) => id,
        None => {
            tracing::warn!(winner = %merge.winner_name, "Skipping merge: winner not found");
            return Ok((false, 0));
        }
    };

    let loser_id = match get_tag_id_by_name(conn, &merge.loser_name) {
        Some(id) => id,
        None => {
            tracing::warn!(loser = %merge.loser_name, "Skipping merge: loser not found");
            return Ok((false, 0));
        }
    };

    if winner_id == loser_id {
        tracing::warn!(
            winner = %merge.winner_name,
            loser = %merge.loser_name,
            "Skipping merge: same tag"
        );
        return Ok((false, 0));
    }

    if is_descendant_of(conn, &loser_id, &winner_id)? {
        tracing::warn!(
            loser = %merge.loser_name,
            winner = %merge.winner_name,
            "Skipping merge: loser is a descendant of winner"
        );
        return Ok((false, 0));
    }
    if is_descendant_of(conn, &winner_id, &loser_id)? {
        tracing::warn!(
            winner = %merge.winner_name,
            loser = %merge.loser_name,
            "Skipping merge: winner is a descendant of loser"
        );
        return Ok((false, 0));
    }

    let atoms_with_loser: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT atom_id FROM atom_tags WHERE tag_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let results: Vec<String> = stmt
            .query_map([&loser_id], |row| row.get(0))
            .map_err(|e| format!("Failed to query atoms: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect atoms: {}", e))?;
        results
    };

    let mut atoms_retagged = 0;
    for atom_id in &atoms_with_loser {
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO atom_tags (atom_id, tag_id, source) VALUES (?1, ?2, 'auto')",
                rusqlite::params![atom_id, &winner_id],
            )
            .map_err(|e| format!("Failed to add winner tag: {}", e))?;

        if inserted > 0 {
            atoms_retagged += 1;
        }
    }

    conn.execute(
        "UPDATE tags SET parent_id = ?1 WHERE parent_id = ?2",
        rusqlite::params![&winner_id, &loser_id],
    )
    .map_err(|e| format!("Failed to reparent children: {}", e))?;

    conn.execute(
        "DELETE FROM tags WHERE id = ?1",
        rusqlite::params![&loser_id],
    )
    .map_err(|e| format!("Failed to delete loser tag: {}", e))?;

    tracing::info!(
        loser = %merge.loser_name,
        winner = %merge.winner_name,
        atoms_retagged,
        reason = %merge.reason,
        "Merged tags"
    );

    Ok((true, atoms_retagged))
}

fn apply_merges(conn: &Connection, merges: &[TagMerge]) -> (i32, i32, Vec<String>) {
    let mut tags_merged = 0;
    let mut atoms_retagged = 0;
    let mut errors = Vec::new();

    for merge in merges {
        match execute_tag_merge(conn, merge) {
            Ok((true, retagged)) => {
                tags_merged += 1;
                atoms_retagged += retagged;
            }
            Ok((false, _)) => {}
            Err(e) => errors.push(format!(
                "Error merging '{}' -> '{}': {}",
                merge.loser_name, merge.winner_name, e
            )),
        }
    }

    (tags_merged, atoms_retagged, errors)
}

/// Read all tags formatted for LLM
pub fn read_all_tags(conn: &Connection) -> Result<String, String> {
    get_all_tags_for_llm(conn)
}

/// Apply merge operations to the database
pub fn apply_merge_operations(conn: &Connection, merges: &[TagMerge]) -> (i32, i32, Vec<String>) {
    apply_merges(conn, merges)
}

/// Fetch merge suggestions from LLM
pub async fn fetch_merge_suggestions(
    provider_config: &ProviderConfig,
    tag_tree: &str,
    model: &str,
    supported_params: Option<Vec<String>>,
) -> Result<MergeResult, String> {
    get_merge_suggestions(provider_config, tag_tree, model, supported_params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::providers::structured::lint_schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (Database, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open_or_create(temp_file.path()).unwrap();
        (db, temp_file)
    }

    // ==================== Schema lint regression test ====================

    #[test]
    fn lint_merge_schema_is_portable() {
        lint_schema(&merge_schema()).expect("merge_schema must be portable across providers");
    }

    fn insert_tag(conn: &Connection, id: &str, name: &str, parent_id: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tags (id, name, parent_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, parent_id, now],
        )
        .unwrap();
    }

    fn insert_atom(conn: &Connection, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO atoms (id, content, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, "test content", now, now],
        )
        .unwrap();
    }

    fn link_atom_tag(conn: &Connection, atom_id: &str, tag_id: &str) {
        conn.execute(
            "INSERT INTO atom_tags (atom_id, tag_id) VALUES (?1, ?2)",
            rusqlite::params![atom_id, tag_id],
        )
        .unwrap();
    }

    #[test]
    fn test_read_all_tags_formatted() {
        let (db, _temp) = create_test_db();
        let conn = db.conn.lock().unwrap();

        // Create a parent tag and child
        insert_tag(&conn, "parent1", "Topics", None);
        insert_tag(&conn, "child1", "AI", Some("parent1"));
        insert_tag(&conn, "child2", "ML", Some("parent1"));

        let result = read_all_tags(&conn).unwrap();

        // Should show hierarchical structure
        assert!(result.contains("Topics"), "Should contain parent tag");
        assert!(result.contains("AI"), "Should contain child tag AI");
        assert!(result.contains("ML"), "Should contain child tag ML");
    }

    #[test]
    fn test_execute_tag_merge_success() {
        let (db, _temp) = create_test_db();
        let conn = db.conn.lock().unwrap();

        // Create two tags and an atom
        insert_tag(&conn, "tag1", "React", None);
        insert_tag(&conn, "tag2", "ReactJS", None);
        insert_atom(&conn, "atom1");
        link_atom_tag(&conn, "atom1", "tag2"); // Atom has loser tag

        let merge = TagMerge {
            winner_name: "React".to_string(),
            loser_name: "ReactJS".to_string(),
            reason: "Same framework".to_string(),
        };

        let (tags_merged, atoms_retagged, errors) = apply_merge_operations(&conn, &[merge]);

        assert_eq!(tags_merged, 1, "Should have merged 1 tag");
        assert_eq!(atoms_retagged, 1, "Should have retagged 1 atom");
        assert!(errors.is_empty(), "Should have no errors");

        // Verify loser tag is deleted
        let loser_exists: bool = conn
            .query_row("SELECT 1 FROM tags WHERE name = 'ReactJS'", [], |_| {
                Ok(true)
            })
            .unwrap_or(false);
        assert!(!loser_exists, "Loser tag should be deleted");

        // Verify atom now has winner tag
        let has_winner: bool = conn
            .query_row(
                "SELECT 1 FROM atom_tags WHERE atom_id = 'atom1' AND tag_id = 'tag1'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(has_winner, "Atom should have winner tag");
    }

    #[test]
    fn test_execute_tag_merge_ancestor_descendant() {
        let (db, _temp) = create_test_db();
        let conn = db.conn.lock().unwrap();

        // Create parent and child tags
        insert_tag(&conn, "parent", "Topics", None);
        insert_tag(&conn, "child", "AI", Some("parent"));

        // Try to merge parent into child (should be blocked)
        let merge = TagMerge {
            winner_name: "AI".to_string(),
            loser_name: "Topics".to_string(),
            reason: "Invalid merge".to_string(),
        };

        let (tags_merged, atoms_retagged, errors) = apply_merge_operations(&conn, &[merge]);

        assert_eq!(tags_merged, 0, "Should not merge ancestor into descendant");
        assert_eq!(atoms_retagged, 0, "No atoms should be retagged");
        assert!(errors.is_empty(), "Skipped merges aren't errors");

        // Verify both tags still exist
        let parent_exists: bool = conn
            .query_row("SELECT 1 FROM tags WHERE name = 'Topics'", [], |_| Ok(true))
            .unwrap_or(false);
        assert!(parent_exists, "Parent tag should still exist");
    }
}
