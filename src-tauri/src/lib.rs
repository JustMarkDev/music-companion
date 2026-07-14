#[cfg(not(target_os = "windows"))]
compile_error!("Music Companion is currently Windows-only.");

use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WebviewWindow, WindowEvent,
};
use tauri_plugin_updater::UpdaterExt;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaState {
    has_session: bool,
    is_playing: bool,
    status: String,
    title: String,
    artist: String,
    album: String,
    source_app: String,
    position_ms: u64,
    duration_ms: Option<u64>,
    playback_rate: Option<f64>,
    playing_session_count: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HotkeyStatus {
    action: String,
    accelerator: String,
    registered: bool,
    error: Option<String>,
}

static HOTKEY_STATUSES: OnceLock<Mutex<Vec<HotkeyStatus>>> = OnceLock::new();

#[tauri::command]
fn get_hotkey_statuses() -> Vec<HotkeyStatus> {
    HOTKEY_STATUSES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|statuses| statuses.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn register_hotkey(
    app: tauri::AppHandle,
    action: String,
    accelerator: String,
) -> Result<HotkeyStatus, String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let shortcut = accelerator
        .parse::<Shortcut>()
        .map_err(|error| format!("Invalid shortcut: {error}"))?;
    let mut statuses = HOTKEY_STATUSES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map_err(|_| "Hotkey registry is unavailable".to_string())?;

    if let Some(current) = statuses.iter().find(|status| status.action == action) {
        if current.accelerator == accelerator && current.registered {
            return Ok(current.clone());
        }
        if current.registered {
            if let Ok(old_shortcut) = current.accelerator.parse::<Shortcut>() {
                let _ = app.global_shortcut().unregister(old_shortcut);
            }
        }
    }

    let status = match app.global_shortcut().register(shortcut) {
        Ok(()) => {
            println!("[hotkey] registered {accelerator} for {action}");
            HotkeyStatus {
                action: action.clone(),
                accelerator,
                registered: true,
                error: None,
            }
        }
        Err(error) => {
            eprintln!("[hotkey] unable to register {accelerator} for {action}: {error}");
            HotkeyStatus {
                action: action.clone(),
                accelerator,
                registered: false,
                error: Some(error.to_string()),
            }
        }
    };
    statuses.retain(|item| item.action != action);
    statuses.push(status.clone());
    Ok(status)
}

impl MediaState {
    fn no_session(status: &str) -> Self {
        Self {
            has_session: false,
            is_playing: false,
            status: status.to_string(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            source_app: String::new(),
            position_ms: 0,
            duration_ms: None,
            playback_rate: None,
            playing_session_count: 0,
        }
    }
}

static MEDIA_QUERY_RUNNING: AtomicBool = AtomicBool::new(false);
static LATEST_LYRICS_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[tauri::command]
fn cancel_lyrics_requests(request_id: u64) {
    LATEST_LYRICS_REQUEST.fetch_max(request_id, Ordering::AcqRel);
}
static MEDIA_CACHE: OnceLock<Mutex<Option<MediaState>>> = OnceLock::new();
const MEDIA_QUERY_WAIT: Duration = Duration::from_millis(650);

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LyricsResult {
    source: String,
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: Option<u64>,
    instrumental: bool,
    synced_lyrics: Option<String>,
    romanized_synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibLyrics {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[tauri::command]
async fn get_media_state() -> Result<MediaState, String> {
    if MEDIA_QUERY_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Windows media session is starting")));
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    tauri::async_runtime::spawn_blocking(move || {
        let result = media::current_media_state()
            .map_err(|error| error.to_string())
            .map(retain_last_media_when_session_disappears);
        if let Ok(state) = &result {
            store_media_state(state.clone());
        }
        MEDIA_QUERY_RUNNING.store(false, Ordering::Release);
        let _ = sender.send(result);
    });

    match tokio::time::timeout(MEDIA_QUERY_WAIT, receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Windows media session reader stopped"))),
        Err(_) => Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Windows media session is starting"))),
    }
}

fn cached_media_state() -> Option<MediaState> {
    MEDIA_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .clone()
}

fn store_media_state(state: MediaState) {
    if let Ok(mut cache) = MEDIA_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *cache = Some(state);
    }
}

#[tauri::command]
fn log_sync_diagnostic(event: String, details: String) {
    println!("[sync] {event} {details}");
}

fn retain_last_media_when_session_disappears(state: MediaState) -> MediaState {
    retain_cached_media_when_session_disappears(state, cached_media_state())
}

fn retain_cached_media_when_session_disappears(
    state: MediaState,
    cached: Option<MediaState>,
) -> MediaState {
    if state.has_session {
        return state;
    }

    let Some(mut cached) = cached.filter(|media| media.has_session) else {
        return state;
    };

    // Some players unregister their WMTC session when playback is paused.
    // Keep the latest track visible and freeze its clock until Windows reports
    // another session, rather than clearing the overlay immediately.
    cached.is_playing = false;
    cached.playback_rate = None;
    cached.status = "Paused session unavailable".to_string();
    cached
}

#[cfg(test)]
mod media_state_tests {
    use super::{retain_cached_media_when_session_disappears, MediaState};

    fn state(has_session: bool, is_playing: bool, position_ms: u64) -> MediaState {
        MediaState {
            has_session,
            is_playing,
            status: String::new(),
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            album: String::new(),
            source_app: String::new(),
            position_ms,
            duration_ms: Some(180_000),
            playback_rate: Some(1.0),
            playing_session_count: 0,
        }
    }

    #[test]
    fn retains_and_pauses_cached_track_when_windows_drops_the_session() {
        let retained = retain_cached_media_when_session_disappears(
            state(false, false, 0),
            Some(state(true, true, 60_000)),
        );

        assert!(retained.has_session);
        assert!(!retained.is_playing);
        assert_eq!(retained.position_ms, 60_000);
        assert_eq!(retained.title, "Track");
        assert_eq!(retained.playback_rate, None);
    }
}

#[tauri::command]
async fn fetch_lyrics(
    title: String,
    artist: String,
    duration_ms: Option<u64>,
    request_id: u64,
) -> Result<Option<LyricsResult>, String> {
    LATEST_LYRICS_REQUEST.fetch_max(request_id, Ordering::AcqRel);
    let started_at = Instant::now();
    let result = lyrics::fetch_lyrics(&title, &artist, duration_ms, request_id).await;
    if matches!(&result, Err(error) if error == "lyrics request superseded") {
        println!("[lyrics] cancelled superseded request id={request_id} title={title:?}");
        return result;
    }
    let duration_seconds = duration_ms.map(|value| value as f64 / 1_000.0);
    println!(
        "[latency] lyrics total={}ms title={title:?} artist={artist:?} duration_seconds={duration_seconds:?} found={}",
        started_at.elapsed().as_millis(),
        matches!(&result, Ok(Some(_)))
    );
    result
}

#[tauri::command]
fn get_start_at_login() -> Result<bool, String> {
    startup::get_start_at_login()
}

#[tauri::command]
fn set_start_at_login(enabled: bool) -> Result<(), String> {
    startup::set_start_at_login(enabled)
}

#[tauri::command]
fn set_always_on_top(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_window_material(
    app: tauri::AppHandle,
    material: String,
    intensity: u8,
) -> Result<(), String> {
    if material != "mica" && material != "acrylic" {
        return Err("Unsupported window material".to_string());
    }

    for label in ["main", "settings"] {
        let window = app
            .get_webview_window(label)
            .ok_or_else(|| format!("{label} window is unavailable"))?;
        persistent_backdrop::apply(&window, intensity.min(100), &material)?;
    }
    Ok(())
}

#[tauri::command]
fn show_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "Settings window is unavailable".to_string())?;
    window
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window
        .emit("settings-window-opened", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    release_hotkeys_and_exit(&app);
}

fn release_hotkeys_and_exit(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if let Err(error) = app.global_shortcut().unregister_all() {
        eprintln!("[hotkey] unable to unregister shortcuts while quitting: {error}");
    }
    app.exit(0);
}

#[tauri::command]
fn retry_failed_hotkeys(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        for delay in [250, 750, 1_500] {
            std::thread::sleep(Duration::from_millis(delay));
            let failed = get_hotkey_statuses()
                .into_iter()
                .filter(|status| !status.registered)
                .collect::<Vec<_>>();
            if failed.is_empty() {
                return;
            }
            let mut recovered = false;
            for status in failed {
                recovered |= register_hotkey(
                    app.clone(),
                    status.action.clone(),
                    status.accelerator.clone(),
                )
                .is_ok_and(|status| status.registered);
            }
            if recovered {
                let _ = app.emit("hotkey-statuses-changed", ());
            }
        }
    });
}

pub fn run() {
    use tauri_plugin_global_shortcut::{Code, Shortcut, ShortcutState};
    use tauri_plugin_window_state::StateFlags;

    let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let action = HOTKEY_STATUSES
                .get()
                .and_then(|statuses| statuses.lock().ok())
                .and_then(|statuses| {
                    statuses
                        .iter()
                        .find(|status| {
                            status.registered
                                && status
                                    .accelerator
                                    .parse::<Shortcut>()
                                    .is_ok_and(|registered| &registered == shortcut)
                        })
                        .map(|status| status.action.clone())
                });
            if action.as_deref() == Some("pinned") {
                println!("[hotkey] pinned-mode hotkey used");
                let _ = app.emit("toggle-overlay-lock", ());
                return;
            }
            let control = match action.as_deref() {
                Some("next") => Some("next"),
                Some("previous") => Some("previous"),
                Some("playPause") => Some("play/pause"),
                _ => None,
            };
            if let Some(action) = control {
                println!("[hotkey] media hotkey used: {action}");
                let uses_same_media_key = shortcut.mods.is_empty()
                    && matches!(
                        (action, shortcut.key),
                        ("next", Code::MediaTrackNext) | ("previous", Code::MediaTrackPrevious)
                    );
                let action = action.to_string();
                std::thread::spawn(move || {
                    match media::send_transport_control(&action, !uses_same_media_key) {
                        Ok(accepted) => println!(
                            "[media-control] Windows API received {action}; accepted={accepted}"
                        ),
                        Err(error) => eprintln!("[media-control] {action} failed: {error}"),
                    }
                });
            }
        })
        .build();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::POSITION | StateFlags::SIZE)
                .with_denylist(&["main"])
                .build(),
        )
        .plugin(shortcut_plugin)
        .invoke_handler(tauri::generate_handler![
            get_media_state,
            get_hotkey_statuses,
            register_hotkey,
            retry_failed_hotkeys,
            log_sync_diagnostic,
            fetch_lyrics,
            cancel_lyrics_requests,
            get_start_at_login,
            set_start_at_login,
            set_always_on_top,
            set_window_material,
            show_settings_window,
            quit_app
        ])
        .setup(|app| {
            build_tray(app)?;
            media::start_event_monitor(app.handle().clone());
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = persistent_backdrop::apply(&window, 100, "acrylic") {
                    eprintln!("Failed to enable the persistent overlay backdrop: {error}");
                }
                overlay_z_order::start_monitor(window);
            }
            start_automatic_update(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run Music Companion");
}

fn start_automatic_update(app: tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }

    tauri::async_runtime::spawn(async move {
        if let Err(error) = update_and_restart(app).await {
            eprintln!("Automatic update failed: {error}");
        }
    });
}

