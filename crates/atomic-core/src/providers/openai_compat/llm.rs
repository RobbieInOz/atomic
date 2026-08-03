//! OpenAI-compatible LLM implementation

use crate::providers::error::ProviderError;
use crate::providers::openai_compat::OpenAICompatProvider;
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
    response_format: Option<ResponseFormat>,
    stream: bool,
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

// ==================== Response Types ====================

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    /// Some servers put structured output in reasoning_content instead of content
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ResponseToolCall>>,
}

impl ResponseMessage {
    /// Get the effective content, falling back to reasoning_content if content is empty
    fn effective_content(&self) -> Option<String> {
        match &self.content {
            Some(c) if !c.is_empty() => Some(c.clone()),
            _ => self.reasoning_content.clone().filter(|r| !r.is_empty()),
        }
    }
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
}

#[derive(Deserialize)]
struct StreamingChoice {
    delta: StreamingDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamingDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
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
    provider: &OpenAICompatProvider,
    messages: &[Message],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    complete_internal(provider, messages, &[], config).await
}

pub async fn complete_with_tools(
    provider: &OpenAICompatProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    complete_internal(provider, messages, tools, config).await
}

async fn complete_internal(
    provider: &OpenAICompatProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(convert_message).collect();
    let api_tools: Option<Vec<ApiTool>> = if tools.is_empty() {
        None
    } else {
        Some(tools.iter().map(convert_tool).collect())
    };

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

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        tool_choice: None,
        response_format,
        stream: false,
    };

    let mut req = provider
        .client()
        .post(format!("{}/chat/completions", provider.base_url()))
        .header("Content-Type", "application/json");

    if let Some(api_key) = provider.api_key() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();

        if status == 429 {
            tracing::warn!(status, retry_after, model = %config.model, body_preview = %crate::providers::error::truncate_utf8(&body, 200), "OpenAI-compat LLM rate limited");
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        tracing::error!(status, model = %config.model, body_preview = %crate::providers::error::truncate_utf8(&body, 500), "OpenAI-compat LLM API error");
        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let body = response.text().await?;

    let chat_response: ChatResponse = serde_json::from_str(&body)
        .map_err(|e| {
            tracing::error!(error = %e, model = %config.model, body_preview = %crate::providers::error::truncate_utf8(&body, 500), "OpenAI-compat LLM parse error");
            ProviderError::ParseError(format!("Failed to parse chat response: {e}"))
        })?;

    let choice = chat_response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::ParseError("No choices in response".to_string()))?;

    let content = choice.message.effective_content().unwrap_or_default();
    let tool_calls = choice
        .message
        .tool_calls
        .map(|tcs| tcs.iter().map(convert_tool_call).collect());

    Ok(CompletionResponse {
        content,
        tool_calls,
        finish_reason: choice.finish_reason.clone(),
        native_finish_reason: None,
        completion_tokens: None,
        upstream_provider: None,
        generation_id: None,
    })
}

// ==================== Streaming Implementation ====================

pub async fn complete_streaming_with_tools(
    provider: &OpenAICompatProvider,
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

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        tool_choice: None,
        response_format: None,
        stream: true,
    };

    let mut req = provider
        .client()
        .post(format!("{}/chat/completions", provider.base_url()))
        .header("Content-Type", "application/json");

    if let Some(api_key) = provider.api_key() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.json(&request).send().await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();

        if status == 429 {
            tracing::warn!(status, retry_after, model = %config.model, body_preview = %crate::providers::error::truncate_utf8(&body, 200), "OpenAI-compat streaming LLM rate limited");
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        tracing::error!(status, model = %config.model, body_preview = %crate::providers::error::truncate_utf8(&body, 500), "OpenAI-compat streaming LLM API error");
        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let mut content = String::new();
    let mut tool_call_accumulators: Vec<ToolCallAccumulator> = Vec::new();
    let mut buffer = String::new();
    let mut finish_reason = None;
    let mut done_emitted = false;

    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| ProviderError::Network(e.to_string()))?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            if line == "data: [DONE]" {
                on_delta(StreamDelta::Done {
                    finish_reason: finish_reason.clone(),
                });
                done_emitted = true;
                break;
            }

            if let Some(json_str) = line.strip_prefix("data: ") {
                match serde_json::from_str::<StreamingResponse>(json_str) {
                    Err(e) => {
                        tracing::debug!(error = %e, chunk_preview = %crate::providers::error::truncate_utf8(json_str, 200), "OpenAI-compat stream chunk parse error");
                    }
                    Ok(response) => {
                        if let Some(choice) = response.choices.first() {
                            if choice.finish_reason.is_some() {
                                finish_reason = choice.finish_reason.clone();
                            }

                            let delta_content = choice
                                .delta
                                .content
                                .as_ref()
                                .filter(|c| !c.is_empty())
                                .or(choice
                                    .delta
                                    .reasoning_content
                                    .as_ref()
                                    .filter(|r| !r.is_empty()));
                            if let Some(delta_content) = delta_content {
                                content.push_str(delta_content);
                                on_delta(StreamDelta::Content(delta_content.clone()));
                            }

                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    while tool_call_accumulators.len() <= tc.index {
                                        tool_call_accumulators.push(ToolCallAccumulator::default());
                                    }

                                    let acc = &mut tool_call_accumulators[tc.index];
                                    let mut name_changed = false;

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
                                            on_delta(StreamDelta::ToolCallArguments {
                                                index: tc.index,
                                                arguments: args.clone(),
                                            });
                                        }
                                    }

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

    // Some OpenAI-compatible servers close the stream without sending [DONE]
    if !done_emitted {
        on_delta(StreamDelta::Done {
            finish_reason: finish_reason.clone(),
        });
    }

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
        native_finish_reason: None,
        completion_tokens: None,
        upstream_provider: None,
        generation_id: None,
    })
}

