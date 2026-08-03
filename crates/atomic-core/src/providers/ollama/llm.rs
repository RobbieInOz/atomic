//! Ollama LLM implementation

use crate::providers::error::ProviderError;
use crate::providers::ollama::OllamaProvider;
use crate::providers::traits::{LlmConfig, StreamCallback};
use crate::providers::types::{
    CompletionResponse, Message, MessageRole, StreamDelta, ToolCall, ToolCallFunction,
    ToolDefinition,
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
    format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<ChatOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
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
    function: ApiFunctionCall,
}

#[derive(Serialize, Clone)]
struct ApiFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

// ==================== Response Types ====================

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
    #[allow(dead_code)]
    done: bool,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize, Clone)]
struct ResponseToolCall {
    function: ResponseFunctionCall,
}

#[derive(Deserialize, Clone)]
struct ResponseFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

// ==================== Streaming Types ====================

#[derive(Deserialize)]
struct StreamingResponse {
    message: StreamingMessage,
    done: bool,
}

#[derive(Deserialize, Default)]
struct StreamingMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<ResponseToolCall>>,
}

// ==================== Conversion Functions ====================

fn convert_message(msg: &Message) -> ApiMessage {
    let role = match msg.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };

    ApiMessage {
        role: role.to_string(),
        content: msg.content.clone().unwrap_or_default(),
        tool_calls: msg.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| {
                    let args_str = tc.get_arguments().unwrap_or("{}");
                    let args: serde_json::Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    ApiToolCall {
                        function: ApiFunctionCall {
                            name: tc.get_name().unwrap_or_default().to_string(),
                            arguments: args,
                        },
                    }
                })
                .collect()
        }),
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

fn convert_tool_call(tc: &ResponseToolCall, _index: usize) -> ToolCall {
    // Ollama returns arguments as parsed JSON, we need to stringify it
    let arguments = serde_json::to_string(&tc.function.arguments).unwrap_or_default();

    ToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4()),
        call_type: Some("function".to_string()),
        function: Some(ToolCallFunction {
            name: tc.function.name.clone(),
            arguments,
        }),
        name: None,
        arguments: None,
    }
}

// ==================== Non-Streaming Implementation ====================

pub async fn complete(
    provider: &OllamaProvider,
    messages: &[Message],
    config: &LlmConfig,
) -> Result<CompletionResponse, ProviderError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(convert_message).collect();

    // Build format if structured output is requested
    let format = config
        .params
        .structured_output
        .as_ref()
        .map(|schema| schema.schema.clone());

    let options = if config.params.temperature.is_some() || config.params.max_tokens.is_some() {
        Some(ChatOptions {
            temperature: config.params.temperature,
            num_predict: config.params.max_tokens,
        })
    } else {
        None
    };

    // Disable thinking for faster responses when minimize_reasoning is true
    let think = if config.params.minimize_reasoning {
        Some(false)
    } else {
        None
    };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: None,
        format,
        options,
        think,
        stream: false,
    };

    let response = provider
        .client()
        .post(format!("{}/api/chat", provider.base_url()))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let chat_response: ChatResponse = response.json().await?;

    let tool_calls = chat_response.message.tool_calls.map(|tcs| {
        tcs.iter()
            .enumerate()
            .map(|(i, tc)| convert_tool_call(tc, i))
            .collect()
    });

    Ok(CompletionResponse {
        content: chat_response.message.content,
        tool_calls,
        finish_reason: None,
        native_finish_reason: None,
        completion_tokens: None,
        upstream_provider: None,
        generation_id: None,
    })
}

pub async fn complete_with_tools(
    provider: &OllamaProvider,
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

    let format = config
        .params
        .structured_output
        .as_ref()
        .map(|schema| schema.schema.clone());

    let options = if config.params.temperature.is_some() || config.params.max_tokens.is_some() {
        Some(ChatOptions {
            temperature: config.params.temperature,
            num_predict: config.params.max_tokens,
        })
    } else {
        None
    };

    // Disable thinking for faster responses when minimize_reasoning is true
    let think = if config.params.minimize_reasoning {
        Some(false)
    } else {
        None
    };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        format,
        options,
        think,
        stream: false,
    };

    let response = provider
        .client()
        .post(format!("{}/api/chat", provider.base_url()))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let chat_response: ChatResponse = response.json().await?;

    let tool_calls = chat_response.message.tool_calls.map(|tcs| {
        tcs.iter()
            .enumerate()
            .map(|(i, tc)| convert_tool_call(tc, i))
            .collect()
    });

    Ok(CompletionResponse {
        content: chat_response.message.content,
        tool_calls,
        finish_reason: None,
        native_finish_reason: None,
        completion_tokens: None,
        upstream_provider: None,
        generation_id: None,
    })
}

