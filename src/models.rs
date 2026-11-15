//! Request builders and typed views of common responses.
//!
//! The gateway speaks Thrift structs as JSON objects with camelCase field
//! names. 64-bit integers frequently arrive as *strings*, so numeric fields
//! here accept either form. Every typed struct keeps unknown fields in
//! `extra`, so nothing the server sends is lost.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};

/// Read an integer that may be encoded as a JSON number or string.
pub(crate) fn value_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn de_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = Value::deserialize(d)?;
    match &v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .ok_or_else(|| serde::de::Error::custom(format!("expected number, got {v}")))
}

fn de_opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(v.as_ref().and_then(value_i64))
}

fn de_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(de_opt_i64(d)?.unwrap_or(0))
}

/// Null-tolerant `String` (Thrift optional strings arrive as `null`).
fn de_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// Thrift `MIDType`, inferred from a mid's first letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidType {
    User = 0,
    Room = 1,
    Group = 2,
    Square = 3,
    SquareChat = 4,
}

impl MidType {
    /// `U…` user, `C…` group chat, `R…` room, `S…` square (case-insensitive).
    pub fn of(mid: &str) -> MidType {
        match mid.as_bytes().first().map(u8::to_ascii_lowercase) {
            Some(b'c') => MidType::Group,
            Some(b'r') => MidType::Room,
            Some(b's') => MidType::SquareChat,
            _ => MidType::User,
        }
    }
}

/// Thrift `ContentType` values used by this crate.
pub mod content_type {
    pub const TEXT: i64 = 0;
    pub const IMAGE: i64 = 1;
    pub const VIDEO: i64 = 2;
    pub const AUDIO: i64 = 3;
    /// Call log entry (1:1 call record or group-call event); see [`super::CallRecord`].
    pub const CALL: i64 = 6;
    pub const STICKER: i64 = 7;
    pub const CONTACT: i64 = 13;
    pub const FILE: i64 = 14;
    pub const LOCATION: i64 = 15;
    pub const FLEX: i64 = 22;
}

/// Thrift `MessageRelationType` (`Message.messageRelationType`, set together
/// with `relatedMessageId`).
pub mod relation_type {
    pub const FORWARD: i64 = 0;
    pub const AUTO_REPLY: i64 = 1;
    pub const SUBORDINATE: i64 = 2;
    pub const REPLY: i64 = 3;
}

/// Thrift `Message.relatedMessageServiceCode`: the related message is a Talk message.
const SERVICE_CODE_TALK: i64 = 1;

/// Thrift `SyncReason`.
pub mod sync_reason {
    pub const INITIALIZATION: i64 = 2;
    pub const OPERATION: i64 = 3;
    pub const FULL_SYNC: i64 = 4;
}

/// Selected Thrift `OpType` values (operations delivered by the event stream).
pub mod op_type {
    pub const END_OF_OPERATION: i64 = 0;
    /// Someone's profile changed: `param1` user, `param2` attribute, `param3` JSON (live).
    pub const NOTIFIED_UPDATE_PROFILE: i64 = 2;
    pub const SEND_MESSAGE: i64 = 25;
    pub const RECEIVE_MESSAGE: i64 = 26;
    /// We read a chat on another device: `param1` chat, `param2` message id (live).
    pub const SEND_CHAT_CHECKED: i64 = 40;
    /// Someone is calling this account (legacy op; never observed live).
    pub const NOTIFIED_RECEIVED_CALL: i64 = 50;
    /// A ringing call was cancelled (legacy op; never observed live).
    pub const CANCEL_CALL: i64 = 51;
    /// Someone read a chat: `param1` chat (the reader's mid in 1:1 chats),
    /// `param2` reader, `param3` last read message id (live).
    pub const NOTIFIED_READ_MESSAGE: i64 = 55;
    pub const NOTIFIED_JOIN_CHAT: i64 = 60;
    /// `param1` chat, `param2` member who left (live).
    pub const NOTIFIED_LEAVE_CHAT: i64 = 61;
    /// Typing notification (reference enum only; never observed live, and the
    /// extension has no method to send one).
    pub const NOTIFIED_TYPING: i64 = 62;
    /// We unsent a message (any device): `param1` chat, `param2` message id (live).
    pub const DESTROY_MESSAGE: i64 = 64;
    /// Someone unsent a message: `param1` chat, `param2` message id (live).
    pub const NOTIFIED_DESTROY_MESSAGE: i64 = 65;
    pub const CREATE_CHAT: i64 = 120;
    /// We changed a chat: `param1` chat, `param2` `ChatAttribute` (live).
    pub const UPDATE_CHAT: i64 = 121;
    pub const NOTIFIED_UPDATE_CHAT: i64 = 122;
    pub const NOTIFIED_INVITE_INTO_CHAT: i64 = 124;
    pub const NOTIFIED_CANCEL_CHAT_INVITATION: i64 = 126;
    /// We left a group (any device): `param1` chat.
    pub const DELETE_SELF_FROM_CHAT: i64 = 127;
    /// A member left a group: `param1` chat, `param2` member (live).
    pub const NOTIFIED_DELETE_SELF_FROM_CHAT: i64 = 128;
    pub const ACCEPT_CHAT_INVITATION: i64 = 129;
    pub const NOTIFIED_ACCEPT_CHAT_INVITATION: i64 = 130;
    /// We declined an invitation (any device): `param1` chat (live).
    pub const REJECT_CHAT_INVITATION: i64 = 131;
    pub const NOTIFIED_DELETE_OTHER_FROM_CHAT: i64 = 133;
    pub const SEND_REACTION: i64 = 139;
    /// A reaction changed: `param1` message id, `param2`
    /// `{"chatMid", "curr": ReactionType}` JSON, `param3` who reacted (live).
    pub const NOTIFIED_SEND_REACTION: i64 = 140;
}