async fn update_and_restart(app: tauri::AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };

    println!("Downloading Music Companion {}", update.version);
    update.download_and_install(|_, _| {}, || {}).await?;
    app.restart();
}

fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Unlock overlay", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Music Companion")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    unlock_overlay(&window);
                }
            }
            "quit" => release_hotkeys_and_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    unlock_overlay(&window);
                }
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn unlock_overlay(window: &WebviewWindow) {
    let _ = window.set_ignore_cursor_events(false);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit("overlay-unlocked", ());
}

#[cfg(target_os = "windows")]
mod persistent_backdrop {
    use std::{ffi::c_void, mem};
    use tauri::WebviewWindow;
    use windows::{
        core::PCSTR,
        Win32::{
            Foundation::{BOOL, HWND},
            System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        },
    };

    const WCA_ACCENT_POLICY: u32 = 0x13;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND: u32 = 4;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_NONE: u32 = 1;
    const DWMSBT_MAINWINDOW: u32 = 2;

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }

    #[repr(C)]
    struct WindowCompositionAttributeData {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }

    type SetWindowCompositionAttribute =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> BOOL;
    type DwmSetWindowAttribute = unsafe extern "system" fn(HWND, u32, *const c_void, u32) -> i32;

    pub fn apply(window: &WebviewWindow, intensity: u8, material: &str) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;

        if material == "mica" {
            set_acrylic(HWND(hwnd.0), 0)?;
            set_dwm_backdrop(HWND(hwnd.0), DWMSBT_MAINWINDOW)
                .or_else(|_| set_acrylic(HWND(hwnd.0), intensity))
        } else {
            let _ = set_dwm_backdrop(HWND(hwnd.0), DWMSBT_NONE);
            set_acrylic(HWND(hwnd.0), intensity)
        }
    }

    fn set_acrylic(hwnd: HWND, intensity: u8) -> Result<(), String> {
        // The documented Windows 11 Acrylic backdrop is disabled for inactive windows.
        // This composition attribute keeps the blur active, which is required for an overlay.
        unsafe {
            let user32 = LoadLibraryA(PCSTR(c"user32.dll".as_ptr().cast()))
                .map_err(|error| error.to_string())?;
            let procedure = GetProcAddress(
                user32,
                PCSTR(c"SetWindowCompositionAttribute".as_ptr().cast()),
            )
            .ok_or_else(|| "SetWindowCompositionAttribute is unavailable".to_string())?;
            let set_window_composition_attribute: SetWindowCompositionAttribute =
                mem::transmute(procedure);

            let mut policy = AccentPolicy {
                state: if intensity == 0 {
                    ACCENT_DISABLED
                } else {
                    ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND
                },
                flags: 0,
                // Acrylic requires non-zero alpha. Increasing it strengthens the perceived
                // backdrop while the webview background independently controls opacity.
                gradient_color: u32::from(intensity.max(1)) << 24,
                animation_id: 0,
            };
            let mut data = WindowCompositionAttributeData {
                attribute: WCA_ACCENT_POLICY,
                data: &mut policy as *mut _ as *mut c_void,
                size: mem::size_of::<AccentPolicy>(),
            };

            if !set_window_composition_attribute(hwnd, &mut data).as_bool() {
                return Err("SetWindowCompositionAttribute rejected the backdrop".to_string());
            }
        }

        Ok(())
    }

    fn set_dwm_backdrop(hwnd: HWND, backdrop_type: u32) -> Result<(), String> {
        unsafe {
            let dwmapi = LoadLibraryA(PCSTR(c"dwmapi.dll".as_ptr().cast()))
                .map_err(|error| error.to_string())?;
            let procedure = GetProcAddress(dwmapi, PCSTR(c"DwmSetWindowAttribute".as_ptr().cast()))
                .ok_or_else(|| "DwmSetWindowAttribute is unavailable".to_string())?;
            let set_window_attribute: DwmSetWindowAttribute = mem::transmute(procedure);
            let result = set_window_attribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop_type as *const _ as *const c_void,
                mem::size_of::<u32>() as u32,
            );
            if result < 0 {
                return Err(format!(
                    "DwmSetWindowAttribute failed with HRESULT {result:#x}"
                ));
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod overlay_z_order {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use tauri::WebviewWindow;
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowLongPtrW, IsWindowVisible, SetWindowPos, GWL_EXSTYLE,
            HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, WS_EX_TOPMOST,
        },
    };

    const MONITOR_INTERVAL: Duration = Duration::from_millis(500);
    const REASSERT_FLAGS: SET_WINDOW_POS_FLAGS =
        SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0 | SWP_ASYNCWINDOWPOS.0);

    pub fn start_monitor(window: WebviewWindow) {
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

                if let Err(error) = reassert_if_needed(overlay_hwnd) {
                    if !reassert_error_reported.swap(true, Ordering::Relaxed) {
                        eprintln!("Unable to restore the overlay Z-order: {error}");
                    }
                }
            }
        });
    }

    fn reassert_if_needed(overlay_hwnd: HWND) -> windows::core::Result<()> {
        // SAFETY: The handles are obtained from Tauri and the Windows foreground-window API.
        // Every operation is observational except SetWindowPos, which preserves position, size,
        // and activation so the foreground application keeps receiving input.
        unsafe {
            let foreground_hwnd = GetForegroundWindow();
            let foreground_exists = !foreground_hwnd.is_invalid();
            let overlay_visible = IsWindowVisible(overlay_hwnd).as_bool();
            let foreground_visible =
                foreground_exists && IsWindowVisible(foreground_hwnd).as_bool();
            let foreground_is_overlay = foreground_exists && foreground_hwnd == overlay_hwnd;
            let foreground_is_topmost = foreground_exists
                && GetWindowLongPtrW(foreground_hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0;

            if should_reassert_overlay(
                overlay_visible,
                foreground_exists,
                foreground_is_overlay,
                foreground_visible,
                foreground_is_topmost,
            ) {
                SetWindowPos(overlay_hwnd, HWND_TOPMOST, 0, 0, 0, 0, REASSERT_FLAGS)?;
            }
        }

        Ok(())
    }

    fn should_reassert_overlay(
        overlay_visible: bool,
        foreground_exists: bool,
        foreground_is_overlay: bool,
        foreground_visible: bool,
        foreground_is_topmost: bool,
    ) -> bool {
        overlay_visible
            && foreground_exists
            && !foreground_is_overlay
            && foreground_visible
            && foreground_is_topmost
    }

    #[cfg(test)]
    mod tests {
        use super::should_reassert_overlay;

        #[test]
        fn reasserts_over_visible_topmost_foreground_window() {
            assert!(should_reassert_overlay(true, true, false, true, true));
        }

        #[test]
        fn ignores_normal_foreground_window() {
            assert!(!should_reassert_overlay(true, true, false, true, false));
        }

        #[test]
        fn ignores_hidden_overlay() {
            assert!(!should_reassert_overlay(false, true, false, true, true));
        }

        #[test]
        fn ignores_overlay_as_foreground_window() {
            assert!(!should_reassert_overlay(true, true, true, true, true));
        }

        #[test]
        fn ignores_missing_foreground_window() {
            assert!(!should_reassert_overlay(true, false, false, false, false));
        }

        #[test]
        fn ignores_hidden_foreground_window() {
            assert!(!should_reassert_overlay(true, true, false, false, true));
        }
    }
}

