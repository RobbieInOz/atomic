import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, Telescope } from 'lucide-react';
import { toast } from 'sonner';
import {
  useReportsStore,
  ReportFindingWithAtom,
  ReportFinding,
  ReportFindingCitation,
} from '../../stores/reports';
import { useUIStore } from '../../stores/ui';
import { useCanvasStore } from '../../stores/canvas';
import { useIsMobile } from '../../hooks';
import { BriefingContent } from '../dashboard/widgets/BriefingContent';
import { CitationPopover, CitationForPopover } from '../wiki/CitationPopover';
import { SigmaCanvas } from '../canvas/SigmaCanvas';
import { offsetWithinContainer, useDomScrollSpy } from '../toc/useDomScrollSpy';
import { useTocItems, type TocItem } from '../toc/useTocItems';
import { useTocPublisher } from '../toc/useTocPublisher';
import { formatRelativeDate } from '../../lib/date';

/** Space left above a heading scrolled to from the outline. */
const SCROLL_TOP_MARGIN = 72;

interface FindingReaderProps {
  atomId: string;
}

interface LoadedFinding {
  atom: ReportFindingWithAtom['atom'];
  finding: ReportFinding;
  citations: ReportFindingCitation[];
}

/// Specialized read-only view for a single finding atom. Distinct from
/// AtomReader because findings are agent output with structured
/// citations: the `[N]` markers in the prose become clickable popovers
/// that link to the cited atom, mirroring the dashboard widget's
/// behavior on a full-page surface.
///
/// Data flow:
/// - Try to read the finding from `useReportsStore.findingsByReport`
///   first (populated when the user reached this view from
///   ReportDetailView).
/// - Otherwise fetch via `fetchFinding(atomId)`, which retrieves the
///   atom, provenance, and citations in parallel and merges them into
///   the cache so subsequent renders are instant.
/// - If the fetch fails or returns null, surface a toast and close.
export function FindingReader({ atomId }: FindingReaderProps) {
  const cachedFinding = useReportsStore(s => {
    for (const findings of Object.values(s.findingsByReport)) {
      for (const f of findings) {
        if (f.atom.id === atomId) return f;
      }
    }
    return null;
  });
  const reportName = useReportsStore(s => {
    const reportId = cachedFinding?.finding.report_id;
    if (!reportId) return null;
    return s.byId[reportId]?.name ?? null;
  });
  const fetchFinding = useReportsStore(s => s.fetchFinding);
  const fetchOne = useReportsStore(s => s.fetchOne);
  const closeFindingReader = useUIStore(s => s.closeFindingReader);
  const openReportDetail = useUIStore(s => s.openReportDetail);
  const openReader = useUIStore(s => s.openReader);
  const setViewMode = useUIStore(s => s.setViewMode);
  const isMobile = useIsMobile();

  const scrollerRef = useRef<HTMLDivElement>(null);
  const [loaded, setLoaded] = useState<LoadedFinding | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [activeCitation, setActiveCitation] = useState<CitationForPopover | null>(null);
  const [anchorRect, setAnchorRect] = useState<
    { top: number; left: number; bottom: number; width: number } | null
  >(null);

  // Resolve the finding payload. Only re-runs on atomId change —
  // critically NOT on cachedFinding changes. fetchFinding's
  // side-effect cache merge (which writes into findingsByReport) used
  // to retrigger this effect via the cachedFinding selector, creating
  // an infinite loop that manifested as perpetual loading. Reading the
  // cache once via getState() inside the effect captures the snapshot
  // we need without subscribing to its updates.
  useEffect(() => {
    let cancelled = false;

    // Seed `loaded` synchronously from the cache so a render between
    // mount and fetch-resolve has prose to show instead of the
    // skeleton. We still fetch — the cache stores atom+provenance but
    // not the citation rows, and the agent's [N] markers need
    // citations to resolve.
    const store = useReportsStore.getState();
    let seed: LoadedFinding | null = null;
    for (const findings of Object.values(store.findingsByReport)) {
      const match = findings.find((f) => f.atom.id === atomId);
      if (match) {
        seed = { atom: match.atom, finding: match.finding, citations: [] };
        break;
      }
    }
    setLoaded(seed);
    setIsLoading(true);

    fetchFinding(atomId).then((result) => {
      if (cancelled) return;
      setIsLoading(false);
      if (!result) {
        toast.error('Finding unavailable', {
          description: 'This atom is no longer a finding, or was deleted.',
        });
        closeFindingReader();
        return;
      }
      setLoaded(result);
      // Prime the report row so the breadcrumb's "From: <Report Name>"
      // link has a name to render right away. fetchOne is idempotent
      // if the row is already cached.
      if (result.finding.report_id) {
        void fetchOne(result.finding.report_id);
      }
    });

    return () => { cancelled = true; };
  }, [atomId, fetchFinding, fetchOne, closeFindingReader]);

  const displayedName = loaded
    ? (reportName ?? loaded.finding.report_name_snapshot ?? 'Report')
    : (reportName ?? cachedFinding?.finding.report_name_snapshot ?? null);

  const displayedDate = loaded?.atom.created_at ?? cachedFinding?.atom.created_at ?? null;
  const content = loaded?.atom.content ?? cachedFinding?.atom.content ?? '';

  // Adapter: BriefingContent expects `FindingCitation` (citation_index +
  // atom_id + excerpt); the reports store hands us `ReportFindingCitation`
  // with `position` instead of `citation_index`. Rename the field.
  const citationsForContent = (loaded?.citations ?? []).map(c => ({
    citation_index: c.position,
    atom_id: c.cited_atom_id,
    excerpt: c.excerpt,
  }));

  // Table of contents, parsed from the raw finding markdown: BriefingContent
  // demotes every heading to <h3>/<h4>, so true levels can only come from the
  // source, and the ids it stamps are matched back by offset. Findings are
  // read-only, so re-parse on the next tick rather than waiting out the
  // editor-oriented debounce.
  const headings = useTocItems(content, 0);
  const headingIdByOffset = useMemo(
    () => new Map(headings.map((heading) => [heading.offset, heading.id])),
    [headings],
  );
  const activeTocId = useDomScrollSpy(scrollerRef, headings);
  const scrollToTocItem = useCallback((item: TocItem) => {
    const container = scrollerRef.current;
    const el = container?.ownerDocument.getElementById(item.id);
    if (!container || !el) return;
    container.scrollTo({ top: offsetWithinContainer(container, el) - SCROLL_TOP_MARGIN });
  }, []);
  useTocPublisher({
    // Nothing to outline until the finding's prose is on screen — before that
    // the body is a skeleton or an unavailable notice.
    source: loaded ? 'finding' : null,
    items: headings,
    activeId: activeTocId,
    scrollToItem: scrollToTocItem,
  });

  // Atom ids the mini-canvas scopes to: every distinct atom cited by
  // this finding. The canvas adds 1-hop neighbors from the existing
  // semantic-edge set so a single-citation finding still renders a
  // meaningful neighborhood instead of one orphan node. Undefined
  // until citations resolve so the canvas doesn't render a global
  // graph during the load window.
  const citedAtomIds = useMemo(() => {
    if (!loaded || loaded.citations.length === 0) return undefined;
    const seen = new Set<string>();
    const out: string[] = [];
    for (const c of loaded.citations) {
      if (seen.has(c.cited_atom_id)) continue;
      seen.add(c.cited_atom_id);
      out.push(c.cited_atom_id);
    }
    return out;
  }, [loaded]);

  // Clicking a node in the mini-canvas jumps to the full canvas
  // focused on that atom. Same affordance as the dashboard widget —
  // `setViewMode('canvas')` deactivates the active tab automatically.
  const handleMiniNodeClick = (clickedAtomId: string) => {
    const canvas = useCanvasStore.getState();
    canvas.setPendingCamera(null);
    canvas.setPendingFocusAtomId(clickedAtomId);
    setViewMode('canvas');
  };

  const handleCitationClick = (
    citation: { citation_index: number; atom_id: string; excerpt: string },
    element: HTMLElement,
  ) => {
    const rect = element.getBoundingClientRect();
    setActiveCitation(citation);
    setAnchorRect({ top: rect.top, left: rect.left, bottom: rect.bottom, width: rect.width });
  };

  const closePopover = () => {
    setActiveCitation(null);
    setAnchorRect(null);
  };

  return (
    <div className="h-full overflow-hidden flex flex-col">
      {/* Header — back + parent-report breadcrumb + date eyebrow. */}
      <div className="flex items-center gap-3 px-5 py-3 border-b border-[var(--color-border)] flex-shrink-0">
        <button
          onClick={closeFindingReader}
          title="Back"
          aria-label="Back"
          className="
            p-1.5 rounded-md text-[var(--color-text-secondary)]
            hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]
            transition-colors
          "
        >
          <ArrowLeft className="w-4 h-4" strokeWidth={2} />
        </button>

        <div className="flex items-center gap-2 min-w-0 flex-1">
          <Telescope className="w-4 h-4 text-[var(--color-text-tertiary)] flex-shrink-0" strokeWidth={2} />
          {displayedName ? (
            <button
              type="button"
              onClick={() => {
                const reportId = loaded?.finding.report_id ?? cachedFinding?.finding.report_id;
                if (reportId) openReportDetail(reportId);
              }}
              disabled={!loaded?.finding.report_id && !cachedFinding?.finding.report_id}
              className="
                text-sm font-medium text-[var(--color-text-primary)] truncate
                hover:text-[var(--color-accent-light)] transition-colors
                disabled:hover:text-[var(--color-text-primary)] disabled:cursor-default
              "
            >
              {displayedName}
            </button>
          ) : isLoading ? (
            <div className="h-4 w-40 bg-[var(--color-border)] rounded animate-pulse" />
          ) : (
            <span className="text-sm font-medium text-[var(--color-text-tertiary)]">
              Finding
            </span>
          )}
          {displayedDate && (
            <>
              <span className="text-[var(--color-text-tertiary)]/40">·</span>
              <span className="text-[10.5px] font-medium uppercase tracking-[0.14em] text-[var(--color-text-tertiary)] tabular-nums">
                {formatRelativeDate(displayedDate).toUpperCase()}
              </span>
            </>
          )}
        </div>
      </div>

      {/* Body — finding content with clickable citation markers, plus
          a mini-canvas of the cited atoms (and their 1-hop neighbors)
          that doubles as a jump-into-canvas affordance. Desktop floats
          the canvas right of the prose; mobile stacks it above so the
          text isn't pushed into a narrow column. */}
      <div ref={scrollerRef} className="flex-1 overflow-y-auto">
        <div className="max-w-3xl mx-auto px-6 py-8">
          {!isMobile && citedAtomIds && citedAtomIds.length > 0 && (
            <div className="float-right ml-4 mb-4 w-80 aspect-[4/3]">
              <SigmaCanvas
                mode="preview"
                filterAtomIds={citedAtomIds}
                onPreviewNodeClick={handleMiniNodeClick}
              />
            </div>
          )}

          {isMobile && citedAtomIds && citedAtomIds.length > 0 && (
            <div className="mb-4 w-full aspect-[16/10]">
              <SigmaCanvas
                mode="preview"
                filterAtomIds={citedAtomIds}
                onPreviewNodeClick={handleMiniNodeClick}
              />
            </div>
          )}

          {loaded ? (
            <BriefingContent
              content={content}
              citations={citationsForContent}
              headingIdByOffset={headingIdByOffset}
              onCitationClick={handleCitationClick}
            />
          ) : isLoading ? (
            <div className="space-y-3 animate-pulse">
              <div className="h-4 w-full bg-[var(--color-border)] rounded" />
              <div className="h-4 w-5/6 bg-[var(--color-border)] rounded" />
              <div className="h-4 w-4/6 bg-[var(--color-border)] rounded" />
              <div className="h-4 w-full bg-[var(--color-border)] rounded mt-6" />
              <div className="h-4 w-3/4 bg-[var(--color-border)] rounded" />
            </div>
          ) : (
            <p className="text-sm text-[var(--color-text-tertiary)]">
              Finding unavailable.
            </p>
          )}

          {/* Clear the float so any subsequent UI sits below the canvas
              rather than wrapping around it. */}
          <div className="md:clear-right" />
        </div>
      </div>

      {/* Citation popover — same component the wiki + dashboard use. */}
      {activeCitation && anchorRect && (
        <CitationPopover
          citation={activeCitation}
          anchorRect={anchorRect}
          onClose={closePopover}
          onViewAtom={(citedAtomId, highlightText) => {
            closePopover();
            openReader(citedAtomId, highlightText ?? undefined);
          }}
        />
      )}
    </div>
  );
}