/// Thrift `ChatAttribute` (`updateChat`'s `updatedAttribute`).
pub mod chat_attribute {
    pub const NAME: i64 = 1;
    pub const PICTURE_STATUS: i64 = 2;
    pub const PREVENTED_JOIN_BY_TICKET: i64 = 4;
    pub const NOTIFICATION_SETTING: i64 = 8;
    pub const INVITATION_TICKET: i64 = 16;
    pub const FAVORITE_TIMESTAMP: i64 = 32;
}

/// Thrift `SettingsAttributeEx` values for account notifications. Each was
/// checked live with `getSettingsAttributes2` (asking for one attribute fills
/// exactly the named field).
pub mod settings_attribute {
    pub const NOTIFICATION_ENABLE: i64 = 0;
    pub const NOTIFICATION_MUTE_EXPIRATION: i64 = 1;
    pub const NOTIFICATION_NEW_MESSAGE: i64 = 2;
    pub const NOTIFICATION_GROUP_INVITATION: i64 = 3;
    pub const NOTIFICATION_SHOW_MESSAGE: i64 = 4;
    pub const NOTIFICATION_MENTION: i64 = 40;
    pub const NOTIFICATION_THUMBNAIL: i64 = 45;
    pub const NOTIFICATION_REACTION: i64 = 67;
}

/// Thrift `ContactSetting` flags (`updateContactSetting`, `Contact.settings` bits).
pub mod contact_setting {
    pub const NOTIFICATION_DISABLE: i64 = 1;
    pub const DISPLAY_NAME_OVERRIDE: i64 = 2;
    pub const CONTACT_HIDE: i64 = 4;
    pub const FAVORITE: i64 = 8;
}

/// A message to send (`Talk.TalkService.sendMessage`'s `Message` struct).
///
/// Mirrors the extension's builders: `{to, toType, contentType, text,
/// contentMetadata, sessionId: 0}` (+ `hasContent` for text/stickers).
/// `from`/`id`/`createdTime` are omitted: the gateway fills them in.
#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingMessage {
    pub to: String,
    pub content_type: i64,
    pub text: Option<String>,
    pub content_metadata: BTreeMap<String, String>,
    /// Additional raw fields merged into the struct (e.g. `location`).
    pub extra: Map<String, Value>,
}

impl OutgoingMessage {
    fn base(to: &str, content_type: i64) -> Self {
        OutgoingMessage {
            to: to.to_owned(),
            content_type,
            text: None,
            content_metadata: BTreeMap::new(),
            extra: Map::new(),
        }
    }

    /// Plain text.
    pub fn text(to: &str, text: impl Into<String>) -> Self {
        let mut m = Self::base(to, content_type::TEXT);
        m.text = Some(text.into());
        m.extra.insert("hasContent".into(), json!(false));
        m
    }

    /// An image/video/audio/file message shell (metadata is filled in by
    /// [`Client::send_media`](crate::Client::send_media)).
    pub fn media(to: &str, content_type: i64) -> Self {
        let mut m = Self::base(to, content_type);
        m.text = Some(String::new());
        m.extra.insert("hasContent".into(), json!(true));
        m
    }

    /// A text reply quoting `message_id`.
    pub fn reply(to: &str, text: impl Into<String>, message_id: &str) -> Self {
        Self::text(to, text).reply_to(message_id)
    }

    /// Make this message a reply quoting `message_id` (any content type, e.g.
    /// a sticker). Replies are not a content type: they are ordinary messages
    /// with `messageRelationType` REPLY (3), `relatedMessageId` and
    /// `relatedMessageServiceCode` 1 (TALK). Like current official apps (live
    /// history), the relation is repeated in `contentMetadata`; it stays
    /// outside the ciphertext when the message is sealed.
    pub fn reply_to(mut self, message_id: &str) -> Self {
        self.extra.insert("relatedMessageId".into(), json!(message_id));
        self.extra
            .insert("messageRelationType".into(), json!(relation_type::REPLY));
        self.extra
            .insert("relatedMessageServiceCode".into(), json!(SERVICE_CODE_TALK));
        for (k, v) in [
            ("message_relation_server_message_id", message_id),
            ("message_relation_service_code", "talk"),
            ("message_relation_type_code", "reply"),
        ] {
            self.content_metadata.insert(k.into(), v.into());
        }
        self
    }

    /// A sticker by package/sticker id.
    pub fn sticker(to: &str, package_id: &str, sticker_id: &str, version: u32) -> Self {
        let mut m = Self::base(to, content_type::STICKER);
        m.text = Some(String::new());
        m.content_metadata.insert("STKID".into(), sticker_id.into());
        m.content_metadata.insert("STKPKGID".into(), package_id.into());
        m.content_metadata.insert("STKVER".into(), version.to_string());
        m.extra.insert("hasContent".into(), json!(true));
        m
    }

