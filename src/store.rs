//! A local message store (feature `store`), backed by SQLite.
//!
//! Keeps what a chat app needs to start instantly and resume after a
//! restart without asking the server again:
//!
//! - **chats** (name, picture, preview, unread count, mute, pending invitation),
//! - **messages**, stored *decrypted* (Letter-Sealed messages are kept in
//!   plain text once read, like the official apps' local databases),
//! - **contacts** (the friend list),
//! - **users**: what was last seen of any user or group, friend or not
//!   (name, status, picture, cover, members), so names and profile pages
//!   show at once,
//! - **images**: downloaded profile pictures and covers, so they are not
//!   fetched again,
//! - **read marks** (who has read each chat up to which message),
//! - the operation-stream **revision**, so [`Client::operations`] resumes
//!   where the last run stopped,
//! - free-form **settings** for the application;
//! - **pinned chats**, kept separately from server chat-list updates, in
//!   named, ordered pin lists ([`PinProfile`]); each account has its own
//!   lists and one active list.
//!
//! The file holds private conversations in clear text, so it is created with
//! owner-only permissions (0600) on Unix; protect it like `session.json`.
//!
//! [`Store::apply`] folds an [`Operation`] from the event stream into the
//! store; call it for every operation, then [`Store::set_stream_revision`].
//!
//! [`Client::operations`]: crate::Client::operations

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::import::{fingerprint, imported_sender_name, is_imported_id};
use crate::models::{Contact, Message, Operation, ReactionKind, apply_reaction, content_type};
use crate::ops::{OpEvent, message_id_cmp};

const SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS profiles (
    mid TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    created INTEGER NOT NULL,
    last_used INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chats (
    mid TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    picture TEXT,
    preview TEXT NOT NULL DEFAULT '',
    last_message_id TEXT,
    last_time INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0,
    invited INTEGER NOT NULL DEFAULT 0,
    muted INTEGER NOT NULL DEFAULT 0,
    last_seen TEXT
);
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    id_num INTEGER NOT NULL,
    chat TEXT NOT NULL,
    sender TEXT NOT NULL,
    time INTEGER NOT NULL,
    content_type INTEGER NOT NULL,
    json TEXT NOT NULL,
    e2ee_error TEXT,
    unsent INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_by_chat ON messages (chat, time, id_num);
CREATE INDEX IF NOT EXISTS messages_by_type ON messages (content_type, time);
CREATE TABLE IF NOT EXISTS contacts (
    mid TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    json TEXT NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS users (
    mid TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    picture TEXT,
    cover TEXT,
    members TEXT NOT NULL DEFAULT '[]',
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS images (
    key TEXT PRIMARY KEY,
    data BLOB NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS read_marks (
    chat TEXT NOT NULL,
    reader TEXT NOT NULL,
    message_id TEXT NOT NULL,
    id_num INTEGER NOT NULL,
    PRIMARY KEY (chat, reader)
);
";

/// Schema 2's pins, keyed by account mid (only used while migrating from 1).
const PINNED_SCHEMA_V2: &str = "
CREATE TABLE IF NOT EXISTS pinned_chats (
    profile TEXT NOT NULL,
    mid TEXT NOT NULL,
    position INTEGER NOT NULL,
    pinned_at INTEGER NOT NULL,
    PRIMARY KEY (profile, mid)
);
";

/// Named pin lists ("pin profiles") of each account, and their pins.
/// Exactly one pin profile per account is `active`.
const PINNED_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pin_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account TEXT NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    position INTEGER NOT NULL,
    active INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL,
    UNIQUE (account, name)
);
CREATE TABLE IF NOT EXISTS pinned_chats (
    pin_profile INTEGER NOT NULL,
    mid TEXT NOT NULL,
    position INTEGER NOT NULL,
    pinned_at INTEGER NOT NULL,
    PRIMARY KEY (pin_profile, mid)
);
";

/// The pin profile created for an account that has none.
pub const DEFAULT_PIN_PROFILE: &str = "Default";

/// The longest pin profile name, in characters.
pub const PIN_PROFILE_NAME_MAX: usize = 40;

/// A chat-list entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatRow {
    pub mid: String,
    pub name: String,
    /// Picture path (`picturePath`) for the profile CDN.
    pub picture: Option<String>,
    /// One-line summary of the newest message.
    pub preview: String,
    pub last_message_id: Option<String>,
    /// Epoch ms of the newest message (or of the invitation).
    pub last_time: i64,
    pub unread: u32,
    /// A group invitation we have not answered.
    pub invited: bool,
    /// Notifications are off for this chat (mirrors LINE's per-chat setting).
    pub muted: bool,
    /// The newest message we have read (any device).
    pub last_seen: Option<String>,
}

/// A locally stored account profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub mid: String,
    pub name: String,
    pub created: i64,
    pub last_used: i64,
}

/// A named, ordered list of pinned chats belonging to one account. An
/// account can have several; exactly one of them is active (shown and
/// edited by the pin methods of [`Store`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinProfile {
    pub id: i64,
    /// The account (mid) the list belongs to.
    pub account: String,
    pub name: String,
    pub active: bool,
    /// Number of pinned chats.
    pub pins: usize,
    pub created: i64,
}

/// What was last seen of a user or group (from `getContactsV2`, `getChats`,
/// `getProfile` or `getCover`), whether or not they are a friend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserRow {
    pub mid: String,
    pub name: String,
    /// Status message (users only).
    pub status: String,
    /// Picture path (`picturePath`) for the profile CDN.
    pub picture: Option<String>,
    /// OBS path of a custom cover picture (`None`: LINE's default, or unknown).
    pub cover: Option<String>,
    /// Group members (groups only).
    pub members: Vec<String>,
    /// Epoch ms of the last update.
    pub updated: i64,
}

/// A stored message.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    /// Decrypted when it could be; `e2ee_error` says why not otherwise.
    pub message: Message,
    pub e2ee_error: Option<String>,
    /// Unsent by its sender (kept so the chat can say so).
    pub unsent: bool,
}

/// A sticker as `(package id, sticker id, version)`.
pub type StickerRef = (String, String, u32);

/// Handle to the database; cheap to clone.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::Protocol(format!("store: {e}"))
}

fn id_num(id: &str) -> i64 {
    id.parse().unwrap_or(0)
}

/// What [`Store::import_messages`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportStats {
    /// Newly stored.
    pub added: usize,
    /// Already imported before.
    pub already: usize,
    /// Skipped because the real message is stored.
    pub duplicates: usize,
}

/// Is `imported` the export's copy of the real message `real` (same minute
/// is checked by the caller)? An unmatched sender only compares content.
fn same_message(imported: &Message, real: &Message) -> bool {
    let sender_ok = imported_sender_name(&imported.from).is_some() || imported.from == real.from;
    sender_ok && fingerprint(imported).is_some_and(|f| fingerprint(real).as_ref() == Some(&f))
}

