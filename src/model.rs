//! Domain vocabulary and always-valid types for wave-tui.
//!
//! This module owns the core domain primitives and aggregates. Constrained
//! values are wrapped in newtypes with smart constructors so invalid values
//! cannot be constructed: if a value of one of these types exists, it is valid.
//!
//! Boundary parsing helpers live here (stream URLs, volume, search query,
//! bitrate, sample rate). Theme names are owned by [`crate::theme`].

use std::fmt;

use serde::{Deserialize, Serialize};

/// Recoverable domain validation errors raised by smart constructors.
///
/// These are typed (not `anyhow`) so callers at boundaries can branch on the
/// specific failure when normalizing untrusted CLI/JSON/catalog input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    EmptyStationId,
    EmptyStationName,
    InvalidStreamUrl(String),
    InvalidVolume(String),
    EmptySearchQuery,
    InvalidBitrate(u32),
    InvalidSampleRate(u32),
    /// Theme name that does not match a known built-in theme.
    UnknownTheme(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::EmptyStationId => write!(f, "station id must not be empty"),
            DomainError::EmptyStationName => write!(f, "station name must not be empty"),
            DomainError::InvalidStreamUrl(raw) => write!(f, "invalid stream url: {raw:?}"),
            DomainError::InvalidVolume(raw) => {
                write!(f, "invalid volume (expected 0-100): {raw:?}")
            }
            DomainError::EmptySearchQuery => write!(f, "search query must not be empty"),
            DomainError::InvalidBitrate(value) => write!(f, "invalid bitrate (kbps): {value}"),
            DomainError::InvalidSampleRate(value) => write!(f, "invalid sample rate (hz): {value}"),
            DomainError::UnknownTheme(raw) => write!(f, "unknown theme name: {raw:?}"),
        }
    }
}

impl std::error::Error for DomainError {}

/// Stable identifier for a station. Non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StationId(String);

