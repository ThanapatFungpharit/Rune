//! Timeline ("myhome") services behind the gateway: profile covers, and the
//! object storage that album photos live in.
//!
//! From the extension bundle (`static/js/main.js`, minified names):
//!
//! * **Channel token** (`getChannelAccessToken` / `renewChannelToken`):
//!   `ChannelService/issueChannelToken(["1341209850"])` →
//!   `{channelAccessToken, expiration, …}`, renewed once `expiration` (ms)
//!   has passed.
//! * **Gateway** (`zU`'s `headerMapper`): requests to `/api/timeline/…` carry
//!   `X-Line-ChannelToken` on top of the usual signed gateway headers, and
//!   answer with the usual `{"message":"OK","data":…}` envelope. The gateway
//!   serves only the three paths the extension uses: `getCover`,
//!   `updateCover` and `homeId` (anything else is `REQUEST_NOT_FOUND`).
//! * **Cover** (`yz`): `GET /api/timeline/getCover?targetMid&myMid&myRegion`
//!   → `{coverObsInfo: {serviceName, obsNamespace, objectId}, isDefaultCover,
//!   videoCoverObsInfo?, profileId, meta}`; the picture is the OBS object
//!   `/r/{serviceName}/{obsNamespace}/{objectId}` (`WO`).
//! * **Cover upload** (`KJ`): POST the image to `/r/myhome/c/reqid-{uuid}`
//!   (`X-Obs-Params: {"ver":"2.0","name":<file name>,"type":"image"}`), then
//!   `POST /api/timeline/updateCover {myMid, myRegion, coverObjectId:
//!   "reqid-…", storyShare: false}` → `{coverObsInfo}`.
//! * **OBS auth** (`mo`/`Sg`): object paths containing `/r/myhome/` use
//!   `X-Line-ChannelToken` instead of the encrypted access token; a 401 renews
//!   the channel token once.
//!
//! Album photos (`/r/album/a/{oid}`) are not fetched by the extension at all;
//! live testing found they need the channel token plus `X-Line-Mid: <group
//! mid>` (see [`crate::album`]).

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::Client;
use crate::error::{ApiError, Error, Result};
use crate::models::{MidType, value_i64};

/// The timeline channel the extension issues its channel token for.
pub const TIMELINE_CHANNEL_ID: &str = "1341209850";

#[derive(Default)]
pub(crate) struct TimelineState {
    /// `(channelAccessToken, expiration in epoch ms)`.
    channel_token: Option<(String, i64)>,
    /// Our `regionCode`, which `getCover` / `updateCover` require.
    region: Option<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// An object in LINE's object storage, as the timeline API names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObsObject {
    /// `serviceName`, e.g. `myhome`.
    pub service_name: String,
    /// `obsNamespace` (the SID), e.g. `c` (cover) or `vc` (video cover).
    pub obs_namespace: String,
    pub object_id: String,
}

impl ObsObject {
    /// `/r/{service}/{sid}/{oid}` on the OBS host.
    pub fn path(&self) -> String {
        format!(
            "/r/{}/{}/{}",
            self.service_name, self.obs_namespace, self.object_id
        )
    }
}

/// A profile's cover (`getCover`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCover {
    /// The cover picture (`coverObsInfo`).
    #[serde(rename = "coverObsInfo")]
    pub image: Option<ObsObject>,
    /// A video cover, when the user set one (`videoCoverObsInfo`); `image`
    /// is then its still frame.
    #[serde(rename = "videoCoverObsInfo")]
    pub video: Option<ObsObject>,
    /// The user never set a cover: `image` is LINE's stock picture.
    #[serde(default)]
    pub is_default_cover: bool,
    pub profile_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ProfileCover {
    /// The cover picture worth showing: `None` for LINE's default cover.
    pub fn custom_image(&self) -> Option<&ObsObject> {
        self.image.as_ref().filter(|_| !self.is_default_cover)
    }
}

