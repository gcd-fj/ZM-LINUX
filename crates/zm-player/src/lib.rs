use egui::{Pos2, Rect, TextureId};
use ruffle_core::{
    FloatDuration, LoadBehavior, Player, PlayerBuilder, PlayerEvent, PlayerRuntime,
    backend::{
        log::LogBackend,
        navigator::{
            ErrorResponse, NavigationMethod, NavigatorBackend, OwnedFuture, Request, SocketMode,
            SuccessResponse,
        },
        ui::{
            DialogResultFuture, FileDialogResult, FileFilter, FontDefinition, FullscreenError,
            LanguageIdentifier, MouseCursor, MultiDialogResultFuture, MultiFileDialogResult,
            UiBackend, US_ENGLISH,
        },
    },
    events::{
        ImeEvent, KeyDescriptor, KeyLocation, LogicalKey, MouseButton, MouseWheelDelta, NamedKey,
        PhysicalKey,
    },
    external::{ExternalInterfaceProvider, Value as ExternalValue},
    font::{DefaultFont, FontFileData, FontQuery},
    indexmap::IndexMap,
    loader::Error as RuffleLoaderError,
    socket::{SocketAction, SocketHandle},
    tag_utils::SwfMovie,
};
use ruffle_frontend_utils::{
    backends::{
        audio::CpalAudioBackend,
        navigator::{ExternalNavigatorBackend, FutureSpawner, NavigatorInterface},
        storage::DiskStorageBackend,
    },
    content::{ContentDescriptor, PlayingContent},
};
use ruffle_render_wgpu::{
    backend::WgpuRenderBackend,
    descriptors::Descriptors,
    target::{RenderTarget, RenderTargetFrame},
};
use std::{
    any::Any,
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, mpsc::Sender},
    time::{Duration, Instant},
};
use url::Url;
use zm_core::{GameKind, GameLaunchRequest, Result, ZmError};

pub const RUFFLE_REVISION: &str = "a4f5b5256e245693bc9077ef6c6b6abc95490e7f";
pub const GAME_WIDTH: u32 = 940;
pub const GAME_HEIGHT: u32 = 590;

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    HostReady,
    LogoutRequested,
    ShowAccountPicker,
    PaymentBlocked,
    FatalError(String),
}

#[derive(Clone)]
pub struct GameFrameInput {
    pub elapsed: Duration,
    pub viewport: Rect,
    pub events: Vec<egui::Event>,
    pub focused: bool,
}

struct EmbeddedSession {
    player: Arc<Mutex<Player>>,
    texture_id: TextureId,
    task_queue: TaskQueue,
    game: GameKind,
    account: String,
    started_at: Instant,
    last_pointer: Option<(f64, f64)>,
    focused: bool,
}

/// A GPU-only render target shared by Ruffle and egui.
///
/// Ruffle resizes its target while building the player, so egui must register
/// the texture only after `PlayerBuilder::build` has finished. Keeping this
/// target in this crate also avoids the staging-buffer copy used by Ruffle's
/// screenshot-oriented `TextureTarget`.
#[derive(Debug)]
struct EguiTextureTarget {
    size: wgpu::Extent3d,
    texture: wgpu::Texture,
}

#[derive(Debug)]
struct EguiTextureFrame(wgpu::TextureView);

impl EguiTextureTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ZM-LINUX embedded game texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });
        Self { size, texture }
    }

    fn texture(&self) -> wgpu::Texture {
        self.texture.clone()
    }
}

impl RenderTargetFrame for EguiTextureFrame {
    fn into_view(self) -> wgpu::TextureView {
        self.0
    }

    fn view(&self) -> &wgpu::TextureView {
        &self.0
    }
}

impl RenderTarget for EguiTextureTarget {
    type Frame = EguiTextureFrame;

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        *self = Self::new(device, width, height);
    }

    fn format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    fn width(&self) -> u32 {
        self.size.width
    }

    fn height(&self) -> u32 {
        self.size.height
    }

    fn get_next_texture(&mut self) -> std::result::Result<Self::Frame, wgpu::SurfaceError> {
        Ok(EguiTextureFrame(
            self.texture.create_view(&Default::default()),
        ))
    }

    fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        command_buffers: I,
        _frame: Self::Frame,
    ) -> wgpu::SubmissionIndex {
        queue.submit(command_buffers)
    }
}

