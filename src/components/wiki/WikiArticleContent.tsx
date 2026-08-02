import { useState, useEffect, useCallback, Fragment, ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Plus } from 'lucide-react';
import type { Element } from 'hast';
import { WikiCitation, WikiLink, RelatedTag } from '../../stores/wiki';
import { CitationLink } from './CitationLink';
import { CitationPopover } from './CitationPopover';
import { WikiLinkInline } from './WikiLinkInline';
import { SearchBar } from '../ui/SearchBar';
import { MarkdownImage } from '../ui/MarkdownImage';
import { useContentSearch } from '../../hooks';
import { formatRelativeTime } from '../../lib/date';

/// Anchor for the article's own title. The body markdown is rendered with its
/// leading `# Title` removed (see `stripArticleTitle` in WikiReader), so an
/// outline entry for the title has to point at this rendered heading instead
/// of at a source offset.
export const WIKI_TITLE_ANCHOR_ID = '__top';

interface WikiArticleContentProps {
  /** The article body, exactly as it should render — the caller has already
   *  stripped the leading title so heading offsets it parsed line up here. */
  content: string;
  citations: WikiCitation[];
  wikiLinks: WikiLink[];
  relatedTags: RelatedTag[];
  tagName: string;
  updatedAt: string;
  sourceCount: number;
  titleActions?: ReactNode;
  /** When present, open the search bar pre-populated with this string so the
   *  article scrolls to / highlights that match on open. Used by the search
   *  palette to take a wiki hit straight to its specific occurrence. */
  highlightText?: string | null;
  /** Ids to stamp on rendered headings, keyed by their offset in `content`.
   *  Supplied by whoever owns the table of contents so its entries and the
   *  DOM agree on names. */
  headingIdByOffset?: Map<number, string>;
  onViewAtom: (atomId: string, highlightText?: string) => void;
  onNavigateToArticle: (tagId: string, tagName: string) => void;
}

interface HeadingProps {
  node?: Element;
  children?: ReactNode;
}

