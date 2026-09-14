use crate::runtime::{RuntimeEvent, RuntimeEventSender};
use ruffle_core::backend::log::LogBackend;
use std::{
    borrow::Cow,
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zm_assets::{ResourceProgress, ResourceProgressCallback};
use zm_core::TimingSamples;

/// Resource-request activity, not the game's overall loading percentage.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLoadingProgress {
    pub pending_requests: u64,
    /// Actual response-body bytes received this session, including retries.
    /// Cache hits are excluded; this is not compressed wire-traffic accounting.
    pub received_bytes: u64,
}

#[derive(Default)]
struct RequestProgress {
    active: bool,
    attempt: u8,
    received: u64,
}

/// Ends activity even when the local AVM future is cancelled. An observer kept
/// by a backend cannot update the counters after its request has been dropped.
pub(crate) struct ResourceLoadGuard {
    metrics: Arc<ResourceMetrics>,
    state: Arc<Mutex<RequestProgress>>,
}

impl ResourceLoadGuard {
    pub(crate) fn observer(&self) -> ResourceProgressCallback {
        let metrics = self.metrics.clone();
        let state = self.state.clone();
        Arc::new(move |progress: ResourceProgress| {
            let mut previous = state.lock().unwrap();
            if !previous.active || progress.cache_hit || progress.attempt < previous.attempt {
                return;
            }
            let baseline = if previous.attempt == progress.attempt {
                previous.received
            } else {
                0
            };
            let current = progress.bytes_loaded.max(baseline);
            previous.attempt = progress.attempt;
            previous.received = current;
            metrics
                .received_bytes
                .fetch_add(current - baseline, std::sync::atomic::Ordering::Relaxed);
        })
    }
}

