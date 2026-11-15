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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
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
    /// For URLs other people chose (see [`untrusted_client`]).
    pub(crate) untrusted_http: reqwest::Client,
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

/// Redirects followed per request, at most.
const MAX_REDIRECTS: usize = 5;

/// Hosts LINE runs its services on (credentialed requests may be redirected among them).
const LINE_DOMAINS: [&str; 2] = ["line-apps.com", "line-scdn.net"];

/// Whether `url` may be fetched without credentials on a sender's say-so:
/// `https`, with a host *name* that is not `localhost`. IP literals are refused
/// outright (the client connects to them without asking the resolver); a name's
/// addresses are checked when it is resolved, by [`PinnedResolver`].
pub(crate) fn is_public_https(url: &url::Url) -> bool {
    url.scheme() == "https" && has_public_host_name(url)
}

/// Whether `url`'s host is a DNS name other than `localhost` (never an IP literal).
fn has_public_host_name(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            // `url` already turns numeric hosts (`2130706433`, `0x7f.1`) into IP
            // literals; a name that is still all digits and dots is not a real one.
            !d.is_empty()
                && d != "localhost"
                && !d.ends_with(".localhost")
                && !d.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        }
        _ => false,
    }
}

/// Whether `ip` is a globally reachable unicast address: not loopback,
/// private, shared (CGNAT), link-local (which holds the cloud metadata
/// service, `169.254.169.254`), documentation, benchmarking, multicast,
/// reserved or otherwise special-purpose (IANA special-purpose registries).
pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                    // "this network"
        || a == 10                              // private
        || (a == 100 && (b & 0xc0) == 64)       // shared address space (CGNAT)
        || a == 127                             // loopback
        || (a == 169 && b == 254)               // link-local, cloud metadata
        || (a == 172 && (b & 0xf0) == 16)       // private
        || (a == 192 && b == 0 && c == 0)       // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)       // documentation
        || (a == 192 && b == 88 && c == 99)     // 6to4 relay anycast
        || (a == 192 && b == 168)               // private
        || (a == 198 && (b & 0xfe) == 18)       // benchmarking
        || (a == 198 && b == 51 && c == 100)    // documentation
        || (a == 203 && b == 0 && c == 113)     // documentation
        || a >= 224) // multicast, reserved, broadcast
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let embedded_v4 = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    // IPv4-mapped (`::ffff:a.b.c.d`) and NAT64 (`64:ff9b::a.b.c.d`) addresses
    // reach the IPv4 address they carry.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(embedded_v4(s[6], s[7]));
    }
    // 6to4 (`2002:AABB:CCDD::/48`) tunnels to the IPv4 address in bits 16..48.
    if s[0] == 0x2002 {
        return is_public_v4(embedded_v4(s[1], s[2]));
    }
    // Only global unicast (2000::/3) is reachable from the internet; that rules
    // out ::, ::1, IPv4-compatible, discard, ULA (fc00::/7), link-local
    // (fe80::/10), site-local and multicast at once.
    (s[0] & 0xe000) == 0x2000
        && !((s[0] == 0x2001 && s[1] < 0x200)   // IETF protocol assignments, Teredo
            || (s[0] == 0x2001 && s[1] == 0xdb8) // documentation
            || (s[0] == 0x3fff && s[1] < 0x1000)) // documentation (3fff::/20)
}

/// A refused destination: a name that resolves to a non-public address.
#[derive(Debug)]
pub(crate) struct NonPublicDestination(pub(crate) String);

impl std::fmt::Display for NonPublicDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} resolves to a non-public address", self.0)
    }
}

impl std::error::Error for NonPublicDestination {}

/// Whether `err` (or anything it wraps) is a [`NonPublicDestination`].
pub(crate) fn is_non_public_destination(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(err);
    while let Some(e) = cur {
        if e.is::<NonPublicDestination>() {
            return true;
        }
        cur = e.source();
    }
    false
}

type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
/// Resolves a host name to all its addresses (A and AAAA).
pub(crate) type Lookup = dyn Fn(String) -> BoxFuture<std::io::Result<Vec<IpAddr>>> + Send + Sync;

/// The system resolver (`getaddrinfo` with no address family, so both A
/// and AAAA records), off the async threads.
fn system_lookup() -> Arc<Lookup> {
    Arc::new(|host: String| {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                Ok((host.as_str(), 0).to_socket_addrs()?.map(|a| a.ip()).collect())
            })
            .await
            .map_err(std::io::Error::other)?
        })
    })
}

/// What an untrusted fetch may reach: which URLs (checked on the first
/// request and on every redirect) and which resolved addresses.
#[derive(Clone, Copy)]
pub(crate) struct Guard {
    pub(crate) url_ok: fn(&url::Url) -> bool,
    pub(crate) ip_ok: fn(IpAddr) -> bool,
}

