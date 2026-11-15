//! Group albums, as far as the Chrome protocol reaches them.
//!
//! The extension has no album browser: the gateway serves no album API (only
//! the three timeline paths in [`crate::timeline`]), so albums cannot be
//! listed, created or edited. What a chat does carry:
//!
//! * **Album posts** — `POSTNOTIFICATION` messages (content type 16) with
//!   `serviceType` `AB`, sent when someone creates an album (`locKey` `BA`)
//!   or adds to one (`BT`). Their metadata names the album (`albumName`,
//!   `albumId`/`cafeId`, `postEndUrl` with the group `homeId`) and the
//!   photos (`previewMedias`: `[{svc:"album", sid:"a", mediaOid, mediaType}]`,
//!   plus `mediaOid`/`mediaType`/`width`/`height` of the first one). Layout
//!   read off live history (2026-11-14).
//! * **Album events** — chat events (content type 18) with `LOC_KEY` `BB`
//!   (renamed: `LOC_ARGS` old␞new), `BD` (deleted: name) or `BO` (items
//!   removed: name); the extension renders them as system lines
//!   (`chat.msg.album.rename|delete|removephoto`).
//!
//! **Photos** are OBS objects `/r/album/a/{mediaOid}`. The extension never
//! downloads them; live testing found they need the timeline channel token
//! plus `X-Line-Mid: <group mid>` (the encrypted access token, or no
//! `X-Line-Mid`, gets 403; `/w580`-style size suffixes get 400).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::Message;

/// `contentType` of album posts (and other timeline notifications).
pub const POST_NOTIFICATION: i64 = 16;
/// `contentType` of chat events (album events among them).
pub const CHAT_EVENT: i64 = 18;

/// One photo or video in an album post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumMedia {
    /// `mediaOid`.
    pub oid: String,
    /// `I` (image) or `V` (video).
    pub media_type: String,
    /// OBS service and SID (`album` / `a`).
    pub svc: String,
    pub sid: String,
}

impl AlbumMedia {
    pub fn is_video(&self) -> bool {
        self.media_type.eq_ignore_ascii_case("V")
    }

    /// `/r/{svc}/{sid}/{oid}` on the OBS host.
    pub fn path(&self) -> String {
        format!("/r/{}/{}/{}", self.svc, self.sid, self.oid)
    }
}

/// What an album post announces (`locKey`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumPostKind {
    /// `BA`: the album was created (with its first photos).
    Created,
    /// `BT`: photos were added.
    Added,
    Other(String),
}

/// An album post: someone created or added to a group album.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumPost {
    pub kind: AlbumPostKind,
    /// `albumId` from `postEndUrl` (or `cafeId`).
    pub album_id: String,
    pub album_name: String,
    /// The group (`chatId`).
    pub chat: String,
    /// The group's timeline home id (from `postEndUrl`).
    pub home_id: Option<String>,
    /// The photos shown in the chat (the first few of the post).
    pub media: Vec<AlbumMedia>,
    /// Size of the first photo, when given.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `line://group/home/albums/album?…`, opens the album in LINE's apps.
    pub url: Option<String>,
}

/// A change to an album, from a chat event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumEvent {
    /// `BB`
    Renamed { from: String, to: String },
    /// `BD`
    Deleted { name: String },
    /// `BO`: one or more items were removed.
    ItemsRemoved { name: String },
}

/// The query parameter `key` of a `line://…?a=b&c=d` URL.
fn url_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    url::form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

