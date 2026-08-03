import { useState, useMemo, useRef, useEffect } from 'react';
import { X, Plus } from 'lucide-react';
import { ConversationWithTags, ScopeMode, scopeModeOf, useChatStore } from '../../stores/chat';
import { useTagsStore } from '../../stores/tags';

interface ScopeEditorProps {
  conversation: ConversationWithTags;
}

// Clicking a chip walks this cycle; "Add tag" enters at the front.
const MODE_CYCLE: ScopeMode[] = ['include', 'require', 'exclude'];

const MODE_STYLE: Record<ScopeMode, { chip: string; name: string; label: string; hint: string }> = {
  include: {
    chip: 'bg-[var(--color-accent)]/20 text-[var(--color-accent-light)]',
    name: '',
    label: '',
    hint: 'Included — a result may carry any included tag. Click to require it.',
  },
  require: {
    chip: 'bg-[var(--color-accent)]/10 text-[var(--color-accent-light)] border border-[var(--color-accent)]',
    name: '',
    label: 'and',
    hint: 'Required — every result must carry this tag. Click to exclude it.',
  },
  exclude: {
    chip: 'bg-red-500/10 text-red-300 border border-red-500/30',
    name: 'line-through decoration-red-400/60',
    label: '',
    hint: 'Excluded — no result may carry this tag. Click to include it again.',
  },
};

function nextMode(mode: ScopeMode): ScopeMode {
  return MODE_CYCLE[(MODE_CYCLE.indexOf(mode) + 1) % MODE_CYCLE.length];
}

// Fuzzy matching algorithm
function fuzzyMatch(text: string, query: string): { score: number; matchIndices: number[] } | null {
  const textLower = text.toLowerCase();
  const queryLower = query.toLowerCase();

  if (!queryLower) return { score: 1, matchIndices: [] };

  const matchIndices: number[] = [];
  let queryIndex = 0;
  let score = 0;
  let consecutiveMatches = 0;
  let lastMatchIndex = -1;

  for (let i = 0; i < textLower.length && queryIndex < queryLower.length; i++) {
    if (textLower[i] === queryLower[queryIndex]) {
      matchIndices.push(i);

      if (lastMatchIndex === i - 1) {
        consecutiveMatches++;
        score += consecutiveMatches * 2;
      } else {
        consecutiveMatches = 0;
      }

      if (i === 0) score += 10;
      if (i > 0 && /[\s\-_]/.test(text[i - 1])) score += 5;
      score += 1;

      lastMatchIndex = i;
      queryIndex++;
    }
  }

  if (queryIndex !== queryLower.length) return null;

  score += Math.max(0, 20 - text.length);
  if (textLower === queryLower) score += 50;
  if (textLower.startsWith(queryLower)) score += 25;

  return { score, matchIndices };
}

// Component to highlight matched characters
function HighlightedText({ text, matchIndices }: { text: string; matchIndices: number[] }) {
  if (matchIndices.length === 0) {
    return <>{text}</>;
  }

  const matchSet = new Set(matchIndices);
  const parts: JSX.Element[] = [];

  for (let i = 0; i < text.length; i++) {
    if (matchSet.has(i)) {
      parts.push(
        <span key={i} className="text-[var(--color-accent-light)] font-semibold">
          {text[i]}
        </span>
      );
    } else {
      parts.push(<span key={i}>{text[i]}</span>);
    }
  }

  return <>{parts}</>;
}

interface FlatTag {
  id: string;
  name: string;
  depth: number;
}