export function WikiArticleContent({ content, citations, wikiLinks, relatedTags, tagName, updatedAt, sourceCount, titleActions, highlightText, headingIdByOffset, onViewAtom, onNavigateToArticle }: WikiArticleContentProps) {
  const [activeCitation, setActiveCitation] = useState<WikiCitation | null>(null);
  const [anchorRect, setAnchorRect] = useState<{ top: number; left: number; bottom: number; width: number } | null>(null);

  // Content search
  const {
    isOpen: isSearchOpen,
    query: searchQuery,
    searchedQuery,
    currentIndex,
    totalMatches,
    setQuery: setSearchQuery,
    openSearch,
    closeSearch,
    goToNext,
    goToPrevious,
    processChildren: highlightChildren,
  } = useContentSearch(content);

  // Keyboard handler for Ctrl+F / Cmd+F
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
        e.preventDefault();
        openSearch();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [openSearch]);

  // When the caller passes a highlight string (e.g. from the search palette
  // selecting a specific wiki match), open the search bar primed with it.
  // The content-search hook handles scrolling to the first occurrence.
  useEffect(() => {
    if (highlightText && highlightText.trim()) {
      openSearch();
      setSearchQuery(highlightText);
    }
  }, [highlightText, openSearch, setSearchQuery]);

  // Create a map of citation index to citation object
  const citationMap = new Map(citations.map(c => [c.citation_index, c]));

  // Create a map of wiki link name (lowercase) to wiki link object
  const wikiLinkMap = new Map(wikiLinks.map(l => [l.target_tag_name.toLowerCase(), l]));

  const handleCitationClick = (citation: WikiCitation, element: HTMLElement) => {
    const rect = element.getBoundingClientRect();
    setActiveCitation(citation);
    setAnchorRect({ top: rect.top, left: rect.left, bottom: rect.bottom, width: rect.width });
  };

  const handleClosePopover = () => {
    setActiveCitation(null);
    setAnchorRect(null);
  };

  const handleWikiLinkClick = (link: WikiLink) => {
    if (link.target_tag_id) {
      onNavigateToArticle(link.target_tag_id, link.target_tag_name);
    }
  };

  // Process text to replace [N] citations and [[wiki links]] with interactive components
  const processTextWithCitations = (text: string): (string | JSX.Element)[] => {
    // Split on both [N] citations and [[wiki links]]
    const parts = text.split(/(\[\d+\]|\[\[[^\]]+\]\])/g);
    return parts.map((part, i) => {
      // Check for citation [N]
      const citationMatch = part.match(/^\[(\d+)\]$/);
      if (citationMatch) {
        const index = parseInt(citationMatch[1], 10);
        const citation = citationMap.get(index);
        if (citation) {
          return (
            <CitationLink
              key={`citation-${i}-${index}`}
              index={index}
              onClick={(e) => handleCitationClick(citation, e.currentTarget)}
            />
          );
        }
      }

      // Check for wiki link [[Name]]
      const wikiLinkMatch = part.match(/^\[\[([^\]]+)\]\]$/);
      if (wikiLinkMatch) {
        const linkName = wikiLinkMatch[1].trim();
        const link = wikiLinkMap.get(linkName.toLowerCase());
        if (link) {
          return (
            <WikiLinkInline
              key={`wikilink-${i}-${linkName}`}
              tagName={linkName}
              hasArticle={link.has_article}
              onClick={() => handleWikiLinkClick(link)}
            />
          );
        }
        // Unknown wiki link — render as plain text with dimmed style
        return (
          <span key={`wikilink-unknown-${i}`} className="text-[var(--color-text-tertiary)]" title="Unknown article">
            {linkName}
          </span>
        );
      }

      // Return raw string so highlighting can be applied
      return part;
    });
  };

  // Process children recursively to handle citations, wiki links, and search highlighting
  const processChildren = useCallback((children: ReactNode): ReactNode => {
    if (typeof children === 'string') {
      const withCitations = processTextWithCitations(children);
      if (isSearchOpen && searchQuery.trim()) {
        return withCitations.map((part, i) => {
          if (typeof part === 'string') {
            return <Fragment key={`hl-${i}`}>{highlightChildren(part)}</Fragment>;
          }
          return part;
        });
      }
      return withCitations.map((part, i) =>
        typeof part === 'string' ? <Fragment key={`t-${i}`}>{part}</Fragment> : part
      );
    }
    if (Array.isArray(children)) {
      return children.map((child, i) => (
        <Fragment key={i}>{processChildren(child)}</Fragment>
      ));
    }
    return children;
  }, [isSearchOpen, searchQuery, highlightChildren, wikiLinks, citations]);

  // Heading ids come from the outline's own parse, matched by source offset.
  // Re-slugging the rendered children here would drift from it on duplicate
  // titles and on inline markup.
  const headingId = (node?: Element): string | undefined => {
    const offset = node?.position?.start?.offset;
    return offset === undefined ? undefined : headingIdByOffset?.get(offset);
  };

  // Custom components for react-markdown
  const components = {
    p: ({ children }: { children?: ReactNode }) => (
      <p>{processChildren(children)}</p>
    ),
    li: ({ children }: { children?: ReactNode }) => (
      <li>{processChildren(children)}</li>
    ),
    td: ({ children }: { children?: ReactNode }) => (
      <td>{processChildren(children)}</td>
    ),
    th: ({ children }: { children?: ReactNode }) => (
      <th>{processChildren(children)}</th>
    ),
    strong: ({ children }: { children?: ReactNode }) => (
      <strong>{processChildren(children)}</strong>
    ),
    em: ({ children }: { children?: ReactNode }) => (
      <em>{processChildren(children)}</em>
    ),
    del: ({ children }: { children?: ReactNode }) => (
      <del>{processChildren(children)}</del>
    ),
    h1: ({ node, children }: HeadingProps) => (
      <h1 id={headingId(node)}>{processChildren(children)}</h1>
    ),
    h2: ({ node, children }: HeadingProps) => (
      <h2 id={headingId(node)}>{processChildren(children)}</h2>
    ),
    h3: ({ node, children }: HeadingProps) => (
      <h3 id={headingId(node)}>{processChildren(children)}</h3>
    ),
    h4: ({ node, children }: HeadingProps) => (
      <h4 id={headingId(node)}>{processChildren(children)}</h4>
    ),
    h5: ({ node, children }: HeadingProps) => (
      <h5 id={headingId(node)}>{processChildren(children)}</h5>
    ),
    h6: ({ node, children }: HeadingProps) => (
      <h6 id={headingId(node)}>{processChildren(children)}</h6>
    ),
    blockquote: ({ children }: { children?: ReactNode }) => (
      <blockquote>{processChildren(children)}</blockquote>
    ),
    a: ({ href, children }: { href?: string; children?: ReactNode }) => (
      <a href={href} target="_blank" rel="noopener noreferrer">
        {processChildren(children)}
      </a>
    ),
    code: ({ className, children }: { className?: string; children?: ReactNode }) => {
      const isBlock = className?.startsWith('language-');
      if (isBlock) {
        return <code className={className}>{processChildren(children)}</code>;
      }
      return <code>{processChildren(children)}</code>;
    },
    pre: ({ children }: { children?: ReactNode }) => (
      <pre>{children}</pre>
    ),
    img: ({ src, alt }: { src?: string; alt?: string }) => (
      <MarkdownImage src={src} alt={alt} />
    ),
  };

  return (
    <>
      <div className="wiki-content relative">
        {/* Search bar */}
        {isSearchOpen && (
          <SearchBar
            query={searchQuery}
            searchedQuery={searchedQuery}
            onQueryChange={setSearchQuery}
            currentIndex={currentIndex}
            totalMatches={totalMatches}
            onNext={goToNext}
            onPrevious={goToPrevious}
            onClose={closeSearch}
          />
        )}

        <div className="max-w-3xl mx-auto px-8 py-6">
          {/* Article title */}
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h1 id={WIKI_TITLE_ANCHOR_ID} className="text-2xl font-bold text-[var(--color-text-primary)] mb-1">{tagName}</h1>
              <p className="text-xs text-[var(--color-text-secondary)]">
                Updated {formatRelativeTime(updatedAt)} • {sourceCount} source{sourceCount !== 1 ? 's' : ''}
              </p>
            </div>
            {titleActions && (
              <div className="flex items-center gap-1 flex-shrink-0">
                {titleActions}
              </div>
            )}
          </div>
          <div className="border-b border-[var(--color-border)] mb-5 mt-4" />

          <div className="prose prose-invert prose-headings:text-[var(--color-text-primary)] prose-h2:border-b prose-h2:border-[var(--color-border)] prose-h2:pb-1.5 prose-h2:mb-3 prose-p:text-[var(--color-text-primary)] prose-p:leading-relaxed prose-a:text-[var(--color-text-primary)] prose-a:underline prose-a:decoration-[var(--color-border-hover)] hover:prose-a:decoration-current prose-strong:text-[var(--color-text-primary)] prose-code:text-[var(--color-accent-light)] prose-code:bg-[var(--color-bg-card)] prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-pre:bg-[var(--color-bg-card)] prose-pre:border prose-pre:border-[var(--color-border)] prose-blockquote:border-l-[var(--color-accent)] prose-blockquote:text-[var(--color-text-secondary)] prose-li:text-[var(--color-text-primary)] prose-hr:border-[var(--color-border)]">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
              {content}
            </ReactMarkdown>
          </div>
        </div>

        {/* Related Articles section (tags with existing articles) */}
        {relatedTags.some(t => t.has_article) && (
          <div className="max-w-3xl mx-auto border-t border-[var(--color-border)] mt-2 pt-4 px-8 pb-4">
            <h3 className="text-xs font-medium text-[var(--color-text-tertiary)] uppercase tracking-wider mb-3">
              Related Articles
            </h3>
            <div className="flex flex-wrap gap-2">
              {relatedTags.filter(t => t.has_article).map(tag => (
                <button
                  key={tag.tag_id}
                  onClick={() => onNavigateToArticle(tag.tag_id, tag.tag_name)}
                  className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs transition-colors bg-[var(--color-accent)]/10 text-[var(--color-accent)] hover:bg-[var(--color-accent)]/20"
                  title={`${tag.shared_atoms} shared atoms, ${tag.semantic_edges} semantic connections`}
                >
                  {tag.tag_name}
                </button>
              ))}
            </div>
          </div>
        )}

        {/* Recommended articles to generate (related tags without articles) */}
        {relatedTags.some(t => !t.has_article) && (
          <div className="max-w-3xl mx-auto border-t border-[var(--color-border)] pt-4 px-8 pb-6">
            <h3 className="text-xs font-medium text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
              Recommended
            </h3>
            <div className="divide-y divide-[var(--color-border)] rounded-lg border border-[var(--color-border)] overflow-hidden">
              {relatedTags.filter(t => !t.has_article).map(tag => (
                <button
                  key={tag.tag_id}
                  onClick={() => onNavigateToArticle(tag.tag_id, tag.tag_name)}
                  className="w-full group flex items-center justify-between px-3 py-2 hover:bg-[var(--color-bg-elevated)] transition-colors text-left"
                >
                  <div className="min-w-0 flex-1">
                    <span className="text-sm text-[var(--color-text-primary)]">
                      {tag.tag_name}
                    </span>
                    <div className="flex items-center gap-2 mt-0.5">
                      <span className="text-[11px] text-[var(--color-text-tertiary)]">
                        {tag.shared_atoms} shared atom{tag.shared_atoms !== 1 ? 's' : ''}
                      </span>
                      {tag.semantic_edges > 0 && (
                        <span className="text-[11px] text-[var(--color-text-tertiary)]">
                          {tag.semantic_edges} semantic link{tag.semantic_edges !== 1 ? 's' : ''}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="flex-shrink-0 opacity-0 group-hover:opacity-100 transition-opacity ml-2">
                    <Plus className="w-4 h-4 text-[var(--color-accent)]" strokeWidth={2} />
                  </div>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Citation popover */}
      {activeCitation && anchorRect && (
        <CitationPopover
          citation={activeCitation}
          anchorRect={anchorRect}
          onClose={handleClosePopover}
          onViewAtom={onViewAtom}
        />
      )}
    </>
  );
}
