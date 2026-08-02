import { useEffect, useRef } from 'react';
import type { TocItem } from './useTocItems';

interface TocPanelProps {
  items: TocItem[];
  activeId: string | null;
  onItemClick: (item: TocItem) => void;
  /** Shown in place of the list when there are no headings. */
  emptyLabel?: string;
  /** Optional section label above the list. */
  title?: string;
}

/** Base inset, matching the tag tree's rows. */
const BASE_INDENT = 8;
const INDENT_PER_LEVEL = 12;

/**
 * Table of contents for the left sidebar. Purely presentational — the owner
 * supplies the items, the active id, and what a click means.
 */
export function TocPanel({ items, activeId, onItemClick, emptyLabel, title }: TocPanelProps) {
  const activeRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: 'nearest' });
  }, [activeId]);

  // Notes rarely start at `#`, and a wiki article's body is all `##`. Indent
  // from the shallowest heading present so the outline hugs the left edge
  // whatever level the document happens to use.
  const minDepth = items.length > 0 ? Math.min(...items.map((item) => item.depth)) : 1;

  return (
    <div className="flex flex-col h-full">
      {title && (
        <div className="px-3 py-2 shrink-0">
          <span className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider">
            {title}
          </span>
        </div>
      )}

      {items.length === 0 ? (
        <div className="px-3 py-4">
          <p className="text-xs text-[var(--color-text-secondary)] leading-relaxed">
            {emptyLabel ?? 'No headings'}
          </p>
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto scrollbar-hidden px-1 pb-2">
          {items.map((item) => {
            const isActive = item.id === activeId;
            return (
              <button
                key={item.id}
                ref={isActive ? activeRef : undefined}
                type="button"
                onClick={() => onItemClick(item)}
                title={item.text}
                style={{ paddingLeft: `${BASE_INDENT + (item.depth - minDepth) * INDENT_PER_LEVEL}px` }}
                className={`w-full flex items-center pr-2 py-1.5 rounded-md text-left text-sm transition-colors ${
                  item.depth === minDepth ? 'font-medium' : ''
                } ${
                  isActive
                    ? 'bg-[var(--color-accent)]/20 text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-card)] hover:text-[var(--color-text-primary)]'
                }`}
              >
                <span className="truncate">{item.text}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
