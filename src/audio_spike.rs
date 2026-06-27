pub fn stream_extension(path_or_url: &str) -> Option<&'static str> {
    let path = path_or_url.split('?').next().unwrap_or(path_or_url);
    if path.ends_with(".mp3") {
        Some("mp3")
    } else if path.ends_with(".aac") {
        Some("aac")
    } else if path.ends_with(".m4a") {
        Some("m4a")
    } else {
        None
    }
}

pub fn resolve_stream_url(audio_base_url: &str) -> String {
    let trimmed = audio_base_url.trim_end_matches('/');
    if trimmed.ends_with("/stream")
        || trimmed.contains("/stream/")
        || stream_extension(trimmed).is_some()
    {
        trimmed.to_string()
    } else {
        format!("{trimmed}/stream")
    }
}

pub fn parse_icy_title(metadata: &str) -> Option<String> {
    let marker = "StreamTitle='";
    let start = metadata.find(marker)? + marker.len();
    let rest = &metadata[start..];
    let end = rest.find("';")?;
    let title = rest[..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn soft_compress(x: f32) -> f32 {
    let k = 2.0;
    (k * x) / (1.0 + k * x)
}

pub fn normalize_value(x: f32, gain: f32) -> f32 {
    let amplified = x * gain;
    if amplified >= 100.0 {
        1.0
    } else {
        soft_compress(amplified).clamp(0.0, 1.0)
    }
}