    /// A contact card.
    pub fn contact(to: &str, mid: &str, display_name: &str) -> Self {
        let mut m = Self::base(to, content_type::CONTACT);
        m.text = Some(String::new());
        m.content_metadata.insert("mid".into(), mid.into());
        m.content_metadata
            .insert("displayName".into(), display_name.into());
        m
    }

    /// A location pin.
    pub fn location(to: &str, latitude: f64, longitude: f64, title: &str, address: &str) -> Self {
        let mut m = Self::base(to, content_type::LOCATION);
        m.text = Some(String::new());
        m.extra.insert(
            "location".into(),
            json!({"title": title, "address": address, "latitude": latitude, "longitude": longitude, "phone": null}),
        );
        m
    }

    /// The wire JSON object.
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("to".into(), json!(self.to));
        o.insert("toType".into(), json!(MidType::of(&self.to) as i64));
        o.insert("contentMetadata".into(), json!(self.content_metadata));
        o.insert("sessionId".into(), json!(0));
        if let Some(t) = &self.text {
            o.insert("text".into(), json!(t));
        }
        o.insert("contentType".into(), json!(self.content_type));
        for (k, v) in &self.extra {
            o.insert(k.clone(), v.clone());
        }
        Value::Object(o)
    }
}

/// `Talk.TalkService.getProfile` result.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    #[serde(deserialize_with = "de_string")]
    pub mid: String,
    pub userid: Option<String>,
    #[serde(deserialize_with = "de_string")]
    pub display_name: String,
    #[serde(deserialize_with = "de_string")]
    pub status_message: String,
    pub picture_path: Option<String>,
    pub region_code: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A contact (`getContactsV2` returns `{contacts: {mid: {contact: Contact}}}`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Contact {
    #[serde(deserialize_with = "de_string")]
    pub mid: String,
    #[serde(deserialize_with = "de_string")]
    pub display_name: String,
    pub display_name_overridden: Option<String>,
    #[serde(deserialize_with = "de_string")]
    pub status_message: String,
    pub picture_path: Option<String>,
    #[serde(deserialize_with = "de_opt_i64")]
    pub r#type: Option<i64>,
    #[serde(deserialize_with = "de_opt_i64")]
    pub relation: Option<i64>,
    pub capable_buddy: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Contact {
    /// Notifications from this friend are turned off (`settings` bit 1).
    pub fn notifications_disabled(&self) -> bool {
        self.extra
            .get("settings")
            .and_then(value_i64)
            .is_some_and(|s| s & contact_setting::NOTIFICATION_DISABLE != 0)
    }

    /// The profile picture's path for `Client::download_picture` /
    /// `media::picture_url`: `picturePath`, else `/{pictureStatus}`. `None`
    /// when the user has no picture (or `getContactsV2` does not show it).
    pub fn picture(&self) -> Option<String> {
        picture_of(
            &self.picture_path,
            self.extra.get("pictureStatus").and_then(Value::as_str),
        )
    }

    /// Name as the client shows it (the user's override wins).
    pub fn name(&self) -> &str {
        self.display_name_overridden
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.display_name)
    }
}

/// A picture path from `picturePath`, else `pictureStatus` (the same
/// picture without the leading `/`); `None` when both are empty.
fn picture_of(path: &Option<String>, status: Option<&str>) -> Option<String> {
    fn nonempty(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    nonempty(path.as_deref())
        .map(str::to_owned)
        .or_else(|| nonempty(status).map(|s| format!("/{}", s.trim_start_matches('/'))))
}

/// A group chat or room from `getChats`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    #[serde(deserialize_with = "de_string")]
    pub chat_mid: String,
    #[serde(deserialize_with = "de_string")]
    pub chat_name: String,
    #[serde(deserialize_with = "de_opt_i64")]
    pub r#type: Option<i64>,
    /// Group picture on the profile CDN (`/0h…`), as `getChats` returns it.
    pub picture_path: Option<String>,
    /// Older field: the same picture without the leading `/`.
    pub picture_status: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Chat {
    /// The group picture's path for `Client::download_picture` /
    /// `media::picture_url`: `picturePath`, else `/{pictureStatus}`.
    /// `None` when the group has no picture.
    pub fn picture(&self) -> Option<String> {
        picture_of(&self.picture_path, self.picture_status.as_deref())
    }

    /// Notifications for this group are turned off (`notificationDisabled`).
    pub fn notifications_disabled(&self) -> bool {
        self.extra.get("notificationDisabled").and_then(Value::as_bool) == Some(true)
    }

    /// When the chat was created (epoch ms), if the server sent it.
    pub fn created_time(&self) -> Option<i64> {
        self.extra.get("createdTime").and_then(value_i64)
    }

    /// Member mids (from `extra.groupExtra.memberMids`; keys are mids).
    pub fn member_mids(&self) -> Vec<String> {
        self.group_extra_keys("memberMids")
    }

    /// Pending invitee mids.
    pub fn invitee_mids(&self) -> Vec<String> {
        self.group_extra_keys("inviteeMids")
    }

    fn group_extra_keys(&self, field: &str) -> Vec<String> {
        match self
            .extra
            .get("extra")
            .and_then(|e| e.get("groupExtra"))
            .and_then(|g| g.get(field))
        {
            Some(Value::Object(m)) => m.keys().cloned().collect(),
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        }
    }
}

/// Voice, video or live: the media of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallMedia {
    Voice,
    Video,
    /// Group "live" broadcast (`GC_MEDIA_TYPE` `LIVE`).
    Live,
}

/// What a call record says happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// 1:1 call that connected (see [`CallRecord::duration_ms`]).
    Answered,
    /// 1:1 call nobody picked up.
    Unanswered,
    /// 1:1 call the caller hung up before it connected.
    Cancelled,
    /// 1:1 call the callee rejected.
    Declined,
    /// 1:1 call to a busy line.
    Busy,
    /// A group call started.
    GroupStarted,
    /// A group call ended.
    GroupEnded,
    /// Someone was invited into a group call.
    GroupInvited,
    /// A value this crate does not know, verbatim (`RESULT` / `GC_EVT_TYPE`).
    Other(String),
}

