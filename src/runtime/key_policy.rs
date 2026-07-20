//! Mode-local key *policy*: what a mapped key means, with no side effects.
//!
//! Key interpretation stays in [`crate::cli::map_key`], which turns a terminal
//! event into a focus-agnostic [`KeyOutcome`]. This module answers the next
//! question — given the mode the UI is in, what should that outcome *do* — and
//! answers it as data: a [`Route`] carrying either a reducer [`Action`] or an
//! [`Effect`] describing adapter work. Nothing here touches audio, search,
//! persistence, or Herdr, so every mode handler is a pure function testable
//! without a terminal, an audio device, or a monitor.
//!
//! [`super::input`] owns the other half: it picks the mode, runs exactly one
//! policy, and applies the resulting route through a single adapter boundary.
//! Each mode returns [`Route::Consumed`] for keys it swallows and only the
//! Agent Planets stage returns [`Route::FallThrough`], so a key is routed at
//! most once and can never issue duplicate commands or persistence writes.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{Action, FocusPane};
use crate::cli::KeyOutcome;

/// Adapter work a key requires, named without performing it.
///
/// These are the only outcomes that reach audio, the search debounce, the
/// persistence marker, or the Herdr monitor. Keeping them as data is what
/// makes the "one key, one command" boundary checkable by inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Effect {
    /// Play the station-list selection as a freshly allocated request.
    PlaySelected,
    /// Stop/resume the current station as a freshly allocated request.
    TogglePlayback,
    /// Raise volume, mark it user-changed, and push it to the audio runtime.
    VolumeUp,
    /// Lower volume, mark it user-changed, and push it to the audio runtime.
    VolumeDown,
    /// Append a character to the live query and (re)schedule the debounce.
    SearchChar(char),
    /// Drop the last query character and (re)schedule the debounce.
    SearchBackspace,
    /// Clear the query, cancel the debounce, and restore the previous source.
    ClearSearch,
    /// Ask the Herdr monitor to focus the selected pane.
    FocusAgentPane,
    /// Submit the inline rename input through the Herdr monitor.
    SubmitRename,
}

/// What one key does in one mode.
///
/// `Act` and `Effect` are mutually exclusive on purpose: a route either folds
/// state through the reducer or asks for adapter work that the effect boundary
/// performs (including its own reducer dispatch), never both.
#[derive(Debug)]
pub(super) enum Route {
    /// Leave the event loop.
    Quit,
    /// Swallowed by this mode: nothing happens, and no other mode sees it.
    Consumed,
    /// Not this mode's key: route it once through the normal-mode policy.
    /// Only the Agent Planets stage produces this.
    FallThrough,
    /// Apply exactly one reducer action; no adapter is involved.
    Act(Action),
    /// Hand the key to the effect boundary.
    Effect(Effect),
}

