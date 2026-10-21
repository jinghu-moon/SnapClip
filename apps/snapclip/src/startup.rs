//! What a launch starts (`docs/41` S2).
//!
//! The development entry opens the save panel so it can be looked at while it is being built. It
//! is **additive**: it adds a window to a normal launch and cannot take a subsystem away.
//!
//! That property is the fix for a real regression. The entry used to replace the whole startup —
//! the binary opened the panel and returned — which silently took the capture overlay with it,
//! and therefore F5, F7 and F9, because the global hotkeys are registered when the overlay starts.
//! The trigger is sticky in a shell, too: `$env:SNAPCLIP_OPEN_SAVE_PANEL=1` in PowerShell makes
//! every later launch in that window open the panel instead of the application, so the symptom
//! arrives long after the experiment that caused it and looks like a broken machine.
//!
//! So the decision lives here as data, and the tests below pin it: whatever else a launch was
//! asked to do, the shell window and the capture overlay start.

use std::ffi::OsString;

/// What the binary asked the shell to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartupOptions {
    /// Open the save panel beside the normal launch, for looking at it while it is built.
    pub open_save_panel: bool,
}

impl StartupOptions {
    /// The development entry, read from the environment.
    pub fn from_env() -> Self {
        Self::from_presence(std::env::var_os("SNAPCLIP_OPEN_SAVE_PANEL"))
    }

    /// The decision itself, so it can be tested without touching the process environment.
    ///
    /// Presence, not truth: any value counts, including the empty string, which is how the entry
    /// has always been triggered and keeps `$env:X="0"` from being a surprise.
    pub fn from_presence(value: Option<OsString>) -> Self {
        Self {
            open_save_panel: value.is_some(),
        }
    }
}

/// The subsystems a launch starts.
///
/// The shell and the capture overlay are not options: they are what a launch *is*. Only the panel
/// is a request, and a request can only add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subsystems {
    /// The shell window: history and settings.
    pub shell_window: bool,
    /// The capture overlay, which is what registers F5, F7 and F9.
    pub capture: bool,
    /// The save panel, which only the development entry asks for today.
    pub save_panel: bool,
}

/// The subsystems this launch starts.
pub const fn subsystems(options: StartupOptions) -> Subsystems {
    Subsystems {
        shell_window: true,
        capture: true,
        save_panel: options.open_save_panel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression, as a test: the entry that opens the panel must not stop the launch.
    #[test]
    fn the_development_entry_still_starts_the_shell_and_capture() {
        let started = subsystems(StartupOptions {
            open_save_panel: true,
        });
        assert!(started.shell_window, "the shell window must start");
        assert!(
            started.capture,
            "the capture overlay must start: it is what registers F5, F7 and F9"
        );
    }

    /// Additive, not alternative: the only difference is the panel.
    #[test]
    fn the_development_entry_only_adds_a_panel() {
        let normal = subsystems(StartupOptions::default());
        let entry = subsystems(StartupOptions {
            open_save_panel: true,
        });
        assert!(!normal.save_panel, "a normal launch opens no panel");
        assert!(entry.save_panel, "the entry opens the panel");
        assert_eq!(normal.shell_window, entry.shell_window);
        assert_eq!(normal.capture, entry.capture);
    }

    /// Presence is the trigger, and it is read without the process environment.
    #[test]
    fn the_presence_of_the_variable_is_what_asks_for_the_panel() {
        assert!(StartupOptions::from_presence(None).open_save_panel == false);
        assert!(StartupOptions::from_presence(Some(OsString::from("1"))).open_save_panel);
        assert!(StartupOptions::from_presence(Some(OsString::from(""))).open_save_panel);
        assert!(
            StartupOptions::from_presence(Some(OsString::from("0"))).open_save_panel,
            "any value asks for the panel: the entry has always been presence-triggered"
        );
    }

    /// The binary has no control flow of its own.
    ///
    /// This is the shape of the bug rather than its cause: the entry lived in the binary and
    /// returned before the application started, so nothing below it ran. Keeping the binary a
    /// single call is what makes that impossible to write again by accident — and if a future
    /// launch really needs a branch, this test is where the author has to say why.
    #[test]
    fn the_binary_cannot_skip_a_subsystem() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
            .expect("the binary's source is readable");
        assert!(
            !source.contains("return"),
            "the binary must not leave early: that is how the development entry skipped the hotkeys"
        );
        assert!(
            !source.contains("if "),
            "the binary must not decide anything: the decision belongs in `startup`"
        );
        assert!(
            source.contains("run("),
            "the binary must start the application"
        );
    }
}
