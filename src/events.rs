//! Receiving operations (incoming messages, invitations, read receipts, ...).
//!
//! The extension streams them as Server-Sent Events:
//!
//! ```text
//! GET /api/operation/receive?version=3.7.2&language=en_US&lastPartialFullSyncs={}&localRev=<rev>
//! ```
//!
//! Unnamed (`message`) events carry operations — `{"operations":[...]}`, a
//! bare list, or a single operation. Named events are control messages:
//! `ping`, `connInfoRevision`, `reconnect`, `talkException`, `fullSync` and
//! `partialFullSync` (the last two carry a `nextRevision` cursor).
//!
//! `localRev` is the resume cursor: seeded from `getLastOpRevision`, advanced
//! by every operation, and re-sent on reconnect so nothing is missed.
//!
//! Deviation (same as the reference client): the extension's `EventSource`
//! authenticates with a cookie; we send `X-Line-Access` + `X-Hmac` headers.

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Map, Value};

use crate::client::Client;
use crate::error::{Error, Result, talk_code};
use crate::models::{Operation, value_i64};
use crate::transport::APP_VERSION;

/// One parsed Server-Sent Event.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    /// `message` for unnamed events.
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental `text/event-stream` parser.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    buf: Vec<u8>,
    event: String,
    data: Vec<String>,
    id: Option<String>,
}

impl SseParser {
    /// Feed bytes; returns the events completed by them.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(SseEvent {
                        event: if self.event.is_empty() {
                            "message".into()
                        } else {
                            std::mem::take(&mut self.event)
                        },
                        data: self.data.join("\n"),
                        id: self.id.clone(),
                    });
                }
                self.event.clear();
                self.data.clear();
                continue;
            }
            if line.starts_with(':') {
                continue; // comment / keep-alive
            }
            let (key, value) = match line.split_once(':') {
                Some((k, v)) => (k, v.strip_prefix(' ').unwrap_or(v)),
                None => (&*line, ""),
            };
            match key {
                "event" => self.event = value.to_owned(),
                "data" => self.data.push(value.to_owned()),
                "id" => self.id = Some(value.to_owned()),
                _ => {}
            }
        }
        out
    }
}

/// What the stream yields.
#[derive(Debug, Clone)]
pub enum Event {
    /// A new operation (already de-duplicated against the cursor).
    Operation(Box<Operation>),
    /// A control event (`ping`, `fullSync`, ...), for callers who care.
    Control { name: String, data: Value },
}

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

/// A self-reconnecting operation stream. Create with [`Client::operations`].
pub struct OperationStream {
    client: Client,
    /// Resume cursor (`localRev`). `None` = seed from `getLastOpRevision`.
    pub local_rev: Option<i64>,
    last_partial_full_syncs: Map<String, Value>,
    conn: Option<(ByteStream, SseParser)>,
    queue: VecDeque<Event>,
    failures: u32,
    got_event: bool,
    /// Wait this long before reopening (set when a connection was unproductive).
    reconnect_delay: Option<Duration>,
    /// Reopen the connection when nothing arrives for this long.
    pub silence_timeout: Duration,
    /// Cap for the exponential reconnect backoff (starts at 1 s).
    pub max_backoff: Duration,
    /// Decrypt Letter-Sealed messages before yielding them (default: true
    /// when the client has keys). Failures are reported in
    /// [`Operation::e2ee_error`], never by dropping the operation.
    pub decrypt: bool,
}

impl Client {
    /// Stream operations starting after `local_rev` (or from now when `None`).
    pub fn operations(&self, local_rev: Option<i64>) -> OperationStream {
        OperationStream {
            client: self.clone(),
            local_rev,
            last_partial_full_syncs: Map::new(),
            conn: None,
            queue: VecDeque::new(),
            failures: 0,
            got_event: false,
            reconnect_delay: None,
            // The server pings every 20 s (observed live); the extension tears the
            // stream down after interval + 10 s spare without an event.
            silence_timeout: Duration::from_secs(30),
            max_backoff: Duration::from_secs(60),
            decrypt: self.e2ee_ready(),
        }
    }
}

impl OperationStream {
    fn query(&self, local_rev: i64) -> String {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("version", APP_VERSION)
            .append_pair("language", &self.client.config().lal())
            .append_pair(
                "lastPartialFullSyncs",
                &Value::Object(self.last_partial_full_syncs.clone()).to_string(),
            )
            .append_pair("localRev", &local_rev.to_string());
        q.finish()
    }

