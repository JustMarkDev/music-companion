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