/// Ruffle rendered into the same wgpu device used by egui.
/// This type must only be used on the UI thread.
pub struct GameRuntime {
    tokio: tokio::runtime::Runtime,
    render_state: egui_wgpu::RenderState,
    repaint: egui::Context,
    events: Sender<RuntimeEvent>,
    session: Option<EmbeddedSession>,
    traces: Arc<Mutex<VecDeque<String>>>,
    secrets: Arc<Mutex<Vec<String>>>,
    last_error: Option<String>,
    volume: f32,
}

impl GameRuntime {
    pub fn new(
        render_state: egui_wgpu::RenderState,
        repaint: egui::Context,
        events: Sender<RuntimeEvent>,
    ) -> Self {
        Self {
            tokio: tokio::runtime::Runtime::new().expect("create embedded network runtime"),
            render_state,
            repaint,
            events,
            session: None,
            traces: Arc::new(Mutex::new(VecDeque::with_capacity(160))),
            secrets: Arc::new(Mutex::new(Vec::new())),
            last_error: None,
            volume: 1.0,
        }
    }

    pub fn start(&mut self, request: GameLaunchRequest) -> Result<()> {
        let runtime = self.tokio.handle().clone();
        let _runtime_guard = runtime.enter();
        self.stop();
        if !request.main_swf.is_file() {
            return Err(ZmError::Runtime("游戏主文件不存在".into()));
        }
        let main_swf = std::fs::read(&request.main_swf)
            .map_err(|error| ZmError::Runtime(format!("读取游戏主文件失败：{error}")))?;
        if !matches!(main_swf.get(..3), Some(b"FWS" | b"CWS" | b"ZWS")) {
            return Err(ZmError::Runtime("游戏主文件格式无效".into()));
        }
        {
            let mut secrets = self.secrets.lock().unwrap();
            secrets.push(request.auth_token.clone());
            secrets.push(request.auth_cookie.clone());
        }

        let descriptors = Arc::new(Descriptors::new(
            wgpu::Instance::new(&wgpu::InstanceDescriptor::default()),
            self.render_state.adapter.clone(),
            self.render_state.device.clone(),
            self.render_state.queue.clone(),
        ));
        let target = EguiTextureTarget::new(&descriptors.device, GAME_WIDTH, GAME_HEIGHT);
        let renderer = WgpuRenderBackend::new(descriptors, target)
            .map_err(|error| ZmError::Runtime(format!("初始化Ruffle渲染器失败：{error}")))?;

        let movie_url = Url::parse(&request.movie_url)
            .map_err(|error| ZmError::Runtime(format!("游戏地址无效：{error}")))?;
        let mut host_movie = SwfMovie::from_data(&main_swf, request.movie_url.clone(), None, None)
            .map_err(|error| ZmError::Runtime(format!("游戏主文件损坏：{error}")))?;
        let (server, port) = game_server(request.game);
        host_movie.append_parameters([
            ("path".into(), resource_root(request.game).into()),
            ("uid".into(), request.uid.to_string()),
            ("token".into(), request.auth_token.clone()),
            ("port".into(), port.to_string()),
            ("ip".into(), server.into()),
            ("username".into(), request.account_display_name.clone()),
            ("displayName".into(), request.account_display_name.clone()),
            ("gameId".into(), request.game.game_id().to_string()),
        ]);

        let task_queue: TaskQueue = Arc::new(Mutex::new(VecDeque::new()));
        let spawner = LocalSpawner {
            queue: task_queue.clone(),
            repaint: self.repaint.clone(),
        };
        let content = Rc::new(PlayingContent::DirectFile(ContentDescriptor::new_remote(
            movie_url.clone(),
        )));
        let navigator = ZmNavigator::new(
            ExternalNavigatorBackend::new(
                movie_url,
                Url::parse("https://www.4399.com/flash/zmhj.htm").ok(),
                None,
                spawner,
                None,
                true,
                HashSet::new(),
                SocketMode::Allow,
                content,
                RestrictedNavigatorInterface,
            ),
            request.game,
            request.uid,
            request.account_display_name.clone(),
            request.auth_cookie.clone(),
        );
        let external = ZmExternalInterface {
            page_url: request.movie_url.clone(),
            events: self.events.clone(),
            secrets: self.secrets.clone(),
        };
        let log = RedactingLogBackend {
            traces: self.traces.clone(),
            secrets: self.secrets.clone(),
        };
        let save_dir = request.cache_root.join("ruffle/shared-objects");
        let mut builder = PlayerBuilder::new()
            .with_movie(host_movie)
            .with_renderer(renderer)
            .with_navigator(navigator)
            .with_storage(Box::new(DiskStorageBackend::new(save_dir)))
            .with_external_interface(Box::new(external))
            .with_log(log)
            .with_autoplay(true)
            .with_load_behavior(LoadBehavior::Streaming)
            .with_viewport_dimensions(GAME_WIDTH, GAME_HEIGHT, 1.0)
            .with_page_url(Some("https://www.4399.com/flash/zmhj.htm".into()))
            .with_player_runtime(PlayerRuntime::FlashPlayer);
        if let Ok(audio) = CpalAudioBackend::new(None) {
            builder = builder.with_audio(audio);
        } else {
            tracing::warn!("audio output unavailable; continuing without sound");
        }
        let player = builder.build();
        let texture = {
            let mut player_guard = player.lock().unwrap();
            player_guard.set_volume(self.volume);
            let renderer = <dyn Any>::downcast_mut::<WgpuRenderBackend<EguiTextureTarget>>(
                player_guard.renderer_mut(),
            )
            .ok_or_else(|| ZmError::Runtime("无法取得嵌入式游戏纹理".into()))?;
            renderer.target().texture()
        };
        let texture_view = texture.create_view(&Default::default());
        let texture_id = self.render_state.renderer.write().register_native_texture(
            &self.render_state.device,
            &texture_view,
            wgpu::FilterMode::Linear,
        );

        self.last_error = None;
        self.session = Some(EmbeddedSession {
            player,
            texture_id,
            task_queue,
            game: request.game,
            account: request.account_display_name,
            started_at: Instant::now(),
            last_pointer: None,
            focused: false,
        });
        self.repaint.request_repaint();
        tracing::info!(game = request.game.slug(), "embedded Ruffle player started");
        Ok(())
    }

