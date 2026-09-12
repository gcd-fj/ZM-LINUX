use crate::runtime::{RuntimeEvent, RuntimeEventSender};
use ruffle_core::backend::log::LogBackend;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zm_core::TimingSamples;

#[derive(Debug, Default)]
pub(crate) struct ResourceMetrics {
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
        for entry in self.recent.lock().unwrap().iter() {
            output.push_str("Resource: ");
            output.push_str(entry);
            output.push('\n');
        }
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
    frame_rate: f64,
    started_at: Option<Instant>,
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
    pub(crate) frame_rate: f64,
}

impl FrameMetrics {
    pub(crate) fn record(&mut self, started_at: Instant, sample: FrameUpdateTimings) {
        self.started_at.get_or_insert(started_at);
        self.updates = self.updates.saturating_add(1);
        self.update.record(sample.update);
        self.input.record(sample.input);
        self.task_poll.record(
            sample.tasks[0]
                .elapsed
                .saturating_add(sample.tasks[1].elapsed),
        );
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
        if sample.frame_rate.is_finite() && sample.frame_rate > 0.0 {
            self.frame_rate = sample.frame_rate;
        }
    }

    pub(crate) fn summary(&self) -> String {
        let elapsed_secs = self
            .started_at
            .map(|started| started.elapsed().as_secs_f64().max(0.001))
            .unwrap_or(1.0);
        let mut output = format!(
            "Frames (session): source_fps={:.2} update_hz={:.2} tick_hz={:.2} updates={} ticks={} render_submissions={} (tick_hz is host tick calls, not AVM frame rate)\nLocal tasks (session): polls={} queued_tasks={} peak_queue_depth={} max_single_poll_ms={:.3} (poll budget is cooperative, not a hard limit)\n",
            self.frame_rate,
            self.updates as f64 / elapsed_secs,
            self.ticks as f64 / elapsed_secs,
            self.updates,
            self.ticks,
            self.render_submissions,
            self.task_polls,
            self.queued_tasks,
            self.peak_queue_depth,
            self.max_single_poll.as_secs_f64() * 1_000.0,
        );
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

impl CompatibilityMetrics {
    pub(crate) fn record(&self, message: &str) {
        let lower = message.to_ascii_lowercase();
        if lower.contains("resourceloadcomplete") || lower.contains("loadbundleassetscomplete") {
            self.module_completions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if lower.contains("addchild") || lower.contains("added_to_stage") {
            self.loader_mounts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if lower.contains("viphandler") || lower.contains("getdailyreward") {
            self.vip_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if message.contains("今日奖励已领取") || lower.contains("already claimed") {
            self.vip_claimed_replies
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if lower.contains("checkredpoint")
            || lower.contains("updateredpoint")
            || lower.contains("update_red_point")
        {
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

#[cfg(test)]
mod tests {
    use super::*;

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
                frame_rate: 30.0,
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
                frame_rate: 60.0,
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
        assert!(summary.contains("source_fps=60.00"));
    }
}
