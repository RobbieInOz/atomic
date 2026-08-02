import { useEffect } from 'react';
import { useTocStore, type TocSource } from '../../stores/toc';
import type { TocItem } from './useTocItems';

interface UseTocPublisherArgs {
  /** `null` while the reader has no document on screen (loading, empty state,
   *  a replaced body) — the panel then shows nothing rather than claiming the
   *  document has no headings. */
  source: TocSource | null;
  items: TocItem[];
  activeId: string | null;
  scrollToItem: (item: TocItem) => void;
}

/**
 * Hand the mounted reader's outline to the left sidebar for as long as the
 * reader is showing the document it describes.
 *
 * Publishing and withdrawing are separate effects on purpose: the outline is
 * cleared on unmount, not on every change, so a re-parse while the user types
 * updates the panel in place rather than blanking it for a frame.
 */
export function useTocPublisher({ source, items, activeId, scrollToItem }: UseTocPublisherArgs): void {
  const publish = useTocStore((s) => s.publish);
  const setActiveId = useTocStore((s) => s.setActiveId);
  const clear = useTocStore((s) => s.clear);

  useEffect(() => {
    if (source === null) {
      clear();
      return;
    }
    publish({ source, items, scrollToItem });
  }, [publish, clear, source, items, scrollToItem]);

  useEffect(() => {
    setActiveId(activeId);
  }, [setActiveId, activeId]);

  useEffect(() => clear, [clear]);
}
