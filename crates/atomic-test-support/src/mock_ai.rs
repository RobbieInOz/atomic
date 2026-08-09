//! Wiremock-backed mock of the OpenAI-compat `/v1/embeddings` and
//! `/v1/chat/completions` endpoints, plus Ollama's `/api/embed` and
//! `/api/chat`.
//!
//! The provider in `atomic-core/src/providers/openai_compat.rs` is the real
//! reqwest client — `MockAiServer::start` just stands up an HTTP listener
//! that speaks the protocol it expects. Tests configure `AtomicCore` to
//! point at `base_url()`, then exercise the full pipeline (chunk → embed →
//! tag → edges) against deterministic responses.
//!
//! ## Dialects
//!
//! One server speaks all three provider wire formats, so the same knobs and
//! counters drive a test whichever provider it points at:
//!
//! - **OpenAI-compat** (`/v1/...`): the original surface. `OpenAICompatProvider`
//!   hits it directly.
//! - **OpenRouter** (`/v1/...`): byte-identical to the above — the provider
//!   normalizes a bare `base_url` by appending `/v1`, so pointing
//!   `openrouter_base_url` at [`MockAiServer::base_url`] lands on the same
//!   routes.
//! - **Ollama** (`/api/chat`, `/api/embed`): a genuinely different wire
//!   format — NDJSON rather than SSE, tool-call arguments as JSON objects
//!   rather than strings, no ids and no `index` on tool calls. Served by
//!   [`OllamaChatResponder`] / [`OllamaEmbedResponder`] so agent-runtime
//!   behavior can be proven against the framing Ollama actually produces
//!   instead of only against the OpenAI shape.
//!
//! ## Mock responder modes
//!
//! [`ChatResponder`] emits tag extraction, wiki/long-form, research-loop,
//! conversation-title, and streaming agent-loop responses, keyed off the
//! request body (see its `respond`). The Ollama responder covers the subset
//! the chat era needs: streaming turns and conversation titles.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Embedding dimension used by the mock. Must stay in lockstep with the
/// default `openai_compat_embedding_dimension` setting and the SQLite
/// `vec_chunks float[1536]` schema so no dimension reconciliation kicks
/// in mid-test.
pub const EMBED_DIM: usize = 1536;

/// Embedding width the **Ollama** dialect serves. Ollama's provider derives
/// the stored width from the model *name* rather than the response
/// (`providers::ollama::get_embedding_dimension`), whose fallback for an
/// unregistered name is 768. A test pointing an Ollama-configured core at
/// this mock therefore has to be embedding at 768, so `/api/embed` answers
/// at that width — returning [`EMBED_DIM`] there would store vectors wider
/// than the column the same config just declared.
pub const OLLAMA_EMBED_DIM: usize = 768;

/// Similarity threshold used by the pipeline when building semantic edges.
/// Exposed here so tests can sanity-check that crafted atom pairs fall on
/// the correct side of the cutoff (see
/// `atomic_core::embedding::compute_semantic_edges...`).
pub const EDGE_SIMILARITY_THRESHOLD: f32 = 0.5;

/// Local HTTP server mimicking OpenAI's `/v1/embeddings` and
/// `/v1/chat/completions`. Holds the server handle for lifetime management.
pub struct MockAiServer {
    server: MockServer,
    counters: Arc<MockAiCounters>,
}

/// An injectable failure response, served instead of the normal payload
/// while set (see [`MockAiServer::set_embedding_failure`] /
/// [`MockAiServer::set_chat_failure`]). Lets tests exercise providers'
/// status-code handling — retry/backoff behavior, rate-limit hints,
/// billing rejections — against the real HTTP clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedFailure {
    /// HTTP 429, optionally carrying a `Retry-After: <secs>` header.
    RateLimited { retry_after_secs: Option<u64> },
    /// HTTP 402 with a provider-style error body.
    PaymentRequired,
    /// HTTP 401 with a provider-style error body (expired/revoked API key).
    Unauthorized,
}

impl InjectedFailure {
    fn response(self) -> ResponseTemplate {
        match self {
            InjectedFailure::RateLimited { retry_after_secs } => {
                let mut response = ResponseTemplate::new(429).set_body_json(json!({
                    "error": { "message": "mock rate limit exceeded" }
                }));
                if let Some(secs) = retry_after_secs {
                    response = response.insert_header("Retry-After", secs.to_string().as_str());
                }
                response
            }
            InjectedFailure::PaymentRequired => ResponseTemplate::new(402).set_body_json(json!({
                "error": { "message": "mock insufficient credits" }
            })),
            InjectedFailure::Unauthorized => ResponseTemplate::new(401).set_body_json(json!({
                "error": { "message": "mock invalid api key" }
            })),
        }
    }
}

