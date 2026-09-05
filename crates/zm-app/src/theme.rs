use eframe::egui;

use eframe::egui::Color32;

pub const INK: Color32 = Color32::from_rgb(15, 20, 30);
pub const DEEP_INK: Color32 = Color32::from_rgb(11, 15, 23);
pub const PANEL: Color32 = Color32::from_rgb(22, 29, 42);
pub const CARD: Color32 = Color32::from_rgb(28, 37, 53);
pub const FIELD: Color32 = Color32::from_rgb(18, 25, 37);
pub const BORDER: Color32 = Color32::from_rgb(56, 70, 92);
pub const ACCENT: Color32 = Color32::from_rgb(111, 177, 255);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(184, 216, 255);
pub const TEXT: Color32 = Color32::from_rgb(235, 241, 250);
pub const PRIMARY: Color32 = Color32::from_rgb(44, 105, 192);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(55, 124, 220);
pub const SUCCESS: Color32 = Color32::from_rgb(48, 173, 139);
pub const MUTED: Color32 = Color32::from_rgb(163, 178, 200);
pub const MUTED_DARK: Color32 = Color32::from_rgb(119, 139, 164);

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
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.interact_size.y = 38.0;
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = INK;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = DEEP_INK;
    style.visuals.faint_bg_color = FIELD;
    style.visuals.widgets.noninteractive.bg_fill = CARD;
    style.visuals.widgets.noninteractive.fg_stroke.color = MUTED;
    style.visuals.widgets.inactive.bg_fill = FIELD;
    style.visuals.widgets.inactive.weak_bg_fill = FIELD;
    style.visuals.widgets.inactive.bg_stroke.color = BORDER;
    style.visuals.widgets.inactive.fg_stroke.color = TEXT;
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(38, 54, 77);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(38, 54, 77);
    style.visuals.widgets.hovered.bg_stroke.color = ACCENT;
    style.visuals.widgets.hovered.fg_stroke.color = ACCENT_TEXT;
    style.visuals.widgets.active.bg_fill = PRIMARY;
    style.visuals.widgets.active.weak_bg_fill = PRIMARY;
    style.visuals.widgets.active.bg_stroke.color = ACCENT_TEXT;
    style.visuals.widgets.active.fg_stroke.color = TEXT;
    style.visuals.widgets.noninteractive.corner_radius = 9.into();
    style.visuals.widgets.inactive.corner_radius = 9.into();
    style.visuals.widgets.hovered.corner_radius = 9.into();
    style.visuals.widgets.active.corner_radius = 9.into();
    style.visuals.selection.bg_fill = PRIMARY;
    style.visuals.selection.stroke.color = ACCENT_TEXT;
    style.visuals.hyperlink_color = ACCENT;
    ctx.set_style_of(egui::Theme::Dark, style);
}
