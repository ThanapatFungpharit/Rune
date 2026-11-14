//! Secondary-device QR login, as performed by the Chrome extension.
//!
//! ```text
//! createSession                -> authSessionId
//! createQrCode                 -> callbackUrl (+ long-poll interval/count)
//! <render callbackUrl?secret=<b64 Curve25519 pubkey>&e2eeVersion=1 as a QR>
//! checkQrCodeVerified          long-poll until the phone scans (retry on 410)
//! verifyCertificate(cert)      ok -> skip PIN; error -> PIN flow:
//!   createPinCode -> show PIN; checkPinCodeVerified long-poll
//! qrCodeLoginV2                -> tokenV3IssueResult, mid, certificate,
//!                                 metaData {publicKey, encryptedKeyChain}
//! ```
//!
//! The QR `secret` is a Curve25519 key generated inside `ltsm.wasm`; with it
//! the phone wraps our Letter Sealing keychain (`encryptedKeyChain`), which is
//! unwrapped here so the keys can be persisted. The keychain is only ever
//! delivered at login time.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::value_i64;
use crate::transport::CallOpts;

const QR_SERVICE: &str = "talk/thrift/LoginQrCode/SecondaryQrCodeLoginService";
const QR_NOTICE: &str = "talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService";
/// Fixed `X-LST` of the PIN long-poll (the bundle's `rH = 11e4`).
const PIN_POLL_LST_MS: u64 = 110_000;

/// Progress of a QR login, delivered to the caller's callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrLoginEvent {
    /// Render this URL as a QR code and scan it with the LINE app.
    QrUrl(String),
    /// Enter this PIN on the phone (first login from this "device").
    Pin(String),
}

/// One Letter Sealing private key, exported from `ltsm.wasm`. Secret.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E2eeKey {
    pub key_id: i32,
    /// `E2EEKey.exportKey()` blob, base64.
    pub exported: String,
}

impl std::fmt::Debug for E2eeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eeKey")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// Outcome of a successful login.
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub mid: Option<String>,
    /// Our Letter Sealing keys (empty if the phone did not send a keychain
    /// or unwrapping failed — see `e2ee_error`).
    pub e2ee_keys: Vec<E2eeKey>,
    pub e2ee_error: Option<String>,
    /// The raw `qrCodeLoginV2` response minus tokens.
    pub raw: Value,
}

/// Options for [`Client::qr_login`].
#[derive(Debug, Clone)]
pub struct QrLoginOptions {
    /// Reported device name (`systemName`).
    pub system_name: String,
    /// Overall time to wait for the scan / PIN confirmation.
    pub wait: Duration,
}

impl Default for QrLoginOptions {
    fn default() -> Self {
        QrLoginOptions {
            system_name: "CHROMEOS".into(),
            wait: Duration::from_secs(180),
        }
    }
}

/// `URL.searchParams.set(...)` on the callback URL.
pub(crate) fn append_qr_secret(callback_url: &str, public_key: &[u8]) -> Result<String> {
    let mut url = url::Url::parse(callback_url)
        .map_err(|e| Error::Protocol(format!("bad callbackUrl {callback_url:?}: {e}")))?;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "secret" && k != "e2eeVersion")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair("secret", &B64.encode(public_key))
        .append_pair("e2eeVersion", "1");
    Ok(url.into())
}

impl Client {
    async fn qr_call(
        &self,
        method: &str,
        args: Value,
        headers: Vec<(&'static str, String)>,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let opts = CallOpts {
            require_auth: false,
            allow_refresh: false,
            extra_headers: headers,
            timeout,
        };
        self.call_opts(method, args, opts).await
    }

