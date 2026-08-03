//! The agent loop: model → tools → model, until the run answers, signals
//! it is done, is cancelled, or exhausts its tool-call budget.
//!
//! The loop is transport-agnostic. It reports progress through a callback
//! (`RunEventSink`), the same pattern the embedding pipeline uses, so a host
//! can bridge it to Tauri events, a broadcast channel, or nothing at all.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;

use crate::providers::traits::{LlmConfig, LlmProvider, StreamCallback, StreamingLlmProvider};
use crate::providers::types::{
    CompletionResponse, GenerationParams, Message, StreamDelta, ToolCall, ToolDefinition,
};
use crate::providers::{create_streaming_llm_provider, get_llm_provider, ProviderConfig};

use super::citations::CitationLedger;
use super::context::truncate_messages_to_context;
use super::tools::{ToolContext, ToolRegistry, ToolResult};

/// Cooperative cancellation handle for one in-flight run. Raise the flag and
/// the loop stops at its next checkpoint — between iterations, between
/// sequential tool executions, and mid-stream — then returns whatever text
/// it had already produced. Hosts own the registry of flags; the loop only
/// reads them.
pub type CancelFlag = Arc<AtomicBool>;

/// Sink for [`RunEvent`]s. `None` on a run means nobody is watching.
pub type RunEventSink = Arc<dyn Fn(RunEvent) + Send + Sync + 'static>;

/// Progress a run makes while it is still in flight.
#[derive(Debug, Clone)]
pub enum RunEvent {
    /// One incremental chunk of model text, exactly as the provider
    /// produced it. Only emitted by streaming runs.
    Delta { text: String },
    /// A tool is about to execute.
    ToolStart {
        tool_call_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    /// A tool finished. `failed` mirrors the recorded call status, so a tool
    /// that errored reads as failed while the run is still going.
    ToolComplete {
        tool_call_id: String,
        tool_name: String,
        results_count: i32,
        failed: bool,
    },
}

/// How a run decides it is finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    /// The first model turn that asks for no tools is the answer, and its
    /// text is the run's content (chat).
    NoToolCalls,
    /// The model calls this sentinel tool to say research is over. The loop
    /// stops after that round having produced no answer of its own — the
    /// caller writes it in a separate pass over [`RunOutcome::messages`]
    /// (reports' `done`). A turn with no tool calls still ends the run.
    Sentinel(String),
}

impl Termination {
    fn is_sentinel(&self, tool_name: &str) -> bool {
        match self {
            Termination::NoToolCalls => false,
            Termination::Sentinel(name) => name == tool_name,
        }
    }
}

/// Why the loop stopped. Callers decide what a non-`Answered` ending looks
/// like to their users — the runtime does not editorialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model answered without asking for more tools.
    Answered,
    /// The run's sentinel tool fired.
    Sentinel,
    /// The cancel flag was raised.
    Cancelled,
    /// The tool-call budget ran out.
    IterationCap,
}

/// Everything about a run that is policy rather than content.
pub struct RunConfig {
    pub model: String,
    pub params: GenerationParams,
    /// Tool-calling rounds the run may take before it gives up.
    pub max_iterations: usize,
    pub termination: Termination,
    /// Stream the model's text as it arrives, emitting [`RunEvent::Delta`]
    /// per chunk. Non-streaming runs use the plain completion path.
    pub streaming: bool,
    /// When the cap hits with nothing said yet, spend one more completion
    /// with no tools offered so the run still yields prose instead of
    /// silence. Runs that write their answer in a separate final pass leave
    /// this off.
    pub salvage_on_cap: bool,
    /// Per-call prompt budget: trim the oldest tool rounds to ~70% of this
    /// many tokens. `None` sends the history as-is.
    pub context_length: Option<usize>,
}

/// One tool call as it happened, for callers that persist a transcript.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
    pub failed: bool,
    pub started_at: String,
    pub completed_at: String,
}