mod media {
    use super::MediaState;
    use std::{
        sync::{Arc, Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tauri::Emitter;
    use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
    use windows::Media::Control::{
        CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus, MediaPropertiesChangedEventArgs,
        PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VK_MEDIA_NEXT_TRACK, VK_MEDIA_PREV_TRACK,
    };

    const WINDOWS_EPOCH_OFFSET_MS: u64 = 11_644_473_600_000;
    static SELECTED_SESSION: OnceLock<Mutex<Option<GlobalSystemMediaTransportControlsSession>>> =
        OnceLock::new();

    struct SessionSubscription {
        session: GlobalSystemMediaTransportControlsSession,
        _media_properties_token: EventRegistrationToken,
        _playback_info_token: EventRegistrationToken,
    }

    pub fn start_event_monitor(app: tauri::AppHandle) {
        std::thread::spawn(move || {
            if let Err(error) = run_event_monitor(app) {
                eprintln!("Unable to subscribe to Windows media events: {error}");
            }
        });
    }

    fn run_event_monitor(app: tauri::AppHandle) -> windows::core::Result<()> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
        let subscriptions = Arc::new(Mutex::new(Vec::<SessionSubscription>::new()));
        subscribe_to_sessions(&manager, &app, &subscriptions)?;

        let current_app = app.clone();
        let _current_session_token =
            manager.CurrentSessionChanged(&TypedEventHandler::<
                GlobalSystemMediaTransportControlsSessionManager,
                CurrentSessionChangedEventArgs,
            >::new(move |_, _| {
                emit_media_change(&current_app, "current-session");
                Ok(())
            }))?;

        let sessions_app = app.clone();
        let sessions_state = subscriptions.clone();
        let _sessions_token = manager.SessionsChanged(&TypedEventHandler::<
            GlobalSystemMediaTransportControlsSessionManager,
            SessionsChangedEventArgs,
        >::new(move |manager, _| {
            if let Some(manager) = manager {
                if let Err(error) = subscribe_to_sessions(manager, &sessions_app, &sessions_state) {
                    eprintln!("Unable to refresh Windows media event subscriptions: {error}");
                }
            }
            emit_media_change(&sessions_app, "sessions");
            Ok(())
        }))?;

        println!("[latency] Windows media event monitor ready");
        loop {
            std::thread::park();
        }
    }

    fn subscribe_to_sessions(
        manager: &GlobalSystemMediaTransportControlsSessionManager,
        app: &tauri::AppHandle,
        subscriptions: &Arc<Mutex<Vec<SessionSubscription>>>,
    ) -> windows::core::Result<()> {
        let sessions = manager.GetSessions()?;
        for index in 0..sessions.Size()? {
            let session = sessions.GetAt(index)?;
            if subscriptions
                .lock()
                .is_ok_and(|items| items.iter().any(|item| item.session == session))
            {
                continue;
            }

            let media_app = app.clone();
            let media_properties_token =
                session.MediaPropertiesChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    MediaPropertiesChangedEventArgs,
                >::new(move |_, _| {
                    emit_media_change(&media_app, "media-properties");
                    Ok(())
                }))?;

            let playback_app = app.clone();
            let playback_info_token =
                session.PlaybackInfoChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    PlaybackInfoChangedEventArgs,
                >::new(move |_, _| {
                    emit_media_change(&playback_app, "playback-info");
                    Ok(())
                }))?;

            if let Ok(mut items) = subscriptions.lock() {
                items.push(SessionSubscription {
                    session,
                    _media_properties_token: media_properties_token,
                    _playback_info_token: playback_info_token,
                });
            }
        }
        Ok(())
    }

    fn emit_media_change(app: &tauri::AppHandle, reason: &str) {
        let _ = app.emit("media-state-changed", reason);
    }

    pub fn send_transport_control(
        action: &str,
        allow_media_key_fallback: bool,
    ) -> windows::core::Result<bool> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
        let session = selected_session().or_else(|| manager.GetCurrentSession().ok());
        let Some(session) = session else {
            println!(
                "[media-control] {action} requested, but no Windows media session is available"
            );
            return Ok(false);
        };
        println!("[media-control] sending {action} to Windows media session");
        let accepted = match action {
            "next" => session.TrySkipNextAsync()?.get(),
            "previous" => session.TrySkipPreviousAsync()?.get(),
            "play/pause" => session.TryTogglePlayPauseAsync()?.get(),
            _ => Ok(false),
        }?;

        if accepted {
            return Ok(true);
        }

        let media_key = match (action, allow_media_key_fallback) {
            ("next", true) => Some(VK_MEDIA_NEXT_TRACK),
            ("previous", true) => Some(VK_MEDIA_PREV_TRACK),
            _ => None,
        };
        let Some(media_key) = media_key else {
            return Ok(false);
        };

        println!("[media-control] Windows session declined {action}; sending media key fallback");
        send_media_key(media_key)?;
        Ok(true)
    }

    fn send_media_key(
        key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    ) -> windows::core::Result<()> {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: key,
                        ..Default::default()
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: key,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            },
        ];
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err(windows::core::Error::from_win32())
        }
    }

    pub fn current_media_state() -> windows::core::Result<MediaState> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
        let sessions = manager.GetSessions()?;
        let mut playing_count = 0;
        let mut available = Vec::new();

        for index in 0..sessions.Size()? {
            let session = sessions.GetAt(index)?;
            let playback = session.GetPlaybackInfo()?;
            if playback.PlaybackStatus()?
                == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            {
                playing_count += 1;
            }
            available.push(session);
        }

        // Keep a valid selected music session even when another application
        // starts playing. Browser videos frequently become Windows' current
        // session, but must not replace the paused song or its lyrics.
        let retained = selected_session().filter(session_is_available);
        let current = manager.GetCurrentSession().ok();
        let current_is_playing = current.as_ref().is_some_and(session_is_playing);
        let playing = if current_is_playing {
            current.clone()
        } else {
            available
                .iter()
                .find(|session| session_is_playing(session))
                .cloned()
        };
        let selected = retained.or(playing).or(current);

        let Some(session) = selected else {
            store_selected_session(None);
            return Ok(MediaState::no_session("No session"));
        };
        store_selected_session(Some(session.clone()));

        let playback = session.GetPlaybackInfo()?;
        let status = playback.PlaybackStatus()?;
        let properties = session.TryGetMediaPropertiesAsync()?.get()?;
        let timeline = session.GetTimelineProperties()?;
        let timeline_position_ms = timespan_to_ms(timeline.Position()?);
        let end_ms = timespan_to_ms(timeline.EndTime()?);
        let playback_rate = playback
            .PlaybackRate()
            .ok()
            .and_then(|value| value.Value().ok());
        let position_ms = current_timeline_position(
            timeline_position_ms,
            end_ms,
            timeline.LastUpdatedTime()?.UniversalTime,
            windows_now_ms(),
            playback_rate.unwrap_or(1.0),
            status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing,
        );

        Ok(MediaState {
            has_session: true,
            is_playing: status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing,
            status: format!("{status:?}"),
            title: properties.Title()?.to_string_lossy(),
            artist: properties.Artist()?.to_string_lossy(),
            album: properties.AlbumTitle()?.to_string_lossy(),
            source_app: session.SourceAppUserModelId()?.to_string_lossy(),
            position_ms,
            duration_ms: (end_ms > 0).then_some(end_ms),
            playback_rate,
            playing_session_count: playing_count,
        })
    }

    fn selected_session() -> Option<GlobalSystemMediaTransportControlsSession> {
        SELECTED_SESSION
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()?
            .clone()
    }

    fn store_selected_session(session: Option<GlobalSystemMediaTransportControlsSession>) {
        if let Ok(mut selected) = SELECTED_SESSION.get_or_init(|| Mutex::new(None)).lock() {
            *selected = session;
        }
    }

    fn session_is_playing(session: &GlobalSystemMediaTransportControlsSession) -> bool {
        session
            .GetPlaybackInfo()
            .and_then(|playback| playback.PlaybackStatus())
            .is_ok_and(|status| {
                status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            })
    }

    fn session_is_available(session: &GlobalSystemMediaTransportControlsSession) -> bool {
        session.GetPlaybackInfo().is_ok()
    }

    fn timespan_to_ms(value: windows::Foundation::TimeSpan) -> u64 {
        if value.Duration <= 0 {
            return 0;
        }
        (value.Duration / 10_000) as u64
    }

    fn windows_now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
            .saturating_add(WINDOWS_EPOCH_OFFSET_MS)
    }

    fn current_timeline_position(
        position_ms: u64,
        _duration_ms: u64,
        last_updated_ticks: i64,
        now_ms: u64,
        playback_rate: f64,
        is_playing: bool,
    ) -> u64 {
        if !is_playing
            || last_updated_ticks <= 0
            || !playback_rate.is_finite()
            || playback_rate <= 0.0
        {
            return position_ms;
        }

        let last_updated_ms = last_updated_ticks as u64 / 10_000;
        let elapsed_ms = now_ms.saturating_sub(last_updated_ms);
        let position_ms =
            position_ms.saturating_add((elapsed_ms as f64 * playback_rate).round() as u64);

        // A number of WMTC providers publish stale EndTime values during
        // playback. The caller still receives that duration as metadata, but
        // the live clock must remain monotonic instead of freezing at it.
        position_ms
    }

    #[cfg(test)]
    mod tests {
        use super::current_timeline_position;

        #[test]
        fn advances_the_position_while_playing() {
            assert_eq!(
                current_timeline_position(60_000, 180_000, 1_000_000_000, 102_500, 1.0, true),
                62_500
            );
        }

        #[test]
        fn keeps_the_reported_position_while_paused() {
            assert_eq!(
                current_timeline_position(60_000, 180_000, 1_000_000_000, 102_500, 1.0, false),
                60_000
            );
        }

        #[test]
        fn keeps_advancing_past_a_stale_track_duration() {
            assert_eq!(
                current_timeline_position(179_000, 180_000, 1_000_000_000, 105_000, 1.0, true),
                184_000
            );
        }
    }
}

