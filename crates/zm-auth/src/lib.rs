mod cryptojs;

use async_trait::async_trait;
use regex::Regex;
use reqwest::{
    Client, StatusCode, Url,
    cookie::{CookieStore, Jar},
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use zm_core::{GameKind, Result, ZmError};

pub use cryptojs::{encrypt_password, encrypt_password_with_salt};

const LOGIN_URL: &str = "https://ptlogin.4399.com/ptlogin/login.do?v=1";
const UID_URL: &str = "https://cz.4399.com/get_role_info.php?ac=cuid";
const AUTH_URL: &str = "https://save.api.4399.com/?ac=user_auth";
const CAPTCHA_PREFIX: &str = "https://ptlogin.4399.com/ptlogin/captcha.do?captchaId=";
const USER_AGENT: &str = "4399.air.wd|4399.zm5.air";

#[derive(Debug, Clone)]
pub struct LoginRequest<'a> {
    pub account: &'a str,
    pub password: &'a str,
    pub game: GameKind,
    pub captcha: Option<CaptchaAnswer<'a>>,
}
#[derive(Debug, Clone, Copy)]
pub struct CaptchaAnswer<'a> {
    pub id: &'a str,
    pub value: &'a str,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub uid: u64,
    pub token: String,
    pub display_name: String,
    /// 已认证的 4399 Cookie，仅保留在当前游戏进程的内存中。
    pub auth_cookie: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptchaChallenge {
    pub id: String,
    pub image_url: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Authenticated(AuthSession),
    CaptchaRequired(CaptchaChallenge),
}