impl Client {
    /// The timeline channel access token (cached until shortly before it expires).
    pub(crate) async fn channel_token(&self, renew: bool) -> Result<String> {
        if !renew
            && let Some((t, exp)) = self.timeline.lock().unwrap().channel_token.clone()
            && exp > now_ms() + 60_000
        {
            return Ok(t);
        }
        let v = self
            .call(
                "talk/thrift/Talk/ChannelService/issueChannelToken",
                json!([TIMELINE_CHANNEL_ID]),
            )
            .await?;
        let tok = v
            .get("channelAccessToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Protocol("issueChannelToken returned no channelAccessToken".into()))?
            .to_owned();
        // Without an expiration, assume a short life so we ask again soon.
        let exp = v
            .get("expiration")
            .and_then(value_i64)
            .unwrap_or_else(|| now_ms() + 10 * 60_000);
        self.timeline.lock().unwrap().channel_token = Some((tok.clone(), exp));
        Ok(tok)
    }

    async fn channel_header(&self, renew: bool) -> Result<HeaderValue> {
        let mut v = HeaderValue::from_str(&self.channel_token(renew).await?)
            .map_err(|e| Error::Protocol(e.to_string()))?;
        v.set_sensitive(true);
        Ok(v)
    }

    /// Call a timeline endpoint: `GET` (`body` is `None`, `query` is appended)
    /// or `POST` (JSON `body`) to `/api/timeline/{path}`, and unwrap the
    /// `{message, data}` envelope. Renews the channel token once on 401 and
    /// the access token once when LINE asks for it.
    pub async fn timeline_call(
        &self,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut full = format!("/api/timeline/{}", path.trim_start_matches('/'));
        if !query.is_empty() {
            let q = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(query)
                .finish();
            full = format!("{full}?{q}");
        }
        let (mut renewed_channel, mut refreshed) = (false, false);
        loop {
            let mut h = HeaderMap::new();
            h.insert("x-line-channeltoken", self.channel_header(renewed_channel).await?);
            let resp = self.t.gateway_request(&full, body, h, None).await?;
            let status = resp.status().as_u16();
            let text = resp.text().await?;
            match crate::transport::decode(&full, status, &text) {
                Ok(v) => return Ok(v),
                Err(e) if e.must_refresh_token() && !refreshed => {
                    refreshed = true;
                    self.refresh_token().await?;
                }
                Err(e) if status == 401 && !renewed_channel => {
                    tracing::debug!("timeline 401, renewing the channel token: {e}");
                    renewed_channel = true;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Our region code (from the profile; cached).
    async fn region(&self) -> Result<String> {
        if let Some(r) = self.timeline.lock().unwrap().region.clone() {
            return Ok(r);
        }
        let r = self
            .profile()
            .await?
            .region_code
            .filter(|r| !r.is_empty())
            .ok_or_else(|| Error::Protocol("the profile has no regionCode".into()))?;
        self.timeline.lock().unwrap().region = Some(r.clone());
        Ok(r)
    }

    /// A user's profile cover (yours or anyone else's). Groups have none.
    pub async fn profile_cover(&self, mid: &str) -> Result<ProfileCover> {
        if MidType::of(mid) != MidType::User {
            return Err(Error::Protocol("only users have profile covers".into()));
        }
        let me = self.my_mid().ok_or(Error::LoginRequired)?;
        let region = self.region().await?;
        let v = self
            .timeline_call(
                "getCover",
                &[("targetMid", mid), ("myMid", &me), ("myRegion", &region)],
                None,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Download the cover picture (`None` when the user has LINE's default cover).
    pub async fn download_cover(&self, cover: &ProfileCover) -> Result<Option<Vec<u8>>> {
        match cover.custom_image() {
            Some(o) => Ok(Some(self.download_obs(&o.path()).await?)),
            None => Ok(None),
        }
    }

    /// Replace your profile cover with a JPEG/PNG picture (metadata such as
    /// GPS is stripped first). Visible to everyone who can see your profile.
    pub async fn set_profile_cover(&self, image: &[u8], file_name: &str) -> Result<ObsObject> {
        let me = self.my_mid().ok_or(Error::LoginRequired)?;
        let region = self.region().await?;
        let oid = format!("reqid-{}", crate::media::random_uuid());
        let data = crate::exif::scrub(image);
        let params = B64.encode(json!({ "ver": "2.0", "name": file_name, "type": "image" }).to_string());
        self.myhome_request(
            Some((&data, params)),
            &format!("{}/r/myhome/c/{oid}", self.t.config.obs),
            &[],
        )
        .await?;
        let v = self
            .timeline_call(
                "updateCover",
                &[],
                Some(&json!({
                    "myMid": me,
                    "myRegion": region,
                    "coverObjectId": oid,
                    "storyShare": false,
                })),
            )
            .await?;
        let info = v
            .get("coverObsInfo")
            .cloned()
            .ok_or_else(|| Error::Protocol("updateCover returned no coverObsInfo".into()))?;
        Ok(serde_json::from_value(info)?)
    }

    /// The timeline home id of a user (`homeId?eMid=`).
    pub async fn timeline_home_id(&self, mid: &str) -> Result<String> {
        let v = self.timeline_call("homeId", &[("eMid", mid)], None).await?;
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("homeId returned no id".into()))
    }

    /// GET an OBS object by path (`/r/{service}/{sid}/{oid}[/{tid}]`) or full
    /// URL, authenticating like the extension: `/r/myhome/` with the channel
    /// token, everything else with the encrypted access token.
    pub async fn download_obs(&self, path: &str) -> Result<Vec<u8>> {
        let url = if path.starts_with("http") {
            path.to_owned()
        } else {
            format!("{}{path}", self.t.config.obs)
        };
        let resp = if url.contains("/r/myhome/") {
            self.myhome_request(None, &url, &[]).await?
        } else {
            self.obs_request(None, &url, &[]).await?
        };
        Ok(resp.bytes().await?.to_vec())
    }

    /// One OBS request authenticated with the channel token (renewed once on 401).
    pub(crate) async fn myhome_request(
        &self,
        post: Option<(&[u8], String)>,
        url: &str,
        extra: &[(&'static str, String)],
    ) -> Result<reqwest::Response> {
        let mut renewed = false;
        loop {
            let mut h = HeaderMap::new();
            h.insert("x-line-channeltoken", self.channel_header(renewed).await?);
            for (k, v) in extra {
                h.insert(
                    *k,
                    HeaderValue::from_str(v).map_err(|e| Error::Protocol(e.to_string()))?,
                );
            }
            let req = match &post {
                Some((body, params)) => {
                    h.insert(
                        "x-obs-params",
                        HeaderValue::from_str(params).map_err(|e| Error::Protocol(e.to_string()))?,
                    );
                    self.t.http.post(url).body(body.to_vec())
                }
                None => self.t.http.get(url),
            };
            let short = url.split('?').next().unwrap_or(url).to_owned();
            tracing::debug!(url = %short, post = post.is_some(), "OBS (channel token)");
            let resp = req.headers(h).timeout(Duration::from_secs(300)).send().await?;
            let status = resp.status().as_u16();
            if status == 401 && !renewed {
                renewed = true;
                continue;
            }
            if status == 404 {
                return Err(Error::MediaUnavailable(format!("{short} not found")));
            }
            if status >= 400 {
                let body = resp.text().await.unwrap_or_default();
                return Err(ApiError {
                    path: short,
                    status,
                    gateway_code: None,
                    talk_code: None,
                    reason: Some(format!("OBS HTTP {status}")),
                    raw: Value::String(body.chars().take(500).collect()),
                }
                .into());
            }
            return Ok(resp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_parses_live_shapes() {
        // Shapes as returned live (2026-11-14), ids shortened.
        let c: ProfileCover = serde_json::from_value(json!({
            "coverObsInfo": {"objectId": "7489c3", "obsNamespace": "c", "serviceName": "myhome"},
            "isDefaultCover": false, "meta": {}, "profileId": "w9dv"
        }))
        .unwrap();
        assert_eq!(c.custom_image().unwrap().path(), "/r/myhome/c/7489c3");
        assert!(c.video.is_none());

        let d: ProfileCover = serde_json::from_value(json!({
            "coverObsInfo": {"objectId": "c0000000000000000000000000000001", "obsNamespace": "c", "serviceName": "myhome"},
            "isDefaultCover": true, "meta": {}, "profileId": "NN73"
        }))
        .unwrap();
        assert!(d.custom_image().is_none());

        let v: ProfileCover = serde_json::from_value(json!({
            "coverObsInfo": {"objectId": "ab21", "obsNamespace": "c", "serviceName": "myhome"},
            "videoCoverObsInfo": {"objectId": "ab21", "obsNamespace": "vc", "serviceName": "myhome"},
            "isDefaultCover": false, "meta": {}, "profileId": "HCnS"
        }))
        .unwrap();
        assert_eq!(v.video.unwrap().path(), "/r/myhome/vc/ab21");
    }
}
