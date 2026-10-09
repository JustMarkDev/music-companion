//! macOS overlay backdrop.
//!
//! Windows composes the overlay with Mica or Acrylic. macOS 26 exposes Liquid
//! Glass, and the plugin used here falls back to `NSVisualEffectView` on earlier
//! versions, so both settings stay meaningful across supported releases.

use objc2::ffi::class_addMethod;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};
use tauri::{Manager, WebviewWindow};
use tauri_plugin_liquid_glass::{GlassMaterialVariant, LiquidGlassConfig, LiquidGlassExt};

/// Must match `--window-radius` for macOS in `styles.css`, so the glass and the
/// overlay surface share the same rounded corners.
const OVERLAY_CORNER_RADIUS: f64 = 12.0;

pub fn apply(window: &WebviewWindow, intensity: u8, material: &str) -> Result<(), String> {
    let config = LiquidGlassConfig {
        // Matching Windows, a zero intensity disables the native backdrop and
        // leaves the webview's own background in charge.
        enabled: intensity > 0,
        corner_radius: OVERLAY_CORNER_RADIUS,
        tint_color: tint_for(intensity),
        variant: variant_for(material),
    };

    window
        .app_handle()
        .liquid_glass()
        .set_effect(window, config)
        .map_err(|error| error.to_string())?;

    keep_glass_active(window);
    set_glass_style(window, material);
    Ok(())
}

/// The plugin chooses Regular or Clear through the private `set_variant:`, which
/// current macOS accepts and ignores, so both options looked the same. The public
/// `style` property does switch the glass, and `GlassMaterialVariant`'s Regular (0)
/// and Clear (1) match `NSGlassEffectViewStyle`. Older macOS has no glass view to
/// find, so this does nothing there.
fn set_glass_style(window: &WebviewWindow, material: &str) {
    let style = variant_for(material) as isize;
    let window = window.clone();
    let target = window.clone();
    let _ = target.run_on_main_thread(move || {
        let (Ok(handle), Some(glass_class)) =
            (window.ns_window(), AnyClass::get(c"NSGlassEffectView"))
        else {
            return;
        };
        // SAFETY: `ns_window` is a live `NSWindow`, this closure runs on the main thread, and
        // every selector is public AppKit API on the receiver it is sent to.
        unsafe {
            let ns_window = handle.cast::<AnyObject>();
            let content: *mut AnyObject = msg_send![ns_window, contentView];
            if content.is_null() {
                return;
            }
            let subviews: *mut AnyObject = msg_send![content, subviews];
            let count: usize = msg_send![subviews, count];
            for index in 0..count {
                let view: *mut AnyObject = msg_send![subviews, objectAtIndex: index];
                let is_glass: Bool = msg_send![view, isKindOfClass: glass_class];
                if is_glass.as_bool() {
                    let _: () = msg_send![view, setStyle: style];
                }
            }
        }
    });
}

extern "C-unwind" fn always_active(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::YES
}

/// AppKit greys out Liquid Glass while its window lacks focus, which dims the
/// overlay whenever another app is frontmost. The glass asks its window for the
/// private `_hasActiveAppearance`, so the overlay's window class always answers
/// yes. The override lives on the window subclass tao creates, so no other
/// `NSWindow` is affected, and `isKeyWindow` keeps reporting the truth. If a future
/// macOS drops the selector, the override is inert and the glass merely dims again.
fn keep_glass_active(window: &WebviewWindow) {
    let window = window.clone();
    let target = window.clone();
    let _ = target.run_on_main_thread(move || {
        let Ok(handle) = window.ns_window() else {
            return;
        };
        // SAFETY: `ns_window` is a live `NSWindow`, and this closure runs on the main thread.
        let class: &AnyClass = unsafe { (*handle.cast::<AnyObject>()).class() };
        // SAFETY: `always_active` matches the `BOOL _hasActiveAppearance` signature. The call
        // is idempotent and a no-op if the class already defines its own implementation, so
        // it runs for every window in case the windows do not share one subclass.
        unsafe {
            class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                sel!(_hasActiveAppearance),
                std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel) -> Bool, Imp>(
                    always_active,
                ),
                c"B@:".as_ptr(),
            );
        }
    });
}

/// Mica is the more solid of the two Windows backdrops and Acrylic the more
/// see-through one, which is how the settings window describes them. `Regular`
/// and `Clear` preserve that relationship on macOS.
fn variant_for(material: &str) -> GlassMaterialVariant {
    if material == "mica" {
        GlassMaterialVariant::Regular
    } else {
        GlassMaterialVariant::Clear
    }
}

/// Windows strengthens its backdrop with a black tint whose alpha byte is the raw
/// blur intensity (at most 100 of 255). Scaling to the full byte range would turn
/// the glass opaque black and hide both the blur and the opacity slider.
fn tint_for(intensity: u8) -> Option<String> {
    if intensity == 0 {
        return None;
    }

    Some(format!("#000000{:02X}", intensity.min(100)))
}

#[cfg(test)]
mod tests {
    use super::{tint_for, variant_for};
    use tauri_plugin_liquid_glass::GlassMaterialVariant;

    #[test]
    fn maps_the_windows_materials_onto_glass_variants() {
        assert_eq!(variant_for("mica"), GlassMaterialVariant::Regular);
        assert_eq!(variant_for("acrylic"), GlassMaterialVariant::Clear);
    }

    #[test]
    fn uses_the_blur_intensity_as_the_tint_alpha_like_windows() {
        assert_eq!(tint_for(0), None);
        assert_eq!(tint_for(100).as_deref(), Some("#00000064"));
        assert_eq!(tint_for(50).as_deref(), Some("#00000032"));
    }

    #[test]
    fn clamps_an_out_of_range_intensity() {
        assert_eq!(tint_for(200).as_deref(), Some("#00000064"));
    }
}
