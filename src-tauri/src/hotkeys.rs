use serde::Serialize;
use std::sync::{Mutex, OnceLock};
use tauri::{Emitter, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HotkeyStatus {
    pub(crate) action: String,
    pub(crate) accelerator: String,
    pub(crate) registered: bool,
    pub(crate) error: Option<String>,
    pub(crate) conflict_action: Option<String>,
}

#[derive(Default)]
struct HotkeyRegistry {
    statuses: Vec<HotkeyStatus>,
    recording: bool,
}

trait ShortcutBackend {
    fn validate(&self, accelerator: &str) -> Result<(), String>;
    fn register(&mut self, accelerator: &str) -> Result<(), String>;
    fn unregister(&mut self, accelerator: &str) -> Result<(), String>;
    fn unregister_all(&mut self) -> Result<(), String>;
}

impl HotkeyRegistry {
    fn register(
        &mut self,
        backend: &mut impl ShortcutBackend,
        action: String,
        accelerator: String,
    ) -> Result<HotkeyStatus, String> {
        backend.validate(&accelerator)?;
        let current = self
            .statuses
            .iter()
            .find(|status| status.action == action)
            .cloned();
        if current
            .as_ref()
            .is_some_and(|status| status.registered && status.accelerator == accelerator)
        {
            return Ok(current.expect("current status exists"));
        }

        if let Some(conflict_action) = self
            .statuses
            .iter()
            .find(|status| {
                status.action != action && status.registered && status.accelerator == accelerator
            })
            .map(|status| status.action.clone())
        {
            return Ok(HotkeyStatus {
                action,
                accelerator,
                registered: false,
                error: Some(format!(
                    "Shortcut is already registered for {conflict_action}"
                )),
                conflict_action: Some(conflict_action),
            });
        }

        if self.recording {
            let status = successful_status(action, accelerator);
            self.replace(status.clone());
            return Ok(status);
        }

        if let Some(current) = &current {
            if current.registered {
                if let Err(error) = backend.unregister(&current.accelerator) {
                    return Ok(failed_status(action, accelerator, error));
                }
            }
        }

        match backend.register(&accelerator) {
            Ok(()) => {
                let status = successful_status(action, accelerator);
                self.replace(status.clone());
                Ok(status)
            }
            Err(error) => {
                let failed = failed_status(action.clone(), accelerator, error);
                let rolled_back = current.as_ref().is_some_and(|status| {
                    status.registered && backend.register(&status.accelerator).is_ok()
                });
                if !rolled_back {
                    self.replace(failed.clone());
                }
                Ok(failed)
            }
        }
    }

    fn set_recording(
        &mut self,
        backend: &mut impl ShortcutBackend,
        recording: bool,
    ) -> Result<(), String> {
        if self.recording == recording {
            return Ok(());
        }
        if recording {
            backend
                .unregister_all()
                .map_err(|error| format!("Unable to suspend shortcuts while recording: {error}"))?;
            self.recording = true;
            return Ok(());
        }

        backend
            .unregister_all()
            .map_err(|error| format!("Unable to reset shortcuts after recording: {error}"))?;
        self.recording = false;
        for status in self.statuses.iter_mut().filter(|status| status.registered) {
            if let Err(error) = backend.register(&status.accelerator) {
                status.registered = false;
                status.error = Some(error);
                status.conflict_action = None;
            }
        }
        Ok(())
    }

    fn retry_failed(&mut self, backend: &mut impl ShortcutBackend) -> bool {
        let failed = self
            .statuses
            .iter()
            .filter(|status| !status.registered)
            .cloned()
            .collect::<Vec<_>>();
        let mut recovered = false;
        for status in failed {
            recovered |= self
                .register(backend, status.action, status.accelerator)
                .is_ok_and(|status| status.registered);
        }
        recovered
    }

    fn replace(&mut self, status: HotkeyStatus) {
        self.statuses.retain(|item| item.action != status.action);
        self.statuses.push(status);
    }
}

fn successful_status(action: String, accelerator: String) -> HotkeyStatus {
    HotkeyStatus {
        action,
        accelerator,
        registered: true,
        error: None,
        conflict_action: None,
    }
}

fn failed_status(action: String, accelerator: String, error: String) -> HotkeyStatus {
    HotkeyStatus {
        action,
        accelerator,
        registered: false,
        error: Some(error),
        conflict_action: None,
    }
}

static REGISTRY: OnceLock<Mutex<HotkeyRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<HotkeyRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(HotkeyRegistry::default()))
}

