//! Shared infrastructure for pipeline integration tests.
//!
//! The wiremock-backed `MockAiServer` and the Postgres truncation helper
//! both live in the workspace's `atomic-test-support` crate so atomic-server
//! can reuse them without duplication. This file owns only the pieces tied
//! to atomic-core's concrete `AtomicCore` shape: the `Backend` switch, the
//! `setup_core` / `open_bare` constructors, chunk-id / pipeline-job helpers,
//! and the `EmbeddingEvent` awaiter.

#![allow(dead_code)] // Referenced by multiple test binaries; some helpers are per-test.

use std::sync::Arc;
use std::time::Duration;

use atomic_core::AtomicCore;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

// Re-export the mock + constants so existing test code keeps using the
// `support::MockAiServer` / `support::EMBED_DIM` paths it already imports.
// `unused_imports` is allowed because each integration-test binary compiles
// this module fresh and only some of them use the mock surface — re-exports
// look unused to a binary that doesn't reach into them.
#[allow(unused_imports)]
pub use atomic_test_support::{MockAiServer, EDGE_SIMILARITY_THRESHOLD, EMBED_DIM};

#[cfg(feature = "postgres")]
#[allow(unused_imports)]
pub use atomic_test_support::truncate_postgres_for_test;

// ==================== Backend switch + test harness ====================

pub enum Backend {
    Sqlite,
    #[cfg(feature = "postgres")]
    Postgres,
}

/// Per-test resources that must outlive the `AtomicCore`. Drop order matters
/// — the temp dir needs to live until after the core is dropped (SQLite has
/// the DB file open). For Postgres, holding nothing extra is fine.
pub struct CoreHandle {
    pub core: AtomicCore,
    _tempdir: Option<TempDir>,
}

/// Build an `AtomicCore` on the chosen backend and wire it up to the mock:
///
/// 1. Open a fresh DB (SQLite temp dir / Postgres truncated).
/// 2. Seed settings pointing at the mock's base URL with the
///    `openai_compat` provider selected.
/// 3. Seed a single auto-tag-target category ("Topics") so the tagging
///    path runs instead of short-circuiting on an empty tag tree.
///
/// Postgres: returns `None` if `ATOMIC_TEST_DATABASE_URL` isn't set so callers
/// can gracefully skip the test on CI configurations without a database.
pub async fn setup_core(backend: Backend, mock_url: &str) -> Option<CoreHandle> {
    let (core, tempdir) = match backend {
        Backend::Sqlite => {
            let dir = TempDir::new().expect("create tempdir");
            let core =
                AtomicCore::open_or_create(dir.path().join("pipeline.db")).expect("open sqlite");
            (core, Some(dir))
        }
        #[cfg(feature = "postgres")]
        Backend::Postgres => {
            let url = std::env::var("ATOMIC_TEST_DATABASE_URL").ok()?;
            // Fresh schema per test run — truncate leaves the schema intact
            // but wipes seeded tags/settings so `open_postgres` re-seeds.
            truncate_postgres_for_test(&url).await;
            let core = AtomicCore::open_postgres(&url, "pipeline_test", None)
                .await
                .expect("open postgres");
            (core, None)
        }
    };

    // Point the pipeline at the mock HTTP server.
    for (k, v) in [
        ("provider", "openai_compat"),
        ("openai_compat_base_url", mock_url),
        ("openai_compat_api_key", "test-key"),
        ("openai_compat_embedding_model", "mock-embed"),
        ("openai_compat_llm_model", "mock-llm"),
        ("openai_compat_embedding_dimension", "1536"),
        ("auto_tagging_enabled", "true"),
    ] {
        core.set_setting(k, v).await.expect("seed test setting");
    }

    // Ensure at least one top-level auto-tag target exists so
    // `get_tag_tree_for_llm` returns a non-empty tree and the tagging path
    // actually runs. For SQLite we start with an empty tags table; for
    // Postgres `open_postgres` seeds default categories but leaves the
    // is_autotag_target flag off.
    core.configure_autotag_targets(&["Topics".to_string()], &[])
        .await
        .expect("configure autotag targets");

    Some(CoreHandle {
        core,
        _tempdir: tempdir,
    })
}

