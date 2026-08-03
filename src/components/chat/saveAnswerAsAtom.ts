import type { ChatCitation, CitationSourceType } from '../../stores/chat';
import { useAtomsStore, type AtomWithTags } from '../../stores/atoms';
import { useTagsStore, type TagWithCount } from '../../stores/tags';

/// Every saved answer carries this tag: the visible, filterable marker that a
/// note was written by the assistant rather than by hand.
export const CHAT_ANSWERS_TAG_NAME = 'Chat Answers';

const MAX_TITLE_LENGTH = 80;
const FALLBACK_TITLE = 'Chat answer';

export interface AnswerAtomContentInput {
  /// Raw assistant markdown. Never the rendered DOM: a `[N]` marker only means
  /// something alongside the citations array it was emitted with.
  answer: string;
  citations: ChatCitation[];
  /// The question that produced the answer; its first line becomes the title.
  question?: string | null;
  conversationTitle?: string | null;
}

/// Assemble the markdown for an atom made from a chat answer.
///
/// Citation markers are resolved rather than carried over: an answer that
/// keeps saying `[3]` in the knowledge base points at nothing. Atom citations
/// become `[[atom-id]]` links — the server extracts those into real atom links,
/// which is what wires the saved answer into the graph — and citations of
/// wikis and report findings, which have no atom to link to, lose their inline
/// marker and survive only in the Sources list.
export function buildAnswerAtomContent({
  answer,
  citations,
  question,
  conversationTitle,
}: AnswerAtomContentInput): string {
  const byIndex = new Map(citations.map((citation) => [citation.citation_index, citation]));
  const sections = [
    `# ${answerTitle(question, conversationTitle)}`,
    convertCitationMarkers(answer.trim(), byIndex),
    buildSourcesSection(citations),
  ];
  return sections.filter((section) => section.length > 0).join('\n\n');
}

/// Turn an assistant answer into an atom. The ordinary create pipeline takes
/// it from there: chunking, embedding, auto-tagging, and link extraction all
/// follow from the write, so nothing here is special-cased.
export async function saveAnswerAsAtom(input: AnswerAtomContentInput): Promise<AtomWithTags> {
  const content = buildAnswerAtomContent(input);
  const tagId = await findOrCreateChatAnswersTag();
  return useAtomsStore.getState().createAtom(content, undefined, [tagId]);
}

function sourceTypeOf(citation: ChatCitation): CitationSourceType {
  return citation.source_type ?? 'atom';
}

/// A fence line: up to three spaces of indent, then three or more backticks
/// or tildes.
const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})(.*)$/;
/// A closing fence: the same run of the same character, nothing after it.
const FENCE_CLOSE = /^ {0,3}(`{3,}|~{3,})[ \t]*$/;
/// A run of backticks delimiting an inline span, closed by a run of the same
/// length — `` `a ` b` `` is one span, not two.
const INLINE_CODE = /(`+)[^]*?\1(?!`)/g;

/// Half-open `[start, end)` ranges of `markdown` that are code: fenced blocks
/// first, then inline spans in the prose between them.
///
/// Citation markers are the assistant's, written into its own prose. The same
/// characters inside a code block are the *user's content* — `items[1]` in a
/// snippet, a `[2]` in a config sample — and rewriting them silently corrupts
/// the atom the answer was saved as.
function codeRanges(markdown: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];

  let position = 0;
  let open: { marker: string; start: number } | null = null;
  for (const line of markdown.split('\n')) {
    const lineStart = position;
    position += line.length + 1; // the '\n' split consumed
    const match = FENCE_OPEN.exec(line);
    if (!match) continue;

    if (!open) {
      const [, marker, info] = match;
      // A line that closes its own run — ```items[1]``` at the start of a
      // sentence — is a code span, not a fence. Treating it as one would
      // swallow the rest of the answer; the inline-span pass below picks it
      // up instead.
      if (info.includes(marker)) continue;
      open = { marker, start: lineStart };
    } else if (
      FENCE_CLOSE.test(line) &&
      match[1][0] === open.marker[0] &&
      match[1].length >= open.marker.length
    ) {
      ranges.push([open.start, Math.min(position, markdown.length)]);
      open = null;
    }
  }
  // An unclosed fence runs to the end of the document, the way a renderer
  // treats it.
  if (open) ranges.push([open.start, markdown.length]);

  const spans: Array<[number, number]> = [];
  const gaps: Array<[number, number]> = [
    ...ranges,
    [markdown.length, markdown.length] as [number, number],
  ];
  let cursor = 0;
  for (const [start, end] of gaps) {
    const gap = markdown.slice(cursor, start);
    for (const span of gap.matchAll(INLINE_CODE)) {
      const at = cursor + (span.index ?? 0);
      spans.push([at, at + span[0].length]);
    }
    cursor = end;
  }

  return [...ranges, ...spans].sort((a, b) => a[0] - b[0]);
}

