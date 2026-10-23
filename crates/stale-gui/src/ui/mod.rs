//! egui drawing code for the main window.

use eframe::egui;

pub mod details;
pub mod filter_panel;
pub mod ghosts_view;
pub mod leftover_popup;
pub mod leftovers_view;
pub mod quarantine_view;
pub mod table;
pub mod toolbar;
pub mod uninstall_dialog;

/// Warning colour (last-used text of rows unused for more than six months, risky leftovers,
/// failures) for the current theme.
pub fn stale_color(visuals: &egui::Visuals) -> egui::Color32 {
    stale_color_for(visuals.dark_mode)
}

/// The warning colour for a dark or light theme; both keep at least 4.5:1 contrast to the panel.
pub fn stale_color_for(dark: bool) -> egui::Color32 {
    if dark { egui::Color32::from_rgb(255, 128, 82) } else { egui::Color32::from_rgb(178, 58, 0) }
}

/// Normal (non-interactive) text: egui's defaults (grey 140 on dark, 80 on light) are hard to read
/// in a dense table, so labels get near-white on dark and near-black on light.
pub const TEXT_DARK: egui::Color32 = egui::Color32::from_gray(225);
pub const TEXT_LIGHT: egui::Color32 = egui::Color32::from_gray(25);

/// Raises the text contrast of both themes. Called once at start-up.
pub fn apply_style(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals.widgets.noninteractive.fg_stroke.color = TEXT_DARK;
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals.widgets.noninteractive.fg_stroke.color = TEXT_LIGHT;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2 contrast ratio between two opaque colours.
    fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
        fn lum(c: egui::Color32) -> f32 {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        let (x, y) = (lum(a) + 0.05, lum(b) + 0.05);
        if x > y { x / y } else { y / x }
    }

    #[test]
    fn table_text_and_warning_colour_are_readable_in_both_themes() {
        for (dark, visuals, text) in [(true, egui::Visuals::dark(), TEXT_DARK), (false, egui::Visuals::light(), TEXT_LIGHT)] {
            // The opaque backgrounds text sits on; the row stripe (`faint_bg_color`) is a nearly
            // transparent overlay on the panel, so the panel covers it.
            for bg in [visuals.panel_fill, visuals.window_fill, visuals.extreme_bg_color] {
                assert!(contrast(text, bg) >= 7.0, "text dark={dark} bg={bg:?}: {}", contrast(text, bg));
                assert!(contrast(stale_color_for(dark), bg) >= 4.5, "warning dark={dark} bg={bg:?}: {}", contrast(stale_color_for(dark), bg));
            }
        }
    }

    #[test]
    fn apply_style_sets_label_text_for_both_themes() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let mut seen = Vec::new();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| seen.push(s.visuals.widgets.noninteractive.fg_stroke.color));
        }
        assert_eq!(seen, vec![TEXT_DARK, TEXT_LIGHT]);
    }
}
