//! The blocking Radio Browser search worker and its response folding.
//!
//! Isolates `reqwest::blocking` and the query cache on a dedicated thread so
//! neither network latency nor an async runtime ever blocks rendering or
//! input. Latest-wins coalescing drops superseded keystrokes before they cost
//! a fetch, and shutdown wins over any queued backlog.

use std::sync::mpsc::{Receiver, Sender};

use crate::app::{Action, App, SearchStatus};
use crate::model::SearchQuery;
use crate::search::{
    RadioBrowserClient, RawSearchTransport, SearchCache, SearchError, SearchResults,
};

use super::debounce::SearchDebounce;

/// A request sent to the blocking search worker thread.
pub(super) enum SearchRequest {
    Query { query: SearchQuery, generation: u64 },
    Shutdown,
}

/// A response from the search worker, tagged with the generation it was fired
/// for so the controller can drop stale results.
pub(super) struct SearchResponse {
    pub(super) generation: u64,
    pub(super) result: Result<SearchResults, SearchError>,
    pub(super) from_cache: bool,
}

/// The blocking Radio Browser worker: owns the HTTP client and the query cache,
/// isolating `reqwest::blocking` on its own thread so rendering never blocks.
/// Cached queries are served without a second network call.
pub(super) fn search_worker(rx: Receiver<SearchRequest>, tx: Sender<SearchResponse>) {
    run_search_worker(RadioBrowserClient::new(), rx, tx);
}

/// Collapse a received request plus everything already queued behind it into the
/// newest query, returning `None` when shutdown was requested.
///
/// While one search is in flight, fast typing can leave several requests waiting.
/// Every one but the last is already superseded — its result could only be
/// discarded as stale by [`apply_search_response`] — so draining the queue here
/// skips that work *before* the fetch instead of paying for it and throwing it
/// away. The controller is the only producer and the channel is FIFO, so the
/// last drained request is the newest one.
///
/// Shutdown wins over any queued query, keeping worker teardown prompt.
fn coalesce_latest_request(
    first: SearchRequest,
    rx: &Receiver<SearchRequest>,
) -> Option<(SearchQuery, u64)> {
    let mut latest = match first {
        SearchRequest::Query { query, generation } => (query, generation),
        SearchRequest::Shutdown => return None,
    };
    while let Ok(queued) = rx.try_recv() {
        match queued {
            SearchRequest::Query { query, generation } => latest = (query, generation),
            SearchRequest::Shutdown => return None,
        }
    }
    Some(latest)
}

/// The worker loop over an explicit client, so tests can drive coalescing and
/// shutdown with an injected transport instead of a network.
///
/// The cache lives here, owned by the single worker thread: it is never shared
/// across threads and needs no lock.
fn run_search_worker<T: RawSearchTransport>(
    client: Result<RadioBrowserClient<T>, SearchError>,
    rx: Receiver<SearchRequest>,
    tx: Sender<SearchResponse>,
) {
    let mut cache = SearchCache::new();
    while let Ok(request) = rx.recv() {
        // Latest-wins: stale queued keystrokes are dropped before any fetch.
        let Some((query, generation)) = coalesce_latest_request(request, &rx) else {
            break;
        };
        let response = match &client {
            Ok(client) => {
                let from_cache = cache.contains(&query);
                match client.search_cached(&mut cache, &query) {
                    Ok(results) => SearchResponse {
                        generation,
                        result: Ok(results),
                        from_cache,
                    },
                    Err(err) => SearchResponse {
                        generation,
                        result: Err(err),
                        from_cache: false,
                    },
                }
            }
            Err(err) => SearchResponse {
                generation,
                result: Err(err.clone()),
                from_cache: false,
            },
        };
        if tx.send(response).is_err() {
            break;
        }
    }
}

