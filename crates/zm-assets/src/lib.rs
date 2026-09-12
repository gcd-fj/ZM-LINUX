use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};
use zm_core::{GameKind, Result, TimingSamples, ZmError};

mod swf_patch;

const HOME_URL: &str = "https://www.4399.com/flash/zmhj.htm";
const PATCH_VERSION: u32 = 5;
const ZM4_BRIDGE_ABC: &[u8] = include_bytes!("../../../assets/bridge/ZmLinuxZm4Bridge.abc");
const ZM5_BRIDGE_ABC: &[u8] = include_bytes!("../../../assets/bridge/ZmLinuxZm5Bridge.abc");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameVersion {
    pub game: GameKind,
    pub file_name: String,
    pub page_url: String,
    pub swf_url: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameAsset {
    pub version: GameVersion,
    pub path: PathBuf,
    /// The validated or freshly patched bytes, shared with the player.
    pub main_swf_bytes: Arc<[u8]>,
    pub sha256: String,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAsset {
    pub bytes: Vec<u8>,
    pub cache_hit: bool,
}

/// Actual response-body bytes received in the current download attempt.
/// A missing total stays unknown until EOF; cache hits use attempt zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceProgress {
    pub attempt: u8,
    pub bytes_loaded: u64,
    pub bytes_total: Option<u64>,
    pub cache_hit: bool,
}

/// Observers should only forward data to a bounded queue or watch channel.
/// They may run on any polling thread and must not execute UI work.
pub type ResourceProgressCallback = Arc<dyn Fn(ResourceProgress) + Send + Sync>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    Game(GameKind),
    All,
}

#[async_trait]
pub trait AssetManager: Send + Sync {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion>;
    async fn ensure_game(&self, game: GameKind) -> Result<GameAsset>;
    async fn fetch_resource(&self, game: GameKind, resource: &str) -> Result<RuntimeAsset>;
    async fn fetch_resource_with_progress(
        &self,
        game: GameKind,
        resource: &str,
        progress: ResourceProgressCallback,
    ) -> Result<RuntimeAsset> {
        let asset = self.fetch_resource(game, resource).await?;
        progress(ResourceProgress {
            attempt: u8::from(!asset.cache_hit),
            bytes_loaded: asset.bytes.len() as u64,
            bytes_total: Some(asset.bytes.len() as u64),
            cache_hit: asset.cache_hit,
        });
        Ok(asset)
    }
    async fn clear_cache(&self, scope: CacheScope) -> Result<()>;
    /// Diagnostics may be unsupported by alternate managers and test doubles.
    fn performance_summary(&self) -> String {
        String::new()
    }
}

/// Bounded samples shared by manager clones, across games and launch sessions.
#[derive(Default)]
struct AssetPerformance {
    cache_validation: AssetSamples,
    lifecycle_wait: AssetSamples,
    resource_registry_wait: AssetSamples,
    resource_lock_wait: AssetSamples,
    version_lookup: AssetSamples,
    network_attempt: AssetSamples,
    swf_patch: AssetSamples,
    main_write: AssetSamples,
    runtime_write: AssetSamples,
    runtime_cache_read: AssetSamples,
}

#[derive(Default)]
struct AssetSamples(StdMutex<TimingSamples>);

impl AssetSamples {
    fn record(&self, elapsed: Duration) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(elapsed);
    }

    fn summary(&self, label: &str) -> String {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summary(label)
    }
}

impl AssetPerformance {
    fn summary(&self) -> String {
        let mut output = String::from(
            "Asset performance (manager lifetime; both games; failed/cancelled attempts included):\n",
        );
        for (label, samples) in [
            ("Asset cache validation", &self.cache_validation),
            ("Asset lifecycle lock wait", &self.lifecycle_wait),
            (
                "Asset resource registry lock wait",
                &self.resource_registry_wait,
            ),
            ("Asset resource lock wait", &self.resource_lock_wait),
            ("Asset version lookup", &self.version_lookup),
            ("Asset network attempt", &self.network_attempt),
            ("Asset SWF patch and hashes", &self.swf_patch),
            ("Asset main write worker", &self.main_write),
            ("Asset runtime write worker", &self.runtime_write),
            ("Asset runtime cache read", &self.runtime_cache_read),
        ] {
            output.push_str(samples.summary(label).trim_end());
            output.push('\n');
        }
        output
    }
}

/// Record early errors and cancelled waits as well as successful operations.
struct AssetTiming<'a> {
    samples: &'a AssetSamples,
    started: Instant,
}

impl<'a> AssetTiming<'a> {
    fn new(samples: &'a AssetSamples) -> Self {
        Self {
            samples,
            started: Instant::now(),
        }
    }
}

impl Drop for AssetTiming<'_> {
    fn drop(&mut self) {
        self.samples.record(self.started.elapsed());
    }
}

#[derive(Clone)]
pub struct OfficialAssetManager {
    client: Client,
    lifecycle: Arc<RwLock<()>>,
    cache_root: PathBuf,
    in_flight: Arc<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
    runtime_dirs: Arc<RwLock<HashMap<GameKind, PathBuf>>>,
    resource_roots: Arc<HashMap<GameKind, Url>>,
    performance: Arc<AssetPerformance>,
    #[cfg(test)]
    test_responses: Option<Arc<TestResponses>>,
}

#[cfg(test)]
struct TestResponses {
    values: Mutex<std::collections::VecDeque<Result<Vec<u8>>>>,
    requests: std::sync::atomic::AtomicUsize,
}

struct DownloadFailure {
    error: ZmError,
    status: Option<StatusCode>,
    transient: bool,
}

impl DownloadFailure {
    fn retryable(&self) -> bool {
        match self.status {
            Some(status) => {
                (status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::TOO_MANY_REQUESTS)
                    || status.is_server_error()
            }
            None => self.transient,
        }
    }
}

impl From<reqwest::Error> for DownloadFailure {
    fn from(error: reqwest::Error) -> Self {
        Self {
            status: error.status(),
            transient: error.is_timeout()
                || error.is_connect()
                || error.is_request()
                || error.is_body()
                // Truncated or compressed response bodies can surface as
                // decode errors when reqwest collects the GET response.
                || error.is_decode(),
            error: ZmError::Network(error.to_string()),
        }
    }
}

