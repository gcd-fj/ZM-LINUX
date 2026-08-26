use fontdb::{Family, Source};
use ruffle_core::{
    backend::ui::{
        DialogResultFuture, FileDialogResult, FileFilter, FontDefinition, FullscreenError,
        LanguageIdentifier, MouseCursor, MultiDialogResultFuture, MultiFileDialogResult,
        US_ENGLISH, UiBackend,
    },
    font::{FontFileData, FontQuery},
};
use std::fs;
use url::Url;

/// 为嵌入式播放器提供系统字体和不依赖桌面窗口句柄的基础 UI 能力。
///
/// 文件选择和全屏由外层 egui 界面负责，因此这里保持安全的取消/空操作语义；
/// 系统字体则必须真实接通，否则 Flash 的设备字体请求会反复失败并造成文字缺失。
pub(crate) struct ZmUiBackend {
    fonts: fontdb::Database,
    mouse_visible: bool,
    clipboard: String,
}

impl ZmUiBackend {
    pub(crate) fn new() -> Self {
        let mut fonts = fontdb::Database::new();
        fonts.load_system_fonts();
        tracing::info!(font_faces = fonts.faces().count(), "系统字体数据库已加载");
        Self {
            fonts,
            mouse_visible: true,
            clipboard: String::new(),
        }
    }

    fn candidate_names(name: &str) -> Vec<&str> {
        let mut candidates = vec![name];
        match name.to_ascii_lowercase().as_str() {
            "times new roman" | "times" | "_serif" | "serif" => candidates.extend([
                "Noto Serif CJK SC",
                "Noto Serif CJK JP",
                "Noto Serif",
                "Liberation Serif",
                "DejaVu Serif",
            ]),
            "verdana" | "arial" | "_sans" | "sans" => candidates.extend([
                "Noto Sans CJK SC",
                "Noto Sans CJK JP",
                "Noto Sans",
                "Liberation Sans",
                "DejaVu Sans",
            ]),
            "courier new" | "_typewriter" | "monospace" => candidates.extend([
                "Noto Sans Mono CJK SC",
                "Noto Sans Mono",
                "Liberation Mono",
                "DejaVu Sans Mono",
            ]),
            _ => {}
        }
        candidates
    }

    fn find_font(&self, query: &FontQuery) -> Option<FontDefinition<'static>> {
        for candidate in Self::candidate_names(&query.name) {
            let database_query = fontdb::Query {
                families: &[Family::Name(candidate)],
                weight: if query.is_bold {
                    fontdb::Weight::BOLD
                } else {
                    fontdb::Weight::NORMAL
                },
                style: if query.is_italic {
                    fontdb::Style::Italic
                } else {
                    fontdb::Style::Normal
                },
                ..Default::default()
            };
            let Some(face_id) = self.fonts.query(&database_query) else {
                continue;
            };
            let Some(face) = self.fonts.face(face_id) else {
                continue;
            };
            let data = match &face.source {
                Source::File(path) => match fs::read(path) {
                    Ok(bytes) => FontFileData::new(bytes),
                    Err(error) => {
                        tracing::warn!(font = %query.name, %error, "读取系统字体失败");
                        continue;
                    }
                },
                Source::Binary(bytes) | Source::SharedFile(_, bytes) => {
                    FontFileData::new_shared(bytes.clone())
                }
            };
            tracing::debug!(
                requested = %query.name,
                fallback = candidate,
                "已匹配设备字体"
            );
            return Some(FontDefinition::FontFile {
                // 使用 SWF 请求的字体名注册，确保 Ruffle 能命中本次查询。
                name: query.name.clone(),
                is_bold: query.is_bold,
                is_italic: query.is_italic,
                data,
                index: face.index,
            });
        }
        None
    }
}

impl UiBackend for ZmUiBackend {
    fn mouse_visible(&self) -> bool {
        self.mouse_visible
    }

    fn set_mouse_visible(&mut self, visible: bool) {
        self.mouse_visible = visible;
    }

    fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {}

    fn clipboard_content(&mut self) -> String {
        self.clipboard.clone()
    }

    fn set_clipboard_content(&mut self, content: String) {
        self.clipboard = content;
    }

    fn set_fullscreen(&mut self, _is_full: bool) -> Result<(), FullscreenError> {
        Ok(())
    }

    fn display_root_movie_download_failed_message(&self, invalid_swf: bool, error: String) {
        tracing::error!(invalid_swf, %error, "主游戏文件加载失败");
    }

    fn message(&self, message: &str) {
        tracing::warn!(%message, "游戏显示消息");
    }

    fn open_virtual_keyboard(&self) {}

    fn close_virtual_keyboard(&self) {}

    fn language(&self) -> LanguageIdentifier {
        US_ENGLISH.clone()
    }

    fn display_unsupported_video(&self, url: Url) {
        tracing::warn!(%url, "当前播放器不支持该视频资源");
    }

    fn load_device_font(&self, query: &FontQuery, register: &mut dyn FnMut(FontDefinition)) {
        if let Some(font) = self.find_font(query) {
            register(font);
        } else {
            tracing::warn!(font = %query.name, "未找到可用的设备字体回退");
        }
    }

    fn sort_device_fonts(
        &self,
        query: &FontQuery,
        register: &mut dyn FnMut(FontDefinition),
    ) -> Vec<FontQuery> {
        self.load_device_font(query, register);
        Vec::new()
    }

    fn display_file_open_dialog(
        &mut self,
        _filters: Vec<FileFilter>,
    ) -> Option<DialogResultFuture> {
        Some(Box::pin(async { Ok(FileDialogResult::Canceled) }))
    }

    fn display_file_open_dialog_multiple(
        &mut self,
        _filters: Vec<FileFilter>,
    ) -> Option<MultiDialogResultFuture> {
        Some(Box::pin(async { Ok(MultiFileDialogResult::Canceled) }))
    }

    fn display_file_save_dialog(
        &mut self,
        _file_name: String,
        _title: String,
    ) -> Option<DialogResultFuture> {
        None
    }

    fn close_file_dialog(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::ZmUiBackend;
    use ruffle_core::font::{FontQuery, FontType};

    #[test]
    fn 常见_windows_字体包含_linux_回退() {
        let sans = ZmUiBackend::candidate_names("Verdana");
        let serif = ZmUiBackend::candidate_names("Times New Roman");
        assert!(sans.contains(&"Noto Sans CJK SC"));
        assert!(sans.contains(&"DejaVu Sans"));
        assert!(serif.contains(&"Noto Serif CJK SC"));
        assert!(serif.contains(&"DejaVu Serif"));
    }

    #[test]
    fn 未知字体仍优先查询原名() {
        assert_eq!(ZmUiBackend::candidate_names("Custom Font"), ["Custom Font"]);
    }

    #[test]
    fn 无系统字体时安全返回未匹配() {
        let backend = ZmUiBackend {
            fonts: fontdb::Database::new(),
            mouse_visible: true,
            clipboard: String::new(),
        };
        let query = FontQuery::new(FontType::Device, "Missing Font".into(), false, false);
        assert!(backend.find_font(&query).is_none());
    }
}
