//! Debounce-and-staleness policy for online search.
//!
//! Pure timing/generation state: no channels, no terminal, no network. The
//! runtime schedules queries through it and uses its generations to drop
//! responses a newer keystroke has already superseded.

use std::time::{Duration, Instant};

use crate::model::SearchQuery;

/// Search debounce window. Within the 300–500ms band required by the spec: long
/// enough to coalesce keystrokes, short enough to feel responsive.
pub(super) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

/// Whether a query change scheduled a pending search or cleared the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryChange {
    /// The query is empty; any pending/in-flight search is cancelled.
    Cleared,
    /// A non-empty query was scheduled to fire after the debounce window.
    Scheduled,
}

/// A pending, not-yet-fired search.
#[derive(Debug, Clone)]
struct PendingSearch {
    query: SearchQuery,
    generation: u64,
    due: Instant,
}

/// Debounce-and-staleness state for online search.
///
/// Every query change bumps a monotonic generation counter, so a search fired
/// for an older keystroke can be recognized as stale and ignored when its result
/// returns ([`SearchDebounce::is_current`]). A non-empty query schedules a fire
/// time `debounce` in the future; [`SearchDebounce::take_due`] yields it once the
/// window elapses.
#[derive(Debug)]
pub(super) struct SearchDebounce {
    debounce: Duration,
    generation: u64,
    pending: Option<PendingSearch>,
}

impl SearchDebounce {
    /// Build a debounce with the given window.
    pub(super) fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            generation: 0,
            pending: None,
        }
    }

    /// The latest query generation. Results tagged with an older generation are
    /// stale.
    ///
    /// The runtime compares generations through [`Self::is_current`]; this
    /// accessor exists for tests that assert on the generation itself.
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `generation` matches the latest query generation (i.e. no newer
    /// keystroke has superseded it).
    pub(super) fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// Record a query change. Bumps the generation (invalidating any in-flight
    /// search) and either schedules a fire after the debounce window or clears
    /// the pending search when the query is empty/whitespace.
    pub(super) fn note_query(&mut self, raw: &str, now: Instant) -> QueryChange {
        // Every change advances the generation so any in-flight search for an
        // earlier query is recognized as stale when its result returns.
        self.generation += 1;
        match SearchQuery::parse(raw) {
            Ok(query) => {
                self.pending = Some(PendingSearch {
                    query,
                    generation: self.generation,
                    due: now + self.debounce,
                });
                QueryChange::Scheduled
            }
            Err(_) => {
                self.pending = None;
                QueryChange::Cleared
            }
        }
    }

    /// Take the pending search if its debounce window has elapsed by `now`.
    pub(super) fn take_due(&mut self, now: Instant) -> Option<(SearchQuery, u64)> {
        match &self.pending {
            Some(pending) if now >= pending.due => {
                let pending = self.pending.take().expect("checked Some above");
                Some((pending.query, pending.generation))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_schedules_after_the_window_and_fires_once() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();
        assert_eq!(debounce.note_query("lofi", t0), QueryChange::Scheduled);

        // Not due before the window elapses.
        assert!(debounce.take_due(t0 + Duration::from_millis(200)).is_none());

        // Due after the window; fires exactly once.
        let due = debounce.take_due(t0 + Duration::from_millis(300));
        assert!(due.is_some());
        assert_eq!(due.unwrap().0.as_str(), "lofi");
        assert!(debounce.take_due(t0 + Duration::from_millis(400)).is_none());
    }

    #[test]
    fn debounce_empty_query_clears_pending() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();
        debounce.note_query("jazz", t0);
        assert_eq!(debounce.note_query("   ", t0), QueryChange::Cleared);
        assert!(debounce.take_due(t0 + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn debounce_generations_distinguish_fresh_from_stale_results() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();

        debounce.note_query("a", t0);
        let (_, first_gen) = debounce.take_due(t0 + Duration::from_millis(300)).unwrap();
        assert!(debounce.is_current(first_gen));

        // A newer keystroke supersedes the in-flight search.
        debounce.note_query("ab", t0 + Duration::from_millis(310));
        assert!(!debounce.is_current(first_gen));
        assert!(debounce.is_current(debounce.generation()));
    }
}