impl AlbumPost {
    pub fn from_message(m: &Message) -> Option<Self> {
        let meta = &m.content_metadata;
        if m.content_type != POST_NOTIFICATION || meta.get("serviceType").map(String::as_str) != Some("AB") {
            return None;
        }
        let url = meta.get("postEndUrl").filter(|u| !u.is_empty()).cloned();
        let album_id = url
            .as_deref()
            .and_then(|u| url_param(u, "albumId").or_else(|| url_param(u, "albumIdV2")))
            .or_else(|| meta.get("albumId").cloned())
            .or_else(|| meta.get("cafeId").cloned())
            .filter(|s| !s.is_empty())?;
        let mut media: Vec<AlbumMedia> = meta
            .get("previewMedias")
            .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                let oid = s("mediaOid");
                (!oid.is_empty()).then(|| AlbumMedia {
                    oid,
                    media_type: s("mediaType"),
                    svc: Some(s("svc"))
                        .filter(|x| !x.is_empty())
                        .unwrap_or_else(|| "album".into()),
                    sid: Some(s("sid"))
                        .filter(|x| !x.is_empty())
                        .unwrap_or_else(|| "a".into()),
                })
            })
            .collect();
        if media.is_empty()
            && let Some(oid) = meta.get("mediaOid").filter(|o| !o.is_empty())
        {
            media.push(AlbumMedia {
                oid: oid.clone(),
                media_type: meta.get("mediaType").cloned().unwrap_or_else(|| "I".into()),
                svc: "album".into(),
                sid: "a".into(),
            });
        }
        let num = |k: &str| meta.get(k).and_then(|v| v.parse().ok()).filter(|n| *n > 0);
        Some(AlbumPost {
            kind: match meta.get("locKey").map(String::as_str) {
                Some("BA") => AlbumPostKind::Created,
                Some("BT") => AlbumPostKind::Added,
                other => AlbumPostKind::Other(other.unwrap_or_default().to_owned()),
            },
            album_id,
            album_name: meta.get("albumName").cloned().unwrap_or_default(),
            chat: meta
                .get("chatId")
                .filter(|c| !c.is_empty())
                .cloned()
                .unwrap_or_else(|| m.to.clone()),
            home_id: url.as_deref().and_then(|u| url_param(u, "homeId")),
            media,
            width: num("width"),
            height: num("height"),
            url,
        })
    }
}