mod romanization {
    use ib_romaji::HepburnRomanizer;
    use lindera::dictionary::load_dictionary;
    use lindera::mode::Mode;
    use lindera::segmenter::Segmenter;
    use lindera::tokenizer::Tokenizer;
    use pinyin::ToPinyin;
    use std::sync::OnceLock;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum LyricsLanguage {
        Japanese,
        Korean,
        Chinese,
    }

    pub fn romanize_lrc(lyrics: &str) -> Option<String> {
        let language = detect_language(lyrics)?;
        let mut changed = false;
        let lines = lyrics
            .lines()
            .map(|line| {
                let text_start = line.rfind(']').map_or(0, |index| index + 1);
                let (prefix, text) = line.split_at(text_start);
                let romanized =
                    capitalize_first_letter(&romanize_text(text.trim_start(), language));
                changed |= romanized != text.trim_start();
                let separator = if text.starts_with(' ') { " " } else { "" };
                format!("{prefix}{separator}{romanized}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        changed.then_some(lines)
    }

    fn capitalize_first_letter(text: &str) -> String {
        let Some((index, character)) = text
            .char_indices()
            .find(|(_, character)| character.is_alphabetic())
        else {
            return text.to_string();
        };
        let uppercase = character.to_uppercase().collect::<String>();
        format!(
            "{}{}{}",
            &text[..index],
            uppercase,
            &text[index + character.len_utf8()..]
        )
    }

    fn detect_language(text: &str) -> Option<LyricsLanguage> {
        if text.chars().any(is_kana) {
            Some(LyricsLanguage::Japanese)
        } else if text.chars().any(is_hangul) {
            Some(LyricsLanguage::Korean)
        } else if text.chars().any(is_han) {
            Some(LyricsLanguage::Chinese)
        } else {
            None
        }
    }

    fn romanize_text(text: &str, language: LyricsLanguage) -> String {
        match language {
            LyricsLanguage::Japanese => romanize_japanese(text),
            LyricsLanguage::Korean => romanize_korean(text),
            LyricsLanguage::Chinese => romanize_chinese(text),
        }
    }

    fn romanize_japanese(text: &str) -> String {
        static TOKENIZER: OnceLock<Result<Tokenizer, String>> = OnceLock::new();
        let tokenizer = TOKENIZER.get_or_init(|| {
            let dictionary =
                load_dictionary("embedded://ipadic").map_err(|error| error.to_string())?;
            Ok(Tokenizer::new(Segmenter::new(
                Mode::Normal,
                dictionary,
                None,
            )))
        });
        let Ok(tokenizer) = tokenizer else {
            return romanize_japanese_without_segmentation(text);
        };
        let Ok(mut tokens) = tokenizer.tokenize(text) else {
            return romanize_japanese_without_segmentation(text);
        };

        let romanizer = japanese_romanizer();
        let mut output = String::new();
        for token in &mut tokens {
            let surface = token.surface.to_string();
            let details = token.details();
            let is_particle = details.first().is_some_and(|part| *part == "助詞");
            let reading = details.get(7).copied().filter(|value| *value != "*");
            let romanized = match (is_particle, surface.as_str()) {
                (true, "は") => "wa".to_string(),
                (true, "へ") => "e".to_string(),
                (true, "を") => "o".to_string(),
                _ => reading
                    .and_then(|value| romanizer.romanize_kana_str_all(value))
                    .unwrap_or_else(|| romanize_japanese_without_segmentation(&surface)),
            };

            if is_japanese_punctuation(&surface) {
                output.push_str(&romanized);
            } else {
                if output.chars().last().is_some_and(|character| {
                    !character.is_whitespace() && !is_opening_punctuation(character)
                }) {
                    output.push(' ');
                }
                output.push_str(&romanized);
            }
        }
        output
    }

    fn japanese_romanizer() -> &'static HepburnRomanizer {
        static ROMANIZER: OnceLock<HepburnRomanizer> = OnceLock::new();
        ROMANIZER.get_or_init(|| {
            HepburnRomanizer::builder()
                .kana(true)
                .kanji(true)
                .word(true)
                .build()
        })
    }

    fn romanize_japanese_without_segmentation(text: &str) -> String {
        let romanizer = japanese_romanizer();
        let mut output = String::new();
        let mut offset = 0;

        while offset < text.len() {
            let remaining = &text[offset..];
            if let Some((length, romaji)) = romanizer
                .romanize_vec(remaining)
                .into_iter()
                .max_by_key(|(length, _)| *length)
            {
                output.push_str(romaji);
                offset += length;
            } else {
                let character = remaining
                    .chars()
                    .next()
                    .expect("remaining text is non-empty");
                output.push(character);
                offset += character.len_utf8();
            }
        }
        output
    }

    fn is_japanese_punctuation(value: &str) -> bool {
        value.chars().all(|character| {
            character.is_ascii_punctuation()
                || matches!(
                    character,
                    '、' | '。' | '！' | '？' | '…' | '・' | '」' | '』' | '）' | '】'
                )
        })
    }

    fn is_opening_punctuation(character: char) -> bool {
        matches!(character, '(' | '[' | '{' | '「' | '『' | '（' | '【')
    }

    fn romanize_chinese(text: &str) -> String {
        let mut output = String::new();
        let mut previous_was_pinyin = false;
        for character in text.chars() {
            if let Some(pinyin) = character.to_pinyin() {
                if previous_was_pinyin
                    || output
                        .chars()
                        .last()
                        .is_some_and(|last| last.is_alphanumeric())
                {
                    output.push(' ');
                }
                output.push_str(pinyin.plain());
                previous_was_pinyin = true;
            } else {
                output.push(character);
                previous_was_pinyin = false;
            }
        }
        output
    }

    fn romanize_korean(text: &str) -> String {
        const INITIALS: [&str; 19] = [
            "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k",
            "t", "p", "h",
        ];
        const VOWELS: [&str; 21] = [
            "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u",
            "wo", "we", "wi", "yu", "eu", "ui", "i",
        ];
        const FINALS: [&str; 28] = [
            "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m",
            "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t",
        ];

        text.chars()
            .map(|character| {
                let code = character as u32;
                if !(0xAC00..=0xD7A3).contains(&code) {
                    return character.to_string();
                }
                let syllable = code - 0xAC00;
                let initial = (syllable / 588) as usize;
                let vowel = ((syllable % 588) / 28) as usize;
                let final_consonant = (syllable % 28) as usize;
                format!(
                    "{}{}{}",
                    INITIALS[initial], VOWELS[vowel], FINALS[final_consonant]
                )
            })
            .collect()
    }

    fn is_kana(character: char) -> bool {
        matches!(character as u32, 0x3040..=0x30FF | 0x31F0..=0x31FF)
    }

    fn is_hangul(character: char) -> bool {
        matches!(character as u32, 0xAC00..=0xD7A3 | 0x1100..=0x11FF)
    }

    fn is_han(character: char) -> bool {
        matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
    }

    #[cfg(test)]
    mod tests {
        use super::romanize_lrc;

        #[test]
        fn romanizes_japanese_lyrics_without_changing_timestamps() {
            assert_eq!(
                romanize_lrc("[00:01.00] 今日は\n[00:02.00]ありがとう").as_deref(),
                Some("[00:01.00] Kyou wa\n[00:02.00]Arigatou")
            );
        }

        #[test]
        fn uses_contextual_japanese_readings_and_word_boundaries() {
            assert_eq!(
                romanize_lrc("[00:01.00] 明日は学校へ行く").as_deref(),
                Some("[00:01.00] Ashita wa gakkou e iku")
            );
        }

        #[test]
        fn romanizes_chinese_lyrics_as_plain_pinyin() {
            assert_eq!(
                romanize_lrc("[00:01.00] 中国人").as_deref(),
                Some("[00:01.00] Zhong guo ren")
            );
        }

        #[test]
        fn romanizes_hangul_lyrics() {
            assert_eq!(
                romanize_lrc("[00:01.00] 한글").as_deref(),
                Some("[00:01.00] Hangeul")
            );
        }

        #[test]
        fn ignores_lyrics_that_are_already_latin() {
            assert_eq!(romanize_lrc("[00:01.00] Hello world"), None);
        }
    }
}

mod lyrics {
    use super::{LrclibLyrics, LyricsResult, LATEST_LYRICS_REQUEST};
    use std::collections::HashSet;
    use std::sync::OnceLock;

    static HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    pub async fn fetch_lyrics(
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
        request_id: u64,
    ) -> Result<Option<LyricsResult>, String> {
        search(http_client()?, title, artist, duration_ms, request_id).await
    }

    fn http_client() -> Result<&'static reqwest::Client, String> {
        HTTP_CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    .user_agent(concat!(
                        "MusicCompanion/",
                        env!("CARGO_PKG_VERSION"),
                        " (https://github.com/JustMarkDev/Music-Companion)"
                    ))
                    // Use Windows' TLS stack and certificate store, matching
                    // the trust configuration used by the browser.
                    .connect_timeout(std::time::Duration::from_secs(8))
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    async fn search(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
        request_id: u64,
    ) -> Result<Option<LyricsResult>, String> {
        let query = format!("{title} {artist}");
        let broad_url = format!(
            "https://lrclib.net/api/search?q={}",
            urlencoding::encode(&query)
        );
        let structured_url = format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            urlencoding::encode(title),
            urlencoding::encode(artist)
        );
        let request_started_at = std::time::Instant::now();
        let searches = async {
            tokio::join!(
                fetch_candidates(client, structured_url, "structured"),
                fetch_candidates(client, broad_url, "broad")
            )
        };
        tokio::pin!(searches);
        let (structured_result, broad_result) = loop {
            tokio::select! {
                results = &mut searches => break results,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    if LATEST_LYRICS_REQUEST.load(std::sync::atomic::Ordering::Acquire) != request_id {
                        return Err("lyrics request superseded".to_string());
                    }
                }
            }
        };
        let (mut results, search_type) = match (structured_result, broad_result) {
            (Ok(structured), Ok(broad)) => {
                (merge_candidates(broad, structured), "structured + broad")
            }
            (Ok(structured), Err(broad_error)) => {
                println!("[lyrics] {broad_error}; using structured results");
                (structured, "structured")
            }
            (Err(structured_error), Ok(broad)) => {
                println!("[lyrics] {structured_error}; using broad results");
                (broad, "broad")
            }
            (Err(structured_error), Err(broad_error)) => {
                return Err(format!("{structured_error}; {broad_error}"));
            }
        };
        println!(
            "[latency] LRCLIB total={}ms search={search_type} candidates={}",
            request_started_at.elapsed().as_millis(),
            results.len()
        );

        results.retain(|item| duration_matches(item.duration, duration_ms));

        let normalized_artist = normalize(artist);
        let normalized_title = canonical_title(title, &normalized_artist);
        results.sort_by_key(|item| {
            ranking_key(item, &normalized_title, &normalized_artist, duration_ms)
        });

        Ok(results.into_iter().next().map(LrclibLyrics::into_result))
    }

