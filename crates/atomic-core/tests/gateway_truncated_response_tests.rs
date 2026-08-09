//! Regression cover for the 2026-08-09 auto-tagging failure on
//! `google/gemma-4-26b-a4b-it`:
//!
//! ```text
//! INFO  Starting auto-tagging atom_id=7922b646… model=google/gemma-4-26b-a4b-it   [15:00:19.103]
//! ERROR OpenRouter LLM parse error error=EOF while parsing a value at line 233
//!       column 0 model=google/gemma-4-26b-a4b-it body_preview=                    [15:01:08.232]
//! ```
//!
//! ## What the wire actually carries
//!
//! Verified against live OpenRouter traffic (2026-08-09, this exact model and
//! the exact request the tagging path builds):
//!
//! - OpenRouter pads **non-streaming** `/chat/completions` responses with
//!   whitespace while it waits on the upstream. The unit is
//!   [`PADDING_UNIT`] — captured verbatim; the body's only byte values are
//!   `0x0a` and `0x20`. Padding appeared on **every** observed call (6–112
//!   newlines).
//! - The real JSON payload that follows is **compact — zero newlines**.
//!   Therefore every line counted in a serde error on this path came from
//!   padding, and nothing else. `line 233` means 232 padding newlines.
//! - The cadence is [`PADDING_NEWLINES_PER_SEC`] ≈ 4.66/s, stable across
//!   calls from 4.5s to 8 minutes. 232 newlines ⇒ ~49.8s of waiting; the
//!   report's own timestamps span 49.13s. The body shape and the wall-clock
//!   agree independently.
//! - When a call goes wrong, padding is *all* that ever arrives: one captured
//!   call ran 8 minutes and delivered 12288 bytes / 2235 newlines / no JSON.
//!
//! ## Why the gateway can only fail this way
//!
//! OpenRouter commits `HTTP/2 200` and sends headers *before* the upstream has
//! produced anything (verified: a request cut after 3s had already returned
//! full headers and a body of nothing but padding). After that commit no
//! failure can be expressed as an HTTP status — the only exits are a JSON
//! error body or simply ending the body. Ending it leaves the client holding
//! whitespace. So *any* late fault, on *any* upstream, degrades into this
//! shape. It is a property of the gateway, not of one provider.
//!
//! Note what the failing body does NOT contain: a `provider` field. The
//! upstream that served a failed call is therefore unknowable from the
//! response — but `x-generation-id` is in the *headers*, arrives before the
//! body, and resolves via `GET /api/v1/generation?id=…` to the provider,
//! timings, finish reason, and cost. It is now captured into the error and
//! the log line (`providers::error::gateway_trace_id`).
//!
//! ## What these tests hold in place
//!
//! Both errors in the report are the same event, differing only in whether
//! the terminating chunk arrived: `Network` ("error decoding response body",
//! stream cut) or `ParseError` ("EOF while parsing a value", stream ended on
//! padding). Classifying the second as permanent is what dropped the work —
//! one attempt of a three-attempt budget, on a fault a retry fixes. Both are
//! now transport failures, retryable and `Transient`.
//!
//! Worth knowing when reading a future report: OpenRouter records these as
//! **successes**. Three deliberately abandoned calls came back
//! `finish_reason: stop`, `cancelled: false`, fully billed — delivery is not
//! modelled in a generation record, so the client seeing nothing leaves no
//! trace on their side. An absence of errors in their dashboard is consistent
//! with this failure, not evidence against it.
//!
//! What is NOT established: what tipped that particular request over. The
//! report's `upstream_provider="DeepInfra"` belongs to the *previous,
//! successful* call for a different atom, not to the failure.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use atomic_core::providers::error::{
    classify_provider_failure, ProviderError, ProviderFailureClass,
};
use atomic_core::providers::openrouter::OpenRouterProvider;
use atomic_core::providers::structured::{
    call_structured_with_provider, StructuredCall, DEFAULT_MAX_OUTPUT_TOKENS,
};
use atomic_core::providers::traits::{LlmConfig, LlmProvider, StreamingLlmProvider};
use atomic_core::providers::types::Message;
use atomic_core::providers::ProviderConfig;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// OpenRouter's keepalive padding unit, captured byte-for-byte from a live
/// non-streaming `/chat/completions` response: newline, nine spaces, newline.
const PADDING_UNIT: &str = "\n         \n";

