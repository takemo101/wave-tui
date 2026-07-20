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
use url::Url;

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
    /// Visualizer mode name that does not match a known built-in mode.
    UnknownVisualizerMode(String),
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
            DomainError::UnknownVisualizerMode(raw) => {
                write!(f, "unknown visualizer mode: {raw:?}")
            }
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
        let raw_authority = trimmed
            .split_once("://")
            .map(|(_, remainder)| remainder.split(['/', '?', '#']).next().unwrap_or_default());
        let has_explicit_host = raw_authority.is_some_and(|authority| {
            !authority.is_empty()
                && !authority.chars().any(|character| {
                    character.is_ascii_whitespace() || character.is_ascii_control()
                })
        });
        if !has_explicit_host {
            return Err(DomainError::InvalidStreamUrl(raw));
        }

        let parsed = Url::parse(trimmed).map_err(|_| DomainError::InvalidStreamUrl(raw.clone()))?;
        if matches!(parsed.scheme(), "http" | "https")
            && parsed
                .host_str()
                .is_some_and(|host| !host.trim().is_empty())
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

/// One of the selectable visualizer renderers.
///
/// `SpectrumStack` is the default. All six modes of the Calm Suite
/// (`docs/ui-design-decisions.md`) are implemented and selectable via the `v`
/// key: `SpectrumStack`, `PeakDots`, and `SkylinePeaks` are FFT-band driven,
/// `WaveScope` and `MirrorWave` draw the time-domain waveform, and `AmbientPulse`
/// is an RMS/band-driven ambient glow. Modes are stored as stable lowercase
/// strings (`spectrum_stack`, `peak_dots`, …) so persisted settings stay stable;
/// unknown names fall back to `SpectrumStack` at the settings boundary via
/// [`VisualizerMode::parse_or_default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VisualizerMode {
    #[default]
    SpectrumStack,
    PeakDots,
    SkylinePeaks,
    WaveScope,
    MirrorWave,
    AmbientPulse,
}

impl VisualizerMode {
    /// Every mode, in `v`-key cycling order.
    pub const ALL: [VisualizerMode; 6] = [
        VisualizerMode::SpectrumStack,
        VisualizerMode::PeakDots,
        VisualizerMode::SkylinePeaks,
        VisualizerMode::WaveScope,
        VisualizerMode::MirrorWave,
        VisualizerMode::AmbientPulse,
    ];

    /// Stable lowercase identifier used for persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            VisualizerMode::SpectrumStack => "spectrum_stack",
            VisualizerMode::PeakDots => "peak_dots",
            VisualizerMode::SkylinePeaks => "skyline_peaks",
            VisualizerMode::WaveScope => "wave_scope",
            VisualizerMode::MirrorWave => "mirror_wave",
            VisualizerMode::AmbientPulse => "ambient_pulse",
        }
    }

    /// Strict boundary parser; rejects unknown names with a [`DomainError`].
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "spectrum_stack" => Ok(VisualizerMode::SpectrumStack),
            "peak_dots" => Ok(VisualizerMode::PeakDots),
            "skyline_peaks" => Ok(VisualizerMode::SkylinePeaks),
            "wave_scope" => Ok(VisualizerMode::WaveScope),
            "mirror_wave" => Ok(VisualizerMode::MirrorWave),
            "ambient_pulse" => Ok(VisualizerMode::AmbientPulse),
            other => Err(DomainError::UnknownVisualizerMode(other.to_string())),
        }
    }

    /// Lenient boundary parser; unknown names fall back to the default mode.
    pub fn parse_or_default(raw: &str) -> Self {
        Self::parse(raw).unwrap_or(VisualizerMode::SpectrumStack)
    }

    /// Next mode in the cycling order, wrapping back to `SpectrumStack`.
    ///
    /// Bound to the `v` key; the order is stable so repeated presses are
    /// predictable.
    pub fn next(self) -> Self {
        match self {
            VisualizerMode::SpectrumStack => VisualizerMode::PeakDots,
            VisualizerMode::PeakDots => VisualizerMode::SkylinePeaks,
            VisualizerMode::SkylinePeaks => VisualizerMode::WaveScope,
            VisualizerMode::WaveScope => VisualizerMode::MirrorWave,
            VisualizerMode::MirrorWave => VisualizerMode::AmbientPulse,
            VisualizerMode::AmbientPulse => VisualizerMode::SpectrumStack,
        }
    }
}

