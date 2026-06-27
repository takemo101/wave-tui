//! Curated stations, station ranking, and session validation state.
//!
//! This module owns the built-in station catalog (a small, manually curated set
//! of candidates), the playback-likelihood ranking used to order stations, and
//! the temporary per-session failed-station tracking. It depends only on
//! [`crate::model`] domain primitives so it stays free of search/audio adapter
//! details.
//!
//! Catalog definitions are boundary data: the curated table is parsed once into
//! always-valid [`Station`] values. A malformed curated entry is a programmer
//! defect (per the error-classification guideline), so building panics loudly in
//! tests rather than silently degrading the catalog.
//!
//! URL policy follows the audio spike findings: station URLs are treated as
//! direct stream URLs by default. `/stream` is appended only for curated entries
//! that explicitly opt in via [`Curated::append_stream`]; arbitrary URLs are
//! never modified.

use std::collections::HashSet;

use crate::model::{
    BitrateKbps, CodecKind, DomainError, Station, StationId, StationName, StationSource, StreamUrl,
};

/// Top-level split of the built-in catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// Background music for work sessions.
    Music,
    /// Spoken-word and news/talk programming.
    SpokenNews,
}

impl Section {
    /// Every section, in display order.
    pub const ALL: [Section; 2] = [Section::Music, Section::SpokenNews];

    /// Human-readable section heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Music => "Music",
            Section::SpokenNews => "Spoken / News",
        }
    }

    /// Categories that belong to this section, in display order.
    pub fn categories(self) -> &'static [Category] {
        match self {
            Section::Music => &[
                Category::Lofi,
                Category::Ambient,
                Category::Jazz,
                Category::Classical,
                Category::Electronic,
            ],
            Section::SpokenNews => &[Category::News, Category::Talk],
        }
    }
}

/// A small, focused catalog category. Each category belongs to one [`Section`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Lofi,
    Ambient,
    Jazz,
    Classical,
    Electronic,
    News,
    Talk,
}

impl Category {
    /// The section this category lives under.
    pub fn section(self) -> Section {
        match self {
            Category::Lofi
            | Category::Ambient
            | Category::Jazz
            | Category::Classical
            | Category::Electronic => Section::Music,
            Category::News | Category::Talk => Section::SpokenNews,
        }
    }

    /// Human-readable category label.
    pub fn title(self) -> &'static str {
        match self {
            Category::Lofi => "Lofi",
            Category::Ambient => "Ambient",
            Category::Jazz => "Jazz",
            Category::Classical => "Classical",
            Category::Electronic => "Electronic",
            Category::News => "News",
            Category::Talk => "Talk",
        }
    }
}

/// A curated station definition: trusted boundary data parsed into a [`Station`].
///
/// Fields are raw because they describe hand-authored catalog rows; they are
/// validated once when [`Self::build`] constructs the always-valid `Station`.
struct Curated {
    id: &'static str,
    name: &'static str,
    category: Category,
    /// Base stream URL. Treated as a direct URL unless `append_stream` is set.
    base_url: &'static str,
    /// When `true`, `/stream` is appended to `base_url` (Icecast/Shoutcast mounts
    /// that require it). Defaults to direct elsewhere; never applied blindly.
    append_stream: bool,
    /// The station's real public homepage, when known. Anchors the candidate to
    /// a verifiable station even when the raw stream endpoint may rotate.
    homepage: Option<&'static str>,
    country: Option<&'static str>,
    language: Option<&'static str>,
    tags: &'static [&'static str],
    codec: &'static str,
    bitrate: Option<u32>,
    votes: Option<u32>,
    click_count: Option<u32>,
}

