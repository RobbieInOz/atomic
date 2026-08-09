//! OpenRouter LLM implementation

use crate::providers::error::ProviderError;
use crate::providers::openrouter::OpenRouterProvider;
use crate::providers::traits::{LlmConfig, StreamCallback};
use crate::providers::types::{
    CompletionResponse, Message, StreamDelta, ToolCall, ToolCallFunction, ToolDefinition,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

// ==================== Request Types ====================

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderPreferences>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
    stream: bool,
}

#[derive(Serialize)]
struct ReasoningConfig {
    effort: String,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize)]
struct ApiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiFunctionDef,
}

#[derive(Serialize)]
struct ApiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiFunctionCall,
}

#[derive(Serialize, Clone)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<JsonSchemaWrapper>,
}

#[derive(Serialize)]
struct JsonSchemaWrapper {
    name: String,
    strict: bool,
    schema: serde_json::Value,
}

#[derive(Serialize)]
struct ProviderPreferences {
    require_parameters: bool,
}

// ==================== Response Types ====================

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ResponseUsage>,
    /// Which upstream host served the request (e.g. "Anthropic", "Azure").
    #[serde(default)]
    provider: Option<String>,
    /// OpenRouter's generation id (`gen-…`), the key into their
    /// per-generation details endpoint.
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    #[serde(default)]
    completion_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
    /// The upstream provider's RAW finish reason, before OpenRouter's
    /// normalization — diagnostic gold when a generation ends early with a
    /// normalized `stop` (e.g. an endpoint-side filter or emulation quirk).
    #[serde(default)]
    native_finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize, Clone)]
struct ResponseToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ResponseFunctionCall,
}

#[derive(Deserialize, Clone)]
struct ResponseFunctionCall {
    name: String,
    arguments: String,
}

// ==================== Streaming Types ====================

#[derive(Deserialize)]
struct StreamingResponse {
    choices: Vec<StreamingChoice>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct StreamingChoice {
    delta: StreamingDelta,
    finish_reason: Option<String>,
    #[serde(default)]
    native_finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamingDelta {
    content: Option<String>,
    tool_calls: Option<Vec<StreamingToolCall>>,
}

#[derive(Deserialize)]
struct StreamingToolCall {
    index: usize,
    id: Option<String>,
    #[serde(rename = "type")]
    call_type: Option<String>,
    function: Option<StreamingFunction>,
}

#[derive(Deserialize)]
struct StreamingFunction {
    name: Option<String>,
    arguments: Option<String>,
}

/// Accumulator for building complete tool calls from streaming deltas
#[derive(Default, Clone)]
struct ToolCallAccumulator {
    id: String,
    call_type: String,
    name: String,
    arguments: String,
}

// ==================== Conversion Functions ====================

fn convert_message(msg: &Message) -> ApiMessage {
    ApiMessage {
        role: msg.role.as_str().to_string(),
        content: msg.content.clone(),
        tool_calls: msg.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| ApiToolCall {
                    id: tc.id.clone(),
                    call_type: tc
                        .call_type
                        .clone()
                        .unwrap_or_else(|| "function".to_string()),
                    function: ApiFunctionCall {
                        name: tc.get_name().unwrap_or_default().to_string(),
                        arguments: tc.get_arguments().unwrap_or_default().to_string(),
                    },
                })
                .collect()
        }),
        tool_call_id: msg.tool_call_id.clone(),
        name: msg.name.clone(),
    }
}

fn convert_tool(tool: &ToolDefinition) -> ApiTool {
    ApiTool {
        tool_type: "function".to_string(),
        function: ApiFunctionDef {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        },
    }
}

fn convert_tool_call(tc: &ResponseToolCall) -> ToolCall {
    ToolCall {
        id: tc.id.clone(),
        call_type: Some(tc.call_type.clone()),
        function: Some(ToolCallFunction {
            name: tc.function.name.clone(),
            arguments: tc.function.arguments.clone(),
        }),
        name: None,
        arguments: None,
    }
}

// ==================== Non-Streaming Implementation ====================

pub async fn complete(
    provider: &OpenRouterProvider,
    messages: &[Message],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    complete_internal(provider, messages, &[], config, false).await
}

pub async fn complete_with_tools(
    provider: &OpenRouterProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    complete_internal(provider, messages, tools, config, false).await
}

