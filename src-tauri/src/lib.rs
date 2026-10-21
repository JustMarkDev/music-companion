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
#[cfg(target_os = "windows")]
#[path = "media_windows.rs"]
mod media;
#[cfg(target_os = "macos")]
#[path = "z_order_macos.rs"]
mod overlay_z_order;
#[cfg(target_os = "windows")]
#[path = "z_order_windows.rs"]
mod overlay_z_order;
#[cfg(target_os = "macos")]
#[path = "backdrop_macos.rs"]
mod persistent_backdrop;
#[cfg(target_os = "windows")]
#[path = "backdrop_windows.rs"]
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

/// When the media read in flight started. A read that never returns (a hung helper
/// process, a WinRT call that never completes) is given up on after
/// `MEDIA_QUERY_STUCK_AFTER`, or every later poll would get the cached state forever.
static MEDIA_QUERY_STARTED: Mutex<Option<Instant>> = Mutex::new(None);
const MEDIA_QUERY_STUCK_AFTER: Duration = Duration::from_secs(10);
static LATEST_LYRICS_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[tauri::command]
fn cancel_lyrics_requests(request_id: u64) {
    LATEST_LYRICS_REQUEST.fetch_max(request_id, Ordering::AcqRel);
}
static MEDIA_CACHE: OnceLock<Mutex<Option<MediaState>>> = OnceLock::new();
const MEDIA_QUERY_WAIT: Duration = Duration::from_millis(650);

#[tauri::command]
async fn get_media_state() -> Result<MediaState, String> {
    let Some(query_started_at) = begin_media_query(&MEDIA_QUERY_STARTED, Instant::now()) else {
        return Ok(cached_media_state()
            .unwrap_or_else(|| MediaState::no_session("Media session is starting")));
    };

    let (sender, receiver) = tokio::sync::oneshot::channel();
    tauri::async_runtime::spawn_blocking(move || {
        let result = media::current_media_state()
            .map_err(|error| error.to_string())
            .map(retain_last_media_when_session_disappears);
        if let Ok(state) = &result {
            store_media_state(state.clone());
        }
        end_media_query(&MEDIA_QUERY_STARTED, query_started_at);
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

/// Claims the right to read the media session, or `None` while a read that began
/// less than `MEDIA_QUERY_STUCK_AFTER` ago is still running.
fn begin_media_query(slot: &Mutex<Option<Instant>>, now: Instant) -> Option<Instant> {
    let mut started = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if started.is_some_and(|at| now.saturating_duration_since(at) < MEDIA_QUERY_STUCK_AFTER) {
        return None;
    }
    *started = Some(now);
    Some(now)
}

/// Releases the claim, unless a newer read took over from this one while it was stuck.
fn end_media_query(slot: &Mutex<Option<Instant>>, claimed_at: Instant) {
    let mut started = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if *started == Some(claimed_at) {
        *started = None;
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
    use super::{
        MEDIA_QUERY_STUCK_AFTER, MediaState, begin_media_query, end_media_query,
        retain_cached_media_when_session_disappears,
    };
    use std::{sync::Mutex, time::Instant};

    #[test]
    fn gives_up_on_a_media_read_that_never_returns() {
        let slot = Mutex::new(None);
        let start = Instant::now();

        let first = begin_media_query(&slot, start).expect("nothing is running");
        assert_eq!(begin_media_query(&slot, start), None);
        assert!(begin_media_query(&slot, start + MEDIA_QUERY_STUCK_AFTER).is_some());

        // The stuck read finishing late must not release its successor's claim.
        end_media_query(&slot, first);
        assert_eq!(
            begin_media_query(&slot, start + MEDIA_QUERY_STUCK_AFTER),
            None
        );
    }

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
    // Tauri would drop the overlay back to the standard floating level, which is
    // below fullscreen windows. Its change lands after any level set here, so on
    // macOS the overlay level alone does the job.
    #[cfg(target_os = "macos")]
    if enabled && let Some(overlay) = window.get_webview_window("main") {
        return overlay_z_order::apply(&overlay);
    }

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
