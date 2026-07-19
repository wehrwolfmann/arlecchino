//! egui theme "Тёмный (Steam)" / steam_dark — palette taken verbatim from Rexon's
//! `ui_base.py` `_SKINS["steam_dark"]["palette"]` and mapped onto egui `Visuals`
//! (QPalette roles noted per constant).

use eframe::egui::{self, Color32, CornerRadius, Stroke};

/// `Window` — main window/panel background.
pub const BG: Color32 = Color32::from_rgb(0x1b, 0x28, 0x38);
/// `Base` — input field background (extreme_bg), darker than the window.
pub const BG2: Color32 = Color32::from_rgb(0x16, 0x20, 0x2d);
/// `AlternateBase` — card/zebra-row surface.
pub const SURFACE: Color32 = Color32::from_rgb(0x1e, 0x2d, 0x3d);
/// Card surface on hover (derived).
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x26, 0x3d, 0x52);
/// `Button` — button fill and borders.
pub const BUTTON: Color32 = Color32::from_rgb(0x2a, 0x40, 0x55);
/// Button on hover (lightened Button).
pub const BUTTON_HI: Color32 = Color32::from_rgb(0x39, 0x58, 0x72);
/// `Highlight` — Steam accent blue (selection, links).
pub const ACCENT: Color32 = Color32::from_rgb(0x66, 0xc0, 0xf4);
/// `HighlightedText` — dark text over the accent.
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x0b, 0x17, 0x22);
/// Steam green (ready/confirm).
pub const GREEN: Color32 = Color32::from_rgb(0x5b, 0xa3, 0x2b);
/// `Text` / `WindowText` — primary text (white on dark).
pub const TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
/// `PlaceholderText` — dimmed text/placeholders.
pub const DIM: Color32 = Color32::from_rgb(0x9f, 0xb4, 0xc6);
/// Amber (warnings/in-progress).
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xc3, 0x00);
/// Red (dangerous actions: delete).
pub const RED: Color32 = Color32::from_rgb(0xff, 0x52, 0x52);

const RADIUS: u8 = 6;

/// Build the "Тёмный (Steam)" `egui::Visuals`.
pub fn steam_dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();

    v.panel_fill = BG; // Window
    v.window_fill = BG; // Window
    v.extreme_bg_color = BG2; // Base
    v.faint_bg_color = SURFACE; // AlternateBase
    v.window_stroke = Stroke::new(1.0_f32, BUTTON);

    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT; // Highlight
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT_TEXT); // HighlightedText
    v.hyperlink_color = ACCENT; // Link

    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS);

    let cr = CornerRadius::same(RADIUS);

    v.widgets.noninteractive.bg_fill = BG;
    v.widgets.noninteractive.weak_bg_fill = BG;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BUTTON);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.noninteractive.corner_radius = cr;

    v.widgets.inactive.bg_fill = BUTTON;
    v.widgets.inactive.weak_bg_fill = BUTTON;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BUTTON);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT); // ButtonText
    v.widgets.inactive.corner_radius = cr;

    v.widgets.hovered.bg_fill = BUTTON_HI;
    v.widgets.hovered.weak_bg_fill = BUTTON_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);
    v.widgets.hovered.corner_radius = cr;

    v.widgets.active.bg_fill = SURFACE_HI;
    v.widgets.active.weak_bg_fill = SURFACE_HI;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.corner_radius = cr;

    v.widgets.open.bg_fill = SURFACE;
    v.widgets.open.weak_bg_fill = SURFACE;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, BUTTON);
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.open.corner_radius = cr;

    v
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals(steam_dark_visuals());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_steam_dark() {
        assert_eq!(BG, Color32::from_rgb(0x1b, 0x28, 0x38));
        assert_eq!(BG2, Color32::from_rgb(0x16, 0x20, 0x2d));
        assert_eq!(SURFACE, Color32::from_rgb(0x1e, 0x2d, 0x3d));
        assert_eq!(BUTTON, Color32::from_rgb(0x2a, 0x40, 0x55));
        assert_eq!(ACCENT, Color32::from_rgb(0x66, 0xc0, 0xf4));
        assert_eq!(ACCENT_TEXT, Color32::from_rgb(0x0b, 0x17, 0x22));
        assert_eq!(DIM, Color32::from_rgb(0x9f, 0xb4, 0xc6));
    }

    #[test]
    fn visuals_apply_core_fields() {
        let v = steam_dark_visuals();
        assert_eq!(v.panel_fill, BG);
        assert_eq!(v.extreme_bg_color, BG2);
        assert_eq!(v.selection.bg_fill, ACCENT);
        assert_eq!(v.override_text_color, Some(TEXT));
        assert_eq!(v.widgets.inactive.weak_bg_fill, BUTTON);
    }
}
