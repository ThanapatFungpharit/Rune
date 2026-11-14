//! # rune — an unofficial Rust client for LINE
//!
//! Speaks the protocol of the LINE **Chrome extension** (`CHROMEOS` 3.7.2)
//! against `line-chrome-gw.line-apps.com`: Thrift-over-JSON RPC, the
//! mandatory `X-Hmac` signature, QR login and the SSE operation stream.
//!
//! This is **not** an official LINE API. It automates a personal account the
//! way the extension does; LINE may change or block it at any time, and
//! automated use can violate LINE's terms and get an account restricted.
//!
//! The `X-Hmac` signature and all Letter Sealing primitives come from LINE's
//! own `ltsm.wasm`, which this crate executes natively with wasmtime (see
//! [`ltsm`]); no Node.js or browser is needed.
//!
//! ```no_run
//! # async fn demo() -> rune::Result<()> {
//! use rune::{Client, Config, QrLoginEvent, QrLoginOptions, Session};
//!
//! let client = Client::new(Config::default())?;
//! let login = client
//!     .qr_login(QrLoginOptions::default(), |ev| match ev {
//!         QrLoginEvent::QrUrl(url) => println!("scan: {url}"),
//!         QrLoginEvent::Pin(pin) => println!("enter PIN {pin} on your phone"),
//!     })
//!     .await?;
//! Session { tokens: client.tokens(), e2ee_keys: login.e2ee_keys }.save("session.json")?;
//!
//! let me = client.profile().await?;
//! client.send_text(&me.mid, "hello from rust").await?;
//! # Ok(()) }
//! ```

pub mod album;
pub mod auth;
pub mod client;
pub mod e2ee;
pub mod endpoints;
pub mod error;
pub mod events;
mod exif;
pub mod import;
pub mod ltsm;
pub mod media;
pub mod models;
pub mod ops;
pub mod session;
#[cfg(feature = "store")]
pub mod store;
pub mod timeline;
pub mod transport;

pub use album::{AlbumEvent, AlbumMedia, AlbumPost, AlbumPostKind};
pub use auth::{E2eeKey, LoginResult, QrLoginEvent, QrLoginOptions};
pub use client::{Client, Reaction, ReadRange, StickerPackage};
pub use error::{ApiError, E2eeErrorKind, E2eeFailure, Error, Result};
pub use events::{Event, OperationStream};
pub use import::{ChatExport, parse_chat_export};
pub use media::{MediaKind, MediaUpload, PictureSize, StickerImage};
pub use models::{
    CallMedia, CallOutcome, CallRecord, Chat, Contact, Message, MessageReaction, MidType, Operation,
    OutgoingMessage, Profile, ReactionKind,
};
pub use ops::OpEvent;
pub use session::Session;
#[cfg(feature = "store")]
pub use store::{ImportStats, PinProfile, Store};
pub use timeline::{ObsObject, ProfileCover};
pub use transport::{Config, Tokens};
