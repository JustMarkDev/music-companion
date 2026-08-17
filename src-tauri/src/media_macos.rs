//! macOS now-playing integration.
//!
//! Windows exposes system-wide playback through WMTC. The macOS counterpart is the
//! private `MediaRemote` framework, which has refused unentitled callers since
//! macOS 15.4. The primary backend therefore drives the vendored
//! `MediaRemoteAdapter` framework through `/usr/bin/perl`, which Apple still
//! entitles for `MediaRemote` access, and reports every player the system knows
//! about, browsers included.
//!
//! When that adapter is unavailable the module falls back to AppleScript against
//! Music and Spotify. The fallback only sees those two players, so browser
//! sessions are lost, but it keeps the overlay working without private APIs.

use super::MediaState;
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::Emitter;

/// Numeric `MRACommand` identifiers from the adapter's public header.
const COMMAND_TOGGLE_PLAY_PAUSE: &str = "2";
const COMMAND_NEXT_TRACK: &str = "4";
const COMMAND_PREVIOUS_TRACK: &str = "5";

/// Collapses bursts of `MediaRemote` notifications into a single overlay refresh.
const STREAM_DEBOUNCE_MS: &str = "150";
const STREAM_RESTART_DELAY: Duration = Duration::from_secs(2);
/// How long an orphaned reader can outlive the overlay after an unclean exit.
const STREAM_SUPERVISOR_INTERVAL_SECONDS: u64 = 2;
const APPLESCRIPT_POLL_INTERVAL: Duration = Duration::from_millis(900);
const UNIT_SEPARATOR: char = '\u{1f}';

static BACKEND: OnceLock<Backend> = OnceLock::new();
static STREAM_CHILD: Mutex<Option<Child>> = Mutex::new(None);

#[derive(Debug)]
struct AdapterPaths {
    framework: PathBuf,
    script: PathBuf,
    /// Only the adapter's `test` command uses this helper, but without it that
    /// command cannot run, so the backend probe depends on it being bundled.
    test_client: PathBuf,
}

#[derive(Debug)]
enum Backend {
    /// The vendored MediaRemote adapter, reporting every system media session.
    Adapter(AdapterPaths),
    /// AppleScript against Music and Spotify only.
    AppleScript,
}

fn backend() -> &'static Backend {
    BACKEND.get_or_init(|| match resolve_adapter_paths() {
        Some(paths) if adapter_is_entitled(&paths) => {
            println!(
                "[media] using the MediaRemote adapter at {}",
                paths.framework.display()
            );
            Backend::Adapter(paths)
        }
        Some(paths) => {
            eprintln!(
                "[media] the MediaRemote adapter at {} is not functional on this system; \
                 falling back to AppleScript for Music and Spotify",
                paths.framework.display()
            );
            Backend::AppleScript
        }
        None => {
            eprintln!(
                "[media] the MediaRemote adapter is missing from this build; \
                 falling back to AppleScript for Music and Spotify"
            );
            Backend::AppleScript
        }
    })
}

/// Locates the bundled adapter, then the `build.rs` staging directory used by
/// `tauri dev`.
fn resolve_adapter_paths() -> Option<AdapterPaths> {
    if let Ok(executable) = std::env::current_exe() {
        // Contents/MacOS/<binary> -> Contents/{Frameworks,Resources}
        if let Some(contents) = executable.parent().and_then(Path::parent) {
            let paths = AdapterPaths {
                framework: contents.join("Frameworks/MediaRemoteAdapter.framework"),
                script: contents.join("Resources/mediaremote-adapter.pl"),
                test_client: contents.join("Resources/MediaRemoteAdapterTestClient"),
            };
            if paths.is_complete() {
                return Some(paths);
            }
        }
    }

    #[cfg(debug_assertions)]
    {
        let staged = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/macos");
        let paths = AdapterPaths {
            framework: staged.join("MediaRemoteAdapter.framework"),
            script: staged.join("mediaremote-adapter.pl"),
            test_client: staged.join("MediaRemoteAdapterTestClient"),
        };
        if paths.is_complete() {
            return Some(paths);
        }
    }

    None
}

impl AdapterPaths {
    fn is_complete(&self) -> bool {
        self.framework.is_dir() && self.script.is_file() && self.test_client.is_file()
    }
}

