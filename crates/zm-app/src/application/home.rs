use super::*;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Vec2,
};
use egui::epaint::{CubicBezierShape, RectShape};

use crate::theme as palette;

pub(super) const CONTENT_MAX_WIDTH: f32 = palette::control::CONTENT_MAX_WIDTH;
pub(super) const OUTER_MARGIN: f32 = palette::control::OUTER_MARGIN;
pub(super) const COLUMN_GAP: f32 = 20.0;

const WIDE_HOME_HEIGHT: f32 = 512.0;
const COMPACT_STAGE_HEIGHT: f32 = 264.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HomeColumns {
    pub(super) stage: f32,
    pub(super) launch: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutClass {
    Wide,
    Compact,
}

pub(super) fn layout_class(width: f32) -> LayoutClass {
    if width >= palette::control::RESPONSIVE_BREAKPOINT {
        LayoutClass::Wide
    } else {
        LayoutClass::Compact
    }
}

pub(super) fn content_width(viewport_width: f32) -> f32 {
    if !viewport_width.is_finite() {
        return 320.0;
    }
    (viewport_width - OUTER_MARGIN * 2.0).clamp(320.0, CONTENT_MAX_WIDTH)
}

pub(super) fn wide_columns(width: f32) -> HomeColumns {
    let usable = (width - COLUMN_GAP).max(0.0);
    let stage = (usable * 0.60).floor();
    HomeColumns {
        stage,
        launch: usable - stage,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GamePresentation {
    pub(super) series_label: &'static str,
    pub(super) tagline: &'static str,
    pub(super) description: &'static str,
    pub(super) accent: Color32,
    pub(super) accent_soft: Color32,
    pub(super) button_text: Color32,
    pub(super) backdrop_top: Color32,
    pub(super) backdrop_bottom: Color32,
    pub(super) mountain_far: Color32,
    pub(super) mountain_mid: Color32,
    pub(super) river: Color32,
}

impl GamePresentation {
    pub(super) const fn for_game(game: GameKind) -> Self {
        match game {
            GameKind::Zm4 => Self {
                series_label: "洪荒大劫篇",
                tagline: "踏云入洪荒，再续西游之路",
                description: "经典横版冒险，与熟悉的伙伴并肩迎战。",
                accent: Color32::from_rgb(237, 182, 79),
                accent_soft: Color32::from_rgb(244, 204, 119),
                button_text: Color32::from_rgb(48, 31, 20),
                backdrop_top: Color32::from_rgb(43, 27, 33),
                backdrop_bottom: Color32::from_rgb(16, 23, 33),
                mountain_far: Color32::from_rgb(42, 39, 48),
                mountain_mid: Color32::from_rgb(23, 28, 37),
                river: Color32::from_rgb(99, 143, 145),
            },
            GameKind::Zm5 => Self {
                series_label: "上古天帝篇",
                tagline: "穿越云海，探寻上古天境",
                description: "在辽阔仙境中历练成长，开启你的天帝冒险。",
                accent: Color32::from_rgb(86, 194, 200),
                accent_soft: Color32::from_rgb(145, 224, 226),
                button_text: Color32::from_rgb(11, 38, 42),
                backdrop_top: Color32::from_rgb(18, 54, 65),
                backdrop_bottom: Color32::from_rgb(13, 25, 35),
                mountain_far: Color32::from_rgb(31, 55, 62),
                mountain_mid: Color32::from_rgb(20, 39, 48),
                river: Color32::from_rgb(89, 173, 177),
            },
        }
    }
}

fn color_with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn mix_color(from: Color32, to: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |start: u8, end: u8| {
        (f32::from(start) + (f32::from(end) - f32::from(start)) * amount).round() as u8
    };
    Color32::from_rgba_unmultiplied(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
        channel(from.a(), to.a()),
    )
}

fn paint_card_shadow(ui: &egui::Ui, rect: Rect) {
    ui.painter().add(
        RectShape::filled(
            rect.translate(Vec2::new(0.0, 7.0)),
            palette::radius::LARGE,
            Color32::from_black_alpha(76),
        )
        .with_blur_width(16.0),
    );
}

fn paint_vertical_gradient(
    painter: &egui::Painter,
    rect: Rect,
    top: Color32,
    bottom: Color32,
    radius: u8,
) {
    const STEPS: usize = 24;
    for step in 0..STEPS {
        let start = step as f32 / STEPS as f32;
        let end = (step + 1) as f32 / STEPS as f32;
        let strip = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + rect.height() * start),
            Pos2::new(
                rect.right(),
                (rect.top() + rect.height() * end + 0.5).min(rect.bottom()),
            ),
        );
        let corner_radius = if step == 0 {
            CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            }
        } else if step + 1 == STEPS {
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius,
                se: radius,
            }
        } else {
            CornerRadius::ZERO
        };
        painter.rect_filled(
            strip,
            corner_radius,
            mix_color(top, bottom, (start + end) * 0.5),
        );
    }
}