impl Curated {
    /// Build the always-valid [`Station`]. Panics on malformed curated data,
    /// which is a code defect rather than a recoverable runtime error.
    fn build(&self) -> Station {
        let url = resolve_curated_url(self.base_url, self.append_stream).unwrap_or_else(|err| {
            panic!("curated station {:?} has an invalid url: {err}", self.id)
        });
        Station {
            id: StationId::new(self.id)
                .unwrap_or_else(|err| panic!("curated station id {:?}: {err}", self.id)),
            name: StationName::new(self.name)
                .unwrap_or_else(|err| panic!("curated station name {:?}: {err}", self.name)),
            url,
            homepage: self.homepage.map(str::to_string),
            country: self.country.map(str::to_string),
            language: self.language.map(str::to_string),
            tags: self.tags.iter().map(|t| t.to_string()).collect(),
            codec: CodecKind::parse(self.codec),
            bitrate: self.bitrate.map(|kbps| {
                BitrateKbps::new(kbps)
                    .unwrap_or_else(|err| panic!("curated station {:?} bitrate: {err}", self.id))
            }),
            votes: self.votes,
            click_count: self.click_count,
            source: StationSource::BuiltIn,
        }
    }
}

/// Resolve a curated base URL into a [`StreamUrl`], appending `/stream` only when
/// the entry explicitly opts in. Direct by default, per the audio spike caveat.
fn resolve_curated_url(base: &str, append_stream: bool) -> Result<StreamUrl, DomainError> {
    if append_stream {
        let trimmed = base.trim_end_matches('/');
        StreamUrl::parse(format!("{trimmed}/stream"))
    } else {
        StreamUrl::parse(base)
    }
}

