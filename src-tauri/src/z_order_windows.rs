use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{Manager, WebviewWindow};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
        HWND_TOPMOST, IsWindowVisible, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, WS_EX_TOPMOST,
    },
};

const MONITOR_INTERVAL: Duration = Duration::from_millis(500);
const REASSERT_FLAGS: SET_WINDOW_POS_FLAGS =
    SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0 | SWP_ASYNCWINDOWPOS.0);

pub fn start_monitor(window: WebviewWindow) {
    let settings = window.app_handle().get_webview_window("settings");
    tauri::async_runtime::spawn(async move {
        let mut reported_handle_error = false;
        let reassert_error_reported = AtomicBool::new(false);

        loop {
            tokio::time::sleep(MONITOR_INTERVAL).await;

            let overlay_hwnd = match window.hwnd() {
                Ok(hwnd) => {
                    reported_handle_error = false;
                    HWND(hwnd.0)
                }
                Err(error) => {
                    if !reported_handle_error {
                        eprintln!("Unable to inspect the overlay window handle: {error}");
                        reported_handle_error = true;
                    }
                    continue;
                }
            };
            let settings_hwnd = settings
                .as_ref()
                .and_then(|window| window.hwnd().ok())
                .map(|hwnd| HWND(hwnd.0));

            if let Err(error) = reassert_if_needed(overlay_hwnd, settings_hwnd)
                && !reassert_error_reported.swap(true, Ordering::Relaxed)
            {
                eprintln!("Unable to restore the overlay Z-order: {error}");
            }
        }
    });
}

fn reassert_if_needed(
    overlay_hwnd: HWND,
    settings_hwnd: Option<HWND>,
) -> windows::core::Result<()> {
    // SAFETY: The handles are obtained from Tauri and the Windows foreground-window API.
    // Every operation is observational except SetWindowPos, which preserves position, size,
    // and activation so the foreground application keeps receiving input.
    unsafe {
        let foreground_hwnd = GetForegroundWindow();
        let foreground_exists = !foreground_hwnd.is_invalid();
        let overlay_visible = IsWindowVisible(overlay_hwnd).as_bool();
        let foreground_visible = foreground_exists && IsWindowVisible(foreground_hwnd).as_bool();
        let foreground_is_overlay = foreground_exists && foreground_hwnd == overlay_hwnd;
        let foreground_is_settings = settings_hwnd.is_some_and(|hwnd| foreground_hwnd == hwnd);
        let foreground_is_same_app =
            foreground_exists && windows_share_process(overlay_hwnd, foreground_hwnd);
        let foreground_is_topmost = foreground_exists
            && GetWindowLongPtrW(foreground_hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0;

        if should_reassert_overlay(
            overlay_visible,
            foreground_exists,
            foreground_is_overlay,
            foreground_is_same_app,
            foreground_visible,
            foreground_is_topmost,
        ) {
            SetWindowPos(overlay_hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, REASSERT_FLAGS)?;
        }

        // Keep the settings window above the lyrics overlay while it is open, without
        // stealing focus when another app is active.
        if let Some(settings_hwnd) = settings_hwnd
            && IsWindowVisible(settings_hwnd).as_bool()
            && (foreground_is_settings || !foreground_is_same_app)
        {
            SetWindowPos(
                settings_hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                REASSERT_FLAGS,
            )?;
        }
    }

    Ok(())
}

fn windows_share_process(left: HWND, right: HWND) -> bool {
    unsafe {
        let mut left_pid = 0u32;
        let mut right_pid = 0u32;
        GetWindowThreadProcessId(left, Some(&mut left_pid));
        GetWindowThreadProcessId(right, Some(&mut right_pid));
        left_pid != 0 && left_pid == right_pid
    }
}

fn should_reassert_overlay(
    overlay_visible: bool,
    foreground_exists: bool,
    foreground_is_overlay: bool,
    foreground_is_same_app: bool,
    foreground_visible: bool,
    foreground_is_topmost: bool,
) -> bool {
    overlay_visible
        && foreground_exists
        && !foreground_is_overlay
        // Settings is also always-on-top. Reasserting over it drops the settings UI
        // behind the lyrics overlay whenever another topmost window (or media update
        // timing) races the monitor loop.
        && !foreground_is_same_app
        && foreground_visible
        && foreground_is_topmost
}

#[cfg(test)]
mod tests {
    use super::should_reassert_overlay;

    #[test]
    fn reasserts_over_visible_topmost_foreground_window() {
        assert!(should_reassert_overlay(
            true, true, false, false, true, true
        ));
    }

    #[test]
    fn ignores_normal_foreground_window() {
        assert!(!should_reassert_overlay(
            true, true, false, false, true, false
        ));
    }

    #[test]
    fn ignores_hidden_overlay() {
        assert!(!should_reassert_overlay(
            false, true, false, false, true, true
        ));
    }

    #[test]
    fn ignores_overlay_as_foreground_window() {
        assert!(!should_reassert_overlay(true, true, true, true, true, true));
    }

    #[test]
    fn ignores_missing_foreground_window() {
        assert!(!should_reassert_overlay(
            true, false, false, false, false, false
        ));
    }

    #[test]
    fn ignores_hidden_foreground_window() {
        assert!(!should_reassert_overlay(
            true, true, false, false, false, true
        ));
    }

    #[test]
    fn ignores_settings_or_other_app_owned_foreground_window() {
        assert!(!should_reassert_overlay(
            true, true, false, true, true, true
        ));
    }
}
