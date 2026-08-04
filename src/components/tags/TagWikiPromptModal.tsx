import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, Loader2 } from 'lucide-react';
import { Modal } from '../ui/Modal';
import { useTagsStore, Tag } from '../../stores/tags';
import { useSettingsStore } from '../../stores/settings';

interface TagWikiPromptModalProps {
  isOpen: boolean;
  /// The tag whose prompts are being edited; null while the modal is closed.
  tag: Pick<Tag, 'id' | 'name'> | null;
  onClose: () => void;
}

const TEXTAREA_CLASS = `
  px-3 py-2 rounded-md text-sm font-mono leading-relaxed resize-y
  bg-[var(--color-bg-main)] border border-[var(--color-border)]
  text-[var(--color-text-primary)] placeholder:text-[var(--color-text-secondary)]/40
  focus:outline-none focus:ring-1 focus:ring-[var(--color-accent)]
`;

const LABEL_CLASS = 'text-xs font-medium uppercase tracking-[0.1em] text-[var(--color-text-tertiary)]';

/// What an empty field falls back to. Settings → Prompts holds the global
/// prompts; when one of those is empty too, Atomic's built-in prompt runs.
function fallbackLabel(globalPrompt: string | undefined): string {
  return globalPrompt?.trim()
    ? 'the global prompt from Settings → Prompts'
    : "Atomic's built-in prompt";
}

export function TagWikiPromptModal({ isOpen, tag, onClose }: TagWikiPromptModalProps) {
  const fetchTagWikiPrompts = useTagsStore(s => s.fetchTagWikiPrompts);
  const saveTagWikiPrompts = useTagsStore(s => s.saveTagWikiPrompts);
  const settings = useSettingsStore(s => s.settings);

  const [generationPrompt, setGenerationPrompt] = useState('');
  const [updatePrompt, setUpdatePrompt] = useState('');
  const [showUpdatePrompt, setShowUpdatePrompt] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tagId = tag?.id ?? null;

  // Prompts aren't carried on the tag tree, so they are read fresh every
  // time the modal opens.
  useEffect(() => {
    if (!isOpen || !tagId) return;
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    fetchTagWikiPrompts(tagId)
      .then((prompts) => {
        if (cancelled) return;
        setGenerationPrompt(prompts.generation_prompt ?? '');
        setUpdatePrompt(prompts.update_prompt ?? '');
        // Unfold the secondary field when this tag already overrides it,
        // so an existing value is never hidden behind a disclosure.
        setShowUpdatePrompt(Boolean(prompts.update_prompt?.trim()));
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, tagId, fetchTagWikiPrompts]);

  const handleSave = async () => {
    if (!tagId || isLoading || isSaving) return;
    setIsSaving(true);
    setError(null);
    try {
      // Blank fields go over as-is; the server reads them as "clear this
      // override".
      await saveTagWikiPrompts(tagId, {
        generation_prompt: generationPrompt,
        update_prompt: updatePrompt,
      });
      onClose();
    } catch (e) {
      // Keep the modal open so the user doesn't lose what they typed.
      setError(String(e));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={tag ? `Wiki Prompt for "${tag.name}"` : 'Wiki Prompt'}
      width="lg"
      confirmLabel={isSaving ? 'Saving…' : 'Save prompts'}
      onConfirm={handleSave}
      confirmDisabled={isLoading || isSaving}
    >
      {isLoading ? (
        <div className="flex items-center justify-center gap-2 py-10 text-sm text-[var(--color-text-secondary)]">
          <Loader2 className="w-4 h-4 animate-spin" strokeWidth={2} />
          Loading prompts…
        </div>
      ) : (
        <div className="flex flex-col gap-5">
          {/* Generation prompt — the reason anyone opens this modal. */}
          <div className="flex flex-col gap-1.5">
            <label className={LABEL_CLASS}>Generation prompt</label>
            <p className="text-xs text-[var(--color-text-secondary)]">
              Replaces the prompt used to write this tag's article. Leave empty to use{' '}
              {fallbackLabel(settings.wiki_generation_prompt)}.
            </p>
            <textarea
              value={generationPrompt}
              onChange={(e) => setGenerationPrompt(e.target.value)}
              placeholder="e.g. Collect every unchecked task from these notes into one checklist, grouped by project. Skip anything already done."
              rows={10}
              className={TEXTAREA_CLASS}
              autoFocus
            />
          </div>

          {/* Update prompt — secondary, folded away until it's in play. */}
          <div className="flex flex-col gap-2">
            <button
              type="button"
              onClick={() => setShowUpdatePrompt(s => !s)}
              className={`self-start flex items-center gap-1.5 ${LABEL_CLASS} hover:text-[var(--color-text-primary)] transition-colors`}
            >
              {showUpdatePrompt ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
              Update prompt
            </button>
            {showUpdatePrompt && (
              <div className="flex flex-col gap-1.5 pl-3 border-l border-[var(--color-border)]">
                <p className="text-xs text-[var(--color-text-secondary)]">
                  Added to the prompt when new atoms are folded into the existing article. Leave empty to
                  use {fallbackLabel(settings.wiki_update_prompt)}.
                </p>
                <textarea
                  value={updatePrompt}
                  onChange={(e) => setUpdatePrompt(e.target.value)}
                  placeholder="e.g. Keep the checklist grouped by project, and drop tasks that are now done."
                  rows={4}
                  className={TEXTAREA_CLASS}
                />
              </div>
            )}
          </div>

          {error && <p className="text-xs text-red-400">{error}</p>}

          <p className="text-[11px] text-[var(--color-text-tertiary)]">
            Saving doesn't rewrite anything. These prompts apply the next time this article is generated or
            updated.
          </p>
        </div>
      )}
    </Modal>
  );
}