#[derive(Default)]
struct MockAiCounters {
    embedding_requests: AtomicUsize,
    chat_requests: AtomicUsize,
    /// The `model` field of every `/v1/chat/completions` request body, in
    /// arrival order — lets tests assert *which* model an operation selected,
    /// not just that a call happened.
    chat_models: Mutex<Vec<String>>,
    /// Every `/v1/chat/completions` request body, in arrival order. Lets a
    /// test assert what an agent actually sent the model — which tool results
    /// came back, and what a final pass was asked to write from.
    chat_bodies: Mutex<Vec<Value>>,
    /// When set, `/v1/embeddings` serves this failure instead of embeddings.
    embedding_failure: Mutex<Option<InjectedFailure>>,
    /// When set, `/v1/chat/completions` serves this failure instead of a
    /// completion.
    chat_failure: Mutex<Option<InjectedFailure>>,
    /// When set, every `/v1/chat/completions` response (success or injected
    /// failure) is held for this long before being sent — latency injection
    /// for tests that need requests to genuinely overlap in flight.
    chat_delay: Mutex<Option<std::time::Duration>>,
    /// When true, a streaming request that offers tools always answers with
    /// another tool call instead of wrapping up — a model that never stops
    /// researching, which is how tests reach the agent loop's iteration cap.
    chat_force_tool_calls: AtomicBool,
    /// When set, the tool-call leg asks for this `(tool_name, arguments)`
    /// instead of `search_atoms`, so a test can drive any registered tool —
    /// or one the registry doesn't have — through the real loop.
    chat_tool_call: Mutex<Option<(String, Value)>>,
    /// Script for the non-streaming tool-bearing leg — the research loops
    /// reports and wiki generation run. One entry per model turn, each a
    /// round of `(tool_name, arguments)` calls issued together. Empty (the
    /// default) means the leg calls `done` on its first turn.
    research_tool_rounds: Mutex<Vec<Vec<(String, Value)>>>,
    /// When true the research leg never falls back to `done`: once the script
    /// runs out it repeats its last round, so the loop can only end at its
    /// own iteration cap. With no script there is nothing to repeat and the
    /// leg still calls `done`.
    research_force_tool_calls: AtomicBool,
    /// Conversation-title requests seen so far. Counted separately from
    /// `chat_requests` because title generation is fire-and-forget: a test
    /// waits on this to know the detached task actually ran.
    title_requests: AtomicUsize,
    /// When set, every streaming request is answered with this body verbatim
    /// instead of a generated one — the escape hatch for pinning a parser
    /// against wire shapes the generated responses don't produce (arguments
    /// split across deltas, several tool calls interleaved, provider
    /// metadata, a stream that ends without its sentinel).
    stream_script: Mutex<Option<String>>,
    /// When set, conversation-title requests serve this failure instead of a
    /// title. Scoped to titles so the exchange that triggers one still
    /// succeeds.
    title_failure: Mutex<Option<InjectedFailure>>,
}

impl MockAiCounters {
    /// The scripted streaming body, if one is set. Served verbatim; the
    /// content type is the SSE one, which every dialect's parser ignores
    /// (all three read the body as bytes and split on newlines).
    fn scripted_stream(&self) -> Option<ResponseTemplate> {
        let script = self
            .stream_script
            .lock()
            .expect("stream_script lock")
            .clone()?;
        Some(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_raw(script.into_bytes(), "text/event-stream"),
        )
    }
}

impl MockAiServer {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let counters = Arc::new(MockAiCounters::default());

        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(EmbedResponder {
                counters: counters.clone(),
            })
            .mount(&server)
            .await;

