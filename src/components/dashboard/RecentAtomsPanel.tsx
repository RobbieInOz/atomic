import { useCallback, useEffect, useRef, useState } from 'react';
import { getTransport } from '../../lib/transport';
import type { AtomSummary, PaginatedAtoms } from '../../stores/atoms';
import { useDatabasesStore } from '../../stores/databases';
import { useUIStore } from '../../stores/ui';
import { formatRelativeDate, formatShortRelativeDate } from '../../lib/date';

/** Enough to fill the panel without paging; this is a jump list, not a view. */
const RECENT_LIMIT = 20;

/** Atom events arrive in bursts (imports, feed polls, a pipeline draining). */
const REFRESH_DEBOUNCE_MS = 600;

/// The dashboard's sidebar: the notes you touched most recently, newest
/// first, one click from the reader.
///
/// The query is deliberately component-local rather than routed through
/// `useAtomsStore`: that store's list *is* the atoms view — its filter,
/// sort, and pagination cursors all describe it — so writing this fixed
/// 20-row query into it would corrupt the view the user goes back to.
export function RecentAtomsPanel() {
  const [atoms, setAtoms] = useState<AtomSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const openReader = useUIStore(s => s.openReader);

  // A database switch resets the data stores, but nothing clears rows held in
  // component state — so the active id is a load dependency here.
  const activeDatabaseId = useDatabasesStore(s => s.activeId);

  // Responses can land out of order (a database switch racing an event-driven
  // refresh); only the newest request may paint.
  const requestSeq = useRef(0);

  const load = useCallback(async () => {
    const seq = ++requestSeq.current;
    try {
      const result = await getTransport().invoke<PaginatedAtoms>('list_atoms', {
        limit: RECENT_LIMIT,
        sortBy: 'updated',
        sortOrder: 'desc',
      });
      if (seq !== requestSeq.current) return;
      setAtoms(result.atoms);
      setError(null);
    } catch (e) {
      if (seq !== requestSeq.current) return;
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    // A switch invalidates the rows outright: show the skeleton rather than
    // the previous database's notes while the new page is in flight.
    if (activeDatabaseId) setAtoms(null);
    void load();
  }, [load, activeDatabaseId]);

  // Keep the list live without polling. The panel body stays mounted while
  // the sidebar is collapsed, so the refresh is debounced into a single
  // request per burst rather than one per atom.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const scheduleRefresh = () => {
      clearTimeout(timer);
      timer = setTimeout(() => void load(), REFRESH_DEBOUNCE_MS);
    };
    const transport = getTransport();
    const unsubs = ['atom-created', 'atom-updated'].map(event =>
      transport.subscribe(event, scheduleRefresh)
    );
    return () => {
      clearTimeout(timer);
      unsubs.forEach(unsub => unsub());
    };
  }, [load]);

  const open = (atomId: string, opts?: { newTab?: boolean }) => {
    openReader(atomId, undefined, opts);
  };

  return (
    <div className="h-full overflow-y-auto scrollbar-hidden">
      {error && (
        <p className="px-3 py-2 text-xs text-red-400 leading-relaxed">{error}</p>
      )}

      {atoms === null ? (
        error ? null : (
          <div className="flex flex-col gap-1 px-3 py-2">
            {Array.from({ length: 5 }, (_, i) => (
              <div key={i} className="py-1.5">
                <div
                  className="h-3.5 rounded bg-[var(--color-bg-card)] animate-pulse"
                  style={{ width: `${70 + i * 15}px` }}
                />
              </div>
            ))}
          </div>
        )
      ) : atoms.length === 0 ? (
        error ? null : (
          <p className="px-3 py-4 text-xs text-[var(--color-text-secondary)] leading-relaxed">
            Nothing captured yet. Notes you write or import show up here, most recent first.
          </p>
        )
      ) : (
        atoms.map((atom) => {
          const title = atom.title || 'Untitled';
          return (
            <button
              key={atom.id}
              type="button"
              onClick={(e) => open(atom.id, { newTab: e.metaKey || e.ctrlKey })}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  open(atom.id, { newTab: true });
                }
              }}
              className="w-full flex items-baseline gap-2 px-3 py-2 text-left hover:bg-[var(--color-bg-card)] transition-colors"
            >
              <span
                className="flex-1 min-w-0 truncate text-sm text-[var(--color-text-primary)]"
                title={title}
              >
                {title}
              </span>
              <span
                className="shrink-0 text-[11px] text-[var(--color-text-tertiary)]"
                title={formatRelativeDate(atom.updated_at)}
              >
                {formatShortRelativeDate(atom.updated_at)}
              </span>
            </button>
          );
        })
      )}
    </div>
  );
}
