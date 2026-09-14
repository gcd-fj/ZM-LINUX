use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use eframe::egui;

use super::AppMessage;

/// Every asynchronous app result must wake the UI, including error-only replies.
#[derive(Clone)]
pub(super) struct AppSender {
    sender: mpsc::Sender<AppMessage>,
    context: egui::Context,
}

impl AppSender {
    pub(super) fn new(sender: mpsc::Sender<AppMessage>, context: egui::Context) -> Self {
        Self { sender, context }
    }

    pub(super) fn send(&self, message: AppMessage) -> Result<(), mpsc::SendError<AppMessage>> {
        self.sender.send(message)?;
        self.context.request_repaint();
        Ok(())
    }
}

/// `remaining` already measures time until the player's actual frame deadline.
/// egui subtracts its predicted frame interval from delayed repaint requests;
/// add that prediction back so an early wake does not turn into repeated zero
/// delays before the game frame is due. Input and task wakeups remain immediate.
pub(super) fn request_game_repaint(context: &egui::Context, remaining: Duration) {
    let delay = if remaining.is_zero() {
        Duration::ZERO
    } else {
        let prediction = context
            .input(|input| Duration::try_from_secs_f32(input.predicted_dt).unwrap_or_default());
        remaining.saturating_add(prediction)
    };
    context.request_repaint_after(delay);
}

const DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct DiagnosticsCache {
    pub(super) text: String,
    updated_at: Option<Instant>,
}

impl DiagnosticsCache {
    pub(super) fn invalidate(&mut self) {
        self.updated_at = None;
    }

    pub(super) fn needs_refresh(&self, now: Instant, live: bool) -> bool {
        self.updated_at.is_none_or(|updated| {
            live && now.saturating_duration_since(updated) >= DIAGNOSTICS_INTERVAL
        })
    }

    pub(super) fn replace(&mut self, text: String, now: Instant) {
        self.text = text;
        self.updated_at = Some(now);
    }

    pub(super) fn next_refresh_in(&self, now: Instant) -> Duration {
        self.updated_at.map_or(Duration::ZERO, |updated| {
            DIAGNOSTICS_INTERVAL.saturating_sub(now.saturating_duration_since(updated))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn repaint_context() -> (egui::Context, Arc<Mutex<Vec<Duration>>>) {
        let context = egui::Context::default();
        for _ in 0..3 {
            let _ = context.run(egui::RawInput::default(), |_| {});
        }
        let delays = Arc::new(Mutex::new(Vec::new()));
        let captured = delays.clone();
        context.set_request_repaint_callback(move |request| {
            captured.lock().unwrap().push(request.delay);
        });
        (context, delays)
    }

    #[test]
    fn game_deadline_compensates_egui_prediction_without_early_repaint_loops() {
        let (context, delays) = repaint_context();
        let remaining = Duration::from_millis(5);
        let _ = context.run(egui::RawInput::default(), |context| {
            context.request_repaint_after(remaining);
        });
        // Demonstrate the upstream behavior that caused premature game updates.
        assert_eq!(*delays.lock().unwrap(), [Duration::ZERO]);

        for prediction in [1.0 / 60.0, 1.0 / 240.0, 0.0] {
            for remaining in [
                Duration::from_millis(30),
                Duration::from_millis(10),
                Duration::from_millis(1),
                Duration::from_micros(1),
            ] {
                delays.lock().unwrap().clear();
                let _ = context.run(
                    egui::RawInput {
                        predicted_dt: prediction,
                        ..Default::default()
                    },
                    |context| request_game_repaint(context, remaining),
                );
                assert_eq!(*delays.lock().unwrap(), [remaining]);
            }
        }
    }

    #[test]
    fn game_deadline_keeps_due_frames_and_input_wakeups_immediate() {
        let (context, delays) = repaint_context();
        let _ = context.run(egui::RawInput::default(), |context| {
            request_game_repaint(context, Duration::ZERO);
        });
        assert!(delays.lock().unwrap().contains(&Duration::ZERO));

        for _ in 0..3 {
            let _ = context.run(egui::RawInput::default(), |_| {});
        }
        delays.lock().unwrap().clear();
        let _ = context.run(
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))],
                ..Default::default()
            },
            |context| request_game_repaint(context, Duration::from_millis(30)),
        );
        assert!(delays.lock().unwrap().contains(&Duration::ZERO));
    }

    #[test]
    fn diagnostics_refreshes_at_most_once_per_second_and_freezes_after_stop() {
        let now = Instant::now();
        let mut cache = DiagnosticsCache::default();
        assert!(cache.needs_refresh(now, false));
        cache.replace("snapshot".into(), now);
        assert!(!cache.needs_refresh(now + Duration::from_millis(999), true));
        assert!(cache.needs_refresh(now + Duration::from_secs(1), true));
        assert!(!cache.needs_refresh(now + Duration::from_secs(30), false));
        cache.invalidate();
        assert!(cache.needs_refresh(now, false));
    }

    #[test]
    fn an_async_error_wakes_an_idle_ui_after_delivering_the_message() {
        let context = egui::Context::default();
        // Consume the initial repaint; the next request must wake an idle viewport.
        for _ in 0..3 {
            let _ = context.run(egui::RawInput::default(), |_| {});
        }
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        context.set_request_repaint_callback(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (tx, rx) = mpsc::channel();
        let sender = AppSender::new(tx, context);
        std::thread::spawn(move || {
            sender
                .send(AppMessage::Notice("credential error".into()))
                .is_ok()
        })
        .join()
        .unwrap();
        assert!(
            matches!(rx.try_recv(), Ok(AppMessage::Notice(message)) if message == "credential error")
        );
        assert!(requests.load(Ordering::Relaxed) > 0);
    }
}
