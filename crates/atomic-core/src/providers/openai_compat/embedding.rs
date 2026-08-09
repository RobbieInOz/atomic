//! OpenAI-compatible embedding implementation

use crate::providers::error::ProviderError;
use crate::providers::openai_compat::OpenAICompatProvider;
use crate::providers::traits::EmbeddingConfig;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// Generate embeddings via an OpenAI-compatible API
pub async fn embed_batch(
    provider: &OpenAICompatProvider,
    texts: &[String],
    config: &EmbeddingConfig,
) -> Result<Vec<Vec<f32>>, ProviderError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let request = EmbeddingRequest {
        model: config.model.clone(),
        input: texts.to_vec(),
    };

    let mut req = provider
        .client()
        .post(format!("{}/embeddings", provider.base_url()))
        .header("Content-Type", "application/json");

    if let Some(api_key) = provider.api_key() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.json(&request).send().await?;

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
            tracing::warn!(status, retry_after, model = %config.model, body_preview = %crate::providers::error::body_for_log(&body, 200), "OpenAI-compat embedding rate limited");
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        tracing::error!(status, model = %config.model, request_id = trace_id.as_deref().unwrap_or("-"), body_preview = %crate::providers::error::body_for_log(&body, 500), "OpenAI-compat embedding API error");
        return Err(ProviderError::Api {
            status,
            message: body,
        });
    }

    let body = response.text().await?;

    let embedding_response: EmbeddingResponse = serde_json::from_str(&body)
        .map_err(|e| {
            tracing::error!(error = %e, model = %config.model, request_id = trace_id.as_deref().unwrap_or("-"), body_preview = %crate::providers::error::body_for_log(&body, 500), "OpenAI-compat embedding response decode failed");
            crate::providers::error::decode_error("embedding response", &body, &e, trace_id.as_deref())
        })?;

    Ok(embedding_response
        .data
        .into_iter()
        .map(|d| d.embedding)
        .collect())
}
