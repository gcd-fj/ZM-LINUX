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
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

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