#[async_trait]
pub trait AuthClient: Send + Sync {
    async fn resolve_uid(&self, account: &str) -> Result<u64>;
    async fn login(&self, request: LoginRequest<'_>) -> Result<LoginOutcome>;
    async fn submit_captcha(&self, request: LoginRequest<'_>) -> Result<LoginOutcome>;
    async fn request_game_token(&self, game: GameKind, account: &str, uid: u64) -> Result<String>;
}

#[derive(Clone)]
pub struct OfficialAuthClient {
    client: Client,
    jar: Arc<Jar>,
}

impl OfficialAuthClient {
    pub fn new() -> Result<Self> {
        let jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_provider(jar.clone())
            .timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| ZmError::Network(e.to_string()))?;
        Ok(Self { client, jar })
    }

    fn auth_cookie_header(&self) -> String {
        let Ok(url) = Url::parse(AUTH_URL) else {
            return String::new();
        };
        self.jar
            .cookies(&url)
            .and_then(|value| value.to_str().ok().map(str::to_owned))
            .unwrap_or_default()
    }

    pub async fn fetch_captcha(&self, image_url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(image_url).map_err(|e| ZmError::Protocol(e.to_string()))?;
        if url.scheme() != "https" || url.host_str() != Some("ptlogin.4399.com") {
            return Err(ZmError::Protocol("拒绝非4399验证码地址".into()));
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ZmError::Network(e.to_string()))?;
        Ok(response
            .bytes()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?
            .to_vec())
    }

    async fn post_login(&self, request: &LoginRequest<'_>) -> Result<Option<CaptchaChallenge>> {
        let encrypted = encrypt_password(request.password)?;
        let mut form = HashMap::from([
            ("loginFrom", "uframe".to_owned()),
            ("iframeId", "popup_login_frame".to_owned()),
            ("postLoginHandler", "default".to_owned()),
            ("layoutSelfAdapting", "true".to_owned()),
            ("externalLogin", "qq".to_owned()),
            ("displayMode", "popup".to_owned()),
            ("layout", "vertical".to_owned()),
            ("bizId", "1199006632".to_owned()),
            ("appId", "dev4399".to_owned()),
            ("gameId", "".to_owned()),
            ("css", "".to_owned()),
            ("redirectUrl", "".to_owned()),
            ("mainDivId", "popup_login_div".to_owned()),
            ("includeFcmInfo", "false".to_owned()),
            ("level", "4".to_owned()),
            ("regLevel", "4".to_owned()),
            ("userNameLabel", "4399用户名".to_owned()),
            ("userNameTip", "请输入4399用户名".to_owned()),
            ("welcomeTip", "欢迎回到4399".to_owned()),
            ("sec", "1".to_owned()),
            ("password", encrypted),
            ("username", request.account.to_owned()),
            ("autoLogin", "on".to_owned()),
            ("sessionId", String::new()),
        ]);
        if let Some(captcha) = request.captcha {
            form.insert("sessionId", captcha.id.to_owned());
            form.insert("inputCaptcha", captcha.value.to_owned());
        }
        let response = self
            .client
            .post(LOGIN_URL)
            .header("Referer", "https://ptlogin.4399.com/ptlogin/loginFrame.do")
            .form(&form)
            .send()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?;
        if status == StatusCode::ACCEPTED {
            return Err(ZmError::Protocol(body));
        }
        for (needle, message) in [
            ("密码错误", "密码错误"),
            ("账号不存在", "账号不存在"),
            ("验证码错误", "验证码错误"),
            ("不正确", "账号或密码不正确"),
        ] {
            if body.contains(needle) {
                return Err(ZmError::Protocol(message.into()));
            }
        }
        if body.contains("login_captcha") || body.contains("ptlogin/captcha.do?captchaId") {
            let re = Regex::new(r#"/ptlogin/captcha\.do\?captchaId=([^\"&]+)"#).unwrap();
            let id = re
                .captures(&body)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_owned())
                .ok_or_else(|| ZmError::Protocol("登录要求验证码，但响应中缺少captchaId".into()))?;
            return Ok(Some(CaptchaChallenge {
                image_url: format!("{CAPTCHA_PREFIX}{id}"),
                id,
            }));
        }
        Ok(None)
    }
}

#[async_trait]
impl AuthClient for OfficialAuthClient {
    async fn resolve_uid(&self, account: &str) -> Result<u64> {
        let mut url = Url::parse(UID_URL).map_err(|e| ZmError::Protocol(e.to_string()))?;
        url.query_pairs_mut().append_pair("uname", account);
        let text = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?
            .error_for_status()
            .map_err(|e| ZmError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?;
        text.trim()
            .parse::<u64>()
            .ok()
            .filter(|uid| *uid > 0)
            .ok_or_else(|| ZmError::Protocol("获取UID失败，请检查账号".into()))
    }

    async fn login(&self, request: LoginRequest<'_>) -> Result<LoginOutcome> {
        if request.account.trim().is_empty() || request.password.is_empty() {
            return Err(ZmError::Protocol("请输入账号密码".into()));
        }
        let uid = self.resolve_uid(request.account).await?;
        if let Ok(token) = self
            .request_game_token(request.game, request.account, uid)
            .await
        {
            return Ok(LoginOutcome::Authenticated(AuthSession {
                uid,
                token,
                display_name: request.account.into(),
                auth_cookie: self.auth_cookie_header(),
            }));
        }
        if let Some(challenge) = self.post_login(&request).await? {
            return Ok(LoginOutcome::CaptchaRequired(challenge));
        }
        let token = self
            .request_game_token(request.game, request.account, uid)
            .await?;
        Ok(LoginOutcome::Authenticated(AuthSession {
            uid,
            token,
            display_name: request.account.into(),
            auth_cookie: self.auth_cookie_header(),
        }))
    }

    async fn submit_captcha(&self, request: LoginRequest<'_>) -> Result<LoginOutcome> {
        if request.captcha.is_none() {
            return Err(ZmError::Protocol("缺少验证码".into()));
        }
        self.login(request).await
    }

    async fn request_game_token(&self, game: GameKind, account: &str, uid: u64) -> Result<String> {
        let response = self
            .client
            .post(AUTH_URL)
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("gameId", game.game_id().to_string()),
                ("userName", account.to_owned()),
                ("userId", uid.to_string()),
            ])
            .send()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?;
        let body = response
            .text()
            .await
            .map_err(|e| ZmError::Network(e.to_string()))?;
        if body.starts_with("Error") || !body.contains('|') {
            return Err(ZmError::Protocol("尚未获得游戏授权".into()));
        }
        tracing::info!(game = game.slug(), uid, "game token acquired");
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captcha_parser_contract() {
        let re = Regex::new(r#"/ptlogin/captcha\.do\?captchaId=([^\"&]+)"#).unwrap();
        assert_eq!(
            re.captures("x /ptlogin/captcha.do?captchaId=abc-123\" y")
                .unwrap()[1]
                .to_string(),
            "abc-123"
        );
    }
}