    fn merge_candidates(
        mut broad: Vec<LrclibLyrics>,
        structured: Vec<LrclibLyrics>,
    ) -> Vec<LrclibLyrics> {
        // Stable ranking and first-match deduplication preserve broad-search
        // priority whenever candidates are otherwise equivalent.
        broad.extend(structured);
        deduplicate_candidates(&mut broad);
        broad
    }

    fn deduplicate_candidates(candidates: &mut Vec<LrclibLyrics>) {
        let mut seen = HashSet::new();
        candidates.retain(|candidate| {
            seen.insert((
                candidate.track_name.clone(),
                candidate.artist_name.clone(),
                candidate.album_name.clone(),
                candidate.duration.map(f64::to_bits),
            ))
        });
    }

    async fn fetch_candidates(
        client: &reqwest::Client,
        url: String,
        search_type: &str,
    ) -> Result<Vec<LrclibLyrics>, String> {
        let request_started_at = std::time::Instant::now();
        let response = send_request(client, &url, search_type).await?;
        let headers_received_at = std::time::Instant::now();
        let status = response.status();

        if !status.is_success() {
            println!(
                "[latency] LRCLIB {search_type} headers={}ms status={status}",
                headers_received_at
                    .duration_since(request_started_at)
                    .as_millis(),
            );
            return Ok(Vec::new());
        }

        let results = response
            .json::<Vec<LrclibLyrics>>()
            .await
            .map_err(|error| format!("{search_type} search: {error}"))?;
        println!(
            "[network] LRCLIB {search_type} succeeded status={status} headers={}ms body={}ms candidates={}",
            headers_received_at
                .duration_since(request_started_at)
                .as_millis(),
            headers_received_at.elapsed().as_millis(),
            results.len()
        );
        Ok(results)
    }

