import { describe, it, expect } from 'vitest';
import { deriveSidebarContext, type SidebarContextInput } from './sidebar-context';
import type { ViewMode } from '../../stores/ui';

function input(overrides: Partial<SidebarContextInput> = {}): SidebarContextInput {
  return {
    chatSidebarOpen: false,
    chatSidebarExpanded: false,
    localGraph: { isOpen: false },
    readerState: { atomId: null },
    wikiReaderState: { tagId: null },
    findingReaderState: { atomId: null },
    reportsDetailState: { reportId: null },
    viewMode: 'atoms',
    ...overrides,
  };
}

describe('deriveSidebarContext', () => {
  it('falls back to the tag tree on the atoms and canvas base views', () => {
    expect(deriveSidebarContext(input({ viewMode: 'atoms' }))).toEqual({ kind: 'tags' });
    expect(deriveSidebarContext(input({ viewMode: 'canvas' }))).toEqual({ kind: 'tags' });
  });

  it('maps the remaining base views to their own panels', () => {
    expect(deriveSidebarContext(input({ viewMode: 'wiki' }))).toEqual({ kind: 'wiki-list' });
    expect(deriveSidebarContext(input({ viewMode: 'dashboard' }))).toEqual({ kind: 'recent' });
    expect(deriveSidebarContext(input({ viewMode: 'reports' }))).toEqual({
      kind: 'findings',
      reportId: null,
    });
  });

  it('shows a table of contents for each of the three readers', () => {
    expect(deriveSidebarContext(input({ readerState: { atomId: 'a1' } }))).toEqual({
      kind: 'toc',
      source: 'atom',
    });
    expect(deriveSidebarContext(input({ wikiReaderState: { tagId: 't1' } }))).toEqual({
      kind: 'toc',
      source: 'wiki',
    });
    expect(deriveSidebarContext(input({ findingReaderState: { atomId: 'f1' } }))).toEqual({
      kind: 'toc',
      source: 'finding',
    });
  });

  it('scopes findings to the open report detail tab', () => {
    expect(deriveSidebarContext(input({ reportsDetailState: { reportId: 'r1' } }))).toEqual({
      kind: 'findings',
      reportId: 'r1',
    });
  });

  it('shows conversations while the chat pane is expanded, whatever else is open', () => {
    const fullscreen: SidebarContextInput = input({
      chatSidebarOpen: true,
      chatSidebarExpanded: true,
      localGraph: { isOpen: true },
      readerState: { atomId: 'a1' },
      reportsDetailState: { reportId: 'r1' },
      viewMode: 'wiki',
    });
    expect(deriveSidebarContext(fullscreen)).toEqual({ kind: 'conversations' });
  });

  it('ignores an expanded flag while the chat pane is closed', () => {
    expect(
      deriveSidebarContext(input({ chatSidebarExpanded: true, chatSidebarOpen: false, viewMode: 'wiki' })),
    ).toEqual({ kind: 'wiki-list' });
    expect(
      deriveSidebarContext(input({ chatSidebarExpanded: false, chatSidebarOpen: true, viewMode: 'wiki' })),
    ).toEqual({ kind: 'wiki-list' });
  });

  it('keeps the tag tree while the local graph is open', () => {
    expect(
      deriveSidebarContext(input({ localGraph: { isOpen: true }, viewMode: 'wiki' })),
    ).toEqual({ kind: 'tags' });
  });

  // The projections are mutually exclusive in practice, but the ordering is
  // what guarantees the sidebar and MainView agree if that ever changes.
  it('resolves overlapping reader slices in MainView order', () => {
    const all: SidebarContextInput = input({
      localGraph: { isOpen: true },
      readerState: { atomId: 'a1' },
      wikiReaderState: { tagId: 't1' },
      findingReaderState: { atomId: 'f1' },
      reportsDetailState: { reportId: 'r1' },
      viewMode: 'dashboard',
    });
    expect(deriveSidebarContext(all)).toEqual({ kind: 'tags' });
    expect(deriveSidebarContext({ ...all, localGraph: { isOpen: false } })).toEqual({
      kind: 'toc',
      source: 'atom',
    });
    expect(
      deriveSidebarContext({ ...all, localGraph: { isOpen: false }, readerState: { atomId: null } }),
    ).toEqual({ kind: 'toc', source: 'wiki' });
    expect(
      deriveSidebarContext({
        ...all,
        localGraph: { isOpen: false },
        readerState: { atomId: null },
        wikiReaderState: { tagId: null },
      }),
    ).toEqual({ kind: 'toc', source: 'finding' });
    expect(
      deriveSidebarContext({
        ...all,
        localGraph: { isOpen: false },
        readerState: { atomId: null },
        wikiReaderState: { tagId: null },
        findingReaderState: { atomId: null },
      }),
    ).toEqual({ kind: 'findings', reportId: 'r1' });
  });

  it('returns a context for every view mode', () => {
    const modes: ViewMode[] = ['dashboard', 'atoms', 'canvas', 'wiki', 'reports'];
    for (const viewMode of modes) {
      expect(deriveSidebarContext(input({ viewMode }))).toBeTruthy();
    }
  });
});