/// The adapter's own `test` command reports whether Perl still reaches
/// `MediaRemote` on this macOS version.
fn adapter_is_entitled(paths: &AdapterPaths) -> bool {
    adapter_command(paths, ["test"])
        .status()
        .is_ok_and(|status| status.success())
}

fn adapter_command<'a>(
    paths: &AdapterPaths,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Command {
    let mut command = Command::new("/usr/bin/perl");
    command.arg(&paths.script).arg(&paths.framework);
    command.args(arguments);
    command
}

pub fn start_event_monitor(app: tauri::AppHandle) {
    match backend() {
        Backend::Adapter(_) => {
            thread::spawn(move || run_adapter_stream(app));
        }
        Backend::AppleScript => {
            thread::spawn(move || run_applescript_poll(app));
        }
    }
}

/// Terminates the long-lived adapter stream so quitting the app does not leave an
/// orphaned Perl process behind. The supervisor's `TERM` trap stops the reader it
/// started.
pub fn shutdown() {
    if let Ok(mut child) = STREAM_CHILD.lock() {
        if let Some(mut running) = child.take() {
            let _ = running.kill();
            let _ = running.wait();
        }
    }
}

/// Wraps the reader in a shell supervisor so the helper process cannot outlive the
/// overlay.
///
/// A clean quit goes through `shutdown`, which signals the supervisor and fires its
/// `TERM` trap. A crash, a force quit, or a `tauri dev` restart never runs that
/// path, so the supervisor also polls for the overlay's process ID and reaps the
/// reader once it disappears.
fn supervised_stream_script(paths: &AdapterPaths) -> String {
    let command = format!(
        "/usr/bin/perl {} {} stream --no-diff --no-artwork --micros --debounce={STREAM_DEBOUNCE_MS}",
        shell_quote(&paths.script.to_string_lossy()),
        shell_quote(&paths.framework.to_string_lossy()),
    );

    format!(
        "{command} & reader=$!\n\
         trap 'kill $reader 2>/dev/null' TERM INT\n\
         while kill -0 {overlay} 2>/dev/null; do sleep {interval}; done\n\
         kill $reader 2>/dev/null\n",
        overlay = std::process::id(),
        interval = STREAM_SUPERVISOR_INTERVAL_SECONDS,
    )
}

/// Both paths are produced by the build, but they still travel through a shell, so
/// they are quoted rather than trusted.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Mirrors the Windows event monitor: the adapter streams `MediaRemote`
/// notifications and every payload becomes one overlay refresh.
fn run_adapter_stream(app: tauri::AppHandle) {
    let Backend::Adapter(paths) = backend() else {
        return;
    };

    loop {
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(supervised_stream_script(paths))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                eprintln!("[media] unable to start the MediaRemote stream: {error}");
                thread::sleep(STREAM_RESTART_DELAY);
                continue;
            }
        };

        let Some(stdout) = child.stdout.take() else {
            eprintln!("[media] the MediaRemote stream produced no output pipe");
            let _ = child.kill();
            thread::sleep(STREAM_RESTART_DELAY);
            continue;
        };

        if let Ok(mut slot) = STREAM_CHILD.lock() {
            if let Some(mut previous) = slot.replace(child) {
                let _ = previous.kill();
                let _ = previous.wait();
            }
        }

        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            emit_media_change(&app, "mediaremote-stream");
        }

        // Reaching this point means the adapter exited, which happens when
        // `mediaremoted` restarts. Restart the stream instead of silently
        // leaving the overlay without playback events.
        shutdown();
        eprintln!("[media] the MediaRemote stream ended; restarting it shortly");
        thread::sleep(STREAM_RESTART_DELAY);
    }
}

/// AppleScript exposes no change notifications, so the fallback polls and only
/// refreshes the overlay when the reported track or clock actually moves.
fn run_applescript_poll(app: tauri::AppHandle) {
    let mut previous = None;
    loop {
        let fingerprint = applescript_media_state().ok().map(|state| {
            (
                state.has_session,
                state.is_playing,
                state.title,
                state.artist,
                state.duration_ms,
            )
        });
        if fingerprint != previous {
            previous = fingerprint;
            emit_media_change(&app, "applescript-poll");
        }
        thread::sleep(APPLESCRIPT_POLL_INTERVAL);
    }
}