export function ScopeEditor({ conversation }: ScopeEditorProps) {
  const [isAdding, setIsAdding] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const addTagToScope = useChatStore(s => s.addTagToScope);
  const removeTagFromScope = useChatStore(s => s.removeTagFromScope);
  const allTags = useTagsStore(s => s.tags);
  const inputRef = useRef<HTMLInputElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Focus input when opening
  useEffect(() => {
    if (isAdding && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isAdding]);

  // Close dropdown on click outside
  useEffect(() => {
    if (!isAdding) return;

    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsAdding(false);
        setSearchQuery('');
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isAdding]);

  // Flatten tag tree for selection
  const flattenTags = (tags: typeof allTags): FlatTag[] => {
    const result: FlatTag[] = [];
    const traverse = (tags: typeof allTags, depth: number) => {
      for (const tag of tags) {
        result.push({ id: tag.id, name: tag.name, depth });
        if (tag.children?.length > 0) {
          traverse(tag.children, depth + 1);
        }
      }
    };
    traverse(tags, 0);
    return result;
  };

  const allFlatTags = flattenTags(allTags);
  const scopeTagIds = new Set(conversation.tags.map((t) => t.id));

  // Fuzzy filter and sort available tags
  const filteredTags = useMemo(() => {
    const available = allFlatTags.filter((tag) => !scopeTagIds.has(tag.id));

    if (!searchQuery.trim()) {
      // No search - return all available tags (limited)
      return available.slice(0, 10).map((tag) => ({ tag, matchIndices: [] as number[] }));
    }

    const matches: { tag: FlatTag; score: number; matchIndices: number[] }[] = [];

    for (const tag of available) {
      const match = fuzzyMatch(tag.name, searchQuery);
      if (match) {
        matches.push({ tag, score: match.score, matchIndices: match.matchIndices });
      }
    }

    matches.sort((a, b) => b.score - a.score);
    return matches.slice(0, 10);
  }, [allFlatTags, scopeTagIds, searchQuery]);

  const handleAddTag = async (tagId: string) => {
    await addTagToScope(tagId, 'include');
    setIsAdding(false);
    setSearchQuery('');
  };

  const handleRemoveTag = async (tagId: string) => {
    await removeTagFromScope(tagId);
  };

  // Same call as adding: the tag is already in scope, so this rewrites its mode.
  const handleCycleMode = async (tagId: string, mode: ScopeMode) => {
    await addTagToScope(tagId, nextMode(mode));
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      setIsAdding(false);
      setSearchQuery('');
    } else if (e.key === 'Enter' && filteredTags.length > 0) {
      e.preventDefault();
      handleAddTag(filteredTags[0].tag.id);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="text-xs text-[var(--color-text-tertiary)] uppercase tracking-wide">Scope:</span>

      {/* Without an included tag the base set is everything, and the
          remaining chips narrow it — say so instead of leaving it implied. */}
      {!conversation.tags.some((tag) => scopeModeOf(tag) === 'include') && (
        <span className="text-sm text-[var(--color-text-secondary)] italic">All atoms</span>
      )}

      {conversation.tags.map((tag) => {
        const mode = scopeModeOf(tag);
        const style = MODE_STYLE[mode];
        return (
          <span
            key={tag.id}
            className={`group inline-flex items-center gap-1 px-2 py-0.5 text-sm rounded ${style.chip}`}
          >
            {style.label && (
              <span className="text-[9px] uppercase tracking-wide opacity-70">{style.label}</span>
            )}
            <button
              onClick={() => handleCycleMode(tag.id, mode)}
              title={style.hint}
              className={style.name}
              aria-label={`${tag.name}: ${mode}. Change how this tag scopes the conversation`}
            >
              {tag.name}
            </button>
            <button
              onClick={() => handleRemoveTag(tag.id)}
              className="opacity-0 group-hover:opacity-100 hover:text-red-400 transition-all"
              aria-label={`Remove ${tag.name} from scope`}
            >
              <X className="w-3 h-3" strokeWidth={2} />
            </button>
          </span>
        );
      })}

      {/* Add tag button/dropdown */}
      {isAdding ? (
        <div ref={dropdownRef} className="relative">
          <input
            ref={inputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Search tags..."
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="w-40 bg-[var(--color-bg-main)] border border-[var(--color-accent)] rounded px-2 py-1 text-sm text-[var(--color-text-primary)] focus:outline-none placeholder-[var(--color-text-tertiary)]"
          />

          {/* Dropdown */}
          {filteredTags.length > 0 && (
            <div className="absolute z-50 w-56 mt-1 bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-md shadow-lg max-h-48 overflow-y-auto">
              {filteredTags.map(({ tag, matchIndices }) => (
                <button
                  key={tag.id}
                  onClick={() => handleAddTag(tag.id)}
                  className="w-full px-3 py-2 text-left text-sm text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors"
                >
                  <HighlightedText text={tag.name} matchIndices={matchIndices} />
                </button>
              ))}
            </div>
          )}

          {searchQuery && filteredTags.length === 0 && (
            <div className="absolute z-50 w-56 mt-1 bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-md shadow-lg px-3 py-2 text-sm text-[var(--color-text-secondary)]">
              No matching tags
            </div>
          )}
        </div>
      ) : (
        <button
          onClick={() => setIsAdding(true)}
          className="inline-flex items-center gap-1 px-2 py-0.5 text-sm rounded border border-dashed border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent-light)] transition-colors"
        >
          <Plus className="w-3 h-3" strokeWidth={2} />
          Add tag
        </button>
      )}
    </div>
  );
}