        // Tag extraction goes through the non-streaming `complete` path
        // with a `response_format: json_schema` payload. The responder
        // inspects the request body so the same mock can serve any
        // structured call — for tagging we return a deterministic
        // `{"tags":[...]}` shape.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ChatResponder {
                counters: counters.clone(),
            })
            .mount(&server)
            .await;

        // Ollama's surface, on the same listener and the same counters: a
        // test switches dialect by pointing `ollama_host` here instead of
        // `openai_compat_base_url`, and every knob/assertion still applies.
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(OllamaEmbedResponder {
                counters: counters.clone(),
            })
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(OllamaChatResponder {
                counters: counters.clone(),
            })
            .mount(&server)
            .await;

        Self { server, counters }
    }

    /// Base URL the `OpenAICompatProvider` should hit. No `/v1` suffix —
    /// the provider normalizes the URL itself.
    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    pub fn embedding_request_count(&self) -> usize {
        self.counters.embedding_requests.load(Ordering::Relaxed)
    }

    pub fn chat_request_count(&self) -> usize {
        self.counters.chat_requests.load(Ordering::Relaxed)
    }

    /// The `model` requested by each chat-completions call so far, in
    /// arrival order.
    pub fn chat_request_models(&self) -> Vec<String> {
        self.counters
            .chat_models
            .lock()
            .expect("chat_models lock")
            .clone()
    }

    /// Every chat-completions request body so far, in arrival order — the
    /// messages an agent actually sent, tool results included.
    pub fn chat_request_bodies(&self) -> Vec<Value> {
        self.counters
            .chat_bodies
            .lock()
            .expect("chat_bodies lock")
            .clone()
    }

    /// Make `/v1/embeddings` fail with `failure` until cleared with `None`.
    /// Requests are still counted while failing.
    pub fn set_embedding_failure(&self, failure: Option<InjectedFailure>) {
        *self
            .counters
            .embedding_failure
            .lock()
            .expect("embedding_failure lock") = failure;
    }

    /// Make `/v1/chat/completions` fail with `failure` until cleared with
    /// `None`. Requests are still counted while failing.
    pub fn set_chat_failure(&self, failure: Option<InjectedFailure>) {
        *self
            .counters
            .chat_failure
            .lock()
            .expect("chat_failure lock") = failure;
    }

    /// Hold every `/v1/chat/completions` response for `delay` until cleared
    /// with `None`. Lets tests keep several chat requests concurrently
    /// in flight (e.g. concurrency-cap assertions) without racing the
    /// responder.
    pub fn set_chat_delay(&self, delay: Option<std::time::Duration>) {
        *self.counters.chat_delay.lock().expect("chat_delay lock") = delay;
    }

    /// Make every tool-bearing streaming request answer with another tool
    /// call, so an agent loop runs until its own iteration cap stops it. A
    /// request that offers no tools still answers with text — there is no
    /// valid tool call to make.
    pub fn set_chat_force_tool_calls(&self, force: bool) {
        self.counters
            .chat_force_tool_calls
            .store(force, Ordering::Relaxed);
    }

    /// Make the tool-call leg request `(tool_name, arguments)` instead of
    /// `search_atoms`. Clear with `None`.
    pub fn set_chat_tool_call(&self, call: Option<(&str, Value)>) {
        *self
            .counters
            .chat_tool_call
            .lock()
            .expect("chat_tool_call lock") =
            call.map(|(name, arguments)| (name.to_string(), arguments));
    }

    /// Script the non-streaming research leg (reports, wiki) turn by turn:
    /// `rounds[n]` is the set of tool calls the model makes on its nth turn,
    /// and the leg calls `done` once the script runs out. An empty script is
    /// the default — `done` immediately, no research.
    pub fn set_research_tool_rounds(&self, rounds: Vec<Vec<(&str, Value)>>) {
        *self
            .counters
            .research_tool_rounds
            .lock()
            .expect("research_tool_rounds lock") = rounds
            .into_iter()
            .map(|round| {
                round
                    .into_iter()
                    .map(|(name, arguments)| (name.to_string(), arguments))
                    .collect()
            })
            .collect();
    }

    /// Keep the research leg on its script's last round forever instead of
    /// falling back to `done`, so a run can only end at its own iteration
    /// cap. Pair it with [`Self::set_research_tool_rounds`] — with no script
    /// there is nothing to repeat.
    pub fn set_research_force_tool_calls(&self, force: bool) {
        self.counters
            .research_force_tool_calls
            .store(force, Ordering::Relaxed);
    }

    /// Answer every **streaming** request with `body` verbatim, whichever
    /// dialect's endpoint it arrives on. Clear with `None`.
    ///
    /// The generated streaming responses are deliberately tidy — one
    /// complete tool call per delta, no provider metadata, always a closing
    /// sentinel — which leaves a provider's accumulator and metadata
    /// handling untested. This hands a test the exact bytes instead, so it
    /// can pin what the parser does with arguments dribbled across a dozen
    /// deltas, two tool calls interleaved by index, or a stream that simply
    /// stops.
    ///
    /// The caller supplies framing: `data: {...}\n\n` for the SSE dialects,
    /// one JSON object per line for Ollama.
    pub fn set_stream_script(&self, body: Option<&str>) {
        *self
            .counters
            .stream_script
            .lock()
            .expect("stream_script lock") = body.map(str::to_string);
    }

    /// Conversation-title completions requested so far. Zero means the
    /// detached title task never reached the provider.
    pub fn title_request_count(&self) -> usize {
        self.counters.title_requests.load(Ordering::Relaxed)
    }

    /// Make conversation-title requests fail with `failure` until cleared
    /// with `None`. Requests are still counted while failing.
    pub fn set_title_failure(&self, failure: Option<InjectedFailure>) {
        *self
            .counters
            .title_failure
            .lock()
            .expect("title_failure lock") = failure;
    }

    pub fn reset_counts(&self) {
        self.counters.embedding_requests.store(0, Ordering::Relaxed);
        self.counters.chat_requests.store(0, Ordering::Relaxed);
        self.counters.title_requests.store(0, Ordering::Relaxed);
        self.counters
            .chat_models
            .lock()
            .expect("chat_models lock")
            .clear();
        self.counters
            .chat_bodies
            .lock()
            .expect("chat_bodies lock")
            .clear();
    }
}

