mod application;
#[cfg(target_os = "linux")]
mod desktop;
mod theme;
use application::ZmApp;

use eframe::egui;
use tracing_subscriber::EnvFilter;
use zm_storage::AppPaths;

const APP_ID: &str = "io.github.gcd-fj.zm-linux";
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/io.github.gcd-fj.zm-linux.png");

fn main() -> eframe::Result {
    let paths = AppPaths::discover().expect("无法确定应用目录");
    paths.ensure().expect("无法创建应用目录");
    let _guard = init_logging(&paths);
    #[cfg(target_os = "linux")]
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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // 默认只保留依赖告警和第一方运行信息，避免 Ruffle/WGPU 的逐帧日志刷屏。
        EnvFilter::new("warn,zm_app=info,zm_auth=info,zm_assets=info,zm_player=info,zm_swf=info")
    });
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
