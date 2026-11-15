//! HTTP transport for the LINE Chrome gateway.
//!
//! Wire format (from the extension bundle, confirmed by the reference client):
//!
//! ```text
//! POST /api/talk/thrift/<Namespace>/<Service>/<method>
//! content-type: application/json
//! X-Line-Access: <access token>          (when logged in)
//! X-Line-Chrome-Version: 3.7.2
//! X-LAL: en_US
//! X-Hmac: <ltsm signature over path + body>
//!
//! [<positional thrift args>]             -> {"message":"OK","data":<result>}
//! ```
//!
//! The signature covers the exact bytes sent, so the body is serialized once
//! and those bytes are both signed and transmitted.

use std::sync::{Mutex, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::error::{ApiError, Error, Result, gateway_code, parse_error};
use crate::ltsm::Ltsm;

/// The Chrome gateway that fronts every Thrift service.
pub const GATEWAY: &str = "https://line-chrome-gw.line-apps.com";
/// Extension version this client reproduces.
pub const APP_VERSION: &str = "3.7.2";
/// `X-Line-Application`, sent only on private OBS requests (never on gateway calls).
pub const APPLICATION_HEADER: &str = "CHROMEOS\t3.7.2\tChrome_OS\t";
/// The extension never sets a User-Agent (the browser does); this is a
/// plausible CrOS Chrome string, same default as the reference client.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Connection settings.
#[derive(Debug, Clone)]
pub struct Config {
    pub gateway: String,
    /// Object storage host for media (`https://obs.line-apps.com`).
    pub obs: String,
    /// `Accept-Language` form, e.g. `en-US`, `th-TH`, `ja-JP`.
    pub locale: String,
    pub user_agent: String,
    /// Per-request timeout for ordinary calls.
    pub timeout: Duration,
    /// Extra attempts on 5xx, network errors and retryable API errors.
    pub max_retries: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            gateway: GATEWAY.to_owned(),
            obs: crate::media::OBS_HOST.to_owned(),
            locale: "en-US".to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(30),
            max_retries: 2,
        }
    }
}

impl Config {
    /// `X-LAL` / SSE `language` form of the locale (`en-US` -> `en_US`).
    pub fn lal(&self) -> String {
        self.locale.replace('-', "_")
    }
}

/// Server retry policy for `tokenRefresh` code 10202.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRetryPolicy {
    #[serde(deserialize_with = "crate::models::de_f64")]
    pub initial_delay_in_millis: f64,
    #[serde(deserialize_with = "crate::models::de_f64")]
    pub max_delay_in_millis: f64,
    #[serde(deserialize_with = "crate::models::de_f64")]
    pub multiplier: f64,
    #[serde(deserialize_with = "crate::models::de_f64")]
    pub jitter_rate: f64,
}

/// Credentials of a logged-in session. Treat as secret.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Our own user mid.
    pub mid: Option<String>,
    /// Device certificate; lets later QR logins skip the PIN step.
    pub certificate: Option<String>,
    pub token_issue_time_epoch_sec: Option<i64>,
    pub duration_until_refresh_in_sec: Option<i64>,
    pub refresh_api_retry_policy: Option<RefreshRetryPolicy>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |o: &Option<String>| o.as_ref().map(|_| "<redacted>");
        f.debug_struct("Tokens")
            .field("access_token", &redact(&self.access_token))
            .field("refresh_token", &redact(&self.refresh_token))
            .field("mid", &self.mid)
            .field("certificate", &redact(&self.certificate))
            .field("token_issue_time_epoch_sec", &self.token_issue_time_epoch_sec)
            .field(
                "duration_until_refresh_in_sec",
                &self.duration_until_refresh_in_sec,
            )
            .finish()
    }
}

impl Tokens {
    /// When the server wants the token renewed (unix seconds), if known.
    pub fn refresh_due_epoch_sec(&self) -> Option<i64> {
        Some(self.token_issue_time_epoch_sec? + self.duration_until_refresh_in_sec?)
    }