/// Bag-of-words style unit-vector embedder at [`EMBED_DIM`]. Two texts
/// sharing words land at the same positions → high cosine similarity → edge
/// crosses the 0.5 threshold. Disjoint texts end up near-orthogonal.
fn embed_text(text: &str) -> Vec<f32> {
    embed_text_at(text, EMBED_DIM)
}

/// [`embed_text`] at an explicit width, so a dialect whose provider pins a
/// different dimension (Ollama, [`OLLAMA_EMBED_DIM`]) gets vectors its own
/// vector column accepts. The hashing is width-relative, so the same text
/// still embeds deterministically — just in a different space.
fn embed_text_at(text: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dim];
    for word in text.split_whitespace() {
        let normalized: String = word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        if normalized.is_empty() {
            continue;
        }
        let mut h = DefaultHasher::new();
        normalized.hash(&mut h);
        let idx = (h.finish() as usize) % dim;
        vec[idx] += 1.0;
    }
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    } else {
        // Empty/punctuation-only input — put a constant at position 0 so
        // every row still has a valid unit vector.
        vec[0] = 1.0;
    }
    vec
}

/// Build a streaming `chat/completions` response. Detects whether the agent
/// is on its first turn (no prior tool results) or has tool results in its
/// message log, and emits the matching SSE stream:
///
/// - First turn: a single `tool_calls` delta requesting `search_atoms` with
///   a query plucked from the most recent user message. Closes with
///   `finish_reason: tool_calls`.
/// - Tool results present, or no tools offered at all: content deltas with
///   deterministic text, closing with `finish_reason: stop`.
///
/// `force_tool_calls` keeps a tool-bearing request on the tool-call leg
/// forever (see [`MockAiServer::set_chat_force_tool_calls`]); `tool_call`
/// replaces the tool the leg asks for (see
/// [`MockAiServer::set_chat_tool_call`]).
///
/// The provider parser is line-oriented (`data: ...\n`) and accepts the
/// stream as a single body payload, so we don't need true chunked transfer
/// to satisfy it.
fn streaming_chat_response(
    body: &Value,
    force_tool_calls: bool,
    tool_call: Option<(String, Value)>,
) -> ResponseTemplate {
    let sse_body = match stream_leg(body, force_tool_calls, tool_call) {
        StreamLeg::Text(fragments) => {
            let mut chunks: Vec<Value> = fragments
                .iter()
                .map(|text| {
                    json!({
                        "choices": [{
                            "delta": { "content": text },
                            "finish_reason": null,
                        }]
                    })
                })
                .collect();
            chunks.push(json!({
                "choices": [{
                    "delta": {},
                    "finish_reason": "stop",
                }]
            }));
            sse_concat(&chunks)
        }
        StreamLeg::ToolCall {
            id,
            name,
            arguments,
        } => {
            let chunks = [
                json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": id,
                                "type": "function",
                                // OpenAI-shaped tool calls carry arguments as
                                // a *string* of JSON, accumulated across
                                // deltas by the provider.
                                "function": {
                                    "name": name,
                                    "arguments": arguments.to_string(),
                                }
                            }]
                        },
                        "finish_reason": null,
                    }]
                }),
                json!({
                    "choices": [{
                        "delta": {},
                        "finish_reason": "tool_calls",
                    }]
                }),
            ];
            sse_concat(&chunks)
        }
    };

    ResponseTemplate::new(200)
        .insert_header("Content-Type", "text/event-stream")
        .set_body_raw(sse_body.into_bytes(), "text/event-stream")
}

/// What a streaming turn answers with, before any wire format is chosen.
/// Shared by every dialect so the *decision* (tool call vs. prose) is one
/// implementation and only the framing differs — which is exactly the axis
/// a cross-provider test wants to vary.
enum StreamLeg {
    /// Assistant prose, pre-split into the fragments the provider should
    /// surface as separate deltas.
    Text(Vec<String>),
    /// One tool call. `arguments` is the JSON *value*; each dialect encodes
    /// it the way its wire format does (string for OpenAI, object for
    /// Ollama).
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
}