    async fn open(&mut self) -> Result<()> {
        let rev = match self.local_rev {
            Some(r) => r,
            None => {
                let r = self.client.last_op_revision().await?;
                self.local_rev = Some(r);
                r
            }
        };
        let path = format!("/api/operation/receive?{}", self.query(rev));
        let mut h = HeaderMap::new();
        h.insert("accept", HeaderValue::from_static("text/event-stream"));
        h.insert("cache-control", HeaderValue::from_static("no-cache"));
        if let Ok(v) = HeaderValue::from_str(&self.client.config().locale) {
            h.insert("accept-language", v);
        }
        let token = self.client.t.access_token().ok_or(Error::LoginRequired)?;
        let mut v = HeaderValue::from_str(&token).map_err(|e| Error::Protocol(e.to_string()))?;
        v.set_sensitive(true);
        h.insert("x-line-access", v);
        let resp = self.client.t.get(&path, h, None).await?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            return Err(crate::transport::decode("/api/operation/receive", status, &text)
                .err()
                .map(Error::from)
                .unwrap_or_else(|| Error::Protocol(format!("SSE open: HTTP {status}"))));
        }
        tracing::debug!(local_rev = rev, "operation stream connected");
        self.last_partial_full_syncs.clear();
        self.conn = Some((Box::pin(resp.bytes_stream()), SseParser::default()));
        self.got_event = false;
        Ok(())
    }

    /// Whether `rev` was delivered already (resume / duplicate).
    fn seen(&self, rev: i64) -> bool {
        rev > 0 && self.local_rev.is_some_and(|cur| rev <= cur)
    }

    /// Advance the cursor to `rev` (never backwards).
    fn note_revision(&mut self, rev: i64) {
        if rev > 0 && !self.seen(rev) {
            self.local_rev = Some(rev);
        }
    }

    /// Handle one SSE event: queue operations / control events.
    async fn handle(&mut self, ev: SseEvent) -> Result<()> {
        let data: Value = serde_json::from_str(&ev.data).unwrap_or(Value::String(ev.data.clone()));
        tracing::debug!(event = %ev.event, bytes = ev.data.len(), "sse event");
        match ev.event.as_str() {
            "message" => {
                let ops = match &data {
                    Value::Object(o) if o.contains_key("operations") => o["operations"].clone(),
                    Value::Array(_) => data.clone(),
                    Value::Object(o) if o.contains_key("type") => Value::Array(vec![data.clone()]),
                    _ => Value::Array(vec![]),
                };
                for raw in ops.as_array().into_iter().flatten() {
                    let mut op: Operation = match serde_json::from_value(raw.clone()) {
                        Ok(op) => op,
                        Err(e) => {
                            // Never drop data: surface what we could not parse.
                            tracing::warn!("unparsable operation: {e}");
                            self.queue.push_back(Event::Control {
                                name: "unparsed".into(),
                                data: raw.clone(),
                            });
                            continue;
                        }
                    };
                    if self.seen(op.revision) {
                        continue; // already delivered (resume / duplicate)
                    }
                    self.client.note_operation(&op);
                    if self.decrypt {
                        Self::decrypt_in_place(&self.client, &mut op).await?;
                    }
                    // Only now: an operation that stopped the stream is
                    // delivered again when it resumes from `local_rev`.
                    self.note_revision(op.revision);
                    self.queue.push_back(Event::Operation(Box::new(op)));
                }
            }
            "fullSync" | "partialFullSync" => {
                if let Some(r) = data.get("nextRevision").and_then(value_i64) {
                    self.note_revision(r);
                }
                self.queue.push_back(Event::Control { name: ev.event, data });
            }
            "reconnect" => {
                self.conn = None; // reopen right away with the cursor
            }
            "talkException" => {
                let code = data.get("code").and_then(value_i64);
                if code == Some(talk_code::MUST_REFRESH_V3_TOKEN) {
                    match self.client.refresh_token().await {
                        Ok(()) => self.conn = None,
                        Err(e @ (Error::Reauth(_) | Error::LoginRequired)) => return Err(e),
                        // A network hiccup: reconnect (with backoff) and try again.
                        Err(e) => {
                            tracing::warn!("token refresh for the operation stream failed: {e}");
                            self.reconnect_later();
                        }
                    }
                } else if matches!(
                    code,
                    Some(
                        talk_code::AUTHENTICATION_FAILED
                            | talk_code::NOT_AVAILABLE_USER
                            | talk_code::NOT_AUTHORIZED_DEVICE
                    )
                ) {
                    return Err(Error::Reauth(format!(
                        "talkException on operation stream: {data}"
                    )));
                }
                self.queue.push_back(Event::Control { name: ev.event, data });
            }
            _ => self.queue.push_back(Event::Control { name: ev.event, data }),
        }
        Ok(())
    }

    // An associated fn (not `&self`): holding `&self` across the await would
    // require the stream to be `Sync`, making it impossible to `tokio::spawn`.
    async fn decrypt_in_place(client: &Client, op: &mut Operation) -> Result<()> {
        let Some(m) = op.message.as_ref().filter(|m| m.is_e2ee()) else {
            return Ok(());
        };
        match client.decrypt_message(m).await {
            Ok(d) => op.message = Some(d),
            // Session-level failures must stop the stream, not be swallowed.
            Err(e @ (Error::LoginRequired | Error::Reauth(_))) => return Err(e),
            Err(Error::Api(a)) if a.is_auth() => return Err(Error::Api(a)),
            Err(Error::E2ee(f)) => {
                tracing::warn!(revision = op.revision, "could not decrypt message: {f}");
                op.e2ee_error = Some(f);
            }
            Err(e) => {
                tracing::warn!(revision = op.revision, "could not decrypt message: {e}");
                op.e2ee_error = Some(crate::error::E2eeFailure {
                    kind: crate::error::E2eeErrorKind::Transient,
                    detail: e.to_string(),
                });
            }
        }
        Ok(())
    }

    fn backoff(&self) -> Duration {
        let secs = 1u64.checked_shl(self.failures.min(20)).unwrap_or(u64::MAX);
        Duration::from_secs(secs).min(self.max_backoff)
    }

    /// The next event, reconnecting as needed. Errors are returned only when
    /// retrying cannot help (logged out, re-login required).
    pub async fn next_event(&mut self) -> Result<Event> {
        loop {
            if let Some(ev) = self.queue.pop_front() {
                return Ok(ev);
            }
            if self.conn.is_none()
                && let Some(wait) = self.reconnect_delay.take()
            {
                tracing::debug!("reopening the operation stream in {wait:?}");
                tokio::time::sleep(wait).await;
            }
            if self.conn.is_none()
                && let Err(e) = self.open().await
            {
                if matches!(e, Error::LoginRequired | Error::Reauth(_))
                    || e.api().is_some_and(|a| a.is_auth() || a.is_must_upgrade())
                {
                    return Err(e);
                }
                let wait = self.backoff();
                self.failures += 1;
                tracing::warn!("operation stream connect failed ({e}); retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                continue;
            }
            let (stream, parser) = self.conn.as_mut().expect("connected");
            let next = tokio::time::timeout(self.silence_timeout, stream.next()).await;
            let events = match next {
                Ok(Some(Ok(bytes))) => parser.feed(&bytes),
                Ok(Some(Err(e))) => {
                    tracing::warn!("operation stream read error: {e}");
                    self.drop_conn();
                    continue;
                }
                Ok(None) => {
                    tracing::debug!("operation stream closed by server");
                    self.drop_conn();
                    continue;
                }
                Err(_) => {
                    tracing::debug!(
                        "operation stream silent for {:?}; reconnecting",
                        self.silence_timeout
                    );
                    self.drop_conn();
                    continue;
                }
            };
            for ev in events {
                self.got_event = true;
                self.failures = 0;
                self.handle(ev).await?;
            }
        }
    }

    /// The next operation, skipping control events.
    pub async fn next_operation(&mut self) -> Result<Operation> {
        loop {
            if let Event::Operation(op) = self.next_event().await? {
                return Ok(*op);
            }
        }
    }

    fn drop_conn(&mut self) {
        // A connection that delivered nothing counts as a failure (backoff);
        // a productive one reconnects immediately, like the extension.
        if self.got_event {
            self.conn = None;
        } else {
            self.reconnect_later();
        }
    }

    /// Close the connection and reopen it after the backoff.
    fn reconnect_later(&mut self) {
        self.reconnect_delay = Some(self.backoff());
        self.failures += 1;
        self.conn = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks_and_named_events() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: ping\r\ndata: {}\r\n").is_empty());
        let evs = p.feed(b"\r\n: comment\ndata: {\"operations\":\ndata: []}\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event, "ping");
        assert_eq!(evs[1].event, "message");
        assert_eq!(evs[1].data, "{\"operations\":\n[]}");
    }
}

#[cfg(test)]
mod send_tests {
    /// The stream must be usable from a spawned task.
    #[allow(dead_code)]
    fn assert_send<T: Send>(_: T) {}
    #[allow(dead_code)]
    fn next_operation_future_is_send(s: &mut super::OperationStream) {
        assert_send(s.next_operation());
    }
}