fn paint_cubic(painter: &egui::Painter, points: [Pos2; 4], color: Color32, width: f32) {
    painter.add(CubicBezierShape::from_points_stroke(
        points,
        false,
        Color32::TRANSPARENT,
        Stroke::new(width, color),
    ));
}

fn paint_home_atmosphere(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    paint_cubic(
        painter,
        [
            Pos2::new(rect.left() + 40.0, rect.top() + 100.0),
            Pos2::new(rect.left() + rect.width() * 0.18, rect.top() + 30.0),
            Pos2::new(rect.left() + rect.width() * 0.33, rect.top() + 165.0),
            Pos2::new(rect.left() + rect.width() * 0.50, rect.top() + 82.0),
        ],
        color_with_alpha(palette::BRAND, 9),
        1.5,
    );
    paint_cubic(
        painter,
        [
            Pos2::new(rect.left() + rect.width() * 0.62, rect.top() + 70.0),
            Pos2::new(rect.left() + rect.width() * 0.74, rect.top() + 20.0),
            Pos2::new(rect.left() + rect.width() * 0.86, rect.top() + 150.0),
            Pos2::new(rect.right() - 20.0, rect.top() + 84.0),
        ],
        Color32::from_rgba_unmultiplied(112, 188, 193, 8),
        1.5,
    );
}

fn paint_brand_mark(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.46;
    painter.circle_filled(center, radius, Color32::from_rgb(22, 29, 38));
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(1.2_f32, color_with_alpha(palette::BRAND, 120)),
    );
    painter.circle_stroke(center, radius * 0.52, Stroke::new(1.6_f32, palette::BRAND));
    let left = rect.left() + 7.0;
    let right = rect.right() - 6.0;
    let mid_y = center.y + 3.0;
    paint_cubic(
        &painter,
        [
            Pos2::new(left, mid_y),
            Pos2::new(left + 8.0, mid_y - 8.0),
            Pos2::new(center.x + 3.0, mid_y + 6.0),
            Pos2::new(right, mid_y - 2.0),
        ],
        palette::BRAND_SOFT,
        1.4,
    );
    paint_cubic(
        &painter,
        [
            Pos2::new(left + 3.0, mid_y + 7.0),
            Pos2::new(left + 11.0, mid_y + 1.0),
            Pos2::new(center.x + 6.0, mid_y + 11.0),
            Pos2::new(right - 2.0, mid_y + 5.0),
        ],
        color_with_alpha(palette::BRAND_SOFT, 205),
        1.25,
    );
    painter.circle_filled(
        center + Vec2::new(radius * 0.35, -radius * 0.43),
        2.0,
        Color32::from_rgb(198, 92, 60),
    );
}

fn paint_mountain_triangle(
    painter: &egui::Painter,
    rect: Rect,
    center_x: f32,
    peak_y: f32,
    half_width: f32,
    base_y: f32,
    color: Color32,
) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(rect.left() + rect.width() * (center_x - half_width), base_y),
            Pos2::new(
                rect.left() + rect.width() * center_x,
                rect.top() + rect.height() * peak_y,
            ),
            Pos2::new(rect.left() + rect.width() * (center_x + half_width), base_y),
        ],
        color,
        Stroke::NONE,
    ));
}

fn paint_mountain_layer(
    painter: &egui::Painter,
    rect: Rect,
    base_ratio: f32,
    peaks: &[(f32, f32, f32)],
    color: Color32,
    highlight: Color32,
) {
    let base_y = rect.top() + rect.height() * base_ratio;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.left(), base_y - 0.5), rect.max),
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: palette::radius::LARGE,
            se: palette::radius::LARGE,
        },
        color,
    );
    for &(center, peak, half_width) in peaks {
        paint_mountain_triangle(painter, rect, center, peak, half_width, base_y, color);
        let peak_point = Pos2::new(
            rect.left() + rect.width() * center,
            rect.top() + rect.height() * peak,
        );
        painter.line_segment(
            [
                peak_point,
                Pos2::new(rect.left() + rect.width() * (center + half_width), base_y),
            ],
            Stroke::new(1.0_f32, highlight),
        );
    }
}

fn paint_sun(painter: &egui::Painter, center: Pos2, radius: f32, presentation: GamePresentation) {
    painter.circle_filled(
        center,
        radius * 1.65,
        color_with_alpha(presentation.accent, 11),
    );
    const RINGS: usize = 14;
    for index in 0..RINGS {
        let t = index as f32 / (RINGS - 1) as f32;
        painter.circle_filled(
            center,
            radius * (1.0 - t * 0.68),
            color_with_alpha(
                mix_color(presentation.accent, presentation.accent_soft, t * 0.72),
                (48.0 + t * 74.0) as u8,
            ),
        );
    }
    painter.circle_stroke(
        center,
        radius * 1.18,
        Stroke::new(1.0_f32, color_with_alpha(presentation.accent_soft, 46)),
    );
    painter.circle_stroke(
        center,
        radius * 1.42,
        Stroke::new(1.0_f32, color_with_alpha(presentation.accent, 24)),
    );
}