    pub fn tick(&mut self, frame: GameFrameInput) {
        let runtime = self.tokio.handle().clone();
        let _runtime_guard = runtime.enter();
        let Some(session) = &mut self.session else {
            return;
        };
        run_local_tasks(&session.task_queue);
        let mut player = session.player.lock().unwrap();
        if session.focused != frame.focused {
            session.focused = frame.focused;
            player.handle_event(if frame.focused {
                PlayerEvent::FocusGained
            } else {
                PlayerEvent::FocusLost
            });
        }
        for event in &frame.events {
            if !frame.focused && event_needs_game_focus(event) {
                continue;
            }
            if matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::F11,
                    ..
                }
            ) {
                continue;
            }
            forward_event(
                &mut player,
                event,
                frame.viewport,
                &mut session.last_pointer,
            );
        }
        player.tick(FloatDuration::from_millis(
            frame.elapsed.as_secs_f64().min(0.25) * 1000.0,
        ));
        player.render();
        drop(player);
        run_local_tasks(&session.task_queue);
        self.repaint.request_repaint_after(Duration::from_millis(8));
    }

    pub fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            self.render_state
                .renderer
                .write()
                .free_texture(&session.texture_id);
            session.task_queue.lock().unwrap().clear();
        }
        self.traces.lock().unwrap().clear();
        self.secrets.lock().unwrap().clear();
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(session) = &self.session {
            session.player.lock().unwrap().set_volume(self.volume);
        }
    }

    pub fn texture_id(&self) -> Option<TextureId> {
        self.session.as_ref().map(|session| session.texture_id)
    }

    pub fn is_running(&self) -> bool {
        self.session.is_some()
    }

    pub fn host_start_elapsed(&self) -> Option<Duration> {
        self.session
            .as_ref()
            .map(|session| session.started_at.elapsed())
    }

    pub fn diagnostics(&self) -> String {
        let mut output = format!(
            "ZM-LINUX={}\nRuffle revision={}\nMode=embedded\nVolume={:.2}\n",
            env!("CARGO_PKG_VERSION"),
            RUFFLE_REVISION,
            self.volume
        );
        if let Some(session) = &self.session {
            output.push_str(&format!(
                "Game={}\nAccount={}\n",
                session.game.slug(),
                session.account
            ));
        } else {
            output.push_str("Game=none\n");
        }
        if let Some(error) = &self.last_error {
            output.push_str(&format!("Last error={error}\n"));
        }
        output.push_str("Recent sanitized AVM log:\n");
        for line in self.traces.lock().unwrap().iter().rev().take(40).rev() {
            output.push_str(line);
            output.push('\n');
        }
        redact(&output, &self.secrets)
    }
}

