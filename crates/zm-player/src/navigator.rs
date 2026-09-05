use crate::{
    diagnostics::ResourceMetrics,
    runtime::{RuntimeEvent, RuntimeEventSender},
};
use ruffle_core::{
    backend::navigator::{
        ErrorResponse, NavigationMethod, NavigatorBackend, OwnedFuture, Request, SuccessResponse,
    },
    indexmap::IndexMap,
    loader::Error as RuffleLoaderError,
    socket::{SocketAction, SocketHandle},
};
use ruffle_frontend_utils::backends::navigator::NavigatorInterface;
use std::{borrow::Cow, fs::File, io, path::Path, sync::Arc, time::Duration};
use url::Url;
use zm_assets::AssetManager;
use zm_core::GameKind;

#[derive(Clone)]
pub(crate) struct RestrictedNavigatorInterface;

impl NavigatorInterface for RestrictedNavigatorInterface {
    fn navigate_to_website(&self, url: Url) {
        if is_official_web_url(&url) {
            if let Err(error) = webbrowser::open(url.as_str()) {
                tracing::warn!("无法打开官方页面：{error}");
            }
        } else {
            tracing::warn!(url = %url, "已阻止非官方页面跳转");
        }
    }

    async fn open_file(&self, _path: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "禁止游戏读取本地文件",
        ))
    }

    async fn confirm_socket(&self, host: &str, _port: u16) -> bool {
        host.ends_with(".4399zmxy.com") || host.parse::<std::net::IpAddr>().is_ok()
    }
}

/// 规范化游戏内鉴权请求，并把官方静态 GET 资源接入统一缓存。
///
/// 鉴权 POST 只补齐当前内存会话所需的参数，仍交给 Ruffle 原始网络后端，
/// 不执行缓存或重试。SWF、图片、XML 和音频等官方 GET 资源才进入资源管理器。
pub(crate) struct ZmNavigator<N> {
    inner: N,
    game: GameKind,
    uid: u64,
    account: String,
    auth_cookie: String,
    assets: Arc<dyn AssetManager>,
    events: RuntimeEventSender,
    metrics: Arc<ResourceMetrics>,
}

pub(crate) struct NavigatorSession {
    pub(crate) game: GameKind,
    pub(crate) uid: u64,
    pub(crate) account: String,
    pub(crate) auth_cookie: String,
}

impl<N> ZmNavigator<N> {
    pub(crate) fn new(
        inner: N,
        session: NavigatorSession,
        assets: Arc<dyn AssetManager>,
        events: RuntimeEventSender,
        metrics: Arc<ResourceMetrics>,
    ) -> Self {
        Self {
            inner,
            game: session.game,
            uid: session.uid,
            account: session.account,
            auth_cookie: session.auth_cookie,
            assets,
            events,
            metrics,
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
                "已规范化游戏内 4399 令牌刷新请求"
            );
        }
        if let Some(resource) = cacheable_resource(self.game, request.method(), request.url()) {
            let url = request.url().to_owned();
            let assets = self.assets.clone();
            let events = self.events.clone();
            let metrics = self.metrics.clone();
            let game = self.game;
            return Box::pin(async move {
                match assets.fetch_resource(game, &resource).await {
                    Ok(asset) => {
                        metrics.record_success(&resource, asset.cache_hit);
                        let _ = events.send(RuntimeEvent::ResourceLoaded {
                            resource: resource.clone(),
                            cache_hit: asset.cache_hit,
                        });
                        tracing::info!(
                            game = game.slug(),
                            resource,
                            cache_hit = asset.cache_hit,
                            "运行时资源已就绪"
                        );
                        Ok(Box::new(RuntimeAssetResponse::new(url, asset.bytes))
                            as Box<dyn SuccessResponse>)
                    }
                    Err(error) => {
                        let message = error.to_string();
                        metrics.record_failure(&resource, &message);
                        let _ = events.send(RuntimeEvent::ResourceLoadFailed {
                            resource: resource.clone(),
                            error: message.clone(),
                        });
                        tracing::error!(
                            game = game.slug(),
                            resource,
                            error = %message,
                            "运行时资源加载失败"
                        );
                        Err(ErrorResponse {
                            url,
                            error: RuffleLoaderError::FetchError(message),
                        })
                    }
                }
            });
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

struct RuntimeAssetResponse {
    url: String,
    bytes: Vec<u8>,
    chunk_read: bool,
}

impl RuntimeAssetResponse {
    fn new(url: String, bytes: Vec<u8>) -> Self {
        Self {
            url,
            bytes,
            chunk_read: false,
        }
    }
}

impl SuccessResponse for RuntimeAssetResponse {
    fn url(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, RuffleLoaderError> {
        Box::pin(async move { Ok(self.bytes) })
    }

    fn text_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        None
    }

    fn status(&self) -> u16 {
        200
    }

    fn redirected(&self) -> bool {
        false
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, RuffleLoaderError> {
        if self.chunk_read {
            Box::pin(async { Ok(None) })
        } else {
            self.chunk_read = true;
            let bytes = self.bytes.clone();
            Box::pin(async move { Ok(Some(bytes)) })
        }
    }

    fn expected_length(&self) -> std::result::Result<Option<u64>, RuffleLoaderError> {
        Ok(Some(self.bytes.len() as u64))
    }
}

fn official_resource_path(game: GameKind, value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    let root = Url::parse(resource_root(game)).ok()?;
    if url.scheme() != "https" || url.host_str() != root.host_str() {
        return None;
    }
    let resource = url.path().strip_prefix(root.path())?;
    if resource.is_empty() {
        return None;
    }
    Some(match url.query() {
        Some(query) => format!("{resource}?{query}"),
        None => resource.to_owned(),
    })
}

fn cacheable_resource(game: GameKind, method: NavigationMethod, value: &str) -> Option<String> {
    if method != NavigationMethod::Get {
        return None;
    }
    official_resource_path(game, value)
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

fn is_official_web_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    matches!(url.host_str(), Some(host) if host == "4399.com"
        || host.ends_with(".4399.com") || host == "4399.cn"
        || host.ends_with(".4399.cn") || host.ends_with(".4399zmxy.com"))
}

pub(crate) fn resource_root(game: GameKind) -> &'static str {
    game.profile().resource_root
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn caches_only_official_get_resources() {
        let resource = "https://sda.4399.com/4399swf/upload_swf/ftp15/csya/20150127/1/ui/icon.swf";
        assert_eq!(
            cacheable_resource(GameKind::Zm4, NavigationMethod::Get, resource),
            Some("ui/icon.swf".into())
        );
        assert_eq!(
            cacheable_resource(GameKind::Zm4, NavigationMethod::Post, resource),
            None
        );
        assert_eq!(
            cacheable_resource(
                GameKind::Zm4,
                NavigationMethod::Get,
                "https://save.api.4399.com/?ac=user_auth"
            ),
            None
        );
    }

    #[test]
    fn rejects_resource_paths_outside_the_official_root() {
        assert_eq!(
            official_resource_path(
                GameKind::Zm4,
                "https://sda.4399.com/4399swf/upload_swf/ftp15/csya/20150127/1/../secret"
            ),
            None
        );
        assert_eq!(
            official_resource_path(GameKind::Zm4, "https://example.com/module.swf"),
            None
        );
    }
}