/// What a run produced.
pub struct RunOutcome {
    /// The model's answer when it gave one; the text it had streamed so far
    /// when the run was cut short; empty when the run ended on its sentinel
    /// and the caller writes the answer.
    pub content: String,
    /// The full message history including every tool round — the input to a
    /// caller's final pass.
    ///
    /// **Balanced only when the run wasn't cancelled.** The loop checks its
    /// cancel flag between tool executions, so a cancel landing partway
    /// through a round leaves an assistant message requesting N tool calls
    /// followed by fewer than N `tool` results. Providers reject that history:
    /// every `tool_calls` entry must be answered. Callers that replay these
    /// messages — a final synthesis pass, a resumed run — must therefore skip
    /// [`StopReason::Cancelled`] outcomes, or balance the history themselves
    /// with [`messages_are_balanced`]. Chat doesn't replay them at all, and
    /// reports pass no cancel flag, which is why nothing does today.
    pub messages: Vec<Message>,
    /// Every tool call made, in order.
    pub tool_calls: Vec<ToolCallRecord>,
    pub stop: StopReason,
}

/// Why a run could not produce anything. Callers own the user-facing
/// wording, so the variants carry the raw provider text rather than a
/// finished message.
#[derive(Debug)]
pub enum RunError {
    /// The provider could not be built from the run's configuration.
    Setup(String),
    /// A model call failed.
    Provider(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Setup(error) => write!(f, "{}", error),
            RunError::Provider(error) => write!(f, "{}", error),
        }
    }
}

impl std::error::Error for RunError {}

/// One agent run, assembled by the caller and executed once.
pub struct AgentRun<'a> {
    pub config: RunConfig,
    pub provider_config: &'a ProviderConfig,
    pub tools: &'a ToolRegistry,
    /// Shared with the tools for the duration of the run; the caller reads
    /// the cited subset back out of it afterwards.
    pub citations: &'a CitationLedger,
    /// System prompt plus history, already assembled by the caller.
    pub messages: Vec<Message>,
    pub cancel: Option<CancelFlag>,
    pub events: Option<RunEventSink>,
}

/// How often the loop samples the cancellation flag while a provider call is
/// in flight. Cancellation is cooperative and user-facing, so the bound only
/// has to be imperceptible, not instant.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