fn event_needs_game_focus(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}

impl Drop for GameRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

type TaskQueue = Arc<Mutex<VecDeque<async_task::Runnable>>>;

#[derive(Clone)]
struct LocalSpawner {
    queue: TaskQueue,
    repaint: egui::Context,
}

impl<E: std::error::Error + 'static> FutureSpawner<E> for LocalSpawner {
    fn spawn(&self, future: OwnedFuture<(), E>) {
        let future = async move {
            if let Err(error) = future.await {
                tracing::error!("Ruffle async task failed: {error}");
            }
        };
        let queue = self.queue.clone();
        let repaint = self.repaint.clone();
        let schedule = move |runnable| {
            queue.lock().unwrap().push_back(runnable);
            repaint.request_repaint();
        };
        let (runnable, task) = async_task::spawn_local(future, schedule);
        task.detach();
        runnable.schedule();
    }
}

fn run_local_tasks(queue: &TaskQueue) {
    for _ in 0..512 {
        let Some(runnable) = queue.lock().unwrap().pop_front() else {
            break;
        };
        runnable.run();
    }
}

#[derive(Clone)]
struct RestrictedNavigatorInterface;

impl NavigatorInterface for RestrictedNavigatorInterface {
    fn navigate_to_website(&self, url: Url) {
        if is_official_web_url(&url) {
            if let Err(error) = webbrowser::open(url.as_str()) {
                tracing::warn!("unable to open official URL: {error}");
            }
        } else {
            tracing::warn!(url = %url, "blocked non-official navigation");
        }
    }

    async fn open_file(&self, _path: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local files are blocked",
        ))
    }

    async fn confirm_socket(&self, host: &str, _port: u16) -> bool {
        host.ends_with(".4399zmxy.com") || host.parse::<std::net::IpAddr>().is_ok()
    }
}

/// Normalizes the platform request expected by 4399's in-game token refresh.
///
/// The SWF refreshes its token after the selection server redirects it to the
/// actual game server. The legacy SWF sends an obsolete request shape and
/// Ruffle's desktop navigator identifies itself as `Ruffle/...`. Rebuilding
/// that one official request from the in-memory launch session keeps it
/// identical to the Rust-side request that already passed authentication.
struct ZmNavigator<N> {
    inner: N,
    game: GameKind,
    uid: u64,
    account: String,
    auth_cookie: String,
}

impl<N> ZmNavigator<N> {
    fn new(inner: N, game: GameKind, uid: u64, account: String, auth_cookie: String) -> Self {
        Self {
            inner,
            game,
            uid,
            account,
            auth_cookie,
        }
    }
}

