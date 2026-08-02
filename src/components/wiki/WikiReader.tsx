import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Clock, RefreshCw } from 'lucide-react';
import { useWikiStore } from '../../stores/wiki';
import { useUIStore } from '../../stores/ui';
import { WikiArticleContent, WIKI_TITLE_ANCHOR_ID } from './WikiArticleContent';
import { WikiEmptyState } from './WikiEmptyState';
import { WikiGenerating } from './WikiGenerating';
import { WikiProposalDiff } from './WikiProposalDiff';
import { offsetWithinContainer, useDomScrollSpy } from '../toc/useDomScrollSpy';
import { useTocItems, type TocItem } from '../toc/useTocItems';
import { useTocPublisher } from '../toc/useTocPublisher';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { formatRelativeTime } from '../../lib/date';

interface WikiReaderProps {
  tagId: string;
  tagName: string;
  highlightText?: string | null;
}

/** Space left above a heading scrolled to from the outline. */
const SCROLL_TOP_MARGIN = 72;

/// The generator opens every article with a `# Tag Name` line, which the
/// reader already shows as page chrome. Strip it once here so the rendered
/// body and the parsed outline see the exact same string — heading offsets
/// only mean anything against the text react-markdown was handed.
function stripArticleTitle(content: string): string {
  return content.replace(/^#\s+[^\n]+\n+/, '');
}

export function WikiReader({ tagId, tagName, highlightText }: WikiReaderProps) {
  const currentArticle = useWikiStore(s => s.currentArticle);
  const articleStatus = useWikiStore(s => s.articleStatus);
  const relatedTags = useWikiStore(s => s.relatedTags);
  const wikiLinks = useWikiStore(s => s.wikiLinks);
  const isLoading = useWikiStore(s => s.isLoading);
  const isGenerating = useWikiStore(s => s.isGenerating);
  const isUpdating = useWikiStore(s => s.isUpdating);
  const error = useWikiStore(s => s.error);
  const clearError = useWikiStore(s => s.clearError);

  const fetchArticle = useWikiStore(s => s.fetchArticle);
  const fetchArticleStatus = useWikiStore(s => s.fetchArticleStatus);
  const fetchRelatedTags = useWikiStore(s => s.fetchRelatedTags);
  const fetchWikiLinks = useWikiStore(s => s.fetchWikiLinks);
  const generateArticle = useWikiStore(s => s.generateArticle);

  // Version history
  const versions = useWikiStore(s => s.versions);
  const selectedVersion = useWikiStore(s => s.selectedVersion);
  const fetchVersions = useWikiStore(s => s.fetchVersions);
  const selectVersion = useWikiStore(s => s.selectVersion);
  const clearSelectedVersion = useWikiStore(s => s.clearSelectedVersion);

  // Proposal state
  const proposal = useWikiStore(s => s.proposal);
  const isProposing = useWikiStore(s => s.isProposing);
  const isAccepting = useWikiStore(s => s.isAccepting);
  const isDismissing = useWikiStore(s => s.isDismissing);
  const reviewingProposal = useWikiStore(s => s.reviewingProposal);
  const proposeArticle = useWikiStore(s => s.proposeArticle);
  const acceptProposal = useWikiStore(s => s.acceptProposal);
  const dismissProposal = useWikiStore(s => s.dismissProposal);
  const startReviewingProposal = useWikiStore(s => s.startReviewingProposal);
  const stopReviewingProposal = useWikiStore(s => s.stopReviewingProposal);
  const fetchProposal = useWikiStore(s => s.fetchProposal);

  const overlayNavigate = useUIStore(s => s.overlayNavigate);

  const [showRegenerateModal, setShowRegenerateModal] = useState(false);
  const [showVersions, setShowVersions] = useState(false);
  const versionsRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const prevTagIdRef = useRef<string | null>(null);

  // Table of contents. Computed above the loading/error/empty returns below so
  // the hook order stays fixed; `scrollerRef` simply has nothing to measure
  // until the article renders.
  const displayContent = useMemo(
    () => stripArticleTitle(selectedVersion?.content ?? currentArticle?.article.content ?? ''),
    [selectedVersion, currentArticle],
  );
  // Nothing is typed here — the content only changes when another article
  // loads — so re-parse on the next tick rather than holding the previous
  // article's outline for the editor-oriented debounce.
  const headings = useTocItems(displayContent, 0);
  const headingIdByOffset = useMemo(
    () => new Map(headings.map((heading) => [heading.offset, heading.id])),
    [headings],
  );
  // The stripped title is still on screen as the page's own heading, so the
  // outline opens with it — clicking it returns to the top of the article.
  const tocItems = useMemo<TocItem[]>(
    () => [
      { id: WIKI_TITLE_ANCHOR_ID, depth: 1, text: tagName, offset: 0, line: 1 },
      ...headings,
    ],
    [headings, tagName],
  );
  const activeTocId = useDomScrollSpy(scrollerRef, tocItems);
  const scrollToTocItem = useCallback((item: TocItem) => {
    const container = scrollerRef.current;
    const el = container?.ownerDocument.getElementById(item.id);
    if (!container || !el) return;
    container.scrollTo({ top: offsetWithinContainer(container, el) - SCROLL_TOP_MARGIN });
  }, []);
  // Every other state below — loading, error, generating, no article yet, the
  // proposal diff — renders something other than the article body, and an
  // outline of a document that isn't on screen would only mislead.
  const isArticleVisible =
    !isLoading && !error && !isGenerating && !!currentArticle &&
    !(reviewingProposal && proposal && !selectedVersion);
  useTocPublisher({
    source: isArticleVisible ? 'wiki' : null,
    items: tocItems,
    activeId: activeTocId,
    scrollToItem: scrollToTocItem,
  });

  // Fetch article data when tagId changes
  useEffect(() => {
    if (tagId === prevTagIdRef.current) return;
    prevTagIdRef.current = tagId;

    // Clear previous article state
    clearSelectedVersion();

    // Fetch all data for this article
    fetchArticle(tagId);
    fetchArticleStatus(tagId);
    fetchRelatedTags(tagId);
    fetchWikiLinks(tagId);
    fetchVersions(tagId);
    fetchProposal(tagId);
  }, [tagId]);

  // Close versions dropdown on outside click
  useEffect(() => {
    if (!showVersions) return;
    const handleClick = (e: MouseEvent) => {
      if (versionsRef.current && !versionsRef.current.contains(e.target as Node)) {
        setShowVersions(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [showVersions]);

  // Escape key dismisses the wiki reader
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !(e.ctrlKey || e.metaKey)) {
        useUIStore.getState().overlayDismiss();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, []);

  const handleGenerate = () => {
    generateArticle(tagId, tagName);
  };

  const handleUpdate = () => {
    proposeArticle(tagId, tagName);
  };

  const handleViewAtom = (atomId: string, highlightText?: string) => {
    overlayNavigate({ type: 'reader', atomId, highlightText });
  };

  const handleNavigateToArticle = (targetTagId: string, targetTagName: string) => {
    overlayNavigate({ type: 'wiki', tagId: targetTagId, tagName: targetTagName });
  };

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full text-[var(--color-text-secondary)]">
        Loading...
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-4 p-4">
        <p className="text-red-400 text-sm">{error}</p>
        <button onClick={clearError} className="text-xs text-[var(--color-accent)] hover:underline">
          Dismiss
        </button>
      </div>
    );
  }

  if (isGenerating) {
    return <WikiGenerating tagName={tagName} atomCount={articleStatus?.current_atom_count || 0} />;
  }

  if (!currentArticle) {
    return (
      <WikiEmptyState
        tagName={tagName}
        atomCount={articleStatus?.current_atom_count || 0}
        onGenerate={handleGenerate}
        isGenerating={false}
      />
    );
  }

  const displayCitations = selectedVersion
    ? selectedVersion.citations
    : currentArticle.citations;

  return (
    <div className="h-full flex flex-col overflow-hidden bg-[var(--color-bg-main)]">
      {/* Version viewing banner */}
      {selectedVersion && (
        <div className="flex items-center justify-between px-6 py-2 bg-amber-500/10 border-b border-amber-500/20 flex-shrink-0">
          <span className="text-sm text-amber-400">Viewing previous version</span>
          <button onClick={clearSelectedVersion} className="text-sm text-amber-400 hover:text-amber-300 underline transition-colors">
            Return to current
          </button>
        </div>
      )}

      {/* Proposal ready banner */}
      {!!proposal && !selectedVersion && (
        <div className="flex items-center justify-between px-6 py-2 bg-[var(--color-accent)]/15 border-b border-[var(--color-accent)]/30 flex-shrink-0">
          <span className="text-sm text-[var(--color-accent-light)]">
            Suggested update ready
            {proposal.new_atom_count > 0 && (
              <> — based on {proposal.new_atom_count} new atom{proposal.new_atom_count !== 1 ? 's' : ''}</>
            )}
          </span>
          <Button variant="primary" size="sm" onClick={startReviewingProposal}>Review</Button>
        </div>
      )}

      {/* New atoms available banner */}
      {!proposal && !selectedVersion && (articleStatus?.new_atoms_available || 0) > 0 && (
        <div className="flex items-center justify-between px-6 py-2 bg-[var(--color-accent)]/10 border-b border-[var(--color-accent)]/20 flex-shrink-0">
          <span className="text-sm text-[var(--color-accent-light)]">
            {articleStatus!.new_atoms_available} new atom{articleStatus!.new_atoms_available !== 1 ? 's' : ''} available
          </span>
          <Button variant="primary" size="sm" onClick={handleUpdate} disabled={isProposing || isUpdating}>
            {isProposing ? 'Generating...' : 'Generate update'}
          </Button>
        </div>
      )}

      {/* Proposal diff view */}
      {reviewingProposal && proposal && !selectedVersion ? (
        <WikiProposalDiff
          liveContent={currentArticle.article.content}
          proposalContent={proposal.content}
          newAtomCount={proposal.new_atom_count}
          createdAt={proposal.created_at}
          onAccept={() => acceptProposal(tagId)}
          onDismiss={() => dismissProposal(tagId)}
          onCancel={stopReviewingProposal}
          isAccepting={isAccepting}
          isDismissing={isDismissing}
        />
      ) : (
        <div ref={scrollerRef} className="flex-1 overflow-y-auto scrollbar-auto-hide">
          <WikiArticleContent
            content={displayContent}
            headingIdByOffset={headingIdByOffset}
            citations={displayCitations}
            wikiLinks={selectedVersion ? [] : wikiLinks}
            relatedTags={selectedVersion ? [] : relatedTags}
            tagName={tagName}
            updatedAt={selectedVersion ? selectedVersion.created_at : currentArticle.article.updated_at}
            sourceCount={displayCitations.length}
            highlightText={highlightText}
            titleActions={
              <>
                {/* Version history */}
                {versions.length > 0 && (
                  <div className="relative" ref={versionsRef}>
                    <Button variant="ghost" size="sm" onClick={() => setShowVersions(!showVersions)}>
                      <Clock className="w-4 h-4 mr-1" strokeWidth={2} />
                      {versions.length}
                    </Button>
                    {showVersions && (
                      <div className="absolute right-0 top-full mt-1 w-64 bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1 max-h-64 overflow-y-auto">
                        {selectedVersion && (
                          <button
                            onClick={() => { clearSelectedVersion(); setShowVersions(false); }}
                            className="w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors text-[var(--color-accent-light)] font-medium"
                          >
                            Current version
                          </button>
                        )}
                        {versions.map((v) => (
                          <button
                            key={v.id}
                            onClick={() => { selectVersion(v.id); setShowVersions(false); }}
                            className="w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors"
                          >
                            <div className="text-[var(--color-text-primary)]">Version {v.version_number}</div>
                            <div className="text-xs text-[var(--color-text-secondary)]">
                              {formatRelativeTime(v.created_at)} • {v.atom_count} source{v.atom_count !== 1 ? 's' : ''}
                            </div>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                )}
                {/* Regenerate */}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setShowRegenerateModal(true)}
                  disabled={isUpdating || !!selectedVersion}
                >
                  <RefreshCw className="w-4 h-4" strokeWidth={2} />
                </Button>
              </>
            }
            onViewAtom={handleViewAtom}
            onNavigateToArticle={handleNavigateToArticle}
          />
        </div>
      )}

      {/* Regenerate confirmation modal */}
      <Modal
        isOpen={showRegenerateModal}
        onClose={() => setShowRegenerateModal(false)}
        title="Regenerate Article"
        confirmLabel="Regenerate"
        confirmVariant="primary"
        onConfirm={() => {
          setShowRegenerateModal(false);
          handleGenerate();
        }}
      >
        <p className="text-[var(--color-text-primary)]">
          This will regenerate the article from scratch, replacing the current content.
          The current version will be saved in the version history.
          Are you sure you want to continue?
        </p>
      </Modal>
    </div>
  );
}
