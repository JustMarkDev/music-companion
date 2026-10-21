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
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MEDIA_NEXT_TRACK,
    VK_MEDIA_PREV_TRACK,
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
    let _current_session_token = manager.CurrentSessionChanged(&TypedEventHandler::<
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
    // A closed session never fires again, so letting go of it releases its handlers
    // instead of collecting one entry for every session of a long-running overlay.
    let live = (0..sessions.Size()?)
        .filter_map(|index| sessions.GetAt(index).ok())
        .collect::<Vec<_>>();
    if let Ok(mut items) = subscriptions.lock() {
        items.retain(|item| live.contains(&item.session));
    }
    for session in live {
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
        let playback_info_token = session.PlaybackInfoChanged(&TypedEventHandler::<
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
        println!("[media-control] {action} requested, but no Windows media session is available");
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
    last_updated_ticks: i64,
    now_ms: u64,
    playback_rate: f64,
    is_playing: bool,
) -> u64 {
    if !is_playing || last_updated_ticks <= 0 || !playback_rate.is_finite() || playback_rate <= 0.0
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
            current_timeline_position(60_000, 1_000_000_000, 102_500, 1.0, true),
            62_500
        );
    }

    #[test]
    fn keeps_the_reported_position_while_paused() {
        assert_eq!(
            current_timeline_position(60_000, 1_000_000_000, 102_500, 1.0, false),
            60_000
        );
    }

    #[test]
    fn keeps_advancing_past_a_stale_track_duration() {
        assert_eq!(
            current_timeline_position(179_000, 1_000_000_000, 105_000, 1.0, true),
            184_000
        );
    }

    #[test]
    fn keeps_the_reported_position_for_invalid_timeline_metadata() {
        assert_eq!(
            current_timeline_position(60_000, 0, 102_500, 1.0, true),
            60_000
        );
        assert_eq!(
            current_timeline_position(60_000, 1_000_000_000, 102_500, f64::NAN, true,),
            60_000
        );
    }
}
