use eframe::egui;
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};
use tokio::runtime::Runtime;
use uuid::Uuid;
use zm_assets::{AssetManager, CacheScope, OfficialAssetManager};
use zm_auth::OfficialAuthClient;
use zm_core::{AccountMode, CredentialState, GameKind};
use zm_launcher::{
    LaunchController, LaunchEvent, LaunchInput, LaunchStage, StartupWatchdog, prepare_launch,
};
use zm_player::{
    GAME_HEIGHT, GAME_WIDTH, GameFrameInput, GameRuntime, RuntimeEvent, RuntimeMessage,
};
use zm_storage::{
    AccountConfig, AppConfig, AppPaths, ConfigStore, CredentialService, receive_credential,
};

#[cfg(target_os = "linux")]
use crate::desktop;
use crate::{
    load_icon_texture,
    theme::{self as palette, configure_ui},
};
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(90);
mod accounts;
mod home;
mod views;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Login,
    Busy,
    Game,
    Settings,
}

enum AppMessage {
    Launch {
        id: u64,
        event: Box<LaunchEvent>,
    },
    CaptchaImage {
        id: u64,
        revision: u64,
        result: Result<Vec<u8>, String>,
    },
    Notice(String),
    CacheCleared,
    PasswordLoaded {
        request_id: u64,
        account_id: Uuid,
        password: Result<Option<String>, String>,
    },
}

pub(crate) struct ZmApp {
    paths: AppPaths,
    config_store: ConfigStore,
    config: AppConfig,
    config_writable: bool,
    rt: Runtime,
    auth: Arc<OfficialAuthClient>,
    assets: Arc<OfficialAssetManager>,
    player: GameRuntime,
    runtime_rx: Receiver<RuntimeMessage>,
    credentials: CredentialService,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
    page: Page,
    account_mode: AccountMode,
    credential_state: CredentialState,
    credential_request_id: u64,
    account_picker_open: bool,
    confirm_switch: bool,
    diagnostics_open: bool,
    account: String,
    password: String,
    manager_account: String,
    manager_password: String,
    manager_save_password: bool,
    captcha_value: String,
    captcha_id: Option<String>,
    captcha_url: Option<String>,
    captcha_texture: Option<egui::TextureHandle>,
    app_icon: egui::TextureHandle,
    status: String,
    active_game: Option<GameKind>,
    active_account: Option<AccountConfig>,
    selected_game: GameKind,
    save_password: bool,
    busy_step: usize,
    launch: LaunchController,
    captcha_revision: u64,
    last_diagnostics: Option<String>,
    startup_watchdog: StartupWatchdog,
    pending_stop: Option<String>,
}

