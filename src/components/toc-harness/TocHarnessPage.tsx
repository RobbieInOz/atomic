import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { Element } from 'hast';
import type { EditorView } from '@codemirror/view';
import type { AtomicCodeMirrorEditorProps } from '@atomic-editor/editor';
import { useFont, useTheme } from '../../hooks';
import { useSettingsStore } from '../../stores/settings';
import { scrollToOffset, tocViewExtension } from '../../editor/toc-scroll';
import { TocPanel } from '../toc/TocPanel';
import { useCm6ScrollSpy } from '../toc/useCm6ScrollSpy';
import { offsetWithinContainer, useDomScrollSpy } from '../toc/useDomScrollSpy';
import { useTocItems, type TocItem } from '../toc/useTocItems';
import { TOC_FIXTURES, type TocFixtureId } from './sample-content';

// Same lazy boundary the atom reader uses — editor module and its code
// language registry pinned into one chunk, with the default `codeLanguages`
// filled in so call sites don't need the sub-path import.
const AtomicCodeMirrorEditor = lazy(async () => {
  const [mod, langs] = await Promise.all([
    import('@atomic-editor/editor'),
    import('@atomic-editor/editor/code-languages'),
  ]);
  const Base = mod.AtomicCodeMirrorEditor;
  const DEFAULT_LANGUAGES = langs.ATOMIC_CODE_LANGUAGES;
  const Wrapped = (props: AtomicCodeMirrorEditorProps) => (
    <Base {...props} codeLanguages={props.codeLanguages ?? DEFAULT_LANGUAGES} />
  );
  return { default: Wrapped };
});

type PaneKind = 'markdown' | 'editor';
const PANES: { id: PaneKind; label: string }[] = [
  { id: 'markdown', label: 'react-markdown' },
  { id: 'editor', label: 'codemirror' },
];

/** Space left above a heading after clicking it in the outline. */
const SCROLL_TOP_MARGIN = 72;

const PROSE_CLASSES =
  'prose prose-invert max-w-none prose-headings:text-[var(--color-text-primary)] ' +
  'prose-p:text-[var(--color-text-primary)] prose-li:text-[var(--color-text-primary)] ' +
  'prose-code:text-[var(--color-accent-light)] prose-code:bg-[var(--color-bg-card)] ' +
  'prose-pre:bg-[var(--color-bg-card)] prose-pre:border prose-pre:border-[var(--color-border)]';

const TOGGLE_BASE = 'rounded border px-2 py-1 text-xs transition-colors ';
const TOGGLE_ON =
  'border-[var(--color-accent)] bg-[var(--color-accent-alpha)] text-[var(--color-text-primary)]';
const TOGGLE_OFF =
  'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-border-hover)]';

/**
 * What a mounted content pane offers the outline: where the reader currently
 * is, and how to get somewhere else. Phase C moves this handshake into a
 * store so the real readers can talk to the real sidebar.
 */
interface TocPaneState {
  activeId: string | null;
  scrollToItem: (item: TocItem) => void;
}

type PublishToc = (state: TocPaneState | null) => void;

export function TocHarnessPage() {
  useTheme();
  useFont();

  // The theme/font hooks read from the settings store, but the store is
  // lazy — it's only populated when the SettingsModal opens. The
  // harness lives outside the normal Layout, so without this fetch it
  // would always show the built-in defaults regardless of what the
  // user selected.
  const fetchSettings = useSettingsStore((s) => s.fetchSettings);
  useEffect(() => {
    fetchSettings().catch(() => {
      // Transport may be disconnected (web mode without saved server
      // config). The defaults from useFont/useTheme are fine in that
      // case — no need to surface the failure.
    });
  }, [fetchSettings]);

  const [fixtureId, setFixtureId] = useState<TocFixtureId>('long');
  const [pane, setPane] = useState<PaneKind>('markdown');
  const [paneToc, setPaneToc] = useState<TocPaneState | null>(null);

  const fixture = TOC_FIXTURES.find((f) => f.id === fixtureId) ?? TOC_FIXTURES[0];
  const items = useTocItems(fixture.source);

  const handleItemClick = useCallback(
    (item: TocItem) => paneToc?.scrollToItem(item),
    [paneToc],
  );

  return (
    <div className="h-screen flex flex-col bg-[var(--color-bg-main)] text-[var(--color-text-primary)]">
      <header className="border-b border-[var(--color-border)] bg-[var(--color-bg-panel)] px-4 py-3">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-3">
            <span className="text-sm font-semibold tracking-wide">TOC Harness</span>
            <Link
              to="/"
              className="text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-accent-light)]"
            >
              ← back to app
            </Link>
          </div>

          <div className="flex items-center gap-1.5">
            <span className="text-xs text-[var(--color-text-secondary)]">fixture:</span>
            {TOC_FIXTURES.map((f) => (
              <button
                key={f.id}
                type="button"
                onClick={() => setFixtureId(f.id)}
                className={TOGGLE_BASE + (fixtureId === f.id ? TOGGLE_ON : TOGGLE_OFF)}
              >
                {f.label}
              </button>
            ))}
          </div>

          {/* Pane toggle — the same outline drives DOM-rendered markdown
              (the wiki/finding readers) and a CodeMirror document (the atom
              reader). Only the second one has to survive virtualization. */}
          <div className="flex items-center gap-1.5">
            <span className="text-xs text-[var(--color-text-secondary)]">pane:</span>
            {PANES.map((p) => (
              <button
                key={p.id}
                type="button"
                onClick={() => setPane(p.id)}
                className={TOGGLE_BASE + (pane === p.id ? TOGGLE_ON : TOGGLE_OFF)}
              >
                {p.label}
              </button>
            ))}
          </div>

          <div className="ml-auto flex items-center gap-4 text-xs text-[var(--color-text-secondary)]">
            <span>
              <span className="font-mono text-[var(--color-text-primary)]">{items.length}</span>{' '}
              headings
            </span>
            <span>
              active:{' '}
              <span className="font-mono text-[var(--color-text-primary)]">
                {paneToc?.activeId ?? '—'}
              </span>
            </span>
          </div>
        </div>
      </header>

      <main className="flex-1 flex overflow-hidden">
        {/* Stand-in for LeftPanel at its real 250px width, so row density and
            truncation are honest here. */}
        <aside className="w-[250px] shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-panel)] overflow-hidden">
          <TocPanel
            items={items}
            activeId={paneToc?.activeId ?? null}
            onItemClick={handleItemClick}
            title="Contents"
            emptyLabel="No headings in this note"
          />
        </aside>

        {pane === 'markdown' ? (
          <MarkdownPane source={fixture.source} items={items} publish={setPaneToc} />
        ) : (
          <EditorPane
            documentId={`toc-harness-${fixture.id}`}
            source={fixture.source}
            items={items}
            publish={setPaneToc}
          />
        )}
      </main>
    </div>
  );
}

