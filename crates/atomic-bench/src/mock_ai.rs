use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const EMBED_DIM: usize = 1536;

pub struct MockAiServer {
    server: MockServer,
    counters: Arc<MockAiCounters>,
}

#[derive(Default)]
struct MockAiCounters {
    embedding_requests: AtomicUsize,
    chat_requests: AtomicUsize,
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

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ChatResponder {
                counters: counters.clone(),
            })
            .mount(&server)
            .await;

        Self { server, counters }
    }

    pub fn base_url(&self) -> String {
        self.server.uri()
    }

    pub fn embedding_request_count(&self) -> usize {
        self.counters.embedding_requests.load(Ordering::Relaxed)
    }

    pub fn chat_request_count(&self) -> usize {
        self.counters.chat_requests.load(Ordering::Relaxed)
    }
}

fn embed_text(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; EMBED_DIM];
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
        let idx = (h.finish() as usize) % EMBED_DIM;
        vec[idx] += 1.0;
    }

    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    } else {
        vec[0] = 1.0;
    }
    vec
}

struct EmbedResponder {
    counters: Arc<MockAiCounters>,
}

impl Respond for EmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.counters
            .embedding_requests
            .fetch_add(1, Ordering::Relaxed);

        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(value) => value,
            Err(_) => return ResponseTemplate::new(400),
        };

        let Some(inputs) = body.get("input").and_then(|v| v.as_array()) else {
            return ResponseTemplate::new(400);
        };

        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(index, text)| {
                json!({
                    "object": "embedding",
                    "index": index,
                    "embedding": embed_text(text.as_str().unwrap_or_default()),
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
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(value) => value,
            Err(_) => return ResponseTemplate::new(400),
        };

        let request_text = body.to_string().to_lowercase();
        // Generative callers keep the schema off the wire (see
        // `GENERATIVE_CALL_ENFORCEMENT`) and state it in the prompt instead, so
        // fall back to sniffing it — otherwise extraction silently benchmarks
        // an empty `{}` response.
        let schema_name = body
            .pointer("/response_format/json_schema/name")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                if request_text.contains("parent_name") {
                    "extraction_result"
                } else {
                    ""
                }
            });

        let content = match schema_name {
            "extraction_result" => json!({
                "tags": [
                    {
                        "name": infer_tag(&request_text),
                        "parent_name": "Topics"
                    }
                ]
            })
            .to_string(),
            _ => "{}".to_string(),
        };

        ResponseTemplate::new(200).set_body_json(json!({
            "id": "atomic-bench-mock",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": content
                    },
                    "finish_reason": "stop"
                }
            ]
        }))
    }
}

fn infer_tag(text: &str) -> &'static str {
    if text.contains("pasta") || text.contains("cooking") || text.contains("dough") {
        "Cooking"
    } else if text.contains("tomato") || text.contains("garden") || text.contains("seedling") {
        "Gardening"
    } else {
        "Physics"
    }
}