struct TauriShortcutBackend<R: Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: Runtime> ShortcutBackend for TauriShortcutBackend<R> {
    fn validate(&self, accelerator: &str) -> Result<(), String> {
        accelerator
            .parse::<Shortcut>()
            .map(|_| ())
            .map_err(|error| format!("Invalid shortcut: {error}"))
    }

    fn register(&mut self, accelerator: &str) -> Result<(), String> {
        let shortcut = accelerator
            .parse::<Shortcut>()
            .map_err(|error| format!("Invalid shortcut: {error}"))?;
        self.app
            .global_shortcut()
            .register(shortcut)
            .map_err(|error| error.to_string())
    }

    fn unregister(&mut self, accelerator: &str) -> Result<(), String> {
        let shortcut = accelerator
            .parse::<Shortcut>()
            .map_err(|error| format!("Invalid shortcut: {error}"))?;
        self.app
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|error| error.to_string())
    }

    fn unregister_all(&mut self) -> Result<(), String> {
        self.app
            .global_shortcut()
            .unregister_all()
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn statuses() -> Vec<HotkeyStatus> {
    registry()
        .lock()
        .map(|registry| registry.statuses.clone())
        .unwrap_or_default()
}

pub(crate) fn set_recording<R: Runtime>(
    app: tauri::AppHandle<R>,
    recording: bool,
) -> Result<(), String> {
    let mut backend = TauriShortcutBackend { app: app.clone() };
    registry()
        .lock()
        .map_err(|_| "Hotkey registry is unavailable".to_string())?
        .set_recording(&mut backend, recording)?;
    if !recording {
        let _ = app.emit("hotkey-statuses-changed", ());
    }
    Ok(())
}

pub(crate) fn register<R: Runtime>(
    app: tauri::AppHandle<R>,
    action: String,
    accelerator: String,
) -> Result<HotkeyStatus, String> {
    let mut backend = TauriShortcutBackend { app };
    let status = registry()
        .lock()
        .map_err(|_| "Hotkey registry is unavailable".to_string())?
        .register(&mut backend, action.clone(), accelerator.clone())?;
    if status.registered {
        println!("[hotkey] registered {accelerator} for {action}");
    } else {
        eprintln!(
            "[hotkey] unable to register {accelerator} for {action}: {}",
            status.error.as_deref().unwrap_or("unknown error")
        );
    }
    Ok(status)
}

pub(crate) fn retry_failed<R: Runtime>(app: tauri::AppHandle<R>) {
    std::thread::spawn(move || {
        for delay in [250, 750, 1_500] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let mut backend = TauriShortcutBackend { app: app.clone() };
            let result = registry().lock().map(|mut registry| {
                if registry.statuses.iter().all(|status| status.registered) {
                    return None;
                }
                Some(registry.retry_failed(&mut backend))
            });
            match result {
                Ok(None) => return,
                Ok(Some(true)) => {
                    let _ = app.emit("hotkey-statuses-changed", ());
                }
                _ => {}
            }
        }
    });
}

