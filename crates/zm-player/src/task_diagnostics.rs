use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

const SLOW_POLL: Duration = Duration::from_millis(20);
const HISTORY_LIMIT: usize = 8;
const SOURCE_LIMIT: usize = 192;

thread_local! {
    static CURRENT_TASK: RefCell<Option<Arc<TaskIdentity>>> = const { RefCell::new(None) };
}

struct TaskIdentity {
    id: u64,
    source: Mutex<Option<String>>,
}

#[derive(Clone)]
struct SlowPoll {
    task_id: u64,
    source: String,
    elapsed: Duration,
    at: Duration,
}

#[derive(Default)]
struct History {
    worst: Option<SlowPoll>,
    recent: VecDeque<SlowPoll>,
}

/// Per-session history that cannot be evicted by cheap non-due UI updates.
#[derive(Default)]
pub(crate) struct TaskDiagnostics {
    started_at: OnceLock<Instant>,
    next_id: AtomicU64,
    history: Mutex<History>,
}

impl TaskDiagnostics {
    pub(crate) fn start_observing(&self, now: Instant) {
        let _ = self.started_at.set(now);
    }

    pub(crate) fn instrument<F: Future>(self: &Arc<Self>, future: F) -> TrackedFuture<F> {
        TrackedFuture {
            inner: Box::pin(future),
            identity: Arc::new(TaskIdentity {
                id: self.next_id.fetch_add(1, Ordering::Relaxed) + 1,
                source: Mutex::new(None),
            }),
            diagnostics: self.clone(),
        }
    }

    fn record(&self, identity: &TaskIdentity, started: Instant, elapsed: Duration) {
        if elapsed < SLOW_POLL {
            return;
        }
        let origin = self.started_at.get_or_init(|| started);
        let event = SlowPoll {
            task_id: identity.id,
            source: identity
                .source
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "local AVM task (no fetch observed)".into()),
            elapsed,
            at: started.saturating_duration_since(*origin),
        };
        // Sources contain no request body, account, query, or authentication data.
        tracing::warn!(
            task_id = event.task_id,
            related_source = %event.source,
            elapsed_ms = event.elapsed.as_secs_f64() * 1000.0,
            at_session_s = event.at.as_secs_f64(),
            "本地加载任务单次执行较慢（包含解析及游戏回调）"
        );
        let mut history = self.history.lock().unwrap();
        if history
            .worst
            .as_ref()
            .is_none_or(|worst| elapsed > worst.elapsed)
        {
            history.worst = Some(event.clone());
        }
        if history.recent.len() == HISTORY_LIMIT {
            history.recent.pop_front();
        }
        history.recent.push_back(event);
    }

    pub(crate) fn summary(&self) -> String {
        let history = self.history.lock().unwrap();
        let mut text = String::from(
            "Slow local tasks (session): threshold_ms=20 history_limit=8; related_source identifies the fetch associated with a poll, not a measured decoding phase; at_session_s starts at the first host update\n",
        );
        let mut append = |label: &str, event: &SlowPoll| {
            text.push_str(&format!(
                "{label}: task={} elapsed_ms={:.3} at_session_s={:.3} related_source={}\n",
                event.task_id,
                event.elapsed.as_secs_f64() * 1000.0,
                event.at.as_secs_f64(),
                event.source,
            ));
        };
        if let Some(worst) = &history.worst {
            append("Slow task worst", worst);
        }
        for event in &history.recent {
            append("Slow task recent", event);
        }
        text
    }
}

pub(crate) struct TrackedFuture<F> {
    inner: Pin<Box<F>>,
    identity: Arc<TaskIdentity>,
    diagnostics: Arc<TaskDiagnostics>,
}

impl<F: Future> Future for TrackedFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let previous = CURRENT_TASK.with(|current| current.replace(Some(this.identity.clone())));
        let _scope = TaskScope(previous);
        let started = Instant::now();
        let outcome = this.inner.as_mut().poll(context);
        this.diagnostics
            .record(&this.identity, started, started.elapsed());
        outcome
    }
}

struct TaskScope(Option<Arc<TaskIdentity>>);

impl Drop for TaskScope {
    fn drop(&mut self) {
        CURRENT_TASK.with(|current| current.replace(self.0.take()));
    }
}

/// Call only for the already validated static-resource path, never a raw URL.
pub(crate) fn mark_static_resource(resource: &str) {
    let path = resource.split(['?', '#']).next().unwrap_or_default();
    let safe: String = path
        .chars()
        .filter(|character| !character.is_control())
        .take(SOURCE_LIMIT)
        .collect();
    mark_source(format!("static resource: {safe}"));
}

pub(crate) fn mark_network(authentication: bool) {
    mark_source(
        if authentication {
            "authentication request"
        } else {
            "non-cache network request"
        }
        .into(),
    );
}

