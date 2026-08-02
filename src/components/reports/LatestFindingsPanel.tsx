import { useEffect, useMemo } from 'react';
import { useReportsStore, type ReportFindingWithAtom } from '../../stores/reports';
import { useUIStore } from '../../stores/ui';
import { formatRelativeDate, formatShortRelativeDate } from '../../lib/date';

interface LatestFindingsPanelProps {
  /** `null` on the reports list — the newest finding from every report. */
  reportId: string | null;
}

/// The reports sidebar. Scoped to a report while its tab is open (walk the
/// siblings without leaving the detail view), otherwise the latest finding
/// from each report, newest first.
///
/// Reads the reports store, which has no unmount reset — the detail view,
/// the tab strip, and the run-completion subscription all share it.
export function LatestFindingsPanel({ reportId }: LatestFindingsPanelProps) {
  const reports = useReportsStore(s => s.reports);
  const reportsById = useReportsStore(s => s.byId);
  const lastFindingByReport = useReportsStore(s => s.lastFindingByReport);
  const scopedFindings = useReportsStore(s =>
    reportId === null ? undefined : s.findingsByReport[reportId]
  );
  const isLoadingList = useReportsStore(s => s.isLoadingList);
  const loadError = useReportsStore(s => s.loadError);
  const fetchAll = useReportsStore(s => s.fetchAll);
  const fetchFindings = useReportsStore(s => s.fetchFindings);
  const openFindingReader = useUIStore(s => s.openFindingReader);

  useEffect(() => {
    if (reportId === null) {
      // Store-guarded: the reports view loads on mount too, and this panel
      // mounts a commit earlier.
      void fetchAll();
      return;
    }
    // The detail view — always mounted beside this panel, since a scoped
    // sidebar means a report tab is open — issues the same load. Only fetch
    // when nothing is cached for this report yet.
    if (useReportsStore.getState().findingsByReport[reportId] !== undefined) return;
    void fetchFindings(reportId);
  }, [reportId, fetchAll, fetchFindings]);

  const rows = useMemo(() => {
    // Findings outlive their report, so fall back to the name the run
    // snapshotted when the live row is gone.
    const reportName = (item: ReportFindingWithAtom) => {
      const live = item.finding.report_id ? reportsById[item.finding.report_id]?.name : undefined;
      return live ?? item.finding.report_name_snapshot;
    };

    const toRow = (item: ReportFindingWithAtom, withReportName: boolean) => ({
      atomId: item.atom.id,
      title: item.atom.title || 'Untitled finding',
      date: item.finding.created_at,
      reportName: withReportName ? reportName(item) : null,
    });

    if (reportId !== null) {
      // Already most-recent-first from the wire.
      return (scopedFindings ?? []).map(item => toRow(item, false));
    }
    return Object.values(lastFindingByReport)
      .filter((item): item is ReportFindingWithAtom => item !== null)
      .sort((a, b) => new Date(b.finding.created_at).getTime() - new Date(a.finding.created_at).getTime())
      .map(item => toRow(item, true));
  }, [reportId, scopedFindings, lastFindingByReport, reportsById]);

  // Distinguish "still arriving" from "genuinely empty". Across all reports
  // that means the list fetch or its per-report last-finding fan-out is
  // still in flight (`undefined` = never fetched, `null` = has no findings).
  const isLoading =
    rows.length === 0 &&
    (reportId === null
      ? isLoadingList || reports.some(r => lastFindingByReport[r.id] === undefined)
      : scopedFindings === undefined);

  const open = (atomId: string, opts?: { newTab?: boolean }) => {
    openFindingReader(atomId, opts);
  };

  return (
    <div className="h-full overflow-y-auto scrollbar-hidden">
      {reportId === null && loadError && (
        <p className="px-3 py-2 text-xs text-red-400 leading-relaxed">{loadError}</p>
      )}

      {isLoading ? (
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
      ) : rows.length === 0 ? (
        loadError ? null : (
          <p className="px-3 py-4 text-xs text-[var(--color-text-secondary)] leading-relaxed">
            {reportId === null && reports.length === 0
              ? 'No reports yet. Create one to have it research your knowledge base on a schedule.'
              : 'No findings yet. Each run writes one here.'}
          </p>
        )
      ) : (
        rows.map((row) => (
          <button
            key={row.atomId}
            type="button"
            onClick={(e) => open(row.atomId, { newTab: e.metaKey || e.ctrlKey })}
            onAuxClick={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                open(row.atomId, { newTab: true });
              }
            }}
            className="w-full block px-3 py-2 text-left hover:bg-[var(--color-bg-card)] transition-colors"
          >
            <span className="flex items-baseline gap-2">
              <span
                className="flex-1 min-w-0 truncate text-sm text-[var(--color-text-primary)]"
                title={row.title}
              >
                {row.title}
              </span>
              <span
                className="shrink-0 text-[11px] text-[var(--color-text-tertiary)]"
                title={formatRelativeDate(row.date)}
              >
                {formatShortRelativeDate(row.date)}
              </span>
            </span>

            {row.reportName && (
              <span
                className="block truncate mt-0.5 text-[11px] text-[var(--color-text-tertiary)]"
                title={row.reportName}
              >
                {row.reportName}
              </span>
            )}
          </button>
        ))
      )}
    </div>
  );
}
