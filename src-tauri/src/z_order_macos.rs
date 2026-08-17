//! macOS overlay stacking.
//!
//! Windows needs a monitor thread that re-asserts `HWND_TOPMOST` because other
//! applications steal the topmost slot. macOS keeps a window level until it is
//! changed, so this module only has to raise the level once and opt the overlay
//! into every Space.

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use tauri::WebviewWindow;

/// One above `NSMainMenuWindowLevel`, which keeps lyrics visible over fullscreen
/// video while staying below system pop-up menus.
const OVERLAY_WINDOW_LEVEL: NSWindowLevel = 25;

/// Named `start_monitor` to mirror the Windows backend's entry point, even though
/// macOS needs no polling to hold the overlay in place.
pub fn start_monitor(window: WebviewWindow) {
    if let Err(error) = apply(&window) {
        eprintln!("[overlay] unable to raise the overlay above other windows: {error}");
    }
}

/// Re-applies the overlay level. `set_always_on_top` resets the window to the
/// standard floating level, so it has to be called again afterwards.
pub fn apply(window: &WebviewWindow) -> Result<(), String> {
    let window = window.clone();
    let target = window.clone();
    target
        .run_on_main_thread(move || {
            let Ok(handle) = window.ns_window() else {
                eprintln!("[overlay] the overlay has no native window yet");
                return;
            };

            // SAFETY: `ns_window` returns the window's `NSWindow`, this closure runs
            // on the main thread, and the reference is not held past this call.
            let native: &NSWindow = unsafe { &*handle.cast::<NSWindow>() };
            native.setLevel(OVERLAY_WINDOW_LEVEL);
            native.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        })
        .map_err(|error| error.to_string())
}
