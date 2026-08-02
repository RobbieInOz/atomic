import { useEffect, useRef, useState } from 'react';
import { Plus } from 'lucide-react';
import { useWikiStore } from '../../stores/wiki';
import { useUIStore } from '../../stores/ui';
import { WikiArticleCard } from './WikiArticleCard';
import { NewWikiModal } from './NewWikiModal';

/// The wiki view's sidebar: every article, plus the tags with enough material
/// to become one. Both kinds of row open a wiki reader tab — a suggestion just
/// lands on the reader's generate affordance instead of an article.
export function WikiArticlesList() {
  const articles = useWikiStore(s => s.articles);
  const suggestedArticles = useWikiStore(s => s.suggestedArticles);
  const isLoadingList = useWikiStore(s => s.isLoadingList);
  const error = useWikiStore(s => s.error);
  const fetchAllArticles = useWikiStore(s => s.fetchAllArticles);

  // Tabs-model successor to the wiki store's `currentTagId`. Opening an
  // article swaps this panel for that article's outline, so the highlight only
  // shows if the list is ever mounted alongside a reader.
  const activeTagId = useUIStore(s => s.wikiReaderState.tagId);
  const openWikiReader = useUIStore(s => s.openWikiReader);

  const [isModalOpen, setIsModalOpen] = useState(false);
  const initializedRef = useRef(false);

  // The panel loads its own data rather than relying on the wiki view being
  // mounted beside it; the store collapses the concurrent pair of fetches.
  useEffect(() => {
    if (initializedRef.current) return;
    initializedRef.current = true;
    fetchAllArticles();
  }, [fetchAllArticles]);

  const openArticle = (tagId: string, tagName: string, opts?: { newTab?: boolean }) => {
    openWikiReader(tagId, tagName, undefined, opts);
  };

  const isEmpty = articles.length === 0 && suggestedArticles.length === 0;

  return (
    <div className="h-full flex flex-col">
      <div className="flex-1 overflow-y-auto scrollbar-hidden">
        {error && (
          <p className="px-3 py-2 text-xs text-red-400 leading-relaxed">{error}</p>
        )}

        {isLoadingList && isEmpty ? (
          <div className="flex flex-col gap-1 px-3 py-2">
            {Array.from({ length: 5 }, (_, i) => (
              <div key={i} className="py-1.5">
                <div
                  className="h-3.5 rounded bg-[var(--color-bg-card)] animate-pulse"
                  style={{ width: `${70 + i * 15}px` }}
                />
              </div>
            ))}
          </div>
        ) : isEmpty ? (
          // A failure shown above already accounts for the empty panel — don't
          // also claim there is simply nothing here.
          error ? null : (
            <p className="px-3 py-4 text-xs text-[var(--color-text-secondary)] leading-relaxed">
              No wiki articles yet. Generate one from a tag to synthesize what your atoms say about it.
            </p>
          )
        ) : (
          <>
            {articles.map((article) => (
              <WikiArticleCard
                key={article.id}
                article={article}
                isActive={article.tag_id === activeTagId}
                onClick={(opts) => openArticle(article.tag_id, article.tag_name, opts)}
              />
            ))}

            {suggestedArticles.length > 0 && (
              <div className={articles.length > 0 ? 'mt-2 border-t border-[var(--color-border)]' : ''}>
                <div className="px-3 pt-3 pb-1">
                  <span className="text-[10px] font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider">
                    Suggested
                  </span>
                </div>
                {suggestedArticles.map((suggestion) => (
                  <button
                    key={suggestion.tag_id}
                    type="button"
                    onClick={(e) => openArticle(suggestion.tag_id, suggestion.tag_name, { newTab: e.metaKey || e.ctrlKey })}
                    onAuxClick={(e) => {
                      if (e.button === 1) {
                        e.preventDefault();
                        openArticle(suggestion.tag_id, suggestion.tag_name, { newTab: true });
                      }
                    }}
                    className="w-full group flex items-center gap-2 px-3 py-2 text-left hover:bg-[var(--color-bg-card)] transition-colors"
                  >
                    <span className="flex-1 min-w-0">
                      <span
                        className="block truncate text-sm text-[var(--color-text-primary)]"
                        title={suggestion.tag_name}
                      >
                        {suggestion.tag_name}
                      </span>
                      <span className="flex items-center gap-2 mt-0.5 text-[11px] text-[var(--color-text-tertiary)]">
                        <span>
                          {suggestion.atom_count} atom{suggestion.atom_count !== 1 ? 's' : ''}
                        </span>
                        {suggestion.mention_count > 0 && (
                          <span>
                            {suggestion.mention_count} mention{suggestion.mention_count !== 1 ? 's' : ''}
                          </span>
                        )}
                      </span>
                    </span>
                    <Plus
                      className="w-4 h-4 shrink-0 text-[var(--color-accent)] opacity-0 group-hover:opacity-100 transition-opacity"
                      strokeWidth={2}
                    />
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      {/* New Article button */}
      <div className="px-3 py-2 border-t border-[var(--color-border)] shrink-0">
        <button
          onClick={() => setIsModalOpen(true)}
          className="w-full flex items-center justify-start gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
        >
          <Plus className="w-3.5 h-3.5" strokeWidth={2} />
          New Article
        </button>
      </div>

      <NewWikiModal isOpen={isModalOpen} onClose={() => setIsModalOpen(false)} />
    </div>
  );
}