fn paint_game_backdrop(ui: &egui::Ui, rect: Rect, game: GameKind, presentation: GamePresentation) {
    let painter = ui.painter_at(rect);
    paint_vertical_gradient(
        &painter,
        rect,
        presentation.backdrop_top,
        presentation.backdrop_bottom,
        palette::radius::LARGE,
    );

    let width = rect.width();
    let height = rect.height();
    let compact = height < 360.0;
    let sun = Pos2::new(
        rect.left() + width * 0.78,
        rect.top() + height * if compact { 0.39 } else { 0.27 },
    );
    paint_sun(
        &painter,
        sun,
        width.min(height) * if compact { 0.12 } else { 0.115 },
        presentation,
    );

    for (x, y, radius) in [
        (0.63, 0.12, 1.8),
        (0.88, 0.16, 1.4),
        (0.70, 0.38, 1.2),
        (0.92, 0.43, 1.8),
    ] {
        painter.circle_filled(
            Pos2::new(rect.left() + width * x, rect.top() + height * y),
            radius,
            color_with_alpha(presentation.accent_soft, 105),
        );
    }

    let cloud = color_with_alpha(presentation.accent_soft, 28);
    paint_cubic(
        &painter,
        [
            Pos2::new(rect.left() + width * 0.54, rect.top() + height * 0.19),
            Pos2::new(rect.left() + width * 0.59, rect.top() + height * 0.11),
            Pos2::new(rect.left() + width * 0.64, rect.top() + height * 0.29),
            Pos2::new(rect.left() + width * 0.70, rect.top() + height * 0.20),
        ],
        cloud,
        1.35,
    );
    paint_cubic(
        &painter,
        [
            Pos2::new(rect.left() + width * 0.64, rect.top() + height * 0.21),
            Pos2::new(rect.left() + width * 0.69, rect.top() + height * 0.13),
            Pos2::new(rect.left() + width * 0.73, rect.top() + height * 0.28),
            Pos2::new(rect.left() + width * 0.80, rect.top() + height * 0.22),
        ],
        cloud,
        1.35,
    );
    paint_cubic(
        &painter,
        [
            Pos2::new(rect.left() + width * 0.08, rect.top() + height * 0.57),
            Pos2::new(rect.left() + width * 0.15, rect.top() + height * 0.44),
            Pos2::new(rect.left() + width * 0.23, rect.top() + height * 0.69),
            Pos2::new(rect.left() + width * 0.31, rect.top() + height * 0.58),
        ],
        color_with_alpha(presentation.accent, 19),
        1.4,
    );

    paint_mountain_layer(
        &painter,
        rect,
        0.79,
        &[(0.18, 0.52, 0.24), (0.48, 0.40, 0.30), (0.80, 0.57, 0.28)],
        presentation.mountain_far,
        color_with_alpha(presentation.accent, 12),
    );
    paint_mountain_layer(
        &painter,
        rect,
        0.90,
        &[(0.22, 0.63, 0.29), (0.58, 0.57, 0.34), (0.87, 0.64, 0.28)],
        presentation.mountain_mid,
        color_with_alpha(presentation.accent_soft, 13),
    );
    paint_mountain_layer(
        &painter,
        rect,
        0.97,
        &[(0.16, 0.71, 0.30), (0.51, 0.67, 0.38), (0.80, 0.73, 0.31)],
        palette::BACKGROUND_DEEP,
        color_with_alpha(presentation.accent, 10),
    );

    paint_cubic(
        &painter,
        [
            Pos2::new(rect.left() + width * 0.65, rect.top() + height * 0.68),
            Pos2::new(rect.left() + width * 0.74, rect.top() + height * 0.75),
            Pos2::new(rect.left() + width * 0.78, rect.top() + height * 0.88),
            Pos2::new(rect.left() + width * 0.73, rect.bottom()),
        ],
        color_with_alpha(presentation.river, 38),
        18.0,
    );
    paint_cubic(
        &painter,
        [
            Pos2::new(rect.left() + width * 0.66, rect.top() + height * 0.68),
            Pos2::new(rect.left() + width * 0.73, rect.top() + height * 0.75),
            Pos2::new(rect.left() + width * 0.75, rect.top() + height * 0.82),
            Pos2::new(rect.left() + width * 0.74, rect.top() + height * 0.88),
        ],
        color_with_alpha(presentation.accent_soft, 22),
        1.2,
    );

    for band in 0..8 {
        let t = band as f32 / 8.0;
        let band_rect = Rect::from_min_max(
            Pos2::new(rect.left() + width * t * 0.075, rect.top()),
            Pos2::new(rect.left() + width * (0.13 + t * 0.075), rect.bottom()),
        );
        painter.rect_filled(
            band_rect,
            CornerRadius::ZERO,
            Color32::from_black_alpha((20.0 * (1.0 - t)) as u8),
        );
    }

    painter.rect_stroke(
        rect.shrink(0.5),
        palette::radius::LARGE,
        Stroke::new(1.0_f32, color_with_alpha(presentation.accent, 54)),
        StrokeKind::Inside,
    );

    if game == GameKind::Zm5 {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 1.0, rect.top() + 54.0),
                Pos2::new(rect.left() + 1.0, rect.top() + 148.0),
            ],
            Stroke::new(2.0_f32, color_with_alpha(presentation.accent, 120)),
        );
    }
}