impl<N: NavigatorBackend> NavigatorBackend for ZmNavigator<N> {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        self.inner.navigate_to_url(url, target, vars_method);
    }

    fn fetch(&self, mut request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        if is_game_auth_url(request.url()) {
            let mut headers = request.headers().clone();
            headers.insert("User-Agent".into(), "4399.air.wd|4399.zm5.air".into());
            if !self.auth_cookie.is_empty() {
                headers.insert("Cookie".into(), self.auth_cookie.clone());
            }
            request.set_headers(headers);
            let body = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("gameId", &self.game.game_id().to_string())
                .append_pair("userName", &self.account)
                .append_pair("userId", &self.uid.to_string())
                .finish();
            request.set_body((
                body.into_bytes(),
                "application/x-www-form-urlencoded".into(),
            ));
            tracing::info!(
                method = %request.method(),
                game = self.game.slug(),
                uid = self.uid,
                "normalized in-game 4399 token refresh"
            );
        }
        self.inner.fetch(request)
    }

    fn resolve_url(&self, url: &str) -> std::result::Result<Url, url::ParseError> {
        self.inner.resolve_url(url)
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), RuffleLoaderError>) {
        self.inner.spawn_future(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        self.inner.pre_process_url(url)
    }

    fn connect_socket(
        &mut self,
        host: String,
        port: u16,
        timeout: Duration,
        handle: SocketHandle,
        receiver: async_channel::Receiver<Vec<u8>>,
        sender: async_channel::Sender<SocketAction>,
    ) {
        self.inner
            .connect_socket(host, port, timeout, handle, receiver, sender);
    }
}

fn is_game_auth_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("save.api.4399.com")
            && url
                .query_pairs()
                .any(|(key, value)| key == "ac" && value == "user_auth")
    })
}

struct ZmExternalInterface {
    page_url: String,
    events: Sender<RuntimeEvent>,
    secrets: Arc<Mutex<Vec<String>>>,
}

impl ExternalInterfaceProvider for ZmExternalInterface {
    fn call_method(
        &self,
        _context: &mut ruffle_core::context::UpdateContext<'_>,
        name: &str,
        args: &[ExternalValue],
    ) -> ExternalValue {
        match name {
            "zmLinux.hostReady" => {
                let _ = self.events.send(RuntimeEvent::HostReady);
                ExternalValue::Undefined
            }
            "zmLinux.userLogOut" => {
                let _ = self.events.send(RuntimeEvent::LogoutRequested);
                ExternalValue::Undefined
            }
            "zmLinux.showAccountPicker" => {
                let _ = self.events.send(RuntimeEvent::ShowAccountPicker);
                ExternalValue::Undefined
            }
            "zmLinux.payMoney" => {
                let _ = self.events.send(RuntimeEvent::PaymentBlocked);
                ExternalValue::Undefined
            }
            "zmLinux.hostError" => {
                let message = args
                    .first()
                    .and_then(|value| match value {
                        ExternalValue::String(value) => Some(value.as_str()),
                        _ => None,
                    })
                    .unwrap_or("游戏平台桥接发生未知错误");
                let _ = self
                    .events
                    .send(RuntimeEvent::FatalError(redact(message, &self.secrets)));
                ExternalValue::Undefined
            }
            "eval" => {
                if matches!(args, [ExternalValue::String(code)] if code == "window.location.href" || code == "document.location.href" || code == "top.location.href")
                {
                    return self.page_url.clone().into();
                }
                ExternalValue::Undefined
            }
            value
                if value == "window.location.href.toString"
                    || value == "document.location.href.toString"
                    || value == "top.location.href.toString" =>
            {
                self.page_url.clone().into()
            }
            _ => {
                tracing::debug!(method = name, "ignored ExternalInterface call");
                ExternalValue::Undefined
            }
        }
    }

    fn on_callback_available(&self, _name: &str) {}

    fn get_id(&self) -> Option<String> {
        Some("zm-linux".into())
    }
}

struct RedactingLogBackend {
    traces: Arc<Mutex<VecDeque<String>>>,
    secrets: Arc<Mutex<Vec<String>>>,
}

impl LogBackend for RedactingLogBackend {
    fn avm_trace(&self, message: &str) {
        self.record("trace", message);
    }

    fn avm_warning(&self, message: &str) {
        self.record("warning", message);
    }
}

