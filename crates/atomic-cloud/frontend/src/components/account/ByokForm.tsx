import { useId, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { SegmentedControl } from '../ui/SegmentedControl';
import { Field } from '../ui/Field';
import { Select } from '../ui/Select';
import { PasswordField } from '../ui/PasswordField';
import { Button } from '../ui/Button';
import { Banner } from '../ui/Banner';
import { ApiError, saveByokProvider } from '../../lib/api';
import type {
  ByokModelConfig,
  ByokProvider,
  ModelCatalogue,
  ProviderWriteResult,
} from '../../lib/api';

interface ByokFormProps {
  /** Whether a key is already stored for this BYOK provider (rotation vs first
   * save). Affects copy only — the stored key is never shown either way. */
  hasExistingKey: boolean;
  /** The server's model catalogue — drives the pickers and, crucially, names
   * the *actual* defaults a blank field falls back to. */
  catalogue: ModelCatalogue;
  /** Called with the server's success result (carries any re-embed warning) so
   * the parent can refresh status and surface the warning. */
  onSaved: (result: ProviderWriteResult) => void;
}

/**
 * The bring-your-own-key entry/rotation form. Mirrors the product app's
 * `AIProviderStep` structure — provider choice, key, model config — against the
 * cloud `PUT /api/account/provider` route, re-themed to the website's light
 * palette.
 *
 * Two contracts this form upholds:
 *
 * - **The stored key is never shown.** This form only ever holds a *new* key
 *   the user is typing; it has no field that could render an existing secret.
 *   Rotation is just submitting a new key.
 * - **Validation is server-side.** The key is verified against the provider
 *   before anything is stored; on failure the server's message is surfaced
 *   verbatim and nothing is saved. Submit is disabled until the form is
 *   minimally valid (a non-empty key, and a base URL for OpenAI-compatible),
 *   so the obvious mistakes never round-trip.
 *
 * # Why the model fields differ by provider
 *
 * **OpenRouter's are optional and picked from a list.** Blank means
 * atomic-core's defaults, which the catalogue names explicitly — the option
 * reads "Provider default — Qwen3 Embedding 8B" rather than leaving the user to
 * guess. The embedding options are pre-filtered server-side to the pinned
 * vector width, so an unservable choice is not merely rejected on save, it is
 * unofferable.
 *
 * **OpenAI-compatible's are required.** That provider has *no* default model
 * ids (`ProviderConfig` leaves both empty), so a blank field is not "use the
 * default" — it sends an empty model id to the endpoint and validation comes
 * back as an opaque "the provider rejected the request". Requiring them turns
 * that dead end into an obvious unfilled field.
 *
 * The LLM field stays free-form for both, offering suggestions via a
 * `datalist`: BYOK spends the user's own money and the server deliberately does
 * not curate their model choice, so a hard dropdown would remove a freedom the
 * product intends to grant.
 */
export function ByokForm({ hasExistingKey, catalogue, onSaved }: ByokFormProps) {
  const [provider, setProvider] = useState<ByokProvider>('openrouter');
  const [apiKey, setApiKey] = useState('');
  const [embeddingModel, setEmbeddingModel] = useState('');
  const [llmModel, setLlmModel] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const llmSuggestionsId = useId();

  const isOpenRouter = provider === 'openrouter';

  /** The catalogue's human name for the id a blank OpenRouter field falls back
   * to, so the "use the default" option can say which model that is. */
  const defaultEmbeddingLabel = useMemo(() => {
    const match = catalogue.openrouter_embedding_models.find(
      (m) => m.id === catalogue.default_embedding_model,
    );
    return match?.name ?? catalogue.default_embedding_model;
  }, [catalogue]);

  const keyOk = apiKey.trim().length > 0;
  // OpenAI-compatible needs a base URL to function (the server errors without
  // one); OpenRouter has a sensible default.
  const baseUrlOk = isOpenRouter || baseUrl.trim().length > 0;
  // OpenAI-compatible has no default model ids, so blanks would save a config
  // that cannot embed or generate (doc comment above).
  const modelsOk = isOpenRouter || (embeddingModel.trim().length > 0 && llmModel.trim().length > 0);
  const valid = keyOk && baseUrlOk && modelsOk && !submitting;

  const modelConfig = useMemo<ByokModelConfig>(() => {
    const config: ByokModelConfig = {};
    if (embeddingModel.trim()) config.embedding_model = embeddingModel.trim();
    if (llmModel.trim()) config.llm_model = llmModel.trim();
    if (provider === 'openai_compat' && baseUrl.trim()) {
      config.openai_compat_base_url = baseUrl.trim();
    }
    if (provider === 'openrouter' && baseUrl.trim()) {
      config.openrouter_base_url = baseUrl.trim();
    }
    return config;
  }, [embeddingModel, llmModel, baseUrl, provider]);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!valid) return;
    setError(null);
    setSubmitting(true);
    try {
      const result = await saveByokProvider({
        provider,
        api_key: apiKey.trim(),
        model_config: modelConfig,
      });
      // Clear the typed key from memory on success.
      setApiKey('');
      onSaved(result);
    } catch (err) {
      if (err instanceof ApiError) {
        // The provider's validation error is surfaced verbatim (the server
        // scrubs the key from it before sending); a dimension mismatch carries
        // its own structured message.
        setError(err.message);
      } else {
        setError('Something went wrong saving your provider. Please try again.');
      }
    } finally {
      // Always re-enable the form. The parent reloads status in place (it bumps
      // a nonce — this same instance stays mounted), so without this the button
      // would be stuck spinning and every field disabled after a *successful*
      // save. The parent surfaces the "Saved" affordance; here we just return
      // to an idle, usable state.
      setSubmitting(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} noValidate className="space-y-5">
      <SegmentedControl<ByokProvider>
        label="Provider"
        value={provider}
        onChange={(v) => {
          setProvider(v);
          setError(null);
          // Model ids are provider-namespaced, so a value carried across the
          // switch is always wrong — and worse than wrong for the embedding
          // field, whose OpenRouter form is a <select>: a held-over free-text
          // id isn't among the options, so the control would render blank
          // while still submitting the stale value.
          setEmbeddingModel('');
          setLlmModel('');
        }}
        disabled={submitting}
        segments={[
          {
            value: 'openrouter',
            label: 'OpenRouter',
            description: 'One key, hundreds of models. The simplest option.',
          },
          {
            value: 'openai_compat',
            label: 'OpenAI-compatible',
            description: 'Any OpenAI-style endpoint — local models, gateways.',
          },
        ]}
      />

      {error && (
        <Banner tone="error" title="Couldn’t save your provider">
          {error}
        </Banner>
      )}

      {!isOpenRouter && (
        <Field
          label="Base URL"
          type="url"
          inputMode="url"
          placeholder="https://your-endpoint.example/v1"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          disabled={submitting}
          required
          help="The OpenAI-compatible API endpoint your key authenticates against."
        />
      )}

      <PasswordField
        label={hasExistingKey ? 'New API key' : 'API key'}
        placeholder={isOpenRouter ? 'sk-or-…' : 'sk-…'}
        value={apiKey}
        onChange={(e) => {
          setApiKey(e.target.value);
          if (error) setError(null);
        }}
        disabled={submitting}
        required
        help={
          hasExistingKey
            ? 'Entering a new key replaces the stored one. The current key is never shown.'
            : isOpenRouter
              ? 'Get a key at openrouter.ai/keys. Validated before it’s stored.'
              : 'Validated against your endpoint before it’s stored.'
        }
      />

      <div className="rounded-xl border border-border-light bg-bg-secondary/50 p-4">
        <p className="text-sm font-medium text-text-primary">
          {isOpenRouter ? 'Models (optional)' : 'Models'}
        </p>
        <p className="mt-0.5 text-xs text-text-muted">
          {isOpenRouter ? (
            <>
              Leave either blank to use Atomic’s default. Embedding choices are
              limited to models producing {catalogue.embedding_dimension}-dimensional
              vectors — the width your knowledge base is stored at.
            </>
          ) : (
            <>
              Your endpoint has no defaults, so name the models it serves. The
              embedding model must produce {catalogue.embedding_dimension}-dimensional
              vectors — any other width is rejected.
            </>
          )}
        </p>
        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          {isOpenRouter ? (
            <Select
              label="Embedding model"
              value={embeddingModel}
              onChange={(e) => setEmbeddingModel(e.target.value)}
              disabled={submitting}
              options={[
                { value: '', label: `Atomic’s default — ${defaultEmbeddingLabel}` },
                // The default is already the first option; listing it again
                // under its own name would be two entries doing one thing.
                ...catalogue.openrouter_embedding_models
                  .filter((model) => model.id !== catalogue.default_embedding_model)
                  .map((model) => ({ value: model.id, label: model.name })),
              ]}
              // Unconditional: a cloud account always arrives here with an
              // existing corpus (it starts on the managed key), so switching
              // away from the default strands those vectors in another space
              // whether or not a BYOK key is already stored.
              help="Changing this degrades search until existing atoms are re-embedded."
            />
          ) : (
            <Field
              label="Embedding model"
              placeholder="text-embedding-3-small"
              value={embeddingModel}
              onChange={(e) => setEmbeddingModel(e.target.value)}
              disabled={submitting}
              required
            />
          )}
          <Field
            label="LLM model"
            list={llmSuggestionsId}
            placeholder={isOpenRouter ? catalogue.default_llm_model : 'your-llm'}
            value={llmModel}
            onChange={(e) => setLlmModel(e.target.value)}
            disabled={submitting}
            required={!isOpenRouter}
            help="Powers wiki synthesis, chat, and reports. Any model your key can reach."
          />
          {/* Suggestions, not a whitelist — the server does not curate BYOK
              model choice, so anything typed is accepted. */}
          <datalist id={llmSuggestionsId}>
            {catalogue.suggested_llm_models.map((id) => (
              <option key={id} value={id} />
            ))}
          </datalist>
        </div>
        {isOpenRouter && (
          <p className="mt-3 text-xs text-text-muted">
            Auto-tagging runs on{' '}
            <span className="font-mono">{catalogue.default_tagging_model}</span> — a
            fast, low-cost utility model, not configurable here.
          </p>
        )}
      </div>

      <div className="flex items-center gap-3">
        <Button type="submit" disabled={!valid} loading={submitting}>
          {submitting ? 'Validating…' : hasExistingKey ? 'Replace key' : 'Save & validate'}
        </Button>
        <p className="text-sm text-text-muted">
          We’ll verify the key with the provider before storing it.
        </p>
      </div>
    </form>
  );
}
