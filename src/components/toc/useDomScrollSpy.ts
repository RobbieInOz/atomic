import { RefObject, useEffect, useState } from 'react';
import type { TocItem } from './useTocItems';

/** Distance below the container's top edge at which a heading counts as reached. */
const DEFAULT_TOP_MARGIN = 80;

/**
 * Vertical position of `el` measured against `container`'s scroll origin.
 *
 * `offsetTop` would only be right when the scroll container happens to be the
 * element's `offsetParent`; rendered markdown nests headings several wrappers
 * deep, so measure with rects instead.
 */
export function offsetWithinContainer(container: HTMLElement, el: HTMLElement): number {
  return (
    el.getBoundingClientRect().top -
    container.getBoundingClientRect().top +
    container.scrollTop
  );
}

function activeItemId(
  container: HTMLElement,
  items: TocItem[],
  topMargin: number,
): string | null {
  const threshold = container.scrollTop + topMargin;
  let active: string | null = null;
  for (const item of items) {
    const el = container.ownerDocument.getElementById(item.id);
    if (!el) continue;
    if (offsetWithinContainer(container, el) > threshold) break;
    active = item.id;
  }
  return active;
}

/**
 * Track which heading the reader is currently under, for DOM-rendered
 * markdown. Headings are located by their injected element ids.
 */
export function useDomScrollSpy(
  containerRef: RefObject<HTMLElement | null>,
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
    const measure = () => {
      frame = 0;
      setActiveId(activeItemId(container, items, topMargin));
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
  }, [containerRef, items, topMargin]);

  return activeId;
}