impl ZmApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        configure_ui(&cc.egui_ctx);
        let app_icon = load_icon_texture(&cc.egui_ctx);
        let config_store = ConfigStore::new(paths.config_file());
        let (config, config_error) = match config_store.load() {
            Ok(config) => (config, None),
            Err(error) => (
                AppConfig::default(),
                Some(format!(
                    "配置读取失败：{error}。原文件已保留，本次禁止覆盖。"
                )),
            ),
        };
        let auth = Arc::new(OfficialAuthClient::new().expect("创建登录客户端失败"));
        let assets =
            Arc::new(OfficialAssetManager::new(&paths.cache_dir).expect("创建资源客户端失败"));
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("ZM-LINUX需要wgpu渲染后端");
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let rt = Runtime::new().expect("创建异步运行时失败");
        let mut player = GameRuntime::new(
            render_state,
            cc.egui_ctx.clone(),
            runtime_tx,
            assets.clone(),
            rt.handle().clone(),
        );
        player.set_volume(config.volume);
        let (tx, rx) = mpsc::channel();
        let initial_mode = config
            .last_account
            .filter(|id| config.accounts.iter().any(|account| account.id == *id))
            .map(AccountMode::Saved)
            .unwrap_or(AccountMode::New);
        let mut app = Self {
            paths,
            config_store,
            config,
            config_writable: config_error.is_none(),
            credentials: CredentialService::new(rt.handle()),
            rt,
            auth,
            assets,
            player,
            runtime_rx,
            tx,
            rx,
            page: Page::Login,
            account_mode: AccountMode::New,
            credential_state: CredentialState::Missing,
            credential_request_id: 0,
            account_picker_open: false,
            confirm_switch: false,
            diagnostics_open: false,
            account: String::new(),
            password: String::new(),
            manager_account: String::new(),
            manager_password: String::new(),
            manager_save_password: true,
            captcha_value: String::new(),
            captcha_id: None,
            captcha_url: None,
            captcha_texture: None,
            app_icon,
            status: "准备就绪".into(),
            active_game: None,
            active_account: None,
            selected_game: GameKind::Zm4,
            save_password: true,
            busy_step: 0,
            launch: LaunchController::default(),
            captcha_revision: 0,
            last_diagnostics: None,
            startup_watchdog: StartupWatchdog::default(),
            pending_stop: None,
        };
        app.select_account(initial_mode, cc.egui_ctx.clone());
        if let Some(error) = config_error {
            app.status = error;
        }
        app
    }

    fn select_account(&mut self, mode: AccountMode, ctx: egui::Context) {
        self.launch.cancel();
        self.captcha_revision = self.captcha_revision.wrapping_add(1);
        self.credential_request_id = self.credential_request_id.wrapping_add(1);
        self.account_mode = mode;
        self.password.clear();
        self.captcha_id = None;
        self.captcha_url = None;
        self.captcha_value.clear();
        self.captcha_texture = None;
        self.account_picker_open = false;
        match mode {
            AccountMode::New => {
                self.account.clear();
                self.credential_state = CredentialState::Missing;
            }
            AccountMode::Saved(account_id) => {
                let Some(saved) = self
                    .config
                    .accounts
                    .iter()
                    .find(|account| account.id == account_id)
                    .cloned()
                else {
                    self.account_mode = AccountMode::New;
                    self.account.clear();
                    self.credential_state = CredentialState::Missing;
                    return;
                };
                self.account.clone_from(&saved.account);
                self.save_password = saved.remember_password;
                let request_id = self.credential_request_id;
                self.credential_state = CredentialState::Loading { request_id };
                let reply = self.credentials.load(&saved.credential_id, &saved.account);
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let password = receive_credential(reply).await;
                    let _ = tx.send(AppMessage::PasswordLoaded {
                        request_id,
                        account_id,
                        password,
                    });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn select_game(&mut self, game: GameKind) {
        if self.selected_game == game {
            return;
        }
        self.selected_game = game;
        self.launch.cancel();
        self.captcha_revision = self.captcha_revision.wrapping_add(1);
        self.captcha_id = None;
        self.captcha_url = None;
        self.captcha_texture = None;
        self.captcha_value.clear();
        self.status = format!("已选择 {}", game.display_name());
    }

    fn begin_login(&mut self, game: GameKind, ctx: egui::Context) {
        if self.account.trim().is_empty() || self.password.is_empty() {
            self.status = "请输入账号和密码".into();
            return;
        }
        if self.captcha_id.is_some() && self.captcha_value.trim().is_empty() {
            self.status = "请输入验证码".into();
            return;
        }
        if self.captcha_id.is_none() {
            match OfficialAuthClient::new() {
                Ok(auth) => self.auth = Arc::new(auth),
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            }
        }
        let mut account = self
            .config
            .accounts
            .iter()
            .find(|entry| entry.account == self.account.trim())
            .cloned()
            .unwrap_or_else(|| AccountConfig::new(self.account.trim()));
        account.remember_password = self.save_password;
        let id = self.launch.begin();
        self.captcha_revision = self.captcha_revision.wrapping_add(1);
        let input = LaunchInput {
            session_id: id,
            game,
            account,
            password: self.password.clone(),
            captcha: self
                .captcha_id
                .clone()
                .map(|value| (value, self.captcha_value.clone())),
            storage_root: self.paths.data_dir.clone(),
        };
        let auth = self.auth.clone();
        let assets = self.assets.clone();
        let tx = self.tx.clone();
        self.page = Page::Busy;
        self.busy_step = 0;
        self.status = LaunchStage::Authenticating.label().into();
        self.launch.attach(self.rt.spawn(async move {
            prepare_launch(input, auth, assets, |event| {
                let _ = tx.send(AppMessage::Launch {
                    id,
                    event: Box::new(event),
                });
                ctx.request_repaint();
            })
            .await;
        }));
    }

    fn set_captcha_image(&mut self, ctx: &egui::Context, image: &[u8]) {
        self.captcha_texture = image::load_from_memory(image).ok().map(|image| {
            let image = image.to_rgba8();
            ctx.load_texture(
                "zm-login-captcha",
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            )
        });
    }

    fn poll_messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                AppMessage::Launch { id, event } => {
                    if !self.launch.accepts(id) {
                        continue;
                    }
                    match *event {
                        LaunchEvent::Stage(stage) => {
                            if self.launch.transition(id, stage) {
                                self.status = stage.label().into();
                                self.busy_step = if stage == LaunchStage::PreparingAssets {
                                    1
                                } else {
                                    2
                                };
                            }
                        }
                        LaunchEvent::Captcha {
                            id: captcha_id,
                            image_url,
                            image,
                            image_error,
                        } => {
                            if !self.launch.transition(id, LaunchStage::AwaitingCaptcha) {
                                continue;
                            }
                            self.captcha_id = Some(captcha_id);
                            self.captcha_url = Some(image_url);
                            self.captcha_value.clear();
                            self.set_captcha_image(ctx, &image);
                            self.page = Page::Login;
                            self.status = image_error
                                .map(|error| format!("验证码图片加载失败，请刷新：{error}"))
                                .unwrap_or_else(|| "请输入验证码后继续登录".into());
                        }
                        LaunchEvent::Prepared { launch, account } => {
                            if self.launch.stage() != LaunchStage::CreatingPlayer {
                                continue;
                            }
                            let game = launch.game;
                            match self.player.start(*launch) {
                                Ok(()) => {
                                    self.launch.transition(id, LaunchStage::AwaitingHost);
                                    self.startup_watchdog = StartupWatchdog::default();
                                    self.active_game = Some(game);
                                    self.active_account = Some(account.clone());
                                    self.page = Page::Game;
                                    self.busy_step = 3;
                                    self.status = "播放器已创建，等待游戏宿主初始化…".into();
                                    self.persist_account(account.clone());
                                    self.account_mode = AccountMode::Saved(account.id);
                                    let reply = self.credentials.save(
                                        &account.credential_id,
                                        &account.account,
                                        &self.password,
                                        self.save_password,
                                    );
                                    let tx = self.tx.clone();
                                    self.rt.spawn(async move {
                                        if let Err(error) = receive_credential(reply).await {
                                            let _ = tx.send(AppMessage::Notice(format!(
                                                "凭据操作失败，系统存储状态未更新：{error}"
                                            )));
                                        }
                                    });
                                    self.captcha_id = None;
                                    self.captcha_url = None;
                                    self.captcha_value.clear();
                                    self.captcha_texture = None;
                                }
                                Err(error) => {
                                    self.stop_game(format!("创建播放器失败：{error}"), false)
                                }
                            }
                        }
                        LaunchEvent::Failed(error) => {
                            let stage = self.launch.stage();
                            self.launch.transition(id, LaunchStage::Failed);
                            self.page = Page::Login;
                            self.status = format!("{}失败：{error}", stage.label());
                        }
                    }
                }
                AppMessage::CaptchaImage {
                    id,
                    revision,
                    result,
                } => {
                    if !self.launch.accepts(id)
                        || revision != self.captcha_revision
                        || self.launch.stage() != LaunchStage::AwaitingCaptcha
                    {
                        continue;
                    }
                    match result {
                        Ok(image) => {
                            self.set_captcha_image(ctx, &image);
                            self.status = "验证码已刷新".into();
                        }
                        Err(error) => self.status = format!("验证码刷新失败：{error}"),
                    }
                }
                AppMessage::Notice(message) => self.status = message,
                AppMessage::CacheCleared => self.status = "游戏缓存已清理".into(),
                AppMessage::PasswordLoaded {
                    request_id,
                    account_id,
                    password,
                } => {
                    if accepts_password_result(
                        self.account_mode,
                        self.credential_request_id,
                        request_id,
                        account_id,
                    ) {
                        match password {
                            Ok(Some(password)) => {
                                self.password = password;
                                self.credential_state = CredentialState::Available;
                            }
                            Ok(None) => {
                                self.password.clear();
                                self.credential_state = CredentialState::Missing;
                            }
                            Err(error) => {
                                self.password.clear();
                                self.credential_state = CredentialState::Error(error);
                            }
                        }
                    }
                }
            }
        }
    }

    fn poll_runtime_events(&mut self) {
        while let Ok(message) = self.runtime_rx.try_recv() {
            if !self.launch.accepts(message.session_id) {
                continue;
            }
            if matches!(
                &message.event,
                RuntimeEvent::HostReady
                    | RuntimeEvent::SessionApplied
                    | RuntimeEvent::ResourceLoaded { .. }
                    | RuntimeEvent::InitializationProgress
            ) && let Some(elapsed) = self.player.host_start_elapsed()
            {
                self.startup_watchdog.progress(elapsed);
            }
            match message.event {
                RuntimeEvent::HostReady => {
                    if self
                        .launch
                        .transition(message.session_id, LaunchStage::AwaitingSession)
                    {
                        self.status = "宿主已就绪，等待游戏请求登录…".into();
                    }
                }
                RuntimeEvent::SessionApplied => {
                    if self
                        .launch
                        .transition(message.session_id, LaunchStage::SessionApplied)
                    {
                        self.status = "登录会话已注入，游戏继续加载；可在诊断中查看运行状态".into();
                    }
                }
                RuntimeEvent::LogoutRequested | RuntimeEvent::ShowAccountPicker => {
                    self.confirm_switch = true
                }
                RuntimeEvent::PaymentBlocked => {
                    self.status = "客户端暂不支持直接打开支付页面".into()
                }
                RuntimeEvent::ResourceLoaded { .. } => {
                    if self.launch.stage() != LaunchStage::SessionApplied {
                        self.status = "正在加载游戏资源…".into();
                    }
                }
                RuntimeEvent::InitializationProgress => {
                    if self.launch.stage() != LaunchStage::SessionApplied {
                        self.status = "资源已加载，正在初始化游戏数据…".into();
                    }
                }
                RuntimeEvent::ResourceLoadFailed { resource, error } => {
                    self.status = format!("资源加载失败（{resource}）：{error}")
                }
                RuntimeEvent::FatalError(error) => {
                    self.pending_stop = Some(format!("游戏宿主错误：{error}"))
                }
            }
        }
    }

    fn save_config(&self) -> zm_core::Result<()> {
        if !self.config_writable {
            return Err(zm_core::ZmError::Config(
                "原配置读取失败，禁止覆盖；请先备份并修复配置文件".into(),
            ));
        }
        self.config_store.save(&self.config)
    }

    fn persist_account(&mut self, account: AccountConfig) {
        if let Some(existing) = self
            .config
            .accounts
            .iter_mut()
            .find(|entry| entry.id == account.id || entry.account == account.account)
        {
            *existing = account.clone();
        } else {
            self.config.accounts.push(account.clone());
        }
        self.config.last_account = Some(account.id);
        if let Err(error) = self.save_config() {
            self.status = format!("账号已登录，但保存账号列表失败：{error}");
        }
    }

    fn stop_game(&mut self, status: String, open_picker: bool) {
        if self.player.is_running() {
            self.last_diagnostics = Some(self.player.diagnostics());
        }
        self.pending_stop = None;
        self.launch.cancel();
        self.player.stop();
        self.active_game = None;
        self.active_account = None;
        self.page = Page::Login;
        self.status = status;
        self.confirm_switch = false;
        self.captcha_id = None;
        self.captcha_url = None;
        self.captcha_texture = None;
        self.captcha_value.clear();
        self.captcha_revision = self.captcha_revision.wrapping_add(1);
        if open_picker {
            self.account_picker_open = true;
        }
    }

    fn refresh_captcha(&mut self, ctx: egui::Context) {
        let Some(image_url) = self.captcha_url.clone() else {
            return;
        };
        let auth = self.auth.clone();
        let tx = self.tx.clone();
        let id = self.launch.current_id();
        self.captcha_revision = self.captcha_revision.wrapping_add(1);
        let revision = self.captcha_revision;
        self.captcha_value.clear();
        self.status = "正在刷新验证码…".into();
        self.launch.attach(self.rt.spawn(async move {
            let result = auth
                .fetch_captcha(&image_url)
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(AppMessage::CaptchaImage {
                id,
                revision,
                result,
            });
            ctx.request_repaint();
        }));
    }

    fn diagnostics(&self) -> String {
        if self.player.is_running() {
            self.player.diagnostics()
        } else {
            self.last_diagnostics
                .clone()
                .unwrap_or_else(|| self.player.diagnostics())
        }
    }
}