impl OfficialAssetManager {
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .user_agent("ZM-LINUX/0.1")
            .build()
            .map_err(|e| ZmError::Network(e.to_string()))?;
        Ok(Self {
            lifecycle: Arc::new(RwLock::new(())),
            client,
            cache_root: cache_root.into(),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            runtime_dirs: Arc::new(RwLock::new(HashMap::new())),
            resource_roots: Arc::new(Self::default_resource_roots()),
            performance: Arc::new(AssetPerformance::default()),
            #[cfg(test)]
            test_responses: None,
        })
    }
    fn game_dir(&self, game: GameKind) -> PathBuf {
        self.cache_root.join(game.slug())
    }
    fn default_resource_roots() -> HashMap<GameKind, Url> {
        [GameKind::Zm4, GameKind::Zm5]
            .into_iter()
            .map(|game| {
                (
                    game,
                    Url::parse(game.profile().resource_root).expect("static official URL"),
                )
            })
            .collect()
    }

    #[cfg(test)]
    fn with_test_responses(
        cache_root: impl Into<PathBuf>,
        responses: Vec<Result<Vec<u8>>>,
    ) -> Result<(Self, Arc<TestResponses>)> {
        let mut manager = Self::new(cache_root)?;
        let responses = Arc::new(TestResponses {
            values: Mutex::new(responses.into()),
            requests: std::sync::atomic::AtomicUsize::new(0),
        });
        manager.test_responses = Some(responses.clone());
        Ok((manager, responses))
    }
    fn standalone_url(game: GameKind) -> String {
        format!("{HOME_URL}?g={}", game.number())
    }

    async fn get_bytes(&self, url: &str, referer: Option<&str>) -> Result<Vec<u8>> {
        self.get_bytes_once(url, referer, None)
            .await
            .map_err(|failure| failure.error)
    }

    async fn get_bytes_once(
        &self,
        url: &str,
        referer: Option<&str>,
        progress: Option<(&ResourceProgressCallback, u8)>,
    ) -> std::result::Result<Vec<u8>, DownloadFailure> {
        let _timing = AssetTiming::new(&self.performance.network_attempt);
        #[cfg(test)]
        if let Some(responses) = &self.test_responses {
            responses
                .requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result = responses
                .values
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Err(ZmError::Network("测试响应已耗尽".into())))
                .map_err(|error| DownloadFailure {
                    error,
                    status: None,
                    transient: true,
                });
            if let (Ok(bytes), Some((callback, attempt))) = (&result, progress) {
                callback(ResourceProgress {
                    attempt,
                    bytes_loaded: bytes.len() as u64,
                    bytes_total: Some(bytes.len() as u64),
                    cache_hit: false,
                });
            }
            return result;
        }
        let mut request = self.client.get(url);
        if let Some(value) = referer {
            request = request.header("Referer", value);
        }
        let mut response = request
            .send()
            .await
            .map_err(DownloadFailure::from)?
            .error_for_status()
            .map_err(DownloadFailure::from)?;
        if let Some((callback, attempt)) = progress {
            let total = response.content_length();
            callback(ResourceProgress {
                attempt,
                bytes_loaded: 0,
                bytes_total: total,
                cache_hit: false,
            });
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(DownloadFailure::from)? {
                if chunk.is_empty() {
                    continue;
                }
                bytes.extend_from_slice(&chunk);
                callback(ResourceProgress {
                    attempt,
                    bytes_loaded: bytes.len() as u64,
                    bytes_total: total,
                    cache_hit: false,
                });
            }
            if total != Some(bytes.len() as u64) {
                callback(ResourceProgress {
                    attempt,
                    bytes_loaded: bytes.len() as u64,
                    bytes_total: Some(bytes.len() as u64),
                    cache_hit: false,
                });
            }
            return Ok(bytes);
        }
        Ok(response
            .bytes()
            .await
            .map_err(DownloadFailure::from)?
            .to_vec())
    }

    async fn get_runtime_bytes(
        &self,
        url: &str,
        progress: Option<&ResourceProgressCallback>,
    ) -> Result<Vec<u8>> {
        let mut last_error = None;
        for (attempt, delay_ms) in [0, 250, 750].into_iter().enumerate() {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            if let Some(callback) = progress {
                callback(ResourceProgress {
                    attempt: (attempt + 1) as u8,
                    bytes_loaded: 0,
                    bytes_total: None,
                    cache_hit: false,
                });
            }
            match self
                .get_bytes_once(
                    url,
                    Some(HOME_URL),
                    progress.map(|callback| (callback, (attempt + 1) as u8)),
                )
                .await
            {
                Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                Ok(_) => last_error = Some("服务器返回了空资源".to_owned()),
                Err(failure) => {
                    let retryable = failure.retryable();
                    last_error = Some(failure.error.to_string());
                    if !retryable {
                        break;
                    }
                }
            }
            if attempt < 2 {
                tracing::warn!(
                    attempt = attempt + 1,
                    resource = %sanitized_resource_url(url),
                    "运行时资源下载失败，准备重试"
                );
            }
        }
        Err(ZmError::Network(format!(
            "运行时资源下载失败：{}",
            last_error.unwrap_or_else(|| "未知错误".into())
        )))
    }

    async fn runtime_resource_dir(&self, game: GameKind) -> PathBuf {
        if let Some(directory) = self.runtime_dirs.read().await.get(&game) {
            return directory.clone();
        }
        let manifest_path = self.game_dir(game).join("manifest.toml");
        let namespace = tokio::fs::read_to_string(manifest_path)
            .await
            .ok()
            .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
            .map(|manifest| resource_namespace(&manifest))
            .unwrap_or_else(|| format!("patch{PATCH_VERSION}-unknown"));
        let directory = self.game_dir(game).join("resources").join(namespace);
        // A concurrent publisher inserts its new namespace while switching the
        // manifest. Prefer that entry over a manifest read started beforehand.
        self.runtime_dirs
            .write()
            .await
            .entry(game)
            .or_insert_with(|| directory.clone())
            .clone()
    }

    async fn invalidate_runtime_resource_dir(&self, game: GameKind) {
        self.runtime_dirs.write().await.remove(&game);
    }

    async fn resource_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = {
            let _timing = AssetTiming::new(&self.performance.resource_registry_wait);
            self.in_flight.lock().await
        };
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(path.to_owned(), Arc::downgrade(&lock));
        lock
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: String,
    raw_sha256: String,
    bridge_sha256: String,
    final_sha256: String,
    patch_version: u32,
    #[serde(default)]
    movie_url: String,
    #[serde(default)]
    page_url: String,
}

fn resource_namespace(manifest: &Manifest) -> String {
    let version_hash = digest(manifest.version.as_bytes());
    format!(
        "patch{}-{}-{}",
        manifest.patch_version,
        &version_hash[..12],
        &digest(manifest.bridge_sha256.as_bytes())[..12]
    )
}

