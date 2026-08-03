import { describe, it, expect } from 'vitest';
import { buildAnswerAtomContent } from './saveAnswerAsAtom';
import type { ChatCitation } from '../../stores/chat';

const ATOM_A = '11111111-1111-4111-8111-111111111111';
const ATOM_B = '22222222-2222-4222-8222-222222222222';

function citation(overrides: Partial<ChatCitation> & { citation_index: number }): ChatCitation {
  return {
    id: `citation-${overrides.citation_index}`,
    message_id: 'm1',
    atom_id: ATOM_A,
    chunk_index: null,
    excerpt: 'an excerpt',
    relevance_score: null,
    ...overrides,
  };
}

describe('buildAnswerAtomContent', () => {
  it('converts atom markers to atom links and lists them as sources', () => {
    const content = buildAnswerAtomContent({
      answer: 'Rust owns memory [1].',
      citations: [citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A })],
      question: 'How does Rust manage memory?',
    });

    expect(content).toBe(
      [
        '# How does Rust manage memory?',
        '',
        `Rust owns memory [[${ATOM_A}]].`,
        '',
        '## Sources',
        `- [[${ATOM_A}]]`,
      ].join('\n'),
    );
  });

  it('treats a citation with no source type as an atom', () => {
    const content = buildAnswerAtomContent({
      answer: 'Legacy citation [1].',
      citations: [citation({ citation_index: 1, atom_id: ATOM_B })],
      question: 'q',
    });

    expect(content).toContain(`Legacy citation [[${ATOM_B}]].`);
    expect(content).toContain(`- [[${ATOM_B}]]`);
  });

  it('strips wiki and finding markers inline but keeps them in Sources', () => {
    const content = buildAnswerAtomContent({
      answer: 'The article says so [1], and the report agrees [2].',
      citations: [
        citation({ citation_index: 1, source_type: 'wiki', source_title: 'Rust' }),
        citation({
          citation_index: 2,
          source_type: 'finding',
          source_title: null,
          excerpt: 'Weekly digest\nsecond line',
        }),
      ],
      question: 'q',
    });

    expect(content).toContain('The article says so, and the report agrees.');
    expect(content).toContain('- Wiki: Rust');
    expect(content).toContain('- Report finding: Weekly digest');
  });

  it('handles a mix of source types in one answer', () => {
    const content = buildAnswerAtomContent({
      answer: 'Both the note [1] and the article [2] say so, and so does [3].',
      citations: [
        citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A }),
        citation({ citation_index: 2, source_type: 'wiki', source_title: 'Ownership' }),
        citation({ citation_index: 3, source_type: 'atom', atom_id: ATOM_B }),
      ],
      question: 'q',
    });

    expect(content).toContain(
      `Both the note [[${ATOM_A}]] and the article say so, and so does [[${ATOM_B}]].`,
    );
    expect(content.split('\n').slice(-3)).toEqual([
      `- [[${ATOM_A}]]`,
      '- Wiki: Ownership',
      `- [[${ATOM_B}]]`,
    ]);
  });

  it('lists each cited source once, in citation order', () => {
    const content = buildAnswerAtomContent({
      answer: 'Twice [2] over [1] and again [2].',
      citations: [
        citation({ citation_index: 2, source_type: 'atom', atom_id: ATOM_B }),
        citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A }),
        citation({ citation_index: 2, source_type: 'atom', atom_id: ATOM_B }),
      ],
      question: 'q',
    });

    expect(content.split('\n').slice(-2)).toEqual([`- [[${ATOM_A}]]`, `- [[${ATOM_B}]]`]);
  });

  it('leaves a bracketed number alone when no citation backs it', () => {
    const content = buildAnswerAtomContent({
      answer: 'Item [7] of the list.',
      citations: [],
      question: 'q',
    });

    expect(content).toBe('# q\n\nItem [7] of the list.');
  });

  it('omits the Sources section when there are no citations', () => {
    const content = buildAnswerAtomContent({
      answer: 'No sources here.',
      citations: [],
      question: 'What is this?',
    });

    expect(content).toBe('# What is this?\n\nNo sources here.');
    expect(content).not.toContain('## Sources');
  });

  it('leaves bracketed numbers inside a fenced code block alone', () => {
    const content = buildAnswerAtomContent({
      answer: [
        'Index it like this [1]:',
        '',
        '```ts',
        'const first = items[1];',
        'const second = items[2];',
        '```',
        '',
        'That is the pattern [2].',
      ].join('\n'),
      citations: [
        citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A }),
        citation({ citation_index: 2, source_type: 'wiki', source_title: 'Arrays' }),
      ],
      question: 'q',
    });

    expect(content).toContain('const first = items[1];');
    expect(content).toContain('const second = items[2];');
    expect(content).toContain(`Index it like this [[${ATOM_A}]]:`);
    expect(content).toContain('That is the pattern.');
  });

  it('handles tilde fences, info strings, and an unclosed fence', () => {
    const fenced = buildAnswerAtomContent({
      answer: ['~~~python', 'print(items[1])', '~~~', '', 'Done [1].'].join('\n'),
      citations: [citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A })],
      question: 'q',
    });
    expect(fenced).toContain('print(items[1])');
    expect(fenced).toContain(`Done [[${ATOM_A}]].`);

    // A fence the answer never closed runs to the end, the way a renderer
    // reads it — so nothing after it is prose to rewrite. (The Sources list
    // still names the citation; only the inline marker is left alone.)
    const unclosed = buildAnswerAtomContent({
      answer: ['```', 'items[1]', 'still code [1]'].join('\n'),
      citations: [citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A })],
      question: 'q',
    });
    expect(unclosed).toBe(
      ['# q', '', '```', 'items[1]', 'still code [1]', '', '## Sources', `- [[${ATOM_A}]]`].join(
        '\n',
      ),
    );
  });

  it('leaves bracketed numbers inside inline code spans alone', () => {
    const content = buildAnswerAtomContent({
      answer: 'Use `items[1]` and ``a ` items[2]`` — then cite [1] and [2].',
      citations: [
        citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A }),
        citation({ citation_index: 2, source_type: 'wiki', source_title: 'Arrays' }),
      ],
      question: 'q',
    });

    expect(content).toContain('`items[1]`');
    expect(content).toContain('``a ` items[2]``');
    expect(content).toContain(`then cite [[${ATOM_A}]] and.`);
  });

  it('treats a line that closes its own backtick run as a span, not a fence', () => {
    // Without this, ```x``` opening a sentence reads as an unclosed fence and
    // swallows the rest of the answer — nothing after it would be rewritten.
    const content = buildAnswerAtomContent({
      answer: ['```items[1]``` is the syntax [1].', '', 'More prose [1].'].join('\n'),
      citations: [citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A })],
      question: 'q',
    });

    expect(content).toContain('```items[1]```');
    expect(content).toContain(`is the syntax [[${ATOM_A}]].`);
    expect(content).toContain(`More prose [[${ATOM_A}]].`);
  });

  it('protects code inside a fence nested in a longer fence', () => {
    const content = buildAnswerAtomContent({
      answer: ['````md', '```', 'items[1]', '```', '````', '', 'cite [1]'].join('\n'),
      citations: [citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A })],
      question: 'q',
    });

    expect(content).toContain('```\nitems[1]\n```');
    expect(content).toContain(`cite [[${ATOM_A}]]`);
  });

  it('leaves reference-style links intact', () => {
    const content = buildAnswerAtomContent({
      answer: [
        'See [the docs][1] and [2](https://example.com), cited as [1].',
        '',
        '[1]: https://example.com/docs',
      ].join('\n'),
      citations: [
        citation({ citation_index: 1, source_type: 'atom', atom_id: ATOM_A }),
        citation({ citation_index: 2, source_type: 'atom', atom_id: ATOM_B }),
      ],
      question: 'q',
    });

    expect(content).toContain('See [the docs][1] and [2](https://example.com)');
    expect(content).toContain(`cited as [[${ATOM_A}]].`);
    expect(content).toContain('[1]: https://example.com/docs');
  });

  it('titles the atom from the question, then the conversation, then a constant', () => {
    const base = { answer: 'Answer.', citations: [] };

    expect(
      buildAnswerAtomContent({ ...base, question: 'First line\nsecond line', conversationTitle: 'Conv' }),
    ).toContain('# First line');
    expect(buildAnswerAtomContent({ ...base, question: '   ', conversationTitle: 'Conv' })).toContain(
      '# Conv',
    );
    expect(buildAnswerAtomContent({ ...base, question: null, conversationTitle: null })).toContain(
      '# Chat answer',
    );
  });

  it('truncates a long question to a title-sized first line', () => {
    const question = `${'word '.repeat(40)}end`;
    const title = buildAnswerAtomContent({ answer: 'a', citations: [], question }).split('\n')[0];

    expect(title.startsWith('# word word')).toBe(true);
    expect(title.endsWith('...')).toBe(true);
    // "# " + 80 chars of question (trailing space trimmed) + the ellipsis.
    expect(title.length).toBeLessThanOrEqual(2 + 80 + 3);
  });
});