/// Route a key in normal mode (no Signal View, no Agent Planets stage).
///
/// `focus` is the only app state this policy reads. List-navigation keys act
/// solely on the focused navigable list — the Browse rail moves its source
/// cursor, the station list moves its selection, and the non-list panes
/// (search strip, Now Playing) swallow them so navigation never leaks into the
/// hidden station cursor. `map_key` stays focus-agnostic; the focus-aware split
/// lives here.
pub(super) fn normal(outcome: &KeyOutcome, focus: FocusPane) -> Route {
    match outcome {
        KeyOutcome::Quit | KeyOutcome::ExitOrBack => Route::Quit,
        KeyOutcome::ToggleSignalView => Route::Act(Action::ToggleSignalView),
        // The reducer keeps this a no-op for standalone/ineligible launches,
        // so `a` stays harmless when no monitor was ever created.
        KeyOutcome::ToggleAgentPulse => Route::Act(Action::ToggleAgentOverlay),
        // `o`/`O` are stage-local. Outside Agent Planets they are inert.
        KeyOutcome::FocusAgentPane | KeyOutcome::Ignore => Route::Consumed,
        KeyOutcome::FocusNext => Route::Act(Action::FocusNext),
        KeyOutcome::FocusPrevious => Route::Act(Action::FocusPrevious),
        KeyOutcome::SelectNext => match nav_target(focus) {
            NavTarget::Stations => Route::Act(Action::SelectNext),
            NavTarget::Browse => Route::Act(Action::BrowseSelectNext),
            NavTarget::None => Route::Consumed,
        },
        KeyOutcome::SelectPrevious => match nav_target(focus) {
            NavTarget::Stations => Route::Act(Action::SelectPrevious),
            NavTarget::Browse => Route::Act(Action::BrowseSelectPrevious),
            NavTarget::None => Route::Consumed,
        },
        KeyOutcome::SelectFirst => match nav_target(focus) {
            NavTarget::Stations => Route::Act(Action::SelectFirst),
            NavTarget::Browse => Route::Act(Action::BrowseSelectFirst),
            NavTarget::None => Route::Consumed,
        },
        KeyOutcome::SelectLast => match nav_target(focus) {
            NavTarget::Stations => Route::Act(Action::SelectLast),
            NavTarget::Browse => Route::Act(Action::BrowseSelectLast),
            NavTarget::None => Route::Consumed,
        },
        // Browse focused: Enter applies the selected source and hands focus to
        // Stations rather than starting playback.
        KeyOutcome::Play => match focus {
            FocusPane::Sections => Route::Act(Action::ApplyBrowseSelection),
            _ => Route::Effect(Effect::PlaySelected),
        },
        // `Space` is the Stations transport toggle. In other panes it must not
        // move or toggle playback (in the search strip `map_key` already routes
        // Space to query text).
        KeyOutcome::TogglePlayback => match focus {
            FocusPane::Stations => Route::Effect(Effect::TogglePlayback),
            _ => Route::Consumed,
        },
        KeyOutcome::ToggleFavorite => Route::Act(Action::ToggleFavorite),
        KeyOutcome::CycleTheme => Route::Act(Action::CycleTheme),
        KeyOutcome::CycleVisualizerMode => Route::Act(Action::CycleVisualizerMode),
        KeyOutcome::VolumeUp => Route::Effect(Effect::VolumeUp),
        KeyOutcome::VolumeDown => Route::Effect(Effect::VolumeDown),
        KeyOutcome::BeginSearch => Route::Act(Action::SetFocus(FocusPane::Search)),
        KeyOutcome::SearchChar(c) => Route::Effect(Effect::SearchChar(*c)),
        KeyOutcome::SearchBackspace => Route::Effect(Effect::SearchBackspace),
        KeyOutcome::ClearSearch => Route::Effect(Effect::ClearSearch),
    }
}

/// Route a key while Signal View is active.
///
/// Only the spec's allowed subset acts: `z`/`Esc` leave the mode, `q` quits,
/// `Space` toggles playback, `+`/`-` adjust volume, `v`/`t` cycle visualizer and
/// theme, and `f` favorites the *current* station (not the hidden station-list
/// selection). Every other key — search, focus movement, station navigation, and
/// station selection — is consumed silently, leaving background search/list
/// state untouched. Signal View has no focus of its own, so this policy needs
/// no app state at all.
pub(super) fn signal_view(outcome: &KeyOutcome) -> Route {
    match outcome {
        KeyOutcome::Quit => Route::Quit,
        KeyOutcome::ExitOrBack | KeyOutcome::ToggleSignalView => {
            Route::Act(Action::LeaveSignalView)
        }
        // Unlike normal mode, Space is not focus-gated here: Signal View shows
        // only the current station, so the transport toggle always applies.
        KeyOutcome::TogglePlayback => Route::Effect(Effect::TogglePlayback),
        // In Signal View `f` targets the current station shown on screen, not
        // the hidden station-list selection.
        KeyOutcome::ToggleFavorite => Route::Act(Action::ToggleCurrentFavorite),
        KeyOutcome::CycleTheme => Route::Act(Action::CycleTheme),
        KeyOutcome::CycleVisualizerMode => Route::Act(Action::CycleVisualizerMode),
        KeyOutcome::VolumeUp => Route::Effect(Effect::VolumeUp),
        KeyOutcome::VolumeDown => Route::Effect(Effect::VolumeDown),
        // Disabled keys are silent no-ops: search, focus movement, station
        // navigation, station selection, and Agent Pulse do nothing while
        // Signal View is active.
        KeyOutcome::Ignore
        | KeyOutcome::ToggleAgentPulse
        | KeyOutcome::FocusAgentPane
        | KeyOutcome::FocusNext
        | KeyOutcome::FocusPrevious
        | KeyOutcome::SelectNext
        | KeyOutcome::SelectPrevious
        | KeyOutcome::SelectFirst
        | KeyOutcome::SelectLast
        | KeyOutcome::Play
        | KeyOutcome::BeginSearch
        | KeyOutcome::SearchChar(_)
        | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch => Route::Consumed,
    }
}