/// A call log entry: a [`content_type::CALL`] message, which the server adds
/// to the chat when a call ends (1:1) or a group call starts/ends.
///
/// The metadata layout comes from the official apps (`TYPE`, `RESULT`,
/// `DURATION` for 1:1 calls; `GC_EVT_TYPE`, `GC_MEDIA_TYPE`, `DURATION` for
/// group calls). The Chrome extension cannot place calls and neither its bundle
/// nor the reference parse these, so parsing is lenient: unknown values fall
/// back to [`CallOutcome::Other`] or to a voice call rather than failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    /// A group-call event rather than a 1:1 call.
    pub group: bool,
    pub media: CallMedia,
    pub outcome: CallOutcome,
    /// Talk time in milliseconds, when the record carries a non-zero one.
    pub duration_ms: Option<u64>,
}

impl CallRecord {
    /// Parses the call metadata of `m`; `None` unless it is a call message.
    pub fn from_message(m: &Message) -> Option<Self> {
        (m.content_type == content_type::CALL).then(|| Self::from_metadata(&m.content_metadata))
    }

    fn from_metadata(meta: &BTreeMap<String, String>) -> Self {
        let get = |k: &str| {
            meta.get(k)
                .map(|v| v.trim().to_ascii_uppercase())
                .filter(|v| !v.is_empty())
        };
        let duration_ms = meta
            .get("DURATION")
            .and_then(|d| d.trim().parse::<u64>().ok())
            .filter(|&d| d > 0);
        let media_value = get("GC_MEDIA_TYPE").or_else(|| get("TYPE"));
        let media = match media_value.as_deref() {
            Some("V" | "VIDEO") => CallMedia::Video,
            Some("LIVE") => CallMedia::Live,
            _ => CallMedia::Voice,
        };
        if let Some(event) = get("GC_EVT_TYPE") {
            let outcome = match event.as_str() {
                "S" => CallOutcome::GroupStarted,
                "E" => CallOutcome::GroupEnded,
                "I" => CallOutcome::GroupInvited,
                _ => CallOutcome::Other(meta["GC_EVT_TYPE"].clone()),
            };
            return Self {
                group: true,
                media,
                outcome,
                duration_ms,
            };
        }
        let outcome = match get("RESULT").as_deref() {
            Some("CANCELED" | "CANCELLED" | "CANCEL") => CallOutcome::Cancelled,
            Some("REJECTED" | "REJECT" | "DECLINED" | "DENIED") => CallOutcome::Declined,
            Some("BUSY") => CallOutcome::Busy,
            Some("NO_ANSWER" | "NOANSWER" | "MISSED" | "ABSENCE" | "TIMEOUT") => CallOutcome::Unanswered,
            // "INFO" is what a normal (connected) record carries.
            None | Some("INFO" | "NORMAL" | "OK") if duration_ms.is_some() => CallOutcome::Answered,
            None | Some("INFO" | "NORMAL" | "OK") => CallOutcome::Unanswered,
            Some(_) if duration_ms.is_some() => CallOutcome::Answered,
            Some(_) => CallOutcome::Other(meta["RESULT"].clone()),
        };
        Self {
            group: false,
            media,
            outcome,
            duration_ms,
        }
    }
}

/// `m:ss`, or `h:mm:ss` from one hour on.
pub fn format_call_duration(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Direction-neutral, e.g. "Voice call · 3:12", "Video call · not answered",
/// "Group voice call started", "Live broadcast ended · 1:02:03".
impl std::fmt::Display for CallRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match (self.media, self.group) {
            (CallMedia::Live, _) => "Live broadcast",
            (CallMedia::Voice, false) => "Voice call",
            (CallMedia::Video, false) => "Video call",
            (CallMedia::Voice, true) => "Group voice call",
            (CallMedia::Video, true) => "Group video call",
        };
        match &self.outcome {
            CallOutcome::Answered | CallOutcome::GroupEnded => {
                let verb = if self.group { " ended" } else { "" };
                write!(f, "{what}{verb}")?;
                if let Some(d) = self.duration_ms {
                    write!(f, " · {}", format_call_duration(d))?;
                }
                Ok(())
            }
            CallOutcome::GroupStarted => write!(f, "{what} started"),
            CallOutcome::GroupInvited => write!(f, "Invited to a {}", what.to_lowercase()),
            CallOutcome::Unanswered => write!(f, "{what} · not answered"),
            CallOutcome::Cancelled => write!(f, "{what} · cancelled"),
            CallOutcome::Declined => write!(f, "{what} · declined"),
            CallOutcome::Busy => write!(f, "{what} · busy"),
            CallOutcome::Other(v) => write!(f, "{what} · {v}"),
        }
    }
}

