//! Radio Browser client, response normalization, and query cache.
//!
//! This module owns the online search boundary. It fetches station records from
//! the Radio Browser API, parses the untrusted JSON once into always-valid
//! [`Station`] values (per "Parse, don't validate"), ranks them with the shared
//! catalog scoring, and caches results by normalized [`SearchQuery`].
//!
//! Raw Radio Browser response shapes are an implementation detail: the
//! [`RawStation`] DTO never leaves this module. The transport seam
//! ([`RawSearchTransport`]) deals only in raw JSON text, so callers and tests can
//! inject canned responses without a network or leaking the wire format.
//!
//! Network and decode problems are recoverable [`SearchError`] values for the app
//! to surface as an offline/error state; this module never panics on a failed or
//! malformed remote response.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;

use crate::catalog::Stations;
use crate::model::{
    BitrateKbps, CodecKind, SearchQuery, Station, StationId, StationName, StationSource, StreamUrl,
};

/// Default Radio Browser API mirror. A fixed mirror keeps the MVP simple; SRV
/// based server discovery can replace this later without touching callers.
const DEFAULT_BASE_URL: &str = "https://de1.api.radio-browser.info";

/// Maximum number of stations requested from a single search.
const SEARCH_LIMIT: &str = "50";

/// Recoverable failures from an online search.
///
/// These are typed and recoverable so the app can render an offline/error state
/// and retry, rather than treating a flaky network as a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// The request could not be sent or returned a transport/HTTP error.
    Network(String),
    /// The response body was not valid Radio Browser JSON.
    Decode(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Network(detail) => write!(f, "search network error: {detail}"),
            SearchError::Decode(detail) => write!(f, "search decode error: {detail}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// A behavior-rich collection of online search results.
///
/// Wrapping the stations keeps ranking with the data and gives callers a typed
/// output instead of a bare `Vec<Station>`. Results are ranked by playback
/// likelihood and popularity using the shared catalog scoring on construction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults(Vec<Station>);

impl SearchResults {
    /// Build ranked results from normalized stations using catalog ranking.
    fn ranked(stations: Vec<Station>) -> Self {
        let ranked = stations
            .into_iter()
            .collect::<Stations>()
            .ranked()
            .into_vec();
        Self(ranked)
    }

    /// An empty result set (used for no-op empty queries).
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Station> {
        self.0.iter()
    }

    pub fn as_slice(&self) -> &[Station] {
        &self.0
    }

    /// Consume the results, yielding the underlying ranked stations.
    pub fn into_vec(self) -> Vec<Station> {
        self.0
    }
}

/// A cache of search results keyed by normalized [`SearchQuery`].
///
/// The key is the already-normalized (trimmed) query, so repeated lookups for
/// the same user input hit the cache. `get` returns a clone so callers always
/// receive a stable, independent snapshot of the cached results.
#[derive(Debug, Clone, Default)]
pub struct SearchCache {
    entries: HashMap<SearchQuery, SearchResults>,
}

impl SearchCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a cloned, stable copy of cached results for a query, if present.
    pub fn get(&self, query: &SearchQuery) -> Option<SearchResults> {
        self.entries.get(query).cloned()
    }

    /// Whether the cache holds results for a query.
    pub fn contains(&self, query: &SearchQuery) -> bool {
        self.entries.contains_key(query)
    }

    /// Store results for a normalized query, replacing any existing entry.
    pub fn insert(&mut self, query: SearchQuery, results: SearchResults) {
        self.entries.insert(query, results);
    }

    /// How many distinct queries are cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop all cached results.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The raw transport seam for fetching Radio Browser search responses.
///
/// Implementations return the raw JSON body as text. Keeping the seam at the
/// text boundary means the wire DTO stays private to this module and tests can
/// inject canned responses without a network.
pub trait RawSearchTransport {
    /// Fetch the raw JSON body for a station search.
    fn fetch(&self, query: &SearchQuery) -> Result<String, SearchError>;
}

/// The real HTTP transport backed by `reqwest`'s blocking client.
pub struct HttpTransport {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    /// Build a transport against the default Radio Browser mirror.
    pub fn new() -> Result<Self, SearchError> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// Build a transport against a specific base URL.
    pub fn with_base_url(base: impl Into<String>) -> Result<Self, SearchError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|err| SearchError::Network(err.to_string()))?;
        Ok(Self {
            base_url: base.into().trim_end_matches('/').to_string(),
            client,
        })
    }
}

