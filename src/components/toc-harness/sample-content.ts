// Fixtures for the table-of-contents harness. Each one isolates a case the
// TOC has to get right: a long note where CodeMirror virtualization hides
// most headings from the DOM, a wiki-shaped article whose title lives in the
// body, and a note with nothing to outline at all.

const FILLER_WORDS = [
  'atom', 'graph', 'tag', 'embedding', 'vector', 'semantic', 'reader', 'wiki',
  'canvas', 'chat', 'agent', 'retrieval', 'chunk', 'similarity', 'index',
  'query', 'markdown', 'editor', 'viewport', 'render', 'prose', 'block',
  'paragraph', 'heading', 'outline', 'anchor', 'offset', 'scroll', 'sidebar',
  'pipeline', 'transport', 'facade', 'store', 'callback', 'payload', 'cursor',
];

/** Deterministic filler so the fixtures are byte-stable across reloads. */
function paragraph(seed: number, wordCount = 44): string {
  const words: string[] = [];
  for (let i = 0; i < wordCount; i++) {
    words.push(FILLER_WORDS[(seed * 7 + i * 13) % FILLER_WORDS.length]);
  }
  const body = words.join(' ');
  return `${body.charAt(0).toUpperCase()}${body.slice(1)}.`;
}

const LONG_DOC_TOPICS = [
  'Ingestion',
  'Chunking',
  'Embedding',
  'Auto-tagging',
  'Semantic edges',
  'Wiki synthesis',
  'Agentic chat',
  'Canvas layout',
  'Scheduled tasks',
  'Feed polling',
  'MCP surface',
  'Migration',
];

function buildLongDoc(): string {
  const lines: string[] = [
    '# Pipeline Field Notes',
    '',
    paragraph(0),
    '',
  ];

  LONG_DOC_TOPICS.forEach((topic, i) => {
    lines.push(`## ${topic}`, '', paragraph(i + 1), '');

    // Repeated verbatim in every section — the duplicate-slug case, and the
    // reason clicks have to travel by offset rather than by text match.
    lines.push('### Implementation notes', '', paragraph(i + 11), '');

    // Fenced code whose lines start with `#`. A regex outline would list
    // these; a parsed one never sees them.
    if (i % 2 === 0) {
      lines.push(
        '```bash',
        '# Not a heading — a shell comment',
        `atomic ${topic.toLowerCase().replace(/\s+/g, '-')} --dry-run`,
        '## Not a heading either',
        '```',
        '',
      );
    } else {
      lines.push(
        '```python',
        '# Not a heading — a Python comment',
        `def ${topic.toLowerCase().replace(/[^a-z]+/g, '_')}(atoms):`,
        '    return [a for a in atoms if a.embedding is not None]',
        '```',
        '',
      );
    }

    lines.push('#### Edge cases', '', paragraph(i + 21, 30), '');
    lines.push(
      `- ${paragraph(i + 31, 9)}`,
      `- ${paragraph(i + 41, 11)}`,
      `- ${paragraph(i + 51, 8)}`,
      '',
    );

    lines.push('### Open questions', '', paragraph(i + 61), '');
  });

  lines.push('## Implementation notes', '', paragraph(99), '');
  lines.push('## Appendix', '', paragraph(101), '');
  return lines.join('\n');
}

const LONG_DOC = buildLongDoc();

const WIKI_DOC = [
  '# Distributed Systems',
  '',
  paragraph(3),
  '',
  '## Consensus',
  '',
  paragraph(4),
  '',
  '### Raft',
  '',
  paragraph(5),
  '',
  '### Paxos',
  '',
  paragraph(6),
  '',
  '## Replication',
  '',
  paragraph(7),
  '',
  '### Leader-follower',
  '',
  paragraph(8),
  '',
  '### Quorum reads',
  '',
  paragraph(9),
  '',
  '## Failure modes',
  '',
  paragraph(10),
  '',
].join('\n');

const HEADINGLESS_DOC = [
  paragraph(12),
  '',
  paragraph(13),
  '',
  '- ' + paragraph(14, 10),
  '- ' + paragraph(15, 12),
  '',
  '```json',
  '{ "note": "# this is not a heading" }',
  '```',
  '',
  paragraph(16),
  '',
].join('\n');

export type TocFixtureId = 'long' | 'wiki' | 'headingless';

export interface TocFixture {
  id: TocFixtureId;
  label: string;
  source: string;
}

export const TOC_FIXTURES: TocFixture[] = [
  { id: 'long', label: 'long note', source: LONG_DOC },
  { id: 'wiki', label: 'wiki article', source: WIKI_DOC },
  { id: 'headingless', label: 'no headings', source: HEADINGLESS_DOC },
];
