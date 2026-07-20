//! CLI argument parsing and key mapping.
//!
//! This module is the argument boundary: it parses untrusted CLI arguments once
//! into typed values (per "Parse, don't validate"), then hands a typed
//! [`CliInvocation`] to [`crate::runtime`], which owns the terminal, the
//! adapters, and the event loop. Nothing here knows about terminal or adapter
//! lifecycles, so parsing stays testable without a terminal, audio device, or
//! network.
//!
//! It also owns key *interpretation* — [`map_key`] turns a terminal key event
//! into a focus-agnostic [`KeyOutcome`]. What an outcome then *does* (actions,
//! audio commands, Herdr requests) belongs to the runtime's input routing.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::VolumePercent;
use crate::settings::Settings;
use crate::theme::ThemeName;

/// `--help` / usage text.
const USAGE: &str = "\
wave-tui — terminal-first internet radio for work sessions

USAGE:
    wave-tui [OPTIONS] [SEARCH]

OPTIONS:
    --theme <name>                Theme for this run: minimal | neon | crt |
                                  solarized | midnight | sakura
    --volume <0-100>              Startup volume override
    --no-auto-play                Start silently even if a previous station exists
    --audio-output-device <name>  CPAL output device name
    --low-power                   Lower UI update cadence (audio unaffected)
    --no-agent-pulse              Disable the Herdr Agent Pulse integration for
                                  this run (never persisted)
    --search <query>              Start in search mode with this query
    -h, --help                    Print help
    -V, --version                 Print version

ARGS:
    [SEARCH]                      Optional positional search query (same as --search)";

/// The `--help` / usage text, owned here alongside the flags it documents.
pub(crate) fn usage() -> &'static str {
    USAGE
}

// === CLI parsing =========================================================

/// Parsed, typed CLI arguments. Boundary strings are converted to domain values
/// (`ThemeName`, `VolumePercent`) once, here, so the rest of the app trusts them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliArgs {
    pub theme: Option<ThemeName>,
    pub volume: Option<VolumePercent>,
    pub no_auto_play: bool,
    pub audio_output_device: Option<String>,
    pub low_power: bool,
    /// Disable the Herdr Agent Pulse integration for this run only; never
    /// persisted and never applied to settings.
    pub no_agent_pulse: bool,
    pub search: Option<String>,
}

/// What a successful parse asked the program to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliInvocation {
    /// Run the player with the given arguments.
    Run(CliArgs),
    /// Print usage and exit.
    Help,
    /// Print the version and exit.
    Version,
}

/// A recoverable CLI parsing error reported at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    UnknownFlag(String),
    MissingValue(String),
    InvalidVolume(String),
    InvalidTheme(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag(flag) => write!(f, "unknown option: {flag}"),
            CliError::MissingValue(flag) => write!(f, "missing value for option: {flag}"),
            CliError::InvalidVolume(raw) => {
                write!(f, "invalid --volume {raw:?} (expected an integer 0-100)")
            }
            CliError::InvalidTheme(raw) => {
                write!(
                    f,
                    "invalid --theme {raw:?} \
                     (expected minimal, neon, crt, solarized, midnight, or sakura)"
                )
            }
        }
    }
}

impl std::error::Error for CliError {}