/// Visualizer modes persist as their stable lowercase string identifier.
impl Serialize for VisualizerMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Strict deserialization: unknown names are a parse error so corrupt persisted
/// settings fail at the boundary rather than silently coercing. Lenient fallback
/// to the default mode belongs to [`VisualizerMode::parse_or_default`].
impl<'de> Deserialize<'de> for VisualizerMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        VisualizerMode::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A normalized phase-portrait coordinate series: paired played-audio samples
/// plotted on X/Y axes, not an amplitude-over-time waveform.
///
/// Both series are clamped to `-1.0..=1.0` and truncated to a shared length on
/// construction, so renderers always receive matched, in-range pairs. How the
/// pairs are derived (stereo channels or lagged mono samples) is an analyzer
/// concern; this type only guarantees renderer-safe coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseTrace {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

impl PhaseTrace {
    pub fn new(x: impl IntoIterator<Item = f32>, y: impl IntoIterator<Item = f32>) -> Self {
        let x: Vec<f32> = x.into_iter().map(|value| value.clamp(-1.0, 1.0)).collect();
        let y: Vec<f32> = y.into_iter().map(|value| value.clamp(-1.0, 1.0)).collect();
        let len = x.len().min(y.len());
        Self {
            x: x[..len].to_vec(),
            y: y[..len].to_vec(),
        }
    }

    /// A trace with no points; renders as nothing rather than a substitute.
    pub fn empty() -> Self {
        Self {
            x: Vec::new(),
            y: Vec::new(),
        }
    }
}

/// RMS at or below this threshold counts as visual silence: renderers keep
/// the display calm and still, and visual captures skip such frames.
pub const SILENCE_RMS: f32 = 0.05;

/// A single visualizer frame: normalized spectrum bands, an RMS level, a
/// low-resolution time-domain waveform, and two phase-portrait traces.
///
/// Bands and RMS are magnitudes clamped to `0.0..=1.0` on construction. The
/// `waveform` is a signed time-domain series clamped to `-1.0..=1.0`, and the
/// phase traces are [`PhaseTrace`] values normalized on their own construction.
/// Renderers receive only these drawing-oriented values, never raw audio
/// buffers.
#[derive(Debug, Clone, PartialEq)]
pub struct VizFrame {
    pub bands: Vec<f32>,
    pub rms: f32,
    pub waveform: Vec<f32>,
    pub primary_phase: PhaseTrace,
    pub secondary_phase: PhaseTrace,
}

impl VizFrame {
    /// Compatibility constructor for callers without phase data; both phase
    /// traces start empty. The analyzer uses [`VizFrame::with_phase`] instead.
    pub fn new(
        bands: impl IntoIterator<Item = f32>,
        rms: f32,
        waveform: impl IntoIterator<Item = f32>,
    ) -> Self {
        Self::with_phase(
            bands,
            rms,
            waveform,
            PhaseTrace::empty(),
            PhaseTrace::empty(),
        )
    }

    /// Full constructor carrying paired phase traces derived from played audio.
    pub fn with_phase(
        bands: impl IntoIterator<Item = f32>,
        rms: f32,
        waveform: impl IntoIterator<Item = f32>,
        primary_phase: PhaseTrace,
        secondary_phase: PhaseTrace,
    ) -> Self {
        Self {
            bands: bands.into_iter().map(|b| b.clamp(0.0, 1.0)).collect(),
            rms: rms.clamp(0.0, 1.0),
            waveform: waveform.into_iter().map(|w| w.clamp(-1.0, 1.0)).collect(),
            primary_phase,
            secondary_phase,
        }
    }

    /// A silent frame with `band_count` zeroed bands, no waveform points, and
    /// empty phase traces.
    ///
    /// An empty waveform is valid and renders as stable silence (a flat
    /// baseline); waveform resolution is an analyzer concern, so the silent
    /// default carries no points.
    pub fn silent(band_count: usize) -> Self {
        Self {
            bands: vec![0.0; band_count],
            rms: 0.0,
            waveform: Vec::new(),
            primary_phase: PhaseTrace::empty(),
            secondary_phase: PhaseTrace::empty(),
        }
    }

