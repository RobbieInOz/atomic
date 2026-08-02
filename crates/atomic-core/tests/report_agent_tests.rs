//! Report-run contracts that only show up end to end: which tools the
//! research loop actually executes, what the final pass is handed, how a run
//! ends, and which numbered sources survive into the stored finding.
//!
//! They run against the wiremock provider (`support::MockAiServer`), whose
//! non-streaming tool leg is scripted per turn (`set_research_tool_rounds`),
//! so a run exercises the real tool dispatch, the real citation numbering,
//! and the real long-form final pass. Scheduled reports must behave
//! identically across refactors of the loop underneath them — that is what
//! this file pins.

mod support;

use atomic_core::reports::RunOutcome;
use atomic_core::{
    AtomicCore, CitationPolicy, CreateAtomRequest, CreateReportRequest, Report,
    ReportFindingCitation,
};
use serde_json::{json, Value};
use support::{await_pipeline, event_collector, setup_core, Backend, MockAiServer};

/// Create an atom and wait for its pipeline, so report scope resolution and
/// the agent's `semantic_search` tool both see it.
async fn seed_atom(core: &AtomicCore, content: &str, tag_ids: &[&str]) -> String {
    let (callback, mut events) = event_collector();
    let atom = core
        .create_atom(
            CreateAtomRequest {
                content: content.to_string(),
                tag_ids: tag_ids.iter().map(|id| id.to_string()).collect(),
                ..Default::default()
            },
            callback,
        )
        .await
        .expect("create atom")
        .expect("atom was inserted");
    await_pipeline(core, &mut events, &atom.atom.id).await;
    atom.atom.id
}

/// A report scoped to `tag_id`. Everything else is the authoring default —
/// context scope "all", captured kinds only — so the tests differ only in the
/// knobs they name.
async fn create_report(
    core: &AtomicCore,
    tag_id: &str,
    citation_policy: CitationPolicy,
    max_tool_iterations: Option<i32>,
) -> Report {
    core.create_report(CreateReportRequest {
        name: "Pelican Watch".to_string(),
        research_prompt: "Summarize what is known about pelicans.".to_string(),
        source_scope_tag_ids: vec![tag_id.to_string()],
        citation_policy,
        max_tool_iterations,
        schedule: "0 0 * * * *".to_string(),
        ..Default::default()
    })
    .await
    .expect("create report")
}

async fn run_to_finding(
    core: &AtomicCore,
    report: &Report,
) -> (String, Vec<ReportFindingCitation>) {
    let outcome = core.run_report_now(&report.id).await.expect("run report");
    let atom_id = match outcome {
        RunOutcome::Succeeded { finding_atom_id } => finding_atom_id,
        other => panic!("expected a finding, got {other:?}"),
    };
    let citations = core
        .list_citations_for_finding(&atom_id)
        .await
        .expect("load finding citations");
    (atom_id, citations)
}

/// The messages of the run's last chat request — the transcript the final
/// pass was asked to write the report from, tool results included.
fn final_pass_messages(mock: &MockAiServer) -> Vec<Value> {
    let bodies = mock.chat_request_bodies();
    let last = bodies.last().expect("at least one chat request");
    last["messages"].as_array().expect("messages array").clone()
}

fn tool_results(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str().map(str::to_string))
        .collect()
}

/// A full research run: the agent reads a source atom, searches the context
/// corpus, calls `done`, and the final pass writes a finding whose `[N]`
/// marker becomes a stored citation pointing at a source atom.
#[tokio::test]
async fn agentic_run_researches_then_writes_a_cited_finding() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core
        .create_tag("ReportScope", None)
        .await
        .expect("create tag");
    // Seeded oldest first; the source list is newest-first, so `sources[0]`
    // is `[1]` in the prompt.
    let older = seed_atom(&core, "Pelicans nest in colonies.", &[tag.id.as_str()]).await;
    let newer = seed_atom(&core, "Pelicans dive for fish.", &[tag.id.as_str()]).await;
    let sources = [newer.clone(), older.clone()];
    // Untagged, so it is context rather than source: `source_only` must show
    // it to the agent without making it citable.
    seed_atom(&core, "Pelicans hold water in their pouches.", &[]).await;

    let report = create_report(&core, &tag.id, CitationPolicy::SourceOnly, None).await;
    mock.set_research_tool_rounds(vec![
        vec![("read_atom", json!({ "atom_id": newer, "limit": 10 }))],
        // Both calls in the final round must execute before the loop stops.
        vec![
            (
                "semantic_search",
                json!({ "query": "pelicans", "limit": 3 }),
            ),
            ("done", json!({})),
        ],
    ]);
    mock.reset_counts();

    let (_atom_id, citations) = run_to_finding(&core, &report).await;

    assert_eq!(
        mock.chat_request_count(),
        3,
        "two research turns and one final pass: {:?}",
        mock.chat_request_models()
    );

    let results = tool_results(&final_pass_messages(&mock));
    assert_eq!(results.len(), 3, "one result per tool call: {results:?}");
    assert!(
        results[0].contains("(lines 1-1 of 1)") && results[0].contains("Pelicans dive for fish."),
        "read_atom returns a line window of the atom: {:?}",
        results[0]
    );
    assert!(
        results[1].contains("(context only, not citable)")
            && results[1].contains("Pelicans hold water in their pouches."),
        "source_only shows context results without a citation number: {:?}",
        results[1]
    );
    assert!(
        !results[1].contains(&older) && !results[1].contains(&newer),
        "source atoms are excluded from their own report's context searches: {:?}",
        results[1]
    );
    assert_eq!(
        results[2], "Acknowledged. Write the report in your next message.",
        "done acknowledges and hands over to the final pass"
    );

    // The mock finding cites `[1]`, which is the first atom of the source
    // list — the numbering the prompt handed the model.
    assert_eq!(citations.len(), 1, "one marker, one row: {citations:?}");
    assert_eq!(citations[0].position, 1);
    assert_eq!(
        citations[0].cited_atom_id, sources[0],
        "[1] is the first source atom"
    );
    assert!(
        citations[0].excerpt.contains("Pelicans dive for fish."),
        "the stored excerpt is the source's own text: {:?}",
        citations[0].excerpt
    );
}

