use std::collections::BTreeMap;

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

// Semantic colors. Views should prefer these names so that the palette can evolve
// without coupling layout code to a specific shade.
pub const BACKGROUND: Color32 = Color32::from_rgb(11, 16, 24);
pub const BACKGROUND_DEEP: Color32 = Color32::from_rgb(9, 14, 21);
pub const SURFACE: Color32 = Color32::from_rgb(17, 25, 37);
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(21, 31, 45);
pub const SURFACE_SUNKEN: Color32 = Color32::from_rgb(13, 21, 32);
pub const SURFACE_HOVERED: Color32 = Color32::from_rgb(31, 39, 51);
pub const OUTLINE: Color32 = Color32::from_rgb(41, 53, 72);
pub const OUTLINE_STRONG: Color32 = Color32::from_rgb(148, 116, 68);
pub const BRAND: Color32 = Color32::from_rgb(217, 174, 92);
pub const BRAND_SOFT: Color32 = Color32::from_rgb(241, 217, 167);
pub const BRAND_STRONG: Color32 = Color32::from_rgb(180, 123, 46);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(243, 240, 232);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(168, 177, 191);
pub const TEXT_TERTIARY: Color32 = Color32::from_rgb(119, 132, 148);
pub const SUCCESS: Color32 = Color32::from_rgb(48, 173, 139);

// Compatibility aliases for the existing views. New UI code should use the
// semantic names above.
pub const INK: Color32 = BACKGROUND;
pub const DEEP_INK: Color32 = BACKGROUND_DEEP;
pub const PANEL: Color32 = SURFACE;
pub const CARD: Color32 = SURFACE_RAISED;
pub const FIELD: Color32 = SURFACE_SUNKEN;
pub const BORDER: Color32 = OUTLINE;
pub const ACCENT: Color32 = BRAND;
pub const ACCENT_TEXT: Color32 = BRAND_SOFT;
pub const PRIMARY: Color32 = BRAND_STRONG;
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(200, 145, 58);
pub const MUTED: Color32 = TEXT_SECONDARY;
pub const MUTED_DARK: Color32 = TEXT_TERTIARY;

pub mod spacing {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

pub mod radius {
    pub const SMALL: u8 = 8;
    pub const MEDIUM: u8 = 12;
    pub const LARGE: u8 = 18;
    pub const PILL: u8 = u8::MAX;
}

pub mod control {
    pub const HEIGHT: f32 = 40.0;
    pub const PRIMARY_HEIGHT: f32 = 52.0;
    pub const HEADER_HEIGHT: f32 = 64.0;
    pub const CONTENT_MAX_WIDTH: f32 = 1_120.0;
    pub const OUTER_MARGIN: f32 = 24.0;
    pub const RESPONSIVE_BREAKPOINT: f32 = 980.0;
}

pub mod type_scale {
    pub const SMALL: f32 = 13.0;
    pub const BODY: f32 = 15.0;
    pub const BUTTON: f32 = 15.0;
    pub const HEADING: f32 = 24.0;
    pub const HERO: f32 = 42.0;
    pub const MONOSPACE: f32 = 13.0;
}

fn text_styles() -> BTreeMap<TextStyle, FontId> {
    use type_scale::{BODY, BUTTON, HEADING, MONOSPACE, SMALL};

    [
        (
            TextStyle::Heading,
            FontId::new(HEADING, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(BODY, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(BUTTON, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(SMALL, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(MONOSPACE, FontFamily::Monospace),
        ),
    ]
    .into()
}

fn configure_style(style: &mut egui::Style) {
    style.text_styles = text_styles();
    style.spacing.item_spacing = egui::vec2(spacing::MD, spacing::MD);
    style.spacing.button_padding = egui::vec2(spacing::LG, spacing::SM);
    style.spacing.interact_size.y = control::HEIGHT;
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = BACKGROUND_DEEP;
    style.visuals.faint_bg_color = SURFACE_SUNKEN;
    style.visuals.window_corner_radius = radius::LARGE.into();
    style.visuals.menu_corner_radius = radius::MEDIUM.into();
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, OUTLINE);
    style.visuals.widgets.noninteractive.bg_fill = SURFACE_RAISED;
    style.visuals.widgets.noninteractive.fg_stroke.color = TEXT_SECONDARY;
    style.visuals.widgets.inactive.bg_fill = SURFACE_SUNKEN;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE_SUNKEN;
    style.visuals.widgets.inactive.bg_stroke.color = OUTLINE;
    style.visuals.widgets.inactive.fg_stroke.color = TEXT_PRIMARY;
    style.visuals.widgets.hovered.bg_fill = SURFACE_HOVERED;
    style.visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVERED;
    style.visuals.widgets.hovered.bg_stroke.color = OUTLINE_STRONG;
    style.visuals.widgets.hovered.fg_stroke.color = BRAND_SOFT;
    style.visuals.widgets.active.bg_fill = BRAND_STRONG;
    style.visuals.widgets.active.weak_bg_fill = BRAND_STRONG;
    style.visuals.widgets.active.bg_stroke.color = BRAND_SOFT;
    style.visuals.widgets.active.fg_stroke.color = TEXT_PRIMARY;
    style.visuals.widgets.noninteractive.corner_radius = radius::SMALL.into();
    style.visuals.widgets.inactive.corner_radius = radius::SMALL.into();
    style.visuals.widgets.hovered.corner_radius = radius::SMALL.into();
    style.visuals.widgets.active.corner_radius = radius::SMALL.into();
    style.visuals.selection.bg_fill = BRAND_STRONG;
    style.visuals.selection.stroke.color = BRAND_SOFT;
    style.visuals.hyperlink_color = BRAND;
}

pub(crate) fn configure_ui(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let families = [
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "Microsoft YaHei",
        "MiSans",
        "WenQuanYi Micro Hei",
        "Sarasa Gothic SC",
        "LXGW WenKai",
        "SimHei",
        "PingFang SC",
    ];
    let mut found = false;
    for family in families {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            ..Default::default()
        };
        if let Some(id) = database.query(&query)
            && let Some(data) = database.with_face_data(id, |bytes, index| {
                let mut font = egui::FontData::from_owned(bytes.to_vec());
                font.index = index;
                font
            })
        {
            fonts.font_data.insert("zm-cjk".into(), data.into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("zm-cjk".into());
            }
            found = true;
            break;
        }
    }
    if !found {
        tracing::warn!("未找到中文字体，请安装 Noto Sans CJK SC 或微软雅黑");
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    configure_style(&mut style);
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_style_uses_shared_design_tokens() {
        let mut style = egui::Style::default();
        configure_style(&mut style);

        assert_eq!(
            style.spacing.item_spacing,
            egui::vec2(spacing::MD, spacing::MD)
        );
        assert_eq!(style.spacing.interact_size.y, control::HEIGHT);
        assert_eq!(style.visuals.panel_fill, BACKGROUND);
        assert_eq!(style.visuals.window_fill, SURFACE);
        assert_eq!(style.visuals.widgets.hovered.bg_fill, SURFACE_HOVERED);
        assert_eq!(
            TextStyle::Body.resolve(&style),
            FontId::new(type_scale::BODY, FontFamily::Proportional)
        );
        assert_eq!(
            TextStyle::Monospace.resolve(&style),
            FontId::new(type_scale::MONOSPACE, FontFamily::Monospace)
        );
    }
}