/// Route a key on the open Agent Planets stage, outside its modals.
///
/// Stage-local: `Tab`/arrows (and their `j`/`k` synonyms) move the planet
/// selection, `a`/`Esc` close the stage, `Enter` opens details for the selected
/// planet without playing a station, `o`/`O` focuses the live pane, and `z`
/// plus every station/search key is consumed so those surfaces stay suppressed
/// behind the stage. The documented global player controls fall through exactly
/// once to normal-mode policy, keeping their normal semantics and side effects.
pub(super) fn stage(outcome: &KeyOutcome) -> Route {
    match outcome {
        KeyOutcome::Quit => Route::Quit,
        KeyOutcome::FocusAgentPane => Route::Effect(Effect::FocusAgentPane),
        KeyOutcome::ToggleAgentPulse | KeyOutcome::ExitOrBack => {
            Route::Act(Action::CloseAgentOverlay)
        }
        KeyOutcome::FocusNext | KeyOutcome::SelectNext => Route::Act(Action::SelectNextAgent),
        KeyOutcome::FocusPrevious | KeyOutcome::SelectPrevious => {
            Route::Act(Action::SelectPreviousAgent)
        }
        KeyOutcome::Play => Route::Act(Action::OpenAgentDetails),
        // Consumed: the station list and search surfaces stay suppressed behind
        // the stage, and Signal View never opens over Agent Planets.
        KeyOutcome::BeginSearch
        | KeyOutcome::SearchChar(_)
        | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch
        | KeyOutcome::SelectFirst
        | KeyOutcome::SelectLast
        | KeyOutcome::ToggleSignalView => Route::Consumed,
        // The documented global player controls keep working over the stage.
        // Listed explicitly (no wildcard) so a new `KeyOutcome` forces a
        // deliberate consume-or-fall-through decision here.
        KeyOutcome::TogglePlayback
        | KeyOutcome::ToggleFavorite
        | KeyOutcome::CycleTheme
        | KeyOutcome::CycleVisualizerMode
        | KeyOutcome::VolumeUp
        | KeyOutcome::VolumeDown
        | KeyOutcome::Ignore => Route::FallThrough,
    }
}