impl RedactingLogBackend {
    fn record(&self, level: &str, message: &str) {
        register_dynamic_token(message, &self.secrets);
        let line = format!("{level}: {}", redact(message, &self.secrets));
        tracing::info!(target: "zm_swf", "{line}");
        let mut traces = self.traces.lock().unwrap();
        if traces.len() >= 160 {
            traces.pop_front();
        }
        traces.push_back(line);
    }
}

fn register_dynamic_token(message: &str, secrets: &Arc<Mutex<Vec<String>>>) {
    let Some((_, candidate)) = message.rsplit_once("token:") else {
        return;
    };
    let candidate = candidate.trim();
    if candidate.matches('|').count() >= 4 && !candidate.starts_with("Error") {
        let mut values = secrets.lock().unwrap();
        if !values.iter().any(|value| value == candidate) {
            values.push(candidate.to_owned());
        }
    }
}

fn redact(value: &str, secrets: &Arc<Mutex<Vec<String>>>) -> String {
    let mut redacted = value.to_owned();
    for secret in secrets
        .lock()
        .unwrap()
        .iter()
        .filter(|value| !value.is_empty())
    {
        redacted = redacted.replace(secret, "<redacted>");
    }
    redacted
}

fn is_official_web_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    matches!(url.host_str(), Some(host) if host == "4399.com"
        || host.ends_with(".4399.com") || host == "4399.cn"
        || host.ends_with(".4399.cn") || host.ends_with(".4399zmxy.com"))
}

fn game_server(game: GameKind) -> (&'static str, u16) {
    match game {
        GameKind::Zm4 => ("g1-zm4.4399zmxy.com", 3010),
        GameKind::Zm5 => ("101.42.229.203", 3010),
    }
}

fn resource_root(game: GameKind) -> &'static str {
    match game {
        GameKind::Zm4 => "https://sda.4399.com/4399swf/upload_swf/ftp15/csya/20150127/1/",
        GameKind::Zm5 => "https://sda.4399.com/4399swf/upload_swf/ftp22/csya/20170622/1/",
    }
}

fn forward_event(
    player: &mut Player,
    event: &egui::Event,
    viewport: Rect,
    last: &mut Option<(f64, f64)>,
) {
    match event {
        egui::Event::PointerMoved(position) => {
            if let Some((x, y)) = game_position(*position, viewport) {
                *last = Some((x, y));
                player.set_mouse_in_stage(true);
                player.handle_event(PlayerEvent::MouseMove { x, y });
            } else if last.take().is_some() {
                player.set_mouse_in_stage(false);
                player.handle_event(PlayerEvent::MouseLeave);
            }
        }
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => {
            if let Some((x, y)) = game_position(*pos, viewport) {
                let button = match button {
                    egui::PointerButton::Primary => MouseButton::Left,
                    egui::PointerButton::Secondary => MouseButton::Right,
                    egui::PointerButton::Middle => MouseButton::Middle,
                    _ => MouseButton::Unknown,
                };
                player.handle_event(if *pressed {
                    PlayerEvent::MouseDown {
                        x,
                        y,
                        button,
                        index: None,
                    }
                } else {
                    PlayerEvent::MouseUp { x, y, button }
                });
            }
        }
        egui::Event::PointerGone => {
            *last = None;
            player.set_mouse_in_stage(false);
            player.handle_event(PlayerEvent::MouseLeave);
        }
        egui::Event::MouseWheel { delta, .. } if last.is_some() => {
            player.handle_event(PlayerEvent::MouseWheel {
                delta: MouseWheelDelta::Pixels(f64::from(delta.y)),
            });
        }
        egui::Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            ..
        } => {
            if *repeat && !*pressed {
                return;
            }
            if let Some(key) = key_descriptor(*physical_key, *key) {
                player.handle_event(if *pressed {
                    PlayerEvent::KeyDown { key }
                } else {
                    PlayerEvent::KeyUp { key }
                });
            }
        }
        egui::Event::Text(text) | egui::Event::Paste(text) => {
            for codepoint in text.chars() {
                player.handle_event(PlayerEvent::TextInput { codepoint });
            }
        }
        egui::Event::Ime(event) => match event {
            egui::ImeEvent::Preedit(text) => {
                player.handle_event(PlayerEvent::Ime(ImeEvent::Preedit(text.clone(), None)));
            }
            egui::ImeEvent::Commit(text) => {
                player.handle_event(PlayerEvent::Ime(ImeEvent::Commit(text.clone())));
            }
            _ => {}
        },
        egui::Event::WindowFocused(focused) => {
            player.handle_event(if *focused {
                PlayerEvent::FocusGained
            } else {
                PlayerEvent::FocusLost
            });
        }
        _ => {}
    }
}