/// Parse CLI arguments (excluding the program name) into a [`CliInvocation`].
///
/// Supports both `--flag value` and `--flag=value` forms for value options. The
/// first non-flag argument is treated as a positional search query. Unknown
/// flags and out-of-range values are recoverable [`CliError`]s, not panics.
pub fn parse_args<I>(args: I) -> Result<CliInvocation, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = CliArgs::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        // Split `--flag=value` once; `inline` carries the value when present.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };

        // Fetch a value from the inline form or the next argument.
        let mut take_value = |flag: &str| -> Result<String, CliError> {
            if let Some(value) = inline.clone() {
                Ok(value)
            } else {
                iter.next()
                    .ok_or_else(|| CliError::MissingValue(flag.to_string()))
            }
        };

        match flag.as_str() {
            "-h" | "--help" => return Ok(CliInvocation::Help),
            "-V" | "--version" => return Ok(CliInvocation::Version),
            "--no-auto-play" => parsed.no_auto_play = true,
            "--low-power" => parsed.low_power = true,
            "--no-agent-pulse" => parsed.no_agent_pulse = true,
            "--theme" => {
                let raw = take_value("--theme")?;
                parsed.theme =
                    Some(ThemeName::parse(&raw).map_err(|_| CliError::InvalidTheme(raw))?);
            }
            "--volume" => {
                let raw = take_value("--volume")?;
                parsed.volume =
                    Some(VolumePercent::parse(&raw).map_err(|_| CliError::InvalidVolume(raw))?);
            }
            "--audio-output-device" => {
                parsed.audio_output_device = Some(take_value("--audio-output-device")?);
            }
            "--search" => parsed.search = Some(take_value("--search")?),
            other if other.starts_with('-') => {
                return Err(CliError::UnknownFlag(other.to_string()));
            }
            // First bare positional argument is a search query.
            _ => {
                if parsed.search.is_none() {
                    parsed.search = Some(arg);
                }
            }
        }
    }

    Ok(CliInvocation::Run(parsed))
}

/// The controller-level meaning of a key press, independent of whether the
/// search input is active. Kept small and comparable so [`map_key`] is fully
/// unit testable without a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Quit,
    /// `Esc` in navigation mode: quit the normal UI, or back out of Signal View
    /// when that mode is active.
    ExitOrBack,
    /// `z`: toggle the Signal View display mode.
    ToggleSignalView,
    /// `a`: toggle the Agent Pulse overlay. The reducer keeps it a no-op
    /// while the integration is hidden or Signal View is active.
    ToggleAgentPulse,
    /// `o`/`O`: focus the selected live Agent Planets pane.
    FocusAgentPane,
    FocusNext,
    FocusPrevious,
    SelectNext,
    SelectPrevious,
    SelectFirst,
    SelectLast,
    /// `Enter`: play the selected station.
    Play,
    /// `Space`: stop/play toggle.
    TogglePlayback,
    ToggleFavorite,
    CycleTheme,
    /// `v`: cycle the visualizer mode.
    CycleVisualizerMode,
    VolumeUp,
    VolumeDown,
    /// `/`: focus the search strip.
    BeginSearch,
    /// A printable character typed while the search strip is focused.
    SearchChar(char),
    /// `Backspace` while the search strip is focused.
    SearchBackspace,
    /// `Esc` while the search strip is focused: clear and leave search.
    ClearSearch,
    /// No mapped meaning for this key in this mode.
    Ignore,
}