impl eframe::App for ZmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(reason) = self.pending_stop.take() {
            self.stop_game(reason, false);
        }
        self.poll_messages(ctx);
        self.poll_runtime_events();

        let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
        if ctx.input(|input| input.key_pressed(egui::Key::F11)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        } else if fullscreen && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }

        if !(self.page == Page::Game && fullscreen) {
            egui::TopBottomPanel::bottom("status-bar")
                .frame(
                    egui::Frame::new()
                        .fill(palette::DEEP_INK)
                        .inner_margin(egui::Margin::symmetric(10, 7)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&self.status).color(palette::MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                        });
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(if self.page == Page::Game {
                        egui::Color32::BLACK
                    } else {
                        palette::INK
                    })
                    .inner_margin(if self.page == Page::Game {
                        egui::Margin::ZERO
                    } else {
                        egui::Margin::same(10)
                    }),
            )
            .show(ctx, |ui| match self.page {
                Page::Login => {
                    egui::ScrollArea::vertical().show(ui, |ui| self.login_ui(ui, ctx));
                }
                Page::Busy => self.busy_ui(ui),
                Page::Game => self.game_ui(ui, ctx, fullscreen),
                Page::Settings => self.settings_ui(ui),
            });

        // A tick can enqueue session/progress events. Consume them before the watchdog.
        self.poll_runtime_events();
        if self.pending_stop.is_none()
            && self.launch.stage() != LaunchStage::SessionApplied
            && let Some(elapsed) = self.player.host_start_elapsed()
            && self
                .startup_watchdog
                .stalled(elapsed, SESSION_READY_TIMEOUT)
        {
            // Free the texture before constructing the NEXT frame, never after painting it.
            self.pending_stop =
                Some("游戏启动连续 90 秒没有加载进展，已停止。上次诊断已保留。".into());
            ctx.request_repaint();
        }
        self.account_picker(ctx);
        self.switch_confirmation(ctx);
        self.diagnostics_window(ctx);
        if !self.player.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self) {
        self.launch.cancel();
        self.player.stop();
        let _ = self.save_config();
    }
}

fn accepts_password_result(
    mode: AccountMode,
    current_request: u64,
    result_request: u64,
    result_account: Uuid,
) -> bool {
    current_request == result_request && mode == AccountMode::Saved(result_account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_password_result_is_rejected() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        assert!(!accepts_password_result(
            AccountMode::Saved(second),
            2,
            1,
            first
        ));
        assert!(!accepts_password_result(
            AccountMode::Saved(second),
            2,
            2,
            first
        ));
        assert!(accepts_password_result(
            AccountMode::Saved(second),
            2,
            2,
            second
        ));
    }

    #[test]
    fn embedded_icon_decodes() {
        let image = image::load_from_memory(crate::APP_ICON_PNG).unwrap();
        assert!(image.width() >= 256 && image.height() >= 256);
    }
}
