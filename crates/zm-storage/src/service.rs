//! Ordered credential operations. Commands are queued at the UI call site, not
//! when a background task happens to be polled, so delete cannot race a save.
use crate::{CredentialStore, SecretServiceStore, SessionCredentialStore};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use zm_core::Result;

type Reply = oneshot::Sender<Result<Option<String>>>;
pub type CredentialReply = oneshot::Receiver<Result<Option<String>>>;
enum Command {
    Load {
        id: String,
        account: String,
        reply: Reply,
    },
    Save {
        id: String,
        account: String,
        password: String,
        remember: bool,
        reply: Reply,
    },
    Delete {
        id: String,
        account: String,
        reply: Reply,
    },
}

pub struct CredentialService {
    tx: mpsc::UnboundedSender<Command>,
}
impl CredentialService {
    pub fn new(runtime: &tokio::runtime::Handle) -> Self {
        Self::with_store(runtime, Arc::new(SecretServiceStore))
    }
    fn with_store(runtime: &tokio::runtime::Handle, keyring: Arc<dyn CredentialStore>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            let memory = SessionCredentialStore::default();
            while let Some(command) = rx.recv().await {
                match command {
                    Command::Load { id, account, reply } => {
                        let result = match memory.load(&id, &account).await {
                            Ok(Some(password)) => Ok(Some(password)),
                            _ => keyring.load(&id, &account).await,
                        };
                        let _ = reply.send(result);
                    }
                    Command::Save {
                        id,
                        account,
                        password,
                        remember,
                        reply,
                    } => {
                        let result = match memory.save(&id, &account, &password).await {
                            Err(error) => Err(error),
                            Ok(()) => {
                                if remember {
                                    keyring.save(&id, &account, &password).await
                                } else {
                                    keyring.delete(&id, &account).await
                                }
                            }
                        };
                        let _ = reply.send(result.map(|()| None));
                    }
                    Command::Delete { id, account, reply } => {
                        let _ = memory.delete(&id, &account).await;
                        let result = keyring.delete(&id, &account).await;
                        let _ = reply.send(result.map(|()| None));
                    }
                }
            }
        });
        Self { tx }
    }
    pub fn load(&self, id: &str, account: &str) -> CredentialReply {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Load {
            id: id.into(),
            account: account.into(),
            reply,
        });
        rx
    }
    pub fn save(&self, id: &str, account: &str, password: &str, remember: bool) -> CredentialReply {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Save {
            id: id.into(),
            account: account.into(),
            password: password.into(),
            remember,
            reply,
        });
        rx
    }
    pub fn delete(&self, id: &str, account: &str) -> CredentialReply {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Delete {
            id: id.into(),
            account: account.into(),
            reply,
        });
        rx
    }
}

pub async fn receive_credential(
    reply: CredentialReply,
) -> std::result::Result<Option<String>, String> {
    reply
        .await
        .map_err(|_| "凭据服务已停止".to_owned())?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn queued_delete_wins_even_when_save_reply_is_dropped() {
        let persistent = Arc::new(SessionCredentialStore::default());
        let service =
            CredentialService::with_store(&tokio::runtime::Handle::current(), persistent.clone());
        drop(service.save("id", "account", "password", true));
        receive_credential(service.delete("id", "account"))
            .await
            .unwrap();
        assert!(
            receive_credential(service.load("id", "account"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(persistent.load("id", "account").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn memory_only_save_removes_old_persistent_password() {
        let persistent = Arc::new(SessionCredentialStore::default());
        persistent.save("id", "account", "old").await.unwrap();
        let service =
            CredentialService::with_store(&tokio::runtime::Handle::current(), persistent.clone());
        receive_credential(service.save("id", "account", "new", false))
            .await
            .unwrap();
        assert_eq!(
            receive_credential(service.load("id", "account"))
                .await
                .unwrap()
                .as_deref(),
            Some("new")
        );
        assert!(persistent.load("id", "account").await.unwrap().is_none());
    }
}