// ==================== Streaming Implementation ====================

pub async fn complete_streaming_with_tools(
    provider: &OllamaProvider,
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

    let options = if config.params.temperature.is_some() || config.params.max_tokens.is_some() {
        Some(ChatOptions {
            temperature: config.params.temperature,
            num_predict: config.params.max_tokens,
        })
    } else {
        None
    };

    // Disable thinking for faster responses when minimize_reasoning is true
    let think = if config.params.minimize_reasoning {
        Some(false)
    } else {
        None
    };

    let request = ChatRequest {
        model: config.model.clone(),
        messages: api_messages,
        tools: api_tools,
        format: None, // Streaming doesn't support structured output
        options,
        think,
        stream: true,
    };

    let response = provider
        .client()
        .post(format!("{}/api/chat", provider.base_url()))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    // Process the NDJSON streaming response
    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut buffer = String::new();

    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| ProviderError::Network(e.to_string()))?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete lines from buffer (NDJSON format)
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            // Skip empty lines
            if line.is_empty() {
                continue;
            }

            // Parse the JSON line
            if let Ok(response) = serde_json::from_str::<StreamingResponse>(&line) {
                // Handle content delta
                if !response.message.content.is_empty() {
                    content.push_str(&response.message.content);
                    on_delta(StreamDelta::Content(response.message.content.clone()));
                }

                // Handle tool calls - they typically come all at once in Ollama.
                // The base is captured before the loop: the push below grows
                // tool_calls while `i` advances, so `tool_calls.len() + i`
                // would double-count within a multi-call frame.
                if let Some(tcs) = response.message.tool_calls {
                    let base = tool_calls.len();
                    for (i, tc) in tcs.iter().enumerate() {
                        let tool_call = convert_tool_call(tc, base + i);

                        // Emit tool call start
                        on_delta(StreamDelta::ToolCallStart {
                            index: base + i,
                            id: tool_call.id.clone(),
                            name: tc.function.name.clone(),
                        });

                        // Emit tool call arguments
                        let args =
                            serde_json::to_string(&tc.function.arguments).unwrap_or_default();
                        on_delta(StreamDelta::ToolCallArguments {
                            index: base + i,
                            arguments: args,
                        });

                        tool_calls.push(tool_call);
                    }
                }

                // Check if done
                if response.done {
                    on_delta(StreamDelta::Done {
                        finish_reason: Some("stop".to_string()),
                    });
                    break;
                }
            }
        }
    }

    Ok(CompletionResponse {
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        finish_reason: None,
        native_finish_reason: None,
        completion_tokens: None,
        upstream_provider: None,
        generation_id: None,
    })
}

#[cfg(test)]
mod tests {
    //! Ollama's streaming parser, against the framing Ollama actually
    //! produces.
    //!
    //! Nothing here is shared with the two OpenAI-shaped providers. The
    //! stream is newline-delimited JSON rather than SSE frames; there is no
    //! `[DONE]` sentinel, just a final object with `done: true`; a tool call
    //! arrives complete in one frame with its arguments as a **JSON object**
    //! rather than dribbled out as a string; and the wire carries neither an
    //! id nor an index, so both are synthesized here. Every one of those is a
    //! place the agent loop's assumptions could quietly not hold.

    use std::sync::{Arc, Mutex};

    use atomic_test_support::MockAiServer;

    use super::*;
    use crate::providers::traits::StreamingLlmProvider;
    use crate::providers::types::GenerationParams;