/// `done` ends research where it is called: the loop does not spend another
/// turn asking the model what to do next, it goes straight to the final pass.
#[tokio::test]
async fn done_ends_research_at_the_turn_it_is_called() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core
        .create_tag("ReportScope", None)
        .await
        .expect("create tag");
    seed_atom(&core, "Pelicans nest in colonies.", &[tag.id.as_str()]).await;

    // A generous cap the run must not use: `done` on the second turn is what
    // stops it.
    let report = create_report(&core, &tag.id, CitationPolicy::SourceOnly, Some(8)).await;
    mock.set_research_tool_rounds(vec![
        vec![("semantic_search", json!({ "query": "pelicans" }))],
        vec![("done", json!({}))],
    ]);
    mock.reset_counts();

    let (_atom_id, citations) = run_to_finding(&core, &report).await;

    assert_eq!(
        mock.chat_request_count(),
        3,
        "two research turns plus the final pass, not the eight-turn budget"
    );
    assert_eq!(citations.len(), 1, "the finding still cites its source");
}

/// Out of tool-call budget the run does not fail or salvage — it writes the
/// report from whatever it gathered, which is the contract the scheduler
/// relies on to keep producing findings.
#[tokio::test]
async fn exhausting_the_iteration_budget_still_writes_a_finding() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core
        .create_tag("ReportScope", None)
        .await
        .expect("create tag");
    seed_atom(&core, "Pelicans nest in colonies.", &[tag.id.as_str()]).await;

    let report = create_report(&core, &tag.id, CitationPolicy::SourceOnly, Some(2)).await;
    // A model that researches forever: the cap is the only thing that stops
    // it.
    mock.set_research_tool_rounds(vec![vec![(
        "semantic_search",
        json!({ "query": "pelicans" }),
    )]]);
    mock.set_research_force_tool_calls(true);
    mock.reset_counts();

    let (_atom_id, citations) = run_to_finding(&core, &report).await;

    assert_eq!(
        mock.chat_request_count(),
        3,
        "the cap allows two research turns; the final pass still runs"
    );
    let results = tool_results(&final_pass_messages(&mock));
    assert_eq!(
        results.len(),
        2,
        "the final pass sees both rounds of research: {results:?}"
    );
    assert_eq!(citations.len(), 1, "a capped run still produces citations");
}

/// `source_and_context` is the same loop with a different citation
/// admission: search results get the next free number and are advertised as
/// citable, where `source_only` refuses them one.
#[tokio::test]
async fn source_and_context_numbers_search_results_after_the_source_list() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core
        .create_tag("ReportScope", None)
        .await
        .expect("create tag");
    seed_atom(&core, "Pelicans nest in colonies.", &[tag.id.as_str()]).await;
    seed_atom(&core, "Pelicans dive for fish.", &[tag.id.as_str()]).await;
    seed_atom(&core, "Pelicans hold water in their pouches.", &[]).await;

    let report = create_report(&core, &tag.id, CitationPolicy::SourceAndContext, None).await;
    mock.set_research_tool_rounds(vec![vec![
        (
            "semantic_search",
            json!({ "query": "pelicans", "limit": 3 }),
        ),
        ("done", json!({})),
    ]]);
    mock.reset_counts();

    run_to_finding(&core, &report).await;

    let results = tool_results(&final_pass_messages(&mock));
    assert!(
        results[0].contains("[3] citable"),
        "two source atoms hold [1] and [2], so the context hit is [3]: {:?}",
        results[0]
    );
}