/// Measured padding cadence (newlines/second), stable across observed calls.
/// Only used to document what a given line number implies in wall-clock.
const PADDING_NEWLINES_PER_SEC: f64 = 4.66;

/// A raw-socket HTTP server: answers every connection with the exact bytes
/// `response`, then closes. Raw bytes rather than a framework because the
/// point is to serve a body no well-behaved server would send. Records each
/// request body so tests can assert what Atomic actually puts on the wire.
struct RawServer {
    base_url: String,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl RawServer {
    async fn start(response: Vec<u8>) -> Self {
        Self::start_sequence(vec![response]).await
    }

    /// Serve `responses[n]` to the nth connection, repeating the last entry
    /// once the sequence is exhausted. Lets a test script a gateway that fails
    /// one attempt and answers the next.
    async fn start_sequence(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (hits_t, reqs_t) = (hits.clone(), requests.clone());
        let responses = Arc::new(responses);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let nth = hits_t.fetch_add(1, Ordering::SeqCst);
                let response = responses[nth.min(responses.len() - 1)].clone();
                let reqs = reqs_t.clone();
                tokio::spawn(async move {
                    // Read headers, then exactly Content-Length body bytes, so
                    // the captured request is complete and the client is never
                    // left blocked on a half-drained socket.
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&raw).to_string();
                        if let Some(head_end) = text.find("\r\n\r\n") {
                            let len: usize = text[..head_end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse().ok())?
                                })
                                .unwrap_or(0);
                            if raw.len() >= head_end + 4 + len {
                                reqs.lock().unwrap().push(
                                    String::from_utf8_lossy(&raw[head_end + 4..]).to_string(),
                                );
                                break;
                            }
                        }
                    }
                    let _ = socket.write_all(&response).await;
                    let _ = socket.flush().await;
                    // Drop closes the connection — how a short body ends.
                });
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            hits,
            requests,
        }
    }

    fn provider(&self) -> OpenRouterProvider {
        OpenRouterProvider::with_base_url("test-key".to_string(), self.base_url.clone())
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn last_request(&self) -> serde_json::Value {
        let reqs = self.requests.lock().unwrap();
        serde_json::from_str(reqs.last().expect("a request was made")).expect("request is JSON")
    }
}

/// Frame a body the way OpenRouter actually does: `Transfer-Encoding: chunked`,
/// one chunk per gateway write. `terminate` controls the *only* thing that
/// differs between the report's two error lines — whether the final
/// `0\r\n\r\n` chunk arrives before the socket closes.
fn chunked_response(parts: &[String], terminate: bool) -> Vec<u8> {
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
    );
    for part in parts {
        out.push_str(&format!("{:x}\r\n{part}\r\n", part.len()));
    }
    if terminate {
        out.push_str("0\r\n\r\n");
    }
    out.into_bytes()
}

/// `newlines` worth of real keepalive padding, properly terminated: a
/// **complete** HTTP message whose entire body is whitespace. There is nothing
/// further to wait for — the server said "that's all".
fn padded_response(newlines: usize) -> Vec<u8> {
    let writes = newlines / PADDING_UNIT.matches('\n').count();
    let body = PADDING_UNIT.repeat(writes);
    assert_eq!(body.matches('\n').count(), newlines);
    assert!(body.trim().is_empty(), "padding is whitespace-only");
    // Why the report's `body_preview=` was blank: the preview is logged raw,
    // so a whitespace body renders as nothing at all. The diagnostic goes
    // dark exactly when the body is the thing you need to see.
    assert!(
        atomic_core::providers::error::truncate_utf8(&body, 500)
            .trim()
            .is_empty(),
        "preview of a padded body is visually empty"
    );
    // One chunk per gateway heartbeat, then the terminator.
    chunked_response(&vec![PADDING_UNIT.to_string(); writes], true)
}

/// The same padding, but the socket closes without the terminating chunk —
/// the stream is cut rather than ended.
fn cut_mid_body_response() -> Vec<u8> {
    chunked_response(&vec![PADDING_UNIT.to_string(); 20], false)
}

