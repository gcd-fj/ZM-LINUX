mod desktop;

use eframe::egui;
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zm_assets::{AssetManager, CacheScope, OfficialAssetManager};
use zm_auth::{AuthClient, CaptchaAnswer, LoginOutcome, LoginRequest, OfficialAuthClient};
use zm_core::{
    AccountConfig, AccountMode, AppConfig, AppPaths, ConfigStore, CredentialState, CredentialStore,
    GameKind, GameLaunchRequest, SecretServiceStore, SessionCredentialStore,
};
use zm_player::{GAME_HEIGHT, GAME_WIDTH, GameFrameInput, GameRuntime, RuntimeEvent};

const APP_ID: &str = "io.github.gcd-fj.zm-linux";
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/io.github.gcd-fj.zm-linux.png");
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(20);

mod palette {
    use eframe::egui::Color32;

    pub const INK: Color32 = Color32::from_rgb(18, 11, 9);
    pub const DEEP_INK: Color32 = Color32::from_rgb(8, 5, 4);
    pub const PANEL: Color32 = Color32::from_rgb(31, 18, 14);
    pub const CARD: Color32 = Color32::from_rgb(39, 22, 16);
    pub const FIELD: Color32 = Color32::from_rgb(24, 14, 11);
    pub const BORDER: Color32 = Color32::from_rgb(100, 57, 29);
    pub const GOLD: Color32 = Color32::from_rgb(245, 184, 56);
    pub const PALE_GOLD: Color32 = Color32::from_rgb(255, 226, 151);
    pub const CREAM: Color32 = Color32::from_rgb(255, 244, 216);
    pub const VERMILION: Color32 = Color32::from_rgb(190, 57, 28);
    pub const VERMILION_HOVER: Color32 = Color32::from_rgb(220, 78, 34);
    pub const JADE: Color32 = Color32::from_rgb(48, 173, 139);
    pub const MUTED: Color32 = Color32::from_rgb(190, 157, 116);
    pub const MUTED_DARK: Color32 = Color32::from_rgb(143, 111, 82);
}

fn main() -> eframe::Result {
    let paths = AppPaths::discover().expect("无法确定应用目录");
    paths.ensure().expect("无法创建应用目录");
    let _guard = init_logging(&paths);
    if let Err(error) = desktop::auto_install_for_appimage() {
        tracing::warn!("自动安装桌面入口失败：{error}");
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 620.0])
            .with_title("ZM-LINUX")
            .with_app_id(APP_ID)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "ZM-LINUX",
        options,
        Box::new(move |cc| Ok(Box::new(ZmApp::new(cc, paths)))),
    )
}

fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(APP_ICON_PNG)
        .expect("内置应用图标损坏")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn load_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(APP_ICON_PNG)
        .expect("内置应用图标损坏")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    ctx.load_texture(
        "zm-linux-app-icon",
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

fn init_logging(paths: &AppPaths) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("zm-linux")
        .filename_suffix("log")
        .build(&paths.log_dir)
    {
        Ok(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .with_ansi(false)
                .try_init();
            Some(guard)
        }
        Err(error) => {
            eprintln!(
                "无法写入日志目录 {}：{error}；本次启动改用标准错误输出",
                paths.log_dir.display()
            );
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .try_init();
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Login,
    Busy,
    Game,
    Settings,
}

enum AppMessage {
    NeedCaptcha {
        id: String,
        image_url: String,
        image: Vec<u8>,
    },
    Progress {
        step: usize,
        status: String,
    },
    LaunchPrepared {
        launch: GameLaunchRequest,
        account: AccountConfig,
    },
    Failed {
        step: usize,
        error: String,
    },
    CacheCleared,
    PasswordLoaded {
        request_id: u64,
        account_id: Uuid,
        password: Result<Option<String>, String>,
    },
}

fn configure_ui(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("zm-cjk".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "zm-cjk".into());
            break;
        }
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.interact_size.y = 38.0;
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = palette::INK;
    style.visuals.window_fill = palette::PANEL;
    style.visuals.extreme_bg_color = palette::DEEP_INK;
    style.visuals.faint_bg_color = palette::FIELD;
    style.visuals.widgets.noninteractive.bg_fill = palette::CARD;
    style.visuals.widgets.noninteractive.fg_stroke.color = palette::MUTED;
    style.visuals.widgets.inactive.bg_fill = palette::FIELD;
    style.visuals.widgets.inactive.weak_bg_fill = palette::FIELD;
    style.visuals.widgets.inactive.bg_stroke.color = palette::BORDER;
    style.visuals.widgets.inactive.fg_stroke.color = palette::CREAM;
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(78, 39, 20);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(78, 39, 20);
    style.visuals.widgets.hovered.bg_stroke.color = palette::GOLD;
    style.visuals.widgets.hovered.fg_stroke.color = palette::PALE_GOLD;
    style.visuals.widgets.active.bg_fill = palette::VERMILION;
    style.visuals.widgets.active.weak_bg_fill = palette::VERMILION;
    style.visuals.widgets.active.bg_stroke.color = palette::PALE_GOLD;
    style.visuals.widgets.active.fg_stroke.color = palette::CREAM;
    style.visuals.widgets.noninteractive.corner_radius = 9.into();
    style.visuals.widgets.inactive.corner_radius = 9.into();
    style.visuals.widgets.hovered.corner_radius = 9.into();
    style.visuals.widgets.active.corner_radius = 9.into();
    style.visuals.selection.bg_fill = palette::VERMILION;
    style.visuals.selection.stroke.color = palette::PALE_GOLD;
    style.visuals.hyperlink_color = palette::GOLD;
    ctx.set_style_of(egui::Theme::Dark, style);
}

struct ZmApp {
    paths: AppPaths,
    config_store: ConfigStore,
    config: AppConfig,
    rt: Runtime,
    auth: Arc<OfficialAuthClient>,
    assets: Arc<OfficialAssetManager>,
    player: GameRuntime,
    runtime_rx: Receiver<RuntimeEvent>,
    keyring: SecretServiceStore,
    session_credentials: SessionCredentialStore,
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
    host_ready: bool,
    last_frame: Instant,
}

impl ZmApp {
    fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        configure_ui(&cc.egui_ctx);
        let app_icon = load_icon_texture(&cc.egui_ctx);
        let config_store = ConfigStore::new(paths.config_file());
        let config = config_store.load().unwrap_or_default();
        let auth = Arc::new(OfficialAuthClient::new().expect("创建登录客户端失败"));
        let assets =
            Arc::new(OfficialAssetManager::new(&paths.cache_dir).expect("创建资源客户端失败"));
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("ZM-LINUX需要wgpu渲染后端");
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let mut player = GameRuntime::new(render_state, cc.egui_ctx.clone(), runtime_tx);
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
            rt: Runtime::new().expect("创建异步运行时失败"),
            auth,
            assets,
            player,
            runtime_rx,
            keyring: SecretServiceStore,
            session_credentials: SessionCredentialStore::default(),
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
            captcha_value: String::new(),
            captcha_id: None,
            captcha_url: None,
            captcha_texture: None,
            app_icon,
            status: "请选择游戏".into(),
            active_game: None,
            active_account: None,
            selected_game: GameKind::Zm4,
            save_password: true,
            busy_step: 0,
            host_ready: false,
            last_frame: Instant::now(),
        };
        app.select_account(initial_mode, cc.egui_ctx.clone());
        app
    }

    fn select_account(&mut self, mode: AccountMode, ctx: egui::Context) {
        self.credential_request_id = self.credential_request_id.wrapping_add(1);
        self.account_mode = mode;
        self.password.clear();
        self.captcha_id = None;
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
                let request_id = self.credential_request_id;
                self.credential_state = CredentialState::Loading { request_id };
                let keyring = self.keyring;
                let session = self.session_credentials.clone();
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let password = match keyring.load(&saved.credential_id, &saved.account).await {
                        Ok(Some(password)) => Ok(Some(password)),
                        Ok(None) => session
                            .load(&saved.credential_id, &saved.account)
                            .await
                            .map_err(|error| error.to_string()),
                        Err(keyring_error) => {
                            match session.load(&saved.credential_id, &saved.account).await {
                                Ok(Some(password)) => Ok(Some(password)),
                                _ => Err(format!("系统密钥环不可用：{keyring_error}")),
                            }
                        }
                    };
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

    fn begin_login(&mut self, game: GameKind, ctx: egui::Context) {
        if self.account.trim().is_empty() || self.password.is_empty() {
            self.status = "请输入账号和密码".into();
            return;
        }
        if self.captcha_id.is_some() && self.captcha_value.trim().is_empty() {
            self.status = "请输入验证码".into();
            return;
        }
        let auth = self.auth.clone();
        let assets = self.assets.clone();
        let tx = self.tx.clone();
        let account_name = self.account.trim().to_owned();
        let password = self.password.clone();
        let captcha_id = self.captcha_id.clone();
        let captcha_value = self.captcha_value.clone();
        let cache_root = self.paths.cache_dir.clone();
        let keyring = self.keyring;
        let session_credentials = self.session_credentials.clone();
        let save_password = self.save_password;
        let mut account_config = match self.account_mode {
            AccountMode::Saved(id) => self
                .config
                .accounts
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .unwrap_or_else(|| AccountConfig::new(&account_name)),
            AccountMode::New => self
                .config
                .accounts
                .iter()
                .find(|entry| entry.account == account_name)
                .cloned()
                .unwrap_or_else(|| AccountConfig::new(&account_name)),
        };
        self.page = Page::Busy;
        self.busy_step = 0;
        self.status = "正在进行4399认证…".into();
        self.rt.spawn(async move {
            let _ = tx.send(AppMessage::Progress {
                step: 0,
                status: "正在进行4399认证…".into(),
            });
            let captcha = captcha_id.as_deref().map(|id| CaptchaAnswer {
                id,
                value: &captcha_value,
            });
            let request = LoginRequest {
                account: &account_name,
                password: &password,
                game,
                captcha,
            };
            match auth.login(request).await {
                Ok(LoginOutcome::CaptchaRequired(challenge)) => {
                    let image = auth
                        .fetch_captcha(&challenge.image_url)
                        .await
                        .unwrap_or_default();
                    let _ = tx.send(AppMessage::NeedCaptcha {
                        id: challenge.id,
                        image_url: challenge.image_url,
                        image,
                    });
                }
                Ok(LoginOutcome::Authenticated(auth_session)) => {
                    let _ = tx.send(AppMessage::Progress {
                        step: 1,
                        status: "认证完成，正在检查游戏资源…".into(),
                    });
                    match assets.ensure_game(game).await {
                        Ok(asset) => {
                            account_config.uid = Some(auth_session.uid);
                            account_config.display_name = auth_session.display_name.clone();
                            if save_password
                                && keyring
                                    .save(&account_config.credential_id, &account_name, &password)
                                    .await
                                    .is_err()
                            {
                                let _ = session_credentials
                                    .save(&account_config.credential_id, &account_name, &password)
                                    .await;
                            }
                            let _ = tx.send(AppMessage::Progress {
                                step: 2,
                                status: "资源就绪，正在创建嵌入式播放器…".into(),
                            });
                            let _ = tx.send(AppMessage::LaunchPrepared {
                                launch: GameLaunchRequest {
                                    game,
                                    uid: auth_session.uid,
                                    account_display_name: auth_session.display_name,
                                    auth_token: auth_session.token,
                                    auth_cookie: auth_session.auth_cookie,
                                    cache_root,
                                    main_swf: asset.path,
                                    movie_url: asset.version.swf_url,
                                },
                                account: account_config,
                            });
                        }
                        Err(error) => {
                            let _ = tx.send(AppMessage::Failed {
                                step: 1,
                                error: error.to_string(),
                            });
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(AppMessage::Failed {
                        step: 0,
                        error: error.to_string(),
                    });
                }
            }
            ctx.request_repaint();
        });
    }

    fn poll_messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                AppMessage::NeedCaptcha {
                    id,
                    image_url,
                    image,
                } => {
                    self.captcha_id = Some(id);
                    self.captcha_url = Some(image_url);
                    self.captcha_texture = image::load_from_memory(&image).ok().map(|image| {
                        let image = image.to_rgba8();
                        let size = [image.width() as usize, image.height() as usize];
                        ctx.load_texture(
                            "zm-login-captcha",
                            egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    self.page = Page::Login;
                    self.status = "请输入验证码后重新登录".into();
                }
                AppMessage::Progress { step, status } => {
                    self.busy_step = step;
                    self.status = status;
                }
                AppMessage::LaunchPrepared { launch, account } => {
                    let game = launch.game;
                    match self.player.start(launch) {
                        Ok(()) => {
                            self.persist_account(account.clone());
                            self.account_mode = AccountMode::Saved(account.id);
                            self.active_game = Some(game);
                            self.active_account = Some(account);
                            self.page = Page::Game;
                            self.busy_step = 3;
                            self.host_ready = false;
                            self.last_frame = Instant::now();
                            self.status = format!("{}已启动", game.display_name());
                        }
                        Err(error) => {
                            self.page = Page::Login;
                            self.status = format!("创建播放器失败：{error}");
                        }
                    }
                }
                AppMessage::Failed { step, error } => {
                    self.busy_step = step;
                    self.status = format!("{}失败：{error}", progress_name(step));
                    self.page = Page::Login;
                }
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
        while let Ok(event) = self.runtime_rx.try_recv() {
            match event {
                RuntimeEvent::HostReady => {
                    self.host_ready = true;
                    self.status = self
                        .active_game
                        .map(|game| format!("{}已启动", game.display_name()))
                        .unwrap_or_else(|| "游戏已启动".into());
                }
                RuntimeEvent::LogoutRequested | RuntimeEvent::ShowAccountPicker => {
                    self.confirm_switch = true;
                }
                RuntimeEvent::PaymentBlocked => {
                    self.status = "为保护账号安全，客户端不会直接打开支付页面".into();
                }
                RuntimeEvent::FatalError(error) => {
                    self.stop_game(format!("注入登录会话失败：{error}"), false);
                }
            }
        }
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
        if let Err(error) = self.config_store.save(&self.config) {
            self.status = format!("账号已登录，但保存账号列表失败：{error}");
        }
    }

    fn stop_game(&mut self, status: String, open_picker: bool) {
        self.player.stop();
        self.active_game = None;
        self.active_account = None;
        self.host_ready = false;
        self.page = Page::Login;
        self.status = status;
        self.confirm_switch = false;
        self.auth = Arc::new(OfficialAuthClient::new().expect("重建登录客户端失败"));
        if open_picker {
            self.account_picker_open = true;
        }
    }

    fn refresh_captcha(&mut self, ctx: egui::Context) {
        let (Some(id), Some(image_url)) = (self.captcha_id.clone(), self.captcha_url.clone())
        else {
            return;
        };
        let auth = self.auth.clone();
        let tx = self.tx.clone();
        self.status = "正在刷新验证码…".into();
        self.rt.spawn(async move {
            match auth.fetch_captcha(&image_url).await {
                Ok(image) => {
                    let _ = tx.send(AppMessage::NeedCaptcha {
                        id,
                        image_url,
                        image,
                    });
                }
                Err(error) => {
                    let _ = tx.send(AppMessage::Failed {
                        step: 0,
                        error: format!("刷新验证码失败：{error}"),
                    });
                }
            }
            ctx.request_repaint();
        });
    }

    fn login_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let backdrop = ui.max_rect();
        ui.painter().circle_filled(
            egui::pos2(backdrop.left() + 220.0, backdrop.center().y),
            270.0,
            egui::Color32::from_rgba_unmultiplied(135, 42, 19, 34),
        );
        ui.painter().circle_filled(
            egui::pos2(backdrop.right() - 150.0, backdrop.top() + 90.0),
            180.0,
            egui::Color32::from_rgba_unmultiplied(231, 158, 42, 18),
        );

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("ZM")
                    .size(21.0)
                    .strong()
                    .color(palette::GOLD),
            );
            ui.label(egui::RichText::new("ZM-LINUX").size(17.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("设置").clicked() {
                    self.page = Page::Settings;
                }
                let backend = if std::env::var_os("WAYLAND_DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_SOCKET").is_some()
                {
                    "Wayland 原生"
                } else {
                    "X11 兼容"
                };
                ui.label(
                    egui::RichText::new(format!("● {backend}"))
                        .small()
                        .color(palette::JADE),
                );
            });
        });
        ui.add_space(18.0);

        ui.columns(2, |columns| {
            columns[0].add_space(20.0);
            columns[0].vertical(|ui| {
                ui.add(
                    egui::Image::new((self.app_icon.id(), egui::vec2(96.0, 96.0)))
                        .corner_radius(22),
                );
                ui.add_space(22.0);
                ui.label(
                    egui::RichText::new("把造梦带回 Linux")
                        .size(34.0)
                        .strong()
                        .color(palette::CREAM),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("原生登录、资源管理与游戏运行，\n一个窗口完成全部操作。")
                        .size(16.0)
                        .line_height(Some(24.0))
                        .color(palette::MUTED),
                );
                ui.add_space(28.0);
                ui.horizontal_wrapped(|ui| {
                    for feature in ["Rust 原生", "Wayland 优先", "安全密钥环"] {
                        egui::Frame::new()
                            .fill(palette::CARD)
                            .stroke(egui::Stroke::new(1.0, palette::BORDER))
                            .corner_radius(20)
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(feature)
                                        .small()
                                        .color(palette::PALE_GOLD),
                                );
                            });
                    }
                });
                ui.add_space(38.0);
                egui::Frame::new()
                    .fill(palette::FIELD)
                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("游戏资源按需从官方获取")
                                .strong()
                                .color(palette::PALE_GOLD),
                        );
                        ui.label(
                            egui::RichText::new("更新失败时保留上一份可用缓存")
                                .small()
                                .color(palette::MUTED_DARK),
                        );
                    });
            });

            egui::Frame::new()
                .fill(palette::CARD)
                .stroke(egui::Stroke::new(1.0, palette::BORDER))
                .corner_radius(18)
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(&mut columns[1], |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("开始游戏").size(25.0).strong());
                            ui.label(
                                egui::RichText::new("登录4399账号并选择游戏").color(palette::MUTED),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("切换账号").clicked() {
                                self.account_picker_open = true;
                            }
                        });
                    });
                    ui.add_space(14.0);

                    match self.account_mode {
                        AccountMode::Saved(id) => self.saved_account_ui(ui, id),
                        AccountMode::New => self.new_account_ui(ui),
                    }

                    if self.captcha_id.is_some() {
                        ui.add_space(3.0);
                        ui.label(egui::RichText::new("验证码").small().strong());
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.captcha_value)
                                    .hint_text("输入图中字符")
                                    .desired_width(150.0),
                            );
                            if let Some(texture) = &self.captcha_texture {
                                ui.add(
                                    egui::Image::new((texture.id(), texture.size_vec2()))
                                        .max_width(140.0)
                                        .corner_radius(6),
                                );
                            } else {
                                ui.label("图片加载失败");
                            }
                            if ui.small_button("刷新").clicked() {
                                self.refresh_captcha(ctx.clone());
                            }
                        });
                    }

                    ui.add_space(6.0);
                    ui.checkbox(&mut self.save_password, "安全保存到系统密钥环");
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("选择游戏").small().strong());
                    ui.columns(2, |game_columns| {
                        if game_columns[0]
                            .add_sized(
                                [game_columns[0].available_width(), 48.0],
                                egui::Button::new(egui::RichText::new("造梦西游 4").strong()).fill(
                                    if self.selected_game == GameKind::Zm4 {
                                        egui::Color32::from_rgb(177, 111, 27)
                                    } else {
                                        palette::FIELD
                                    },
                                ),
                            )
                            .clicked()
                        {
                            self.selected_game = GameKind::Zm4;
                        }
                        if game_columns[1]
                            .add_sized(
                                [game_columns[1].available_width(), 48.0],
                                egui::Button::new(egui::RichText::new("造梦西游 5").strong()).fill(
                                    if self.selected_game == GameKind::Zm5 {
                                        palette::VERMILION
                                    } else {
                                        palette::FIELD
                                    },
                                ),
                            )
                            .clicked()
                        {
                            self.selected_game = GameKind::Zm5;
                        }
                    });
                    ui.add_space(10.0);
                    let launch_label = format!("登录并启动 {}", self.selected_game.display_name());
                    let ready = !matches!(self.credential_state, CredentialState::Loading { .. });
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(
                                egui::RichText::new(launch_label)
                                    .strong()
                                    .color(palette::CREAM),
                            )
                            .fill(palette::VERMILION_HOVER)
                            .stroke(egui::Stroke::new(1.0, palette::GOLD))
                            .min_size(egui::vec2(ui.available_width(), 48.0)),
                        )
                        .clicked()
                    {
                        self.begin_login(self.selected_game, ctx.clone());
                    }
                    ui.add_space(4.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("密码、Cookie与token不会写入配置文件")
                                .small()
                                .color(palette::MUTED_DARK),
                        );
                    });
                });
        });
    }

    fn saved_account_ui(&mut self, ui: &mut egui::Ui, id: Uuid) {
        let Some(saved) = self.config.accounts.iter().find(|account| account.id == id) else {
            return;
        };
        egui::Frame::new()
            .fill(palette::FIELD)
            .stroke(egui::Stroke::new(1.0, palette::BORDER))
            .corner_radius(12)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("孙")
                            .size(22.0)
                            .strong()
                            .color(palette::GOLD),
                    );
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&saved.display_name).strong());
                        ui.label(
                            egui::RichText::new(&saved.account)
                                .small()
                                .color(palette::MUTED),
                        );
                    });
                });
            });
        match &self.credential_state {
            CredentialState::Loading { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("正在从系统密钥环读取密码…");
                });
            }
            CredentialState::Available => {
                ui.label(
                    egui::RichText::new("✓ 已从系统密钥环安全读取密码")
                        .small()
                        .color(palette::JADE),
                );
            }
            CredentialState::Missing => self.password_input(ui, "此账号没有已保存密码"),
            CredentialState::Error(error) => {
                let error = error.clone();
                ui.label(
                    egui::RichText::new(error)
                        .small()
                        .color(palette::VERMILION_HOVER),
                );
                self.password_input(ui, "密钥环不可用，请输入密码");
            }
        }
    }

    fn new_account_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("4399账号").small().strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.account)
                .hint_text("请输入4399账号")
                .desired_width(f32::INFINITY),
        );
        self.password_input(ui, "请输入账号密码");
    }

    fn password_input(&mut self, ui: &mut egui::Ui, hint: &str) {
        ui.add_space(3.0);
        ui.label(egui::RichText::new("密码").small().strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.password)
                .password(true)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    }

    fn busy_ui(&mut self, ui: &mut egui::Ui) {
        const STEPS: [&str; 4] = ["4399认证", "资源检查", "创建播放器", "注入会话"];
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.16).max(35.0));
            ui.add(
                egui::Image::new((self.app_icon.id(), egui::vec2(78.0, 78.0))).corner_radius(18),
            );
            ui.add_space(16.0);
            ui.heading(format!("正在启动 {}", self.selected_game.display_name()));
            ui.label(egui::RichText::new(&self.status).color(palette::MUTED));
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                for (index, label) in STEPS.iter().enumerate() {
                    let color = if index < self.busy_step {
                        palette::JADE
                    } else if index == self.busy_step {
                        palette::GOLD
                    } else {
                        palette::MUTED_DARK
                    };
                    egui::Frame::new()
                        .fill(palette::CARD)
                        .stroke(egui::Stroke::new(1.0, color))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::symmetric(18, 12))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{}  {label}", index + 1))
                                    .strong()
                                    .color(color),
                            );
                        });
                }
            });
            ui.add_space(18.0);
            ui.spinner();
        });
    }

    fn game_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, fullscreen: bool) {
        if !fullscreen {
            egui::Frame::new()
                .fill(palette::PANEL)
                .inner_margin(egui::Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let account = self
                            .active_account
                            .as_ref()
                            .map(|account| account.display_name.as_str())
                            .unwrap_or("未登录");
                        let game = self
                            .active_game
                            .map(GameKind::display_name)
                            .unwrap_or("游戏");
                        ui.label(
                            egui::RichText::new(format!("{game}  ·  {account}"))
                                .strong()
                                .color(palette::PALE_GOLD),
                        );
                        ui.separator();
                        ui.label("音量");
                        if ui
                            .add(
                                egui::Slider::new(&mut self.config.volume, 0.0..=1.0)
                                    .show_value(false)
                                    .max_decimals(2),
                            )
                            .changed()
                        {
                            self.player.set_volume(self.config.volume);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("退出游戏").clicked() {
                                self.stop_game("游戏已退出".into(), false);
                            }
                            if ui.button("全屏 F11").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                            }
                            if ui.button("诊断").clicked() {
                                self.diagnostics_open = true;
                            }
                            if ui.button("切换账号").clicked() {
                                self.confirm_switch = true;
                            }
                        });
                    });
                });
        }

        let available = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(available, 0.0, egui::Color32::BLACK);
        let game_aspect = GAME_WIDTH as f32 / GAME_HEIGHT as f32;
        let available_aspect = available.width() / available.height().max(1.0);
        let size = if available_aspect > game_aspect {
            egui::vec2(available.height() * game_aspect, available.height())
        } else {
            egui::vec2(available.width(), available.width() / game_aspect)
        };
        let game_rect = egui::Rect::from_center_size(available.center(), size);
        let response = ui.allocate_rect(game_rect, egui::Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }
        if let Some(texture_id) = self.player.texture_id() {
            egui::Image::new((texture_id, size)).paint_at(ui, game_rect);
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        let events = ctx.input(|input| input.raw.events.clone());
        self.player.tick(GameFrameInput {
            elapsed,
            viewport: game_rect,
            events,
            focused: response.has_focus() || response.hovered(),
        });
        if !self.host_ready
            && self
                .player
                .host_start_elapsed()
                .is_some_and(|elapsed| elapsed > SESSION_READY_TIMEOUT)
        {
            let diagnostics = self.player.diagnostics();
            self.stop_game(
                format!(
                    "注入登录会话超时；播放器已安全停止。可在设置中复制诊断信息。\n{}",
                    diagnostics.lines().take(4).collect::<Vec<_>>().join(" · ")
                ),
                false,
            );
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.add_space(8.0);
        if ui
            .add(egui::Slider::new(&mut self.config.volume, 0.0..=1.0).text("音量"))
            .changed()
        {
            self.player.set_volume(self.config.volume);
        }
        ui.label(format!("缓存目录：{}", self.paths.cache_dir.display()));
        ui.label(format!("日志目录：{}", self.paths.log_dir.display()));
        ui.label("开发者：gcd-fj");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("重新安装桌面入口").clicked() {
                self.status = match desktop::install() {
                    Ok(path) => format!("桌面入口已安装：{}", path.display()),
                    Err(error) => error,
                };
            }
            if ui.button("卸载桌面入口").clicked() {
                self.status = match desktop::uninstall() {
                    Ok(()) => "桌面入口已卸载（不会删除程序或缓存）".into(),
                    Err(error) => error,
                };
            }
        });
        if ui.button("清空全部游戏缓存").clicked() {
            let assets = self.assets.clone();
            let tx = self.tx.clone();
            self.rt.spawn(async move {
                let result = assets.clear_cache(CacheScope::All).await;
                let _ = tx.send(
                    result
                        .map(|_| AppMessage::CacheCleared)
                        .unwrap_or_else(|error| AppMessage::Failed {
                            step: 1,
                            error: error.to_string(),
                        }),
                );
            });
        }
        if let AccountMode::Saved(id) = self.account_mode
            && ui.button("删除当前保存的账号").clicked()
            && let Some(index) = self.config.accounts.iter().position(|entry| entry.id == id)
        {
            let removed = self.config.accounts.remove(index);
            self.config.last_account = None;
            let keyring = self.keyring;
            let session = self.session_credentials.clone();
            self.rt.spawn(async move {
                let _ = keyring
                    .delete(&removed.credential_id, &removed.account)
                    .await;
                let _ = session
                    .delete(&removed.credential_id, &removed.account)
                    .await;
            });
            let _ = self.config_store.save(&self.config);
            self.select_account(AccountMode::New, ui.ctx().clone());
            self.status = "账号记录已删除".into();
        }
        ui.separator();
        let diagnostics = self.player.diagnostics();
        egui::ScrollArea::vertical()
            .max_height(160.0)
            .show(ui, |ui| ui.monospace(&diagnostics));
        if ui.button("复制诊断信息").clicked() {
            ui.ctx().copy_text(diagnostics);
            self.status = "诊断信息已复制".into();
        }
        if ui.button("返回登录页").clicked() {
            let _ = self.config_store.save(&self.config);
            self.page = Page::Login;
        }
    }

    fn account_picker(&mut self, ctx: &egui::Context) {
        if !self.account_picker_open {
            return;
        }
        let mut open = self.account_picker_open;
        let mut selection = None;
        egui::Window::new("切换账号")
            .collapsible(false)
            .resizable(false)
            .default_width(390.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("选择一个已保存的4399账号").color(palette::MUTED));
                ui.add_space(6.0);
                for account in &self.config.accounts {
                    let selected = self.account_mode == AccountMode::Saved(account.id);
                    let label = format!("{}\n{}", account.display_name, account.account);
                    if ui
                        .add_sized(
                            [ui.available_width(), 52.0],
                            egui::Button::new(label).selected(selected),
                        )
                        .clicked()
                    {
                        selection = Some(AccountMode::Saved(account.id));
                    }
                }
                if self.config.accounts.is_empty() {
                    ui.label("还没有保存的账号");
                }
                ui.separator();
                if ui
                    .add_sized(
                        [ui.available_width(), 42.0],
                        egui::Button::new("＋ 使用其他账号"),
                    )
                    .clicked()
                {
                    selection = Some(AccountMode::New);
                }
            });
        self.account_picker_open = open;
        if let Some(mode) = selection {
            self.select_account(mode, ctx.clone());
            self.status = "账号已切换".into();
        }
    }

    fn switch_confirmation(&mut self, ctx: &egui::Context) {
        if !self.confirm_switch {
            return;
        }
        egui::Window::new("确认切换账号")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("切换账号会立即退出当前游戏，但不会删除游戏资源缓存。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.confirm_switch = false;
                    }
                    if ui
                        .add(
                            egui::Button::new("退出并切换")
                                .fill(palette::VERMILION)
                                .stroke(egui::Stroke::new(1.0, palette::GOLD)),
                        )
                        .clicked()
                    {
                        self.stop_game("当前游戏已退出，请选择账号".into(), true);
                    }
                });
            });
    }

    fn diagnostics_window(&mut self, ctx: &egui::Context) {
        if !self.diagnostics_open {
            return;
        }
        let diagnostics = self.player.diagnostics();
        egui::Window::new("诊断信息")
            .open(&mut self.diagnostics_open)
            .default_width(640.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| ui.monospace(&diagnostics));
                if ui.button("复制").clicked() {
                    ui.ctx().copy_text(diagnostics.clone());
                }
            });
    }
}

impl eframe::App for ZmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                Page::Login => self.login_ui(ui, ctx),
                Page::Busy => self.busy_ui(ui),
                Page::Game => self.game_ui(ui, ctx, fullscreen),
                Page::Settings => self.settings_ui(ui),
            });

        self.account_picker(ctx);
        self.switch_confirmation(ctx);
        self.diagnostics_window(ctx);
        ctx.request_repaint_after(if self.player.is_running() {
            Duration::from_millis(8)
        } else {
            Duration::from_millis(100)
        });
    }

    fn on_exit(&mut self) {
        self.player.stop();
        let _ = self.config_store.save(&self.config);
    }
}

fn progress_name(step: usize) -> &'static str {
    ["4399认证", "资源检查", "创建播放器", "注入会话"]
        .get(step)
        .copied()
        .unwrap_or("启动")
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
        let image = image::load_from_memory(APP_ICON_PNG).unwrap();
        assert!(image.width() >= 256 && image.height() >= 256);
    }
}
