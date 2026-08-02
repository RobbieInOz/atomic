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

        Ok(RunOutcome {
            content,
            messages,
            tool_calls,
            stop,
        })
    }
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
