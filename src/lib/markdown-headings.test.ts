import { describe, it, expect } from 'vitest';
import { parseHeadings } from './markdown-headings';
import { MATCH_END, MATCH_START } from '../components/search-palette/markdownToPlainText';

describe('parseHeadings', () => {
  it('returns an empty list for empty and headingless sources', () => {
    expect(parseHeadings('')).toEqual([]);
    expect(parseHeadings('Just a paragraph.\n\n- and a list\n- of items\n')).toEqual([]);
  });

  it('reports depth, text, offset and line for ATX headings', () => {
    const source = ['# Title', '', 'Intro prose.', '', '### Deep', ''].join('\n');
    expect(parseHeadings(source)).toEqual([
      { depth: 1, text: 'Title', offset: 0, line: 1, slug: 'title' },
      { depth: 3, text: 'Deep', offset: source.indexOf('### Deep'), line: 5, slug: 'deep' },
    ]);
  });

  it('points offsets at the first character of the heading', () => {
    const source = 'Lead in.\n\n## Second\n\nMore.\n\n#### Fourth\n';
    for (const heading of parseHeadings(source)) {
      expect(source.slice(heading.offset)).toMatch(/^#+ /);
    }
  });

  it('ignores `#` lines inside fenced code blocks', () => {
    const source = [
      '# Real heading',
      '',
      '```bash',
      '# not a heading, just a shell comment',
      'echo hi',
      '```',
      '',
      '```python',
      '## also not a heading',
      '```',
      '',
      '## Second real heading',
      '',
    ].join('\n');
    expect(parseHeadings(source).map((h) => h.text)).toEqual([
      'Real heading',
      'Second real heading',
    ]);
  });

  it('ignores `#` lines inside indented code blocks', () => {
    const source = 'Prose.\n\n    # indented code, not a heading\n\n## Actual\n';
    expect(parseHeadings(source).map((h) => h.text)).toEqual(['Actual']);
  });

  it('parses setext headings', () => {
    const source = 'Big Title\n=========\n\nSmaller\n-------\n';
    expect(parseHeadings(source)).toEqual([
      { depth: 1, text: 'Big Title', offset: 0, line: 1, slug: 'big-title' },
      { depth: 2, text: 'Smaller', offset: source.indexOf('Smaller'), line: 4, slug: 'smaller' },
    ]);
  });

  it('flattens inline formatting into plain text', () => {
    const source = '## The **bold** and `code()` [linked](https://example.com) bit\n';
    const [heading] = parseHeadings(source);
    expect(heading.text).toBe('The bold and code() linked bit');
    expect(heading.slug).toBe('the-bold-and-code-linked-bit');
  });

  it('gives duplicate headings unique slugs while keeping their own offsets', () => {
    const source = '## Setup\n\na\n\n## Setup\n\nb\n\n## Setup\n';
    const headings = parseHeadings(source);
    expect(headings.map((h) => h.slug)).toEqual(['setup', 'setup-2', 'setup-3']);
    expect(headings.map((h) => h.offset)).toEqual([
      0,
      source.indexOf('## Setup', 1),
      source.lastIndexOf('## Setup'),
    ]);
  });

  it('does not collide when a literal heading already owns a suffixed slug', () => {
    const source = '## Setup\n\n## Setup 2\n\n## Setup\n';
    expect(parseHeadings(source).map((h) => h.slug)).toEqual(['setup', 'setup-2', 'setup-3']);
  });

  it('falls back to a stable slug when a heading has no slug-able characters', () => {
    const source = '## ???\n\n## !!!\n';
    expect(parseHeadings(source).map((h) => h.slug)).toEqual(['section', 'section-2']);
  });

  it('strips FTS match markers from heading text and slugs', () => {
    const source = `## Vector ${MATCH_START}search${MATCH_END} notes\n`;
    const [heading] = parseHeadings(source);
    expect(heading.text).toBe('Vector search notes');
    expect(heading.slug).toBe('vector-search-notes');
  });

  it('finds headings nested inside block containers', () => {
    const source = '> ## Quoted heading\n\n- item\n\n  ### Nested heading\n';
    expect(parseHeadings(source).map((h) => h.text)).toEqual([
      'Quoted heading',
      'Nested heading',
    ]);
  });

  it('returns headings in document order with ascending offsets', () => {
    const source = '# A\n\n## B\n\n### C\n\n## D\n';
    const offsets = parseHeadings(source).map((h) => h.offset);
    expect(offsets).toEqual([...offsets].sort((a, b) => a - b));
    expect(new Set(offsets).size).toBe(offsets.length);
  });

  it('keeps unicode headings distinguishable', () => {
    const headings = parseHeadings('## Café\n\n## 日本語\n');
    expect(headings.map((h) => h.text)).toEqual(['Café', '日本語']);
    expect(new Set(headings.map((h) => h.slug)).size).toBe(2);
  });

  it('handles trailing closing hashes and extra whitespace', () => {
    const [heading] = parseHeadings('##   Spaced   out   ##\n');
    expect(heading.text).toBe('Spaced out');
    expect(heading.slug).toBe('spaced-out');
  });
});