/// Pick the leg for a streaming request: a tool call while there is
/// research to do, prose once tool results are in — or immediately when the
/// request offers no tools at all, which is the shape of the agent loop's
/// iteration-cap salvage call.
fn stream_leg(
    body: &Value,
    force_tool_calls: bool,
    tool_call: Option<(String, Value)>,
) -> StreamLeg {
    let has_tool_results = body
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|msgs| {
            msgs.iter()
                .any(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        })
        .unwrap_or(false);
    let tools_offered = body
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|tools| !tools.is_empty())
        .unwrap_or(false);
    // Without tools on the request there is nothing to call, so the only
    // honest answer is prose — that's the shape of the agent loop's
    // no-tools salvage call.
    if !tools_offered || (has_tool_results && !force_tool_calls) {
        // Final leg: the assistant text, split so tests exercise incremental
        // streaming rather than one-shot content. The trailing `[1]` is the
        // citation contract in miniature: the runtime stores only the
        // evidence the answer actually cites, so an answer with no markers
        // produces no citations at all.
        return StreamLeg::Text(vec![
            "Mock assistant reply ".to_string(),
            "grounded in the search results. [1]".to_string(),
        ]);
    }

    // First leg: ask the runtime to run a tool — `search_atoms` by default,
    // with the query lifted from the most recent user message so the search
    // hits the seeded atoms verbatim. The tool-call id must be unique per
    // response — the runtime persists tool calls by this id, and concurrent
    // conversations would otherwise collide.
    let (name, arguments) = tool_call.unwrap_or_else(|| {
        let query = latest_user_query(body).unwrap_or_else(|| "atomic".to_string());
        (
            "search_atoms".to_string(),
            json!({ "query": query, "limit": 5 }),
        )
    });
    static TOOL_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);
    let id = format!(
        "call_mock_{}_{}",
        name,
        TOOL_CALL_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    StreamLeg::ToolCall {
        id,
        name,
        arguments,
    }
}

/// Build a non-streaming `chat/completions` response for a research loop —
/// the shape reports and wiki generation drive through `complete_with_tools`,
/// where the model researches with tools and calls `done` when it has enough.
///
/// Which turn the model is on is recovered by counting the assistant messages
/// that already carry tool calls, so `rounds[n]` is served on the nth turn
/// however many calls each round makes. Past the end of the script the leg
/// calls `done` — which, with the default empty script, is the first turn.
/// `force` keeps it on the script's last round instead (see
/// [`MockAiServer::set_research_force_tool_calls`]).
fn research_chat_response(
    body: &Value,
    rounds: &[Vec<(String, Value)>],
    force: bool,
) -> ResponseTemplate {
    let done_round = vec![("done".to_string(), json!({}))];
    let turn = completed_tool_rounds(body);
    let round = rounds
        .get(turn)
        .or_else(|| force.then(|| rounds.last()).flatten())
        .unwrap_or(&done_round);

    // Ids must be unique per call — both loops key their tool-result messages
    // by id, and a repeated id makes the transcript ambiguous.
    static TOOL_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);
    let tool_calls: Vec<Value> = round
        .iter()
        .map(|(name, arguments)| {
            json!({
                "id": format!("call_mock_{}_{}", name, TOOL_CALL_SEQ.fetch_add(1, Ordering::Relaxed)),
                "type": "function",
                "function": { "name": name, "arguments": arguments.to_string() }
            })
        })
        .collect();

    ResponseTemplate::new(200).set_body_json(json!({
        "id": "mock-cmpl",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": tool_calls,
            },
            "finish_reason": "tool_calls"
        }]
    }))
}

/// Model turns already spent on tools — the index of the round being asked
/// for now.
fn completed_tool_rounds(body: &Value) -> usize {
    body.get("messages")
        .and_then(|v| v.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(|r| r.as_str()) == Some("assistant")
                        && message
                            .get("tool_calls")
                            .and_then(|calls| calls.as_array())
                            .is_some_and(|calls| !calls.is_empty())
                })
                .count()
        })
        .unwrap_or(0)
}