impl RawSearchTransport for HttpTransport {
    fn fetch(&self, query: &SearchQuery) -> Result<String, SearchError> {
        let url = format!("{}/json/stations/search", self.base_url);
        let response = self
            .client
            .get(&url)
            .header("User-Agent", "wave-tui/0.1")
            .query(&[
                ("name", query.as_str()),
                ("limit", SEARCH_LIMIT),
                ("order", "votes"),
                ("reverse", "true"),
                ("hidebroken", "true"),
            ])
            .send()
            .map_err(|err| SearchError::Network(err.to_string()))?
            .error_for_status()
            .map_err(|err| SearchError::Network(err.to_string()))?;
        response
            .text()
            .map_err(|err| SearchError::Network(err.to_string()))
    }
}

/// Radio Browser online search client.
///
/// Generic over the transport so production uses [`HttpTransport`] while tests
/// inject a fake. Normalization and ranking are transport-independent.
pub struct RadioBrowserClient<T: RawSearchTransport = HttpTransport> {
    transport: T,
}

impl RadioBrowserClient<HttpTransport> {
    /// Build a client backed by the default HTTP transport.
    pub fn new() -> Result<Self, SearchError> {
        Ok(Self::with_transport(HttpTransport::new()?))
    }
}

impl<T: RawSearchTransport> RadioBrowserClient<T> {
    /// Build a client over an explicit transport (used for injection/tests).
    pub fn with_transport(transport: T) -> Self {
        Self { transport }
    }

    /// Run an online search for a normalized query, returning ranked results.
    ///
    /// The query type already guarantees a non-empty value, so an arbitrary
    /// empty-query API call cannot occur through this path.
    pub fn search(&self, query: &SearchQuery) -> Result<SearchResults, SearchError> {
        let body = self.transport.fetch(query)?;
        let stations = parse_body(&body)?;
        Ok(SearchResults::ranked(stations))
    }

    /// Parse raw boundary text into a query and search, treating an empty or
    /// whitespace-only input as a no-op empty result rather than an API call.
    pub fn search_text(&self, raw: &str) -> Result<SearchResults, SearchError> {
        match SearchQuery::parse(raw) {
            Ok(query) => self.search(&query),
            Err(_) => Ok(SearchResults::empty()),
        }
    }

    /// Search with a cache: return cached results when present, otherwise fetch,
    /// cache, and return. Repeated queries avoid a second transport call.
    pub fn search_cached(
        &self,
        cache: &mut SearchCache,
        query: &SearchQuery,
    ) -> Result<SearchResults, SearchError> {
        if let Some(cached) = cache.get(query) {
            return Ok(cached);
        }
        let results = self.search(query)?;
        cache.insert(query.clone(), results.clone());
        Ok(results)
    }
}

/// Raw Radio Browser station record. Private to this module: the wire shape must
/// not leak past normalization. Missing fields default so partial records still
/// decode and are filtered during normalization.
#[derive(Debug, Deserialize)]
struct RawStation {
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    votes: u32,
    #[serde(default)]
    clickcount: u32,
}

/// Parse a raw JSON body into normalized, always-valid stations.
///
/// Decode errors are recoverable [`SearchError::Decode`]. Individual records
/// that fail normalization (missing id/name/url) are skipped rather than failing
/// the whole search, so one bad row does not discard good results.
fn parse_body(body: &str) -> Result<Vec<Station>, SearchError> {
    let raw: Vec<RawStation> =
        serde_json::from_str(body).map_err(|err| SearchError::Decode(err.to_string()))?;
    Ok(raw.into_iter().filter_map(normalize).collect())
}

