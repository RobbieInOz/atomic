//! End-to-end smoke test for the MCP HTTP endpoint.
//!
//! Validates the gap that mcp_auth.rs's unit tests can't reach: the McpAuth
//! middleware wrapped around the real AtomicMcpTransport scope, mounted in
//! a live actix server, reachable over HTTP. We exercise the auth gate and
//! a minimal protocol round-trip (`initialize`) so a regression in either
//! the auth wiring or the transport scope surfaces here.
//!
//! Deeper MCP protocol semantics (tool dispatch, session lifecycle,
//! cancellation) belong in the rmcp crate's own suite; this file owns the
//! "does our server expose MCP at all" contract — plus the *tool surface*
//! contract (names, titles, read-only/destructive annotations), which the
//! docs and the connectors-directory listing depend on.

mod support;

use serde_json::{json, Value};
use support::{spawn_live_server, Backend, TestCtx};

#[actix_web::test]
async fn mcp_rejects_missing_auth_sqlite() {
    run_mcp_rejects_missing_auth(Backend::Sqlite).await;
}

#[actix_web::test]
async fn mcp_rejects_missing_auth_postgres() {
    if std::env::var("ATOMIC_TEST_DATABASE_URL").is_err() {
        eprintln!("mcp_rejects_missing_auth_postgres: skipping (ATOMIC_TEST_DATABASE_URL not set)");
        return;
    }
    run_mcp_rejects_missing_auth(Backend::Postgres).await;
}

async fn run_mcp_rejects_missing_auth(backend: Backend) {
    let Some(ctx) = TestCtx::new(backend).await else {
        return;
    };
    let server = spawn_live_server(&ctx).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/mcp", server.base_url))
        .json(&initialize_request())
        .send()
        .await
        .expect("POST /mcp without auth");
    assert_eq!(resp.status(), 401, "missing Bearer should yield 401");
    let www_authenticate = resp
        .headers()
        .get("WWW-Authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        www_authenticate.starts_with("Bearer "),
        "WWW-Authenticate header should carry Bearer challenge; got {www_authenticate:?}"
    );

    server.stop().await;
}

#[actix_web::test]
async fn mcp_rejects_wrong_token_sqlite() {
    run_mcp_rejects_wrong_token(Backend::Sqlite).await;
}

#[actix_web::test]
async fn mcp_rejects_wrong_token_postgres() {
    if std::env::var("ATOMIC_TEST_DATABASE_URL").is_err() {
        eprintln!("mcp_rejects_wrong_token_postgres: skipping (ATOMIC_TEST_DATABASE_URL not set)");
        return;
    }
    run_mcp_rejects_wrong_token(Backend::Postgres).await;
}

async fn run_mcp_rejects_wrong_token(backend: Backend) {
    let Some(ctx) = TestCtx::new(backend).await else {
        return;
    };
    let server = spawn_live_server(&ctx).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/mcp", server.base_url))
        .bearer_auth("not-a-real-token")
        .json(&initialize_request())
        .send()
        .await
        .expect("POST /mcp with wrong token");
    assert_eq!(resp.status(), 401, "unknown token should be 401");

    server.stop().await;
}

#[actix_web::test]
async fn mcp_initialize_round_trip_sqlite() {
    run_mcp_initialize_round_trip(Backend::Sqlite).await;
}

#[actix_web::test]
async fn mcp_initialize_round_trip_postgres() {
    if std::env::var("ATOMIC_TEST_DATABASE_URL").is_err() {
        eprintln!(
            "mcp_initialize_round_trip_postgres: skipping (ATOMIC_TEST_DATABASE_URL not set)"
        );
        return;
    }
    run_mcp_initialize_round_trip(Backend::Postgres).await;
}

async fn run_mcp_initialize_round_trip(backend: Backend) {
    let Some(ctx) = TestCtx::new(backend).await else {
        return;
    };
    let server = spawn_live_server(&ctx).await;

    let client = reqwest::Client::new();
    // Streamable HTTP transport will pick JSON or SSE based on Accept. We
    // ask for both so the server can choose whichever shape it prefers for
    // `initialize` — both prove the route is reachable through McpAuth.
    let resp = client
        .post(format!("{}/mcp", server.base_url))
        .bearer_auth(&ctx.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize_request())
        .send()
        .await
        .expect("POST /mcp initialize");

    assert!(
        resp.status().is_success(),
        "MCP initialize should succeed; got {} (body: {})",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // Streamable HTTP returns either application/json or text/event-stream.
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.text().await.expect("read body");

    let result_present = if content_type.starts_with("application/json") {
        let parsed: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("invalid JSON body: {e}\nbody = {body}"));
        parsed.get("result").is_some() || parsed.get("error").is_some()
    } else {
        // SSE framing: look for a `data:` line carrying a JSON-RPC payload.
        body.lines().any(|line| {
            line.strip_prefix("data: ").is_some_and(|payload| {
                serde_json::from_str::<Value>(payload)
                    .ok()
                    .and_then(|v| {
                        if v.get("result").is_some() || v.get("error").is_some() {
                            Some(())
                        } else {
                            None
                        }
                    })
                    .is_some()
            })
        })
    };
    assert!(
        result_present,
        "MCP initialize response should carry a JSON-RPC result or error; \
         content-type = {content_type:?}, body = {body}"
    );

    server.stop().await;
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "atomic-e2e", "version": "0.0.0" }
        }
    })
}

