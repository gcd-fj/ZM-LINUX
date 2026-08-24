use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};
use zm_core::{GameKind, Result, ZmError};

mod swf_patch;

const HOME_URL: &str = "https://www.4399.com/flash/zmhj.htm";
const PATCH_VERSION: u32 = 3;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    Game(GameKind),
    All,
}

#[async_trait]
pub trait AssetManager: Send + Sync {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion>;
    async fn ensure_game(&self, game: GameKind) -> Result<GameAsset>;
    async fn fetch_resource(&self, game: GameKind, resource: &str) -> Result<Vec<u8>>;
    async fn clear_cache(&self, scope: CacheScope) -> Result<()>;
}

#[derive(Clone)]
pub struct OfficialAssetManager {
    client: Client,
    cache_root: PathBuf,
}

impl OfficialAssetManager {
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .user_agent("ZM-LINUX/0.1")
            .build()
            .map_err(|e| ZmError::Network(e.to_string()))?;
        Ok(Self {
            client,
            cache_root: cache_root.into(),
        })
    }
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }
    fn game_dir(&self, game: GameKind) -> PathBuf {
        self.cache_root.join(game.slug())
    }
    fn resource_root(game: GameKind) -> &'static str {
        match game {
            GameKind::Zm4 => "https://sda.4399.com/4399swf/upload_swf/ftp15/csya/20150127/1/",
            GameKind::Zm5 => "https://sda.4399.com/4399swf/upload_swf/ftp22/csya/20170622/1/",
        }
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
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: String,
    raw_sha256: String,
    bridge_sha256: String,
    final_sha256: String,
    patch_version: u32,
}

#[async_trait]
impl AssetManager for OfficialAssetManager {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion> {
        let landing_url = Self::standalone_url(game);
        let landing = String::from_utf8_lossy(&self.get_bytes(&landing_url, Some(HOME_URL)).await?)
            .into_owned();
        let folder = match game {
            GameKind::Zm4 => "ftp15",
            GameKind::Zm5 => "ftp22",
        };
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
        let version = self.resolve_version(game).await?;
        let dir = self.game_dir(game);
        let path = dir.join("main.swf");
        let manifest_path = dir.join("manifest.toml");
        if let Ok(raw) = tokio::fs::read_to_string(&manifest_path).await
            && let Ok(manifest) = toml::from_str::<Manifest>(&raw)
            && manifest.version == version.file_name
            && manifest.patch_version == PATCH_VERSION
            && manifest.bridge_sha256 == digest(bridge_abc(game))
            && path.exists()
        {
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| ZmError::io(&path, e))?;
            if digest(&bytes) == manifest.final_sha256 {
                return Ok(GameAsset {
                    version,
                    path,
                    sha256: manifest.final_sha256,
                    cache_hit: true,
                });
            }
        }
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ZmError::io(&dir, e))?;
        let source_bytes = self
            .get_bytes(&version.swf_url, Some(&version.page_url))
            .await?;
        if source_bytes.len() < 8 || !matches!(&source_bytes[..3], b"FWS" | b"CWS" | b"ZWS") {
            return Err(ZmError::Asset("下载内容不是有效SWF".into()));
        }
        let raw_sha256 = digest(&source_bytes);
        let bridge_class = match game {
            GameKind::Zm4 => "ZmLinuxZm4Bridge",
            GameKind::Zm5 => "ZmLinuxZm5Bridge",
        };
        let bridge = bridge_abc(game);
        let bytes = swf_patch::inject_bridge(&source_bytes, bridge, bridge_class)?;
        let sha256 = digest(&bytes);
        let temp = dir.join("main.swf.part");
        tokio::fs::write(&temp, &bytes)
            .await
            .map_err(|e| ZmError::io(&temp, e))?;
        tokio::fs::rename(&temp, &path)
            .await
            .map_err(|e| ZmError::io(&path, e))?;
        let raw = toml::to_string_pretty(&Manifest {
            version: version.file_name.clone(),
            raw_sha256,
            bridge_sha256: digest(bridge),
            final_sha256: sha256.clone(),
            patch_version: PATCH_VERSION,
        })
        .map_err(|e| ZmError::Asset(e.to_string()))?;
        tokio::fs::write(&manifest_path, raw)
            .await
            .map_err(|e| ZmError::io(&manifest_path, e))?;
        Ok(GameAsset {
            version,
            path,
            sha256,
            cache_hit: false,
        })
    }

    async fn fetch_resource(&self, game: GameKind, resource: &str) -> Result<Vec<u8>> {
        let resource = resource
            .split('?')
            .next()
            .unwrap_or(resource)
            .trim_start_matches('/');
        let path = Path::new(resource);
        if resource.contains("://")
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(ZmError::Asset("拒绝不安全的资源路径".into()));
        }
        let local = self.game_dir(game).join("resources").join(path);
        if local.exists() {
            return tokio::fs::read(&local)
                .await
                .map_err(|e| ZmError::io(&local, e));
        }
        let url = Url::parse(Self::resource_root(game))
            .unwrap()
            .join(resource)
            .map_err(|e| ZmError::Asset(e.to_string()))?;
        let bytes = self.get_bytes(url.as_str(), Some(HOME_URL)).await?;
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ZmError::io(parent, e))?;
        }
        let temp = local.with_extension("part");
        tokio::fs::write(&temp, &bytes)
            .await
            .map_err(|e| ZmError::io(&temp, e))?;
        tokio::fs::rename(&temp, &local)
            .await
            .map_err(|e| ZmError::io(&local, e))?;
        Ok(bytes)
    }

    async fn clear_cache(&self, scope: CacheScope) -> Result<()> {
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

fn bridge_abc(game: GameKind) -> &'static [u8] {
    match game {
        GameKind::Zm4 => ZM4_BRIDGE_ABC,
        GameKind::Zm5 => ZM5_BRIDGE_ABC,
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