#[async_trait]
impl AssetManager for OfficialAssetManager {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion> {
        let _timing = AssetTiming::new(&self.performance.version_lookup);
        let landing_url = Self::standalone_url(game);
        let landing = String::from_utf8_lossy(&self.get_bytes(&landing_url, Some(HOME_URL)).await?)
            .into_owned();
        let folder = game.profile().discovery_folder;
        let page_re = Regex::new(&format!(
            r#"(https://sda\.4399\.com/4399swf/upload_swf/{folder}/csya/\d+/\d+/[^\"'?\s;]+\.htm)"#
        ))
        .unwrap();
        let page_url = page_re
            .captures(&landing)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| {
                ZmError::Asset(format!("未在4399页面找到{}入口", game.display_name()))
            })?;
        let page =
            String::from_utf8_lossy(&self.get_bytes(&page_url, Some(HOME_URL)).await?).into_owned();
        let swf_re =
            Regex::new(r#"<param\s+name=[\"']movie[\"']\s+value=[\"']([^\"']+\.swf)[\"']"#)
                .unwrap();
        let file_name = swf_re
            .captures(&page)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| ZmError::Asset("未找到游戏SWF文件名".into()))?;
        let swf_url = Url::parse(&page_url)
            .and_then(|u| u.join(&file_name))
            .map_err(|e| ZmError::Asset(e.to_string()))?
            .to_string();
        Ok(GameVersion {
            game,
            file_name,
            page_url,
            swf_url,
        })
    }

    async fn ensure_game(&self, game: GameKind) -> Result<GameAsset> {
        let lifecycle = {
            let _timing = AssetTiming::new(&self.performance.lifecycle_wait);
            self.lifecycle.clone().read_owned().await
        };
        let lock = self
            .resource_lock(&self.game_dir(game).join("manifest.toml"))
            .await;
        let guard = {
            let _timing = AssetTiming::new(&self.performance.resource_lock_wait);
            lock.lock_owned().await
        };
        let (previous, version) = tokio::join!(self.cached_game(game), self.resolve_version(game));
        let version = match version {
            Ok(version) => version,
            Err(error) => return previous.ok_or(error),
        };
        if let Some(asset) = &previous
            && asset.version.swf_url == version.swf_url
        {
            return Ok(asset.clone());
        }
        match self.publish_game(game, version, lifecycle, guard).await {
            Ok(asset) => Ok(asset),
            Err(error) => {
                if previous.is_some() {
                    tracing::warn!(game = game.slug(), "更新失败，使用已校验的上一版缓存");
                }
                previous.ok_or(error)
            }
        }
    }

    async fn fetch_resource(&self, game: GameKind, resource: &str) -> Result<RuntimeAsset> {
        self.fetch_runtime_resource(game, resource, None).await
    }

    async fn fetch_resource_with_progress(
        &self,
        game: GameKind,
        resource: &str,
        progress: ResourceProgressCallback,
    ) -> Result<RuntimeAsset> {
        self.fetch_runtime_resource(game, resource, Some(progress))
            .await
    }

    async fn clear_cache(&self, scope: CacheScope) -> Result<()> {
        let _lifecycle = {
            let _timing = AssetTiming::new(&self.performance.lifecycle_wait);
            self.lifecycle.write().await
        };
        let target = match scope {
            CacheScope::Game(game) => {
                self.invalidate_runtime_resource_dir(game).await;
                self.game_dir(game)
            }
            CacheScope::All => {
                self.runtime_dirs.write().await.clear();
                self.cache_root.clone()
            }
        };
        if target.exists() {
            tokio::fs::remove_dir_all(&target)
                .await
                .map_err(|e| ZmError::io(&target, e))?;
        }
        Ok(())
    }

    fn performance_summary(&self) -> String {
        self.performance.summary()
    }
}

impl OfficialAssetManager {
    async fn fetch_runtime_resource(
        &self,
        game: GameKind,
        resource: &str,
        progress: Option<ResourceProgressCallback>,
    ) -> Result<RuntimeAsset> {
        let lifecycle = {
            let _timing = AssetTiming::new(&self.performance.lifecycle_wait);
            self.lifecycle.clone().read_owned().await
        };
        let resource = resource
            .split('?')
            .next()
            .unwrap_or(resource)
            .trim_start_matches('/');
        let path = Path::new(resource);
        if resource.is_empty()
            || resource.contains('\\')
            || resource.contains(':')
            || resource.contains("%")
            || resource.contains("://")
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(ZmError::Asset("拒绝不安全的资源路径".into()));
        }
        let local = self.runtime_resource_dir(game).await.join(path);
        let resource_lock = self.resource_lock(&local).await;
        let _resource_guard = {
            let _timing = AssetTiming::new(&self.performance.resource_lock_wait);
            resource_lock.lock_owned().await
        };
        let cached = {
            let _timing = AssetTiming::new(&self.performance.runtime_cache_read);
            tokio::fs::read(&local).await
        };
        match cached {
            Ok(bytes) => {
                if let Some(callback) = &progress {
                    callback(ResourceProgress {
                        attempt: 0,
                        bytes_loaded: bytes.len() as u64,
                        bytes_total: Some(bytes.len() as u64),
                        cache_hit: true,
                    });
                }
                return Ok(RuntimeAsset {
                    bytes,
                    cache_hit: true,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ZmError::io(&local, error)),
        }
        let url = self
            .resource_roots
            .get(&game)
            .ok_or_else(|| ZmError::Asset("缺少游戏资源根地址".into()))?
            .join(resource)
            .map_err(|e| ZmError::Asset(e.to_string()))?;
        let bytes = self
            .get_runtime_bytes(url.as_str(), progress.as_ref())
            .await?;
        let bytes = atomic_cache_write(
            &local,
            bytes,
            lifecycle,
            _resource_guard,
            self.performance.clone(),
        )
        .await?;
        Ok(RuntimeAsset {
            bytes,
            cache_hit: false,
        })
    }

    async fn cached_game(&self, game: GameKind) -> Option<GameAsset> {
        let _timing = AssetTiming::new(&self.performance.cache_validation);
        let dir = self.game_dir(game);
        tokio::task::spawn_blocking(move || {
            let raw = std::fs::read_to_string(dir.join("manifest.toml")).ok()?;
            let manifest: Manifest = toml::from_str(&raw).ok()?;
            if manifest.patch_version != PATCH_VERSION
                || manifest.bridge_sha256 != digest(bridge_abc(game))
                || manifest.final_sha256.len() != 64
                || !manifest.final_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            let url = Url::parse(&manifest.movie_url).ok()?;
            if url.scheme() != "https" || url.host_str() != Some("sda.4399.com") {
                return None;
            }
            let path = dir
                .join("versions")
                .join(format!("{}.swf", manifest.final_sha256));
            let bytes = std::fs::read(&path).ok()?;
            if digest(&bytes) != manifest.final_sha256 {
                return None;
            }
            Some(GameAsset {
                version: GameVersion {
                    game,
                    file_name: manifest.version,
                    page_url: manifest.page_url,
                    swf_url: manifest.movie_url,
                },
                path,
                main_swf_bytes: bytes.into(),
                sha256: manifest.final_sha256,
                cache_hit: true,
            })
        })
        .await
        .ok()
        .flatten()
    }

    async fn publish_game(
        &self,
        game: GameKind,
        version: GameVersion,
        lifecycle: tokio::sync::OwnedRwLockReadGuard<()>,
        guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<GameAsset> {
        let source = self
            .get_bytes(&version.swf_url, Some(&version.page_url))
            .await?;
        let dir = self.game_dir(game);
        let performance = self.performance.clone();
        let runtime_dirs = self.runtime_dirs.clone();
        tokio::task::spawn_blocking(move || {
            // Cancellation may drop the caller, but publication owns its locks
            // through the manifest switch and in-memory namespace refresh.
            let (_lifecycle, _guard) = (lifecycle, guard);
            let (raw_sha256, bytes, sha256) = {
                let _timing = AssetTiming::new(&performance.swf_patch);
                let raw_sha256 = digest(&source);
                let bytes = swf_patch::inject_bridge(&source, bridge_abc(game), game)?;
                let sha256 = digest(&bytes);
                (raw_sha256, bytes, sha256)
            };
            let main_swf_bytes: Arc<[u8]> = bytes.into();
            let path = dir.join("versions").join(format!("{sha256}.swf"));
            let manifest = Manifest {
                version: version.file_name.clone(),
                raw_sha256,
                bridge_sha256: digest(bridge_abc(game)),
                final_sha256: sha256.clone(),
                patch_version: PATCH_VERSION,
                movie_url: version.swf_url.clone(),
                page_url: version.page_url.clone(),
            };
            let namespace = dir.join("resources").join(resource_namespace(&manifest));
            let raw = toml::to_string_pretty(&manifest)
                .map_err(|error| ZmError::Asset(error.to_string()))?;
            {
                let _timing = AssetTiming::new(&performance.main_write);
                // Publish content before the pointer. Readers of the namespace
                // are excluded until the new pointer and namespace agree.
                atomic_write_sync(&path, &main_swf_bytes)?;
                let mut directories = runtime_dirs.blocking_write();
                atomic_write_sync(&dir.join("manifest.toml"), raw.as_bytes())?;
                directories.insert(game, namespace);
            }
            Ok(GameAsset {
                version,
                path,
                main_swf_bytes,
                sha256,
                cache_hit: false,
            })
        })
        .await
        .map_err(|error| ZmError::Asset(error.to_string()))?
    }
}

async fn atomic_cache_write(
    path: &Path,
    bytes: Vec<u8>,
    lifecycle: tokio::sync::OwnedRwLockReadGuard<()>,
    guard: tokio::sync::OwnedMutexGuard<()>,
    performance: Arc<AssetPerformance>,
) -> Result<Vec<u8>> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let _timing = AssetTiming::new(&performance.runtime_write);
        // Runtime resources are disposable cache entries. Atomic publication
        // is enough; fsync on every image and SWF delays their first display.
        let (_lifecycle, _guard) = (lifecycle, guard);
        atomic_cache_write_sync(&path, &bytes)?;
        Ok(bytes)
    })
    .await
    .map_err(|error| ZmError::Asset(error.to_string()))?
}