/// Extract the JSON-RPC `result` object from a Streamable HTTP response body,
/// which may be plain JSON or SSE-framed (`data: {...}` lines).
fn extract_jsonrpc_result(content_type: &str, body: &str) -> Value {
    let payloads: Vec<Value> = if content_type.starts_with("application/json") {
        vec![serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("invalid JSON body: {e}\nbody = {body}"))]
    } else {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
            .collect()
    };
    payloads
        .into_iter()
        .find_map(|v| v.get("result").cloned())
        .unwrap_or_else(|| panic!("no JSON-RPC result in response; body = {body}"))
}

/// The tool surface is a public contract: the docs (README, manual guide) and
/// the connectors-directory listing enumerate these names, and clients gate
/// write confirmation on the annotations. Storage-independent, so SQLite only.
#[actix_web::test]
async fn mcp_tools_list_pins_tool_surface_sqlite() {
    const READ_ONLY: &[&str] = &[
        "semantic_search",
        "read_atom",
        "find_similar",
        "list_tags",
        "list_atoms",
        "list_databases",
        "list_wikis",
        "get_wiki",
        "list_reports",
        "get_report_findings",
        "search",
        "fetch",
    ];
    const WRITE: &[&str] = &["create_atom", "ingest_url", "update_atom", "edit_atom"];

    let Some(ctx) = TestCtx::new(Backend::Sqlite).await else {
        return;
    };
    let server = spawn_live_server(&ctx).await;
    let client = reqwest::Client::new();
    let mcp_url = format!("{}/mcp", server.base_url);

    let resp = client
        .post(&mcp_url)
        .bearer_auth(&ctx.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize_request())
        .send()
        .await
        .expect("POST /mcp initialize");
    assert!(resp.status().is_success(), "initialize: {}", resp.status());
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("initialize response carries Mcp-Session-Id")
        .to_string();

    let resp = client
        .post(&mcp_url)
        .bearer_auth(&ctx.token)
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .expect("POST /mcp initialized notification");
    assert!(
        resp.status().is_success(),
        "initialized notification: {}",
        resp.status()
    );

    let resp = client
        .post(&mcp_url)
        .bearer_auth(&ctx.token)
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
        .send()
        .await
        .expect("POST /mcp tools/list");
    assert!(resp.status().is_success(), "tools/list: {}", resp.status());
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.text().await.expect("read tools/list body");
    let result = extract_jsonrpc_result(&content_type, &body);

    let tools = result
        .get("tools")
        .and_then(|t| t.as_array())
        .expect("tools/list result carries a tools array");
    let by_name: std::collections::HashMap<&str, &Value> = tools
        .iter()
        .map(|t| {
            (
                t.get("name")
                    .and_then(|n| n.as_str())
                    .expect("tool has a name"),
                t,
            )
        })
        .collect();
    assert_eq!(by_name.len(), tools.len(), "duplicate tool names");

    for name in READ_ONLY.iter().chain(WRITE) {
        let tool = by_name
            .get(name)
            .unwrap_or_else(|| panic!("tools/list missing {name}"));
        let annotations = tool
            .get("annotations")
            .unwrap_or_else(|| panic!("{name} missing annotations"));
        assert!(
            annotations
                .get("title")
                .and_then(|t| t.as_str())
                .is_some_and(|t| !t.is_empty()),
            "{name} missing annotation title"
        );
    }
    for name in READ_ONLY {
        assert_eq!(
            by_name[name]["annotations"]["readOnlyHint"],
            json!(true),
            "{name} should be annotated read-only"
        );
    }
    for name in WRITE {
        assert_eq!(
            by_name[name]["annotations"]["readOnlyHint"],
            json!(false),
            "{name} should be annotated as a write tool"
        );
        assert!(
            by_name[name]["annotations"]
                .get("destructiveHint")
                .is_some(),
            "{name} must declare destructiveHint"
        );
    }
    assert_eq!(
        tools.len(),
        READ_ONLY.len() + WRITE.len(),
        "tool surface changed — update this test, the README tool list, and \
         docs/manual/guides/mcp-server.md"
    );

    server.stop().await;
}