impl Drop for ResourceLoadGuard {
    fn drop(&mut self) {
        self.state.lock().unwrap().active = false;
        self.metrics
            .pending_requests
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
pub(crate) struct ResourceMetrics {
    pending_requests: std::sync::atomic::AtomicU64,
    received_bytes: std::sync::atomic::AtomicU64,
    cache_hits: std::sync::atomic::AtomicU64,
    downloads: std::sync::atomic::AtomicU64,
    failures: std::sync::atomic::AtomicU64,
    dynamic_modules: std::sync::atomic::AtomicU64,
    cache_micros: std::sync::atomic::AtomicU64,
    cache_peak_micros: std::sync::atomic::AtomicU64,
    download_micros: std::sync::atomic::AtomicU64,
    download_peak_micros: std::sync::atomic::AtomicU64,
    recent: Mutex<VecDeque<String>>,
}

impl ResourceMetrics {
    pub(crate) fn begin_load(self: &Arc<Self>) -> ResourceLoadGuard {
        self.pending_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ResourceLoadGuard {
            metrics: self.clone(),
            state: Arc::new(Mutex::new(RequestProgress {
                active: true,
                ..Default::default()
            })),
        }
    }

    pub(crate) fn loading_progress(&self) -> ResourceLoadingProgress {
        ResourceLoadingProgress {
            pending_requests: self
                .pending_requests
                .load(std::sync::atomic::Ordering::Relaxed),
            received_bytes: self
                .received_bytes
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub(crate) fn record_success(&self, resource: &str, cache_hit: bool, elapsed: Duration) {
        let counter = if cache_hit {
            &self.cache_hits
        } else {
            &self.downloads
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let elapsed_micros = elapsed.as_micros().min(u64::MAX as u128) as u64;
        let (total, peak) = if cache_hit {
            (&self.cache_micros, &self.cache_peak_micros)
        } else {
            (&self.download_micros, &self.download_peak_micros)
        };
        total.fetch_add(elapsed_micros, std::sync::atomic::Ordering::Relaxed);
        peak.fetch_max(elapsed_micros, std::sync::atomic::Ordering::Relaxed);
        if resource.to_ascii_lowercase().ends_with(".swf") {
            self.dynamic_modules
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.record_recent(format!(
            "{} {resource}",
            if cache_hit {
                "缓存命中"
            } else {
                "已下载"
            }
        ));
    }

    pub(crate) fn record_failure(&self, resource: &str, error: &str) {
        self.failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.record_recent(format!("加载失败 {resource}: {error}"));
    }

    fn record_recent(&self, message: String) {
        let mut recent = self.recent.lock().unwrap();
        if recent.len() >= 12 {
            recent.pop_front();
        }
        recent.push_back(message);
    }

    pub(crate) fn summary(&self) -> String {
        let mut output = self.performance_summary();
        for entry in self.recent.lock().unwrap().iter() {
            output.push_str("Resource: ");
            output.push_str(entry);
            output.push('\n');
        }
        output
    }

    /// Numeric counters only: safe for optional periodic performance logging.
    pub(crate) fn performance_summary(&self) -> String {
        let hits = self.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let downloads = self.downloads.load(std::sync::atomic::Ordering::Relaxed);
        let failures = self.failures.load(std::sync::atomic::Ordering::Relaxed);
        let dynamic_modules = self
            .dynamic_modules
            .load(std::sync::atomic::Ordering::Relaxed);
        let cache_total = self.cache_micros.load(std::sync::atomic::Ordering::Relaxed);
        let cache_peak = self
            .cache_peak_micros
            .load(std::sync::atomic::Ordering::Relaxed);
        let download_total = self
            .download_micros
            .load(std::sync::atomic::Ordering::Relaxed);
        let download_peak = self
            .download_peak_micros
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut output = format!(
            "Resources: cache_hits={hits} downloads={downloads} failures={failures} dynamic_swf_ready={dynamic_modules}\nResource timing: cache_avg_ms={:.2} cache_peak_ms={:.2} download_avg_ms={:.2} download_peak_ms={:.2}\n",
            average_millis(cache_total, hits),
            cache_peak as f64 / 1000.0,
            average_millis(download_total, downloads),
            download_peak as f64 / 1000.0,
        );
        let loading = self.loading_progress();
        output.push_str(&format!(
            "Resource transfer (session): pending_requests={} received_body_bytes={} (cache hits excluded; retries included; not overall game progress)\n",
            loading.pending_requests, loading.received_bytes,
        ));
        output
    }
}

#[derive(Debug, Default)]
pub(crate) struct FrameMetrics {
    update: TimingSamples,
    input: TimingSamples,
    task_poll: TimingSamples,
    player_tick: TimingSamples,
    render_submit: TimingSamples,
    schedule_late: TimingSamples,
    updates: u64,
    ticks: u64,
    render_submissions: u64,
    task_polls: u64,
    queued_tasks: usize,
    peak_queue_depth: usize,
    max_single_poll: Duration,
    source_fps: f64,
    game_target_fps: f64,
    started_at: Option<Instant>,
    session_peaks: [SessionPeak; 6],
}

#[derive(Debug, Default)]
struct SessionPeak {
    elapsed: Duration,
    at_update_start: Option<Duration>,
}

impl SessionPeak {
    fn record(&mut self, elapsed: Duration, at_update_start: Duration) {
        if self.at_update_start.is_none() || elapsed > self.elapsed {
            self.elapsed = elapsed;
            self.at_update_start = Some(at_update_start);
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TaskPollReport {
    pub(crate) elapsed: Duration,
    pub(crate) polls: u64,
    pub(crate) peak_queue_depth: usize,
    pub(crate) remaining: usize,
    pub(crate) max_single_poll: Duration,
}

#[derive(Debug, Default)]
pub(crate) struct FrameUpdateTimings {
    pub(crate) update: Duration,
    pub(crate) input: Duration,
    pub(crate) tasks: [TaskPollReport; 2],
    pub(crate) player_tick: Option<Duration>,
    pub(crate) render_submit: Option<Duration>,
    pub(crate) schedule_late: Option<Duration>,
    pub(crate) game_target_fps: f64,
}

impl FrameMetrics {
    pub(crate) fn new(source_fps: f64) -> Self {
        Self {
            source_fps,
            ..Default::default()
        }
    }

    pub(crate) fn record(&mut self, started_at: Instant, sample: FrameUpdateTimings) {
        let session_start = *self.started_at.get_or_insert(started_at);
        let at_update_start = started_at.saturating_duration_since(session_start);
        let task_elapsed = sample.tasks[0]
            .elapsed
            .saturating_add(sample.tasks[1].elapsed);
        for (peak, elapsed) in self.session_peaks.iter_mut().zip([
            Some(sample.update),
            Some(sample.input),
            Some(task_elapsed),
            sample.player_tick,
            sample.render_submit,
            sample.schedule_late,
        ]) {
            if let Some(elapsed) = elapsed {
                peak.record(elapsed, at_update_start);
            }
        }
        self.updates = self.updates.saturating_add(1);
        self.update.record(sample.update);
        self.input.record(sample.input);
        self.task_poll.record(task_elapsed);
        for report in sample.tasks {
            self.task_polls = self.task_polls.saturating_add(report.polls);
            self.peak_queue_depth = self.peak_queue_depth.max(report.peak_queue_depth);
            self.max_single_poll = self.max_single_poll.max(report.max_single_poll);
            self.queued_tasks = report.remaining;
        }
        if let Some(elapsed) = sample.player_tick {
            self.ticks = self.ticks.saturating_add(1);
            self.player_tick.record(elapsed);
        }
        if let Some(elapsed) = sample.render_submit {
            self.render_submissions = self.render_submissions.saturating_add(1);
            self.render_submit.record(elapsed);
        }
        if let Some(elapsed) = sample.schedule_late {
            self.schedule_late.record(elapsed);
        }
        if sample.game_target_fps.is_finite() && sample.game_target_fps > 0.0 {
            self.game_target_fps = sample.game_target_fps;
        }
    }

    pub(crate) fn summary(&self) -> String {
        self.summary_at(Instant::now())
    }

    fn summary_at(&self, now: Instant) -> String {
        let elapsed_secs = self
            .started_at
            .map(|started| now.saturating_duration_since(started).as_secs_f64())
            .unwrap_or_default();
        let rate_denominator = elapsed_secs.max(0.001);
        let mut output = format!(
            "Frames (session): source_fps={:.2} game_target_fps={:.2} update_hz={:.2} tick_hz={:.2} updates={} ticks={} render_submissions={} session_elapsed_s={elapsed_secs:.3} (observed since first host update; source_fps is the loaded SWF header; game_target_fps may change at runtime; tick_hz is host tick calls, not AVM frame rate)\nLocal tasks (session): polls={} queued_tasks={} peak_queue_depth={} max_single_poll_ms={:.3} (poll budget is cooperative, not a hard limit)\n",
            self.source_fps,
            self.game_target_fps,
            self.updates as f64 / rate_denominator,
            self.ticks as f64 / rate_denominator,
            self.updates,
            self.ticks,
            self.render_submissions,
            self.task_polls,
            self.queued_tasks,
            self.peak_queue_depth,
            self.max_single_poll.as_secs_f64() * 1_000.0,
        );
        output.push_str("Timing windows: percentiles and peaks below use each stage's bounded recent samples; session peaks are retained separately for the whole observed session. CPU wall is elapsed host-thread time, not CPU utilization or GPU execution; at_update_start_s identifies the containing host update's start.\n");
        for (label, samples) in [
            ("Player update CPU wall", &self.update),
            ("Player input CPU wall", &self.input),
            ("Player task_poll CPU wall", &self.task_poll),
            ("Player player_tick CPU wall", &self.player_tick),
            (
                "Player render_submit CPU wall (not GPU execution)",
                &self.render_submit,
            ),
            ("Player schedule_late", &self.schedule_late),
        ] {
            output.push_str(&samples.summary(label));
        }
        for (stage, peak) in [
            "update_cpu_wall",
            "input_cpu_wall",
            "task_poll_cpu_wall",
            "player_tick_cpu_wall",
            "render_submit_cpu_wall",
            "schedule_late",
        ]
        .into_iter()
        .zip(&self.session_peaks)
        {
            let at_update_start = peak.at_update_start.map_or_else(
                || "none".to_owned(),
                |elapsed| format!("{:.3}", elapsed.as_secs_f64()),
            );
            output.push_str(&format!(
                "Player session peak: stage={stage} peak_ms={:.3} at_update_start_s={at_update_start}\n",
                peak.elapsed.as_secs_f64() * 1_000.0,
            ));
        }
        output
    }
}

#[derive(Debug, Default)]
pub(crate) struct StartupMetrics {
    pub(crate) total: TimingSamples,
    pub(crate) file_read: TimingSamples,
    pub(crate) descriptors: TimingSamples,
    pub(crate) movie_parse: TimingSamples,
    pub(crate) font_scan: TimingSamples,
    pub(crate) player_build: TimingSamples,
    pub(crate) texture_register: TimingSamples,
}

impl StartupMetrics {
    pub(crate) fn summary(&self) -> String {
        let mut output = String::new();
        for (label, samples) in [
            ("Startup total CPU wall", &self.total),
            ("Startup file_read CPU wall", &self.file_read),
            ("Startup descriptors CPU wall", &self.descriptors),
            ("Startup movie_parse CPU wall", &self.movie_parse),
            ("Startup font_scan CPU wall", &self.font_scan),
            ("Startup player_build CPU wall", &self.player_build),
            ("Startup texture_register CPU wall", &self.texture_register),
        ] {
            output.push_str(&samples.summary(label));
        }
        output
    }
}

fn average_millis(total_micros: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total_micros as f64 / count as f64 / 1000.0
    }
}

#[derive(Debug, Default)]
pub(crate) struct CompatibilityMetrics {
    module_completions: std::sync::atomic::AtomicU64,
    loader_mounts: std::sync::atomic::AtomicU64,
    vip_requests: std::sync::atomic::AtomicU64,
    vip_claimed_replies: std::sync::atomic::AtomicU64,
    red_point_updates: std::sync::atomic::AtomicU64,
}

fn compatibility_matches(message: &str) -> [bool; 5] {
    // Keep the original conversion: scratch buffers and uppercase prechecks
    // regressed representative inputs in the release microbenchmark.
    let lower = message.to_ascii_lowercase();
    [
        lower.contains("resourceloadcomplete") || lower.contains("loadbundleassetscomplete"),
        lower.contains("addchild") || lower.contains("added_to_stage"),
        lower.contains("viphandler") || lower.contains("getdailyreward"),
        message.contains("今日奖励已领取") || lower.contains("already claimed"),
        lower.contains("checkredpoint")
            || lower.contains("updateredpoint")
            || lower.contains("update_red_point"),
    ]
}

impl CompatibilityMetrics {
    pub(crate) fn record(&self, message: &str) {
        let [module, mount, vip, claimed, red_point] = compatibility_matches(message);
        if module {
            self.module_completions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if mount {
            self.loader_mounts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if vip {
            self.vip_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if claimed {
            self.vip_claimed_replies
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if red_point {
            self.red_point_updates
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(crate) fn summary(&self) -> String {
        let module_completions = self
            .module_completions
            .load(std::sync::atomic::Ordering::Relaxed);
        let loader_mounts = self
            .loader_mounts
            .load(std::sync::atomic::Ordering::Relaxed);
        let vip_requests = self.vip_requests.load(std::sync::atomic::Ordering::Relaxed);
        let vip_claimed = self
            .vip_claimed_replies
            .load(std::sync::atomic::Ordering::Relaxed);
        let red_point_updates = self
            .red_point_updates
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut output = format!(
            "Compatibility: module_complete={module_completions} loader_mount_trace={loader_mounts}\nVIP trace matches: requests={vip_requests} claimed_text={vip_claimed} red_point_text={red_point_updates} (zero does not imply a missing reply/update)\n"
        );
        if module_completions > 0 && loader_mounts == 0 {
            output.push_str(
                "Loader finding=动态资源已报告完成，但未观察到显示列表挂载追踪；界面缺失时需核对 Loader.load() 挂载时序\n",
            );
        }
        if vip_claimed > 0 && red_point_updates == 0 {
            output.push_str(
                "VIP finding=日志含已领取提示，未匹配到红点刷新文本；这些文本计数不能证明回调是否执行\n",
            );
        }
        output
    }
}

pub(crate) struct RedactingLogBackend {
    pub(crate) events: RuntimeEventSender,
    pub(crate) traces: Arc<Mutex<VecDeque<String>>>,
    pub(crate) secrets: Arc<Mutex<Vec<String>>>,
    pub(crate) compatibility: Arc<CompatibilityMetrics>,
}

impl LogBackend for RedactingLogBackend {
    fn avm_trace(&self, message: &str) {
        self.record("trace", message);
    }

    fn avm_warning(&self, message: &str) {
        self.record("warning", message);
    }
}

impl RedactingLogBackend {
    fn record(&self, level: &str, message: &str) {
        if message == "InitGameCmd"
            || message.starts_with("initgame ")
            || message.contains("nfLoadBundleAssetsComplete")
        {
            let _ = self.events.send(RuntimeEvent::InitializationProgress);
        }
        register_dynamic_token(message, &self.secrets);
        self.compatibility.record(message);
        let line = format!("{level}: {}", redact(message, &self.secrets));
        tracing::info!(target: "zm_swf", "{line}");
        let mut traces = self.traces.lock().unwrap();
        if traces.len() >= 160 {
            traces.pop_front();
        }
        traces.push_back(line);
    }
}

pub(crate) fn register_dynamic_token(message: &str, secrets: &Arc<Mutex<Vec<String>>>) {
    let Some((_, candidate)) = message.rsplit_once("token:") else {
        return;
    };
    let candidate = candidate.trim();
    if candidate.matches('|').count() >= 4 && !candidate.starts_with("Error") {
        let mut values = secrets.lock().unwrap();
        if !values.iter().any(|value| value == candidate) {
            values.push(candidate.to_owned());
        }
    }
}

pub(crate) fn redact(value: &str, secrets: &Arc<Mutex<Vec<String>>>) -> String {
    let mut redacted = Cow::Borrowed(value);
    for secret in secrets
        .lock()
        .unwrap()
        .iter()
        .filter(|value| !value.is_empty())
    {
        if let Some(first_match) = redacted.find(secret) {
            // Reuse the first match instead of searching the unchanged prefix
            // again. Each secret still applies to the previous replacement's
            // output, preserving overlapping-secret and placeholder semantics.
            let mut replaced = String::with_capacity(redacted.len());
            replaced.push_str(&redacted[..first_match]);
            replaced.push_str("<redacted>");
            let rest = &redacted[first_match + secret.len()..];
            let mut copied_to = 0;
            for (offset, _) in rest.match_indices(secret) {
                replaced.push_str(&rest[copied_to..offset]);
                replaced.push_str("<redacted>");
                copied_to = offset + secret.len();
            }
            replaced.push_str(&rest[copied_to..]);
            redacted = Cow::Owned(replaced);
        }
    }
    redacted.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_resource_metrics_exclude_paths_and_error_payloads() {
        let metrics = ResourceMetrics::default();
        metrics.record_success(
            "assets/private.swf?token=secret",
            true,
            Duration::from_millis(5),
        );
        metrics.record_failure("assets/failed.swf", "private response payload");
        let summary = metrics.performance_summary();
        assert!(summary.contains("cache_hits=1 downloads=0 failures=1"));
        assert!(!summary.contains("assets/"));
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("payload"));
        assert!(metrics.summary().contains("Resource: "));
    }

    #[test]
    fn resource_activity_tracks_retries_cache_hits_and_cancelled_observers() {
        let metrics = Arc::new(ResourceMetrics::default());
        let download = metrics.begin_load();
        let cached = metrics.begin_load();
        let observer = download.observer();
        let report = |attempt, bytes_loaded| ResourceProgress {
            attempt,
            bytes_loaded,
            bytes_total: None,
            cache_hit: false,
        };
        observer(report(1, 4));
        observer(report(1, 9));
        observer(report(1, 9)); // Repeated totals must not double-count bytes.
        observer(report(1, 4)); // Neither may an out-of-order notification.
        observer(report(1, 9));
        observer(report(2, 0));
        observer(report(2, 2)); // Retried bytes were actually received again.
        observer(report(1, 9));
        cached.observer()(ResourceProgress {
            cache_hit: true,
            bytes_loaded: 1000,
            bytes_total: Some(1000),
            attempt: 0,
        });
        assert_eq!(
            metrics.loading_progress(),
            ResourceLoadingProgress {
                pending_requests: 2,
                received_bytes: 11,
            },
        );
        drop(cached);
        assert_eq!(metrics.loading_progress().pending_requests, 1);
        drop(download); // Same drop path as a cancelled AVM future.
        observer(report(2, 100));
        assert_eq!(
            metrics.loading_progress(),
            ResourceLoadingProgress {
                pending_requests: 0,
                received_bytes: 11,
            },
        );
    }

    // Keep the pre-optimization algorithms independent for equivalence checks
    // and the opt-in microbenchmark below.
    fn original_redact(value: &str, secrets: &Arc<Mutex<Vec<String>>>) -> String {
        let mut redacted = value.to_owned();
        for secret in secrets
            .lock()
            .unwrap()
            .iter()
            .filter(|value| !value.is_empty())
        {
            redacted = redacted.replace(secret, "<redacted>");
        }
        redacted
    }

    fn original_compatibility_matches(message: &str) -> [bool; 5] {
        let lower = message.to_ascii_lowercase();
        [
            lower.contains("resourceloadcomplete") || lower.contains("loadbundleassetscomplete"),
            lower.contains("addchild") || lower.contains("added_to_stage"),
            lower.contains("viphandler") || lower.contains("getdailyreward"),
            message.contains("今日奖励已领取") || lower.contains("already claimed"),
            lower.contains("checkredpoint")
                || lower.contains("updateredpoint")
                || lower.contains("update_red_point"),
        ]
    }

    #[test]
    fn redaction_matches_original_for_overlaps_unicode_and_empty_secrets() {
        let messages = [
            "",
            "plain loading message",
            "aaaa aa a",
            "token=abcabc ab abc",
            "令牌=测试账号🙂|secret|测试账号🙂",
            "<redacted> redacted token",
            "Token TOKEN token",
        ];
        let secret_sets = [
            vec![],
            vec![""],
            vec!["missing", "also-missing"],
            vec!["aa", "a"],
            vec!["a", "aa"],
            vec!["abc", "ab"],
            vec!["ab", "abc"],
            vec!["测试账号🙂", "secret", ""],
            vec!["token", "redacted", "<redacted>"],
        ];
        for secret_set in secret_sets {
            let secrets = Arc::new(Mutex::new(
                secret_set.into_iter().map(str::to_owned).collect(),
            ));
            for message in messages {
                assert_eq!(
                    redact(message, &secrets),
                    original_redact(message, &secrets)
                );
            }
        }
    }

    #[test]
    fn compatibility_matches_original_case_rules_without_changing_counts() {
        let messages = [
            "RESOURCELOADCOMPLETE resourceLoadComplete loadBundleAssetsComplete",
            "addChild ADDED_TO_STAGE vipHandler getDailyReward",
            "今日奖励已领取 ALREADY CLAIMED checkRedPoint updateRedPoint UPDATE_RED_POINT",
            "xReSoUrCeLoAdCoMpLeTex 🙂 VIPHANDLER 中文",
            "今日奖励尚未领取 update-red-point getdailyrewards",
            "plain loading message",
            "",
        ];
        let metrics = CompatibilityMetrics::default();
        let mut expected = [0; 5];
        for message in messages {
            let original = original_compatibility_matches(message);
            assert_eq!(compatibility_matches(message), original);
            metrics.record(message);
            for (count, matched) in expected.iter_mut().zip(original) {
                *count += u64::from(matched);
            }
        }
        use std::sync::atomic::Ordering;
        assert_eq!(
            [
                metrics.module_completions.load(Ordering::Relaxed),
                metrics.loader_mounts.load(Ordering::Relaxed),
                metrics.vip_requests.load(Ordering::Relaxed),
                metrics.vip_claimed_replies.load(Ordering::Relaxed),
                metrics.red_point_updates.load(Ordering::Relaxed),
            ],
            expected,
        );
    }

    #[test]
    fn long_messages_and_short_messages_after_them_keep_all_matches() {
        let long_message_bytes = 16 * 1_024;
        for length in [
            long_message_bytes - 64,
            long_message_bytes,
            long_message_bytes + 1,
        ] {
            let message = format!("{} VIPHANDLER 今日奖励已领取", "x".repeat(length));
            assert_eq!(
                compatibility_matches(&message),
                original_compatibility_matches(&message)
            );
        }
        assert_eq!(compatibility_matches("plain message"), [false; 5]);
        assert_eq!(
            compatibility_matches("getDailyReward"),
            [false, false, true, false, false]
        );
    }

    #[test]
    fn refreshed_tokens_are_registered_once_and_redacted_like_the_original() {
        let secrets = Arc::new(Mutex::new(vec![String::new()]));
        let message = "接受数据来自平台的token:1|account|nickname|1234567890|signature";
        register_dynamic_token(message, &secrets);
        register_dynamic_token(message, &secrets);
        assert_eq!(secrets.lock().unwrap().len(), 2);
        assert_eq!(
            redact(message, &secrets),
            "接受数据来自平台的token:<redacted>"
        );
        assert_eq!(
            redact(message, &secrets),
            original_redact(message, &secrets)
        );
    }

    #[test]
    #[ignore = "release-only synthetic comparison; run with --release --ignored --nocapture"]
    // An explicitly requested debug run must fail instead of reporting
    // misleading unoptimized timings; ordinary test runs skip this benchmark.
    #[allow(clippy::assertions_on_constants)]
    fn log_processing_microbenchmark() {
        use std::hint::black_box;

        assert!(
            !cfg!(debug_assertions),
            "run this microbenchmark with --release"
        );

        fn measure<T>(iterations: usize, operation: &mut impl FnMut() -> T) -> f64 {
            let started = Instant::now();
            for _ in 0..iterations {
                black_box(operation());
            }
            started.elapsed().as_nanos() as f64 / iterations as f64
        }

        fn compare<T>(
            label: &str,
            iterations: usize,
            mut original: impl FnMut() -> T,
            mut optimized: impl FnMut() -> T,
        ) {
            for _ in 0..32 {
                black_box(original());
                black_box(optimized());
            }
            let mut before = Vec::new();
            let mut after = Vec::new();
            for round in 0..7 {
                if round % 2 == 0 {
                    before.push(measure(iterations, &mut original));
                    after.push(measure(iterations, &mut optimized));
                } else {
                    after.push(measure(iterations, &mut optimized));
                    before.push(measure(iterations, &mut original));
                }
            }
            before.sort_by(f64::total_cmp);
            after.sort_by(f64::total_cmp);
            println!(
                "{label}: iterations={iterations} original_ns_per_op={:.1} optimized_ns_per_op={:.1} optimized_over_original={:.3}",
                before[3],
                after[3],
                after[3] / before[3].max(1.0),
            );
        }

        println!(
            "Synthetic microbenchmark only: wall time per call, seven alternating rounds, median reported. This does not measure actual game performance or allocation counts."
        );
        let cases = [
            ("short_no_hit", "普通资源加载日志 frame ready".to_owned()),
            ("short_hit", "VIPHANDLER 今日奖励已领取 token=benchmark-secret-00".to_owned()),
            ("long_no_hit", "普通资源加载日志 frame ready | ".repeat(256)),
            ("long_hit", "RESOURCELOADCOMPLETE VIPHANDLER 今日奖励已领取 updateRedPoint token=benchmark-secret-00 | ".repeat(128)),
            ("oversized_hit", "VIPHANDLER token=benchmark-secret-00 | ".repeat(1_024)),
        ];
        for (name, message) in cases {
            let iterations = (2_000_000 / message.len().max(1)).clamp(128, 20_000);
            assert_eq!(
                compatibility_matches(&message),
                original_compatibility_matches(&message)
            );
            compare(
                &format!("compatibility/{name}/bytes={}", message.len()),
                iterations,
                || original_compatibility_matches(black_box(&message)),
                || compatibility_matches(black_box(&message)),
            );
            for secret_count in [0, 2, 20] {
                let secrets = Arc::new(Mutex::new(
                    (0..secret_count)
                        .map(|index| format!("benchmark-secret-{index:02}"))
                        .collect(),
                ));
                assert_eq!(
                    redact(&message, &secrets),
                    original_redact(&message, &secrets)
                );
                compare(
                    &format!(
                        "redact/{name}/secrets={secret_count}/bytes={}",
                        message.len()
                    ),
                    iterations,
                    || original_redact(black_box(&message), black_box(&secrets)),
                    || redact(black_box(&message), black_box(&secrets)),
                );
            }
        }
    }

    #[test]
    fn non_due_updates_are_counted_without_inventing_ticks_or_renders() {
        let mut metrics = FrameMetrics::default();
        metrics.record(
            Instant::now(),
            FrameUpdateTimings {
                update: Duration::from_millis(5),
                input: Duration::from_millis(1),
                tasks: [
                    TaskPollReport {
                        elapsed: Duration::from_millis(1),
                        polls: 3,
                        peak_queue_depth: 9,
                        remaining: 2,
                        max_single_poll: Duration::from_micros(700),
                    },
                    TaskPollReport {
                        elapsed: Duration::from_millis(2),
                        polls: 2,
                        peak_queue_depth: 2,
                        remaining: 0,
                        max_single_poll: Duration::from_micros(900),
                    },
                ],
                game_target_fps: 30.0,
                ..Default::default()
            },
        );
        let summary = metrics.summary();
        assert!(summary.contains("updates=1 ticks=0 render_submissions=0"));
        assert!(
            summary.contains("polls=5 queued_tasks=0 peak_queue_depth=9 max_single_poll_ms=0.900")
        );
        assert!(summary.contains("Player update CPU wall: sample_count=1 total_count=1"));
        assert!(
            summary
                .contains("Player task_poll CPU wall: sample_count=1 total_count=1 p50_ms=3.000")
        );
        assert!(summary.contains("Player player_tick CPU wall: sample_count=0 total_count=0"));
        assert!(summary.contains("Player schedule_late: sample_count=0 total_count=0"));
        assert!(!summary.contains("actual_fps="));
    }

    #[test]
    fn due_tick_and_render_submission_have_separate_samples() {
        let mut metrics = FrameMetrics::default();
        metrics.record(Instant::now(), FrameUpdateTimings::default());
        metrics.record(
            Instant::now(),
            FrameUpdateTimings {
                update: Duration::from_millis(8),
                player_tick: Some(Duration::from_millis(4)),
                render_submit: Some(Duration::from_millis(2)),
                schedule_late: Some(Duration::from_millis(3)),
                game_target_fps: 60.0,
                ..Default::default()
            },
        );
        let summary = metrics.summary();
        assert!(summary.contains("updates=2 ticks=1 render_submissions=1"));
        assert!(
            summary
                .contains("Player player_tick CPU wall: sample_count=1 total_count=1 p50_ms=4.000")
        );
        assert!(summary.contains("Player render_submit CPU wall (not GPU execution): sample_count=1 total_count=1 p50_ms=2.000"));
        assert!(
            summary.contains("Player schedule_late: sample_count=1 total_count=1 p50_ms=3.000")
        );
        assert!(summary.contains("game_target_fps=60.00"));
    }

    #[test]
    fn source_frame_rate_is_not_overwritten_by_dynamic_game_target() {
        let mut metrics = FrameMetrics::new(24.0);
        for target in [30.0, 60.0, 125.0] {
            metrics.record(
                Instant::now(),
                FrameUpdateTimings {
                    game_target_fps: target,
                    ..Default::default()
                },
            );
            let summary = metrics.summary();
            assert!(summary.contains("source_fps=24.00"));
            assert!(summary.contains(&format!("game_target_fps={target:.2}")));
        }
    }

    #[test]
    fn session_peaks_survive_recent_window_eviction_with_their_update_times() {
        let start = Instant::now();
        let mut metrics = FrameMetrics::new(30.0);
        metrics.record(start, FrameUpdateTimings::default());
        metrics.record(
            start + Duration::from_millis(250),
            FrameUpdateTimings {
                update: Duration::from_millis(1_600),
                input: Duration::from_millis(1),
                tasks: [
                    TaskPollReport {
                        elapsed: Duration::from_millis(1_535),
                        polls: 1,
                        max_single_poll: Duration::from_millis(1_535),
                        ..Default::default()
                    },
                    TaskPollReport::default(),
                ],
                player_tick: Some(Duration::from_millis(60)),
                render_submit: Some(Duration::from_millis(1)),
                schedule_late: Some(Duration::from_millis(1_529)),
                game_target_fps: 30.0,
            },
        );
        for millis in 2_000..3_200 {
            metrics.record(
                start + Duration::from_millis(millis),
                FrameUpdateTimings {
                    update: Duration::from_millis(1),
                    input: Duration::from_micros(50),
                    player_tick: Some(Duration::from_micros(500)),
                    render_submit: Some(Duration::from_micros(100)),
                    schedule_late: Some(Duration::from_micros(100)),
                    game_target_fps: 30.0,
                    ..Default::default()
                },
            );
        }

        let summary = metrics.summary_at(start + Duration::from_secs(4));
        assert!(summary.contains("session_elapsed_s=4.000"));
        assert!(summary.contains("update_hz=300.50"));
        let recent_update = summary
            .lines()
            .find(|line| line.starts_with("Player update CPU wall:"))
            .unwrap();
        assert!(recent_update.contains("sample_count=1200 total_count=1202"));
        assert!(recent_update.ends_with("peak_ms=1.000"));
        for (stage, peak_ms) in [
            ("update_cpu_wall", "1600.000"),
            ("input_cpu_wall", "1.000"),
            ("task_poll_cpu_wall", "1535.000"),
            ("player_tick_cpu_wall", "60.000"),
            ("render_submit_cpu_wall", "1.000"),
            ("schedule_late", "1529.000"),
        ] {
            assert!(summary.contains(&format!(
                "Player session peak: stage={stage} peak_ms={peak_ms} at_update_start_s=0.250"
            )));
        }
    }

    #[test]
    fn absent_stages_have_no_session_peak_time() {
        let start = Instant::now();
        let mut metrics = FrameMetrics::default();
        let empty = metrics.summary_at(start);
        assert!(empty.contains("session_elapsed_s=0.000"));
        assert!(empty.contains("update_hz=0.00 tick_hz=0.00"));
        metrics.record(start, FrameUpdateTimings::default());
        let summary = metrics.summary_at(start + Duration::from_secs(2));
        assert!(summary.contains("session_elapsed_s=2.000"));
        assert!(summary.contains("stage=update_cpu_wall peak_ms=0.000 at_update_start_s=0.000"));
        assert!(
            summary.contains("stage=player_tick_cpu_wall peak_ms=0.000 at_update_start_s=none")
        );
        assert!(
            summary.contains("stage=render_submit_cpu_wall peak_ms=0.000 at_update_start_s=none")
        );
    }
}
