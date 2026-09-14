use super::palette;
use eframe::egui::{self, Align, FontId, Layout, RichText, Sense, Stroke, Ui, vec2};
use std::time::{Duration, Instant};
use zm_player::ResourceLoadingProgress;

const ROW_HEIGHT: f32 = 26.0;

#[derive(Default)]
pub(super) struct ResourceActivity {
    started: Option<Instant>,
    last_pending: Option<Instant>,
    progress: ResourceLoadingProgress,
    visible: bool,
}

impl ResourceActivity {
    pub(super) fn update(
        &mut self,
        now: Instant,
        progress: Option<ResourceLoadingProgress>,
    ) -> Option<ResourceLoadingProgress> {
        let progress = progress.filter(|progress| progress.pending_requests > 0);
        if (progress.is_none() || self.progress.pending_requests == 0)
            && self.last_pending.is_some_and(|last| {
                now.saturating_duration_since(last) >= Duration::from_millis(600)
            })
        {
            *self = Self::default();
        }
        if let Some(progress) = progress {
            let started = *self.started.get_or_insert(now);
            self.last_pending = Some(now);
            self.progress = progress;
            self.visible |= now.saturating_duration_since(started) >= Duration::from_millis(180);
        } else {
            self.progress.pending_requests = 0;
        }
        self.visible.then_some(self.progress)
    }
}

pub(super) fn show(ui: &mut Ui, status: &str, progress: Option<ResourceLoadingProgress>) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 18.0;
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.set_min_height(ROW_HEIGHT);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(palette::TEXT_TERTIARY),
            );
            if let Some(progress) = progress {
                loading_badge(ui, progress);
            }
            // Reserve the same row with or without activity so the game does
            // not resize as requests start and finish. Errors retain priority.
            let (rect, response) = ui.allocate_exact_size(
                vec2(ui.available_width().max(0.0), ROW_HEIGHT),
                Sense::hover(),
            );
            let mut text = egui::text::LayoutJob::simple(
                status.into(),
                FontId::proportional(13.0),
                palette::TEXT_SECONDARY,
                rect.width(),
            );
            text.wrap.max_rows = 1;
            text.wrap.break_anywhere = true;
            let galley = ui.painter().layout_job(text);
            ui.painter()
                .with_clip_rect(rect.intersect(ui.clip_rect()))
                .galley(
                    egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
                    galley,
                    palette::TEXT_SECONDARY,
                );
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, status));
            response.on_hover_text(status);
        });
    });
}

fn loading_badge(ui: &mut Ui, progress: ResourceLoadingProgress) {
    let title = "资源加载";
    let galley = ui.painter().layout_no_wrap(
        title.into(),
        FontId::proportional(12.0),
        palette::BRAND_SOFT,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 40.0, ROW_HEIGHT), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            13,
            palette::SURFACE_RAISED,
            Stroke::new(1.0_f32, palette::OUTLINE),
            egui::StrokeKind::Inside,
        );
        let center = egui::pos2(rect.left() + 16.0, rect.center().y);
        let phase = ui.input(|input| input.time) * 3.0;
        for index in 0..3 {
            let brightness = if progress.pending_requests == 0 {
                0.65
            } else {
                0.35 + 0.65 * (0.5 + 0.5 * (phase - index as f64 * 0.9).cos())
            };
            ui.painter().circle_filled(
                center + vec2((index as f32 - 1.0) * 4.5, 0.0),
                1.5,
                palette::BRAND.linear_multiply(brightness as f32),
            );
        }
        ui.painter().galley(
            egui::pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0),
            galley,
            palette::BRAND_SOFT,
        );
        // The active game already schedules frames. Unlike egui::Spinner, this
        // indicator must not request immediate repaints and bypass that limit.
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, title)
    });
    response.on_hover_text(format!(
        "游戏资源按需加载。\n待完成：{} 项\n本次游戏已接收：{:.1} MiB（不含本地缓存）",
        progress.pending_requests,
        progress.received_bytes as f64 / (1024.0 * 1024.0),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn short_reads_stay_hidden_and_bursts_do_not_flash_the_indicator() {
        let now = Instant::now();
        let pending = Some(ResourceLoadingProgress {
            pending_requests: 1,
            received_bytes: 20,
        });
        let mut activity = ResourceActivity::default();
        assert!(activity.update(now, pending).is_none());
        assert!(
            activity
                .update(now + Duration::from_millis(50), None)
                .is_none()
        );
        assert!(
            activity
                .update(now + Duration::from_millis(180), pending)
                .is_some()
        );
        let idle = activity
            .update(now + Duration::from_millis(200), None)
            .unwrap();
        assert_eq!(idle.pending_requests, 0);
        assert_eq!(idle.received_bytes, 20);
        assert!(
            activity
                .update(now + Duration::from_millis(400), pending)
                .is_some()
        );
        assert!(
            activity
                .update(now + Duration::from_millis(999), None)
                .is_some()
        );
        assert!(
            activity
                .update(now + Duration::from_millis(1000), None)
                .is_none()
        );
        assert!(
            activity
                .update(now + Duration::from_millis(1010), pending)
                .is_none()
        );
        assert!(
            activity
                .update(now + Duration::from_millis(1200), pending)
                .is_some()
        );
        // A long host frame does not hide an already visible active download.
        assert!(
            activity
                .update(now + Duration::from_secs(3), pending)
                .is_some()
        );
    }

    #[test]
    fn loading_indicator_preserves_game_deadline_and_viewport_height() {
        let context = egui::Context::default();
        let delays = Arc::new(Mutex::new(Vec::new()));
        let captured = delays.clone();
        context.set_request_repaint_callback(move |request| {
            captured.lock().unwrap().push(request.delay);
        });
        let mut heights = Vec::new();
        for width in [640.0, 1280.0] {
            for progress in [
                None,
                Some(ResourceLoadingProgress {
                    pending_requests: 1,
                    received_bytes: 5_242_880,
                }),
                Some(ResourceLoadingProgress {
                    pending_requests: 100,
                    received_bytes: u64::MAX,
                }),
            ] {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(width, 720.0),
                    )),
                    ..Default::default()
                };
                // Settle egui's initial layout passes and hover state.
                for _ in 0..3 {
                    let _ = context.run(input.clone(), |ctx| {
                        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                            show(ui, "Resource error must stay visible", progress);
                        });
                    });
                }
                delays.lock().unwrap().clear();
                let _ = context.run(input, |ctx| {
                    let panel = egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                        show(ui, "Resource error must stay visible", progress);
                    });
                    heights.push(panel.response.rect.height());
                    super::super::refresh::request_game_repaint(ctx, Duration::from_millis(25));
                });
                assert_eq!(*delays.lock().unwrap(), [Duration::from_millis(25)]);
            }
        }
        assert!(heights.iter().all(|height| *height == heights[0]));
    }
}
