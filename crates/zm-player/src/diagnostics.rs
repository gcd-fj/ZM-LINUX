use crate::runtime::{RuntimeEvent, RuntimeEventSender};
use ruffle_core::backend::log::LogBackend;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
pub(crate) struct ResourceMetrics {
    cache_hits: std::sync::atomic::AtomicU64,
    downloads: std::sync::atomic::AtomicU64,
    failures: std::sync::atomic::AtomicU64,
    dynamic_modules: std::sync::atomic::AtomicU64,
    recent: Mutex<VecDeque<String>>,
}

impl ResourceMetrics {
    pub(crate) fn record_success(&self, resource: &str, cache_hit: bool) {
        let counter = if cache_hit {
            &self.cache_hits
        } else {
            &self.downloads
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let mut output = format!(
            "Resources: cache_hits={hits} downloads={downloads} failures={failures} dynamic_swf_ready={dynamic_modules}\n"
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
    samples: VecDeque<Duration>,
    rendered_frames: u64,
    ticks: u64,
    frame_rate: f64,
    started_at: Option<Instant>,
}

impl FrameMetrics {
    pub(crate) fn record(&mut self, elapsed: Duration, rendered: bool, frame_rate: f64) {
        self.started_at.get_or_insert_with(Instant::now);
        if self.samples.len() >= 120 {
            self.samples.pop_front();
        }
        self.samples.push_back(elapsed);
        self.ticks += 1;
        self.rendered_frames += u64::from(rendered);
        if frame_rate.is_finite() && frame_rate > 0.0 {
            self.frame_rate = frame_rate;
        }
    }

    pub(crate) fn summary(&self) -> String {
        let average_ms = if self.samples.is_empty() {
            0.0
        } else {
            self.samples.iter().map(Duration::as_secs_f64).sum::<f64>() * 1000.0
                / self.samples.len() as f64
        };
        let peak_ms = self
            .samples
            .iter()
            .map(Duration::as_secs_f64)
            .fold(0.0, f64::max)
            * 1000.0;
        let actual_fps = self
            .started_at
            .map(|started| self.ticks as f64 / started.elapsed().as_secs_f64().max(0.001))
            .unwrap_or(0.0);
        format!(
            "Frames: source_fps={:.2} actual_fps={actual_fps:.2} ticks={} renders={} avg_tick_ms={average_ms:.2} peak_tick_ms={peak_ms:.2}\n",
            self.frame_rate, self.ticks, self.rendered_frames
        )
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