    /// Drive one streaming completion against a scripted NDJSON body,
    /// returning the assembled response and every delta emitted, in order.
    async fn stream(script: &str) -> (CompletionResponse, Vec<StreamDelta>) {
        let mock = MockAiServer::start().await;
        mock.set_stream_script(Some(script));
        let provider = OllamaProvider::new(Some(mock.base_url()), Some(30));
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

    fn tool_starts(deltas: &[StreamDelta]) -> Vec<(usize, String)> {
        deltas
            .iter()
            .filter_map(|delta| match delta {
                StreamDelta::ToolCallStart { index, name, .. } => Some((*index, name.clone())),
                _ => None,
            })
            .collect()
    }

    /// Each NDJSON frame's content is forwarded on its own, and the closing
    /// `done` frame ends the stream. Empty content frames carry no delta —
    /// Ollama sends one as the terminator and an empty chunk on the wire is
    /// not something a UI should render.
    #[tokio::test]
    async fn ndjson_content_frames_are_forwarded_one_at_a_time() {
        let script = concat!(
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"Mock "},"done":false}"#,
            "\n",
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"answer"},"done":false}"#,
            "\n",
            r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#,
            "\n",
        );
        let (response, deltas) = stream(script).await;

        assert_eq!(response.content, "Mock answer");
        assert_eq!(content_deltas(&deltas), vec!["Mock ", "answer"]);
        assert!(
            matches!(deltas.last(), Some(StreamDelta::Done { .. })),
            "the `done` frame closes the stream: {deltas:?}"
        );
        assert!(response.tool_calls.is_none());
    }

    /// A tool call arrives whole, with **object**-valued arguments, and no
    /// id. The parser has to stringify the arguments (every consumer, from
    /// the agent loop to the stored transcript, reads them as a JSON string)
    /// and mint an id, since the loop keys tool results by one.
    #[tokio::test]
    async fn object_valued_arguments_are_stringified_and_an_id_is_minted() {
        let script = concat!(
            r#"{"model":"llama3.2","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"search_atoms","arguments":{"query":"pelicans","limit":5}}}]},"done":false}"#,
            "\n",
            r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#,
            "\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("a tool call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].get_name(), Some("search_atoms"));
        let arguments: serde_json::Value =
            serde_json::from_str(calls[0].get_arguments().expect("arguments"))
                .expect("arguments must be a JSON string the loop can parse");
        assert_eq!(
            arguments,
            serde_json::json!({ "query": "pelicans", "limit": 5 })
        );
        assert!(
            !calls[0].id.is_empty(),
            "an id has to be minted; the wire sends none"
        );
        assert_eq!(calls[0].call_type.as_deref(), Some("function"));

        assert_eq!(tool_starts(&deltas), vec![(0, "search_atoms".to_string())]);
    }

    /// Several tool calls in one turn arrive in order, complete, with
    /// distinct minted ids — ids that collided would make the loop's
    /// tool-result transcript ambiguous — and contiguous delta indices
    /// even when one frame carries more than one call.
    #[tokio::test]
    async fn several_tool_calls_arrive_in_order_with_distinct_ids() {
        let script = concat!(
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"first","arguments":{"a":1}}},{"function":{"name":"second","arguments":{"b":2}}}]},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"third","arguments":{"c":3}}}]},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":""},"done":true}"#,
            "\n",
        );
        let (response, deltas) = stream(script).await;

        let calls = response.tool_calls.expect("three tool calls");
        assert_eq!(calls.len(), 3);
        let names: Vec<_> = calls.iter().filter_map(|c| c.get_name()).collect();
        assert_eq!(names, vec!["first", "second", "third"]);

        let ids: std::collections::HashSet<_> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "minted ids must not collide: {calls:?}");

        // Every call is announced, in wire order, at contiguous indices —
        // including within the two-call first frame.
        assert_eq!(
            tool_starts(&deltas),
            vec![
                (0, "first".to_string()),
                (1, "second".to_string()),
                (2, "third".to_string()),
            ]
        );
    }

    /// A frame may carry prose and a tool call together; both are surfaced.
    #[tokio::test]
    async fn a_frame_carrying_both_prose_and_a_tool_call_surfaces_both() {
        let script = concat!(
            r#"{"message":{"role":"assistant","content":"Looking it up. ","tool_calls":[{"function":{"name":"search_atoms","arguments":{"query":"x"}}}]},"done":false}"#,
            "\n",
            r#"{"message":{"role":"assistant","content":""},"done":true}"#,
            "\n",
        );
        let (response, deltas) = stream(script).await;

        assert_eq!(response.content, "Looking it up. ");
        assert_eq!(content_deltas(&deltas), vec!["Looking it up. "]);
        assert_eq!(response.tool_calls.expect("a tool call").len(), 1);
    }

    /// Blank lines and frames the parser cannot read are skipped rather
    /// than failing the stream — the same tolerance the SSE parsers have.
    #[tokio::test]
    async fn blank_and_unreadable_lines_are_skipped() {
        let script = concat!(
            "\n",
            r#"{"message":{"role":"assistant","content":"kept"},"done":false}"#,
            "\n",
            "{not json at all}\n",
            "\n",
            r#"{"message":{"role":"assistant","content":""},"done":true}"#,
            "\n",
        );
        let (response, _deltas) = stream(script).await;
        assert_eq!(response.content, "kept");
    }
}
