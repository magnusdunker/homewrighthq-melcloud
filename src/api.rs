//! MELCloud's web API, the one the MELCloud app and app.melcloud.com use.
//!
//! Login with e-mail and password gives a context key, sent as
//! `X-MitsContextKey` on every later request. When MELCloud answers 401 the
//! client logs in again once and repeats the request.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Mutex;

pub const BASE_URL: &str = "https://app.melcloud.com/Mitsubishi.Wifi.Client";
/// The app version MELCloud's own web client sends at login.
const APP_VERSION: &str = "1.34.12.0";

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// Wrong e-mail or password, or the account is locked.
    Auth,
    /// HTTP 429. MELCloud blocks accounts that ask too often; the duration is
    /// `Retry-After` when MELCloud sends it.
    RateLimited(Option<Duration>),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Auth => write!(f, "MELCloud afviste e-mail eller adgangskode"),
            ApiError::RateLimited(_) => write!(f, "MELCloud beder om færre forespørgsler (HTTP 429)"),
            ApiError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        ApiError::Other(e.to_string())
    }
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    email: String,
    password: String,
    key: Mutex<Option<String>>,
}

impl Client {
    pub fn new(base: &str, email: &str, password: &str) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_default();
        Self { http, base: base.trim_end_matches('/').to_string(), email: email.into(), password: password.into(), key: Mutex::new(None) }
    }

    /// Log in and keep the context key.
    pub async fn login(&self) -> Result<String, ApiError> {
        let body = json!({
            "Email": self.email,
            "Password": self.password,
            "Language": 0,
            "AppVersion": APP_VERSION,
            "Persist": true,
            "CaptchaResponse": null
        });
        let res = self.http.post(format!("{}/Login/ClientLogin", self.base)).json(&body).send().await?;
        let v = read(res).await?;
        if !v["ErrorId"].is_null() {
            return Err(ApiError::Auth);
        }
        let key = v["LoginData"]["ContextKey"].as_str().filter(|k| !k.is_empty()).ok_or(ApiError::Auth)?.to_string();
        *self.key.lock().await = Some(key.clone());
        Ok(key)
    }

    async fn key(&self) -> Result<String, ApiError> {
        if let Some(k) = self.key.lock().await.clone() {
            return Ok(k);
        }
        self.login().await
    }

    /// GET with `body = None`, else POST. Logs in again once on 401.
    async fn send(&self, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        for attempt in 0..2 {
            let key = self.key().await?;
            let url = format!("{}{path}", self.base);
            let req = match body {
                Some(b) => self.http.post(url).json(b),
                None => self.http.get(url),
            };
            let res = req.header("X-MitsContextKey", key).header("Accept", "application/json").send().await?;
            if res.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                *self.key.lock().await = None;
                continue;
            }
            return read(res).await;
        }
        Err(ApiError::Auth)
    }

    /// Buildings with their floors, areas and devices.
    pub async fn list_devices(&self) -> Result<Value, ApiError> {
        self.send("/User/ListDevices", None).await
    }

    /// The current state of one device.
    pub async fn device(&self, device_id: i64, building_id: i64) -> Result<Value, ApiError> {
        self.send(&format!("/Device/Get?id={device_id}&buildingID={building_id}"), None).await
    }

    /// Send a changed state. `kind` is `Ata` or `Atw`. MELCloud answers with
    /// the state it will send to the unit.
    pub async fn set(&self, kind: &str, state: &Value) -> Result<Value, ApiError> {
        self.send(&format!("/Device/Set{kind}"), Some(state)).await
    }
}

async fn read(res: reqwest::Response) -> Result<Value, ApiError> {
    let status = res.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let after = res.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok()).map(Duration::from_secs);
        return Err(ApiError::RateLimited(after));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ApiError::Auth);
    }
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(ApiError::Other(format!("HTTP {status}: {}", text.chars().take(200).collect::<String>())));
    }
    let text = res.text().await?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| ApiError::Other(format!("ugyldigt svar: {e}")))
}
