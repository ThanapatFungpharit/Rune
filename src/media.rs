//! Media: images, video, audio and files (object storage, "OBS"), profile
//! pictures and sticker images.
//!
//! Everything here follows the extension bundle (`ltsmSandbox.js` ships the
//! app code; function names below are its minified ones):
//!
//! * **Auth** (`mo`): OBS requests carry `X-Line-Access: <encrypted access
//!   token for OBS_GENERAL>` (from `acquireEncryptedAccessToken(2)`) plus
//!   `X-Line-Application`; no `X-Hmac`. On 401/403 the token is renewed once.
//! * **Paths** (`yb`/`vb`): sealed media `/r/talk/{SID}/{OID}` (preview
//!   `{OID}__ud-preview`); plain media `/r/talk/m/{messageId}` with `/preview`
//!   or `/original`; `?p={OBS_POP}` when the message has it; bot media uses
//!   public `DOWNLOAD_URL`/`PREVIEW_URL`.
//! * **Sealed media** (`gv`/`vv`/`mv`): 32 random bytes `ENC_KM` → HKDF-SHA256
//!   (empty salt, info `FileEncryption`, 76 bytes) → AES-256-CTR key, HMAC key,
//!   12-byte nonce; counter block `nonce || 0x00000000`, 32-bit counter;
//!   blob = ciphertext || HMAC-SHA256. Video tags the concatenated SHA-256 of
//!   each 128 KiB ciphertext chunk instead (that list is also uploaded as
//!   `{OID}__ud-hash`). `ENC_KM` and `FILE_NAME` travel inside the sealed message.
//! * **Upload V2** (`nE`/`oE`): POST the encrypted blob to
//!   `/r/talk/{emi|emv|ema|emf}/reqid-{uuid}` with
//!   `X-Obs-Params: base64({"ver":"2.0","name":"<ms timestamp>","type":"file"})`;
//!   the server answers with the real id in `x-obs-oid`; then the preview and
//!   chunk hashes; then the sealed message with `SID`, `OID`, `FILE_SIZE`.
//! * **Upload V1** (`eE`): send the (plain) message first, then POST the bytes
//!   to `/r/talk/m/{messageId}` with type `image|video|audio|file`.
//! * **Which flow**: `determineMediaMessageFlow({chatMid}).flowMap[contentType]`
//!   (2 = V2), cached for `cacheTtlMillis`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::Aes256;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ctr::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::models::{Message, OutgoingMessage, content_type, value_i64};
use crate::transport::APPLICATION_HEADER;

/// Object storage host (`getServerBaseUrl("obs")`).
pub const OBS_HOST: &str = "https://obs.line-apps.com";
/// Profile / group picture CDN (public).
pub const PROFILE_CDN: &str = "https://profile.line-scdn.net";
/// Sticker CDN (public).
pub const STICKER_CDN: &str = "https://stickershop.line-scdn.net";

const CHUNK: usize = 128 * 1024;
const TAG_LEN: usize = 32;
/// The extension refuses files above 1 GiB (`O = 1 * 2**30`).
const MAX_FILE: usize = 1 << 30;
/// Images above 20 MiB are sent as files (`Hb`).
const MAX_IMAGE: usize = 20 * 1024 * 1024;
/// Largest download accepted: a 1 GiB file plus its sealing overhead
/// (the HMAC tag, or a video's per-chunk hash list).
pub(crate) const MAX_DOWNLOAD: usize = MAX_FILE + (MAX_FILE / CHUNK + 1) * TAG_LEN;
/// Largest preview, profile picture or sticker accepted.
pub(crate) const MAX_PICTURE_DOWNLOAD: usize = MAX_IMAGE;