    async fn send_request(
        client: &reqwest::Client,
        url: &str,
        search_type: &str,
    ) -> Result<reqwest::Response, String> {
        client.get(url).send().await.map_err(|error| {
            println!("[lyrics] {search_type} request failed: {error:?}");
            format!("{search_type} search: {error}")
        })
    }

    fn normalize(value: &str) -> String {
        value
            .to_lowercase()
            .chars()
            .filter(|char| char.is_alphanumeric() || char.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn canonical_title(value: &str, normalized_artist: &str) -> String {
        let normalized_title = normalize(value);
        if normalized_artist.is_empty() {
            return normalized_title;
        }

        normalized_title
            .strip_prefix(normalized_artist)
            .and_then(|title| title.strip_prefix(' '))
            .or_else(|| {
                normalized_title
                    .strip_suffix(normalized_artist)
                    .and_then(|title| title.strip_suffix(' '))
            })
            .unwrap_or(&normalized_title)
            .to_string()
    }

    fn score(value: Option<&str>, expected: &str) -> u8 {
        if expected.is_empty() {
            return 0;
        }

        let Some(value) = value else {
            return 0;
        };

        let value = normalize(value);
        if value == expected {
            4
        } else if value.contains(expected) || expected.contains(&value) {
            2
        } else {
            0
        }
    }

    fn duration_difference_ms(candidate_seconds: Option<f64>, expected_ms: Option<u64>) -> u64 {
        let Some(expected_ms) = expected_ms else {
            return 0;
        };
        let Some(candidate_seconds) = candidate_seconds.filter(|value| value.is_finite()) else {
            return u64::MAX;
        };

        let candidate_ms = (candidate_seconds.max(0.0) * 1_000.0).round() as u64;
        candidate_ms.abs_diff(expected_ms)
    }

    fn duration_matches(candidate_seconds: Option<f64>, expected_ms: Option<u64>) -> bool {
        const DURATION_TOLERANCE_MS: u64 = 3_000;

        expected_ms.is_none()
            || duration_difference_ms(candidate_seconds, expected_ms) <= DURATION_TOLERANCE_MS
    }

    fn has_synced_lyrics(candidate: &LrclibLyrics) -> bool {
        candidate.instrumental || has_lyrics(candidate.synced_lyrics.as_deref())
    }

    fn has_lyrics(lyrics: Option<&str>) -> bool {
        lyrics.is_some_and(|value| !value.trim().is_empty())
    }

    fn ranking_key(
        candidate: &LrclibLyrics,
        normalized_title: &str,
        normalized_artist: &str,
        duration_ms: Option<u64>,
    ) -> (
        std::cmp::Reverse<bool>,
        std::cmp::Reverse<bool>,
        std::cmp::Reverse<u8>,
        u64,
    ) {
        let candidate_title = candidate
            .track_name
            .as_deref()
            .map(|title| canonical_title(title, normalized_artist));
        let title_score = [
            score(candidate_title.as_deref(), normalized_title),
            score(candidate.album_name.as_deref(), normalized_title),
        ]
        .into_iter()
        .max()
        .unwrap_or_default();
        let artist_score = [
            score(candidate.artist_name.as_deref(), normalized_artist),
            score(candidate.track_name.as_deref(), normalized_artist),
            score(candidate.album_name.as_deref(), normalized_artist),
        ]
        .into_iter()
        .max()
        .unwrap_or_default();
        let metadata_score = title_score * 4 + artist_score * 3;
        let metadata_matches = title_score > 0 && artist_score > 0;

        (
            std::cmp::Reverse(metadata_matches),
            std::cmp::Reverse(has_synced_lyrics(candidate)),
            std::cmp::Reverse(metadata_score),
            duration_difference_ms(candidate.duration, duration_ms),
        )
    }

    impl LrclibLyrics {
        fn into_result(self) -> LyricsResult {
            let romanized_synced_lyrics = self
                .synced_lyrics
                .as_deref()
                .and_then(super::romanization::romanize_lrc);
            LyricsResult {
                source: "LRCLIB".to_string(),
                track_name: self.track_name.unwrap_or_default(),
                artist_name: self.artist_name.unwrap_or_default(),
                album_name: self.album_name.unwrap_or_default(),
                duration: self.duration.map(|value| value.round() as u64),
                instrumental: self.instrumental,
                synced_lyrics: self.synced_lyrics,
                romanized_synced_lyrics,
                plain_lyrics: self.plain_lyrics,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn candidate(track_name: &str, artist_name: &str, duration: f64) -> LrclibLyrics {
            candidate_with_metadata(track_name, artist_name, None, duration, true)
        }

        fn candidate_with_metadata(
            track_name: &str,
            artist_name: &str,
            album_name: Option<&str>,
            duration: f64,
            synced: bool,
        ) -> LrclibLyrics {
            LrclibLyrics {
                track_name: Some(track_name.to_string()),
                artist_name: Some(artist_name.to_string()),
                album_name: album_name.map(str::to_string),
                duration: Some(duration),
                instrumental: false,
                synced_lyrics: synced.then(|| "[00:00.00]Lyrics".to_string()),
                plain_lyrics: Some("Lyrics".to_string()),
            }
        }

        #[test]
        fn metadata_match_outranks_closer_duration() {
            let normalized_artist = normalize("Jace June");
            let normalized_title = canonical_title("Goodbye My Baby", &normalized_artist);
            let expected_duration_ms = Some(182_000);
            let mut results = [
                candidate("Deeper Than It Seems", "Jace June", 182.0),
                candidate("Goodbye My Baby", "Jace June", 194.0),
            ];

            results.sort_by_key(|item| {
                ranking_key(
                    item,
                    &normalized_title,
                    &normalized_artist,
                    expected_duration_ms,
                )
            });

            assert_eq!(results[0].track_name.as_deref(), Some("Goodbye My Baby"));
        }

        #[test]
        fn combined_artist_and_title_forms_have_equal_metadata_rank() {
            let normalized_artist = normalize("Jace June");
            let normalized_title = canonical_title("Goodbye My Baby", &normalized_artist);
            let expected_duration_ms = Some(194_000);
            let candidates = [
                candidate("Goodbye My Baby", "Jace June", 194.0),
                candidate("Jace June - Goodbye My Baby", "Jace June", 194.0),
                candidate("Goodbye My Baby - Jace June", "Jace June", 194.0),
            ];

            let keys = candidates.map(|item| {
                ranking_key(
                    &item,
                    &normalized_title,
                    &normalized_artist,
                    expected_duration_ms,
                )
            });

            assert_eq!(keys[0], keys[1]);
            assert_eq!(keys[1], keys[2]);
        }

        #[test]
        fn only_durations_within_three_seconds_are_eligible() {
            assert!(duration_matches(Some(177.0), Some(180_000)));
            assert!(duration_matches(Some(183.0), Some(180_000)));
            assert!(!duration_matches(Some(176.999), Some(180_000)));
            assert!(!duration_matches(Some(184.0), Some(180_000)));
            assert!(!duration_matches(Some(215.0), Some(180_000)));
            assert!(!duration_matches(None, Some(180_000)));
            assert!(duration_matches(Some(215.0), None));
        }

        #[test]
        fn broad_candidate_is_kept_when_searches_return_the_same_metadata() {
            let mut broad = candidate("Golden Brown", "The Stranglers", 206.781);
            broad.synced_lyrics = Some("[00:21.04] Broad".to_string());
            let mut structured = candidate("Golden Brown", "The Stranglers", 206.781);
            structured.synced_lyrics = Some("[00:22.95] Structured".to_string());

            let results = merge_candidates(vec![broad], vec![structured]);

            assert_eq!(results.len(), 1);
            assert_eq!(
                results[0].synced_lyrics.as_deref(),
                Some("[00:21.04] Broad")
            );
        }

        #[test]
        fn synced_combined_metadata_outranks_exact_unsynced_metadata() {
            let normalized_artist = normalize("Temper City");
            let normalized_title = canonical_title("Self Aware", &normalized_artist);
            let expected_duration_ms = Some(181_000);
            let mut results = [
                candidate_with_metadata(
                    "Self Aware",
                    "Temper City",
                    Some("Self Aware"),
                    181.0,
                    false,
                ),
                candidate_with_metadata(
                    "Temper City - Self Aware",
                    "DanceHype",
                    Some("Self Aware Temper City"),
                    181.0,
                    true,
                ),
            ];

            results.sort_by_key(|item| {
                ranking_key(
                    item,
                    &normalized_title,
                    &normalized_artist,
                    expected_duration_ms,
                )
            });

            assert_eq!(results[0].artist_name.as_deref(), Some("DanceHype"));
        }

        #[test]
        fn unrelated_synced_candidate_does_not_outrank_relevant_unsynced_candidate() {
            let normalized_artist = normalize("Temper City");
            let normalized_title = canonical_title("Self Aware", &normalized_artist);
            let expected_duration_ms = Some(181_000);
            let mut results = [
                candidate_with_metadata(
                    "Self Aware",
                    "Temper City",
                    Some("Self Aware"),
                    181.0,
                    false,
                ),
                candidate("Different Song", "Different Artist", 181.0),
            ];

            results.sort_by_key(|item| {
                ranking_key(
                    item,
                    &normalized_title,
                    &normalized_artist,
                    expected_duration_ms,
                )
            });

            assert_eq!(results[0].track_name.as_deref(), Some("Self Aware"));
        }
    }
}

mod startup {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APP_NAME: &str = "Music Companion";

    pub fn get_start_at_login() -> Result<bool, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run = hkcu
            .open_subkey(RUN_KEY)
            .map_err(|error| error.to_string())?;
        Ok(run.get_value::<String, _>(APP_NAME).is_ok())
    }

    pub fn set_start_at_login(enabled: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(|error| error.to_string())?;

        if enabled {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let value = format!("\"{}\"", exe.display());
            run.set_value(APP_NAME, &value)
                .map_err(|error| error.to_string())?;
        } else {
            let _ = run.delete_value(APP_NAME);
        }

        Ok(())
    }
}
