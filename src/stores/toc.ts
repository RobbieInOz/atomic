import { create } from 'zustand';
import type { TocItem } from '../components/toc/useTocItems';

export type { TocItem };

/// Which reader an outline came from — the same set `SidebarContext`'s `toc`
/// kind carries, so the sidebar can tell whether what's published belongs to
/// the reader it is currently rendering for.
export type TocSource = 'atom' | 'wiki' | 'finding';

interface TocPublication {
  source: TocSource;
  items: TocItem[];
  scrollToItem: (item: TocItem) => void;
}

interface TocStore {
  /// The reader that owns the outline. `null` when no reader is mounted, in
  /// which case `items` is empty and `scrollToItem` does nothing.
  source: TocSource | null;
  items: TocItem[];
  activeId: string | null;
  scrollToItem: (item: TocItem) => void;

  publish: (publication: TocPublication) => void;
  setActiveId: (id: string | null) => void;
  clear: () => void;
}

const NO_ITEMS: TocItem[] = [];
const noScroll = () => {};

/// The channel between the mounted reader and the left sidebar's table of
/// contents. Deliberately not persisted: an outline only means anything while
/// the document it describes is on screen.
///
/// Readers publish through `useTocPublisher`, which also clears on unmount.
/// React runs an unmounting subtree's cleanups before the replacing subtree's
/// effects, so switching documents always clears before the next publish.
export const useTocStore = create<TocStore>()((set) => ({
  source: null,
  items: NO_ITEMS,
  activeId: null,
  scrollToItem: noScroll,

  publish: ({ source, items, scrollToItem }) => set({ source, items, scrollToItem }),
  setActiveId: (activeId) => set({ activeId }),
  clear: () => set({ source: null, items: NO_ITEMS, activeId: null, scrollToItem: noScroll }),
}));
