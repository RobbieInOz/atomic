//! Numbered citation ledger — the runtime's single citation system.
//!
//! The contract (ported from the reports agent, now shared by every run):
//! a tool registers the evidence it surfaces and gets a number back, the
//! tool's own output shows the model that number, and the `[N]` markers in
//! the run's final text decide which citables are actually stored. Evidence
//! the model never cites leaves no trace, so a Sources list says what an
//! answer rests on instead of logging everything the agent touched.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use regex::Regex;

/// What a citation points at. The variant decides how `source_id` is read,
/// both here and in the `chat_citations.source_type` column it is stored as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CitationSource {
    /// An atom — `source_id` is the atom id.
    Atom,
    /// A wiki article — `source_id` is the *tag* id the article belongs to,
    /// which is how wiki articles are addressed everywhere else.
    Wiki,
    /// A report finding — `source_id` is the finding atom's id. Distinct
    /// from [`CitationSource::Atom`] because a finding opens in its own
    /// reader, not the atom reader.
    Finding,
}

impl CitationSource {
    /// Stable name for storage and the wire. Kept explicit (rather than
    /// derived from the variant) because it becomes a persisted column
    /// value.
    pub fn as_str(self) -> &'static str {
        match self {
            CitationSource::Atom => "atom",
            CitationSource::Wiki => "wiki",
            CitationSource::Finding => "finding",
        }
    }
}

/// One numbered piece of evidence the model was shown and may cite.
#[derive(Debug, Clone)]
pub struct Citable {
    /// The `[N]` the model must use to cite this.
    pub number: i32,
    pub source: CitationSource,
    /// Id of the cited thing, interpreted per [`CitationSource`].
    pub source_id: String,
    /// Which chunk of the source matched, when the tool knows.
    pub chunk_index: Option<i32>,
    pub excerpt: String,
}

/// Whether tools may mint new citation numbers while a run is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CitationAdmission {
    /// Anything a tool surfaces becomes citable (chat).
    Open,
    /// Only evidence seeded before the run starts is citable. Tools may
    /// still surface other material, but it gets no number and must be
    /// presented as background the model can read and not cite (reports'
    /// `source_only` policy).
    SeededOnly,
}

/// The run's citable-evidence map: `(source, source_id)` → number, plus the
/// `[N]` resolution that turns a finished answer into stored citations.
///
/// Shared immutably with tools (`&CitationLedger` on the tool context), so
/// numbering is behind a lock rather than `&mut`.
pub struct CitationLedger {
    admission: CitationAdmission,
    inner: Mutex<LedgerInner>,
}

#[derive(Default)]
struct LedgerInner {
    by_key: HashMap<(CitationSource, String), Citable>,
    next_number: i32,
}

impl CitationLedger {
    pub fn new(admission: CitationAdmission) -> Self {
        Self {
            admission,
            inner: Mutex::new(LedgerInner {
                by_key: HashMap::new(),
                next_number: 1,
            }),
        }
    }

    /// Number a piece of evidence up front, bypassing admission — the
    /// pre-run source list a `SeededOnly` run is allowed to cite.
    pub fn seed(
        &self,
        source: CitationSource,
        source_id: &str,
        chunk_index: Option<i32>,
        excerpt: impl Into<String>,
    ) -> i32 {
        self.insert(source, source_id, chunk_index, excerpt, true)
            .expect("seeding always admits")
    }

    /// Number evidence a tool just surfaced, reusing the number when this
    /// source has been seen before. `None` means the run's admission policy
    /// refuses new evidence, and the tool must present it as uncitable
    /// background.
    pub fn register(
        &self,
        source: CitationSource,
        source_id: &str,
        chunk_index: Option<i32>,
        excerpt: impl Into<String>,
    ) -> Option<i32> {
        self.insert(
            source,
            source_id,
            chunk_index,
            excerpt,
            self.admission == CitationAdmission::Open,
        )
    }

    fn insert(
        &self,
        source: CitationSource,
        source_id: &str,
        chunk_index: Option<i32>,
        excerpt: impl Into<String>,
        admit: bool,
    ) -> Option<i32> {
        let mut inner = self.lock();
        let key = (source, source_id.to_string());
        // First sighting wins: the excerpt the model was shown alongside the
        // number is the one a reader should get back.
        if let Some(existing) = inner.by_key.get(&key) {
            return Some(existing.number);
        }
        if !admit {
            return None;
        }
        let number = inner.next_number;
        inner.next_number += 1;
        inner.by_key.insert(
            key,
            Citable {
                number,
                source,
                source_id: source_id.to_string(),
                chunk_index,
                excerpt: excerpt.into(),
            },
        );
        Some(number)
    }