/// The manually curated candidate set.
///
/// Intentionally small: a handful of recognizable candidates per category rather
/// than broad Radio Browser dumps. The Spoken/News section deliberately carries
/// both Japanese and English candidates.
///
/// Stream URLs are best-effort entries drawn from public radio directories and
/// the stations' own players. They are *not* verified by this module's tests
/// (which run without network); reachability is a runtime concern handled by the
/// audio path and background validation, and individual endpoints may rotate or
/// geo-restrict. Open direct MP3/AAC streams for Japanese hard-news are scarce
/// (most are HLS or region-locked), so the Japanese candidates here are
/// globally accessible community/talk stations anchored to real station
/// homepages. Tests only assert that every row builds into a well-typed
/// `Station`.
const CURATED: &[Curated] = &[
    // ---- Music: Lofi ----
    Curated {
        id: "music.lofi.chillhop",
        name: "Chillhop Radio",
        category: Category::Lofi,
        base_url: "https://streams.fluxfm.de/Chillhop/mp3-320",
        append_stream: false,
        homepage: Some("https://www.chillhop.com/"),
        country: Some("Germany"),
        language: Some("Instrumental"),
        tags: &["lofi", "chillhop", "beats"],
        codec: "mp3",
        bitrate: Some(320),
        votes: Some(1400),
        click_count: Some(9000),
    },
    // ---- Music: Ambient ----
    Curated {
        id: "music.ambient.dronezone",
        name: "SomaFM Drone Zone",
        category: Category::Ambient,
        base_url: "https://ice1.somafm.com/dronezone-128-mp3",
        append_stream: false,
        homepage: Some("https://somafm.com/dronezone/"),
        country: Some("United States"),
        language: Some("Instrumental"),
        tags: &["ambient", "atmospheric", "space"],
        codec: "mp3",
        bitrate: Some(128),
        votes: Some(1100),
        click_count: Some(7200),
    },
    // ---- Music: Jazz ----
    Curated {
        id: "music.jazz.jazz24",
        name: "Jazz24",
        category: Category::Jazz,
        base_url: "https://live.amperwave.net/direct/ppm-jazz24aac-ibc1",
        append_stream: false,
        homepage: Some("https://www.jazz24.org/"),
        country: Some("United States"),
        language: Some("Instrumental"),
        tags: &["jazz", "smooth", "classics"],
        codec: "aac",
        bitrate: Some(64),
        votes: Some(950),
        click_count: Some(5400),
    },
    // ---- Music: Classical ----
    Curated {
        id: "music.classical.venice",
        name: "Venice Classic Radio",
        category: Category::Classical,
        base_url: "https://uk2.streamingpulse.com/ssl/vcr1",
        append_stream: false,
        homepage: Some("https://www.veniceclassicradio.eu/"),
        country: Some("Italy"),
        language: Some("Instrumental"),
        tags: &["classical", "baroque", "orchestral"],
        codec: "mp3",
        bitrate: Some(128),
        votes: Some(800),
        click_count: Some(4100),
    },
    // ---- Music: Electronic ----
    Curated {
        id: "music.electronic.beatblender",
        name: "SomaFM Beat Blender",
        category: Category::Electronic,
        base_url: "https://ice2.somafm.com/beatblender-128-mp3",
        append_stream: false,
        homepage: Some("https://somafm.com/beatblender/"),
        country: Some("United States"),
        language: Some("Instrumental"),
        tags: &["electronic", "downtempo", "house"],
        codec: "mp3",
        bitrate: Some(128),
        votes: Some(1000),
        click_count: Some(6300),
    },
    // Real station that serves its Icecast mount at `/stream`; the one curated
    // entry that opts into `/stream` appending (base + "/stream").
    Curated {
        id: "music.electronic.54house",
        name: "54 House FM",
        category: Category::Electronic,
        base_url: "https://54house.fm:9013",
        append_stream: true,
        homepage: Some("https://54house.fm/"),
        country: Some("United Kingdom"),
        language: Some("Instrumental"),
        tags: &["electronic", "house", "deep-house"],
        codec: "mp3",
        bitrate: Some(128),
        votes: Some(420),
        click_count: Some(2100),
    },
    // ---- Spoken / News: News (English) ----
    Curated {
        id: "spoken.news.npr",
        name: "NPR Program Stream",
        category: Category::News,
        base_url: "https://npr-ice.streamguys1.com/live.mp3",
        append_stream: false,
        homepage: Some("https://www.npr.org/"),
        country: Some("United States"),
        language: Some("English"),
        tags: &["news", "public-radio"],
        codec: "mp3",
        bitrate: Some(128),
        votes: Some(1300),
        click_count: Some(8800),
    },
    Curated {
        id: "spoken.news.bbc-ws",
        name: "BBC World Service",
        category: Category::News,
        base_url: "https://stream.live.vc.bbcmedia.co.uk/bbc_world_service",
        append_stream: false,
        homepage: Some("https://www.bbc.co.uk/worldserviceradio"),
        country: Some("United Kingdom"),
        language: Some("English"),
        tags: &["news", "world", "talk"],
        codec: "aac",
        bitrate: Some(96),
        votes: Some(1500),
        click_count: Some(9600),
    },
    // ---- Spoken / News: Talk (Japanese) ----
    // Japanese candidates are globally accessible community/talk stations
    // anchored to real homepages; see the module/CURATED notes on JP stream
    // availability.
    Curated {
        id: "spoken.talk.shonan-beach-fm",
        name: "湘南ビーチFM (Shonan Beach FM)",
        category: Category::Talk,
        base_url: "https://shonanbeachfm.out.airtime.pro/shonanbeachfm_a",
        append_stream: false,
        homepage: Some("https://www.beachfm.co.jp/"),
        country: Some("Japan"),
        language: Some("Japanese"),
        tags: &["talk", "community", "music"],
        codec: "aac",
        bitrate: Some(128),
        votes: Some(360),
        click_count: Some(1700),
    },
    Curated {
        id: "spoken.talk.love-fm",
        name: "LOVE FM (Fukuoka)",
        category: Category::Talk,
        base_url: "https://lovefm.out.airtime.pro/lovefm_a",
        append_stream: false,
        homepage: Some("https://lovefm.co.jp/"),
        country: Some("Japan"),
        language: Some("Japanese"),
        tags: &["talk", "multilingual", "community"],
        codec: "aac",
        bitrate: Some(128),
        votes: Some(300),
        click_count: Some(1400),
    },
];

/// A behavior-rich collection of stations.
///
/// Wrapping `Vec<Station>` keeps ranking and failed-station filtering in one
/// place instead of scattering loops across `app`, `ui`, and `catalog`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stations(Vec<Station>);

