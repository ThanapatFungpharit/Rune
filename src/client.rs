//! The public client.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::ltsm::Ltsm;
use crate::models::{
    Chat, Contact, Message, OutgoingMessage, Profile, chat_attribute, contact_setting, sync_reason, value_i64,
};
use crate::transport::{CallOpts, Config, Tokens, Transport};

/// `getContactsV2` batch size (the extension's fallback for `limit.sync.contacts`).
const CONTACTS_BATCH: usize = 100;
/// `getChats` batch size used by the reference client.
const CHATS_BATCH: usize = 100;

/// Predefined message reactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Nice = 2,
    Love = 3,
    Fun = 4,
    Amazing = 5,
    Sad = 6,
    Omg = 7,
}

impl Reaction {
    pub const ALL: [Reaction; 6] = [
        Reaction::Nice,
        Reaction::Love,
        Reaction::Fun,
        Reaction::Amazing,
        Reaction::Sad,
        Reaction::Omg,
    ];

    /// From `predefinedReactionType`.
    pub fn from_i64(n: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|r| *r as i64 == n)
    }

    /// The emoji the official apps draw for it.
    pub fn emoji(self) -> &'static str {
        match self {
            Reaction::Nice => "👍",
            Reaction::Love => "❤️",
            Reaction::Fun => "😂",
            Reaction::Amazing => "🤩",
            Reaction::Sad => "😢",
            Reaction::Omg => "😱",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Reaction::Nice => "Nice",
            Reaction::Love => "Love",
            Reaction::Fun => "Fun",
            Reaction::Amazing => "Amazing",
            Reaction::Sad => "Sad",
            Reaction::Omg => "OMG",
        }
    }
}

/// Who has read how far, from `getMessageReadRange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRange {
    pub chat: String,
    pub reader: String,
    pub start_message_id: String,
    pub end_message_id: String,
    /// Epoch ms of the newest read message.
    pub end_time: i64,
}

/// An owned sticker package (`getOwnedProductSummaries`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerPackage {
    pub id: String,
    pub name: String,
    /// `STKVER` to send with its stickers.
    pub version: u32,
    pub sticker_ids: Vec<String>,
    /// Animated / sound stickers (`stickerResourceType` other than 1).
    pub animated: bool,
}

/// An owned LINE emoji package (`getOwnedProductSummaries("sticonshop")`
/// plus its public `meta.json`), whose emoji can be sent as reactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmojiPackage {
    pub id: String,
    pub name: String,
    /// `latestVersion`, sent as the reaction's `version`.
    pub version: u32,
    /// `sticonResourceType`, sent as the reaction's `resourceType`.
    pub resource_type: i64,
    /// `(emojiId, alt text)` in the shop's order.
    pub emojis: Vec<(String, String)>,
}

impl EmojiPackage {
    /// The emoji listed in a package's `meta.json` (`orders` and `altTexts`).
    pub fn emojis_from_meta(meta: &Value) -> Vec<(String, String)> {
        let alts = meta.get("altTexts");
        meta.get("orders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| {
                let alt = alts
                    .and_then(|a| a.get(id))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // Some alt texts carry no-break spaces (live).
                (id.to_owned(), alt.replace('\u{a0}', " "))
            })
            .collect()
    }
}

/// How long before LINE's suggested refresh time the refresher renews:
/// a tenth of the token's refresh period, at most ten minutes.
const MAX_REFRESH_LEAD_SECS: i64 = 600;
/// Waits are cut into slices this long and re-checked against the wall
/// clock, so a machine waking from sleep refreshes promptly.
const REFRESH_SLICE: std::time::Duration = std::time::Duration::from_secs(60);
/// When the server gives no schedule even after a refresh.
const REFRESH_FALLBACK: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

#[cfg(test)]
mod refresh_tests {
    use super::*;

    #[test]
    fn renews_a_little_before_the_suggested_time() {
        let t = |issue, period| Tokens {
            token_issue_time_epoch_sec: Some(issue),
            duration_until_refresh_in_sec: Some(period),
            ..Default::default()
        };
        let secs = |w: Option<std::time::Duration>| w.map(|d| d.as_secs());
        // 1 h period: renew 6 min early.
        assert_eq!(secs(refresh_wait(&t(1000, 3600), 1000)), Some(3600 - 360));
        // Long periods: at most 10 min early.
        assert_eq!(secs(refresh_wait(&t(0, 86_400), 0)), Some(86_400 - 600));
        // Overdue (e.g. after sleep or a restart): now.
        assert_eq!(secs(refresh_wait(&t(0, 3600), 99_999)), Some(0));
        assert_eq!(refresh_wait(&Tokens::default(), 0), None);
    }
}