impl AlbumEvent {
    pub fn from_message(m: &Message) -> Option<Self> {
        if m.content_type != CHAT_EVENT {
            return None;
        }
        let meta = &m.content_metadata;
        // `VR`: arguments split by U+001E, each a U+001F-separated list.
        let args: Vec<String> = meta
            .get("LOC_ARGS")
            .map(|a| {
                a.split('\u{1e}')
                    .map(|x| x.split('\u{1f}').next().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let arg = |i: usize| args.get(i).cloned();
        match meta.get("LOC_KEY")?.as_str() {
            "BB" if args.len() >= 2 => Some(AlbumEvent::Renamed {
                from: arg(0)?,
                to: arg(1)?,
            }),
            "BD" => Some(AlbumEvent::Deleted { name: arg(0)? }),
            "BO" => Some(AlbumEvent::ItemsRemoved { name: arg(0)? }),
            _ => None,
        }
    }

    /// The extension's English wording, without the actor (prefix a name).
    pub fn describe(&self) -> String {
        match self {
            AlbumEvent::Renamed { from, to } => {
                format!("changed the name of the album \"{from}\" to \"{to}\"")
            }
            AlbumEvent::Deleted { name } => format!("deleted the album \"{name}\""),
            AlbumEvent::ItemsRemoved { name } => {
                format!("deleted one or more items from the album \"{name}\"")
            }
        }
    }
}

impl Message {
    /// The album post this message announces (content type 16, `serviceType` `AB`).
    pub fn album_post(&self) -> Option<AlbumPost> {
        AlbumPost::from_message(self)
    }

    /// The album change this chat event records (`LOC_KEY` `BB`/`BD`/`BO`).
    pub fn album_event(&self) -> Option<AlbumEvent> {
        AlbumEvent::from_message(self)
    }
}

impl Client {
    /// Download a photo (or video) of a group album. `chat` is the group mid
    /// ([`AlbumPost::chat`]); LINE checks that you are a member.
    pub async fn download_album_media(&self, chat: &str, media: &AlbumMedia) -> Result<Vec<u8>> {
        if chat.is_empty() {
            return Err(Error::Protocol("album media needs the group mid".into()));
        }
        let url = format!("{}{}", self.t.config.obs, media.path());
        let resp = self
            .myhome_request(None, &url, &[("x-line-mid", chat.to_owned())])
            .await?;
        crate::media::read_body(resp, crate::media::MAX_DOWNLOAD).await
    }

    /// The first photo of an album post (what [`Client::download_media`]
    /// returns for one).
    pub async fn download_album_post(&self, post: &AlbumPost) -> Result<Vec<u8>> {
        let media = post
            .media
            .first()
            .ok_or_else(|| Error::MediaUnavailable("the album post names no photo".into()))?;
        self.download_album_media(&post.chat, media).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn msg(ct: i64, meta: &[(&str, &str)]) -> Message {
        Message {
            content_type: ct,
            to: "cGROUP".into(),
            content_metadata: meta
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_live_album_post() {
        // Metadata of a live album post (2026-11-14), ids shortened.
        let m = msg(
            16,
            &[
                ("albumName", "ค่ารถรด. 10/11/26"),
                ("cafeId", "5344"),
                ("chatId", "cde01"),
                ("contentType", "P"),
                ("height", "1446"),
                ("locKey", "BA"),
                ("mediaCount", "0"),
                ("mediaOid", "2e4e.26111015"),
                ("mediaType", "I"),
                (
                    "postEndUrl",
                    "line://group/home/albums/album?albumId=5344&homeId=CzER&albumIdV2=5344",
                ),
                (
                    "previewMedias",
                    r#"[{"svc":"album","sid":"a","mediaOid":"2e4e.26111015","mediaType":"I"}]"#,
                ),
                ("serviceType", "AB"),
                ("width", "1110"),
            ],
        );
        let p = m.album_post().unwrap();
        assert_eq!(p.kind, AlbumPostKind::Created);
        assert_eq!(p.album_id, "5344");
        assert_eq!(p.album_name, "ค่ารถรด. 10/11/26");
        assert_eq!(p.chat, "cde01");
        assert_eq!(p.home_id.as_deref(), Some("CzER"));
        assert_eq!((p.width, p.height), (Some(1110), Some(1446)));
        assert_eq!(p.media.len(), 1);
        assert_eq!(p.media[0].path(), "/r/album/a/2e4e.26111015");
        assert!(!p.media[0].is_video());
    }

    #[test]
    fn falls_back_to_first_media_fields() {
        let m = msg(
            16,
            &[
                ("serviceType", "AB"),
                ("locKey", "BT"),
                ("albumId", "9"),
                ("mediaOid", "abc"),
                ("mediaType", "V"),
            ],
        );
        let p = m.album_post().unwrap();
        assert_eq!(p.kind, AlbumPostKind::Added);
        assert_eq!(p.album_id, "9");
        assert_eq!(p.chat, "cGROUP", "defaults to the message's chat");
        assert!(p.media[0].is_video());
    }

    #[test]
    fn other_post_notifications_are_not_albums() {
        assert!(
            msg(16, &[("serviceType", "GB"), ("albumId", "1")])
                .album_post()
                .is_none()
        );
        assert!(
            msg(0, &[("serviceType", "AB"), ("albumId", "1")])
                .album_post()
                .is_none()
        );
    }

    #[test]
    fn parses_album_events() {
        let r = msg(18, &[("LOC_KEY", "BB"), ("LOC_ARGS", "Trip\u{1e}Trip 2026")]);
        assert_eq!(
            r.album_event(),
            Some(AlbumEvent::Renamed {
                from: "Trip".into(),
                to: "Trip 2026".into()
            })
        );
        assert_eq!(
            r.album_event().unwrap().describe(),
            "changed the name of the album \"Trip\" to \"Trip 2026\""
        );
        let d = msg(18, &[("LOC_KEY", "BD"), ("LOC_ARGS", "Trip")]);
        assert_eq!(d.album_event(), Some(AlbumEvent::Deleted { name: "Trip".into() }));
        let o = msg(18, &[("LOC_KEY", "BO"), ("LOC_ARGS", "Trip")]);
        assert_eq!(
            o.album_event(),
            Some(AlbumEvent::ItemsRemoved { name: "Trip".into() })
        );
        // Membership events are not album events.
        assert!(
            msg(18, &[("LOC_KEY", "C_MI"), ("LOC_ARGS", "u1\u{1e}u2")])
                .album_event()
                .is_none()
        );
    }
}