/// Normalize one raw record into a [`Station`], or `None` if it lacks the typed
/// invariants a station requires (non-empty id/name and a usable stream URL).
fn normalize(raw: RawStation) -> Option<Station> {
    let id = StationId::new(raw.stationuuid).ok()?;
    let name = StationName::new(raw.name).ok()?;

    // Prefer the resolved URL (a direct stream endpoint per the audio spike),
    // falling back to the advertised URL when resolution is absent.
    let url_raw = if raw.url_resolved.trim().is_empty() {
        raw.url
    } else {
        raw.url_resolved
    };
    let url = StreamUrl::parse(url_raw).ok()?;

    Some(Station {
        id,
        name,
        url,
        homepage: non_empty(raw.homepage),
        country: non_empty(raw.country),
        language: non_empty(raw.language),
        tags: parse_tags(&raw.tags),
        codec: CodecKind::parse(&raw.codec),
        // Radio Browser reports `0` for unknown bitrate; `BitrateKbps` rejects it,
        // so an unknown/invalid bitrate normalizes to `None`.
        bitrate: BitrateKbps::new(raw.bitrate).ok(),
        votes: Some(raw.votes),
        click_count: Some(raw.clickcount),
        source: StationSource::RadioBrowser,
    })
}

/// Trim a boundary string to `Some(value)`, or `None` when empty.
fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Split Radio Browser's comma-separated tag string into trimmed, non-empty tags.
fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A fake transport returning a canned body and counting calls, so tests can
    /// assert normalization, ranking, and cache behavior without a network.
    struct FakeTransport {
        body: String,
        calls: Cell<usize>,
    }

    impl FakeTransport {
        fn new(body: impl Into<String>) -> Self {
            Self {
                body: body.into(),
                calls: Cell::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.get()
        }
    }

    impl RawSearchTransport for FakeTransport {
        fn fetch(&self, _query: &SearchQuery) -> Result<String, SearchError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.body.clone())
        }
    }

    /// A transport that always fails, modeling an offline/unreachable API.
    struct FailingTransport;

    impl RawSearchTransport for FailingTransport {
        fn fetch(&self, _query: &SearchQuery) -> Result<String, SearchError> {
            Err(SearchError::Network("offline".to_string()))
        }
    }

    fn query(raw: &str) -> SearchQuery {
        SearchQuery::parse(raw).unwrap()
    }

    const ONE_STATION_BODY: &str = r#"[
        {
            "stationuuid": "uuid-1",
            "name": "Lofi Beats",
            "url": "http://example.com/advertised",
            "url_resolved": "https://example.com/resolved.mp3",
            "homepage": "https://example.com/",
            "country": "Japan",
            "language": "Japanese",
            "tags": "lofi, chill , ,beats",
            "codec": "MP3",
            "bitrate": 128,
            "votes": 42,
            "clickcount": 99
        }
    ]"#;

    #[test]
    fn normalize_parses_every_documented_field() {
        let stations = parse_body(ONE_STATION_BODY).unwrap();
        assert_eq!(stations.len(), 1);
        let station = &stations[0];

        assert_eq!(station.id.as_str(), "uuid-1");
        assert_eq!(station.name.as_str(), "Lofi Beats");
        // url_resolved is preferred over the advertised url.
        assert_eq!(station.url.as_str(), "https://example.com/resolved.mp3");
        assert_eq!(station.homepage.as_deref(), Some("https://example.com/"));
        assert_eq!(station.country.as_deref(), Some("Japan"));
        assert_eq!(station.language.as_deref(), Some("Japanese"));
        // Tags are trimmed and empties dropped.
        assert_eq!(station.tags, vec!["lofi", "chill", "beats"]);
        assert_eq!(station.codec, CodecKind::Mp3);
        assert_eq!(station.bitrate.map(BitrateKbps::get), Some(128));
        assert_eq!(station.votes, Some(42));
        assert_eq!(station.click_count, Some(99));
        assert_eq!(station.source, StationSource::RadioBrowser);
    }

    #[test]
    fn normalize_falls_back_to_advertised_url_and_zero_bitrate_is_none() {
        let body = r#"[
            {
                "stationuuid": "uuid-2",
                "name": "No Resolved",
                "url": "https://example.com/only-advertised",
                "url_resolved": "",
                "codec": "",
                "bitrate": 0,
                "votes": 0,
                "clickcount": 0
            }
        ]"#;
        let stations = parse_body(body).unwrap();
        assert_eq!(stations.len(), 1);
        let station = &stations[0];
        assert_eq!(station.url.as_str(), "https://example.com/only-advertised");
        // Unknown codec and zero bitrate normalize to typed "unknown" values.
        assert_eq!(station.codec, CodecKind::Unknown);
        assert_eq!(station.bitrate, None);
        assert!(station.tags.is_empty());
        assert_eq!(station.votes, Some(0));
    }

    #[test]
    fn normalize_skips_records_missing_required_invariants() {
        let body = r#"[
            { "stationuuid": "", "name": "No Id", "url_resolved": "https://example.com/a.mp3" },
            { "stationuuid": "ok", "name": "", "url_resolved": "https://example.com/b.mp3" },
            { "stationuuid": "ok2", "name": "No Url", "url_resolved": "", "url": "ftp://nope" },
            { "stationuuid": "good", "name": "Good", "url_resolved": "https://example.com/c.mp3" }
        ]"#;
        let stations = parse_body(body).unwrap();
        // Only the fully valid record survives normalization.
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].id.as_str(), "good");
    }

    #[test]
    fn decode_error_is_recoverable_not_a_panic() {
        let err = parse_body("not json").unwrap_err();
        assert!(matches!(err, SearchError::Decode(_)));
    }

    #[test]
    fn search_ranks_results_by_playback_likelihood() {
        let body = r#"[
            { "stationuuid": "unknown", "name": "Unknown Codec",
              "url_resolved": "https://example.com/u", "codec": "", "votes": 1000 },
            { "stationuuid": "mp3", "name": "Mp3 Station",
              "url_resolved": "https://example.com/m.mp3", "codec": "mp3", "bitrate": 128, "votes": 1 }
        ]"#;
        let client = RadioBrowserClient::with_transport(FakeTransport::new(body));
        let results = client.search(&query("any")).unwrap();
        let ids: Vec<_> = results.iter().map(|s| s.id.as_str()).collect();
        // The playable MP3 outranks the unknown-codec station despite fewer votes.
        assert_eq!(ids, vec!["mp3", "unknown"]);
    }

    #[test]
    fn search_text_empty_query_is_a_noop_without_api_call() {
        let client = RadioBrowserClient::with_transport(FakeTransport::new(ONE_STATION_BODY));
        let results = client.search_text("\t\n ").unwrap();
        assert!(results.is_empty());
        // The empty query short-circuits before reaching the transport.
        assert_eq!(client.transport.call_count(), 0);
    }

    #[test]
    fn cache_returns_cloned_stable_results_for_repeated_queries() {
        let mut cache = SearchCache::new();
        let q = query("jazz");
        let results = parse_one_into_results();
        cache.insert(q.clone(), results.clone());

        let first = cache.get(&q).unwrap();
        let second = cache.get(&q).unwrap();
        // Repeated lookups return equal, independent snapshots.
        assert_eq!(first, second);
        assert_eq!(first, results);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn search_cached_fetches_once_then_serves_from_cache() {
        let client = RadioBrowserClient::with_transport(FakeTransport::new(ONE_STATION_BODY));
        let mut cache = SearchCache::new();
        let q = query("lofi");

        let first = client.search_cached(&mut cache, &q).unwrap();
        let second = client.search_cached(&mut cache, &q).unwrap();

        assert_eq!(first, second);
        assert!(cache.contains(&q));
        // The transport was hit exactly once across two identical queries.
        assert_eq!(client.transport.call_count(), 1);
    }

    #[test]
    fn network_failure_is_recoverable_result() {
        let client = RadioBrowserClient::with_transport(FailingTransport);
        let err = client.search(&query("anything")).unwrap_err();
        assert_eq!(err, SearchError::Network("offline".to_string()));
    }

    fn parse_one_into_results() -> SearchResults {
        SearchResults::ranked(parse_body(ONE_STATION_BODY).unwrap())
    }
}
