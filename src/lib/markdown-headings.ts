import { fromMarkdown } from 'mdast-util-from-markdown';
import { gfmFromMarkdown } from 'mdast-util-gfm';
import { gfm } from 'micromark-extension-gfm';
import { stripMatchMarkers } from '../components/search-palette/markdownToPlainText';

/** A heading found in a markdown source, in document order. */
export interface HeadingEntry {
  /** 1-6, as written (`#` → 1). Setext headings report 1 or 2. */
  depth: number;
  /** Rendered text of the heading, inline markup removed. */
  text: string;
  /** Character offset of the heading's first character in the source. */
  offset: number;
  /** 1-based line number of the heading's first character. */
  line: number;
  /** URL/DOM-safe id, unique within the returned list. */
  slug: string;
}

/** Minimal structural view of the mdast nodes this module touches. */
interface MdastNode {
  type: string;
  value?: string;
  depth?: number;
  position?: { start?: { offset?: number; line?: number } };
  children?: MdastNode[];
}

function collectHeadingNodes(node: MdastNode, out: MdastNode[]): void {
  if (node.type === 'heading') {
    // Headings cannot nest, so there is nothing below one to descend into.
    out.push(node);
    return;
  }
  if (node.children) {
    for (const child of node.children) collectHeadingNodes(child, out);
  }
}

function collectInlineText(node: MdastNode, out: string[]): void {
  if (node.type === 'text' || node.type === 'inlineCode') {
    if (typeof node.value === 'string') out.push(node.value);
    return;
  }
  if (node.type === 'break') {
    out.push(' ');
    return;
  }
  if (node.children) {
    for (const child of node.children) collectInlineText(child, out);
  }
}

function headingText(node: MdastNode): string {
  const parts: string[] = [];
  collectInlineText(node, parts);
  return stripMatchMarkers(parts.join('')).replace(/\s+/g, ' ').trim();
}

/**
 * Lowercase, collapse every run of non-alphanumerics to a single dash, trim
 * dashes. Unicode letters and digits count as alphanumeric so notes written
 * in non-Latin scripts still get meaningful slugs instead of all colliding
 * on the fallback.
 */
function slugify(text: string): string {
  const slug = text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '');
  return slug.length > 0 ? slug : 'section';
}

function uniqueSlug(
  base: string,
  counts: Map<string, number>,
  taken: Set<string>,
): string {
  let seen = counts.get(base) ?? 0;
  let candidate = seen === 0 ? base : `${base}-${seen + 1}`;
  // A literal heading may already own the suffixed form ("Setup" + "Setup 2"),
  // so keep bumping until the candidate is genuinely free.
  while (taken.has(candidate)) {
    seen += 1;
    candidate = `${base}-${seen + 1}`;
  }
  counts.set(base, seen + 1);
  taken.add(candidate);
  return candidate;
}

/**
 * Extract the heading outline of a markdown document.
 *
 * Parsing (rather than scanning for `^#`) is what makes `#` lines inside
 * fenced code blocks, front matter, and indented code disappear for free —
 * they never become `heading` nodes.
 */
export function parseHeadings(source: string): HeadingEntry[] {
  let tree: MdastNode;
  try {
    tree = fromMarkdown(source, {
      extensions: [gfm()],
      mdastExtensions: [gfmFromMarkdown()],
    }) as MdastNode;
  } catch {
    return [];
  }

  const nodes: MdastNode[] = [];
  collectHeadingNodes(tree, nodes);

  const entries: HeadingEntry[] = [];
  const slugCounts = new Map<string, number>();
  const takenSlugs = new Set<string>();
  for (const node of nodes) {
    const offset = node.position?.start?.offset;
    const line = node.position?.start?.line;
    if (offset === undefined || line === undefined) continue;
    const text = headingText(node);
    entries.push({
      depth: node.depth ?? 1,
      text,
      offset,
      line,
      slug: uniqueSlug(slugify(text), slugCounts, takenSlugs),
    });
  }
  return entries;
}