/// A received or sent message.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    #[serde(deserialize_with = "de_string")]
    pub id: String,
    #[serde(rename = "from", deserialize_with = "de_string")]
    pub from: String,
    #[serde(deserialize_with = "de_string")]
    pub to: String,
    #[serde(deserialize_with = "de_i64")]
    pub to_type: i64,
    pub text: Option<String>,
    #[serde(deserialize_with = "de_i64")]
    pub content_type: i64,
    pub content_metadata: BTreeMap<String, String>,
    #[serde(deserialize_with = "de_opt_i64")]
    pub created_time: Option<i64>,
    /// Letter-Sealing chunks when the message is end-to-end encrypted.
    pub chunks: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Message {
    /// True when the payload is Letter-Sealed (text is in `chunks`).
    pub fn is_e2ee(&self) -> bool {
        self.chunks.as_ref().is_some_and(|c| c.len() >= 3)
    }

    /// `relatedMessageId` and `messageRelationType`, when the message points at
    /// another one.
    fn relation(&self) -> Option<(&str, i64)> {
        let id = self
            .extra
            .get("relatedMessageId")
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .filter(|s| !s.is_empty() && *s != "0")?;
        let kind = match self.extra.get("messageRelationType").and_then(value_i64) {
            Some(k) => k,
            // Fall back to the metadata copy newer apps add.
            None => match self
                .content_metadata
                .get("message_relation_type_code")
                .map(String::as_str)
            {
                Some("reply") => relation_type::REPLY,
                Some("forward") => relation_type::FORWARD,
                _ => return None,
            },
        };
        Some((id, kind))
    }

    /// The id of the message this one replies to (quotes), if it is a reply.
    pub fn reply_to(&self) -> Option<&str> {
        self.relation()
            .filter(|&(_, k)| k == relation_type::REPLY)
            .map(|(id, _)| id)
    }

    /// An animated sticker (`STKOPT` contains `A`, live); its animation is
    /// [`StickerImage::Animation`](crate::StickerImage::Animation).
    pub fn is_animated_sticker(&self) -> bool {
        self.content_type == content_type::STICKER
            && self
                .content_metadata
                .get("STKOPT")
                .is_some_and(|o| o.contains('A'))
    }

    /// Reactions on this message (`reactions`, filled in by the server).
    pub fn reactions(&self) -> Vec<MessageReaction> {
        self.extra
            .get("reactions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(MessageReaction::from_json).collect())
            .unwrap_or_default()
    }

    /// When the server delivered the message (`deliveredTime`, epoch ms);
    /// with `id` it is the cursor for older history.
    pub fn delivered_time(&self) -> Option<i64> {
        self.extra.get("deliveredTime").and_then(value_i64)
    }

    /// The call log entry, for a [`content_type::CALL`] message.
    pub fn call(&self) -> Option<CallRecord> {
        CallRecord::from_message(self)
    }

    /// True for a forwarded message (`relatedMessageId` with relation FORWARD).
    pub fn is_forwarded(&self) -> bool {
        self.relation().is_some_and(|(_, k)| k == relation_type::FORWARD)
    }

    /// The chat this message belongs to, from our point of view: the group/
    /// room for group messages, otherwise the other party.
    pub fn chat_mid(&self, my_mid: &str) -> &str {
        if self.to_type != MidType::User as i64 || self.from == my_mid {
            &self.to
        } else {
            &self.from
        }
    }
}

/// What kind of reaction someone left.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReactionKind {
    /// A built-in reaction (`predefinedReactionType`, see [`crate::Reaction`]).
    Predefined(i64),
    /// A purchased emoji reaction (`paidReactionType`).
    Paid { product_id: String, emoji_id: String },
}

impl ReactionKind {
    /// Parse a Thrift `ReactionType` object.
    pub fn from_json(v: &Value) -> Option<Self> {
        if let Some(n) = v.get("predefinedReactionType").and_then(value_i64) {
            return Some(ReactionKind::Predefined(n));
        }
        let p = v.get("paidReactionType")?;
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(ReactionKind::Paid {
            product_id: s("productId"),
            emoji_id: s("emojiId"),
        })
    }

    /// A Unicode emoji to draw for it: the built-in reactions, and the
    /// "LINE Basic Emoji" pack's faces, hands and objects. `None` for other
    /// LINE emoji (they only exist as pictures, see
    /// [`emoji_url`](crate::media::emoji_url)).
    pub fn emoji(&self) -> Option<&'static str> {
        match self {
            ReactionKind::Predefined(n) => crate::Reaction::from_i64(*n).map(crate::Reaction::emoji),
            ReactionKind::Paid { product_id, emoji_id } if product_id == BASIC_EMOJI_PACKAGE => {
                basic_emoji_glyph(emoji_id)
            }
            ReactionKind::Paid { .. } => None,
        }
    }

    /// The Thrift `ReactionType` object.
    pub fn to_json(&self) -> Value {
        match self {
            ReactionKind::Predefined(n) => json!({ "predefinedReactionType": n }),
            ReactionKind::Paid { product_id, emoji_id } => {
                json!({ "paidReactionType": { "productId": product_id, "emojiId": emoji_id } })
            }
        }
    }
}