fn sse_concat(chunks: &[Value]) -> String {
    let mut out = String::new();
    for chunk in chunks {
        out.push_str("data: ");
        out.push_str(&chunk.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

fn latest_user_query(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    for msg in messages.iter().rev() {
        if msg.get("role").and_then(|v| v.as_str()) == Some("user") {
            return msg
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

/// Count `[N]` markers in the LLM request's user message — used by the
/// wiki-generation responder to figure out how many numbered sources were
/// embedded in the prompt so it can cite at least one of them.
fn count_numbered_sources(body: &Value) -> i32 {
    let mut max_seen = 0i32;
    if let Some(messages) = body.get("messages").and_then(|v| v.as_array()) {
        for msg in messages {
            if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                for cap in content.split('[').skip(1) {
                    if let Some(end) = cap.find(']') {
                        if let Ok(n) = cap[..end].parse::<i32>() {
                            if n > max_seen {
                                max_seen = n;
                            }
                        }
                    }
                }
            }
        }
    }
    max_seen
}

/// Wiki incremental updates label new sources with indices that start
/// strictly *after* the existing citations. Recover that starting index
/// from the prompt — it's the first marker following the
/// `NEW SOURCES TO INCORPORATE (cite as [N]` substring.
fn first_new_source_index(body: &Value) -> Option<i32> {
    let messages = body.get("messages")?.as_array()?;
    for msg in messages {
        if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
            // The prompt text explicitly tells the LLM where new sources
            // start: "NEW SOURCES TO INCORPORATE (cite as [N] onwards)".
            if let Some(anchor) = content.find("NEW SOURCES TO INCORPORATE (cite as [") {
                let tail = &content[anchor + "NEW SOURCES TO INCORPORATE (cite as [".len()..];
                if let Some(end) = tail.find(']') {
                    if let Ok(n) = tail[..end].parse::<i32>() {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

struct EmbedResponder {
    counters: Arc<MockAiCounters>,
}

impl Respond for EmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.counters
            .embedding_requests
            .fetch_add(1, Ordering::Relaxed);
        if let Some(failure) = *self
            .counters
            .embedding_failure
            .lock()
            .expect("embedding_failure lock")
        {
            return failure.response();
        }
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return ResponseTemplate::new(400),
        };
        let Some(inputs) = body.get("input").and_then(|v| v.as_array()) else {
            return ResponseTemplate::new(400);
        };
        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let text = text.as_str().unwrap_or_default();
                json!({
                    "object": "embedding",
                    "index": index,
                    "embedding": embed_text(text),
                })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": data,
            "model": body.get("model").cloned().unwrap_or(Value::Null),
        }))
    }
}

struct ChatResponder {
    counters: Arc<MockAiCounters>,
}

impl Respond for ChatResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.counters.chat_requests.fetch_add(1, Ordering::Relaxed);
        // Latency injection applies to every chat response uniformly —
        // success, injected failure, or malformed-request 400.
        let delay = *self.counters.chat_delay.lock().expect("chat_delay lock");
        let with_delay = |response: ResponseTemplate| match delay {
            Some(d) => response.set_delay(d),
            None => response,
        };
        if let Some(failure) = *self
            .counters
            .chat_failure
            .lock()
            .expect("chat_failure lock")
        {
            return with_delay(failure.response());
        }
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return with_delay(ResponseTemplate::new(400)),
        };
        if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
            self.counters
                .chat_models
                .lock()
                .expect("chat_models lock")
                .push(model.to_string());
        }
        self.counters
            .chat_bodies
            .lock()
            .expect("chat_bodies lock")
            .push(body.clone());

        // Streaming chat (agent loop). `stream: true` is answered with SSE
        // whatever else the request carries — see `streaming_chat_response`
        // for how it picks between a tool call and final text. Wiki and
        // tagging only stream when explicitly enabled (they don't, today).
        let is_streaming = body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_tools = body
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|t| !t.is_empty())
            .unwrap_or(false);
        if is_streaming {
            if let Some(script) = self.counters.scripted_stream() {
                return with_delay(script);
            }
            let force = self.counters.chat_force_tool_calls.load(Ordering::Relaxed);
            let tool_call = self
                .counters
                .chat_tool_call
                .lock()
                .expect("chat_tool_call lock")
                .clone();
            return with_delay(streaming_chat_response(&body, force, tool_call));
        }

        // Non-streaming chat with tools — the research loops reports and wiki
        // generation run. Unscripted, it calls `done` immediately, which
        // keeps the research phase out of tests that only care about what a
        // run writes. `set_research_tool_rounds` scripts the research turns
        // for the tests that do care.
        if !is_streaming && has_tools {
            let rounds = self
                .counters
                .research_tool_rounds
                .lock()
                .expect("research_tool_rounds lock")
                .clone();
            let force = self
                .counters
                .research_force_tool_calls
                .load(Ordering::Relaxed);
            return with_delay(research_chat_response(&body, &rounds, force));
        }

        // Inspect the requested schema name so this responder can serve
        // more than just tag extraction as the test matrix grows.
        //
        // Long-form callers (wiki full articles, the report final pass)
        // send no `response_format` at all — they use the markdown +
        // `CITATIONS_USED:` trailer contract, whose instruction line is
        // detectable in the request text. `call_structured`'s prompt-based
        // fallback also arrives schema-less; its nudge embeds the schema
        // JSON, so property-name sniffing routes it to the right arm.
        let request_text = body.to_string().to_lowercase();
        let wire_schema = body
            .pointer("/response_format/json_schema/name")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Conversation titles: a plain completion whose system prompt asks
        // for "a conversation title". The answer is deliberately decorated
        // (wrapping quotes, trailing period) so tests see the caller's
        // sanitizer do its job rather than a pre-cleaned string.
        if request_text.contains("conversation title") {
            self.counters.title_requests.fetch_add(1, Ordering::Relaxed);
            if let Some(failure) = *self
                .counters
                .title_failure
                .lock()
                .expect("title_failure lock")
            {
                return with_delay(failure.response());
            }
            return with_delay(ResponseTemplate::new(200).set_body_json(json!({
                "id": "mock-cmpl",
                "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "\"Notes About Pelicans.\"" },
                    "finish_reason": "stop",
                }],
            })));
        }

        if wire_schema.is_none() && request_text.contains("citations_used:") {
            if request_text.contains("wikifail") {
                return with_delay(ResponseTemplate::new(400).set_body_json(json!({
                    "error": { "message": "mock wiki generation failure" }
                })));
            }
            let content = if request_text.contains("wiki article") {
                let n = count_numbered_sources(&body);
                let cited: Vec<i32> = (1..=n.min(2).max(1)).collect();
                let markers = cited
                    .iter()
                    .map(|i| format!("[{i}]"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let list = cited
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                // A `##` section is load-bearing: the section-ops applier
                // recognizes only ##/### headings, so an article without one
                // rejects every targeted operation.
                format!(
                    "# Mock Wiki\n\n## Overview\n\nThis is a deterministic mock article body. {markers}\n\nCITATIONS_USED: {list}"
                )
            } else {
                "# Mock Finding\n\nA deterministic mock finding body. [1]\n\nCITATIONS_USED: 1"
                    .to_string()
            };
            let mut response = ResponseTemplate::new(200).set_body_json(json!({
                "id": "mock-cmpl",
                "object": "chat.completion",
                "choices": [
                    {
                        "index": 0,
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop",
                    }
                ],
            }));
            if request_text.contains("wikislow") {
                response = response.set_delay(std::time::Duration::from_millis(1500));
            }
            return with_delay(response);
        }

        // Schema-less requests are routed by sniffing the schema the caller
        // stated in the prompt. Two paths arrive this way: the prompt-based
        // fallback, and every generative caller, which keeps `response_format`
        // off the wire entirely (see `GENERATIVE_CALL_ENFORCEMENT`).
        //
        // Distinctive property names are the discriminator, so order matters
        // where schemas overlap — consolidation also carries `parent_name` and
        // must be tested before extraction claims it. A schema that reaches
        // the fallback arm here answers `{}`, which surfaces as an unhelpful
        // "parse failed" far from the cause, so add an arm when adding a
        // generative call.
        let schema_name = wire_schema.unwrap_or_else(|| {
            if request_text.contains("after_heading") {
                "wiki_update_section_ops".to_string()
            } else if request_text.contains("winner_name") {
                "merge_result".to_string()
            } else if request_text.contains("tags_to_remove") {
                "consolidation_result".to_string()
            } else if request_text.contains("parent_name") {
                "extraction_result".to_string()
            } else {
                String::new()
            }
        });

        let content = match schema_name.as_str() {
            "extraction_result" => {
                let tag_name = if request_text.contains("biology") {
                    "Biology"
                } else if request_text.contains("cooking") || request_text.contains("pasta") {
                    "Cooking"
                } else {
                    "Physics"
                };
                json!({
                    "tags": [
                        { "name": tag_name, "parent_name": "Topics" },
                    ]
                })
                .to_string()
            }
            // Tag compaction. The route hands the LLM a flat list of
            // tag rows (`tag_id | name | parent_name | atom_count`). We
            // emit a single merge of the conventional test tag pair
            // ("MockLoser" → "MockWinner") when both names appear in the
            // prompt; otherwise return an empty array so the route
            // reports `tags_merged: 0` without touching real data.
            "merge_result" => {
                if request_text.contains("mockwinner") && request_text.contains("mockloser") {
                    json!({
                        "merges": [{
                            "winner_name": "MockWinner",
                            "loser_name": "MockLoser",
                            "reason": "Deterministic mock merge for the compaction test."
                        }]
                    })
                    .to_string()
                } else {
                    json!({ "merges": [] }).to_string()
                }
            }
            // Wiki incremental update: emit a single AppendToSection op
            // pinned to the heading the existing article uses, referencing
            // the first new-source index. Tests assert that the update
            // resolves a citation pointing at the freshly added atom.
            "wiki_update_section_ops" => {
                let new_index = first_new_source_index(&body).unwrap_or(2);
                json!({
                    "operations": [
                        {
                            "op": "AppendToSection",
                            "heading": "Overview",
                            "after_heading": "",
                            "content": format!(
                                "Additional mock context referencing the new source. [{new_index}]"
                            ),
                        }
                    ],
                    "citations_used": [new_index],
                })
                .to_string()
            }
            // Default: empty content, still valid JSON for callers that
            // tolerate-parse. Individual tests can assert on the request
            // shape they care about.
            _ => "{}".to_string(),
        };

        let response = ResponseTemplate::new(200).set_body_json(json!({
            "id": "mock-cmpl",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": content,
                    },
                    "finish_reason": "stop",
                }
            ],
        }));
        with_delay(response)
    }
}

