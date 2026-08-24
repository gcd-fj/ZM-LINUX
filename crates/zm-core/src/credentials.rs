use crate::{Result, ZmError};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialAvailability {
    Available,
    SessionOnly,
}

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn save(&self, id: &str, account: &str, password: &str) -> Result<()>;
    async fn load(&self, id: &str, account: &str) -> Result<Option<String>>;
    async fn delete(&self, id: &str, account: &str) -> Result<()>;
    fn availability(&self) -> CredentialAvailability;
}

#[derive(Debug, Default, Clone)]
pub struct SessionCredentialStore {
    values: Arc<RwLock<HashMap<String, String>>>,
}
#[async_trait]
impl CredentialStore for SessionCredentialStore {
    async fn save(&self, id: &str, _: &str, password: &str) -> Result<()> {
        self.values
            .write()
            .unwrap()
            .insert(id.into(), password.into());
        Ok(())
    }
    async fn load(&self, id: &str, _: &str) -> Result<Option<String>> {
        Ok(self.values.read().unwrap().get(id).cloned())
    }
    async fn delete(&self, id: &str, _: &str) -> Result<()> {
        self.values.write().unwrap().remove(id);
        Ok(())
    }
    fn availability(&self) -> CredentialAvailability {
        CredentialAvailability::SessionOnly
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SecretServiceStore;
impl SecretServiceStore {
    fn entry(id: &str, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new("io.github.gcd-fj.zm-linux", &format!("{id}:{account}"))
            .map_err(|e| ZmError::Credential(e.to_string()))
    }
}
#[async_trait]
impl CredentialStore for SecretServiceStore {
    async fn save(&self, id: &str, account: &str, password: &str) -> Result<()> {
        Self::entry(id, account)?
            .set_password(password)
            .map_err(|e| ZmError::Credential(e.to_string()))
    }
    async fn load(&self, id: &str, account: &str) -> Result<Option<String>> {
        match Self::entry(id, account)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(ZmError::Credential(e.to_string())),
        }
    }
    async fn delete(&self, id: &str, account: &str) -> Result<()> {
        match Self::entry(id, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(ZmError::Credential(e.to_string())),
        }
    }
    fn availability(&self) -> CredentialAvailability {
        CredentialAvailability::Available
    }
}
