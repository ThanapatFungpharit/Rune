//! Error types.

use std::fmt;

use serde_json::Value;

use crate::ltsm::LtsmError;

/// Gateway envelope codes (the extension's `qU` enum).
pub mod gateway_code {
    pub const REQUEST_NEED_LOGIN: i64 = 10004;
    pub const REQUEST_INVALID_HMAC: i64 = 10005;
    pub const REQUEST_MUST_UPGRADE: i64 = 10006;
    pub const RESPONSE_ERROR: i64 = 10051;
    pub const RESPONSE_HTTP_ERROR: i64 = 10052;
    /// `/api/auth/tokenRefresh`: refresh token rejected — log in again.
    pub const AUTH_INVALID_REQUEST: i64 = 10201;
    /// `/api/auth/tokenRefresh`: retry later with backoff.
    pub const AUTH_RETRY_REQUIRED: i64 = 10202;
    pub const UNKNOWN_ERROR: i64 = 99999;
}

/// Selected `TalkException` codes (the extension's `mU` enum).
pub mod talk_code {
    pub const AUTHENTICATION_FAILED: i64 = 1;
    pub const NOT_AVAILABLE_USER: i64 = 7;
    pub const NOT_AUTHORIZED_DEVICE: i64 = 8;
    /// The chat requires Letter Sealing; plain sends are refused.
    pub const E2EE_RETRY_ENCRYPT: i64 = 82;
    pub const SHOULD_RETRY: i64 = 115;
    pub const MUST_REFRESH_V3_TOKEN: i64 = 119;
}

/// An application-level error returned by the LINE gateway.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// Request path (no host).
    pub path: String,
    /// HTTP status (or the nested `statusCode` of a 10052 envelope).
    pub status: u16,
    /// Outer gateway envelope code (`10051`, `10052`, `99999`, ...).
    pub gateway_code: Option<i64>,
    /// Inner `TalkException` code when present.
    pub talk_code: Option<i64>,
    /// Human-readable reason from the server.
    pub reason: Option<String>,
    /// Full decoded error body.
    pub raw: Value,
}

impl ApiError {
    /// The most specific code: the inner `TalkException` code, else the
    /// gateway envelope code.
    pub fn code(&self) -> Option<i64> {
        self.talk_code.or(self.gateway_code)
    }

    /// True when the access token is no longer usable (kicked out, logged
    /// out elsewhere, ...). Only meaningful for `/api/talk/thrift/Talk*`
    /// paths, mirroring the extension's interceptor scope.
    pub fn is_auth(&self) -> bool {
        matches!(self.status, 401 | 403)
            || (talk_auth_scoped(&self.path)
                && matches!(
                    self.talk_code,
                    Some(
                        talk_code::AUTHENTICATION_FAILED
                            | talk_code::NOT_AVAILABLE_USER
                            | talk_code::NOT_AUTHORIZED_DEVICE
                    )
                ))
    }

    /// The gateway demands a newer client version.
    pub fn is_must_upgrade(&self) -> bool {
        self.gateway_code == Some(gateway_code::REQUEST_MUST_UPGRADE)
    }

    /// The server asked the client to renew the V3 token and replay.
    pub fn must_refresh_token(&self) -> bool {
        talk_auth_scoped(&self.path) && self.talk_code == Some(talk_code::MUST_REFRESH_V3_TOKEN)
    }

    /// Retryable per the extension's default retry condition.
    pub fn is_retryable(&self) -> bool {
        self.gateway_code == Some(gateway_code::UNKNOWN_ERROR)
            || self.talk_code == Some(talk_code::SHOULD_RETRY)
    }

    /// The chat is Letter-Sealed and needs an encrypted message.
    pub fn requires_e2ee(&self) -> bool {
        self.talk_code == Some(talk_code::E2EE_RETRY_ENCRYPT)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (HTTP {}", self.path, self.status)?;
        if let Some(c) = self.gateway_code {
            write!(f, ", gateway {c}")?;
        }
        if let Some(c) = self.talk_code {
            write!(f, ", talk {c}")?;
        }
        write!(f, ")")?;
        if let Some(r) = &self.reason {
            write!(f, ": {r}")?;
        }
        Ok(())
    }
}

/// The talk-auth interceptor scope of the extension: `/api/talk/thrift/Talk*`
/// minus `Talk/ChannelService` and `Talk/E2EEKeyBackupService`.
pub(crate) fn talk_auth_scoped(path: &str) -> bool {
    path.starts_with("/api/talk/thrift/Talk")
        && !path.starts_with("/api/talk/thrift/Talk/ChannelService")
        && !path.starts_with("/api/talk/thrift/Talk/E2EEKeyBackupService")
}

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The gateway answered with an application error.
    #[error("LINE API error: {0}")]
    Api(Box<ApiError>),
    /// No access token yet — run a login flow or load a session.
    #[error("not logged in")]
    LoginRequired,
    /// The token could not be refreshed; a new login is needed.
    #[error("re-login required: {0}")]
    Reauth(String),
    /// Network / HTTP-level failure.
    #[error("HTTP transport error: {0}")]
    Http(#[from] reqwest::Error),
    /// The secure module failed (signing, E2EE).
    #[error(transparent)]
    Ltsm(#[from] LtsmError),
    /// A response did not have the expected shape.
    #[error("unexpected response: {0}")]
    Protocol(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A long-poll or flow gave up waiting.
    #[error("timed out: {0}")]
    Timeout(String),
    /// The media was deleted by LINE (`FILE_EXPIRE_TIMESTAMP` passed) or
    /// does not exist (e.g. no preview for files).
    #[error("media not available: {0}")]
    MediaUnavailable(String),
    /// A Letter Sealing operation failed; see [`E2eeErrorKind`].
    #[error("E2EE: {0}")]
    E2ee(E2eeFailure),
}

/// Why a Letter Sealing operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eeErrorKind {
    /// No Letter Sealing keys are loaded (log in by QR to receive the keychain).
    NoKeys,
    /// The message was sealed to one of our keys that we do not hold
    /// (e.g. issued after our login). A new QR login fetches the current keychain.
    MissingOwnKey { key_id: i64 },
    /// The server rejected our sender key (`E2EE_UPDATE_SENDER_KEY`, 83): our
    /// keychain is outdated; log in by QR again.
    OwnKeyOutdated,
    /// The peer's public key for this id is not available any more (the
    /// server answers "invalid keyId" for retired keys).
    PeerKeyUnavailable { mid: String, key_id: i64 },
    /// The peer has no negotiable Letter Sealing key.
    NegotiationFailed { mid: String },
    /// Chunks missing or not decodable.
    MalformedMessage,
    /// Authentication/decryption failed (wrong key, tampered or corrupted data).
    DecryptionFailed,
    /// A spec/e2ee version this client does not implement.
    UnsupportedVersion(i64),
    /// Fetching keys failed for another reason (network, server error); retrying may help.
    Transient,
}

/// A Letter Sealing failure: kind plus human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eeFailure {
    pub kind: E2eeErrorKind,
    pub detail: String,
}

impl fmt::Display for E2eeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl Error {
    pub(crate) fn e2ee(kind: E2eeErrorKind, detail: impl Into<String>) -> Self {
        Error::E2ee(E2eeFailure {
            kind,
            detail: detail.into(),
        })
    }