/// Route a key while the planet details modal is open.
///
/// Every non-quit key is modal-local: `r`/`R` opens the inline rename input,
/// `Enter`/`Esc` close details, `a` closes the whole stage, `o`/`O` focuses the
/// pane, and `Tab`/arrows (with their `j`/`k` synonyms) cycle the planet
/// selection with the modal following. Nothing falls through, so no player
/// control fires from behind the modal.
pub(super) fn details(key: KeyEvent, outcome: &KeyOutcome) -> Route {
    // `r` is read from the raw event: `map_key` has no rename outcome, and the
    // modal claims the key before any other meaning could apply.
    if pressed(key) && matches!(key.code, KeyCode::Char('r' | 'R')) {
        return Route::Act(Action::OpenAgentRename);
    }
    match outcome {
        KeyOutcome::Quit => Route::Quit,
        KeyOutcome::ToggleAgentPulse => Route::Act(Action::CloseAgentOverlay),
        KeyOutcome::FocusAgentPane => Route::Effect(Effect::FocusAgentPane),
        KeyOutcome::ExitOrBack | KeyOutcome::Play => Route::Act(Action::CloseAgentDetails),
        KeyOutcome::FocusNext | KeyOutcome::SelectNext => Route::Act(Action::SelectNextAgent),
        KeyOutcome::FocusPrevious | KeyOutcome::SelectPrevious => {
            Route::Act(Action::SelectPreviousAgent)
        }
        KeyOutcome::ToggleSignalView
        | KeyOutcome::SelectFirst
        | KeyOutcome::SelectLast
        | KeyOutcome::TogglePlayback
        | KeyOutcome::ToggleFavorite
        | KeyOutcome::CycleTheme
        | KeyOutcome::CycleVisualizerMode
        | KeyOutcome::VolumeUp
        | KeyOutcome::VolumeDown
        | KeyOutcome::BeginSearch
        | KeyOutcome::SearchChar(_)
        | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch
        | KeyOutcome::Ignore => Route::Consumed,
    }
}

/// Route a key while the inline rename input is open.
///
/// The input claims text keys, so it is read from the raw event rather than the
/// mapped outcome: `Enter` submits, `Backspace` deletes, and any unmodified
/// character is appended. `Esc` cancels the input without closing the modal or
/// the stage, and `q` still quits — the outcome checks run first, exactly as
/// they did before this policy was extracted.
pub(super) fn rename(key: KeyEvent, outcome: &KeyOutcome) -> Route {
    match outcome {
        KeyOutcome::Quit => return Route::Quit,
        KeyOutcome::ExitOrBack => return Route::Act(Action::CloseAgentRename),
        _ => {}
    }
    if !pressed(key) {
        return Route::Consumed;
    }
    match key.code {
        KeyCode::Enter => Route::Effect(Effect::SubmitRename),
        KeyCode::Backspace => Route::Act(Action::BackspaceAgentRename),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Route::Act(Action::AppendAgentRename(character))
        }
        _ => Route::Consumed,
    }
}

