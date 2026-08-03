import { useRef, useEffect } from 'react';
import { TagTree } from '../tags/TagTree';
import { TocPanel } from '../toc/TocPanel';
import { WikiArticlesList } from '../wiki/WikiArticlesList';
import { RecentAtomsPanel } from '../dashboard/RecentAtomsPanel';
import { LatestFindingsPanel } from '../reports/LatestFindingsPanel';
import { ConversationsSidebar } from '../chat/ConversationsSidebar';
import { useUIStore } from '../../stores/ui';
import { useTocStore, type TocSource } from '../../stores/toc';
import { isTauri } from '../../lib/platform';
import { SIDEBAR_CONTEXT_TITLES, useSidebarContext, type SidebarContext } from './sidebar-context';

const COLLAPSE_BREAKPOINT = 768;

const TOC_EMPTY_LABELS: Record<TocSource, string> = {
  atom: 'No headings in this note',
  wiki: 'No headings in this article',
  finding: 'No headings in this finding',
};

/// The tag tree's empty state offers to configure tag categories. Layout owns
/// the app's single SettingsModal, so ask for it by event rather than by prop.
function openTagCategorySettings() {
  window.dispatchEvent(new CustomEvent('open-settings', { detail: { tab: 'tag-categories' } }));
}

/// Table of contents for the reader that is currently mounted. The reader
/// publishes its outline from an effect, one commit after the context flips,
/// so until its publication lands render nothing — showing the previous
/// document's headings, or a premature empty state, would both be wrong.
function TocSidebar({ source }: { source: TocSource }) {
  const publishedSource = useTocStore((s) => s.source);
  const items = useTocStore((s) => s.items);
  const activeId = useTocStore((s) => s.activeId);
  const scrollToItem = useTocStore((s) => s.scrollToItem);

  if (publishedSource !== source) return null;

  return (
    <TocPanel
      items={items}
      activeId={activeId}
      onItemClick={scrollToItem}
      emptyLabel={TOC_EMPTY_LABELS[source]}
    />
  );
}

/// Panel body for the current context. Swapping the body never touches the
/// panel's open/closed state — that stays the user's manual toggle.
function SidebarBody({ context }: { context: SidebarContext }) {
  switch (context.kind) {
    case 'tags':
      return <TagTree onOpenTagSettings={openTagCategorySettings} />;
    case 'toc':
      return <TocSidebar source={context.source} />;
    case 'wiki-list':
      return <WikiArticlesList />;
    case 'recent':
      return <RecentAtomsPanel />;
    case 'findings':
      return <LatestFindingsPanel reportId={context.reportId} />;
    case 'conversations':
      return <ConversationsSidebar />;
  }
}

export function LeftPanel() {
  const context = useSidebarContext();
  const leftPanelOpen = useUIStore(s => s.leftPanelOpen);
  const setLeftPanelOpen = useUIStore(s => s.setLeftPanelOpen);
  const panelRef = useRef<HTMLDivElement>(null);

  // On mount, force-collapse on small screens (the sidebar covers content
  // there). On large screens we respect the persisted preference. Resize
  // events still auto-toggle in both directions.
  useEffect(() => {
    const mq = window.matchMedia(`(max-width: ${COLLAPSE_BREAKPOINT}px)`);
    if (mq.matches) {
      setLeftPanelOpen(false);
    }
    const handleChange = (e: MediaQueryListEvent) => {
      setLeftPanelOpen(!e.matches);
    };
    mq.addEventListener('change', handleChange);
    return () => mq.removeEventListener('change', handleChange);
  }, [setLeftPanelOpen]);

  // Close panel on outside click when in overlay mode (small screens)
  useEffect(() => {
    if (!leftPanelOpen) return;
    const mq = window.matchMedia(`(max-width: ${COLLAPSE_BREAKPOINT}px)`);
    if (!mq.matches) return;

    const handleClick = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        setLeftPanelOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [leftPanelOpen, setLeftPanelOpen]);

  return (
    <>
      {/* Backdrop for overlay mode on small screens */}
      <div
        className={`fixed inset-0 bg-black/40 z-30 md:hidden transition-opacity duration-200 ${
          leftPanelOpen ? 'opacity-100' : 'opacity-0 pointer-events-none'
        }`}
      />

      {/* Panel */}
      <aside
        ref={panelRef}
        className={`
          relative h-full z-10 flex-shrink-0 overflow-hidden
          md:transition-[width,border-color] md:duration-300 md:ease-in-out
          max-md:transition-all max-md:duration-300 max-md:ease-in-out
          max-md:fixed max-md:top-0 max-md:left-0 max-md:z-40 max-md:shadow-2xl max-md:w-[250px]
          max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)] max-md:pl-[env(safe-area-inset-left)]
          ${leftPanelOpen ? 'max-md:translate-x-0' : 'max-md:-translate-x-full'}
          ${leftPanelOpen ? 'md:w-[250px]' : 'md:w-0'}
        `}
      >
        <div
          className="h-full w-[250px] bg-[var(--color-bg-panel)]/80 border-r border-[var(--color-border)] backdrop-blur-xl flex flex-col overflow-hidden md:absolute md:inset-y-0 md:left-0 md:translate-x-0 md:pointer-events-auto"
        >
          {/* Titlebar row — names whatever the body is currently showing.
              While the panel is open this is the leftmost visible surface, so
              it, and only it, holds the Tauri traffic lights clear; the panel
              collapses to zero width when closed, at which point the clearance
              passes to MainView's titlebar (or, with chat expanded to
              fullscreen and that bar collapsed too, ChatViewer's header). */}
          <div className={`h-[52px] flex items-center px-3 flex-shrink-0 gap-1 ${isTauri() ? 'pl-[78px]' : ''}`} data-tauri-drag-region>
            <span className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider truncate select-none">
              {SIDEBAR_CONTEXT_TITLES[context.kind]}
            </span>
          </div>

          {/* Contextual body */}
          <div className="flex-1 overflow-hidden">
            <SidebarBody context={context} />
          </div>
        </div>
      </aside>
    </>
  );
}
