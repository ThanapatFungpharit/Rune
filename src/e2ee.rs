//! Letter Sealing (LINE's end-to-end encryption) for text and location
//! messages, 1:1 and group.
//!
//! The crypto runs inside `ltsm.wasm`; this module does the protocol around
//! it, following the extension (via the reference implementation):
//!
//! * **1:1** — `negotiateE2EEPublicKey(peer)` gives the peer's key id, public
//!   key, `specVersion` and `allowedTypes`. Channel = ECDH(our latest key,
//!   peer key).
//! * **group** — the group has a shared private key, wrapped per member.
//!   `getLastE2EEGroupSharedKey` → unwrap with ECDH(our key `receiverKeyId`,
//!   creator's key) → group key. Sending: channel = ECDH(group key, our
//!   public key); receiving: ECDH(group key, sender's public key). A group
//!   without a key gets one (`registerE2EEGroupKey`), as the extension does.
//! * **V2 wire format** — ciphertext `ct` from `encryptV2` (AAD: to, from,
//!   key ids, content type) is sent as
//!   `chunks = [ct[0:16], ct[28:], ct[16:28], BE32(senderKeyId), BE32(receiverKeyId)]`
//!   with `contentMetadata.e2eeVersion = "2"`; `text`/`location`/`from` are
//!   removed. V1: `[ct[0:8], ct[8:-16], ct[-16:], sid, rid]`, no AAD.
//! * **plaintext** — compact JSON `{text, location, REPLACE}`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Map, Value, json};

use crate::auth::E2eeKey;
use crate::client::Client;
use crate::error::{E2eeErrorKind as Kind, Error, Result};
use crate::ltsm::{ChannelHandle, KeyHandle};
use crate::models::{Message, MidType, OutgoingMessage, content_type, value_i64};

/// Content types that may be sealed (the extension's `fv`): text, location
/// and media. Media is only sealed on the V2 media flow (see `media.rs`).
const SEALABLE: [i64; 6] = [
    content_type::TEXT,
    content_type::LOCATION,
    content_type::IMAGE,
    content_type::VIDEO,
    content_type::FILE,
    content_type::AUDIO,
];
/// Sealed automatically by `send_message` on code 82 (media goes through `send_media`).
const AUTO_SEAL: [i64; 2] = [content_type::TEXT, content_type::LOCATION];

/// Send errors after which the extension drops cached E2EE state and retries.
const RETRY_CODES: [i64; 5] = [
    84, // E2EE_UPDATE_RECEIVER_KEY
    86, // E2EE_INVALID_VERSION
    87, // E2EE_SENDER_DISABLED
    88, // E2EE_RECEIVER_DISABLED
    90, // E2EE_RECEIVER_NOT_ALLOWED
];
const RECREATE_GROUP_KEY: i64 = 99;
const REFRESH_MEDIA_FLOW: i64 = 122;
const NOT_FOUND: i64 = 5;
/// `E2EE_UPDATE_SENDER_KEY`: our sender key is not current any more.
const UPDATE_SENDER_KEY: i64 = 83;
/// `E2EE_INVALID_ARGUMENT` — live, `getE2EEPublicKey` answers it ("invalid
/// keyId") for retired key ids and for id 0.
const INVALID_ARGUMENT: i64 = 85;
/// How many decrypted messages to remember (duplicate deliveries are common:
/// the same message arrives via the stream and via history).
const DECRYPT_CACHE: usize = 1024;

/// Operation types after which cached peer/group key state for the chats they
/// mention is dropped (local cache invalidation; the next send/receive simply
/// re-fetches). Key changes: UPDATE_PUBLICKEYCHAIN(66),
/// NOTIFIED_UPDATE_PUBLICKEYCHAIN(67), REGISTER_E2EE_PUBLICKEY(72),
/// NOTIFIED_E2EE_KEY_UPDATE(77). Membership changes: NOTIFIED_JOIN/LEAVE_CHAT
/// (60/61) and the chat invite/accept/leave/kick family (123–133).
const INVALIDATING_OPS: [i64; 17] = [
    66, 67, 72, 77, 60, 61, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133,
];

// -- framing (pure) -----------------------------------------------------------