// ==================== Ollama dialect ====================

/// `POST /api/embed` — Ollama's batch embedding endpoint. Same deterministic
/// embedder as the OpenAI surface, at [`OLLAMA_EMBED_DIM`] (see that
/// constant for why the width differs), and wrapped in Ollama's
/// `{ "embeddings": [[..]] }` envelope rather than OpenAI's `data` list.
struct OllamaEmbedResponder {
    counters: Arc<MockAiCounters>,
}

impl Respond for OllamaEmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.counters
            .embedding_requests
            .fetch_add(1, Ordering::Relaxed);
        if let Some(failure) = *self
            .counters
            .embedding_failure
            .lock()
            .expect("embedding_failure lock")
        {
            return failure.response();
        }
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return ResponseTemplate::new(400),
        };
        let Some(inputs) = body.get("input").and_then(|v| v.as_array()) else {
            return ResponseTemplate::new(400);
        };
        let embeddings: Vec<Vec<f32>> = inputs
            .iter()
            .map(|text| embed_text_at(text.as_str().unwrap_or_default(), OLLAMA_EMBED_DIM))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "model": body.get("model").cloned().unwrap_or(Value::Null),
            "embeddings": embeddings,
        }))
    }
}

/// `POST /api/chat` — Ollama's single chat endpoint, streaming and not.
///
/// The framing is the point. Where OpenAI streams `data: {...}` SSE frames
/// and accumulates tool-call arguments from string deltas, Ollama streams
/// **newline-delimited JSON objects**, terminated by one carrying
/// `done: true`, and emits each tool call complete in a single frame with
/// its arguments as a **JSON object** and no id and no `index`. The
/// provider synthesizes ids locally. Reproducing that faithfully is what
/// lets an agent-runtime test prove delta emission, tool accumulation and
/// cancellation against Ollama rather than against the OpenAI shape wearing
/// an Ollama label.
struct OllamaChatResponder {
    counters: Arc<MockAiCounters>,
}

