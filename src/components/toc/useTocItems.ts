import { useEffect, useRef, useState } from 'react';
import { parseHeadings } from '../../lib/markdown-headings';

/**
 * A heading as the table of contents consumes it. `id` is the parsed slug —
 * unique within a document, and the same string readers inject as the DOM id
 * on rendered headings so a scroll spy can find them again.
 */
export interface TocItem {
  id: string;
  depth: number;
  text: string;
  offset: number;
  line: number;
}

function tocItemsFrom(source: string): TocItem[] {
  return parseHeadings(source).map((heading) => ({
    id: heading.slug,
    depth: heading.depth,
    text: heading.text,
    offset: heading.offset,
    line: heading.line,
  }));
}

/**
 * Table-of-contents items for a markdown source.
 *
 * The first parse is synchronous so a reader never flashes an empty outline
 * on open; later changes are debounced because the live editor calls this on
 * every keystroke and a re-parse of a long note is not free.
 */
export function useTocItems(source: string, debounceMs = 300): TocItem[] {
  const [items, setItems] = useState<TocItem[]>(() => tocItemsFrom(source));
  const parsedSourceRef = useRef(source);

  useEffect(() => {
    if (parsedSourceRef.current === source) return;
    const timer = window.setTimeout(() => {
      parsedSourceRef.current = source;
      setItems(tocItemsFrom(source));
    }, debounceMs);
    return () => window.clearTimeout(timer);
  }, [source, debounceMs]);

  return items;
}
