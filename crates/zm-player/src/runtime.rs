use crate::{
    diagnostics::{
        CompatibilityMetrics, FrameMetrics, FrameUpdateTimings, RedactingLogBackend,
        ResourceMetrics, StartupMetrics, TaskPollReport, redact,
    },
    input::{event_needs_game_focus, forward_event},
    navigator::{
        NavigatorSession, RestrictedNavigatorInterface, ZmNavigator, open_official_web_url,
        resource_root,
    },
    task_diagnostics::TaskDiagnostics,
    ui_backend::ZmUiBackend,
};
use egui::{Rect, TextureId};
use ruffle_core::{
    FloatDuration, LoadBehavior, Player, PlayerBuilder, PlayerEvent, PlayerRuntime,
    backend::navigator::{OwnedFuture, SocketMode},
    external::{ExternalInterfaceProvider, Value as ExternalValue},
    font::DefaultFont,
    tag_utils::SwfMovie,
};
use ruffle_frontend_utils::{
    backends::{
        audio::CpalAudioBackend,
        navigator::{ExternalNavigatorBackend, FutureSpawner},
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
    cell::RefCell,
    collections::{HashSet, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex, mpsc::Sender},
    time::{Duration, Instant},
};
use url::Url;
use zm_assets::AssetManager;
use zm_core::{GameKind, GameLaunchRequest, Result, ZmError};

pub const RUFFLE_REVISION: &str = "a4f5b5256e245693bc9077ef6c6b6abc95490e7f";
pub const GAME_WIDTH: u32 = 940;
pub const GAME_HEIGHT: u32 = 590;

/// Keep the loading movie's timeline running while dependencies are preloaded.
/// Delayed suppresses those frames, including the game's own loading animation.
pub const fn game_load_behavior() -> LoadBehavior {
    LoadBehavior::Streaming
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    HostReady,
    SessionApplied,
    InitializationProgress,
    LogoutRequested,
    ShowAccountPicker,
    PaymentOpened,
    PaymentOpenFailed(String),
    ResourceLoaded { resource: String, cache_hit: bool },
    ResourceLoadFailed { resource: String, error: String },
    FatalError(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeMessage {
    pub session_id: u64,
    pub event: RuntimeEvent,
}

#[derive(Clone)]
pub(crate) struct RuntimeEventSender {
    session_id: u64,
    sender: Sender<RuntimeMessage>,
}
impl RuntimeEventSender {
    pub(crate) fn send(
        &self,
        event: RuntimeEvent,
    ) -> std::result::Result<(), std::sync::mpsc::SendError<RuntimeMessage>> {
        self.sender.send(RuntimeMessage {
            session_id: self.session_id,
            event,
        })
    }
}

fn frame_interval(frame_rate: f64) -> Duration {
    let frame_rate = if frame_rate.is_finite() && frame_rate > 0.0 {
        // 遵循 SWF 自身帧率；只对异常元数据设置宽松安全上限。
        frame_rate.clamp(1.0, 240.0)
    } else {
        30.0
    };
    Duration::from_secs_f64(1.0 / frame_rate)
}

fn should_submit_render(frame_due: bool, needs_render: bool) -> bool {
    frame_due && needs_render
}

fn advance_frame_deadline(previous: Instant, tick_started: Instant, interval: Duration) -> Instant {
    let interval_nanos = interval.as_nanos();
    if interval_nanos == 0 {
        return tick_started;
    }
    let intervals = tick_started
        .saturating_duration_since(previous)
        .as_nanos()
        .checked_div(interval_nanos)
        .unwrap_or(0)
        .saturating_add(1);
    if intervals > u32::MAX as u128 {
        return tick_started.checked_add(interval).unwrap_or(tick_started);
    }
    previous
        .checked_add(interval.saturating_mul(intervals as u32))
        .or_else(|| tick_started.checked_add(interval))
        .unwrap_or(tick_started)
}

fn validate_main_swf_header(bytes: &[u8]) -> Result<()> {
    if matches!(bytes.get(..3), Some(b"FWS" | b"CWS" | b"ZWS")) {
        Ok(())
    } else {
        Err(ZmError::Runtime("游戏主文件格式无效".into()))
    }
}

fn configure_default_fonts(player: &mut Player) {
    player.set_default_font(
        DefaultFont::Serif,
        [
            "Times New Roman",
            "Noto Serif CJK SC",
            "Noto Serif",
            "Liberation Serif",
            "DejaVu Serif",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    player.set_default_font(
        DefaultFont::Sans,
        [
            "Verdana",
            "Arial",
            "Noto Sans CJK SC",
            "Noto Sans",
            "Liberation Sans",
            "DejaVu Sans",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    player.set_default_font(
        DefaultFont::Typewriter,
        [
            "Courier New",
            "Noto Sans Mono CJK SC",
            "Noto Sans Mono",
            "Liberation Mono",
            "DejaVu Sans Mono",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    let cjk = ["Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans"]
        .map(str::to_owned)
        .to_vec();
    player.set_default_font(DefaultFont::JapaneseGothic, cjk.clone());
    player.set_default_font(DefaultFont::JapaneseGothicMono, cjk.clone());
    player.set_default_font(DefaultFont::JapaneseMincho, cjk);
}

#[derive(Clone)]
pub struct GameFrameInput {
    pub viewport: Rect,
    pub events: Vec<egui::Event>,
    pub focused: bool,
}

struct EmbeddedSession {
    player: Arc<Mutex<Player>>,
    texture_id: TextureId,
    task_queue: TaskQueue,
    tasks: LocalTasks,
    game: GameKind,
    account: String,
    started_at: Instant,
    last_pointer: Option<(f64, f64)>,
    focused: bool,
    last_tick_at: Instant,
    next_tick_at: Instant,
}

/// Ruffle 与 egui 共享的纯 GPU 渲染目标。
///
/// Ruffle 会在创建播放器时调整目标尺寸，因此必须等 `PlayerBuilder::build`
/// 完成后再向 egui 注册纹理。直接共享纹理还可避免截图型目标所需的暂存缓冲复制。
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

/// 使用 egui 同一套 wgpu 设备渲染 Ruffle，只能在 UI 线程中调用。
pub struct GameRuntime {
    tokio: tokio::runtime::Handle,
    render_state: egui_wgpu::RenderState,
    repaint: egui::Context,
    events: Sender<RuntimeMessage>,
    assets: Arc<dyn AssetManager>,
    fonts: Arc<fontdb::Database>,
    // The render state has one fixed device/queue for this runtime's lifetime.
    // Only device-wide pipelines are shared; each session owns its renderer and textures.
    descriptors: Option<Arc<Descriptors>>,
    descriptors_created: u64,
    descriptors_reused: u64,
    session: Option<EmbeddedSession>,
    traces: Arc<Mutex<VecDeque<String>>>,
    secrets: Arc<Mutex<Vec<String>>>,
    last_error: Option<String>,
    volume: f32,
    frame_metrics: FrameMetrics,
    startup_metrics: StartupMetrics,
    resource_metrics: Arc<ResourceMetrics>,
    compatibility_metrics: Arc<CompatibilityMetrics>,
    task_diagnostics: Arc<TaskDiagnostics>,
}

impl GameRuntime {
    pub fn new(
        render_state: egui_wgpu::RenderState,
        repaint: egui::Context,
        events: Sender<RuntimeMessage>,
        assets: Arc<dyn AssetManager>,
        tokio: tokio::runtime::Handle,
        fonts: Arc<fontdb::Database>,
    ) -> Self {
        Self {
            tokio,
            render_state,
            repaint,
            events,
            assets,
            fonts,
            descriptors: None,
            descriptors_created: 0,
            descriptors_reused: 0,
            session: None,
            traces: Arc::new(Mutex::new(VecDeque::with_capacity(160))),
            secrets: Arc::new(Mutex::new(Vec::new())),
            last_error: None,
            volume: 1.0,
            frame_metrics: FrameMetrics::default(),
            startup_metrics: StartupMetrics::default(),
            resource_metrics: Arc::new(ResourceMetrics::default()),
            compatibility_metrics: Arc::new(CompatibilityMetrics::default()),
            task_diagnostics: Arc::new(TaskDiagnostics::default()),
        }
    }

    pub fn start(&mut self, request: GameLaunchRequest) -> Result<()> {
        let runtime = self.tokio.clone();
        let _runtime_guard = runtime.enter();
        self.stop();
        let startup_started = Instant::now();
        self.startup_metrics = StartupMetrics::default();
        let events = RuntimeEventSender {
            session_id: request.session_id,
            sender: self.events.clone(),
        };
        // 每次启动独立统计，避免切换造四/造五后把两款游戏的证据混在一起。
        self.traces = Arc::new(Mutex::new(VecDeque::with_capacity(160)));
        self.secrets = Arc::new(Mutex::new(Vec::new()));
        self.resource_metrics = Arc::new(ResourceMetrics::default());
        self.compatibility_metrics = Arc::new(CompatibilityMetrics::default());
        self.task_diagnostics = Arc::new(TaskDiagnostics::default());
        // The asset manager already read and verified these bytes. Do not reopen
        // the diagnostic path: it may have changed since launch preparation.
        let main_swf = request.main_swf_bytes.as_ref();
        validate_main_swf_header(main_swf)?;
        {
            let mut secrets = self.secrets.lock().unwrap();
            secrets.push(request.auth_token.clone());
            secrets.push(request.auth_cookie.clone());
        }

        let descriptors = if let Some(descriptors) = &self.descriptors {
            self.descriptors_reused = self.descriptors_reused.saturating_add(1);
            descriptors.clone()
        } else {
            let descriptors_started = Instant::now();
            let descriptors = Arc::new(Descriptors::new(
                wgpu::Instance::new(&wgpu::InstanceDescriptor::default()),
                self.render_state.adapter.clone(),
                self.render_state.device.clone(),
                self.render_state.queue.clone(),
            ));
            self.startup_metrics
                .descriptors
                .record(descriptors_started.elapsed());
            self.descriptors = Some(descriptors.clone());
            self.descriptors_created = self.descriptors_created.saturating_add(1);
            descriptors
        };
        let target = EguiTextureTarget::new(&descriptors.device, GAME_WIDTH, GAME_HEIGHT);
        let renderer = WgpuRenderBackend::new(descriptors, target)
            .map_err(|error| ZmError::Runtime(format!("初始化Ruffle渲染器失败：{error}")))?;

        let movie_url = Url::parse(&request.movie_url)
            .map_err(|error| ZmError::Runtime(format!("游戏地址无效：{error}")))?;
        let parse_started = Instant::now();
        let mut host_movie =
            SwfMovie::from_data(main_swf, request.movie_url.clone(), None, None)
                .map_err(|error| ZmError::Runtime(format!("游戏主文件损坏：{error}")))?;
        self.startup_metrics
            .movie_parse
            .record(parse_started.elapsed());
        let source_frame_rate: f64 = host_movie.frame_rate().into();
        let profile = request.game.profile();
        let (server, port) = (profile.server, profile.port);
        host_movie.append_parameters([
            ("path".into(), resource_root(request.game).into()),
            ("uid".into(), request.uid.to_string()),
            ("token".into(), request.auth_token.clone()),
            ("port".into(), port.to_string()),
            ("ip".into(), server.into()),
            ("username".into(), request.account_name.clone()),
            ("displayName".into(), request.account_display_name.clone()),
            ("gameId".into(), request.game.game_id().to_string()),
        ]);

        let task_queue: TaskQueue = Arc::new(Mutex::new(VecDeque::new()));
        let tasks: LocalTasks = Rc::new(RefCell::new(Vec::new()));
        let spawner = LocalSpawner {
            tasks: tasks.clone(),
            queue: task_queue.clone(),
            repaint: self.repaint.clone(),
            diagnostics: self.task_diagnostics.clone(),
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
            NavigatorSession {
                game: request.game,
                uid: request.uid,
                account: request.account_name.clone(),
                auth_cookie: request.auth_cookie.clone(),
            },
            self.assets.clone(),
            events.clone(),
            self.resource_metrics.clone(),
        );
        let external = ZmExternalInterface {
            page_url: request.movie_url.clone(),
            game: request.game,
            uid: request.uid,
            account: request.account_name.clone(),
            events: events.clone(),
            secrets: self.secrets.clone(),
        };
        let log = RedactingLogBackend {
            events,
            traces: self.traces.clone(),
            secrets: self.secrets.clone(),
            compatibility: self.compatibility_metrics.clone(),
        };
        let save_dir = request
            .storage_root
            .join("ruffle/shared-objects")
            .join(request.game.slug())
            .join(request.uid.to_string());
        let mut builder = PlayerBuilder::new()
            .with_movie(host_movie)
            .with_renderer(renderer)
            .with_navigator(navigator)
            .with_storage(Box::new(DiskStorageBackend::new(save_dir)))
            .with_external_interface(Box::new(external))
            .with_log(log)
            .with_ui(ZmUiBackend::new(self.fonts.clone()))
            .with_max_execution_duration(Duration::from_secs(15))
            .with_autoplay(true)
            .with_load_behavior(game_load_behavior())
            .with_viewport_dimensions(GAME_WIDTH, GAME_HEIGHT, 1.0)
            .with_page_url(Some("https://www.4399.com/flash/zmhj.htm".into()))
            .with_player_runtime(PlayerRuntime::FlashPlayer);
        if let Ok(audio) = CpalAudioBackend::new(None) {
            builder = builder.with_audio(audio);
        } else {
            tracing::warn!("音频输出不可用，将以静音模式继续运行");
        }
        let build_started = Instant::now();
        let player = builder.build();
        self.startup_metrics
            .player_build
            .record(build_started.elapsed());
        let texture = {
            let mut player_guard = player.lock().unwrap();
            player_guard.set_volume(self.volume);
            configure_default_fonts(&mut player_guard);
            let renderer = <dyn Any>::downcast_mut::<WgpuRenderBackend<EguiTextureTarget>>(
                player_guard.renderer_mut(),
            )
            .ok_or_else(|| ZmError::Runtime("无法取得嵌入式游戏纹理".into()))?;
            renderer.target().texture()
        };
        let texture_started = Instant::now();
        let texture_view = texture.create_view(&Default::default());
        let texture_id = self.render_state.renderer.write().register_native_texture(
            &self.render_state.device,
            &texture_view,
            wgpu::FilterMode::Linear,
        );
        self.startup_metrics
            .texture_register
            .record(texture_started.elapsed());

        self.last_error = None;
        self.frame_metrics = FrameMetrics::new(source_frame_rate);
        let now = Instant::now();
        self.session = Some(EmbeddedSession {
            player,
            texture_id,
            task_queue,
            tasks,
            game: request.game,
            account: request.account_display_name,
            started_at: Instant::now(),
            last_pointer: None,
            focused: false,
            last_tick_at: now,
            next_tick_at: now,
        });
        self.startup_metrics.total.record(startup_started.elapsed());
        self.repaint.request_repaint();
        tracing::info!(game = request.game.slug(), "嵌入式 Ruffle 播放器已启动");
        Ok(())
    }

    pub fn tick(&mut self, frame: GameFrameInput) -> Duration {
        let tick_started = Instant::now();
        let runtime = self.tokio.clone();
        let _runtime_guard = runtime.enter();
        let Some(session) = &mut self.session else {
            return Duration::from_millis(100);
        };
        self.task_diagnostics.start_observing(tick_started);
        let tasks_before = run_local_tasks(&session.task_queue);
        let input_started = Instant::now();
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
        let now = Instant::now();
        let input_elapsed = now.saturating_duration_since(input_started);
        let frame_rate = player.frame_rate();
        let interval = frame_interval(frame_rate);
        let due = now >= session.next_tick_at;
        let schedule_delay = now.saturating_duration_since(session.next_tick_at);
        let mut player_tick_elapsed = None;
        if due {
            let elapsed = now
                .saturating_duration_since(session.last_tick_at)
                .min(Duration::from_millis(250));
            let player_tick_started = Instant::now();
            player.tick(FloatDuration::from_millis(elapsed.as_secs_f64() * 1000.0));
            player_tick_elapsed = Some(player_tick_started.elapsed());
            session.last_tick_at = now;
            // Skip deadlines already missed before this tick, not deadlines
            // crossed while executing it. Ruffle has its own bounded catch-up;
            // waiting another whole slot after a costly tick can otherwise
            // lock it into two game frames per render at half the target rate.
            session.next_tick_at = advance_frame_deadline(
                session.next_tick_at,
                now,
                frame_interval(player.frame_rate()),
            );
        }
        let rendered = should_submit_render(due, player.needs_render());
        let mut render_submit_elapsed = None;
        if rendered {
            let render_started = Instant::now();
            player.render();
            render_submit_elapsed = Some(render_started.elapsed());
        }
        let frame_rate = player.frame_rate();
        drop(player);
        let tasks_after = run_local_tasks(&session.task_queue);
        session
            .tasks
            .borrow_mut()
            .retain(|task| !task.is_finished());
        self.frame_metrics.record(
            tick_started,
            FrameUpdateTimings {
                update: tick_started.elapsed(),
                input: input_elapsed,
                tasks: [tasks_before, tasks_after],
                player_tick: player_tick_elapsed,
                render_submit: render_submit_elapsed,
                schedule_late: due.then_some(schedule_delay),
                game_target_fps: frame_rate,
            },
        );
        session
            .next_tick_at
            .saturating_duration_since(Instant::now())
            .min(interval)
    }

    pub fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            self.render_state
                .renderer
                .write()
                .free_texture(&session.texture_id);
            // Dropping task handles cancels pending network futures. Poll their
            // cancellation on this thread because the AVM futures are !Send.
            session.tasks.borrow_mut().clear();
            while !session.task_queue.lock().unwrap().is_empty() {
                run_local_tasks(&session.task_queue);
            }
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

    pub fn resource_loading_progress(&self) -> Option<crate::ResourceLoadingProgress> {
        self.session.as_ref()?;
        let progress = self.resource_metrics.loading_progress();
        (progress.pending_requests > 0).then_some(progress)
    }

    /// Performance-only snapshot, without account data, AVM getters or logs.
    pub fn performance_summary(&self) -> String {
        let mut output = format!(
            "Game={}\n",
            self.session
                .as_ref()
                .map_or("none", |session| session.game.slug()),
        );
        output.push_str(&self.frame_metrics.summary());
        output.push_str(&self.resource_metrics.performance_summary());
        output
    }

    pub fn diagnostics(&self) -> String {
        let mut output = format!(
            "ZM-LINUX={}\nRuffle revision={}\nRuffle patches=json-number-precision-v1,date-formats-v1,bitmap-cache-origin-v1,timeline-overlay-v2,visible-render-bounds-v1,empty-filter-bounds-v1,sort-on-primitives-v1,slow-phase-trace-v1\nMode=embedded\nVolume={:.2}\n",
            env!("CARGO_PKG_VERSION"),
            RUFFLE_REVISION,
            self.volume
        );
        output.push_str(if cfg!(debug_assertions) {
            "Build debug_assertions=true\n"
        } else {
            "Build debug_assertions=false\n"
        });
        let adapter = self.render_state.adapter.get_info();
        output.push_str(&format!(
            "GPU: name={} backend={:?} type={:?}\n",
            adapter.name, adapter.backend, adapter.device_type
        ));
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
        if let Some(session) = &self.session
            && session.game.slug() == "zm4"
        {
            let state = session.player.lock().unwrap().call_internal_interface(
                "zmLinuxReadVipState",
                std::iter::empty::<ExternalValue>(),
            );
            if let ExternalValue::String(state) = state {
                output.push_str(&state);
                output.push('\n');
            }
        }
        output.push_str(&self.frame_metrics.summary());
        output.push_str(&format!(
            "Startup shared resources: main_swf=prepared_bytes font_database=shared file_read=omitted font_scan=omitted\nGPU descriptors (runtime lifetime): created={} reused={}\n",
            self.descriptors_created, self.descriptors_reused,
        ));
        output.push_str(&self.startup_metrics.summary());
        output.push_str(&self.task_diagnostics.summary());
        output.push_str(&self.resource_metrics.summary());
        output.push_str(&self.compatibility_metrics.summary());
        output.push_str("Recent sanitized AVM log:\n");
        for line in self.traces.lock().unwrap().iter().rev().take(40).rev() {
            output.push_str(line);
            output.push('\n');
        }
        redact(&output, &self.secrets)
    }
}

impl Drop for GameRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

type TaskQueue = Arc<Mutex<VecDeque<async_task::Runnable>>>;
type LocalTasks = Rc<RefCell<Vec<async_task::Task<()>>>>;

#[derive(Clone)]
struct LocalSpawner {
    tasks: LocalTasks,
    queue: TaskQueue,
    repaint: egui::Context,
    diagnostics: Arc<TaskDiagnostics>,
}

impl<E: std::error::Error + 'static> FutureSpawner<E> for LocalSpawner {
    fn spawn(&self, future: OwnedFuture<(), E>) {
        let future = async move {
            if let Err(error) = future.await {
                tracing::error!("Ruffle 异步任务失败：{error}");
            }
        };
        let queue = self.queue.clone();
        let future = self.diagnostics.instrument(future);
        let repaint = self.repaint.clone();
        let schedule = move |runnable| {
            queue.lock().unwrap().push_back(runnable);
            repaint.request_repaint();
        };
        let (runnable, task) = async_task::spawn_local(future, schedule);
        self.tasks.borrow_mut().push(task);
        runnable.schedule();
    }
}

fn run_local_tasks(queue: &TaskQueue) -> TaskPollReport {
    let started = Instant::now();
    let mut report = TaskPollReport::default();
    for _ in 0..512 {
        if started.elapsed() >= Duration::from_millis(4) {
            break;
        }
        let runnable = {
            let mut queue = queue.lock().unwrap();
            report.peak_queue_depth = report.peak_queue_depth.max(queue.len());
            queue.pop_front()
        };
        let Some(runnable) = runnable else {
            break;
        };
        let poll_started = Instant::now();
        runnable.run();
        report.polls += 1;
        report.max_single_poll = report.max_single_poll.max(poll_started.elapsed());
    }
    report.remaining = queue.lock().unwrap().len();
    report.peak_queue_depth = report.peak_queue_depth.max(report.remaining);
    report.elapsed = started.elapsed();
    report
}

struct ZmExternalInterface {
    page_url: String,
    game: GameKind,
    uid: u64,
    account: String,
    events: RuntimeEventSender,
    secrets: Arc<Mutex<Vec<String>>>,
}

fn payment_url(game: GameKind, uid: u64, account: &str) -> Url {
    let mut url = Url::parse("https://save.api.4399.com/exchange/v2/flash/Pay")
        .expect("static payment URL must be valid");
    url.query_pairs_mut()
        .append_pair("gameid", &game.game_id().to_string())
        .append_pair("userid", &uid.to_string())
        .append_pair("username", account)
        .append_pair("money", "10");
    url
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
            "zmLinux.sessionApplied" => {
                let _ = self.events.send(RuntimeEvent::SessionApplied);
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
                let url = payment_url(self.game, self.uid, &self.account);
                let event = match open_official_web_url(&url) {
                    Ok(()) => RuntimeEvent::PaymentOpened,
                    Err(error) => RuntimeEvent::PaymentOpenFailed(error),
                };
                let _ = self.events.send(event);
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
                tracing::debug!(method = name, "已忽略未实现的 ExternalInterface 调用");
                ExternalValue::Undefined
            }
        }
    }

    fn on_callback_available(&self, _name: &str) {}

    fn get_id(&self) -> Option<String> {
        Some("zm-linux".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_movie_bytes_preserve_compression_header_validation() {
        for bytes in [b"FWS".as_slice(), b"CWS", b"ZWS"] {
            assert!(validate_main_swf_header(bytes).is_ok());
        }
        for bytes in [b"".as_slice(), b"FW", b"<html>", b"fws"] {
            assert!(validate_main_swf_header(bytes).is_err());
        }
        // Magic validation remains separate from the full Ruffle parse.
        assert!(
            SwfMovie::from_data(b"FWS", "https://example.com/game.swf".into(), None, None).is_err()
        );
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
        crate::diagnostics::register_dynamic_token(message, &secrets);
        assert_eq!(
            redact(message, &secrets),
            "接受数据来自平台的token:<redacted>"
        );
    }

    #[test]
    fn schedules_swf_frames_at_thirty_fps() {
        let interval = frame_interval(30.0);
        assert!((interval.as_secs_f64() - 1.0 / 30.0).abs() < 0.000_001);
        assert_eq!(frame_interval(f64::NAN), interval);
        assert!((frame_interval(125.0).as_secs_f64() - 1.0 / 125.0).abs() < 0.000_001);
    }

    #[test]
    fn renders_only_when_a_due_frame_is_dirty() {
        assert!(should_submit_render(true, true));
        assert!(!should_submit_render(true, false));
        assert!(!should_submit_render(false, true));
        assert!(!should_submit_render(false, false));
    }

    #[test]
    fn frame_deadline_keeps_its_cadence_after_a_late_wakeup() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let completed = start + Duration::from_millis(17);
        assert_eq!(
            advance_frame_deadline(start, completed, interval),
            start + Duration::from_millis(20)
        );
    }

    #[test]
    fn frame_deadline_skips_missed_intervals_after_a_long_pause() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let completed = start + Duration::from_millis(95);
        assert_eq!(
            advance_frame_deadline(start, completed, interval),
            start + Duration::from_millis(100)
        );
    }

    #[test]
    fn expensive_tick_does_not_skip_the_next_frame_deadline_again() {
        let start = Instant::now();
        let interval = frame_interval(30.0);
        let completed = start + Duration::from_millis(36);
        let next = advance_frame_deadline(start, start, interval);
        assert_eq!(next, start + interval);
        assert!(next < completed); // The next update can run immediately.
    }

    #[test]
    fn bounded_catchup_recovers_render_cadence_after_one_stall() {
        // Deterministic model of Ruffle's accumulator and two-frame catch-up:
        // one frame (12ms) + bookkeeping (12ms) + render (2ms) fits 30Hz,
        // but two frames plus bookkeeping cross the next 33ms deadline.
        // A single 100ms stall must not lock subsequent renders at 15Hz.
        fn simulate(use_completion: bool) -> (u32, u32) {
            let origin = Instant::now();
            let interval = frame_interval(30.0);
            let mut now = origin;
            let mut last_tick = origin;
            let mut deadline = origin + interval;
            let mut accumulated = Duration::ZERO;
            let mut first = true;
            let (mut rendered, mut executed) = (0, 0);
            loop {
                let started = now.max(deadline);
                if started >= origin + Duration::from_secs(5) {
                    break;
                }
                accumulated += started.duration_since(last_tick);
                last_tick = started;
                let frames = (accumulated.as_nanos() / interval.as_nanos()).min(2) as u32;
                accumulated -= interval * frames;
                if accumulated >= interval {
                    accumulated = Duration::ZERO;
                }
                let completed = started
                    + Duration::from_millis(u64::from(frames) * 12 + 12)
                    + if first {
                        Duration::from_millis(100)
                    } else {
                        Duration::ZERO
                    };
                first = false;
                deadline = advance_frame_deadline(
                    deadline,
                    if use_completion { completed } else { started },
                    interval,
                );
                now = completed
                    + if frames > 0 {
                        Duration::from_millis(2)
                    } else {
                        Duration::ZERO
                    };
                if started >= origin + Duration::from_secs(1) {
                    executed += frames;
                    rendered += u32::from(frames > 0);
                }
            }
            (rendered, executed)
        }
        let (old_renders, old_frames) = simulate(true);
        let (renders, frames) = simulate(false);
        assert!((58..=62).contains(&old_renders));
        assert!((118..=122).contains(&renders));
        assert!(old_frames.abs_diff(frames) <= 2);
    }

    #[test]
    fn records_vip_claim_without_faking_the_red_point_state() {
        let metrics = CompatibilityMetrics::default();
        metrics.record("scene.vipHandler.getDailyReward");
        metrics.record("今日奖励已领取");
        let summary = metrics.summary();
        assert!(summary.contains("claimed_text=1"));
        assert!(summary.contains("red_point_text=0"));
        assert!(summary.contains("这些文本计数不能证明回调是否执行"));
    }

    #[test]
    fn payment_url_uses_the_selected_game_and_account() {
        let url = payment_url(GameKind::Zm5, 3940522873, "测试 account");
        let pairs = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("save.api.4399.com"));
        assert_eq!(
            pairs.get("gameid").map(|value| value.as_ref()),
            Some("100051601")
        );
        assert_eq!(
            pairs.get("userid").map(|value| value.as_ref()),
            Some("3940522873")
        );
        assert_eq!(
            pairs.get("username").map(|value| value.as_ref()),
            Some("测试 account")
        );
        assert_eq!(pairs.get("money").map(|value| value.as_ref()), Some("10"));
    }

    #[test]
    fn stopping_local_tasks_drops_pending_futures() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let queue: TaskQueue = Arc::new(Mutex::new(VecDeque::new()));
        let tasks: LocalTasks = Rc::new(RefCell::new(vec![]));
        let spawner = LocalSpawner {
            queue: queue.clone(),
            tasks: tasks.clone(),
            repaint: egui::Context::default(),
            diagnostics: Arc::new(TaskDiagnostics::default()),
        };
        let guard = Dropped(dropped.clone());
        let future: OwnedFuture<(), std::io::Error> = Box::pin(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
            Ok(())
        });
        spawner.spawn(future);
        let report = run_local_tasks(&queue);
        assert_eq!(report.polls, 1);
        assert_eq!(report.peak_queue_depth, 1);
        assert_eq!(report.remaining, 0);
        assert!(!dropped.load(Ordering::SeqCst));
        tasks.borrow_mut().clear();
        run_local_tasks(&queue);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(queue.lock().unwrap().is_empty());
    }
}