pub(crate) fn action_for(shortcut: &Shortcut) -> Option<String> {
    registry().lock().ok().and_then(|registry| {
        if registry.recording {
            return None;
        }
        registry
            .statuses
            .iter()
            .find(|status| {
                status.registered
                    && status
                        .accelerator
                        .parse::<Shortcut>()
                        .is_ok_and(|registered| &registered == shortcut)
            })
            .map(|status| status.action.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::{HotkeyRegistry, ShortcutBackend};
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeBackend {
        active: HashSet<String>,
        rejected: HashSet<String>,
        rejected_unregistrations: HashSet<String>,
        unregister_all_calls: usize,
    }

    impl ShortcutBackend for FakeBackend {
        fn validate(&self, accelerator: &str) -> Result<(), String> {
            (!accelerator.is_empty())
                .then_some(())
                .ok_or_else(|| "invalid".to_string())
        }

        fn register(&mut self, accelerator: &str) -> Result<(), String> {
            if self.rejected.contains(accelerator) {
                return Err("unavailable".to_string());
            }
            self.active.insert(accelerator.to_string());
            Ok(())
        }

        fn unregister(&mut self, accelerator: &str) -> Result<(), String> {
            if self.rejected_unregistrations.contains(accelerator) {
                return Err("unable to unregister".to_string());
            }
            self.active.remove(accelerator);
            Ok(())
        }

        fn unregister_all(&mut self) -> Result<(), String> {
            self.unregister_all_calls += 1;
            self.active.clear();
            Ok(())
        }
    }

    #[test]
    fn failed_change_rolls_back_the_previous_shortcut() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();
        backend.rejected.insert("Alt+N".into());

        let result = registry
            .register(&mut backend, "next".into(), "Alt+N".into())
            .unwrap();

        assert!(!result.registered);
        assert!(backend.active.contains("Ctrl+Right"));
        assert_eq!(registry.statuses[0].accelerator, "Ctrl+Right");
        assert!(registry.statuses[0].registered);
    }

    #[test]
    fn failed_unregistration_leaves_the_previous_shortcut_unchanged() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();
        backend.rejected_unregistrations.insert("Ctrl+Right".into());

        let result = registry
            .register(&mut backend, "next".into(), "Alt+N".into())
            .unwrap();

        assert!(!result.registered);
        assert_eq!(registry.statuses[0].accelerator, "Ctrl+Right");
        assert!(backend.active.contains("Ctrl+Right"));
        assert!(!backend.active.contains("Alt+N"));
    }

    #[test]
    fn conflicting_action_leaves_existing_registrations_unchanged() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();

        let result = registry
            .register(&mut backend, "previous".into(), "Ctrl+Right".into())
            .unwrap();

        assert_eq!(result.conflict_action.as_deref(), Some("next"));
        assert_eq!(registry.statuses.len(), 1);
        assert!(backend.active.contains("Ctrl+Right"));
    }

    #[test]
    fn recording_suspension_is_idempotent_and_restores_shortcuts() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();

        registry.set_recording(&mut backend, true).unwrap();
        registry.set_recording(&mut backend, true).unwrap();
        assert!(backend.active.is_empty());
        assert_eq!(backend.unregister_all_calls, 1);

        registry.set_recording(&mut backend, false).unwrap();
        registry.set_recording(&mut backend, false).unwrap();
        assert!(backend.active.contains("Ctrl+Right"));
        assert_eq!(backend.unregister_all_calls, 2);
    }

    #[test]
    fn shortcut_changed_while_recording_is_registered_on_resume() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();
        registry.set_recording(&mut backend, true).unwrap();

        registry
            .register(&mut backend, "next".into(), "Alt+N".into())
            .unwrap();
        assert!(backend.active.is_empty());
        registry.set_recording(&mut backend, false).unwrap();

        assert!(backend.active.contains("Alt+N"));
        assert!(!backend.active.contains("Ctrl+Right"));
    }

    #[test]
    fn restoration_failure_becomes_a_retryable_status() {
        let mut registry = HotkeyRegistry::default();
        let mut backend = FakeBackend::default();
        registry
            .register(&mut backend, "next".into(), "Ctrl+Right".into())
            .unwrap();
        registry.set_recording(&mut backend, true).unwrap();
        backend.rejected.insert("Ctrl+Right".into());

        registry.set_recording(&mut backend, false).unwrap();

        assert!(!registry.statuses[0].registered);
        backend.rejected.clear();
        assert!(registry.retry_failed(&mut backend));
        assert!(registry.statuses[0].registered);
    }
}