impl AgentRun<'_> {
    pub async fn execute(self) -> Result<RunOutcome, RunError> {
        let AgentRun {
            config,
            provider_config,
            tools,
            citations,
            mut messages,
            cancel,
            events,
        } = self;

        let completer = Completer::new(config.streaming, provider_config)?;
        let llm_config = LlmConfig::new(&config.model).with_params(config.params.clone());

        // Every token the model produced this run, across all iterations —
        // the same text a streaming caller has been rendering. Only used
        // when the run ends without a clean final answer.
        let produced = Arc::new(Mutex::new(String::new()));
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
        let mut content = String::new();
        let mut stop = StopReason::IterationCap;

        'iterations: for _ in 0..config.max_iterations {
            if is_cancelled(&cancel) {
                stop = StopReason::Cancelled;
                break;
            }

            let call_messages =
                truncate_messages_to_context(messages.clone(), config.context_length);
            let on_delta = config
                .streaming
                .then(|| delta_callback(events.clone(), Arc::clone(&produced), cancel.clone()));

            // Racing the request against the cancel flag drops the request
            // future on cancel, which tears down the in-flight HTTP stream
            // instead of paying for tokens nobody will read.
            let response = tokio::select! {
                biased;
                result = completer.complete(&call_messages, tools.definitions(), &llm_config, on_delta) => result?,
                _ = cancellation_raised(&cancel) => {
                    stop = StopReason::Cancelled;
                    break;
                }
            };

            let requested = match response.tool_calls {
                Some(ref calls) if !calls.is_empty() => calls.clone(),
                _ => {
                    content = response.content;
                    stop = StopReason::Answered;
                    break;
                }
            };

            // Non-streaming runs have no deltas to accumulate, so preamble
            // text is captured here instead.
            if !config.streaming && !response.content.is_empty() {
                accumulate(&produced, &response.content);
            }
            messages.push(assistant_turn(&response.content, requested.clone()));

            let mut sentinel_fired = false;
            for call in &requested {
                if is_cancelled(&cancel) {
                    stop = StopReason::Cancelled;
                    break 'iterations;
                }

                let name = call.get_name().unwrap_or_default().to_string();
                let args: serde_json::Value =
                    serde_json::from_str(call.get_arguments().unwrap_or_default())
                        .unwrap_or(serde_json::Value::Null);

                emit(
                    &events,
                    RunEvent::ToolStart {
                        tool_call_id: call.id.clone(),
                        tool_name: name.clone(),
                        input: args.clone(),
                    },
                );

                let started_at = Utc::now().to_rfc3339();
                let result = match tools.get(&name) {
                    Some(tool) => tool.execute(&args, &ToolContext { citations }).await,
                    // The model asked for a tool this run doesn't have — a
                    // failed call, not an empty result.
                    None => ToolResult::failed(format!("Unknown tool: {}", name)),
                };
                sentinel_fired |= config.termination.is_sentinel(&name);

                tool_calls.push(ToolCallRecord {
                    id: call.id.clone(),
                    name: name.clone(),
                    input: args,
                    output: result.output.clone(),
                    failed: result.failed,
                    started_at,
                    completed_at: Utc::now().to_rfc3339(),
                });

                emit(
                    &events,
                    RunEvent::ToolComplete {
                        tool_call_id: call.id.clone(),
                        tool_name: name,
                        results_count: result.results_count,
                        failed: result.failed,
                    },
                );

                messages.push(Message::tool_result(&call.id, result.output));
            }

            if sentinel_fired {
                stop = StopReason::Sentinel;
                break;
            }
        }

        if matches!(stop, StopReason::Cancelled | StopReason::IterationCap) {
            let partial = produced.lock().map(|text| text.clone()).unwrap_or_default();
            content = if stop == StopReason::IterationCap
                && config.salvage_on_cap
                && partial.trim().is_empty()
            {
                // Out of tool-call budget with nothing said. Salvage rather
                // than discard: one more call, without tools, so the model
                // has to answer in prose from the context it gathered.
                salvage(
                    &completer,
                    &config,
                    &llm_config,
                    &messages,
                    &produced,
                    &events,
                    &cancel,
                )
                .await
            } else {
                partial
            };
        }

        debug_assert!(
            stop == StopReason::Cancelled || messages_are_balanced(&messages),
            "a run that wasn't cancelled must leave every requested tool call answered"
        );
        Ok(RunOutcome {
            content,
            messages,
            tool_calls,
            stop,
        })
    }
}

/// Whether every tool call requested in `messages` has a matching `tool`
/// result — the precondition providers impose on a replayed history. See
/// [`RunOutcome::messages`] for when it can fail to hold.
pub fn messages_are_balanced(messages: &[Message]) -> bool {
    let answered: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    messages
        .iter()
        .filter_map(|message| message.tool_calls.as_ref())
        .flatten()
        .all(|call| answered.contains(call.id.as_str()))
}

/// One tool-free completion to turn a run's gathered context into an answer.
/// A failure here is not fatal — the run returns what it has and the caller
/// decides how to label it.
async fn salvage(
    completer: &Completer,
    config: &RunConfig,
    llm_config: &LlmConfig,
    messages: &[Message],
    produced: &Arc<Mutex<String>>,
    events: &Option<RunEventSink>,
    cancel: &Option<CancelFlag>,
) -> String {
    let call_messages = truncate_messages_to_context(messages.to_vec(), config.context_length);
    let on_delta = config
        .streaming
        .then(|| delta_callback(events.clone(), Arc::clone(produced), cancel.clone()));
    match completer
        .complete(&call_messages, &[], llm_config, on_delta)
        .await
    {
        Ok(response) => response.content,
        Err(error) => {
            tracing::warn!(%error, "[agent_runtime] iteration-cap salvage completion failed");
            String::new()
        }
    }
}

/// Assistant turn carrying the tool calls it requested, keeping any text it
/// said alongside them.
fn assistant_turn(content: &str, tool_calls: Vec<ToolCall>) -> Message {
    if content.is_empty() {
        Message::assistant_with_tool_calls(tool_calls)
    } else {
        let mut message = Message::assistant(content);
        message.tool_calls = Some(tool_calls);
        message
    }
}

