#[cfg(not(target_os = "windows"))]
compile_error!("Music Companion is currently Windows-only.");

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WebviewWindow, WindowEvent,
};

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
    MEDIA_CACHE.get_or_init(|| Mutex::new(None)).lock().ok()?.clone()
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
    album: String,
    duration_ms: Option<u64>,
) -> Result<Option<LyricsResult>, String> {
    lyrics::fetch_lyrics(&title, &artist, &album, duration_ms).await
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

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_media_state,
            fetch_lyrics,
            get_start_at_login,
            set_start_at_login,
            set_always_on_top
        ])
        .setup(|app| {
            build_tray(app)?;
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

mod media {
    use super::MediaState;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    pub fn current_media_state() -> windows::core::Result<MediaState> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
        let sessions = manager.GetSessions()?;
        let mut playing_count = 0;
        let mut selected: Option<GlobalSystemMediaTransportControlsSession> =
            manager.GetCurrentSession().ok();

        for index in 0..sessions.Size()? {
            let session = sessions.GetAt(index)?;
            let playback = session.GetPlaybackInfo()?;
            if playback.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
                playing_count += 1;
                selected = Some(session);
            }
        }

        let Some(session) = selected else {
            return Ok(MediaState::no_session("No session"));
        };

        let playback = session.GetPlaybackInfo()?;
        let status = playback.PlaybackStatus()?;
        let properties = session.TryGetMediaPropertiesAsync()?.get()?;
        let timeline = session.GetTimelineProperties()?;
        let position_ms = timespan_to_ms(timeline.Position()?);
        let end_ms = timespan_to_ms(timeline.EndTime()?);
        let playback_rate = playback
            .PlaybackRate()
            .ok()
            .and_then(|value| value.Value().ok());

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

    fn timespan_to_ms(value: windows::Foundation::TimeSpan) -> u64 {
        if value.Duration <= 0 {
            return 0;
        }
        (value.Duration / 10_000) as u64
    }
}

mod lyrics {
    use super::{LrclibLyrics, LyricsResult};

    pub async fn fetch_lyrics(
        title: &str,
        artist: &str,
        album: &str,
        duration_ms: Option<u64>,
    ) -> Result<Option<LyricsResult>, String> {
        let client = reqwest::Client::builder()
            .user_agent("MusicCompanion/0.1.0 (https://github.com/local/music-companion)")
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|error| error.to_string())?;

        if let Some(found) = exact_match(&client, title, artist, album, duration_ms).await? {
            return Ok(Some(found));
        }

        search(&client, title, artist).await
    }

    async fn exact_match(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
        album: &str,
        duration_ms: Option<u64>,
    ) -> Result<Option<LyricsResult>, String> {
        let duration = duration_ms.map(|value| (value as f64 / 1000.0).round() as u64);
        let mut url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            urlencoding::encode(title),
            urlencoding::encode(artist)
        );

        if !album.trim().is_empty() {
            url.push_str("&album_name=");
            url.push_str(&urlencoding::encode(album));
        }

        if let Some(seconds) = duration {
            url.push_str("&duration=");
            url.push_str(&seconds.to_string());
        }

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;

        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Ok(None);
        }

        let payload = response
            .json::<LrclibLyrics>()
            .await
            .map_err(|error| error.to_string())?;

        Ok(Some(payload.into_result()))
    }

    async fn search(
        client: &reqwest::Client,
        title: &str,
        artist: &str,
    ) -> Result<Option<LyricsResult>, String> {
        let query = format!("{artist} {title}");
        let url = format!("https://lrclib.net/api/search?q={}", urlencoding::encode(&query));
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
            std::cmp::Reverse(track_score + artist_score)
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
        let run = hkcu.open_subkey(RUN_KEY).map_err(|error| error.to_string())?;
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
