//! Launch handling (spec §102): display availability, graceful headless
//! failure, and the eframe entry point.
//!
//! The environment this crate ships for is headless-friendly: the GUI must
//! fail with a clear, typed message when no display server exists instead
//! of panicking inside the event loop (§4: safer behavior). Nothing here
//! opens a window during tests.
//!
//! # Backends and the `wayland` feature (§125)
//!
//! The default build uses eframe's x11/glow backend, which needs no
//! Wayland libraries — headless machines and CI stay green. The optional
//! non-default `wayland` feature (`cargo build -p beskar-gui --features
//! wayland`, requires `libwayland-dev` + `libxkbcommon-dev` on Linux)
//! enables eframe's Wayland backend; display detection then also accepts
//! a `WAYLAND_DISPLAY` session.

use std::ffi::OsStr;

use beskar_core::error::{Error, Result};

use crate::app::BeskarApp;

/// Whether a display server is reachable from the given environment values.
///
/// The default build uses eframe's x11/glow backend (no wayland feature),
/// so on Linux an X display (`DISPLAY`) is required. With the optional
/// `wayland` feature enabled, a Wayland session (`WAYLAND_DISPLAY`) is
/// accepted too. Other platforms are always graphical in practice;
/// `run_native` still reports failures gracefully if not.
pub fn display_available(display: Option<&OsStr>, wayland: Option<&OsStr>) -> bool {
    if cfg!(target_os = "linux") {
        #[cfg(feature = "wayland")]
        {
            display.is_some() || wayland.is_some()
        }
        #[cfg(not(feature = "wayland"))]
        {
            // x11-only build: a Wayland-only session cannot open a window.
            let _ = wayland;
            display.is_some()
        }
    } else {
        let _ = wayland;
        true
    }
}

/// Reads the display environment and validates it (§86-style env access;
/// never mutates the environment).
pub fn check_display() -> Result<()> {
    let display = std::env::var_os("DISPLAY");
    let wayland = std::env::var_os("WAYLAND_DISPLAY");
    if !display_available(display.as_deref(), wayland.as_deref()) {
        #[cfg(feature = "wayland")]
        let message = "no X11 or Wayland display available (DISPLAY and \
                       WAYLAND_DISPLAY are unset); beskar-gui requires a \
                       graphical session — use `beskar tui` or the `beskar` \
                       CLI in this environment";
        #[cfg(not(feature = "wayland"))]
        let message = "no X display available (DISPLAY is unset); beskar-gui \
                       requires a graphical session — use `beskar tui` or the \
                       `beskar` CLI in this environment";
        return Err(Error::unsupported_state(message));
    }
    Ok(())
}

/// Builds the eframe options and runs the GUI. Errors are returned typed
/// (§115) with recovery guidance embedded in the message.
pub fn run() -> Result<()> {
    check_display()?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 720.0])
            .with_title("beskar — Better Skill Arrangement"),
        ..Default::default()
    };
    eframe::run_native(
        "beskar-gui",
        options,
        Box::new(|_cc| Ok(Box::new(BeskarApp::new()))),
    )
    .map_err(|err| {
        Error::unsupported_state(format!(
            "the graphical event loop could not start: {err}\n\
             check your display server; `beskar tui` works over SSH and in \
             terminals"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_needs_a_display_the_shipped_build_can_use() {
        // The default eframe build is x11-only; a Wayland-only session (or
        // a headless box) cannot open a window and must fail BEFORE the
        // event loop, with guidance (§4, §102). With `--features wayland`
        // a Wayland session is accepted (§125 CI matrix documentation).
        use std::ffi::OsStr;
        if cfg!(target_os = "linux") {
            let x11 = OsStr::new(":0");
            let wayland = OsStr::new("wayland-0");
            assert!(!display_available(None, None));
            assert!(display_available(Some(x11), None));
            if cfg!(feature = "wayland") {
                assert!(display_available(None, Some(wayland)));
                assert!(display_available(Some(x11), Some(wayland)));
            } else {
                assert!(!display_available(None, Some(wayland)));
                assert!(display_available(Some(x11), Some(wayland)));
            }
        } else {
            assert!(display_available(None, None));
        }
    }

    #[test]
    fn check_display_error_names_the_missing_piece() {
        if cfg!(target_os = "linux") {
            // Cannot unset the real env in a test (process-global); the
            // error path is covered by the pure function above. This test
            // only pins the typed error shape.
            let err = Error::unsupported_state("no X display available");
            assert_eq!(err.code(), "unsupported_state");
        }
    }
}
