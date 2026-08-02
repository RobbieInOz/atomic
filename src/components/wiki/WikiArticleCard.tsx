import { WikiArticleSummary } from '../../stores/wiki';
import { formatRelativeDate, formatShortRelativeDate } from '../../lib/date';

interface WikiArticleCardProps {
  article: WikiArticleSummary;
  isActive?: boolean;
  onClick: (opts?: { newTab?: boolean }) => void;
}

/// One article row in the sidebar's article list. Sized for the 250px panel,
/// so the meta line stays on one line: short relative date, source count, and
/// inbound links only when there are any.
export function WikiArticleCard({ article, isActive, onClick }: WikiArticleCardProps) {
  return (
    <div
      onClick={(e) => onClick({ newTab: e.metaKey || e.ctrlKey })}
      onAuxClick={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          onClick({ newTab: true });
        }
      }}
      className={`px-3 py-2 cursor-pointer transition-colors ${
        isActive
          ? 'bg-[var(--color-accent)]/20'
          : 'hover:bg-[var(--color-bg-card)]'
      }`}
    >
      <div className="flex items-baseline gap-2">
        <span
          className="flex-1 min-w-0 truncate text-sm font-medium text-[var(--color-text-primary)]"
          title={article.tag_name}
        >
          {article.tag_name}
        </span>
        <span
          className="shrink-0 text-[11px] text-[var(--color-text-tertiary)]"
          title={formatRelativeDate(article.updated_at)}
        >
          {formatShortRelativeDate(article.updated_at)}
        </span>
      </div>

      <div className="flex items-center gap-2 mt-0.5 text-[11px] text-[var(--color-text-tertiary)]">
        <span>
          {article.atom_count} source{article.atom_count !== 1 ? 's' : ''}
        </span>
        {article.inbound_links > 0 && (
          <span className="text-[var(--color-accent-light)]">
            {article.inbound_links} link{article.inbound_links !== 1 ? 's' : ''}
          </span>
        )}
      </div>
    </div>
  );
}
