import { Fragment, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { Element } from 'hast';
import { CitationLink } from '../../wiki/CitationLink';
import type { FindingCitation } from '../../../stores/featuredReport';

interface BriefingContentProps {
  content: string;
  citations: FindingCitation[];
  /** Ids to stamp on rendered headings, keyed by their offset in `content`.
   *  Opt-in: only surfaces that drive a table of contents pass it. */
  headingIdByOffset?: Map<number, string>;
  onCitationClick: (citation: FindingCitation, element: HTMLElement) => void;
}

interface HeadingProps {
  node?: Element;
  children?: ReactNode;
}

/**
 * Renders briefing markdown with inline `[N]` citation markers replaced by
 * clickable CitationLink buttons. Intentionally simpler than WikiArticleContent:
 * no wiki-links, no search highlighting, no related tags.
 */
export function BriefingContent({ content, citations, headingIdByOffset, onCitationClick }: BriefingContentProps) {
  const citationMap = new Map(citations.map(c => [c.citation_index, c]));

  const processTextWithCitations = (text: string): (string | JSX.Element)[] => {
    const parts = text.split(/(\[\d+\])/g);
    return parts.map((part, i) => {
      const match = part.match(/^\[(\d+)\]$/);
      if (!match) return part;
      const index = parseInt(match[1], 10);
      const citation = citationMap.get(index);
      if (!citation) return part;
      return (
        <CitationLink
          key={`c-${i}-${index}`}
          index={index}
          onClick={(e) => onCitationClick(citation, e.currentTarget)}
        />
      );
    });
  };

  const processChildren = (children: ReactNode): ReactNode => {
    if (typeof children === 'string') {
      return processTextWithCitations(children).map((part, i) =>
        typeof part === 'string' ? <Fragment key={`t-${i}`}>{part}</Fragment> : part
      );
    }
    if (Array.isArray(children)) {
      return children.map((child, i) => <Fragment key={i}>{processChildren(child)}</Fragment>);
    }
    return children;
  };

  // Heading ids are matched by source offset rather than re-slugged from the
  // rendered text, so they survive the demotion below and stay identical to
  // whatever outline the caller parsed.
  const headingId = (node?: Element): string | undefined => {
    const offset = node?.position?.start?.offset;
    return offset === undefined ? undefined : headingIdByOffset?.get(offset);
  };

  // Agent-authored markdown is body content, not page structure: whatever
  // heading level the model chose, it renders as a modest section header
  // and can never compete with the app's own chrome. h1–h3 demote to a
  // semantic <h3> (one <h1> per page, and it isn't the model's), h4+ to
  // <h4>; the container's prose-headings utilities cap the visual scale.
  const components = {
    p: ({ children }: { children?: ReactNode }) => <p>{processChildren(children)}</p>,
    li: ({ children }: { children?: ReactNode }) => <li>{processChildren(children)}</li>,
    strong: ({ children }: { children?: ReactNode }) => <strong>{processChildren(children)}</strong>,
    em: ({ children }: { children?: ReactNode }) => <em>{processChildren(children)}</em>,
    h1: ({ node, children }: HeadingProps) => <h3 id={headingId(node)}>{processChildren(children)}</h3>,
    h2: ({ node, children }: HeadingProps) => <h3 id={headingId(node)}>{processChildren(children)}</h3>,
    h3: ({ node, children }: HeadingProps) => <h3 id={headingId(node)}>{processChildren(children)}</h3>,
    h4: ({ node, children }: HeadingProps) => <h4 id={headingId(node)}>{processChildren(children)}</h4>,
    h5: ({ node, children }: HeadingProps) => <h4 id={headingId(node)}>{processChildren(children)}</h4>,
    h6: ({ node, children }: HeadingProps) => <h4 id={headingId(node)}>{processChildren(children)}</h4>,
  };

  return (
    <div className="prose prose-invert prose-sm md:prose-base max-w-none text-[var(--color-text-secondary)] [&_p]:leading-relaxed [&_p]:my-3 first:[&_p]:mt-0 last:[&_p]:mb-0 prose-headings:text-lg prose-headings:font-semibold prose-headings:tracking-normal prose-headings:mt-5 prose-headings:mb-2 prose-h4:text-base prose-headings:first:mt-0">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