/// Open a fresh core on either backend without seeding provider settings.
/// Used by tests that need to exercise the no-provider failure path — the
/// "happy path" `setup_core` plumbs a working mock provider in.
pub async fn open_bare(backend: Backend) -> Option<CoreHandle> {
    match backend {
        Backend::Sqlite => {
            let dir = TempDir::new().expect("create tempdir");
            let core = AtomicCore::open_or_create(dir.path().join("pipeline.db"))
                .expect("open sqlite test db");
            Some(CoreHandle {
                core,
                _tempdir: Some(dir),
            })
        }
        #[cfg(feature = "postgres")]
        Backend::Postgres => {
            let url = std::env::var("ATOMIC_TEST_DATABASE_URL").ok()?;
            truncate_postgres_for_test(&url).await;
            let core = AtomicCore::open_postgres(&url, "pipeline_test", None)
                .await
                .expect("open postgres");
            Some(CoreHandle {
                core,
                _tempdir: None,
            })
        }
    }
}

/// Return chunk IDs for an atom, ordered by chunk_index. Cross-backend so the
/// same assertion ("chunks preserved across a re-embed") works against both
/// SQLite and Postgres.
pub async fn chunk_ids_for_atom(core: &AtomicCore, atom_id: &str) -> Vec<String> {
    if core.database().is_some() {
        let conn = rusqlite::Connection::open(core.db_path()).expect("open sqlite db");
        let mut stmt = conn
            .prepare("SELECT id FROM atom_chunks WHERE atom_id = ?1 ORDER BY chunk_index")
            .expect("prepare chunk query");
        stmt.query_map([atom_id], |row| row.get::<_, String>(0))
            .expect("query chunk ids")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect chunk ids")
    } else {
        #[cfg(feature = "postgres")]
        {
            use sqlx::postgres::PgPoolOptions;
            let url =
                std::env::var("ATOMIC_TEST_DATABASE_URL").expect("ATOMIC_TEST_DATABASE_URL unset");
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect chunk-id pool");
            let rows: Vec<(String,)> = sqlx::query_as(
                "SELECT id FROM atom_chunks WHERE atom_id = $1 ORDER BY chunk_index",
            )
            .bind(atom_id)
            .fetch_all(&pool)
            .await
            .expect("query chunk ids");
            rows.into_iter().map(|(id,)| id).collect()
        }
        #[cfg(not(feature = "postgres"))]
        panic!("Postgres backend reached without postgres feature");
    }
}