/// Plaintext JSON of a message (`{text, location, REPLACE}`, absent fields
/// omitted), in the extension's field order.
pub(crate) fn serialize_plaintext(msg: &Value) -> Vec<u8> {
    let mut o = Map::new();
    if let Some(t) = msg.get("text").filter(|v| !v.is_null()) {
        o.insert("text".into(), t.clone());
    }
    if let Some(l) = msg.get("location").filter(|v| !v.is_null()) {
        o.insert("location".into(), l.clone());
    }
    if let Some(Value::String(r)) = msg.get("contentMetadata").and_then(|m| m.get("REPLACE"))
        && let Ok(v) = serde_json::from_str::<Value>(r)
    {
        o.insert("REPLACE".into(), v);
    }
    // Sealed media (`gL`): the key material and file name go inside.
    let meta = msg.get("contentMetadata");
    if let Some(km) = meta
        .and_then(|m| m.get("ENC_KM"))
        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
    {
        o.insert("keyMaterial".into(), km.clone());
        if let Some(f) = meta.and_then(|m| m.get("FILE_NAME")).filter(|v| !v.is_null()) {
            o.insert("fileName".into(), f.clone());
        }
    }
    // serde_json's Map is sorted; write the keys in insertion order by hand.
    let mut out = String::from("{");
    for (i, k) in ["text", "location", "REPLACE", "keyMaterial", "fileName"]
        .iter()
        .filter(|k| o.contains_key(**k))
        .enumerate()
    {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(k).unwrap());
        out.push(':');
        out.push_str(&serde_json::to_string(&o[*k]).unwrap());
    }
    out.push('}');
    out.into_bytes()
}

/// Parse decrypted plaintext. Raw C0 control characters (invalid inside JSON
/// strings) are escaped first, like the extension's `xL`; unparsable data is
/// returned as `{"text": <raw>}`.
pub(crate) fn deserialize_plaintext(data: &[u8]) -> Value {
    let text = String::from_utf8_lossy(data);
    let escaped: String = text
        .chars()
        .map(|c| {
            if (c as u32) < 0x20 {
                format!("\\u{:04x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    serde_json::from_str(&escaped).unwrap_or_else(|_| json!({ "text": text }))
}

fn be32(n: i64) -> String {
    B64.encode((n as u32).to_be_bytes())
}

pub(crate) fn build_chunks(
    version: u8,
    ct: &[u8],
    sender_key_id: i64,
    receiver_key_id: i64,
) -> Result<Vec<String>> {
    let (a, b, c) = match version {
        2 if ct.len() >= 28 => (&ct[0..16], &ct[28..], &ct[16..28]),
        1 if ct.len() >= 24 => (&ct[0..8], &ct[8..ct.len() - 16], &ct[ct.len() - 16..]),
        _ => return Err(Error::Protocol(format!("ciphertext too short for V{version}"))),
    };
    Ok(vec![
        B64.encode(a),
        B64.encode(b),
        B64.encode(c),
        be32(sender_key_id),
        be32(receiver_key_id),
    ])
}

/// `(ciphertext, sender_key_id, receiver_key_id)`; key ids only with 5 chunks.
pub(crate) fn parse_chunks(version: u8, chunks: &[String]) -> Result<(Vec<u8>, Option<i64>, Option<i64>)> {
    let dec = |s: &String| {
        B64.decode(s)
            .map_err(|e| Error::Protocol(format!("bad chunk: {e}")))
    };
    if chunks.len() < 3 {
        return Err(Error::Protocol("sealed message with fewer than 3 chunks".into()));
    }
    let (c0, c1, c2) = (dec(&chunks[0])?, dec(&chunks[1])?, dec(&chunks[2])?);
    // V2 transmits [salt, body, nonce] but encrypts as salt + nonce + body.
    let ct = if version == 1 {
        [c0, c1, c2].concat()
    } else {
        [c0, c2, c1].concat()
    };
    let id = |s: &String| -> Result<i64> {
        let b = dec(s)?;
        Ok(b.iter().fold(0i64, |acc, x| (acc << 8) | *x as i64))
    };
    if chunks.len() >= 5 {
        Ok((ct, Some(id(&chunks[3])?), Some(id(&chunks[4])?)))
    } else {
        Ok((ct, None, None))
    }
}

/// Turn a plain message JSON into the sealed wire struct.
pub(crate) fn seal_message(msg: &Value, chunks: Vec<String>, version: u8) -> Value {
    let mut out = msg.clone();
    let o = out.as_object_mut().expect("message object");
    o.remove("text");
    o.remove("location");
    o.remove("from"); // the gateway 500s on `from` (reference, live-verified)
    let meta = o.entry("contentMetadata").or_insert_with(|| json!({}));
    if let Some(m) = meta.as_object_mut() {
        m.remove("REPLACE");
        // `EL`: sealed-media key material now lives in the ciphertext.
        if m.remove("ENC_KM").is_some() {
            m.remove("FILE_NAME");
        }
        m.insert("e2eeVersion".into(), json!(version.to_string()));
    }
    o.insert("chunks".into(), json!(chunks));
    out
}

fn message_version(m: &Message) -> Result<u8> {
    match m.content_metadata.get("e2eeVersion").map(|v| v.trim()) {
        Some("1") => Ok(1),
        None | Some("") | Some("2") => Ok(2),
        Some(v) => Err(Error::e2ee(
            Kind::UnsupportedVersion(v.parse().unwrap_or(-1)),
            format!("e2eeVersion {v}"),
        )),
    }
}

/// Our view of the account's Letter Sealing keys versus the server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eeStatus {
    /// Key ids we hold private keys for.
    pub local_key_ids: Vec<i64>,
    /// Key ids the server currently has registered for the account
    /// (`getE2EEPublicKeysEx`).
    pub server_key_ids: Vec<i64>,
}

impl E2eeStatus {
    /// True when we hold every key the server considers current. When false,
    /// another device rotated the account key: new messages will be sealed to
    /// a key we do not have — log in by QR again to receive the new keychain.
    pub fn is_current(&self) -> bool {
        !self.server_key_ids.is_empty() && self.server_key_ids.iter().all(|k| self.local_key_ids.contains(k))
    }
}

fn is_group(to: &str, to_type: i64) -> bool {
    to_type != MidType::User as i64 || MidType::of(to) != MidType::User
}

// -- state ----------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Peer {
    my_key_id: i64,
    key_id: i64,
    spec_version: i64,
    allowed_types: Vec<i64>,
    channel: ChannelHandle,
}

#[derive(Debug, Clone)]
struct GroupInfo {
    key_id: i64,
    spec_version: i64,
    allowed_types: Vec<i64>,
}

/// Cached Letter Sealing state of a client.
#[derive(Debug)]
pub(crate) struct E2eeState {
    my_keys: BTreeMap<i64, KeyHandle>,
    /// The blobs the keys were loaded from (loaded keys cannot be re-exported).
    exported: Vec<E2eeKey>,
    decrypted: HashMap<String, Message>,
    decrypted_order: VecDeque<String>,
    public_keys: HashMap<(String, i64), Vec<u8>>,
    peers: HashMap<String, Peer>,
    group_keys: HashMap<(String, i64), KeyHandle>,
    groups: HashMap<String, GroupInfo>,
    /// Chats known to reject plain messages (TalkException 82).
    sealed_chats: HashSet<String>,
    seq: u64,
}

impl Default for E2eeState {
    fn default() -> Self {
        // V2 sequence numbers: the reference counts from 0 per process; a
        // clock seed avoids repeating them across restarts.
        let seq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        E2eeState {
            my_keys: BTreeMap::new(),
            exported: Vec::new(),
            decrypted: HashMap::new(),
            decrypted_order: VecDeque::new(),
            public_keys: HashMap::new(),
            peers: HashMap::new(),
            group_keys: HashMap::new(),
            groups: HashMap::new(),
            sealed_chats: HashSet::new(),
            seq,
        }
    }
}

fn str_field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol(format!("missing {k} in {v}")))
}