impl StationId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(DomainError::EmptyStationId)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StationId {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<StationId> for String {
    fn from(value: StationId) -> Self {
        value.0
    }
}

/// Human-readable station name. Non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StationName(String);

impl StationName {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(DomainError::EmptyStationName)
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StationName {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<StationName> for String {
    fn from(value: StationName) -> Self {
        value.0
    }
}

/// A playable HTTP(S) stream URL.
///
/// Parsing only enforces a usable scheme and host. URL *resolution* rules (such
/// as when to append `/stream` for curated bases) belong to the audio module,
/// per the audio spike findings; this primitive does not append anything.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StreamUrl(String);

impl StreamUrl {
    pub fn parse(raw: impl Into<String>) -> Result<Self, DomainError> {
        let raw = raw.into();
        let trimmed = raw.trim();
        let has_host = |scheme: &str| trimmed.len() > scheme.len();
        if (trimmed.starts_with("https://") && has_host("https://"))
            || (trimmed.starts_with("http://") && has_host("http://"))
        {
            Ok(Self(trimmed.to_string()))
        } else {
            Err(DomainError::InvalidStreamUrl(raw))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StreamUrl {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<StreamUrl> for String {
    fn from(value: StreamUrl) -> Self {
        value.0
    }
}

/// Playback volume as a percentage in `0..=100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct VolumePercent(u8);

impl VolumePercent {
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 100;

    /// Construct from a numeric value, rejecting anything above `MAX`.
    pub fn new(value: u8) -> Result<Self, DomainError> {
        if value > Self::MAX {
            Err(DomainError::InvalidVolume(value.to_string()))
        } else {
            Ok(Self(value))
        }
    }

    /// Parse a CLI/string boundary value into a volume.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        let value: u8 = trimmed
            .parse()
            .map_err(|_| DomainError::InvalidVolume(raw.to_string()))?;
        Self::new(value)
    }

    /// Clamp an arbitrary integer into a valid volume (never fails).
    pub fn clamped(value: i32) -> Self {
        Self(value.clamp(Self::MIN as i32, Self::MAX as i32) as u8)
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for VolumePercent {
    type Error = DomainError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<VolumePercent> for u8 {
    fn from(value: VolumePercent) -> Self {
        value.0
    }
}

/// A normalized, non-empty search query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SearchQuery(String);

impl SearchQuery {
    pub fn parse(raw: impl Into<String>) -> Result<Self, DomainError> {
        let raw = raw.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            Err(DomainError::EmptySearchQuery)
        } else {
            Ok(Self(trimmed.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SearchQuery {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SearchQuery> for String {
    fn from(value: SearchQuery) -> Self {
        value.0
    }
}

/// Stream bitrate in kbps. Positive and within a sane upper bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BitrateKbps(u32);

impl BitrateKbps {
    const MAX: u32 = 2_000;

    pub fn new(value: u32) -> Result<Self, DomainError> {
        if value == 0 || value > Self::MAX {
            Err(DomainError::InvalidBitrate(value))
        } else {
            Ok(Self(value))
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for BitrateKbps {
    type Error = DomainError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BitrateKbps> for u32 {
    fn from(value: BitrateKbps) -> Self {
        value.0
    }
}

/// Audio sample rate in Hz, within the range supported by typical decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct SampleRateHz(u32);

impl SampleRateHz {
    const MIN: u32 = 8_000;
    const MAX: u32 = 384_000;

    pub fn new(value: u32) -> Result<Self, DomainError> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DomainError::InvalidSampleRate(value))
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for SampleRateHz {
    type Error = DomainError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SampleRateHz> for u32 {
    fn from(value: SampleRateHz) -> Self {
        value.0
    }
}

/// Decoder-relevant codec classification for a station stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodecKind {
    Mp3,
    Aac,
    Other(String),
    Unknown,
}

impl CodecKind {
    /// Classify a raw codec string from a boundary (catalog or Radio Browser).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" => CodecKind::Unknown,
            "mp3" => CodecKind::Mp3,
            "aac" | "aac+" | "aacp" => CodecKind::Aac,
            other => CodecKind::Other(other.to_string()),
        }
    }
}

/// Where a station record originated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StationSource {
    BuiltIn,
    RadioBrowser,
    Favorite,
}

/// An always-valid station record built from typed primitives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub id: StationId,
    pub name: StationName,
    pub url: StreamUrl,
    pub homepage: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub codec: CodecKind,
    pub bitrate: Option<BitrateKbps>,
    pub votes: Option<u32>,
    pub click_count: Option<u32>,
    pub source: StationSource,
}

/// Current playback lifecycle state for the selected station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Connecting,
    Playing,
    Failed(String),
}

/// A single visualizer frame: normalized spectrum bands plus an RMS level.
///
/// Bands and RMS are clamped to `0.0..=1.0` on construction so renderers never
/// receive out-of-range magnitudes.
#[derive(Debug, Clone, PartialEq)]
pub struct VizFrame {
    pub bands: Vec<f32>,
    pub rms: f32,
}

impl VizFrame {
    pub fn new(bands: impl IntoIterator<Item = f32>, rms: f32) -> Self {
        Self {
            bands: bands.into_iter().map(|b| b.clamp(0.0, 1.0)).collect(),
            rms: rms.clamp(0.0, 1.0),
        }
    }

    /// A silent frame with `band_count` zeroed bands.
    pub fn silent(band_count: usize) -> Self {
        Self {
            bands: vec![0.0; band_count],
            rms: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_id_rejects_blank() {
        assert_eq!(StationId::new("  "), Err(DomainError::EmptyStationId));
        assert_eq!(
            StationId::new("music.lofi.x").unwrap().as_str(),
            "music.lofi.x"
        );
    }

    #[test]
    fn station_name_rejects_blank() {
        assert_eq!(StationName::new(""), Err(DomainError::EmptyStationName));
        assert!(StationName::new("Lofi Beats").is_ok());
    }

    #[test]
    fn stream_url_requires_http_scheme_and_host() {
        assert!(StreamUrl::parse("https://example.com/stream.mp3").is_ok());
        assert!(StreamUrl::parse("http://a.example/x").is_ok());
        assert!(matches!(
            StreamUrl::parse("ftp://example.com/x"),
            Err(DomainError::InvalidStreamUrl(_))
        ));
        assert!(matches!(
            StreamUrl::parse("https://"),
            Err(DomainError::InvalidStreamUrl(_))
        ));
        assert!(matches!(
            StreamUrl::parse(""),
            Err(DomainError::InvalidStreamUrl(_))
        ));
    }

    #[test]
    fn stream_url_trims_surrounding_whitespace() {
        assert_eq!(
            StreamUrl::parse("  https://example.com/x  ")
                .unwrap()
                .as_str(),
            "https://example.com/x"
        );
    }

    #[test]
    fn volume_constructor_and_parser_enforce_range() {
        assert_eq!(VolumePercent::new(0).unwrap().get(), 0);
        assert_eq!(VolumePercent::new(100).unwrap().get(), 100);
        assert!(matches!(
            VolumePercent::new(101),
            Err(DomainError::InvalidVolume(_))
        ));
        assert_eq!(VolumePercent::parse(" 60 ").unwrap().get(), 60);
        assert!(matches!(
            VolumePercent::parse("nope"),
            Err(DomainError::InvalidVolume(_))
        ));
        assert!(matches!(
            VolumePercent::parse("200"),
            Err(DomainError::InvalidVolume(_))
        ));
        assert_eq!(VolumePercent::clamped(-5).get(), 0);
        assert_eq!(VolumePercent::clamped(999).get(), 100);
    }

    #[test]
    fn search_query_normalizes_and_rejects_empty() {
        assert_eq!(SearchQuery::parse("  jazz  ").unwrap().as_str(), "jazz");
        assert_eq!(
            SearchQuery::parse("   "),
            Err(DomainError::EmptySearchQuery)
        );
    }

    #[test]
    fn bitrate_rejects_zero_and_absurd_values() {
        assert_eq!(BitrateKbps::new(128).unwrap().get(), 128);
        assert!(matches!(
            BitrateKbps::new(0),
            Err(DomainError::InvalidBitrate(0))
        ));
        assert!(matches!(
            BitrateKbps::new(50_000),
            Err(DomainError::InvalidBitrate(50_000))
        ));
    }

    #[test]
    fn sample_rate_enforces_supported_window() {
        assert_eq!(SampleRateHz::new(44_100).unwrap().get(), 44_100);
        assert!(matches!(
            SampleRateHz::new(0),
            Err(DomainError::InvalidSampleRate(0))
        ));
        assert!(matches!(
            SampleRateHz::new(1_000_000),
            Err(DomainError::InvalidSampleRate(1_000_000))
        ));
    }

    #[test]
    fn codec_parse_classifies_known_codecs() {
        assert_eq!(CodecKind::parse("MP3"), CodecKind::Mp3);
        assert_eq!(CodecKind::parse("aac+"), CodecKind::Aac);
        assert_eq!(CodecKind::parse(""), CodecKind::Unknown);
        assert_eq!(
            CodecKind::parse("flac"),
            CodecKind::Other("flac".to_string())
        );
    }

    #[test]
    fn viz_frame_clamps_bands_and_rms() {
        let frame = VizFrame::new([-1.0, 0.5, 2.0], 3.0);
        assert_eq!(frame.bands, vec![0.0, 0.5, 1.0]);
        assert_eq!(frame.rms, 1.0);
        assert_eq!(VizFrame::silent(4).bands, vec![0.0; 4]);
    }

    #[test]
    fn serde_roundtrip_preserves_station() {
        let station = Station {
            id: StationId::new("demo").unwrap(),
            name: StationName::new("Demo").unwrap(),
            url: StreamUrl::parse("https://example.com/stream.mp3").unwrap(),
            homepage: None,
            country: Some("Japan".to_string()),
            language: Some("Japanese".to_string()),
            tags: vec!["news".to_string()],
            codec: CodecKind::Mp3,
            bitrate: Some(BitrateKbps::new(128).unwrap()),
            votes: Some(10),
            click_count: Some(20),
            source: StationSource::BuiltIn,
        };
        let json = serde_json::to_string(&station).unwrap();
        let decoded: Station = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, station);
    }

    #[test]
    fn serde_rejects_invalid_primitive_at_boundary() {
        // Volume above the allowed range must fail to deserialize, proving the
        // invariant holds across the serde boundary, not just direct calls.
        assert!(serde_json::from_str::<VolumePercent>("150").is_err());
        assert!(serde_json::from_str::<StreamUrl>("\"ftp://x\"").is_err());
        assert!(serde_json::from_str::<BitrateKbps>("0").is_err());
    }
}