    /// Adopt a `tokenV3IssueResult` (login or refresh).
    pub(crate) fn adopt_issue_result(&mut self, r: &Value) -> Result<()> {
        let access = r
            .get("accessToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Protocol("tokenV3IssueResult without accessToken".into()))?;
        self.access_token = Some(access.to_owned());
        if let Some(rt) = r
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.refresh_token = Some(rt.to_owned());
        }
        let num = |k: &str| crate::models::value_i64(r.get(k)?);
        self.token_issue_time_epoch_sec = num("tokenIssueTimeEpochSec");
        self.duration_until_refresh_in_sec = num("durationUntilRefreshInSec");
        self.refresh_api_retry_policy = r
            .get("refreshApiRetryPolicy")
            .and_then(|p| serde_json::from_value(p.clone()).ok());
        Ok(())
    }
}

/// Per-request options.
#[derive(Debug, Clone)]
pub(crate) struct CallOpts {
    pub require_auth: bool,
    pub allow_refresh: bool,
    pub extra_headers: Vec<(&'static str, String)>,
    pub timeout: Option<Duration>,
}

impl Default for CallOpts {
    fn default() -> Self {
        CallOpts {
            require_auth: true,
            allow_refresh: true,
            extra_headers: Vec::new(),
            timeout: None,
        }
    }
}

pub(crate) struct Transport {
    pub(crate) http: reqwest::Client,
    pub(crate) config: Config,
    tokens: RwLock<Tokens>,
    pub(crate) tokens_tx: watch::Sender<Tokens>,
    /// The secure module. Signing takes well under a millisecond once the
    /// per-token key is derived, so it is called inline.
    pub(crate) ltsm: Mutex<Ltsm>,
    refresh_lock: tokio::sync::Mutex<()>,
}

const OK: &str = "OK";
pub(crate) const TOKEN_REFRESH_PATH: &str = "/api/auth/tokenRefresh";
/// Most `tokenRefresh` retries on code 10202, whatever the server's policy says.
const MAX_REFRESH_RETRIES: u32 = 8;

impl Transport {
    pub(crate) fn new(config: Config, tokens: Tokens, ltsm: Ltsm) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let (tokens_tx, _) = watch::channel(tokens.clone());
        Ok(Transport {
            http,
            config,
            tokens: RwLock::new(tokens),
            tokens_tx,
            ltsm: Mutex::new(ltsm),
            refresh_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn tokens(&self) -> Tokens {
        self.tokens.read().unwrap().clone()
    }

    pub(crate) fn access_token(&self) -> Option<String> {
        self.tokens.read().unwrap().access_token.clone()
    }

    pub(crate) fn update_tokens(&self, f: impl FnOnce(&mut Tokens) -> Result<()>) -> Result<()> {
        let snapshot = {
            let mut t = self.tokens.write().unwrap();
            f(&mut t)?;
            t.clone()
        };
        self.tokens_tx.send_replace(snapshot);
        Ok(())
    }

    /// `X-Hmac` for `path` (with query) + `body`, keyed by the current token.
    pub(crate) fn sign(&self, path: &str, body: &str) -> Result<String> {
        let token = self.access_token().unwrap_or_default();
        Ok(self.ltsm.lock().unwrap().hmac(&token, path, body)?)
    }

    fn gateway_headers(&self, with_access: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        let mut put = |k: &'static str, v: &str| {
            if let Ok(v) = HeaderValue::from_str(v) {
                h.insert(HeaderName::from_static(k), v);
            }
        };
        put("accept", "application/json, text/plain, */*");
        put("x-line-chrome-version", APP_VERSION);
        put("accept-language", &self.config.locale);
        put("x-lal", &self.config.lal());
        if with_access && let Some(t) = self.access_token() {
            let mut v = HeaderValue::from_str(&t).unwrap_or(HeaderValue::from_static(""));
            v.set_sensitive(true);
            h.insert("x-line-access", v);
        }
        h
    }

    /// Invoke a Thrift method, e.g. `call("talk/thrift/Talk/TalkService/getProfile", json!([2]))`.
    pub(crate) async fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.call_with(method, args, CallOpts::default()).await
    }

    pub(crate) async fn call_with(&self, method: &str, args: Value, opts: CallOpts) -> Result<Value> {
        let path = format!("/api/{}", method.trim_start_matches('/'));
        self.post_json(&path, &args, opts).await
    }