    /// Whether this frame is visually audible: RMS above [`SILENCE_RMS`]
    /// with at least one non-empty phase trace, so a display capture of it
    /// can actually draw a phase scope. Analyzer silence carries all-zero
    /// traces and zero RMS, so it is never audible.
    pub fn is_audible(&self) -> bool {
        self.rms > SILENCE_RMS
            && (!self.primary_phase.x.is_empty() || !self.secondary_phase.x.is_empty())
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
    fn stream_url_rejects_hostless_and_non_http_values_with_raw_domain_errors() {
        for raw in [
            "https:///path",
            "https://?q=x",
            "https://   ",
            "ftp://example.com/stream.mp3",
        ] {
            assert_eq!(
                StreamUrl::parse(raw),
                Err(DomainError::InvalidStreamUrl(raw.to_string()))
            );
        }
    }

    #[test]
    fn stream_url_rejects_embedded_ascii_whitespace_in_authority() {
        for raw in ["https://example.com\t/path", "https://example.com\n/path"] {
            assert_eq!(
                StreamUrl::parse(raw),
                Err(DomainError::InvalidStreamUrl(raw.to_string()))
            );
        }
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
    fn visualizer_mode_default_is_spectrum_stack() {
        assert_eq!(VisualizerMode::default(), VisualizerMode::SpectrumStack);
    }

    #[test]
    fn visualizer_mode_roundtrips_through_str() {
        for mode in VisualizerMode::ALL {
            assert_eq!(VisualizerMode::parse(mode.as_str()).unwrap(), mode);
        }
    }

    #[test]
    fn visualizer_mode_uses_stable_lowercase_names() {
        assert_eq!(VisualizerMode::SpectrumStack.as_str(), "spectrum_stack");
        assert_eq!(VisualizerMode::PeakDots.as_str(), "peak_dots");
        assert_eq!(VisualizerMode::SkylinePeaks.as_str(), "skyline_peaks");
        assert_eq!(VisualizerMode::WaveScope.as_str(), "wave_scope");
        assert_eq!(VisualizerMode::MirrorWave.as_str(), "mirror_wave");
        assert_eq!(VisualizerMode::AmbientPulse.as_str(), "ambient_pulse");
    }

    #[test]
    fn visualizer_mode_parses_case_insensitively() {
        assert_eq!(
            VisualizerMode::parse(" Spectrum_Stack ").unwrap(),
            VisualizerMode::SpectrumStack
        );
        assert_eq!(
            VisualizerMode::parse("WAVE_SCOPE").unwrap(),
            VisualizerMode::WaveScope
        );
    }

    #[test]
    fn unknown_visualizer_mode_is_rejected_but_falls_back_leniently() {
        assert!(matches!(
            VisualizerMode::parse("hologram"),
            Err(DomainError::UnknownVisualizerMode(_))
        ));
        assert_eq!(
            VisualizerMode::parse_or_default("hologram"),
            VisualizerMode::SpectrumStack
        );
    }

    #[test]
    fn visualizer_mode_cycles_through_the_six_modes_and_wraps() {
        // SkylinePeaks groups with the other FFT-band modes (after PeakDots).
        assert_eq!(
            VisualizerMode::SpectrumStack.next(),
            VisualizerMode::PeakDots
        );
        assert_eq!(
            VisualizerMode::PeakDots.next(),
            VisualizerMode::SkylinePeaks
        );
        assert_eq!(
            VisualizerMode::SkylinePeaks.next(),
            VisualizerMode::WaveScope
        );
        assert_eq!(VisualizerMode::WaveScope.next(), VisualizerMode::MirrorWave);
        assert_eq!(
            VisualizerMode::MirrorWave.next(),
            VisualizerMode::AmbientPulse
        );
        assert_eq!(
            VisualizerMode::AmbientPulse.next(),
            VisualizerMode::SpectrumStack
        );
        // Six steps return to the start.
        let start = VisualizerMode::SpectrumStack;
        assert_eq!(start.next().next().next().next().next().next(), start);
    }

    #[test]
    fn visualizer_mode_serializes_as_lowercase_string_and_roundtrips() {
        let json = serde_json::to_string(&VisualizerMode::PeakDots).unwrap();
        assert_eq!(json, "\"peak_dots\"");
        let decoded: VisualizerMode = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, VisualizerMode::PeakDots);
    }

    #[test]
    fn visualizer_mode_deserialization_rejects_unknown_strictly() {
        assert!(serde_json::from_str::<VisualizerMode>("\"hologram\"").is_err());
    }

    #[test]
    fn viz_frame_clamps_bands_and_rms() {
        let frame = VizFrame::new([-1.0, 0.5, 2.0], 3.0, []);
        assert_eq!(frame.bands, vec![0.0, 0.5, 1.0]);
        assert_eq!(frame.rms, 1.0);
        assert_eq!(VizFrame::silent(4).bands, vec![0.0; 4]);
    }

    #[test]
    fn viz_frame_clamps_waveform_to_bipolar_range() {
        // Waveform is a time-domain series, so it is signed and clamps to
        // -1.0..=1.0 (unlike bands/RMS, which are magnitudes in 0.0..=1.0).
        let frame = VizFrame::new([0.5], 0.5, [-2.0, -0.3, 0.0, 0.4, 2.0]);
        assert_eq!(frame.waveform, vec![-1.0, -0.3, 0.0, 0.4, 1.0]);
    }

    #[test]
    fn viz_frame_silent_has_valid_empty_waveform() {
        let frame = VizFrame::silent(4);
        assert_eq!(frame.bands, vec![0.0; 4]);
        assert_eq!(frame.rms, 0.0);
        assert!(frame.waveform.is_empty());
    }

    #[test]
    fn phase_trace_clamps_coordinates_and_truncates_to_paired_length() {
        let trace = PhaseTrace::new([-2.0, -0.25, 2.0], [-0.5, 0.5]);
        assert_eq!(trace.x, vec![-1.0, -0.25]);
        assert_eq!(trace.y, vec![-0.5, 0.5]);
    }

    #[test]
    fn phase_trace_empty_has_no_points() {
        let trace = PhaseTrace::empty();
        assert!(trace.x.is_empty());
        assert!(trace.y.is_empty());
    }

    #[test]
    fn legacy_viz_frame_constructor_has_empty_phase_traces() {
        let frame = VizFrame::new([0.2], 0.4, [0.1]);
        assert!(frame.primary_phase.x.is_empty());
        assert!(frame.secondary_phase.y.is_empty());
    }

    #[test]
    fn viz_frame_with_phase_carries_normalized_traces() {
        let frame = VizFrame::with_phase(
            [0.5],
            0.5,
            [0.0],
            PhaseTrace::new([2.0, 0.1], [0.2, -2.0]),
            PhaseTrace::new([0.3], [0.4]),
        );
        assert_eq!(frame.primary_phase.x, vec![1.0, 0.1]);
        assert_eq!(frame.primary_phase.y, vec![0.2, -1.0]);
        assert_eq!(frame.secondary_phase.x, vec![0.3]);
        assert_eq!(frame.secondary_phase.y, vec![0.4]);
    }

    #[test]
    fn viz_frame_is_audible_requires_rms_and_a_phase_trace() {
        assert!(!VizFrame::silent(4).is_audible(), "silence is not audible");
        let zero_traces = VizFrame::with_phase(
            [0.0],
            0.0,
            [],
            PhaseTrace::new([0.0, 0.0], [0.0, 0.0]),
            PhaseTrace::empty(),
        );
        assert!(
            !zero_traces.is_audible(),
            "zero RMS stays silent even with non-empty traces"
        );
        let loud_without_phase = VizFrame::new([0.5], 0.5, []);
        assert!(
            !loud_without_phase.is_audible(),
            "a frame without phase data cannot draw a scope"
        );
        let audible = VizFrame::with_phase(
            [0.5],
            0.5,
            [],
            PhaseTrace::new([0.1], [0.2]),
            PhaseTrace::empty(),
        );
        assert!(audible.is_audible());
    }

    #[test]
    fn viz_frame_silent_has_empty_phase_traces() {
        let frame = VizFrame::silent(4);
        assert!(frame.primary_phase.x.is_empty());
        assert!(frame.secondary_phase.x.is_empty());
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
