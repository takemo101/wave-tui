//! Typed boundary between the realtime output callback and the analyzer.
//!
//! A [`PlayedSample`] captures one actually played decoded source frame before
//! volume scaling: the pre-volume mono mix the FFT/RMS path consumes plus the
//! source left/right pair the phase-trace path consumes. It deliberately hides
//! CPAL frame layout from the analyzer so channel handling stays in `output`.

/// One played source frame mirrored for analysis, clamped to `-1.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayedSample {
    pub(crate) mono: f32,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) is_stereo: bool,
}

impl PlayedSample {
    /// Build from a decoded source frame (one sample per source channel).
    ///
    /// Returns `None` for an empty frame. Mono frames duplicate their single
    /// channel into `left`/`right` so downstream pairing never reads a
    /// synthetic zero channel.
    pub(crate) fn from_source_frame(source: &[f32]) -> Option<Self> {
        let &left = source.first()?;
        let right = source.get(1).copied().unwrap_or(left);
        let mono = source.iter().copied().sum::<f32>() / source.len() as f32;
        Some(Self {
            mono: mono.clamp(-1.0, 1.0),
            left: left.clamp(-1.0, 1.0),
            right: right.clamp(-1.0, 1.0),
            is_stereo: source.len() >= 2,
        })
    }
}