#[cfg(test)]
mod tests {
    //! What this parser does that the OpenRouter one does not.
    //!
    //! "OpenAI-compatible" is a family, not a spec, and the two divergences
    //! this provider carries exist because real servers in that family
    //! diverge: some (reasoning models behind vLLM/SGLang) put the answer in
    //! `reasoning_content` rather than `content`, and some close the
    //! connection without ever sending `data: [DONE]`. Both are silent
    //! failures if the parser stops handling them — an empty answer, or a
    //! stream that never signals completion. The shared machinery (argument
    //! accumulation across deltas) is pinned here too because it is a second
    //! copy of the code, free to drift from OpenRouter's.

    use std::sync::{Arc, Mutex};

    use atomic_test_support::MockAiServer;

    use super::*;
    use crate::providers::openai_compat::OpenAICompatProvider;
    use crate::providers::traits::StreamingLlmProvider;
    use crate::providers::types::GenerationParams;

    /// Drive one streaming completion against a scripted body, returning the
    /// assembled response and every delta the provider emitted, in order.
    async fn stream(script: &str) -> (CompletionResponse, Vec<StreamDelta>) {
        let mock = MockAiServer::start().await;
        mock.set_stream_script(Some(script));
        let provider = OpenAICompatProvider::new(mock.base_url(), Some("k".to_string()), Some(30));
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

    fn content_deltas(deltas: &[StreamDelta]) -> Vec<String> {
        deltas
            .iter()
            .filter_map(|delta| match delta {
                StreamDelta::Content(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Servers that stream the answer as `reasoning_content` are answered
    /// the same way as servers that use `content` — otherwise the turn
    /// arrives empty with no error to explain it.
    #[tokio::test]
    async fn reasoning_content_stands_in_for_an_empty_content_field() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"content":"","reasoning_content":"thought "},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"reasoning_content":"then answer"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;
        assert_eq!(response.content, "thought then answer");
        assert_eq!(content_deltas(&deltas), vec!["thought ", "then answer"]);
    }

    /// When both fields are populated, `content` wins — `reasoning_content`
    /// is a fallback, not an addition, and concatenating both would
    /// duplicate the answer.
    #[tokio::test]
    async fn content_wins_over_reasoning_content_when_both_arrive() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"content":"real","reasoning_content":"scratch"},"finish_reason":null}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;
        assert_eq!(response.content, "real");
        assert_eq!(content_deltas(&deltas), vec!["real"]);
    }

    /// A stream that just ends still reports completion. Without the
    /// fallback the caller would wait for a `Done` that never comes.
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

    /// Argument accumulation, pinned independently of OpenRouter's copy.
    #[tokio::test]
    async fn tool_call_arguments_accumulate_across_deltas() {
        let script = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"search_atoms","arguments":"{\"query\""}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"pelicans\"}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("a tool call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].get_name(), Some("search_atoms"));
        assert_eq!(calls[0].get_arguments(), Some(r#"{"query":"pelicans"}"#));
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));

        let starts = deltas
            .iter()
            .filter(|d| matches!(d, StreamDelta::ToolCallStart { .. }))
            .count();
        assert_eq!(starts, 1, "announced once, not once per fragment");
    }
}
