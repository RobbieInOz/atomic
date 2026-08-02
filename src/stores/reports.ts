import { create } from 'zustand';
import { toast } from 'sonner';
import { getTransport } from '../lib/transport';

// =====================================================================
// Types — mirror crates/atomic-core/src/models.rs
// =====================================================================

/// Discriminator on every atom; reports return findings with kind=report.
export type AtomKind = 'captured' | 'report';

/// Wire shape of `SourceScopeWindow`. The Rust enum is
/// `#[serde(rename_all = "snake_case")]` over `{ SinceLastRun,
/// Duration(String) }`, which externally-tagged serializes as either
/// the bare string `"since_last_run"` or the object `{"duration":
/// "P7D"}`. The TS shape must match exactly or the backend rejects the
/// payload during JSON deserialization.
export type SourceScopeWindow = 'since_last_run' | { duration: string };

/// Wire shape of `ContextScopeWindow`. Distinct enum from the source
/// window — the contradiction-scan idiom needs `OlderThanSource` which
/// has no source-side analog.
export type ContextScopeWindow = 'older_than_source' | { duration: string };

/// Wire-aligned `ContextScopeMode`. Backend variants are
/// `same_as_source | all | explicit`. "Explicit" with an empty tag
/// list is how the UI expresses "no context" — there is no `None`
/// variant.
export type ContextScopeMode = 'same_as_source' | 'all' | 'explicit';

/// Wire-aligned `CitationPolicy`. Backend variants are `source_only`
/// (citations resolve to atoms in the run's source scope) and
/// `source_and_context` (semantic_search results also become citable).
export type CitationPolicy = 'source_only' | 'source_and_context';

/// One report definition. Matches `Report` in atomic-core. Cache fields
/// (`last_run_at`, `last_finding_atom_id`, `last_error`) are advisory —
/// authoritative state lives on `task_runs` + `report_findings`.
export interface Report {
  id: string;
  name: string;
  description: string | null;
  research_prompt: string;

  source_scope_tag_ids: string[];
  source_scope_window: SourceScopeWindow | null;
  source_include_kinds: AtomKind[];

  context_scope_mode: ContextScopeMode;
  context_scope_tag_ids: string[];
  context_scope_window: ContextScopeWindow | null;
  context_include_kinds: AtomKind[];

  citation_policy: CitationPolicy;

  max_source_atoms: number | null;
  max_source_tokens: number | null;
  max_tool_iterations: number | null;

  schedule: string;
  schedule_tz: string | null;

  enabled: boolean;
  output_atom_tags: string[];

  last_run_at: string | null;
  last_finding_atom_id: string | null;
  last_error: string | null;

  created_at: string;
  updated_at: string;
}

export interface ReportFinding {
  finding_atom_id: string;
  report_id: string | null;
  run_id: string | null;
  report_name_snapshot: string;
  created_at: string;
}

export interface ReportFindingCitation {
  finding_atom_id: string;
  cited_atom_id: string;
  position: number;
  excerpt: string;
}

/// Cleaned-up shape we cache after destructuring the
/// `list_findings_for_report` response. The wire format is a JSON
/// 2-tuple `[ReportFinding, AtomWithTags]` (because Rust serializes
/// `Vec<(A, B)>` as `[[a,b], ...]`); the store decodes those tuples
/// into this object before handing them to consumers. AtomWithTags
/// uses `#[serde(flatten)]` so atom fields sit at the top of `atom`.
export interface ReportFindingWithAtom {
  finding: ReportFinding;
  atom: {
    id: string;
    content: string;
    /// Server-derived plain-text first line (markdown stripped). Findings
    /// open with their headline, so this is the row label everywhere a
    /// finding is listed compactly.
    title: string;
    source_url: string | null;
    created_at: string;
    updated_at: string;
    kind: AtomKind;
    [k: string]: unknown;
  };
}

