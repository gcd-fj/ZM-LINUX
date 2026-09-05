//! UI-independent launch workflow. Each attempt owns its authentication context.
mod watchdog;
pub use watchdog::StartupWatchdog;

use std::{path::PathBuf, sync::Arc};
use zm_assets::AssetManager;
use zm_auth::{AuthClient, CaptchaAnswer, LoginOutcome, LoginRequest};
use zm_core::{GameKind, GameLaunchRequest, Result};
use zm_storage::AccountConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStage {
    Idle,
    Authenticating,
    AwaitingCaptcha,
    PreparingAssets,
    CreatingPlayer,
    AwaitingHost,
    AwaitingSession,
    SessionApplied,
    Failed,
}

impl LaunchStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "选择游戏和账号",
            Self::Authenticating => "4399 认证",
            Self::AwaitingCaptcha => "等待验证码",
            Self::PreparingAssets => "检查游戏资源",
            Self::CreatingPlayer => "创建播放器",
            Self::AwaitingHost => "等待游戏宿主",
            Self::AwaitingSession => "等待游戏请求登录",
            Self::SessionApplied => "会话已注入",
            Self::Failed => "启动失败",
        }
    }
}

/// A generation fence protects the UI from cancelled tasks and stopped players.
#[derive(Default)]
pub struct LaunchController {
    generation: u64,
    stage: Option<LaunchStage>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl LaunchController {
    pub fn begin(&mut self) -> u64 {
        self.cancel();
        self.stage = Some(LaunchStage::Authenticating);
        self.generation
    }
    pub fn current_id(&self) -> u64 {
        self.generation
    }
    pub fn stage(&self) -> LaunchStage {
        self.stage.unwrap_or(LaunchStage::Idle)
    }
    pub fn accepts(&self, id: u64) -> bool {
        id == self.generation && !matches!(self.stage(), LaunchStage::Idle | LaunchStage::Failed)
    }
    pub fn transition(&mut self, id: u64, next: LaunchStage) -> bool {
        if !self.accepts(id) {
            return false;
        }
        let valid = next == LaunchStage::Failed
            || matches!(
                (self.stage(), next),
                (
                    LaunchStage::Authenticating,
                    LaunchStage::AwaitingCaptcha | LaunchStage::PreparingAssets
                ) | (LaunchStage::AwaitingCaptcha, LaunchStage::Authenticating)
                    | (LaunchStage::PreparingAssets, LaunchStage::CreatingPlayer)
                    | (LaunchStage::CreatingPlayer, LaunchStage::AwaitingHost)
                    | (LaunchStage::AwaitingHost, LaunchStage::AwaitingSession)
                    | (
                        LaunchStage::AwaitingHost | LaunchStage::AwaitingSession,
                        LaunchStage::SessionApplied
                    )
            );
        if valid {
            self.stage = Some(next);
        }
        valid
    }
    pub fn attach(&mut self, task: tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.task.replace(task) {
            previous.abort();
        }
    }
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = self.generation.wrapping_add(1);
        self.stage = None;
    }
}
impl Drop for LaunchController {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct LaunchInput {
    pub session_id: u64,
    pub game: GameKind,
    pub account: AccountConfig,
    pub password: String,
    pub captcha: Option<(String, String)>,
    pub storage_root: PathBuf,
}

pub enum LaunchEvent {
    Stage(LaunchStage),
    Captcha {
        id: String,
        image_url: String,
        image: Vec<u8>,
        image_error: Option<String>,
    },
    Prepared {
        launch: Box<GameLaunchRequest>,
        account: AccountConfig,
    },
    Failed(String),
}

/// Caller supplies a session-scoped client; captcha retries reuse that same client.
pub async fn prepare_launch(
    input: LaunchInput,
    auth: Arc<dyn AuthClient>,
    assets: Arc<dyn AssetManager>,
    emit: impl Fn(LaunchEvent),
) {
    if let Err(error) = prepare(input, auth, assets, &emit).await {
        emit(LaunchEvent::Failed(error.to_string()));
    }
}
async fn prepare(
    mut input: LaunchInput,
    auth: Arc<dyn AuthClient>,
    assets: Arc<dyn AssetManager>,
    emit: &impl Fn(LaunchEvent),
) -> Result<()> {
    let captcha = input
        .captcha
        .as_ref()
        .map(|(id, value)| CaptchaAnswer { id, value });
    let outcome = auth
        .login(LoginRequest {
            account: &input.account.account,
            password: &input.password,
            game: input.game,
            captcha,
        })
        .await?;
    // No password crosses the prepared-launch interface.
    input.password.clear();
    match outcome {
        LoginOutcome::CaptchaRequired(challenge) => {
            let (image, image_error) = match auth.fetch_captcha(&challenge.image_url).await {
                Ok(bytes) => (bytes, None),
                Err(error) => (vec![], Some(error.to_string())),
            };
            emit(LaunchEvent::Captcha {
                id: challenge.id,
                image_url: challenge.image_url,
                image,
                image_error,
            });
        }
        LoginOutcome::Authenticated(session) => {
            emit(LaunchEvent::Stage(LaunchStage::PreparingAssets));
            let asset = assets.ensure_game(input.game).await?;
            input.account.uid = Some(session.uid);
            input.account.display_name = session.display_name.clone();
            emit(LaunchEvent::Stage(LaunchStage::CreatingPlayer));
            emit(LaunchEvent::Prepared {
                launch: Box::new(GameLaunchRequest {
                    session_id: input.session_id,
                    game: input.game,
                    account_name: input.account.account.clone(),
                    uid: session.uid,
                    account_display_name: session.display_name,
                    auth_token: session.token,
                    auth_cookie: session.auth_cookie,
                    storage_root: input.storage_root,
                    main_swf: asset.path,
                    movie_url: asset.version.swf_url,
                }),
                account: input.account,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_attempt_cannot_change_new_session() {
        let mut state = LaunchController::default();
        let old = state.begin();
        let new = state.begin();
        assert!(!state.transition(old, LaunchStage::PreparingAssets));
        assert!(state.transition(new, LaunchStage::PreparingAssets));
        state.cancel();
        assert!(!state.accepts(new));
    }
    #[test]
    fn host_ready_is_not_session_ready() {
        let mut state = LaunchController::default();
        let id = state.begin();
        assert!(!state.transition(id, LaunchStage::SessionApplied));
        for stage in [
            LaunchStage::PreparingAssets,
            LaunchStage::CreatingPlayer,
            LaunchStage::AwaitingHost,
            LaunchStage::AwaitingSession,
        ] {
            assert!(state.transition(id, stage));
        }
        assert_eq!(state.stage(), LaunchStage::AwaitingSession);
        assert!(state.transition(id, LaunchStage::SessionApplied));
        assert!(!state.transition(id, LaunchStage::AwaitingSession));
    }
    #[test]
    fn failure_is_terminal() {
        let mut state = LaunchController::default();
        let id = state.begin();
        assert!(state.transition(id, LaunchStage::Failed));
        assert!(!state.accepts(id));
    }
}