interface PaneProps {
  source: string;
  items: TocItem[];
  publish: PublishToc;
}

/**
 * Rendered-markdown pane. Heading ids come from the parse, keyed by source
 * offset — the technique the wiki and finding readers will use, since
 * re-slugging the rendered children would drift from `parseHeadings` on
 * duplicates and inline markup.
 */
function MarkdownPane({ source, items, publish }: PaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const activeId = useDomScrollSpy(containerRef, items);

  const components = useMemo(() => {
    const idByOffset = new Map(items.map((item) => [item.offset, item.id]));
    const idFor = (node?: Element) => {
      const offset = node?.position?.start?.offset;
      return offset === undefined ? undefined : idByOffset.get(offset);
    };
    const headings: Components = {
      h1: ({ node, children }) => <h1 id={idFor(node)}>{children}</h1>,
      h2: ({ node, children }) => <h2 id={idFor(node)}>{children}</h2>,
      h3: ({ node, children }) => <h3 id={idFor(node)}>{children}</h3>,
      h4: ({ node, children }) => <h4 id={idFor(node)}>{children}</h4>,
      h5: ({ node, children }) => <h5 id={idFor(node)}>{children}</h5>,
      h6: ({ node, children }) => <h6 id={idFor(node)}>{children}</h6>,
    };
    return headings;
  }, [items]);

  const scrollToItem = useCallback((item: TocItem) => {
    const container = containerRef.current;
    const el = container?.ownerDocument.getElementById(item.id);
    if (!container || !el) return;
    container.scrollTo({ top: offsetWithinContainer(container, el) - SCROLL_TOP_MARGIN });
  }, []);

  useEffect(() => {
    publish({ activeId, scrollToItem });
    return () => publish(null);
  }, [publish, activeId, scrollToItem]);

  return (
    <div ref={containerRef} className="flex-1 overflow-y-auto scrollbar-auto-hide">
      <div className={`max-w-3xl mx-auto px-8 py-6 ${PROSE_CLASSES}`}>
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
          {source}
        </ReactMarkdown>
      </div>
    </div>
  );
}

/**
 * CodeMirror pane, laid out like the atom reader: the editor grows to its
 * content height inside a page-level scroller, so scrolling and scroll
 * tracking both go through this container rather than `view.scrollDOM`.
 */
function EditorPane({ documentId, source, items, publish }: PaneProps & { documentId: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const activeId = useCm6ScrollSpy(containerRef, viewRef, items);

  // The editor captures `extensions` once per document identity, so this has
  // to be a stable reference over a stable ref.
  const extensions = useMemo(() => [tocViewExtension(viewRef)], []);

  const scrollToItem = useCallback((item: TocItem) => {
    const view = viewRef.current;
    if (view) scrollToOffset(view, item.offset, SCROLL_TOP_MARGIN);
  }, []);

  useEffect(() => {
    publish({ activeId, scrollToItem });
    return () => publish(null);
  }, [publish, activeId, scrollToItem]);

  return (
    <div ref={containerRef} className="flex-1 overflow-y-auto scrollbar-auto-hide">
      <div className="max-w-3xl mx-auto px-8 py-6">
        <Suspense fallback={null}>
          <AtomicCodeMirrorEditor
            key={documentId}
            documentId={documentId}
            markdownSource={source}
            readOnly
            blurEditorOnMount
            extensions={extensions}
          />
        </Suspense>
      </div>
    </div>
  );
}