async fn complete_internal(
    provider: &OpenRouterProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
    _stream: bool,
) -> Result<CompletionResponse, ProviderError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(convert_message).collect();
    let api_tools: Option<Vec<ApiTool>> = if tools.is_empty() {
        None
    } else {
        Some(tools.iter().map(convert_tool).collect())
    };

    // Build response format if structured output is requested
    let response_format = config
        .params
        .structured_output
        .as_ref()
        .map(|schema| ResponseFormat {
            format_type: "json_schema".to_string(),
            json_schema: Some(JsonSchemaWrapper {
                name: schema.name.clone(),
                strict: schema.strict,
                schema: schema.schema.clone(),
            }),
        });

    let provider_prefs = if config.params.structured_output.is_some() {
        Some(ProviderPreferences {
            require_parameters: true,
        })
    } else {
        None
    };

    // Filter parameters based on model support. Under `require_parameters`
    // (structured output), sampling preferences ride ONLY when positively
    // known supported: OpenRouter routes strict requests exclusively to
    // endpoints supporting every sent parameter, so an assumed-universal
    // param (temperature on the GPT-5 reasoning family) yields a routing 404
    // rather than being ignored. A preference must never constrain routing.
    let strict_routing = provider_prefs.is_some();
    let param_ok = |name: &str| {
        if strict_routing {
            config.params.is_param_known_supported(name)
        } else {
            config.params.is_param_supported(name)
        }
    };

    let temperature = if param_ok("temperature") {
        config.params.temperature
    } else {
        None
    };

    // max_tokens is exempt from the strict-routing filter: it's universal
    // across the OpenAI-compatible surface, and DROPPING it is the harmful
    // direction — Anthropic's API requires the field, so an absent value
    // gets a router-filled small default and long outputs come back
    // truncated mid-sentence (which is exactly what happened when strict
    // filtering stripped it here: capabilities are rarely loaded on
    // background paths, so `is_param_known_supported` said no to
    // everything). Temperature stays filtered — it's the param that
    // empties the endpoint pool into a routing 404 when over-sent.
    let max_tokens = config.params.max_tokens;

    // Only minimize reasoning when explicitly requested (for simple tasks like tag extraction)
    let reasoning =
        if config.params.minimize_reasoning && config.params.is_param_supported("reasoning") {
            Some(ReasoningConfig {
                effort: "minimal".to_string(),
            })
        } else {
            None
        };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        tool_choice: None,
        temperature,
        max_tokens,
        response_format,
        provider: provider_prefs,
        reasoning,
        stream: false,
    };

    let response = provider
        .client()
        .post(format!("{}/chat/completions", provider.base_url()))
        .header("Authorization", format!("Bearer {}", provider.api_key()))
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://atomic.app")
        .header("X-Title", "Atomic")
        .json(&request)
        .send()
        .await?;

    // The gateway's id for this request rides in the HEADERS, which arrive
    // before any body — so it survives exactly the failures where the body
    // does not. Capture it before anything consumes the response; without it
    // a call that delivered nothing leaves no thread back to what happened.
    let trace_id = crate::providers::error::gateway_trace_id(response.headers());

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();

        if status == 429 {
            tracing::warn!(status, retry_after, model = %config.model, body_preview = %crate::providers::error::body_for_log(&body, 200), "OpenRouter LLM rate limited");
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        tracing::error!(status, model = %config.model, generation_id = trace_id.as_deref().unwrap_or("-"), body_preview = %crate::providers::error::body_for_log(&body, 500), "OpenRouter LLM API error");
        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let body = response.text().await?;

    // A 200 carrying an error object instead of a completion — one of the two
    // exits left to a gateway that already committed its status. `choices` is
    // required, so without this the envelope would land as a permanent parse
    // failure. 502 marks it upstream-transient, matching the embedding path.
    if let Some((code, message)) = super::upstream_error(&body) {
        tracing::error!(
            model = %config.model,
            error_code = %code,
            error_message = %message,
            generation_id = trace_id.as_deref().unwrap_or("-"),
            "OpenRouter returned 200 with error body (upstream provider failure)"
        );
        return Err(ProviderError::Api {
            status: 502,
            message: format!("[upstream {}] {}", code, message),
        });
    }

    let chat_response: ChatResponse = serde_json::from_str(&body)
        .map_err(|e| {
            tracing::error!(error = %e, model = %config.model, generation_id = trace_id.as_deref().unwrap_or("-"), body_preview = %crate::providers::error::body_for_log(&body, 500), "OpenRouter LLM response decode failed");
            crate::providers::error::decode_error("chat response", &body, &e, trace_id.as_deref())
        })?;

    let completion_tokens = chat_response
        .usage
        .as_ref()
        .and_then(|u| u.completion_tokens);
    let upstream_provider = chat_response.provider.clone();
    let generation_id = chat_response.id.clone();
    let choice = chat_response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::ParseError("No choices in response".to_string()))?;

    let tool_calls = choice
        .message
        .tool_calls
        .map(|tcs| tcs.iter().map(convert_tool_call).collect());

    // Any non-`stop` end is worth a log line even when the caller treats
    // the response as success — an invisible finish reason lets a
    // truncated completion reach the database looking healthy.
    if let Some(reason) = choice.finish_reason.as_deref() {
        if reason != "stop" && reason != "tool_calls" {
            tracing::warn!(
                finish_reason = reason,
                native_finish_reason = choice.native_finish_reason.as_deref().unwrap_or("-"),
                completion_tokens,
                upstream_provider = upstream_provider.as_deref().unwrap_or("-"),
                model = %config.model,
                "OpenRouter completion ended early"
            );
        }
    }
    Ok(CompletionResponse {
        finish_reason: choice.finish_reason.clone(),
        native_finish_reason: choice.native_finish_reason.clone(),
        completion_tokens,
        upstream_provider,
        generation_id,
        content: choice.message.content.unwrap_or_default(),
        tool_calls,
    })
}