/// Ids of the imported messages in `chat` that copy the real message `m`.
fn imported_copies(c: &Connection, chat: &str, m: &Message, time: i64) -> rusqlite::Result<Vec<String>> {
    if fingerprint(m).is_none() {
        return Ok(Vec::new());
    }
    let start = time.div_euclid(60_000) * 60_000;
    let mut st = c.prepare_cached(
        "SELECT id, json FROM messages WHERE chat = ?1 AND id_num < 0 AND time >= ?2 AND time < ?3",
    )?;
    let rows: Vec<(String, String)> = st
        .query_map(params![chat, start, start + 60_000], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, j)| serde_json::from_str::<Message>(j).is_ok_and(|i| same_message(&i, m)))
        .map(|(id, _)| id)
        .collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Open (or create) the database at `path`, created with mode 0600.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            opts.open(path)?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        // WAL keeps readers and the writer from blocking each other; SQLite
        // creates the -wal/-shm files with the database's permissions.
        conn.pragma_update(None, "journal_mode", "WAL").map_err(db_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(db_err)?;
        Self::init(conn, Some(path.to_owned()))
    }

    /// A throw-away in-memory database (tests, `--no-store`).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db_err)?, None)
    }

    fn init(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .map_err(db_err)?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        let version = version.and_then(|v| v.parse::<i64>().ok());
        if let Some(v) = version
            && !(1..=SCHEMA_VERSION).contains(&v)
        {
            return Err(Error::Protocol(format!(
                "store: schema version {v} is newer than this build ({SCHEMA_VERSION})"
            )));
        }

        conn.execute_batch(SCHEMA).map_err(db_err)?;
        let tx = conn.transaction().map_err(db_err)?;
        match version {
            Some(1) => {
                Self::migrate_v1(&tx).map_err(db_err)?;
                Self::migrate_v2(&tx).map_err(db_err)?;
            }
            Some(2) => Self::migrate_v2(&tx).map_err(db_err)?,
            _ => tx.execute_batch(PINNED_SCHEMA).map_err(db_err)?,
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION.to_string()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// Schema 1 → 2: one global pin list becomes the owner's pin list.
    fn migrate_v1(tx: &Connection) -> rusqlite::Result<()> {
        tx.execute_batch("ALTER TABLE pinned_chats RENAME TO pinned_chats_v1;")?;
        tx.execute_batch(PINNED_SCHEMA_V2)?;
        let profile: String = tx
            .query_row("SELECT value FROM meta WHERE key = 'owner'", [], |r| r.get(0))
            .optional()?
            .unwrap_or_default();
        let now = now_ms();
        tx.execute(
            "INSERT OR IGNORE INTO profiles (mid, name, created, last_used) VALUES (?1, '', ?2, ?2)",
            params![profile, now],
        )?;
        let old_pins: Vec<(String, i64)> = {
            let mut st = tx.prepare("SELECT mid, pinned_at FROM pinned_chats_v1 ORDER BY pinned_at, mid")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (position, (mid, pinned_at)) in old_pins.into_iter().enumerate() {
            tx.execute(
                "INSERT INTO pinned_chats (profile, mid, position, pinned_at) VALUES (?1, ?2, ?3, ?4)",
                params![profile, mid, position as i64, pinned_at],
            )?;
        }
        tx.execute_batch("DROP TABLE pinned_chats_v1;")
    }

    /// Schema 2 → 3: each account's pins become its active "Default" pin
    /// profile, keeping their order.
    fn migrate_v2(tx: &Connection) -> rusqlite::Result<()> {
        tx.execute_batch("ALTER TABLE pinned_chats RENAME TO pinned_chats_v2;")?;
        tx.execute_batch(PINNED_SCHEMA)?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO pin_profiles (account, name, position, active, created)
             SELECT account, ?1, 0, 1, ?2 FROM (
                 SELECT mid AS account FROM profiles
                 UNION SELECT profile AS account FROM pinned_chats_v2
             )",
            params![DEFAULT_PIN_PROFILE, now],
        )?;
        tx.execute_batch(
            "INSERT INTO pinned_chats (pin_profile, mid, position, pinned_at)
                 SELECT p.id, v.mid, v.position, v.pinned_at
                 FROM pinned_chats_v2 v JOIN pin_profiles p ON p.account = v.profile;
             DROP TABLE pinned_chats_v2;",
        )
    }

    /// Where the database lives (`None` in memory).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn with<R>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<R>) -> Result<R> {
        f(&mut self.conn.lock().unwrap()).map_err(db_err)
    }

    fn active_profile(c: &Connection) -> rusqlite::Result<String> {
        Ok(
            c.query_row("SELECT value FROM meta WHERE key = 'owner'", [], |r| r.get(0))
                .optional()?
                .unwrap_or_default(),
        )
    }

    /// The active pin profile of `account`, choosing (or creating) one when
    /// it has none.
    fn active_pin_profile_id(c: &Connection, account: &str) -> rusqlite::Result<i64> {
        let found: Option<(i64, bool)> = c
            .query_row(
                "SELECT id, active FROM pin_profiles WHERE account = ?1
                 ORDER BY active DESC, position, id LIMIT 1",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match found {
            Some((id, true)) => Ok(id),
            Some((id, false)) => {
                Self::set_active_pin_profile(c, account, id)?;
                Ok(id)
            }
            None => {
                c.execute(
                    "INSERT INTO pin_profiles (account, name, position, active, created)
                     VALUES (?1, ?2, 0, 1, ?3)",
                    params![account, DEFAULT_PIN_PROFILE, now_ms()],
                )?;
                Ok(c.last_insert_rowid())
            }
        }
    }

    fn set_active_pin_profile(c: &Connection, account: &str, id: i64) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE pin_profiles SET active = (id = ?2) WHERE account = ?1",
            params![account, id],
        )
        .map(drop)
    }

    /// The pin profile the pin methods act on: the bound account's active one.
    fn current_pin_profile(c: &Connection) -> rusqlite::Result<i64> {
        Self::active_pin_profile_id(c, &Self::active_profile(c)?)
    }

    fn pin_profiles_in(c: &Connection, account: &str) -> rusqlite::Result<Vec<PinProfile>> {
        let mut st = c.prepare(
            "SELECT p.id, p.account, p.name, p.active, p.created,
                    (SELECT COUNT(*) FROM pinned_chats WHERE pin_profile = p.id)
             FROM pin_profiles p WHERE p.account = ?1 ORDER BY p.position, p.id",
        )?;
        st.query_map([account], |r| {
            Ok(PinProfile {
                id: r.get(0)?,
                account: r.get(1)?,
                name: r.get(2)?,
                active: r.get(3)?,
                created: r.get(4)?,
                pins: r.get::<_, i64>(5)?.max(0) as usize,
            })
        })?
        .collect()
    }

    fn renumber_pin_profiles(c: &Connection, account: &str) -> rusqlite::Result<()> {
        let ids: Vec<i64> = {
            let mut st = c.prepare("SELECT id FROM pin_profiles WHERE account = ?1 ORDER BY position, id")?;
            st.query_map([account], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (position, id) in ids.into_iter().enumerate() {
            c.execute(
                "UPDATE pin_profiles SET position = ?2 WHERE id = ?1",
                params![id, position as i64],
            )?;
        }
        Ok(())
    }

    /// Delete pin profiles and their pins.
    fn delete_pin_profiles_where(
        c: &Connection,
        filter: &str,
        arg: &dyn rusqlite::ToSql,
    ) -> rusqlite::Result<()> {
        c.execute(
            &format!(
                "DELETE FROM pinned_chats WHERE pin_profile IN (SELECT id FROM pin_profiles WHERE {filter})"
            ),
            [arg],
        )?;
        c.execute(&format!("DELETE FROM pin_profiles WHERE {filter}"), [arg])
            .map(drop)
    }

    fn renumber_pins(c: &Connection, pin_profile: i64) -> rusqlite::Result<()> {
        let mids = Self::pinned_chats_in(c, pin_profile)?;
        for (position, mid) in mids.into_iter().enumerate() {
            c.execute(
                "UPDATE pinned_chats SET position = ?3 WHERE pin_profile = ?1 AND mid = ?2",
                params![pin_profile, mid, position as i64],
            )?;
        }
        Ok(())
    }

    fn pinned_chats_in(c: &Connection, pin_profile: i64) -> rusqlite::Result<Vec<String>> {
        let mut st =
            c.prepare("SELECT mid FROM pinned_chats WHERE pin_profile = ?1 ORDER BY position, mid")?;
        st.query_map([pin_profile], |r| r.get(0))?.collect()
    }

    /// Move the pins made before any account was bound (account `''`) to
    /// `account`: same-named pin profiles are merged (appended), others are
    /// moved over.
    fn adopt_unbound_pins(c: &Connection, account: &str) -> rusqlite::Result<()> {
        let had_own: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pin_profiles WHERE account = ?1)",
            [account],
            |r| r.get(0),
        )?;
        let unbound: Vec<(i64, String)> = {
            let mut st =
                c.prepare("SELECT id, name FROM pin_profiles WHERE account = '' ORDER BY position, id")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, name) in unbound {
            let target: Option<i64> = c
                .query_row(
                    "SELECT id FROM pin_profiles WHERE account = ?1 AND name = ?2",
                    params![account, name],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(target) = target {
                let start: i64 = c.query_row(
                    "SELECT COUNT(*) FROM pinned_chats WHERE pin_profile = ?1",
                    [target],
                    |r| r.get(0),
                )?;
                c.execute(
                    "INSERT OR IGNORE INTO pinned_chats (pin_profile, mid, position, pinned_at)
                     SELECT ?2, mid, ?3 + position, pinned_at FROM pinned_chats WHERE pin_profile = ?1",
                    params![id, target, start],
                )?;
                c.execute("DELETE FROM pinned_chats WHERE pin_profile = ?1", [id])?;
                c.execute("DELETE FROM pin_profiles WHERE id = ?1", [id])?;
                Self::renumber_pins(c, target)?;
            } else {
                c.execute(
                    "UPDATE pin_profiles SET account = ?2, active = active AND NOT ?3,
                         position = (SELECT COUNT(*) FROM pin_profiles WHERE account = ?2)
                     WHERE id = ?1",
                    params![id, account, had_own],
                )?;
            }
        }
        Self::renumber_pin_profiles(c, account)?;
        Self::active_pin_profile_id(c, account).map(drop)
    }

    fn profile_in(c: &Connection, mid: &str) -> rusqlite::Result<Option<Profile>> {
        c.query_row(
            "SELECT mid, name, created, last_used FROM profiles WHERE mid = ?1",
            [mid],
            |r| {
                Ok(Profile {
                    mid: r.get(0)?,
                    name: r.get(1)?,
                    created: r.get(2)?,
                    last_used: r.get(3)?,
                })
            },
        )
        .optional()
    }

    /// Tie the database to an account. If it holds another account's data,
    /// that data is wiped first (returns `true` then); profile pins are kept.
    pub fn bind_account(&self, my_mid: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let old_owner: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key = 'owner'", [], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        let switched = old_owner.as_deref().is_some_and(|owner| owner != my_mid);
        if switched {
            tx.execute_batch(
                "DELETE FROM chats; DELETE FROM messages; DELETE FROM contacts; DELETE FROM read_marks;
                 DELETE FROM users; DELETE FROM images;
                 DELETE FROM meta WHERE key = 'stream_rev' OR key LIKE 'cache.%';",
            )
            .map_err(db_err)?;
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO profiles (mid, name, created, last_used) VALUES (?1, '', ?2, ?2)
             ON CONFLICT(mid) DO UPDATE SET last_used = excluded.last_used",
            params![my_mid, now],
        )
        .map_err(db_err)?;
        if old_owner.is_none() {
            Self::adopt_unbound_pins(&tx, my_mid).map_err(db_err)?;
        } else {
            Self::active_pin_profile_id(&tx, my_mid).map_err(db_err)?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('owner', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [my_mid],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(switched)
    }

    /// Delete the active pin profile's pins and all cached account data.
    /// Other pin profiles (of this or other accounts), their pins and
    /// application settings are kept.
    pub fn clear(&self) -> Result<()> {
        self.with(|c| {
            let pin_profile = Self::current_pin_profile(c)?;
            c.execute_batch(
                "DELETE FROM chats; DELETE FROM messages; DELETE FROM contacts; DELETE FROM read_marks;
                 DELETE FROM users; DELETE FROM images;
                 DELETE FROM meta WHERE key = 'stream_rev' OR key LIKE 'cache.%';",
            )?;
            c.execute("DELETE FROM pinned_chats WHERE pin_profile = ?1", [pin_profile])?;
            Ok(())
        })
    }

    // -- key/value -------------------------------------------------------------------

    /// A stored value (`meta` table). Keys starting with `setting.` are a
    /// good place for application settings; `cache.` keys are dropped by
    /// [`clear`](Self::clear).
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map(drop)
        })
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM meta WHERE key = ?1", [key]).map(drop))
    }

    /// The currently bound profile (or the unbound profile when no account is
    /// active yet).
    pub fn profile(&self) -> Result<Profile> {
        self.with(|c| {
            let mid = Self::active_profile(c)?;
            Ok(Self::profile_in(c, &mid)?.unwrap_or(Profile {
                mid,
                ..Default::default()
            }))
        })
    }

    /// All bound profiles, most recently used first.
    pub fn profiles(&self) -> Result<Vec<Profile>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT mid, name, created, last_used FROM profiles WHERE mid != '' ORDER BY last_used DESC, mid",
            )?;
            st.query_map([], |r| {
                Ok(Profile {
                    mid: r.get(0)?,
                    name: r.get(1)?,
                    created: r.get(2)?,
                    last_used: r.get(3)?,
                })
            })?
            .collect()
        })
    }

    /// Save a display name for the active profile.
    pub fn set_profile_name(&self, name: &str) -> Result<()> {
        self.with(|c| {
            let mid = Self::active_profile(c)?;
            let now = now_ms();
            c.execute(
                "INSERT INTO profiles (mid, name, created, last_used) VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(mid) DO UPDATE SET name = excluded.name",
                params![mid, name, now],
            )
            .map(drop)
        })
    }

    /// Delete an inactive account profile with all its pin profiles. The
    /// active profile cannot be deleted while it is bound to the store.
    pub fn delete_profile(&self, mid: &str) -> Result<bool> {
        self.with(|c| {
            if Self::active_profile(c)? == mid {
                return Ok(false);
            }
            let tx = c.transaction()?;
            Self::delete_pin_profiles_where(&tx, "account = ?1", &mid)?;
            let removed = tx.execute("DELETE FROM profiles WHERE mid = ?1", [mid])?;
            tx.commit()?;
            Ok(removed != 0)
        })
    }

    /// The operation-stream revision processed last (resume cursor).
    pub fn stream_revision(&self) -> Result<Option<i64>> {
        Ok(self.get("stream_rev")?.and_then(|v| v.parse().ok()))
    }

    /// Record the stream position. Never moves backwards.
    pub fn set_stream_revision(&self, rev: i64) -> Result<()> {
        if rev <= 0 || self.stream_revision()?.is_some_and(|cur| cur >= rev) {
            return Ok(());
        }
        self.set("stream_rev", &rev.to_string())
    }

    // -- chats -----------------------------------------------------------------------

    fn chat_from_row(r: &rusqlite::Row) -> rusqlite::Result<ChatRow> {
        Ok(ChatRow {
            mid: r.get(0)?,
            name: r.get(1)?,
            picture: r.get(2)?,
            preview: r.get(3)?,
            last_message_id: r.get(4)?,
            last_time: r.get(5)?,
            unread: r.get::<_, i64>(6)?.max(0) as u32,
            invited: r.get(7)?,
            muted: r.get(8)?,
            last_seen: r.get(9)?,
        })
    }

    const CHAT_COLS: &str =
        "mid, name, picture, preview, last_message_id, last_time, unread, invited, muted, last_seen";

    /// All chats, newest first (pending invitations first of all).
    pub fn chats(&self) -> Result<Vec<ChatRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {} FROM chats ORDER BY invited DESC, last_time DESC",
                Self::CHAT_COLS
            ))?;
            st.query_map([], Self::chat_from_row)?.collect()
        })
    }

    // -- pin profiles ----------------------------------------------------------------

    /// The bound account's pin profiles in their saved order (one is created
    /// when it has none).
    pub fn pin_profiles(&self) -> Result<Vec<PinProfile>> {
        self.with(|c| {
            let account = Self::active_profile(c)?;
            Self::active_pin_profile_id(c, &account)?;
            Self::pin_profiles_in(c, &account)
        })
    }

    /// The pin profiles of any stored account, in their saved order.
    pub fn pin_profiles_of(&self, account: &str) -> Result<Vec<PinProfile>> {
        self.with(|c| Self::pin_profiles_in(c, account))
    }

    /// The bound account's active pin profile.
    pub fn active_pin_profile(&self) -> Result<PinProfile> {
        let profiles = self.pin_profiles()?;
        Ok(profiles
            .into_iter()
            .find(|p| p.active)
            .expect("an account always has an active pin profile"))
    }

    /// Check a pin profile name: trimmed, non-empty, at most
    /// [`PIN_PROFILE_NAME_MAX`] characters and not used (ignoring case) by
    /// another of the account's pin profiles.
    fn pin_profile_name(c: &Connection, account: &str, name: &str, except: Option<i64>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Protocol("store: pin profile name cannot be empty".into()));
        }
        if name.chars().count() > PIN_PROFILE_NAME_MAX {
            return Err(Error::Protocol(format!(
                "store: pin profile names are at most {PIN_PROFILE_NAME_MAX} characters"
            )));
        }
        let taken: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pin_profiles WHERE account = ?1 AND name = ?2 AND id != ?3)",
                params![account, name, except.unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if taken {
            return Err(Error::Protocol(format!(
                "store: a pin profile named “{name}” exists"
            )));
        }
        Ok(name.to_owned())
    }

    /// `id` if it is one of the bound account's pin profiles.
    fn own_pin_profile(c: &Connection, id: i64) -> Result<String> {
        let account = Self::active_profile(c).map_err(db_err)?;
        let owned: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pin_profiles WHERE id = ?1 AND account = ?2)",
                params![id, account],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        if owned {
            Ok(account)
        } else {
            Err(Error::Protocol(format!(
                "store: no pin profile {id} for this account"
            )))
        }
    }

    /// Add an empty pin profile to the bound account (at the end of the
    /// list; the active one stays active).
    pub fn create_pin_profile(&self, name: &str) -> Result<PinProfile> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let account = Self::active_profile(&tx).map_err(db_err)?;
        Self::active_pin_profile_id(&tx, &account).map_err(db_err)?;
        let name = Self::pin_profile_name(&tx, &account, name, None)?;
        let created = now_ms();
        tx.execute(
            "INSERT INTO pin_profiles (account, name, position, active, created)
             VALUES (?1, ?2, (SELECT COUNT(*) FROM pin_profiles WHERE account = ?1), 0, ?3)",
            params![account, name, created],
        )
        .map_err(db_err)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(db_err)?;
        Ok(PinProfile {
            id,
            account,
            name,
            active: false,
            pins: 0,
            created,
        })
    }

    /// Rename one of the bound account's pin profiles.
    pub fn rename_pin_profile(&self, id: i64, name: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let account = Self::own_pin_profile(&tx, id)?;
        let name = Self::pin_profile_name(&tx, &account, name, Some(id))?;
        tx.execute(
            "UPDATE pin_profiles SET name = ?2 WHERE id = ?1",
            params![id, name],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// Delete one of the bound account's pin profiles and its pins. The last
    /// one cannot be deleted; deleting the active one activates its
    /// neighbour (the next one, else the previous one).
    pub fn delete_pin_profile(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let account = Self::own_pin_profile(&tx, id)?;
        let profiles = Self::pin_profiles_in(&tx, &account).map_err(db_err)?;
        if profiles.len() <= 1 {
            return Err(Error::Protocol(
                "store: the last pin profile cannot be deleted".into(),
            ));
        }
        let index = profiles.iter().position(|p| p.id == id).unwrap_or(0);
        Self::delete_pin_profiles_where(&tx, "id = ?1", &id).map_err(db_err)?;
        if profiles[index].active {
            let next = profiles
                .get(index + 1)
                .unwrap_or(&profiles[index.saturating_sub(1)]);
            Self::set_active_pin_profile(&tx, &account, next.id).map_err(db_err)?;
        }
        Self::renumber_pin_profiles(&tx, &account).map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// Make one of the bound account's pin profiles the active one.
    pub fn switch_pin_profile(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let account = Self::own_pin_profile(&tx, id)?;
        Self::set_active_pin_profile(&tx, &account, id).map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    // -- pins (of the active pin profile) ----------------------------------------

    /// Chat mids pinned in the active pin profile, in their saved order.
    pub fn pinned_chats(&self) -> Result<Vec<String>> {
        self.with(|c| Self::pinned_chats_in(c, Self::current_pin_profile(c)?))
    }

    /// Chat mids pinned in account `profile`'s active pin profile, in their
    /// saved order.
    pub fn pinned_chats_of(&self, profile: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let id: Option<i64> = c
                .query_row(
                    "SELECT id FROM pin_profiles WHERE account = ?1 ORDER BY active DESC, position, id LIMIT 1",
                    [profile],
                    |r| r.get(0),
                )
                .optional()?;
            id.map_or(Ok(Vec::new()), |id| Self::pinned_chats_in(c, id))
        })
    }

    /// Chat mids pinned in pin profile `id` (of any account), in order.
    pub fn pinned_chats_in_profile(&self, id: i64) -> Result<Vec<String>> {
        self.with(|c| Self::pinned_chats_in(c, id))
    }

    /// Pin or unpin a chat in the active pin profile. Returns whether the
    /// stored state changed.
    pub fn set_pinned(&self, mid: &str, pinned: bool) -> Result<bool> {
        self.with(|c| {
            let tx = c.transaction()?;
            let profile = Self::current_pin_profile(&tx)?;
            let changed = if pinned {
                let invited: bool = tx
                    .query_row("SELECT invited FROM chats WHERE mid = ?1", [mid], |r| r.get(0))
                    .optional()?
                    .unwrap_or(false);
                if invited {
                    0
                } else {
                    tx.execute(
                        "INSERT OR IGNORE INTO pinned_chats (pin_profile, mid, position, pinned_at)
                         VALUES (?1, ?2, (SELECT COUNT(*) FROM pinned_chats WHERE pin_profile = ?1), ?3)",
                        params![profile, mid, now_ms()],
                    )?
                }
            } else {
                tx.execute(
                    "DELETE FROM pinned_chats WHERE pin_profile = ?1 AND mid = ?2",
                    params![profile, mid],
                )?
            };
            if changed != 0 {
                Self::renumber_pins(&tx, profile)?;
            }
            tx.commit()?;
            Ok(changed != 0)
        })
    }

    /// Whether `mid` is pinned in the active pin profile.
    pub fn is_pinned(&self, mid: &str) -> Result<bool> {
        self.with(|c| {
            let profile = Self::current_pin_profile(c)?;
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM pinned_chats WHERE pin_profile = ?1 AND mid = ?2)",
                params![profile, mid],
                |r| r.get(0),
            )
        })
    }

    /// Move a pin by `delta` positions. Returns false if it cannot be moved.
    pub fn move_pinned(&self, mid: &str, delta: i32) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let profile = Self::current_pin_profile(&tx).map_err(db_err)?;
        let invited: bool = tx
            .query_row("SELECT invited FROM chats WHERE mid = ?1", [mid], |r| r.get(0))
            .optional()
            .map_err(db_err)?
            .unwrap_or(false);
        if invited || delta == 0 {
            tx.commit().map_err(db_err)?;
            return Ok(false);
        }
        let mut pins = Self::pinned_chats_in(&tx, profile).map_err(db_err)?;
        let Some(from) = pins.iter().position(|p| p == mid) else {
            tx.commit().map_err(db_err)?;
            return Ok(false);
        };
        let to = from as i64 + i64::from(delta);
        if to < 0 || to >= pins.len() as i64 {
            tx.commit().map_err(db_err)?;
            return Ok(false);
        }
        let pin = pins.remove(from);
        pins.insert(to as usize, pin);
        Self::write_pin_order(&tx, profile, &pins).map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    fn write_pin_order(c: &Connection, profile: i64, order: &[String]) -> rusqlite::Result<()> {
        for (position, mid) in order.iter().enumerate() {
            c.execute(
                "UPDATE pinned_chats SET position = ?3 WHERE pin_profile = ?1 AND mid = ?2",
                params![profile, mid, position as i64],
            )?;
        }
        Ok(())
    }

    /// Set the complete pin order of the active pin profile. `order` must
    /// contain each currently pinned chat exactly once.
    pub fn set_pinned_order(&self, order: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let profile = Self::current_pin_profile(&tx).map_err(db_err)?;
        let current = Self::pinned_chats_in(&tx, profile).map_err(db_err)?;
        let current_set: HashSet<&str> = current.iter().map(String::as_str).collect();
        let order_set: HashSet<&str> = order.iter().map(String::as_str).collect();
        if current.len() != order.len() || current_set != order_set {
            return Err(Error::Protocol(
                "store: pin order must contain every pinned chat exactly once".into(),
            ));
        }
        Self::write_pin_order(&tx, profile, order).map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn chat(&self, mid: &str) -> Result<Option<ChatRow>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {} FROM chats WHERE mid = ?1", Self::CHAT_COLS),
                [mid],
                Self::chat_from_row,
            )
            .optional()
        })
    }

    /// Insert or replace a chat row.
    pub fn upsert_chat(&self, ch: &ChatRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT OR REPLACE INTO chats ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    Self::CHAT_COLS
                ),
                params![
                    ch.mid,
                    ch.name,
                    ch.picture,
                    ch.preview,
                    ch.last_message_id,
                    ch.last_time,
                    ch.unread,
                    ch.invited,
                    ch.muted,
                    ch.last_seen
                ],
            )
            .map(drop)
        })
    }

    /// Update `mid` with `f` (starting from an empty row when unknown).
    pub fn update_chat(&self, mid: &str, f: impl FnOnce(&mut ChatRow)) -> Result<ChatRow> {
        let mut row = self.chat(mid)?.unwrap_or_else(|| ChatRow {
            mid: mid.to_owned(),
            ..Default::default()
        });
        f(&mut row);
        self.upsert_chat(&row)?;
        Ok(row)
    }

    /// Set the name and picture of chats (and contacts' 1:1 chats) that exist.
    pub fn set_chat_names(&self, names: &HashMap<String, (String, Option<String>)>) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare("UPDATE chats SET name = ?2, picture = ?3 WHERE mid = ?1")?;
                for (mid, (name, pic)) in names {
                    st.execute(params![mid, name, pic])?;
                }
            }
            tx.commit()
        })
    }

    /// Forget a chat with its messages and read marks (left or declined).
    pub fn delete_chat(&self, mid: &str) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            // The chat is gone for the account: unpin it from all its pin profiles.
            let account = Self::active_profile(&tx)?;
            let holders: Vec<i64> = {
                let mut st = tx.prepare(
                    "SELECT pin_profile FROM pinned_chats WHERE mid = ?2
                     AND pin_profile IN (SELECT id FROM pin_profiles WHERE account = ?1)",
                )?;
                st.query_map(params![account, mid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            tx.execute("DELETE FROM chats WHERE mid = ?1", [mid])?;
            for profile in holders {
                tx.execute(
                    "DELETE FROM pinned_chats WHERE pin_profile = ?1 AND mid = ?2",
                    params![profile, mid],
                )?;
                Self::renumber_pins(&tx, profile)?;
            }
            tx.execute("DELETE FROM messages WHERE chat = ?1", [mid])?;
            tx.execute("DELETE FROM read_marks WHERE chat = ?1", [mid])?;
            tx.commit()
        })
    }

    /// Drop a chat's stored messages (keeps the chat).
    pub fn clear_messages(&self, chat: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM messages WHERE chat = ?1", [chat])
                .map(drop)
        })
    }

    /// Drop a chat's messages that came from LINE, keeping imported ones
    /// (which the server cannot give back).
    pub fn clear_fetched_messages(&self, chat: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM messages WHERE chat = ?1 AND id_num >= 0", [chat])
                .map(drop)
        })
    }

    // -- messages --------------------------------------------------------------------

    /// Store a message (already decrypted where possible) in `chat`.
    /// Returns `true` if it was new. A re-fetched copy replaces the stored
    /// one (fresher reactions; a later successful decryption).
    pub fn put_message(&self, chat: &str, m: &Message, e2ee_error: Option<&str>) -> Result<bool> {
        self.put_messages(chat, std::slice::from_ref(m), e2ee_error)
            .map(|n| n == 1)
    }

    /// Store several messages of one chat in a transaction; returns how many were new.
    pub fn put_messages(&self, chat: &str, msgs: &[Message], e2ee_error: Option<&str>) -> Result<usize> {
        let rows: Vec<(&Message, String)> = msgs
            .iter()
            .filter(|m| !m.id.is_empty())
            .map(|m| Ok((m, serde_json::to_string(m)?)))
            .collect::<Result<_>>()?;
        self.with(|c| {
            let tx = c.transaction()?;
            let mut new = 0;
            {
                let mut exists = tx.prepare("SELECT 1 FROM messages WHERE id = ?1")?;
                let mut st = tx.prepare(
                    "INSERT INTO messages (id, id_num, chat, sender, time, content_type, json, e2ee_error)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET json = excluded.json,
                         e2ee_error = excluded.e2ee_error, content_type = excluded.content_type",
                )?;
                for (m, json) in &rows {
                    if !exists.exists([&m.id])? {
                        new += 1;
                    }
                    let time = m.created_time.unwrap_or_else(now_ms);
                    st.execute(params![
                        m.id,
                        id_num(&m.id),
                        chat,
                        m.from,
                        time,
                        m.content_type,
                        json,
                        e2ee_error
                    ])?;
                    if !is_imported_id(&m.id) {
                        // The real message supersedes its imported copy.
                        for id in imported_copies(&tx, chat, m, time)? {
                            tx.execute("DELETE FROM messages WHERE id = ?1", [id])?;
                        }
                    }
                }
            }
            tx.commit()?;
            Ok(new)
        })
    }

    /// Store messages imported from a LINE chat export (see [`crate::import`])
    /// in `chat`. Messages already stored (the same file imported before) and
    /// messages whose real copy is stored are skipped; nothing is replaced.
    pub fn import_messages(&self, chat: &str, msgs: &[Message]) -> Result<ImportStats> {
        let rows: Vec<(&Message, String)> = msgs
            .iter()
            .filter(|m| is_imported_id(&m.id))
            .map(|m| Ok((m, serde_json::to_string(m)?)))
            .collect::<Result<_>>()?;
        self.with(|c| {
            let tx = c.transaction()?;
            let mut stats = ImportStats::default();
            {
                let mut exists = tx.prepare("SELECT 1 FROM messages WHERE id = ?1")?;
                let mut real = tx.prepare(
                    "SELECT json FROM messages WHERE chat = ?1 AND id_num >= 0 AND time >= ?2 AND time < ?3",
                )?;
                let mut st = tx.prepare(
                    "INSERT INTO messages (id, id_num, chat, sender, time, content_type, json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?;
                for (m, json) in &rows {
                    if exists.exists([&m.id])? {
                        stats.already += 1;
                        continue;
                    }
                    let time = m.created_time.unwrap_or(0);
                    let start = time.div_euclid(60_000) * 60_000;
                    let candidates: Vec<String> = real
                        .query_map(params![chat, start, start + 60_000], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    let dup = candidates
                        .iter()
                        .any(|j| serde_json::from_str::<Message>(j).is_ok_and(|r| same_message(m, &r)));
                    if dup {
                        stats.duplicates += 1;
                        continue;
                    }
                    st.execute(params![
                        m.id,
                        id_num(&m.id),
                        chat,
                        m.from,
                        time,
                        m.content_type,
                        json
                    ])?;
                    stats.added += 1;
                }
            }
            tx.commit()?;
            Ok(stats)
        })
    }

    /// How many imported messages `chat` holds.
    pub fn imported_count(&self, chat: &str) -> Result<usize> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat = ?1 AND id_num < 0",
                [chat],
                |r| r.get::<_, i64>(0),
            )
        })
        .map(|n| n as usize)
    }

    /// Delete the messages imported into `chat`; returns how many there were.
    pub fn delete_imported(&self, chat: &str) -> Result<usize> {
        self.with(|c| c.execute("DELETE FROM messages WHERE chat = ?1 AND id_num < 0", [chat]))
    }

    fn message_from_row(r: &rusqlite::Row) -> rusqlite::Result<(String, Option<String>, bool)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    }

    fn decode(rows: Vec<(String, Option<String>, bool)>) -> Result<Vec<StoredMessage>> {
        rows.into_iter()
            .map(|(json, e2ee_error, unsent)| {
                Ok(StoredMessage {
                    message: serde_json::from_str(&json)?,
                    e2ee_error,
                    unsent,
                })
            })
            .collect()
    }

    /// Up to `limit` messages of `chat`, oldest first. With `before`, only
    /// messages older than that message id (to page backwards).
    pub fn messages(&self, chat: &str, limit: usize, before: Option<&str>) -> Result<Vec<StoredMessage>> {
        let rows = self.with(|c| {
            let (sql, cutoff) = match before {
                Some(id) => {
                    let t: Option<i64> = c
                        .query_row("SELECT time FROM messages WHERE id = ?1", [id], |r| r.get(0))
                        .optional()?;
                    (
                        "SELECT json, e2ee_error, unsent FROM messages
                         WHERE chat = ?1 AND (time < ?3 OR (time = ?3 AND id_num < ?4))
                         ORDER BY time DESC, id_num DESC LIMIT ?2",
                        Some((t.unwrap_or(i64::MAX), id_num(id))),
                    )
                }
                None => (
                    "SELECT json, e2ee_error, unsent FROM messages WHERE chat = ?1
                     ORDER BY time DESC, id_num DESC LIMIT ?2",
                    None,
                ),
            };
            let mut st = c.prepare(sql)?;
            let rows: rusqlite::Result<Vec<_>> = match cutoff {
                Some((t, n)) => st
                    .query_map(params![chat, limit as i64, t, n], Self::message_from_row)?
                    .collect(),
                None => st
                    .query_map(params![chat, limit as i64], Self::message_from_row)?
                    .collect(),
            };
            rows
        })?;
        let mut out = Self::decode(rows)?;
        out.reverse();
        Ok(out)
    }

    /// The newest message of `chat` that came from LINE (not imported).
    pub fn newest_fetched_message(&self, chat: &str) -> Result<Option<StoredMessage>> {
        let row = self.with(|c| {
            c.query_row(
                "SELECT json, e2ee_error, unsent FROM messages WHERE chat = ?1 AND id_num >= 0
                 ORDER BY time DESC, id_num DESC LIMIT 1",
                [chat],
                Self::message_from_row,
            )
            .optional()
        })?;
        Ok(Self::decode(row.into_iter().collect())?.pop())
    }

    pub fn message(&self, id: &str) -> Result<Option<StoredMessage>> {
        let row = self.with(|c| {
            c.query_row(
                "SELECT json, e2ee_error, unsent FROM messages WHERE id = ?1",
                [id],
                Self::message_from_row,
            )
            .optional()
        })?;
        Ok(Self::decode(row.into_iter().collect())?.pop())
    }

    pub fn has_message(&self, id: &str) -> Result<bool> {
        self.with(|c| c.prepare("SELECT 1 FROM messages WHERE id = ?1")?.exists([id]))
    }

    pub fn message_count(&self, chat: &str) -> Result<usize> {
        self.with(|c| {
            c.query_row("SELECT COUNT(*) FROM messages WHERE chat = ?1", [chat], |r| {
                r.get::<_, i64>(0)
            })
        })
        .map(|n| n as usize)
    }

    /// Mark a message as unsent; returns its chat if it was stored.
    pub fn mark_unsent(&self, id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.execute("UPDATE messages SET unsent = 1 WHERE id = ?1", [id])?;
            c.query_row("SELECT chat FROM messages WHERE id = ?1", [id], |r| r.get(0))
                .optional()
        })
    }

    /// Record `from`'s reaction (`None` removes it). Returns the updated
    /// message if it is stored.
    pub fn set_reaction(
        &self,
        message_id: &str,
        from: &str,
        kind: Option<ReactionKind>,
        at: i64,
    ) -> Result<Option<Message>> {
        let Some(mut sm) = self.message(message_id)? else {
            return Ok(None);
        };
        let mut r = sm.message.reactions();
        apply_reaction(&mut r, from, kind, at);
        sm.message.extra.insert(
            "reactions".into(),
            serde_json::Value::Array(r.iter().map(|x| x.to_json()).collect()),
        );
        let json = serde_json::to_string(&sm.message)?;
        self.with(|c| {
            c.execute(
                "UPDATE messages SET json = ?2 WHERE id = ?1",
                params![message_id, json],
            )
        })?;
        Ok(Some(sm.message))
    }

    /// Stickers seen in any chat, most recent first, without duplicates.
    pub fn recent_stickers(&self, limit: usize) -> Result<Vec<StickerRef>> {
        let rows: Vec<String> = self.with(|c| {
            let mut st = c.prepare(
                "SELECT json FROM messages WHERE content_type = ?1 AND unsent = 0
                 ORDER BY time DESC LIMIT 2000",
            )?;
            st.query_map([content_type::STICKER], |r| r.get(0))?.collect()
        })?;
        let mut out: Vec<StickerRef> = Vec::new();
        for json in rows {
            let Ok(m) = serde_json::from_str::<Message>(&json) else {
                continue;
            };
            let meta = &m.content_metadata;
            let (Some(pkg), Some(id)) = (meta.get("STKPKGID"), meta.get("STKID")) else {
                continue;
            };
            if out.iter().any(|s| &s.1 == id) {
                continue;
            }
            let ver = meta.get("STKVER").and_then(|v| v.parse().ok()).unwrap_or(1);
            out.push((pkg.clone(), id.clone(), ver));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    // -- read marks ------------------------------------------------------------------

    /// `reader` has read `chat` up to `message_id` (kept only if newer).
    pub fn set_read_mark(&self, chat: &str, reader: &str, message_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO read_marks (chat, reader, message_id, id_num) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(chat, reader) DO UPDATE SET message_id = excluded.message_id,
                     id_num = excluded.id_num WHERE excluded.id_num > read_marks.id_num",
                params![chat, reader, message_id, id_num(message_id)],
            )
            .map(drop)
        })
    }

    /// Reader → newest message id they have read, for one chat.
    pub fn read_marks(&self, chat: &str) -> Result<HashMap<String, String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT reader, message_id FROM read_marks WHERE chat = ?1")?;
            st.query_map([chat], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
    }

    // -- contacts --------------------------------------------------------------------

    pub fn upsert_contacts<'a>(&self, contacts: impl IntoIterator<Item = &'a Contact>) -> Result<()> {
        let rows: Vec<(&Contact, String)> = contacts
            .into_iter()
            .filter(|c| !c.mid.is_empty())
            .map(|c| Ok((c, serde_json::to_string(c)?)))
            .collect::<Result<_>>()?;
        let now = now_ms();
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare(
                    "INSERT OR REPLACE INTO contacts (mid, name, json, updated) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for (ct, json) in &rows {
                    st.execute(params![ct.mid, ct.name(), json, now])?;
                }
            }
            tx.commit()
        })
    }

    /// All stored contacts, by name.
    pub fn contacts(&self) -> Result<Vec<Contact>> {
        let rows: Vec<String> = self.with(|c| {
            let mut st = c.prepare("SELECT json FROM contacts ORDER BY name COLLATE NOCASE")?;
            st.query_map([], |r| r.get(0))?.collect()
        })?;
        rows.iter().map(|j| Ok(serde_json::from_str(j)?)).collect()
    }

    pub fn contact(&self, mid: &str) -> Result<Option<Contact>> {
        let row: Option<String> = self.with(|c| {
            c.query_row("SELECT json FROM contacts WHERE mid = ?1", [mid], |r| r.get(0))
                .optional()
        })?;
        row.map(|j| Ok(serde_json::from_str(&j)?)).transpose()
    }

    // -- users and images ----------------------------------------------------------

    /// Insert or replace what is known of a user or group (`updated` is set now).
    pub fn upsert_user(&self, u: &UserRow) -> Result<()> {
        if u.mid.is_empty() {
            return Ok(());
        }
        let members = serde_json::to_string(&u.members)?;
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO users (mid, name, status, picture, cover, members, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![u.mid, u.name, u.status, u.picture, u.cover, members, now_ms()],
            )
            .map(drop)
        })
    }

    fn user_from(r: &rusqlite::Row) -> rusqlite::Result<UserRow> {
        let members: String = r.get(5)?;
        Ok(UserRow {
            mid: r.get(0)?,
            name: r.get(1)?,
            status: r.get(2)?,
            picture: r.get(3)?,
            cover: r.get(4)?,
            members: serde_json::from_str(&members).unwrap_or_default(),
            updated: r.get(6)?,
        })
    }

    pub fn user(&self, mid: &str) -> Result<Option<UserRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT mid, name, status, picture, cover, members, updated FROM users WHERE mid = ?1",
                [mid],
                Self::user_from,
            )
            .optional()
        })
    }

    /// Every stored user and group.
    pub fn users(&self) -> Result<Vec<UserRow>> {
        self.with(|c| {
            let mut st =
                c.prepare("SELECT mid, name, status, picture, cover, members, updated FROM users")?;
            st.query_map([], Self::user_from)?.collect()
        })
    }

    /// Keep downloaded image bytes under `key` (e.g. a picture path and size).
    pub fn put_image(&self, key: &str, data: &[u8]) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO images (key, data, updated) VALUES (?1, ?2, ?3)",
                params![key, data, now_ms()],
            )
            .map(drop)
        })
    }

    pub fn image(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.with(|c| {
            c.query_row("SELECT data FROM images WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
    }

    /// Drop the oldest images until they take at most `max_bytes`; returns
    /// how many were dropped.
    pub fn prune_images(&self, max_bytes: u64) -> Result<usize> {
        self.with(|c| {
            let rows: Vec<(String, i64)> = {
                let mut st = c.prepare("SELECT key, length(data) FROM images ORDER BY updated DESC, key")?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut total = 0u64;
            let mut drop_keys = Vec::new();
            for (key, len) in rows {
                total += len.max(0) as u64;
                if total > max_bytes {
                    drop_keys.push(key);
                }
            }
            let tx = c.transaction()?;
            for k in &drop_keys {
                tx.execute("DELETE FROM images WHERE key = ?1", [k])?;
            }
            tx.commit()?;
            Ok(drop_keys.len())
        })
    }

    /// `(chats, messages, contacts)` counts.
    pub fn stats(&self) -> Result<(usize, usize, usize)> {
        self.with(|c| {
            let n = |t: &str| c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get::<_, i64>(0));
            Ok((
                n("chats")? as usize,
                n("messages")? as usize,
                n("contacts")? as usize,
            ))
        })
    }

    // -- sync ------------------------------------------------------------------------

    /// Fold one operation into the store and advance the stream position.
    ///
    /// `preview` turns a message into its chat-list summary. Messages from
    /// others raise the chat's unread count; reading on any device clears it.
    pub fn apply(&self, op: &Operation, my_mid: &str, preview: impl Fn(&Message) -> String) -> Result<()> {
        match op.event() {
            OpEvent::Message(m) => {
                let chat = m.chat_mid(my_mid).to_owned();
                let err = op.e2ee_error.as_ref().map(|e| e.detail.as_str());
                if self.put_message(&chat, m, err)? {
                    let mine = m.from == my_mid;
                    let time = m.created_time.unwrap_or_else(now_ms);
                    self.update_chat(&chat, |c| {
                        if time >= c.last_time {
                            c.last_time = time;
                            c.last_message_id = Some(m.id.clone());
                            c.preview = preview(m);
                        }
                        if mine {
                            c.unread = 0;
                            c.last_seen = Some(m.id.clone());
                        } else {
                            c.unread += 1;
                        }
                    })?;
                }
            }
            OpEvent::Read {
                chat,
                reader,
                message_id,
            } => self.set_read_mark(chat, reader, message_id)?,
            OpEvent::ChatChecked { chat, message_id } => {
                if self.chat(chat)?.is_some() {
                    self.update_chat(chat, |c| {
                        let newest = c
                            .last_message_id
                            .as_deref()
                            .is_none_or(|l| message_id_cmp(message_id, l).is_ge());
                        if newest {
                            c.unread = 0;
                        }
                        c.last_seen = Some(message_id.to_owned());
                    })?;
                }
            }
            OpEvent::Unsent { message_id, .. } => {
                self.mark_unsent(message_id)?;
            }
            OpEvent::Reaction {
                message_id,
                from,
                reaction,
                ..
            } => {
                self.set_reaction(message_id, from, reaction, now_ms())?;
            }
            OpEvent::LeftChat { chat } => self.delete_chat(chat)?,
            OpEvent::InvitationAnswered { chat, accepted } => {
                if accepted {
                    if self.chat(chat)?.is_some() {
                        self.update_chat(chat, |c| c.invited = false)?;
                    }
                } else {
                    self.delete_chat(chat)?;
                }
            }
            _ => {}
        }
        self.set_stream_revision(op.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(id: &str, from: &str, to: &str, t: i64, text: &str) -> Message {
        serde_json::from_value(
            json!({"id": id, "from": from, "to": to, "toType": if to.starts_with('C') {2} else {0},
            "contentType": 0, "text": text, "createdTime": t.to_string()}),
        )
        .unwrap()
    }

    fn op(rev: i64, v: serde_json::Value) -> Operation {
        let mut v = v;
        v["revision"] = json!(rev);
        serde_json::from_value(v).unwrap()
    }

    fn preview(m: &Message) -> String {
        m.text.clone().unwrap_or_default()
    }

    #[test]
    fn imported_history_dedupes_and_survives_gap_clears() {
        let s = Store::open_in_memory().unwrap();
        let export = crate::import::parse_chat_export(
            "2024/01/15 Mon\n09:00\tAlice\thello\n09:00\tMe\t[Sticker]\n09:01\tAlice\tbye\n",
        )
        .unwrap();
        let map = |n: &str| match n {
            "Alice" => Some("Ua".to_owned()),
            "Me" => Some("Ume".to_owned()),
            _ => None,
        };
        let msgs = export.to_messages("Ua", map, |t| t.utc_ms(0));
        let t0 = msgs[0].created_time.unwrap();
        // The real "hello" is already stored: its imported copy is skipped.
        s.put_message("Ua", &msg("500", "Ua", "Ume", t0, "hello"), None)
            .unwrap();
        let st = s.import_messages("Ua", &msgs).unwrap();
        assert_eq!((st.added, st.duplicates, st.already), (2, 1, 0));
        assert_eq!(s.import_messages("Ua", &msgs).unwrap().already, 2);
        assert_eq!(s.imported_count("Ua").unwrap(), 2);
        // Ordered by time with the real one; the newest fetched is the real one.
        let all = s.messages("Ua", 10, None).unwrap();
        let texts: Vec<_> = all.iter().map(|m| m.message.text.clone().unwrap()).collect();
        assert_eq!(texts, ["hello", "[Sticker]", "bye"]);
        assert_eq!(s.newest_fetched_message("Ua").unwrap().unwrap().message.id, "500");
        // Fetching the real "bye" later replaces the imported copy.
        s.put_message("Ua", &msg("501", "Ua", "Ume", t0 + 61_000, "bye"), None)
            .unwrap();
        assert_eq!(s.imported_count("Ua").unwrap(), 1);
        s.clear_fetched_messages("Ua").unwrap();
        assert_eq!(s.message_count("Ua").unwrap(), 1);
        assert_eq!(s.delete_imported("Ua").unwrap(), 1);
        assert_eq!(s.message_count("Ua").unwrap(), 0);
    }

    #[test]
    fn pinned_chats_are_idempotent_persistent_and_pruned() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rune-pinned-{}-{unique}.db", std::process::id()));
        let s = Store::open(&path).unwrap();
        assert!(s.set_pinned("Ua", true).unwrap());
        assert!(!s.set_pinned("Ua", true).unwrap());
        assert!(s.is_pinned("Ua").unwrap());
        assert!(s.set_pinned("Ub", true).unwrap());
        s.upsert_chat(&ChatRow {
            mid: "Cinvite".into(),
            invited: true,
            ..Default::default()
        })
        .unwrap();
        assert!(!s.set_pinned("Cinvite", true).unwrap());
        assert!(!s.move_pinned("Cinvite", 1).unwrap());
        assert!(!s.bind_account("Uowner").unwrap());
        assert_eq!(s.pinned_chats().unwrap(), ["Ua", "Ub"]);
        assert!(s.move_pinned("Ub", -1).unwrap());
        assert_eq!(s.pinned_chats().unwrap(), ["Ub", "Ua"]);
        assert!(
            !s.move_pinned("Ub", -1).unwrap(),
            "moving before the start is a no-op"
        );
        s.set_pinned_order(&["Ua".into(), "Ub".into()]).unwrap();
        let pinned = s.pinned_chats().unwrap();
        assert_eq!(pinned, ["Ua", "Ub"]);
        drop(s);

        let s = Store::open(&path).unwrap();
        assert!(!s.bind_account("Uowner").unwrap());
        assert_eq!(s.pinned_chats().unwrap(), pinned);
        assert!(s.set_pinned("Ua", false).unwrap());
        assert!(!s.set_pinned("Ua", false).unwrap());
        assert!(!s.is_pinned("Ua").unwrap());

        assert!(s.set_pinned("Uc", true).unwrap());
        s.delete_chat("Uc").unwrap();
        assert!(!s.is_pinned("Uc").unwrap());
        assert!(s.set_pinned("Cg", true).unwrap());
        s.clear().unwrap();
        assert!(s.pinned_chats().unwrap().is_empty());
        drop(s);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pins_are_separate_per_profile_and_order_survives_reopen() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rune-profiles-{}-{unique}.db", std::process::id()));
        let s = Store::open(&path).unwrap();
        assert!(!s.bind_account("UA").unwrap());
        assert!(s.set_pinned("Ca", true).unwrap());
        assert!(s.set_pinned("Cb", true).unwrap());
        assert!(s.move_pinned("Cb", -1).unwrap());
        s.set_profile_name("Alice").unwrap();
        assert_eq!(s.profile().unwrap().name, "Alice");

        assert!(s.bind_account("UB").unwrap());
        assert!(s.pinned_chats().unwrap().is_empty());
        assert_eq!(s.pinned_chats_of("UA").unwrap(), ["Cb", "Ca"]);
        assert!(s.set_pinned("Cc", true).unwrap());
        s.set_profile_name("Bob").unwrap();
        assert_eq!(s.pinned_chats_of("UB").unwrap(), ["Cc"]);

        assert!(s.bind_account("UA").unwrap());
        assert_eq!(s.pinned_chats().unwrap(), ["Cb", "Ca"]);
        assert_eq!(s.profile().unwrap().name, "Alice");
        assert!(s.bind_account("UB").unwrap());
        s.clear().unwrap();
        assert!(s.pinned_chats().unwrap().is_empty());
        assert_eq!(s.pinned_chats_of("UA").unwrap(), ["Cb", "Ca"]);
        assert!(!s.delete_profile("UB").unwrap(), "the active profile is retained");
        assert!(s.bind_account("UA").unwrap());
        assert!(s.delete_profile("UB").unwrap());
        assert!(s.pinned_chats_of("UB").unwrap().is_empty());
        drop(s);

        let s = Store::open(&path).unwrap();
        assert!(!s.bind_account("UA").unwrap());
        assert_eq!(s.pinned_chats().unwrap(), ["Cb", "Ca"]);
        assert_eq!(s.profile().unwrap().name, "Alice");
        drop(s);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migrates_v1_pins_in_existing_order() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rune-v1-{}-{unique}.db", std::process::id()));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('schema', '1'), ('owner', 'Ulegacy');
             CREATE TABLE pinned_chats (mid TEXT PRIMARY KEY, pinned_at INTEGER NOT NULL);
             INSERT INTO pinned_chats (mid, pinned_at) VALUES ('Ub', 20), ('Ua', 10);",
        )
        .unwrap();
        drop(conn);

        let s = Store::open(&path).unwrap();
        assert_eq!(s.get("schema").unwrap().as_deref(), Some("3"));
        assert_eq!(s.pinned_chats().unwrap(), ["Ua", "Ub"]);
        assert_eq!(s.profile().unwrap().mid, "Ulegacy");
        let lists = s.pin_profiles().unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, DEFAULT_PIN_PROFILE);
        assert!(lists[0].active);
        drop(s);
        std::fs::remove_file(path).unwrap();
    }

    fn temp_db(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rune-{tag}-{}-{unique}.db", std::process::id()))
    }

    #[test]
    fn migrates_v2_pins_into_default_pin_profiles() {
        let path = temp_db("v2");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(PINNED_SCHEMA_V2).unwrap();
        conn.execute_batch(
            "INSERT INTO meta (key, value) VALUES ('schema', '2'), ('owner', 'UA');
             INSERT INTO profiles (mid, name, created, last_used) VALUES
                 ('UA', 'Alice', 1, 2), ('UB', 'Bob', 1, 1), ('UC', '', 1, 1);
             INSERT INTO pinned_chats (profile, mid, position, pinned_at) VALUES
                 ('UA', 'Cx', 1, 5), ('UA', 'Cy', 0, 9), ('UB', 'Cz', 0, 1), ('', 'Cu', 0, 1);",
        )
        .unwrap();
        drop(conn);

        let s = Store::open(&path).unwrap();
        assert_eq!(s.get("schema").unwrap().as_deref(), Some("3"));
        assert_eq!(s.pinned_chats().unwrap(), ["Cy", "Cx"], "order is kept");
        assert_eq!(s.pinned_chats_of("UB").unwrap(), ["Cz"]);
        assert_eq!(s.pinned_chats_of("").unwrap(), ["Cu"]);
        for account in ["UA", "UB", "UC", ""] {
            let lists = s.pin_profiles_of(account).unwrap();
            assert_eq!(lists.len(), 1, "{account}");
            assert_eq!(lists[0].name, DEFAULT_PIN_PROFILE);
            assert!(lists[0].active);
        }
        assert_eq!(s.active_pin_profile().unwrap().pins, 2);
        assert!(!s.bind_account("UA").unwrap());
        assert_eq!(s.pinned_chats().unwrap(), ["Cy", "Cx"]);
        drop(s);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pin_profiles_are_independent_ordered_and_persistent() {
        let path = temp_db("pin-profiles");
        let s = Store::open(&path).unwrap();
        s.bind_account("UA").unwrap();
        let default = s.active_pin_profile().unwrap();
        assert_eq!(default.name, DEFAULT_PIN_PROFILE);
        s.set_pinned("C1", true).unwrap();
        s.set_pinned("C2", true).unwrap();
        s.move_pinned("C2", -1).unwrap();

        let work = s.create_pin_profile("  Work ").unwrap();
        assert_eq!(work.name, "Work");
        assert!(!work.active, "creating does not switch");
        assert!(
            s.create_pin_profile("work").is_err(),
            "names are unique ignoring case"
        );
        assert!(s.create_pin_profile("   ").is_err());
        assert!(
            s.create_pin_profile(&"x".repeat(PIN_PROFILE_NAME_MAX + 1))
                .is_err()
        );

        s.switch_pin_profile(work.id).unwrap();
        assert!(s.pinned_chats().unwrap().is_empty());
        s.set_pinned("C3", true).unwrap();
        s.set_pinned("C1", true).unwrap();
        assert_eq!(s.pinned_chats().unwrap(), ["C3", "C1"]);
        assert_eq!(s.pinned_chats_in_profile(default.id).unwrap(), ["C2", "C1"]);

        s.switch_pin_profile(default.id).unwrap();
        assert_eq!(
            s.pinned_chats().unwrap(),
            ["C2", "C1"],
            "order kept across switches"
        );
        s.set_pinned("C1", false).unwrap();
        assert_eq!(s.pinned_chats_in_profile(work.id).unwrap(), ["C3", "C1"]);
        s.switch_pin_profile(work.id).unwrap();

        s.rename_pin_profile(work.id, "Office").unwrap();
        assert!(s.rename_pin_profile(work.id, "default").is_err());
        s.rename_pin_profile(work.id, "OFFICE").unwrap();

        // Other accounts neither see nor can touch these lists.
        s.bind_account("UB").unwrap();
        assert_eq!(s.pin_profiles().unwrap().len(), 1);
        assert!(s.switch_pin_profile(work.id).is_err());
        assert!(s.rename_pin_profile(work.id, "x").is_err());
        assert!(s.delete_pin_profile(work.id).is_err());
        let b_default = s.active_pin_profile().unwrap();
        assert!(s.delete_pin_profile(b_default.id).is_err(), "last list stays");
        drop(s);

        let s = Store::open(&path).unwrap();
        s.bind_account("UA").unwrap();
        let lists = s.pin_profiles().unwrap();
        assert_eq!(
            lists
                .iter()
                .map(|p| (p.name.as_str(), p.active, p.pins))
                .collect::<Vec<_>>(),
            [("Default", false, 1), ("OFFICE", true, 2)]
        );
        assert_eq!(
            s.pinned_chats().unwrap(),
            ["C3", "C1"],
            "the active list survives restarts"
        );

        // Leaving a chat unpins it everywhere; clearing empties the active list only.
        s.delete_chat("C1").unwrap();
        assert_eq!(s.pinned_chats().unwrap(), ["C3"]);
        s.clear().unwrap();
        assert!(s.pinned_chats().unwrap().is_empty());
        assert_eq!(s.pinned_chats_in_profile(default.id).unwrap(), ["C2"]);

        // Deleting the active list activates its neighbour.
        s.delete_pin_profile(work.id).unwrap();
        let lists = s.pin_profiles().unwrap();
        assert_eq!(lists.len(), 1);
        assert!(lists[0].active);
        assert_eq!(s.pinned_chats().unwrap(), ["C2"]);
        assert!(s.pinned_chats_in_profile(work.id).unwrap().is_empty());

        // Deleting the account profile removes its pin lists.
        s.bind_account("UB").unwrap();
        assert!(s.delete_profile("UA").unwrap());
        assert!(s.pin_profiles_of("UA").unwrap().is_empty());
        drop(s);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unbound_pin_profiles_are_adopted_on_first_bind() {
        let s = Store::open_in_memory().unwrap();
        s.set_pinned("C1", true).unwrap();
        let trips = s.create_pin_profile("Trips").unwrap();
        s.switch_pin_profile(trips.id).unwrap();
        s.set_pinned("C2", true).unwrap();
        s.bind_account("UA").unwrap();
        let lists = s.pin_profiles().unwrap();
        assert_eq!(
            lists
                .iter()
                .map(|p| (p.name.as_str(), p.active, p.pins))
                .collect::<Vec<_>>(),
            [("Default", false, 1), ("Trips", true, 1)]
        );
        assert!(s.pin_profiles_of("").unwrap().is_empty());
        assert_eq!(s.pinned_chats().unwrap(), ["C2"]);
    }

    #[test]
    fn users_and_images_round_trip_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let u = UserRow {
            mid: "Ua".into(),
            name: "Alice".into(),
            status: "hi".into(),
            picture: Some("/0hpic".into()),
            cover: Some("/r/myhome/c/oid".into()),
            members: vec!["Ub".into()],
            updated: 0,
        };
        s.upsert_user(&u).unwrap();
        let got = s.user("Ua").unwrap().unwrap();
        assert!(got.updated > 0);
        assert_eq!(UserRow { updated: 0, ..got }, u);
        assert_eq!(s.users().unwrap().len(), 1);
        assert!(s.user("Uz").unwrap().is_none());

        s.put_image("a", &[1; 10]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.put_image("b", &[2; 10]).unwrap();
        assert_eq!(s.image("a").unwrap().unwrap(), vec![1; 10]);
        // Over the limit: the older one goes.
        assert_eq!(s.prune_images(15).unwrap(), 1);
        assert!(s.image("a").unwrap().is_none());
        assert!(s.image("b").unwrap().is_some());

        s.clear().unwrap();
        assert!(s.users().unwrap().is_empty() && s.image("b").unwrap().is_none());
    }

    #[test]
    fn messages_page_backwards_in_order() {
        let s = Store::open_in_memory().unwrap();
        let msgs: Vec<Message> = (1..=5)
            .map(|i| msg(&format!("10{i}"), "Ua", "Ume", i * 1000, &format!("m{i}")))
            .collect();
        assert_eq!(s.put_messages("Ua", &msgs, None).unwrap(), 5);
        assert_eq!(s.put_messages("Ua", &msgs[..2], None).unwrap(), 0, "idempotent");
        let last2 = s.messages("Ua", 2, None).unwrap();
        let texts: Vec<_> = last2.iter().map(|m| m.message.text.clone().unwrap()).collect();
        assert_eq!(texts, ["m4", "m5"]);
        let older = s.messages("Ua", 10, Some("104")).unwrap();
        assert_eq!(older.len(), 3);
        assert_eq!(older[0].message.id, "101");
        assert!(s.messages("Ub", 10, None).unwrap().is_empty());
    }

    #[test]
    fn apply_tracks_unread_reads_reactions_and_unsend() {
        let s = Store::open_in_memory().unwrap();
        let me = "Ume";
        let m1 = msg("200", "Ua", "Cg", 1000, "hi");
        s.apply(&op(10, json!({"type": 26, "message": m1})), me, preview)
            .unwrap();
        s.apply(
            &op(
                11,
                json!({"type": 26, "message": msg("201", "Ub", "Cg", 2000, "yo")}),
            ),
            me,
            preview,
        )
        .unwrap();
        // A replay of the same op changes nothing.
        s.apply(
            &op(
                11,
                json!({"type": 26, "message": msg("201", "Ub", "Cg", 2000, "yo")}),
            ),
            me,
            preview,
        )
        .unwrap();
        let c = s.chat("Cg").unwrap().unwrap();
        assert_eq!((c.unread, c.preview.as_str(), c.last_time), (2, "yo", 2000));
        assert_eq!(s.stream_revision().unwrap(), Some(11));

        // Read on the phone.
        s.apply(
            &op(
                12,
                json!({"type": 40, "param1": "Cg", "param2": "201", "param3": "0"}),
            ),
            me,
            preview,
        )
        .unwrap();
        assert_eq!(s.chat("Cg").unwrap().unwrap().unread, 0);

        // Our message; others read it.
        s.apply(
            &op(
                13,
                json!({"type": 25, "message": msg("202", me, "Cg", 3000, "mine")}),
            ),
            me,
            preview,
        )
        .unwrap();
        s.apply(
            &op(
                14,
                json!({"type": 55, "param1": "Cg", "param2": "Ua", "param3": "202"}),
            ),
            me,
            preview,
        )
        .unwrap();
        s.apply(
            &op(
                15,
                json!({"type": 55, "param1": "Cg", "param2": "Ua", "param3": "200"}),
            ),
            me,
            preview,
        )
        .unwrap();
        assert_eq!(
            s.read_marks("Cg").unwrap()["Ua"],
            "202",
            "read marks never move back"
        );

        s.apply(
            &op(
                16,
                json!({"type": 140, "param1": "202", "param3": "Ub",
                "param2": "{\"chatMid\":\"Cg\",\"curr\":{\"predefinedReactionType\":3}}"}),
            ),
            me,
            preview,
        )
        .unwrap();
        let r = s.message("202").unwrap().unwrap().message.reactions();
        assert_eq!((r.len(), r[0].from.as_str()), (1, "Ub"));

        s.apply(
            &op(
                17,
                json!({"type": 65, "param1": "Cg", "param2": "201", "param3": "0"}),
            ),
            me,
            preview,
        )
        .unwrap();
        assert!(s.message("201").unwrap().unwrap().unsent);

        s.apply(&op(18, json!({"type": 127, "param1": "Cg"})), me, preview)
            .unwrap();
        assert!(s.chat("Cg").unwrap().is_none());
        assert_eq!(s.message_count("Cg").unwrap(), 0);
        assert_eq!(s.stream_revision().unwrap(), Some(18));
        s.set_stream_revision(3).unwrap();
        assert_eq!(s.stream_revision().unwrap(), Some(18), "never backwards");
    }

    #[test]
    fn file_is_private_and_bound_to_one_account() {
        let dir = std::env::temp_dir().join(format!("rune-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rune.db");
        {
            let s = Store::open(&path).unwrap();
            assert!(!s.bind_account("Ume").unwrap());
            s.put_message("Ua", &msg("1", "Ua", "Ume", 1, "x"), None).unwrap();
            s.set("setting.bell", "on").unwrap();
            s.set_stream_revision(42).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.stream_revision().unwrap(), Some(42), "survives a restart");
        assert!(!s.bind_account("Ume").unwrap());
        assert!(s.bind_account("Uother").unwrap(), "another account wipes");
        assert_eq!(s.stats().unwrap(), (0, 0, 0));
        assert_eq!(s.stream_revision().unwrap(), None);
        assert_eq!(s.get("setting.bell").unwrap().as_deref(), Some("on"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recent_stickers_dedupe() {
        let s = Store::open_in_memory().unwrap();
        let stk = |id: &str, t: i64, stk: &str| -> Message {
            serde_json::from_value(json!({"id": id, "from": "Ua", "to": "Ume", "contentType": 7,
                "createdTime": t, "contentMetadata": {"STKID": stk, "STKPKGID": "11537", "STKVER": "2"}}))
            .unwrap()
        };
        s.put_messages(
            "Ua",
            &[stk("1", 1, "a"), stk("2", 2, "b"), stk("3", 3, "a")],
            None,
        )
        .unwrap();
        assert_eq!(
            s.recent_stickers(10).unwrap(),
            [("11537".into(), "a".into(), 2), ("11537".into(), "b".into(), 2)]
        );
    }
}