fn game_position(position: Pos2, viewport: Rect) -> Option<(f64, f64)> {
    if !viewport.contains(position) || viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return None;
    }
    Some((
        f64::from((position.x - viewport.left()) / viewport.width()) * f64::from(GAME_WIDTH),
        f64::from((position.y - viewport.top()) / viewport.height()) * f64::from(GAME_HEIGHT),
    ))
}

fn key_descriptor(physical: Option<egui::Key>, logical: egui::Key) -> Option<KeyDescriptor> {
    Some(KeyDescriptor {
        physical_key: map_physical_key(physical.unwrap_or(logical)),
        logical_key: map_logical_key(logical)?,
        key_location: KeyLocation::Standard,
    })
}

fn map_logical_key(key: egui::Key) -> Option<LogicalKey> {
    use egui::Key;
    let named = match key {
        Key::ArrowDown => NamedKey::ArrowDown,
        Key::ArrowLeft => NamedKey::ArrowLeft,
        Key::ArrowRight => NamedKey::ArrowRight,
        Key::ArrowUp => NamedKey::ArrowUp,
        Key::Escape => NamedKey::Escape,
        Key::Tab => NamedKey::Tab,
        Key::Backspace => NamedKey::Backspace,
        Key::Enter => NamedKey::Enter,
        Key::Space => return Some(LogicalKey::Character(' ')),
        Key::Insert => NamedKey::Insert,
        Key::Delete => NamedKey::Delete,
        Key::Home => NamedKey::Home,
        Key::End => NamedKey::End,
        Key::PageUp => NamedKey::PageUp,
        Key::PageDown => NamedKey::PageDown,
        Key::F1 => NamedKey::F1,
        Key::F2 => NamedKey::F2,
        Key::F3 => NamedKey::F3,
        Key::F4 => NamedKey::F4,
        Key::F5 => NamedKey::F5,
        Key::F6 => NamedKey::F6,
        Key::F7 => NamedKey::F7,
        Key::F8 => NamedKey::F8,
        Key::F9 => NamedKey::F9,
        Key::F10 => NamedKey::F10,
        Key::F11 => NamedKey::F11,
        Key::F12 => NamedKey::F12,
        value => return key_character(value).map(LogicalKey::Character),
    };
    Some(LogicalKey::Named(named))
}