/// Read a response body, refusing one larger than `limit` bytes (by its
/// `Content-Length` up front, else as it streams in), so a hostile or broken
/// server cannot exhaust memory.
pub(crate) async fn read_body(resp: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let too_big = |n: u64| {
        Error::Protocol(format!(
            "download refused: {n} bytes exceeds the {limit}-byte limit"
        ))
    };
    if let Some(n) = resp.content_length()
        && n > limit as u64
    {
        return Err(too_big(n));
    }
    // At most 1 MiB reserved up front (the claimed length may not be honest);
    // the rest grows as data actually arrives.
    let mut out = Vec::with_capacity(resp.content_length().unwrap_or(0).min(1 << 20) as usize);
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if out.len() + chunk.len() > limit {
            return Err(too_big((out.len() + chunk.len()) as u64));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

type Aes256Ctr32 = ctr::Ctr32BE<Aes256>;
type HmacSha256 = Hmac<Sha256>;

// -- blob crypto ------------------------------------------------------------------

/// `mv`: HKDF-SHA256(ENC_KM, salt = empty, info = "FileEncryption") → 76 bytes.
fn file_keys(key_material: &[u8]) -> ([u8; 32], [u8; 32], [u8; 12]) {
    let hk = hkdf::Hkdf::<Sha256>::new(None, key_material);
    let mut okm = [0u8; 76];
    hk.expand(b"FileEncryption", &mut okm)
        .expect("76 bytes is a valid HKDF length");
    let mut enc = [0u8; 32];
    let mut mac = [0u8; 32];
    let mut nonce = [0u8; 12];
    enc.copy_from_slice(&okm[..32]);
    mac.copy_from_slice(&okm[32..64]);
    nonce.copy_from_slice(&okm[64..]);
    (enc, mac, nonce)
}

fn ctr_apply(enc: &[u8; 32], nonce: &[u8; 12], data: &mut [u8]) {
    let mut iv = [0u8; 16];
    iv[..12].copy_from_slice(nonce);
    let mut c = Aes256Ctr32::new_from_slices(enc, &iv).expect("key/iv sizes");
    c.apply_keystream(data);
}

/// `bv`: SHA-256 of each 128 KiB chunk, concatenated.
fn chunk_hashes(data: &[u8]) -> Vec<u8> {
    data.chunks(CHUNK)
        .flat_map(|c| Sha256::digest(c).to_vec())
        .collect()
}

fn hmac(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(key).expect("any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// `gv`: encrypt a media blob. Returns `ciphertext || tag` and, when
/// `chunked` (video), the chunk-hash list to upload as `__ud-hash`.
pub fn encrypt_blob(key_material: &[u8], data: &[u8], chunked: bool) -> (Vec<u8>, Option<Vec<u8>>) {
    let (enc, mac, nonce) = file_keys(key_material);
    let mut ct = data.to_vec();
    ctr_apply(&enc, &nonce, &mut ct);
    let (tag, hashes) = if chunked {
        let h = chunk_hashes(&ct);
        (hmac(&mac, &h), Some(h))
    } else {
        (hmac(&mac, &ct), None)
    };
    ct.extend_from_slice(&tag);
    (ct, hashes)
}

/// `vv`: verify and decrypt a media blob.
pub fn decrypt_blob(key_material: &[u8], blob: &[u8], chunked: bool) -> Result<Vec<u8>> {
    if blob.len() < TAG_LEN {
        return Err(Error::e2ee(
            crate::E2eeErrorKind::MalformedMessage,
            "media blob shorter than its tag",
        ));
    }
    let (enc, mac, nonce) = file_keys(key_material);
    let (ct, tag) = blob.split_at(blob.len() - TAG_LEN);
    let expected = if chunked {
        hmac(&mac, &chunk_hashes(ct))
    } else {
        hmac(&mac, ct)
    };
    // Constant-time comparison.
    let diff = expected.iter().zip(tag).fold(0u8, |a, (x, y)| a | (x ^ y));
    if diff != 0 {
        return Err(Error::e2ee(
            crate::E2eeErrorKind::DecryptionFailed,
            "media blob HMAC mismatch",
        ));
    }
    let mut pt = ct.to_vec();
    ctr_apply(&enc, &nonce, &mut pt);
    Ok(pt)
}

/// `SE`: `X-Talk-Meta` for a message id — a tiny Thrift-binary struct
/// (string field 4 = id, empty `list<struct>` field 27), base64, wrapped as
/// `base64({"message": ...})`.
pub fn talk_meta(message_id: &str) -> String {
    let id = message_id.as_bytes();
    let mut b = vec![11u8];
    b.extend_from_slice(&4i16.to_be_bytes());
    b.extend_from_slice(&(id.len() as i32).to_be_bytes());
    b.extend_from_slice(id);
    b.push(15);
    b.extend_from_slice(&27i16.to_be_bytes());
    b.push(12);
    b.extend_from_slice(&0i32.to_be_bytes());
    b.push(0);
    B64.encode(json!({ "message": B64.encode(&b) }).to_string())
}

fn obs_params(name: &str, kind: &str, cat: Option<&str>) -> String {
    let mut v = json!({ "ver": "2.0", "name": name, "type": kind });
    if let Some(c) = cat {
        v["cat"] = json!(c);
    }
    B64.encode(v.to_string())
}

pub(crate) fn random_uuid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

// -- public types -------------------------------------------------------------------

/// What kind of media to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

impl MediaKind {
    fn content_type(self) -> i64 {
        match self {
            MediaKind::Image => content_type::IMAGE,
            MediaKind::Video => content_type::VIDEO,
            MediaKind::Audio => content_type::AUDIO,
            MediaKind::File => content_type::FILE,
        }
    }
    /// `sE`: sealed-media service id.
    fn sealed_sid(self) -> &'static str {
        match self {
            MediaKind::Image => "emi",
            MediaKind::Video => "emv",
            MediaKind::Audio => "ema",
            MediaKind::File => "emf",
        }
    }
    /// `aE`: plain-flow OBS type.
    fn obs_type(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::File => "file",
        }
    }
    /// `Ty`: extension the plain flow forces on non-original media names.
    fn forced_ext(self) -> Option<&'static str> {
        match self {
            MediaKind::Image => Some("jpg"),
            MediaKind::Video => Some("mp4"),
            MediaKind::Audio => Some("m4a"),
            MediaKind::File => None,
        }
    }
}

/// A file to send.
#[derive(Debug, Clone)]
pub struct MediaUpload {
    pub kind: MediaKind,
    pub data: Vec<u8>,
    /// File name shown to the recipient.
    pub name: String,
    /// Video/audio length in milliseconds (`DURATION`).
    pub duration_ms: Option<u64>,
    /// Thumbnail JPEG (video). Images get one generated automatically.
    pub preview: Option<Vec<u8>>,
    /// Images: send the original pixels (`MEDIA_CONTENT_INFO.category =
    /// "original"`) instead of a JPEG of at most 1280×1280 pixels' area.
    /// Metadata is scrubbed losslessly first: GPS, serial numbers, names,
    /// maker notes, thumbnails, XMP and IPTC are removed; orientation,
    /// camera/lens model, capture time and exposure settings are kept
    /// (see the `exif` module).
    pub original: bool,
}

impl MediaUpload {
    pub fn image(data: Vec<u8>, name: impl Into<String>) -> Self {
        Self::new(MediaKind::Image, data, name)
    }
    pub fn file(data: Vec<u8>, name: impl Into<String>) -> Self {
        Self::new(MediaKind::File, data, name)
    }
    /// MP4 video (the extension transcodes to MP4 first; this crate does not).
    pub fn video(data: Vec<u8>, name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            duration_ms: Some(duration_ms),
            ..Self::new(MediaKind::Video, data, name)
        }
    }
    /// M4A audio.
    pub fn audio(data: Vec<u8>, name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            duration_ms: Some(duration_ms),
            ..Self::new(MediaKind::Audio, data, name)
        }
    }
    fn new(kind: MediaKind, data: Vec<u8>, name: impl Into<String>) -> Self {
        MediaUpload {
            kind,
            data,
            name: name.into(),
            duration_ms: None,
            preview: None,
            original: false,
        }
    }
}

