use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountMode {
    Saved(Uuid),
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialState {
    Loading { request_id: u64 },
    Available,
    Missing,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameKind {
    Zm4,
    Zm5,
}

impl GameKind {
    pub const fn game_id(self) -> u32 {
        match self {
            Self::Zm4 => 100_036_512,
            Self::Zm5 => 100_051_601,
        }
    }
    pub const fn number(self) -> u8 {
        match self {
            Self::Zm4 => 4,
            Self::Zm5 => 5,
        }
    }
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Zm4 => "zm4",
            Self::Zm5 => "zm5",
        }
    }
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Zm4 => "造梦西游4",
            Self::Zm5 => "造梦西游5",
        }
    }
}

#[derive(Clone)]
pub struct GameLaunchRequest {
    pub session_id: u64,
    pub game: GameKind,
    pub account_name: String,
    pub uid: u64,
    pub account_display_name: String,
    pub auth_token: String,
    /// 已认证的平台 Cookie，仅用于当前内存会话刷新游戏令牌。
    pub auth_cookie: String,
    pub storage_root: PathBuf,
    pub main_swf: PathBuf,
    /// 播放缓存内容时用作影片身份的官方地址。
    pub movie_url: String,
}
