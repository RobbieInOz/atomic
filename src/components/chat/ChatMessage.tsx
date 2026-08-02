import { useState, useCallback, Fragment, ReactNode, useEffect, useMemo } from 'react';
import { BookOpen, CheckCircle2, Loader2, Telescope, Wrench, XCircle } from 'lucide-react';
import { ChatMessageWithContext, ChatCitation, ChatToolCall } from '../../stores/chat';
import { useAtomsStore, type AtomSummary, type AtomWithTags } from '../../stores/atoms';
import { useUIStore } from '../../stores/ui';
import { getTransport } from '../../lib/transport';
import { CitationLink, CitationPopover } from '../wiki';
import { MarkdownImage } from '../ui/MarkdownImage';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

interface ChatMessageProps {
  message: ChatMessageWithContext;
  isStreaming?: boolean;
  onViewAtom?: (atomId: string, highlightText?: string) => void;
  searchQuery?: string;
  highlightText?: (text: string) => ReactNode;
}

export function ChatMessage({ message, isStreaming = false, onViewAtom, searchQuery = '', highlightText }: ChatMessageProps) {
  const isUser = message.role === 'user';
  const isAssistant = message.role === 'assistant';
  const atoms = useAtomsStore(s => s.atoms);
  const openWikiReader = useUIStore(s => s.openWikiReader);
  const openFindingReader = useUIStore(s => s.openFindingReader);

  const [activeCitation, setActiveCitation] = useState<ChatCitation | null>(null);
  const [anchorRect, setAnchorRect] = useState<{ top: number; left: number; bottom: number; width: number } | null>(null);
  const [fetchedAtomTitles, setFetchedAtomTitles] = useState<Record<string, string>>({});

  const atomTitleMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const atom of atoms) {
      map.set(atom.id, displayTitleForAtom(atom));
    }
    for (const [atomId, title] of Object.entries(fetchedAtomTitles)) {
      map.set(atomId, title);
    }
    return map;
  }, [atoms, fetchedAtomTitles]);

  const referencedAtomIds = useMemo(() => (
    isAssistant
      ? Array.from(message.content.matchAll(/\[\[([0-9a-fA-F-]{36})\]\]/g), (match) => match[1])
      : []
  ), [isAssistant, message.content]);

  useEffect(() => {
    if (!isAssistant || referencedAtomIds.length === 0) return;
    const missingIds = Array.from(new Set(referencedAtomIds)).filter((atomId) => !atomTitleMap.has(atomId));
    if (missingIds.length === 0) return;

    let cancelled = false;
    for (const atomId of missingIds) {
      getTransport()
        .invoke<AtomWithTags | null>('get_atom_by_id', { id: atomId })
        .then((atom) => {
          if (cancelled || !atom) return;
          setFetchedAtomTitles((current) => ({
            ...current,
            [atomId]: displayTitleForAtom(atom),
          }));
        })
        .catch(() => {
          if (cancelled) return;
          setFetchedAtomTitles((current) => ({
            ...current,
            [atomId]: atomId.slice(0, 8),
          }));
        });
    }

    return () => {
      cancelled = true;
    };
  }, [atomTitleMap, isAssistant, referencedAtomIds]);

  // Create a map of citation index to citation object
  const citationMap = useMemo(
    () => new Map((message.citations || []).map((c) => [c.citation_index, c])),
    [message.citations]
  );

  const handleCitationClick = (citation: ChatCitation, element: HTMLElement) => {
    const rect = element.getBoundingClientRect();
    setActiveCitation(citation);
    setAnchorRect({ top: rect.top, left: rect.left, bottom: rect.bottom, width: rect.width });
  };

  const handleClosePopover = () => {
    setActiveCitation(null);
    setAnchorRect(null);
  };

  const handleViewAtom = (atomId: string, highlightText?: string) => {
    if (onViewAtom) {
      onViewAtom(atomId, highlightText);
    }
    handleClosePopover();
  };

  // Open whatever a citation points at. The excerpt doubles as the passage
  // to scroll to, which is what turns a citation into a deep link — except
  // for findings, whose reader has no highlight support (accepted; the
  // finding still opens, just at the top).
  const handleOpenCitation = (citation: ChatCitation) => {
    switch (citation.source_type) {
      case 'wiki':
        openWikiReader(citation.atom_id, citation.source_title ?? 'Wiki', citation.excerpt);
        handleClosePopover();
        return;
      case 'finding':
        openFindingReader(citation.atom_id);
        handleClosePopover();
        return;
      default:
        handleViewAtom(citation.atom_id, citation.excerpt);
    }
  };

  // Process text to replace [N] citations and [[atom-id]] references with
  // interactive components. Strings stay as strings so search highlighting can
  // still operate on ordinary text.
  const processTextWithCitations = useCallback((text: string): (string | JSX.Element)[] => {
    const parts = text.split(/(\[\d+\]|\[\[[0-9a-fA-F-]{36}\]\])/g);
    return parts.map((part, i) => {
      const citationMatch = part.match(/^\[(\d+)\]$/);
      if (citationMatch) {
        const index = parseInt(citationMatch[1], 10);
        const citation = citationMap.get(index);
        if (citation) {
          return (
            <CitationLink
              key={`citation-${i}-${index}`}
              index={index}
              onClick={(e) => handleCitationClick(citation, e.currentTarget)}
            />
          );
        }
      }

      const atomMatch = part.match(/^\[\[([0-9a-fA-F-]{36})\]\]$/);
      if (atomMatch) {
        const atomId = atomMatch[1];
        const label = atomTitleMap.get(atomId) ?? atomId.slice(0, 8);
        return (
          <button
            key={`atom-link-${i}-${atomId}`}
            type="button"
            onClick={() => handleViewAtom(atomId)}
            className="inline-flex items-center rounded px-1 py-0.5 font-mono text-[0.85em] text-[var(--color-accent-light)] underline decoration-[var(--color-border-hover)] underline-offset-2 hover:decoration-current hover:bg-[var(--color-bg-hover)]"
            title={`Open atom ${atomId}`}
          >
            {label}
          </button>
        );
      }
      // Return raw string so highlighting can be applied
      return part;
    });
  }, [atomTitleMap, citationMap]);

  // Process children recursively to handle citations and search highlighting in all text nodes
  const processChildren = useCallback((children: ReactNode): ReactNode => {
    if (typeof children === 'string') {
      // First process citations, then apply highlighting
      const withCitations = processTextWithCitations(children);
      if (searchQuery.trim() && highlightText) {
        // Apply highlighting to string parts, keep citation elements as-is
        return withCitations.map((part, i) => {
          if (typeof part === 'string') {
            return <Fragment key={`hl-${i}`}>{highlightText(part)}</Fragment>;
          }
          // Citation link element - keep as is
          return part;
        });
      }
      // No search - wrap strings in fragments for valid React output
      return withCitations.map((part, i) =>
        typeof part === 'string' ? <Fragment key={`t-${i}`}>{part}</Fragment> : part
      );
    }
    if (Array.isArray(children)) {
      return children.map((child, i) => (
        <Fragment key={i}>{processChildren(child)}</Fragment>
      ));
    }
    return children;
  }, [processTextWithCitations, searchQuery, highlightText]);

  // Custom components for react-markdown with citation processing
  const markdownComponents = {
    p: ({ children }: { children?: ReactNode }) => (
      <p>{processChildren(children)}</p>
    ),
    li: ({ children }: { children?: ReactNode }) => (
      <li>{processChildren(children)}</li>
    ),
    td: ({ children }: { children?: ReactNode }) => (
      <td>{processChildren(children)}</td>
    ),
    th: ({ children }: { children?: ReactNode }) => (
      <th>{processChildren(children)}</th>
    ),
    strong: ({ children }: { children?: ReactNode }) => (
      <strong>{processChildren(children)}</strong>
    ),
    em: ({ children }: { children?: ReactNode }) => (
      <em>{processChildren(children)}</em>
    ),
    del: ({ children }: { children?: ReactNode }) => (
      <del>{processChildren(children)}</del>
    ),
    h1: ({ children }: { children?: ReactNode }) => (
      <h1>{processChildren(children)}</h1>
    ),
    h2: ({ children }: { children?: ReactNode }) => (
      <h2>{processChildren(children)}</h2>
    ),
    h3: ({ children }: { children?: ReactNode }) => (
      <h3>{processChildren(children)}</h3>
    ),
    h4: ({ children }: { children?: ReactNode }) => (
      <h4>{processChildren(children)}</h4>
    ),
    h5: ({ children }: { children?: ReactNode }) => (
      <h5>{processChildren(children)}</h5>
    ),
    h6: ({ children }: { children?: ReactNode }) => (
      <h6>{processChildren(children)}</h6>
    ),
    blockquote: ({ children }: { children?: ReactNode }) => (
      <blockquote>{processChildren(children)}</blockquote>
    ),
    // Style links with search highlighting
    a: ({ children, href }: { children?: ReactNode; href?: string }) => (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className="underline underline-offset-2 decoration-[var(--color-border-hover)] hover:decoration-current transition-colors"
      >
        {processChildren(children)}
      </a>
    ),
    // Style code with search highlighting
    code: ({ className, children }: { className?: string; children?: ReactNode }) => {
      const isBlock = className?.startsWith('language-');
      if (isBlock) {
        return <code className={className}>{processChildren(children)}</code>;
      }
      return (
        <code className="px-1 py-0.5 bg-[var(--color-bg-main)] rounded text-[#e5c07b]">
          {processChildren(children)}
        </code>
      );
    },
    pre: ({ children }: { children?: ReactNode }) => (
      <pre>{children}</pre>
    ),
    img: ({ src, alt }: { src?: string; alt?: string }) => (
      <MarkdownImage src={src} alt={alt} />
    ),
  };

  return (
    <>
      <div className={`flex ${isUser ? 'justify-end' : 'justify-start'}`}>
        <div
          className={`max-w-[85%] rounded-lg px-4 py-3 ${
            isUser
              ? 'bg-[var(--color-accent)] text-white'
              : 'bg-[var(--color-bg-card)] text-[var(--color-text-primary)]'
          }`}
        >
          {/* Tool calls — render above message content so users see the
              retrieval steps before the prose that references them. Rendered
              during streaming (from streamingToolCalls) and persisted after
              completion (from message.tool_calls). */}
          {isAssistant && message.tool_calls && message.tool_calls.length > 0 && (
            <ToolCallList calls={message.tool_calls} />
          )}

          {/* Message content */}
          {isAssistant ? (
            message.content ? (
              <div className="prose prose-invert prose-sm max-w-none prose-headings:text-[var(--color-text-primary)] prose-p:text-[var(--color-text-primary)] prose-a:text-[var(--color-text-primary)] prose-a:underline prose-a:decoration-[var(--color-border-hover)] prose-a:hover:decoration-current prose-strong:text-[var(--color-text-primary)] prose-code:text-[var(--color-accent-light)] prose-code:bg-[var(--color-bg-card)] prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-pre:bg-[var(--color-bg-card)] prose-pre:border prose-pre:border-[var(--color-border)] prose-blockquote:border-l-[var(--color-accent)] prose-blockquote:text-[var(--color-text-secondary)] prose-li:text-[var(--color-text-primary)] prose-hr:border-[var(--color-border)]">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={markdownComponents}
                >
                  {message.content}
                </ReactMarkdown>
              </div>
            ) : isStreaming ? (
              <div className="flex items-center gap-2 text-sm text-[var(--color-text-secondary)]">
                <div className="flex gap-1">
                  <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] animate-bounce" style={{ animationDelay: '0ms' }} />
                  <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] animate-bounce" style={{ animationDelay: '150ms' }} />
                  <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] animate-bounce" style={{ animationDelay: '300ms' }} />
                </div>
                <span>Thinking…</span>
              </div>
            ) : null
          ) : (
            <p className="whitespace-pre-wrap text-sm">
              {searchQuery.trim() && highlightText
                ? highlightText(message.content)
                : message.content}
            </p>
          )}

          {/* Streaming indicator (cursor) — only once content has started */}
          {isStreaming && message.content && (
            <span className="inline-block w-2 h-4 ml-1 bg-[var(--color-accent-light)] animate-pulse" />
          )}

          {/* Citations (for assistant messages) */}
          {isAssistant && message.citations && message.citations.length > 0 && (
            <div className="mt-3 pt-3 border-t border-[var(--color-border)]">
              <p className="text-xs text-[var(--color-text-secondary)] mb-2">Sources:</p>
              <div className="flex flex-wrap gap-1">
                {message.citations.map((citation) => (
                  <button
                    key={citation.id}
                    onClick={(e) => handleCitationClick(citation, e.currentTarget)}
                    className="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded bg-[var(--color-bg-hover)] hover:bg-[var(--color-border-hover)] text-[var(--color-accent-light)] transition-colors cursor-pointer"
                    title={citation.source_title ? `${citation.source_title} — ${citation.excerpt}` : citation.excerpt}
                  >
                    <SourceIcon sourceType={citation.source_type} />
                    [{citation.citation_index}]
                  </button>
                ))}
              </div>
            </div>
          )}

        </div>
      </div>

      {/* Citation popover. Where "view source" goes depends on what the
          citation cites, so the popover's callback defers to the citation
          rather than assuming an atom. */}
      {activeCitation && anchorRect && (
        <CitationPopover
          citation={activeCitation}
          anchorRect={anchorRect}
          onClose={handleClosePopover}
          onViewAtom={() => handleOpenCitation(activeCitation)}
        />
      )}
    </>
  );
}

