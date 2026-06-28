//! Guards the active primary-binary entry path (MIK-013).
//!
//! The pre-replacement prototype drove the executable through `src/main.rs` plus
//! `src/api.rs`: external `ffplay`/`mpv` playback, hard-coded Ratatui `Color::`
//! values, a raw Radio Browser station struct, and a fake visualizer. That path
//! must be retired before MIK-010 wires the replacement modules into the real
//! event loop. These tests assert the prototype is gone from the active path;
//! they do not require the integrated player to exist yet.

use std::path::PathBuf;

fn crate_file(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[test]
fn main_rs_has_no_external_player_or_prototype_markers() {
    let main_rs =
        std::fs::read_to_string(crate_file("src/main.rs")).expect("src/main.rs should exist");

    for marker in ["ffplay", "mpv", "Command::new", "Color::"] {
        assert!(
            !main_rs.contains(marker),
            "src/main.rs must not contain prototype marker `{marker}`; \
             external players and hard-coded colors belong to the retired path"
        );
    }
}

#[test]
fn api_rs_is_removed_from_active_path() {
    assert!(
        !crate_file("src/api.rs").exists(),
        "src/api.rs (old top-vote Radio Browser path) must be removed; \
         search lives in src/search.rs in the replacement architecture"
    );
}

#[test]
fn main_rs_does_not_declare_the_old_api_module() {
    let main_rs =
        std::fs::read_to_string(crate_file("src/main.rs")).expect("src/main.rs should exist");

    assert!(
        !main_rs.contains("mod api"),
        "src/main.rs must not declare the old `api` module"
    );
}