fn now_epoch_sec() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How long to wait before renewing `t` at `now` (unix seconds); `None`
/// when the tokens carry no refresh schedule.
pub(crate) fn refresh_wait(t: &Tokens, now: i64) -> Option<std::time::Duration> {
    let due = t.refresh_due_epoch_sec()?;
    let period = t.duration_until_refresh_in_sec.unwrap_or(0).max(0);
    let at = due - (period / 10).min(MAX_REFRESH_LEAD_SECS);
    Some(std::time::Duration::from_secs((at - now).max(0) as u64))
}

/// An unofficial client for LINE's Chrome-extension gateway.
///
/// Cheap to clone; clones share the connection pool, tokens and the secure
/// module. Construct with [`Client::new`] then log in (see
/// [`Client::qr_login`](crate::Client::qr_login)), or restore a saved
/// [`Session`](crate::Session).
#[derive(Clone)]
pub struct Client {
    pub(crate) t: Arc<Transport>,
    req_seq: Arc<AtomicI64>,
    pub(crate) e2ee: Arc<std::sync::Mutex<crate::e2ee::E2eeState>>,
    pub(crate) media: Arc<std::sync::Mutex<crate::media::MediaState>>,
    pub(crate) timeline: Arc<std::sync::Mutex<crate::timeline::TimelineState>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("tokens", &self.t.tokens())
            .finish_non_exhaustive()
    }
}