    /// Long-poll `method` until it succeeds, retrying while the server reports
    /// that the poll window elapsed (HTTP status in `retry_statuses`).
    async fn long_poll(
        &self,
        method: &str,
        session: &str,
        lst_ms: u64,
        attempts: u32,
        retry_statuses: &[u16],
    ) -> Result<Value> {
        for _ in 0..attempts.max(1) {
            let headers = vec![
                ("x-line-session-id", session.to_owned()),
                ("x-lst", lst_ms.to_string()),
            ];
            let timeout = Some(Duration::from_millis(lst_ms) + Duration::from_secs(15));
            match self
                .qr_call(method, json!([{ "authSessionId": session }]), headers, timeout)
                .await
            {
                Ok(v) => return Ok(v),
                Err(Error::Api(e)) if retry_statuses.contains(&e.status) => continue,
                Err(Error::Http(e)) if e.is_timeout() => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Timeout(format!("{method} not confirmed")))
    }

    /// Log in by QR code. `on_event` receives the URL to show as a QR code
    /// and, on a first login, the PIN to enter on the phone.
    ///
    /// On success the client holds fresh tokens (see [`Client::tokens`]);
    /// persist them together with the returned E2EE keys via
    /// [`Session`](crate::Session).
    pub async fn qr_login(
        &self,
        opts: QrLoginOptions,
        mut on_event: impl FnMut(QrLoginEvent),
    ) -> Result<LoginResult> {
        let created = self
            .qr_call(&format!("{QR_SERVICE}/createSession"), json!([{}]), vec![], None)
            .await?;
        let session = created["authSessionId"]
            .as_str()
            .ok_or_else(|| Error::Protocol(format!("createSession: {created}")))?
            .to_owned();
        let qr = self
            .qr_call(
                &format!("{QR_SERVICE}/createQrCode"),
                json!([{ "authSessionId": session }]),
                vec![],
                None,
            )
            .await?;
        let callback = qr["callbackUrl"]
            .as_str()
            .ok_or_else(|| Error::Protocol(format!("createQrCode: {qr}")))?;
        let interval = qr
            .get("longPollingIntervalSec")
            .and_then(value_i64)
            .filter(|&n| n > 0)
            .unwrap_or(10) as u64;
        let max_count = qr
            .get("longPollingMaxCount")
            .and_then(value_i64)
            .filter(|&n| n > 0)
            .unwrap_or(12) as u32;
        let attempts = max_count.max((opts.wait.as_secs() / interval) as u32 + 1);

        // The QR secret: a Curve25519 key generated inside the secure module.
        let (curve_key, public_key) = self.with_ltsm(|l| -> Result<_> {
            let k = l.generate_curve_key()?;
            Ok((k, l.public_key(k)?))
        })?;
        on_event(QrLoginEvent::QrUrl(append_qr_secret(callback, &public_key)?));

        // The scan poll retries on 410 only, like the extension.
        self.long_poll(
            &format!("{QR_NOTICE}/checkQrCodeVerified"),
            &session,
            interval * 1000,
            attempts,
            &[410],
        )
        .await?;

        let cert = self.tokens().certificate.unwrap_or_default();
        let verified = self
            .qr_call(
                &format!("{QR_SERVICE}/verifyCertificate"),
                json!([{ "authSessionId": session, "certificate": cert }]),
                vec![],
                None,
            )
            .await;
        if let Err(e) = verified {
            if !matches!(e, Error::Api(_)) {
                return Err(e);
            }
            // Unknown device: PIN confirmation.
            let pin = self
                .qr_call(
                    &format!("{QR_SERVICE}/createPinCode"),
                    json!([{ "authSessionId": session }]),
                    vec![],
                    None,
                )
                .await?;
            if let Some(p) = pin.get("pinCode").and_then(Value::as_str) {
                on_event(QrLoginEvent::Pin(p.to_owned()));
            }
            self.long_poll(
                &format!("{QR_NOTICE}/checkPinCodeVerified"),
                &session,
                PIN_POLL_LST_MS,
                attempts,
                &[408, 410],
            )
            .await?;
        }

        let mut raw = self
            .qr_call(
                &format!("{QR_SERVICE}/qrCodeLoginV2"),
                json!([{ "systemName": opts.system_name, "modelName": "CHROME",
                         "autoLoginIsRequired": false, "authSessionId": session }]),
                vec![],
                None,
            )
            .await?;
        let issue = raw
            .get("tokenV3IssueResult")
            .cloned()
            .ok_or_else(|| Error::Protocol("qrCodeLoginV2 without tokenV3IssueResult".into()))?;
        let mid = raw.get("mid").and_then(Value::as_str).map(str::to_owned);
        let certificate = raw.get("certificate").and_then(Value::as_str).map(str::to_owned);
        self.t.update_tokens(|t| {
            t.adopt_issue_result(&issue)?;
            if mid.is_some() {
                t.mid = mid.clone();
            }
            if certificate.is_some() {
                t.certificate = certificate.clone();
            }
            Ok(())
        })?;
        // Keep secrets out of `raw`.
        if let Some(o) = raw.as_object_mut() {
            o.remove("tokenV3IssueResult");
            o.remove("certificate");
        }

        let (e2ee_keys, e2ee_error) = match raw.get("metaData") {
            Some(meta) if meta.get("encryptedKeyChain").is_some() => {
                match self.unwrap_login_keychain(curve_key, meta) {
                    Ok(keys) => (keys, None),
                    Err(e) => {
                        tracing::warn!("E2EE keychain unwrap failed: {e}");
                        (Vec::new(), Some(e.to_string()))
                    }
                }
            }
            _ => (Vec::new(), Some("no E2EE keychain in login response".into())),
        };
        if let Some(o) = raw.as_object_mut() {
            o.remove("metaData");
        }
        self.with_ltsm(|l| l.release_key(curve_key))?;
        if !e2ee_keys.is_empty() {
            self.load_e2ee_keys(&e2ee_keys)?;
        }
        Ok(LoginResult {
            mid,
            e2ee_keys,
            e2ee_error,
            raw,
        })
    }

    fn unwrap_login_keychain(&self, curve_key: crate::ltsm::KeyHandle, meta: &Value) -> Result<Vec<E2eeKey>> {
        let field = |k: &str| -> Result<Vec<u8>> {
            let s = meta
                .get(k)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Protocol(format!("metaData.{k} missing")))?;
            B64.decode(s)
                .map_err(|e| Error::Protocol(format!("metaData.{k}: {e}")))
        };
        let (server_pub, chain) = (field("publicKey")?, field("encryptedKeyChain")?);
        self.with_ltsm(|l| -> Result<Vec<E2eeKey>> {
            let ch = l.create_channel(curve_key, &server_pub)?;
            let handles = l.unwrap_keychain(ch, &chain)?;
            l.release_channel(ch)?;
            let mut out = Vec::new();
            for h in handles {
                out.push(E2eeKey {
                    key_id: l.key_id(h)?,
                    exported: B64.encode(l.export_e2ee_key(h)?),
                });
                l.release_key(h)?;
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_secret_is_form_encoded() {
        let u = append_qr_secret("https://line.me/R/au/lgn/sq/abc?foo=1", &[0xfb, 0xff, 0x00]).unwrap();
        assert_eq!(
            u,
            "https://line.me/R/au/lgn/sq/abc?foo=1&secret=%2B%2F8A&e2eeVersion=1"
        );
    }
}