fn game_switcher(ui: &mut egui::Ui, stage_rect: Rect, selected_game: GameKind) -> Option<GameKind> {
    let compact = stage_rect.height() < 360.0;
    let dock_height = if compact { 58.0 } else { 68.0 };
    let dock_rect = Rect::from_min_max(
        Pos2::new(
            stage_rect.left() + 20.0,
            stage_rect.bottom() - dock_height - 18.0,
        ),
        Pos2::new(stage_rect.right() - 20.0, stage_rect.bottom() - 18.0),
    );
    let painter = ui.painter_at(stage_rect);
    painter.rect_filled(
        dock_rect,
        palette::radius::MEDIUM,
        Color32::from_rgba_unmultiplied(8, 13, 20, 234),
    );
    painter.rect_stroke(
        dock_rect,
        palette::radius::MEDIUM,
        Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
        StrokeKind::Inside,
    );

    let inner = dock_rect.shrink(6.0);
    let tab_width = inner.width() * 0.5;
    let mut requested = None;
    for (index, game) in [GameKind::Zm4, GameKind::Zm5].into_iter().enumerate() {
        let presentation = GamePresentation::for_game(game);
        let tab_rect = Rect::from_min_max(
            Pos2::new(inner.left() + tab_width * index as f32, inner.top()),
            Pos2::new(
                inner.left() + tab_width * (index + 1) as f32,
                inner.bottom(),
            ),
        )
        .shrink2(Vec2::new(2.0, 0.0));
        let response = ui
            .interact(
                tab_rect,
                ui.id().with(("game-switch", game)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let selected = game == selected_game;
        if selected || response.hovered() {
            painter.rect_filled(
                tab_rect,
                palette::radius::SMALL,
                color_with_alpha(presentation.accent, if selected { 29 } else { 13 }),
            );
        }
        if selected {
            painter.rect_filled(
                Rect::from_min_max(
                    tab_rect.left_top(),
                    Pos2::new(tab_rect.left() + 3.0, tab_rect.bottom()),
                ),
                2.0,
                presentation.accent,
            );
        }

        let number_x = tab_rect.left() + 22.0;
        painter.text(
            Pos2::new(number_x, tab_rect.center().y),
            Align2::LEFT_CENTER,
            format!("{:02}", game.number()),
            FontId::proportional(if compact { 18.0 } else { 21.0 }),
            color_with_alpha(presentation.accent_soft, if selected { 255 } else { 205 }),
        );
        let text_x = tab_rect.left() + if compact { 60.0 } else { 68.0 };
        painter.text(
            Pos2::new(
                text_x,
                tab_rect.center().y - if compact { 7.0 } else { 9.0 },
            ),
            Align2::LEFT_CENTER,
            presentation.series_label,
            FontId::proportional(if compact { 13.0 } else { 14.0 }),
            palette::TEXT_PRIMARY,
        );
        painter.text(
            Pos2::new(
                text_x,
                tab_rect.center().y + if compact { 9.0 } else { 11.0 },
            ),
            Align2::LEFT_CENTER,
            if selected {
                "当前游戏"
            } else {
                "点击切换"
            },
            FontId::proportional(11.0),
            palette::TEXT_TERTIARY,
        );
        if selected {
            painter.circle_filled(
                Pos2::new(tab_rect.right() - 22.0, tab_rect.center().y),
                3.5,
                presentation.accent,
            );
        } else {
            let center = Pos2::new(tab_rect.right() - 22.0, tab_rect.center().y);
            painter.line_segment(
                [center + Vec2::new(-3.0, -5.0), center + Vec2::new(3.0, 0.0)],
                Stroke::new(1.5_f32, palette::TEXT_TERTIARY),
            );
            painter.line_segment(
                [center + Vec2::new(3.0, 0.0), center + Vec2::new(-3.0, 5.0)],
                Stroke::new(1.5_f32, palette::TEXT_TERTIARY),
            );
        }
        if response.clicked() && !selected {
            requested = Some(game);
        }
    }
    requested
}

fn paint_game_title(ui: &egui::Ui, rect: Rect, game: GameKind, presentation: GamePresentation) {
    let painter = ui.painter_at(rect);
    let compact = rect.height() < 360.0;
    let left = rect.left() + if compact { 22.0 } else { 28.0 };
    let top = rect.top() + if compact { 20.0 } else { 30.0 };
    let pill_height = 28.0;
    let pill_width = if compact { 138.0 } else { 154.0 };
    let pill = Rect::from_min_size(Pos2::new(left, top), Vec2::new(pill_width, pill_height));
    painter.rect_filled(
        pill,
        palette::radius::PILL,
        color_with_alpha(presentation.accent, 24),
    );
    painter.rect_stroke(
        pill,
        palette::radius::PILL,
        Stroke::new(1.0_f32, color_with_alpha(presentation.accent, 70)),
        StrokeKind::Inside,
    );
    painter.circle_filled(
        Pos2::new(pill.left() + 16.0, pill.center().y),
        3.0,
        presentation.accent,
    );
    painter.text(
        Pos2::new(pill.left() + 28.0, pill.center().y),
        Align2::LEFT_CENTER,
        format!("{:02} · {}", game.number(), presentation.series_label),
        FontId::proportional(if compact { 12.0 } else { 13.0 }),
        presentation.accent,
    );

    let title_pos = Pos2::new(left, pill.bottom() + if compact { 18.0 } else { 23.0 });
    let title_font = FontId::proportional(if compact {
        34.0
    } else {
        palette::type_scale::HERO
    });
    let number_font = FontId::proportional(if compact { 39.0 } else { 48.0 });
    let title_rect = painter.text(
        title_pos,
        Align2::LEFT_TOP,
        "造梦西游",
        title_font,
        palette::TEXT_PRIMARY,
    );
    painter.text(
        Pos2::new(
            title_rect.right() + 12.0,
            title_pos.y - if compact { 3.0 } else { 5.0 },
        ),
        Align2::LEFT_TOP,
        game.number(),
        number_font,
        presentation.accent,
    );
    painter.line_segment(
        [
            Pos2::new(left, title_rect.bottom() + 12.0),
            Pos2::new(left + 42.0, title_rect.bottom() + 12.0),
        ],
        Stroke::new(3.0_f32, presentation.accent),
    );

    painter.text(
        Pos2::new(
            left,
            title_rect.bottom() + if compact { 29.0 } else { 45.0 },
        ),
        Align2::LEFT_TOP,
        presentation.tagline,
        FontId::proportional(if compact { 15.0 } else { 17.0 }),
        presentation.accent_soft,
    );
    if !compact {
        painter.text(
            Pos2::new(left, title_rect.bottom() + 82.0),
            Align2::LEFT_TOP,
            presentation.description,
            FontId::proportional(13.5),
            Color32::from_white_alpha(184),
        );
    }
}

fn theme_badge(ui: &mut egui::Ui, game: GameKind, presentation: GamePresentation) {
    egui::Frame::new()
        .fill(color_with_alpha(presentation.accent, 18))
        .stroke(Stroke::new(
            1.0_f32,
            color_with_alpha(presentation.accent, 55),
        ))
        .corner_radius(palette::radius::PILL)
        .inner_margin(egui::Margin::symmetric(13, 5))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("造梦西游 {}", game.number()))
                    .size(12.5)
                    .strong()
                    .color(presentation.accent),
            );
        });
}