// ==================== Streaming Implementation ====================

pub async fn complete_streaming_with_tools(
    provider: &OpenRouterProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
    on_delta: StreamCallback,
) -> Result<CompletionResponse, ProviderError> {
    complete_streaming_internal(provider, messages, tools, config, on_delta).await
}

async fn complete_streaming_internal(
    provider: &OpenRouterProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
    on_delta: StreamCallback,
) -> Result<CompletionResponse, ProviderError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(convert_message).collect();
    let api_tools: Option<Vec<ApiTool>> = if tools.is_empty() {
        None
    } else {
        Some(tools.iter().map(convert_tool).collect())
    };

    // Only minimize reasoning when explicitly requested
    let reasoning =
        if config.params.minimize_reasoning && config.params.is_param_supported("reasoning") {
            Some(ReasoningConfig {
                effort: "minimal".to_string(),
            })
        } else {
            None
        };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        tool_choice: None,
        temperature: config.params.temperature,
        max_tokens: config.params.max_tokens,
        response_format: None, // Streaming doesn't support structured output
        provider: None,
        reasoning,
        stream: true,
    };

    let response = provider
        .client()
        .post(format!("{}/chat/completions", provider.base_url()))
        .header("Authorization", format!("Bearer {}", provider.api_key()))
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://atomic.app")
        .header("X-Title", "Atomic")
        .json(&request)
        .send()
        .await?;

    let trace_id = crate::providers::error::gateway_trace_id(response.headers());

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();

        if status == 429 {
            tracing::warn!(status, retry_after, model = %config.model, body_preview = %crate::providers::error::body_for_log(&body, 200), "OpenRouter streaming LLM rate limited");
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        tracing::error!(status, model = %config.model, generation_id = trace_id.as_deref().unwrap_or("-"), body_preview = %crate::providers::error::body_for_log(&body, 500), "OpenRouter streaming LLM API error");
        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    // Process the streaming response
    let mut content = String::new();
    let mut tool_call_accumulators: Vec<ToolCallAccumulator> = Vec::new();
    let mut buffer = String::new();
    let mut finish_reason = None;
    let mut native_finish_reason = None;
    let mut upstream_provider = None;
    let mut generation_id = None;
    let mut done_emitted = false;
    // Did the stream carry a single parseable SSE payload? A committed 200
    // that then delivers nothing but keepalive padding yields zero of them,
    // and would otherwise be indistinguishable from a model that legitimately
    // returned empty content.
    let mut saw_payload = false;

    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| ProviderError::Network(e.to_string()))?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete lines from buffer
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            // Skip empty lines
            if line.is_empty() {
                continue;
            }

            // Check for stream end
            if line == "data: [DONE]" {
                done_emitted = true;
                on_delta(StreamDelta::Done {
                    finish_reason: finish_reason.clone(),
                });
                break;
            }

            // Parse SSE data line
            if let Some(json_str) = line.strip_prefix("data: ") {
                match serde_json::from_str::<StreamingResponse>(json_str) {
                    Err(e) => {
                        tracing::debug!(error = %e, chunk_preview = %crate::providers::error::truncate_utf8(json_str, 200), "OpenRouter stream chunk parse error");
                    }
                    Ok(response) => {
                        saw_payload = true;
                        if response.provider.is_some() {
                            upstream_provider = response.provider.clone();
                        }
                        if response.id.is_some() {
                            generation_id = response.id.clone();
                        }
                        if let Some(choice) = response.choices.first() {
                            // Update finish reason
                            if choice.finish_reason.is_some() {
                                finish_reason = choice.finish_reason.clone();
                            }
                            if choice.native_finish_reason.is_some() {
                                native_finish_reason = choice.native_finish_reason.clone();
                            }

                            // Handle content delta
                            if let Some(delta_content) = &choice.delta.content {
                                content.push_str(delta_content);
                                on_delta(StreamDelta::Content(delta_content.clone()));
                            }

                            // Handle tool call deltas
                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    // Ensure accumulator exists for this index
                                    while tool_call_accumulators.len() <= tc.index {
                                        tool_call_accumulators.push(ToolCallAccumulator::default());
                                    }

                                    let acc = &mut tool_call_accumulators[tc.index];
                                    let mut name_changed = false;

                                    // Accumulate fields
                                    if let Some(id) = &tc.id {
                                        acc.id = id.clone();
                                    }
                                    if let Some(call_type) = &tc.call_type {
                                        acc.call_type = call_type.clone();
                                    }
                                    if let Some(func) = &tc.function {
                                        if let Some(name) = &func.name {
                                            if acc.name.is_empty() {
                                                acc.name = name.clone();
                                                name_changed = true;
                                            }
                                        }
                                        if let Some(args) = &func.arguments {
                                            acc.arguments.push_str(args);
                                            // Emit argument delta
                                            on_delta(StreamDelta::ToolCallArguments {
                                                index: tc.index,
                                                arguments: args.clone(),
                                            });
                                        }
                                    }

                                    // Emit tool call start when we have both id and name
                                    if name_changed && !acc.id.is_empty() && !acc.name.is_empty() {
                                        on_delta(StreamDelta::ToolCallStart {
                                            index: tc.index,
                                            id: acc.id.clone(),
                                            name: acc.name.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // A stream that carried no payload and no terminator delivered nothing at
    // all — the streaming face of a gateway that committed 200 and then ended
    // the body on padding. Returning `Ok` here would hand back an empty
    // completion that looks like a model choosing to say nothing, and the
    // caller would persist that silence as a result. Fail transiently instead,
    // matching the non-streaming path; errors return before `Done` is emitted,
    // as every earlier bail in this function does.
    if !saw_payload && !done_emitted {
        tracing::error!(
            model = %config.model,
            generation_id = trace_id.as_deref().unwrap_or("-"),
            "OpenRouter stream closed without delivering any payload"
        );
        return Err(crate::providers::error::stream_delivered_nothing(
            "chat stream",
            trace_id.as_deref(),
        ));
    }

    // Some upstreams close the stream without sending [DONE] — mirror the
    // openai_compat parser and still close out the delta stream exactly once.
    if !done_emitted {
        on_delta(StreamDelta::Done {
            finish_reason: finish_reason.clone(),
        });
    }

    // Convert accumulators to ToolCall
    let tool_calls = if tool_call_accumulators.is_empty() {
        None
    } else {
        Some(
            tool_call_accumulators
                .into_iter()
                .map(|acc| ToolCall {
                    id: acc.id,
                    call_type: Some(acc.call_type),
                    function: Some(ToolCallFunction {
                        name: acc.name,
                        arguments: acc.arguments,
                    }),
                    name: None,
                    arguments: None,
                })
                .collect(),
        )
    };

    Ok(CompletionResponse {
        content,
        tool_calls,
        finish_reason,
        native_finish_reason,
        // Usage arrives only in a final SSE chunk we don't request
        // (stream_options.include_usage); not worth the extra chunk here.
        completion_tokens: None,
        upstream_provider,
        generation_id,
    })
}

#[cfg(test)]
mod tests {
    //! The streaming parser against wire shapes OpenRouter actually
    //! produces and the generated mock responses never do.
    //!
    //! Two things are only reachable here. First, **argument accumulation**:
    //! a real provider dribbles a tool call's JSON out over many deltas, and
    //! [`ToolCallAccumulator`] exists solely to stitch them back together —
    //! a mock that sends each call whole in one delta exercises none of it.
    //! Second, the **OpenRouter-only metadata** (`provider`, generation `id`,
    //! `native_finish_reason`), which is diagnostic gold when a generation
    //! ends early and is silently dropped if the parser stops reading it.

    use std::sync::{Arc, Mutex};

    use atomic_test_support::MockAiServer;

    use super::*;
    use crate::providers::openrouter::OpenRouterProvider;
    use crate::providers::traits::StreamingLlmProvider;
    use crate::providers::types::GenerationParams;

    /// Drive one streaming completion against a scripted body, returning the
    /// assembled response and every delta the provider emitted, in order.
    async fn stream(script: &str) -> (CompletionResponse, Vec<StreamDelta>) {
        let mock = MockAiServer::start().await;
        mock.set_stream_script(Some(script));
        // A bare base URL: the provider appends `/v1` itself, which is how
        // the request finds the mock's OpenAI-shaped route.
        let provider = OpenRouterProvider::with_base_url("test-key".to_string(), mock.base_url());
        let deltas = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&deltas);
        let response = provider
            .complete_streaming_with_tools(
                &[Message::user("go")],
                &[ToolDefinition::new(
                    "search_atoms",
                    "search",
                    serde_json::json!({ "type": "object" }),
                )],
                &LlmConfig::new("mock-model").with_params(GenerationParams::new()),
                Box::new(move |delta| sink.lock().expect("delta sink").push(delta)),
            )
            .await
            .expect("streaming completion");
        let deltas = deltas.lock().expect("delta sink").clone();
        (response, deltas)
    }

    fn tool_arg_deltas(deltas: &[StreamDelta]) -> Vec<(usize, String)> {
        deltas
            .iter()
            .filter_map(|delta| match delta {
                StreamDelta::ToolCallArguments { index, arguments } => {
                    Some((*index, arguments.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn tool_starts(deltas: &[StreamDelta]) -> Vec<(usize, String, String)> {
        deltas
            .iter()
            .filter_map(|delta| match delta {
                StreamDelta::ToolCallStart { index, id, name } => {
                    Some((*index, id.clone(), name.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// A tool call arriving one fragment at a time — the normal case on a
    /// real stream — reassembles into one call with the whole argument
    /// string, and announces itself exactly once.
    #[tokio::test]
    async fn tool_call_arguments_accumulate_across_deltas() {
        let script = concat!(
            // The opening frame names the call; arguments start empty.
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"search_atoms","arguments":""}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\":"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"pelic"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ans\"}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("a tool call");
        assert_eq!(calls.len(), 1, "fragments are one call, not four");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].get_name(), Some("search_atoms"));
        assert_eq!(
            calls[0].get_arguments(),
            Some(r#"{"query":"pelicans"}"#),
            "the argument string must reassemble in order"
        );
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));

        // Announced once, on the frame that named it — repeated starts would
        // make a UI render a new tool card per fragment.
        assert_eq!(
            tool_starts(&deltas),
            vec![(0, "call_1".to_string(), "search_atoms".to_string())]
        );
        // Every fragment is forwarded as it arrives, all under index 0, and
        // together they spell the same string the accumulator built — a
        // consumer rendering the deltas live sees exactly what the finished
        // call says.
        let forwarded = tool_arg_deltas(&deltas);
        assert!(
            forwarded.iter().all(|(index, _)| *index == 0),
            "one call means one index: {forwarded:?}"
        );
        let joined: String = forwarded.into_iter().map(|(_, args)| args).collect();
        assert_eq!(joined, r#"{"query":"pelicans"}"#);
    }

    /// Two tool calls in one turn are kept apart by their `index`, even when
    /// their fragments interleave — which is what an accumulator keyed by
    /// index buys over one keyed by arrival order.
    #[tokio::test]
    async fn parallel_tool_calls_are_kept_apart_by_index() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"first","arguments":"{\"a\":"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"second","arguments":"{\"b\":"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("two tool calls");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].get_name(), Some("first"));
        assert_eq!(calls[0].get_arguments(), Some(r#"{"a":1}"#));
        assert_eq!(calls[1].id, "call_b");
        assert_eq!(calls[1].get_name(), Some("second"));
        assert_eq!(calls[1].get_arguments(), Some(r#"{"b":2}"#));

        assert_eq!(
            tool_starts(&deltas),
            vec![
                (0, "call_a".to_string(), "first".to_string()),
                (1, "call_b".to_string(), "second".to_string()),
            ]
        );
    }

    /// A tool call whose first frame carries only an index and arguments —
    /// no id, no name — still lands under its index. The upstream shape
    /// exists (some endpoints send the header frame late), and dropping such
    /// fragments would silently truncate the arguments.
    #[tokio::test]
    async fn argument_fragments_before_the_naming_frame_are_not_lost() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_late","type":"function","function":{"name":"search_atoms","arguments":"1}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("a tool call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_late");
        assert_eq!(calls[0].get_arguments(), Some(r#"{"q":1}"#));
        assert_eq!(
            tool_starts(&deltas),
            vec![(0, "call_late".to_string(), "search_atoms".to_string())],
            "the start fires once the call is nameable, not before"
        );
    }

    /// OpenRouter's routing metadata survives the stream: which upstream
    /// served the request, its generation id, and the upstream's raw finish
    /// reason behind OpenRouter's normalized one. All three are what make an
    /// early-ending generation diagnosable after the fact.
    #[tokio::test]
    async fn routing_metadata_and_native_finish_reason_survive() {
        let script = concat!(
            r#"data: {"id":"gen-abc123","provider":"Anthropic","choices":[{"delta":{"content":"partial "},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"content":"answer"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop","native_finish_reason":"max_tokens"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;

        assert_eq!(response.content, "partial answer");
        assert_eq!(response.upstream_provider.as_deref(), Some("Anthropic"));
        assert_eq!(response.generation_id.as_deref(), Some("gen-abc123"));
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            response.native_finish_reason.as_deref(),
            Some("max_tokens"),
            "a truncation hidden behind a normalized `stop` must still be visible"
        );

        // Content is forwarded chunk by chunk, and the sentinel closes with
        // the finish reason the last frame carried.
        let content: Vec<String> = deltas
            .iter()
            .filter_map(|delta| match delta {
                StreamDelta::Content(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(content, vec!["partial ", "answer"]);
        assert!(matches!(
            deltas.last(),
            Some(StreamDelta::Done { finish_reason }) if finish_reason.as_deref() == Some("stop")
        ));
    }

    /// Keep-alive comments and unparseable frames are noise, not failures:
    /// a stream carrying them still yields the content around them.
    #[tokio::test]
    async fn unparseable_frames_are_skipped_rather_than_failing_the_stream() {
        let script = concat!(
            ": OPENROUTER PROCESSING\n\n",
            r#"data: {"choices":[{"delta":{"content":"kept"},"finish_reason":null}]}"#,
            "\n\n",
            "data: {not json at all}\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, _deltas) = stream(script).await;
        assert_eq!(response.content, "kept");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
    }

    /// A stream that just ends still reports completion — mirrored from the
    /// openai_compat parser so the two SSE loops keep one contract.
    #[tokio::test]
    async fn a_stream_without_the_done_sentinel_still_completes() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"content":"all there is"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "\n\n",
        );
        let (response, deltas) = stream(script).await;
        assert_eq!(response.content, "all there is");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert!(
            matches!(
                deltas.last(),
                Some(StreamDelta::Done { finish_reason }) if finish_reason.as_deref() == Some("stop")
            ),
            "the close has to be synthesized: {deltas:?}"
        );
        assert_eq!(
            deltas
                .iter()
                .filter(|d| matches!(d, StreamDelta::Done { .. }))
                .count(),
            1,
            "exactly one close, never a duplicate"
        );
    }

    /// The sentinel is not double-counted when it *is* sent.
    #[tokio::test]
    async fn the_done_sentinel_closes_the_stream_exactly_once() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (_response, deltas) = stream(script).await;
        assert_eq!(
            deltas
                .iter()
                .filter(|d| matches!(d, StreamDelta::Done { .. }))
                .count(),
            1
        );
    }
}