    /// POST a JSON body to a gateway path and unwrap the `{message, data}` envelope.
    pub(crate) async fn post_json(&self, path: &str, body: &Value, opts: CallOpts) -> Result<Value> {
        if opts.require_auth && self.access_token().is_none() {
            return Err(Error::LoginRequired);
        }
        let body = serde_json::to_string(body)?;
        let url = format!("{}{}", self.config.gateway, path);
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            // Headers (and X-Hmac) are rebuilt per attempt: a refresh changes the token.
            let mut headers = self.gateway_headers(opts.require_auth || path == TOKEN_REFRESH_PATH);
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            headers.insert(
                "x-hmac",
                HeaderValue::from_str(&self.sign(path, &body)?).expect("base64"),
            );
            for (k, v) in &opts.extra_headers {
                if let Ok(v) = HeaderValue::from_str(v) {
                    headers.insert(*k, v);
                }
            }
            tracing::debug!(%path, attempt, "POST");
            let sent = self
                .http
                .post(&url)
                .headers(headers)
                .body(body.clone())
                .timeout(opts.timeout.unwrap_or(self.config.timeout))
                .send()
                .await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) if attempt < self.config.max_retries && !e.is_builder() => {
                    tracing::warn!(%path, "request failed, retrying: {e}");
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status().as_u16();
            if status >= 500 && attempt < self.config.max_retries {
                attempt += 1;
                continue;
            }
            let text = resp.text().await?;
            match decode(path, status, &text) {
                Ok(v) => return Ok(v),
                Err(api) => {
                    let can_refresh = opts.allow_refresh
                        && !refreshed
                        && path != TOKEN_REFRESH_PATH
                        && self.tokens.read().unwrap().refresh_token.is_some();
                    if (api.must_refresh_token() || api.status == 401) && can_refresh {
                        refreshed = true;
                        Box::pin(self.refresh_token()).await?;
                        continue;
                    }
                    if api.is_retryable() && attempt < self.config.max_retries {
                        attempt += 1;
                        continue;
                    }
                    if api.must_refresh_token() {
                        return Err(Error::Reauth(api.to_string()));
                    }
                    return Err(api.into());
                }
            }
        }
    }

