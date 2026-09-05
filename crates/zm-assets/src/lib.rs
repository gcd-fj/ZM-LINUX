use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use zm_core::{GameKind, Result, ZmError};

mod swf_patch;

const HOME_URL: &str = "https://www.4399.com/flash/zmhj.htm";
const PATCH_VERSION: u32 = 4;
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
    pub sha256: String,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAsset {
    pub bytes: Vec<u8>,
    pub cache_hit: bool,
}
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
    async fn clear_cache(&self, scope: CacheScope) -> Result<()>;
}

#[derive(Clone)]
pub struct OfficialAssetManager {
    client: Client,
    lifecycle: Arc<RwLock<()>>,
    cache_root: PathBuf,
    in_flight: Arc<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
    resource_roots: Arc<HashMap<GameKind, Url>>,
    #[cfg(test)]
    test_responses: Option<Arc<TestResponses>>,
}

#[cfg(test)]
struct TestResponses {
    values: Mutex<std::collections::VecDeque<Result<Vec<u8>>>>,
    requests: std::sync::atomic::AtomicUsize,
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
            resource_roots: Arc::new(Self::default_resource_roots()),
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
        let mut request = self.client.get(url);
        if let Some(value) = referer {
            request = request.header("Referer", value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ZmError::Network(e.to_string()))?;
        Ok(response
            .bytes()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?
            .to_vec())
    }

    async fn get_runtime_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut last_error = None;
        for (attempt, delay_ms) in [0, 250, 750].into_iter().enumerate() {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            match self.get_runtime_bytes_once(url).await {
                Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                Ok(_) => last_error = Some("服务器返回了空资源".to_owned()),
                Err(error) => last_error = Some(error.to_string()),
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

    async fn get_runtime_bytes_once(&self, url: &str) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(responses) = &self.test_responses {
            responses
                .requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return responses
                .values
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Err(ZmError::Network("测试响应已耗尽".into())));
        }
        self.get_bytes(url, Some(HOME_URL)).await
    }

    async fn runtime_resource_dir(&self, game: GameKind) -> PathBuf {
        let manifest_path = self.game_dir(game).join("manifest.toml");
        let namespace = tokio::fs::read_to_string(manifest_path)
            .await
            .ok()
            .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
            .map(|manifest| {
                let version_hash = digest(manifest.version.as_bytes());
                format!(
                    "patch{}-{}-{}",
                    manifest.patch_version,
                    &version_hash[..12],
                    &digest(manifest.bridge_sha256.as_bytes())[..12]
                )
            })
            .unwrap_or_else(|| format!("patch{PATCH_VERSION}-unknown"));
        self.game_dir(game).join("resources").join(namespace)
    }

    async fn resource_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.in_flight.lock().await;
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

#[async_trait]
impl AssetManager for OfficialAssetManager {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion> {
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
        let lifecycle = self.lifecycle.clone().read_owned().await;
        let lock = self
            .resource_lock(&self.game_dir(game).join("manifest.toml"))
            .await;
        let guard = lock.lock_owned().await;
        let previous = self.cached_game(game).await;
        let version = match self.resolve_version(game).await {
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
        let lifecycle = self.lifecycle.clone().read_owned().await;
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
        let _resource_guard = resource_lock.lock_owned().await;
        if local.exists() {
            let bytes = tokio::fs::read(&local)
                .await
                .map_err(|e| ZmError::io(&local, e))?;
            return Ok(RuntimeAsset {
                bytes,
                cache_hit: true,
            });
        }
        let url = self
            .resource_roots
            .get(&game)
            .ok_or_else(|| ZmError::Asset("缺少游戏资源根地址".into()))?
            .join(resource)
            .map_err(|e| ZmError::Asset(e.to_string()))?;
        let bytes = self.get_runtime_bytes(url.as_str()).await?;
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ZmError::io(parent, e))?;
        }
        atomic_write(&local, bytes.clone(), lifecycle, _resource_guard).await?;
        Ok(RuntimeAsset {
            bytes,
            cache_hit: false,
        })
    }

    async fn clear_cache(&self, scope: CacheScope) -> Result<()> {
        let _lifecycle = self.lifecycle.write().await;
        let target = match scope {
            CacheScope::Game(game) => self.game_dir(game),
            CacheScope::All => self.cache_root.clone(),
        };
        if target.exists() {
            tokio::fs::remove_dir_all(&target)
                .await
                .map_err(|e| ZmError::io(&target, e))?;
        }
        Ok(())
    }
}

impl OfficialAssetManager {
    async fn cached_game(&self, game: GameKind) -> Option<GameAsset> {
        let dir = self.game_dir(game);
        let raw = tokio::fs::read_to_string(dir.join("manifest.toml"))
            .await
            .ok()?;
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
        let bytes = tokio::fs::read(&path).await.ok()?;
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
            sha256: manifest.final_sha256,
            cache_hit: true,
        })
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
        let raw_sha256 = digest(&source);
        let bytes =
            swf_patch::inject_bridge(&source, bridge_abc(game), game.profile().bridge_class)?;
        let sha256 = digest(&bytes);
        let dir = self.game_dir(game);
        let path = dir.join("versions").join(format!("{sha256}.swf"));
        // Publish the content before the pointer. Cancellation never invalidates the old pointer.
        let raw = toml::to_string_pretty(&Manifest {
            version: version.file_name.clone(),
            raw_sha256,
            bridge_sha256: digest(bridge_abc(game)),
            final_sha256: sha256.clone(),
            patch_version: PATCH_VERSION,
            movie_url: version.swf_url.clone(),
            page_url: version.page_url.clone(),
        })
        .map_err(|error| ZmError::Asset(error.to_string()))?;
        let destination = path.clone();
        tokio::task::spawn_blocking(move || {
            let (_lifecycle, _guard) = (lifecycle, guard);
            atomic_write_sync(&destination, &bytes)?;
            atomic_write_sync(&dir.join("manifest.toml"), raw.as_bytes())
        })
        .await
        .map_err(|error| ZmError::Asset(error.to_string()))??;
        Ok(GameAsset {
            version,
            path,
            sha256,
            cache_hit: false,
        })
    }
}

async fn atomic_write(
    path: &Path,
    bytes: Vec<u8>,
    lifecycle: tokio::sync::OwnedRwLockReadGuard<()>,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        // Keep both locks until publication finishes, even if the awaiting task is cancelled.
        let (_lifecycle, _guard) = (lifecycle, guard);
        atomic_write_sync(&path, &bytes)
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
    use std::sync::atomic::Ordering;

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
