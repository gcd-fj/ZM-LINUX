use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ZmError {
    #[error("配置错误：{0}")]
    Config(String),
    #[error("凭据存储错误：{0}")]
    Credential(String),
    #[error("网络请求失败：{0}")]
    Network(String),
    #[error("服务器返回了无法识别的数据：{0}")]
    Protocol(String),
    #[error("游戏资源错误：{0}")]
    Asset(String),
    #[error("游戏运行时错误：{0}")]
    Runtime(String),
    #[error("文件操作失败 ({path}): {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, ZmError>;

impl ZmError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