fn ok_response() -> Vec<u8> {
    let body = serde_json::json!({
        "id": "gen-ok", "provider": "DeepInfra",
        "choices": [{"message": {"role": "assistant",
            "content": "{\"tags\":[{\"name\":\"Rust\",\"parent_name\":\"Topics\"}]}"},
            "finish_reason": "stop"}],
        "usage": {"completion_tokens": 42}
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[derive(Debug, Deserialize)]
struct ExtractionResult {
    #[allow(dead_code)]
    tags: Vec<serde_json::Value>,
}

fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"tags": {"type": "array", "items": {
            "type": "object",
            "properties": {"name": {"type": "string"}, "parent_name": {"type": "string"}},
            "required": ["name", "parent_name"], "additionalProperties": false}}},
        "required": ["tags"],
        "additionalProperties": false
    })
}

async fn complete_against(server: &RawServer) -> ProviderError {
    server
        .provider()
        .complete(
            &[Message::user("tag this atom")],
            &LlmConfig::new("google/gemma-4-26b-a4b-it".to_string()),
        )
        .await
        .expect_err("a padded/cut body cannot produce a completion")
}

/// The reported failure, driven from real captured padding: a complete 200
/// whose body is nothing but keepalive whitespace is a transport fault, and
/// must be reported as one.
#[tokio::test]
async fn padded_body_is_a_retryable_transport_failure() {
    let server = RawServer::start(padded_response(232)).await;

    let err = complete_against(&server).await;

    // 232 newlines is not an arbitrary number — it is how long the call was in
    // flight. The report's own timestamps span 49.13s.
    let implied_wait = 232.0 / PADDING_NEWLINES_PER_SEC;
    assert!(
        (implied_wait - 49.13).abs() < 2.0,
        "padding cadence should explain the reported 49.13s gap, got {implied_wait:.1}s"
    );

    let ProviderError::Network(msg) = &err else {
        panic!("a padded body is a truncated transfer, not a parse failure: {err:?}");
    };
    assert!(
        msg.contains("232 newlines") && msg.contains("without ever sending a payload"),
        "the error should say what actually arrived, got: {msg}"
    );

    // Both consequences that used to bite, now inverted:
    assert!(err.is_retryable(), "the call must be retried");
    assert_eq!(
        classify_provider_failure(&err.to_string()),
        ProviderFailureClass::Transient,
        "schedulers must back off rather than terminally failing the work"
    );
}

/// One event, two framings — cut mid-stream, or ended politely on padding.
/// Both of the report's error lines are this. Neither may be permanent.
///
/// This is also where "why not just wait for the body?" is answered: we *do*
/// wait. In the terminated case the wait **succeeded** — reqwest read the
/// message to completion and handed us the whole body. That the body was
/// whitespace is not something more waiting fixes; the server already said
/// "that's all". Waiting is only meaningful while a stream is open, and
/// neither of these is.
#[tokio::test]
async fn both_framings_of_a_delivered_nothing_are_retryable() {
    let cut = RawServer::start(cut_mid_body_response()).await;
    let terminated = RawServer::start(padded_response(232)).await;

    let cut_err = complete_against(&cut).await;
    let terminated_err = complete_against(&terminated).await;

    for (label, err) in [
        ("cut mid-stream", &cut_err),
        ("ended on padding", &terminated_err),
    ] {
        assert!(
            matches!(err, ProviderError::Network(_)),
            "{label}: expected a transport error, got {err:?}"
        );
        assert!(err.is_retryable(), "{label}: must be retried");
        assert_eq!(
            classify_provider_failure(&err.to_string()),
            ProviderFailureClass::Transient,
            "{label}: schedulers must see a transient fault"
        );
    }
}

/// The streaming face of the same failure: a committed 200 that closes having
/// carried no SSE payload at all. Returning `Ok` with empty content would
/// persist the silence as if the model had chosen it.
#[tokio::test]
async fn a_stream_that_carries_nothing_fails_instead_of_returning_empty() {
    let server = RawServer::start(padded_response(40)).await;

    let err = server
        .provider()
        .complete_streaming_with_tools(
            &[Message::user("tag this atom")],
            &[],
            &LlmConfig::new("google/gemma-4-26b-a4b-it".to_string()),
            Box::new(|_| {}),
        )
        .await
        .expect_err("a stream that delivered no payload is not an empty answer");

    assert!(matches!(err, ProviderError::Network(_)), "got {err:?}");
    assert!(err.is_retryable());
    assert_eq!(
        classify_provider_failure(&err.to_string()),
        ProviderFailureClass::Transient
    );
}

