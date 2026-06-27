use crate::{Station, RADIO_BROWSER_BASE};
use anyhow::Result;

pub fn fetch_top_stations(limit: usize) -> Result<Vec<Station>> {
    let url = format!("{}/json/stations/topvote/{}", RADIO_BROWSER_BASE, limit);
    let resp = reqwest::blocking::get(&url)?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let stations: Vec<Station> = resp.json()?;
    Ok(stations
        .into_iter()
        .filter(|s| !s.url.trim().is_empty())
        .collect())
}

// Simple fallback stations so the app is usable even offline or on API hiccup
pub fn demo_stations() -> Vec<Station> {
    vec![
        Station {
            name: "Radio Paradise Main Mix".to_string(),
            url: "http://stream-uk1.radioparadise.com/aac-320".to_string(),
            country: "United States".to_string(),
            tags: "eclectic,free,non-commercial".to_string(),
            codec: "AAC".to_string(),
            bitrate: 320,
        },
        Station {
            name: "0R - LO-FI".to_string(),
            url: "https://0nlineradio.radioho.st/0r-lo-fi".to_string(),
            country: "Germany".to_string(),
            tags: "lofi,chill,study,beats".to_string(),
            codec: "MP3".to_string(),
            bitrate: 192,
        },
        Station {
            name: "Classic Vinyl HD".to_string(),
            url: "https://icecast.walmradio.com:8443/classic".to_string(),
            country: "United States".to_string(),
            tags: "classic,oldies,jazz,vinyl".to_string(),
            codec: "MP3".to_string(),
            bitrate: 320,
        },
        Station {
            name: "Dance Wave!".to_string(),
            url: "https://dancewave.online/dance.mp3".to_string(),
            country: "Hungary".to_string(),
            tags: "dance,electronic,house,trance".to_string(),
            codec: "MP3".to_string(),
            bitrate: 128,
        },
        Station {
            name: "[laut.fm] lofi".to_string(),
            url: "https://stream.laut.fm/lofi".to_string(),
            country: "Germany".to_string(),
            tags: "lofi,instrumental".to_string(),
            codec: "MP3".to_string(),
            bitrate: 128,
        },
        Station {
            name: "Adroit Jazz Underground".to_string(),
            url: "https://icecast.walmradio.com:8443/jazz".to_string(),
            country: "United States".to_string(),
            tags: "jazz,bebop,mainstream".to_string(),
            codec: "MP3".to_string(),
            bitrate: 320,
        },
    ]
}