    /// The Letter Sealing failure kind, if this is one.
    pub fn e2ee_kind(&self) -> Option<&E2eeErrorKind> {
        match self {
            Error::E2ee(f) => Some(&f.kind),
            _ => None,
        }
    }
}

impl Error {
    /// The API error, if this is one.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Error::Api(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Error::Api(Box::new(e))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Pull the outer/inner codes and reason out of an error body. Shapes seen:
///
/// ```json
/// {"code":10051,"message":"RESPONSE_ERROR","data":{"name":"TalkException","code":82,"reason":"..."}}
/// {"message":"FAILED","error":{"code":20,"message":"bad request"}}
/// {"code":10052,"message":"RESPONSE_HTTP_ERROR","data":{"statusCode":410,...}}
/// ```
pub(crate) fn parse_error(path: &str, http_status: u16, body: Value) -> ApiError {
    let mut e = ApiError {
        path: path.to_owned(),
        status: http_status,
        gateway_code: None,
        talk_code: None,
        reason: None,
        raw: Value::Null,
    };
    if let Value::Object(obj) = &body {
        let err = obj.get("error").filter(|v| v.is_object()).unwrap_or(&body);
        let s = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
        e.reason = s(err.get("message"))
            .or(s(err.get("reason")))
            .or(s(err.get("debugMessage")));
        e.gateway_code = obj.get("code").and_then(Value::as_i64).or_else(|| {
            obj.get("error")
                .and_then(|x| x.get("code"))
                .and_then(Value::as_i64)
        });
        if let Some(nested) = err.get("data").filter(|v| v.is_object()) {
            if let Some(c) = nested.get("code").and_then(Value::as_i64) {
                e.talk_code = Some(c);
            }
            if let Some(r) = s(nested.get("reason"))
                .or(s(nested.get("alertMessage")))
                .or(s(nested.get("message")))
            {
                e.reason = Some(r);
            }
            if e.gateway_code == Some(gateway_code::RESPONSE_HTTP_ERROR)
                && let Some(sc) = nested.get("statusCode").and_then(Value::as_u64)
            {
                e.status = sc as u16;
            }
        }
    }
    e.raw = body;
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_talk_exception() {
        let e = parse_error(
            "/api/talk/thrift/Talk/TalkService/sendMessage",
            400,
            json!({"code":10051,"message":"RESPONSE_ERROR","data":{"name":"TalkException","code":82,"reason":"can not send using plain mode"}}),
        );
        assert_eq!(e.gateway_code, Some(10051));
        assert_eq!(e.talk_code, Some(82));
        assert_eq!(e.code(), Some(82));
        assert!(e.requires_e2ee());
        assert_eq!(e.reason.as_deref(), Some("can not send using plain mode"));
        assert!(!e.is_auth());
    }

    #[test]
    fn http_error_envelope_uses_nested_status() {
        let e = parse_error(
            "/api/talk/thrift/LoginQrCode/SecondaryQrCodeLoginPermitNoticeService/checkQrCodeVerified",
            200,
            json!({"code":10052,"message":"RESPONSE_HTTP_ERROR","data":{"statusCode":410}}),
        );
        assert_eq!(e.status, 410);
    }

    #[test]
    fn auth_codes_are_path_scoped() {
        let body = json!({"code":10051,"data":{"code":1}});
        assert!(parse_error("/api/talk/thrift/Talk/TalkService/getProfile", 400, body.clone()).is_auth());
        assert!(
            !parse_error(
                "/api/talk/thrift/Relation/RelationService/addFriendByMid",
                400,
                body.clone()
            )
            .is_auth()
        );
        assert!(
            !parse_error(
                "/api/talk/thrift/Talk/ChannelService/issueChannelToken",
                400,
                body
            )
            .is_auth()
        );
    }

    #[test]
    fn error_block_shape_and_retry() {
        let e = parse_error(
            "/x",
            500,
            json!({"message":"FAILED","error":{"code":99999,"message":"oops"}}),
        );
        assert_eq!(e.gateway_code, Some(99999));
        assert!(e.is_retryable());
        assert_eq!(e.reason.as_deref(), Some("oops"));
    }
}
