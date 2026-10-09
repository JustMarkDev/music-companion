#[cfg(not(any(target_os = "windows", target_os = "macos")))]
compile_error!("Music Companion supports Windows and macOS only.");

use serde::Serialize;
use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{
    Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_updater::UpdaterExt;

mod hotkeys;
mod lyrics;
mod ttml;

use lyrics::LyricsResult;

// Each platform provides its own media, backdrop, and stacking backend behind a
// shared interface, so the orchestration below stays platform-neutral.
#[cfg(target_os = "macos")]
#[path = "media_macos.rs"]
mod media;
#[cfg(target_os = "macos")]
#[path = "z_order_macos.rs"]
mod overlay_z_order;
#[cfg(target_os = "macos")]
#[path = "backdrop_macos.rs"]
mod persistent_backdrop;

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

#[tauri::command]
fn get_hotkey_statuses() -> Vec<hotkeys::HotkeyStatus> {
    hotkeys::statuses()
}

#[tauri::command]
fn set_hotkey_recording(app: tauri::AppHandle, recording: bool) -> Result<(), String> {
    hotkeys::set_recording(app, recording)
}

#[tauri::command]
fn register_hotkey(
    app: tauri::AppHandle,
    action: String,
    accelerator: String,
) -> Result<hotkeys::HotkeyStatus, String> {
    hotkeys::register(app, action, accelerator)
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

#[tauri::command]
async fn get_media_state() -> Result<MediaState, String> {
    if MEDIA_QUERY_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Media session is starting")));
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
            .unwrap_or_else(|| MediaState::no_session("Media session reader stopped"))),
        Err(_) => Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Media session is starting"))),
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

    // Some players unregister their media session when playback is paused.
    // Keep the latest track visible and freeze its clock until the system reports
    // another session, rather than clearing the overlay immediately.
    cached.is_playing = false;
    cached.playback_rate = None;
    cached.status = "Paused session unavailable".to_string();
    cached
}

#[cfg(test)]
mod media_state_tests {
    use super::{MediaState, retain_cached_media_when_session_disappears};

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
    fn retains_and_pauses_cached_track_when_the_system_drops_the_session() {
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

    #[test]
    fn keeps_a_reported_media_session_instead_of_the_cache() {
        let reported = state(true, true, 90_000);
        let retained = retain_cached_media_when_session_disappears(
            reported.clone(),
            Some(state(true, false, 60_000)),
        );

        assert_eq!(retained.position_ms, reported.position_ms);
        assert_eq!(retained.is_playing, reported.is_playing);
    }

    #[test]
    fn keeps_no_session_when_there_is_no_valid_cached_session() {
        let reported = state(false, false, 0);
        let retained = retain_cached_media_when_session_disappears(
            reported.clone(),
            Some(state(false, false, 60_000)),
        );

        assert!(!retained.has_session);
        assert_eq!(retained.position_ms, reported.position_ms);
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

/// Word-timed lyrics for a song, from lrc.red's alignment model. `None` when
/// lrc.red cannot time it; an error is worth retrying later.
#[tauri::command]
async fn sync_lyrics_words(
    title: String,
    artist: String,
    duration_ms: Option<u64>,
) -> Result<Option<LyricsResult>, String> {
    lyrics::sync_words(&title, &artist, duration_ms).await
}

/// The autostart plugin registers a `Run` key on Windows and a launch agent on
/// macOS, which keeps one implementation for both platforms.
#[tauri::command]
fn get_start_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;

    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_start_at_login(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;

    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable().map_err(|error| error.to_string())
    } else {
        autolaunch.disable().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn set_always_on_top(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|error| error.to_string())?;

    // Tauri drops the overlay back to the standard floating level, which is below
    // fullscreen windows. Restore the level the overlay needs.
    #[cfg(target_os = "macos")]
    if enabled && let Some(overlay) = window.get_webview_window("main") {
        overlay_z_order::apply(&overlay)?;
    }

    Ok(())
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

    // macOS orders windows strictly by level, so the standard always-on-top level
    // would leave the settings window stuck behind the raised overlay. Matching the
    // overlay's level restores the Windows behaviour, where focus decides which of
    // the two is in front.
    #[cfg(target_os = "macos")]
    overlay_z_order::apply(&window)?;

    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    focus_without_cursor_warp(&window).map_err(|error| error.to_string())?;
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
    media::shutdown();
    app.exit(0);
}

#[tauri::command]
fn retry_failed_hotkeys(app: tauri::AppHandle) {
    hotkeys::retry_failed(app);
}

pub fn run() {
    use tauri_plugin_global_shortcut::{Code, ShortcutState};
    use tauri_plugin_window_state::StateFlags;

    let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let action = hotkeys::action_for(shortcut);
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
                let _ = app.emit("media-hotkey", action);
                let uses_same_media_key = shortcut.mods.is_empty()
                    && matches!(
                        (action, shortcut.key),
                        ("next", Code::MediaTrackNext) | ("previous", Code::MediaTrackPrevious)
                    );
                let action = action.to_string();
                std::thread::spawn(move || {
                    match media::send_transport_control(&action, !uses_same_media_key) {
                        Ok(accepted) => println!(
                            "[media-control] system media API received {action}; accepted={accepted}"
                        ),
                        Err(error) => eprintln!("[media-control] {action} failed: {error}"),
                    }
                });
            }
        })
        .build();

    // Must be registered first so a second launch exits before other plugins start.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                unlock_overlay(&window);
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::POSITION | StateFlags::SIZE)
                .with_denylist(&["main"])
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(shortcut_plugin);

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_liquid_glass::init());

    builder
        .invoke_handler(tauri::generate_handler![
            get_media_state,
            get_hotkey_statuses,
            set_hotkey_recording,
            register_hotkey,
            retry_failed_hotkeys,
            log_sync_diagnostic,
            fetch_lyrics,
            sync_lyrics_words,
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
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let _ = window.hide();
                api.prevent_close();
            }
            WindowEvent::Focused(true) if !SKIP_FOCUS_CURSOR_WARP.swap(false, Ordering::SeqCst) => {
                move_cursor_to_title_bar(window);
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("failed to run Music Companion");
}

/// Where the pointer lands, in logical pixels below the window's top edge. This is
/// inside the drag strip of both the overlay and the settings header.
const TITLE_BAR_CURSOR_OFFSET: f64 = 16.0;

/// Where to warp the pointer so a window that just took focus can be dragged at once,
/// or `None` when the pointer is already over the window, as after a click. All values
/// are physical pixels except `scale`.
fn title_bar_cursor_target(
    cursor: (f64, f64),
    origin: (f64, f64),
    size: (f64, f64),
    scale: f64,
) -> Option<(f64, f64)> {
    let inside = cursor.0 >= origin.0
        && cursor.0 < origin.0 + size.0
        && cursor.1 >= origin.1
        && cursor.1 < origin.1 + size.1;
    (!inside).then(|| {
        (
            origin.0 + size.0 / 3.0,
            origin.1 + TITLE_BAR_CURSOR_OFFSET * scale,
        )
    })
}

/// Set just before a programmatic focus so the resulting focus event leaves the pointer
/// alone; only keyboard switching should move it.
static SKIP_FOCUS_CURSOR_WARP: AtomicBool = AtomicBool::new(false);

/// Focuses a window without moving the pointer to its title bar.
fn focus_without_cursor_warp(window: &WebviewWindow) -> tauri::Result<()> {
    // An already focused window raises no event, which would leave the flag set.
    if !window.is_focused()? {
        SKIP_FOCUS_CURSOR_WARP.store(true, Ordering::SeqCst);
    }
    let result = window.set_focus();
    if result.is_err() {
        SKIP_FOCUS_CURSOR_WARP.store(false, Ordering::SeqCst);
    }
    result
}

/// Moves the pointer onto the title bar after Cmd+Tab or Alt+Tab focuses a window.
fn move_cursor_to_title_bar(window: &tauri::Window) {
    let (Ok(cursor), Ok(origin), Ok(size), Ok(scale)) = (
        window.cursor_position(),
        window.outer_position(),
        window.outer_size(),
        window.scale_factor(),
    ) else {
        return;
    };
    if let Some((x, y)) = title_bar_cursor_target(
        (cursor.x, cursor.y),
        (f64::from(origin.x), f64::from(origin.y)),
        (f64::from(size.width), f64::from(size.height)),
        scale,
    ) {
        // tao takes client-area coordinates on Windows and adds the inner position on
        // macOS, so both want the target relative to the inner position.
        let Ok(inner) = window.inner_position() else {
            return;
        };
        let _ = window.set_cursor_position(PhysicalPosition::new(
            x - f64::from(inner.x),
            y - f64::from(inner.y),
        ));
    }
}

#[cfg(test)]
mod title_bar_cursor_tests {
    use super::title_bar_cursor_target;

    #[test]
    fn moves_a_pointer_outside_the_window_onto_its_title_bar() {
        let target = title_bar_cursor_target((10.0, 10.0), (100.0, 200.0), (600.0, 400.0), 2.0);
        assert_eq!(target, Some((300.0, 232.0)));
    }

    #[test]
    fn leaves_a_pointer_that_is_already_over_the_window() {
        let target = title_bar_cursor_target((150.0, 250.0), (100.0, 200.0), (600.0, 400.0), 2.0);
        assert_eq!(target, None);
    }
}

fn start_automatic_update(app: tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }

    // Replacing the app bundle in place requires a Developer ID signature that
    // Gatekeeper accepts. Until release signing is configured, macOS updates are
    // installed manually. See the release notes in README.md.
    if cfg!(target_os = "macos") {
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
                && let Some(window) = tray.app_handle().get_webview_window("main")
            {
                unlock_overlay(&window);
            }
        });

    // A template icon lets the macOS menu bar recolour the glyph for light, dark,
    // and tinted appearances.
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn unlock_overlay(window: &WebviewWindow) {
    let _ = window.set_ignore_cursor_events(false);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = focus_without_cursor_warp(window);
    let _ = window.emit("overlay-unlocked", ());
}