fn emit_media_change(app: &tauri::AppHandle, reason: &str) {
    let _ = app.emit("media-state-changed", reason);
}

pub fn current_media_state() -> Result<MediaState, String> {
    match backend() {
        Backend::Adapter(paths) => adapter_media_state(paths),
        Backend::AppleScript => applescript_media_state(),
    }
}

fn adapter_media_state(paths: &AdapterPaths) -> Result<MediaState, String> {
    let output = adapter_command(paths, ["get", "--micros", "--no-artwork"])
        .output()
        .map_err(|error| format!("unable to read the now playing session: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "the MediaRemote adapter exited with {}",
            output.status
        ));
    }

    let payload = String::from_utf8_lossy(&output.stdout);
    let payload = payload.trim();
    if payload.is_empty() {
        return Ok(MediaState::no_session("No session"));
    }

    let info: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| format!("unable to parse the now playing session: {error}"))?;

    Ok(media_state_from_adapter(&info, now_epoch_ms()))
}

fn media_state_from_adapter(info: &serde_json::Value, now_ms: u64) -> MediaState {
    let title = string_field(info, "title");
    let artist = string_field(info, "artist");
    if title.is_empty() && artist.is_empty() {
        return MediaState::no_session("No session");
    }

    let is_playing = info
        .get("playing")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let playback_rate = info
        .get("playbackRate")
        .and_then(serde_json::Value::as_f64)
        // Paused players report a rate of 0, which must not be mistaken for
        // metadata the overlay can use to advance its clock.
        .filter(|rate| *rate > 0.0);
    let duration_ms = micros_field(info, "durationMicros").map(|micros| micros / 1_000);
    let elapsed_ms = micros_field(info, "elapsedTimeMicros").unwrap_or(0) / 1_000;
    let reported_at_ms = micros_field(info, "timestampEpochMicros").map(|micros| micros / 1_000);

    MediaState {
        has_session: true,
        is_playing,
        status: playback_status(is_playing),
        title,
        artist,
        album: string_field(info, "album"),
        source_app: string_field(info, "bundleIdentifier"),
        position_ms: current_timeline_position(
            elapsed_ms,
            reported_at_ms,
            now_ms,
            playback_rate.unwrap_or(1.0),
            is_playing,
        ),
        duration_ms: duration_ms.filter(|duration| *duration > 0),
        playback_rate,
        playing_session_count: u32::from(is_playing),
    }
}

/// Advances a reported position to the present moment, matching how the Windows
/// backend treats WMTC timeline properties.
fn current_timeline_position(
    elapsed_ms: u64,
    reported_at_ms: Option<u64>,
    now_ms: u64,
    playback_rate: f64,
    is_playing: bool,
) -> u64 {
    let Some(reported_at_ms) = reported_at_ms else {
        return elapsed_ms;
    };
    if !is_playing || reported_at_ms == 0 || !playback_rate.is_finite() || playback_rate <= 0.0 {
        return elapsed_ms;
    }

    let since_report_ms = now_ms.saturating_sub(reported_at_ms);
    elapsed_ms.saturating_add((since_report_ms as f64 * playback_rate).round() as u64)
}

fn playback_status(is_playing: bool) -> String {
    if is_playing { "Playing" } else { "Paused" }.to_string()
}

