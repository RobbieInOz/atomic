//! Curated registry of OpenRouter text embedding models.
//!
//! OpenRouter exposes a `/api/v1/embeddings/models` endpoint that lists available
//! embedding models, but the schema does not include vector dimensions — those only
//! appear in free-text descriptions. Since Atomic's SQLite-vec index is fixed at
//! creation time, we need authoritative dimensions per model, so we hand-curate
//! the list here.
//!
//! Adding a new model is a one-line change: append to `EMBEDDING_MODELS`.

use serde::{Deserialize, Serialize};

/// Metadata for an OpenRouter text embedding model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterEmbeddingModel {
    /// OpenRouter model ID (e.g. "openai/text-embedding-3-small")
    pub id: &'static str,
    /// Human-readable display name
    pub name: &'static str,
    /// Native output vector dimension (what the API returns by default)
    pub dimension: usize,
    /// Maximum input context length in tokens
    pub context_length: usize,
}

/// Curated list of OpenRouter text embedding models with verified dimensions.
///
/// `dimension` is the width Atomic requests and stores for the model:
/// `embed_batch` passes it as the `dimensions` parameter and enforces it
/// client-side (truncate + renormalize) for MRL models, so this value is the
/// authoritative vector width the schema is created at. For MRL-capable models
/// it may be smaller than the native maximum (e.g. Qwen3-Embedding-8B is native
/// 4096 but we store 1536).
///
/// **Ids are unique.** [`get_embedding_dimension`] resolves by first match, and
/// the list is rendered verbatim into model pickers, so a duplicate id would
/// both show the model twice and let a picker advertise a width the lookup
/// disagrees with. Enforced by `ids_are_unique` below.
pub const EMBEDDING_MODELS: &[OpenRouterEmbeddingModel] = &[
    // Qwen (default — top open-weight MTEB, cheapest, self-hostable).
    // Native 4096; we store 1536 (Matryoshka) to match the existing schema.
    OpenRouterEmbeddingModel {
        id: "qwen/qwen3-embedding-8b",
        name: "Qwen3 Embedding 8B",
        dimension: 1536,
        context_length: 32768,
    },
    // OpenAI
    OpenRouterEmbeddingModel {
        id: "openai/text-embedding-3-small",
        name: "OpenAI: text-embedding-3-small",
        dimension: 1536,
        context_length: 8192,
    },
    OpenRouterEmbeddingModel {
        id: "openai/text-embedding-3-large",
        name: "OpenAI: text-embedding-3-large",
        dimension: 3072,
        context_length: 8192,
    },
    OpenRouterEmbeddingModel {
        id: "openai/text-embedding-ada-002",
        name: "OpenAI: text-embedding-ada-002",
        dimension: 1536,
        context_length: 8192,
    },
    // Google
    OpenRouterEmbeddingModel {
        id: "google/gemini-embedding-001",
        name: "Google: Gemini Embedding 001",
        dimension: 3072,
        context_length: 20000,
    },
    OpenRouterEmbeddingModel {
        id: "google/gemini-embedding-2-preview",
        name: "Google: Gemini Embedding 2 Preview",
        dimension: 3072,
        context_length: 8192,
    },
    // Mistral
    OpenRouterEmbeddingModel {
        id: "mistralai/mistral-embed-2312",
        name: "Mistral: Mistral Embed 2312",
        dimension: 1024,
        context_length: 8192,
    },
    // Qwen (the 8B entry lives at the top of the list — it is the default).
    OpenRouterEmbeddingModel {
        id: "qwen/qwen3-embedding-4b",
        name: "Qwen: Qwen3 Embedding 4B",
        dimension: 2560,
        context_length: 32768,
    },
    // BAAI
    OpenRouterEmbeddingModel {
        id: "baai/bge-m3",
        name: "BAAI: bge-m3 (multilingual)",
        dimension: 1024,
        context_length: 8192,
    },
    OpenRouterEmbeddingModel {
        id: "baai/bge-large-en-v1.5",
        name: "BAAI: bge-large-en-v1.5",
        dimension: 1024,
        context_length: 512,
    },
    OpenRouterEmbeddingModel {
        id: "baai/bge-base-en-v1.5",
        name: "BAAI: bge-base-en-v1.5",
        dimension: 768,
        context_length: 512,
    },
    // Intfloat E5
    OpenRouterEmbeddingModel {
        id: "intfloat/multilingual-e5-large",
        name: "Intfloat: Multilingual E5 Large",
        dimension: 1024,
        context_length: 512,
    },
    OpenRouterEmbeddingModel {
        id: "intfloat/e5-large-v2",
        name: "Intfloat: E5 Large v2",
        dimension: 1024,
        context_length: 512,
    },
    OpenRouterEmbeddingModel {
        id: "intfloat/e5-base-v2",
        name: "Intfloat: E5 Base v2",
        dimension: 768,
        context_length: 512,
    },
    // Perplexity
    OpenRouterEmbeddingModel {
        id: "perplexity/pplx-embed-v1-4b",
        name: "Perplexity: Embed V1 4B",
        dimension: 2560,
        context_length: 32000,
    },
    // NVIDIA
    OpenRouterEmbeddingModel {
        id: "nvidia/llama-nemotron-embed-vl-1b-v2:free",
        name: "NVIDIA: Llama Nemotron Embed VL 1B V2 (free)",
        dimension: 2048,
        context_length: 131072,
    },
    // Thenlper GTE
    OpenRouterEmbeddingModel {
        id: "thenlper/gte-large",
        name: "Thenlper: GTE Large",
        dimension: 1024,
        context_length: 512,
    },
    OpenRouterEmbeddingModel {
        id: "thenlper/gte-base",
        name: "Thenlper: GTE Base",
        dimension: 768,
        context_length: 512,
    },
];

/// Get the full list of curated embedding models.
pub fn get_embedding_models() -> &'static [OpenRouterEmbeddingModel] {
    EMBEDDING_MODELS
}

/// Look up the native vector dimension for a given OpenRouter embedding model ID.
/// Returns `None` if the model is not in the curated registry.
pub fn get_embedding_dimension(model_id: &str) -> Option<usize> {
    EMBEDDING_MODELS
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.dimension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A duplicate id is a silent trap: [`get_embedding_dimension`] resolves by
    /// first match while [`get_embedding_models`] hands the whole list to model
    /// pickers, so a second entry for the same id renders the model twice and
    /// can advertise a width the lookup will never return. (This regressed
    /// once: `qwen/qwen3-embedding-8b` was listed at both 1536 and 4096.)
    #[test]
    fn ids_are_unique() {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for model in EMBEDDING_MODELS {
            if let Some(previous) = seen.insert(model.id, model.dimension) {
                panic!(
                    "duplicate embedding model id {:?} (dimensions {previous} and {}); \
                     the lookup would resolve only the first",
                    model.id, model.dimension
                );
            }
        }
    }

    /// The default the rest of the stack assumes must be present at exactly the
    /// width the vector schema is created at — a drift here would wedge every
    /// deployment that never set an explicit model.
    #[test]
    fn default_model_is_registered_at_the_default_dimension() {
        assert_eq!(
            get_embedding_dimension(crate::providers::DEFAULT_EMBEDDING_MODEL),
            Some(crate::providers::DEFAULT_EMBEDDING_DIMENSION),
        );
    }
}
