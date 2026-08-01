import type { AccountOverview, ModelCatalogue } from '../lib/api';

/**
 * The provider status's model catalogue, shaped like the server's
 * `byok_model_catalogue`. The embedding list is already width-filtered by the
 * server, so the fixture carries only valid entries.
 */
export function modelCatalogue(patch: Partial<ModelCatalogue> = {}): ModelCatalogue {
  return {
    openrouter_embedding_models: [
      { id: 'qwen/qwen3-embedding-8b', name: 'Qwen3 Embedding 8B', context_length: 32768 },
      {
        id: 'openai/text-embedding-3-small',
        name: 'OpenAI: text-embedding-3-small',
        context_length: 8192,
      },
    ],
    embedding_dimension: 1536,
    default_embedding_model: 'qwen/qwen3-embedding-8b',
    default_llm_model: 'anthropic/claude-sonnet-5',
    default_tagging_model: 'openai/gpt-5-nano',
    suggested_llm_models: ['openai/gpt-5-mini', 'anthropic/claude-sonnet-5'],
    ...patch,
  };
}

/**
 * A ready account overview for tests, with sane defaults. Pass a partial to
 * override any field (e.g. `overview({ billing_state: 'read_only' })`).
 */
export function overview(patch: Partial<AccountOverview> = {}): AccountOverview {
  return {
    subdomain: 'alpha',
    email: 'alpha@example.com',
    plan: { id: 'pro', name: 'Pro' },
    billing_state: 'active',
    billing_configured: true,
    trial_ends_at: null,
    usage: {
      atoms_used: 3,
      atom_limit: null,
      kb_count: 1,
      kb_limit: null,
      ai_credits_monthly_cents: 50,
    },
    provider: {
      configured: true,
      origin: 'managed',
      provider: 'openrouter',
      model_config: { embedding_model: 'openai/text-embedding-3-small' },
      last_validated_at: null,
      last_validation_error: null,
    },
    mcp_url: 'https://alpha.atomic.cloud/mcp',
    ...patch,
  };
}