impl Stations {
    /// An empty collection.
    pub fn new() -> Self {
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

    /// Consume the collection, yielding the underlying stations.
    pub fn into_vec(self) -> Vec<Station> {
        self.0
    }

    /// A copy ordered by descending playback-likelihood [`station_score`].
    ///
    /// Ties break by descending votes, then ascending id, so ordering is stable
    /// and deterministic for tests and UI.
    pub fn ranked(&self) -> Stations {
        let mut ranked = self.0.clone();
        ranked.sort_by(|a, b| {
            station_score(b)
                .cmp(&station_score(a))
                .then_with(|| b.votes.unwrap_or(0).cmp(&a.votes.unwrap_or(0)))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Stations(ranked)
    }

    /// A copy with stations currently marked failed for this session removed.
    pub fn without_failed(&self, health: &SessionStationHealth) -> Stations {
        Stations(
            self.0
                .iter()
                .filter(|station| !health.is_failed(&station.id))
                .cloned()
                .collect(),
        )
    }
}

impl FromIterator<Station> for Stations {
    fn from_iter<I: IntoIterator<Item = Station>>(iter: I) -> Self {
        Stations(iter.into_iter().collect())
    }
}

/// Score a station by estimated playback likelihood and popularity.
///
/// Higher is better. The codec term dominates so a known-playable MP3/AAC stream
/// always outranks an unknown-codec one regardless of popularity. Bitrate
/// rewards a sane streaming range, and popularity (votes/clicks) acts as a
/// bounded tiebreaker. Stream URLs are non-empty and direct by the [`StreamUrl`]
/// invariant, so that requirement is satisfied structurally rather than scored.
pub fn station_score(station: &Station) -> u32 {
    let codec = match station.codec {
        CodecKind::Mp3 | CodecKind::Aac => 1000,
        CodecKind::Other(_) => 200,
        CodecKind::Unknown => 0,
    };

    // `BitrateKbps` already guarantees a positive value within its sane upper
    // bound, so the upper edge of each band relies on that type invariant rather
    // than an arbitrary cap here.
    let bitrate = match station.bitrate.map(BitrateKbps::get) {
        // The sweet spot for streaming radio: clear audio, modest bandwidth.
        Some(kbps) if (96..=320).contains(&kbps) => 400,
        // Other in-range bitrates still play fine but are less ideal.
        Some(kbps) if kbps >= 48 => 250,
        // A known but very low bitrate: playable, but poor quality.
        Some(_) => 100,
        // Unknown bitrate: ranked above a known-very-low stream because absence
        // of metadata is not evidence of bad quality (many good streams omit it).
        None => 150,
    };

    // Bounded popularity so it tunes ordering within a codec/bitrate tier but
    // never overrides codec viability.
    let votes = station.votes.unwrap_or(0).min(1000) / 10; // 0..=100
    let clicks = station.click_count.unwrap_or(0).min(2000) / 20; // 0..=100

    codec + bitrate + votes + clicks
}

/// The built-in catalog: curated stations grouped by category.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

/// One curated station together with the category it was placed in.
#[derive(Debug, Clone)]
struct CatalogEntry {
    category: Category,
    station: Station,
}

impl Catalog {
    /// Build the curated catalog from the static definitions.
    pub fn curated() -> Self {
        let entries = CURATED
            .iter()
            .map(|c| CatalogEntry {
                category: c.category,
                station: c.build(),
            })
            .collect();
        Self { entries }
    }

    /// Every curated station, unranked, in definition order.
    pub fn stations(&self) -> Stations {
        self.entries.iter().map(|e| e.station.clone()).collect()
    }

    /// Curated stations in a section, ranked by playback likelihood.
    pub fn section_stations(&self, section: Section) -> Stations {
        self.entries
            .iter()
            .filter(|e| e.category.section() == section)
            .map(|e| e.station.clone())
            .collect::<Stations>()
            .ranked()
    }

    /// Curated stations in a single category, ranked by playback likelihood.
    pub fn category_stations(&self, category: Category) -> Stations {
        self.entries
            .iter()
            .filter(|e| e.category == category)
            .map(|e| e.station.clone())
            .collect::<Stations>()
            .ranked()
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self::curated()
    }
}

/// Temporary, session-only record of stations that failed to play.
///
/// This is deliberately *not* persisted: per the spec, failed stations are
/// disabled for the current session only and are never written to a durable
/// blacklist. A fresh process starts with an empty set.
#[derive(Debug, Clone, Default)]
pub struct SessionStationHealth {
    failed: HashSet<StationId>,
}

impl SessionStationHealth {
    /// A clean session with no failures recorded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a station as failed for the rest of this session.
    ///
    /// Returns `true` if this newly marked the station (it was not already
    /// failed).
    pub fn mark_failed(&mut self, id: &StationId) -> bool {
        self.failed.insert(id.clone())
    }

    /// Clear a single station's failed state (e.g. after a successful retry).
    ///
    /// Returns `true` if the station had been marked failed.
    pub fn recover(&mut self, id: &StationId) -> bool {
        self.failed.remove(id)
    }

    /// Whether a station is currently marked failed for this session.
    pub fn is_failed(&self, id: &StationId) -> bool {
        self.failed.contains(id)
    }

    /// How many stations are currently marked failed.
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// Clear all session failures.
    pub fn reset(&mut self) {
        self.failed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CodecKind;

    fn station(id: &str, codec: CodecKind, bitrate: Option<u32>, votes: Option<u32>) -> Station {
        Station {
            id: StationId::new(id).unwrap(),
            name: StationName::new("Test Station").unwrap(),
            url: StreamUrl::parse("https://example.com/stream.mp3").unwrap(),
            homepage: None,
            country: None,
            language: None,
            tags: vec![],
            codec,
            bitrate: bitrate.map(|b| BitrateKbps::new(b).unwrap()),
            votes,
            click_count: None,
            source: StationSource::BuiltIn,
        }
    }

    #[test]
    fn every_curated_entry_builds_into_a_valid_station() {
        // Panics here would mean malformed curated data; this exercises build().
        let catalog = Catalog::curated();
        assert_eq!(catalog.stations().len(), CURATED.len());
        assert!(!catalog.stations().is_empty());
    }

    #[test]
    fn catalog_separates_music_and_spoken_news() {
        let catalog = Catalog::curated();
        let music = catalog.section_stations(Section::Music);
        let spoken = catalog.section_stations(Section::SpokenNews);

        assert!(!music.is_empty());
        assert!(!spoken.is_empty());

        // Sections are disjoint: a music id never appears in spoken/news.
        let music_ids: HashSet<_> = music.iter().map(|s| s.id.as_str().to_string()).collect();
        for s in spoken.iter() {
            assert!(!music_ids.contains(s.id.as_str()));
        }
        // Every station is accounted for by exactly one section.
        assert_eq!(music.len() + spoken.len(), catalog.stations().len());
    }

    #[test]
    fn music_has_the_five_required_categories() {
        let catalog = Catalog::curated();
        for category in [
            Category::Lofi,
            Category::Ambient,
            Category::Jazz,
            Category::Classical,
            Category::Electronic,
        ] {
            assert_eq!(category.section(), Section::Music);
            assert!(
                !catalog.category_stations(category).is_empty(),
                "expected at least one curated station in {:?}",
                category
            );
        }
    }

    #[test]
    fn spoken_news_prioritizes_japanese_and_english_candidates() {
        let catalog = Catalog::curated();
        let spoken = catalog.section_stations(Section::SpokenNews);
        let languages: HashSet<_> = spoken.iter().filter_map(|s| s.language.clone()).collect();
        assert!(languages.contains("Japanese"));
        assert!(languages.contains("English"));
    }

    #[test]
    fn curated_urls_are_direct_unless_entry_opts_into_stream() {
        let catalog = Catalog::curated();
        let stations = catalog.stations();

        // The single opt-in entry has `/stream` appended to its base.
        const OPT_IN_ID: &str = "music.electronic.54house";
        let opt_in = stations
            .iter()
            .find(|s| s.id.as_str() == OPT_IN_ID)
            .expect("the /stream opt-in curated entry exists");
        assert_eq!(opt_in.url.as_str(), "https://54house.fm:9013/stream");

        // No other curated entry gains a `/stream` suffix.
        for s in stations.iter() {
            if s.id.as_str() != OPT_IN_ID {
                assert!(
                    !s.url.as_str().ends_with("/stream"),
                    "{} unexpectedly ends with /stream",
                    s.id.as_str()
                );
            }
        }
    }

    #[test]
    fn resolve_url_appends_stream_only_on_opt_in() {
        assert_eq!(
            resolve_curated_url("https://host.example/mount", false)
                .unwrap()
                .as_str(),
            "https://host.example/mount"
        );
        assert_eq!(
            resolve_curated_url("https://host.example/mount", true)
                .unwrap()
                .as_str(),
            "https://host.example/mount/stream"
        );
        // A trailing slash on the base does not produce a doubled separator.
        assert_eq!(
            resolve_curated_url("https://host.example/mount/", true)
                .unwrap()
                .as_str(),
            "https://host.example/mount/stream"
        );
    }

    #[test]
    fn score_prefers_known_codecs_over_unknown() {
        let mp3 = station("a", CodecKind::Mp3, Some(128), Some(10));
        let aac = station("b", CodecKind::Aac, Some(128), Some(10));
        let unknown = station("c", CodecKind::Unknown, Some(128), Some(10));
        assert!(station_score(&mp3) > station_score(&unknown));
        assert!(station_score(&aac) > station_score(&unknown));
        assert_eq!(station_score(&mp3), station_score(&aac));
    }

    #[test]
    fn score_rewards_reasonable_bitrate_and_popularity() {
        let good_rate = station("a", CodecKind::Mp3, Some(128), None);
        let weak_rate = station("b", CodecKind::Mp3, Some(16), None);
        assert!(station_score(&good_rate) > station_score(&weak_rate));

        let popular = station("a", CodecKind::Mp3, Some(128), Some(900));
        let unpopular = station("b", CodecKind::Mp3, Some(128), Some(0));
        assert!(station_score(&popular) > station_score(&unpopular));
    }

    #[test]
    fn ranked_orders_by_descending_score() {
        let stations: Stations = [
            station("unknown", CodecKind::Unknown, None, None),
            station("popular-mp3", CodecKind::Mp3, Some(128), Some(900)),
            station("quiet-mp3", CodecKind::Mp3, Some(128), Some(1)),
        ]
        .into_iter()
        .collect();

        let ranked = ranked_ids(&stations.ranked());
        assert_eq!(ranked, vec!["popular-mp3", "quiet-mp3", "unknown"]);
    }

    fn ranked_ids(stations: &Stations) -> Vec<String> {
        stations.iter().map(|s| s.id.as_str().to_string()).collect()
    }

    #[test]
    fn session_health_marks_and_recovers_without_persisting() {
        let id = StationId::new("music.lofi.chillhop").unwrap();
        let other = StationId::new("music.jazz.jazz24").unwrap();

        let mut health = SessionStationHealth::new();
        assert!(!health.is_failed(&id));

        assert!(health.mark_failed(&id));
        // Marking again does not double-count.
        assert!(!health.mark_failed(&id));
        assert!(health.is_failed(&id));
        assert!(!health.is_failed(&other));
        assert_eq!(health.failed_count(), 1);

        assert!(health.recover(&id));
        assert!(!health.is_failed(&id));
        assert_eq!(health.failed_count(), 0);

        // A brand-new session starts clean: state is session-only, not durable.
        let fresh = SessionStationHealth::new();
        assert_eq!(fresh.failed_count(), 0);
    }

    #[test]
    fn without_failed_filters_session_failures() {
        let stations: Stations = [
            station("keep", CodecKind::Mp3, Some(128), Some(5)),
            station("drop", CodecKind::Mp3, Some(128), Some(5)),
        ]
        .into_iter()
        .collect();

        let mut health = SessionStationHealth::new();
        health.mark_failed(&StationId::new("drop").unwrap());

        let available = stations.without_failed(&health);
        assert_eq!(available.len(), 1);
        assert_eq!(available.iter().next().unwrap().id.as_str(), "keep");

        // Recovering restores the station to the available set.
        health.recover(&StationId::new("drop").unwrap());
        assert_eq!(stations.without_failed(&health).len(), 2);
    }
}
