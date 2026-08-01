import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ByokForm } from './ByokForm';
import * as apiModule from '../../lib/api';
import { ApiError } from '../../lib/api';
import { modelCatalogue } from '../../test/fixtures';

const catalogue = modelCatalogue();

describe('ByokForm', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('disables submit until the form is minimally valid', async () => {
    const user = userEvent.setup();
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={() => {}} />);

    const submit = screen.getByRole('button', { name: /save & validate/i });
    // No key yet → disabled.
    expect(submit).toBeDisabled();

    // A non-empty key on the default OpenRouter provider is enough.
    await user.type(screen.getByLabelText(/api key/i), 'sk-or-test');
    expect(submit).toBeEnabled();
  });

  it('requires a base URL and both models for the OpenAI-compatible provider', async () => {
    const user = userEvent.setup();
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={() => {}} />);

    // Switch to OpenAI-compatible (a radio in the segmented control).
    await user.click(screen.getByRole('radio', { name: /openai-compatible/i }));

    const submit = screen.getByRole('button', { name: /save & validate/i });
    await user.type(screen.getByLabelText(/api key/i), 'sk-test');
    // A key alone isn't enough for openai_compat — the base URL is required.
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText(/base url/i), 'https://endpoint.example/v1');
    // Still not enough: this provider has NO default model ids, so blank
    // fields would send an empty model id and come back as an opaque
    // "the provider rejected the request".
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText(/embedding model/i), 'text-embedding-3-small');
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText(/llm model/i), 'local-llama');
    expect(submit).toBeEnabled();
  });

  it('names the real default in the OpenRouter embedding picker', () => {
    // Regression: the field used to be free text with a hardcoded placeholder
    // that had drifted from the actual fallback, suggesting a model that would
    // have silently put new vectors in a different space from existing ones.
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={() => {}} />);

    const picker = screen.getByLabelText(/embedding model/i) as HTMLSelectElement;
    // Blank (the default) is selected, and the option says which model that is.
    expect(picker.value).toBe('');
    expect(picker.selectedOptions[0].textContent).toContain('Qwen3 Embedding 8B');
    // The LLM placeholder names its real fallback too.
    expect(screen.getByLabelText(/llm model/i)).toHaveAttribute(
      'placeholder',
      catalogue.default_llm_model,
    );
  });

  it('clears model fields when the provider changes', async () => {
    const user = userEvent.setup();
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={() => {}} />);

    // Pick a non-default OpenRouter embedding model, then switch providers.
    await user.selectOptions(
      screen.getByLabelText(/embedding model/i),
      'openai/text-embedding-3-small',
    );
    await user.type(screen.getByLabelText(/llm model/i), 'openai/gpt-5-mini');
    await user.click(screen.getByRole('radio', { name: /openai-compatible/i }));

    // Model ids are provider-namespaced: carrying them across would submit an
    // OpenRouter id to an OpenAI-compatible endpoint.
    expect((screen.getByLabelText(/embedding model/i) as HTMLInputElement).value).toBe('');
    expect((screen.getByLabelText(/llm model/i) as HTMLInputElement).value).toBe('');
  });

  it('omits blank model fields from the saved config', async () => {
    const user = userEvent.setup();
    const spy = vi.spyOn(apiModule, 'saveByokProvider').mockResolvedValue({
      status: 'saved',
      provider: 'openrouter',
      origin: 'user',
      reembed_warning: null,
    });
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={() => {}} />);

    await user.type(screen.getByLabelText(/api key/i), 'sk-or-test');
    await user.click(screen.getByRole('button', { name: /save & validate/i }));

    // Leaving the pickers alone must send an empty config — the server then
    // applies atomic-core's defaults, rather than us pinning a guess.
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].model_config).toEqual({});
  });

  it('surfaces the server validation error verbatim and stores nothing', async () => {
    const user = userEvent.setup();
    const providerMessage = 'HTTP 401: invalid api key for provider';
    const spy = vi
      .spyOn(apiModule, 'saveByokProvider')
      .mockRejectedValue(
        new ApiError(400, 'provider_validation_failed', providerMessage),
      );
    const onSaved = vi.fn();
    render(<ByokForm hasExistingKey={false} catalogue={catalogue} onSaved={onSaved} />);

    await user.type(screen.getByLabelText(/api key/i), 'sk-or-bad');
    await user.click(screen.getByRole('button', { name: /save & validate/i }));

    // The provider's message is shown verbatim.
    await waitFor(() => {
      expect(screen.getByText(providerMessage)).toBeInTheDocument();
    });
    expect(spy).toHaveBeenCalledTimes(1);
    // Nothing "stored": onSaved (the success path) never fired.
    expect(onSaved).not.toHaveBeenCalled();
  });

  it('never renders a key value — the input is a password and clears on success', async () => {
    const user = userEvent.setup();
    vi.spyOn(apiModule, 'saveByokProvider').mockResolvedValue({
      status: 'saved',
      provider: 'openrouter',
      origin: 'user',
      reembed_warning: null,
    });
    render(<ByokForm hasExistingKey catalogue={catalogue} onSaved={() => {}} />);

    const keyInput = screen.getByLabelText(/new api key/i) as HTMLInputElement;
    // The key field masks by default (never shows the secret as text).
    expect(keyInput.type).toBe('password');

    await user.type(keyInput, 'sk-or-secret-value');
    await user.click(screen.getByRole('button', { name: /replace key/i }));

    // On success the typed key is cleared from the field (not retained or
    // echoed back anywhere).
    await waitFor(() => {
      expect((screen.getByLabelText(/new api key/i) as HTMLInputElement).value).toBe('');
    });
    // And the secret never appears as visible text anywhere in the document.
    expect(screen.queryByText('sk-or-secret-value')).not.toBeInTheDocument();
  });

  it('re-enables the form after a successful save (not stuck submitting)', async () => {
    const user = userEvent.setup();
    vi.spyOn(apiModule, 'saveByokProvider').mockResolvedValue({
      status: 'saved',
      provider: 'openrouter',
      origin: 'user',
      reembed_warning: null,
    });
    // The parent reloads status in place (it bumps a nonce — this same instance
    // stays mounted), so a save that left `submitting` set would brick the form.
    render(<ByokForm hasExistingKey catalogue={catalogue} onSaved={() => {}} />);

    const submit = () => screen.getByRole('button', { name: /replace key|validating/i });
    await user.type(screen.getByLabelText(/new api key/i), 'sk-or-secret-value');
    await user.click(submit());

    // After the save resolves the button must return to its idle label and be
    // usable again — never frozen in the 'Validating…' / disabled state.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /replace key/i })).toBeInTheDocument();
    });
    const button = screen.getByRole('button', { name: /replace key/i });
    expect(button).not.toHaveAttribute('aria-busy', 'true');
    // The key field is cleared on success, so submit is (correctly) disabled
    // for being empty — re-typing a key must re-enable it, proving the form
    // isn't disabled by a stuck `submitting` flag.
    expect(screen.queryByRole('button', { name: /validating/i })).not.toBeInTheDocument();
    await user.type(screen.getByLabelText(/new api key/i), 'sk-or-another');
    expect(screen.getByRole('button', { name: /replace key/i })).toBeEnabled();
    // And every field is interactive again (not disabled by `submitting`).
    expect(screen.getByLabelText(/new api key/i)).toBeEnabled();
  });
});