#[cfg(target_os = "windows")]
mod persistent_backdrop {
    use std::{ffi::c_void, mem};
    use tauri::WebviewWindow;
    use windows::{
        Win32::{
            Foundation::HWND,
            System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        },
        core::{BOOL, PCSTR},
    };

    const WCA_ACCENT_POLICY: u32 = 0x13;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_ACRYLIC_BLUR_BEHIND: u32 = 4;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_NONE: u32 = 1;

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
        // Windows offers only Acrylic. A stored Mica choice from an older
        // version is treated as Acrylic so legacy settings keep working.
        // Mica (`DWMSBT_MAINWINDOW`) is intentionally no longer applied.
        let _ = material;
        let _ = set_dwm_backdrop(HWND(hwnd.0), DWMSBT_NONE);
        set_acrylic(HWND(hwnd.0), intensity)
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
    use tauri::{Manager, WebviewWindow};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
            HWND_TOPMOST, IsWindowVisible, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, WS_EX_TOPMOST,
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
            let foreground_visible =
                foreground_exists && IsWindowVisible(foreground_hwnd).as_bool();
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
}

#[cfg(target_os = "windows")]
mod media {
    use super::MediaState;
    use std::{
        sync::{Arc, Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tauri::Emitter;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus, MediaPropertiesChangedEventArgs,
        PlaybackInfoChangedEventArgs, SessionsChangedEventArgs, TimelinePropertiesChangedEventArgs,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VK_MEDIA_NEXT_TRACK, VK_MEDIA_PREV_TRACK,
    };

    const WINDOWS_EPOCH_OFFSET_MS: u64 = 11_644_473_600_000;
    static SELECTED_SESSION: OnceLock<Mutex<Option<GlobalSystemMediaTransportControlsSession>>> =
        OnceLock::new();

    struct SessionSubscription {
        session: GlobalSystemMediaTransportControlsSession,
        // windows 0.62 represents event registration tokens as plain i64.
        _media_properties_token: i64,
        _playback_info_token: i64,
        _timeline_properties_token: i64,
    }

    pub fn start_event_monitor(app: tauri::AppHandle) {
        std::thread::spawn(move || {
            if let Err(error) = run_event_monitor(app) {
                eprintln!("Unable to subscribe to Windows media events: {error}");
            }
        });
    }

    /// WMTC event subscriptions are released with the process, so there is nothing
    /// to tear down here. macOS needs this hook to stop its helper process.
    pub fn shutdown() {}

    fn run_event_monitor(app: tauri::AppHandle) -> windows::core::Result<()> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
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
            if let Some(manager) = &*manager
                && let Err(error) = subscribe_to_sessions(manager, &sessions_app, &sessions_state)
            {
                eprintln!("Unable to refresh Windows media event subscriptions: {error}");
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

            // Players announce a jump in position (seeking, a restart) here, which
            // no other event reports.
            let timeline_app = app.clone();
            let timeline_properties_token =
                session.TimelinePropertiesChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    TimelinePropertiesChangedEventArgs,
                >::new(move |_, _| {
                    emit_media_change(&timeline_app, "timeline-properties");
                    Ok(())
                }))?;

            if let Ok(mut items) = subscriptions.lock() {
                items.push(SessionSubscription {
                    session,
                    _media_properties_token: media_properties_token,
                    _playback_info_token: playback_info_token,
                    _timeline_properties_token: timeline_properties_token,
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
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
        let session = selected_session().or_else(|| manager.GetCurrentSession().ok());
        let Some(session) = session else {
            println!(
                "[media-control] {action} requested, but no Windows media session is available"
            );
            return Ok(false);
        };
        println!("[media-control] sending {action} to Windows media session");
        let accepted = match action {
            "next" => session.TrySkipNextAsync()?.join(),
            "previous" => session.TrySkipPreviousAsync()?.join(),
            "play/pause" => session.TryTogglePlayPauseAsync()?.join(),
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
            Err(windows::core::Error::from_hresult(
                windows::core::HRESULT::from_win32(unsafe {
                    windows::Win32::Foundation::GetLastError().0
                }),
            ))
        }
    }

    pub fn current_media_state() -> windows::core::Result<MediaState> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
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
        // Browsers rank below every other app, so a browser that was the only
        // session at launch gives way once a music app registers one.
        let retained = selected_session().filter(session_is_available);
        let current = manager.GetCurrentSession().ok();
        let playing = current
            .clone()
            .filter(session_is_playing)
            .or_else(|| available.iter().find(|s| session_is_playing(s)).cloned());
        let is_music = |session: &GlobalSystemMediaTransportControlsSession| !is_browser(session);
        let selected = retained
            .clone()
            .filter(is_music)
            .or_else(|| {
                available
                    .iter()
                    .find(|s| is_music(s) && session_is_playing(s))
                    .cloned()
            })
            .or_else(|| current.clone().filter(is_music))
            .or_else(|| available.iter().find(|s| is_music(s)).cloned())
            .or(retained)
            .or(playing)
            .or(current);

        let Some(session) = selected else {
            store_selected_session(None);
            return Ok(MediaState::no_session("No session"));
        };
        store_selected_session(Some(session.clone()));

        let playback = session.GetPlaybackInfo()?;
        let status = playback.PlaybackStatus()?;
        let properties = session.TryGetMediaPropertiesAsync()?.join()?;
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

    fn is_browser(session: &GlobalSystemMediaTransportControlsSession) -> bool {
        session
            .SourceAppUserModelId()
            .is_ok_and(|id| is_browser_app(&id.to_string_lossy()))
    }

    /// Matches the app ID WMTC reports for the common browsers.
    // ponytail: substring list; Firefox installs report a path hash, so only its
    // default install is known. Extend the list for other browsers as they surface.
    fn is_browser_app(app_id: &str) -> bool {
        let app_id = app_id.to_ascii_lowercase();
        [
            "chrome",
            "chromium",
            "msedge",
            "microsoftedge",
            "firefox",
            "308046b0af4a39cb",
            "brave",
            "opera",
            "vivaldi",
            "zen",
            "librewolf",
        ]
        .iter()
        .any(|browser| app_id.contains(browser))
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
        use super::{current_timeline_position, is_browser_app};

        #[test]
        fn recognises_browsers_but_not_music_apps() {
            for browser in [
                "Chrome",
                "MSEdge",
                "firefox.exe",
                "308046B0AF4A39CB",
                "Brave",
            ] {
                assert!(is_browser_app(browser), "{browser}");
            }
            for music in [
                "Spotify.exe",
                "AppleInc.AppleMusicWin_nzyj5cx40ttqa!App",
                "Cider",
            ] {
                assert!(!is_browser_app(music), "{music}");
            }
        }

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

        #[test]
        fn keeps_the_reported_position_for_invalid_timeline_metadata() {
            assert_eq!(
                current_timeline_position(60_000, 180_000, 0, 102_500, 1.0, true),
                60_000
            );
            assert_eq!(
                current_timeline_position(60_000, 180_000, 1_000_000_000, 102_500, f64::NAN, true,),
                60_000
            );
        }
    }
}

#[cfg(test)]
mod bench_support {
    /// Prints the steady-state throughput of `work` for `bun run bench:rust`.
    pub fn report_ops_per_sec(name: &str, mut work: impl FnMut()) {
        for _ in 0..8 {
            work();
        }
        let started = std::time::Instant::now();
        let mut iterations = 0usize;
        while started.elapsed() < std::time::Duration::from_millis(400) {
            work();
            iterations += 1;
        }
        let ops = iterations as f64 / started.elapsed().as_secs_f64();
        println!("curriculum_metric name={name} unit=ops_per_sec value={ops:.2}");
        assert!(iterations > 0, "{name} completed zero iterations");
    }
}
