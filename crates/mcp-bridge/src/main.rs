//! Atomic MCP Bridge
//!
//! A stdio-to-Streamable-HTTP bridge that allows MCP clients (like Claude Desktop)
//! to communicate with Atomic's HTTP-based MCP server.
//!
//! This implements the MCP Streamable HTTP transport protocol, converting between
//! stdio JSON-RPC and HTTP with proper session management.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const DEFAULT_PORT: u16 = 44380;
const DEFAULT_HOST: &str = "127.0.0.1";
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const TOKEN_FILE_NAME: &str = "local_server_token";

/// Resolve the platform-specific Atomic data directory.
/// Must match Tauri's `app.path().app_data_dir()` for identifier `com.atomic.app`.
fn atomic_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("com.atomic.app"))
}

/// Resolve the server's MCP endpoint. `ATOMIC_URL` (a full base URL such as
/// `https://you.atomicapp.ai`) wins so the bridge can front remote HTTPS
/// servers; otherwise compose the local-sidecar form from
/// `ATOMIC_HOST`/`ATOMIC_PORT`.
fn resolve_endpoint() -> String {
    if let Ok(url) = env::var("ATOMIC_URL") {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/mcp") {
            return trimmed.to_string();
        }
        return format!("{}/mcp", trimmed);
    }
    let port: u16 = env::var("ATOMIC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let host = env::var("ATOMIC_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    format!("http://{}:{}/mcp", host, port)
}

/// Discover the auth token: ATOMIC_TOKEN env var, then local token file on disk.
fn discover_token() -> Option<String> {
    if let Ok(token) = env::var("ATOMIC_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    let data_dir = atomic_data_dir()?;
    let token_path = data_dir.join(TOKEN_FILE_NAME);
    match std::fs::read_to_string(&token_path) {
        Ok(token) => {
            let token = token.trim().to_string();
            if token.is_empty() {
                None
            } else {
                tracing::info!(path = ?token_path, "Read auth token from disk");
                Some(token)
            }
        }
        Err(_) => {
            tracing::warn!(path = ?token_path, "Could not read token file");
            None
        }
    }
}

/// JSON-RPC error response
#[derive(Serialize)]
struct JsonRpcError {
    jsonrpc: &'static str,
    id: serde_json::Value,
    error: JsonRpcErrorData,
}

#[derive(Serialize)]
struct JsonRpcErrorData {
    code: i32,
    message: String,
}

impl JsonRpcError {
    fn internal_error(id: serde_json::Value, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            error: JsonRpcErrorData {
                code: -32603,
                message,
            },
        }
    }
}

/// Minimal JSON-RPC request/notification parsing
#[derive(Deserialize)]
struct JsonRpcMessage {
    id: Option<serde_json::Value>,
    method: Option<String>,
}

/// Send a line to stdout with proper flushing
fn send_stdout(line: &str) {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    writeln!(handle, "{}", line).ok();
    handle.flush().ok();
}

/// Send an error response to stdout
fn send_error(id: serde_json::Value, message: String) {
    let error = JsonRpcError::internal_error(id, message);
    if let Ok(json) = serde_json::to_string(&error) {
        send_stdout(&json);
    }
}

/// Process SSE stream and output JSON-RPC messages to stdout
async fn process_sse_stream(response: reqwest::Response, request_id: serde_json::Value) {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                buffer.push_str(&text);

                // Process complete SSE events (double newline separated)
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_block = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Parse SSE event
                    let mut data_lines: Vec<&str> = Vec::new();

                    for line in event_block.lines() {
                        if line.starts_with("data:") {
                            let data = line.strip_prefix("data:").unwrap().trim();
                            if !data.is_empty() {
                                data_lines.push(data);
                            }
                        }
                        // Ignore event:, id:, retry: lines for now
                    }

                    // Combine data lines and output
                    if !data_lines.is_empty() {
                        let data = data_lines.join("\n");
                        if data != "[DONE]" {
                            send_stdout(&data);
                        }
                    }
                }
            }
            Err(e) => {
                send_error(request_id, format!("Stream error: {}", e));
                return;
            }
        }
    }

    // Process any remaining buffer content
    if !buffer.trim().is_empty() {
        for line in buffer.lines() {
            if line.starts_with("data:") {
                let data = line.strip_prefix("data:").unwrap().trim();
                if !data.is_empty() && data != "[DONE]" {
                    send_stdout(data);
                }
            }
        }
    }
}

/// Process a single JSON-RPC message
async fn process_message(
    client: &reqwest::Client,
    endpoint: &str,
    session_id: Arc<Mutex<Option<String>>>,
    auth_token: Option<&str>,
    line: String,
) {
    // Parse the message to get ID and method
    let msg = serde_json::from_str::<JsonRpcMessage>(&line).ok();
    let request_id = msg
        .as_ref()
        .and_then(|m| m.id.clone())
        .unwrap_or(serde_json::Value::Null);
    let method = msg.as_ref().and_then(|m| m.method.clone());
    let is_notification = msg.as_ref().map(|m| m.id.is_none()).unwrap_or(false);

    // Build request with appropriate headers
    let mut request = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Protocol-Version", MCP_PROTOCOL_VERSION);

    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    // Add session ID if we have one (not for initialize request)
    let current_session = session_id.lock().unwrap().clone();
    if let Some(sid) = &current_session {
        request = request.header("Mcp-Session-Id", sid);
    }

    // Send the request
    let response = match request.body(line.clone()).send().await {
        Ok(resp) => resp,
        Err(e) => {
            if !is_notification {
                send_error(request_id, format!("HTTP request failed: {}", e));
            }
            return;
        }
    };

    // Check for session ID in response headers (from initialize response)
    if method.as_deref() == Some("initialize") {
        if let Some(new_session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            tracing::debug!(session_id = %new_session_id, "Captured session ID");
            *session_id.lock().unwrap() = Some(new_session_id.to_string());
        }
    }

    // Check response status
    let status = response.status();

    // 202 Accepted is valid for notifications
    if status == reqwest::StatusCode::ACCEPTED {
        return;
    }

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if !is_notification {
            send_error(request_id, format!("HTTP {} - {}", status, body));
        }
        return;
    }

    // Check content type to determine how to handle response
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("text/event-stream") {
        // Handle SSE streaming response
        process_sse_stream(response, request_id).await;
    } else {
        // Handle regular JSON response
        match response.text().await {
            Ok(body) if !body.trim().is_empty() => {
                send_stdout(&body);
            }
            Ok(_) => {
                // Empty response is ok for some requests
            }
            Err(e) => {
                if !is_notification {
                    send_error(request_id, format!("Failed to read response: {}", e));
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "atomic_mcp_bridge=info,warn".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Parse configuration from environment
    let endpoint = resolve_endpoint();

    let auth_token = discover_token();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        endpoint = %endpoint,
        has_token = auth_token.is_some(),
        protocol_version = MCP_PROTOCOL_VERSION,
        "Atomic MCP Bridge starting"
    );

    // Session ID storage (assigned by server during initialize)
    let session_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Create HTTP client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 5 min timeout for long operations
        .build()
        .expect("Failed to create HTTP client");

    // Create channel for stdin lines
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Spawn stdin reader thread (blocking IO must be on separate thread)
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let handle = stdin.lock();

        for line in handle.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => continue, // Skip empty lines
                Err(_) => break,
            }
        }
    });

    // Process messages from stdin
    while let Some(line) = rx.recv().await {
        process_message(
            &client,
            &endpoint,
            session_id.clone(),
            auth_token.as_deref(),
            line,
        )
        .await;
    }
}