fn atomic_write_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| ZmError::Asset("缓存路径缺少父目录".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| ZmError::io(parent, error))?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| ZmError::io(parent, error))?;
    file.write_all(bytes)
        .map_err(|error| ZmError::io(path, error))?;
    file.as_file()
        .sync_all()
        .map_err(|error| ZmError::io(path, error))?;
    file.persist(path)
        .map_err(|error| ZmError::io(path, error.error))?;
    Ok(())
}

fn atomic_cache_write_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| ZmError::Asset("缓存路径缺少父目录".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| ZmError::io(parent, error))?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| ZmError::io(parent, error))?;
    file.write_all(bytes)
        .map_err(|error| ZmError::io(path, error))?;
    file.persist(path)
        .map_err(|error| ZmError::io(path, error.error))?;
    Ok(())
}

fn bridge_abc(game: GameKind) -> &'static [u8] {
    match game {
        GameKind::Zm4 => ZM4_BRIDGE_ABC,
        GameKind::Zm5 => ZM5_BRIDGE_ABC,
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sanitized_resource_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .map(|url| {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or(""),
                url.path()
            )
        })
        .unwrap_or_else(|| "<无效资源地址>".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        future::Future,
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::Poll,
    };

    enum HttpReply {
        Status(u16, &'static [u8]),
        Truncated,
        Stalled,
        Progressive {
            known_length: bool,
            release: std::sync::mpsc::Receiver<()>,
        },
    }

    struct LocalHttpServer {
        root: Url,
        requests: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl LocalHttpServer {
        fn new(replies: impl IntoIterator<Item = HttpReply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let root = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let worker_requests = requests.clone();
            let worker_stop = stop.clone();
            let mut replies: VecDeque<_> = replies.into_iter().collect();
            let worker = std::thread::spawn(move || {
                while !worker_stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("local HTTP accept failed: {error}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 1024];
                    while request.len() < 16 * 1024
                        && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                    {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(length) => request.extend_from_slice(&buffer[..length]),
                        }
                    }
                    worker_requests.fetch_add(1, Ordering::SeqCst);
                    let reply = replies
                        .pop_front()
                        .unwrap_or(HttpReply::Status(500, b"exhausted"));
                    let reply = match reply {
                        HttpReply::Progressive {
                            known_length,
                            release,
                        } => {
                            let length = if known_length {
                                "Content-Length: 9\r\n"
                            } else {
                                ""
                            };
                            let header =
                                format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n");
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(b"part");
                            // The consumer must observe this actual network prefix
                            // before the test allows the remaining bytes to arrive.
                            let _ = release.recv_timeout(Duration::from_secs(5));
                            let _ = stream.write_all(b"-done");
                            continue;
                        }
                        reply => reply,
                    };
                    let (status, body, length) = match reply {
                        HttpReply::Status(status, body) => (status, body, body.len()),
                        HttpReply::Truncated => (200, &b"partial"[..], 128),
                        HttpReply::Stalled => {
                            std::thread::sleep(Duration::from_millis(350));
                            (200, &b"late"[..], 4)
                        }
                        HttpReply::Progressive { .. } => unreachable!(),
                    };
                    let header = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                    );
                    // The timeout case deliberately closes the client first.
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
            });
            Self {
                root,
                requests,
                stop,
                worker: Some(worker),
            }
        }

        fn manager(&self, directory: &Path) -> OfficialAssetManager {
            let mut manager = OfficialAssetManager::new(directory).unwrap();
            manager.resource_roots = Arc::new(HashMap::from([(GameKind::Zm4, self.root.clone())]));
            manager.client = Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            manager
        }
    }

    impl Drop for LocalHttpServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    type ProgressLog = Arc<StdMutex<Vec<ResourceProgress>>>;

    fn observe_progress() -> (
        ProgressLog,
        ResourceProgressCallback,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let values = Arc::new(StdMutex::new(Vec::new()));
        let observed = values.clone();
        let (prefix, ready) = tokio::sync::oneshot::channel();
        let prefix = StdMutex::new(Some(prefix));
        let callback: ResourceProgressCallback = Arc::new(move |sample| {
            observed.lock().unwrap().push(sample);
            if sample.bytes_loaded == 4
                && !sample.cache_hit
                && let Some(prefix) = prefix.lock().unwrap().take()
            {
                let _ = prefix.send(());
            }
        });
        (values, callback, ready)
    }

    async fn write_manifest(manager: &OfficialAssetManager, game: GameKind, version: &str) {
        let directory = manager.game_dir(game);
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let manifest = Manifest {
            movie_url: format!("{}{}", game.profile().resource_root, version),
            page_url: HOME_URL.into(),
            version: version.into(),
            raw_sha256: "raw".into(),
            bridge_sha256: digest(bridge_abc(game)),
            final_sha256: "final".into(),
            patch_version: PATCH_VERSION,
        };
        tokio::fs::write(
            directory.join("manifest.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .await
        .unwrap();
        manager.invalidate_runtime_resource_dir(game).await;
    }

    async fn write_cached_fixture(
        manager: &OfficialAssetManager,
        game: GameKind,
        version: &str,
        bytes: &[u8],
    ) -> PathBuf {
        write_manifest(manager, game, version).await;
        let manifest_path = manager.game_dir(game).join("manifest.toml");
        let mut manifest: Manifest =
            toml::from_str(&tokio::fs::read_to_string(&manifest_path).await.unwrap()).unwrap();
        manifest.final_sha256 = digest(bytes);
        let path = manager
            .game_dir(game)
            .join("versions")
            .join(format!("{}.swf", manifest.final_sha256));
        atomic_write_sync(&path, bytes).unwrap();
        atomic_write_sync(
            &manifest_path,
            toml::to_string(&manifest).unwrap().as_bytes(),
        )
        .unwrap();
        path
    }

    fn version_responses(game: GameKind, file_name: &str) -> Vec<Result<Vec<u8>>> {
        vec![
            Ok(format!("{}index.htm", game.profile().resource_root).into_bytes()),
            Ok(format!("<param name='movie' value='{file_name}'>").into_bytes()),
        ]
    }

    fn main_swf_fixture() -> Vec<u8> {
        let mut body = vec![0x08, 0x00, 0x00, 0x18, 0x01, 0x00];
        let mut symbols = vec![1, 0, 0, 0];
        symbols.extend_from_slice(b"Main\0");
        body.extend_from_slice(&((76_u16 << 6) | symbols.len() as u16).to_le_bytes());
        body.extend_from_slice(&symbols);
        body.extend_from_slice(&(1_u16 << 6).to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        let mut source = b"FWS\x0a".to_vec();
        source.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        source.extend_from_slice(&body);
        source
    }

    async fn wait_for_published_content(path: &Path) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !tokio::fs::try_exists(path).await.unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("publication did not reach the namespace gate");
    }

    #[test]
    fn rejects_traversal() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let manager = OfficialAssetManager::new(dir.path()).unwrap();
        assert!(
            rt.block_on(manager.fetch_resource(GameKind::Zm4, "../secret"))
                .is_err()
        );
        assert!(
            rt.block_on(manager.fetch_resource(GameKind::Zm4, "file:///etc/passwd"))
                .is_err()
        );
    }
    #[test]
    fn sha256_is_stable() {
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn warm_launch_checks_version_and_keeps_verified_bytes_after_cache_removal() {
        let directory = tempfile::tempdir().unwrap();
        let game = GameKind::Zm4;
        let (manager, responses) = OfficialAssetManager::with_test_responses(
            directory.path(),
            version_responses(game, "main.swf"),
        )
        .unwrap();
        let expected = b"FWSverified-content";
        let path = write_cached_fixture(&manager, game, "main.swf", expected).await;
        let asset = manager.ensure_game(game).await.unwrap();
        assert!(asset.cache_hit);
        assert_eq!(asset.main_swf_bytes.as_ref(), expected);
        assert_eq!(responses.requests.load(Ordering::SeqCst), 2);
        let clone = asset.clone();
        assert!(Arc::ptr_eq(&asset.main_swf_bytes, &clone.main_swf_bytes));
        manager.clear_cache(CacheScope::All).await.unwrap();
        assert!(!path.exists());
        assert_eq!(asset.main_swf_bytes.as_ref(), expected);
    }

    #[tokio::test]
    async fn discovery_download_and_patch_failures_preserve_only_valid_previous_bytes() {
        let game = GameKind::Zm4;
        let offline = || Err(ZmError::Network("offline".into()));
        let mut download_failure = version_responses(game, "new.swf");
        download_failure.push(offline());
        let mut patch_failure = version_responses(game, "new.swf");
        patch_failure.push(Ok(b"invalid-swf".to_vec()));
        for values in [vec![offline()], download_failure, patch_failure] {
            let directory = tempfile::tempdir().unwrap();
            let (manager, _) =
                OfficialAssetManager::with_test_responses(directory.path(), values).unwrap();
            let expected = b"FWSprevious-validated-version";
            write_cached_fixture(&manager, game, "old.swf", expected).await;
            let previous_namespace = manager.runtime_resource_dir(game).await;
            let asset = manager.ensure_game(game).await.unwrap();
            assert!(asset.cache_hit);
            assert_eq!(asset.version.file_name, "old.swf");
            assert_eq!(asset.main_swf_bytes.as_ref(), expected);
            assert_eq!(manager.runtime_resource_dir(game).await, previous_namespace);
        }

        let directory = tempfile::tempdir().unwrap();
        let (manager, responses) = OfficialAssetManager::with_test_responses(
            directory.path(),
            vec![offline(), offline(), offline()],
        )
        .unwrap();
        assert!(manager.ensure_game(game).await.is_err());
        let path = write_cached_fixture(&manager, game, "old.swf", b"original").await;
        tokio::fs::write(path, b"corrupt").await.unwrap();
        assert!(manager.ensure_game(game).await.is_err());
        write_cached_fixture(&manager, game, "old.swf", b"original").await;
        let manifest_path = manager.game_dir(game).join("manifest.toml");
        let mut manifest: Manifest =
            toml::from_str(&tokio::fs::read_to_string(&manifest_path).await.unwrap()).unwrap();
        manifest.patch_version += 1;
        tokio::fs::write(&manifest_path, toml::to_string(&manifest).unwrap())
            .await
            .unwrap();
        assert!(manager.ensure_game(game).await.is_err());
        assert_eq!(responses.requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn fresh_publication_returns_the_exact_patched_content_and_refreshes_namespace() {
        for game in [GameKind::Zm4, GameKind::Zm5] {
            let directory = tempfile::tempdir().unwrap();
            let mut responses = version_responses(game, "main.swf");
            responses.push(Ok(main_swf_fixture()));
            let (manager, _) =
                OfficialAssetManager::with_test_responses(directory.path(), responses).unwrap();
            let unknown_namespace = manager.runtime_resource_dir(game).await;
            let asset = manager.ensure_game(game).await.unwrap();
            assert!(!asset.cache_hit);
            assert_eq!(
                asset.main_swf_bytes.as_ref(),
                tokio::fs::read(&asset.path).await.unwrap()
            );
            assert_eq!(asset.sha256, digest(&asset.main_swf_bytes));
            assert_ne!(manager.runtime_resource_dir(game).await, unknown_namespace);
            let cached = manager.cached_game(game).await.unwrap();
            assert_eq!(cached.main_swf_bytes, asset.main_swf_bytes);
        }
    }

    #[tokio::test]
    async fn cancelled_publisher_finishes_namespace_invalidation_before_the_next_launch() {
        let directory = tempfile::tempdir().unwrap();
        let game = GameKind::Zm4;
        let source = main_swf_fixture();
        let expected = swf_patch::inject_bridge(&source, bridge_abc(game), game).unwrap();
        let mut values = version_responses(game, "new.swf");
        values.push(Ok(source));
        values.extend(version_responses(game, "new.swf"));
        let (manager, responses) =
            OfficialAssetManager::with_test_responses(directory.path(), values).unwrap();
        write_cached_fixture(&manager, game, "old.swf", b"old-content").await;
        let old_namespace = manager.runtime_resource_dir(game).await;
        let namespace_gate = manager.runtime_dirs.write().await;
        let content_path = manager
            .game_dir(game)
            .join("versions")
            .join(format!("{}.swf", digest(&expected)));
        let publisher = manager.clone();
        let task = tokio::spawn(async move { publisher.ensure_game(game).await });
        wait_for_published_content(&content_path).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(manager.lifecycle.try_write().is_err());
        drop(namespace_gate);
        // The detached blocking worker must finish publication before giving up
        // its lifecycle guard, even though its launch future was cancelled.
        drop(manager.lifecycle.write().await);
        assert_ne!(
            manager.runtime_dirs.read().await.get(&game),
            Some(&old_namespace)
        );
        let asset = manager.ensure_game(game).await.unwrap();
        assert!(asset.cache_hit);
        assert_eq!(asset.main_swf_bytes.as_ref(), expected);
        assert_eq!(responses.requests.load(Ordering::SeqCst), 5);
        assert_ne!(manager.runtime_resource_dir(game).await, old_namespace);
    }

    #[tokio::test]
    async fn cache_clear_waits_for_cancelled_publication_and_removes_its_output() {
        let directory = tempfile::tempdir().unwrap();
        let game = GameKind::Zm4;
        let source = main_swf_fixture();
        let expected = swf_patch::inject_bridge(&source, bridge_abc(game), game).unwrap();
        let mut values = version_responses(game, "new.swf");
        values.push(Ok(source));
        let (manager, _) =
            OfficialAssetManager::with_test_responses(directory.path(), values).unwrap();
        let namespace_gate = manager.runtime_dirs.write().await;
        let content_path = manager
            .game_dir(game)
            .join("versions")
            .join(format!("{}.swf", digest(&expected)));
        let publisher = manager.clone();
        let task = tokio::spawn(async move { publisher.ensure_game(game).await });
        wait_for_published_content(&content_path).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let cleaner = manager.clone();
        let clearing = tokio::spawn(async move { cleaner.clear_cache(CacheScope::All).await });
        assert!(manager.lifecycle.try_write().is_err());
        drop(namespace_gate);
        clearing.await.unwrap().unwrap();
        assert!(!content_path.exists());
        assert!(!manager.game_dir(game).join("manifest.toml").exists());
        assert!(manager.runtime_dirs.read().await.is_empty());
    }

    #[test]
    fn retries_then_uses_the_atomic_cache() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (manager, responses) = OfficialAssetManager::with_test_responses(
            directory.path(),
            vec![
                Err(ZmError::Network("第一次失败".into())),
                Err(ZmError::Network("第二次失败".into())),
                Ok(b"runtime-asset".to_vec()),
            ],
        )
        .unwrap();
        runtime.block_on(write_manifest(&manager, GameKind::Zm4, "main-v1.swf"));

        let first = runtime
            .block_on(manager.fetch_resource(GameKind::Zm4, "ui/icon.png"))
            .unwrap();
        let second = runtime
            .block_on(manager.fetch_resource(GameKind::Zm4, "ui/icon.png"))
            .unwrap();
        assert_eq!(first.bytes, b"runtime-asset");
        assert!(!first.cache_hit);
        assert!(second.cache_hit);
        assert_eq!(responses.requests.load(Ordering::SeqCst), 3);
        let cached = runtime
            .block_on(manager.runtime_resource_dir(GameKind::Zm4))
            .join("ui/icon.png");
        assert!(cached.is_file());
        assert!(!cached.with_extension("part").exists());
    }

    #[tokio::test]
    async fn permanent_http_errors_fail_once_without_publishing_a_cache_entry() {
        for status in [400, 401, 403, 404, 410] {
            let directory = tempfile::tempdir().unwrap();
            let server = LocalHttpServer::new([
                HttpReply::Status(status, b"permanent"),
                HttpReply::Status(200, b"must-not-retry"),
            ]);
            let manager = server.manager(directory.path());
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                manager.fetch_resource(GameKind::Zm4, "asset.bin"),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains(&status.to_string()));
            assert_eq!(server.requests.load(Ordering::SeqCst), 1);
            let path = manager
                .runtime_resource_dir(GameKind::Zm4)
                .await
                .join("asset.bin");
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn reports_real_http_prefix_before_completion_and_preserves_shared_cache() {
        for known_length in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let (release, gate) = std::sync::mpsc::channel();
            let server = LocalHttpServer::new([HttpReply::Progressive {
                known_length,
                release: gate,
            }]);
            let manager = server.manager(directory.path());
            let (samples, callback, prefix) = observe_progress();
            let downloader = manager.clone();
            let downloading = tokio::spawn(async move {
                downloader
                    .fetch_resource_with_progress(GameKind::Zm4, "asset.bin", callback)
                    .await
            });
            tokio::time::timeout(Duration::from_secs(5), prefix)
                .await
                .unwrap()
                .unwrap();
            assert!(!downloading.is_finished());
            let path = manager
                .runtime_resource_dir(GameKind::Zm4)
                .await
                .join("asset.bin");
            assert!(!path.exists());
            assert!(samples.lock().unwrap().iter().any(|sample| {
                sample.bytes_loaded == 4
                    && sample.bytes_total == known_length.then_some(9)
                    && sample.attempt == 1
                    && !sample.cache_hit
            }));

            let (cached_samples, cached_callback, _) = observe_progress();
            let follower = manager.clone();
            let following = tokio::spawn(async move {
                follower
                    .fetch_resource_with_progress(GameKind::Zm4, "asset.bin", cached_callback)
                    .await
            });
            release.send(()).unwrap();
            let asset = tokio::time::timeout(Duration::from_secs(5), downloading)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let cached = tokio::time::timeout(Duration::from_secs(5), following)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(asset.bytes, b"part-done");
            assert!(!asset.cache_hit);
            assert_eq!(cached.bytes, asset.bytes);
            assert!(cached.cache_hit);
            assert_eq!(server.requests.load(Ordering::SeqCst), 1);
            let samples = samples.lock().unwrap().clone();
            assert!(
                samples
                    .windows(2)
                    .all(|pair| pair[0].bytes_loaded <= pair[1].bytes_loaded)
            );
            assert_eq!(samples.last().unwrap().bytes_total, Some(9));
            assert_eq!(samples.last().unwrap().bytes_loaded, 9);
            assert_eq!(
                *cached_samples.lock().unwrap(),
                [ResourceProgress {
                    attempt: 0,
                    bytes_loaded: 9,
                    bytes_total: Some(9),
                    cache_hit: true,
                }]
            );
            assert_eq!(tokio::fs::read(path).await.unwrap(), asset.bytes);
        }
    }

    #[tokio::test]
    async fn progress_retries_reset_the_attempt_without_replaying_response_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let server =
            LocalHttpServer::new([HttpReply::Truncated, HttpReply::Status(200, b"recovered")]);
        let manager = server.manager(directory.path());
        let (samples, callback, _) = observe_progress();
        let asset = tokio::time::timeout(
            Duration::from_secs(5),
            manager.fetch_resource_with_progress(GameKind::Zm4, "retry.bin", callback),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(asset.bytes, b"recovered");
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
        let samples = samples.lock().unwrap().clone();
        assert!(samples.iter().any(|sample| sample.attempt == 1));
        assert!(
            samples
                .iter()
                .any(|sample| sample.attempt == 2 && sample.bytes_loaded == 0)
        );
        for attempt in [1, 2] {
            let lengths: Vec<_> = samples
                .iter()
                .filter(|sample| sample.attempt == attempt)
                .map(|sample| sample.bytes_loaded)
                .collect();
            assert!(lengths.windows(2).all(|pair| pair[0] <= pair[1]));
        }
        assert_eq!(samples.last().unwrap().attempt, 2);
        assert_eq!(samples.last().unwrap().bytes_loaded, 9);

        let server = LocalHttpServer::new([HttpReply::Status(404, b"missing")]);
        let manager = server.manager(directory.path());
        let (samples, callback, _) = observe_progress();
        assert!(
            manager
                .fetch_resource_with_progress(GameKind::Zm4, "missing.bin", callback)
                .await
                .is_err()
        );
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        assert!(
            samples
                .lock()
                .unwrap()
                .iter()
                .all(|sample| sample.attempt == 1 && sample.bytes_loaded == 0)
        );
    }

    #[tokio::test]
    async fn cancelling_a_progress_download_stops_callbacks_and_keeps_cache_clear() {
        let directory = tempfile::tempdir().unwrap();
        let (release, gate) = std::sync::mpsc::channel();
        let server = LocalHttpServer::new([HttpReply::Progressive {
            known_length: true,
            release: gate,
        }]);
        let manager = server.manager(directory.path());
        let (samples, callback, prefix) = observe_progress();
        let downloader = manager.clone();
        let downloading = tokio::spawn(async move {
            downloader
                .fetch_resource_with_progress(GameKind::Zm4, "asset.bin", callback)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), prefix)
            .await
            .unwrap()
            .unwrap();
        downloading.abort();
        assert!(downloading.await.unwrap_err().is_cancelled());
        let observed = samples.lock().unwrap().len();
        manager.clear_cache(CacheScope::All).await.unwrap();
        release.send(()).unwrap();
        drop(server);
        assert_eq!(samples.lock().unwrap().len(), observed);
        assert!(!manager.game_dir(GameKind::Zm4).exists());
    }

    #[tokio::test]
    async fn transient_http_and_body_failures_retry_through_the_real_download_path() {
        for first in [
            HttpReply::Status(408, b"timeout"),
            HttpReply::Status(429, b"limited"),
            HttpReply::Status(500, b"failure"),
            HttpReply::Status(503, b"unavailable"),
            HttpReply::Status(200, b""),
            HttpReply::Truncated,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let server = LocalHttpServer::new([first, HttpReply::Status(200, b"recovered")]);
            let manager = server.manager(directory.path());
            let asset = tokio::time::timeout(
                Duration::from_secs(5),
                manager.fetch_resource(GameKind::Zm4, "asset.bin"),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(asset.bytes, b"recovered");
            assert!(!asset.cache_hit);
            let cached = manager
                .fetch_resource(GameKind::Zm4, "asset.bin")
                .await
                .unwrap();
            assert_eq!(cached.bytes, asset.bytes);
            assert!(cached.cache_hit);
            assert_eq!(server.requests.load(Ordering::SeqCst), 2);
        }
    }

    #[tokio::test]
    async fn actual_request_timeout_retries_and_repeated_server_failure_stops_at_three() {
        let directory = tempfile::tempdir().unwrap();
        let server =
            LocalHttpServer::new([HttpReply::Stalled, HttpReply::Status(200, b"recovered")]);
        let mut manager = server.manager(directory.path());
        manager.client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let asset = tokio::time::timeout(
            Duration::from_secs(5),
            manager.fetch_resource(GameKind::Zm4, "timeout.bin"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(asset.bytes, b"recovered");
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);

        let server = LocalHttpServer::new([
            HttpReply::Status(503, b"failure"),
            HttpReply::Status(503, b"failure"),
            HttpReply::Status(503, b"failure"),
            HttpReply::Status(200, b"must-not-retry"),
        ]);
        let manager = server.manager(directory.path());
        assert!(
            tokio::time::timeout(
                Duration::from_secs(5),
                manager.fetch_resource(GameKind::Zm4, "failure.bin"),
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(server.requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_non_missing_cache_read_error_does_not_download() {
        let directory = tempfile::tempdir().unwrap();
        let (manager, responses) =
            OfficialAssetManager::with_test_responses(directory.path(), vec![]).unwrap();
        let path = manager
            .runtime_resource_dir(GameKind::Zm4)
            .await
            .join("is-a-directory");
        tokio::fs::create_dir_all(&path).await.unwrap();
        assert!(matches!(
            manager
                .fetch_resource(GameKind::Zm4, "is-a-directory")
                .await,
            Err(ZmError::Io { .. })
        ));
        assert_eq!(responses.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn atomic_runtime_write_returns_its_original_allocation_after_publication() {
        let directory = tempfile::tempdir().unwrap();
        let manager = OfficialAssetManager::new(directory.path()).unwrap();
        let path = directory.path().join("nested/resource.bin");
        let bytes = vec![37_u8; 1024 * 1024];
        let pointer = bytes.as_ptr() as usize;
        let capacity = bytes.capacity();
        let lifecycle = manager.lifecycle.clone().read_owned().await;
        let guard = manager.resource_lock(&path).await.lock_owned().await;
        let bytes = atomic_cache_write(&path, bytes, lifecycle, guard, manager.performance.clone())
            .await
            .unwrap();
        assert_eq!(bytes.as_ptr() as usize, pointer);
        assert_eq!(bytes.capacity(), capacity);
        assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
    }

    #[test]
    fn cancelled_queued_runtime_write_finishes_before_cache_clear() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let manager = OfficialAssetManager::new(directory.path()).unwrap();
        runtime.block_on(async {
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, blocked) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                // Also exits if a test assertion drops the sender while unwinding.
                let _ = blocked.recv_timeout(Duration::from_secs(5));
            });
            ready.await.unwrap();
            let path = manager
                .game_dir(GameKind::Zm4)
                .join("resources/nested/resource.bin");
            let lifecycle = manager.lifecycle.clone().read_owned().await;
            let guard = manager.resource_lock(&path).await.lock_owned().await;
            let mut write = Box::pin(atomic_cache_write(
                &path,
                vec![17; 4096],
                lifecycle,
                guard,
                manager.performance.clone(),
            ));
            // Poll exactly once to enqueue the blocking write behind the gate,
            // then cancel its waiter before the write creates any directories.
            std::future::poll_fn(|context| {
                assert!(write.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(write);
            assert!(manager.lifecycle.try_write().is_err());
            let cleaner = manager.clone();
            let clearing = tokio::spawn(async move { cleaner.clear_cache(CacheScope::All).await });
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), clearing)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            blocker.await.unwrap();
            assert!(!manager.game_dir(GameKind::Zm4).exists());
            assert!(!path.exists());
        });
    }

    #[test]
    fn merges_concurrent_requests_for_the_same_resource() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (manager, responses) = OfficialAssetManager::with_test_responses(
            directory.path(),
            vec![Ok(b"one-download".to_vec())],
        )
        .unwrap();
        runtime.block_on(write_manifest(&manager, GameKind::Zm5, "main-v1.swf"));

        let (first, second) = runtime.block_on(async {
            tokio::join!(
                manager.fetch_resource(GameKind::Zm5, "module.swf"),
                manager.fetch_resource(GameKind::Zm5, "module.swf")
            )
        });
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert_ne!(first.cache_hit, second.cache_hit);
        assert_eq!(responses.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn performance_samples_are_shared_across_clones_and_cache_clears() {
        let directory = tempfile::tempdir().unwrap();
        let (manager, _) = OfficialAssetManager::with_test_responses(
            directory.path(),
            vec![Ok(b"runtime-asset".to_vec())],
        )
        .unwrap();
        let before = manager.performance.network_attempt.summary("network");
        let clone = manager.clone();
        clone
            .fetch_resource(GameKind::Zm4, "private-resource-name.png")
            .await
            .unwrap();
        let after = manager.performance.network_attempt.summary("network");
        assert_ne!(before, after);
        manager.clear_cache(CacheScope::All).await.unwrap();
        assert_eq!(
            after,
            manager.performance.network_attempt.summary("network")
        );

        let summary = manager.performance_summary();
        assert!(summary.contains("manager lifetime; both games"));
        assert!(summary.contains("Asset runtime write worker"));
        assert!(!summary.contains("private-resource-name"));
    }

    #[test]
    fn manifest_version_and_patch_change_the_resource_namespace() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let manager = OfficialAssetManager::new(directory.path()).unwrap();
        runtime.block_on(write_manifest(&manager, GameKind::Zm4, "main-v1.swf"));
        let first = runtime.block_on(manager.runtime_resource_dir(GameKind::Zm4));
        runtime.block_on(write_manifest(&manager, GameKind::Zm4, "main-v2.swf"));
        let second = runtime.block_on(manager.runtime_resource_dir(GameKind::Zm4));
        assert_ne!(first, second);

        let manifest_path = manager.game_dir(GameKind::Zm4).join("manifest.toml");
        let mut manifest: Manifest = toml::from_str(
            &runtime
                .block_on(tokio::fs::read_to_string(&manifest_path))
                .unwrap(),
        )
        .unwrap();
        manifest.patch_version += 1;
        runtime
            .block_on(tokio::fs::write(
                manifest_path,
                toml::to_string(&manifest).unwrap(),
            ))
            .unwrap();
        runtime.block_on(manager.invalidate_runtime_resource_dir(GameKind::Zm4));
        let third = runtime.block_on(manager.runtime_resource_dir(GameKind::Zm4));
        assert_ne!(second, third);
    }
    #[tokio::test]
    async fn validates_published_content_and_rejects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let manager = OfficialAssetManager::new(dir.path()).unwrap();
        write_manifest(&manager, GameKind::Zm4, "main.swf").await;
        let manifest_path = manager.game_dir(GameKind::Zm4).join("manifest.toml");
        let mut manifest: Manifest =
            toml::from_str(&tokio::fs::read_to_string(&manifest_path).await.unwrap()).unwrap();
        let bytes = b"test-published-content";
        manifest.final_sha256 = digest(bytes);
        let file = manager
            .game_dir(GameKind::Zm4)
            .join("versions")
            .join(format!("{}.swf", manifest.final_sha256));
        atomic_write_sync(&file, bytes).unwrap();
        atomic_write_sync(
            &manifest_path,
            toml::to_string(&manifest).unwrap().as_bytes(),
        )
        .unwrap();
        assert!(manager.cached_game(GameKind::Zm4).await.is_some());
        atomic_write_sync(&file, b"corrupted").unwrap();
        assert!(manager.cached_game(GameKind::Zm4).await.is_none());
    }
    #[tokio::test]
    async fn rejects_cross_platform_paths_before_network_access() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, responses) =
            OfficialAssetManager::with_test_responses(dir.path(), vec![]).unwrap();
        for path in [
            "",
            "C:/secret",
            "a\\..\\secret",
            "%2e%2e/secret",
            "a:stream",
        ] {
            assert!(
                manager.fetch_resource(GameKind::Zm4, path).await.is_err(),
                "{path}"
            );
        }
        assert_eq!(responses.requests.load(Ordering::SeqCst), 0);
    }
}
