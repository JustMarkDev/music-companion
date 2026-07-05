#[cfg(not(target_os = "windows"))]
compile_error!("Music Companion is currently Windows-only.");

use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
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
        let result = media::current_media_state().map_err(|error| error.to_string());
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
async fn fetch_lyrics(
    title: String,
    artist: String,
    duration_ms: Option<u64>,
) -> Result<Option<LyricsResult>, String> {
    lyrics::fetch_lyrics(&title, &artist, duration_ms).await
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
fn show_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "Settings window is unavailable".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window
        .emit("settings-window-opened", ())
        .map_err(|error| error.to_string())
}

pub fn run() {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
    use tauri_plugin_window_state::StateFlags;

    let lock_shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyL);
    let shortcut_for_handler = lock_shortcut.clone();
    let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_shortcut(lock_shortcut)
        .expect("failed to register Ctrl+Shift+L")
        .with_handler(move |app, shortcut, event| {
            if shortcut == &shortcut_for_handler && event.state() == ShortcutState::Pressed {
                let _ = app.emit("toggle-overlay-lock", ());
            }
        })
        .build();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::POSITION | StateFlags::SIZE)
                .build(),
        )
        .plugin(shortcut_plugin)
        .invoke_handler(tauri::generate_handler![
            get_media_state,
            fetch_lyrics,
            get_start_at_login,
            set_start_at_login,
            set_always_on_top,
            show_settings_window
        ])
        .setup(|app| {
            build_tray(app)?;
            if let Some(window) = app.get_webview_window("main") {
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
            "quit" => app.exit(0),
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
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    const WINDOWS_EPOCH_OFFSET_MS: u64 = 11_644_473_600_000;
    static SELECTED_SESSION: OnceLock<Mutex<Option<GlobalSystemMediaTransportControlsSession>>> =
        OnceLock::new();

    pub fn current_media_state() -> windows::core::Result<MediaState> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
        let sessions = manager.GetSessions()?;
        let mut playing_count = 0;

        for index in 0..sessions.Size()? {
            let session = sessions.GetAt(index)?;
            let playback = session.GetPlaybackInfo()?;
            if playback.PlaybackStatus()?
                == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            {
                playing_count += 1;
            }
        }

        let retained = selected_session().filter(session_is_playing);
        let current = manager.GetCurrentSession().ok();
        let current_is_playing = current.as_ref().is_some_and(session_is_playing);
        let mut selected = retained.or(current);

        if !current_is_playing
            && selected
                .as_ref()
                .is_none_or(|session| !session_is_playing(session))
        {
            selected = (0..sessions.Size()?).find_map(|index| {
                let session = sessions.GetAt(index).ok()?;
                session_is_playing(&session).then_some(session)
            });
        }

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

mod lyrics {
    use super::{LrclibLyrics, LyricsResult};

    pub async fn fetch_lyrics(
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Result<Option<LyricsResult>, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "MusicCompanion/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/JustMarkDev/Music-Companion)"
            ))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|error| error.to_string())?;

        search(&client, title, artist, duration_ms).await
    }

    async fn search(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        duration_ms: Option<u64>,
    ) -> Result<Option<LyricsResult>, String> {
        let query = format!("{artist} {title}");
        let url = format!(
            "https://lrclib.net/api/search?q={}",
            urlencoding::encode(&query)
        );
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let mut results = response
            .json::<Vec<LrclibLyrics>>()
            .await
            .map_err(|error| error.to_string())?;

        let normalized_title = normalize(title);
        let normalized_artist = normalize(artist);
        results.sort_by_key(|item| {
            let track_score = score(item.track_name.as_deref(), &normalized_title);
            let artist_score = score(item.artist_name.as_deref(), &normalized_artist);
            let metadata_score = track_score * 4 + artist_score * 3;
            let is_synced = has_synced_lyrics(item);
            let duration_difference = duration_difference_ms(item.duration, duration_ms);
            (
                std::cmp::Reverse(is_synced),
                duration_difference,
                std::cmp::Reverse(metadata_score),
            )
        });

        Ok(results.into_iter().next().map(LrclibLyrics::into_result))
    }

    fn normalize(value: &str) -> String {
        value
            .to_lowercase()
            .chars()
            .filter(|char| char.is_alphanumeric() || char.is_whitespace())
            .collect::<String>()
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

    fn has_synced_lyrics(candidate: &LrclibLyrics) -> bool {
        candidate.instrumental || has_lyrics(candidate.synced_lyrics.as_deref())
    }

    fn has_lyrics(lyrics: Option<&str>) -> bool {
        lyrics.is_some_and(|value| !value.trim().is_empty())
    }

    impl LrclibLyrics {
        fn into_result(self) -> LyricsResult {
            LyricsResult {
                source: "LRCLIB".to_string(),
                track_name: self.track_name.unwrap_or_default(),
                artist_name: self.artist_name.unwrap_or_default(),
                album_name: self.album_name.unwrap_or_default(),
                duration: self.duration.map(|value| value.round() as u64),
                instrumental: self.instrumental,
                synced_lyrics: self.synced_lyrics,
                plain_lyrics: self.plain_lyrics,
            }
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