/// Profile/group picture sizes on the profile CDN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureSize {
    Full,
    /// `/preview`
    Preview,
    /// `/small`
    Small,
}

/// Public URL of a profile or group picture (`picturePath` from a profile,
/// contact or chat). `None` when there is no picture.
pub fn picture_url(picture_path: &str, size: PictureSize) -> Option<String> {
    let p = picture_path.trim();
    if p.is_empty() {
        return None;
    }
    let base = if p.starts_with("http") {
        p.to_owned()
    } else {
        format!("{PROFILE_CDN}/{}", p.trim_start_matches('/'))
    };
    Some(match size {
        PictureSize::Full => base,
        PictureSize::Preview => format!("{base}/preview"),
        PictureSize::Small => format!("{base}/small"),
    })
}

/// Sticker image variants on the sticker CDN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickerImage {
    /// `android/sticker.png`
    Static,
    /// `iPhone/sticker@2x.png` (higher resolution)
    Static2x,
    /// `android/sticker_key.png` (small)
    Key,
    /// `android/sticker_animation.png`: an APNG, for animated stickers
    /// (`STKOPT` `A`, see [`Message::is_animated_sticker`](crate::Message::is_animated_sticker)).
    Animation,
}

/// Public URL of a sticker image (`STKID` from a sticker message).
pub fn sticker_url(sticker_id: &str, variant: StickerImage) -> String {
    let file = match variant {
        StickerImage::Static => "android/sticker.png",
        StickerImage::Static2x => "iPhone/sticker@2x.png",
        StickerImage::Key => "android/sticker_key.png",
        StickerImage::Animation => "android/sticker_animation.png",
    };
    format!("{STICKER_CDN}/stickershop/v1/sticker/{sticker_id}/{file}")
}

/// Public URL of a sticker package's icon (`STKPKGID`).
pub fn sticker_package_icon_url(package_id: &str) -> String {
    format!("{STICKER_CDN}/stickershop/v1/product/{package_id}/LINEStorePC/main.png")
}

/// LINE emoji ids (`productId`, `emojiId`) are hex / decimal; reactions
/// carry whatever the reactor's client sent, so only plain ids go into URLs.
fn is_plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Public URL of a LINE emoji picture (a paid reaction's `productId` / `emojiId`),
/// `None` for ids that are not plain alphanumerics.
pub fn emoji_url(product_id: &str, emoji_id: &str) -> Option<String> {
    (is_plain_id(product_id) && is_plain_id(emoji_id))
        .then(|| format!("{STICKER_CDN}/sticonshop/v1/sticon/{product_id}/iPhone/{emoji_id}.png"))
}

/// Public URL of a LINE emoji package's `meta.json` (`{productId, orders: [id],
/// altTexts: {id: text}}`, live).
pub(crate) fn emoji_meta_url(product_id: &str) -> Option<String> {
    is_plain_id(product_id)
        .then(|| format!("{STICKER_CDN}/sticonshop/v1/{product_id}/sticon/iphone/meta.json"))
}

// -- image processing (the extension's canvas resizes) ------------------------------

struct PreparedImage {
    data: Vec<u8>,
    preview: Vec<u8>,
    thumb_info: Value,
    content_info: Option<Value>,
    name: String,
}

pub(crate) fn jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // The canvas paints white under transparent pixels before encoding JPEG.
    let rgb = image::DynamicImage::ImageRgb8(flatten_white(img));
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .map_err(|e| Error::Protocol(format!("JPEG encoding failed: {e}")))?;
    Ok(out)
}

fn flatten_white(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    })
}

