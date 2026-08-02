import { useShallow } from 'zustand/react/shallow';
import { useUIStore, type ViewMode } from '../../stores/ui';

/// What the left sidebar shows for the current navigation state. The panel's
/// open/closed state is never derived — only its content.
export type SidebarContext =
  | { kind: 'tags' }
  | { kind: 'toc'; source: 'atom' | 'wiki' | 'finding' }
  | { kind: 'wiki-list' }
  | { kind: 'recent' }
  | { kind: 'findings'; reportId: string | null };

/// The slices `deriveSidebarContext` reads. Structurally a subset of the UI
/// store, so the store's state can be passed straight in.
export interface SidebarContextInput {
  localGraph: { isOpen: boolean };
  readerState: { atomId: string | null };
  wikiReaderState: { tagId: string | null };
  findingReaderState: { atomId: string | null };
  reportsDetailState: { reportId: string | null };
  viewMode: ViewMode;
}

export const SIDEBAR_CONTEXT_TITLES: Record<SidebarContext['kind'], string> = {
  tags: 'Tags',
  toc: 'Contents',
  'wiki-list': 'Articles',
  recent: 'Recent',
  findings: 'Findings',
};

/// Decide the sidebar's content from the projected reader slices.
///
/// The branch order mirrors MainView's content ternary chain (the
/// `localGraph.isOpen ? … : readerState.atomId ? …` cascade) exactly, and
/// reads the same fields, so the sidebar and the main view can never disagree
/// about what the user is looking at. Change one, change the other.
///
/// MainView additionally guards on `localGraph.centerAtomId` and
/// `wikiReaderState.tagName`; those are written atomically with the ids they
/// accompany by `projectActiveEntry`, so testing the ids alone is equivalent.
export function deriveSidebarContext(s: SidebarContextInput): SidebarContext {
  if (s.localGraph.isOpen) return { kind: 'tags' };
  if (s.readerState.atomId) return { kind: 'toc', source: 'atom' };
  if (s.wikiReaderState.tagId) return { kind: 'toc', source: 'wiki' };
  if (s.findingReaderState.atomId) return { kind: 'toc', source: 'finding' };
  // A report tab is open: show that report's findings so the user can walk
  // its siblings without leaving the detail view.
  if (s.reportsDetailState.reportId) {
    return { kind: 'findings', reportId: s.reportsDetailState.reportId };
  }
  switch (s.viewMode) {
    case 'wiki':
      return { kind: 'wiki-list' };
    case 'dashboard':
      return { kind: 'recent' };
    case 'reports':
      return { kind: 'findings', reportId: null };
    case 'atoms':
    case 'canvas':
      return { kind: 'tags' };
  }
}

/// Subscribe to the sidebar context. Shallow equality on the derived value
/// means consumers only re-render when the context actually changes, not on
/// every unrelated UI-store write.
export function useSidebarContext(): SidebarContext {
  return useUIStore(useShallow((s) => deriveSidebarContext(s)));
}