    /// The citables named by `[N]` markers in `text`, in the order the
    /// markers appear.
    ///
    /// The parsing contract, which every consumer of stored citations
    /// depends on:
    /// - A citation's stored index is the **marker number N**, not its
    ///   position in the text. Readers render `[N]` and look the citation
    ///   up by that same N, so `[3]` before `[1]` must not renumber.
    /// - Repeated markers collapse to one citable — one row per marker,
    ///   however many times the model wrote it.
    /// - Markers with no matching citable are dropped with a warning: the
    ///   model invented a number, and a dangling citation is worse than a
    ///   missing one.
    pub fn cited_in(&self, text: &str) -> Vec<Citable> {
        static MARKER: OnceLock<Option<Regex>> = OnceLock::new();
        let Some(marker) = MARKER
            .get_or_init(|| Regex::new(r"\[(\d+)\]").ok())
            .as_ref()
        else {
            tracing::error!("[agent_runtime] citation marker regex failed to compile");
            return Vec::new();
        };

        let by_number = self.by_number();
        let mut seen: HashSet<i32> = HashSet::new();
        let mut cited = Vec::new();
        for capture in marker.captures_iter(text) {
            let Some(digits) = capture.get(1) else {
                continue;
            };
            let Ok(number) = digits.as_str().parse::<i32>() else {
                continue;
            };
            if !seen.insert(number) {
                continue;
            }
            match by_number.get(&number) {
                Some(citable) => cited.push(citable.clone()),
                None => tracing::warn!(
                    citation_number = number,
                    "[agent_runtime] agent cited an unknown number; dropping"
                ),
            }
        }
        cited
    }

    /// The citables named by an explicit list of numbers — the
    /// `CITATIONS_USED:` trailer of the long-form markdown contract.
    /// Unknown numbers are dropped, same as [`Self::cited_in`].
    pub fn resolve(&self, numbers: &[i32]) -> Vec<Citable> {
        let by_number = self.by_number();
        let mut seen: HashSet<i32> = HashSet::new();
        numbers
            .iter()
            .filter(|number| seen.insert(**number))
            .filter_map(|number| by_number.get(number).cloned())
            .collect()
    }

    /// Every citable registered so far, ordered by number.
    pub fn snapshot(&self) -> Vec<Citable> {
        let mut all: Vec<Citable> = self.lock().by_key.values().cloned().collect();
        all.sort_by_key(|citable| citable.number);
        all
    }

    fn by_number(&self) -> HashMap<i32, Citable> {
        self.lock()
            .by_key
            .values()
            .map(|citable| (citable.number, citable.clone()))
            .collect()
    }

    /// A panic while a tool held the ledger must not poison the rest of the
    /// turn: the map is append-only, so whatever landed before the panic is
    /// still coherent.
    fn lock(&self) -> MutexGuard<'_, LedgerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> CitationLedger {
        CitationLedger::new(CitationAdmission::Open)
    }

    #[test]
    fn numbers_are_assigned_on_first_sighting_and_reused() {
        let ledger = open();
        assert_eq!(
            ledger.register(CitationSource::Atom, "a1", Some(0), "first"),
            Some(1)
        );
        assert_eq!(
            ledger.register(CitationSource::Atom, "a2", None, "second"),
            Some(2)
        );
        // Same source again — same number, original excerpt.
        assert_eq!(
            ledger.register(CitationSource::Atom, "a1", None, "rewritten"),
            Some(1)
        );
        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].excerpt, "first");
    }

    #[test]
    fn seeded_only_refuses_evidence_the_run_did_not_start_with() {
        let ledger = CitationLedger::new(CitationAdmission::SeededOnly);
        assert_eq!(ledger.seed(CitationSource::Atom, "a1", None, "source"), 1);
        assert_eq!(
            ledger.register(CitationSource::Atom, "a1", None, "source"),
            Some(1),
            "seeded evidence stays citable"
        );
        assert_eq!(
            ledger.register(CitationSource::Atom, "a2", None, "context"),
            None,
            "new evidence is background only"
        );
        assert_eq!(ledger.snapshot().len(), 1);
    }

    #[test]
    fn only_cited_numbers_are_returned_and_markers_keep_their_number() {
        let ledger = open();
        for id in ["a1", "a2", "a3"] {
            ledger.register(CitationSource::Atom, id, None, id);
        }
        // Out of sequence, one marker repeated, one never cited.
        let cited = ledger.cited_in("First [3]. Then [1]. Again [1].");
        assert_eq!(cited.len(), 2);
        assert_eq!(cited[0].number, 3);
        assert_eq!(cited[0].source_id, "a3");
        assert_eq!(cited[1].number, 1);
        assert_eq!(cited[1].source_id, "a1");
    }

    #[test]
    fn invented_markers_are_dropped() {
        let ledger = open();
        ledger.register(CitationSource::Atom, "a1", None, "only source");
        let cited = ledger.cited_in("Real [1]. Hallucinated [9].");
        assert_eq!(cited.len(), 1);
        assert_eq!(cited[0].source_id, "a1");
    }

    #[test]
    fn resolve_maps_an_explicit_trailer_list() {
        let ledger = open();
        ledger.register(CitationSource::Atom, "a1", None, "one");
        ledger.register(CitationSource::Atom, "a2", None, "two");
        let cited = ledger.resolve(&[2, 2, 7]);
        assert_eq!(cited.len(), 1);
        assert_eq!(cited[0].source_id, "a2");
    }
}