fn toggle(ui: &mut egui::Ui, value: &mut bool, accent: Color32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(38.0, 22.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response = response.on_hover_cursor(CursorIcon::PointingHand);
    let fill = if *value {
        if response.hovered() {
            mix_color(accent, Color32::WHITE, 0.12)
        } else {
            accent
        }
    } else {
        palette::OUTLINE
    };
    ui.painter().rect_filled(rect, palette::radius::PILL, fill);
    let knob_x = if *value {
        rect.right() - 11.0
    } else {
        rect.left() + 11.0
    };
    ui.painter().circle_filled(
        Pos2::new(knob_x, rect.center().y),
        8.0,
        palette::TEXT_PRIMARY,
    );
    response
}

fn primary_action(
    ui: &mut egui::Ui,
    label: &str,
    presentation: GamePresentation,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), palette::control::PRIMARY_HEIGHT),
        sense,
    );
    let response = response.on_hover_cursor(if enabled {
        CursorIcon::PointingHand
    } else {
        CursorIcon::NotAllowed
    });
    let top = if enabled {
        mix_color(presentation.accent_soft, presentation.accent, 0.34)
    } else {
        palette::OUTLINE
    };
    let bottom = if enabled {
        mix_color(presentation.accent, presentation.button_text, 0.10)
    } else {
        palette::SURFACE_HOVERED
    };
    paint_vertical_gradient(
        ui.painter(),
        rect,
        if response.hovered() && enabled {
            mix_color(top, Color32::WHITE, 0.08)
        } else {
            top
        },
        bottom,
        palette::radius::MEDIUM,
    );
    ui.painter().rect_stroke(
        rect,
        palette::radius::MEDIUM,
        Stroke::new(
            1.0_f32,
            if enabled {
                color_with_alpha(presentation.accent_soft, 170)
            } else {
                palette::OUTLINE
            },
        ),
        StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            Pos2::new(rect.left() + 15.0, rect.top() + 1.0),
            Pos2::new(rect.right() - 15.0, rect.top() + 1.0),
        ],
        Stroke::new(
            1.0_f32,
            if enabled {
                Color32::from_white_alpha(84)
            } else {
                Color32::TRANSPARENT
            },
        ),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(15.5),
        if enabled {
            presentation.button_text
        } else {
            palette::TEXT_TERTIARY
        },
    );
    response
}