/// The "LINE Basic Emoji" package, which every account owns
/// (`grantedByDefault`, live).
pub const BASIC_EMOJI_PACKAGE: &str = "670e0cce840a8236ddd4ee4c";

/// Unicode stand-ins for "LINE Basic Emoji" `077`–`211`, matched to the
/// pictures on the sticker CDN (`001`–`076` are LINE characters).
const BASIC_EMOJI_GLYPHS: [&str; 135] = [
    // 077–110: faces
    "🙂", "😌", "😃", "😂", "🥺", "😗", "😇", "🥰", "😝", "😎", "😶", "🙁", "😮", "😟", "🥹", "😔", "😭",
    "😲", "😵", "💔", "😓", "😏", "😰", "😳", "😫", "😢", "😴", "🤤", "🥱", "🤭", "🤮", "😷", "😠", "😈",
    // 111–142: the round character
    "😊", "✌️", "👍", "🥹", "😍", "😆", "🤩", "🤔", "😢", "😇", "🌈", "👀", "🫡", "🙃", "🫠", "😨", "😧",
    "😭", "😣", "🙇", "😞", "😡", "🫣", "🥳", "❓", "💃", "🙌", "💪", "🤸", "🏃", "🛌", "🦵",
    // 143–156: hands
    "👍", "👎", "🤞", "✌️", "👋", "🙏", "💪", "🫶", "👈", "👉", "🤘", "👌", "👊", "🫰",
    // 157–201: animals, people, symbols, food, nature, things
    "🐱", "🐶", "🎅", "👻", "🤡", "👽", "💩", "💰", "❤️", "💔", "🔥", "✨", "💧", "🎶", "🎵", "🎉", "🍎",
    "🍌", "🥦", "🍞", "🍖", "🍔", "🍦", "🎂", "☕", "🥤", "🍺", "🌞", "⭐", "🌙", "🌸", "🪴", "🌲", "🌊",
    "🗻", "🌍", "🚗", "🚑", "⚽", "🎤", "🎸", "🔨", "🕒", "✅", "❌",
    // 202–211: digits (full-width: one code point, two columns)
    "０", "１", "２", "３", "４", "５", "６", "７", "８", "９",
];

/// Unicode stand-in for a "LINE Basic Emoji" id (`"077"`…`"211"`).
pub fn basic_emoji_glyph(emoji_id: &str) -> Option<&'static str> {
    let n: usize = emoji_id.parse().ok()?;
    BASIC_EMOJI_GLYPHS.get(n.checked_sub(77)?).copied()
}

/// One entry of `Message.reactions` (live shape: `{fromUserMid, atMillis, reactionType}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageReaction {
    pub from: String,
    /// Epoch ms.
    pub at: i64,
    pub kind: ReactionKind,
}

impl MessageReaction {
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(MessageReaction {
            from: v.get("fromUserMid")?.as_str()?.to_owned(),
            at: v.get("atMillis").and_then(value_i64).unwrap_or(0),
            kind: ReactionKind::from_json(v.get("reactionType")?)?,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({ "fromUserMid": self.from, "atMillis": self.at.to_string(), "reactionType": self.kind.to_json() })
    }
}

/// Replace `from`'s reaction in a `reactions` list (`None` removes it).
pub fn apply_reaction(reactions: &mut Vec<MessageReaction>, from: &str, kind: Option<ReactionKind>, at: i64) {
    reactions.retain(|r| r.from != from);
    if let Some(kind) = kind {
        reactions.push(MessageReaction {
            from: from.to_owned(),
            at,
            kind,
        });
    }
}

/// One talk operation from the event stream.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Operation {
    #[serde(deserialize_with = "de_i64")]
    pub revision: i64,
    #[serde(deserialize_with = "de_i64")]
    pub r#type: i64,
    #[serde(deserialize_with = "de_opt_i64")]
    pub req_seq: Option<i64>,
    pub param1: Option<String>,
    pub param2: Option<String>,
    pub param3: Option<String>,
    pub message: Option<Message>,
    /// Set by the event stream when this operation's message is Letter-Sealed
    /// and could not be decrypted; `message` then still holds the sealed
    /// original so nothing is lost (retry with `Client::decrypt_message`).
    #[serde(skip)]
    pub e2ee_error: Option<crate::error::E2eeFailure>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_and_forwards() {
        let wire = OutgoingMessage::text("Cg", "hi")
            .reply_to("640442637832946311")
            .to_json();
        assert_eq!(wire["relatedMessageId"], "640442637832946311");
        assert_eq!(wire["messageRelationType"], 3);
        assert_eq!(wire["relatedMessageServiceCode"], 1);
        assert_eq!(wire["contentMetadata"]["message_relation_type_code"], "reply");
        assert_eq!(wire["contentType"], 0);
        // Shapes seen in live history.
        let msg = |v: Value| -> Message { serde_json::from_value(v).unwrap() };
        let reply = msg(json!({"id": "2", "contentType": 7, "messageRelationType": 3,
                               "relatedMessageId": "1", "relatedMessageServiceCode": 1}));
        assert_eq!((reply.reply_to(), reply.is_forwarded()), (Some("1"), false));
        let fwd = msg(json!({"id": "3", "messageRelationType": 0, "relatedMessageId": "1"}));
        assert_eq!((fwd.reply_to(), fwd.is_forwarded()), (None, true));
        let meta_only = msg(json!({"id": "4", "relatedMessageId": "1",
                                   "contentMetadata": {"message_relation_type_code": "reply"}}));
        assert_eq!(meta_only.reply_to(), Some("1"));
        let plain = msg(json!({"id": "5", "messageRelationType": 0}));
        assert_eq!((plain.reply_to(), plain.is_forwarded()), (None, false));
    }

