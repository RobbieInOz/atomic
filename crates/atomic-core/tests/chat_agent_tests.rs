//! Agent-loop contracts that only show up end to end: what the caller sees
//! while a turn streams, how a turn ends when it doesn't end cleanly, and
//! which sources survive into the stored message.
//!
//! They all run against the wiremock provider (`support::MockAiServer`), so
//! they exercise the real streaming parser and the real tool registry — no
//! live provider, no timing races (cancellation is driven from an event
//! callback, not a sleep).

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use atomic_core::reports::RunOutcome;
use atomic_core::{
    AtomicCore, ChatEvent, ChatMessageWithContext, CreateAtomRequest, CreateReportRequest,
    ScopeMode,
};
use serde_json::json;
use support::{await_pipeline, event_collector, setup_core, Backend, MockAiServer};

/// Collect every `ChatEvent` the turn emits, and optionally react to one.
fn chat_event_collector(
    on_event: impl Fn(&ChatEvent) + Send + Sync + 'static,
) -> (
    impl Fn(ChatEvent) + Send + Sync + 'static,
    Arc<Mutex<Vec<ChatEvent>>>,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let callback = move |event: ChatEvent| {
        on_event(&event);
        sink.lock().expect("event sink").push(event);
    };
    (callback, events)
}

fn stream_deltas(events: &Arc<Mutex<Vec<ChatEvent>>>) -> Vec<String> {
    events
        .lock()
        .expect("event sink")
        .iter()
        .filter_map(|event| match event {
            ChatEvent::StreamDelta { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// Create an atom and wait for its pipeline, so the agent's search tool can
/// actually find it.
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

/// What the turn's single tool call returned — the text the model was shown
/// and the UI's tool card renders.
fn tool_output(message: &ChatMessageWithContext) -> String {
    message
        .tool_calls
        .first()
        .expect("the turn called a tool")
        .tool_output
        .as_ref()
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The tool names offered to the model on its first turn.
fn offered_tools(mock: &MockAiServer) -> Vec<String> {
    let bodies = mock.chat_request_bodies();
    let first = bodies.first().expect("at least one chat request");
    first["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
        .collect()
}

async fn new_conversation(core: &AtomicCore) -> String {
    core.create_conversation(&[], Some("agent loop"))
        .await
        .expect("create conversation")
        .conversation
        .id
}

/// A conversation the way the UI creates one: no title, so the first
/// exchange triggers title generation.
async fn untitled_conversation(core: &AtomicCore) -> String {
    core.create_conversation(&[], None)
        .await
        .expect("create conversation")
        .conversation
        .id
}

async fn conversation_title(core: &AtomicCore, conversation_id: &str) -> Option<String> {
    core.get_conversation(conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists")
        .conversation
        .title
}

/// Poll `condition` until it holds. Title generation is detached from the
/// turn, so tests wait on its effect rather than on a handle.
async fn wait_until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !condition().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Give a detached title task time to reach the provider, for assertions
/// that something did *not* happen. The task only reads the conversation
/// before deciding, so this is orders of magnitude more than it needs.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

/// Deltas reach the caller as the provider produces them: several small
/// chunks, each carrying only its own text, concatenating to the final
/// message. The pre-0.1 behavior emitted one delta holding the whole
/// iteration's text, which this catches.
#[tokio::test]
async fn stream_deltas_arrive_incrementally() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let (callback, events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(
            &conversation_id,
            "what did I write about pelicans?",
            callback,
            None,
        )
        .await
        .expect("chat turn");

    let deltas = stream_deltas(&events);
    assert!(
        deltas.len() > 1,
        "expected one event per provider chunk, got {deltas:?}"
    );
    assert_eq!(
        deltas.concat(),
        message.message.content,
        "deltas must concatenate to the final message, not repeat it"
    );
    for delta in &deltas {
        assert!(
            delta != &message.message.content,
            "a delta carrying the whole message means accumulation leaked back in"
        );
    }
}

/// A model that never stops calling tools used to end the turn with
/// `Err("Max iterations reached")` and throw the work away. Now the loop
/// spends one tool-free call to get a real answer and labels it.
#[tokio::test]
async fn iteration_cap_salvages_an_answer() {
    let mock = MockAiServer::start().await;
    mock.set_chat_force_tool_calls(true);
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let (callback, _events) = chat_event_collector(|event| {
        assert!(
            !matches!(event, ChatEvent::Error { .. }),
            "hitting the cap is not an error path anymore"
        );
    });
    let message = core
        .send_chat_message(&conversation_id, "keep digging", callback, None)
        .await
        .expect("cap must salvage, not fail");

    assert!(
        message.message.content.contains("Mock assistant reply"),
        "the tool-free salvage call supplies the answer: {:?}",
        message.message.content
    );
    assert!(
        message.message.content.contains("reached tool-call limit"),
        "the answer is labelled incomplete: {:?}",
        message.message.content
    );
    assert!(
        !message.tool_calls.is_empty(),
        "the tool calls made before the cap are still persisted"
    );

    // And the salvaged answer is what the conversation now holds.
    let stored = core
        .get_conversation(&conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists");
    let last = stored.messages.last().expect("assistant message persisted");
    assert_eq!(last.message.role, "assistant");
    assert_eq!(last.message.content, message.message.content);
}

/// A provider failure has to reach the UI over the event stream, not just as
/// the HTTP body of a request the client may never read.
#[tokio::test]
async fn provider_failure_emits_a_chat_error_event() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;
    mock.set_chat_failure(Some(atomic_test_support::InjectedFailure::Unauthorized));

    let (callback, events) = chat_event_collector(|_| {});
    let result = core
        .send_chat_message(&conversation_id, "anything", callback, None)
        .await;

    assert!(result.is_err(), "a dead provider still fails the call");
    let errors: Vec<String> = events
        .lock()
        .expect("event sink")
        .iter()
        .filter_map(|event| match event {
            ChatEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "exactly one error event: {errors:?}");
}

/// An untitled conversation names itself off its first exchange, and the
/// title reaches the caller on the event stream — the list and the header
/// both update without a refetch.
#[tokio::test]
async fn first_exchange_names_the_conversation() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = untitled_conversation(&core).await;

    let (callback, events) = chat_event_collector(|_| {});
    core.send_chat_message(
        &conversation_id,
        "what did I write about pelicans?",
        callback,
        None,
    )
    .await
    .expect("chat turn");

    wait_until("the generated title to be persisted", || async {
        conversation_title(&core, &conversation_id).await.is_some()
    })
    .await;

    // The mock answers with `"Notes About Pelicans."` — quotes and full stop
    // included, both of which the sanitizer has to drop.
    assert_eq!(
        conversation_title(&core, &conversation_id).await.as_deref(),
        Some("Notes About Pelicans")
    );
    assert_eq!(mock.title_request_count(), 1);

    let titles: Vec<(String, String)> = events
        .lock()
        .expect("event sink")
        .iter()
        .filter_map(|event| match event {
            ChatEvent::ConversationUpdated {
                conversation_id,
                title,
            } => Some((conversation_id.clone(), title.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        titles,
        vec![(conversation_id.clone(), "Notes About Pelicans".to_string())],
        "exactly one ConversationUpdated, carrying the sanitized title"
    );
}

/// A title that already exists is the user's — whether we generated it last
/// turn or they typed it. The second exchange must not spend a call, let
/// alone overwrite it.
#[tokio::test]
async fn an_existing_title_is_never_regenerated() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = untitled_conversation(&core).await;

    let (callback, _events) = chat_event_collector(|_| {});
    core.send_chat_message(&conversation_id, "first question", callback, None)
        .await
        .expect("first turn");
    wait_until("the generated title to be persisted", || async {
        conversation_title(&core, &conversation_id).await.is_some()
    })
    .await;

    // Rename it the way the UI does, then take another turn.
    core.update_conversation(&conversation_id, Some("Renamed by hand"), None)
        .await
        .expect("rename conversation");

    let (callback, _events) = chat_event_collector(|_| {});
    core.send_chat_message(&conversation_id, "second question", callback, None)
        .await
        .expect("second turn");
    settle().await;

    assert_eq!(
        conversation_title(&core, &conversation_id).await.as_deref(),
        Some("Renamed by hand"),
        "the manual rename survives the next exchange"
    );
    assert_eq!(
        mock.title_request_count(),
        1,
        "the second turn asked the model for nothing"
    );
}

/// Titles are a nicety; the message they describe is not. A title model that
/// refuses costs the exchange nothing and leaves the conversation untitled.
#[tokio::test]
async fn a_failed_title_attempt_leaves_the_conversation_untitled() {
    let mock = MockAiServer::start().await;
    mock.set_title_failure(Some(atomic_test_support::InjectedFailure::Unauthorized));
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = untitled_conversation(&core).await;

    let (callback, events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "what about pelicans?", callback, None)
        .await
        .expect("the turn succeeds regardless of the title model");
    assert!(message.message.content.contains("Mock assistant reply"));

    wait_until("the title attempt to reach the provider", || async {
        mock.title_request_count() >= 1
    })
    .await;

    assert_eq!(
        conversation_title(&core, &conversation_id).await,
        None,
        "a refused title leaves the column NULL, not a stub"
    );
    let stored = core
        .get_conversation(&conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists");
    assert_eq!(
        stored.messages.len(),
        2,
        "both sides of the exchange are persisted"
    );
    assert!(
        !events
            .lock()
            .expect("event sink")
            .iter()
            .any(|event| matches!(event, ChatEvent::ConversationUpdated { .. })),
        "no title event without a title"
    );
}

/// Sources are what an answer rests on, not a log of what the agent read.
/// The search tool surfaces three atoms; the answer cites one; one citation
/// is stored, under the number the answer actually wrote.
#[tokio::test]
async fn only_cited_sources_are_stored() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let mut seeded = Vec::new();
    for content in [
        "pelicans dive for fish along the coast",
        "pelicans nest in large colonies on islands",
        "pelicans carry water in enormous throat pouches",
    ] {
        seeded.push(seed_atom(&core, content, &[]).await);
    }
    let conversation_id = new_conversation(&core).await;

    let (callback, events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "pelicans", callback, None)
        .await
        .expect("chat turn");

    let touched = events
        .lock()
        .expect("event sink")
        .iter()
        .find_map(|event| match event {
            ChatEvent::ToolComplete { results_count, .. } => Some(*results_count),
            _ => None,
        })
        .expect("the search tool ran");
    assert_eq!(touched, 3, "the agent saw all three atoms");

    // The mock's answer cites `[1]` and nothing else.
    assert_eq!(
        message.citations.len(),
        1,
        "uncited sources leave no rows: {:?}",
        message.citations
    );
    assert_eq!(
        message.citations[0].citation_index, 1,
        "the stored index is the marker the answer wrote"
    );
    assert!(
        seeded.contains(&message.citations[0].atom_id),
        "the citation resolves to a seeded atom; got {:?}",
        message.citations[0].atom_id
    );

    let stored = core
        .get_conversation(&conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists");
    assert_eq!(
        stored
            .messages
            .last()
            .expect("assistant message persisted")
            .citations
            .len(),
        1,
        "persistence agrees with the returned message"
    );
}

/// A tool that fails says so on the event stream and in the persisted call,
/// and the error text still reaches the model so it can recover.
#[tokio::test]
async fn a_failing_tool_reports_failure_and_tells_the_model() {
    let mock = MockAiServer::start().await;
    // An atom with no content is the cheapest deterministic tool failure.
    mock.set_chat_tool_call(Some(("create_atom", json!({ "content": "   " }))));
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let (callback, events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "save this for me", callback, None)
        .await
        .expect("a failed tool does not fail the turn");

    let failures: Vec<bool> = events
        .lock()
        .expect("event sink")
        .iter()
        .filter_map(|event| match event {
            ChatEvent::ToolComplete { failed, .. } => Some(*failed),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec![true], "the failure is live, not post-hoc");

    let call = message.tool_calls.first().expect("the call is persisted");
    assert_eq!(call.status, "failed");
    let output = call
        .tool_output
        .as_ref()
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(
        output.contains("Cannot create an empty atom"),
        "the model is told what went wrong: {output:?}"
    );
    assert!(
        message.message.content.contains("Mock assistant reply"),
        "the turn still answers"
    );
}

/// A tool this build doesn't register is a failed call, not an empty result
/// the model might mistake for "nothing found".
#[tokio::test]
async fn a_tool_outside_the_registry_fails_the_call() {
    let mock = MockAiServer::start().await;
    mock.set_chat_tool_call(Some(("delete_everything", json!({}))));
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let (callback, _events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "do something drastic", callback, None)
        .await
        .expect("an unknown tool does not fail the turn");

    let call = message.tool_calls.first().expect("the call is persisted");
    assert_eq!(call.status, "failed");
    assert!(
        call.tool_output
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .contains("Unknown tool: delete_everything"),
        "the model learns the tool does not exist: {:?}",
        call.tool_output
    );
}

/// Stopping mid-turn returns the partial answer through the normal path —
/// no error, no wedged conversation — and stops calling tools immediately.
#[tokio::test]
async fn cancelling_mid_turn_persists_the_partial_answer() {
    let mock = MockAiServer::start().await;
    // Without this the mock wraps up after one tool call and there is no
    // mid-turn to cancel.
    mock.set_chat_force_tool_calls(true);
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let cancel: atomic_core::ChatCancel = Arc::new(AtomicBool::new(false));
    let trigger = Arc::clone(&cancel);
    // Cancel the moment the first tool starts — deterministic, unlike a timer.
    let (callback, _events) = chat_event_collector(move |event| {
        if matches!(event, ChatEvent::ToolStart { .. }) {
            trigger.store(true, Ordering::Relaxed);
        }
    });

    let message = core
        .send_chat_message(&conversation_id, "keep digging", callback, Some(cancel))
        .await
        .expect("a stopped turn still returns its message");

    assert!(
        message.message.content.contains("*(stopped)*"),
        "the partial answer is marked stopped: {:?}",
        message.message.content
    );
    assert_eq!(
        message.tool_calls.len(),
        1,
        "the loop stopped at its next checkpoint instead of running to the cap"
    );

    let stored = core
        .get_conversation(&conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists");
    assert_eq!(
        stored
            .messages
            .last()
            .expect("assistant message persisted")
            .message
            .content,
        message.message.content,
    );
}

// ==================== Knowledge-base structure tools ====================

/// Everything the agent needs to reason about the knowledge base's shape —
/// not just its atoms — is registered on every turn, with no context or
/// caller opt-in required.
#[tokio::test]
async fn structure_tools_are_offered_on_every_turn() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;
    let conversation_id = new_conversation(&core).await;

    let (callback, _events) = chat_event_collector(|_| {});
    core.send_chat_message(&conversation_id, "what do I know?", callback, None)
        .await
        .expect("chat turn");

    let offered = offered_tools(&mock);
    for tool in ["list_tags", "get_wiki", "list_reports", "get_finding"] {
        assert!(
            offered.contains(&tool.to_string()),
            "{tool} must be callable; got {offered:?}"
        );
    }
}

/// `list_tags` answers "how is this knowledge base organized?": the tree,
/// with ids to drill in by and counts that include sub-tags. A tag is not a
/// source, so nothing it surfaces becomes citable.
#[tokio::test]
async fn list_tags_renders_the_tree_with_counts_and_ids() {
    let mock = MockAiServer::start().await;
    mock.set_chat_tool_call(Some(("list_tags", json!({}))));
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let parent = core.create_tag("Birds", None).await.expect("create parent");
    let child = core
        .create_tag("Pelicans", Some(&parent.id))
        .await
        .expect("create child");
    seed_atom(&core, "pelicans dive for fish", &[child.id.as_str()]).await;

    let conversation_id = new_conversation(&core).await;
    let (callback, _events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "how is this organized?", callback, None)
        .await
        .expect("chat turn");

    let output = tool_output(&message);
    assert!(
        output.contains(&format!("Birds (1 atom, tag_id: {})", parent.id)),
        "parents carry their descendants' atoms in the count: {output}"
    );
    assert!(
        output.contains(&format!("Pelicans (1 atom, tag_id: {})", child.id)),
        "children are rendered under their parent: {output}"
    );
    assert!(
        message.citations.is_empty(),
        "tags are navigation, not sources: {:?}",
        message.citations
    );
}

/// A wiki article is a first-class source: reading one makes it citable, and
/// the answer's `[N]` stores a citation that points at the tag — with the
/// tag's name resolved, so the UI can open the article.
#[tokio::test]
async fn a_cited_wiki_article_is_stored_as_a_wiki_source() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core.create_tag("Pelicans", None).await.expect("create tag");
    seed_atom(&core, "pelicans dive for fish", &[tag.id.as_str()]).await;
    core.generate_wiki(&tag.id, &tag.name)
        .await
        .expect("generate wiki article");

    // Resolution is exact-then-case-insensitive, and the model rarely
    // matches the user's capitalization.
    mock.set_chat_tool_call(Some(("get_wiki", json!({ "tag": "pelicans" }))));
    let conversation_id = new_conversation(&core).await;
    let (callback, _events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "summarize pelicans", callback, None)
        .await
        .expect("chat turn");

    let output = tool_output(&message);
    assert!(
        output.starts_with(&format!(
            "[1] (wiki article for tag: Pelicans, tag_id: {}",
            tag.id
        )),
        "the model is shown the number it must cite: {output}"
    );
    assert!(
        !output.contains("mock article body. ["),
        "the article's own citation markers are stripped — they number a \
         different source list: {output}"
    );

    let citation = match message.citations.as_slice() {
        [citation] => citation,
        other => panic!("expected exactly the cited wiki article, got {other:?}"),
    };
    assert_eq!(citation.source_type, "wiki");
    assert_eq!(
        citation.atom_id, tag.id,
        "a wiki citation points at its tag"
    );
    assert_eq!(citation.citation_index, 1);
    assert_eq!(citation.source_title.as_deref(), Some("Pelicans"));

    // And it survives the round trip, name included — the read path joins
    // the tag rather than trusting a stored copy.
    let stored = core
        .get_conversation(&conversation_id)
        .await
        .expect("load conversation")
        .expect("conversation exists");
    let reloaded = stored
        .messages
        .last()
        .expect("assistant message persisted")
        .citations
        .first()
        .expect("the citation persisted")
        .clone();
    assert_eq!(reloaded.source_type, "wiki");
    assert_eq!(reloaded.atom_id, tag.id);
    assert_eq!(reloaded.source_title.as_deref(), Some("Pelicans"));
}

/// Reports are the other synthesized surface: the agent can see what
/// research already ran and read a finding, and citing one stores a citation
/// the finding reader can open.
#[tokio::test]
async fn reports_are_listable_and_a_cited_finding_is_stored_as_a_finding_source() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let tag = core
        .create_tag("ReportScope", None)
        .await
        .expect("create tag");
    seed_atom(&core, "pelicans dive for fish", &[tag.id.as_str()]).await;
    let report = core
        .create_report(CreateReportRequest {
            name: "Pelican Watch".to_string(),
            research_prompt: "Summarize what is known about pelicans.".to_string(),
            source_scope_tag_ids: vec![tag.id.clone()],
            schedule: "0 0 * * * *".to_string(),
            ..Default::default()
        })
        .await
        .expect("create report");
    let finding_atom_id = match core.run_report_now(&report.id).await.expect("run report") {
        RunOutcome::Succeeded { finding_atom_id } => finding_atom_id,
        other => panic!("expected a finding, got {other:?}"),
    };

    // list_reports is the menu…
    mock.set_chat_tool_call(Some(("list_reports", json!({}))));
    let conversation_id = new_conversation(&core).await;
    let (callback, _events) = chat_event_collector(|_| {});
    let listing = core
        .send_chat_message(&conversation_id, "what research runs?", callback, None)
        .await
        .expect("chat turn");

    let output = tool_output(&listing);
    assert!(
        output.contains(&format!("Pelican Watch (report_id: {}", report.id)),
        "reports are listed with the id get_finding needs: {output}"
    );
    assert!(
        output.contains("latest finding: ") && output.contains(&finding_atom_id),
        "each report carries its most recent finding: {output}"
    );
    assert!(
        listing.citations.is_empty(),
        "a listing is not evidence: {:?}",
        listing.citations
    );

    // …and get_finding is the read, which is citable.
    mock.set_chat_tool_call(Some(("get_finding", json!({ "atom_id": finding_atom_id }))));
    let (callback, _events) = chat_event_collector(|_| {});
    let read = core
        .send_chat_message(&conversation_id, "what did it find?", callback, None)
        .await
        .expect("chat turn");

    let output = tool_output(&read);
    assert!(
        output.starts_with("[1] (finding from report: Pelican Watch"),
        "the finding's provenance leads its content: {output}"
    );

    let citation = match read.citations.as_slice() {
        [citation] => citation,
        other => panic!("expected exactly the cited finding, got {other:?}"),
    };
    assert_eq!(citation.source_type, "finding");
    assert_eq!(citation.atom_id, finding_atom_id);
    assert_eq!(
        citation.source_title, None,
        "a finding names itself from its own atom"
    );
}

/// Boolean scope is enforced where retrieval happens, not by asking the model
/// nicely: a conversation that includes one tag, requires a second and
/// excludes a third only ever sees the atom that satisfies all three.
#[tokio::test]
async fn require_and_exclude_shape_what_search_returns() {
    let mock = MockAiServer::start().await;
    let handle = setup_core(Backend::Sqlite, &mock.base_url())
        .await
        .expect("sqlite core");
    let core = handle.core;

    let rust = core.create_tag("Rust", None).await.expect("create Rust");
    let sailing = core
        .create_tag("Sailing", None)
        .await
        .expect("create Sailing");
    let draft = core.create_tag("Draft", None).await.expect("create Draft");

    let in_scope = seed_atom(
        &core,
        "pelicans satisfy every scope tag",
        &[rust.id.as_str(), sailing.id.as_str()],
    )
    .await;
    seed_atom(&core, "pelicans miss the required tag", &[rust.id.as_str()]).await;
    seed_atom(
        &core,
        "pelicans carry the excluded tag",
        &[rust.id.as_str(), sailing.id.as_str(), draft.id.as_str()],
    )
    .await;

    let conversation_id = core
        .create_conversation(&[rust.id.clone()], Some("boolean scope"))
        .await
        .expect("create conversation")
        .conversation
        .id;
    core.add_tag_to_scope(&conversation_id, &sailing.id, ScopeMode::Require)
        .await
        .expect("require Sailing");
    core.add_tag_to_scope(&conversation_id, &draft.id, ScopeMode::Exclude)
        .await
        .expect("exclude Draft");

    let (callback, events) = chat_event_collector(|_| {});
    let message = core
        .send_chat_message(&conversation_id, "pelicans", callback, None)
        .await
        .expect("chat turn");

    let results = events
        .lock()
        .expect("event sink")
        .iter()
        .find_map(|event| match event {
            ChatEvent::ToolComplete { results_count, .. } => Some(*results_count),
            _ => None,
        })
        .expect("the search tool ran");
    assert_eq!(
        results, 1,
        "only the atom passing include + require + exclude reaches the model"
    );

    let output = tool_output(&message);
    assert!(
        output.contains(&in_scope),
        "the surviving result is the atom that satisfies the whole scope: {output}"
    );
    assert!(
        !output.contains("required tag") && !output.contains("excluded tag"),
        "atoms outside the scope never reach the model: {output}"
    );

    // The prompt tells the model the same contract retrieval enforces.
    // Seeding atoms also hits the provider for auto-tagging; only the chat
    // turn offers tools.
    let system_prompt = mock
        .chat_request_bodies()
        .into_iter()
        .find(|body| body["tools"].as_array().is_some_and(|t| !t.is_empty()))
        .expect("a chat turn request")["messages"][0]["content"]
        .as_str()
        .expect("system prompt")
        .to_string();
    assert!(
        system_prompt.contains("any of: Rust")
            && system_prompt.contains("every one of: Sailing")
            && system_prompt.contains("Draft"),
        "the scope paragraph names all three lists: {system_prompt}"
    );
}