fn parse<T: DeserializeOwned>(v: Value) -> Result<T> {
    Ok(serde_json::from_value(v)?)
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

impl Client {
    /// A client that is not logged in yet.
    ///
    /// Compiles the bundled `ltsm.wasm` (≈0.3 s in release builds); do this
    /// once and clone the client.
    pub fn new(config: Config) -> Result<Self> {
        Self::with_tokens(config, Tokens::default())
    }

    /// A client using existing credentials.
    pub fn with_tokens(config: Config, tokens: Tokens) -> Result<Self> {
        Self::from_parts(config, tokens, Ltsm::new()?)
    }

    pub(crate) fn from_parts(config: Config, tokens: Tokens, ltsm: Ltsm) -> Result<Self> {
        // reqSeq de-duplicates requests server-side. The reference client
        // restarts at 1 each process; seeding from the clock avoids replaying
        // sequence numbers across restarts (inferred safeguard, int32 range).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() % 1_000_000) as i64 * 1000)
            .unwrap_or(0);
        Ok(Client {
            t: Arc::new(Transport::new(config, tokens, ltsm)?),
            req_seq: Arc::new(AtomicI64::new(seed)),
            e2ee: Arc::default(),
            media: Arc::default(),
            timeline: Arc::default(),
        })
    }

    /// A snapshot of the current credentials (secret material).
    pub fn tokens(&self) -> Tokens {
        self.t.tokens()
    }

    /// Notified whenever tokens change (login, refresh) — persist the session there.
    pub fn subscribe_tokens(&self) -> watch::Receiver<Tokens> {
        self.t.tokens_tx.subscribe()
    }

    /// Our own mid, once logged in.
    pub fn my_mid(&self) -> Option<String> {
        self.t.tokens().mid
    }

    pub fn config(&self) -> &Config {
        &self.t.config
    }

    pub fn is_logged_in(&self) -> bool {
        self.t.access_token().is_some()
    }

    /// Next request sequence number.
    pub fn next_req_seq(&self) -> i64 {
        self.req_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Call any Thrift method by its path below `/api/`, with positional
    /// arguments, e.g. `call("talk/thrift/Talk/TalkService/getProfile", json!([2]))`.
    /// See [`crate::endpoints`] for the known methods.
    pub async fn call(&self, method: &str, args: Value) -> Result<Value> {
        self.t.call(method, args).await
    }

    /// Force a token refresh now.
    pub async fn refresh_token(&self) -> Result<()> {
        self.t.refresh_token().await
    }

    /// Renew the access token ahead of time, forever: sleeps until shortly
    /// before LINE's suggested refresh time (`tokenIssueTimeEpochSec` +
    /// `durationUntilRefreshInSec`), refreshes, and repeats. Without this the
    /// token is only renewed when the server rejects it (code 119).
    ///
    /// Waits follow the wall clock (a laptop waking from sleep renews right
    /// away) and restart whenever the tokens change. Transient failures are
    /// retried with backoff. While logged out it waits for a login. Returns
    /// only when a new login is required ([`Error::Reauth`]).
    pub async fn run_token_refresher(&self) -> Result<()> {
        let mut rx = self.subscribe_tokens();
        let mut failures = 0u32;
        let mut probed_unknown = false;
        loop {
            let t = self.tokens();
            if t.refresh_token.is_none() || t.access_token.is_none() {
                // Not logged in (yet, or any more): wait for a login.
                if rx.changed().await.is_err() {
                    return Ok(());
                }
                continue;
            }
            let wait = match refresh_wait(&t, now_epoch_sec()) {
                Some(w) => w,
                // No schedule: one refresh tells us LINE's.
                None if !probed_unknown => {
                    probed_unknown = true;
                    std::time::Duration::ZERO
                }
                None => REFRESH_FALLBACK,
            };
            if !wait.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(wait.min(REFRESH_SLICE)) => {}
                    r = rx.changed() => if r.is_err() { return Ok(()) },
                }
                continue; // re-check against the clock / new tokens
            }
            match self.refresh_token().await {
                Ok(()) => {
                    failures = 0;
                    rx.mark_unchanged();
                    let next = refresh_wait(&self.tokens(), now_epoch_sec());
                    if next.is_some() {
                        probed_unknown = false;
                    }
                    // A renewed token that is already due (clock skew, a zero
                    // period) must not turn into back-to-back refreshes.
                    if next.is_some_and(|w| w.is_zero()) {
                        tracing::warn!(
                            "the renewed token is already due for renewal; waiting {REFRESH_SLICE:?}"
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(REFRESH_SLICE) => {}
                            r = rx.changed() => if r.is_err() { return Ok(()) },
                        }
                    }
                }
                Err(e @ (Error::Reauth(_) | Error::LoginRequired)) => return Err(e),
                Err(e) => {
                    let backoff = std::time::Duration::from_secs((30u64 << failures.min(5)).min(900));
                    failures += 1;
                    tracing::warn!("scheduled token refresh failed ({e}); retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// [`run_token_refresher`](Self::run_token_refresher) on a Tokio task.
    pub fn spawn_token_refresher(&self) -> tokio::task::JoinHandle<Result<()>> {
        let c = self.clone();
        tokio::spawn(async move { c.run_token_refresher().await })
    }

    /// Run a closure against the secure module (E2EE primitives, signing).
    pub fn with_ltsm<R>(&self, f: impl FnOnce(&mut Ltsm) -> R) -> R {
        f(&mut self.t.ltsm.lock().unwrap())
    }

    async fn talk(&self, method: &str, args: Value) -> Result<Value> {
        self.t
            .call(&format!("talk/thrift/Talk/TalkService/{method}"), args)
            .await
    }

    // -- account ---------------------------------------------------------------

    /// Our profile. Also records our mid in the tokens if it was unknown.
    pub async fn profile(&self) -> Result<Profile> {
        let p: Profile = parse(
            self.talk("getProfile", json!([sync_reason::INITIALIZATION]))
                .await?,
        )?;
        if self.my_mid().is_none() && !p.mid.is_empty() {
            self.t.update_tokens(|t| {
                t.mid = Some(p.mid.clone());
                Ok(())
            })?;
        }
        Ok(p)
    }

    /// Server time in epoch milliseconds.
    pub async fn server_time(&self) -> Result<i64> {
        let v = self.talk("getServerTime", json!([])).await?;
        value_i64(&v).ok_or_else(|| Error::Protocol(format!("getServerTime: {v}")))
    }

    /// Account settings (raw `Settings` struct).
    pub async fn settings(&self) -> Result<Value> {
        self.talk("getSettings", json!([sync_reason::INITIALIZATION]))
            .await
    }

    /// `updateProfileAttributes`. `attributes` maps `ProfileAttribute` enum
    /// values (as strings, e.g. `"2"` = DISPLAY_NAME, `"16"` = STATUS_MESSAGE)
    /// to `{"value": ..., "meta": {}}` objects.
    pub async fn update_profile_attributes(&self, attributes: Value) -> Result<Value> {
        let seq = self.next_req_seq();
        self.talk(
            "updateProfileAttributes",
            json!([seq, { "profileAttributes": attributes }]),
        )
        .await
    }

    /// Read selected settings (`getSettingsAttributes2`); see
    /// [`crate::models::settings_attribute`]. Fields not asked for come back
    /// with default values, so read only the ones you asked for.
    pub async fn settings_attributes(&self, attributes: &[i64]) -> Result<Value> {
        self.talk("getSettingsAttributes2", json!([attributes])).await
    }

    /// Change account settings (`updateSettingsAttributes2`): `settings` is
    /// a partial `Settings` struct holding the fields named by `attributes`,
    /// e.g. `(&[2], json!({"notificationNewMessage": false}))`. These are the
    /// account's settings, shared by all its devices.
    pub async fn update_settings(&self, attributes: &[i64], settings: Value) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk("updateSettingsAttributes2", json!([seq, attributes, settings]))
            .await
            .map(drop)
    }

    /// Change our display name (`ProfileAttribute` 2).
    pub async fn set_display_name(&self, name: &str) -> Result<()> {
        self.update_profile_attributes(json!({ "2": { "value": name, "meta": {} } }))
            .await
            .map(drop)
    }

    /// Change our status message (`ProfileAttribute` 16).
    pub async fn set_status_message(&self, text: &str) -> Result<()> {
        self.update_profile_attributes(json!({ "16": { "value": text, "meta": {} } }))
            .await
            .map(drop)
    }

    /// Log out this device (`logoutV2`) and forget the tokens.
    pub async fn logout(&self) -> Result<()> {
        self.t
            .call("talk/thrift/Talk/AuthService/logoutV2", json!([]))
            .await?;
        self.t.update_tokens(|t| {
            *t = Tokens::default();
            Ok(())
        })
    }

    // -- contacts --------------------------------------------------------------

    /// Mids of all friends.
    pub async fn contact_ids(&self) -> Result<Vec<String>> {
        Ok(str_list(
            &self
                .talk("getAllContactIds", json!([sync_reason::FULL_SYNC]))
                .await?,
        ))
    }

    /// Contacts by mid (batched automatically).
    pub async fn contacts(&self, mids: &[String]) -> Result<HashMap<String, Contact>> {
        let mut out = HashMap::new();
        for batch in mids.chunks(CONTACTS_BATCH) {
            let v = self
                .talk(
                    "getContactsV2",
                    json!([{ "targetUserMids": batch, "neededContactCalendarEvents": [] }, sync_reason::FULL_SYNC]),
                )
                .await?;
            if let Some(map) = v.get("contacts").and_then(Value::as_object) {
                for (mid, wrapper) in map {
                    // A user that no longer exists comes back as only
                    // `{snapshotTimeMillis, userStatus}` (live): no contact.
                    let c = match wrapper.get("contact") {
                        Some(c) => c.clone(),
                        None if wrapper.get("mid").is_some() => wrapper.clone(),
                        None => {
                            tracing::debug!("getContactsV2: no contact for {mid}");
                            continue;
                        }
                    };
                    // One odd entry must not lose the whole batch.
                    match parse(c) {
                        Ok(c) => drop(out.insert(mid.clone(), c)),
                        Err(e) => tracing::warn!("getContactsV2: skipping {mid}: {e}"),
                    }
                }
            }
        }
        Ok(out)
    }

    /// Look a user up by their LINE ID.
    pub async fn find_contact_by_userid(&self, userid: &str) -> Result<Contact> {
        parse(self.talk("findContactByUserid", json!([userid])).await?)
    }

    /// Add a friend (`Relation.RelationService.addFriendByMid`).
    pub async fn add_friend(&self, mid: &str) -> Result<Value> {
        let seq = self.next_req_seq();
        self.t
            .call(
                "talk/thrift/Relation/RelationService/addFriendByMid",
                json!([{ "reqSeq": seq, "userMid": mid,
                         "tracking": { "reference": "", "trackingMetaV2": { "friendRecommendation": {} } } }]),
            )
            .await
    }

    pub async fn block_contact(&self, mid: &str) -> Result<()> {
        self.talk("blockContact", json!([self.next_req_seq(), mid]))
            .await
            .map(drop)
    }

    pub async fn unblock_contact(&self, mid: &str) -> Result<()> {
        self.talk("unblockContact", json!([self.next_req_seq(), mid, ""]))
            .await
            .map(drop)
    }

    pub async fn blocked_contact_ids(&self) -> Result<Vec<String>> {
        Ok(str_list(
            &self
                .talk("getBlockedContactIds", json!([sync_reason::FULL_SYNC]))
                .await?,
        ))
    }

    /// `updateContactSetting(reqSeq, mid, flag, value)`; flags are in
    /// [`crate::models::contact_setting`].
    pub async fn update_contact_setting(&self, mid: &str, flag: i64, value: &str) -> Result<()> {
        self.talk(
            "updateContactSetting",
            json!([self.next_req_seq(), mid, flag, value]),
        )
        .await
        .map(drop)
    }

    pub async fn favorite_mids(&self) -> Result<Vec<String>> {
        Ok(str_list(&self.talk("getFavoriteMids", json!([])).await?))
    }

    // -- chats -------------------------------------------------------------------

    /// `(member_chat_mids, invited_chat_mids)`.
    pub async fn chat_mids(&self) -> Result<(Vec<String>, Vec<String>)> {
        let v = self
            .talk(
                "getAllChatMids",
                json!([{ "withMemberChats": true, "withInvitedChats": true }, sync_reason::FULL_SYNC]),
            )
            .await?;
        Ok((str_list(&v["memberChatMids"]), str_list(&v["invitedChatMids"])))
    }

    /// Group chats we have been invited to but not joined yet.
    /// Accept with [`accept_chat_invitation`](Self::accept_chat_invitation),
    /// decline with [`reject_chat_invitation`](Self::reject_chat_invitation).
    pub async fn invited_chats(&self) -> Result<Vec<Chat>> {
        let (_, invited) = self.chat_mids().await?;
        if invited.is_empty() {
            return Ok(Vec::new());
        }
        self.chats(&invited).await
    }

    /// Group chats by mid, with members and invitees (batched).
    pub async fn chats(&self, mids: &[String]) -> Result<Vec<Chat>> {
        let mut out = Vec::new();
        for batch in mids.chunks(CHATS_BATCH) {
            let v = self
                .talk(
                    "getChats",
                    json!([{ "chatMids": batch, "withMembers": true, "withInvitees": true }, sync_reason::FULL_SYNC]),
                )
                .await?;
            if let Some(a) = v.get("chats").and_then(Value::as_array) {
                for c in a {
                    out.push(parse(c.clone())?);
                }
            }
        }
        Ok(out)
    }

    /// Create a group chat (`type` 0 = GROUP).
    pub async fn create_chat(&self, name: &str, members: &[String]) -> Result<Value> {
        let seq = self.next_req_seq();
        self.talk(
            "createChat",
            json!([{ "reqSeq": seq, "type": 0, "name": name, "targetUserMids": members }]),
        )
        .await
    }

    /// Create a group with `members` invited, returning it (`createChat`
    /// answers `{chat: Chat}`). Visible to the invitees.
    ///
    /// The name is trimmed and the member list deduplicated (blank mids are
    /// dropped); an empty name, no members, or an answer without a chat mid
    /// is an error rather than a phantom group.
    pub async fn create_group(&self, name: &str, members: &[String]) -> Result<Chat> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Protocol("createChat: the group needs a name".into()));
        }
        let mut targets: Vec<String> = Vec::with_capacity(members.len());
        for m in members.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
            if !targets.iter().any(|t| t == m) {
                targets.push(m.to_owned());
            }
        }
        if targets.is_empty() {
            return Err(Error::Protocol("createChat: invite at least one member".into()));
        }
        let v = self.create_chat(name, &targets).await?;
        let mut chat: Chat = parse(v.get("chat").cloned().unwrap_or(v))?;
        if chat.chat_mid.trim().is_empty() {
            return Err(Error::Protocol("createChat: the answer has no chat mid".into()));
        }
        if chat.chat_name.is_empty() {
            chat.chat_name = name.to_owned();
        }
        Ok(chat)
    }

    /// `updateChat` with one changed attribute ([`crate::models::chat_attribute`]);
    /// `chat` is a partial `Chat` struct carrying `chatMid` and the field.
    pub async fn update_chat(&self, chat: Value, attribute: i64) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            "updateChat",
            json!([{ "reqSeq": seq, "chat": chat, "updatedAttribute": attribute }]),
        )
        .await
        .map(drop)
    }

    /// Rename a group (visible to its members).
    pub async fn rename_chat(&self, chat: &str, name: &str) -> Result<()> {
        self.update_chat(
            json!({ "chatMid": chat, "chatName": name, "type": 0 }),
            chat_attribute::NAME,
        )
        .await
    }

    /// Turn notifications for a chat off (`muted`) or on. Groups use
    /// `updateChat` (`notificationDisabled`), 1:1 chats the friend's contact
    /// setting. This is LINE's own per-chat setting, shared by all devices.
    pub async fn set_chat_muted(&self, chat: &str, muted: bool) -> Result<()> {
        match crate::models::MidType::of(chat) {
            crate::models::MidType::User => {
                self.update_contact_setting(chat, contact_setting::NOTIFICATION_DISABLE, &muted.to_string())
                    .await
            }
            _ => {
                self.update_chat(
                    json!({ "chatMid": chat, "type": 0, "notificationDisabled": muted }),
                    chat_attribute::NOTIFICATION_SETTING,
                )
                .await
            }
        }
    }

    async fn chat_members_op(&self, method: &str, chat: &str, mids: &[String]) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            method,
            json!([{ "reqSeq": seq, "chatMid": chat, "targetUserMids": mids }]),
        )
        .await
        .map(drop)
    }

    pub async fn invite_into_chat(&self, chat: &str, mids: &[String]) -> Result<()> {
        self.chat_members_op("inviteIntoChat", chat, mids).await
    }

    pub async fn kick_from_chat(&self, chat: &str, mids: &[String]) -> Result<()> {
        self.chat_members_op("deleteOtherFromChat", chat, mids).await
    }

    pub async fn cancel_chat_invitation(&self, chat: &str, mids: &[String]) -> Result<()> {
        self.chat_members_op("cancelChatInvitation", chat, mids).await
    }

    pub async fn leave_chat(&self, chat: &str) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk("deleteSelfFromChat", json!([{ "reqSeq": seq, "chatMid": chat }]))
            .await
            .map(drop)
    }

    /// Join a group chat we were invited to (visible to its members).
    pub async fn accept_chat_invitation(&self, chat: &str) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            "acceptChatInvitation",
            json!([{ "reqSeq": seq, "chatMid": chat }]),
        )
        .await
        .map(drop)
    }

    /// Decline a group chat invitation (it disappears from
    /// [`invited_chats`](Self::invited_chats)).
    pub async fn reject_chat_invitation(&self, chat: &str) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            "rejectChatInvitation",
            json!([{ "reqSeq": seq, "chatMid": chat }]),
        )
        .await
        .map(drop)
    }

    // -- messages ----------------------------------------------------------------

    /// Send a message. Returns the server's copy (with `id` and `createdTime`).
    ///
    /// Chats that enforce Letter Sealing reject plain messages with
    /// `TalkException` 82; when Letter Sealing keys are loaded, text and
    /// location messages are then sealed and re-sent automatically, and later
    /// sends to that chat are sealed up front. Other content types (stickers,
    /// contacts) are always sent plain, like the extension.
    pub async fn send_message(&self, msg: &OutgoingMessage) -> Result<Message> {
        let sealable = Self::is_sealable(msg) && self.e2ee_ready();
        if sealable && self.chat_is_sealed(&msg.to) {
            return self.send_sealed(msg).await;
        }
        let seq = self.next_req_seq();
        match self.talk("sendMessage", json!([seq, msg.to_json()])).await {
            Ok(v) => parse(v),
            Err(Error::Api(e)) if e.requires_e2ee() && sealable => {
                self.mark_chat_sealed(&msg.to);
                self.send_sealed(msg).await
            }
            Err(e) => Err(e),
        }
    }

    /// Send a text message.
    pub async fn send_text(&self, to: &str, text: &str) -> Result<Message> {
        self.send_message(&OutgoingMessage::text(to, text)).await
    }

    /// Send a text reply quoting `message_id` (sealed automatically like
    /// [`send_message`](Self::send_message)).
    pub async fn send_reply(&self, to: &str, text: &str, message_id: &str) -> Result<Message> {
        self.send_message(&OutgoingMessage::reply(to, text, message_id))
            .await
    }

    /// Recall (unsend) one of our messages.
    pub async fn unsend_message(&self, message_id: &str) -> Result<()> {
        self.talk("unsendMessage", json!([self.next_req_seq(), message_id]))
            .await
            .map(drop)
    }

    pub async fn react(&self, message_id: &str, reaction: Reaction) -> Result<()> {
        self.react_raw(message_id, json!({ "predefinedReactionType": reaction as i64 }))
            .await
    }

    /// React with a LINE emoji from an owned package (`paidReactionType`,
    /// accepted from this client live, 2026-11-15).
    pub async fn react_emoji(&self, message_id: &str, package: &EmojiPackage, emoji_id: &str) -> Result<()> {
        self.react_raw(
            message_id,
            json!({ "paidReactionType": {
                "productId": package.id, "emojiId": emoji_id,
                "resourceType": package.resource_type, "version": package.version.to_string() } }),
        )
        .await
    }

    async fn react_raw(&self, message_id: &str, reaction_type: Value) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            "react",
            json!([{ "reqSeq": seq, "messageId": message_id, "reactionType": reaction_type }]),
        )
        .await
        .map(drop)
    }

    pub async fn cancel_reaction(&self, message_id: &str) -> Result<()> {
        let seq = self.next_req_seq();
        self.talk(
            "cancelReaction",
            json!([{ "reqSeq": seq, "messageId": message_id }]),
        )
        .await
        .map(drop)
    }

    /// Mark `chat` as read up to `last_message_id`.
    pub async fn mark_read(&self, chat: &str, last_message_id: &str) -> Result<()> {
        self.talk(
            "sendChatChecked",
            json!([self.next_req_seq(), chat, last_message_id, 0]),
        )
        .await
        .map(drop)
    }

    /// The newest `count` messages of a chat (message box id = chat mid).
    pub async fn recent_messages(&self, chat: &str, count: u32) -> Result<Vec<Message>> {
        parse(self.talk("getRecentMessagesV2", json!([chat, count])).await?)
    }

    /// Messages older than (`end_message_id`, `delivered_time`).
    pub async fn previous_messages(
        &self,
        chat: &str,
        end_message_id: &str,
        delivered_time: i64,
        count: u32,
    ) -> Result<Vec<Message>> {
        parse(
            self.talk(
                "getPreviousMessagesV2WithRequest",
                json!([{ "messageBoxId": chat,
                         "endMessageId": { "messageId": end_message_id, "deliveredTime": delivered_time },
                         "messagesCount": count }, sync_reason::OPERATION]),
            )
            .await?,
        )
    }

    /// Read ranges of chats (`getMessageReadRange([chatIds], syncReason)` →
    /// `[{chatId, ranges: {mid: [{startMessageId, endMessageId, startTime, endTime}]}}]`).
    ///
    /// Live, the gateway answered `[]` for every chat tried (2026-11-14), so
    /// read state is mostly learnt from `NOTIFIED_READ_MESSAGE` operations.
    pub async fn message_read_ranges(&self, chats: &[String]) -> Result<Vec<ReadRange>> {
        let v = self
            .talk("getMessageReadRange", json!([chats, sync_reason::OPERATION]))
            .await?;
        let s = |v: &Value, k: &str| match v.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(n @ Value::Number(_)) => n.to_string(),
            _ => String::new(),
        };
        let mut out = Vec::new();
        for c in v.as_array().into_iter().flatten() {
            let chat = s(c, "chatId");
            for (reader, entries) in c.get("ranges").and_then(Value::as_object).into_iter().flatten() {
                // The newest range ends at the reader's last read message.
                let newest =
                    entries.as_array().into_iter().flatten().max_by(|a, b| {
                        crate::ops::message_id_cmp(&s(a, "endMessageId"), &s(b, "endMessageId"))
                    });
                if let Some(e) = newest {
                    out.push(ReadRange {
                        chat: chat.clone(),
                        reader: reader.clone(),
                        start_message_id: s(e, "startMessageId"),
                        end_message_id: s(e, "endMessageId"),
                        end_time: e.get("endTime").and_then(value_i64).unwrap_or(0),
                    });
                }
            }
        }
        Ok(out)
    }

    /// The chat list (`getMessageBoxes`), raw: the active chats only.
    pub async fn message_boxes(&self, limit: u32) -> Result<Value> {
        self.message_boxes_page(None, limit, true).await
    }

    /// One page of `getMessageBoxes`, raw (`{"messageBoxes": [...], "hasNext": bool}`).
    /// `after` is the last box id of the previous page (exclusive cursor).
    /// With `active_only` false the list also holds the inactive chats (hidden
    /// from the chat list, no `lastMessages`; `lastDeliveredMessageId` is then
    /// `{"messageId", "deliveredTime"}`) — live, see PROTOCOL.md.
    pub async fn message_boxes_page(
        &self,
        after: Option<&str>,
        limit: u32,
        active_only: bool,
    ) -> Result<Value> {
        self.talk(
            "getMessageBoxes",
            json!([{ "minChatId": after, "activeOnly": active_only, "unreadOnly": false,
                     "messageBoxCountLimit": limit, "withUnreadCount": true,
                     "lastMessagesPerMessageBoxCount": 1 }, sync_reason::INITIALIZATION]),
        )
        .await
    }

    /// Current operation revision (event-stream cursor).
    pub async fn last_op_revision(&self) -> Result<i64> {
        let v = self.talk("getLastOpRevision", json!([])).await?;
        value_i64(&v).ok_or_else(|| Error::Protocol(format!("getLastOpRevision: {v}")))
    }

    // -- stickers -----------------------------------------------------------------

    /// Sticker packages this account owns (`ShopService.getOwnedProductSummaries`),
    /// with their sticker ids (from `stickerIdRanges`, live).
    pub async fn owned_sticker_packages(&self) -> Result<Vec<StickerPackage>> {
        const PAGE: i64 = 200;
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let v = self
                .call(
                    "shop/thrift/ShopService/ShopService/getOwnedProductSummaries",
                    json!(["stickershop", offset, PAGE, { "language": "en", "country": "JP" }]),
                )
                .await?;
            let list = v
                .get("productList")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for p in &list {
                let summary = &p["productTypeSummary"]["stickerSummary"];
                let mut ids = Vec::new();
                for r in summary
                    .get("stickerIdRanges")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let start = r.get("start").and_then(value_i64).unwrap_or(0);
                    let size = r.get("size").and_then(value_i64).unwrap_or(0).clamp(0, 500);
                    ids.extend((start..start + size).map(|i| i.to_string()));
                }
                if ids.is_empty() {
                    continue;
                }
                out.push(StickerPackage {
                    id: p
                        .get("id")
                        .map(|i| match i {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default(),
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    version: p.get("latestVersion").and_then(value_i64).unwrap_or(1).max(1) as u32,
                    sticker_ids: ids,
                    animated: summary
                        .get("stickerResourceType")
                        .and_then(value_i64)
                        .unwrap_or(1)
                        != 1,
                });
            }
            let total = v.get("totalSize").and_then(value_i64).unwrap_or(0);
            offset += list.len() as i64;
            if list.is_empty() || offset >= total {
                return Ok(out);
            }
        }
    }

    /// LINE emoji packages this account owns (`getOwnedProductSummaries`
    /// with `"sticonshop"`; every account has "LINE Basic Emoji"), each with
    /// its emoji list from the sticker CDN's `meta.json`. A package whose
    /// list cannot be fetched is left out; if all fail, the first error is returned.
    pub async fn owned_emoji_packages(&self) -> Result<Vec<EmojiPackage>> {
        const PAGE: i64 = 200;
        let mut summaries = Vec::new();
        let mut offset = 0;
        loop {
            let v = self
                .call(
                    "shop/thrift/ShopService/ShopService/getOwnedProductSummaries",
                    json!(["sticonshop", offset, PAGE, { "language": "en", "country": "JP" }]),
                )
                .await?;
            let list = v
                .get("productList")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = v.get("totalSize").and_then(value_i64).unwrap_or(0);
            offset += list.len() as i64;
            let done = list.is_empty() || offset >= total;
            summaries.extend(list);
            if done {
                break;
            }
        }
        let mut out = Vec::new();
        let mut first_err = None;
        for p in summaries {
            let id = match p.get("id") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => continue,
            };
            let emojis = match self.emoji_meta(&id).await {
                Ok(meta) => EmojiPackage::emojis_from_meta(&meta),
                Err(e) => {
                    first_err.get_or_insert(e);
                    continue;
                }
            };
            if emojis.is_empty() {
                continue;
            }
            out.push(EmojiPackage {
                name: p
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                version: p.get("latestVersion").and_then(value_i64).unwrap_or(1).max(1) as u32,
                resource_type: p["productTypeSummary"]["sticonSummary"]
                    .get("sticonResourceType")
                    .and_then(value_i64)
                    .unwrap_or(1),
                emojis,
                id,
            });
        }
        match first_err {
            Some(e) if out.is_empty() => Err(e),
            _ => Ok(out),
        }
    }

    // -- E2EE public keys (server side of Letter Sealing) ----------------------

    /// `negotiateE2EEPublicKey(mid)` -> `{publicKey:{keyId,keyData,...}, specVersion, allowedTypes}`.
    pub async fn negotiate_e2ee_public_key(&self, mid: &str) -> Result<Value> {
        self.talk("negotiateE2EEPublicKey", json!([mid])).await
    }

    /// `getE2EEPublicKey(mid, keyVersion, keyId)`.
    pub async fn e2ee_public_key(&self, mid: &str, key_version: i64, key_id: i64) -> Result<Value> {
        self.talk("getE2EEPublicKey", json!([mid, key_version, key_id]))
            .await
    }

    pub(crate) async fn call_opts(&self, method: &str, args: Value, opts: CallOpts) -> Result<Value> {
        self.t.call_with(method, args, opts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emojis_from_live_meta() {
        // Shape of the CDN's meta.json for "LINE Basic Emoji" (live, 2026-11-15).
        let meta = json!({"productId": "670e", "orders": ["002", "001", "051"],
            "altTexts": {"001": "heart eyes Moon", "002": "pleading Moon", "051": "tearing\u{a0}Leonard"}});
        assert_eq!(
            EmojiPackage::emojis_from_meta(&meta),
            [
                ("002".to_owned(), "pleading Moon".to_owned()),
                ("001".into(), "heart eyes Moon".into()),
                ("051".into(), "tearing Leonard".into()),
            ]
        );
        assert!(EmojiPackage::emojis_from_meta(&json!({})).is_empty());
    }
}