/// Raw wire shape — exactly what the server returns for
/// `list_findings_for_report`. Kept private so consumers see the clean
/// object form above.
type FindingTuple = [ReportFinding, ReportFindingWithAtom['atom']];

// =====================================================================
// Write request shapes — mirror Create/UpdateReportRequest
// =====================================================================

/// `POST /api/reports` body. Mirrors `CreateReportRequest`. Backend
/// fills sensible defaults for any field omitted; we keep them
/// optional here for the editor's progressive-disclosure UX.
export interface CreateReportInput {
  name: string;
  description?: string | null;
  research_prompt: string;

  source_scope_tag_ids?: string[];
  source_scope_window?: SourceScopeWindow | null;
  source_include_kinds?: AtomKind[];

  context_scope_mode?: ContextScopeMode;
  context_scope_tag_ids?: string[];
  context_scope_window?: ContextScopeWindow | null;
  context_include_kinds?: AtomKind[];

  citation_policy?: CitationPolicy;

  max_source_atoms?: number | null;
  max_source_tokens?: number | null;
  max_tool_iterations?: number | null;

  schedule: string;
  schedule_tz?: string | null;

  enabled?: boolean;
  output_atom_tags?: string[];
}

/// `PUT /api/reports/:id` body. Every field optional; only present
/// fields are written. Mirrors `UpdateReportRequest`. Note the nested
/// `Option<Option<T>>` pattern from Rust collapses to plain optional
/// here — we send `null` when the user is explicitly clearing the
/// field, omit the key entirely when they aren't touching it.
export interface UpdateReportInput {
  name?: string;
  description?: string | null;
  research_prompt?: string;

  source_scope_tag_ids?: string[];
  source_scope_window?: SourceScopeWindow | null;
  source_include_kinds?: AtomKind[];

  context_scope_mode?: ContextScopeMode;
  context_scope_tag_ids?: string[];
  context_scope_window?: ContextScopeWindow | null;
  context_include_kinds?: AtomKind[];

  citation_policy?: CitationPolicy;

  max_source_atoms?: number | null;
  max_source_tokens?: number | null;
  max_tool_iterations?: number | null;

  schedule?: string;
  schedule_tz?: string | null;

  enabled?: boolean;
  output_atom_tags?: string[];
}

// =====================================================================
// Store
// =====================================================================

interface ReportsStore {
  reports: Report[];
  byId: Record<string, Report>;

  /// Cached last finding per report so the list view's tertiary line
  /// (the italic excerpt) doesn't issue N requests on every render.
  /// `null` after a fetch attempt means "no findings yet" — distinguishes
  /// from `undefined` ("never fetched").
  lastFindingByReport: Record<string, ReportFindingWithAtom | null>;

  /// Full findings cache, keyed by reportId. Populated by the detail
  /// view on mount. Most-recent first; the order matches the wire
  /// response from `list_findings_for_report`.
  findingsByReport: Record<string, ReportFindingWithAtom[]>;

  /// Lazy-loaded citation counts keyed by finding atom id. The
  /// findings response carries the finding row + atom but not the
  /// citation count, so the detail-view row mounts a small effect
  /// that calls `list_finding_citations` and stashes the count here.
  /// `undefined` = not yet fetched, number = resolved.
  citationCountsByAtomId: Record<string, number>;

  /// Reports the user has manually dispatched and whose run hasn't yet
  /// resolved. Two things clear it: an `atom-created` event for a
  /// kind=report atom that matches the report's new most-recent
  /// finding (success), or a 30s poll while the detail view is open
  /// that observes `last_error` set with `last_run_at` after dispatch
  /// (failure). A 5-minute stale guard in the detail view clears
  /// optimistic state if neither resolves.
  runningReportIds: Set<string>;

  /// Wall-clock dispatch timestamps per report, epoch ms. The detail
  /// view's stale-guard timeout reads this to decide when to give up.
  runDispatchedAt: Record<string, number>;