// Quietly marks the sources that aren't atoms — same icons the wiki and
// reports views use in the nav.
function SourceIcon({ sourceType }: { sourceType?: ChatCitation['source_type'] }) {
  if (sourceType === 'wiki') return <BookOpen className="w-3 h-3" strokeWidth={2} />;
  if (sourceType === 'finding') return <Telescope className="w-3 h-3" strokeWidth={2} />;
  return null;
}

function displayTitleForAtom(atom: Pick<AtomSummary, 'id' | 'title' | 'snippet'>): string {
  const title = atom.title.trim();
  if (title) return title;

  const snippet = atom.snippet.trim();
  if (snippet) return snippet.length > 48 ? `${snippet.slice(0, 45).trimEnd()}...` : snippet;

  return atom.id.slice(0, 8);
}

function ToolCallList({ calls }: { calls: ChatToolCall[] }) {
  return (
    <div className="mb-2 space-y-1">
      {calls.map((call) => (
        <ToolCallCard key={call.id} call={call} />
      ))}
    </div>
  );
}

function ToolCallCard({ call }: { call: ChatToolCall }) {
  const statusIcon =
    call.status === 'running' ? (
      <Loader2 className="w-3.5 h-3.5 text-[var(--color-accent-light)] animate-spin" />
    ) : call.status === 'failed' ? (
      <XCircle className="w-3.5 h-3.5 text-red-400" />
    ) : call.status === 'complete' ? (
      <CheckCircle2 className="w-3.5 h-3.5 text-[var(--color-accent-light)]" />
    ) : (
      <Wrench className="w-3.5 h-3.5 text-[var(--color-text-tertiary)]" />
    );

  const resultsCount =
    call.tool_output && typeof call.tool_output === 'object' && call.tool_output !== null
      ? (call.tool_output as { results_count?: number }).results_count
      : undefined;

  const summaryText =
    call.status === 'running'
      ? 'running…'
      : resultsCount !== undefined
      ? `${resultsCount} result${resultsCount === 1 ? '' : 's'}`
      : call.status;

  return (
    <details className="group text-xs bg-[var(--color-bg-main)] rounded border border-[var(--color-border)] open:border-[var(--color-border-hover)]">
      <summary className="flex items-center gap-2 px-2 py-1.5 cursor-pointer list-none hover:bg-[var(--color-bg-hover)]">
        {statusIcon}
        <span className="font-mono text-[var(--color-accent)]">{call.tool_name}</span>
        <span className="ml-auto text-[var(--color-text-tertiary)]">{summaryText}</span>
      </summary>
      <div className="px-2 pb-2 pt-1 border-t border-[var(--color-border)] space-y-2">
        <ToolJsonBlock label="input" value={call.tool_input} />
        {call.tool_output !== null && call.tool_output !== undefined && (
          <ToolJsonBlock label="output" value={call.tool_output} />
        )}
      </div>
    </details>
  );
}

function ToolJsonBlock({ label, value }: { label: string; value: unknown }) {
  const formatted = (() => {
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  })();
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wide text-[var(--color-text-tertiary)] mb-0.5">
        {label}
      </div>
      <pre className="text-[11px] whitespace-pre-wrap break-words bg-[var(--color-bg-card)] rounded px-2 py-1.5 text-[var(--color-text-secondary)]">
        {formatted}
      </pre>
    </div>
  );
}
