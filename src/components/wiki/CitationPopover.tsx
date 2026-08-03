import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { ArrowRight, BookOpen, Telescope } from 'lucide-react';
import { useKeyboard } from '../../hooks/useKeyboard';

// Generic citation interface that works with both WikiCitation and ChatCitation
export interface CitationForPopover {
  citation_index: number;
  atom_id: string;
  excerpt: string;
  // Chat citations can point at a wiki article or a report finding instead
  // of an atom. Everything else cites atoms only and omits this.
  source_type?: 'atom' | 'wiki' | 'finding';
  source_title?: string | null;
}

// How each kind of source names itself. Atoms keep the original wording so
// the wiki, dashboard and finding readers are untouched by chat's sources.
const SOURCE_LABELS = {
  atom: { kind: 'Source excerpt', open: 'View full atom', Icon: null },
  wiki: { kind: 'Wiki excerpt', open: 'View wiki article', Icon: BookOpen },
  finding: { kind: 'Finding excerpt', open: 'View finding', Icon: Telescope },
} as const;

interface CitationPopoverProps {
  citation: CitationForPopover;
  anchorRect: { top: number; left: number; bottom: number; width: number } | null;
  onClose: () => void;
  onViewAtom: (atomId: string, highlightText?: string) => void;
}

// Calculate position based on anchor rect
function calculatePosition(
  anchorRect: { top: number; left: number; bottom: number; width: number },
  popoverHeight: number,
  popoverWidth: number
): { top: number; left: number } {
  // Calculate position - prefer below, but go above if not enough space
  const spaceBelow = window.innerHeight - anchorRect.bottom;
  const spaceAbove = anchorRect.top;

  let top: number;
  if (spaceBelow >= popoverHeight + 8 || spaceBelow >= spaceAbove) {
    // Position below
    top = anchorRect.bottom + 8;
  } else {
    // Position above
    top = anchorRect.top - popoverHeight - 8;
  }

  // Horizontal positioning - center on anchor, but keep within viewport
  let left = anchorRect.left + anchorRect.width / 2 - popoverWidth / 2;
  left = Math.max(8, Math.min(left, window.innerWidth - popoverWidth - 8));

  return { top, left };
}

export function CitationPopover({ citation, anchorRect, onClose, onViewAtom }: CitationPopoverProps) {
  const popoverRef = useRef<HTMLDivElement>(null);

  // Calculate initial position immediately (synchronously)
  const initialPosition = anchorRect ? calculatePosition(anchorRect, 180, 400) : null;
  const [position, setPosition] = useState<{ top: number; left: number } | null>(initialPosition);

  // Close on Escape
  useKeyboard('Escape', onClose, true);

  // Close on click outside
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(event.target as Node)) {
        onClose();
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [onClose]);

  // Update position when anchorRect changes
  useEffect(() => {
    if (!anchorRect) return;
    const pos = calculatePosition(anchorRect, 180, 400);
    setPosition(pos);
  }, [anchorRect]);

  // Refine position after render with actual dimensions
  useLayoutEffect(() => {
    if (!anchorRect || !popoverRef.current) return;

    const popoverRect = popoverRef.current.getBoundingClientRect();
    const refinedPos = calculatePosition(anchorRect, popoverRect.height, popoverRect.width);

    setPosition(refinedPos);
  }, [anchorRect]);

  const handleViewAtom = () => {
    onViewAtom(citation.atom_id, citation.excerpt);
    onClose();
  };

  const source = SOURCE_LABELS[citation.source_type ?? 'atom'];

  // Truncate excerpt if needed
  const displayExcerpt = citation.excerpt.length > 300
    ? citation.excerpt.slice(0, 297) + '...'
    : citation.excerpt;

  // Don't render until we have a position
  if (!position) {
    return null;
  }

  // Render in a portal to avoid transform containment issues.
  // data-modal="true" marks this as a modal surface so useClickOutside
  // on other overlays treats clicks inside as outside-of-self.
  return createPortal(
    <div
      ref={popoverRef}
      data-modal="true"
      className="fixed z-[100] w-[400px] max-w-[calc(100vw-16px)] bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg shadow-xl"
      style={{ top: position.top, left: position.left }}
    >
      {/* Citation number badge */}
      <div className="px-4 py-2 border-b border-[var(--color-border)] flex items-center gap-2">
        <span className="inline-flex items-center justify-center w-6 h-6 rounded bg-[var(--color-accent)]/20 text-[var(--color-accent-light)] text-xs font-medium">
          {citation.citation_index}
        </span>
        {source.Icon && (
          <source.Icon className="w-3.5 h-3.5 text-[var(--color-text-tertiary)]" strokeWidth={2} />
        )}
        <span className="text-xs text-[var(--color-text-secondary)]">
          {citation.source_title ? `${source.kind} — ${citation.source_title}` : source.kind}
        </span>
      </div>

      {/* Excerpt content */}
      <div className="px-4 py-3 prose prose-invert prose-sm max-w-none [&_h1]:text-sm [&_h2]:text-sm [&_h3]:text-sm [&_h4]:text-sm [&_h1]:m-0 [&_h2]:m-0 [&_h3]:m-0 [&_h4]:m-0 max-h-[200px] overflow-y-auto">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>
          {displayExcerpt}
        </ReactMarkdown>
      </div>

      {/* Footer with link */}
      <div className="px-4 py-2 border-t border-[var(--color-border)]">
        <button
          onClick={handleViewAtom}
          className="flex items-center gap-1 text-sm text-[var(--color-accent)] hover:text-[var(--color-accent-light)] transition-colors"
        >
          {source.open}
          <ArrowRight className="w-4 h-4" strokeWidth={2} />
        </button>
      </div>
    </div>,
    document.body
  );
}