/// Map a terminal key event to a [`KeyOutcome`].
///
/// `searching` is `true` when the search strip is focused: printable keys then
/// edit the query instead of triggering navigation commands. Navigation keys
/// (`Tab`, arrows, `Enter`, `Home`/`End`) work in both modes; `Ctrl+C` always
/// quits.
pub fn map_key(key: KeyEvent, searching: bool) -> KeyOutcome {
    // Only act on presses (and auto-repeat); ignore key releases so terminals
    // that report both do not double-fire.
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return KeyOutcome::Ignore;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => KeyOutcome::Quit,
            _ => KeyOutcome::Ignore,
        };
    }

    // Navigation keys behave the same whether or not search input is active.
    match key.code {
        KeyCode::Tab => return KeyOutcome::FocusNext,
        KeyCode::BackTab => return KeyOutcome::FocusPrevious,
        KeyCode::Up => return KeyOutcome::SelectPrevious,
        KeyCode::Down => return KeyOutcome::SelectNext,
        KeyCode::Home => return KeyOutcome::SelectFirst,
        KeyCode::End => return KeyOutcome::SelectLast,
        KeyCode::Enter => return KeyOutcome::Play,
        _ => {}
    }

    if searching {
        match key.code {
            KeyCode::Esc => KeyOutcome::ClearSearch,
            KeyCode::Backspace => KeyOutcome::SearchBackspace,
            KeyCode::Char(c) => KeyOutcome::SearchChar(c),
            _ => KeyOutcome::Ignore,
        }
    } else {
        match key.code {
            KeyCode::Char('q') => KeyOutcome::Quit,
            KeyCode::Esc => KeyOutcome::ExitOrBack,
            KeyCode::Char('z') => KeyOutcome::ToggleSignalView,
            KeyCode::Char('a') => KeyOutcome::ToggleAgentPulse,
            KeyCode::Char('o') | KeyCode::Char('O') => KeyOutcome::FocusAgentPane,
            KeyCode::Char('j') => KeyOutcome::SelectNext,
            KeyCode::Char('k') => KeyOutcome::SelectPrevious,
            KeyCode::Char('g') => KeyOutcome::SelectFirst,
            KeyCode::Char('G') => KeyOutcome::SelectLast,
            KeyCode::Char('/') => KeyOutcome::BeginSearch,
            KeyCode::Char(' ') => KeyOutcome::TogglePlayback,
            KeyCode::Char('+') | KeyCode::Char('=') => KeyOutcome::VolumeUp,
            KeyCode::Char('-') | KeyCode::Char('_') => KeyOutcome::VolumeDown,
            KeyCode::Char('f') => KeyOutcome::ToggleFavorite,
            KeyCode::Char('t') => KeyOutcome::CycleTheme,
            KeyCode::Char('v') => KeyOutcome::CycleVisualizerMode,
            _ => KeyOutcome::Ignore,
        }
    }
}