/// Count rows in `atom_pipeline_jobs`. Used by tests that assert the ledger
/// is cleared after terminal states fire.
pub async fn pending_pipeline_job_count(core: &AtomicCore) -> i64 {
    if core.database().is_some() {
        let conn = rusqlite::Connection::open(core.db_path()).expect("open sqlite db");
        conn.query_row("SELECT COUNT(*) FROM atom_pipeline_jobs", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("count pipeline jobs")
    } else {
        #[cfg(feature = "postgres")]
        {
            use sqlx::postgres::PgPoolOptions;
            let url =
                std::env::var("ATOMIC_TEST_DATABASE_URL").expect("ATOMIC_TEST_DATABASE_URL unset");
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect job-count pool");
            sqlx::query_scalar("SELECT COUNT(*) FROM atom_pipeline_jobs")
                .fetch_one(&pool)
                .await
                .expect("count pipeline jobs")
        }
        #[cfg(not(feature = "postgres"))]
        panic!("Postgres backend reached without postgres feature");
    }
}

// ==================== Pipeline completion awaiter ====================

/// Event channel returned to a test so it can await specific pipeline
/// milestones without sprinkling `sleep`s.
pub type EventRx = UnboundedReceiver<atomic_core::EmbeddingEvent>;

/// Make an `on_event` callback that forwards every event into a channel.
/// Returns the callback (to hand to `create_atom`) and the receiver (to poll
/// in the test). The callback is `Arc`-backed because `create_atom`'s bound
/// is `Fn + Send + Sync + 'static`.
pub fn event_collector() -> (
    impl Fn(atomic_core::EmbeddingEvent) + Send + Sync + Clone + 'static,
    EventRx,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx = Arc::new(tx);
    let cb = move |ev| {
        let _ = tx.send(ev);
    };
    (cb, rx)
}

/// Wait until both `EmbeddingComplete`, a terminal tagging event
/// (`TaggingComplete` / `TaggingSkipped` / `TaggingFailed`), and the owning
/// queue run's completion have fired. Returns the captured target-atom events
/// so tests can assert on payloads.
///
/// # Why this also watches durable state
///
/// A save's `on_event` sink is *not* guaranteed to observe that save's own
/// pipeline events. The job ledger is per-storage while workers are spawned
/// per-call, and a worker drains the ledger before releasing its permit — so
/// a still-draining worker from an *earlier* save on the same core can claim
/// this atom's job and emit into that earlier call's sink. The originating
/// call then claims an empty queue, returns 0 without spawning, and drops the
/// only sender this channel had.
///
/// Production never notices: every sink is process-wide (atomic-server's
/// broadcast channel, Tauri's `app_handle.emit`), so the events reach clients
/// either way. Only a per-call test channel can see the difference, and it
/// sees it as a closed channel. Falling back to the durable record keeps the
/// assertion honest — the work must still have completed — without asserting
/// a 1:1 call-to-sink binding the architecture doesn't promise.
pub async fn await_pipeline(
    core: &AtomicCore,
    rx: &mut EventRx,
    atom_id: &str,
) -> Vec<atomic_core::EmbeddingEvent> {
    use atomic_core::EmbeddingEvent;

    let mut captured = Vec::new();
    let mut embedding_done = false;
    let mut tagging_done = false;
    let mut queue_done = false;

    // A generous budget — the mock responds instantly, but CI runners can
    // stall under load. Fails loudly instead of hanging forever.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

    while !(embedding_done && tagging_done && queue_done) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "pipeline did not complete for {atom_id} within 15s. Captured: {:?}",
                captured
            );
        }

        let ev = match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(ev)) => ev,
            // Every sender is gone, so no further event can arrive on this
            // channel — another worker owns the run (see the doc comment).
            // Switch to the durable record for the rest of the budget.
            Ok(None) => {
                await_pipeline_via_storage(core, atom_id, deadline).await;
                return captured;
            }
            Err(_) => panic!(
                "timed out waiting for pipeline events for {atom_id}. Captured: {:?}",
                captured
            ),
        };

        let matches_target = match &ev {
            EmbeddingEvent::Started { atom_id: id }
            | EmbeddingEvent::EmbeddingComplete { atom_id: id }
            | EmbeddingEvent::EmbeddingFailed { atom_id: id, .. }
            | EmbeddingEvent::TaggingComplete { atom_id: id, .. }
            | EmbeddingEvent::TaggingSkipped { atom_id: id }
            | EmbeddingEvent::TaggingFailed { atom_id: id, .. } => id == atom_id,
            EmbeddingEvent::BatchProgress { .. }
            | EmbeddingEvent::PipelineQueueStarted { .. }
            | EmbeddingEvent::PipelineQueueProgress { .. } => false,
            EmbeddingEvent::PipelineQueueCompleted { .. } => {
                queue_done = true;
                false
            }
        };

        if matches_target {
            match &ev {
                EmbeddingEvent::EmbeddingComplete { .. } => embedding_done = true,
                EmbeddingEvent::EmbeddingFailed { error, .. } => {
                    panic!("embedding failed for {atom_id}: {error}")
                }
                EmbeddingEvent::TaggingComplete { .. } | EmbeddingEvent::TaggingSkipped { .. } => {
                    tagging_done = true
                }
                EmbeddingEvent::TaggingFailed { error, .. } => {
                    panic!("tagging failed for {atom_id}: {error}")
                }
                _ => {}
            }
            captured.push(ev);
        }
    }

    captured
}

/// Poll the atom's durable pipeline status until both halves are terminal.
///
/// The fallback for [`await_pipeline`] when its event channel closes because
/// another worker claimed the run. Asserts the same outcomes the event path
/// does — embedding complete, tagging complete or deliberately skipped — and
/// fails just as loudly on a recorded failure.
async fn await_pipeline_via_storage(
    core: &AtomicCore,
    atom_id: &str,
    deadline: tokio::time::Instant,
) {
    loop {
        let atom = core
            .get_atom(atom_id)
            .await
            .expect("read atom while awaiting pipeline")
            .expect("atom exists while awaiting pipeline")
            .atom;

        match atom.embedding_status.as_str() {
            "failed" => panic!("embedding failed for {atom_id} (recorded on the atom)"),
            "complete" => match atom.tagging_status.as_str() {
                "failed" => panic!("tagging failed for {atom_id} (recorded on the atom)"),
                "complete" | "skipped" => return,
                _ => {}
            },
            _ => {}
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "pipeline did not complete for {atom_id} before the deadline (its event channel \
                 closed, so another worker owned the run; last seen embedding_status={:?} \
                 tagging_status={:?})",
                atom.embedding_status, atom.tagging_status
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
