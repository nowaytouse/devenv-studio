use crate::prefs::{Accent, Appearance, Density, Preferences};
use eframe::egui::{self, Color32, FontId, Stroke, Vec2};
use std::cell::Cell;

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub accent: Color32,
    pub wash: Color32,
    pub danger: Color32,
    pub on_accent: Color32,
}
impl Palette {
    pub fn for_theme(dark: bool, accent: Accent) -> Self {
        let rgb = match (dark, accent) {
            (false, Accent::Blue) => [49, 93, 229],
            (false, Accent::Teal) => [8, 127, 121],
            (false, Accent::Violet) => [121, 80, 201],
            (false, Accent::Rose) => [182, 64, 107],
            (false, Accent::Amber) => [152, 99, 0],
            (true, Accent::Blue) => [125, 162, 255],
            (true, Accent::Teal) => [75, 209, 189],
            (true, Accent::Violet) => [185, 154, 255],
            (true, Accent::Rose) => [255, 150, 179],
            (true, Accent::Amber) => [237, 197, 111],
        };
        let surface = if dark { [28, 36, 51] } else { [255, 255, 255] };
        let wash = Color32::from_rgb(
            ((surface[0] as u16 * 9 + rgb[0] as u16) / 10) as u8,
            ((surface[1] as u16 * 9 + rgb[1] as u16) / 10) as u8,
            ((surface[2] as u16 * 9 + rgb[2] as u16) / 10) as u8,
        );
        Self {
            canvas: if dark {
                Color32::from_rgb(21, 27, 40)
            } else {
                Color32::from_rgb(243, 246, 250)
            },
            surface: Color32::from_rgb(surface[0], surface[1], surface[2]),
            ink: if dark {
                Color32::from_rgb(227, 233, 244)
            } else {
                Color32::from_rgb(32, 45, 72)
            },
            muted: if dark {
                Color32::from_rgb(169, 180, 200)
            } else {
                Color32::from_rgb(91, 106, 130)
            },
            line: if dark {
                Color32::from_rgb(53, 65, 85)
            } else {
                Color32::from_rgb(217, 225, 237)
            },
            accent: Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            wash,
            danger: if dark {
                Color32::from_rgb(255, 157, 165)
            } else {
                Color32::from_rgb(173, 58, 67)
            },
            on_accent: if dark {
                Color32::from_rgb(19, 28, 45)
            } else {
                Color32::WHITE
            },
        }
    }
}
thread_local! { static COLORS: Cell<Palette> = Cell::new(Palette::for_theme(false, Accent::Blue)); }
pub(super) fn palette() -> Palette {
    COLORS.with(Cell::get)
}

pub(super) fn apply(ctx: &egui::Context, preferences: &Preferences) {
    ctx.set_theme(match preferences.appearance {
        Appearance::System => egui::ThemePreference::System,
        Appearance::Light => egui::ThemePreference::Light,
        Appearance::Dark => egui::ThemePreference::Dark,
    });
    let dark = ctx.theme() == egui::Theme::Dark;
    let colors = Palette::for_theme(dark, preferences.accent);
    COLORS.with(|value| value.set(colors));
    let mut style = (*ctx.style_of(ctx.theme())).clone();
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = colors.canvas;
    style.visuals.window_fill = colors.surface;
    style.visuals.extreme_bg_color = colors.surface;
    style.visuals.faint_bg_color = colors.wash;
    style.visuals.override_text_color = Some(colors.ink);
    style.visuals.widgets.inactive.bg_fill = colors.surface;
    style.visuals.widgets.inactive.weak_bg_fill = colors.surface;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, colors.line);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, colors.ink);
    style.visuals.widgets.hovered.bg_fill = colors.wash;
    style.visuals.widgets.hovered.weak_bg_fill = colors.wash;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, colors.ink);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, colors.accent);
    style.visuals.widgets.active.bg_fill = colors.wash;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, colors.accent);
    style.visuals.selection.bg_fill = colors.accent;
    style.visuals.selection.stroke = Stroke::new(1.0, colors.on_accent);
    style.visuals.hyperlink_color = colors.accent;
    let compact = matches!(preferences.density, Density::Compact);
    style.spacing.item_spacing = Vec2::new(10.0, if compact { 6.0 } else { 10.0 });
    style.spacing.button_padding = Vec2::new(12.0, if compact { 6.0 } else { 9.0 });
    style.spacing.interact_size.y = if compact { 30.0 } else { 36.0 };
    for (kind, size) in [
        (egui::TextStyle::Body, 14.0),
        (egui::TextStyle::Button, 14.0),
        (egui::TextStyle::Small, 12.0),
        (egui::TextStyle::Heading, 24.0),
    ] {
        style.text_styles.insert(kind, FontId::proportional(size));
    }
    ctx.set_global_style(style);
    ctx.set_zoom_factor(preferences.scale_percent as f32 / 100.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(color: Color32) -> f32 {
        let values = [color.r(), color.g(), color.b()].map(|value| {
            let value = value as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722
    }
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn every_theme_accent_has_readable_controls_and_body_text() {
        for dark in [false, true] {
            for accent in [
                Accent::Blue,
                Accent::Teal,
                Accent::Violet,
                Accent::Rose,
                Accent::Amber,
            ] {
                let p = Palette::for_theme(dark, accent);
                assert!(contrast(p.accent, p.on_accent) >= 4.5);
                assert!(contrast(p.accent, p.surface) >= 4.5);
                assert!(contrast(p.ink, p.canvas) >= 7.0);
                assert!(contrast(p.muted, p.surface) >= 4.5);
            }
        }
    }
}