  /// Snapshot of `report.last_error` at dispatch time. The runner
  /// intentionally does *not* update `last_run_at` on failure (a
  /// first-run failure would otherwise stamp an unparseable value
  /// into the schedule anchor), so we can't use the timestamp to tell
  /// our dispatch's failure apart from an earlier one. Instead the
  /// detail view's failure poll compares the freshly-fetched
  /// `last_error` against this snapshot: a value that has changed
  /// (especially: was null, now set) means a new failure outcome was
  /// just recorded.
  lastErrorAtDispatch: Record<string, string | null>;

  isLoadingList: boolean;
  loadError: string | null;

  /// Has the atom-created subscription already been set up? Guards
  /// against double-subscription if `fetchAll` is called twice.
  hasSubscription: boolean;

  fetchAll: () => Promise<void>;
  fetchLastFinding: (reportId: string) => Promise<void>;

  /// Wire up the live-refresh `atom-created` subscription if it isn't
  /// already. Safe to call from any consumer that needs running-state
  /// to clear on completion (list view, detail view). Idempotent —
  /// subsequent calls no-op via the `hasSubscription` guard.
  ///
  /// Lives separately from `fetchAll` so a direct deep-link to
  /// `/reports/:id` (which bypasses the list view entirely) still
  /// observes scheduled and manual run completions.
  ensureSubscription: () => void;

  /// Fetch a single report by id and merge it into the store. Used by
  /// the detail view on cold-start deep links when the list hasn't
  /// been hydrated yet — calling `fetchAll` would work but is wasteful
  /// for the "open one report directly" path. Returns the report on
  /// success, `null` if the server returned 404 (report deleted).
  fetchOne: (reportId: string) => Promise<Report | null>;

  /// Fetch the full findings history for a report. The detail view
  /// mounts a single load on enter; subsequent live refreshes come
  /// from the AtomCreated event wired in 4c-3.
  fetchFindings: (reportId: string, limit?: number) => Promise<void>;

  /// Fetch the citation count for a finding atom. Idempotent —
  /// returns immediately if already cached. Used by FindingRow's
  /// in-view-effect so we don't paint the whole list up front.
  fetchCitationCount: (atomId: string) => Promise<void>;

  /// Fetch a single finding by atom id — the atom payload, its
  /// provenance row (parent report + run + name snapshot), and its
  /// citation rows in one shot. Used by the FindingReader specialized
  /// view for cold deep-links (URL `/findings/:atomId` without any
  /// in-memory cache). Returns `null` if the atom isn't a finding or
  /// doesn't exist. As a side effect, the result is merged into
  /// `findingsByReport[report_id]` so the TabStrip's cache-walk for
  /// the tab label finds it and labels reads "Daily Briefing · 2H AGO"
  /// instead of falling back to "Finding N".
  fetchFinding: (atomId: string) => Promise<{
    atom: ReportFindingWithAtom['atom'];
    finding: ReportFinding;
    citations: ReportFindingCitation[];
  } | null>;

  /// Dispatch a manual run. Adds the report to `runningReportIds`,
  /// stamps `runDispatchedAt`, fires `POST /api/reports/:id/run`. The
  /// server responds 202 immediately; we wait for the AtomCreated
  /// event (success) or the failure poll to clear the running state.
  /// Throws on the dispatch itself failing (network / 404); the store
  /// toasts on top.
  runNow: (reportId: string) => Promise<void>;

  /// Clear the running state for one report. Called by the success
  /// detection in the AtomCreated handler, the failure detection in
  /// the detail view's poll, and the 5-minute stale guard.
  clearRunning: (reportId: string) => void;

  /// Create a new report. Returns the created `Report` so the caller
  /// (typically the editor modal) can navigate to it on success.
  /// Throws on failure with a useful message; the store toasts and the
  /// caller can keep its modal open.
  create: (input: CreateReportInput) => Promise<Report>;