impl Guard {
    pub(crate) const PUBLIC: Guard = Guard {
        url_ok: is_public_https,
        ip_ok: is_public_ip,
    };
}

/// The resolver of the untrusted-fetch client. Every address a name resolves
/// to must pass the guard, or the whole name is refused (a mixed answer is
/// how rebinding attacks hedge). The addresses returned are the ones the
/// connection is made to, so nothing re-resolves the name between the check
/// and the connect (no rebinding window); TLS SNI, certificate checks and the
/// `Host` header still use the URL's host name.
pub(crate) struct PinnedResolver {
    lookup: Arc<Lookup>,
    ip_ok: fn(IpAddr) -> bool,
}

impl reqwest::dns::Resolve for PinnedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let lookup = (self.lookup)(host.clone());
        let ip_ok = self.ip_ok;
        Box::pin(async move {
            let ips = lookup.await?;
            if ips.is_empty() {
                return Err(format!("{host} has no addresses").into());
            }
            if !ips.iter().all(|ip| ip_ok(*ip)) {
                return Err(Box::new(NonPublicDestination(host)) as _);
            }
            let addrs: reqwest::dns::Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// The client for URLs chosen by other people (media links in messages,
/// public CDN pictures): no proxy (a proxy would resolve names itself,
/// past the guard), no `Referer`, addresses pinned by [`PinnedResolver`],
/// every redirect re-checked against the guard (and its target resolved
/// through the same resolver), bounded connect and read times.
pub(crate) fn untrusted_client(
    user_agent: &str,
    lookup: Arc<Lookup>,
    guard: Guard,
) -> Result<reqwest::Client> {
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if (guard.url_ok)(attempt.url()) {
            attempt.follow()
        } else {
            let next = attempt.url();
            let to = format!("{}://{}", next.scheme(), next.host_str().unwrap_or(""));
            attempt.error(format!("refusing a redirect to {to}"))
        }
    });
    Ok(reqwest::Client::builder()
        .user_agent(user_agent)
        .no_proxy()
        .referer(false)
        .dns_resolver(PinnedResolver {
            lookup,
            ip_ok: guard.ip_ok,
        })
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .redirect(redirects)
        .build()?)
}

fn is_line_host(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| {
            let h = h.trim_end_matches('.').to_ascii_lowercase();
            LINE_DOMAINS
                .iter()
                .any(|d| h == *d || h.ends_with(&format!(".{d}")))
        })
}

/// Which redirects to follow. Same-origin ones always (up to
/// [`MAX_REDIRECTS`]). Requests to the gateway or OBS carry the access token,
/// which reqwest does not strip (it only knows the standard auth headers),
/// so they follow cross-origin redirects only to LINE's own `https` hosts.
/// Uncredentialed requests to other hosts go through [`untrusted_client`]
/// instead; on this client they follow only public `https` redirects.
fn redirect_policy(credentialed: Vec<url::Origin>) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let next = attempt.url();
        let (Some(first), Some(last)) = (attempt.previous().first(), attempt.previous().last()) else {
            return attempt.follow();
        };
        if next.origin() == last.origin() {
            return attempt.follow();
        }
        let allowed = if credentialed.contains(&first.origin()) {
            is_line_host(next)
        } else {
            is_public_https(next)
        };
        if allowed {
            attempt.follow()
        } else {
            let to = format!("{}://{}", next.scheme(), next.host_str().unwrap_or(""));
            attempt.error(format!("refusing a redirect to {to}"))
        }
    })
}

