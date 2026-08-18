//! macOS overlay backdrop.
//!
//! Windows composes the overlay with Mica or Acrylic. macOS 26 exposes Liquid
//! Glass, and the plugin used here falls back to `NSVisualEffectView` on earlier
//! versions, so both settings stay meaningful across supported releases.

use tauri::{Manager, WebviewWindow};
use tauri_plugin_liquid_glass::{GlassMaterialVariant, LiquidGlassConfig, LiquidGlassExt};

/// The overlay is drawn square by `styles.css`, so the glass layer must not round
/// its own corners.
const OVERLAY_CORNER_RADIUS: f64 = 0.0;

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
        .map_err(|error| error.to_string())
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

/// Windows strengthens its backdrop with a black tint whose alpha is the blur
/// intensity. The same tint keeps the slider's effect consistent across platforms.
fn tint_for(intensity: u8) -> Option<String> {
    if intensity == 0 {
        return None;
    }

    let alpha = (f64::from(intensity.min(100)) / 100.0 * 255.0).round() as u8;
    Some(format!("#000000{alpha:02X}"))
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
    fn scales_the_blur_intensity_into_a_tint_alpha() {
        assert_eq!(tint_for(0), None);
        assert_eq!(tint_for(100).as_deref(), Some("#000000FF"));
        assert_eq!(tint_for(50).as_deref(), Some("#00000080"));
    }

    #[test]
    fn clamps_an_out_of_range_intensity() {
        assert_eq!(tint_for(200).as_deref(), Some("#000000FF"));
    }
}