/// Whether the event is a real key press (or auto-repeat) rather than a release.
fn pressed(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

/// Which list (if any) the focused pane navigates with list-navigation keys.
///
/// Only the Stations and Browse rail panes are navigable lists; the search strip
/// and Now Playing are not, so list-navigation keys must do nothing there rather
/// than leaking into the hidden station cursor.
enum NavTarget {
    Stations,
    Browse,
    None,
}

/// Resolve which list the given focus navigates.
fn nav_target(focus: FocusPane) -> NavTarget {
    match focus {
        FocusPane::Stations => NavTarget::Stations,
        FocusPane::Sections => NavTarget::Browse,
        FocusPane::Search | FocusPane::NowPlaying => NavTarget::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `KeyOutcome` a mode policy must classify.
    ///
    /// Tables below cover this list exhaustively, so a mode can never quietly
    /// leave an outcome unrouted.
    const ALL_OUTCOMES: &[KeyOutcome] = &[
        KeyOutcome::Quit,
        KeyOutcome::ExitOrBack,
        KeyOutcome::ToggleSignalView,
        KeyOutcome::ToggleAgentPulse,
        KeyOutcome::FocusAgentPane,
        KeyOutcome::FocusNext,
        KeyOutcome::FocusPrevious,
        KeyOutcome::SelectNext,
        KeyOutcome::SelectPrevious,
        KeyOutcome::SelectFirst,
        KeyOutcome::SelectLast,
        KeyOutcome::Play,
        KeyOutcome::TogglePlayback,
        KeyOutcome::ToggleFavorite,
        KeyOutcome::CycleTheme,
        KeyOutcome::CycleVisualizerMode,
        KeyOutcome::VolumeUp,
        KeyOutcome::VolumeDown,
        KeyOutcome::BeginSearch,
        KeyOutcome::SearchChar('x'),
        KeyOutcome::SearchBackspace,
        KeyOutcome::ClearSearch,
        KeyOutcome::Ignore,
    ];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // `Action` carries payloads (audio events, search results) that are not
    // `PartialEq`, so every table pins a route by its `Debug` rendering. That
    // fixes both the variant and its payload without widening derives on the
    // reducer vocabulary.

    #[test]
    fn normal_mode_routes_every_outcome_for_stations_focus() {
        let table: &[(KeyOutcome, &str)] = &[
            (KeyOutcome::Quit, "Quit"),
            (KeyOutcome::ExitOrBack, "Quit"),
            (KeyOutcome::ToggleSignalView, "Act(ToggleSignalView)"),
            (KeyOutcome::ToggleAgentPulse, "Act(ToggleAgentOverlay)"),
            // `o`/`O` are stage-local, so they are inert here.
            (KeyOutcome::FocusAgentPane, "Consumed"),
            (KeyOutcome::FocusNext, "Act(FocusNext)"),
            (KeyOutcome::FocusPrevious, "Act(FocusPrevious)"),
            (KeyOutcome::SelectNext, "Act(SelectNext)"),
            (KeyOutcome::SelectPrevious, "Act(SelectPrevious)"),
            (KeyOutcome::SelectFirst, "Act(SelectFirst)"),
            (KeyOutcome::SelectLast, "Act(SelectLast)"),
            (KeyOutcome::Play, "Effect(PlaySelected)"),
            (KeyOutcome::TogglePlayback, "Effect(TogglePlayback)"),
            (KeyOutcome::ToggleFavorite, "Act(ToggleFavorite)"),
            (KeyOutcome::CycleTheme, "Act(CycleTheme)"),
            (KeyOutcome::CycleVisualizerMode, "Act(CycleVisualizerMode)"),
            (KeyOutcome::VolumeUp, "Effect(VolumeUp)"),
            (KeyOutcome::VolumeDown, "Effect(VolumeDown)"),
            (KeyOutcome::BeginSearch, "Act(SetFocus(Search))"),
            (KeyOutcome::SearchChar('x'), "Effect(SearchChar('x'))"),
            (KeyOutcome::SearchBackspace, "Effect(SearchBackspace)"),
            (KeyOutcome::ClearSearch, "Effect(ClearSearch)"),
            (KeyOutcome::Ignore, "Consumed"),
        ];

        assert_eq!(
            table.len(),
            ALL_OUTCOMES.len(),
            "the normal-mode table must cover every KeyOutcome"
        );
        for (outcome, expected) in table {
            assert_eq!(
                format!("{:?}", normal(outcome, FocusPane::Stations)),
                *expected,
                "normal {outcome:?}"
            );
        }
        // Normal mode is the last stop: nothing may fall through past it.
        for outcome in ALL_OUTCOMES {
            assert!(
                !matches!(normal(outcome, FocusPane::Stations), Route::FallThrough),
                "normal mode must resolve {outcome:?}"
            );
        }
    }

    #[test]
    fn list_navigation_follows_focus_and_never_leaks_to_hidden_lists() {
        // (focus, next, previous, first, last) as Debug renderings, so the
        // focus-aware split is pinned for every navigable and non-navigable pane.
        let table: &[(FocusPane, &str, &str, &str, &str)] = &[
            (
                FocusPane::Stations,
                "Act(SelectNext)",
                "Act(SelectPrevious)",
                "Act(SelectFirst)",
                "Act(SelectLast)",
            ),
            (
                FocusPane::Sections,
                "Act(BrowseSelectNext)",
                "Act(BrowseSelectPrevious)",
                "Act(BrowseSelectFirst)",
                "Act(BrowseSelectLast)",
            ),
            (
                FocusPane::Search,
                "Consumed",
                "Consumed",
                "Consumed",
                "Consumed",
            ),
            (
                FocusPane::NowPlaying,
                "Consumed",
                "Consumed",
                "Consumed",
                "Consumed",
            ),
        ];

        for (focus, next, previous, first, last) in table {
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::SelectNext, *focus)),
                *next,
                "{focus:?} SelectNext"
            );
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::SelectPrevious, *focus)),
                *previous,
                "{focus:?} SelectPrevious"
            );
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::SelectFirst, *focus)),
                *first,
                "{focus:?} SelectFirst"
            );
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::SelectLast, *focus)),
                *last,
                "{focus:?} SelectLast"
            );
        }
    }

    #[test]
    fn enter_and_space_are_focus_gated_in_normal_mode() {
        // Enter plays everywhere except the Browse rail, where it applies the
        // source instead; Space is the Stations-only transport toggle.
        let table: &[(FocusPane, &str, &str)] = &[
            (
                FocusPane::Stations,
                "Effect(PlaySelected)",
                "Effect(TogglePlayback)",
            ),
            (FocusPane::Sections, "Act(ApplyBrowseSelection)", "Consumed"),
            (FocusPane::Search, "Effect(PlaySelected)", "Consumed"),
            (FocusPane::NowPlaying, "Effect(PlaySelected)", "Consumed"),
        ];

        for (focus, play, space) in table {
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::Play, *focus)),
                *play,
                "{focus:?} Enter"
            );
            assert_eq!(
                format!("{:?}", normal(&KeyOutcome::TogglePlayback, *focus)),
                *space,
                "{focus:?} Space"
            );
        }
    }

    #[test]
    fn signal_view_allows_only_the_documented_subset() {
        let table: &[(KeyOutcome, &str)] = &[
            (KeyOutcome::Quit, "Quit"),
            (KeyOutcome::ExitOrBack, "Act(LeaveSignalView)"),
            (KeyOutcome::ToggleSignalView, "Act(LeaveSignalView)"),
            (KeyOutcome::TogglePlayback, "Effect(TogglePlayback)"),
            // `f` targets the current station, not the hidden selection.
            (KeyOutcome::ToggleFavorite, "Act(ToggleCurrentFavorite)"),
            (KeyOutcome::CycleTheme, "Act(CycleTheme)"),
            (KeyOutcome::CycleVisualizerMode, "Act(CycleVisualizerMode)"),
            (KeyOutcome::VolumeUp, "Effect(VolumeUp)"),
            (KeyOutcome::VolumeDown, "Effect(VolumeDown)"),
            // Everything else is a silent no-op.
            (KeyOutcome::ToggleAgentPulse, "Consumed"),
            (KeyOutcome::FocusAgentPane, "Consumed"),
            (KeyOutcome::FocusNext, "Consumed"),
            (KeyOutcome::FocusPrevious, "Consumed"),
            (KeyOutcome::SelectNext, "Consumed"),
            (KeyOutcome::SelectPrevious, "Consumed"),
            (KeyOutcome::SelectFirst, "Consumed"),
            (KeyOutcome::SelectLast, "Consumed"),
            (KeyOutcome::Play, "Consumed"),
            (KeyOutcome::BeginSearch, "Consumed"),
            (KeyOutcome::SearchChar('x'), "Consumed"),
            (KeyOutcome::SearchBackspace, "Consumed"),
            (KeyOutcome::ClearSearch, "Consumed"),
            (KeyOutcome::Ignore, "Consumed"),
        ];

        assert_eq!(
            table.len(),
            ALL_OUTCOMES.len(),
            "the Signal View table must cover every KeyOutcome"
        );
        for (outcome, expected) in table {
            assert_eq!(
                format!("{:?}", signal_view(outcome)),
                *expected,
                "Signal View {outcome:?}"
            );
        }
        // Signal View never falls through: it is a complete gate.
        for outcome in ALL_OUTCOMES {
            assert!(
                !matches!(signal_view(outcome), Route::FallThrough),
                "Signal View must consume or act on {outcome:?}, never fall through"
            );
        }
    }

    #[test]
    fn agent_planets_stage_consumes_stage_keys_and_falls_through_player_controls() {
        let table: &[(KeyOutcome, &str)] = &[
            (KeyOutcome::Quit, "Quit"),
            (KeyOutcome::ExitOrBack, "Act(CloseAgentOverlay)"),
            (KeyOutcome::ToggleAgentPulse, "Act(CloseAgentOverlay)"),
            (KeyOutcome::FocusAgentPane, "Effect(FocusAgentPane)"),
            (KeyOutcome::FocusNext, "Act(SelectNextAgent)"),
            (KeyOutcome::SelectNext, "Act(SelectNextAgent)"),
            (KeyOutcome::FocusPrevious, "Act(SelectPreviousAgent)"),
            (KeyOutcome::SelectPrevious, "Act(SelectPreviousAgent)"),
            (KeyOutcome::Play, "Act(OpenAgentDetails)"),
            // Suppressed surfaces behind the stage.
            (KeyOutcome::BeginSearch, "Consumed"),
            (KeyOutcome::SearchChar('x'), "Consumed"),
            (KeyOutcome::SearchBackspace, "Consumed"),
            (KeyOutcome::ClearSearch, "Consumed"),
            (KeyOutcome::SelectFirst, "Consumed"),
            (KeyOutcome::SelectLast, "Consumed"),
            (KeyOutcome::ToggleSignalView, "Consumed"),
            // Documented global player controls keep working.
            (KeyOutcome::TogglePlayback, "FallThrough"),
            (KeyOutcome::ToggleFavorite, "FallThrough"),
            (KeyOutcome::CycleTheme, "FallThrough"),
            (KeyOutcome::CycleVisualizerMode, "FallThrough"),
            (KeyOutcome::VolumeUp, "FallThrough"),
            (KeyOutcome::VolumeDown, "FallThrough"),
            (KeyOutcome::Ignore, "FallThrough"),
        ];

        assert_eq!(
            table.len(),
            ALL_OUTCOMES.len(),
            "the stage table must cover every KeyOutcome"
        );
        for (outcome, expected) in table {
            assert_eq!(
                format!("{:?}", stage(outcome)),
                *expected,
                "stage {outcome:?}"
            );
        }
    }

    #[test]
    fn details_modal_consumes_every_key_and_never_falls_through() {
        let table: &[(KeyOutcome, &str)] = &[
            (KeyOutcome::Quit, "Quit"),
            (KeyOutcome::ToggleAgentPulse, "Act(CloseAgentOverlay)"),
            (KeyOutcome::FocusAgentPane, "Effect(FocusAgentPane)"),
            (KeyOutcome::ExitOrBack, "Act(CloseAgentDetails)"),
            (KeyOutcome::Play, "Act(CloseAgentDetails)"),
            (KeyOutcome::FocusNext, "Act(SelectNextAgent)"),
            (KeyOutcome::SelectNext, "Act(SelectNextAgent)"),
            (KeyOutcome::FocusPrevious, "Act(SelectPreviousAgent)"),
            (KeyOutcome::SelectPrevious, "Act(SelectPreviousAgent)"),
            // Modal-local: no player control fires from behind the modal.
            (KeyOutcome::ToggleSignalView, "Consumed"),
            (KeyOutcome::SelectFirst, "Consumed"),
            (KeyOutcome::SelectLast, "Consumed"),
            (KeyOutcome::TogglePlayback, "Consumed"),
            (KeyOutcome::ToggleFavorite, "Consumed"),
            (KeyOutcome::CycleTheme, "Consumed"),
            (KeyOutcome::CycleVisualizerMode, "Consumed"),
            (KeyOutcome::VolumeUp, "Consumed"),
            (KeyOutcome::VolumeDown, "Consumed"),
            (KeyOutcome::BeginSearch, "Consumed"),
            (KeyOutcome::SearchChar('x'), "Consumed"),
            (KeyOutcome::SearchBackspace, "Consumed"),
            (KeyOutcome::ClearSearch, "Consumed"),
            (KeyOutcome::Ignore, "Consumed"),
        ];

        assert_eq!(
            table.len(),
            ALL_OUTCOMES.len(),
            "the details table must cover every KeyOutcome"
        );
        // A non-`r` key carries the outcome; `Tab` is inert as a raw code.
        for (outcome, expected) in table {
            assert_eq!(
                format!("{:?}", details(key(KeyCode::Tab), outcome)),
                *expected,
                "details {outcome:?}"
            );
            assert!(
                !matches!(details(key(KeyCode::Tab), outcome), Route::FallThrough),
                "details must consume {outcome:?}"
            );
        }
    }

    #[test]
    fn details_r_opens_rename_before_any_other_meaning() {
        for code in [KeyCode::Char('r'), KeyCode::Char('R')] {
            assert_eq!(
                format!("{:?}", details(key(code), &KeyOutcome::Ignore)),
                "Act(OpenAgentRename)",
                "{code:?} opens the rename input"
            );
        }
        // A release event is not a press, so it must not open rename.
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(
            format!("{:?}", details(release, &KeyOutcome::Ignore)),
            "Consumed"
        );
    }

    #[test]
    fn rename_input_claims_text_keys_but_keeps_quit_and_cancel() {
        // (raw key, mapped outcome, expected route)
        let table: &[(KeyEvent, KeyOutcome, &str)] = &[
            // `q` still quits and `Esc` cancels: the outcome checks run first.
            (key(KeyCode::Char('q')), KeyOutcome::Quit, "Quit"),
            (
                key(KeyCode::Esc),
                KeyOutcome::ExitOrBack,
                "Act(CloseAgentRename)",
            ),
            // Text editing comes from the raw event, not the mapped outcome.
            (
                key(KeyCode::Enter),
                KeyOutcome::Play,
                "Effect(SubmitRename)",
            ),
            (
                key(KeyCode::Backspace),
                KeyOutcome::Ignore,
                "Act(BackspaceAgentRename)",
            ),
            (
                key(KeyCode::Char('a')),
                KeyOutcome::ToggleAgentPulse,
                "Act(AppendAgentRename('a'))",
            ),
            (
                key(KeyCode::Char(' ')),
                KeyOutcome::TogglePlayback,
                "Act(AppendAgentRename(' '))",
            ),
            (
                key(KeyCode::Char('/')),
                KeyOutcome::BeginSearch,
                "Act(AppendAgentRename('/'))",
            ),
            (
                key(KeyCode::Char('f')),
                KeyOutcome::ToggleFavorite,
                "Act(AppendAgentRename('f'))",
            ),
            // Non-text keys are consumed rather than leaking to the stage.
            (key(KeyCode::Tab), KeyOutcome::FocusNext, "Consumed"),
            (key(KeyCode::Up), KeyOutcome::SelectPrevious, "Consumed"),
        ];

        for (event, outcome, expected) in table {
            assert_eq!(
                format!("{:?}", rename(*event, outcome)),
                *expected,
                "rename {:?}",
                event.code
            );
            assert!(
                !matches!(rename(*event, outcome), Route::FallThrough),
                "rename must consume {:?}",
                event.code
            );
        }
    }

    #[test]
    fn rename_ignores_modified_characters_and_release_events() {
        // Ctrl/Alt chords are not text, so they must not reach the input.
        let ctrl = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(
            format!("{:?}", rename(ctrl, &KeyOutcome::Ignore)),
            "Consumed"
        );
        let alt = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::ALT);
        assert_eq!(
            format!("{:?}", rename(alt, &KeyOutcome::Ignore)),
            "Consumed"
        );

        // Shift is text (capital letters must type).
        let shift = KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT);
        assert_eq!(
            format!("{:?}", rename(shift, &KeyOutcome::Ignore)),
            "Act(AppendAgentRename('U'))"
        );

        let release = KeyEvent::new_with_kind(
            KeyCode::Char('u'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(
            format!("{:?}", rename(release, &KeyOutcome::Ignore)),
            "Consumed"
        );
    }
}
