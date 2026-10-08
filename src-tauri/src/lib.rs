#[cfg(not(any(target_os = "windows", target_os = "macos")))]
compile_error!("Music Companion supports Windows and macOS only.");

use serde::{Deserialize, Serialize};
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

#[derive(Debug, Deserialize, Clone)]
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
        PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
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

/// Byte range of the first enhanced-LRC word tag (`<mm:ss.xx>`) in `text`.
fn next_word_tag(text: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(offset) = text[from..].find('<') {
        let start = from + offset;
        if let Some(length) = text[start + 1..].find('>') {
            let inner = &text[start + 1..start + 1 + length];
            if !inner.is_empty()
                && inner.chars().all(|character| {
                    character.is_ascii_digit() || character == ':' || character == '.'
                })
            {
                return Some((start, start + length + 2));
            }
        }
        from = start + 1;
    }
    None
}

/// True when some line of the enhanced LRC times two or more separate words. A
/// line that only has a start and an end tag is timed as a whole, which the
/// overlay shows line by line, so it does not count.
fn has_word_timing(lrc: &str) -> bool {
    lrc.lines().any(|line| {
        let mut rest = line.rfind(']').map_or(line, |index| &line[index + 1..]);
        let mut words = 0;
        while let Some((start, end)) = next_word_tag(rest) {
            words += usize::from(!rest[..start].trim().is_empty());
            rest = &rest[end..];
        }
        words + usize::from(!rest.trim().is_empty()) >= 2
    })
}

mod romanization {
    use ib_romaji::HepburnRomanizer;
    use lindera::dictionary::load_dictionary;
    use lindera::mode::Mode;
    use lindera::segmenter::Segmenter;
    use pinyin::ToPinyin;
    use std::borrow::Cow;
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
                let romanized = capitalize_first_letter(&if super::next_word_tag(text).is_some() {
                    romanize_tagged(text.trim_start(), language)
                } else {
                    romanize_text(text.trim_start(), language)
                });
                changed |= romanized != text.trim_start();
                let separator = if text.starts_with(' ') { " " } else { "" };
                format!("{prefix}{separator}{romanized}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        changed.then_some(lines)
    }

