use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use zm_assets::{AssetManager, CacheScope, GameAsset, GameVersion, RuntimeAsset};
use zm_auth::{AuthClient, AuthSession, CaptchaChallenge, LoginOutcome, LoginRequest};
use zm_core::{GameKind, Result, ZmError};
use zm_launcher::{LaunchEvent, LaunchInput, LaunchStage, prepare_launch};
use zm_storage::AccountConfig;

struct Auth {
    captcha: bool,
    image_fails: bool,
}
#[async_trait]
impl AuthClient for Auth {
    async fn fetch_captcha(&self, _: &str) -> Result<Vec<u8>> {
        if self.image_fails {
            Err(ZmError::Network("image unavailable".into()))
        } else {
            Ok(vec![1])
        }
    }
    async fn resolve_uid(&self, _: &str) -> Result<u64> {
        Ok(42)
    }
    async fn login(&self, request: LoginRequest<'_>) -> Result<LoginOutcome> {
        assert_eq!(request.account, "account-login");
        if self.captcha && request.captcha.is_none() {
            Ok(LoginOutcome::CaptchaRequired(CaptchaChallenge {
                id: "challenge".into(),
                image_url: "https://example.invalid/captcha".into(),
            }))
        } else {
            Ok(LoginOutcome::Authenticated(AuthSession {
                uid: 42,
                token: "secret-token".into(),
                display_name: "different-display-name".into(),
                auth_cookie: "secret-cookie".into(),
            }))
        }
    }
    async fn submit_captcha(&self, request: LoginRequest<'_>) -> Result<LoginOutcome> {
        self.login(request).await
    }
    async fn request_game_token(&self, _: GameKind, _: &str, _: u64) -> Result<String> {
        unreachable!()
    }
}
struct Assets {
    calls: AtomicUsize,
    fails: bool,
}
#[async_trait]
impl AssetManager for Assets {
    async fn resolve_version(&self, game: GameKind) -> Result<GameVersion> {
        Ok(GameVersion {
            game,
            file_name: "game.swf".into(),
            page_url: "https://example.invalid/".into(),
            swf_url: "https://example.invalid/game.swf".into(),
        })
    }
    async fn ensure_game(&self, game: GameKind) -> Result<GameAsset> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fails {
            return Err(ZmError::Asset("download failed".into()));
        }
        Ok(GameAsset {
            version: self.resolve_version(game).await?,
            path: PathBuf::from("game.swf"),
            sha256: "hash".into(),
            cache_hit: false,
        })
    }
    async fn fetch_resource(&self, _: GameKind, _: &str) -> Result<RuntimeAsset> {
        unreachable!()
    }
    async fn clear_cache(&self, _: CacheScope) -> Result<()> {
        Ok(())
    }
}
fn input() -> LaunchInput {
    LaunchInput {
        session_id: 7,
        game: GameKind::Zm4,
        account: AccountConfig::new("account-login"),
        password: "password".into(),
        captcha: None,
        storage_root: PathBuf::from("cache"),
    }
}
#[tokio::test]
async fn captcha_never_downloads_game_and_preserves_image_error() {
    let assets = Arc::new(Assets {
        calls: AtomicUsize::new(0),
        fails: false,
    });
    let events = Mutex::new(vec![]);
    prepare_launch(
        input(),
        Arc::new(Auth {
            captcha: true,
            image_fails: true,
        }),
        assets.clone(),
        |event| events.lock().unwrap().push(event),
    )
    .await;
    assert_eq!(assets.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        &events.lock().unwrap()[..],
        [LaunchEvent::Captcha {
            image_error: Some(_),
            ..
        }]
    ));
}
#[tokio::test]
async fn prepared_session_keeps_login_identity_separate_from_display_name() {
    let events = Mutex::new(vec![]);
    prepare_launch(
        input(),
        Arc::new(Auth {
            captcha: false,
            image_fails: false,
        }),
        Arc::new(Assets {
            calls: AtomicUsize::new(0),
            fails: false,
        }),
        |event| events.lock().unwrap().push(event),
    )
    .await;
    let events = events.lock().unwrap();
    assert!(matches!(
        events[0],
        LaunchEvent::Stage(LaunchStage::PreparingAssets)
    ));
    assert!(matches!(
        events[1],
        LaunchEvent::Stage(LaunchStage::CreatingPlayer)
    ));
    let LaunchEvent::Prepared { launch, .. } = &events[2] else {
        panic!("launch missing")
    };
    assert_eq!(launch.session_id, 7);
    assert_eq!(launch.account_name, "account-login");
    assert_eq!(launch.account_display_name, "different-display-name");
}
#[tokio::test]
async fn asset_failure_never_creates_player() {
    let events = Mutex::new(vec![]);
    prepare_launch(
        input(),
        Arc::new(Auth {
            captcha: false,
            image_fails: false,
        }),
        Arc::new(Assets {
            calls: AtomicUsize::new(0),
            fails: true,
        }),
        |event| events.lock().unwrap().push(event),
    )
    .await;
    assert!(matches!(
        &events.lock().unwrap()[..],
        [
            LaunchEvent::Stage(LaunchStage::PreparingAssets),
            LaunchEvent::Failed(_)
        ]
    ));
}