fn int_field(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(value_i64)
        .ok_or_else(|| Error::Protocol(format!("missing {k} in {v}")))
}

fn int_list(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(value_i64).collect())
        .unwrap_or_default()
}

fn b64(s: &str) -> Result<Vec<u8>> {
    B64.decode(s)
        .map_err(|e| Error::Protocol(format!("bad base64: {e}")))
}

impl Client {
    fn e2ee(&self) -> std::sync::MutexGuard<'_, E2eeState> {
        self.e2ee.lock().unwrap()
    }

    /// Load Letter Sealing private keys (from [`LoginResult::e2ee_keys`](crate::LoginResult)
    /// or a [`Session`](crate::Session)). Replaces previously loaded keys.
    pub fn load_e2ee_keys(&self, keys: &[E2eeKey]) -> Result<()> {
        let mut loaded = BTreeMap::new();
        for k in keys {
            let h = self.with_ltsm(|l| l.load_e2ee_key(&b64(&k.exported)?).map_err(Error::from))?;
            loaded.insert(k.key_id as i64, h);
        }
        let old = {
            let mut st = self.e2ee();
            st.exported = keys.to_vec();
            std::mem::replace(&mut st.my_keys, loaded)
        };
        self.with_ltsm(|l| old.into_values().try_for_each(|h| l.release_key(h)))?;
        Ok(())
    }

    /// The loaded Letter Sealing keys, as exported blobs (for [`Session`](crate::Session)).
    pub fn e2ee_keys(&self) -> Vec<E2eeKey> {
        self.e2ee().exported.clone()
    }

    /// Whether Letter Sealing keys are loaded.
    pub fn e2ee_ready(&self) -> bool {
        !self.e2ee().my_keys.is_empty()
    }

    fn latest_key(&self) -> Result<(i64, KeyHandle)> {
        self.e2ee()
            .my_keys
            .iter()
            .next_back()
            .map(|(id, h)| (*id, *h))
            .ok_or_else(|| Error::e2ee(Kind::NoKeys, "no Letter Sealing keys loaded (log in by QR)"))
    }

    /// Our key `id` (the latest one when the message carries no key id).
    /// Never falls back silently: decrypting with the wrong key only yields a
    /// misleading authentication error.
    fn my_key(&self, id: Option<i64>) -> Result<KeyHandle> {
        let Some(id) = id else {
            return Ok(self.latest_key()?.1);
        };
        if self.e2ee().my_keys.is_empty() {
            return Err(Error::e2ee(
                Kind::NoKeys,
                "no Letter Sealing keys loaded (log in by QR)",
            ));
        }
        self.e2ee().my_keys.get(&id).copied().ok_or_else(|| {
            Error::e2ee(
                Kind::MissingOwnKey { key_id: id },
                format!("no private key {id} in our keychain"),
            )
        })
    }

    /// A user's public key for `key_id` (cached; key ids are immutable).
    async fn user_public_key(&self, mid: &str, key_id: i64) -> Result<Vec<u8>> {
        if let Some(k) = self.e2ee().public_keys.get(&(mid.to_owned(), key_id)) {
            return Ok(k.clone());
        }
        // Our own keys: use the local public half. The server stops serving
        // retired key ids (live: "invalid keyId"), but we still hold them.
        if self.my_mid().as_deref() == Some(mid) {
            let own = self.e2ee().my_keys.get(&key_id).copied();
            if let Some(h) = own {
                let key = self.with_ltsm(|l| l.public_key(h))?;
                self.e2ee()
                    .public_keys
                    .insert((mid.to_owned(), key_id), key.clone());
                return Ok(key);
            }
        }
        let v = match self.e2ee_public_key(mid, 1, key_id).await {
            Ok(v) => v,
            Err(Error::Api(e)) if matches!(e.talk_code, Some(INVALID_ARGUMENT | NOT_FOUND)) => {
                return Err(Error::e2ee(
                    Kind::PeerKeyUnavailable {
                        mid: mid.to_owned(),
                        key_id,
                    },
                    format!("server has no public key {key_id} for {mid}: {e}"),
                ));
            }
            Err(e) => return Err(e),
        };
        let key = b64(str_field(&v, "keyData")?)?;
        self.e2ee()
            .public_keys
            .insert((mid.to_owned(), key_id), key.clone());
        Ok(key)
    }

    async fn peer(&self, mid: &str) -> Result<Peer> {
        if let Some(p) = self.e2ee().peers.get(mid) {
            return Ok(p.clone());
        }
        let neg = self.negotiate_e2ee_public_key(mid).await.map_err(|e| match e {
            Error::Api(a) if !a.is_auth() => {
                Error::e2ee(Kind::NegotiationFailed { mid: mid.to_owned() }, a.to_string())
            }
            e => e,
        })?;
        // The reference falls back to getE2EEPublicKey(mid, 1, 0) here, but the
        // live server rejects key id 0 ("invalid keyId"), so there is no fallback.
        let (key_id, key) = match neg.get("publicKey") {
            Some(pk) if pk.get("keyData").is_some() => {
                (int_field(pk, "keyId")?, b64(str_field(pk, "keyData")?)?)
            }
            _ => {
                return Err(Error::e2ee(
                    Kind::NegotiationFailed { mid: mid.to_owned() },
                    format!("negotiateE2EEPublicKey returned no public key: {neg}"),
                ));
            }
        };
        let (my_key_id, my) = self.latest_key()?;
        let channel = self.with_ltsm(|l| l.create_channel(my, &key))?;
        let peer = Peer {
            my_key_id,
            key_id,
            spec_version: neg.get("specVersion").and_then(value_i64).unwrap_or(2),
            allowed_types: int_list(neg.get("allowedTypes")),
            channel,
        };
        let mut st = self.e2ee();
        st.public_keys.insert((mid.to_owned(), key_id), key);
        if let Some(old) = st.peers.insert(mid.to_owned(), peer.clone()) {
            drop(st);
            self.with_ltsm(|l| l.release_channel(old.channel))?;
        }
        Ok(peer)
    }

    /// Drop cached negotiation/group keys for a chat (the extension's `resetE2eeInfo`).
    pub fn reset_e2ee_chat(&self, chat: &str) {
        let (peer, groups) = {
            let mut st = self.e2ee();
            st.groups.remove(chat);
            let keys: Vec<_> = st.group_keys.keys().filter(|(g, _)| g == chat).cloned().collect();
            let groups: Vec<KeyHandle> = keys.iter().filter_map(|k| st.group_keys.remove(k)).collect();
            (st.peers.remove(chat), groups)
        };
        let _ = self.with_ltsm(|l| -> crate::ltsm::Result<()> {
            if let Some(p) = peer {
                l.release_channel(p.channel)?;
            }
            groups.into_iter().try_for_each(|h| l.release_key(h))
        });
    }

    async fn unwrap_group_key(&self, group: &str, gsk: &Value) -> Result<(KeyHandle, i64)> {
        let gkid = int_field(gsk, "groupKeyId")?;
        let mine = self.my_key(gsk.get("receiverKeyId").and_then(value_i64))?;
        // creator may be us: user_public_key serves our own keys locally.
        let creator_pub = self
            .user_public_key(str_field(gsk, "creator")?, int_field(gsk, "creatorKeyId")?)
            .await?;
        let wrapped = b64(str_field(gsk, "encryptedSharedKey")?)?;
        let handle = self.with_ltsm(|l| -> crate::ltsm::Result<KeyHandle> {
            let ch = l.create_channel(mine, &creator_pub)?;
            let k = l.unwrap_group_shared_key(ch, &wrapped);
            l.release_channel(ch)?;
            k
        })?;
        let mut st = self.e2ee();
        st.group_keys.insert((group.to_owned(), gkid), handle);
        st.groups.insert(
            group.to_owned(),
            GroupInfo {
                key_id: gkid,
                spec_version: gsk.get("specVersion").and_then(value_i64).unwrap_or(2),
                allowed_types: int_list(gsk.get("allowedTypes")),
            },
        );
        Ok((handle, gkid))
    }

    /// The group key `key_id`, or the latest one (creating it if the group
    /// has none yet).
    async fn group_key(&self, group: &str, key_id: Option<i64>) -> Result<(KeyHandle, i64)> {
        {
            let st = self.e2ee();
            let id = key_id.or_else(|| st.groups.get(group).map(|g| g.key_id));
            if let Some(id) = id
                && let Some(h) = st.group_keys.get(&(group.to_owned(), id))
            {
                return Ok((*h, id));
            }
        }
        let gsk = match key_id {
            Some(id) => {
                self.talk_e2ee("getE2EEGroupSharedKey", json!([1, group, id]))
                    .await?
            }
            None => match self
                .talk_e2ee("getLastE2EEGroupSharedKey", json!([1, group]))
                .await
            {
                Err(Error::Api(e)) if e.talk_code == Some(NOT_FOUND) => {
                    return self.register_group_key(group).await;
                }
                r => r?,
            },
        };
        self.unwrap_group_key(group, &gsk).await
    }

    /// Create and upload a new shared key for a group (`registerE2EEGroupKey`).
    /// Every member's client will use it from then on.
    pub async fn register_group_key(&self, group: &str) -> Result<(KeyHandle, i64)> {
        let members = self.talk_e2ee("getLastE2EEPublicKeys", json!([group])).await?;
        let members = members
            .as_object()
            .filter(|m| !m.is_empty())
            .ok_or_else(|| Error::Protocol(format!("no member keys for {group}")))?;
        let (_, mine) = self.latest_key()?;
        let mut mids = Vec::new();
        let mut key_ids = Vec::new();
        let mut member_keys = Vec::new();
        for (mid, pk) in members {
            if let (Some(data), Some(id)) = (
                pk.get("keyData").and_then(Value::as_str),
                pk.get("keyId").and_then(value_i64),
            ) {
                mids.push(mid.clone());
                key_ids.push(id);
                member_keys.push(b64(data)?);
            }
        }
        let wrapped = self.with_ltsm(|l| -> crate::ltsm::Result<Vec<String>> {
            let shared = l.generate_curve_key()?;
            let mut out = Vec::new();
            for pk in &member_keys {
                let ch = l.create_channel(mine, pk)?;
                out.push(B64.encode(l.wrap_group_shared_key(ch, shared)?));
                l.release_channel(ch)?;
            }
            l.release_key(shared)?;
            Ok(out)
        })?;
        let result = self
            .talk_e2ee("registerE2EEGroupKey", json!([1, group, mids, key_ids, wrapped]))
            .await?;
        tracing::info!(%group, members = mids.len(), "registered a new E2EE group key");
        self.unwrap_group_key(group, &result).await
    }

    async fn talk_e2ee(&self, method: &str, args: Value) -> Result<Value> {
        self.call(&format!("talk/thrift/Talk/TalkService/{method}"), args)
            .await
    }

    fn next_e2ee_seq(&self) -> u64 {
        let mut st = self.e2ee();
        st.seq += 1;
        st.seq
    }

    /// Seal a message. `Ok(None)` when it must go out plain (content type not
    /// sealable or not allowed by the chat's negotiation).
    pub async fn seal_message(&self, msg: &OutgoingMessage) -> Result<Option<Value>> {
        if !SEALABLE.contains(&msg.content_type) {
            return Ok(None);
        }
        let wire = msg.to_json();
        let plaintext = serialize_plaintext(&wire);
        let my_mid = self.my_mid().ok_or(Error::LoginRequired)?;
        let ct_type = msg.content_type;
        let (channel, temp_channel, spec, sid, rid) = if is_group(&msg.to, MidType::of(&msg.to) as i64) {
            let (gk, gkid) = self.group_key(&msg.to, None).await?;
            let info = self.e2ee().groups.get(&msg.to).cloned();
            let (spec, allowed) = info.map_or((2, vec![]), |g| (g.spec_version, g.allowed_types));
            if !allowed.is_empty() && !allowed.contains(&ct_type) {
                return Ok(None);
            }
            let (kid, mine) = self.latest_key()?;
            let ch = self.with_ltsm(|l| -> crate::ltsm::Result<_> {
                let my_pub = l.public_key(mine)?;
                l.create_channel(gk, &my_pub)
            })?;
            (ch, true, spec, kid, gkid)
        } else {
            let p = self.peer(&msg.to).await?;
            if !p.allowed_types.is_empty() && !p.allowed_types.contains(&ct_type) {
                return Ok(None);
            }
            (p.channel, false, p.spec_version, p.my_key_id, p.key_id)
        };
        let seq = self.next_e2ee_seq();
        let sealed = self.with_ltsm(|l| -> Result<Value> {
            let r = match spec {
                1 => l.encrypt_v1(channel, &plaintext).map(|ct| (1u8, ct)),
                2 => l
                    .encrypt_v2(
                        channel,
                        &msg.to,
                        &my_mid,
                        sid as u32,
                        rid as u32,
                        ct_type as u32,
                        seq,
                        &plaintext,
                    )
                    .map(|ct| (2u8, ct)),
                v => {
                    return Err(Error::e2ee(
                        Kind::UnsupportedVersion(v),
                        format!("specVersion {v}"),
                    ));
                }
            };
            if temp_channel {
                l.release_channel(channel)?;
            }
            let (version, ct) = r?;
            Ok(seal_message(
                &wire,
                build_chunks(version, &ct, sid, rid)?,
                version,
            ))
        })?;
        Ok(Some(sealed))
    }

    /// Send a message Letter-Sealed, with the extension's E2EE retry rules
    /// (codes 84/86/87/88/90: re-negotiate and retry, up to 3 times; 99:
    /// recreate the group key and retry; 122: reset and fail). A message
    /// that cannot be sealed is sent plain.
    pub async fn send_sealed(&self, msg: &OutgoingMessage) -> Result<Message> {
        self.send_sealed_inner(msg, false).await
    }

    /// Like [`Client::send_sealed`], but fail instead of sending plain when
    /// the message cannot be sealed (sealed media must never go out plain:
    /// its key would be exposed in `contentMetadata`).
    pub(crate) async fn send_sealed_required(&self, msg: &OutgoingMessage) -> Result<Message> {
        self.send_sealed_inner(msg, true).await
    }

    async fn send_sealed_inner(&self, msg: &OutgoingMessage, required: bool) -> Result<Message> {
        let mut attempt = 0;
        loop {
            let wire = match self.seal_message(msg).await? {
                Some(v) => v,
                None if required => {
                    return Err(Error::e2ee(
                        Kind::NegotiationFailed { mid: msg.to.clone() },
                        format!(
                            "content type {} is not allowed sealed in this chat",
                            msg.content_type
                        ),
                    ));
                }
                None => msg.to_json(),
            };
            let seq = self.next_req_seq();
            match self
                .call("talk/thrift/Talk/TalkService/sendMessage", json!([seq, wire]))
                .await
            {
                Ok(v) => {
                    self.e2ee().sealed_chats.insert(msg.to.clone());
                    return Ok(serde_json::from_value(v)?);
                }
                Err(Error::Api(e)) if e.talk_code == Some(UPDATE_SENDER_KEY) => {
                    return Err(Error::e2ee(
                        Kind::OwnKeyOutdated,
                        format!("server rejected our sender key ({e}); log in by QR again"),
                    ));
                }
                Err(Error::Api(e)) if attempt < 3 => {
                    let code = e.talk_code.unwrap_or(-1);
                    if code == REFRESH_MEDIA_FLOW
                        || !(RETRY_CODES.contains(&code) || code == RECREATE_GROUP_KEY)
                    {
                        if code == REFRESH_MEDIA_FLOW {
                            self.reset_e2ee_chat(&msg.to);
                        }
                        return Err(Error::Api(e));
                    }
                    tracing::info!(code, chat = %msg.to, "E2EE send rejected; resetting and retrying");
                    self.reset_e2ee_chat(&msg.to);
                    if code == RECREATE_GROUP_KEY && is_group(&msg.to, 0) {
                        self.register_group_key(&msg.to).await?;
                    }
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Whether `chat` is known to require Letter Sealing.
    pub(crate) fn chat_is_sealed(&self, chat: &str) -> bool {
        self.e2ee().sealed_chats.contains(chat)
    }

    pub(crate) fn mark_chat_sealed(&self, chat: &str) {
        self.e2ee().sealed_chats.insert(chat.to_owned());
    }

    pub(crate) fn is_sealable(msg: &OutgoingMessage) -> bool {
        AUTO_SEAL.contains(&msg.content_type)
    }

    /// Decrypt a Letter-Sealed message (1:1 or group, V1 or V2, including
    /// our own sent messages). Plain messages are returned unchanged.
    ///
    /// Failures are [`Error::E2ee`] with a specific
    /// [`E2eeErrorKind`](crate::E2eeErrorKind); the input message is never
    /// modified, so callers can keep it and retry later (e.g. after a QR login
    /// that brings newer keys).
    pub async fn decrypt_message(&self, m: &Message) -> Result<Message> {
        let Some(chunks) = m.chunks.as_ref().filter(|c| !c.is_empty()) else {
            return Ok(m.clone());
        };
        if !m.id.is_empty()
            && let Some(d) = self.e2ee().decrypted.get(&m.id)
        {
            return Ok(d.clone());
        }
        let version = message_version(m)?;
        let (ct, sid, rid) =
            parse_chunks(version, chunks).map_err(|e| Error::e2ee(Kind::MalformedMessage, e.to_string()))?;
        let my_mid = self.my_mid().ok_or(Error::LoginRequired)?;
        let (ours, peer_pub) = if is_group(&m.to, m.to_type) {
            let (gk, _) = self.group_key(&m.to, rid).await?;
            (gk, self.user_public_key(&m.from, sid.unwrap_or(0)).await?)
        } else if m.from == my_mid {
            // Our own message (from any of our devices): the counterparty is the recipient.
            (
                self.my_key(sid)?,
                self.user_public_key(&m.to, rid.unwrap_or(0)).await?,
            )
        } else {
            (
                self.my_key(rid)?,
                self.user_public_key(&m.from, sid.unwrap_or(0)).await?,
            )
        };
        let plaintext = self
            .with_ltsm(|l| -> crate::ltsm::Result<Vec<u8>> {
                let ch = l.create_channel(ours, &peer_pub)?;
                let r = if version == 1 {
                    l.decrypt_v1(ch, &ct)
                } else {
                    let (s, r) = (sid.unwrap_or(0) as u32, rid.unwrap_or(0) as u32);
                    l.decrypt_v2(ch, &m.to, &m.from, s, r, m.content_type as u32, &ct)
                };
                l.release_channel(ch)?;
                r
            })
            .map_err(|e| Error::e2ee(Kind::DecryptionFailed, e.0))?;
        let out = apply_plaintext(m, &deserialize_plaintext(&plaintext));
        if !m.id.is_empty() {
            let mut st = self.e2ee();
            if st.decrypted.insert(m.id.clone(), out.clone()).is_none() {
                st.decrypted_order.push_back(m.id.clone());
                if st.decrypted_order.len() > DECRYPT_CACHE
                    && let Some(old) = st.decrypted_order.pop_front()
                {
                    st.decrypted.remove(&old);
                }
            }
        }
        Ok(out)
    }

    /// Compare our keychain with the keys the server has registered for the
    /// account (`getE2EEPublicKeysEx`).
    pub async fn e2ee_status(&self) -> Result<E2eeStatus> {
        let v = self
            .talk_e2ee(
                "getE2EEPublicKeysEx",
                json!([true, crate::models::sync_reason::INITIALIZATION]),
            )
            .await?;
        let server_key_ids = v
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|k| k.get("keyId").and_then(value_i64))
                    .collect()
            })
            .unwrap_or_default();
        let local_key_ids = self.e2ee().my_keys.keys().copied().collect();
        Ok(E2eeStatus {
            local_key_ids,
            server_key_ids,
        })
    }

    /// Let the E2EE caches react to an operation: key-change and membership
    /// operations drop the cached negotiation / group key of the chats they
    /// mention. The event stream calls this for every operation.
    pub fn note_operation(&self, op: &crate::models::Operation) {
        if !INVALIDATING_OPS.contains(&op.r#type) {
            return;
        }
        let known: Vec<String> = {
            let st = self.e2ee();
            [&op.param1, &op.param2, &op.param3]
                .into_iter()
                .flatten()
                .filter(|p| st.peers.contains_key(*p) || st.groups.contains_key(*p))
                .cloned()
                .collect()
        };
        for chat in known {
            tracing::debug!(op = op.r#type, %chat, "dropping cached E2EE state");
            self.reset_e2ee_chat(&chat);
        }
    }
}

/// Merge decrypted plaintext back into the message (the extension's `yL`).
pub(crate) fn apply_plaintext(m: &Message, plain: &Value) -> Message {
    let mut out = m.clone();
    out.chunks = None;
    if let Some(t) = plain
        .get("text")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        out.text = Some(t.to_owned());
    }
    if let Some(l) = plain.get("location").filter(|v| !v.is_null()) {
        out.extra.insert("location".into(), l.clone());
    }
    let mut put = |key: &str, field: &str| {
        if let Some(v) = plain.get(field).filter(|v| !v.is_null()) {
            let s = v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string());
            out.content_metadata.insert(key.into(), s);
        }
    };
    put("REPLACE", "REPLACE");
    put("ENC_KM", "keyMaterial");
    put("FILE_NAME", "fileName");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_round_trip_and_order() {
        let msg = json!({"to": "u1", "text": "สวัสดี \"q\"", "location": null,
                         "contentMetadata": {"REPLACE": "{\"sticon\":{}}"}});
        let pt = serialize_plaintext(&msg);
        assert_eq!(
            String::from_utf8(pt.clone()).unwrap(),
            r#"{"text":"สวัสดี \"q\"","REPLACE":{"sticon":{}}}"#
        );
        assert_eq!(deserialize_plaintext(&pt)["text"], "สวัสดี \"q\"");
    }

    #[test]
    fn raw_control_chars_are_tolerated() {
        let v = deserialize_plaintext(b"{\"text\":\"a\nb\tc\"}");
        assert_eq!(v["text"], "a\nb\tc");
        assert_eq!(deserialize_plaintext(b"not json")["text"], "not json");
    }

    #[test]
    fn v2_chunks_reorder_and_round_trip() {
        let ct: Vec<u8> = (0u8..60).collect();
        let chunks = build_chunks(2, &ct, 5714523, 7).unwrap();
        assert_eq!(B64.decode(&chunks[0]).unwrap(), &ct[0..16]);
        assert_eq!(B64.decode(&chunks[1]).unwrap(), &ct[28..]);
        assert_eq!(B64.decode(&chunks[2]).unwrap(), &ct[16..28]);
        assert_eq!(chunks[3], B64.encode(5714523u32.to_be_bytes()));
        assert_eq!(
            parse_chunks(2, &chunks).unwrap(),
            (ct.clone(), Some(5714523), Some(7))
        );
        let v1 = build_chunks(1, &ct, 1, 2).unwrap();
        assert_eq!(parse_chunks(1, &v1).unwrap(), (ct, Some(1), Some(2)));
        assert_eq!(parse_chunks(2, &chunks[..3]).unwrap().1, None);
    }

    #[test]
    fn sealed_message_shape() {
        let msg = OutgoingMessage::text("u1", "hi").to_json();
        let s = seal_message(&msg, vec!["a".into(), "b".into(), "c".into()], 2);
        assert!(s.get("text").is_none());
        assert_eq!(s["contentMetadata"]["e2eeVersion"], "2");
        assert_eq!(s["chunks"], json!(["a", "b", "c"]));
        assert_eq!(s["contentType"], 0);
    }

    #[test]
    fn sealed_reply_keeps_relation_outside() {
        let msg = OutgoingMessage::reply("Cg", "hi", "42").to_json();
        assert_eq!(serialize_plaintext(&msg), br#"{"text":"hi"}"#.to_vec());
        let s = seal_message(&msg, vec!["a".into(), "b".into(), "c".into()], 2);
        assert_eq!(
            (&s["relatedMessageId"], &s["messageRelationType"]),
            (&json!("42"), &json!(3))
        );
        assert_eq!(s["contentMetadata"]["message_relation_type_code"], "reply");
    }

    #[test]
    fn apply_plaintext_restores_fields() {
        let m = Message {
            chunks: Some(vec!["x".into(); 5]),
            ..Default::default()
        };
        let out = apply_plaintext(&m, &json!({"text": "hello", "REPLACE": {"a": 1}}));
        assert_eq!(out.text.as_deref(), Some("hello"));
        assert_eq!(out.content_metadata["REPLACE"], r#"{"a":1}"#);
        assert!(!out.is_e2ee());
    }
}