/// Apply a search response, ignoring stale generations and mapping recoverable
/// errors to offline/error search status.
pub(super) fn apply_search_response(
    app: &mut App,
    debounce: &SearchDebounce,
    response: SearchResponse,
) {
    if !debounce.is_current(response.generation) {
        return; // a newer keystroke superseded this search.
    }
    match response.result {
        Ok(results) => {
            app.apply(Action::SearchResults(results));
            app.apply(Action::SetSearchStatus(SearchStatus::Loaded {
                from_cache: response.from_cache,
            }));
            app.apply(Action::SetOffline(false));
        }
        Err(SearchError::Network(_)) => {
            app.apply(Action::SetOffline(true));
            app.apply(Action::SetSearchStatus(SearchStatus::Offline));
        }
        Err(SearchError::Decode(message)) => {
            app.apply(Action::SetSearchStatus(SearchStatus::Error(message)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    use crate::catalog::Catalog;
    use crate::settings::Settings;

    use super::super::debounce::SEARCH_DEBOUNCE;

    /// App plus the real debounce, so the generations under test are exactly
    /// the ones the controller would produce.
    fn controller() -> (App, SearchDebounce) {
        (
            App::new(Settings::default(), Catalog::curated()),
            SearchDebounce::new(SEARCH_DEBOUNCE),
        )
    }

    /// A canned one-station body naming the query, so a response can be traced
    /// back to the request that produced it without a network.
    fn body_for(query: &str) -> String {
        format!(
            r#"[{{"stationuuid": "uuid-{query}", "name": "Station {query}",
                  "url_resolved": "https://example.com/{query}.mp3",
                  "codec": "mp3", "bitrate": 128}}]"#
        )
    }

    /// A transport that parks on the query `"a"` until the test releases it and
    /// records every query that actually reached a fetch.
    ///
    /// Parking the first request lets the test queue later requests behind an
    /// in-flight one deterministically — no sleeps, no network, no real device.
    struct ParkingTransport {
        fetched: Arc<Mutex<Vec<String>>>,
        started: Sender<()>,
        release: Mutex<Receiver<()>>,
    }

    impl RawSearchTransport for ParkingTransport {
        fn fetch(&self, query: &SearchQuery) -> Result<String, SearchError> {
            self.fetched
                .lock()
                .expect("fetch log not poisoned")
                .push(query.as_str().to_string());
            if query.as_str() == "a" {
                let _ = self.started.send(());
                let _ = self
                    .release
                    .lock()
                    .expect("release channel not poisoned")
                    .recv();
            }
            Ok(body_for(query.as_str()))
        }
    }

    /// Queue A, B, C against a parked worker: B must never reach a fetch, and
    /// only the newest generation may update the app.
    #[test]
    fn stale_queued_search_requests_are_skipped_before_fetch_and_only_latest_updates_app() {
        let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
        let (response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let fetched = Arc::new(Mutex::new(Vec::new()));

        let transport = ParkingTransport {
            fetched: Arc::clone(&fetched),
            started: started_tx,
            release: Mutex::new(release_rx),
        };
        let worker = thread::spawn(move || {
            run_search_worker(
                Ok(RadioBrowserClient::with_transport(transport)),
                request_rx,
                response_tx,
            )
        });

        // Three keystrokes through the real debounce, so the generations under
        // test are exactly the ones the controller would produce.
        let (mut app, mut debounce) = controller();
        let now = Instant::now();
        let send = |raw: &str, debounce: &mut SearchDebounce| {
            debounce.note_query(raw, now);
            let (query, generation) = debounce
                .take_due(now + SEARCH_DEBOUNCE)
                .expect("a non-empty query is scheduled");
            request_tx
                .send(SearchRequest::Query { query, generation })
                .expect("worker is alive");
            generation
        };

        let generation_a = send("a", &mut debounce);
        // A is now parked inside the transport, so B and C queue behind it.
        started_rx.recv().expect("A reached the transport");
        let generation_b = send("b", &mut debounce);
        let generation_c = send("c", &mut debounce);
        release_tx.send(()).expect("worker is parked on release");

        let response_a = response_rx.recv().expect("A responds once released");
        let response_c = response_rx
            .recv()
            .expect("the coalesced newest query responds");
        assert_eq!(response_a.generation, generation_a);
        assert_eq!(
            response_c.generation, generation_c,
            "the queued stale request B must not produce a response"
        );

        request_tx
            .send(SearchRequest::Shutdown)
            .expect("worker is alive");
        worker.join().expect("worker shuts down cleanly");

        assert_eq!(
            *fetched.lock().expect("fetch log not poisoned"),
            vec!["a".to_string(), "c".to_string()],
            "B must be dropped before any fetch, not merely ignored afterwards"
        );
        assert!(
            response_rx.try_recv().is_err(),
            "no response is produced for the skipped generation {generation_b}"
        );

        // Only current-generation results may reach the reducer: A's in-flight
        // response is stale by the time it lands, C's is current.
        apply_search_response(&mut app, &debounce, response_a);
        assert_eq!(
            app.search_status(),
            &SearchStatus::Idle,
            "a stale response must not touch app state"
        );

        apply_search_response(&mut app, &debounce, response_c);
        assert_eq!(
            app.search_status(),
            &SearchStatus::Loaded { from_cache: false }
        );
        let visible: Vec<_> = app.visible().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            visible,
            vec!["uuid-c"],
            "only the newest query's results show"
        );
    }

    /// Shutdown queued behind pending queries wins immediately: the worker stops
    /// without fetching the backlog.
    #[test]
    fn search_worker_shutdown_skips_queued_queries() {
        let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
        let (response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let fetched = Arc::new(Mutex::new(Vec::new()));

        let transport = ParkingTransport {
            fetched: Arc::clone(&fetched),
            started: started_tx,
            release: Mutex::new(release_rx),
        };
        let worker = thread::spawn(move || {
            run_search_worker(
                Ok(RadioBrowserClient::with_transport(transport)),
                request_rx,
                response_tx,
            )
        });

        let query = |raw: &str| SearchQuery::parse(raw).expect("non-empty query");
        request_tx
            .send(SearchRequest::Query {
                query: query("a"),
                generation: 1,
            })
            .expect("worker is alive");
        started_rx.recv().expect("A reached the transport");
        request_tx
            .send(SearchRequest::Query {
                query: query("b"),
                generation: 2,
            })
            .expect("worker is alive");
        request_tx
            .send(SearchRequest::Shutdown)
            .expect("worker is alive");
        release_tx.send(()).expect("worker is parked on release");

        worker.join().expect("worker shuts down cleanly");
        assert_eq!(
            *fetched.lock().expect("fetch log not poisoned"),
            vec!["a".to_string()],
            "shutdown must win over the queued backlog"
        );
        let generations: Vec<_> = response_rx.iter().map(|r| r.generation).collect();
        assert_eq!(generations, vec![1], "only the in-flight request responds");
    }
}
