use crate::{Result, ZmError};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountConfig {
    pub id: Uuid,
    pub account: String,
    pub display_name: String,
    pub uid: Option<u64>,
    pub credential_id: String,
}

impl AccountConfig {
    pub fn new(account: impl Into<String>) -> Self {
        let account = account.into();
        let id = Uuid::new_v4();
        Self {
            id,
            display_name: account.clone(),
            account,
            uid: None,
            credential_id: id.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub schema_version: u32,
    pub accounts: Vec<AccountConfig>,
    pub last_account: Option<Uuid>,
    pub volume: f32,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            accounts: vec![],
            last_account: None,
            volume: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}
impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(&self) -> Result<AppConfig> {
        if !self.path.exists() {
            return Ok(AppConfig::default());
        }
        let raw = fs::read_to_string(&self.path).map_err(|e| ZmError::io(&self.path, e))?;
        let config: AppConfig = toml::from_str(&raw).map_err(|e| ZmError::Config(e.to_string()))?;
        if config.schema_version != SCHEMA_VERSION {
            return Err(ZmError::Config(format!(
                "不支持的配置版本 {}",
                config.schema_version
            )));
        }
        Ok(config)
    }
    pub fn save(&self, config: &AppConfig) -> Result<()> {
        if config.schema_version != SCHEMA_VERSION {
            return Err(ZmError::Config("拒绝写入未知配置版本".into()));
        }
        let raw = toml::to_string_pretty(config).map_err(|e| ZmError::Config(e.to_string()))?;
        if ["password", "token", "cookie"]
            .iter()
            .any(|v| raw.to_ascii_lowercase().contains(v))
        {
            return Err(ZmError::Config("配置中包含敏感字段".into()));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| ZmError::Config("配置路径没有父目录".into()))?;
        fs::create_dir_all(parent).map_err(|e| ZmError::io(parent, e))?;
        let tmp = self.path.with_extension("toml.tmp");
        fs::write(&tmp, raw).map_err(|e| ZmError::io(&tmp, e))?;
        fs::rename(&tmp, &self.path).map_err(|e| ZmError::io(&self.path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_never_serializes_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        store.save(&AppConfig::default()).unwrap();
        let raw = fs::read_to_string(store.path()).unwrap();
        assert!(!raw.contains("password") && !raw.contains("token") && !raw.contains("cookie"));
    }
}
