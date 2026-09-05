use crate::GameKind;

/// Official game differences live here; runtime and resource code share this profile.
pub struct GameProfile {
    pub resource_root: &'static str,
    pub discovery_folder: &'static str,
    pub bridge_class: &'static str,
    pub server: &'static str,
    pub port: u16,
}

impl GameKind {
    pub const fn profile(self) -> &'static GameProfile {
        match self {
            Self::Zm4 => &GameProfile {
                resource_root: "https://sda.4399.com/4399swf/upload_swf/ftp15/csya/20150127/1/",
                discovery_folder: "ftp15",
                bridge_class: "ZmLinuxZm4Bridge",
                server: "g1-zm4.4399zmxy.com",
                port: 3010,
            },
            Self::Zm5 => &GameProfile {
                resource_root: "https://sda.4399.com/4399swf/upload_swf/ftp22/csya/20170622/1/",
                discovery_folder: "ftp22",
                bridge_class: "ZmLinuxZm5Bridge",
                server: "101.42.229.203",
                port: 3010,
            },
        }
    }
}