impl Respond for OllamaChatResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.counters.chat_requests.fetch_add(1, Ordering::Relaxed);
        let delay = *self.counters.chat_delay.lock().expect("chat_delay lock");
        let with_delay = |response: ResponseTemplate| match delay {
            Some(d) => response.set_delay(d),
            None => response,
        };
        if let Some(failure) = *self
            .counters
            .chat_failure
            .lock()
            .expect("chat_failure lock")
        {
            return with_delay(failure.response());
        }
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return with_delay(ResponseTemplate::new(400)),
        };
        if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
            self.counters
                .chat_models
                .lock()
                .expect("chat_models lock")
                .push(model.to_string());
        }
        self.counters
            .chat_bodies
            .lock()
            .expect("chat_bodies lock")
            .push(body.clone());

        if body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            if let Some(script) = self.counters.scripted_stream() {
                return with_delay(script);
            }
            let force = self.counters.chat_force_tool_calls.load(Ordering::Relaxed);
            let tool_call = self
                .counters
                .chat_tool_call
                .lock()
                .expect("chat_tool_call lock")
                .clone();
            return with_delay(ollama_stream_response(&body, force, tool_call));
        }

        // Conversation titles are the one non-streaming call the chat era
        // makes, and the model it rides is provider-specific — so it has to
        // be answerable in this dialect too. Same deliberately decorated
        // answer as the OpenAI surface, so the same sanitizer assertions
        // hold whichever provider produced it.
        let request_text = body.to_string().to_lowercase();
        if request_text.contains("conversation title") {
            self.counters.title_requests.fetch_add(1, Ordering::Relaxed);
            if let Some(failure) = *self
                .counters
                .title_failure
                .lock()
                .expect("title_failure lock")
            {
                return with_delay(failure.response());
            }
            return with_delay(ollama_message_response("\"Notes About Pelicans.\""));
        }

        // Anything else non-streaming (tagging's structured `format` call,
        // the research loops' `complete_with_tools`) gets an empty, valid
        // answer. Those surfaces are covered against the OpenAI dialect;
        // scripting them here too would be duplicating the responder, not
        // the coverage.
        with_delay(ollama_message_response("{}"))
    }
}

/// Ollama's NDJSON stream: one JSON object per line, the last carrying
/// `done: true`. Content arrives as `message.content` fragments; tool calls
/// arrive whole, with object-valued arguments and no ids.
fn ollama_stream_response(
    body: &Value,
    force_tool_calls: bool,
    tool_call: Option<(String, Value)>,
) -> ResponseTemplate {
    let mut lines: Vec<Value> = Vec::new();
    match stream_leg(body, force_tool_calls, tool_call) {
        StreamLeg::Text(fragments) => {
            for text in fragments {
                lines.push(json!({
                    "model": "mock-ollama",
                    "message": { "role": "assistant", "content": text },
                    "done": false,
                }));
            }
        }
        StreamLeg::ToolCall {
            // Ollama never sends a tool-call id; the provider mints one.
            id: _,
            name,
            arguments,
        } => {
            lines.push(json!({
                "model": "mock-ollama",
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{ "function": { "name": name, "arguments": arguments } }],
                },
                "done": false,
            }));
        }
    }
    // The terminating frame. The provider stops reading at the first
    // `done: true`, so nothing may follow it.
    lines.push(json!({
        "model": "mock-ollama",
        "message": { "role": "assistant", "content": "" },
        "done": true,
        "done_reason": "stop",
    }));

    let ndjson = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "application/x-ndjson")
        .set_body_raw(ndjson.into_bytes(), "application/x-ndjson")
}

/// A non-streaming Ollama `/api/chat` answer carrying prose.
fn ollama_message_response(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "mock-ollama",
        "message": { "role": "assistant", "content": content },
        "done": true,
        "done_reason": "stop",
    }))
}