/// Whether the bracketed number at `[start, end)` is part of a link rather
/// than a citation marker: the reference half of `[text][1]`, an inline link
/// `[1](url)`, or a definition `[1]: url`. Rewriting any of those breaks the
/// link.
function isLinkSyntax(markdown: string, start: number, end: number): boolean {
  const previous = markdown[start - 1];
  if (previous === ']' || previous === '[') return true;
  const next = markdown[end];
  if (next === '(' || next === '[') return true;
  if (next !== ':') return false;
  // A definition only counts at the start of a line — "cited as [1]: here"
  // is prose, and the marker in it is ours.
  const lineStart = markdown.lastIndexOf('\n', start - 1) + 1;
  return /^ {0,3}$/.test(markdown.slice(lineStart, start));
}

function convertCitationMarkers(markdown: string, byIndex: Map<number, ChatCitation>): string {
  // The space before a marker is part of the match so dropping a non-atom
  // marker doesn't leave "the answer ." behind.
  const rewrite = (prose: string, base: number): string =>
    prose.replace(/([ \t]?)\[(\d+)\]/g, (marker, space: string, index: string, at: number) => {
      const bracket = base + at + space.length;
      if (isLinkSyntax(markdown, bracket, bracket + marker.length - space.length)) return marker;
      const citation = byIndex.get(Number(index));
      // A marker with no citation behind it isn't ours to rewrite — the answer
      // may simply have written a bracketed number.
      if (!citation) return marker;
      if (sourceTypeOf(citation) !== 'atom') return '';
      return `${space}[[${citation.atom_id}]]`;
    });

  let out = '';
  let cursor = 0;
  for (const [start, end] of codeRanges(markdown)) {
    // Ranges are sorted but a fence can contain nothing else; guard anyway so
    // an overlapping pair can never duplicate text.
    if (start < cursor) continue;
    out += rewrite(markdown.slice(cursor, start), cursor);
    out += markdown.slice(start, end);
    cursor = end;
  }
  return out + rewrite(markdown.slice(cursor), cursor);
}

function buildSourcesSection(citations: ChatCitation[]): string {
  const seen = new Set<string>();
  const lines: string[] = [];

  for (const citation of [...citations].sort((a, b) => a.citation_index - b.citation_index)) {
    const key = `${sourceTypeOf(citation)}:${citation.atom_id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    lines.push(`- ${sourceLine(citation)}`);
  }

  if (lines.length === 0) return '';
  return ['## Sources', ...lines].join('\n');
}

function sourceLine(citation: ChatCitation): string {
  switch (sourceTypeOf(citation)) {
    case 'wiki':
      return `Wiki: ${firstLine(citation.source_title) || 'Untitled article'}`;
    case 'finding':
      return `Report finding: ${firstLine(citation.source_title) || firstLine(citation.excerpt) || 'Untitled finding'}`;
    default:
      return `[[${citation.atom_id}]]`;
  }
}

function answerTitle(question?: string | null, conversationTitle?: string | null): string {
  return firstLine(question) || firstLine(conversationTitle) || FALLBACK_TITLE;
}

function firstLine(text?: string | null, max = MAX_TITLE_LENGTH): string {
  const line = (text ?? '').trim().split('\n')[0].trim();
  if (line.length <= max) return line;
  return `${line.slice(0, max).trimEnd()}...`;
}

/// Resolve the provenance tag, creating it the first time an answer is saved.
async function findOrCreateChatAnswersTag(): Promise<string> {
  const cached = matchChatAnswersTag(useTagsStore.getState().tags);
  if (cached) return cached.id;

  // A miss can just mean the tree hasn't loaded yet — confirm against a fresh
  // tree before creating, or every cold start would mint another
  // "Chat Answers".
  await useTagsStore.getState().fetchTags();
  const fresh = matchChatAnswersTag(useTagsStore.getState().tags);
  if (fresh) return fresh.id;

  try {
    const created = await useTagsStore.getState().createTag(CHAT_ANSWERS_TAG_NAME);
    return created.id;
  } catch (error) {
    // Creation is the racy step: SQLite makes tag names globally unique
    // (case-insensitively), so another client — or this one, a moment ago —
    // may have taken the name since the fetch above. The name is the identity
    // here, so re-resolving is the right answer to "already exists".
    await useTagsStore.getState().fetchTags();
    const existing = matchChatAnswersTag(useTagsStore.getState().tags);
    if (existing) return existing.id;
    throw error;
  }
}

/// Find the provenance tag wherever the user has since filed it. Matching on
/// name alone is deliberate: the tag is created at the root, but a user who
/// nests it under, say, "Meta" has renamed nothing — and on SQLite the name is
/// unique across the whole tree, so a parent-anchored lookup would miss it
/// forever and every save would fail on the duplicate name.
function matchChatAnswersTag(tags: TagWithCount[]): TagWithCount | undefined {
  const wanted = CHAT_ANSWERS_TAG_NAME.toLowerCase();
  for (const tag of tags) {
    if (tag.name.trim().toLowerCase() === wanted) return tag;
    const nested = matchChatAnswersTag(tag.children);
    if (nested) return nested;
  }
  return undefined;
}