    /// Romanizes the text between word tags one word at a time, so each romanized
    /// word keeps the timing of the original. Japanese and Chinese romanization is
    /// space-separated, so a space is added between words that had none; Korean
    /// keeps its own spacing because its words are often split mid-word.
    fn romanize_tagged(text: &str, language: LyricsLanguage) -> String {
        let mut output = String::new();
        // Last character of the previous word, unless whitespace followed it.
        let mut open_end: Option<char> = None;
        let mut rest = text;
        loop {
            let (chunk, tag) = match super::next_word_tag(rest) {
                Some((start, end)) => (&rest[..start], Some(&rest[start..end])),
                None => (rest, None),
            };
            let core = chunk.trim();
            if core.is_empty() {
                if !chunk.is_empty() {
                    open_end = None;
                }
                output.push_str(chunk);
            } else {
                let leading = &chunk[..chunk.len() - chunk.trim_start().len()];
                let romanized = romanize_text(core, language);
                let needs_space = language != LyricsLanguage::Korean
                    && leading.is_empty()
                    && open_end.is_some_and(char::is_alphanumeric)
                    && romanized.chars().next().is_some_and(char::is_alphanumeric);
                let trailing = &chunk[chunk.trim_end().len()..];
                output.push_str(leading);
                if needs_space {
                    output.push(' ');
                }
                output.push_str(&romanized);
                output.push_str(trailing);
                open_end = if trailing.is_empty() {
                    romanized.chars().last()
                } else {
                    None
                };
            }
            let Some(tag) = tag else { break };
            output.push_str(tag);
            rest = &rest[chunk.len() + tag.len()..];
        }
        output
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
        static SEGMENTER: OnceLock<Result<Segmenter, String>> = OnceLock::new();
        let segmenter = SEGMENTER.get_or_init(|| {
            let dictionary =
                load_dictionary("embedded://ipadic").map_err(|error| error.to_string())?;
            Ok(Segmenter::new(Mode::Normal, dictionary, None))
        });
        let Ok(segmenter) = segmenter else {
            return romanize_japanese_without_segmentation(text);
        };
        let Ok(mut tokens) = segmenter.segment(Cow::Borrowed(text)) else {
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
        use std::time::{Duration, Instant};

        #[test]
        fn romanizes_each_timed_word_and_keeps_its_tags() {
            assert_eq!(
                romanize_lrc("[00:01.00]<00:01.00>夢<00:01.50>なら<00:02.00>").as_deref(),
                Some("[00:01.00]<00:01.00>Yume<00:01.50> nara<00:02.00>")
            );
        }

        #[test]
        fn timed_words_keep_their_own_spacing_and_do_not_gain_extra_spaces() {
            assert_eq!(
                romanize_lrc(
                    "[00:01.00]<00:01.00>今日は <00:02.00>、<00:02.50>ありがとう<00:03.00>"
                )
                .as_deref(),
                Some("[00:01.00]<00:01.00>Kyou wa <00:02.00>、<00:02.50>arigatou<00:03.00>")
            );
        }

        #[test]
        fn romanizes_timed_chinese_words_as_separate_pinyin_words() {
            assert_eq!(
                romanize_lrc("[00:01.00]<00:01.00>中国<00:02.00>人<00:03.00>").as_deref(),
                Some("[00:01.00]<00:01.00>Zhong guo<00:02.00> ren<00:03.00>")
            );
        }

        #[test]
        fn timed_korean_syllables_stay_joined() {
            assert_eq!(
                romanize_lrc("[00:01.00]<00:01.00>한<00:02.00>글<00:03.00>").as_deref(),
                Some("[00:01.00]<00:01.00>Han<00:02.00>geul<00:03.00>")
            );
        }

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

        fn report_ops_per_sec(name: &str, mut work: impl FnMut()) {
            for _ in 0..8 {
                work();
            }
            let started = Instant::now();
            let mut iterations = 0usize;
            while started.elapsed() < Duration::from_millis(400) {
                work();
                iterations += 1;
            }
            let ops = iterations as f64 / started.elapsed().as_secs_f64();
            println!("curriculum_metric name={name} unit=ops_per_sec value={ops:.2}");
            assert!(iterations > 0, "{name} completed zero iterations");
        }

        fn sample_lrc(lines: &[&str]) -> String {
            lines
                .iter()
                .enumerate()
                .map(|(index, text)| format!("[00:{index:02}.00] {text}"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        #[test]
        #[ignore = "curriculum bench; run via bun run bench:rust"]
        fn curriculum_metric_romanize_japanese_lrc() {
            let lyrics = sample_lrc(&[
                "今日はいい天気ですね",
                "明日は学校へ行く",
                "ありがとうございます",
                "君の名は何ですか",
                "桜の花が咲きました",
            ]);
            report_ops_per_sec("romanize_japanese_lrc", || {
                assert!(romanize_lrc(&lyrics).is_some());
            });
        }

        #[test]
        #[ignore = "curriculum bench; run via bun run bench:rust"]
        fn curriculum_metric_romanize_chinese_lrc() {
            let lyrics = sample_lrc(&[
                "中国人喜欢喝茶",
                "今天天气很好",
                "我喜欢学习音乐",
                "春天来了花开了",
                "月亮代表我的心",
            ]);
            report_ops_per_sec("romanize_chinese_lrc", || {
                assert!(romanize_lrc(&lyrics).is_some());
            });
        }

        #[test]
        #[ignore = "curriculum bench; run via bun run bench:rust"]
        fn curriculum_metric_romanize_korean_lrc() {
            let lyrics = sample_lrc(&[
                "한글은 아름답습니다",
                "오늘 날씨가 좋아요",
                "나는 음악을 좋아해요",
                "친구와 함께 가요",
                "밤하늘에 별이 빛나요",
            ]);
            report_ops_per_sec("romanize_korean_lrc", || {
                assert!(romanize_lrc(&lyrics).is_some());
            });
        }
    }
}

mod lyrics {
    use super::{LATEST_LYRICS_REQUEST, LrclibLyrics, LyricsResult};
    use serde::Deserialize;
    use std::collections::HashSet;
    use std::sync::OnceLock;
    use std::time::Duration;

    static HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

    /// Total time one provider gets for its whole lookup (every request it
    /// makes, not each one), so a stalled service cannot hold up an answer a
    /// lower-priority provider already has.
    const PROVIDER_TIMEOUT: Duration = Duration::from_secs(6);

    type Lookup = Result<Option<LyricsResult>, String>;

    /// Gives `lookup` at most `deadline`; running out of time is an error, so the
    /// lookup is retried later instead of being cached as a miss.
    async fn within_deadline(
        provider: &str,
        deadline: Duration,
        lookup: impl Future<Output = Lookup>,
    ) -> Lookup {
        tokio::time::timeout(deadline, lookup)
            .await
            .unwrap_or_else(|_| Err(provider_error(provider, "timed out")))
    }

    /// Tries ranked candidates in order and returns the first one that yields
    /// lyrics. A candidate whose request fails does not stop the search; its
    /// error is returned only when no candidate yields lyrics, so a transient
    /// failure is retried instead of cached as a miss.
    async fn first_found<K: Clone, T, Fetch: Future<Output = Result<Option<T>, String>>>(
        candidates: Vec<(LrclibLyrics, K)>,
        mut fetch: impl FnMut(K) -> Fetch,
    ) -> Result<Option<(LrclibLyrics, K, T)>, String> {
        let mut failure = None;
        for (candidate, key) in candidates {
            match fetch(key.clone()).await {
                Ok(Some(found)) => return Ok(Some((candidate, key, found))),
                Ok(None) => {}
                Err(error) => failure = Some(error),
            }
        }
        failure.map_or(Ok(None), Err)
    }

    /// Queries lrc.red, LRCLIB and Netease concurrently and returns the answer
    /// of the highest-priority provider that has synced lyrics. Providers are
    /// dropped (cancelled) as soon as the answer is decided.
    pub async fn fetch_lyrics(
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
        request_id: u64,
    ) -> Lookup {
        let client = http_client()?;
        let mut lrc_red = std::pin::pin!(within_deadline(
            "lrc.red",
            PROVIDER_TIMEOUT,
            fetch_lrc_red(client, title, artist, duration_ms)
        ));
        let mut lrclib = std::pin::pin!(within_deadline(
            "LRCLIB",
            PROVIDER_TIMEOUT,
            fetch_lrclib(client, title, artist, duration_ms)
        ));
        let mut netease = std::pin::pin!(within_deadline(
            "Netease",
            PROVIDER_TIMEOUT,
            fetch_netease(client, title, artist, duration_ms)
        ));
        let mut slots: [Option<Lookup>; 3] = [None, None, None];

        loop {
            if let Some(answer) = resolve(&slots) {
                return answer;
            }
            tokio::select! {
                result = &mut lrc_red, if slots[0].is_none() => slots[0] = Some(result),
                result = &mut lrclib, if slots[1].is_none() => slots[1] = Some(result),
                result = &mut netease, if slots[2].is_none() => slots[2] = Some(result),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if LATEST_LYRICS_REQUEST.load(std::sync::atomic::Ordering::Acquire) != request_id {
                        return Err("lyrics request superseded".to_string());
                    }
                }
            }
        }
    }

    /// Providers that can return word-timed lyrics, in the order of `slots`
    /// (lrc.red, LRCLIB, Netease).
    const CAN_TIME_WORDS: [bool; 3] = [true, false, true];

    fn is_word_timed(result: &LyricsResult) -> bool {
        result
            .synced_lyrics
            .as_deref()
            .is_some_and(super::has_word_timing)
    }

    /// Decides the lookup from the providers' answers, listed in priority
    /// order. `None` means an answer that could still change the outcome is
    /// pending.
    ///
    /// Word-timed lyrics beat line-synced ones from any provider, so a
    /// word-timed answer is used once every higher-priority provider that could
    /// also time words has answered. Otherwise synced lyrics (or an instrumental
    /// flag) win in priority order, then plain lyrics. An error surfaces only
    /// when no provider produced anything, so the miss is retried instead of
    /// cached.
    fn resolve(slots: &[Option<Lookup>]) -> Option<Lookup> {
        for (slot, can_time_words) in slots.iter().zip(CAN_TIME_WORDS) {
            match slot {
                Some(Ok(Some(result))) if is_word_timed(result) => {
                    return Some(Ok(Some(result.clone())));
                }
                None if can_time_words => return None,
                _ => {}
            }
        }

        let mut plain = None;
        let mut failure = None;
        for slot in slots {
            match slot.as_ref()? {
                Ok(Some(result))
                    if result.instrumental || has_lyrics(result.synced_lyrics.as_deref()) =>
                {
                    return Some(Ok(Some(result.clone())));
                }
                Ok(Some(result)) => plain = plain.or(Some(result)),
                Ok(None) => {}
                Err(error) => failure = failure.or(Some(error)),
            }
        }
        Some(match (plain, failure) {
            (Some(result), _) => Ok(Some(result.clone())),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => Ok(None),
        })
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

    async fn fetch_lrclib(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Lookup {
        // Featured-artist suffixes often point at the same recording indexed under
        // the base title, so search and rank on the stripped title.
        let title = strip_feature_credits(title);
        let query = format!("{title} {artist}");
        let broad_url = format!(
            "https://lrclib.net/api/search?q={}",
            urlencoding::encode(&query)
        );
        let structured_url = format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            urlencoding::encode(&title),
            urlencoding::encode(artist)
        );
        let request_started_at = std::time::Instant::now();
        let (structured_result, broad_result) = tokio::join!(
            fetch_candidates(client, structured_url, "structured"),
            fetch_candidates(client, broad_url, "broad")
        );
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
        let normalized_title = canonical_title(&title, &normalized_artist);
        results.sort_by_key(|item| {
            ranking_key(item, &normalized_title, &normalized_artist, duration_ms)
        });

        Ok(results.into_iter().next().map(LrclibLyrics::into_result))
    }

    /// Drops trailing "(feat. X)", "[ft. X]", "(with X)" or " featuring X" credits.
    /// Returns the title unchanged if stripping would leave nothing.
    fn strip_feature_credits(title: &str) -> String {
        let trimmed = title.trim();
        let mut stripped = trimmed.to_string();
        while let Some(next) = strip_one_feature_credit(&stripped) {
            if next.is_empty() {
                return trimmed.to_string();
            }
            stripped = next;
        }
        stripped
    }

    fn strip_one_feature_credit(title: &str) -> Option<String> {
        let title = title.trim_end();
        strip_trailing_feature_group(title).or_else(|| strip_trailing_feature_phrase(title))
    }

    fn strip_trailing_feature_group(title: &str) -> Option<String> {
        let open_index = match title.chars().last()? {
            ')' => title.rfind('(')?,
            ']' => title.rfind('[')?,
            _ => return None,
        };
        let inner = &title[open_index + 1..title.len() - 1];
        is_feature_credit_label(inner).then(|| title[..open_index].trim_end().to_string())
    }

    fn strip_trailing_feature_phrase(title: &str) -> Option<String> {
        let lower = title.to_ascii_lowercase();
        [" feat. ", " feat ", " ft. ", " ft ", " featuring "]
            .iter()
            .find_map(|marker| lower.rfind(marker))
            .map(|index| title[..index].trim_end().to_string())
            .filter(|stripped| !stripped.is_empty())
    }

    fn is_feature_credit_label(label: &str) -> bool {
        let label = label.trim().to_ascii_lowercase();
        [
            "feat.",
            "feat ",
            "ft.",
            "ft ",
            "featuring ",
            "with ",
            "con ",
        ]
        .iter()
        .any(|prefix| label.starts_with(prefix))
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

    /// How well a candidate's track name alone matches the playing title, 0-4.
    /// Unlike the title score in `metadata_scores`, an album with that name does
    /// not count.
    fn track_title_score(
        candidate: &LrclibLyrics,
        normalized_title: &str,
        normalized_artist: &str,
    ) -> u8 {
        let track_title = candidate
            .track_name
            .as_deref()
            .map(|title| canonical_title(title, normalized_artist));
        score(track_title.as_deref(), normalized_title)
    }

    /// How well a candidate's title and artist match what is playing, each 0-4.
    fn metadata_scores(
        candidate: &LrclibLyrics,
        normalized_title: &str,
        normalized_artist: &str,
    ) -> (u8, u8) {
        // The album only vouches for the title when the hit has no track name:
        // every track of an album named like the playing song (AC/DC's "Back In
        // Black") would otherwise tie with the song itself.
        let title_score = match candidate
            .track_name
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            Some(_) => track_title_score(candidate, normalized_title, normalized_artist),
            None => score(candidate.album_name.as_deref(), normalized_title),
        };
        let artist_score = [
            score(candidate.artist_name.as_deref(), normalized_artist),
            score(candidate.track_name.as_deref(), normalized_artist),
            score(candidate.album_name.as_deref(), normalized_artist),
        ]
        .into_iter()
        .max()
        .unwrap_or_default();
        (title_score, artist_score)
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
        let (title_score, artist_score) =
            metadata_scores(candidate, normalized_title, normalized_artist);
        let metadata_score = title_score * 4 + artist_score * 3;
        let metadata_matches = title_score > 0 && artist_score > 0;

        (
            std::cmp::Reverse(metadata_matches),
            std::cmp::Reverse(has_synced_lyrics(candidate)),
            std::cmp::Reverse(metadata_score),
            duration_difference_ms(candidate.duration, duration_ms),
        )
    }

    /// Metadata-only candidate, so providers other than LRCLIB can reuse
    /// `ranking_key` and its title, artist and duration rules.
    fn metadata_candidate(
        track_name: Option<String>,
        artist_name: Option<String>,
        album_name: Option<String>,
        duration: Option<f64>,
    ) -> LrclibLyrics {
        LrclibLyrics {
            track_name,
            artist_name,
            album_name,
            duration,
            instrumental: false,
            synced_lyrics: None,
            plain_lyrics: None,
        }
    }

    fn is_latin_letter(char: char) -> bool {
        char.is_ascii_alphabetic() || matches!(char, '\u{C0}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}')
    }

    fn has_non_latin_letters(value: &str) -> bool {
        value
            .chars()
            .any(|char| char.is_alphabetic() && !is_latin_letter(char))
    }

    /// True when exactly one of the two titles is written in a non-Latin script,
    /// so a differing spelling says nothing about whether they are the same song.
    /// A hit without a title tells nothing about its script.
    fn titles_differ_in_script(candidate_title: Option<&str>, playing_title: &str) -> bool {
        candidate_title.is_some_and(|title| {
            has_non_latin_letters(title) != has_non_latin_letters(playing_title)
        })
    }

    /// Keeps the hits that plausibly are the playing song, best first. Search
    /// providers return fuzzy results (remixes, covers, other artists), so a hit
    /// needs a matching length and a matching title. The same song can be
    /// credited to an artist written in another script, so the artist need not
    /// match; a title may differ only when it is written in another script, never
    /// to pass off another song by the same artist.
    fn rank_matches<T>(
        mut hits: Vec<(LrclibLyrics, T)>,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Vec<(LrclibLyrics, T)> {
        let normalized_artist = normalize(artist);
        let normalized_title = canonical_title(title, &normalized_artist);
        hits.retain(|(candidate, _)| {
            let (title_score, artist_score) =
                metadata_scores(candidate, &normalized_title, &normalized_artist);
            duration_matches(candidate.duration, duration_ms)
                && (title_score > 0
                    || (artist_score > 0
                        && titles_differ_in_script(candidate.track_name.as_deref(), title)))
        });
        hits.sort_by_key(|(candidate, _)| {
            ranking_key(
                candidate,
                &normalized_title,
                &normalized_artist,
                duration_ms,
            )
        });
        hits
    }

    fn synced_result(source: &str, candidate: LrclibLyrics, synced_lyrics: String) -> LyricsResult {
        build_result(source, candidate, false, Some(synced_lyrics))
    }

    fn instrumental_result(source: &str, candidate: LrclibLyrics) -> LyricsResult {
        build_result(source, candidate, true, None)
    }

    fn build_result(
        source: &str,
        candidate: LrclibLyrics,
        instrumental: bool,
        synced_lyrics: Option<String>,
    ) -> LyricsResult {
        let romanized_synced_lyrics = synced_lyrics
            .as_deref()
            .and_then(super::romanization::romanize_lrc);
        LyricsResult {
            source: source.to_string(),
            track_name: candidate.track_name.unwrap_or_default(),
            artist_name: candidate.artist_name.unwrap_or_default(),
            album_name: candidate.album_name.unwrap_or_default(),
            duration: candidate.duration.map(|value| value.round() as u64),
            instrumental,
            synced_lyrics,
            romanized_synced_lyrics,
            plain_lyrics: None,
        }
    }

    /// Removes enhanced-LRC word timestamps such as `<00:27.55>`.
    fn strip_word_tags(lrc: &str) -> String {
        let mut stripped = String::with_capacity(lrc.len());
        let mut rest = lrc;
        while let Some((start, end)) = super::next_word_tag(rest) {
            stripped.push_str(&rest[..start]);
            rest = &rest[end..];
        }
        stripped.push_str(rest);
        stripped
    }

    /// True when at least one `[mm:ss]` line carries text, so a metadata-only
    /// or empty file is not mistaken for lyrics.
    fn has_timed_lyrics(lrc: &str) -> bool {
        lrc.lines().any(|line| {
            line.strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
                .is_some_and(|(tag, text)| {
                    tag.starts_with(|char: char| char.is_ascii_digit())
                        && !strip_word_tags(text).trim().is_empty()
                })
        })
    }

    fn provider_error(provider: &str, error: impl std::fmt::Display) -> String {
        format!("{provider}: {error}")
    }

    #[derive(Deserialize)]
    struct LrcRedMatches {
        #[serde(default)]
        hits: Vec<LrcRedHit>,
    }

    #[derive(Deserialize)]
    struct LrcRedHit {
        isrc: String,
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        duration: Option<f64>,
    }

    /// lrc.red (formerly BiniLyrics): Apple Music lyrics, many with word timing.
    /// Players often join collaborators into one artist ("Gorillaz & Del the
    /// Funky Homosapien") that lrc.red cannot match, so a miss is retried with
    /// the primary artist alone.
    async fn fetch_lrc_red(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Lookup {
        let found = search_lrc_red(client, title, artist, duration_ms).await;
        if !matches!(found, Ok(None)) {
            return found;
        }
        match primary_artist(artist) {
            Some(primary) => search_lrc_red(client, title, primary, duration_ms).await,
            None => found,
        }
    }

    /// The first credited artist of a joined artist string, or `None` when it
    /// names a single artist. Separators need surrounding spaces, so names such
    /// as "Simon&Garfunkel" are left whole.
    fn primary_artist(artist: &str) -> Option<&str> {
        const SEPARATORS: [&str; 9] = [
            " & ",
            ", ",
            "; ",
            " feat. ",
            " feat ",
            " ft. ",
            " featuring ",
            " x ",
            " / ",
        ];
        let lower = artist.to_ascii_lowercase();
        let index = SEPARATORS
            .iter()
            .filter_map(|separator| lower.find(separator))
            .min()?;
        let primary = artist[..index].trim();
        (!primary.is_empty()).then_some(primary)
    }

    /// One `/match.json` query: the hits for a title and artist, with the
    /// duration weighed in when given.
    async fn lrc_red_hits(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_seconds: Option<f64>,
    ) -> Result<Vec<LrcRedHit>, String> {
        let mut url = format!(
            "https://lrc.red/match.json?title={}&artist={}",
            urlencoding::encode(title),
            urlencoding::encode(artist)
        );
        if let Some(duration_seconds) = duration_seconds {
            url.push_str(&format!("&duration={duration_seconds}"));
        }
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|error| provider_error("lrc.red match", error))?;
        if !response.status().is_success() {
            return Err(provider_error("lrc.red match", response.status()));
        }
        let matches = response
            .json::<LrcRedMatches>()
            .await
            .map_err(|error| provider_error("lrc.red match", error))?;
        Ok(matches.hits)
    }

    /// Hits of both queries without repeats, those of the title query first.
    fn merge_lrc_red_hits(by_title: Vec<LrcRedHit>, by_duration: Vec<LrcRedHit>) -> Vec<LrcRedHit> {
        let mut seen = HashSet::new();
        by_title
            .into_iter()
            .chain(by_duration)
            .filter(|hit| seen.insert(hit.isrc.clone()))
            .collect()
    }

    /// The recordings lrc.red lists for a song that plausibly are it, best
    /// first, each with its ISRC.
    ///
    /// `/match.json` weighs the duration above the title, so with one it lists
    /// other songs of about that length and can leave out the song itself. The
    /// query without a duration finds the song by its name, the one with a
    /// duration finds its variants of the right length; both are ranked here.
    async fn lrc_red_matches(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Result<Vec<(LrclibLyrics, String)>, String> {
        let hits = match duration_ms {
            None => lrc_red_hits(client, title, artist, None).await?,
            Some(duration_ms) => {
                let (by_title, by_duration) = tokio::join!(
                    lrc_red_hits(client, title, artist, None),
                    lrc_red_hits(
                        client,
                        title,
                        artist,
                        Some((duration_ms as f64 / 1_000.0).round())
                    )
                );
                match (by_title, by_duration) {
                    (Ok(by_title), Ok(by_duration)) => merge_lrc_red_hits(by_title, by_duration),
                    (Ok(hits), Err(_)) | (Err(_), Ok(hits)) => hits,
                    (Err(error), Err(_)) => return Err(error),
                }
            }
        };
        let candidates = hits
            .into_iter()
            .map(|hit| {
                let candidate = metadata_candidate(hit.title, hit.artist, hit.album, hit.duration);
                (candidate, hit.isrc)
            })
            .collect();
        Ok(rank_matches(candidates, title, artist, duration_ms))
    }

    /// `/match.json` finds the recording, `/s/{isrc}.lrc` is its enhanced LRC.
    async fn search_lrc_red(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Lookup {
        let started_at = std::time::Instant::now();
        // A hit can lack a lyrics file, so fall through to the next best one.
        let ranked = lrc_red_matches(client, title, artist, duration_ms)
            .await?
            .into_iter()
            .take(3)
            .collect();
        let found = first_found(ranked, |isrc: String| async move {
            fetch_lrc_red_lrc(client, &isrc).await
        })
        .await?;
        let Some((candidate, isrc, lrc)) = found else {
            println!(
                "[latency] lrc.red total={}ms no match",
                started_at.elapsed().as_millis()
            );
            return Ok(None);
        };
        println!(
            "[latency] lrc.red total={}ms isrc={isrc}",
            started_at.elapsed().as_millis()
        );
        Ok(Some(synced_result("lrc.red", candidate, lrc)))
    }

    /// The enhanced LRC for one recording, or `None` when it has no usable file.
    async fn fetch_lrc_red_lrc(
        client: &reqwest::Client,
        isrc: &str,
    ) -> Result<Option<String>, String> {
        let response = client
            .get(format!(
                "https://lrc.red/s/{}.lrc",
                urlencoding::encode(isrc)
            ))
            .send()
            .await
            .map_err(|error| provider_error("lrc.red lyrics", error))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(provider_error("lrc.red lyrics", response.status()));
        }
        let lrc = response
            .text()
            .await
            .map_err(|error| provider_error("lrc.red lyrics", error))?;
        Ok(has_timed_lyrics(&lrc).then_some(lrc))
    }

    /// The first sync of a recording runs lrc.red's alignment model, which takes
    /// seconds; later requests for it are answered from what it stored.
    const WORD_SYNC_TIMEOUT: Duration = Duration::from_secs(45);

    #[derive(Deserialize)]
    struct LrcRedSong {
        #[serde(default)]
        lyrics: LrcRedLyrics,
    }

    #[derive(Deserialize, Default)]
    struct LrcRedLyrics {
        #[serde(default)]
        lines: Vec<LrcRedLine>,
    }

    #[derive(Deserialize)]
    struct LrcRedLine {
        /// Words made of one or more timed syllables; empty for an untimed line.
        #[serde(default)]
        words: Vec<Vec<LrcRedWord>>,
    }

    #[derive(Deserialize)]
    struct LrcRedWord {
        text: String,
        /// Seconds.
        begin: f64,
        end: f64,
    }

    /// `seconds` as `mm:ss.xx`, rounded to centiseconds in one step the way
    /// lrc.red's own LRC files are: a begin of 37.175 s is `00:37.17` there, which
    /// rounding to milliseconds first would turn into `00:37.18`.
    fn lrc_timestamp_centis(seconds: f64) -> String {
        let centiseconds = (seconds * 100.0).round().max(0.0) as u64;
        format!(
            "{:02}:{:02}.{:02}",
            centiseconds / 6_000,
            centiseconds / 100 % 60,
            centiseconds % 100
        )
    }

    /// Writes a synced song the way `/s/{isrc}.lrc` does: one tag per word, its
    /// syllables joined, and a closing tag after the last word of the line.
    fn lrc_red_song_to_enhanced_lrc(song: &LrcRedSong) -> String {
        song.lyrics
            .lines
            .iter()
            .filter_map(|line| {
                let words = line
                    .words
                    .iter()
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>();
                let first = words.first()?.first()?;
                let last = words.last()?.last()?;
                let mut lrc = format!("[{}]", lrc_timestamp_centis(first.begin));
                for word in words {
                    let text = word
                        .iter()
                        .map(|syllable| syllable.text.as_str())
                        .collect::<String>();
                    lrc.push_str(&format!("<{}>{text} ", lrc_timestamp_centis(word[0].begin)));
                }
                lrc.push_str(&format!("<{}>", lrc_timestamp_centis(last.end)));
                Some(lrc)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Asks lrc.red to time every word of one recording and returns the result
    /// as enhanced LRC. `None` means lrc.red cannot, which will not change on
    /// retrying; an error is a failure worth retrying later.
    async fn sync_lrc_red_words(
        client: &reqwest::Client,
        isrc: &str,
    ) -> Result<Option<String>, String> {
        let response = client
            .post(format!(
                "https://lrc.red/s/{}/sync",
                urlencoding::encode(isrc)
            ))
            .timeout(WORD_SYNC_TIMEOUT)
            .send()
            .await
            .map_err(|error| provider_error("lrc.red sync", error))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(provider_error("lrc.red sync", status));
        }
        if status.is_client_error() {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(provider_error("lrc.red sync", status));
        }
        let song = response
            .json::<LrcRedSong>()
            .await
            .map_err(|error| provider_error("lrc.red sync", error))?;
        let lrc = lrc_red_song_to_enhanced_lrc(&song);
        Ok(super::has_word_timing(&lrc).then_some(lrc))
    }

    /// Word-timed lyrics for a song, from lrc.red's alignment model. The song is
    /// found the same way as for a lookup, so a different recording of it is
    /// never timed by mistake.
    pub async fn sync_words(title: &str, artist: &str, duration_ms: Option<u64>) -> Lookup {
        let started_at = std::time::Instant::now();
        let client = http_client()?;
        let mut matches = lrc_red_matches(client, title, artist, duration_ms).await?;
        if matches.is_empty()
            && let Some(primary) = primary_artist(artist)
        {
            matches = lrc_red_matches(client, title, primary, duration_ms).await?;
        }
        // A recording lrc.red cannot time falls through to the next best one.
        let ranked = matches.into_iter().take(3).collect();
        let found = first_found(ranked, |isrc: String| async move {
            sync_lrc_red_words(client, &isrc).await
        })
        .await?;
        let Some((candidate, isrc, lrc)) = found else {
            println!(
                "[latency] lrc.red sync total={}ms not timed",
                started_at.elapsed().as_millis()
            );
            return Ok(None);
        };
        println!(
            "[latency] lrc.red sync total={}ms isrc={isrc}",
            started_at.elapsed().as_millis()
        );
        Ok(Some(synced_result("lrc.red", candidate, lrc)))
    }

    #[derive(Deserialize)]
    struct NeteaseSearch {
        result: Option<NeteaseSongs>,
    }

    #[derive(Deserialize)]
    struct NeteaseSongs {
        #[serde(default)]
        songs: Vec<NeteaseSong>,
    }

    #[derive(Deserialize)]
    struct NeteaseSong {
        id: u64,
        name: Option<String>,
        /// Milliseconds.
        duration: Option<f64>,
        #[serde(default)]
        artists: Vec<NeteaseName>,
        album: Option<NeteaseName>,
    }

    #[derive(Deserialize)]
    struct NeteaseName {
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct NeteaseLyrics {
        lrc: Option<NeteaseLrc>,
        /// Word-timed lyrics, present for some songs.
        yrc: Option<NeteaseLrc>,
    }

    #[derive(Deserialize)]
    struct NeteaseLrc {
        lyric: Option<String>,
    }

    const NETEASE_REFERER: &str = "https://music.163.com/";

    /// What Netease has for one song.
    enum NeteaseFound {
        Lyrics(String),
        Instrumental,
    }

    /// Netease Cloud Music's unofficial web API: line-synced LRC, strong on
    /// Asian catalogues. Unauthenticated, so it can change without notice.
    async fn fetch_netease(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Lookup {
        let started_at = std::time::Instant::now();
        // Searched with POST: the GET `/search/get/web` form now answers with
        // an encrypted blob instead of JSON.
        let response = client
            .post("https://music.163.com/api/search/get")
            .header(reqwest::header::REFERER, NETEASE_REFERER)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(format!(
                "s={}&type=1&limit=5&offset=0",
                urlencoding::encode(&format!("{title} {artist}"))
            ))
            .send()
            .await
            .map_err(|error| provider_error("Netease search", error))?;
        if !response.status().is_success() {
            return Err(provider_error("Netease search", response.status()));
        }
        let search = response
            .json::<NeteaseSearch>()
            .await
            .map_err(|error| provider_error("Netease search", error))?;
        let candidates = search
            .result
            .map(|result| result.songs)
            .unwrap_or_default()
            .into_iter()
            .map(|song| {
                let artists = song
                    .artists
                    .into_iter()
                    .filter_map(|artist| artist.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                let candidate = metadata_candidate(
                    song.name,
                    Some(artists),
                    song.album.and_then(|album| album.name),
                    song.duration.map(|milliseconds| milliseconds / 1_000.0),
                );
                (candidate, song.id)
            })
            .collect();

        let normalized_artist = normalize(artist);
        let normalized_title = canonical_title(title, &normalized_artist);
        let ranked = rank_matches(candidates, title, artist, duration_ms)
            .into_iter()
            .take(2)
            .map(|(candidate, id)| {
                // Only a song titled exactly like the playing one may be called
                // instrumental; a search can also return an "(Instrumental)" cut
                // of a song that has vocals, or another track from an album named
                // like the playing song, so the album does not count here.
                let exact_title =
                    track_title_score(&candidate, &normalized_title, &normalized_artist) == 4;
                (candidate, (id, exact_title))
            })
            .collect();
        let found = first_found(ranked, |(id, exact_title)| async move {
            fetch_netease_lyrics(client, id, exact_title).await
        })
        .await?;
        let Some((candidate, (id, _), found)) = found else {
            println!(
                "[latency] Netease total={}ms no match",
                started_at.elapsed().as_millis()
            );
            return Ok(None);
        };
        println!(
            "[latency] Netease total={}ms id={id}",
            started_at.elapsed().as_millis()
        );
        Ok(Some(match found {
            NeteaseFound::Lyrics(lrc) => synced_result("Netease", candidate, lrc),
            NeteaseFound::Instrumental => instrumental_result("Netease", candidate),
        }))
    }

    /// The timed lyrics of one song (word-timed when Netease has them), or
    /// `Instrumental` when the song is marked as such and `instrumental_allowed`.
    async fn fetch_netease_lyrics(
        client: &reqwest::Client,
        id: u64,
        instrumental_allowed: bool,
    ) -> Result<Option<NeteaseFound>, String> {
        let response = client
            .get(format!(
                "https://music.163.com/api/song/lyric?id={id}&lv=1&yv=1&tv=-1"
            ))
            .header(reqwest::header::REFERER, NETEASE_REFERER)
            .send()
            .await
            .map_err(|error| provider_error("Netease lyrics", error))?;
        if !response.status().is_success() {
            return Err(provider_error("Netease lyrics", response.status()));
        }
        let lyrics = response
            .json::<NeteaseLyrics>()
            .await
            .map_err(|error| provider_error("Netease lyrics", error))?;
        let line_synced = lyrics.lrc.and_then(|lrc| lrc.lyric);
        let word_timed = lyrics
            .yrc
            .and_then(|yrc| yrc.lyric)
            .map(|yrc| clean_netease_lrc(&yrc_to_enhanced_lrc(&yrc)));
        let line_timed = line_synced.as_deref().map(clean_netease_lrc);
        let lrc = [word_timed, line_timed]
            .into_iter()
            .flatten()
            .find(|lrc| has_timed_lyrics(lrc));
        Ok(match lrc {
            Some(lrc) => Some(NeteaseFound::Lyrics(lrc)),
            None if instrumental_allowed
                && line_synced.as_deref().is_some_and(is_netease_instrumental) =>
            {
                Some(NeteaseFound::Instrumental)
            }
            None => None,
        })
    }

    /// The text of an LRC line without its timestamps and word tags.
    fn lrc_line_text(line: &str) -> String {
        strip_word_tags(line.rfind(']').map_or(line, |index| &line[index + 1..]))
    }

    /// Netease marks an instrumental with a lyric line, `纯音乐，请欣赏`.
    fn is_netease_instrumental(lrc: &str) -> bool {
        lrc.lines()
            .any(|line| lrc_line_text(line).trim_start().starts_with("纯音乐"))
    }

    /// Netease puts credits (`作词 : …`) at 00:00 and marks instrumentals with a
    /// lyric line (`纯音乐，请欣赏`); neither belongs on screen. The marker is read
    /// by `is_netease_instrumental` before it is dropped here.
    fn clean_netease_lrc(lrc: &str) -> String {
        const CREDITS: [&str; 6] = ["作词", "作詞", "作曲", "编曲", "編曲", "制作人"];
        lrc.lines()
            .filter(|line| {
                let text = lrc_line_text(line);
                let text = text.trim_start();
                let is_credit = CREDITS.iter().any(|credit| {
                    text.strip_prefix(credit)
                        .is_some_and(|rest| rest.trim_start().starts_with([':', '：']))
                });
                !is_credit && !text.starts_with("纯音乐")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `(start, duration)` of a Netease word header such as `(1000,300,0)` at the
    /// start of `text`, plus the header's length in bytes.
    fn yrc_header(text: &str) -> Option<(u64, u64, usize)> {
        let length = text.strip_prefix('(')?.find(')')?;
        let mut numbers = text[1..=length]
            .split(',')
            .map(|number| number.parse::<u64>());
        let (start, duration, _) = (
            numbers.next()?.ok()?,
            numbers.next()?.ok()?,
            numbers.next()?.ok()?,
        );
        numbers
            .next()
            .is_none()
            .then_some((start, duration, length + 2))
    }

    fn lrc_timestamp(milliseconds: u64) -> String {
        format!(
            "{:02}:{:02}.{:03}",
            milliseconds / 60_000,
            milliseconds / 1_000 % 60,
            milliseconds % 1_000
        )
    }

    /// Converts Netease's word-timed `yrc` (`[start,duration](start,duration,0)word…`)
    /// into enhanced LRC. Credit lines are JSON objects in `yrc` and are skipped.
    fn yrc_to_enhanced_lrc(yrc: &str) -> String {
        yrc.lines()
            .filter_map(|line| {
                let (header, body) = line.strip_prefix('[')?.split_once(']')?;
                let line_start = header.split_once(',')?.0.parse::<u64>().ok()?;
                let headers = body
                    .match_indices('(')
                    .filter_map(|(position, _)| {
                        yrc_header(&body[position..]).map(|header| (position, header))
                    })
                    .collect::<Vec<_>>();
                let words = headers
                    .iter()
                    .enumerate()
                    .map(|(index, (position, (start, duration, length)))| {
                        let text_end = headers.get(index + 1).map_or(body.len(), |next| next.0);
                        (*start, *duration, &body[position + length..text_end])
                    })
                    .collect::<Vec<_>>();
                if words.iter().all(|(_, _, text)| text.trim().is_empty()) {
                    return None;
                }

                let mut lrc = format!("[{}]", lrc_timestamp(line_start));
                for (index, (start, duration, text)) in words.iter().enumerate() {
                    lrc.push_str(&format!("<{}>{text}", lrc_timestamp(*start)));
                    // Close a word that is followed by a gap, and the last one.
                    let end = start + duration;
                    if words.get(index + 1).is_none_or(|next| next.0 > end) {
                        lrc.push_str(&format!("<{}>", lrc_timestamp(end)));
                    }
                }
                Some(lrc)
            })
            .collect::<Vec<_>>()
            .join("\n")
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

        fn lookup(source: &str, synced: Option<&str>, plain: Option<&str>) -> Lookup {
            Ok(Some(LyricsResult {
                source: source.to_string(),
                track_name: String::new(),
                artist_name: String::new(),
                album_name: String::new(),
                duration: None,
                instrumental: false,
                synced_lyrics: synced.map(str::to_string),
                romanized_synced_lyrics: None,
                plain_lyrics: plain.map(str::to_string),
            }))
        }

        fn source_of(answer: Option<Lookup>) -> Option<String> {
            answer?.ok()?.map(|result| result.source)
        }

        fn hit(title: &str, artist: &str, duration: f64) -> LrclibLyrics {
            metadata_candidate(
                Some(title.to_string()),
                Some(artist.to_string()),
                None,
                Some(duration),
            )
        }

        #[test]
        fn waits_for_the_highest_priority_provider_before_using_a_lower_one() {
            let slots = [
                None,
                Some(lookup("LRCLIB", Some("[00:01.00]a"), None)),
                Some(lookup("Netease", Some("[00:01.00]a"), None)),
            ];
            assert!(resolve(&slots).is_none());
        }

        #[test]
        fn highest_priority_synced_result_wins() {
            let slots = [
                Some(lookup("lrc.red", Some("[00:01.00]a"), None)),
                Some(lookup("LRCLIB", Some("[00:01.00]a"), None)),
                Some(Ok(None)),
            ];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("lrc.red"));
        }

        #[test]
        fn falls_through_empty_and_failed_providers_to_synced_lyrics() {
            let slots = [
                Some(Err("lrc.red match: timed out".to_string())),
                Some(Ok(None)),
                Some(lookup("Netease", Some("[00:01.00]a"), None)),
            ];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("Netease"));
        }

        #[test]
        fn plain_lyrics_are_used_only_when_no_provider_has_synced_lyrics() {
            let plain_only = [
                Some(Ok(None)),
                Some(lookup("LRCLIB", None, Some("words"))),
                Some(Ok(None)),
            ];
            assert_eq!(source_of(resolve(&plain_only)).as_deref(), Some("LRCLIB"));

            let later_synced = [
                Some(Ok(None)),
                Some(lookup("LRCLIB", None, Some("words"))),
                Some(lookup("Netease", Some("[00:01.00]a"), None)),
            ];
            assert_eq!(
                source_of(resolve(&later_synced)).as_deref(),
                Some("Netease")
            );
        }

        #[test]
        fn an_error_surfaces_only_when_nothing_was_found() {
            let failed = [
                Some(Err("lrc.red match: timed out".to_string())),
                Some(Ok(None)),
                Some(Ok(None)),
            ];
            assert!(matches!(resolve(&failed), Some(Err(_))));

            let missing = [Some(Ok(None)), Some(Ok(None)), Some(Ok(None))];
            assert!(matches!(resolve(&missing), Some(Ok(None))));
        }

        #[test]
        fn rank_matches_rejects_other_lengths_and_unrelated_songs() {
            let hits = vec![
                (hit("Blinding Lights", "The Weeknd", 200.0), "original"),
                (
                    hit("Blinding Lights (Remix)", "The Weeknd", 216.0),
                    "long remix",
                ),
                (hit("Other Song", "Other Artist", 200.0), "unrelated"),
            ];

            let ranked = rank_matches(hits, "Blinding Lights", "The Weeknd", Some(200_000));

            assert_eq!(ranked.len(), 1);
            assert_eq!(ranked[0].1, "original");
        }

        #[test]
        fn rank_matches_prefers_the_exact_title_over_a_same_length_variant() {
            let hits = vec![
                (hit("Blinding Lights (Remix)", "The Weeknd", 201.0), "remix"),
                (hit("Blinding Lights", "The Weeknd", 202.0), "original"),
            ];

            let ranked = rank_matches(hits, "Blinding Lights", "The Weeknd", Some(200_000));

            assert_eq!(ranked[0].1, "original");
        }

        /// The start of what `POST /s/AUDJ02102297/sync` answered, and the lines
        /// `/s/AUDJ02102297.lrc` then served for it.
        const SYNCED_SONG: &str = r#"{"id":"AUDJ02102297","lyrics":{"lines":[
            {"words":[[{"text":"Some","begin":15.816,"end":16.345},{"text":"one","begin":16.345,"end":16.776}],
                [{"text":"said","begin":16.776,"end":17.296}],
                [{"text":"they","begin":17.296,"end":17.641}],
                [{"text":"left","begin":17.641,"end":18.063}],
                [{"text":"to","begin":18.063,"end":18.38},{"text":"geth","begin":18.38,"end":18.936},{"text":"er","begin":18.936,"end":19.662}]],
             "text":"Someone said they left together","timed":true},
            {"words":[[{"text":"I","begin":19.675,"end":20.129}],
                [{"text":"ran","begin":20.129,"end":20.599}],
                [{"text":"her","begin":22.695,"end":22.715}]],
             "text":"I ran her","timed":true},
            {"words":[],"text":"An untimed line","timed":false}]}}"#;

        fn lrc_red_hit(isrc: &str, title: &str) -> LrcRedHit {
            LrcRedHit {
                isrc: isrc.to_string(),
                title: Some(title.to_string()),
                artist: None,
                album: None,
                duration: None,
            }
        }

        #[test]
        fn lrc_red_hits_of_both_queries_are_merged_without_repeats() {
            let merged = merge_lrc_red_hits(
                vec![lrc_red_hit("A", "Song"), lrc_red_hit("B", "Song (Live)")],
                vec![lrc_red_hit("C", "Other"), lrc_red_hit("A", "Song")],
            );

            let isrcs = merged
                .iter()
                .map(|hit| hit.isrc.as_str())
                .collect::<Vec<_>>();
            assert_eq!(isrcs, ["A", "B", "C"]);
        }

        #[test]
        fn a_synced_song_becomes_the_enhanced_lrc_lrc_red_serves() {
            let song = serde_json::from_str::<LrcRedSong>(SYNCED_SONG).unwrap();

            assert_eq!(
                lrc_red_song_to_enhanced_lrc(&song),
                "[00:15.82]<00:15.82>Someone <00:16.78>said <00:17.30>they <00:17.64>left <00:18.06>together <00:19.66>\n\
                 [00:19.68]<00:19.68>I <00:20.13>ran <00:22.70>her <00:22.72>"
            );
        }

        #[test]
        fn a_synced_song_with_word_timing_is_recognised_and_one_without_is_not() {
            let timed = serde_json::from_str::<LrcRedSong>(SYNCED_SONG).unwrap();
            assert!(crate::has_word_timing(&lrc_red_song_to_enhanced_lrc(
                &timed
            )));

            let untimed =
                serde_json::from_str::<LrcRedSong>(r#"{"lyrics":{"lines":[{"words":[]}]}}"#)
                    .unwrap();
            assert!(!crate::has_word_timing(&lrc_red_song_to_enhanced_lrc(
                &untimed
            )));

            let empty = serde_json::from_str::<LrcRedSong>("{}").unwrap();
            assert_eq!(lrc_red_song_to_enhanced_lrc(&empty), "");
        }

        #[test]
        fn lrc_timestamps_round_to_the_nearest_centisecond() {
            assert_eq!(lrc_timestamp_centis(0.0), "00:00.00");
            assert_eq!(lrc_timestamp_centis(15.816), "00:15.82");
            assert_eq!(lrc_timestamp_centis(19.675), "00:19.68");
            // Begins lrc.red's own LRC files round down.
            assert_eq!(lrc_timestamp_centis(37.175), "00:37.17");
            assert_eq!(lrc_timestamp_centis(38.495), "00:38.49");
            assert_eq!(lrc_timestamp_centis(66.195), "01:06.19");
            assert_eq!(lrc_timestamp_centis(0.0149), "00:00.01");
            assert_eq!(lrc_timestamp_centis(60.476), "01:00.48");
            assert_eq!(lrc_timestamp_centis(3_599.999), "60:00.00");
            assert_eq!(lrc_timestamp_centis(-1.0), "00:00.00");
        }

        #[test]
        fn rank_matches_does_not_confuse_a_song_with_others_on_the_album_named_after_it() {
            let on_album = |title: &str, duration: f64| {
                candidate_with_metadata(title, "AC/DC", Some("Back In Black"), duration, true)
            };
            let hits = vec![
                (
                    on_album("Rock and Roll Ain't Noise Pollution", 255.648),
                    "other track",
                ),
                (on_album("Back In Black", 256.0), "the song"),
            ];

            let ranked = rank_matches(hits, "Back In Black", "AC/DC", Some(255_000));

            assert_eq!(ranked.len(), 1);
            assert_eq!(ranked[0].1, "the song");
        }

        #[test]
        fn rank_matches_rejects_another_song_by_the_same_artist() {
            let hits = vec![(
                hit("Dirty Deeds Done Dirt Cheap", "AC/DC", 253.0),
                "other song",
            )];

            assert!(rank_matches(hits, "Back In Black", "AC/DC", Some(253_000)).is_empty());
        }

        #[test]
        fn rank_matches_rejects_a_hit_without_a_title_that_only_shares_the_artist() {
            let untitled = metadata_candidate(None, Some("AC/DC".to_string()), None, Some(256.0));

            assert!(
                rank_matches(
                    vec![(untitled, "untitled")],
                    "Back In Black",
                    "AC/DC",
                    Some(256_000)
                )
                .is_empty()
            );
        }

        #[test]
        fn rank_matches_accepts_the_artist_alone_when_the_titles_use_different_scripts() {
            let hits = vec![(hit("夜曲", "周杰伦", 226.0), "hit")];

            assert_eq!(
                rank_matches(hits, "Ye Qu", "周杰伦", Some(226_000)).len(),
                1
            );
        }

        #[test]
        fn rank_matches_accepts_a_hit_when_only_the_artist_script_differs() {
            let hits = vec![(hit("夜曲", "周杰伦", 226.0), "hit")];

            assert_eq!(
                rank_matches(hits, "夜曲", "Jay Chou", Some(226_000)).len(),
                1
            );
        }

        #[test]
        fn word_tags_are_removed_and_other_angle_brackets_are_kept() {
            assert_eq!(
                strip_word_tags("[00:27.40]<00:27.40>I <00:27.55>been <00:28.96>"),
                "[00:27.40]I been "
            );
            assert_eq!(
                strip_word_tags("[00:01.00]a < b > c <3"),
                "[00:01.00]a < b > c <3"
            );
        }

        #[test]
        fn timed_lyrics_need_a_timestamp_and_text() {
            assert!(has_timed_lyrics(
                "[ti:Song]\n[00:01.00]<00:01.00>Hello <00:02.00>"
            ));
            assert!(!has_timed_lyrics("[ti:Song]\n[ar:Artist]"));
            assert!(!has_timed_lyrics("[00:01.00]<00:01.00>"));
            assert!(!has_timed_lyrics(""));
        }

        #[test]
        fn netease_credits_and_instrumental_markers_are_dropped() {
            let cleaned = clean_netease_lrc(
                "[00:00.00] 作词 : 黄家驹\n[00:01.00] 作曲 : 黄家驹\n[00:18.85]今天我 寒夜里看雪飘过\n[00:20.00]作曲家的梦",
            );
            assert_eq!(
                cleaned,
                "[00:18.85]今天我 寒夜里看雪飘过\n[00:20.00]作曲家的梦"
            );
            assert!(!has_timed_lyrics(&clean_netease_lrc(
                "[00:00.00]纯音乐，请欣赏"
            )));
        }

        fn run<T>(future: impl Future<Output = T>) -> T {
            tauri::async_runtime::block_on(future)
        }

        #[test]
        fn a_provider_that_stalls_ends_in_a_timeout_error_not_a_miss() {
            let stalled = within_deadline("lrc.red", Duration::from_millis(30), async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(None)
            });
            assert_eq!(run(stalled).err().as_deref(), Some("lrc.red: timed out"));

            let quick = within_deadline("lrc.red", Duration::from_secs(5), async { Ok(None) });
            assert!(matches!(run(quick), Ok(None)));
        }

        #[test]
        fn the_deadline_covers_every_request_a_provider_makes() {
            // Three sequential 20 ms "requests" cannot fit in 30 ms in total.
            let slow_candidates = within_deadline("Netease", Duration::from_millis(30), async {
                for _ in 0..3 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                lookup("Netease", Some(WORD), None)
            });
            assert!(run(slow_candidates).is_err());
        }

        fn ranked(keys: &[u32]) -> Vec<(LrclibLyrics, u32)> {
            keys.iter()
                .map(|key| (hit("Song", "Artist", 200.0), *key))
                .collect()
        }

        #[test]
        fn a_failing_candidate_does_not_stop_the_next_one() {
            let found = run(first_found(ranked(&[1, 2, 3]), |key| async move {
                match key {
                    1 => Err("lrc.red lyrics: 500 Internal Server Error".to_string()),
                    2 => Ok(Some("lyrics")),
                    _ => panic!("candidate {key} must not be tried after a hit"),
                }
            }));
            assert_eq!(
                found.unwrap().map(|(_, key, lyrics)| (key, lyrics)),
                Some((2, "lyrics"))
            );
        }

        #[test]
        fn a_candidate_error_surfaces_only_when_no_candidate_has_lyrics() {
            let failed = run(first_found(ranked(&[1, 2]), |key| async move {
                if key == 1 {
                    Err::<Option<&str>, _>("500".to_string())
                } else {
                    Ok(None)
                }
            }));
            assert_eq!(failed.map(|found| found.is_some()), Err("500".to_string()));

            let missing = run(first_found(ranked(&[1, 2]), |_| async {
                Ok::<Option<&str>, String>(None)
            }));
            assert_eq!(missing.map(|found| found.is_some()), Ok(false));
        }

        #[test]
        fn the_netease_instrumental_marker_is_recognised_before_it_is_cleaned_away() {
            let raw = "[00:00.00]纯音乐，请欣赏";
            assert!(is_netease_instrumental(raw));
            assert!(!has_timed_lyrics(&clean_netease_lrc(raw)));
            assert!(is_netease_instrumental(
                "[00:00.000]<00:00.000>纯音乐，请欣赏<00:05.000>"
            ));
            assert!(!is_netease_instrumental("[00:01.00]今天我 寒夜里看雪飘过"));
        }

        #[test]
        fn only_the_track_name_makes_a_title_exact_for_instrumentals() {
            let artist = normalize("Artist");
            let title = canonical_title("Song A", &artist);
            let on_title_album = |track: &str| {
                metadata_candidate(
                    Some(track.to_string()),
                    Some("Artist".to_string()),
                    Some("Song A".to_string()),
                    Some(200.0),
                )
            };

            // The title track itself is exact.
            assert_eq!(
                track_title_score(&on_title_album("Song A"), &title, &artist),
                4
            );
            // Another track on an album called "Song A" is not, and the album
            // does not vouch for it in `metadata_scores` either.
            let other_track = on_title_album("Song B");
            assert_eq!(track_title_score(&other_track, &title, &artist), 0);
            assert_eq!(metadata_scores(&other_track, &title, &artist).0, 0);
            // A variant is not exact either.
            let variant = on_title_album("Song A (Instrumental)");
            assert!(track_title_score(&variant, &title, &artist) < 4);
        }

        #[test]
        fn an_instrumental_result_carries_the_flag_and_no_lyrics() {
            let result = instrumental_result("Netease", hit("Song", "Artist", 200.0));
            assert!(result.instrumental);
            assert_eq!(result.source, "Netease");
            assert!(result.synced_lyrics.is_none() && result.plain_lyrics.is_none());
            // The resolver treats it as a final answer, like an LRCLIB instrumental.
            let slots = [Some(Ok(None)), Some(Ok(None)), Some(Ok(Some(result)))];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("Netease"));
        }

        const WORD: &str = "[00:01.00]<00:01.00>a <00:02.00>b<00:03.00>";

        #[test]
        fn word_timed_lyrics_win_over_line_synced_ones_from_any_provider() {
            let slots = [
                Some(lookup("lrc.red", Some("[00:01.00]a"), None)),
                Some(lookup("LRCLIB", Some("[00:01.00]a"), None)),
                Some(lookup("Netease", Some(WORD), None)),
            ];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("Netease"));
        }

        #[test]
        fn whole_line_timing_from_lrc_red_does_not_beat_word_timed_netease() {
            let whole_line = "[00:01.00]<00:01.00>a whole line <00:03.00>";
            let slots = [
                Some(lookup("lrc.red", Some(whole_line), None)),
                Some(Ok(None)),
                Some(lookup("Netease", Some(WORD), None)),
            ];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("Netease"));
        }

        #[test]
        fn a_line_only_answer_waits_for_a_provider_that_may_time_words() {
            let pending_netease = [
                Some(lookup("lrc.red", Some("[00:01.00]a"), None)),
                Some(lookup("LRCLIB", Some("[00:01.00]a"), None)),
                None,
            ];
            assert!(resolve(&pending_netease).is_none());
        }

        #[test]
        fn lrclib_never_delays_a_word_timed_answer() {
            let slots = [
                Some(Ok(None)),
                None,
                Some(lookup("Netease", Some(WORD), None)),
            ];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("Netease"));
        }

        #[test]
        fn word_timed_lrc_red_answers_without_waiting_for_the_others() {
            let slots = [Some(lookup("lrc.red", Some(WORD), None)), None, None];
            assert_eq!(source_of(resolve(&slots)).as_deref(), Some("lrc.red"));
        }

        #[test]
        fn a_pending_lrc_red_holds_back_a_word_timed_netease() {
            let slots = [
                None,
                Some(Ok(None)),
                Some(lookup("Netease", Some(WORD), None)),
            ];
            assert!(resolve(&slots).is_none());
        }

        #[test]
        fn whole_line_timing_is_not_word_timing() {
            let super_has = super::super::has_word_timing;
            assert!(super_has(WORD));
            assert!(super_has("[00:01.00]<00:01.00>a<00:01.50>b<00:02.00>"));
            assert!(!super_has(
                "[00:01.85]<00:01.85>素晴らしき世界に今日も乾杯 <00:04.64>"
            ));
            assert!(!super_has("[00:01.00]<00:01.00>Hello <00:02.00>"));
            assert!(!super_has("[00:01.00]plain line"));
        }

        #[test]
        fn yrc_becomes_enhanced_lrc_with_closing_tags_only_where_needed() {
            let yrc = "{\"t\":0,\"c\":[{\"tx\":\"作词: x\"}]}\n\
                [1000,1500](1000,300,0)I (1300,200,0)been (1700,400,0)(Oh)\n\
                [4000,500](4000,500,0)  ";
            assert_eq!(
                yrc_to_enhanced_lrc(yrc),
                "[00:01.000]<00:01.000>I <00:01.300>been <00:01.500><00:01.700>(Oh)<00:02.100>"
            );
            assert_eq!(
                yrc_to_enhanced_lrc("[0,600](0,300,0)a (300,300,0)b"),
                "[00:00.000]<00:00.000>a <00:00.300>b<00:00.600>"
            );
        }

        #[test]
        fn converted_yrc_parses_as_timed_lyrics() {
            let lrc = yrc_to_enhanced_lrc("[1000,900](1000,300,0)a (1500,400,0)b");
            assert!(has_timed_lyrics(&lrc));
            assert!(super::super::next_word_tag(&lrc).is_some());
            assert_eq!(strip_word_tags(&lrc), "[00:01.000]a b");
        }

        #[test]
        fn tagged_credit_lines_are_dropped_too() {
            let cleaned = clean_netease_lrc(
                "[00:00.000]<00:00.000>作词 : <00:01.000>x<00:02.000>\n[00:03.000]<00:03.000>hi <00:04.000>there<00:05.000>",
            );
            assert_eq!(
                cleaned,
                "[00:03.000]<00:03.000>hi <00:04.000>there<00:05.000>"
            );
        }

        #[test]
        fn romanized_lyrics_keep_the_word_timing() {
            let lrc =
                "[00:01.00]<00:01.00>今日は<00:02.00>\n[00:03.00]<00:03.00>ありがとう<00:04.00>";

            let result = synced_result(
                "lrc.red",
                metadata_candidate(None, None, None, None),
                lrc.to_string(),
            );

            assert_eq!(result.synced_lyrics.as_deref(), Some(lrc));
            let romanized = result
                .romanized_synced_lyrics
                .expect("japanese is romanized");
            assert_eq!(strip_word_tags(&romanized).lines().count(), 2);
            assert_eq!(
                romanized.matches('<').count(),
                lrc.matches('<').count(),
                "every word tag survives: {romanized}"
            );
            assert!(!strip_word_tags(&romanized).contains(['今', 'あ']));
            assert_eq!(result.source, "lrc.red");
        }

        fn report_ops_per_sec(name: &str, mut work: impl FnMut()) {
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

        #[test]
        #[ignore = "curriculum bench; run via bun run bench:rust"]
        fn curriculum_metric_lrclib_rank_candidates() {
            let normalized_artist = normalize("Temper City");
            let normalized_title = canonical_title("Self Aware", &normalized_artist);
            let expected_duration_ms = Some(181_000);
            let mut candidates = Vec::with_capacity(64);
            for index in 0..60 {
                candidates.push(candidate(
                    &format!("Different Song {index}"),
                    &format!("Different Artist {index}"),
                    181.0 + (index as f64) * 0.01,
                ));
            }
            candidates.push(candidate_with_metadata(
                "Temper City - Self Aware",
                "DanceHype",
                Some("Self Aware Temper City"),
                181.0,
                true,
            ));
            candidates.push(candidate_with_metadata(
                "Self Aware",
                "Temper City",
                Some("Self Aware"),
                181.0,
                false,
            ));

            report_ops_per_sec("lrclib_rank_candidates", || {
                let mut ranked = candidates.clone();
                ranked.sort_by_key(|item| {
                    ranking_key(
                        item,
                        &normalized_title,
                        &normalized_artist,
                        expected_duration_ms,
                    )
                });
                assert_eq!(ranked[0].artist_name.as_deref(), Some("DanceHype"));
            });
        }

        #[test]
        fn strips_featured_artist_credits_from_titles() {
            for title in [
                "Love Me Not (feat. Rex Orange County)",
                "Love Me Not [ft. Rex Orange County]",
                "Love Me Not (with Rex Orange County)",
                "Love Me Not feat. Rex Orange County",
                "Love Me Not featuring Rex Orange County",
                "Love Me Not",
            ] {
                assert_eq!(strip_feature_credits(title), "Love Me Not", "{title}");
            }
            assert_eq!(
                strip_feature_credits("Song (Official Video)"),
                "Song (Official Video)"
            );
            assert_eq!(strip_feature_credits("(feat. X)"), "(feat. X)");
        }

        #[test]
        fn base_title_synced_lyrics_outrank_feature_title_plain_lyrics() {
            let normalized_artist = normalize("Ravyn Lenae");
            let normalized_title = canonical_title(
                &strip_feature_credits("Love Me Not (feat. Rex Orange County)"),
                &normalized_artist,
            );
            let mut feat_plain = candidate_with_metadata(
                "Love Me Not (feat. Rex Orange County)",
                "Ravyn Lenae",
                None,
                213.5,
                false,
            );
            feat_plain.synced_lyrics = None;
            let mut results = [feat_plain, candidate("Love Me Not", "Ravyn Lenae", 213.0)];

            results.sort_by_key(|item| {
                ranking_key(item, &normalized_title, &normalized_artist, Some(213_500))
            });

            assert_eq!(results[0].track_name.as_deref(), Some("Love Me Not"));
        }

        #[test]
        fn primary_artist_is_the_first_credited_artist() {
            assert_eq!(
                primary_artist("Gorillaz & Del the Funky Homosapien"),
                Some("Gorillaz")
            );
            assert_eq!(primary_artist("Drake, Future"), Some("Drake"));
            assert_eq!(primary_artist("Ravyn Lenae feat. Rex"), Some("Ravyn Lenae"));
            assert_eq!(primary_artist("Gorillaz"), None);
            assert_eq!(primary_artist("Simon&Garfunkel"), None);
        }
    }
}
