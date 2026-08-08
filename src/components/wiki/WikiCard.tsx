import { memo, useMemo, useState } from 'react';
import { Sparkles, BookOpen, Link2, MoreVertical } from 'lucide-react';
import { WikiArticleSummary, SuggestedArticle } from '../../stores/wiki';
import { formatRelativeDate, formatShortRelativeDate } from '../../lib/date';
import { ContextMenu } from '../ui/ContextMenu';

interface WikiArticleCardProps {
  type: 'article';
  article: WikiArticleSummary;
  onClick: (opts?: { newTab?: boolean }) => void;
  onOpenPrompts: () => void;
}

interface WikiSuggestionCardProps {
  type: 'suggestion';
  suggestion: SuggestedArticle;
  onClick: (opts?: { newTab?: boolean }) => void;
  onOpenPrompts: () => void;
}

type WikiCardProps = WikiArticleCardProps | WikiSuggestionCardProps;

/// The overflow (⋮) menu both card shapes carry, following ReportRow's idiom.
/// Suggestions get it as well as articles: the prompt that matters most is the
/// one set *before* an article is first generated.
///
/// Hidden until the card is hovered on pointer devices, always visible below
/// `md` where there is no hover to reveal it, and revealed by focus so it stays
/// keyboard-reachable — the same three-way visibility ConversationCard uses.
function CardActions({ onOpenPrompts }: { onOpenPrompts: () => void }) {
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);

  const items = useMemo(
    () => [
      {
        label: 'Wiki Prompt…',
        icon: <BookOpen className="w-4 h-4" strokeWidth={2} />,
        onClick: onOpenPrompts,
      },
    ],
    [onOpenPrompts]
  );

  return (
    <>
      <button
        type="button"
        // The card opens the article on click and middle-click; neither of
        // those gestures belongs to the button sitting inside it.
        onClick={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.getBoundingClientRect();
          // Anchor at the button's bottom-right corner; ContextMenu nudges
          // itself back into the viewport when it overflows.
          setMenuPos({ x: rect.right - 160, y: rect.bottom + 4 });
        }}
        onAuxClick={(e) => e.stopPropagation()}
        title="Article actions"
        aria-label="Article actions"
        /* Negative margins cancel the padding: the icon lines up with the
           card's content edge, and the taller hit area doesn't push the rest
           of the card down — which below `md` it would do permanently. */
        className="
          p-1 -my-1 -mr-1 rounded-md text-[var(--color-text-tertiary)]
          hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]
          transition-colors
          opacity-0 group-hover:opacity-100 focus:opacity-100 max-md:opacity-100
        "
      >
        <MoreVertical className="w-4 h-4" strokeWidth={2} />
      </button>
      {menuPos && (
        <ContextMenu
          items={items}
          position={menuPos}
          onClose={() => setMenuPos(null)}
          autoFocus
        />
      )}
    </>
  );
}

export const WikiCard = memo(function WikiCard(props: WikiCardProps) {
  if (props.type === 'suggestion') {
    const { suggestion, onClick, onOpenPrompts } = props;
    return (
      <div
        onClick={(e) => onClick({ newTab: e.metaKey || e.ctrlKey })}
        onAuxClick={(e) => {
          if (e.button === 1) {
            e.preventDefault();
            onClick({ newTab: true });
          }
        }}
        className="relative flex flex-col p-4 border border-dashed border-[var(--color-border)] rounded-lg cursor-pointer hover:border-[var(--color-accent)]/50 hover:bg-[var(--color-bg-card)]/50 transition-all duration-150 h-full min-w-0 overflow-hidden group"
      >
        <div className="flex-1 min-h-0">
          <div className="flex items-start justify-between gap-2">
            <p className="text-sm font-medium text-[var(--color-text-primary)] line-clamp-1">
              {suggestion.tag_name}
            </p>
            <div className="flex items-center gap-1 shrink-0">
              <Sparkles className="w-4 h-4 text-[var(--color-accent)] opacity-60 group-hover:opacity-100 transition-opacity" strokeWidth={2} />
              <CardActions onOpenPrompts={onOpenPrompts} />
            </div>
          </div>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-2 leading-relaxed">
            Generate a wiki article from {suggestion.atom_count} atom{suggestion.atom_count !== 1 ? 's' : ''}
            {suggestion.mention_count > 0 && (
              <> and {suggestion.mention_count} mention{suggestion.mention_count !== 1 ? 's' : ''}</>
            )}
          </p>
        </div>
        <div className="mt-3 pt-3 border-t border-dashed border-[var(--color-border)]">
          <span className="text-xs font-medium text-[var(--color-accent)] group-hover:text-[var(--color-accent-light)] transition-colors">
            Generate article
          </span>
        </div>
      </div>
    );
  }

  const { article, onClick, onOpenPrompts } = props;

  return (
    <div
      onClick={(e) => onClick({ newTab: e.metaKey || e.ctrlKey })}
      onAuxClick={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          onClick({ newTab: true });
        }
      }}
      className="group relative flex flex-col p-4 bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg cursor-pointer hover:border-[var(--color-border-hover)] hover:bg-[var(--color-bg-hover)] transition-all duration-150 h-full min-w-0 overflow-hidden break-words"
    >
      <div className="flex-1 min-h-0">
        <div className="flex items-baseline justify-between gap-2">
          <p className="text-sm font-medium text-[var(--color-text-primary)] line-clamp-1 min-w-0">
            {article.tag_name}
          </p>
          {/* The date leads the cluster so it, not the taller button, sets the
              row's baseline — the title stays aligned with it as before. */}
          <div className="flex items-center gap-1 shrink-0">
            <span className="text-xs text-[var(--color-text-tertiary)]" title={formatRelativeDate(article.updated_at)}>
              {formatShortRelativeDate(article.updated_at)}
            </span>
            <CardActions onOpenPrompts={onOpenPrompts} />
          </div>
        </div>
      </div>
      <div className="mt-3 pt-3 border-t border-[var(--color-border)]">
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-1 text-xs text-[var(--color-text-tertiary)]">
            <BookOpen className="w-3.5 h-3.5" strokeWidth={2} />
            {article.atom_count} source{article.atom_count !== 1 ? 's' : ''}
          </span>
          {article.inbound_links > 0 && (
            <span className="flex items-center gap-1 text-xs text-[var(--color-accent-light)]">
              <Link2 className="w-3.5 h-3.5" strokeWidth={2} />
              {article.inbound_links} link{article.inbound_links !== 1 ? 's' : ''}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}, (prev, next) => {
  if (prev.type !== next.type) return false;
  if (prev.type === 'article' && next.type === 'article') {
    return prev.article.id === next.article.id
      && prev.article.updated_at === next.article.updated_at
      && prev.article.atom_count === next.article.atom_count
      && prev.article.inbound_links === next.article.inbound_links;
  }
  if (prev.type === 'suggestion' && next.type === 'suggestion') {
    return prev.suggestion.tag_id === next.suggestion.tag_id
      && prev.suggestion.atom_count === next.suggestion.atom_count;
  }
  return false;
});