/// The bug that actually lost work, end to end: auto-tagging used to burn ONE
/// request and quit on a transport fault, so the atom went untagged. The same
/// gateway behaviour must now recover on the following attempt.
///
/// The retry-budget arithmetic is covered by the paused-time unit tests in
/// `providers::structured`; what only a socket can prove is that a real padded
/// HTTP response is what gets retried. One real 2s backoff is the price.
#[tokio::test]
async fn a_padded_first_attempt_now_recovers_on_retry() {
    let server = RawServer::start_sequence(vec![padded_response(232), ok_response()]).await;
    let provider: Arc<dyn LlmProvider> = Arc::new(server.provider());

    let config = ProviderConfig::from_settings(&Default::default());
    let messages = [Message::user("tag this atom")];
    let call = StructuredCall::<ExtractionResult>::new(
        &config,
        "google/gemma-4-26b-a4b-it",
        &messages,
        "extraction_result",
        extraction_schema(),
    )
    .with_max_retries(1);

    let tags = call_structured_with_provider(call, provider)
        .await
        .expect("the second attempt returns a real payload");

    assert_eq!(tags.tags.len(), 1, "the work survived the padded attempt");
    assert_eq!(
        server.hits(),
        2,
        "one padded attempt, then a successful one"
    );
}

/// Separate defect found while reproducing the above: the tagging path sent
/// **no `max_tokens`**. `StructuredCall::new` sets a default, then
/// `with_params(extraction_params())` replaced the whole struct and dropped it
/// — the exact hazard `openrouter/llm.rs` warns about.
///
/// Not cosmetic: it changes *routing*. Live check on
/// `google/gemma-4-26b-a4b-it` — with `max_tokens: 32000` OpenRouter returns
/// 404 "No endpoints found" for DeepInfra (its endpoint caps completions at
/// 16384); with the field omitted, DeepInfra serves the call and the ceiling
/// becomes whatever the upstream picks. A cap must always be sent, and for
/// tagging it must be small enough to keep the endpoint pool intact.
#[tokio::test]
async fn every_structured_call_sends_an_output_cap() {
    let config = ProviderConfig::from_settings(&Default::default());
    let messages = [Message::user("tag this atom")];

    // A caller that builds its own params and forgets a cap, as tagging did.
    let forgetful = RawServer::start(ok_response()).await;
    let call = StructuredCall::<ExtractionResult>::new(
        &config,
        "google/gemma-4-26b-a4b-it",
        &messages,
        "extraction_result",
        extraction_schema(),
    )
    .with_params(
        atomic_core::providers::types::GenerationParams::new()
            .with_temperature(0.1)
            .with_minimize_reasoning(true),
    );
    call_structured_with_provider(call, Arc::new(forgetful.provider()) as Arc<dyn LlmProvider>)
        .await
        .expect("mock returns valid tags");
    assert_eq!(
        forgetful
            .last_request()
            .get("max_tokens")
            .and_then(|v| v.as_u64()),
        Some(DEFAULT_MAX_OUTPUT_TOKENS as u64),
        "a forgotten cap must fall back to the default, never to nothing"
    );

    // An explicit smaller cap is honoured as-is.
    let explicit = RawServer::start(ok_response()).await;
    let call = StructuredCall::<ExtractionResult>::new(
        &config,
        "google/gemma-4-26b-a4b-it",
        &messages,
        "extraction_result",
        extraction_schema(),
    )
    .with_params(atomic_core::providers::types::GenerationParams::new().with_max_tokens(8192));
    call_structured_with_provider(call, Arc::new(explicit.provider()) as Arc<dyn LlmProvider>)
        .await
        .expect("mock returns valid tags");
    assert_eq!(
        explicit
            .last_request()
            .get("max_tokens")
            .and_then(|v| v.as_u64()),
        Some(8192),
    );
}
