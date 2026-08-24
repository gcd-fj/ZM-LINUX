use image::{DynamicImage, ImageFormat, imageops::FilterType};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const APP_ID: &str = "io.github.gcd-fj.zm-linux";
const ICON: &[u8] = include_bytes!("../../../assets/io.github.gcd-fj.zm-linux.png");
const ICON_SIZES: [u32; 4] = [64, 128, 256, 512];

pub fn auto_install_for_appimage() -> Result<bool, String> {
    let Some(appimage) = env::var_os("APPIMAGE") else {
        return Ok(false);
    };
    install_with_executable(&PathBuf::from(appimage))?;
    Ok(true)
}

pub fn install() -> Result<PathBuf, String> {
    let executable = env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| env::current_exe().ok())
        .ok_or_else(|| "无法确定程序路径".to_owned())?;
    install_with_executable(&executable)?;
    Ok(executable)
}

pub fn uninstall() -> Result<(), String> {
    uninstall_at(&data_home())?;
    refresh_caches();
    Ok(())
}

fn install_with_executable(executable: &Path) -> Result<(), String> {
    install_at(&data_home(), executable, ICON)?;
    refresh_caches();
    Ok(())
}

fn data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

fn install_at(data_home: &Path, executable: &Path, icon: &[u8]) -> Result<(), String> {
    if !executable.is_absolute() {
        return Err("桌面入口的程序路径必须是绝对路径".into());
    }
    let applications = data_home.join("applications");
    fs::create_dir_all(&applications).map_err(|error| format!("创建桌面入口目录失败：{error}"))?;
    let desktop_path = applications.join(format!("{APP_ID}.desktop"));
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=ZM-LINUX\nComment=造梦西游 Linux 原生客户端\nExec={}\nIcon={APP_ID}\nCategories=Game;\nKeywords=造梦西游;西游;Linux;\nStartupWMClass={APP_ID}\nTerminal=false\n",
        quote_exec(executable)
    );
    atomic_write(&desktop_path, desktop.as_bytes())?;

    let source = image::load_from_memory(icon).map_err(|error| format!("内置图标损坏：{error}"))?;
    for size in ICON_SIZES {
        let directory = data_home.join(format!("icons/hicolor/{size}x{size}/apps"));
        fs::create_dir_all(&directory).map_err(|error| format!("创建图标目录失败：{error}"))?;
        let resized = if source.width() == size && source.height() == size {
            source.clone()
        } else {
            DynamicImage::ImageRgba8(
                source
                    .resize_exact(size, size, FilterType::Lanczos3)
                    .to_rgba8(),
            )
        };
        let path = directory.join(format!("{APP_ID}.png"));
        let temporary = directory.join(format!(".{APP_ID}.{size}.tmp"));
        resized
            .save_with_format(&temporary, ImageFormat::Png)
            .map_err(|error| format!("写入图标失败：{error}"))?;
        fs::rename(&temporary, &path).map_err(|error| format!("安装图标失败：{error}"))?;
    }
    Ok(())
}

fn uninstall_at(data_home: &Path) -> Result<(), String> {
    remove_if_exists(&data_home.join(format!("applications/{APP_ID}.desktop")))?;
    for size in ICON_SIZES {
        remove_if_exists(
            &data_home.join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png")),
        )?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除 {} 失败：{error}", path.display())),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("desktop.tmp");
    fs::write(&temporary, bytes).map_err(|error| format!("写入桌面入口失败：{error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("安装桌面入口失败：{error}"))
}

fn quote_exec(path: &Path) -> String {
    let value = path.to_string_lossy();
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

fn refresh_caches() {
    if let Some(data_home) = data_home().to_str() {
        let icon_cache = format!("{data_home}/icons/hicolor");
        let _ = Command::new("update-desktop-database")
            .arg(format!("{data_home}/applications"))
            .status();
        let _ = Command::new("gtk-update-icon-cache")
            .args(["-f", "-t", &icon_cache])
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_uninstall_is_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let executable = Path::new("/tmp/ZM $LINUX` test.AppImage");
        install_at(directory.path(), executable, ICON).unwrap();
        install_at(directory.path(), executable, ICON).unwrap();
        let desktop = fs::read_to_string(
            directory
                .path()
                .join(format!("applications/{APP_ID}.desktop")),
        )
        .unwrap();
        assert!(desktop.contains("Exec=\"/tmp/ZM \\$LINUX\\` test.AppImage\""));
        let desktop_path = directory
            .path()
            .join(format!("applications/{APP_ID}.desktop"));
        let icon_path = directory
            .path()
            .join(format!("icons/hicolor/512x512/apps/{APP_ID}.png"));
        assert!(icon_path.exists());
        uninstall_at(directory.path()).unwrap();
        assert!(!desktop_path.exists());
        assert!(!icon_path.exists());
        assert!(!desktop.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn relative_executable_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        assert!(install_at(directory.path(), Path::new("zm-linux"), ICON).is_err());
    }
}