fn string_field(info: &serde_json::Value, key: &str) -> String {
    info.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn micros_field(info: &serde_json::Value, key: &str) -> Option<u64> {
    let value = info.get(key)?.as_f64()?;
    (value.is_finite() && value >= 0.0).then_some(value.round() as u64)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn send_transport_control(
    action: &str,
    // Windows falls back to synthesising media keys when a session declines a
    // command. MediaRemote is the system media controller itself, so there is no
    // lower layer to fall back to here.
    _allow_media_key_fallback: bool,
) -> Result<bool, String> {
    match backend() {
        Backend::Adapter(paths) => {
            let Some(command) = adapter_transport_command(action) else {
                return Ok(false);
            };
            println!("[media-control] sending {action} to the now playing application");
            let status = adapter_command(paths, ["send", command])
                .status()
                .map_err(|error| format!("unable to send {action}: {error}"))?;
            Ok(status.success())
        }
        Backend::AppleScript => applescript_transport_control(action),
    }
}

fn adapter_transport_command(action: &str) -> Option<&'static str> {
    match action {
        "next" => Some(COMMAND_NEXT_TRACK),
        "previous" => Some(COMMAND_PREVIOUS_TRACK),
        "play/pause" => Some(COMMAND_TOGGLE_PLAY_PAUSE),
        _ => None,
    }
}

/// AppleScript player definitions. Music reports track length in seconds while
/// Spotify reports milliseconds, so each player carries its own divisor.
struct ScriptedPlayer {
    name: &'static str,
    bundle_id: &'static str,
    duration_divisor: f64,
}

const SCRIPTED_PLAYERS: [ScriptedPlayer; 2] = [
    ScriptedPlayer {
        name: "Music",
        bundle_id: "com.apple.Music",
        duration_divisor: 0.001,
    },
    ScriptedPlayer {
        name: "Spotify",
        bundle_id: "com.spotify.client",
        duration_divisor: 1.0,
    },
];

fn applescript_media_state() -> Result<MediaState, String> {
    let mut paused = None;
    for player in &SCRIPTED_PLAYERS {
        let Some(state) = scripted_player_state(player) else {
            continue;
        };
        if state.is_playing {
            return Ok(state);
        }
        paused = paused.or(Some(state));
    }

    Ok(paused.unwrap_or_else(|| MediaState::no_session("No session")))
}

fn scripted_player_state(player: &ScriptedPlayer) -> Option<MediaState> {
    let output = run_applescript(&now_playing_script(player.name))?;
    let fields: Vec<&str> = output.split(UNIT_SEPARATOR).collect();
    let [status, title, artist, album, duration, position] = fields.as_slice() else {
        return None;
    };

    let is_playing = status.eq_ignore_ascii_case("playing");
    let duration_ms = duration
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| (value / player.duration_divisor).round() as u64);
    let position_ms = position
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| (value * 1_000.0).round() as u64)
        .unwrap_or(0);

    Some(MediaState {
        has_session: true,
        is_playing,
        status: playback_status(is_playing),
        title: title.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
        source_app: player.bundle_id.to_string(),
        position_ms,
        duration_ms,
        // AppleScript reports the position at query time, so there is no
        // timestamp to extrapolate from.
        playback_rate: is_playing.then_some(1.0),
        playing_session_count: u32::from(is_playing),
    })
}

/// `is running` is used deliberately: addressing a player any other way would
/// launch it.
fn now_playing_script(player: &str) -> String {
    format!(
        r#"if application "{player}" is not running then return ""
tell application "{player}"
  try
    if player state is stopped then return ""
    set separator to (ASCII character 31)
    set item_ to current track
    return (player state as string) & separator & (name of item_ as string) & separator & (artist of item_ as string) & separator & (album of item_ as string) & separator & (duration of item_ as string) & separator & (player position as string)
  on error
    return ""
  end try
end tell"#
    )
}

fn applescript_transport_control(action: &str) -> Result<bool, String> {
    let command = match action {
        "next" => "next track",
        "previous" => "previous track",
        "play/pause" => "playpause",
        _ => return Ok(false),
    };

    for player in &SCRIPTED_PLAYERS {
        // Only command the player the overlay is actually following.
        if scripted_player_state(player).is_none() {
            continue;
        }
        println!("[media-control] sending {action} to {}", player.name);
        let script = format!(
            r#"if application "{name}" is not running then return ""
tell application "{name}"
  {command}
end tell
return "ok""#,
            name = player.name
        );
        if run_applescript(&script).is_some() {
            return Ok(true);
        }
    }

    println!("[media-control] {action} requested, but no scriptable player is running");
    Ok(false)
}

