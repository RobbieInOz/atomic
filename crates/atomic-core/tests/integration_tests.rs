//! Integration tests that exercise AtomicCore end-to-end with real databases.
//!
//! These tests verify that the full stack works: AtomicCore → StorageBackend → database.
//! They cover the same workflows a user would hit: creating atoms with tags,
//! querying them back, updating, deleting, and verifying consistency.
//!
//! SQLite tests always run. These use real SQLite files (not mocks).

use atomic_core::models::*;
use atomic_core::{AtomicCore, CreateAtomRequest, ListAtomsParams, UpdateAtomRequest};
use tempfile::TempDir;

fn create_core() -> (AtomicCore, TempDir) {
    let dir = TempDir::new().unwrap();
    let core = AtomicCore::open_or_create(dir.path().join("test.db")).unwrap();
    (core, dir)
}

async fn create_atom(core: &AtomicCore, content: &str) -> AtomWithTags {
    core.create_atom(
        CreateAtomRequest {
            content: content.to_string(),
            ..Default::default()
        },
        |_| {},
    )
    .await
    .unwrap()
    .unwrap()
}

// ==================== Atom lifecycle ====================

#[tokio::test]
async fn test_atom_create_read_update_delete() {
    let (core, _dir) = create_core();

    // Create
    let atom = create_atom(&core, "# Hello\n\nOriginal content").await;
    assert_eq!(atom.atom.embedding_status, "pending");
    assert!(!atom.atom.id.is_empty());

    // Read
    let fetched = core.get_atom(&atom.atom.id).await.unwrap().unwrap();
    assert_eq!(fetched.atom.content, "# Hello\n\nOriginal content");
    assert_eq!(fetched.atom.title, "Hello");

    // Update
    let updated = core
        .update_atom(
            &atom.atom.id,
            UpdateAtomRequest {
                content: "# Updated\n\nNew content".to_string(),
                source_url: None,
                published_at: None,
                tag_ids: None,
            },
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(updated.atom.content, "# Updated\n\nNew content");
    assert_eq!(updated.atom.title, "Updated");

    // Verify update persisted
    let re_fetched = core.get_atom(&atom.atom.id).await.unwrap().unwrap();
    assert_eq!(re_fetched.atom.content, "# Updated\n\nNew content");

    // Delete
    core.delete_atom(&atom.atom.id).await.unwrap();
    assert!(core.get_atom(&atom.atom.id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_bulk_create_with_dedup() {
    let (core, _dir) = create_core();

    let requests = vec![
        CreateAtomRequest {
            content: "Atom 1".to_string(),
            source_url: Some("https://example.com/1".to_string()),
            skip_if_source_exists: true,
            ..Default::default()
        },
        CreateAtomRequest {
            content: "Atom 2".to_string(),
            source_url: Some("https://example.com/2".to_string()),
            skip_if_source_exists: true,
            ..Default::default()
        },
    ];

    let result = core
        .create_atoms_bulk(requests.clone(), |_| {})
        .await
        .unwrap();
    assert_eq!(result.count, 2);
    assert_eq!(result.skipped, 0);

    // Creating again with same source_urls should skip
    let result2 = core.create_atoms_bulk(requests, |_| {}).await.unwrap();
    assert_eq!(result2.count, 0);
    assert_eq!(result2.skipped, 2);
}

// ==================== Tags ====================

#[tokio::test]
async fn test_tag_hierarchy_and_atom_association() {
    let (core, _dir) = create_core();

    // Create parent and child tags
    let parent = core.create_tag("Science", None).await.unwrap();
    let child = core.create_tag("Physics", Some(&parent.id)).await.unwrap();

    // Create atom with child tag
    let atom = core
        .create_atom(
            CreateAtomRequest {
                content: "Quantum mechanics".to_string(),
                tag_ids: vec![child.id.clone()],
                ..Default::default()
            },
            |_| {},
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(atom.tags.len(), 1);
    assert_eq!(atom.tags[0].name, "Physics");

    // get_atoms_by_tag on parent should include child's atoms
    let by_parent = core
        .get_atoms_by_tag(&parent.id, &atomic_core::models::KindFilter::All)
        .await
        .unwrap();
    assert_eq!(by_parent.len(), 1);
    assert_eq!(by_parent[0].atom.id, atom.atom.id);

    // get_atoms_by_tag on child directly
    let by_child = core
        .get_atoms_by_tag(&child.id, &atomic_core::models::KindFilter::All)
        .await
        .unwrap();
    assert_eq!(by_child.len(), 1);

    // Update tag name
    let updated = core
        .update_tag(&child.id, "Quantum Physics", Some(&parent.id))
        .await
        .unwrap();
    assert_eq!(updated.name, "Quantum Physics");

    // Delete child tag shouldn't delete the atom
    core.delete_tag(&child.id, false).await.unwrap();
    let atom_still_exists = core.get_atom(&atom.atom.id).await.unwrap();
    assert!(atom_still_exists.is_some());
    assert!(atom_still_exists.unwrap().tags.is_empty());
}

// ==================== Pagination ====================

#[tokio::test]
async fn test_list_atoms_offset_and_cursor_pagination() {
    let (core, _dir) = create_core();

    // Create 10 atoms
    for i in 0..10 {
        create_atom(&core, &format!("Atom number {}", i)).await;
    }

    // Offset pagination: page 1
    let page1 = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 3,
                offset: 0,
                cursor: None,
                cursor_id: None,
                source_filter: SourceFilter::All,
                source_value: None,
                sort_by: SortField::Updated,
                sort_order: SortOrder::Desc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();
    assert_eq!(page1.atoms.len(), 3);
    assert_eq!(page1.total_count, 10);
    assert!(page1.next_cursor.is_some());
    assert!(page1.next_cursor_id.is_some());

    // Cursor pagination: page 2 using cursor from page 1
    let page2 = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 3,
                offset: 0,
                cursor: page1.next_cursor,
                cursor_id: page1.next_cursor_id,
                source_filter: SourceFilter::All,
                source_value: None,
                sort_by: SortField::Updated,
                sort_order: SortOrder::Desc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();
    assert_eq!(page2.atoms.len(), 3);

    // Pages should not overlap
    let page1_ids: Vec<&str> = page1.atoms.iter().map(|a| a.id.as_str()).collect();
    for a in &page2.atoms {
        assert!(!page1_ids.contains(&a.id.as_str()), "Page overlap detected");
    }
}

#[tokio::test]
async fn test_list_atoms_sort_fields() {
    let (core, _dir) = create_core();

    create_atom(&core, "# Banana\n\nContent").await;
    create_atom(&core, "# Apple\n\nContent").await;
    create_atom(&core, "# Cherry\n\nContent").await;

    // Sort by title ascending
    let result = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 10,
                offset: 0,
                cursor: None,
                cursor_id: None,
                source_filter: SourceFilter::All,
                source_value: None,
                sort_by: SortField::Title,
                sort_order: SortOrder::Asc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();

    let titles: Vec<&str> = result.atoms.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Apple", "Banana", "Cherry"]);
}

// ==================== Chat ====================

#[tokio::test]
async fn test_conversation_lifecycle() {
    let (core, _dir) = create_core();

    let tag = core.create_tag("Chat Topic", None).await.unwrap();

    // Create conversation with scope
    let conv = core
        .create_conversation(&[tag.id.clone()], Some("Test Chat"))
        .await
        .unwrap();
    assert_eq!(conv.conversation.title.as_deref(), Some("Test Chat"));
    assert_eq!(conv.tags.len(), 1);

    // List conversations
    let convs = core.get_conversations(None, 10, 0, false).await.unwrap();
    assert_eq!(convs.len(), 1);

    // Update title
    let updated = core
        .update_conversation(&conv.conversation.id, Some("Renamed"), None)
        .await
        .unwrap();
    assert_eq!(updated.title.as_deref(), Some("Renamed"));

    // Delete
    core.delete_conversation(&conv.conversation.id)
        .await
        .unwrap();
    assert!(core
        .get_conversation(&conv.conversation.id)
        .await
        .unwrap()
        .is_none());
}

// ==================== Wiki ====================

#[tokio::test]
async fn test_wiki_article_lifecycle() {
    let (core, _dir) = create_core();

    let tag = core.create_tag("Wiki Topic", None).await.unwrap();

    // Check status before article exists
    let status = core.get_wiki_status(&tag.id).await.unwrap();
    assert!(!status.has_article);

    // No article yet
    assert!(core.get_wiki(&tag.id).await.unwrap().is_none());

    // Delete non-existent (should not error)
    core.delete_wiki(&tag.id).await.unwrap();
}

// ==================== Source filtering ====================

#[tokio::test]
async fn test_source_url_tracking() {
    let (core, _dir) = create_core();

    core.create_atom(
        CreateAtomRequest {
            content: "From the web".to_string(),
            source_url: Some("https://example.com/article".to_string()),
            published_at: None,
            tag_ids: vec![],
            ..Default::default()
        },
        |_| {},
    )
    .await
    .unwrap();

    create_atom(&core, "Manual note").await;

    let sources = core.get_source_list().await.unwrap();
    assert!(sources.iter().any(|s| s.source == "example.com"));

    // Filter to external only
    let external = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 10,
                offset: 0,
                cursor: None,
                cursor_id: None,
                source_filter: SourceFilter::External,
                source_value: None,
                sort_by: SortField::Updated,
                sort_order: SortOrder::Desc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();
    assert_eq!(external.total_count, 1);

    // Filter to manual only
    let manual = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 10,
                offset: 0,
                cursor: None,
                cursor_id: None,
                source_filter: SourceFilter::Manual,
                source_value: None,
                sort_by: SortField::Updated,
                sort_order: SortOrder::Desc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();
    assert_eq!(manual.total_count, 1);
}

// ==================== Settings + Tokens ====================

#[tokio::test]
async fn test_settings_roundtrip() {
    let (core, _dir) = create_core();

    // Default settings should exist
    let settings = core.get_settings().await.unwrap();
    assert!(settings.contains_key("provider"));

    // Set and get
    core.set_setting("test_key", "test_value").await.unwrap();
    let settings = core.get_settings().await.unwrap();
    assert_eq!(settings.get("test_key").unwrap(), "test_value");
}

#[tokio::test]
async fn test_token_lifecycle() {
    let (core, _dir) = create_core();

    let (info, raw) = core.create_api_token("integration-test").await.unwrap();
    core.create_api_token("replacement").await.unwrap();
    assert!(raw.starts_with("at_"));

    // Verify
    let verified = core.verify_api_token(&raw).await.unwrap();
    assert!(verified.is_some());
    assert_eq!(verified.unwrap().id, info.id);

    // List
    let tokens = core.list_api_tokens().await.unwrap();
    assert!(tokens.iter().any(|t| t.id == info.id));

    // Revoke
    core.revoke_api_token(&info.id).await.unwrap();
    assert!(core.verify_api_token(&raw).await.unwrap().is_none());
}

// ==================== Edge cases ====================

#[tokio::test]
async fn test_get_nonexistent_atom_returns_none() {
    let (core, _dir) = create_core();
    assert!(core.get_atom("does-not-exist").await.unwrap().is_none());
}

#[tokio::test]
async fn test_empty_database_queries() {
    let (core, _dir) = create_core();

    assert!(core.get_all_atoms().await.unwrap().is_empty());
    let page = core
        .list_atoms(
            &ListAtomsParams {
                tag_id: None,
                limit: 10,
                offset: 0,
                cursor: None,
                cursor_id: None,
                source_filter: SourceFilter::All,
                source_value: None,
                sort_by: SortField::Updated,
                sort_order: SortOrder::Desc,
            },
            &atomic_core::models::KindFilter::All,
        )
        .await
        .unwrap();
    assert_eq!(page.total_count, 0);
    assert!(core.get_source_list().await.unwrap().is_empty());
    assert!(core
        .get_conversations(None, 10, 0, false)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_atom_positions_roundtrip() {
    let (core, _dir) = create_core();

    let atom = create_atom(&core, "Canvas atom").await;

    core.save_atom_positions(&[AtomPosition {
        atom_id: atom.atom.id.clone(),
        x: 100.5,
        y: 200.3,
    }])
    .await
    .unwrap();

    let positions = core.get_atom_positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].atom_id, atom.atom.id);
    assert!((positions[0].x - 100.5).abs() < 0.01);
    assert!((positions[0].y - 200.3).abs() < 0.01);
}
