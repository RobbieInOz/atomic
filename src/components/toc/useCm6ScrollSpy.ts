import { RefObject, useEffect, useState } from 'react';
import type { EditorView } from '@codemirror/view';
import type { TocEditorViewRef } from '../../editor/toc-scroll';
import type { TocItem } from './useTocItems';

/** Distance below the container's top edge at which a heading counts as reached. */
const DEFAULT_TOP_MARGIN = 80;

/** ~1s at 60fps — long enough for the lazy editor chunk to mount, short
 *  enough that a view which never arrives doesn't spin a frame loop. */
const VIEW_WAIT_FRAMES = 60;

/**
 * Vertical position of a document offset, measured against `container`'s
 * scroll origin.
 *
 * Positions come from CM6's height map rather than the DOM because the lines
 * in question are usually not rendered. Rows outside the measured range are
 * estimated, which is more than accurate enough to decide which heading the
 * reader is under.
 */
function blockTopInContainer(
  view: EditorView,
  container: HTMLElement,
  offset: number,
): number {
  const pos = Math.max(0, Math.min(offset, view.state.doc.length));
  return (
    view.documentTop +
    view.lineBlockAt(pos).top -
    container.getBoundingClientRect().top +
    container.scrollTop
  );
}

function activeItemId(
  view: EditorView,
  container: HTMLElement,
  items: TocItem[],
  topMargin: number,
): string | null {
  const threshold = container.scrollTop + topMargin;
  let active: string | null = null;
  for (const item of items) {
    if (blockTopInContainer(view, container, item.offset) > threshold) break;
    active = item.id;
  }
  return active;
}

/**
 * Track which heading the reader is currently under, for content rendered by
 * CodeMirror. `containerRef` is the element that actually scrolls — in the
 * readers that is the page-level scroller wrapping the editor, not
 * `view.scrollDOM`.
 */
export function useCm6ScrollSpy(
  containerRef: RefObject<HTMLElement | null>,
  viewRef: TocEditorViewRef,
  items: TocItem[],
  topMargin = DEFAULT_TOP_MARGIN,
): string | null {
  const [activeId, setActiveId] = useState<string | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || items.length === 0) {
      setActiveId(null);
      return;
    }

    let frame = 0;
    let framesWaited = 0;
    const measure = () => {
      frame = 0;
      const view = viewRef.current;
      if (!view) {
        // The editor mounts behind a lazy() boundary, so the view can appear
        // several frames after this effect runs.
        if (framesWaited++ < VIEW_WAIT_FRAMES) frame = requestAnimationFrame(measure);
        return;
      }
      setActiveId(activeItemId(view, container, items, topMargin));
    };
    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(measure);
    };

    measure();
    container.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      container.removeEventListener('scroll', onScroll);
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  }, [containerRef, viewRef, items, topMargin]);

  return activeId;
}