impl Transport {
    pub(crate) fn new(config: Config, tokens: Tokens, ltsm: Ltsm) -> Result<Self> {
        let credentialed = [&config.gateway, &config.obs]
            .into_iter()
            .filter_map(|u| url::Url::parse(u).ok())
            .map(|u| u.origin())
            .collect();
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(Duration::from_secs(15))
            .redirect(redirect_policy(credentialed))
            .build()?;
        let untrusted_http = untrusted_client(&config.user_agent, system_lookup(), Guard::PUBLIC)?;
        let (tokens_tx, _) = watch::channel(tokens.clone());
        Ok(Transport {
            http,
            untrusted_http,
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
    fn only_https_urls_with_public_host_names_pass() {
        let public = |s: &str| is_public_https(&url::Url::parse(s).unwrap());
        assert!(public("https://example.com/a.jpg"));
        assert!(public("https://obs.line-apps.com:443/r/x"));
        assert!(public("https://xn--bcher-kva.example/x"));
        for bad in [
            "http://example.com/a.jpg",
            "ftp://example.com/a.jpg",
            "file:///etc/passwd",
            "https://localhost/x",
            "https://LOCALHOST./x",
            "https://api.localhost./x",
            // IP literals of any kind: the client would connect without resolving.
            "https://8.8.8.8/x",
            "https://[2001:4860::8888]/x",
            "https://127.0.0.1/x",
            "https://2130706433/x", // 127.0.0.1 as a number
            "https://0x7f.1/x",
            "https://017700000001/x",
            "https://10.1.2.3/x",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/x",
            "https://[fd00::1]/x",
            "https://[fe80::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://[::ffff:169.254.169.254]/x",
        ] {
            assert!(!public(bad), "{bad} must not count as public");
        }
    }

    #[test]
    fn only_globally_reachable_addresses_are_public() {
        let ip = |s: &str| is_public_ip(s.parse().unwrap());
        for good in [
            "8.8.8.8",
            "1.1.1.1",
            "147.92.128.1",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "2001:4860:4860::8888",
            "2404:6800:4004:80a::200e",
            "64:ff9b::808:808", // NAT64 of 8.8.8.8
            "2002:808:808::1",  // 6to4 of 8.8.8.8
            "::ffff:8.8.8.8",
        ] {
            assert!(ip(good), "{good} must count as public");
        }
        for bad in [
            // IPv4: this network, private, CGNAT, loopback, link-local and metadata,
            // special, documentation, benchmarking, multicast, reserved, broadcast.
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "169.254.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.170",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            // IPv6: unspecified, loopback, IPv4-compatible/mapped/NAT64/6to4 of
            // private addresses, discard, ULA, link-local, site-local, multicast,
            // Teredo, documentation, AWS's IPv6 metadata address.
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2002:a00:1::1",
            "2002:7f00:1::1",
            "100::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(!ip(bad), "{bad} must not count as public");
        }
    }

    /// A lookup that answers from a table, counting calls per name.
    fn fake_dns(table: &[(&str, &[&str])]) -> (Arc<Lookup>, Arc<std::sync::atomic::AtomicUsize>) {
        let table: std::collections::HashMap<String, Vec<IpAddr>> = table
            .iter()
            .map(|(h, ips)| (h.to_string(), ips.iter().map(|i| i.parse().unwrap()).collect()))
            .collect();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let lookup: Arc<Lookup> = Arc::new(move |host: String| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let ans = table.get(&host).cloned();
            Box::pin(async move { ans.ok_or_else(|| std::io::Error::other("NXDOMAIN")) })
        });
        (lookup, calls)
    }

    async fn resolve(lookup: Arc<Lookup>, host: &str) -> std::result::Result<Vec<IpAddr>, String> {
        use reqwest::dns::Resolve;
        let r = PinnedResolver {
            lookup,
            ip_ok: is_public_ip,
        };
        match r.resolve(host.parse().unwrap()).await {
            Ok(addrs) => Ok(addrs.map(|a| a.ip()).collect()),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    is_non_public_destination(&*e)
                        || msg.contains("NXDOMAIN")
                        || msg.contains("no addresses"),
                    "{msg}"
                );
                Err(e.to_string())
            }
        }
    }

    #[tokio::test]
    async fn names_resolving_to_any_non_public_address_are_refused() {
        let (dns, _) = fake_dns(&[
            (
                "cdn.example",
                &["93.184.215.14", "2606:2800:21f:cb07:6820:80da:af6b:8b2c"],
            ),
            ("loopback.example", &["127.0.0.1"]),
            ("private.example", &["10.0.0.7"]),
            ("private6.example", &["fd12:3456::1"]),
            ("linklocal.example", &["fe80::1"]),
            ("metadata.example", &["169.254.169.254"]),
            ("metadata6.example", &["fd00:ec2::254"]),
            ("mapped.example", &["::ffff:192.168.0.1"]),
            // A public A record next to a private AAAA (or the other way round).
            ("mixed.example", &["93.184.215.14", "::1"]),
            ("mixed6.example", &["2606:2800:21f:cb07::1", "192.168.1.1"]),
            ("empty.example", &[]),
        ]);
        assert_eq!(
            resolve(dns.clone(), "cdn.example").await.unwrap().len(),
            2,
            "both A and AAAA answers are kept"
        );
        for host in [
            "loopback.example",
            "private.example",
            "private6.example",
            "linklocal.example",
            "metadata.example",
            "metadata6.example",
            "mapped.example",
            "mixed.example",
            "mixed6.example",
        ] {
            let err = resolve(dns.clone(), host).await.unwrap_err();
            assert!(err.contains("non-public"), "{host}: {err}");
        }
        assert!(resolve(dns.clone(), "empty.example").await.is_err());
        assert!(resolve(dns, "nxdomain.example").await.is_err());
    }

