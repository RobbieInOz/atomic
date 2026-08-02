import { useMemo } from 'react';
import { Eye, EyeOff, X } from 'lucide-react';
import { useChatStore } from '../../stores/chat';
import { useUIStore } from '../../stores/ui';
import { useAtomsStore } from '../../stores/atoms';
import { useTagsStore, TagWithCount } from '../../stores/tags';
import { isDemoInstance } from '../../lib/transport';

function findTagName(nodes: TagWithCount[], tagId: string): string | null {
  for (const node of nodes) {
    if (node.id === tagId) return node.name;
    const found = node.children?.length ? findTagName(node.children, tagId) : null;
    if (found) return found;
  }
  return null;
}

/**
 * What `buildPageContext()` in the chat store will attach to the next
 * message, in the same precedence order it uses. `null` means the current
 * view carries nothing worth announcing.
 */
function usePageContextLabel(): string | null {
  const readerAtomId = useUIStore((s) => s.readerState.atomId);
  const wikiTagName = useUIStore((s) => s.wikiReaderState.tagName);
  const graphAtomId = useUIStore((s) => (s.localGraph.isOpen ? s.localGraph.centerAtomId : null));
  const selectedTagId = useUIStore((s) => s.selectedTagId);
  const atoms = useAtomsStore((s) => s.atoms);
  const tags = useTagsStore((s) => s.tags);

  const atomId = readerAtomId ?? (wikiTagName ? null : graphAtomId);

  return useMemo(() => {
    if (atomId) {
      // The atom may not be in the loaded page (opened from a citation), so
      // name it generically rather than guessing.
      return atoms.find((a) => a.id === atomId)?.title || 'the open atom';
    }
    if (wikiTagName) return `the ${wikiTagName} wiki`;
    if (selectedTagId) {
      const name = findTagName(tags, selectedTagId);
      return name ? `the ${name} filter` : null;
    }
    return null;
  }, [atomId, atoms, wikiTagName, selectedTagId, tags]);
}

/**
 * One quiet row above the composer naming what leaves with the next message.
 * Dismissing it suppresses page context for this conversation (see the chat
 * store's `pageContextOff`), and the row stays put so the choice is
 * reversible.
 */
export function ChatContextChip({ conversationId }: { conversationId: string }) {
  const label = usePageContextLabel();
  const isOff = useChatStore((s) => Boolean(s.pageContextOff[conversationId]));
  const setPageContextEnabled = useChatStore((s) => s.setPageContextEnabled);

  if (isDemoInstance() || !label) return null;

  return (
    <div className="flex-shrink-0 px-3 pt-2 -mb-1">
      <div className="inline-flex items-center gap-1.5 max-w-full px-2 py-0.5 rounded text-xs bg-[var(--color-bg-card)] border border-[var(--color-border)] text-[var(--color-text-secondary)]">
        {isOff ? (
          <EyeOff className="w-3 h-3 flex-shrink-0" strokeWidth={2} />
        ) : (
          <Eye className="w-3 h-3 flex-shrink-0 text-[var(--color-accent-light)]" strokeWidth={2} />
        )}
        <span className="truncate">
          {isOff ? 'Not sharing' : 'Sharing'} {label}
        </span>
        {isOff ? (
          <button
            onClick={() => setPageContextEnabled(conversationId, true)}
            className="flex-shrink-0 text-[var(--color-text-tertiary)] hover:text-[var(--color-accent-light)] transition-colors"
            title="Share this page with the assistant again"
          >
            Share
          </button>
        ) : (
          <button
            onClick={() => setPageContextEnabled(conversationId, false)}
            className="flex-shrink-0 text-[var(--color-text-tertiary)] hover:text-red-400 transition-colors"
            aria-label="Stop sharing page context"
            title="Stop sharing page context"
          >
            <X className="w-3 h-3" strokeWidth={2} />
          </button>
        )}
      </div>
    </div>
  );
}