impl ZmApp {
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        paint_home_atmosphere(ui);
        let outer_width = ui.available_width();
        let content_width = content_width(outer_width);
        let side_space = ((outer_width - content_width) * 0.5).max(0.0);
        let layout = layout_class(outer_width);

        ui.horizontal_top(|ui| {
            ui.add_space(side_space);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                self.home_header(ui);
                ui.add_space(palette::spacing::LG);
                match layout {
                    LayoutClass::Wide => self.wide_home(ui, ctx, content_width),
                    LayoutClass::Compact => self.compact_home(ui, ctx),
                }
                ui.add_space(palette::spacing::LG);
            });
        });
    }

    fn home_header(&mut self, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), palette::control::HEADER_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let (mark_rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                paint_brand_mark(ui, mark_rect);
                ui.add_space(palette::spacing::XS);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("云境启动器")
                            .size(22.0)
                            .strong()
                            .color(palette::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new("造梦西游 · 桌面启动器")
                            .size(12.5)
                            .color(palette::TEXT_SECONDARY),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let header_button = |text: &str| {
                        egui::Button::new(egui::RichText::new(text).size(13.5))
                            .fill(palette::SURFACE_SUNKEN)
                            .stroke(Stroke::new(1.0_f32, palette::OUTLINE))
                            .corner_radius(palette::radius::MEDIUM)
                            .min_size(Vec2::new(104.0, 38.0))
                    };
                    if ui.add(header_button("设置")).clicked() {
                        self.page = Page::Settings;
                    }
                    if ui.add(header_button("账号管理")).clicked() {
                        self.account_picker_open = true;
                    }
                });
            },
        );
    }

    fn wide_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, width: f32) {
        let columns = wide_columns(width);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            ui.allocate_ui_with_layout(
                Vec2::new(columns.stage, WIDE_HOME_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.game_stage(ui, WIDE_HOME_HEIGHT),
            );
            ui.allocate_ui_with_layout(
                Vec2::new(columns.launch, WIDE_HOME_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.launch_panel(ui, ctx, Some(WIDE_HOME_HEIGHT)),
            );
        });
    }

    fn compact_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.game_stage(ui, COMPACT_STAGE_HEIGHT);
        ui.add_space(palette::spacing::LG);
        self.launch_panel(ui, ctx, None);
    }

    fn game_stage(&mut self, ui: &mut egui::Ui, height: f32) {
        let presentation = GamePresentation::for_game(self.selected_game);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        paint_card_shadow(ui, rect);
        paint_game_backdrop(ui, rect, self.selected_game, presentation);
        paint_game_title(ui, rect, self.selected_game, presentation);
        if let Some(game) = game_switcher(ui, rect, self.selected_game) {
            self.select_game(game);
        }
    }

    fn launch_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, height: Option<f32>) {
        let presentation = GamePresentation::for_game(self.selected_game);
        if let Some(height) = height {
            let shadow_rect =
                Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), height));
            paint_card_shadow(ui, shadow_rect);
        }
        let panel = egui::Frame::new()
            .fill(palette::SURFACE)
            .stroke(Stroke::new(1.0_f32, palette::OUTLINE))
            .corner_radius(palette::radius::LARGE)
            .inner_margin(egui::Margin::same(palette::spacing::XL as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(height) = height {
                    ui.set_min_height((height - 48.0).max(0.0));
                }

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("开始游戏")
                            .size(25.0)
                            .strong()
                            .color(palette::TEXT_PRIMARY),
                    );
                    ui.add_space(8.0);
                    theme_badge(ui, self.selected_game, presentation);
                });
                ui.label(
                    egui::RichText::new("选择账号，进入你的独立游戏会话")
                        .color(palette::TEXT_SECONDARY),
                );
                ui.add_space(palette::spacing::LG);
                ui.label(
                    egui::RichText::new("登录账号")
                        .size(12.0)
                        .strong()
                        .color(palette::TEXT_TERTIARY),
                );

                let previous_account = self.account.clone();
                match self.account_mode {
                    AccountMode::Saved(id) => self.saved_account_ui(ui, id),
                    AccountMode::New => self.new_account_ui(ui),
                }
                if previous_account != self.account {
                    self.launch.cancel();
                    self.captcha_revision = self.captcha_revision.wrapping_add(1);
                    self.captcha_id = None;
                    self.captcha_url = None;
                    self.captcha_texture = None;
                    self.captcha_value.clear();
                }
                if self.captcha_id.is_some() {
                    self.captcha_ui(ui, ctx);
                }
                self.security_settings_ui(ui, presentation);

                const FOOTER_HEIGHT: f32 = 118.0;
                if height.is_some() {
                    ui.add_space((ui.available_height() - FOOTER_HEIGHT).max(palette::spacing::SM));
                } else {
                    ui.add_space(palette::spacing::LG);
                }
                ui.separator();
                ui.label(
                    egui::RichText::new("✓  官方资源 · 独立会话 · 密钥环保护")
                        .size(12.5)
                        .color(Color32::from_rgb(123, 186, 166)),
                );
                ui.add_space(palette::spacing::XS);

                let ready = !matches!(self.credential_state, CredentialState::Loading { .. });
                let action = primary_action(
                    ui,
                    &format!("进入造梦西游 {}  →", self.selected_game.number()),
                    presentation,
                    ready,
                );
                if action.clicked() {
                    self.begin_login(self.selected_game, ctx.clone());
                }
                ui.add_space(palette::spacing::XS);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("启动后仍可从顶部工具栏切换账号")
                            .size(11.0)
                            .color(palette::TEXT_TERTIARY),
                    );
                });
            });

        let accent_start = panel.response.rect.left() + 28.0;
        ui.painter().line_segment(
            [
                Pos2::new(accent_start, panel.response.rect.top() + 1.0),
                Pos2::new(accent_start + 72.0, panel.response.rect.top() + 1.0),
            ],
            Stroke::new(3.0_f32, presentation.accent),
        );
    }

    fn captcha_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new("图形验证码")
                .size(12.0)
                .strong()
                .color(palette::TEXT_TERTIARY),
        );
        ui.horizontal(|ui| {
            let image_width = 112.0_f32.min(ui.available_width() * 0.34);
            ui.add(
                egui::TextEdit::singleline(&mut self.captcha_value)
                    .hint_text("请输入验证码")
                    .desired_width((ui.available_width() - image_width - 72.0).max(90.0)),
            );
            if let Some(texture) = &self.captcha_texture {
                ui.add(
                    egui::Image::new((texture.id(), texture.size_vec2())).max_width(image_width),
                );
            } else {
                ui.label(egui::RichText::new("图片未加载").small());
            }
            if ui.small_button("刷新").clicked() {
                self.refresh_captcha(ctx.clone());
            }
        });
    }

    fn saved_account_ui(&mut self, ui: &mut egui::Ui, id: Uuid) {
        let Some(saved) = self
            .config
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
        else {
            return;
        };
        let display_name = if saved.display_name.trim().is_empty() {
            &saved.account
        } else {
            &saved.display_name
        };
        let avatar = display_name
            .chars()
            .next()
            .unwrap_or('用')
            .to_uppercase()
            .collect::<String>();
        let presentation = GamePresentation::for_game(self.selected_game);
        egui::Frame::new()
            .fill(palette::SURFACE_SUNKEN)
            .stroke(Stroke::new(1.0_f32, palette::OUTLINE))
            .corner_radius(palette::radius::MEDIUM)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_min_height(54.0);
                ui.horizontal(|ui| {
                    let (avatar_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
                    ui.painter().circle_filled(
                        avatar_rect.center(),
                        23.0,
                        mix_color(presentation.accent, Color32::from_rgb(193, 91, 52), 0.34),
                    );
                    ui.painter().text(
                        avatar_rect.center(),
                        Align2::CENTER_CENTER,
                        &avatar,
                        FontId::proportional(18.0),
                        presentation.button_text,
                    );

                    let text_width = (ui.available_width() - 88.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(text_width, 46.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add_space(if display_name.trim() == saved.account.trim() {
                                11.0
                            } else {
                                2.0
                            });
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(display_name).size(15.0).strong(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(display_name);
                            if display_name.trim() != saved.account.trim() {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&saved.account)
                                            .size(12.0)
                                            .color(palette::TEXT_TERTIARY),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&saved.account);
                            }
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("切换  ›")
                                        .size(12.5)
                                        .color(presentation.accent),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.account_picker_open = true;
                        }
                    });
                });
            });

        match self.credential_state.clone() {
            CredentialState::Loading { .. } | CredentialState::Available => {}
            CredentialState::Missing => self.password_input(ui, "此账号没有已保存密码"),
            CredentialState::Error(error) => {
                ui.label(
                    egui::RichText::new(error)
                        .small()
                        .color(palette::ACCENT_HOVER),
                );
                self.password_input(ui, "密钥环不可用，请输入密码");
            }
        }
    }

    fn new_account_ui(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.account)
                .hint_text("请输入4399账号")
                .desired_width(f32::INFINITY),
        );
        self.password_input(ui, "请输入账号密码");
    }

    fn password_input(&mut self, ui: &mut egui::Ui, hint: &str) {
        ui.label(
            egui::RichText::new("密码")
                .size(12.0)
                .strong()
                .color(palette::TEXT_TERTIARY),
        );
        ui.add(
            egui::TextEdit::singleline(&mut self.password)
                .password(true)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    }

    fn security_settings_ui(&mut self, ui: &mut egui::Ui, presentation: GamePresentation) {
        let state = self.credential_state.clone();
        egui::Frame::new()
            .fill(palette::SURFACE_SUNKEN)
            .corner_radius(palette::radius::MEDIUM)
            .inner_margin(egui::Margin::symmetric(13, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| match &state {
                    CredentialState::Loading { .. } => {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("正在从系统密钥环读取密码…")
                                .size(12.5)
                                .color(palette::TEXT_SECONDARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("取消").clicked() {
                                self.stop_game("启动已取消".into(), false);
                            }
                        });
                    }
                    CredentialState::Available => {
                        ui.label(egui::RichText::new("✓").strong().color(palette::SUCCESS));
                        ui.label(
                            egui::RichText::new("密码已由系统密钥环保护")
                                .size(12.5)
                                .color(palette::TEXT_SECONDARY),
                        );
                    }
                    CredentialState::Missing => {
                        ui.label(egui::RichText::new("◇").strong().color(presentation.accent));
                        ui.label(
                            egui::RichText::new("登录后可将密码保存到系统密钥环")
                                .size(12.5)
                                .color(palette::TEXT_SECONDARY),
                        );
                    }
                    CredentialState::Error(_) => {
                        ui.label(
                            egui::RichText::new("!")
                                .strong()
                                .color(palette::ACCENT_HOVER),
                        );
                        ui.label(
                            egui::RichText::new("系统密钥环暂不可用")
                                .size(12.5)
                                .color(palette::TEXT_SECONDARY),
                        );
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("记住密码")
                            .size(13.0)
                            .color(palette::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new("下次自动读取")
                            .size(11.5)
                            .color(palette::TEXT_TERTIARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        toggle(ui, &mut self.save_password, presentation.accent);
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_changes_at_the_shared_breakpoint() {
        assert_eq!(layout_class(979.99), LayoutClass::Compact);
        assert_eq!(layout_class(980.0), LayoutClass::Wide);
        assert_eq!(
            layout_class(palette::control::RESPONSIVE_BREAKPOINT),
            LayoutClass::Wide
        );
    }

    #[test]
    fn invalid_or_tiny_widths_use_the_compact_layout() {
        assert_eq!(layout_class(f32::NAN), LayoutClass::Compact);
        assert_eq!(layout_class(0.0), LayoutClass::Compact);
        assert_eq!(layout_class(-1.0), LayoutClass::Compact);
    }

    #[test]
    fn every_game_has_distinct_presentation_tokens() {
        let zm4 = GamePresentation::for_game(GameKind::Zm4);
        let zm5 = GamePresentation::for_game(GameKind::Zm5);

        assert_eq!(zm4.series_label, "洪荒大劫篇");
        assert_eq!(zm5.series_label, "上古天帝篇");
        assert_ne!(zm4.accent, zm5.accent);
        assert_ne!(zm4.backdrop_top, zm5.backdrop_top);
        assert_ne!(zm4.backdrop_bottom, zm5.backdrop_bottom);
        assert_ne!(zm4.mountain_far, zm5.mountain_far);
    }

    #[test]
    fn home_dimensions_follow_theme_tokens() {
        assert_eq!(CONTENT_MAX_WIDTH, 1_120.0);
        assert_eq!(OUTER_MARGIN, 24.0);
    }

    #[test]
    fn content_is_centerable_and_columns_preserve_available_width() {
        assert_eq!(content_width(900.0), 852.0);
        assert_eq!(content_width(1_180.0), 1_120.0);
        assert_eq!(content_width(1_600.0), 1_120.0);

        let columns = wide_columns(1_120.0);
        assert_eq!(columns.stage + COLUMN_GAP + columns.launch, 1_120.0);
        assert!(columns.stage > columns.launch);
    }

    #[test]
    fn color_mixing_keeps_endpoints_exact() {
        let from = Color32::from_rgb(1, 2, 3);
        let to = Color32::from_rgb(101, 102, 103);
        assert_eq!(mix_color(from, to, 0.0), from);
        assert_eq!(mix_color(from, to, 1.0), to);
    }
}