    /// For the HTTP-level tests: the mock server listens on 127.0.0.1 over
    /// plain http, so this guard stands in for "public" with exactly that
    /// address and the `http` scheme; everything else about the client (the
    /// pinned resolver, redirect re-checks, IP-literal refusal) is the real one.
    const TEST_GUARD: Guard = Guard {
        url_ok: |u| u.scheme() == "http" && has_public_host_name(u),
        ip_ok: |ip| ip == IpAddr::V4(Ipv4Addr::LOCALHOST),
    };

    fn named(server: &wiremock::MockServer, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", server.address().port())
    }

    #[tokio::test]
    async fn connections_go_to_the_validated_address_with_the_original_host() {
        use wiremock::matchers::{header, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let host = format!("media.example:{}", server.address().port());
        Mock::given(path("/a.jpg"))
            .and(header("host", host.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"JPEG".to_vec()))
            .mount(&server)
            .await;
        // Rebinding: the first answer passes, every later one points inside.
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (f, c) = (first.clone(), calls.clone());
        let lookup: Arc<Lookup> = Arc::new(move |_host: String| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let ip: IpAddr = if f.swap(false, std::sync::atomic::Ordering::SeqCst) {
                Ipv4Addr::LOCALHOST.into()
            } else {
                Ipv4Addr::new(10, 0, 0, 1).into()
            };
            Box::pin(async move { Ok(vec![ip]) })
        });
        let client = untrusted_client("t", lookup.clone(), TEST_GUARD).unwrap();
        let resp = client
            .get(named(&server, "media.example", "/a.jpg"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap().as_ref(), b"JPEG");
        // Resolved once, and the connection used that (checked) answer.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A new connection resolves again, and the rebound answer is refused.
        let fresh = untrusted_client("t", lookup, TEST_GUARD).unwrap();
        let err = fresh
            .get(named(&server, "media.example", "/a.jpg"))
            .send()
            .await
            .unwrap_err();
        assert!(is_non_public_destination(&err), "{err:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn redirects_are_checked_like_the_first_request() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let port = server.address().port();
        let redirect = |to: String| ResponseTemplate::new(302).insert_header("location", to);
        let (dns, _) = fake_dns(&[
            ("media.example", &["127.0.0.1"]),
            ("cdn.example", &["127.0.0.1"]),
            ("metadata.example", &["169.254.169.254"]),
            ("private.example", &["127.0.0.1", "10.0.0.1"]),
        ]);
        let routes = [
            ("/ok", named(&server, "cdn.example", "/img")),
            (
                "/to-metadata",
                named(&server, "metadata.example", "/latest/meta-data"),
            ),
            ("/to-mixed", named(&server, "private.example", "/secret")),
            ("/to-literal", format!("http://127.0.0.1:{port}/secret")),
            ("/to-literal6", format!("http://[::1]:{port}/secret")),
            ("/to-localhost", named(&server, "localhost", "/secret")),
            (
                "/to-https-literal",
                "https://169.254.169.254/latest/meta-data".to_owned(),
            ),
        ];
        for (from, to) in &routes {
            Mock::given(path(*from))
                .respond_with(redirect(to.clone()))
                .mount(&server)
                .await;
        }
        Mock::given(path("/loop"))
            .respond_with(redirect(named(&server, "media.example", "/loop")))
            .mount(&server)
            .await;
        Mock::given(path("/img"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"IMG".to_vec()))
            .mount(&server)
            .await;
        Mock::given(path("/secret"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"internal".to_vec()))
            .expect(0)
            .mount(&server)
            .await;
        let client = untrusted_client("t", dns, TEST_GUARD).unwrap();
        let get = |p: &str| client.get(named(&server, "media.example", p)).send();
        let ok = get("/ok").await.unwrap();
        assert_eq!(ok.bytes().await.unwrap().as_ref(), b"IMG");
        for (from, _) in &routes[1..] {
            let err = get(from).await.unwrap_err();
            assert!(
                err.is_redirect() || is_non_public_destination(&err),
                "{from}: {err:?}"
            );
        }
        assert!(get("/loop").await.unwrap_err().is_redirect());
        // A non-http(s) target is not followed: the 3xx itself comes back, which
        // `public_get` treats as a failure (only 2xx bodies are read).
        Mock::given(path("/to-file"))
            .respond_with(redirect("file:///etc/passwd".to_owned()))
            .mount(&server)
            .await;
        assert!(get("/to-file").await.unwrap().status().is_redirection());
        // `expect(0)` on /secret is verified when the server drops.
    }

    #[test]
    fn line_hosts_are_recognised_by_domain_suffix() {
        let line = |s: &str| is_line_host(&url::Url::parse(s).unwrap());
        assert!(line("https://obs.line-apps.com/r/x"));
        assert!(line("https://profile.line-scdn.net/p"));
        assert!(!line("http://obs.line-apps.com/r/x"));
        assert!(!line("https://evil-line-apps.com/"));
        assert!(!line("https://line-apps.com.evil.example/"));
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