  /// Patch an existing report. Returns the merged row from the server.
  /// On failure, throws and leaves the in-memory row untouched.
  update: (id: string, input: UpdateReportInput) => Promise<Report>;

  /// Convenience for the row-level toggle. Optimistic: flips the flag
  /// locally first, reverts on failure. Wired through `update_report`
  /// rather than a dedicated endpoint to keep the transport surface
  /// narrow.
  setEnabled: (id: string, enabled: boolean) => Promise<void>;

  /// Delete a report. Optimistic: removes from the list first, restores
  /// on failure. Findings outlive their producer by design — only the
  /// schedule + definition go away. (The backend already clears the
  /// dashboard's `featured_report_id` if it pointed at this report.)
  delete: (id: string) => Promise<void>;

  reset: () => void;
}

export const useReportsStore = create<ReportsStore>((set, get) => {
  return {
    reports: [],
    byId: {},
    lastFindingByReport: {},
    findingsByReport: {},
    citationCountsByAtomId: {},
    runningReportIds: new Set<string>(),
    runDispatchedAt: {},
    lastErrorAtDispatch: {},
    isLoadingList: false,
    loadError: null,
    hasSubscription: false,

    fetchAll: async () => {
      // The reports view and the sidebar's findings panel both load on
      // mount and they mount together — collapse that pair into one round
      // trip (the per-report last-finding fan-out it chains into is the
      // expensive half).
      if (get().isLoadingList) return;
      set({ isLoadingList: true, loadError: null });
      try {
        const reports = await getTransport().invoke<Report[]>('list_reports');
        const byId: Record<string, Report> = {};
        for (const r of reports) byId[r.id] = r;
        set({ reports, byId, isLoadingList: false });

        // Lazily prime last-finding for every report. Issue requests in
        // parallel; failures degrade to "no excerpt available" without
        // surfacing per-report toasts.
        await Promise.all(reports.map(r => get().fetchLastFinding(r.id)));
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        set({ isLoadingList: false, loadError: msg });
        toast.error('Failed to load reports', { description: msg });
      }
    },

    ensureSubscription: () => {
      // Idempotent — subsequent calls take the fast-path. The
      // subscription's payload contract (AtomCreated with flattened
      // AtomWithTags fields including `kind`) matches what the
      // dashboard widget already consumes.
      if (get().hasSubscription) return;
      getTransport().subscribe('atom-created', async (payload) => {
        const p = payload as { kind?: string; id?: string } | undefined;
        if (p?.kind !== 'report') return;
        const newAtomId = p.id;

        const state = get();
        const allIds = Object.keys(state.byId);
        const runningIds = Array.from(state.runningReportIds);

        // Re-prime the list view's last-finding cache + every running
        // report's full findings cache. Running reports are a strict
        // subset of allIds, so we issue findings requests for them and
        // last-finding requests for the rest.
        const runningSet = new Set(runningIds);
        const lastFindingPromises = allIds
          .filter(id => !runningSet.has(id))
          .map(id => state.fetchLastFinding(id));
        const findingsPromises = runningIds.map(id => state.fetchFindings(id));
        await Promise.all([...lastFindingPromises, ...findingsPromises]);

        // After cache rehydration, clear any running report whose
        // most-recent finding matches the new atom id.
        if (newAtomId) {
          const after = get();
          for (const id of runningIds) {
            const first = after.findingsByReport[id]?.[0];
            if (first && first.atom.id === newAtomId) {
              get().clearRunning(id);
              toast.success('New finding', {
                description: after.byId[id]?.name,
              });
            }
          }
        }
      });
      set({ hasSubscription: true });
    },

    fetchFinding: async (atomId: string) => {
      try {
        // Parallel fetch — the three calls are independent. If any of
        // the three errors with 404 we treat the whole thing as
        // "not a finding" rather than rendering a half-broken view.
        const transport = getTransport();
        const [atomRaw, provenanceRaw, citationsRaw] = await Promise.allSettled([
          transport.invoke<ReportFindingWithAtom['atom']>('get_atom', { id: atomId }),
          transport.invoke<ReportFinding>('get_finding_provenance', { atom_id: atomId }),
          transport.invoke<ReportFindingCitation[]>('list_finding_citations', { atom_id: atomId }),
        ]);

        if (atomRaw.status !== 'fulfilled') return null;
        if (provenanceRaw.status !== 'fulfilled') return null;
        if (citationsRaw.status !== 'fulfilled') return null;

        const atom = atomRaw.value;
        const finding = provenanceRaw.value;
        const citations = citationsRaw.value;

        // Side-effect: merge into findingsByReport so the TabStrip's
        // cache-walk (used to label finding tabs) and the
        // ReportDetailView's findings list both see the row without an
        // extra fetch. If the report id is null (orphaned finding —
        // report deleted, FK SET NULL), skip the cache merge; the
        // FindingReader still renders from the returned payload.
        //
        // Critically: if the cache already holds a row with the same
        // atom updated_at + finding shape, we skip the write
        // entirely. fetchFinding is called from FindingReader's mount
        // effect; if every call produced a new array reference, every
        // downstream selector that walks findingsByReport would
        // re-emit, including the one FindingReader itself uses for
        // display fallbacks. That was harmless after we removed it
        // from the effect's deps, but a no-write fast path also
        // saves an unnecessary set() round-trip.
        if (finding.report_id) {
          const reportId = finding.report_id;
          set(state => {
            const existing = state.findingsByReport[reportId] ?? [];
            const current = existing.find(f => f.atom.id === atomId);
            const isCurrent =
              current !== undefined &&
              current.atom.updated_at === atom.updated_at &&
              current.finding.run_id === finding.run_id &&
              current.finding.report_name_snapshot === finding.report_name_snapshot;

            const nextCitationCount =
              state.citationCountsByAtomId[atomId] === citations.length
                ? state.citationCountsByAtomId
                : { ...state.citationCountsByAtomId, [atomId]: citations.length };

            if (isCurrent) {
              // Cache already current — leave findingsByReport alone
              // so referential equality holds and downstream
              // subscribers don't re-emit.
              return nextCitationCount === state.citationCountsByAtomId
                ? state
                : { citationCountsByAtomId: nextCitationCount };
            }

            const merged = current
              ? existing.map(f => (f.atom.id === atomId ? { finding, atom } : f))
              : [{ finding, atom }, ...existing];
            return {
              findingsByReport: { ...state.findingsByReport, [reportId]: merged },
              citationCountsByAtomId: nextCitationCount,
            };
          });
        } else {
          set(state => {
            if (state.citationCountsByAtomId[atomId] === citations.length) return state;
            return {
              citationCountsByAtomId: {
                ...state.citationCountsByAtomId,
                [atomId]: citations.length,
              },
            };
          });
        }

        return { atom, finding, citations };
      } catch (e) {
        console.error('[reports] fetchFinding failed', atomId, e);
        return null;
      }
    },

    runNow: async (reportId: string) => {
      // Optimistically mark running before the dispatch lands, so the
      // button disables instantly. Even if dispatch fails we revert.
      const dispatchedAt = Date.now();
      const lastErrorSnapshot = get().byId[reportId]?.last_error ?? null;
      set(state => {
        const next = new Set(state.runningReportIds);
        next.add(reportId);
        return {
          runningReportIds: next,
          runDispatchedAt: { ...state.runDispatchedAt, [reportId]: dispatchedAt },
          lastErrorAtDispatch: {
            ...state.lastErrorAtDispatch,
            [reportId]: lastErrorSnapshot,
          },
        };
      });
      try {
        await getTransport().invoke('run_report_now', { report_id: reportId });
        // 202 means "accepted, will run". The completion is observed
        // via the AtomCreated subscription (success) or the detail
        // view's failure poll. Nothing else to do here.
      } catch (e) {
        // Dispatch itself failed — revert.
        get().clearRunning(reportId);
        const msg = e instanceof Error ? e.message : String(e);
        toast.error('Failed to dispatch report run', { description: msg });
        throw e;
      }
    },

    clearRunning: (reportId: string) => {
      set(state => {
        if (!state.runningReportIds.has(reportId)) return state;
        const next = new Set(state.runningReportIds);
        next.delete(reportId);
        const { [reportId]: _omitDispatch, ...restDispatched } = state.runDispatchedAt;
        const { [reportId]: _omitError, ...restErrors } = state.lastErrorAtDispatch;
        return {
          runningReportIds: next,
          runDispatchedAt: restDispatched,
          lastErrorAtDispatch: restErrors,
        };
      });
    },

    fetchOne: async (reportId: string) => {
      try {
        const report = await getTransport().invoke<Report>('get_report', { report_id: reportId });
        set(state => ({
          // Prepend if not already present, otherwise replace in place.
          // Either way the byId map is the source of truth for the
          // detail view.
          reports: state.byId[reportId]
            ? state.reports.map(r => (r.id === reportId ? report : r))
            : [report, ...state.reports],
          byId: { ...state.byId, [reportId]: report },
        }));
        return report;
      } catch (e) {
        // 404 means the report was deleted; the caller (detail view)
        // surfaces this with a toast and navigates back. Other errors
        // toast directly so they're visible.
        const msg = e instanceof Error ? e.message : String(e);
        if (/not found|404/i.test(msg)) {
          return null;
        }
        toast.error('Failed to load report', { description: msg });
        return null;
      }
    },

    fetchFindings: async (reportId: string, limit = 50) => {
      try {
        const results = await getTransport().invoke<FindingTuple[]>(
          'list_findings_for_report',
          { report_id: reportId, limit }
        );
        const decoded: ReportFindingWithAtom[] = results.map(([finding, atom]) => ({ finding, atom }));
        set(state => ({
          findingsByReport: { ...state.findingsByReport, [reportId]: decoded },
          // Keep the list-view's last-finding cache in sync — it's a
          // strict subset of what we just loaded.
          lastFindingByReport: {
            ...state.lastFindingByReport,
            [reportId]: decoded[0] ?? null,
          },
        }));
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        toast.error('Failed to load findings', { description: msg });
      }
    },

    fetchCitationCount: async (atomId: string) => {
      // Idempotent: skip if already cached.
      if (get().citationCountsByAtomId[atomId] !== undefined) return;
      try {
        const citations = await getTransport().invoke<ReportFindingCitation[]>(
          'list_finding_citations',
          { atom_id: atomId }
        );
        set(state => ({
          citationCountsByAtomId: {
            ...state.citationCountsByAtomId,
            [atomId]: citations.length,
          },
        }));
      } catch (e) {
        // Soft-fail: a missing citation count just hides the badge.
        // Don't toast — could be N concurrent failures during a
        // mass-render and we'd flood the user.
        console.error('[reports] fetchCitationCount failed', atomId, e);
      }
    },

    fetchLastFinding: async (reportId: string) => {
      try {
        const results = await getTransport().invoke<FindingTuple[]>(
          'list_findings_for_report',
          { report_id: reportId, limit: 1 }
        );
        // Server ships JSON 2-tuples (Rust `Vec<(A, B)>` semantics);
        // destructure into the clean shape we cache.
        const first: ReportFindingWithAtom | null = results[0]
          ? { finding: results[0][0], atom: results[0][1] }
          : null;
        set(state => ({
          lastFindingByReport: { ...state.lastFindingByReport, [reportId]: first },
        }));
      } catch (e) {
        // Per-report failure: leave the cache untouched, log, and let the
        // row render without an excerpt. We don't toast — N possible
        // failures would flood the user.
        console.error('[reports] fetchLastFinding failed', reportId, e);
      }
    },

    create: async (input: CreateReportInput) => {
      try {
        const created = await getTransport().invoke<Report>('create_report', input as unknown as Record<string, unknown>);
        // Prepend on success; the list view shows most-recently-created
        // first by default. The next fetchAll will re-canonicalize
        // sort, but in-the-moment ordering should feel snappy.
        set(state => ({
          reports: [created, ...state.reports.filter(r => r.id !== created.id)],
          byId: { ...state.byId, [created.id]: created },
        }));
        toast.success('Report created', { description: created.name });
        return created;
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        toast.error('Failed to create report', { description: msg });
        throw e;
      }
    },

    update: async (id: string, input: UpdateReportInput) => {
      try {
        const merged = await getTransport().invoke<Report>('update_report', {
          report_id: id,
          ...input,
        });
        set(state => ({
          reports: state.reports.map(r => (r.id === id ? merged : r)),
          byId: { ...state.byId, [id]: merged },
        }));
        return merged;
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        toast.error('Failed to update report', { description: msg });
        throw e;
      }
    },

    setEnabled: async (id: string, enabled: boolean) => {
      const prev = get().byId[id];
      if (!prev) return;
      // Optimistic local flip so the row's badge updates instantly.
      const optimistic: Report = { ...prev, enabled };
      set(state => ({
        reports: state.reports.map(r => (r.id === id ? optimistic : r)),
        byId: { ...state.byId, [id]: optimistic },
      }));
      try {
        const merged = await getTransport().invoke<Report>('update_report', {
          report_id: id,
          enabled,
        });
        set(state => ({
          reports: state.reports.map(r => (r.id === id ? merged : r)),
          byId: { ...state.byId, [id]: merged },
        }));
      } catch (e) {
        // Revert. The next fetchAll would heal anyway, but a snappy
        // revert avoids a stuck-toggle perception while the user reads
        // the toast.
        set(state => ({
          reports: state.reports.map(r => (r.id === id ? prev : r)),
          byId: { ...state.byId, [id]: prev },
        }));
        const msg = e instanceof Error ? e.message : String(e);
        toast.error(enabled ? 'Failed to enable report' : 'Failed to pause report', {
          description: msg,
        });
      }
    },

    delete: async (id: string) => {
      const prev = get().reports;
      const prevById = get().byId;
      const target = prevById[id];
      // Optimistic removal.
      set(state => ({
        reports: state.reports.filter(r => r.id !== id),
        byId: Object.fromEntries(Object.entries(state.byId).filter(([k]) => k !== id)),
      }));
      try {
        await getTransport().invoke('delete_report', { report_id: id });
        toast.success('Report deleted', {
          description: target ? `${target.name} — findings remain in your atoms` : undefined,
        });
      } catch (e) {
        // Restore on failure.
        set({ reports: prev, byId: prevById });
        const msg = e instanceof Error ? e.message : String(e);
        toast.error('Failed to delete report', { description: msg });
        throw e;
      }
    },

    reset: () => {
      // Note: the `atom-created` subscription is intentionally *not*
      // torn down here. It's session-scoped infrastructure — needed
      // by both the list view and the detail view, set up once via
      // `ensureSubscription`, and not safe to drop just because one
      // view unmounted (the other still depends on it). The data
      // caches reset; the subscription survives.
      set({
        reports: [],
        byId: {},
        lastFindingByReport: {},
        findingsByReport: {},
        citationCountsByAtomId: {},
        runningReportIds: new Set<string>(),
        runDispatchedAt: {},
        lastErrorAtDispatch: {},
        isLoadingList: false,
        loadError: null,
        // `hasSubscription` is intentionally not reset — see above.
      });
    },
  };
});