    #[test]
    fn contact_picture_falls_back_to_picture_status() {
        let c: Contact = serde_json::from_value(json!({"mid":"U1","picturePath":"/0habc"})).unwrap();
        assert_eq!(c.picture().as_deref(), Some("/0habc"));
        let c: Contact =
            serde_json::from_value(json!({"mid":"U1","picturePath":"","pictureStatus":"0hold"})).unwrap();
        assert_eq!(c.picture().as_deref(), Some("/0hold"));
        // Live shape for someone with no picture: an empty path, no status.
        let c: Contact = serde_json::from_value(json!({"mid":"U1","picturePath":" "})).unwrap();
        assert_eq!(c.picture(), None);
    }

    #[test]
    fn chat_picture_prefers_picture_path() {
        let c: Chat = serde_json::from_value(json!({"chatMid":"C1","picturePath":"/0habc"})).unwrap();
        assert_eq!(c.picture().as_deref(), Some("/0habc"));
        let c: Chat = serde_json::from_value(json!({"chatMid":"C1","pictureStatus":"0hold"})).unwrap();
        assert_eq!(c.picture().as_deref(), Some("/0hold"));
        let c: Chat = serde_json::from_value(json!({"chatMid":"C1","picturePath":""})).unwrap();
        assert_eq!(c.picture(), None);
    }

    #[test]
    fn text_message_wire_shape() {
        let m = OutgoingMessage::text("Cabc", "hi").to_json();
        assert_eq!(
            m,
            json!({"to":"Cabc","toType":2,"contentMetadata":{},"sessionId":0,"text":"hi","contentType":0,"hasContent":false})
        );
    }

    #[test]
    fn sticker_metadata() {
        let m = OutgoingMessage::sticker("u1", "11537", "52002734", 1).to_json();
        assert_eq!(m["contentType"], 7);
        assert_eq!(
            m["contentMetadata"],
            json!({"STKID":"52002734","STKPKGID":"11537","STKVER":"1"})
        );
    }

    #[test]
    fn mid_types() {
        assert_eq!(MidType::of("U0123"), MidType::User);
        assert_eq!(MidType::of("c0123"), MidType::Group);
        assert_eq!(MidType::of("R0123"), MidType::Room);
        assert_eq!(MidType::of(""), MidType::User);
    }

    #[test]
    fn operation_with_string_int64s() {
        let op: Operation = serde_json::from_value(json!({
            "revision": "123456789012", "type": 26, "reqSeq": -1,
            "message": {"id": "555", "from": "uA", "to": "uB", "toType": 0, "text": "yo",
                        "contentType": 0, "contentMetadata": {}, "createdTime": "1700000000000",
                        "chunks": null, "unknownField": 1}
        }))
        .unwrap();
        assert_eq!(op.revision, 123456789012);
        let m = op.message.unwrap();
        assert_eq!(m.created_time, Some(1_700_000_000_000));
        assert_eq!(m.chat_mid("uB"), "uA");
        assert_eq!(m.extra["unknownField"], 1);
        assert!(!m.is_e2ee());
    }

    #[test]
    fn contact_name_override() {
        let c: Contact =
            serde_json::from_value(json!({"mid":"u1","displayName":"A","displayNameOverridden":"Bee"}))
                .unwrap();
        assert_eq!(c.name(), "Bee");
    }

    fn call_msg(meta: Value) -> Message {
        serde_json::from_value(json!({"id": "9", "contentType": 6, "contentMetadata": meta})).unwrap()
    }

    #[test]
    fn one_to_one_call_records() {
        let c = call_msg(json!({"TYPE": "A", "RESULT": "INFO", "DURATION": "192000", "CAUSE": "16"}))
            .call()
            .unwrap();
        assert_eq!(
            c,
            CallRecord {
                group: false,
                media: CallMedia::Voice,
                outcome: CallOutcome::Answered,
                duration_ms: Some(192_000)
            }
        );
        assert_eq!(c.to_string(), "Voice call · 3:12");

        let video = |meta| call_msg(meta).call().unwrap().to_string();
        assert_eq!(
            video(json!({"TYPE": "V", "DURATION": "3723000"})),
            "Video call · 1:02:03"
        );
        assert_eq!(
            video(json!({"TYPE": "V", "RESULT": "INFO", "DURATION": "0"})),
            "Video call · not answered"
        );
        assert_eq!(
            video(json!({"TYPE": "A", "RESULT": "CANCELED"})),
            "Voice call · cancelled"
        );
        assert_eq!(
            video(json!({"TYPE": "A", "RESULT": "REJECTED"})),
            "Voice call · declined"
        );
        assert_eq!(video(json!({"TYPE": "a", "RESULT": "busy"})), "Voice call · busy");
        assert_eq!(
            video(json!({"TYPE": "A", "RESULT": "NO_ANSWER"})),
            "Voice call · not answered"
        );
        // Unknown values are kept, not guessed.
        assert_eq!(video(json!({"RESULT": "WEIRD"})), "Voice call · WEIRD");
        assert_eq!(
            video(json!({"RESULT": "WEIRD", "DURATION": "5000"})),
            "Voice call · 0:05"
        );
        assert_eq!(video(json!({})), "Voice call · not answered");
    }