/// Forward every chunk to the run's sink as it arrives and keep a copy for
/// the salvage paths.
fn delta_callback(
    events: Option<RunEventSink>,
    produced: Arc<Mutex<String>>,
    cancel: Option<CancelFlag>,
) -> StreamCallback {
    Box::new(move |delta: StreamDelta| {
        let StreamDelta::Content(text) = delta else {
            return;
        };
        if text.is_empty() || is_cancelled(&cancel) {
            return;
        }
        accumulate(&produced, &text);
        emit(&events, RunEvent::Delta { text });
    })
}

fn accumulate(produced: &Arc<Mutex<String>>, text: &str) {
    if let Ok(mut accumulated) = produced.lock() {
        accumulated.push_str(text);
    }
}

fn emit(events: &Option<RunEventSink>, event: RunEvent) {
    if let Some(sink) = events {
        sink(event);
    }
}

fn is_cancelled(cancel: &Option<CancelFlag>) -> bool {
    cancel
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// Resolves once `cancel` is raised; never resolves when there is no flag.
/// Polling keeps cancellation out of the provider traits — the loop races
/// this against the in-flight request and drops the request future on cancel.
async fn cancellation_raised(cancel: &Option<CancelFlag>) {
    match cancel {
        Some(flag) => {
            while !flag.load(Ordering::Relaxed) {
                tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
            }
        }
        None => std::future::pending().await,
    }
}

/// The provider a run talks to, streaming or not. Both legs return the same
/// `CompletionResponse`, so the loop above never branches on transport.
enum Completer {
    Streaming(Arc<dyn StreamingLlmProvider>),
    Blocking(Arc<dyn LlmProvider>),
}

impl Completer {
    fn new(streaming: bool, config: &ProviderConfig) -> Result<Self, RunError> {
        if streaming {
            create_streaming_llm_provider(config)
                .map(Completer::Streaming)
                .map_err(|e| RunError::Setup(e.to_string()))
        } else {
            get_llm_provider(config)
                .map(Completer::Blocking)
                .map_err(|e| RunError::Setup(e.to_string()))
        }
    }

    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &LlmConfig,
        on_delta: Option<StreamCallback>,
    ) -> Result<CompletionResponse, RunError> {
        match self {
            Completer::Streaming(provider) => {
                let on_delta = on_delta.unwrap_or_else(|| Box::new(|_| {}));
                provider
                    .complete_streaming_with_tools(messages, tools, config, on_delta)
                    .await
            }
            Completer::Blocking(provider) => {
                provider.complete_with_tools(messages, tools, config).await
            }
        }
        .map_err(|e| RunError::Provider(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    //! The loop's provider-facing contract, proven against **every** wire
    //! dialect rather than only the OpenAI-compatible SSE shape the
    //! integration suite drives.
    //!
    //! This matters because the three providers diverge exactly where this
    //! module is most load-bearing. OpenRouter and OpenAI-compat stream SSE
    //! frames and rebuild each tool call from string-valued `arguments`
    //! deltas keyed by `index`; Ollama streams newline-delimited JSON, emits
    //! each tool call whole with **object**-valued arguments, sends no id and
    //! no index, and leaves the assembled response's `finish_reason` unset. A
    //! loop that only ever saw the first shape could depend on any of those
    //! accidents. Every test below therefore runs its assertions once per
    //! dialect against a mock server that speaks all three.

    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use atomic_test_support::{InjectedFailure, MockAiServer};
    use serde_json::json;

    use super::*;
    use crate::agent_runtime::citations::CitationAdmission;
    use crate::agent_runtime::AgentTool;
    use crate::providers::types::ToolDefinition;
    use crate::providers::ProviderType;

    /// A tool that always succeeds, so a run can get past its first turn.
    struct Echo;

    #[async_trait]
    impl AgentTool for Echo {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "echo",
                "Echo the note back",
                json!({
                    "type": "object",
                    "properties": { "note": { "type": "string" } },
                    "required": ["note"],
                    "additionalProperties": false,
                }),
            )
        }

        async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext<'_>) -> ToolResult {
            ToolResult::ok(
                format!("echo: {}", args["note"].as_str().unwrap_or("<missing>")),
                1,
            )
        }
    }

    /// The three provider dialects, each pointed at the same mock server.
    /// Iterated by every test so a divergence shows up as a named failure.
    fn dialects(mock: &MockAiServer) -> Vec<(&'static str, ProviderConfig)> {
        let base = ProviderConfig::from_settings(&std::collections::HashMap::new());

        let mut openrouter = base.clone();
        openrouter.provider_type = ProviderType::OpenRouter;
        openrouter.openrouter_api_key = Some("mock-openrouter-key".to_string());
        // Deliberately the bare mock URL: the provider's own normalization
        // has to append `/v1` for this to reach the mock at all.
        openrouter.openrouter_base_url = mock.base_url();

        let mut compat = base.clone();
        compat.provider_type = ProviderType::OpenAICompat;
        compat.openai_compat_base_url = mock.base_url();
        compat.openai_compat_api_key = Some("mock-compat-key".to_string());

        let mut ollama = base;
        ollama.provider_type = ProviderType::Ollama;
        ollama.ollama_host = mock.base_url();

        vec![
            ("openrouter", openrouter),
            ("openai_compat", compat),
            ("ollama", ollama),
        ]
    }

    /// Collect every [`RunEvent`] a run emits.
    fn event_sink() -> (RunEventSink, Arc<Mutex<Vec<RunEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (
            Arc::new(move |event| sink.lock().expect("event sink").push(event)),
            events,
        )
    }

    fn deltas(events: &Arc<Mutex<Vec<RunEvent>>>) -> Vec<String> {
        events
            .lock()
            .expect("event sink")
            .iter()
            .filter_map(|event| match event {
                RunEvent::Delta { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// A streaming chat-shaped run: the configuration `agent::chat` uses,
    /// with the caps a test can afford.
    fn chat_config(max_iterations: usize, salvage_on_cap: bool) -> RunConfig {
        RunConfig {
            model: "mock-model".to_string(),
            params: GenerationParams::new()
                .with_temperature(0.7)
                .with_max_tokens(4000),
            max_iterations,
            termination: Termination::NoToolCalls,
            streaming: true,
            salvage_on_cap,
            context_length: None,
        }
    }

    /// Reset every knob and counter so each dialect's pass starts clean.
    fn reset(mock: &MockAiServer) {
        mock.reset_counts();
        mock.set_chat_failure(None);
        mock.set_chat_delay(None);
        mock.set_chat_force_tool_calls(false);
        mock.set_chat_tool_call(None);
    }

    /// Text streams back chunk by chunk on every dialect, and the chunks
    /// concatenate to the answer rather than repeating it. NDJSON framing
    /// has to yield the same per-fragment cadence as SSE.
    #[tokio::test]
    async fn streamed_text_arrives_one_event_per_chunk_on_every_dialect() {
        let mock = MockAiServer::start().await;
        for (dialect, provider_config) in dialects(&mock) {
            reset(&mock);
            let (sink, events) = event_sink();
            let ledger = CitationLedger::new(CitationAdmission::Open);
            let outcome = AgentRun {
                config: chat_config(4, true),
                provider_config: &provider_config,
                tools: &ToolRegistry::new(),
                citations: &ledger,
                messages: vec![Message::user("what did I write about pelicans?")],
                cancel: None,
                events: Some(sink),
            }
            .execute()
            .await
            .unwrap_or_else(|e| panic!("{dialect}: run failed: {e}"));

            assert_eq!(outcome.stop, StopReason::Answered, "{dialect}");
            let deltas = deltas(&events);
            assert!(
                deltas.len() > 1,
                "{dialect}: expected one event per provider chunk, got {deltas:?}"
            );
            assert_eq!(
                deltas.concat(),
                outcome.content,
                "{dialect}: deltas must concatenate to the answer, not repeat it"
            );
        }
    }

    /// A tool call streamed by the provider reaches the loop intact — name
    /// and arguments both — is dispatched, and its result carries the run
    /// into a second turn that answers.
    ///
    /// This is the accumulation contract at its most dialect-sensitive:
    /// OpenAI-shaped providers rebuild `arguments` from string deltas while
    /// Ollama hands over a JSON object, and only one of those needs an id
    /// from the wire. Either way the loop must see the same decoded input.
    #[tokio::test]
    async fn streamed_tool_calls_reach_the_loop_intact_on_every_dialect() {
        let mock = MockAiServer::start().await;
        for (dialect, provider_config) in dialects(&mock) {
            reset(&mock);
            mock.set_chat_tool_call(Some(("echo", json!({ "note": "pelicans" }))));

            let (sink, events) = event_sink();
            let ledger = CitationLedger::new(CitationAdmission::Open);
            let outcome = AgentRun {
                config: chat_config(4, true),
                provider_config: &provider_config,
                tools: &ToolRegistry::new().with(Echo),
                citations: &ledger,
                messages: vec![Message::user("echo something")],
                cancel: None,
                events: Some(sink),
            }
            .execute()
            .await
            .unwrap_or_else(|e| panic!("{dialect}: run failed: {e}"));

            assert_eq!(outcome.stop, StopReason::Answered, "{dialect}");
            assert_eq!(
                outcome.tool_calls.len(),
                1,
                "{dialect}: exactly one tool round then an answer"
            );
            let call = &outcome.tool_calls[0];
            assert_eq!(call.name, "echo", "{dialect}");
            assert_eq!(
                call.input,
                json!({ "note": "pelicans" }),
                "{dialect}: the arguments must survive the provider's encoding"
            );
            assert!(!call.failed, "{dialect}: {}", call.output);
            assert_eq!(call.output, "echo: pelicans", "{dialect}");
            assert!(
                !call.id.is_empty(),
                "{dialect}: every call needs an id, even where the wire sends none"
            );

            // The events a UI renders mirror the record.
            let started: Vec<_> = events
                .lock()
                .expect("event sink")
                .iter()
                .filter_map(|event| match event {
                    RunEvent::ToolStart {
                        tool_name, input, ..
                    } => Some((tool_name.clone(), input.clone())),
                    _ => None,
                })
                .collect();
            assert_eq!(
                started,
                vec![("echo".to_string(), json!({ "note": "pelicans" }))],
                "{dialect}"
            );

            // And the loop went on to answer from the tool result.
            assert!(
                outcome.content.contains("Mock assistant reply"),
                "{dialect}: second turn should answer, got {:?}",
                outcome.content
            );
        }
    }

    /// Out of tool-call budget with nothing said, the run spends one more
    /// completion **with no tools offered** so it yields prose instead of
    /// silence — on every dialect, since "offered no tools" is encoded
    /// differently by each and the mock only answers in prose when it sees
    /// none.
    #[tokio::test]
    async fn iteration_cap_salvages_with_a_tool_free_call_on_every_dialect() {
        let mock = MockAiServer::start().await;
        for (dialect, provider_config) in dialects(&mock) {
            reset(&mock);
            // A model that never stops researching.
            mock.set_chat_force_tool_calls(true);
            mock.set_chat_tool_call(Some(("echo", json!({ "note": "again" }))));

            let ledger = CitationLedger::new(CitationAdmission::Open);
            let outcome = AgentRun {
                config: chat_config(2, true),
                provider_config: &provider_config,
                tools: &ToolRegistry::new().with(Echo),
                citations: &ledger,
                messages: vec![Message::user("keep digging")],
                cancel: None,
                events: None,
            }
            .execute()
            .await
            .unwrap_or_else(|e| panic!("{dialect}: run failed: {e}"));

            assert_eq!(outcome.stop, StopReason::IterationCap, "{dialect}");
            assert_eq!(
                outcome.tool_calls.len(),
                2,
                "{dialect}: the budget is spent before salvage"
            );
            assert!(
                outcome.content.contains("Mock assistant reply"),
                "{dialect}: salvage must produce prose, got {:?}",
                outcome.content
            );

            // The salvage call is the last one, and it offered no tools —
            // that is what forces the model to answer instead of researching.
            let bodies = mock.chat_request_bodies();
            assert_eq!(
                bodies.len(),
                3,
                "{dialect}: two capped turns plus one salvage"
            );
            let salvage = bodies.last().expect("salvage request");
            assert!(
                salvage
                    .get("tools")
                    .map(|tools| tools.as_array().is_some_and(|t| t.is_empty()))
                    .unwrap_or(true),
                "{dialect}: the salvage call must offer no tools: {salvage}"
            );
        }
    }

    /// Raising the cancel flag while a request is in flight drops that
    /// request rather than waiting it out — the `tokio::select!` race, which
    /// the integration suite's callback-driven cancellation never reaches
    /// (it flips the flag between tool executions instead).
    ///
    /// Proven by latency: the provider is held for far longer than the run
    /// is allowed to take.
    #[tokio::test]
    async fn cancelling_an_in_flight_request_stops_the_run_on_every_dialect() {
        const PROVIDER_HOLD: Duration = Duration::from_secs(30);
        // How long the run may take to notice, measured from the flag going
        // up. Generous against a loaded box, still an order of magnitude
        // under the hold — a run that waited the provider out would land at
        // ~30s and fail loudly.
        const CANCEL_BUDGET: Duration = Duration::from_secs(3);

        let mock = MockAiServer::start().await;
        for (dialect, provider_config) in dialects(&mock) {
            reset(&mock);
            mock.set_chat_delay(Some(PROVIDER_HOLD));

            let cancel: CancelFlag = Arc::new(AtomicBool::new(false));
            let trigger = Arc::clone(&cancel);
            // Stamped before the flag goes up, so a run that observes the
            // flag always sees a stamp.
            let raised_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
            let stamp = Arc::clone(&raised_at);
            let raiser = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                *stamp.lock().expect("raise stamp") = Some(Instant::now());
                trigger.store(true, Ordering::Relaxed);
            });

            let ledger = CitationLedger::new(CitationAdmission::Open);
            let outcome = AgentRun {
                config: chat_config(4, true),
                provider_config: &provider_config,
                tools: &ToolRegistry::new().with(Echo),
                citations: &ledger,
                messages: vec![Message::user("start something slow")],
                cancel: Some(cancel),
                events: None,
            }
            .execute()
            .await
            .unwrap_or_else(|e| panic!("{dialect}: a cancelled run still returns: {e}"));
            let returned_at = Instant::now();
            raiser.await.expect("flag raiser joined");

            assert_eq!(outcome.stop, StopReason::Cancelled, "{dialect}");

            // Latency is measured from the flag going up, not from the start
            // of the run: the first HTTP client a process builds can stall
            // the runtime for seconds loading the system trust store, which
            // has nothing to do with how fast cancellation is noticed.
            let raised_at = raised_at
                .lock()
                .expect("raise stamp")
                .expect("the flag was raised before the run returned");
            let latency = returned_at.duration_since(raised_at);
            assert!(
                latency < CANCEL_BUDGET,
                "{dialect}: the run must abandon the in-flight request, not wait \
                 it out — noticing took {latency:?}, and the provider was only \
                 going to answer after {PROVIDER_HOLD:?}"
            );
            assert!(
                outcome.tool_calls.is_empty(),
                "{dialect}: nothing ran, so nothing is recorded"
            );
        }
    }

    /// A dead provider is a `RunError::Provider` carrying the provider's own
    /// words — the variant chat turns into a `ChatEvent::Error`. Each dialect
    /// has its own error-decoding path; all three must classify a 401 the
    /// same way.
    #[tokio::test]
    async fn a_failing_provider_is_a_run_error_on_every_dialect() {
        let mock = MockAiServer::start().await;
        for (dialect, provider_config) in dialects(&mock) {
            reset(&mock);
            mock.set_chat_failure(Some(InjectedFailure::Unauthorized));

            let ledger = CitationLedger::new(CitationAdmission::Open);
            let error = AgentRun {
                config: chat_config(4, true),
                provider_config: &provider_config,
                tools: &ToolRegistry::new().with(Echo),
                citations: &ledger,
                messages: vec![Message::user("anything")],
                cancel: None,
                events: None,
            }
            .execute()
            .await
            .err()
            .unwrap_or_else(|| panic!("{dialect}: a dead provider must fail the run"));

            assert!(
                matches!(error, RunError::Provider(_)),
                "{dialect}: got {error:?}"
            );
            assert!(
                error.to_string().contains("401"),
                "{dialect}: the provider's status has to survive: {error}"
            );
        }
    }
}