fn mark_source(source: String) {
    CURRENT_TASK.with(|current| {
        if let Some(identity) = current.borrow().as_ref() {
            *identity.source.lock().unwrap() = Some(source);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;

    #[test]
    fn nested_poll_sources_are_restored_and_never_store_request_queries() {
        let diagnostics = Arc::new(TaskDiagnostics::default());
        let nested = diagnostics.instrument(async { mark_network(true) });
        let nested_identity = nested.identity.clone();
        let mut nested = Box::pin(nested);
        let mut outer = Box::pin(diagnostics.instrument(async move {
            mark_static_resource("assets/icon.png?token=secret#fragment");
            let mut context = Context::from_waker(Waker::noop());
            assert!(nested.as_mut().poll(&mut context).is_ready());
            mark_static_resource("assets/final.swf?cookie=secret");
        }));
        let outer_identity = outer.identity.clone();
        let mut context = Context::from_waker(Waker::noop());
        assert!(outer.as_mut().poll(&mut context).is_ready());
        assert_eq!(
            outer_identity.source.lock().unwrap().as_deref(),
            Some("static resource: assets/final.swf")
        );
        assert_eq!(
            nested_identity.source.lock().unwrap().as_deref(),
            Some("authentication request")
        );
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));
    }

    #[test]
    fn source_survives_pending_without_leaking_into_other_tasks() {
        let diagnostics = Arc::new(TaskDiagnostics::default());
        let mut polls = 0;
        let mut future = Box::pin(diagnostics.instrument(std::future::poll_fn(|_| {
            polls += 1;
            if polls == 1 {
                mark_static_resource("assets/pending.swf?token=secret");
                Poll::Pending
            } else {
                CURRENT_TASK.with(|current| {
                    assert_eq!(
                        current
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .source
                            .lock()
                            .unwrap()
                            .as_deref(),
                        Some("static resource: assets/pending.swf")
                    );
                });
                Poll::Ready(())
            }
        })));
        let identity = future.identity.clone();
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));

        // A different runnable may execute between this task's polls.
        let mut other = Box::pin(diagnostics.instrument(async { mark_network(false) }));
        assert!(other.as_mut().poll(&mut context).is_ready());
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));
        assert!(future.as_mut().poll(&mut context).is_ready());
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));
        assert_eq!(
            identity.source.lock().unwrap().as_deref(),
            Some("static resource: assets/pending.swf")
        );
    }

    #[test]
    fn cancelling_a_pending_task_releases_its_identity_and_inner_future() {
        let diagnostics = Arc::new(TaskDiagnostics::default());
        let payload = Arc::new(());
        let released_payload = Arc::downgrade(&payload);
        let mut future = Box::pin(diagnostics.instrument(async move {
            let _payload = payload;
            mark_static_resource("assets/cancelled.swf");
            std::future::pending::<()>().await;
        }));
        let released_identity = Arc::downgrade(&future.identity);
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(released_payload.upgrade().is_some());
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));

        drop(future);
        assert!(released_identity.upgrade().is_none());
        assert!(released_payload.upgrade().is_none());
        assert!(CURRENT_TASK.with(|current| current.borrow().is_none()));

        let mut next = Box::pin(diagnostics.instrument(async {}));
        assert!(next.as_mut().poll(&mut context).is_ready());
        assert!(next.identity.source.lock().unwrap().is_none());
    }

    #[test]
    fn static_sources_strip_controls_and_bound_unicode_paths() {
        let diagnostics = Arc::new(TaskDiagnostics::default());
        let path = format!(
            "assets/\0\n\r\t\u{7f}\u{85}{}#fragment?token=secret",
            "界".repeat(SOURCE_LIMIT)
        );
        let mut future = Box::pin(diagnostics.instrument(async move {
            mark_static_resource(&path);
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_ready());
        let source = future.identity.source.lock().unwrap();
        let path = source
            .as_deref()
            .unwrap()
            .strip_prefix("static resource: ")
            .unwrap();
        assert_eq!(path.chars().count(), SOURCE_LIMIT);
        assert_eq!(path, format!("assets/{}", "界".repeat(SOURCE_LIMIT - 7)));
        assert!(!path.chars().any(char::is_control));
        assert!(!path.contains("fragment"));
        assert!(!path.contains("secret"));
    }

    #[test]
    fn bounded_recent_history_keeps_the_session_worst_and_separate_sessions() {
        let diagnostics = TaskDiagnostics::default();
        let now = Instant::now();
        diagnostics.start_observing(now);
        let identity = TaskIdentity {
            id: 1,
            source: Mutex::new(Some("static resource: assets/slow.swf".into())),
        };
        diagnostics.record(
            &identity,
            now + Duration::from_secs(1),
            Duration::from_millis(1500),
        );
        for index in 0..12 {
            diagnostics.record(
                &identity,
                now + Duration::from_secs(2 + index),
                Duration::from_millis(20 + index),
            );
        }
        // Small polls must not evict useful slow-task evidence.
        for _ in 0..2000 {
            diagnostics.record(&identity, now, Duration::from_micros(5));
        }
        let summary = diagnostics.summary();
        assert!(summary.contains("Slow task worst: task=1 elapsed_ms=1500.000 at_session_s=1.000"));
        assert_eq!(summary.matches("Slow task recent:").count(), HISTORY_LIMIT);
        assert!(
            !TaskDiagnostics::default()
                .summary()
                .contains("Slow task worst:")
        );
    }
}