/// `Cy` "contain" / "maxPixel" sizing with `keepSizeForSmallerImage`.
fn fit(w: u32, h: u32, mode: &str, tw: u32, th: u32) -> (u32, u32) {
    let (wf, hf) = (w as f64, h as f64);
    let (nw, nh) = match mode {
        "contain" => {
            let s = (tw as f64 / wf).min(th as f64 / hf);
            ((wf * s).round(), (hf * s).round())
        }
        _ => {
            let s = (tw as f64 * th as f64 / (wf * hf)).sqrt().min(1.0);
            ((wf * s).floor(), (hf * s).floor())
        }
    };
    let (nw, nh) = (nw.max(1.0) as u32, nh.max(1.0) as u32);
    if nw >= w && nh >= h { (w, h) } else { (nw, nh) }
}

fn prepare_image(up: &MediaUpload) -> Result<PreparedImage> {
    let img = image::load_from_memory(&up.data)
        .map_err(|e| Error::Protocol(format!("unsupported image ({e}); send it as a file instead")))?;
    let (w, h) = (img.width(), img.height());
    let (pw, ph) = fit(w, h, "contain", 384, 384);
    let preview_img = img.resize_exact(pw, ph, image::imageops::FilterType::Triangle);
    let preview = jpeg(&preview_img, 30)?;
    let thumb_info = json!({ "width": pw, "height": ph });
    let stem = up
        .name
        .rsplit_once('.')
        .map_or(up.name.as_str(), |(s, _)| s)
        .to_owned();
    if up.original {
        if let Some(data) = crate::exif::scrub(&up.data) {
            let ext = up
                .name
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_else(|| "jpg".into());
            let info = json!({ "category": "original", "fileSize": data.len(), "extension": ext,
                               "animated": false, "width": w, "height": h });
            return Ok(PreparedImage {
                data,
                preview,
                thumb_info,
                content_info: Some(info),
                name: up.name.clone(),
            });
        }
        // Its metadata (GPS and all) could not be removed: never send it as
        // is; a re-encoded copy carries no metadata.
        tracing::warn!(
            "cannot strip the metadata of {}; sending a resized copy instead of the original",
            up.name
        );
    }
    let (mw, mh) = fit(w, h, "maxPixel", 1280, 1280);
    let main = if (mw, mh) == (w, h) {
        img
    } else {
        img.resize_exact(mw, mh, image::imageops::FilterType::Triangle)
    };
    Ok(PreparedImage {
        data: jpeg(&main, 60)?,
        preview,
        thumb_info,
        content_info: None,
        name: format!("{stem}.jpg"),
    })
}

// -- client methods ---------------------------------------------------------------------

/// Cached OBS token and media-flow answers.
#[derive(Debug, Default)]
pub(crate) struct MediaState {
    obs_token: Option<String>,
    /// Held while a new OBS token is fetched, so parallel downloads that all
    /// find none (or all get a 401) share one `acquireEncryptedAccessToken`.
    obs_fetch: Arc<tokio::sync::Mutex<()>>,
    flows: HashMap<String, (Instant, Value)>,
}

/// Split the `acquireEncryptedAccessToken` blob: rows separated by U+001E,
/// columns by U+001F; the token is row 1, column 0 (live: rows of 6 and 1304 chars).
fn parse_encrypted_token(raw: &str) -> Option<String> {
    let rows: Vec<Vec<&str>> = raw
        .split('\u{1e}')
        .filter(|r| !r.is_empty())
        .map(|r| r.split('\u{1f}').collect())
        .collect();
    rows.get(1)
        .and_then(|r| r.first())
        .filter(|t| !t.is_empty())
        .map(|t| (*t).to_owned())
}

