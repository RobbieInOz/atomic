//! Agent-loop contracts that only show up end to end: what the caller sees
//! while a turn streams, and how a turn ends when it doesn't end cleanly.
//!
//! All three run against the wiremock provider (`support::MockAiServer`), so
//! they exercise the real streaming parser and the real tool dispatch — no
//! live provider, no timing races (cancellation is driven from an event
//! callback, not a sleep).

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use atomic_core::{AtomicCore, ChatEvent};
use support::{setup_core, Backend, MockAiServer};

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