    /// Signed GET (the query string is part of the signed path).
    pub(crate) async fn get(
        &self,
        path_and_query: &str,
        headers: HeaderMap,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response> {
        let mut headers = headers;
        headers.insert(
            "x-hmac",
            HeaderValue::from_str(&self.sign(path_and_query, "")?).expect("base64"),
        );
        let mut req = self
            .http
            .get(format!("{}{}", self.config.gateway, path_and_query))
            .headers(headers);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        Ok(req.send().await?)
    }

    /// One signed gateway request without the `{message, data}` unwrapping:
    /// `GET` without a body, `POST` with a JSON body. The caller adds its own
    /// headers (e.g. `X-Line-ChannelToken`); no retries or token refresh.
    pub(crate) async fn gateway_request(
        &self,
        path_and_query: &str,
        body: Option<&Value>,
        extra: HeaderMap,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response> {
        let body = body.map(serde_json::to_string).transpose()?;
        let mut headers = self.gateway_headers(true);
        headers.extend(extra);
        headers.insert(
            "x-hmac",
            HeaderValue::from_str(&self.sign(path_and_query, body.as_deref().unwrap_or(""))?)
                .expect("base64"),
        );
        let url = format!("{}{}", self.config.gateway, path_and_query);
        let req = match body {
            Some(b) => {
                headers.insert("content-type", HeaderValue::from_static("application/json"));
                self.http.post(url).body(b)
            }
            None => self.http.get(url),
        };
        tracing::debug!(path = %path_and_query, "gateway request");
        Ok(req
            .headers(headers)
            .timeout(timeout.unwrap_or(self.config.timeout))
            .send()
            .await?)
    }

    /// Renew the access token via `/api/auth/tokenRefresh`.
    ///
    /// Concurrent callers share one refresh. Honours the server's
    /// `refreshApiRetryPolicy` for code 10202 and maps 10201 to
    /// [`Error::Reauth`].
    pub(crate) async fn refresh_token(&self) -> Result<()> {
        let before = self.access_token();
        let _guard = self.refresh_lock.lock().await;
        if self.access_token() != before {
            return Ok(()); // someone else refreshed while we waited
        }
        let (rt, policy) = {
            let t = self.tokens.read().unwrap();
            let rt = t
                .refresh_token
                .clone()
                .ok_or_else(|| Error::Reauth("no refresh token".into()))?;
            (rt, t.refresh_api_retry_policy.clone())
        };
        let mut delay_ms = policy.as_ref().map_or(0.0, |p| p.initial_delay_in_millis);
        let mut retries = 0;
        loop {
            let opts = CallOpts {
                require_auth: false,
                allow_refresh: false,
                ..Default::default()
            };
            match self
                .post_json(TOKEN_REFRESH_PATH, &json!({ "refreshToken": rt }), opts)
                .await
            {
                Ok(data) => {
                    let issue = data.get("tokenV3IssueResult").unwrap_or(&data).clone();
                    self.update_tokens(|t| t.adopt_issue_result(&issue))?;
                    tracing::info!("access token refreshed");
                    return Ok(());
                }
                Err(Error::Api(e)) if e.gateway_code == Some(gateway_code::AUTH_INVALID_REQUEST) => {
                    return Err(Error::Reauth(format!("refresh token rejected: {e}")));
                }
                Err(Error::Api(e)) if e.gateway_code == Some(gateway_code::AUTH_RETRY_REQUIRED) => {
                    let Some(p) = &policy else {
                        return Err(Error::Api(e));
                    };
                    // The attempt cap also ends policies whose delay never
                    // grows (initial delay 0, multiplier 1, NaN).
                    retries += 1;
                    if delay_ms >= p.max_delay_in_millis || retries > MAX_REFRESH_RETRIES {
                        return Err(Error::Reauth("token refresh retry budget exhausted".into()));
                    }
                    let jitter = 1.0 + p.jitter_rate * (2.0 * rand_unit() - 1.0);
                    // Clamp server-provided delays to 60 s per sleep.
                    let sleep = (delay_ms * jitter).clamp(0.0, 60_000.0);
                    tokio::time::sleep(Duration::from_millis(sleep as u64)).await;
                    delay_ms *= p.multiplier.max(1.0);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn rand_unit() -> f64 {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

/// Decode a gateway response: `{"message":"OK","data":...}` -> data; anything
/// else (including a 2xx without the OK envelope) is an error.
pub(crate) fn decode(path: &str, status: u16, text: &str) -> std::result::Result<Value, ApiError> {
    let payload: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
    };
    if (200..300).contains(&status)
        && let Some(obj) = payload.as_object()
        && obj.get("message").and_then(Value::as_str) == Some(OK)
    {
        return Ok(obj.get("data").cloned().unwrap_or(payload));
    }
    let mut e = parse_error(path, status, payload);
    if e.reason.is_none() && (200..300).contains(&status) {
        e.reason = Some("response without OK envelope".into());
    }
    Err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_envelope_is_unwrapped() {
        let v = decode("/p", 200, r#"{"message":"OK","data":{"mid":"u1"}}"#).unwrap();
        assert_eq!(v, json!({"mid":"u1"}));
        // OK without data returns the envelope itself (e.g. void methods).
        assert_eq!(
            decode("/p", 200, r#"{"message":"OK"}"#).unwrap(),
            json!({"message":"OK"})
        );
    }

    #[test]
    fn non_ok_2xx_is_error() {
        let e = decode("/p", 200, r#"{"message":"FAILED","code":10051}"#).unwrap_err();
        assert_eq!(e.gateway_code, Some(10051));
        let e = decode("/p", 200, "not json").unwrap_err();
        assert_eq!(e.raw, json!("not json"));
    }

    #[test]
    fn tokens_debug_is_redacted() {
        let t = Tokens {
            access_token: Some("secret-token".into()),
            ..Default::default()
        };
        let s = format!("{t:?}");
        assert!(!s.contains("secret-token"), "{s}");
    }

    #[test]
    fn adopt_issue_result_accepts_string_numbers() {
        let mut t = Tokens::default();
        t.adopt_issue_result(&json!({
            "accessToken": "a", "refreshToken": "r",
            "tokenIssueTimeEpochSec": "1700000000", "durationUntilRefreshInSec": 3600,
            "refreshApiRetryPolicy": {"initialDelayInMillis": "1000", "maxDelayInMillis": 60000, "multiplier": 2, "jitterRate": 0.1}
        }))
        .unwrap();
        assert_eq!(t.refresh_due_epoch_sec(), Some(1_700_003_600));
        assert_eq!(
            t.refresh_api_retry_policy.unwrap().initial_delay_in_millis,
            1000.0
        );
    }
}