fn map_physical_key(key: egui::Key) -> PhysicalKey {
    use egui::Key;
    match key {
        Key::ArrowDown => PhysicalKey::ArrowDown,
        Key::ArrowLeft => PhysicalKey::ArrowLeft,
        Key::ArrowRight => PhysicalKey::ArrowRight,
        Key::ArrowUp => PhysicalKey::ArrowUp,
        Key::Escape => PhysicalKey::Escape,
        Key::Tab => PhysicalKey::Tab,
        Key::Backspace => PhysicalKey::Backspace,
        Key::Enter => PhysicalKey::Enter,
        Key::Space => PhysicalKey::Space,
        Key::Insert => PhysicalKey::Insert,
        Key::Delete => PhysicalKey::Delete,
        Key::Home => PhysicalKey::Home,
        Key::End => PhysicalKey::End,
        Key::PageUp => PhysicalKey::PageUp,
        Key::PageDown => PhysicalKey::PageDown,
        Key::F1 => PhysicalKey::F1,
        Key::F2 => PhysicalKey::F2,
        Key::F3 => PhysicalKey::F3,
        Key::F4 => PhysicalKey::F4,
        Key::F5 => PhysicalKey::F5,
        Key::F6 => PhysicalKey::F6,
        Key::F7 => PhysicalKey::F7,
        Key::F8 => PhysicalKey::F8,
        Key::F9 => PhysicalKey::F9,
        Key::F10 => PhysicalKey::F10,
        Key::F11 => PhysicalKey::F11,
        Key::F12 => PhysicalKey::F12,
        value => match key_character(value).map(|value| value.to_ascii_uppercase()) {
            Some('A') => PhysicalKey::KeyA,
            Some('B') => PhysicalKey::KeyB,
            Some('C') => PhysicalKey::KeyC,
            Some('D') => PhysicalKey::KeyD,
            Some('E') => PhysicalKey::KeyE,
            Some('F') => PhysicalKey::KeyF,
            Some('G') => PhysicalKey::KeyG,
            Some('H') => PhysicalKey::KeyH,
            Some('I') => PhysicalKey::KeyI,
            Some('J') => PhysicalKey::KeyJ,
            Some('K') => PhysicalKey::KeyK,
            Some('L') => PhysicalKey::KeyL,
            Some('M') => PhysicalKey::KeyM,
            Some('N') => PhysicalKey::KeyN,
            Some('O') => PhysicalKey::KeyO,
            Some('P') => PhysicalKey::KeyP,
            Some('Q') => PhysicalKey::KeyQ,
            Some('R') => PhysicalKey::KeyR,
            Some('S') => PhysicalKey::KeyS,
            Some('T') => PhysicalKey::KeyT,
            Some('U') => PhysicalKey::KeyU,
            Some('V') => PhysicalKey::KeyV,
            Some('W') => PhysicalKey::KeyW,
            Some('X') => PhysicalKey::KeyX,
            Some('Y') => PhysicalKey::KeyY,
            Some('Z') => PhysicalKey::KeyZ,
            Some('0') => PhysicalKey::Digit0,
            Some('1') => PhysicalKey::Digit1,
            Some('2') => PhysicalKey::Digit2,
            Some('3') => PhysicalKey::Digit3,
            Some('4') => PhysicalKey::Digit4,
            Some('5') => PhysicalKey::Digit5,
            Some('6') => PhysicalKey::Digit6,
            Some('7') => PhysicalKey::Digit7,
            Some('8') => PhysicalKey::Digit8,
            Some('9') => PhysicalKey::Digit9,
            _ => PhysicalKey::Unknown,
        },
    }
}

fn key_character(key: egui::Key) -> Option<char> {
    use egui::Key;
    Some(match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_viewport_coordinates() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(940.0, 590.0));
        assert_eq!(
            game_position(Pos2::new(100.0, 50.0), rect),
            Some((0.0, 0.0))
        );
        assert_eq!(
            game_position(Pos2::new(1040.0, 640.0), rect),
            Some((940.0, 590.0))
        );
        assert_eq!(game_position(Pos2::new(20.0, 20.0), rect), None);
    }

    #[test]
    fn redacts_complete_token() {
        let secrets = Arc::new(Mutex::new(vec!["uid|name|token".into()]));
        assert_eq!(redact("token=uid|name|token", &secrets), "token=<redacted>");
    }

    #[test]
    fn learns_and_redacts_refreshed_game_token() {
        let secrets = Arc::new(Mutex::new(Vec::new()));
        let message = "接受数据来自平台的token:1|account|nickname|1234567890|signature";
        register_dynamic_token(message, &secrets);
        assert_eq!(
            redact(message, &secrets),
            "接受数据来自平台的token:<redacted>"
        );
    }

    #[test]
    fn only_official_links_are_allowed() {
        assert!(is_official_web_url(
            &Url::parse("https://www.4399.com/x").unwrap()
        ));
        assert!(!is_official_web_url(
            &Url::parse("https://example.com/x").unwrap()
        ));
        assert!(!is_official_web_url(
            &Url::parse("javascript:alert(1)").unwrap()
        ));
    }

    #[test]
    fn recognizes_only_the_official_token_refresh_endpoint() {
        assert!(is_game_auth_url("https://save.api.4399.com/?ac=user_auth"));
        assert!(!is_game_auth_url("https://save.api.4399.com/?ac=get_time"));
        assert!(!is_game_auth_url("https://example.com/?ac=user_auth"));
    }
}