impl Client {
    fn media(&self) -> std::sync::MutexGuard<'_, MediaState> {
        self.media.lock().unwrap()
    }

    /// The cached OBS token, fetched if there is none or if the cached one is
    /// `stale` (the one a request was just refused with). Concurrent callers
    /// wait for one fetch instead of each making their own.
    async fn obs_token(&self, stale: Option<&str>) -> Result<String> {
        let usable = |m: &MediaState| m.obs_token.clone().filter(|t| Some(t.as_str()) != stale);
        let lock = {
            let m = self.media();
            if let Some(t) = usable(&m) {
                return Ok(t);
            }
            m.obs_fetch.clone()
        };
        let _fetching = lock.lock().await;
        // Someone else may have fetched one while we waited.
        if let Some(t) = usable(&self.media()) {
            return Ok(t);
        }
        let v = self
            .call(
                "talk/thrift/Talk/TalkService/acquireEncryptedAccessToken",
                json!([2]),
            )
            .await?;
        let tok = v
            .as_str()
            .and_then(parse_encrypted_token)
            .ok_or_else(|| Error::Protocol("acquireEncryptedAccessToken returned no token".into()))?;
        self.media().obs_token = Some(tok.clone());
        Ok(tok)
    }

    /// OBS auth headers, and the token they carry.
    async fn obs_headers(&self, stale: Option<&str>) -> Result<(HeaderMap, String)> {
        let token = self.obs_token(stale).await?;
        let mut h = HeaderMap::new();
        let mut tok = HeaderValue::from_str(&token).map_err(|e| Error::Protocol(e.to_string()))?;
        tok.set_sensitive(true);
        h.insert("x-line-access", tok);
        h.insert("x-line-application", HeaderValue::from_static(APPLICATION_HEADER));
        if let Ok(v) = HeaderValue::from_str(&self.config().locale) {
            h.insert("accept-language", v);
        }
        Ok((h, token))
    }

    /// One OBS request with the extension's auth; renews the token once on 401/403.
    pub(crate) async fn obs_request(
        &self,
        post: Option<(&[u8], String)>,
        url: &str,
        extra: &[(&'static str, String)],
    ) -> Result<reqwest::Response> {
        let mut refused: Option<String> = None;
        loop {
            let (mut h, used) = self.obs_headers(refused.as_deref()).await?;
            for (k, v) in extra {
                h.insert(
                    *k,
                    HeaderValue::from_str(v).map_err(|e| Error::Protocol(e.to_string()))?,
                );
            }
            let req = match &post {
                Some((body, params)) => {
                    h.insert("x-obs-params", HeaderValue::from_str(params).expect("base64"));
                    self.t.http.post(url).body(body.to_vec())
                }
                None => self.t.http.get(url),
            };
            tracing::debug!(%url, post = post.is_some(), "OBS");
            let resp = req.headers(h).timeout(Duration::from_secs(300)).send().await?;
            let status = resp.status().as_u16();
            if matches!(status, 401 | 403) && refused.is_none() {
                refused = Some(used);
                continue;
            }
            if status >= 400 {
                let body = resp.text().await.unwrap_or_default();
                if status == 404 {
                    return Err(Error::MediaUnavailable(format!(
                        "{} not found",
                        url.split('?').next().unwrap_or(url)
                    )));
                }
                return Err(crate::error::ApiError {
                    path: url.split('?').next().unwrap_or(url).to_owned(),
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

    /// Fetch a public (uncredentialed) URL. Message media links are chosen by
    /// the sender, so apart from the configured OBS host itself only `https`
    /// URLs with a host name are fetched, through the untrusted-fetch client:
    /// every address the name (or a redirect's) resolves to must be public and
    /// the connection goes to exactly those addresses. The body is capped at
    /// `limit`, and an error status's body is never read.
    async fn public_get(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let parsed = url::Url::parse(url).map_err(|e| Error::Protocol(format!("bad media URL: {e}")))?;
        let on_obs = url::Url::parse(&self.t.config.obs).is_ok_and(|o| o.origin() == parsed.origin());
        let refused = |what: &url::Url| {
            Error::MediaUnavailable(format!(
                "refusing to fetch media from {}://{} (only public https hosts)",
                what.scheme(),
                what.host_str().unwrap_or("")
            ))
        };
        if !on_obs && !crate::transport::is_public_https(&parsed) {
            return Err(refused(&parsed));
        }
        let http = if on_obs {
            &self.t.http
        } else {
            &self.t.untrusted_http
        };
        let resp = http
            .get(parsed.clone())
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| {
                if e.is_redirect() || crate::transport::is_non_public_destination(&e) {
                    refused(&parsed)
                } else {
                    e.into()
                }
            })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(crate::error::ApiError {
                path: url.to_owned(),
                status,
                gateway_code: None,
                talk_code: None,
                reason: Some(format!("HTTP {status}")),
                raw: Value::Null,
            }
            .into());
        }
        read_body(resp, limit).await
    }

    /// The media flow LINE wants for `content_type` in `chat`: 2 = sealed (V2),
    /// 1 = plain (V1). Cached for the server's `cacheTtlMillis`.
    pub async fn media_flow(&self, chat: &str, content_type: i64) -> Result<i64> {
        let cached = self
            .media()
            .flows
            .get(chat)
            .filter(|(exp, _)| *exp > Instant::now())
            .map(|(_, v)| v.clone());
        let v = match cached {
            Some(v) => v,
            None => {
                let v = self
                    .call(
                        "talk/thrift/Talk/TalkService/determineMediaMessageFlow",
                        json!([{ "chatMid": chat }]),
                    )
                    .await?;
                let ttl = v.get("cacheTtlMillis").and_then(value_i64).unwrap_or(0).max(0) as u64;
                self.media().flows.insert(
                    chat.to_owned(),
                    (Instant::now() + Duration::from_millis(ttl), v.clone()),
                );
                v
            }
        };
        Ok(v.get("flowMap")
            .and_then(|m| m.get(content_type.to_string()))
            .and_then(value_i64)
            .unwrap_or(1))
    }

    /// Download the media of an image/video/audio/file message (decrypting
    /// sealed messages and blobs as needed). `preview` fetches the thumbnail.
    /// For an album post ([`Message::album_post`]) this is its first photo
    /// (albums have no separate previews).
    pub async fn download_media(&self, m: &Message, preview: bool) -> Result<Vec<u8>> {
        let m = if m.is_e2ee() {
            self.decrypt_message(m).await?
        } else {
            m.clone()
        };
        // Album posts point at an album photo rather than carrying media.
        if let Some(post) = m.album_post() {
            if preview && post.media.first().is_some_and(crate::album::AlbumMedia::is_video) {
                return Err(Error::MediaUnavailable("album videos have no preview".into()));
            }
            return self.download_album_post(&post).await;
        }
        let meta = &m.content_metadata;
        if let Some(exp) = meta
            .get("FILE_EXPIRE_TIMESTAMP")
            .and_then(|t| t.parse::<u128>().ok())
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            if exp > 0 && exp < now {
                return Err(Error::MediaUnavailable(format!(
                    "file expired at {exp} (epoch ms)"
                )));
            }
        }
        if preview && m.content_type == content_type::FILE {
            return Err(Error::MediaUnavailable("file messages have no preview".into()));
        }
        let public = if preview {
            meta.get("PREVIEW_URL")
        } else {
            meta.get("DOWNLOAD_URL")
        };
        let limit = if preview {
            MAX_PICTURE_DOWNLOAD
        } else {
            MAX_DOWNLOAD
        };
        if let Some(url) = public.filter(|u| u.starts_with("http")) {
            return self.public_get(url, limit).await;
        }
        let pop = meta.get("OBS_POP").map(|p| format!("?p={p}")).unwrap_or_default();
        let key = meta.get("ENC_KM").filter(|k| !k.is_empty());
        let path = if key.is_some() || meta.contains_key("e2eeVersion") {
            let sid = meta.get("SID").map_or("m", String::as_str);
            let oid = meta.get("OID").map_or(m.id.as_str(), String::as_str);
            let oid = if preview {
                format!("{oid}__ud-preview")
            } else {
                oid.to_owned()
            };
            format!("/r/talk/{sid}/{oid}{pop}")
        } else {
            let original = meta
                .get("MEDIA_CONTENT_INFO")
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .is_some_and(|v| v["category"] == "original");
            let tid = if preview {
                "/preview"
            } else if original {
                "/original"
            } else {
                ""
            };
            format!("/r/talk/m/{}{tid}{pop}", m.id)
        };
        let extra = match key {
            Some(_) => vec![("x-talk-meta", talk_meta(&m.id))],
            None => vec![],
        };
        let resp = self
            .obs_request(None, &format!("{}{path}", self.t.config.obs), &extra)
            .await?;
        let data = read_body(resp, limit).await?;
        match key {
            Some(k) => {
                let km = B64
                    .decode(k)
                    .map_err(|e| Error::Protocol(format!("bad ENC_KM: {e}")))?;
                let chunked = m.content_type == content_type::VIDEO && !preview;
                // HMAC + AES over up to 1 GiB: off the async workers, so other
                // downloads (and the UI) keep going meanwhile.
                tokio::task::spawn_blocking(move || decrypt_blob(&km, &data, chunked))
                    .await
                    .map_err(|e| Error::Protocol(format!("media decryption task failed: {e}")))?
            }
            None => Ok(data),
        }
    }

    /// Download a profile or group picture by its `picturePath` (public CDN).
    pub async fn download_picture(&self, picture_path: &str, size: PictureSize) -> Result<Vec<u8>> {
        let url = picture_url(picture_path, size).ok_or_else(|| Error::Protocol("no picture".into()))?;
        self.public_get(&url, MAX_PICTURE_DOWNLOAD).await
    }

    /// Download a sticker image (public CDN).
    pub async fn download_sticker(&self, sticker_id: &str, variant: StickerImage) -> Result<Vec<u8>> {
        self.public_get(&sticker_url(sticker_id, variant), MAX_PICTURE_DOWNLOAD)
            .await
    }

    /// Download a LINE emoji picture (public CDN), e.g. a paid reaction's.
    pub async fn download_emoji(&self, product_id: &str, emoji_id: &str) -> Result<Vec<u8>> {
        let url = emoji_url(product_id, emoji_id)
            .ok_or_else(|| Error::Protocol(format!("bad emoji id {product_id}/{emoji_id}")))?;
        self.public_get(&url, MAX_PICTURE_DOWNLOAD).await
    }

    /// A LINE emoji package's `meta.json` (public CDN).
    pub(crate) async fn emoji_meta(&self, product_id: &str) -> Result<Value> {
        let url = emoji_meta_url(product_id)
            .ok_or_else(|| Error::Protocol(format!("bad emoji package id {product_id}")))?;
        let body = self.public_get(&url, 1 << 20).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// Upload one object; returns the `x-obs-oid` response header if present.
    async fn obs_upload(&self, sid: &str, oid: &str, data: &[u8], params: String) -> Result<Option<String>> {
        let url = format!("{}/r/talk/{sid}/{oid}", self.t.config.obs);
        let resp = self.obs_request(Some((data, params)), &url, &[]).await?;
        Ok(resp
            .headers()
            .get("x-obs-oid")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned))
    }

    /// Send an image, video, audio clip or file. Uses the sealed (V2) flow
    /// when LINE asks for it and Letter Sealing keys are loaded, else the
    /// plain (V1) flow. Returns the sent message.
    pub async fn send_media(&self, to: &str, upload: MediaUpload) -> Result<Message> {
        let mut up = upload;
        if up.data.len() > MAX_FILE {
            return Err(Error::Protocol("file exceeds LINE's 1 GiB limit".into()));
        }
        if up.kind == MediaKind::Image && up.data.len() > MAX_IMAGE {
            up.kind = MediaKind::File;
        }
        // Images are normalised and get a preview, like the extension does.
        let image = match up.kind {
            MediaKind::Image => match prepare_image(&up) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!("{e}; sending as a file");
                    up.kind = MediaKind::File;
                    None
                }
            },
            _ => None,
        };
        let ct = up.kind.content_type();
        let (data, name) = match &image {
            Some(p) => (p.data.clone(), p.name.clone()),
            None => (up.data.clone(), up.name.clone()),
        };
        let mut msg = OutgoingMessage::media(to, ct);
        let meta = &mut msg.content_metadata;
        if let Some(d) = up.duration_ms {
            meta.insert("DURATION".into(), d.to_string());
        }
        if let Some(info) = image.as_ref().and_then(|p| p.content_info.as_ref()) {
            meta.insert("MEDIA_CONTENT_INFO".into(), info.to_string());
        }
        if up.kind == MediaKind::File {
            meta.insert("FILE_NAME".into(), name.clone());
            meta.insert("FILE_SIZE".into(), data.len().to_string());
        }
        let sealed = self.e2ee_ready() && self.media_flow(to, ct).await? == 2;
        if !sealed {
            return self
                .send_media_v1(
                    msg,
                    up.kind,
                    &data,
                    &name,
                    image.is_some_and(|p| p.content_info.is_some()),
                )
                .await;
        }

        // V2: encrypt, upload, then send the sealed message pointing at it.
        let mut km = [0u8; 32];
        getrandom::fill(&mut km).map_err(|e| Error::Protocol(format!("getrandom: {e}")))?;
        let km_b64 = B64.encode(km);
        let chunked = up.kind == MediaKind::Video;
        let size = data.len();
        let preview = image.as_ref().map(|p| p.preview.clone()).or(up.preview.clone());
        // Encryption is CPU work over up to 1 GiB: keep it off the async workers.
        let (blob, hashes, pblob) = tokio::task::spawn_blocking(move || {
            let (blob, hashes) = encrypt_blob(&km, &data, chunked);
            let pblob = preview.map(|pv| encrypt_blob(&km, &pv, false).0);
            (blob, hashes, pblob)
        })
        .await
        .map_err(|e| Error::Protocol(format!("media encryption task failed: {e}")))?;
        let sid = up.kind.sealed_sid();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let oid = self
            .obs_upload(
                sid,
                &format!("reqid-{}", random_uuid()),
                &blob,
                obs_params(&stamp.to_string(), "file", None),
            )
            .await?
            .ok_or_else(|| Error::Protocol("OBS upload returned no x-obs-oid".into()))?;
        // The preview and the chunk hashes hang off the object id; they go up together.
        let preview_up = async {
            if let Some(pblob) = &pblob {
                self.obs_upload(
                    sid,
                    &format!("{oid}__ud-preview"),
                    pblob,
                    obs_params("preview.jpg", "file", None),
                )
                .await?;
            }
            Ok::<_, Error>(())
        };
        let hash_up = async {
            if let Some(h) = &hashes {
                self.obs_upload(
                    sid,
                    &format!("{oid}__ud-hash"),
                    h,
                    obs_params("chunk-hash", "file", None),
                )
                .await?;
            }
            Ok::<_, Error>(())
        };
        tokio::try_join!(preview_up, hash_up)?;
        if pblob.is_some()
            && let Some(t) = image.as_ref().map(|p| p.thumb_info.clone())
        {
            msg.content_metadata
                .insert("MEDIA_THUMB_INFO".into(), t.to_string());
        }
        let meta = &mut msg.content_metadata;
        meta.insert("ENC_KM".into(), km_b64);
        meta.insert("SID".into(), sid.into());
        meta.insert("OID".into(), oid);
        meta.insert("FILE_NAME".into(), name);
        meta.insert("FILE_SIZE".into(), size.to_string());
        self.send_sealed_required(&msg).await
    }

    async fn send_media_v1(
        &self,
        msg: OutgoingMessage,
        kind: MediaKind,
        data: &[u8],
        name: &str,
        original: bool,
    ) -> Result<Message> {
        let sent: Message = serde_json::from_value(
            self.call(
                "talk/thrift/Talk/TalkService/sendMessage",
                json!([self.next_req_seq(), msg.to_json()]),
            )
            .await?,
        )?;
        if sent.id.is_empty() {
            return Err(Error::Protocol("sendMessage returned no message id".into()));
        }
        // `oE` (V1): original images and files keep their name; others get
        // "<stem>.<jpg|mp4|m4a>".
        let upload_name = match kind.forced_ext() {
            Some(ext) if !original => format!("{}.{ext}", name.rsplit_once('.').map_or(name, |(s, _)| s)),
            _ => name.to_owned(),
        };
        let cat = (kind == MediaKind::Image && original).then_some("original");
        self.obs_upload(
            "m",
            &sent.id,
            data,
            obs_params(&upload_name, kind.obs_type(), cat),
        )
        .await?;
        Ok(sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_round_trip_and_tamper() {
        let km = [7u8; 32];
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        for chunked in [false, true] {
            let (blob, hashes) = encrypt_blob(&km, &data, chunked);
            assert_eq!(blob.len(), data.len() + 32);
            assert_eq!(hashes.as_ref().map(Vec::len), chunked.then_some(3 * 32)); // 3 chunks of 128 KiB
            assert_eq!(decrypt_blob(&km, &blob, chunked).unwrap(), data);
            let mut bad = blob.clone();
            bad[10] ^= 1;
            assert!(decrypt_blob(&km, &bad, chunked).is_err());
            assert!(decrypt_blob(&km, &blob, !chunked).is_err(), "tag mode must match");
        }
    }

    #[test]
    fn talk_meta_matches_reference() {
        // Value computed with OkLine's build_talk_meta("640466092447498768").
        assert_eq!(
            talk_meta("640466092447498768"),
            "eyJtZXNzYWdlIjoiQ3dBRUFBQUFFalkwTURRMk5qQTVNalEwTnpRNU9EYzJPQThBR3d3QUFBQUFBQT09In0="
        );
    }

    #[test]
    fn urls() {
        assert_eq!(
            picture_url("/0habc", PictureSize::Preview).unwrap(),
            "https://profile.line-scdn.net/0habc/preview"
        );
        assert_eq!(picture_url("", PictureSize::Full), None);
        assert_eq!(
            sticker_url("403", StickerImage::Static),
            "https://stickershop.line-scdn.net/stickershop/v1/sticker/403/android/sticker.png"
        );
        assert_eq!(
            emoji_url("670e0cce840a8236ddd4ee4c", "143").unwrap(),
            "https://stickershop.line-scdn.net/sticonshop/v1/sticon/670e0cce840a8236ddd4ee4c/iPhone/143.png"
        );
        // Reactions carry whatever the reactor sent: nothing but plain ids goes into a URL.
        for (p, e) in [("../x", "1"), ("670e", "1?a=b"), ("", "1"), ("670e", "a/b")] {
            assert_eq!(emoji_url(p, e), None, "{p} {e}");
        }
        assert!(emoji_meta_url("670e/../..").is_none());
    }

    #[test]
    fn token_blob_parsing() {
        assert_eq!(
            parse_encrypted_token("\u{1e}abc123\u{1e}TOKEN\u{1f}x"),
            Some("TOKEN".into())
        );
        assert_eq!(parse_encrypted_token("only"), None);
    }

    #[test]
    fn image_sizing_matches_canvas_rules() {
        assert_eq!(fit(4000, 3000, "contain", 384, 384), (384, 288));
        assert_eq!(fit(200, 100, "contain", 384, 384), (200, 100)); // keep smaller
        assert_eq!(fit(4032, 3024, "maxPixel", 1280, 1280), (1478, 1108));
        assert_eq!(fit(800, 600, "maxPixel", 1280, 1280), (800, 600));
    }
}

#[cfg(test)]
mod vector_tests {
    use super::*;

    /// Vectors from the extension's own algorithm on WebCrypto
    /// (`scripts/gen_blob_vectors.js`).
    #[test]
    fn blob_encryption_matches_webcrypto() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/blob_vectors.json")).unwrap();
        for c in cases {
            let km = B64.decode(c["keyMaterial"].as_str().unwrap()).unwrap();
            let len = c["len"].as_u64().unwrap() as usize;
            let chunked = c["chunked"].as_bool().unwrap();
            let data: Vec<u8> = (0..len).map(|i| ((i * 7 + 3) % 251) as u8).collect();
            let (blob, hashes) = encrypt_blob(&km, &data, chunked);
            let hex = |b: &[u8]| {
                Sha256::digest(b)
                    .iter()
                    .map(|x| format!("{x:02x}"))
                    .collect::<String>()
            };
            assert_eq!(blob.len() as u64, c["blobLen"].as_u64().unwrap());
            assert_eq!(
                hex(&blob),
                c["blobSha256"].as_str().unwrap(),
                "len {len} chunked {chunked}"
            );
            assert_eq!(
                hashes.as_deref().map(hex),
                c["hashesSha256"].as_str().map(str::to_owned)
            );
            assert_eq!(decrypt_blob(&km, &blob, chunked).unwrap(), data);
        }
    }
}

#[cfg(test)]
mod original_tests {
    use super::*;

    #[test]
    fn an_original_whose_metadata_cannot_be_stripped_is_sent_re_encoded() {
        let full = crate::exif::tests::jpeg_with_exif();
        // Junk before a marker: decoders skip it, the scrubber cannot walk it.
        let exif_end = 4 + u16::from_be_bytes([full[4], full[5]]) as usize;
        let broken = [&full[..exif_end], b"\x00\x00junk", &full[exif_end..]].concat();
        assert!(image::load_from_memory(&broken).is_ok(), "decodable");
        let up = MediaUpload {
            original: true,
            ..MediaUpload::image(broken, "photo.jpg")
        };
        let p = prepare_image(&up).unwrap();
        assert!(p.content_info.is_none(), "not sent as an original");
        let s = String::from_utf8_lossy(&p.data);
        assert!(!s.contains("SN12345") && !s.contains("Bob") && !s.contains("Exif"));
        assert!(image::load_from_memory(&p.data).is_ok());

        // A clean original is still sent as one, scrubbed.
        let up = MediaUpload {
            original: true,
            ..MediaUpload::image(full, "photo.jpg")
        };
        let p = prepare_image(&up).unwrap();
        assert!(p.content_info.is_some());
        assert!(!String::from_utf8_lossy(&p.data).contains("SN12345"));
    }
}
