//! Per-run settings persistence policy.
//!
//! Owns the one rule that separates a one-run `--volume` override from a
//! volume the user actually chose during the session, so a clean shutdown
//! never silently rewrites the saved volume.

use crate::app::{Action, App};
use crate::model::VolumePercent;
use crate::settings::{self, Settings};

/// Persistence policy for a single run.
///
/// `--volume` is a one-run startup override: it must not be written back to disk
/// merely because the app shut down cleanly or because some *other* setting
/// (favorites, previous station) changed. The saved volume is only updated when
/// the user actually changes volume via `+`/`-` during the session. Everything
/// else — favorites, previous station, and the `--theme` override — persists as
/// normal.
pub(super) struct Persistence {
    /// The volume that was on disk before CLI overrides were applied.
    baseline_volume: VolumePercent,
    /// Whether the user changed volume via `+`/`-` during this run.
    user_changed_volume: bool,
}

impl Persistence {
    pub(super) fn new(baseline_volume: VolumePercent) -> Self {
        Self {
            baseline_volume,
            user_changed_volume: false,
        }
    }

    /// Record that the user changed volume, so it is now theirs to keep.
    pub(super) fn mark_user_changed_volume(&mut self) {
        self.user_changed_volume = true;
    }

    /// The settings to actually write to disk: identical to `current`, except the
    /// volume falls back to the saved baseline when the user has not changed it
    /// (discarding any one-run `--volume` override).
    pub(super) fn settings_to_save(&self, current: &Settings) -> Settings {
        if self.user_changed_volume {
            current.clone()
        } else {
            Settings {
                volume: self.baseline_volume,
                ..current.clone()
            }
        }
    }

    /// Persist the app's settings under this policy.
    ///
    /// The save is nonfatal either way: the recoverable outcome is reported to
    /// the app so the UI can raise (or clear) the settings-save-failure notice.
    pub(super) fn save(&self, app: &mut App) {
        let failed = settings::save(&self.settings_to_save(app.settings())).is_err();
        app.apply(Action::SettingsSaveResult { failed });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    #[test]
    fn clean_shutdown_without_user_change_keeps_saved_volume() {
        // Baseline (on-disk) volume is 60; the run uses the override 50. With no
        // user +/- during the run, persistence must write back the saved 60, not
        // the run override 50.
        let baseline = VolumePercent::new(60).unwrap();
        let persistence = Persistence::new(baseline);
        let current = Settings {
            volume: VolumePercent::new(50).unwrap(),
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(
            to_save.volume.get(),
            60,
            "shutdown must not persist the one-run volume override"
        );
    }

    #[test]
    fn user_volume_change_during_run_is_persisted() {
        // Once the user presses +/-, the changed volume is theirs to keep.
        let mut persistence = Persistence::new(VolumePercent::new(60).unwrap());
        persistence.mark_user_changed_volume();
        let current = Settings {
            volume: VolumePercent::new(80).unwrap(),
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(
            to_save.volume.get(),
            80,
            "a user volume change persists the new value"
        );
    }

    #[test]
    fn persistence_preserves_other_fields_including_theme_override() {
        // Non-volume state (favorites, previous station, theme override) always
        // persists, even when the volume override is being discarded.
        let persistence = Persistence::new(VolumePercent::new(60).unwrap());
        let current = Settings {
            volume: VolumePercent::new(50).unwrap(),
            theme: ThemeName::Neon,
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(to_save.volume.get(), 60, "volume override discarded");
        assert_eq!(
            to_save.theme,
            ThemeName::Neon,
            "theme override still persists"
        );
    }
}