fn run_applescript(script: &str) -> Option<String> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let error = error.trim();
        if !error.is_empty() {
            eprintln!("[media] AppleScript failed: {error}");
        }
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{
        current_timeline_position, media_state_from_adapter, now_playing_script, UNIT_SEPARATOR,
    };

    #[test]
    fn advances_the_position_while_playing() {
        assert_eq!(
            current_timeline_position(60_000, Some(100_000), 102_500, 1.0, true),
            62_500
        );
    }

    #[test]
    fn keeps_the_reported_position_while_paused() {
        assert_eq!(
            current_timeline_position(60_000, Some(100_000), 102_500, 1.0, false),
            60_000
        );
    }

    #[test]
    fn keeps_the_reported_position_for_invalid_timeline_metadata() {
        assert_eq!(
            current_timeline_position(60_000, None, 102_500, 1.0, true),
            60_000
        );
        assert_eq!(
            current_timeline_position(60_000, Some(0), 102_500, 1.0, true),
            60_000
        );
        assert_eq!(
            current_timeline_position(60_000, Some(100_000), 102_500, f64::NAN, true),
            60_000
        );
    }

    #[test]
    fn maps_an_adapter_payload_onto_the_media_state() {
        let info = serde_json::json!({
            "title": "Clint Eastwood",
            "artist": "Gorillaz",
            "album": "Gorillaz",
            "bundleIdentifier": "com.spotify.client",
            "playing": true,
            "playbackRate": 1.0,
            "durationMicros": 267_021_000_u64,
            "elapsedTimeMicros": 60_000_000_u64,
            "timestampEpochMicros": 1_700_000_000_000_000_u64,
        });

        let state = media_state_from_adapter(&info, 1_700_000_002_500);

        assert!(state.has_session);
        assert!(state.is_playing);
        assert_eq!(state.status, "Playing");
        assert_eq!(state.title, "Clint Eastwood");
        assert_eq!(state.artist, "Gorillaz");
        assert_eq!(state.source_app, "com.spotify.client");
        assert_eq!(state.duration_ms, Some(267_021));
        assert_eq!(state.position_ms, 62_500);
        assert_eq!(state.playing_session_count, 1);
    }

    #[test]
    fn reports_no_session_for_an_empty_adapter_payload() {
        let state = media_state_from_adapter(&serde_json::json!({}), 0);

        assert!(!state.has_session);
        assert_eq!(state.status, "No session");
    }

    #[test]
    fn freezes_the_clock_and_drops_the_rate_for_a_paused_payload() {
        let info = serde_json::json!({
            "title": "Clint Eastwood",
            "artist": "Gorillaz",
            "playing": false,
            "playbackRate": 0.0,
            "elapsedTimeMicros": 60_000_000_u64,
            "timestampEpochMicros": 1_700_000_000_000_000_u64,
        });

        let state = media_state_from_adapter(&info, 1_700_000_030_000);

        assert!(!state.is_playing);
        assert_eq!(state.status, "Paused");
        assert_eq!(state.position_ms, 60_000);
        assert_eq!(state.playback_rate, None);
        assert_eq!(state.playing_session_count, 0);
    }

    #[test]
    fn supervises_the_reader_against_an_unclean_exit() {
        let script = super::supervised_stream_script(&super::AdapterPaths {
            framework: "/Apps/Music Companion.app/Frameworks/MediaRemoteAdapter.framework".into(),
            script: "/Apps/Music Companion.app/Resources/mediaremote-adapter.pl".into(),
        });

        // A TERM trap covers a clean quit and the process-ID poll covers a crash,
        // a force quit, or a `tauri dev` restart.
        assert!(script.contains("trap 'kill $reader 2>/dev/null' TERM INT"));
        assert!(script.contains(&format!("while kill -0 {} ", std::process::id())));
        // Paths with spaces must survive the shell.
        assert!(script.contains("'/Apps/Music Companion.app/Resources/mediaremote-adapter.pl'"));
        assert!(script.contains("--no-artwork"));
    }

    #[test]
    fn quotes_paths_that_could_break_out_of_the_shell() {
        assert_eq!(super::shell_quote("/tmp/plain"), "'/tmp/plain'");
        assert_eq!(super::shell_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
    }

    #[test]
    fn scripts_a_player_without_launching_it() {
        let script = now_playing_script("Music");

        assert!(script.contains(r#"if application "Music" is not running then return """#));
        assert!(script.contains("ASCII character 31"));
        assert_eq!(UNIT_SEPARATOR, '\u{1f}');
    }
}