    #[test]
    fn group_call_records() {
        let rec = |meta| call_msg(meta).call().unwrap();
        let started = rec(json!({"GC_EVT_TYPE": "S", "GC_MEDIA_TYPE": "AUDIO", "GC_CHAT_MID": "c1"}));
        assert!(started.group);
        assert_eq!(started.outcome, CallOutcome::GroupStarted);
        assert_eq!(started.to_string(), "Group voice call started");
        assert_eq!(
            rec(json!({"GC_EVT_TYPE": "E", "GC_MEDIA_TYPE": "VIDEO", "DURATION": "61000"})).to_string(),
            "Group video call ended · 1:01"
        );
        assert_eq!(
            rec(json!({"GC_EVT_TYPE": "E", "GC_MEDIA_TYPE": "AUDIO"})).to_string(),
            "Group voice call ended"
        );
        assert_eq!(
            rec(json!({"GC_EVT_TYPE": "I", "GC_MEDIA_TYPE": "VIDEO"})).to_string(),
            "Invited to a group video call"
        );
        assert_eq!(
            rec(json!({"GC_EVT_TYPE": "S", "GC_MEDIA_TYPE": "LIVE"})).to_string(),
            "Live broadcast started"
        );
        assert_eq!(
            rec(json!({"GC_EVT_TYPE": "X"})).outcome,
            CallOutcome::Other("X".into())
        );
    }

    #[test]
    fn reaction_emoji() {
        assert_eq!(ReactionKind::Predefined(2).emoji(), Some("👍"));
        assert_eq!(ReactionKind::Predefined(99).emoji(), None);
        let basic = |id: &str| ReactionKind::Paid {
            product_id: BASIC_EMOJI_PACKAGE.into(),
            emoji_id: id.into(),
        };
        // Checked against the CDN's pictures and alt texts.
        for (id, glyph) in [
            ("077", "🙂"),
            ("099", "😰"),
            ("143", "👍"),
            ("165", "❤️"),
            ("201", "❌"),
            ("211", "９"),
        ] {
            assert_eq!(basic(id).emoji(), Some(glyph), "{id}");
        }
        // LINE characters, out of range, other packages: no stand-in.
        for id in ["001", "076", "212", "x"] {
            assert_eq!(basic(id).emoji(), None, "{id}");
        }
        let other = ReactionKind::Paid {
            product_id: "5ac1bfd5040ab15980c9b435".into(),
            emoji_id: "143".into(),
        };
        assert_eq!(other.emoji(), None);
    }

    #[test]
    fn reactions_from_live_shapes() {
        let m: Message = serde_json::from_value(json!({"id": "1", "reactions": [
            {"atMillis": "1794528887011", "fromUserMid": "Ua", "reactionType": {"paidReactionType":
                {"emojiId": "099", "productId": "670e", "resourceType": 1, "version": "3"}}},
            {"atMillis": 5, "fromUserMid": "Ub", "reactionType": {"predefinedReactionType": 3}}
        ]}))
        .unwrap();
        let mut r = m.reactions();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].at, 1_794_528_887_011);
        assert_eq!(
            r[0].kind,
            ReactionKind::Paid {
                product_id: "670e".into(),
                emoji_id: "099".into()
            }
        );
        assert_eq!(r[1].kind, ReactionKind::Predefined(3));
        apply_reaction(&mut r, "Ub", Some(ReactionKind::Predefined(7)), 9);
        apply_reaction(&mut r, "Ua", None, 9);
        assert_eq!(
            r,
            [MessageReaction {
                from: "Ub".into(),
                at: 9,
                kind: ReactionKind::Predefined(7)
            }]
        );
        assert_eq!(MessageReaction::from_json(&r[0].to_json()), Some(r[0].clone()));
    }

    #[test]
    fn animated_stickers() {
        let m = |opt: Option<&str>| -> Message {
            let mut v = json!({"id": "1", "contentType": 7, "contentMetadata": {"STKID": "52002739"}});
            if let Some(o) = opt {
                v["contentMetadata"]["STKOPT"] = json!(o);
            }
            serde_json::from_value(v).unwrap()
        };
        assert!(m(Some("A")).is_animated_sticker());
        assert!(!m(Some("0")).is_animated_sticker());
        assert!(!m(None).is_animated_sticker());
    }

    #[test]
    fn mute_flags() {
        let c: Chat = serde_json::from_value(json!({"chatMid": "C1", "notificationDisabled": true})).unwrap();
        assert!(c.notifications_disabled());
        let u: Contact = serde_json::from_value(json!({"mid": "U1", "settings": "1"})).unwrap();
        assert!(u.notifications_disabled());
        let u: Contact = serde_json::from_value(json!({"mid": "U1", "settings": "8"})).unwrap();
        assert!(!u.notifications_disabled());
    }

    #[test]
    fn only_call_messages_are_calls() {
        let mut m = call_msg(json!({"TYPE": "V"}));
        m.content_type = content_type::TEXT;
        assert!(m.call().is_none());
        assert_eq!(format_call_duration(59_999), "0:59");
    }
}