/// Apply per-run CLI overrides onto loaded settings.
///
/// `--theme` and `--volume` adjust the in-memory settings used for this run
/// (display, audio startup volume, and the base for `+`/`-` stepping). What is
/// later *persisted* is governed separately by [`Persistence`].
pub(crate) fn apply_overrides(mut settings: Settings, args: &CliArgs) -> Settings {
    if let Some(theme) = args.theme {
        settings.theme = theme;
    }
    if let Some(volume) = args.volume {
        settings.volume = volume;
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn parse(args: &[&str]) -> Result<CliInvocation, CliError> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_args_defaults_to_an_empty_run() {
        assert_eq!(parse(&[]).unwrap(), CliInvocation::Run(CliArgs::default()));
    }

    #[test]
    fn parse_args_reads_all_flags_in_space_form() {
        let invocation = parse(&[
            "--theme",
            "neon",
            "--volume",
            "42",
            "--no-auto-play",
            "--audio-output-device",
            "Speakers",
            "--low-power",
            "--no-agent-pulse",
            "--search",
            "lofi",
        ])
        .unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                theme: Some(ThemeName::Neon),
                volume: Some(VolumePercent::new(42).unwrap()),
                no_auto_play: true,
                audio_output_device: Some("Speakers".to_string()),
                low_power: true,
                no_agent_pulse: true,
                search: Some("lofi".to_string()),
            })
        );
    }

    #[test]
    fn parse_args_accepts_every_stable_theme_name() {
        let cases = [
            ("minimal", ThemeName::Minimal),
            ("neon", ThemeName::Neon),
            ("crt", ThemeName::Crt),
            ("solarized", ThemeName::Solarized),
            ("midnight", ThemeName::Midnight),
            ("sakura", ThemeName::Sakura),
        ];
        for (raw, expected) in cases {
            let invocation = parse(&["--theme", raw]).unwrap();
            assert_eq!(
                invocation,
                CliInvocation::Run(CliArgs {
                    theme: Some(expected),
                    ..CliArgs::default()
                }),
                "--theme {raw} should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn invalid_theme_error_lists_every_supported_name() {
        let message = CliError::InvalidTheme("aurora".to_string()).to_string();
        for name in ["minimal", "neon", "crt", "solarized", "midnight", "sakura"] {
            assert!(
                message.contains(name),
                "invalid-theme error {message:?} should mention {name}"
            );
        }
    }

    #[test]
    fn parse_args_accepts_equals_form_for_values() {
        let invocation = parse(&["--theme=crt", "--volume=10", "--search=jazz radio"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                theme: Some(ThemeName::Crt),
                volume: Some(VolumePercent::new(10).unwrap()),
                search: Some("jazz radio".to_string()),
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn parse_args_treats_first_positional_as_search() {
        let invocation = parse(&["ambient"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                search: Some("ambient".to_string()),
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn parse_args_handles_help_and_version() {
        assert_eq!(parse(&["--help"]).unwrap(), CliInvocation::Help);
        assert_eq!(parse(&["-h"]).unwrap(), CliInvocation::Help);
        assert_eq!(parse(&["--version"]).unwrap(), CliInvocation::Version);
        assert_eq!(parse(&["-V"]).unwrap(), CliInvocation::Version);
    }

    #[test]
    fn parse_args_rejects_unknown_flags_and_bad_values() {
        assert_eq!(
            parse(&["--nope"]).unwrap_err(),
            CliError::UnknownFlag("--nope".to_string())
        );
        assert!(matches!(
            parse(&["--volume", "999"]).unwrap_err(),
            CliError::InvalidVolume(_)
        ));
        assert!(matches!(
            parse(&["--theme", "aurora"]).unwrap_err(),
            CliError::InvalidTheme(_)
        ));
        assert!(matches!(
            parse(&["--volume"]).unwrap_err(),
            CliError::MissingValue(_)
        ));
    }

    #[test]
    fn parse_args_reads_no_agent_pulse() {
        let invocation = parse(&["--no-agent-pulse"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                no_agent_pulse: true,
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn usage_documents_no_agent_pulse() {
        assert!(
            USAGE.contains("--no-agent-pulse"),
            "help text must document --no-agent-pulse"
        );
    }

    #[test]
    fn map_key_navigation_mode_bindings_match_the_spec() {
        assert_eq!(map_key(key(KeyCode::Tab), false), KeyOutcome::FocusNext);
        assert_eq!(
            map_key(key(KeyCode::BackTab), false),
            KeyOutcome::FocusPrevious
        );
        assert_eq!(
            map_key(key(KeyCode::Char('j')), false),
            KeyOutcome::SelectNext
        );
        assert_eq!(map_key(key(KeyCode::Down), false), KeyOutcome::SelectNext);
        assert_eq!(
            map_key(key(KeyCode::Char('k')), false),
            KeyOutcome::SelectPrevious
        );
        assert_eq!(map_key(key(KeyCode::Up), false), KeyOutcome::SelectPrevious);
        assert_eq!(map_key(key(KeyCode::Enter), false), KeyOutcome::Play);
        assert_eq!(
            map_key(key(KeyCode::Char(' ')), false),
            KeyOutcome::TogglePlayback
        );
        assert_eq!(
            map_key(key(KeyCode::Char('+')), false),
            KeyOutcome::VolumeUp
        );
        assert_eq!(
            map_key(key(KeyCode::Char('=')), false),
            KeyOutcome::VolumeUp
        );
        assert_eq!(
            map_key(key(KeyCode::Char('-')), false),
            KeyOutcome::VolumeDown
        );
        assert_eq!(
            map_key(key(KeyCode::Char('f')), false),
            KeyOutcome::ToggleFavorite
        );
        assert_eq!(
            map_key(key(KeyCode::Char('t')), false),
            KeyOutcome::CycleTheme
        );
        assert_eq!(
            map_key(key(KeyCode::Char('v')), false),
            KeyOutcome::CycleVisualizerMode
        );
        assert_eq!(
            map_key(key(KeyCode::Char('/')), false),
            KeyOutcome::BeginSearch
        );
        assert_eq!(map_key(key(KeyCode::Char('q')), false), KeyOutcome::Quit);
        // `Esc` in navigation mode is "exit or back": it quits the normal UI but
        // backs out of Signal View when that mode is active.
        assert_eq!(map_key(key(KeyCode::Esc), false), KeyOutcome::ExitOrBack);
        assert_eq!(
            map_key(key(KeyCode::Char('z')), false),
            KeyOutcome::ToggleSignalView
        );
    }

    #[test]
    fn map_key_search_mode_edits_query_instead_of_commands() {
        // Printable keys become query text, including ones that are commands in
        // navigation mode.
        assert_eq!(
            map_key(key(KeyCode::Char('q')), true),
            KeyOutcome::SearchChar('q')
        );
        assert_eq!(
            map_key(key(KeyCode::Char('j')), true),
            KeyOutcome::SearchChar('j')
        );
        assert_eq!(
            map_key(key(KeyCode::Char('v')), true),
            KeyOutcome::SearchChar('v')
        );
        assert_eq!(
            map_key(key(KeyCode::Backspace), true),
            KeyOutcome::SearchBackspace
        );
        assert_eq!(map_key(key(KeyCode::Esc), true), KeyOutcome::ClearSearch);
        // Navigation still works while searching.
        assert_eq!(map_key(key(KeyCode::Tab), true), KeyOutcome::FocusNext);
        assert_eq!(map_key(key(KeyCode::Down), true), KeyOutcome::SelectNext);
        assert_eq!(map_key(key(KeyCode::Enter), true), KeyOutcome::Play);
    }

    #[test]
    fn map_key_ctrl_c_always_quits_and_release_events_are_ignored() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_c, false), KeyOutcome::Quit);
        assert_eq!(map_key(ctrl_c, true), KeyOutcome::Quit);

        let release = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)
            .modifiers
            .is_empty()
            .then(|| {
                let mut e = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
                e.kind = KeyEventKind::Release;
                e
            })
            .unwrap();
        assert_eq!(map_key(release, false), KeyOutcome::Ignore);
    }

    #[test]
    fn map_key_maps_z_to_signal_view_toggle_in_navigation_mode() {
        assert_eq!(
            map_key(key(KeyCode::Char('z')), false),
            KeyOutcome::ToggleSignalView
        );
        // While the search strip is focused, `z` is ordinary query text.
        assert_eq!(
            map_key(key(KeyCode::Char('z')), true),
            KeyOutcome::SearchChar('z')
        );
    }

    #[test]
    fn map_key_maps_a_to_agent_pulse_toggle_in_navigation_mode() {
        assert_eq!(
            map_key(key(KeyCode::Char('a')), false),
            KeyOutcome::ToggleAgentPulse
        );
        // While the search strip is focused, `a` stays ordinary query text.
        assert_eq!(
            map_key(key(KeyCode::Char('a')), true),
            KeyOutcome::SearchChar('a')
        );
    }

    #[test]
    fn no_agent_pulse_leaves_settings_untouched() {
        // The flag is a one-run disable: it must not flow into the in-memory
        // settings (and therefore never into what persistence writes back).
        let saved = Settings::default();
        let args = CliArgs {
            no_agent_pulse: true,
            ..CliArgs::default()
        };
        assert_eq!(apply_overrides(saved.clone(), &args), saved);
    }

    #[test]
    fn cli_volume_override_drives_runtime_startup_volume() {
        // Loaded/saved settings carry the persisted volume; --volume overrides it
        // for this run, so the runtime (audio SetVolume / auto-play) uses 50.
        let saved = Settings {
            volume: VolumePercent::new(60).unwrap(),
            ..Settings::default()
        };
        let args = CliArgs {
            volume: Some(VolumePercent::new(50).unwrap()),
            ..CliArgs::default()
        };
        let effective = apply_overrides(saved, &args);
        assert_eq!(
            effective.volume.get(),
            50,
            "runtime startup volume reflects the --volume override"
        );
    }
}
