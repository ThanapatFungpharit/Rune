//! `rune-tui` — a terminal chat client built on the `rune` library.
//!
//! ```sh
//! cargo run --release --features tui --bin rune-tui [SESSION]
//! ```
//!
//! Options (`--help` lists them; see `cli.rs`, which refuses unknown ones):
//! `--no-read-receipts` (don't mark chats as read when opened),
//! `--no-graphics` (draw pictures with text instead of the terminal's image protocol),
//! `--cells MODE`, `--no-mouse` (leave the mouse to the terminal, e.g. for text selection),
//! `--db PATH` (local message store; default: the session path with `.db`),
//! `--no-store` (keep nothing on disk: an in-memory store), `--log PATH`
//! (diagnostics to a 0600 file; `RUST_LOG` sets the level), `--plugins DIR`,
//! `--no-plugins`.
//!
//! Chats, messages (decrypted), contacts, read marks and the event-stream
//! position live in a local SQLite store, so start-up shows the chat list at
//! once and catches up on what happened while rune was closed.
//!
//! Keys are context-aware and listed in full by `?` (or `F1`, which also
//! works while typing); the status line hints at the common ones. Arrows
//! navigate (`j`/`k` are aliases in lists), `Enter` confirms or opens, `Esc`
//! goes back and `m` opens the selected item's menu, which lists everything
//! that can be done with it. `Ctrl-x` opens the main menu (everything not
//! about one chat or message, each entry with its shortcut), `Ctrl-c` quits,
//! `Ctrl-s` opens settings, `Ctrl-r` turns read receipts on or off; with a
//! chat open, `Ctrl-o` sends a photo/file/sticker/location/contact, `Ctrl-g`
//! opens the chat's menu and `Ctrl-p` its profile — these work while typing.
//! Screens opened from Settings come back to it when they close
//! (`App::modal_parent`).
//!
//! In a chat you type straight away (the line edits at a cursor: `←` `→`
//! `Home` `End` `Delete`); `↑` steps up into the messages, `↓` past the
//! newest one returns to typing (so does any letter that is not a message
//! key). On a selected message: `Enter` views media (videos, audio and media
//! files play in mpv) or opens its link, `r` reply (`Esc` cancels), `e`
//! react, `s` save media to `~/Downloads/rune`, `o` open it with the system
//! viewer (from `~/.cache/rune/view`, so Downloads only holds what you
//! saved), `p` the sender's profile, `x` unsend (yours, asks first). In the
//! viewer: `←/→` previous/next media, `Space` play/pause, `s`, `o`, `u` (QR
//! link), `Esc`. `Tab` jumps between the chat list and the open chat.
//!
//! The chat list starts with the active chats; moving to the last one loads
//! older and hidden chats, a page at a time. Group invitations are listed
//! first, in yellow: `Enter` on one asks whether to join or decline. Pins are
//! kept in order per pin list (`K`/`J` move the selected pin); unread pins
//! sit at the top of the pinned section until read, and other unread chats
//! (not muted ones) sit right under the pins until read. Unread chats out
//! of view are counted on the list's border (`▲ n unread` / `▼ n unread`,
//! click to go there); `u` selects the nearest unread chat. In the chat
//! list `/` (or `Ctrl-f` anywhere) searches chats and friends, `m` opens the chat's menu (profile,
//! albums, mute, pin, invite, rename, leave), `n` creates a group and `p`
//! shows the chat's profile. Pin lists are in the main menu and Settings › Chats.
//!
//! Mouse: click a reaction line to see who reacted; click a chat to open it,
//! click a picture to view it, wheel to
//! scroll; in the viewer the wheel steps through media and a click outside
//! (or right-click) closes it. Shift-drag still selects text in most terminals.
//! Links are underlined; click one to open it in the browser. Pictures are
//! scanned for QR codes, shown under the picture and in the viewer (`u`
//! or a click opens a QR link).
//!
//! Profiles: `p` on a chat or message, `Ctrl-p` for the open chat, your own
//! from the main menu or Settings › Account, or click the chat title or a sender's name. The
//! page shows the picture, name, status message, buttons (`←`/`→` and
//! `Enter`, or click) and, for groups, members; a member opens their profile,
//! `Esc` goes back.
//!
//! Commands in the input line act on the open chat: `/attach [PATH]` (also
//! `/image`, `/file`), `/sticker`, `/location`, `/contact`, `/invite`,
//! `/rename`, `/albums`, `/leave`, `/mute`, `/unmute`, `/save PATH` (latest
//! media in this chat) (the message line's help screen lists them all, from
//! `keymap::COMMANDS`). Screens are opened with keys and menus, never
//! typed; an unknown `/word` is never sent.
//! Pasting (or dropping) a file path into the input line offers to send it.
//!
//! Plugins (`plugins/`, `docs/PLUGINS.md`): sandboxed WebAssembly modules
//! that add `/commands`, `Alt-<letter>` keys, views, forms and settings;
//! Settings › Plugins (also `Ctrl-x` › Plugins) installs them from Git,
//! enables, configures, updates and uninstalls them; changed files reload
//! while rune runs.
//!
//! GIFs, animated stickers (APNG) and animated WebPs play, looping, in the
//! viewer itself: frames are decoded one at a time and replace the picture
//! on a fixed 12 fps tick (`Space` pauses). Videos and audio play in mpv (written to
//! `~/.cache/rune/view` first); without mpv the system app opens them.
//!
//! Pictures (inline and in the viewer) use the terminal's image protocol —
//! Kitty, Sixel or iTerm2, as detected at start-up — at full resolution.
//! Without one, or with `--no-graphics`, they are drawn with coloured block
//! glyphs: quadrants (2×2 pixels per cell) by default, `--cells sextant` for
//! 2×3 in terminals that draw Unicode's legacy-computing sextants, `--cells
//! half` for plain half-blocks.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange,
    EnableMouseCapture, Event as TermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::FilterType;
use ratatui_image::picker::Picker;
use ratatui_image::picker::ProtocolType;

mod cli;
mod clipboard;
mod graphics;
mod input;
mod keymap;
mod links;
mod notice;
mod overlays;
mod plugins;
mod receipts;
mod retry;
mod theme;
use albums::media_type;
use avatar::{Avatar, HEAD_AVATAR, LIST_AVATAR};
use cells::CellMode;
use fetchq::{FetchQueue, MediaKey};
use graphics::{anim, avatar, cells, fetchq, piccache, picture};
use input::{Edit, TextInput};
use modal::{Action, Modal};
use overlays::{albums, attach, find, modal, plugin_ui, profile, settings, shortcuts, stickers};
use piccache::PicCache;
use picture::{Layer, Picture};
use profile::{ProfileAction, ProfileData, ProfileLoaded};
use receipts::Receipts;
use retry::{Job, RetryQueue};
use rune::models::{Operation, content_type as ct, op_type};
use rune::ops::message_id_cmp;
use rune::store::{ChatRow, UserRow};
use rune::{
    Chat, Client, Config, Contact, MediaUpload, Message, MessageReaction, MidType, OpEvent, OutgoingMessage,
    PictureSize, Profile, QrLoginEvent, QrLoginOptions, ReactionKind, Session, StickerImage, StickerPackage,
    Store,
};
use settings::{NotifyMode, Prefs};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// -- model -----------------------------------------------------------------------------

/// Where [`App::jump_to_unread`] looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Toward {
    /// The closest to the selection, either way.
    Nearest,
    /// The closest above what is on screen.
    Above,
    /// The closest below what is on screen.
    Below,
}

/// The position in `rows` nearest to `from`, never `from` itself; on a tie
/// the one above (unread chats float up, so that is where more of them are).
fn nearest_unread(rows: &[usize], from: usize) -> Option<usize> {
    rows.iter()
        .copied()
        .filter(|&k| k != from)
        .min_by_key(|&k| (k.abs_diff(from), k > from))
}

#[derive(Debug, Clone, Default)]
struct ChatEntry {
    mid: String,
    preview: String,
    last_time: i64,
    unread: u32,
    /// A group invitation we have not answered yet.
    invited: bool,
    /// LINE's notification setting for the chat is off.
    muted: bool,
    /// Newest message id (from the server's chat list or live).
    last_id: Option<String>,
    /// Newest message we have read.
    last_seen: Option<String>,
}

impl ChatEntry {
    fn from_row(r: ChatRow) -> Self {
        ChatEntry {
            mid: r.mid,
            preview: r.preview,
            last_time: r.last_time,
            unread: r.unread,
            invited: r.invited,
            muted: r.muted,
            last_id: r.last_message_id,
            last_seen: r.last_seen,
        }
    }

    /// Unread and not muted: floats above the other unpinned chats and is
    /// what the chat list's unread indicators and `u` lead to.
    fn wants_attention(&self) -> bool {
        self.unread > 0 && !self.muted && !self.invited
    }
}

/// Chat-list preview of a pending invitation.
fn invite_preview() -> String {
    format!(
        "Invitation · {}",
        keymap::with_keys(keymap::Ctx::ChatList, keymap::Cmd::Confirm, "to join or decline")
    )
}

impl ChatEntry {
    fn invitation(chat: &Chat) -> Self {
        ChatEntry {
            mid: chat.chat_mid.clone(),
            preview: invite_preview(),
            last_time: chat.created_time().unwrap_or(0),
            unread: 0,
            invited: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
struct UiMsg {
    id: String,
    from: String,
    time: i64,
    text: String,
    /// Kept for media and sticker messages so they can be fetched later.
    raw: Option<Message>,
    /// Id of the message this one quotes.
    reply_to: Option<String>,
    forwarded: bool,
    reactions: Vec<MessageReaction>,
    /// Unsent by its sender.
    unsent: bool,
    /// `deliveredTime`, the cursor for older history.
    delivered: i64,
    /// Composed by its sender (not a call record, album notice or bot card):
    /// sending it means they had the chat open, so they had seen what came before.
    written: bool,
    /// A text message: `text` is what its sender typed (and can be copied).
    plain: bool,
}

/// How we know a member has seen a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SeenBy {
    /// Their read receipt covers it.
    Receipt,
    /// They sent a message after it.
    WroteLater,
    /// They reacted to it or to a later message.
    Reacted,
}

/// How far one member is known to have seen a chat (see [`App::read_state`]).
#[derive(Debug, Default)]
struct Seen {
    /// Newest message id their read receipts cover.
    receipt: Option<String>,
    /// Newest message id they wrote or reacted to.
    inferred: Option<(String, SeenBy)>,
}

/// The members of `state` who have seen message `id`, sorted by mid; a read
/// receipt wins over an inference.
fn readers_in(state: &HashMap<String, Seen>, id: &str) -> Vec<(String, SeenBy)> {
    let covers = |mark: &str| message_id_cmp(mark, id).is_ge();
    let mut v: Vec<(String, SeenBy)> = state
        .iter()
        .filter_map(|(who, seen)| {
            let how = match (&seen.receipt, &seen.inferred) {
                (Some(r), _) if covers(r) => SeenBy::Receipt,
                (_, Some((i, how))) if covers(i) => *how,
                _ => return None,
            };
            Some((who.clone(), how))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

impl UiMsg {
    fn media_kind(&self) -> Option<i64> {
        if self.unsent {
            return None;
        }
        self.raw
            .as_ref()
            .map(media_type)
            .filter(|t| matches!(*t, ct::IMAGE | ct::VIDEO | ct::AUDIO | ct::FILE | ct::STICKER))
    }

    /// Plays in mpv: video, audio and audio/video files. (Animations play
    /// in the viewer.)
    fn plays(&self) -> bool {
        let Some(raw) = self.raw.as_ref().filter(|_| !self.unsent) else {
            return false;
        };
        match media_type(raw) {
            ct::VIDEO | ct::AUDIO => true,
            ct::FILE => raw
                .content_metadata
                .get("FILE_NAME")
                .is_some_and(|n| is_motion_file(n)),
            _ => false,
        }
    }

    /// Its text is worth copying: what its sender typed, still there.
    fn copyable(&self) -> bool {
        self.plain && !self.unsent && !self.text.is_empty()
    }

    /// Has a picture we can draw (thumbnail / viewer).
    fn is_visual(&self) -> bool {
        matches!(self.media_kind(), Some(ct::IMAGE | ct::VIDEO | ct::STICKER))
    }
}

/// Where paging through older chats stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MoreChats {
    /// More may be there; the next page starts after this chat id.
    Ready(Option<String>),
    Loading(Option<String>),
    /// Every chat is listed.
    Done,
}

/// A row of the chat list: a listed chat (an index into `App::chats`) or,
/// while a search query is typed, a friend who has no chat listed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ListRow {
    Chat(usize),
    Friend(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Chats,
    Messages,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Unread {
    After(String),
    Last(u32),
}

/// A loading slot.
enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// Downloaded originals kept in memory before the least recently used go
/// (they are fetched again when needed). The newest one always stays.
const FULL_CACHE_BYTES: usize = 256 * 1024 * 1024;

/// A downloaded original.
struct FullMedia {
    data: Vec<u8>,
    file_name: String,
    /// Drawn in the viewer (images, stickers; video shows its preview).
    picture: Option<Picture>,
    /// Set when the original is gone from LINE's server and `data` is the preview.
    note: Option<String>,
}

/// Pictures are shrunk to this many pixels per side once decoded: enough for a
/// full-screen viewer, and keeps re-encoding on resize fast.
const MAX_PICTURE: u32 = 1600;

/// Decode and shrink a picture off the UI thread.
async fn decode_picture(bytes: Vec<u8>) -> Result<image::DynamicImage, String> {
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        Ok(if img.width() > MAX_PICTURE || img.height() > MAX_PICTURE {
            img.resize(MAX_PICTURE, MAX_PICTURE, FilterType::CatmullRom)
        } else {
            img
        })
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
}

/// A finished download, before the picture is turned into a protocol.
struct Downloaded {
    data: Vec<u8>,
    file_name: String,
    picture: Option<image::DynamicImage>,
    note: Option<String>,
    /// QR codes found in the picture.
    qr: Vec<String>,
}

/// Look for QR codes in a picture off the UI thread.
/// `img` scaled down (never up) to fit `cap` pixels, off the UI thread.
async fn shrink(img: image::DynamicImage, cap: (u32, u32)) -> image::DynamicImage {
    tokio::task::spawn_blocking(move || shrink_now(img, cap))
        .await
        .unwrap_or_else(|_| image::DynamicImage::new_rgb8(1, 1))
}

fn shrink_now(img: image::DynamicImage, cap: (u32, u32)) -> image::DynamicImage {
    if img.width() <= cap.0 && img.height() <= cap.1 {
        return img;
    }
    img.resize(cap.0.max(1), cap.1.max(1), FilterType::CatmullRom)
}

async fn scan_qr(img: image::DynamicImage) -> (image::DynamicImage, Vec<String>) {
    tokio::task::spawn_blocking(move || {
        let qr = links::scan_qr(&img);
        (img, qr)
    })
    .await
    .unwrap_or_else(|_| (image::DynamicImage::new_rgb8(1, 1), Vec::new()))
}

/// What to do once a download finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    Nothing,
    Save,
    Open,
    /// Open a file of a type that can run programs, the user having agreed
    /// (see [`runs_programs`]).
    OpenAnyway,
    /// Play it in mpv (videos, audio).
    Play,
}

/// Inline thumbnail sizes (columns × rows), picked in Settings › Chats.
const THUMB_SIZES: [(u16, u16); 4] = [(32, 8), (48, 14), (64, 20), (96, 30)];
/// Memory for decoded inline thumbnails; past it the least recently drawn
/// go (and are fetched again if scrolled back to). See `piccache`.
const THUMB_CACHE_BYTES: usize = 128 * 1024 * 1024;
/// Memory for the sticker picker's pictures.
const STICKER_CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Memory for the file browser's previews (≤1600 px each).
const PREVIEW_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Scaling for inline thumbnails: Lanczos keeps block-glyph pictures sharp
/// (nearest-neighbour makes them noisy).
const THUMB_FILTER: FilterType = FilterType::Lanczos3;
/// The viewer re-scales large originals on resize; Catmull-Rom is a good
/// quality/speed trade-off there.
const VIEW_FILTER: FilterType = FilterType::CatmullRom;

/// Results of background work, delivered to the UI loop.
enum AppEvent {
    /// SIGTERM or SIGHUP: quit as `Ctrl-q` does (see [`spawn_signal_watch`]).
    Terminate,
    Chats(Vec<ChatEntry>),
    /// A page of older (inactive) chats and the cursor for the next page
    /// (`None`: that was the last).
    MoreChats(Result<(Vec<ChatEntry>, Option<String>), String>),
    Names(HashMap<String, String>),
    /// Profile/group picture paths, per mid; `None`: it has no picture.
    Pictures(HashMap<String, Option<String>>),
    /// Mids a name/picture lookup answered for this run.
    LookedUp(Vec<String>),
    /// A profile page fetched this picture path (`None`: no picture); the
    /// page itself follows with `Profile`.
    ProfilePicture(String, Option<String>),
    /// A rounded avatar, or `None` if it could not be fetched.
    Avatar(String, Option<image::DynamicImage>),
    History(String, Vec<Message>),
    /// Older history: chat, messages (oldest first), nothing older left?
    Older(String, Vec<Message>, bool),
    Incoming(Box<Operation>),
    /// A named stream event (`fullSync`, …).
    Control(String, serde_json::Value),
    /// Sent: chat, how to show it, the message to store.
    Sent(String, Box<UiMsg>, Option<Box<Message>>),
    Status(String),
    /// A search in a chat finished (see `find`).
    Found(Box<find::Found>),
    /// Group members and invitees, per chat.
    Members(HashMap<String, (Vec<String>, Vec<String>)>),
    /// LINE's per-chat notification setting, per mid.
    Mutes(HashMap<String, bool>),
    Contacts(Vec<Contact>),
    Unsent(String, String, Result<(), String>),
    Left(String, Result<(), String>),
    Muted(String, bool, Result<(), String>),
    /// A new group, and who was invited.
    GroupCreated(Result<Chat, String>, Vec<String>),
    Invited(String, Vec<String>, Result<(), String>),
    Renamed(String, String, Result<(), String>),
    StickerPackages(Result<Vec<StickerPackage>, String>),
    EmojiPackages(Result<Vec<rune::EmojiPackage>, String>),
    StickerPic(String, Result<image::DynamicImage, String>),
    /// Preview of a local file in the attach dialog.
    FilePreview(String, Result<image::DynamicImage, String>),
    /// An upload finished: chat, its label, error.
    UploadDone(String, String, Option<String>),
    ServerSettings(Result<serde_json::Value, String>),
    ServerSettingSaved(&'static str, bool, Result<(), String>),
    MyProfile(Result<Profile, String>),
    ProfileChanged,
    /// The new profile cover was uploaded (or not).
    CoverSet(Result<(), String>),
    /// Re-check timed things (typing indicators).
    Tick,
    /// A read receipt request ended.
    Receipt(receipts::Send, receipts::Outcome),
    /// Inline picture and the QR codes in it.
    Thumb(String, Result<(image::DynamicImage, Vec<String>), String>),
    Full(String, Result<Downloaded, String>),
    /// The viewer's animation decoded a new frame for this message.
    AnimFrame(String),
    Profile(String, Result<Box<ProfileLoaded>, String>),
    /// Pending group invitations (added to the chat list); `true` when it
    /// is the complete list.
    Invitations(Vec<Chat>, bool),
    /// An invitation was answered: chat, accepted?, outcome.
    InviteAnswered(String, bool, Result<(), String>),
    /// LINE refused our reaction: message, the reaction we had before, the
    /// one we tried, and why.
    ReactionFailed(String, Option<ReactionKind>, Option<ReactionKind>, String),
    /// A background load failed: it is queued to be tried again.
    Failed(Job, String),
    /// A plugin was compiled (or not): start it.
    PluginLoaded(Box<plugins::Loaded>),
    /// These plugin directories changed on disk (hot reload).
    PluginFiles(Vec<String>),
    /// A Git install or update made progress or ended.
    PluginInstall(plugin_ui::InstallEvent),
}

struct App {
    client: Client,
    me: String,
    tx: mpsc::UnboundedSender<AppEvent>,
    /// Local message store.
    store: Store,
    prefs: Prefs,
    /// `--no-read-receipts` given: never send them this run.
    no_read_receipts_flag: bool,
    /// The user's shortcuts (Settings › Shortcuts), as stored.
    keys: keymap::Keymap,
    /// `--default-keys` given: keys answer with the defaults this run.
    default_keys: bool,
    /// What in the stored shortcuts could not be used (shown in Settings ›
    /// Shortcuts until they are next changed).
    key_problems: Vec<String>,
    /// The file the shortcuts are also kept in (`keys.json`), if any.
    keys_file: Option<shortcuts::KeysFile>,
    /// The last search in a chat: (chat, query), offered again there.
    last_find: Option<(String, String)>,
    /// The open overlay, if any.
    modal: Option<Modal>,
    /// The screen `modal` was opened from (Settings), shown again once it
    /// closes (`settle_modal`).
    modal_parent: Option<Box<Modal>>,
    /// Friends: mid → name (for pickers).
    contacts: HashMap<String, String>,
    /// Group members / pending invitees, per chat.
    members: HashMap<String, Vec<String>>,
    invitees: HashMap<String, Vec<String>>,
    /// Per chat: reader → newest message id they have read.
    read_marks: HashMap<String, HashMap<String, String>>,
    /// Per chat: who is typing, until when.
    typing: HashMap<String, (String, std::time::Instant)>,
    /// Uploads in progress: chat, label.
    uploads: Vec<(String, String)>,
    /// The sticker and emoji pickers' pictures (see `STICKER_CACHE_BYTES`).
    sticker_pics: PicCache<Picture>,
    /// Owned LINE emoji packages, as last fetched (cached in the store).
    emoji_packs: Vec<rune::EmojiPackage>,
    /// The file browser's previews (see `PREVIEW_CACHE_BYTES`).
    file_previews: PicCache<Picture>,
    server_settings: Option<Load<serde_json::Value>>,
    /// Our name and status message, for the settings page.
    my_profile: Option<Load<(String, String)>>,
    /// Chats whose newest message is not in the store (fetch on open).
    stale: HashSet<String>,
    /// Chats that may have missed messages after a `fullSync` (fetch on open).
    resync: HashSet<String>,
    older_loading: HashSet<String>,
    /// Chats whose whole history is loaded.
    no_older: HashSet<String>,
    /// The conversation is scrolled to its oldest loaded row.
    at_top: bool,
    /// Boundary between read and unread messages in one chat.
    unread_divider: Option<(String, Unread)>,
    /// Scroll the divider into view after the unread chat is opened.
    jump_unread: bool,
    /// The terminal has focus (for notifications and read receipts: what
    /// arrives while you are away is marked read when you come back).
    focused: bool,
    chats: Vec<ChatEntry>,
    /// The chat list is filtering its entries.
    searching: bool,
    /// Name or participant query shown in the chat-list search field.
    chat_query: TextInput,
    /// Chat mids pinned in the active pin list, in display order.
    pinned: Vec<String>,
    /// The active pin list's name, when the account has more than one
    /// (shown on the pinned divider).
    pin_list: Option<String>,
    list: ListState,
    /// Paging through older chats (selecting the last chat loads more).
    more_chats: MoreChats,
    names: HashMap<String, String>,
    /// When a name lookup was last started, per mid (retried after `NAME_RETRY`).
    name_requests: HashMap<String, std::time::Instant>,
    /// Mids drawn without a known name (or, for avatars, not looked up this
    /// run) since the last lookup round.
    name_misses: std::cell::RefCell<HashSet<String>>,
    /// Mids whose name and picture were looked up this run: what the store
    /// remembers of the others may be stale (or missing), so drawing them
    /// asks once.
    looked_up: HashSet<String>,
    /// Picture paths learned with names, per mid.
    picture_paths: HashMap<String, String>,
    /// Avatars beside names, per mid.
    avatars: HashMap<String, Load<Avatar>>,
    /// Pictures (thumbnails, avatars) waiting to download, on-screen first.
    media: FetchQueue<MediaKey, MediaStart>,
    messages: HashMap<String, Vec<UiMsg>>,
    loading: HashSet<String>,
    open: Option<String>,
    focus: Focus,
    input: TextInput,
    /// Lines scrolled up from the bottom of the conversation.
    scroll: usize,
    status: String,
    /// The status message as last drawn, and since when (see `expire_status`).
    status_since: Option<(String, Instant)>,
    quit: bool,
    /// Selected message (index into the open chat's messages).
    sel: Option<usize>,
    /// Inline pictures, per message id (see `THUMB_CACHE_BYTES`).
    thumbs: PicCache<Picture>,
    full: HashMap<String, Load<FullMedia>>,
    /// Downloaded originals in `full`, least recently used first (see `FULL_CACHE_BYTES`).
    full_lru: Vec<String>,
    /// Actions waiting for a full download, per message id.
    pending: HashMap<String, After>,
    /// Message id shown in the media viewer.
    viewer: Option<String>,
    /// The animation playing in the viewer.
    anim: Option<anim::Player>,
    /// Profile pages, newest on top (back pops one).
    profile: Vec<String>,
    /// Selected member on the open profile page (`None`: the buttons).
    profile_sel: Option<usize>,
    /// Highlighted button on the open profile page.
    profile_btn: usize,
    profiles: HashMap<String, Load<ProfileData>>,
    /// QR codes found in pictures, per message id.
    qr: HashMap<String, Vec<String>>,
    /// How pictures are drawn with text when there is no image protocol.
    cell_mode: CellMode,
    /// The terminal's image protocol (full resolution), if it has a usable one.
    picker: Option<Picker>,
    /// Ask the terminal loop to fully redraw (clears leftover graphics).
    needs_clear: bool,
    /// Index into `THUMB_SIZES`.
    thumb_size: usize,
    /// Scroll the selected message into view on the next draw.
    follow_sel: bool,
    /// Where things were drawn last frame, for mouse clicks.
    hit: HitAreas,
    /// Message id the next text message replies to.
    replying: Option<String>,
    /// The message lines of chats other than the open one: switching chats
    /// puts the line away with its chat, so a draft never goes to another.
    drafts: HashMap<String, Draft>,
    /// The key being handled came in a burst of keys: a paste typed out by a
    /// terminal without bracketed paste. Its Enters are line breaks, not sends.
    pasting: bool,
    /// Loads that failed, waiting to be tried again.
    retry: RetryQueue,
    /// Read receipts not acknowledged yet (see `receipts.rs`); mirrored in
    /// the store's `pending_receipts`.
    receipts: Receipts,
    /// Read receipt requests in flight, by ticket (aborted when receipts
    /// are turned off).
    receipt_tasks: HashMap<u64, tokio::task::AbortHandle>,
    /// Requests started, instead of sending them (tests).
    #[cfg(test)]
    sent_receipts: Vec<receipts::Send>,
    /// The last write to the local store that failed, for the status line.
    store_failure: std::cell::RefCell<Option<String>>,
    /// Installed plugins (none until `start_plugins`; see `plugins/`).
    plugins: plugins::Plugins,
    /// The Git install or update running (one at a time), as Settings shows it.
    plugin_job: Option<String>,
    /// Newer releases that Git-installed plugins do not follow (pinned or
    /// out of range), by plugin id, from the last check.
    plugin_available: HashMap<String, String>,
}

/// An unsent message line, kept per chat.
struct Draft {
    input: TextInput,
    replying: Option<String>,
}

/// Store key prefix of draft texts (a `cache.` key: wiped with the chats).
const DRAFT_KEY: &str = "cache.draft.";

/// The composer grows with its text up to this many rows (and a third of the screen).
const INPUT_MAX_ROWS: u16 = 10;

/// Screen areas from the last frame.
#[derive(Default)]
struct HitAreas {
    /// Inside the chat list's border.
    chats: Rect,
    /// The chat list's "unread above / below" indicators (click: jump there).
    unread_above: Option<Rect>,
    unread_below: Option<Rect>,
    /// Inside the conversation's border.
    conv: Rect,
    input: Rect,
    viewer: Option<Rect>,
    /// The conversation's title row (click: chat profile).
    conv_title: Rect,
    /// Visible message headers: row and sender (click: their profile).
    heads: Vec<(u16, String)>,
    profile: Option<Rect>,
    profile_buttons: Vec<(Rect, ProfileAction)>,
    /// Visible member rows on a group profile.
    profile_rows: Vec<(u16, usize)>,
    /// Links on screen (conversation or viewer) and where they are.
    links: Vec<(Rect, String)>,
    /// Visible messages: rows `y0..y1` belong to message index `i`.
    msgs: Vec<(u16, u16, usize)>,
    /// Visible reaction lines and the message whose reactions they show.
    reactions: Vec<(u16, String)>,
    /// The open overlay's box.
    modal: Option<Rect>,
    /// Overlay rows: screen row and item index.
    modal_rows: Vec<(u16, usize)>,
    /// A confirmation's buttons: area and whether it is the "yes" one.
    confirm_buttons: Vec<(Rect, bool)>,
}

/// One row of the conversation pane.
enum Row {
    Text(Line<'static>),
    Reaction {
        id: String,
        line: Line<'static>,
    },
    /// Top row of an inline thumbnail `h` rows high.
    Image {
        id: String,
        w: u16,
        h: u16,
        right: bool,
    },
    /// Rows covered by a thumbnail.
    Pad,
}

// -- helpers ---------------------------------------------------------------------------

/// Events older than this when they arrive (replayed after a restart) do
/// not notify.
const NOTIFY_MAX_AGE_MS: i64 = 120_000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn hhmm(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) if t.date_naive() == chrono::Local::now().date_naive() => t.format("%H:%M").to_string(),
        Some(t) => t.format("%d/%m %H:%M").to_string(),
        None => String::new(),
    }
}

/// Rows per chat-list entry: name, preview, spacer.
const CHAT_ROW_H: u16 = 3;
/// Column (inside the list) where entry text starts: accent bar + 4-wide avatar.
const CHAT_TEXT_X: u16 = 5;

/// Compact timestamp for the chat list: time today, "Yesterday", weekday this week, else a date.
fn chat_time(ms: i64) -> String {
    use chrono::TimeZone;
    if ms <= 0 {
        return String::new();
    }
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let days = (chrono::Local::now().date_naive() - t.date_naive()).num_days();
    match days {
        ..=0 => t.format("%H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => t.format("%a").to_string(),
        _ => t.format("%d/%m/%y").to_string(),
    }
}

/// Cut `s` to at most `max` columns, ending with `…` when shortened.
/// Where `word` (lowercase) occurs in `text`, ignoring case: byte ranges of
/// `text`, each with whether it starts a word there.
fn find_word(text: &str, word: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let needle: Vec<char> = word.chars().collect();
    // Lowercased characters, each with the byte range of the character it came from.
    let lower: Vec<(char, usize, usize)> = text
        .char_indices()
        .flat_map(|(at, c)| c.to_lowercase().map(move |l| (l, at, at + c.len_utf8())))
        .collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while !needle.is_empty() && i + needle.len() <= lower.len() {
        let char_start = i == 0 || lower[i - 1].1 != lower[i].1;
        if char_start
            && lower[i..i + needle.len()]
                .iter()
                .map(|l| l.0)
                .eq(needle.iter().copied())
        {
            let word_start = i == 0 || !lower[i - 1].0.is_alphanumeric();
            hits.push((lower[i].1..lower[i + needle.len() - 1].2, word_start));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// How well `name` matches every query word: 0 when each starts a word of
/// it, 1 when each is somewhere in it, `None` when one is missing.
fn name_rank(name: &str, words: &[String]) -> Option<u8> {
    let mut rank = 0;
    for w in words {
        let hits = find_word(name, w);
        if hits.is_empty() {
            return None;
        }
        if !hits.iter().any(|(_, word_start)| *word_start) {
            rank = 1;
        }
    }
    Some(rank)
}

/// The accent bar at the left of a chat-list row.
fn row_bar(selected: bool, focused: bool) -> Span<'static> {
    match (selected, focused) {
        (true, true) => Span::styled("▌", Style::new().fg(theme::t().accent)),
        (true, false) => Span::styled("▌", Style::new().fg(theme::t().muted)),
        _ => Span::raw(" "),
    }
}

/// A chat-list row's avatar tile (top and bottom line): blank when the
/// picture is loaded (it is drawn over it), else a coloured monogram.
fn row_tile(mid: &str, name: &str, picture: bool) -> (Span<'static>, Span<'static>) {
    if picture {
        return (Span::raw("    "), Span::raw("    "));
    }
    let tile = Style::new().fg(Color::White).bg(monogram_color(mid)).bold();
    let initial: String = name
        .chars()
        .find(|ch| ch.width().unwrap_or(0) > 0)
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    let iw = initial.width().min(4);
    let left = (4 - iw) / 2;
    let top = format!("{}{initial}{}", " ".repeat(left), " ".repeat(4 - iw - left));
    (Span::styled(top, tile), Span::styled("    ", tile))
}

/// `text` as spans in `base`, with what the query words match highlighted.
fn highlighted(text: &str, words: &[String], base: Style) -> Vec<Span<'static>> {
    let mut marks: Vec<std::ops::Range<usize>> = words
        .iter()
        // The text went through `ellipsize`, so the words must be spelled the same way.
        .flat_map(|w| find_word(text, &cell_safe(w)))
        .map(|(r, _)| r)
        .collect();
    marks.sort_by_key(|r| r.start);
    let mark = base.fg(theme::t().warn).underlined();
    let mut spans = Vec::new();
    let mut at = 0;
    for r in marks {
        let start = r.start.max(at);
        if start >= r.end {
            continue;
        }
        if start > at {
            spans.push(Span::styled(text[at..start].to_owned(), base));
        }
        spans.push(Span::styled(text[start..r.end].to_owned(), mark));
        at = r.end;
    }
    if at < text.len() || spans.is_empty() {
        spans.push(Span::styled(text[at..].to_owned(), base));
    }
    spans
}

/// `s` with every grapheme at most one cell wide unless it is a wide character.
/// Thai SARA AM (and the Lao AM) is a spacing mark: it joins the preceding consonant
/// into one grapheme that ratatui keeps in a single two-column cell, while terminals
/// draw it as two characters (or, grapheme-aware ones, one column). Repainting that
/// cell then leaves a lone `ำ` behind, or shifts the rest of the row. Its compatibility
/// decomposition (NIKHAHIT + SARA AA) looks the same and splits into a zero-width mark
/// on the consonant plus a one-column vowel, which every terminal agrees on.
fn cell_safe(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains(['\u{0E33}', '\u{0EB3}']) {
        return s.into();
    }
    s.chars()
        .fold(String::with_capacity(s.len() + 8), |mut out, ch| {
            match ch {
                '\u{0E33}' => out.push_str("\u{0E4D}\u{0E32}"),
                '\u{0EB3}' => out.push_str("\u{0ECD}\u{0EB2}"),
                _ => out.push(ch),
            }
            out
        })
        .into()
}

/// Where the last letter of `s` (with any zero-width marks after it) starts.
pub fn letter_start(s: &str) -> Option<usize> {
    s.char_indices()
        .rev()
        .find(|&(_, c)| c.width().unwrap_or(0) > 0)
        .map(|(i, _)| i)
}

/// Whether `c` is a SARA AM (Thai or Lao), which belongs to the letter before it:
/// text is never wrapped between the two.
pub fn joins_previous(c: char) -> bool {
    matches!(c, '\u{0E33}' | '\u{0EB3}')
}

/// Lay every cell holding a SARA AM out as [`cell_safe`] text, one column per
/// cell, whatever drew it (messages, the message line, menus, plugin views).
/// ratatui keeps `กำ` in one two-column cell, which WezTerm and other
/// grapheme-aware terminals draw one column wide: the rest of the row then
/// shifts left and its vowels and tone marks land on the wrong letters.
fn split_spacing_marks(buf: &mut ratatui::buffer::Buffer) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let sym = buf[(x, y)].symbol();
            if sym.len() < 3 || !sym.contains(['\u{0E33}', '\u{0EB3}']) {
                continue;
            }
            let safe = cell_safe(sym).into_owned();
            // One part per column: a letter with its zero-width marks.
            let mut parts: Vec<(String, usize)> = Vec::new();
            for ch in safe.chars() {
                let w = ch.width().unwrap_or(0);
                match parts.last_mut() {
                    Some((p, pw)) if w == 0 || *pw == 0 => {
                        p.push(ch);
                        *pw += w;
                    }
                    _ => parts.push((ch.to_string(), w)),
                }
            }
            // Only into the columns the cell already covers.
            let fits = parts.iter().all(|&(_, w)| w == 1)
                && parts.len() == sym.width()
                && x + parts.len() as u16 <= area.right();
            if !fits {
                buf[(x, y)].set_symbol(&safe);
                continue;
            }
            let cell = buf[(x, y)].clone();
            for (i, (part, _)) in parts.into_iter().enumerate() {
                let mut c = cell.clone();
                c.set_symbol(&part);
                buf[(x + i as u16, y)] = c;
            }
        }
    }
}

/// `s` cut to `max` columns with a trailing `…`, made safe to draw (`cell_safe`).
fn ellipsize(s: &str, max: usize) -> String {
    let s = cell_safe(s);
    if s.width() <= max {
        return s.into_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// Stable background colour for a chat's monogram tile.
fn monogram_color(mid: &str) -> Color {
    let tiles = &theme::t().tiles;
    let h = mid
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
    tiles[h as usize % tiles.len()]
}

/// What to show for a message.
fn describe(m: &Message, e2ee_error: Option<&str>) -> Option<String> {
    if let Some(e) = e2ee_error {
        return Some(format!("🔒 [cannot decrypt: {e}]"));
    }
    let meta = &m.content_metadata;
    let file = || meta.get("FILE_NAME").cloned().unwrap_or_default();
    Some(match m.content_type {
        ct::TEXT => {
            if m.is_e2ee() {
                "🔒 [sealed]".into()
            } else {
                m.text.clone().unwrap_or_default()
            }
        }
        ct::IMAGE => "🖼  [image]".into(),
        ct::VIDEO => format!(
            "🎞  [video {}s]",
            meta.get("DURATION")
                .and_then(|d| d.parse::<u64>().ok())
                .unwrap_or(0)
                / 1000
        ),
        ct::AUDIO => "🎤 [audio]".into(),
        ct::CALL => call_line(m),
        ct::STICKER => match meta.get("STKTXT").map(String::as_str) {
            Some(t) if !t.is_empty() && t != "[Sticker]" => format!("[sticker {t}]"),
            _ => "[sticker]".into(),
        },
        ct::CONTACT => format!(
            "👤 [contact {}]",
            meta.get("displayName").cloned().unwrap_or_default()
        ),
        ct::FILE => format!("📎 [file {}]", file()),
        ct::LOCATION => {
            let title = m
                .extra
                .get("location")
                .and_then(|l| l.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            format!("📍 [location {title}]")
        }
        ct::FLEX => format!(
            "[{}]",
            meta.get("ALT_TEXT")
                .cloned()
                .unwrap_or_else(|| "flex message".into())
        ),
        rune::album::POST_NOTIFICATION => match m.album_post() {
            Some(p) => {
                let what = match p.kind {
                    rune::AlbumPostKind::Created => "created the album",
                    _ => "added to the album",
                };
                let more = match p.media.len() {
                    0 | 1 => String::new(),
                    n => format!(" (+{} more)", n - 1),
                };
                format!("🖼  {what} «{}»{more}", p.album_name)
            }
            None => "[timeline post]".into(),
        },
        // Chat events (joins, settings) are not shown, except album changes.
        rune::album::CHAT_EVENT => format!("🖼  {}", m.album_event()?.describe()),
        other => format!("[message type {other}]"),
    })
}

/// "📞 Voice call · 3:12", "📹 Group video call started", …
fn call_line(m: &Message) -> String {
    use rune::{CallMedia, CallOutcome};
    let Some(call) = m.call() else {
        return String::new();
    };
    let icon = match call.media {
        CallMedia::Voice => "📞",
        CallMedia::Video => "📹",
        CallMedia::Live => "📺",
    };
    let missed = matches!(
        call.outcome,
        CallOutcome::Unanswered | CallOutcome::Declined | CallOutcome::Busy
    );
    format!("{icon} {call}{}", if missed { " ✗" } else { "" })
}

/// What an unsent message turns into.
const UNSENT_TEXT: &str = "🚫 message unsent";

fn to_ui(m: &Message, e2ee_error: Option<&str>) -> Option<UiMsg> {
    let text = describe(m, e2ee_error)?;
    let has_media = matches!(
        media_type(m),
        ct::IMAGE | ct::VIDEO | ct::AUDIO | ct::FILE | ct::STICKER
    );
    Some(UiMsg {
        id: m.id.clone(),
        from: m.from.clone(),
        time: m.created_time.unwrap_or_else(now_ms),
        text,
        raw: has_media.then(|| m.clone()),
        reply_to: m.reply_to().map(str::to_owned),
        forwarded: m.is_forwarded(),
        reactions: m.reactions(),
        unsent: false,
        delivered: m.delivered_time().or(m.created_time).unwrap_or(0),
        plain: m.content_type == ct::TEXT && !m.is_e2ee() && e2ee_error.is_none(),
        written: matches!(
            m.content_type,
            ct::TEXT
                | ct::IMAGE
                | ct::VIDEO
                | ct::AUDIO
                | ct::STICKER
                | ct::CONTACT
                | ct::FILE
                | ct::LOCATION
        ),
    })
}

fn stored_to_ui(sm: &rune::store::StoredMessage) -> Option<UiMsg> {
    let mut ui = to_ui(&sm.message, sm.e2ee_error.as_deref())?;
    if sm.unsent {
        ui.unsent = true;
        ui.text = UNSENT_TEXT.into();
    }
    Some(ui)
}

/// Chat-list summary of a message.
fn preview_of(m: &Message) -> String {
    describe(m, None).unwrap_or_default()
}

/// Complete LINE's copy of a message we sent (which may still be sealed)
/// from what we sent, so it can be shown and stored in plain form.
fn fill_from_outgoing(m: &mut Message, out: &OutgoingMessage, me: &str) {
    if m.from.is_empty() {
        m.from = me.to_owned();
    }
    if m.to.is_empty() {
        m.to = out.to.clone();
    }
    m.to_type = MidType::of(&out.to) as i64;
    m.content_type = out.content_type;
    let empty = m.text.as_deref().is_none_or(str::is_empty);
    if m.is_e2ee() || empty {
        m.text = out.text.clone();
    }
    m.chunks = None;
    for (k, v) in &out.content_metadata {
        m.content_metadata.entry(k.clone()).or_insert_with(|| v.clone());
    }
    for (k, v) in &out.extra {
        if k != "hasContent" {
            m.extra.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if m.created_time.is_none() {
        m.created_time = Some(now_ms());
    }
}

/// The emoji drawn for a reaction: Unicode where there is one, ✨ for other LINE emoji.
fn reaction_glyph(kind: &ReactionKind) -> &'static str {
    kind.emoji().unwrap_or(match kind {
        ReactionKind::Predefined(_) => "•",
        ReactionKind::Paid { .. } => "✨",
    })
}

/// Wrap text to `width` display columns (handles Thai, CJK, emoji).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut w = 0;
        for ch in para.chars() {
            let cw = ch.width().unwrap_or(0);
            if w + cw > width && !line.is_empty() {
                if ch == ' ' {
                    // Break at this space and drop it.
                    out.push(std::mem::take(&mut line));
                    w = 0;
                    continue;
                }
                // Break after the last space, if any; long words split anywhere
                // but between a letter and its SARA AM.
                match line.rfind(' ') {
                    Some(i) if i > 0 => {
                        let rest = line.split_off(i + 1);
                        line.pop();
                        out.push(std::mem::replace(&mut line, rest));
                    }
                    _ => match letter_start(&line).filter(|&i| i > 0 && joins_previous(ch)) {
                        Some(i) => {
                            let rest = line.split_off(i);
                            out.push(std::mem::replace(&mut line, rest));
                        }
                        None => out.push(std::mem::take(&mut line)),
                    },
                }
                w = line.width();
            }
            line.push(ch);
            w += cw;
        }
        out.push(line);
    }
    out
}

/// A link drawn on one bubble row: column offset from the row's start, width, URL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkSpan {
    off: u16,
    w: u16,
    url: String,
}

/// Whether a bubble row is one of ours (right-aligned).
fn mine_of(line: &Line) -> bool {
    line.alignment == Some(ratatui::layout::Alignment::Right)
}

/// One drawn bubble row and the links on it.
type BubbleRow = (Line<'static>, Vec<LinkSpan>);

/// A message bubble: `text` wrapped to at most `max_w` columns (borders
/// included) inside a rounded box, with a one-column margin on the outer side.
/// Web links are underlined and reported per row so clicks can open them.
fn bubble(text: &str, max_w: usize, right: bool, border: Style, body: Style) -> Vec<BubbleRow> {
    // Which link (if any) each character of `text` belongs to.
    let ranges = links::find_links(text);
    let flags: Vec<(char, Option<usize>)> = text
        .char_indices()
        .map(|(i, c)| (c, ranges.iter().position(|r| r.contains(&i))))
        .collect();
    let link_style = body.fg(theme::t().link).underlined();
    let lines = wrap(text, max_w.saturating_sub(4).max(1));
    let inner = lines.iter().map(|l| l.width()).max().unwrap_or(0);
    let edge = |l: &str, r: &str| Span::styled(format!("{l}{}{r}", "─".repeat(inner + 2)), border);
    let place = |mut spans: Vec<Span<'static>>| {
        if right {
            spans.push(Span::raw(" "));
            Line::from(spans).right_aligned()
        } else {
            spans.insert(0, Span::raw(" "));
            Line::from(spans)
        }
    };
    // Content starts after the margin (left bubbles) and "│ ".
    let base: u16 = if right { 2 } else { 3 };
    let mut out = vec![(place(vec![edge("╭", "╮")]), Vec::new())];
    let mut p = 0;
    for l in lines {
        // Walk the original text alongside the wrapped line (wrapping only
        // drops spaces and newlines) to recover each character's link.
        let mut runs: Vec<(String, Option<usize>)> = Vec::new();
        for c in l.chars() {
            while p < flags.len() && flags[p].0 != c {
                p += 1;
            }
            let link = flags.get(p).and_then(|f| f.1);
            p += 1;
            match runs.last_mut() {
                Some((s, k)) if *k == link => s.push(c),
                _ => runs.push((c.to_string(), link)),
            }
        }
        let mut spans = vec![Span::styled("│", border), Span::styled(" ", body)];
        let mut found = Vec::new();
        let mut col = base;
        for (run, link) in runs {
            let w = run.width() as u16;
            match link {
                Some(k) => {
                    found.push(LinkSpan {
                        off: col,
                        w,
                        url: text[ranges[k].clone()].to_owned(),
                    });
                    spans.push(Span::styled(run, link_style));
                }
                None => spans.push(Span::styled(run, body)),
            }
            col += w;
        }
        spans.push(Span::styled(format!("{} ", " ".repeat(inner - l.width())), body));
        spans.push(Span::styled("│", border));
        out.push((place(spans), found));
    }
    out.push((place(vec![edge("╰", "╯")]), Vec::new()));
    out
}

/// Guess a file extension from the first bytes.
fn sniff_ext(b: &[u8]) -> &'static str {
    match b {
        [0xff, 0xd8, ..] => "jpg",
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'%', b'P', b'D', b'F', ..] => "pdf",
        [_, _, _, _, b'f', b't', b'y', b'p', b'M', b'4', b'A', ..] => "m4a",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "mp4",
        _ => "bin",
    }
}

fn download_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads").join("rune")
}

/// Write `data` to the download directory as `name` (adds " (n)" on clashes)
/// and return the path. If a file there (under `name` or a numbered variant)
/// already holds exactly `data`, reuse it, so saving or opening the same
/// picture again doesn't pile up copies.
fn store_download(name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    store_download_in(&download_dir(), name, data)
}

/// Longest file name [`safe_file_name`] returns, in bytes: file systems cap
/// names at 255, and a clash adds " (n)".
const MAX_FILE_NAME: usize = 200;

/// A sender-chosen file name made safe to create in a directory: no path
/// separators or control characters (which would also garble the status
/// line), no leading dots (hidden files, `..`), at most [`MAX_FILE_NAME`]
/// bytes with the extension kept.
fn safe_file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim_start_matches('.').trim_end();
    if safe.trim_matches('.').is_empty() {
        return "file".to_owned();
    }
    if safe.len() <= MAX_FILE_NAME {
        return safe.to_owned();
    }
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 16 => (s, &safe[s.len()..]),
        _ => (safe, ""),
    };
    let mut cut = MAX_FILE_NAME - ext.len();
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", &stem[..cut])
}

/// Create `p` and write `data` to it, failing with `AlreadyExists` instead of
/// replacing (or writing through a symlink at) anything already there. A
/// failed write leaves no partial file behind.
fn write_new(p: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(p)?;
    f.write_all(data).and_then(|()| f.sync_all()).inspect_err(|_| {
        let _ = std::fs::remove_file(p);
    })
}

fn store_download_in(dir: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    // Never let a server-supplied name escape the directory.
    let safe = safe_file_name(name);
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (safe.clone(), String::new()),
    };
    let mut p = dir.join(&safe);
    let mut n = 1;
    loop {
        match std::fs::symlink_metadata(&p) {
            // Cheap length check before reading the file back.
            Ok(meta)
                if meta.is_file()
                    && meta.len() == data.len() as u64
                    && std::fs::read(&p).is_ok_and(|b| b == data) =>
            {
                return Ok(p);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match write_new(&p, data) {
                Ok(()) => return Ok(p),
                // Another save took the name first: look at what it wrote.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
        p = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
}

/// Whether the system opener might run a file of this name as a program
/// (or install or launch something) rather than show it: launchers,
/// scripts, executables, installers and packages. The sender picks the
/// name, so opening one is asked first. Judged on the name as it is
/// written to disk ([`safe_file_name`]).
fn runs_programs(name: &str) -> bool {
    let safe = safe_file_name(name);
    let ext = safe
        .trim_end_matches('.')
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        // Linux launchers, scripts and packages
        "desktop" | "sh" | "bash" | "zsh" | "csh" | "ksh" | "fish" | "run" | "bin" | "appimage"
            | "flatpakref" | "flatpakrepo" | "snap" | "deb" | "rpm" | "so"
            // interpreters a desktop may associate with their files
            | "jar" | "jnlp" | "py" | "pyw" | "pl" | "rb" | "php" | "js" | "mjs" | "lua" | "tcl"
            // macOS
            | "app" | "command" | "tool" | "pkg" | "mpkg" | "dmg" | "scpt" | "applescript"
            | "workflow" | "terminal" | "webloc" | "dylib"
            // Windows (run by Wine where it is installed)
            | "exe" | "msi" | "com" | "bat" | "cmd" | "scr" | "pif" | "lnk" | "ps1" | "vbs"
            | "vbe" | "wsf" | "hta" | "reg" | "cpl" | "dll" | "inf" | "url"
            // browser extensions
            | "xpi" | "crx"
    )
}

/// Video or audio, judged by the file extension. (GIFs and other
/// animations play in the viewer.)
fn is_motion_file(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "mp4"
            | "m4v"
            | "mov"
            | "webm"
            | "mkv"
            | "avi"
            | "3gp"
            | "m4a"
            | "mp3"
            | "aac"
            | "ogg"
            | "oga"
            | "opus"
            | "wav"
            | "flac"
    )
}

/// The mpv command line for `path`: its own window (also for audio, so it
/// can be closed), never touching rune's terminal.
fn mpv_args(path: &std::path::Path) -> Vec<std::ffi::OsString> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    vec![
        "--force-window=immediate".into(),
        "--no-terminal".into(),
        format!("--title=rune · {name}").into(),
        "--".into(),
        path.as_os_str().to_owned(),
    ]
}

/// Play a downloaded file in mpv. Without mpv, fall back to the system app.
/// Returns the status line; a failed playback is reported later.
fn play_media(tx: &mpsc::UnboundedSender<AppEvent>, path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let spawned = std::process::Command::new("mpv")
        .args(mpv_args(path))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            let (tx, n) = (tx.clone(), name.clone());
            std::thread::spawn(move || {
                if let Ok(st) = child.wait()
                    && !st.success()
                {
                    let code = st.code().map_or("a signal".into(), |c| format!("code {c}"));
                    let _ = tx.send(AppEvent::Status(format!("mpv could not play {n} (exit {code})")));
                }
            });
            format!("playing {name} in mpv")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match open_external(path) {
            Ok(()) => format!("mpv is not installed — opened {name} with the system app"),
            Err(e) => format!("mpv is not installed and the system app failed: {e}"),
        },
        Err(e) => format!("could not start mpv: {e}"),
    }
}

fn open_external(path: &std::path::Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    spawn_reaped(std::process::Command::new(opener).arg(path))
}

/// Start a program that goes its own way (an opener, a notifier), detached
/// from the terminal, and wait for it on a thread so that it does not stay
/// behind as a zombie process once it exits.
pub fn spawn_reaped(cmd: &mut std::process::Command) -> std::io::Result<()> {
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let reaper = std::thread::Builder::new()
        .name("reap-child".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(e) = reaper {
        tracing::warn!("cannot wait for a started program: {e}");
    }
    Ok(())
}

/// What a queued picture download needs to start (see `fetchq`).
enum MediaStart {
    Thumb(Box<Message>),
    /// The picture path of an avatar.
    Avatar(String),
}

/// How many rows row `r` is from the window of `height` rows at `start` (0 inside it).
fn rows_away(r: usize, start: usize, height: usize) -> fetchq::Distance {
    let end = start + height;
    let d = if r < start {
        start - r
    } else if r >= end {
        r + 1 - end
    } else {
        return fetchq::VISIBLE;
    };
    d.try_into().unwrap_or(fetchq::Distance::MAX)
}

/// Fetch the picture shown inline for a message. LINE's previews are tiny
/// (≤384 px, JPEG quality 0.3), so images use the main picture (≤1280 px)
/// and fall back to the preview; videos only have the preview frame.
/// The picture is shrunk to `cap` pixels (what the largest thumbnail can
/// show) once QR codes have been looked for at full size.
fn spawn_thumb(client: Client, tx: mpsc::UnboundedSender<AppEvent>, m: Message, cap: (u32, u32)) {
    tokio::spawn(async move {
        let bytes = match media_type(&m) {
            ct::STICKER => {
                let id = m.content_metadata.get("STKID").cloned().unwrap_or_default();
                client.download_sticker(&id, StickerImage::Static2x).await
            }
            ct::IMAGE => match client.download_media(&m, false).await {
                Ok(b) => Ok(b),
                Err(_) => client.download_media(&m, true).await,
            },
            _ => client.download_media(&m, true).await,
        };
        let res = match bytes {
            Ok(b) => match decode_picture(b).await {
                Ok(img) => {
                    let (img, qr) = if media_type(&m) == ct::IMAGE {
                        scan_qr(img).await
                    } else {
                        (img, Vec::new())
                    };
                    Ok((shrink(img, cap).await, qr))
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(AppEvent::Thumb(m.id.clone(), res));
    });
}

/// True when LINE no longer has the object (deleted original, expired file).
fn is_gone(e: &rune::Error) -> bool {
    matches!(e, rune::Error::MediaUnavailable(_)) || e.api().is_some_and(|a| matches!(a.status, 404 | 410))
}

async fn decode(bytes: Vec<u8>) -> Option<image::DynamicImage> {
    decode_picture(bytes).await.ok()
}

/// Fetch the original (and a picture to show for it). If the original of an
/// old picture/video is gone, fall back to its preview.
fn spawn_full(client: Client, tx: mpsc::UnboundedSender<AppEvent>, m: Message) {
    tokio::spawn(async move {
        let res = async {
            let mut note = None;
            let data = if m.content_type == ct::STICKER {
                let id = m.content_metadata.get("STKID").cloned().unwrap_or_default();
                // Animated stickers come as an APNG, played in the viewer.
                let anim = if m.is_animated_sticker() {
                    client.download_sticker(&id, StickerImage::Animation).await.ok()
                } else {
                    None
                };
                match anim {
                    Some(a) => Ok(a),
                    None => client.download_sticker(&id, StickerImage::Static2x).await,
                }
            } else {
                match client.download_media(&m, false).await {
                    Err(e) if is_gone(&e) && matches!(m.content_type, ct::IMAGE | ct::VIDEO) => {
                        let p = client
                            .download_media(&m, true)
                            .await
                            .map_err(|pe| format!("{e} (the preview is gone too: {pe})"))?;
                        note = Some(
                            "the original is no longer on LINE's server — showing the preview".to_owned(),
                        );
                        Ok(p)
                    }
                    r => r,
                }
            }
            .map_err(|e| e.to_string())?;
            // The decrypted message carries FILE_NAME for sealed media.
            let meta = client
                .decrypt_message(&m)
                .await
                .map(|d| d.content_metadata)
                .unwrap_or_default();
            let ext = sniff_ext(&data);
            let album_oid = m
                .album_post()
                .and_then(|p| p.media.first().map(|x| x.oid.clone()));
            let mut file_name = meta
                .get("FILE_NAME")
                .filter(|n| !n.is_empty())
                .cloned()
                .or_else(|| album_oid.map(|oid| format!("album-{oid}.{ext}")))
                .unwrap_or_else(|| format!("line-{}.{ext}", m.id));
            if note.is_some() {
                let stem = file_name
                    .rsplit_once('.')
                    .map_or(file_name.as_str(), |(s, _)| s)
                    .to_owned();
                file_name = format!("{stem}-preview.{ext}");
            }
            let kind = media_type(&m);
            let picture = match kind {
                ct::IMAGE | ct::STICKER => decode(data.clone()).await,
                // Picture files (a GIF sent as a file) show (and animate) too.
                ct::FILE if matches!(ext, "jpg" | "png" | "gif" | "webp") => decode(data.clone()).await,
                // Videos show their preview frame (`data` itself when the original is gone).
                ct::VIDEO if note.is_some() => decode(data.clone()).await,
                // Album videos have no preview frame.
                ct::VIDEO if kind != m.content_type => None,
                ct::VIDEO => match client.download_media(&m, true).await {
                    Ok(p) => decode(p).await,
                    Err(_) => None,
                },
                _ => None,
            };
            let (picture, qr) = match picture {
                Some(img) if kind == ct::IMAGE => {
                    let (img, qr) = scan_qr(img).await;
                    (Some(img), qr)
                }
                p => (p, Vec::new()),
            };
            Ok::<_, String>(Downloaded {
                data,
                file_name,
                picture,
                note,
                qr,
            })
        }
        .await;
        let _ = tx.send(AppEvent::Full(m.id.clone(), res));
    });
}

/// The character a key types, if it types one (no Ctrl/Alt chords).
/// A status saying something went wrong (shown in red).
/// The status line's two parts within `width` columns: the status message (padded,
/// `None` when there is none) and the key hint. The hint's first entry (how to get
/// help) keeps its room, so a long message is shortened before it; the other
/// entries take what is left, whole or not at all (`keymap::fit_hint`).
fn status_line(status: &str, hint: &str, width: usize) -> (Option<String>, String) {
    if status.is_empty() {
        return (None, keymap::fit_hint(hint, width));
    }
    let help_w = hint.split(keymap::HINT_SEP).next().unwrap_or("").width();
    // The message's padding and the gap after it take 3 columns.
    let message_room = width
        .saturating_sub(help_w + 3)
        .max(width.min(12).saturating_sub(3));
    let message = format!(" {} ", ellipsize(status, message_room));
    let left = width.saturating_sub(message.width() + 1);
    (Some(message), keymap::fit_hint(hint, left))
}

/// A frame's bottom title for `hint`, fitted between the corners of `frame`.
fn frame_hint(hint: &str, frame: Rect) -> Span<'static> {
    Span::styled(
        keymap::fit_hint(hint, frame.width.saturating_sub(2) as usize),
        theme::muted(),
    )
}

fn status_is_error(status: &str) -> bool {
    let s = status.to_lowercase();
    [
        "could not",
        "couldn't",
        "cannot",
        "can't",
        "failed",
        "error",
        "no such",
    ]
    .iter()
    .any(|w| s.contains(w))
}

/// Whether `word` looks like a `/command` (`/` and a name), not text such as a path.
fn is_command_word(word: &str) -> bool {
    word.strip_prefix('/').is_some_and(|name| {
        name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

fn typed_char(k: &KeyEvent) -> Option<char> {
    match k.code {
        KeyCode::Char(c)
            if !c.is_control() && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

fn sender_color(mid: &str) -> Color {
    let senders = &theme::t().senders;
    let h = mid
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    senders[(h as usize) % senders.len()]
}

// -- background work -------------------------------------------------------------------

/// A message id in a message box: a string, a number, or (inactive chats)
/// `{"messageId": …, "deliveredTime": …}`.
fn box_message_id(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.is_empty() && s != "0" => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Object(o) => o.get("messageId").and_then(box_message_id),
        _ => None,
    }
}

/// An int64 that may arrive as a JSON string.
fn json_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

/// Chat list entries from a `getMessageBoxes` page.
async fn parse_boxes(client: &Client, boxes: &serde_json::Value) -> Vec<ChatEntry> {
    let mut chats = Vec::new();
    for b in boxes["messageBoxes"].as_array().into_iter().flatten() {
        let Some(mid) = b["id"].as_str() else { continue };
        let last: Option<Message> = b["lastMessages"]
            .get(0)
            .and_then(|m| serde_json::from_value(m.clone()).ok());
        let delivered = &b["lastDeliveredMessageId"];
        let (preview, last_time, last_id) = match &last {
            Some(m) => {
                let m = client.decrypt_message(m).await.unwrap_or_else(|_| m.clone());
                (preview_of(&m), m.created_time.unwrap_or(0), Some(m.id.clone()))
            }
            // Inactive chats carry no message, only when the last one arrived.
            None => (
                String::new(),
                json_i64(&delivered["deliveredTime"]).unwrap_or(0),
                None,
            ),
        };
        let unread = json_i64(&b["unreadCount"])
            .unwrap_or(0)
            .clamp(0, i64::from(u32::MAX)) as u32;
        chats.push(ChatEntry {
            mid: mid.to_owned(),
            preview,
            last_time,
            unread,
            invited: false,
            muted: false,
            last_id: last_id.or_else(|| box_message_id(delivered)),
            last_seen: box_message_id(&b["lastSeenMessageId"]),
        });
    }
    chats
}

/// Fetch the chat list (`getMessageBoxes`): unread counts and newest
/// messages, then invitations and names.
fn spawn_chat_list(client: Client, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let boxes = match client.message_boxes(100).await {
            Ok(v) => v,
            Err(e) => {
                return drop(tx.send(AppEvent::Failed(Job::ChatList, format!("chat list failed: {e}"))));
            }
        };
        let chats = parse_boxes(&client, &boxes).await;
        let mids: Vec<String> = chats.iter().map(|c| c.mid.clone()).collect();
        let _ = tx.send(AppEvent::Chats(chats));
        fetch_invitations(&client, &tx).await;
        resolve_names(&client, &tx, mids).await;
    });
}

/// The complete list of pending group invitations.
async fn fetch_invitations(client: &Client, tx: &mpsc::UnboundedSender<AppEvent>) {
    let _ = tx.send(match client.invited_chats().await {
        Ok(inv) => AppEvent::Invitations(inv, true),
        Err(e) => AppEvent::Failed(Job::Invitations, format!("invitations failed: {e}")),
    });
}

/// Chats fetched per "load more" page.
const MORE_CHATS_PAGE: u32 = 100;

/// Fetch the next page of all chats, inactive ones included (the start-up
/// list only has the active ones). `known` chats are left out; pages that
/// hold nothing new are skipped. Posts `AppEvent::MoreChats`.
fn spawn_more_chats(
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    mut after: Option<String>,
    known: HashSet<String>,
) {
    tokio::spawn(async move {
        loop {
            let page = match client
                .message_boxes_page(after.as_deref(), MORE_CHATS_PAGE, false)
                .await
            {
                Ok(v) => v,
                Err(e) => return drop(tx.send(AppEvent::MoreChats(Err(e.to_string())))),
            };
            let last = page["messageBoxes"]
                .as_array()
                .and_then(|a| a.last())
                .and_then(|b| b["id"].as_str())
                .map(str::to_owned);
            let has_next = page["hasNext"].as_bool().unwrap_or(false) && last.is_some();
            let mut chats = parse_boxes(&client, &page).await;
            chats.retain(|c| !known.contains(&c.mid));
            after = last;
            if chats.is_empty() && has_next {
                continue;
            }
            let mids: Vec<String> = chats.iter().map(|c| c.mid.clone()).collect();
            let next = if has_next { after } else { None };
            let _ = tx.send(AppEvent::MoreChats(Ok((chats, next))));
            return resolve_names(&client, &tx, mids).await;
        }
    });
}

/// How long the friend list in the store is trusted before refreshing it.
const CONTACTS_MAX_AGE_MS: i64 = 12 * 3600 * 1000;

/// Refresh the friend list (names, pictures, mute settings) when the stored
/// copy is old.
fn spawn_contacts(client: Client, tx: mpsc::UnboundedSender<AppEvent>, store: Store, force: bool) {
    let fresh = store
        .get("cache.contacts_at")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .is_some_and(|at| now_ms() - at < CONTACTS_MAX_AGE_MS);
    if fresh && !force {
        return;
    }
    tokio::spawn(async move {
        let res = async {
            let ids = client.contact_ids().await?;
            client.contacts(&ids).await
        }
        .await;
        match res {
            Ok(map) => {
                let _ = store.set("cache.contacts_at", &now_ms().to_string());
                let _ = tx.send(AppEvent::Contacts(map.into_values().collect()));
            }
            Err(e) => drop(tx.send(AppEvent::Failed(
                Job::Contacts,
                format!("friend list failed: {e}"),
            ))),
        }
    });
}

/// Check whether a `NOTIFIED_INVITE_INTO_CHAT` invited us (the same operation
/// tells members about others being invited) and list it if so.
fn spawn_invite_check(client: Client, tx: mpsc::UnboundedSender<AppEvent>, chat: String, me: String) {
    tokio::spawn(async move {
        if let Ok(chats) = client.chats(&[chat]).await {
            let inv: Vec<Chat> = chats
                .into_iter()
                .filter(|c| c.invitee_mids().contains(&me))
                .collect();
            if !inv.is_empty() {
                let _ = tx.send(AppEvent::Invitations(inv, false));
            }
        }
    });
}

/// How long before a name that could not be looked up is asked for again.
const NAME_RETRY: Duration = Duration::from_secs(60);

/// Images kept in the store before the oldest are dropped.
const IMAGE_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// Store key of a profile/group picture at one size.
fn picture_key(path: &str, size: PictureSize) -> String {
    let size = match size {
        PictureSize::Full => "full",
        PictureSize::Preview => "preview",
        PictureSize::Small => "small",
    };
    format!("pic:{size}:{path}")
}

/// Store key of a cover picture (by its OBS path).
fn cover_key(obs_path: &str) -> String {
    format!("cover:{obs_path}")
}

/// A profile/group picture from the store, else downloaded and stored.
/// The second value says whether it was downloaded now.
async fn cached_picture(
    client: &Client,
    store: &Store,
    path: &str,
    size: PictureSize,
) -> Option<(Vec<u8>, bool)> {
    let key = picture_key(path, size);
    if let Ok(Some(b)) = store.image(&key) {
        return Some((b, false));
    }
    let b = match client.download_picture(path, size).await {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(path, "picture download failed: {e}");
            return None;
        }
    };
    let _ = store.put_image(&key, &b);
    Some((b, true))
}

/// Where pictures are written for the system viewer to open (not the
/// download folder, which is for what you save): `$XDG_CACHE_HOME/rune/view`.
fn view_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".cache")
        });
    cache.join("rune").join("view")
}

/// Files in [`view_dir`] older than this are removed at start-up.
const VIEW_CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// ... and the newest ones are kept only up to this total size.
const VIEW_CACHE_BYTES: u64 = 1 << 30;

/// Remove the files in `dir` older than `max_age`, then the oldest of the
/// rest until they total at most `max_bytes`. Returns how many went. A
/// missing directory is fine; subdirectories and symlinks are left alone.
fn prune_dir(
    dir: &std::path::Path,
    max_age: Duration,
    max_bytes: u64,
    now: std::time::SystemTime,
) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let meta = e.metadata().ok().filter(std::fs::Metadata::is_file)?;
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    // Newest first: what is kept is a prefix.
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut kept = 0u64;
    let mut removed = 0;
    for (modified, len, path) in files {
        let old = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if old || kept + len > max_bytes {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        } else {
            kept += len;
        }
    }
    Ok(removed)
}

/// [`view_dir`], created if needed and private to this user: it holds
/// decrypted copies of media and pictures. Every writer goes through here.
fn private_view_dir() -> std::io::Result<PathBuf> {
    let dir = view_dir();
    make_private_dir(&dir)?;
    Ok(dir)
}

/// Create `dir` (and its parents) owner-only, and make an existing one
/// owner-only too: a directory created earlier with the default mode (by an
/// older version, or by hand) would otherwise stay readable by everyone.
fn make_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        b.mode(0o700);
        b.create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    b.create(dir)
}

/// Write `data` to [`view_dir`] for opening (private to this user).
fn store_for_viewing(name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let dir = private_view_dir()?;
    store_download_in(&dir, name, data)
}

/// A mid `getContactsV2` / `getChats` can look up (not empty, not a square).
/// Mids are either `U`/`C`/`R` + 32 hex digits or, as most are now, 44
/// base64url characters (`-` and `_` included; live).
fn resolvable(mid: &str) -> bool {
    mid.len() >= 8
        && mid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && matches!(MidType::of(mid), MidType::User | MidType::Group | MidType::Room)
}

/// Contacts for `mids`; when a batch is refused (one bad mid can fail it),
/// split it so the rest still resolve.
/// Mids whose own request failed are added to `failed` (the others were
/// answered, or are unknown to LINE).
async fn contacts_lenient(
    client: &Client,
    mids: &[String],
    failed: &mut Vec<String>,
) -> HashMap<String, Contact> {
    let mut out = HashMap::new();
    let mut todo = vec![mids];
    while let Some(part) = todo.pop() {
        match client.contacts(part).await {
            Ok(cs) => out.extend(cs),
            Err(e) if part.len() > 1 => {
                tracing::debug!("getContactsV2 for {} mids failed ({e}); splitting", part.len());
                let (a, b) = part.split_at(part.len() / 2);
                todo.extend([a, b]);
            }
            Err(e) => {
                tracing::debug!("getContactsV2 {}: {e}", part[0]);
                failed.extend_from_slice(part);
            }
        }
    }
    out
}

/// Chats for `mids`, split like [`contacts_lenient`] when a batch is refused.
async fn chats_lenient(client: &Client, mids: &[String], failed: &mut Vec<String>) -> Vec<Chat> {
    let mut out = Vec::new();
    let mut todo = vec![mids];
    while let Some(part) = todo.pop() {
        match client.chats(part).await {
            Ok(cs) => out.extend(cs),
            Err(e) if part.len() > 1 => {
                tracing::debug!("getChats for {} mids failed ({e}); splitting", part.len());
                let (a, b) = part.split_at(part.len() / 2);
                todo.extend([a, b]);
            }
            Err(e) => {
                tracing::debug!("getChats {}: {e}", part[0]);
                failed.extend_from_slice(part);
            }
        }
    }
    out
}

/// Look up display names and pictures for users (contacts) and groups
/// (chats). Every view takes both from here (or from the friend list and
/// profile pages, which post the same events).
async fn resolve_names(client: &Client, tx: &mpsc::UnboundedSender<AppEvent>, mids: Vec<String>) {
    lookup_names(client, mids).await.post(tx);
}

/// What one round of name lookups found.
#[derive(Default)]
struct Lookup {
    names: HashMap<String, String>,
    pictures: HashMap<String, Option<String>>,
    mutes: HashMap<String, bool>,
    members: HashMap<String, (Vec<String>, Vec<String>)>,
    /// Answered, or unknown to LINE (a group we left): not asked again this run.
    looked_up: Vec<String>,
}

impl Lookup {
    /// Tell the app what was found.
    fn post(self, tx: &mpsc::UnboundedSender<AppEvent>) {
        if !self.members.is_empty() {
            let _ = tx.send(AppEvent::Members(self.members));
        }
        if !self.mutes.is_empty() {
            let _ = tx.send(AppEvent::Mutes(self.mutes));
        }
        if !self.pictures.is_empty() {
            let _ = tx.send(AppEvent::Pictures(self.pictures));
        }
        if !self.names.is_empty() {
            let _ = tx.send(AppEvent::Names(self.names));
        }
        if !self.looked_up.is_empty() {
            let _ = tx.send(AppEvent::LookedUp(self.looked_up));
        }
    }
}

/// Look up the names, pictures, mute settings (and, for groups, members) of `mids`.
async fn lookup_names(client: &Client, mids: Vec<String>) -> Lookup {
    let mut mids: Vec<String> = mids.into_iter().filter(|m| resolvable(m)).collect();
    mids.sort();
    mids.dedup();
    let (groups, users): (Vec<String>, Vec<String>) = mids
        .iter()
        .cloned()
        .partition(|m| MidType::of(m) != MidType::User);
    let mut found = Lookup::default();
    let mut failed = Vec::new();
    if !users.is_empty() {
        let cs = contacts_lenient(client, &users, &mut failed).await;
        for (mid, c) in cs {
            // `None` too: the picture was removed (or is not shown to us).
            found.pictures.insert(mid.clone(), c.picture());
            found.mutes.insert(mid.clone(), c.notifications_disabled());
            found.names.insert(mid, c.name().to_owned());
        }
    }
    if !groups.is_empty() {
        for c in chats_lenient(client, &groups, &mut failed).await {
            found.pictures.insert(c.chat_mid.clone(), c.picture());
            found.mutes.insert(c.chat_mid.clone(), c.notifications_disabled());
            found
                .members
                .insert(c.chat_mid.clone(), (c.member_mids(), c.invitee_mids()));
            found.names.insert(c.chat_mid.clone(), c.chat_name.clone());
        }
    }
    mids.retain(|m| !failed.contains(m));
    found.looked_up = mids;
    found
}

/// Pages fetched backwards to close a gap between stored and new messages.
const GAP_PAGES: usize = 4;

/// Fetch a chat's recent history from the server (when the store does not
/// have its newest messages). If the store holds older messages that the
/// recent page does not reach, page back to join them up; if that fails,
/// the stored ones are dropped so the history never has a silent hole.
fn spawn_history(client: Client, tx: mpsc::UnboundedSender<AppEvent>, store: Store, chat: String) {
    tokio::spawn(async move {
        let res = async {
            let mut msgs = client.recent_messages(&chat, 60).await?;
            msgs.reverse();
            let newest_stored = store.newest_message(&chat).ok().flatten();
            if let Some(stored) = newest_stored.map(|s| s.message) {
                let mut joined = msgs.iter().any(|m| message_id_cmp(&m.id, &stored.id).is_le());
                let mut pages = 0;
                while !joined && pages < GAP_PAGES {
                    let Some(oldest) = msgs.first().cloned() else {
                        break;
                    };
                    let delivered = oldest.delivered_time().or(oldest.created_time).unwrap_or(0);
                    let mut older = client
                        .previous_messages(&chat, &oldest.id, delivered, 100)
                        .await?;
                    if older.is_empty() {
                        break;
                    }
                    older.reverse();
                    joined = older.iter().any(|m| message_id_cmp(&m.id, &stored.id).is_le());
                    older.append(&mut msgs);
                    msgs = older;
                    pages += 1;
                }
                if !joined {
                    tracing::info!(%chat, "history gap too large; dropping stored messages");
                    let _ = store.clear_messages(&chat);
                }
            }
            let mut out = Vec::with_capacity(msgs.len());
            for m in msgs {
                // Keep undecryptable messages; describe() shows them as sealed.
                out.push(client.decrypt_message(&m).await.unwrap_or(m));
            }
            Ok::<_, rune::Error>(out)
        }
        .await;
        match res {
            Ok(out) => {
                let senders: Vec<String> = out
                    .iter()
                    .map(|m| m.from.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                let _ = tx.send(AppEvent::History(chat, out));
                resolve_names(&client, &tx, senders).await;
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Failed(
                    Job::History(chat),
                    format!("history failed: {e}"),
                ));
            }
        }
    });
}

/// Older messages than `(id, delivered)` from the server.
fn spawn_older(
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    chat: String,
    id: String,
    delivered: i64,
) {
    const PAGE: u32 = 50;
    tokio::spawn(async move {
        match client.previous_messages(&chat, &id, delivered, PAGE).await {
            Ok(mut msgs) => {
                msgs.reverse();
                let done = msgs.len() < PAGE as usize;
                let mut out = Vec::with_capacity(msgs.len());
                for m in msgs {
                    out.push(client.decrypt_message(&m).await.unwrap_or(m));
                }
                let _ = tx.send(AppEvent::Older(chat, out, done));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Older(chat, Vec::new(), false));
                let _ = tx.send(AppEvent::Status(format!("older messages failed: {e}")));
            }
        }
    });
}

/// The operation stream, resuming at `from` (the stored position) so events
/// that happened while rune was closed are replayed first.
fn spawn_stream(client: Client, tx: mpsc::UnboundedSender<AppEvent>, from: Option<i64>) {
    tokio::spawn(async move {
        // First run: start from now, and remember that, so the next start
        // replays whatever happens after this point even if nothing happens now.
        let from = match from {
            Some(r) => Some(r),
            None => match client.last_op_revision().await {
                Ok(r) => {
                    let _ = tx.send(AppEvent::Control(
                        "start".into(),
                        serde_json::json!({ "nextRevision": r }),
                    ));
                    Some(r)
                }
                Err(_) => None,
            },
        };
        let mut ops = client.operations(from);
        loop {
            let ev = match ops.next_event().await {
                Ok(rune::Event::Operation(op)) => AppEvent::Incoming(op),
                Ok(rune::Event::Control { name, data }) => {
                    if !matches!(name.as_str(), "fullSync" | "partialFullSync" | "unparsed") {
                        continue;
                    }
                    AppEvent::Control(name, data)
                }
                Err(e) => {
                    // Signed out or too old a client: trying again cannot help.
                    let permanent = matches!(e, rune::Error::LoginRequired)
                        || e.api().is_some_and(|a| a.is_must_upgrade());
                    let msg = format!("event stream stopped: {e}");
                    let _ = tx.send(if permanent {
                        AppEvent::Status(msg)
                    } else {
                        AppEvent::Failed(Job::Stream, msg)
                    });
                    return;
                }
            };
            if tx.send(ev).is_err() {
                return;
            }
        }
    });
}

/// How a failed read receipt request is handled: the server refusing that
/// position for good (a `TalkException`, e.g. the message is gone) drops it;
/// anything else — network, server trouble, token refresh — is retried.
fn receipt_outcome(e: &rune::Error) -> receipts::Outcome {
    match e.api() {
        Some(a)
            if a.talk_code.is_some()
                && !a.is_retryable()
                && !a.is_auth()
                && !a.must_refresh_token()
                && a.status < 500 =>
        {
            receipts::Outcome::Refused
        }
        _ => receipts::Outcome::Retry,
    }
}

// -- app logic ---------------------------------------------------------------------------

impl App {
    /// A fresh app showing what the store already knows.
    fn new(client: Client, me: String, tx: mpsc::UnboundedSender<AppEvent>, store: Store) -> App {
        let prefs = Prefs::load(&store);
        theme::apply(prefs.theme);
        let (keys, key_problems) = shortcuts::load(&store);
        for problem in &key_problems {
            tracing::warn!("shortcuts: {problem}");
        }
        keymap::install(keys.clone());
        let store_for_plugins = store.clone();
        let mut app = App {
            client,
            me,
            tx,
            thumb_size: prefs.thumb_size,
            store,
            prefs,
            no_read_receipts_flag: false,
            keys,
            default_keys: false,
            key_problems,
            keys_file: None,
            last_find: None,
            modal: None,
            modal_parent: None,
            contacts: HashMap::new(),
            members: HashMap::new(),
            invitees: HashMap::new(),
            read_marks: HashMap::new(),
            typing: HashMap::new(),
            uploads: Vec::new(),
            sticker_pics: PicCache::new(STICKER_CACHE_BYTES),
            emoji_packs: Vec::new(),
            file_previews: PicCache::new(PREVIEW_CACHE_BYTES),
            server_settings: None,
            my_profile: None,
            stale: HashSet::new(),
            resync: HashSet::new(),
            older_loading: HashSet::new(),
            no_older: HashSet::new(),
            at_top: false,
            unread_divider: None,
            jump_unread: false,
            focused: true,
            chats: Vec::new(),
            searching: false,
            chat_query: TextInput::default(),
            pinned: Vec::new(),
            pin_list: None,
            list: ListState::default(),
            more_chats: MoreChats::Ready(None),
            names: HashMap::new(),
            name_requests: HashMap::new(),
            name_misses: Default::default(),
            looked_up: HashSet::new(),
            picture_paths: HashMap::new(),
            avatars: HashMap::new(),
            media: FetchQueue::default(),
            messages: HashMap::new(),
            loading: HashSet::new(),
            open: None,
            focus: Focus::Chats,
            input: TextInput::default(),
            scroll: 0,
            status: String::new(),
            status_since: None,
            quit: false,
            sel: None,
            thumbs: PicCache::new(THUMB_CACHE_BYTES),
            full: HashMap::new(),
            full_lru: Vec::new(),
            pending: HashMap::new(),
            viewer: None,
            profile: Vec::new(),
            profile_sel: None,
            profile_btn: 0,
            profiles: HashMap::new(),
            qr: HashMap::new(),
            cell_mode: CellMode::Quadrant,
            picker: None,
            anim: None,
            needs_clear: false,
            follow_sel: false,
            hit: HitAreas::default(),
            replying: None,
            drafts: HashMap::new(),
            pasting: false,
            retry: RetryQueue::default(),
            receipts: Receipts::default(),
            receipt_tasks: HashMap::new(),
            #[cfg(test)]
            sent_receipts: Vec::new(),
            store_failure: Default::default(),
            plugins: plugins::Plugins::new(store_for_plugins, None, false, keymap::CORE_COMMANDS),
            plugin_job: None,
            plugin_available: HashMap::new(),
        };
        app.receipts = Receipts::load(app.store.pending_receipts().unwrap_or_default());
        app.reload_pins();
        app.load_emoji_packages();
        for (key, text) in app.store.values_with_prefix(DRAFT_KEY).unwrap_or_default() {
            let input = TextInput::from(text);
            let mid = key[DRAFT_KEY.len()..].to_owned();
            app.drafts.insert(
                mid,
                Draft {
                    input,
                    replying: None,
                },
            );
        }
        // Chat list, names and friends from the last run, before any request.
        for row in app.store.chats().unwrap_or_default() {
            if !row.name.is_empty() {
                app.names.insert(row.mid.clone(), row.name.clone());
            }
            if let Some(p) = row.picture.clone().filter(|p| !p.is_empty()) {
                app.picture_paths.insert(row.mid.clone(), p);
            }
            app.chats.push(ChatEntry::from_row(row));
        }
        for c in app.store.contacts().unwrap_or_default() {
            app.contacts.insert(c.mid.clone(), c.name().to_owned());
            app.names
                .entry(c.mid.clone())
                .or_insert_with(|| c.name().to_owned());
            if let Some(p) = c.picture() {
                app.picture_paths.entry(c.mid.clone()).or_insert(p);
            }
        }
        // Everyone seen before, friends or not (group members), from earlier runs.
        for u in app.store.users().unwrap_or_default() {
            if !u.name.is_empty() {
                app.names.entry(u.mid.clone()).or_insert(u.name);
            }
            if let Some(p) = u.picture.filter(|p| !p.is_empty()) {
                app.picture_paths.entry(u.mid).or_insert(p);
            }
        }
        app.sort_chats(None);
        if !app.list_rows().is_empty() {
            app.list.select(Some(0));
        }
        app
    }

    fn name(&self, mid: &str) -> String {
        if mid == self.me {
            return "You".into();
        }
        match self.names.get(mid).filter(|n| !n.is_empty()) {
            Some(n) => n.clone(),
            None => {
                // Whatever is shown without a name gets looked up after this frame.
                if let Ok(mut misses) = self.name_misses.try_borrow_mut() {
                    misses.insert(mid.to_owned());
                }
                format!("{}…", mid.get(..8).unwrap_or(mid))
            }
        }
    }

    fn is_pinned(&self, mid: &str) -> bool {
        self.pinned.iter().any(|p| p == mid)
    }

    /// Start a lookup for the names and pictures of `mids` that were not
    /// looked up this run and were not asked for in the last `NAME_RETRY`.
    fn want_names(&mut self, mids: impl IntoIterator<Item = String>) {
        let now = std::time::Instant::now();
        let mut ask: Vec<String> = Vec::new();
        for mid in mids {
            if !resolvable(&mid)
                || mid == self.me
                // Answered this run (a group we left stays nameless: not asked again).
                || self.looked_up.contains(&mid)
                || ask.contains(&mid)
                || self
                    .name_requests
                    .get(&mid)
                    .is_some_and(|at| now.duration_since(*at) < NAME_RETRY)
            {
                continue;
            }
            self.name_requests.insert(mid.clone(), now);
            ask.push(mid);
        }
        if !ask.is_empty() {
            let (client, tx) = (self.client.clone(), self.tx.clone());
            tokio::spawn(async move { resolve_names(&client, &tx, ask).await });
        }
    }

    /// Look up every name that was drawn as a bare mid.
    fn request_missing_names(&mut self) {
        let mut misses = std::mem::take(&mut *self.name_misses.borrow_mut());
        // Hidden (nameless) groups are never drawn: ask for them here, so one
        // that only lacks its name yet shows up once it has it.
        misses.extend(
            self.chats
                .iter()
                .filter(|c| !self.looked_up.contains(&c.mid) && self.hidden(c))
                .map(|c| c.mid.clone()),
        );
        if !misses.is_empty() {
            self.want_names(misses);
        }
    }

    /// Remember names and pictures in the store's user table (keeping what
    /// a profile page learned there), so everyone shows by name on the next
    /// start too, group members who are not friends included.
    fn persist_users(&self, mids: &[String]) {
        for mid in mids {
            let Some(name) = self.names.get(mid).filter(|n| !n.is_empty()) else {
                continue;
            };
            let old = self.store.user(mid).ok().flatten();
            let mut row = old.clone().unwrap_or_else(|| UserRow {
                mid: mid.clone(),
                ..Default::default()
            });
            row.name = name.clone();
            if let Some(p) = self.picture_paths.get(mid) {
                row.picture = Some(p.clone());
            }
            if old.is_none_or(|o| {
                UserRow { updated: 0, ..o }
                    != UserRow {
                        updated: 0,
                        ..row.clone()
                    }
            }) {
                self.saved(self.store.upsert_user(&row));
            }
        }
    }

    fn selected_mid(&self) -> Option<String> {
        let rows = self.list_rows();
        Some(self.row_mid(rows.get(self.list.selected()?)?).to_owned())
    }

    fn row_mid<'a>(&'a self, row: &'a ListRow) -> &'a str {
        match row {
            ListRow::Chat(i) => &self.chats[*i].mid,
            ListRow::Friend(mid) => mid,
        }
    }

    /// A search query is typed (the list shows only what it matches).
    fn filtering(&self) -> bool {
        self.searching && !self.chat_query.as_str().trim().is_empty()
    }

    /// The search query's words, lowercased (none unless [`App::filtering`]).
    fn search_words(&self) -> Vec<String> {
        if !self.searching {
            return Vec::new();
        }
        self.chat_query
            .as_str()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect()
    }

    /// The chat list's rows. While a query is typed: the chats it matches,
    /// best first (see [`App::chat_rank`]; list order — invitations, pins,
    /// recent activity — among equals), then the matching friends who have
    /// no chat listed.
    fn list_rows(&self) -> Vec<ListRow> {
        let words = self.search_words();
        if words.is_empty() {
            return (0..self.chats.len())
                .filter(|&i| !self.hidden(&self.chats[i]))
                .map(ListRow::Chat)
                .collect();
        }
        let mut chats: Vec<(u8, usize)> = self
            .chats
            .iter()
            .enumerate()
            .filter(|(_, c)| !self.hidden(c))
            .filter_map(|(i, c)| Some((self.chat_rank(c, &words)?, i)))
            .collect();
        chats.sort_by_key(|&(rank, _)| rank);
        let listed: HashSet<&str> = self.chats.iter().map(|c| c.mid.as_str()).collect();
        let mut friends: Vec<(u8, String, &String)> = self
            .contacts
            .iter()
            .filter(|(mid, _)| **mid != self.me && !listed.contains(mid.as_str()))
            .filter_map(|(mid, contact_name)| {
                let name = self.known_name(mid).unwrap_or(contact_name);
                Some((name_rank(name, &words)?, name.to_lowercase(), mid))
            })
            .collect();
        // By name for reading, the mid breaking ties (names may collide).
        friends.sort();
        chats
            .into_iter()
            .map(|(_, i)| ListRow::Chat(i))
            .chain(
                friends
                    .into_iter()
                    .map(|(_, _, mid)| ListRow::Friend(mid.clone())),
            )
            .collect()
    }

    /// The listed chats in list order (the rows that are chats).
    #[cfg(test)]
    fn visible_chats(&self) -> Vec<usize> {
        self.list_rows()
            .into_iter()
            .filter_map(|r| match r {
                ListRow::Chat(i) => Some(i),
                ListRow::Friend(_) => None,
            })
            .collect()
    }

    /// A group or room with no name is left out of the chat list: almost
    /// always one we are no longer in (LINE no longer answers for it). An
    /// invitation, or the chat that is open, stays.
    fn hidden(&self, c: &ChatEntry) -> bool {
        matches!(MidType::of(&c.mid), MidType::Group | MidType::Room)
            && !c.invited
            && self.open.as_deref() != Some(c.mid.as_str())
            && self.known_name(&c.mid).is_none()
    }

    fn known_name(&self, mid: &str) -> Option<&str> {
        self.names.get(mid).map(String::as_str).filter(|n| !n.is_empty())
    }

    /// How well a chat matches the query words: its name's [`name_rank`],
    /// else 2 when every word is in its name or a member's (or invitee's).
    fn chat_rank(&self, chat: &ChatEntry, words: &[String]) -> Option<u8> {
        let name = self.name(&chat.mid);
        if let Some(rank) = name_rank(&name, words) {
            return Some(rank);
        }
        words
            .iter()
            .all(|w| !find_word(&name, w).is_empty() || self.members_matching(chat, w).next().is_some())
            .then_some(2)
    }

    /// Names of the chat's members and invitees (other than us) that contain `word`.
    fn members_matching<'a>(
        &'a self,
        chat: &'a ChatEntry,
        word: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.members
            .get(&chat.mid)
            .into_iter()
            .chain(self.invitees.get(&chat.mid))
            .flatten()
            .filter(|mid| **mid != self.me)
            .filter_map(|mid| self.known_name(mid))
            .filter(move |name| !find_word(name, word).is_empty())
    }

    /// The chat at list position `pos` (`None` for a friend row).
    fn chat_at(&self, pos: usize) -> Option<&ChatEntry> {
        match self.list_rows().get(pos)? {
            ListRow::Chat(i) => self.chats.get(*i),
            ListRow::Friend(_) => None,
        }
    }

    fn select_chat_mid(&mut self, mid: Option<&str>) {
        let rows = self.list_rows();
        self.list
            .select(mid.and_then(|mid| rows.iter().position(|r| self.row_mid(r) == mid)));
    }

    fn set_chat_query(&mut self, query: String) {
        let selected = self.selected_mid();
        self.chat_query.set(query);
        self.chat_query_changed(selected);
    }

    /// The query was edited: the best match is selected, so Enter opens it.
    /// With the query emptied the full list is back, `selected` still selected.
    fn chat_query_changed(&mut self, selected: Option<String>) {
        if self.filtering() {
            let any = !self.list_rows().is_empty();
            self.list.select(any.then_some(0));
            return;
        }
        self.select_chat_mid(selected.as_deref());
        if self.list.selected().is_none() && !self.chats.is_empty() {
            self.list.select(Some(0));
        }
    }

    /// `/` in the chat list or Ctrl-f anywhere: type in the search field, the
    /// query left by an earlier search (if any) still there.
    fn start_search(&mut self) {
        self.focus = Focus::Chats;
        if !self.searching {
            self.searching = true;
            self.set_chat_query(String::new());
        }
    }

    /// The focus moved away from the search field: a typed query stays (the
    /// list stays filtered) until Esc or a chat is opened; an empty one ends.
    fn left_search(&mut self) {
        if self.searching && self.focus != Focus::Chats && !self.filtering() {
            self.end_search();
        }
    }

    fn end_search(&mut self) {
        let selected = self.selected_mid();
        self.searching = false;
        self.chat_query.clear();
        self.select_chat_mid(selected.as_deref());
    }

    fn read_receipts(&self) -> bool {
        self.prefs.read_receipts && !self.no_read_receipts_flag
    }

    /// `Ctrl-r`: the *Send read receipts* setting, from anywhere. Turned
    /// back on in a chat, what was read there meanwhile is marked read;
    /// other chats are marked when next opened (see `receipts.rs`).
    fn toggle_read_receipts(&mut self) {
        if self.no_read_receipts_flag {
            self.status = "read receipts are off for this run (--no-read-receipts)".into();
            return;
        }
        self.prefs.read_receipts ^= true;
        self.prefs.save(&self.store);
        self.receipts_switched();
        self.status = if self.prefs.read_receipts {
            "read receipts on: chats you read are marked read".into()
        } else {
            match keymap::global(keymap::Cmd::ReadReceipts) {
                key if key.is_empty() => {
                    "read receipts off: read without others seeing it (Settings › Chats turns them back on)"
                        .into()
                }
                key => format!("read receipts off: read without others seeing it ({key} turns them back on)"),
            }
        };
    }

    /// The *Send read receipts* setting changed (`Ctrl-r` or the settings
    /// page). Off: requests in flight are cancelled, their positions kept.
    /// On: what was read in the open chat meanwhile goes out.
    pub fn receipts_switched(&mut self) {
        if !self.read_receipts() {
            for ticket in self.receipts.disable() {
                if let Some(task) = self.receipt_tasks.remove(&ticket) {
                    task.abort();
                }
            }
            return;
        }
        match self.open.clone() {
            Some(open) if self.focused => {
                self.mark_seen(&open);
                self.note_read(&open, None);
            }
            _ => self.flush_receipts(),
        }
    }

    /// Clear `chat`'s unread count and divider position up to its newest
    /// message — only with read receipts on. With them off a chat stays
    /// unread, here as on the server, until it is read with them on or on
    /// another device (op 40, [`OpEvent::ChatChecked`]).
    fn mark_seen(&mut self, chat: &str) {
        if !self.read_receipts() {
            return;
        }
        let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) else {
            return;
        };
        if c.unread == 0 && c.last_seen == c.last_id {
            return;
        }
        c.unread = 0;
        c.last_seen = c.last_id.clone();
        self.persist_chat(chat);
        self.sort_chats(self.selected_mid());
    }

    /// The user read `chat` (they are in it) up to `id`, or saw nothing new
    /// there (`None`: being there still approves what was read with
    /// receipts off). With receipts on it is sent; off, it is held — also
    /// under `--no-read-receipts`, which counts as off: held reads go out
    /// only when the user is in that chat again in a run with receipts on.
    fn note_read(&mut self, chat: &str, id: Option<&str>) {
        let on = self.read_receipts();
        let changed = match id {
            Some(id) => self.receipts.read(chat, id, on),
            None => on && self.receipts.promote(chat),
        };
        if changed {
            self.persist_receipt(chat);
        }
        self.flush_receipts();
    }

    fn persist_receipt(&self, chat: &str) {
        let (approved, held) = self.receipts.row(chat);
        self.saved(self.store.set_pending_receipt(chat, approved, held));
    }

    /// Start the read receipt requests that are due (none while receipts are off).
    fn flush_receipts(&mut self) {
        for s in self.receipts.take_due(self.read_receipts()) {
            self.start_receipt(s);
        }
    }

    #[cfg(not(test))]
    fn start_receipt(&mut self, s: receipts::Send) {
        let (client, tx, ticket) = (self.client.clone(), self.tx.clone(), s.ticket);
        let task = tokio::spawn(async move {
            let outcome = match client.mark_read(&s.chat, &s.id).await {
                Ok(()) => receipts::Outcome::Acked,
                Err(e) => {
                    tracing::warn!("read receipt for {} up to {} failed: {e}", s.chat, s.id);
                    receipt_outcome(&e)
                }
            };
            let _ = tx.send(AppEvent::Receipt(s, outcome));
        });
        self.receipt_tasks.insert(ticket, task.abort_handle());
    }

    #[cfg(test)]
    fn start_receipt(&mut self, s: receipts::Send) {
        self.sent_receipts.push(s);
    }

    /// A read receipt request ended.
    fn on_receipt(&mut self, s: receipts::Send, outcome: receipts::Outcome) {
        self.receipt_tasks.remove(&s.ticket);
        let (changed, next) = self.receipts.finished(&s, outcome);
        if changed {
            self.persist_receipt(&s.chat);
        }
        let job = Job::Receipt(s.chat);
        match next {
            receipts::Next::BackOff => {
                self.retry.failed(job, Instant::now());
            }
            receipts::Next::Flush => {
                self.retry.succeeded(&job);
                self.flush_receipts();
            }
            receipts::Next::Nothing => {
                if outcome != receipts::Outcome::Retry {
                    self.retry.succeeded(&job);
                }
            }
        }
    }

    /// The terminal gained or lost focus. Coming back to the open chat reads
    /// what arrived there meanwhile (marked read with read receipts on).
    fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            return;
        }
        let Some(open) = self.open.clone() else {
            return;
        };
        let on = self.read_receipts();
        let Some(c) = self.chats.iter_mut().find(|c| c.mid == open) else {
            return;
        };
        let last = c.last_id.clone();
        let new = last.is_some() && c.last_seen != last;
        if new && on {
            self.mark_seen(&open);
        }
        self.note_read(&open, if new { last.as_deref() } else { None });
    }

    /// Check a write to the local store: a failure is logged and shown on the
    /// status line (see [`App::report_store_failure`]) instead of vanishing.
    fn saved<T>(&self, r: rune::Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("local store write failed: {e}");
                *self.store_failure.borrow_mut() = Some(e.to_string());
                None
            }
        }
    }

    fn report_store_failure(&mut self) {
        if let Some(e) = self.store_failure.get_mut().take() {
            self.status = format!("could not save to the local store: {e}");
        }
    }

    fn unread_index(&self, chat: &str, msgs: &[UiMsg]) -> Option<usize> {
        let (divider_chat, unread) = self.unread_divider.as_ref()?;
        if divider_chat != chat {
            return None;
        }
        match unread {
            Unread::After(anchor) => msgs
                .iter()
                .position(|m| !m.id.is_empty() && m.from != self.me && message_id_cmp(&m.id, anchor).is_gt()),
            Unread::Last(n) => msgs
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, m)| !m.id.is_empty() && m.from != self.me)
                .nth(n.saturating_sub(1) as usize)
                .map(|(i, _)| i),
        }
    }

    /// Open a chat the way the user asked for it (Enter, a click, a profile's
    /// Chat button): the chat list follows it and the message line takes the
    /// keys. An invitation asks whether to join instead.
    fn enter_chat(&mut self, mid: String) {
        if self.chats.iter().any(|c| c.mid == mid && c.invited) {
            return self.invitation_menu(&mid);
        }
        if self.searching {
            self.end_search();
        }
        self.open_chat(mid.clone());
        if self.open.as_deref() == Some(mid.as_str()) {
            // A chat that is not listed (a friend never written to) keeps the old selection.
            if self.chats.iter().any(|c| c.mid == mid) {
                self.select_chat_mid(Some(&mid));
            }
            self.focus = Focus::Input;
        }
    }

    /// Move the chat-list selection by `delta` rows, staying on the list
    /// (ratatui only clamps a selection when drawing, which is too late for
    /// keys that arrive before the next frame).
    fn move_chat_sel(&mut self, delta: isize) {
        let n = self.list_rows().len();
        if n == 0 {
            self.list.select(None);
            return;
        }
        let i = match self.list.selected() {
            Some(i) => i.min(n - 1).saturating_add_signed(delta).min(n - 1),
            None if delta < 0 => n - 1,
            None => 0,
        };
        self.list.select(Some(i));
    }

    fn open_chat(&mut self, mid: String) {
        if self.chats.iter().any(|c| c.mid == mid && c.invited) {
            self.status = format!(
                "invitation to {} — {}",
                self.name(&mid),
                keymap::with_keys(keymap::Ctx::ChatList, keymap::Cmd::Confirm, "to join or decline")
            );
            return;
        }
        let mut mark = None;
        let mut has_unread = false;
        let on = self.read_receipts();
        if let Some(c) = self.chats.iter_mut().find(|c| c.mid == mid) {
            has_unread = c.unread > 0 || c.last_seen != c.last_id;
            if has_unread {
                self.unread_divider = Some((
                    mid.clone(),
                    c.last_seen
                        .clone()
                        .map(Unread::After)
                        .unwrap_or(Unread::Last(c.unread)),
                ));
                mark = c.last_id.clone();
            } else if self.open.as_deref() != Some(mid.as_str()) {
                self.unread_divider = None;
            }
            // With receipts off the chat stays unread, as it does on the server.
            if on {
                c.unread = 0;
                c.last_seen = c.last_id.clone();
            }
        } else if self.open.as_deref() != Some(mid.as_str()) {
            self.unread_divider = None;
        }
        self.jump_unread = has_unread;
        if !self.messages.contains_key(&mid) {
            let stored = self.store.messages(&mid, 200, None).unwrap_or_default();
            if !stored.is_empty() {
                self.messages
                    .insert(mid.clone(), stored.iter().filter_map(stored_to_ui).collect());
            }
        }
        if !self.read_marks.contains_key(&mid) {
            let marks = self.store.read_marks(&mid).unwrap_or_default();
            self.read_marks.insert(mid.clone(), marks);
        }
        let empty = self.messages.get(&mid).is_none_or(Vec::is_empty);
        let suspect = self.stale.contains(&mid) || self.resync.contains(&mid);
        if (empty || suspect) && self.loading.insert(mid.clone()) {
            self.status = "loading…".into();
            spawn_history(
                self.client.clone(),
                self.tx.clone(),
                self.store.clone(),
                mid.clone(),
            );
        }
        self.note_read(&mid, mark.as_deref());
        self.persist_chat(&mid);
        if has_unread && on {
            // A pinned chat that floated while unread returns to its pin position.
            self.sort_chats(self.selected_mid());
        }
        self.switch_draft(Some(&mid));
        self.open = Some(mid.clone());
        self.scroll = 0;
        self.sel = None;
        self.plugins_chat_opened(&mid);
    }

    /// Write a chat-list entry (with its name and picture) to the store.
    fn persist_chat(&self, mid: &str) {
        let Some(c) = self.chats.iter().find(|c| c.mid == mid) else {
            return;
        };
        let row = ChatRow {
            mid: c.mid.clone(),
            name: self.names.get(mid).cloned().unwrap_or_default(),
            picture: self.picture_paths.get(mid).cloned(),
            preview: c.preview.clone(),
            last_message_id: c.last_id.clone(),
            last_time: c.last_time,
            unread: c.unread,
            invited: c.invited,
            muted: c.muted,
            last_seen: c.last_seen.clone(),
        };
        self.saved(self.store.upsert_chat(&row));
    }

    /// Put the message line away with the open chat and bring back `to`'s.
    fn switch_draft(&mut self, to: Option<&str>) {
        if self.open.as_deref() == to {
            return;
        }
        let draft = Draft {
            input: std::mem::take(&mut self.input),
            replying: self.replying.take(),
        };
        if let Some(from) = self.open.clone() {
            self.save_draft(&from, &draft.input);
            if !draft.input.as_str().trim().is_empty() {
                self.drafts.insert(from, draft);
            }
        }
        if let Some(d) = to.and_then(|mid| self.drafts.remove(mid)) {
            self.input = d.input;
            self.replying = d.replying;
        }
    }

    /// Keep a chat's draft text across restarts (or forget it once empty).
    fn save_draft(&self, mid: &str, input: &TextInput) {
        let key = format!("{DRAFT_KEY}{mid}");
        self.saved(match input.as_str().trim() {
            "" => self.store.remove(&key),
            _ => self.store.set(&key, input.as_str()),
        });
    }

    /// The open chat's draft, at exit.
    fn save_open_draft(&self) {
        if let Some(mid) = &self.open {
            self.save_draft(mid, &self.input);
        }
    }

    /// Forget a chat (left, declined, removed).
    fn remove_chat(&mut self, mid: &str) {
        let selected = self.selected_mid();
        if let Some(task) = self
            .receipts
            .forget(mid)
            .and_then(|t| self.receipt_tasks.remove(&t))
        {
            task.abort();
        }
        self.retry.forget(&Job::Receipt(mid.to_owned()));
        self.chats.retain(|c| c.mid != mid);
        self.pinned.retain(|p| p != mid);
        if self.unread_divider.as_ref().is_some_and(|(chat, _)| chat == mid) {
            self.unread_divider = None;
            self.jump_unread = false;
        }
        self.messages.remove(mid);
        for set in [
            &mut self.stale,
            &mut self.resync,
            &mut self.older_loading,
            &mut self.no_older,
        ] {
            set.remove(mid);
        }
        self.read_marks.remove(mid);
        self.typing.remove(mid);
        self.drafts.remove(mid);
        self.saved(self.store.remove(&format!("{DRAFT_KEY}{mid}")));
        self.saved(self.store.delete_chat(mid));
        if self.open.as_deref() == Some(mid) {
            self.open = None;
            self.sel = None;
            self.replying = None;
            self.input.clear();
            self.focus = Focus::Chats;
            // The viewer only ever shows the open chat's media.
            if self.viewer.is_some() {
                self.close_viewer();
            }
        }
        self.sort_chats(selected);
        let visible_len = self.list_rows().len();
        if self.list.selected().is_none_or(|i| i >= visible_len) {
            self.list.select((visible_len > 0).then_some(0));
        }
    }

    /// Re-read the active pin list (pins and name) from the store. Returns
    /// false, with the error in the status line, when it cannot be read.
    fn reload_pins(&mut self) -> bool {
        let lists = match self.store.pin_profiles() {
            Ok(lists) => lists,
            Err(e) => {
                self.status = format!("could not load pin lists: {e}");
                return false;
            }
        };
        self.pin_list = (lists.len() > 1)
            .then(|| lists.into_iter().find(|p| p.active).map(|p| p.name))
            .flatten();
        match self.store.pinned_chats() {
            Ok(pins) => {
                self.pinned = pins;
                true
            }
            Err(e) => {
                self.status = format!("could not refresh pinned chats: {e}");
                false
            }
        }
    }

    fn set_pinned(&mut self, mid: &str, pinned: bool) {
        if self.chats.iter().any(|c| c.mid == mid && c.invited) {
            self.status = "invitations cannot be pinned".into();
            return;
        }
        if let Err(e) = self.store.set_pinned(mid, pinned) {
            self.status = format!("could not {} chat: {e}", if pinned { "pin" } else { "unpin" });
            return;
        }
        if !self.reload_pins() {
            return;
        }
        let selected = self.selected_mid();
        if pinned && selected.as_deref() == Some(mid) {
            // Pinning the selected chat moves it up; the selection stays at the
            // same list position so the next chat down is selected.
            let index = self.list.selected();
            self.sort_chats(None);
            let visible_len = self.list_rows().len();
            self.list
                .select(index.map(|i| i.min(visible_len.saturating_sub(1))));
        } else {
            self.sort_chats(selected);
        }
        self.status = format!(
            "{} {}",
            if pinned { "pinned" } else { "unpinned" },
            self.name(mid)
        );
    }

    fn move_pin(&mut self, mid: &str, delta: i32) {
        if self.chats.iter().any(|c| c.mid == mid && c.invited) {
            self.status = "invitations cannot be moved".into();
            return;
        }
        if !self.is_pinned(mid) {
            self.status = "chat is not pinned".into();
            return;
        }
        match self.store.move_pinned(mid, delta) {
            Ok(true) => {}
            Ok(false) => {
                self.status = "pin cannot move further".into();
                return;
            }
            Err(e) => {
                self.status = format!("could not move pin: {e}");
                return;
            }
        }
        if !self.reload_pins() {
            return;
        }
        let selected = self.selected_mid();
        self.sort_chats(selected);
        self.status = format!(
            "moved {} pin {}",
            self.name(mid),
            if delta < 0 { "up" } else { "down" }
        );
    }

    /// How far each member (other than us) is known to have seen `chat`:
    /// their read receipts, and what their own messages and reactions imply.
    /// LINE answers no read history, so the latter is all there is for reads
    /// from before rune was listening.
    fn read_state(&self, chat: &str) -> HashMap<String, Seen> {
        let mut state: HashMap<String, Seen> = HashMap::new();
        for (reader, id) in self.read_marks.get(chat).into_iter().flatten() {
            if *reader != self.me {
                state.entry(reader.clone()).or_default().receipt = Some(id.clone());
            }
        }
        for m in self.messages.get(chat).into_iter().flatten() {
            if m.id.is_empty() {
                continue;
            }
            let writer = m.written.then_some((m.from.as_str(), SeenBy::WroteLater));
            let reactors = m.reactions.iter().map(|r| (r.from.as_str(), SeenBy::Reacted));
            for (who, how) in writer.into_iter().chain(reactors) {
                if who.is_empty() || who == self.me {
                    continue;
                }
                let seen = state.entry(who.to_owned()).or_default();
                if seen
                    .inferred
                    .as_ref()
                    .is_none_or(|(id, _)| message_id_cmp(&m.id, id).is_gt())
                {
                    seen.inferred = Some((m.id.clone(), how));
                }
            }
        }
        state
    }

    /// Who (other than us) has seen message `id` in `chat` as far as we know,
    /// and how we know, sorted by mid.
    fn readers(&self, chat: &str, id: &str) -> Vec<(String, SeenBy)> {
        readers_in(&self.read_state(chat), id)
    }

    fn find_msg(&self, id: &str) -> Option<&UiMsg> {
        self.messages.values().flatten().find(|m| m.id == id)
    }

    /// The newest message of `chat` older than `id` that is still there
    /// (not unsent), from what is loaded or else the store.
    fn read_before(&self, chat: &str, id: &str) -> Option<String> {
        let older = |m: &str| message_id_cmp(m, id).is_lt();
        if let Some(m) = self.messages.get(chat).and_then(|list| {
            list.iter()
                .filter(|m| !m.unsent && older(&m.id))
                .max_by(|a, b| message_id_cmp(&a.id, &b.id))
        }) {
            return Some(m.id.clone());
        }
        let stored = self.store.messages(chat, 20, Some(id)).unwrap_or_default();
        stored
            .iter()
            .rev()
            .find(|m| !m.unsent && older(&m.message.id))
            .map(|m| m.message.id.clone())
    }

    fn find_msg_mut(&mut self, id: &str) -> Option<&mut UiMsg> {
        self.messages.values_mut().flatten().find(|m| m.id == id)
    }

    /// Add older messages at the top of a chat.
    fn prepend(&mut self, chat: &str, older: Vec<UiMsg>) -> usize {
        let list = self.messages.entry(chat.to_owned()).or_default();
        let have: HashSet<String> = list.iter().map(|m| m.id.clone()).collect();
        let older: Vec<UiMsg> = older.into_iter().filter(|m| !have.contains(&m.id)).collect();
        let n = older.len();
        list.splice(0..0, older);
        if self.open.as_deref() == Some(chat) {
            self.sel = self.sel.map(|i| i + n);
        }
        n
    }

    /// Scrolled past the oldest loaded message: load more (store first, then LINE).
    fn load_older(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        if self.older_loading.contains(&chat) || self.no_older.contains(&chat) || self.loading.contains(&chat)
        {
            return;
        }
        let Some(list) = self.messages.get(&chat) else {
            return;
        };
        let Some(oldest) = list.iter().find(|m| !m.id.is_empty()).cloned() else {
            return;
        };
        let stored = self
            .store
            .messages(&chat, 100, Some(&oldest.id))
            .unwrap_or_default();
        if !stored.is_empty() {
            let ui = stored.iter().filter_map(stored_to_ui).collect();
            let n = self.prepend(&chat, ui);
            self.status = format!("{n} older messages");
            return;
        }
        self.older_loading.insert(chat.clone());
        self.status = "loading older messages…".into();
        spawn_older(
            self.client.clone(),
            self.tx.clone(),
            chat,
            oldest.id,
            oldest.delivered,
        );
    }

    /// Rebuild a chat's loaded messages from the store (time order), keeping
    /// at least `keep` of the newest, the selection, and anything not stored yet.
    fn reload_from_store(&mut self, chat: &str, keep: usize) {
        let selected = (self.open.as_deref() == Some(chat))
            .then(|| self.selected_msg().map(|m| m.id.clone()))
            .flatten();
        let stored = self.store.messages(chat, keep, None).unwrap_or_default();
        let mut all: Vec<UiMsg> = stored.iter().filter_map(stored_to_ui).collect();
        for m in self.messages.remove(chat).unwrap_or_default() {
            if m.id.is_empty() {
                all.push(m);
            }
        }
        self.messages.insert(chat.to_owned(), all);
        if let Some(id) = selected {
            self.sel = self.open_messages().iter().position(|m| m.id == id);
        }
    }

    /// Send any message, showing it once LINE accepted it.
    fn send_outgoing(&mut self, chat: String, out: OutgoingMessage, label: String) {
        self.status = "sending…".into();
        // What we send shows at the bottom: go there, as if scrolled back down.
        if self.open.as_deref() == Some(chat.as_str()) {
            self.scroll = 0;
        }
        let (client, tx, me) = (self.client.clone(), self.tx.clone(), self.me.clone());
        tokio::spawn(async move {
            match client.send_message(&out).await {
                Ok(m) => {
                    let mut m = client.decrypt_message(&m).await.unwrap_or(m);
                    fill_from_outgoing(&mut m, &out, &me);
                    let mut ui = to_ui(&m, None).unwrap_or_else(|| UiMsg {
                        id: m.id.clone(),
                        from: me.clone(),
                        time: m.created_time.unwrap_or_else(now_ms),
                        text: label,
                        raw: None,
                        reply_to: None,
                        forwarded: false,
                        reactions: Vec::new(),
                        unsent: false,
                        delivered: 0,
                        written: true,
                        plain: false,
                    });
                    ui.from = me;
                    let _ = tx.send(AppEvent::Sent(chat, Box::new(ui), Some(Box::new(m))));
                }
                Err(e) => drop(tx.send(AppEvent::Status(format!("send failed: {e}")))),
            }
        });
    }

    /// Desktop notification / bell for an incoming message, per the settings.
    fn maybe_notify(&self, chat: &str, m: &Message, e2ee_error: Option<&str>) {
        let p = &self.prefs;
        if p.notify == NotifyMode::Off || m.from == self.me {
            return;
        }
        // Replayed history after a restart is not news.
        if m.created_time.is_some_and(|t| now_ms() - t > NOTIFY_MAX_AGE_MS) {
            return;
        }
        let group = MidType::of(chat) != MidType::User;
        if group && !p.notify_groups {
            return;
        }
        if self.is_muted(chat) {
            return;
        }
        if self.open.as_deref() == Some(chat) && self.focused && !p.notify_focused {
            return;
        }
        let Some(gist) = notice::gist(m, e2ee_error) else {
            return;
        };
        let (preview, is_call) = (p.notify_preview, m.content_type == ct::CALL);
        let (sender, chat_mid) = (m.from.clone(), chat.to_owned());
        let mut mids = vec![sender.clone()];
        if group {
            mids.push(chat_mid.clone());
        }
        // The sender's picture, else (in a group) the group's.
        self.notify_later(&mids, mids.clone(), Some(chat_mid.clone()), move |names| {
            let about = notice::About {
                sender: names.get(&sender).map(String::as_str),
                group: group.then(|| names.get(&chat_mid).map(String::as_str)),
            };
            Some(notice::message_notice(&about, &gist, preview, is_call))
        });
    }

    /// The chat (or 1:1 contact) has LINE's notifications off.
    fn is_muted(&self, mid: &str) -> bool {
        self.chats.iter().any(|c| c.mid == mid && c.muted)
    }

    /// Show the notice `build` makes from the names of `mids` once the
    /// unknown ones are looked up (the app learns them too), with the first
    /// picture of `icon_from` as its icon. A lookup that finds `mute_key`
    /// muted cancels it.
    fn notify_later(
        &self,
        mids: &[String],
        icon_from: Vec<String>,
        mute_key: Option<String>,
        build: impl FnOnce(&HashMap<String, String>) -> Option<notice::Notice> + Send + 'static,
    ) {
        self.notify_later_as(self.prefs.notify, mids, icon_from, mute_key, build);
    }

    /// [`App::notify_later`] in the given mode rather than the settings'.
    fn notify_later_as(
        &self,
        mode: NotifyMode,
        mids: &[String],
        icon_from: Vec<String>,
        mute_key: Option<String>,
        build: impl FnOnce(&HashMap<String, String>) -> Option<notice::Notice> + Send + 'static,
    ) {
        let mut names = HashMap::new();
        let mut lookup = Vec::new();
        for mid in mids {
            match self.names.get(mid).filter(|n| !n.trim().is_empty()) {
                Some(n) => drop(names.insert(mid.clone(), n.clone())),
                // A group we left stays nameless: not asked again.
                None if resolvable(mid) && !self.looked_up.contains(mid) => lookup.push(mid.clone()),
                None => {}
            }
        }
        let pictures = icon_from
            .iter()
            .filter_map(|mid| Some((mid.clone(), self.picture_paths.get(mid)?.clone())))
            .collect();
        notice::spawn(
            self.client.clone(),
            self.tx.clone(),
            self.store.clone(),
            notice::Pending {
                mode,
                lookup,
                pictures,
                icon_from,
                mute_key,
                build: Box::new(build),
                names,
            },
        );
    }

    /// Notify about a ringing call (op 50). Unlike messages it ignores focus:
    /// the call has to be answered on the phone, now.
    fn notify_ringing(&self, op: &Operation) {
        if self.prefs.notify == NotifyMode::Off || op.r#type != op_type::NOTIFIED_RECEIVED_CALL {
            return;
        }
        // Replayed after a restart: the ringing is long over.
        if op
            .created_time()
            .is_some_and(|t| now_ms() - t > NOTIFY_MAX_AGE_MS)
        {
            return;
        }
        let params = [&op.param1, &op.param2];
        if params.into_iter().flatten().any(|mid| self.is_muted(mid)) {
            return;
        }
        let caller = Self::caller_of(op);
        let mids: Vec<String> = caller.iter().cloned().collect();
        self.notify_later(&mids, mids.clone(), caller.clone(), move |names| {
            Some(notice::ringing_notice(
                caller.as_ref().and_then(|c| names.get(c)).map(String::as_str),
            ))
        });
    }

    /// The calling user of a ringing-call operation.
    fn caller_of(op: &Operation) -> Option<String> {
        [&op.param1, &op.param2]
            .into_iter()
            .flatten()
            .find(|p| p.len() > 1 && p.starts_with(['u', 'U']))
            .cloned()
    }

    /// Apply one stream operation to the store and the screen.
    fn on_operation(&mut self, op: Operation) {
        // Asked before the op is stored: a message delivered again (a replay,
        // a full sync, or fetched with history first) is not news to notify
        // about or to hand to plugins.
        let fresh = match op.event() {
            OpEvent::Message(m) => m.id.is_empty() || !self.store.has_message(&m.id).unwrap_or(false),
            _ => false,
        };
        self.saved(self.store.apply(&op, &self.me, preview_of));
        match op.event() {
            OpEvent::Message(m) => {
                let chat = m.chat_mid(&self.me).to_owned();
                if self.typing.get(&chat).is_some_and(|(who, _)| *who == m.from) {
                    self.typing.remove(&chat);
                }
                if let Some(ui) = to_ui(m, op.e2ee_error.as_ref().map(|e| e.detail.as_str())) {
                    let mine = m.from == self.me;
                    let new = self
                        .messages
                        .get(&chat)
                        .is_none_or(|list| !list.iter().any(|x| x.id == m.id));
                    self.push_message(&chat, ui, !mine);
                    if new && fresh {
                        self.plugins_hear(&chat, m);
                    }
                    // A count-placed divider counts from the newest message:
                    // keep it on the same message as others arrive.
                    if new
                        && !mine
                        && let Some((divider_chat, Unread::Last(n))) = self.unread_divider.as_mut()
                        && *divider_chat == chat
                    {
                        *n += 1;
                    }
                    if mine {
                        // With receipts off, only when it hides nothing unread.
                        let on = self.read_receipts();
                        if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat)
                            && (on || c.unread == 0)
                        {
                            c.unread = 0;
                            c.last_seen = Some(m.id.clone());
                        }
                        self.sort_chats(self.selected_mid());
                    } else {
                        if fresh {
                            self.maybe_notify(&chat, m, op.e2ee_error.as_ref().map(|e| e.detail.as_str()));
                        }
                        if self.focused && self.open.as_deref() == Some(chat.as_str()) {
                            if self.read_receipts()
                                && let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat)
                            {
                                c.last_seen = Some(m.id.clone());
                            }
                            self.note_read(&chat, Some(&m.id));
                        }
                    }
                    self.persist_chat(&chat);
                }
            }
            OpEvent::Read {
                chat,
                reader,
                message_id,
            } => {
                // A chat not loaded yet reads its marks (this one included:
                // the store has applied the op) from the store when opened;
                // starting its map here would hide the stored ones.
                if reader != self.me
                    && let Some(marks) = self.read_marks.get_mut(chat)
                {
                    let newer = marks
                        .get(reader)
                        .is_none_or(|cur| message_id_cmp(message_id, cur).is_gt());
                    if newer {
                        marks.insert(reader.to_owned(), message_id.to_owned());
                    }
                }
            }
            OpEvent::ChatChecked { chat, message_id } => {
                // Read this far (by our request or on another device): the
                // receipts it covers are done.
                if self.receipts.acked(chat, message_id) {
                    self.persist_receipt(chat);
                }
                // The unread divider stays where it was put until another
                // chat is opened, however far the chat is read meanwhile.
                if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                    if c.last_id
                        .as_deref()
                        .is_none_or(|l| message_id_cmp(message_id, l).is_ge())
                    {
                        c.unread = 0;
                    }
                    c.last_seen = Some(message_id.to_owned());
                }
                self.persist_chat(chat);
                self.sort_chats(self.selected_mid());
            }
            OpEvent::Unsent { chat, message_id, .. } => {
                let (approved, held) = self.receipts.row(chat);
                if [approved, held].contains(&Some(message_id)) {
                    let fallback = self.read_before(chat, message_id);
                    if self.receipts.unsent(chat, message_id, fallback.as_deref()) {
                        self.persist_receipt(chat);
                    }
                }
                let mut was_last = false;
                if let Some(m) = self.find_msg_mut(message_id) {
                    m.unsent = true;
                    m.text = UNSENT_TEXT.into();
                    m.raw = None;
                }
                // The sender took it back: stop showing (and holding) its media.
                self.thumbs.remove(message_id);
                self.full.remove(message_id);
                self.full_lru.retain(|id| id != message_id);
                self.pending.remove(message_id);
                self.qr.remove(message_id);
                if self.viewer.as_deref() == Some(message_id) {
                    self.close_viewer();
                    self.status = "the message was unsent".into();
                }
                if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat)
                    && c.last_id.as_deref() == Some(message_id)
                {
                    c.preview = UNSENT_TEXT.into();
                    was_last = true;
                }
                if was_last {
                    self.persist_chat(chat);
                }
            }
            OpEvent::Reaction {
                message_id,
                from,
                reaction,
                ..
            } => {
                if let Some(m) = self.find_msg_mut(message_id) {
                    rune::models::apply_reaction(&mut m.reactions, from, reaction, now_ms());
                }
            }
            OpEvent::Typing { chat, from } => {
                if from != self.me {
                    let until = std::time::Instant::now() + Duration::from_secs(6);
                    self.typing.insert(chat.to_owned(), (from.to_owned(), until));
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(6100)).await;
                        let _ = tx.send(AppEvent::Tick);
                    });
                }
            }
            OpEvent::ChatUpdated { chat, .. } => {
                let (client, tx, mids) = (self.client.clone(), self.tx.clone(), vec![chat.to_owned()]);
                tokio::spawn(async move { resolve_names(&client, &tx, mids).await });
            }
            OpEvent::MemberJoined { chat, member } => {
                let m = self.members.entry(chat.to_owned()).or_default();
                if !m.iter().any(|x| x == member) {
                    m.push(member.to_owned());
                }
                if let Some(inv) = self.invitees.get_mut(chat) {
                    inv.retain(|x| x != member);
                }
            }
            OpEvent::MemberLeft { chat, member } => {
                if member == self.me {
                    self.remove_chat(chat);
                } else if let Some(m) = self.members.get_mut(chat) {
                    m.retain(|x| x != member);
                }
            }
            OpEvent::LeftChat { chat } => self.remove_chat(chat),
            OpEvent::Invited { chat } => {
                spawn_invite_check(
                    self.client.clone(),
                    self.tx.clone(),
                    chat.to_owned(),
                    self.me.clone(),
                );
            }
            OpEvent::InvitationAnswered { chat, accepted } => {
                if accepted {
                    if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                        c.invited = false;
                    }
                    self.persist_chat(chat);
                } else {
                    self.remove_chat(chat);
                }
            }
            OpEvent::ProfileUpdated { mid } => {
                if self.names.contains_key(mid) {
                    let (client, tx, mids) = (self.client.clone(), self.tx.clone(), vec![mid.to_owned()]);
                    self.avatars.remove(mid);
                    tokio::spawn(async move { resolve_names(&client, &tx, mids).await });
                }
            }
            OpEvent::Other => {
                if let Some(s) = self.call_notice(&op) {
                    self.notify_ringing(&op);
                    self.status = s;
                }
            }
        }
    }

    fn open_messages(&self) -> &[UiMsg] {
        self.open
            .as_ref()
            .and_then(|c| self.messages.get(c))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn selected_msg(&self) -> Option<&UiMsg> {
        self.sel.and_then(|i| self.open_messages().get(i))
    }

    /// Start (or reuse) the full download of a message, then run `after`.
    fn fetch_full(&mut self, msg: &UiMsg, after: After) {
        let Some(raw) = msg.raw.clone() else {
            self.status = "this message has no media".into();
            return;
        };
        match self.full.get(&msg.id) {
            Some(Load::Ready(_)) => self.finish(&msg.id.clone(), after),
            Some(Load::Loading) => {
                if after != After::Nothing {
                    self.pending.insert(msg.id.clone(), after);
                }
            }
            Some(Load::Failed(_)) | None => {
                self.full.insert(msg.id.clone(), Load::Loading);
                if after != After::Nothing {
                    self.pending.insert(msg.id.clone(), after);
                }
                self.status = "downloading…".into();
                spawn_full(self.client.clone(), self.tx.clone(), raw);
            }
        }
    }

    /// Pixels the largest inline thumbnail can show: thumbnails are kept no
    /// bigger. Text-cell pictures get twice their sub-cell grid, for a
    /// smoother downscale.
    fn thumb_cap(&self) -> (u32, u32) {
        let (cols, rows) = THUMB_SIZES[THUMB_SIZES.len() - 1];
        let (fw, fh) = match &self.picker {
            Some(p) => {
                let font = p.font_size();
                (u32::from(font.width), u32::from(font.height))
            }
            None => {
                let (gw, gh) = self.cell_mode.grid();
                (gw * 2, gh * 2)
            }
        };
        (u32::from(cols) * fw.max(1), u32::from(rows) * fh.max(1))
    }

    /// Keep a downloaded original, dropping the least recently used ones
    /// (never the one on screen) once they hold more than `FULL_CACHE_BYTES`.
    fn remember_full(&mut self, id: String, media: FullMedia) {
        self.full.insert(id.clone(), Load::Ready(media));
        self.full_lru.retain(|x| *x != id);
        self.full_lru.push(id);
        self.trim_full(FULL_CACHE_BYTES);
    }

    fn trim_full(&mut self, cap: usize) {
        let size = |app: &App, id: &str| match app.full.get(id) {
            Some(Load::Ready(m)) => m.data.len(),
            _ => 0,
        };
        let mut total: usize = self.full_lru.iter().map(|id| size(self, id)).sum();
        let mut i = 0;
        while total > cap && i + 1 < self.full_lru.len() {
            if self.viewer.as_ref() == Some(&self.full_lru[i]) {
                i += 1;
                continue;
            }
            let old = self.full_lru.remove(i);
            total -= size(self, &old);
            self.full.remove(&old);
        }
    }

    /// Save or open a downloaded original.
    fn finish(&mut self, id: &str, after: After) {
        let Some(Load::Ready(media)) = self.full.get(id) else {
            if after == After::OpenAnyway {
                self.status = "that download is no longer kept — open the message again".into();
            }
            return;
        };
        if after == After::Nothing {
            return;
        }
        let motion = is_motion_file(&media.file_name);
        if after == After::Open && !motion && runs_programs(&media.file_name) {
            let text = format!(
                "{} is a kind of file that can run programs on this computer. \
                 Open it only if you trust the sender and expected it.",
                safe_file_name(&media.file_name)
            );
            self.confirm(text, modal::Action::OpenAnyway(id.to_owned()));
            return;
        }
        let after = if after == After::OpenAnyway {
            After::Open
        } else {
            after
        };
        if after == After::Play && (media.note.is_some() || !motion) {
            self.status = if media.note.is_some() {
                "the original is no longer on LINE's server — nothing to play".into()
            } else {
                "this is not a video, animation or audio clip".into()
            };
            return;
        }
        // Motion media plays in mpv, whether asked to play or to open.
        let play = motion && matches!(after, After::Open | After::Play) && media.note.is_none();
        // Writing (and comparing with an earlier copy) can take a while for a
        // video, so it runs off the UI thread and reports on the status line.
        let (name, data, tx) = (media.file_name.clone(), media.data.clone(), self.tx.clone());
        self.status = format!(
            "{} {}…",
            if after == After::Save { "saving" } else { "opening" },
            safe_file_name(&name)
        );
        tokio::task::spawn_blocking(move || {
            // Only `s` keeps a copy in Downloads; opening and playing use the cache folder.
            let written = if after == After::Save {
                store_download(&name, &data)
            } else {
                store_for_viewing(&name, &data)
            };
            let result = written.and_then(|p| {
                if after == After::Open && !play {
                    open_external(&p)?;
                }
                Ok(p)
            });
            let status = match (result, after) {
                (Ok(p), _) if play => play_media(&tx, &p),
                (Ok(p), After::Open) => format!("opened {}", p.display()),
                (Ok(p), _) => format!("saved to {}", p.display()),
                (Err(e), _) => format!("could not save: {e}"),
            };
            let _ = tx.send(AppEvent::Status(status));
        });
    }

    /// Play the selected message's video or audio in mpv.
    fn play_selected(&mut self) {
        let Some(m) = self.selected_msg().cloned() else {
            return;
        };
        self.fetch_full(&m, After::Play);
    }

    /// The media in the viewer can be played in mpv (video, audio).
    fn viewer_plays(&self) -> bool {
        let Some(id) = &self.viewer else { return false };
        let downloaded_motion = matches!(self.full.get(id), Some(Load::Ready(f)) if is_motion_file(&f.file_name) && f.note.is_none());
        downloaded_motion || self.find_msg(id).is_some_and(UiMsg::plays)
    }

    /// The viewer shows an animation (more than one frame decoded).
    fn viewer_animates(&self) -> bool {
        self.anim
            .as_ref()
            .is_some_and(|p| self.viewer.as_ref() == Some(&p.id) && p.is_animated())
    }

    /// Keep the viewer's animation in step with what it shows: start
    /// playing a downloaded GIF/APNG/WebP, stop when the viewer moves on.
    fn sync_anim(&mut self) {
        let want = self.viewer.as_ref().filter(|id| {
            matches!(self.full.get(*id), Some(Load::Ready(m))
                if m.picture.is_some() && m.note.is_none() && anim::may_animate(&m.data))
        });
        if self.anim.as_ref().map(|p| &p.id) == want {
            return;
        }
        self.anim = want.and_then(|id| match self.full.get(id) {
            Some(Load::Ready(m)) => Some(anim::Player::start(id.clone(), m.data.clone(), self.tx.clone())),
            _ => None,
        });
    }

    fn open_viewer(&mut self) {
        let Some(m) = self.selected_msg().cloned() else {
            return;
        };
        if m.media_kind().is_none() {
            self.status = "this message has no media".into();
            return;
        }
        self.viewer = Some(m.id.clone());
        self.needs_clear = true;
        self.follow_sel = true;
        self.fetch_full(&m, After::Nothing);
    }

    /// Move the selection to the previous/next media message (viewer ←/→).
    fn step_media(&mut self, forward: bool) {
        let msgs = self.open_messages();
        let Some(cur) = self.sel else { return };
        let found = if forward {
            (cur + 1..msgs.len()).find(|&i| msgs[i].media_kind().is_some())
        } else {
            (0..cur).rev().find(|&i| msgs[i].media_kind().is_some())
        };
        if let Some(i) = found {
            self.sel = Some(i);
            self.open_viewer();
        }
    }

    /// Insert a message into a chat (deduplicated by id) and bump the chat.
    /// A chat whose messages are not loaded only has its entry bumped: the
    /// message is in the store already and shows, after the stored history,
    /// once the chat is opened.
    fn push_message(&mut self, chat: &str, msg: UiMsg, unread: bool) {
        let preview = msg.text.clone();
        let time = msg.time;
        let id = (!msg.id.is_empty()).then(|| msg.id.clone());
        match self.messages.get_mut(chat) {
            Some(list) => {
                if list.iter().any(|m| m.id == msg.id && !msg.id.is_empty()) {
                    return;
                }
                list.push(msg);
            }
            None => {
                // No list to deduplicate against: a message no newer than the
                // chat's newest is not news (e.g. replayed).
                let known = self
                    .chats
                    .iter()
                    .find(|c| c.mid == chat)
                    .and_then(|c| c.last_id.as_deref());
                if let (Some(id), Some(newest)) = (id.as_deref(), known)
                    && message_id_cmp(id, newest).is_le()
                {
                    return;
                }
                // The open chat is fetching its history (nothing was stored):
                // show the message now, the history merges in when it lands.
                if self.open.as_deref() == Some(chat) {
                    self.messages.insert(chat.to_owned(), vec![msg]);
                }
            }
        }
        let selected = self.selected_mid();
        // The open chat counts too while receipts are off: nothing there is marked read.
        let counted = unread && (self.open.as_deref() != Some(chat) || !self.read_receipts());
        match self.chats.iter_mut().find(|c| c.mid == chat) {
            Some(c) => {
                if time >= c.last_time {
                    c.preview = preview;
                    c.last_time = time;
                    c.last_id = id.or(c.last_id.take());
                }
                if counted {
                    c.unread += 1;
                }
            }
            None => {
                self.chats.push(ChatEntry {
                    mid: chat.to_owned(),
                    preview,
                    last_time: time,
                    unread: u32::from(counted),
                    last_id: id,
                    ..Default::default()
                });
                self.want_names([chat.to_owned()]);
            }
        }
        self.sort_chats(selected);
    }

    /// Status line for the (legacy, never observed live) ringing-call
    /// operations; rune cannot take calls, so it points at the phone.
    fn call_notice(&self, op: &Operation) -> Option<String> {
        let caller = Self::caller_of(op).map(|mid| self.name(&mid));
        match op.r#type {
            op_type::NOTIFIED_RECEIVED_CALL => Some(match caller {
                Some(who) => format!("📞 {who} is calling — answer on your phone"),
                None => "📞 incoming call — answer on your phone".into(),
            }),
            op_type::CANCEL_CALL => Some(match caller {
                Some(who) => format!("📞 call from {who} ended"),
                None => "📞 call ended".into(),
            }),
            _ => None,
        }
    }

    fn on_invitations(&mut self, chats: Vec<Chat>, full: bool) {
        let selected = self.selected_mid();
        if full {
            // Invitations answered elsewhere since the last run disappear.
            let pending: HashSet<&str> = chats.iter().map(|c| c.chat_mid.as_str()).collect();
            let gone: Vec<String> = self
                .chats
                .iter()
                .filter(|c| c.invited && !pending.contains(c.mid.as_str()))
                .map(|c| c.mid.clone())
                .collect();
            for mid in gone {
                self.chats.retain(|c| c.mid != mid);
                self.pinned.retain(|p| p != &mid);
                self.saved(self.store.delete_chat(&mid));
            }
        }
        let new: Vec<String> = chats
            .iter()
            .filter(|c| !self.chats.iter().any(|e| e.mid == c.chat_mid && e.invited))
            .map(|c| c.chat_mid.clone())
            .collect();
        for chat in chats {
            self.learn_pictures(HashMap::from([(chat.chat_mid.clone(), chat.picture())]));
            self.names.insert(chat.chat_mid.clone(), chat.chat_name.clone());
            match self.chats.iter_mut().find(|c| c.mid == chat.chat_mid) {
                Some(c) => *c = ChatEntry::invitation(&chat),
                None => self.chats.push(ChatEntry::invitation(&chat)),
            }
            self.persist_chat(&chat.chat_mid);
        }
        if !full && self.prefs.notify != NotifyMode::Off {
            for mid in new.into_iter().filter(|m| !self.is_muted(m)) {
                let mids = vec![mid.clone()];
                self.notify_later(&mids, mids.clone(), Some(mid.clone()), move |names| {
                    Some(notice::invitation_notice(names.get(&mid).map(String::as_str)))
                });
            }
        }
        self.sort_chats(selected);
        let visible_len = self.list_rows().len();
        if self.list.selected().is_none() && visible_len > 0 {
            self.list.select(Some(0));
        }
    }

    /// Join (`accept`) or decline a group invitation.
    fn answer_invite_for(&mut self, mid: String, accept: bool) {
        let name = self.name(&mid);
        self.status = if accept {
            format!("joining {name}…")
        } else {
            format!("declining {name}…")
        };
        let (client, tx) = (self.client.clone(), self.tx.clone());
        tokio::spawn(async move {
            let res = if accept {
                client.accept_chat_invitation(&mid).await
            } else {
                client.reject_chat_invitation(&mid).await
            };
            let _ = tx.send(AppEvent::InviteAnswered(
                mid,
                accept,
                res.map_err(|e| e.to_string()),
            ));
        });
    }

    fn on_invite_answered(&mut self, mid: String, accepted: bool, res: Result<(), String>) {
        let name = self.name(&mid);
        if let Err(e) = res {
            let verb = if accepted { "accept" } else { "decline" };
            self.status = format!("could not {verb} {name}: {e}");
            return;
        }
        let selected = self.selected_mid();
        if accepted {
            if let Some(c) = self.chats.iter_mut().find(|c| c.mid == mid) {
                c.invited = false;
                c.preview = "You joined".into();
                c.last_time = now_ms();
            }
            self.persist_chat(&mid);
            self.status = format!("joined {name}");
        } else {
            self.chats.retain(|c| c.mid != mid);
            self.pinned.retain(|p| p != &mid);
            self.saved(self.store.delete_chat(&mid));
            self.status = format!("declined {name}");
        }
        self.sort_chats(selected);
        let visible_len = self.list_rows().len();
        if self.list.selected().is_none_or(|i| i >= visible_len) {
            self.list.select((visible_len > 0).then_some(0));
        }
    }

    fn sort_chats(&mut self, keep_selected: Option<String>) {
        // Invitations stay first, then pinned chats, then unread chats, then
        // recent activity. Pinned chats with unread messages float to the top
        // of the pinned section (among themselves in pin order) until they are
        // read, then drop back to their pin position: the pin order itself
        // never changes. Unpinned unread (not muted) chats likewise sit above
        // the other unpinned ones, newest first, until read; nothing is stored.
        self.chats.sort_by_cached_key(|c| {
            let pin_position = self.pinned.iter().position(|mid| mid == &c.mid);
            let pinned = pin_position.is_some() && !c.invited;
            (
                std::cmp::Reverse(c.invited),
                std::cmp::Reverse(pinned),
                std::cmp::Reverse(pinned && c.unread > 0),
                pin_position.unwrap_or(usize::MAX),
                std::cmp::Reverse(!pinned && c.wants_attention()),
                std::cmp::Reverse(c.last_time),
                // Never the name (display-only, may collide): the mid makes
                // equal-time chats deterministic.
                c.mid.clone(),
            )
        });
        if let Some(sel) = keep_selected {
            self.select_chat_mid(Some(&sel));
        }
    }

    /// A load succeeded: its retry schedule (only its own) is dropped.
    fn loaded(&mut self, job: &Job) {
        if self.retry.succeeded(job) && *job == Job::Stream {
            self.status = "reconnected".into();
        }
    }

    /// Queue a failed load to be tried again (quietly: its view shows the failure).
    fn retry_later(&mut self, job: Job) {
        self.retry.failed(job, Instant::now());
    }

    /// A background load failed: say so and queue it.
    fn on_failed(&mut self, job: Job, msg: String) {
        if let Job::History(chat) = &job {
            // Still `stale`/`resync`: the next attempt (or opening it again) fetches it.
            self.loading.remove(chat);
        }
        let wait = self.retry.failed(job, Instant::now());
        self.status = format!("{msg} — retrying in {}s", wait.as_secs().max(1));
    }

    /// Start the picture downloads the last frame asked for, most urgent first,
    /// as far as free slots allow (the rest are asked for again by the next frame).
    fn start_media(&mut self) {
        for (key, start) in self.media.take() {
            match (key, start) {
                (MediaKey::Thumb(id), MediaStart::Thumb(m)) => {
                    self.thumbs.insert(id, Load::Loading);
                    spawn_thumb(self.client.clone(), self.tx.clone(), *m, self.thumb_cap());
                }
                (MediaKey::Avatar(mid), MediaStart::Avatar(path)) => {
                    self.avatars.insert(mid.clone(), Load::Loading);
                    avatar::spawn_avatar(
                        self.client.clone(),
                        self.tx.clone(),
                        self.store.clone(),
                        mid,
                        path,
                    );
                }
                (key, _) => self.media.done(&key),
            }
        }
    }

    /// Start the queued loads that are due. Per-item loads (avatars,
    /// thumbnails) only drop their failure: whatever still draws them asks
    /// again, and what is no longer on screen is not fetched.
    fn run_retries(&mut self) {
        self.run_retries_at(Instant::now());
    }

    fn run_retries_at(&mut self, now: Instant) {
        for job in self.retry.take_due(now) {
            match job {
                Job::ChatList => spawn_chat_list(self.client.clone(), self.tx.clone()),
                Job::Invitations => {
                    let (client, tx) = (self.client.clone(), self.tx.clone());
                    tokio::spawn(async move { fetch_invitations(&client, &tx).await });
                }
                Job::Contacts => {
                    spawn_contacts(self.client.clone(), self.tx.clone(), self.store.clone(), true)
                }
                Job::Stream => {
                    let from = self.store.stream_revision().ok().flatten();
                    spawn_stream(self.client.clone(), self.tx.clone(), from);
                }
                Job::History(chat) => {
                    if self.open.as_deref() == Some(chat.as_str()) && self.loading.insert(chat.clone()) {
                        spawn_history(self.client.clone(), self.tx.clone(), self.store.clone(), chat);
                    } else if !self.loading.contains(&chat) {
                        // Not open any more: opening it fetches it.
                        self.retry.forget(&Job::History(chat));
                    }
                }
                Job::Avatar(mid) => {
                    if matches!(self.avatars.get(&mid), Some(Load::Failed(_))) {
                        self.avatars.remove(&mid);
                    } else {
                        self.retry.forget(&Job::Avatar(mid));
                    }
                }
                Job::Thumb(id) => {
                    if matches!(self.thumbs.get(&id), Some(Load::Failed(_))) {
                        self.thumbs.remove(&id);
                    } else {
                        self.retry.forget(&Job::Thumb(id));
                    }
                }
                Job::Profile(mid) => {
                    if self.profile.last() == Some(&mid)
                        && matches!(self.profiles.get(&mid), Some(Load::Failed(_)))
                    {
                        self.profiles.insert(mid.clone(), Load::Loading);
                        self.fetch_profile(&mid, true);
                    } else {
                        // Opening the page tries a failed load again anyway.
                        self.retry.forget(&Job::Profile(mid));
                    }
                }
                Job::MyProfile => {
                    if matches!(self.my_profile, Some(Load::Failed(_))) {
                        self.my_profile = None;
                    }
                    if matches!(&self.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Account) {
                        self.fetch_tab_data(settings::Tab::Account);
                    } else {
                        self.retry.forget(&Job::MyProfile);
                    }
                }
                Job::Receipt(chat) => {
                    if self.receipts.backing_off(&chat) {
                        self.receipts.ready(&chat);
                        self.flush_receipts();
                    } else {
                        self.retry.forget(&Job::Receipt(chat));
                    }
                }
                Job::ServerSettings => {
                    if matches!(&self.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Notifications)
                    {
                        self.fetch_tab_data(settings::Tab::Notifications);
                    } else {
                        self.retry.forget(&Job::ServerSettings);
                    }
                }
            }
        }
    }

    /// Names learned from any source: shown everywhere (chat list, messages,
    /// open profile pages) and kept for the next start.
    fn learn_names(&mut self, n: HashMap<String, String>) {
        let selected = self.selected_mid();
        let profile_name = n.get(&self.me).cloned();
        let changed: Vec<String> = n
            .iter()
            .filter(|(mid, name)| self.names.get(*mid) != Some(name))
            .map(|(mid, _)| mid.clone())
            .collect();
        self.names.extend(n);
        if let Some(name) = profile_name {
            self.saved(self.store.set_profile_name(&name));
        }
        for mid in &changed {
            self.persist_chat(mid);
            if mid != &self.me
                && let Some(name) = self.names.get(mid).filter(|n| !n.is_empty())
                && let Some(Load::Ready(p)) = self.profiles.get_mut(mid)
            {
                p.name = name.clone();
            }
        }
        self.persist_users(&changed);
        self.select_chat_mid(selected.as_deref());
    }

    /// Picture paths learned from any source (`None`: no picture). A
    /// changed picture drops the old avatar (fetched again where drawn) and
    /// refreshes a profile page that shows it.
    fn learn_pictures(&mut self, p: HashMap<String, Option<String>>) {
        self.learn_pictures_from(p, None);
    }

    /// [`App::learn_pictures`], except for the profile page of `fresh`,
    /// which already shows the new picture.
    fn learn_pictures_from(&mut self, p: HashMap<String, Option<String>>, fresh: Option<&str>) {
        for (mid, path) in p {
            let path = path.filter(|p| !p.trim().is_empty());
            if self.picture_paths.get(&mid) == path.as_ref() {
                continue;
            }
            match path {
                Some(path) => drop(self.picture_paths.insert(mid.clone(), path)),
                None => {
                    self.picture_paths.remove(&mid);
                    // `persist_users` keeps a stored picture; this one is gone.
                    if let Ok(Some(u)) = self.store.user(&mid)
                        && u.picture.is_some()
                    {
                        self.saved(self.store.upsert_user(&UserRow { picture: None, ..u }));
                    }
                }
            }
            self.avatars.remove(&mid);
            self.retry.forget(&Job::Avatar(mid.clone()));
            self.persist_chat(&mid);
            self.persist_users(std::slice::from_ref(&mid));
            if fresh == Some(mid.as_str()) {
                continue;
            }
            if self.profile.contains(&mid) {
                if matches!(self.profiles.get(&mid), Some(Load::Ready(_))) {
                    // Shown as it is until the new one arrives.
                    self.fetch_profile(&mid, false);
                }
            } else {
                self.profiles.remove(&mid);
            }
        }
    }

    fn on_event(&mut self, ev: AppEvent) {
        self.handle_event(ev);
        self.settle_modal();
    }

    /// A screen opened from another one (`modal_parent`) has closed: the
    /// other one comes back, unless the app went elsewhere meanwhile.
    fn settle_modal(&mut self) {
        if self.modal.is_some() {
            return;
        }
        if let Some(parent) = self.modal_parent.take()
            && self.profile.is_empty()
            && !self.quit
        {
            if let Modal::Settings(s) = &*parent {
                // What it shows may have changed meanwhile (e.g. a new display name).
                let tab = s.tab;
                self.fetch_tab_data(tab);
            }
            self.modal = Some(*parent);
        }
    }

    fn handle_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Chats(server) => {
                self.loaded(&Job::ChatList);
                self.on_server_chats(server)
            }
            AppEvent::Failed(job, msg) => self.on_failed(job, msg),
            AppEvent::PluginLoaded(done) => {
                let fx = self.plugins.loaded(*done);
                self.apply_plugin_effects(fx);
            }
            AppEvent::PluginFiles(ids) => {
                self.refresh_plugins(&ids);
            }
            AppEvent::PluginInstall(ev) => self.on_plugin_install(ev),
            AppEvent::MoreChats(res) => self.on_more_chats(res),
            AppEvent::Names(n) => self.learn_names(n),
            AppEvent::Pictures(p) => self.learn_pictures(p),
            AppEvent::LookedUp(mids) => self.looked_up.extend(mids),
            AppEvent::ProfilePicture(mid, path) => {
                self.looked_up.insert(mid.clone());
                self.learn_pictures_from(HashMap::from([(mid.clone(), path)]), Some(&mid));
            }
            AppEvent::Avatar(mid, img) => self.on_avatar(mid, img),
            AppEvent::History(chat, msgs) => {
                self.loaded(&Job::History(chat.clone()));
                self.loading.remove(&chat);
                self.stale.remove(&chat);
                self.resync.remove(&chat);
                self.saved(self.store.put_messages(&chat, &msgs, None));
                // The store now has everything (stored + fetched + live), in order;
                // anything shown but not stored (no id yet) stays.
                let keep = self.messages.get(&chat).map_or(0, Vec::len).max(200);
                self.reload_from_store(&chat, keep);
                if self.status == "loading…" {
                    self.status.clear();
                }
            }
            AppEvent::Older(chat, msgs, done) => {
                self.older_loading.remove(&chat);
                self.saved(self.store.put_messages(&chat, &msgs, None));
                let ui: Vec<UiMsg> = msgs.iter().filter_map(|m| to_ui(m, None)).collect();
                let n = self.prepend(&chat, ui);
                if done {
                    // Nothing older on the server (the store was read before asking it).
                    self.no_older.insert(chat);
                }
                self.status = if n == 0 && done {
                    "this is the beginning of the conversation".into()
                } else {
                    format!("{n} older messages")
                };
            }
            AppEvent::Incoming(op) => {
                self.loaded(&Job::Stream);
                self.on_operation(*op)
            }
            AppEvent::Control(name, data) => {
                self.loaded(&Job::Stream);
                if let Some(r) = data.get("nextRevision").and_then(|v| match v {
                    serde_json::Value::String(s) => s.parse().ok(),
                    v => v.as_i64(),
                }) {
                    self.saved(self.store.set_stream_revision(r));
                }
                if name == "fullSync" {
                    // Too much happened while away to replay: re-read the chat list;
                    // chats with unseen messages are fetched when opened.
                    self.status = "catching up…".into();
                    self.resync.extend(self.chats.iter().map(|c| c.mid.clone()));
                    self.messages
                        .retain(|chat, _| self.open.as_deref() == Some(chat.as_str()));
                    if let Some(open) = self.open.clone()
                        && self.loading.insert(open.clone())
                    {
                        spawn_history(self.client.clone(), self.tx.clone(), self.store.clone(), open);
                    }
                    spawn_chat_list(self.client.clone(), self.tx.clone());
                }
            }
            AppEvent::Sent(chat, msg, stored) => {
                if let Some(m) = stored {
                    self.saved(self.store.put_message(&chat, &m, None));
                }
                self.push_message(&chat, *msg, false);
                // With receipts off, only when it hides nothing unread.
                let on = self.read_receipts();
                if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat)
                    && (on || c.unread == 0)
                {
                    c.unread = 0;
                    c.last_seen = c.last_id.clone();
                }
                self.persist_chat(&chat);
                self.sort_chats(self.selected_mid());
                self.status = "sent".into();
            }
            AppEvent::Status(s) => self.status = s,
            AppEvent::Found(found) => self.on_found(*found),
            AppEvent::Terminate => self.quit = true,
            AppEvent::Invitations(chats, full) => {
                if full {
                    self.loaded(&Job::Invitations);
                }
                self.on_invitations(chats, full)
            }
            AppEvent::InviteAnswered(mid, accepted, res) => self.on_invite_answered(mid, accepted, res),
            AppEvent::ReactionFailed(id, before, tried, e) => self.on_reaction_failed(&id, before, tried, e),
            AppEvent::Profile(mid, res) => self.on_profile_loaded(mid, res),
            AppEvent::Members(m) => {
                let selected = self.selected_mid();
                for (chat, (members, invitees)) in m {
                    self.members.insert(chat.clone(), members);
                    self.invitees.insert(chat, invitees);
                }
                self.select_chat_mid(selected.as_deref());
            }
            AppEvent::Mutes(m) => {
                for (mid, muted) in m {
                    if let Some(c) = self.chats.iter_mut().find(|c| c.mid == mid)
                        && c.muted != muted
                    {
                        c.muted = muted;
                        self.persist_chat(&mid);
                    }
                }
                // A muted unread chat stops floating (and an unmuted one starts).
                self.sort_chats(self.selected_mid());
            }
            AppEvent::Contacts(list) => {
                self.loaded(&Job::Contacts);
                self.saved(self.store.upsert_contacts(&list));
                let mut mutes = HashMap::new();
                let mut names = HashMap::new();
                let mut pictures = HashMap::new();
                for c in &list {
                    self.contacts.insert(c.mid.clone(), c.name().to_owned());
                    names.insert(c.mid.clone(), c.name().to_owned());
                    pictures.insert(c.mid.clone(), c.picture());
                    mutes.insert(c.mid.clone(), c.notifications_disabled());
                }
                // Through the same paths as any lookup, so every view picks the changes up.
                self.looked_up.extend(names.keys().cloned());
                self.learn_names(names);
                self.learn_pictures(pictures);
                self.handle_event(AppEvent::Mutes(mutes));
            }
            AppEvent::Unsent(chat, id, res) => match res {
                Ok(()) => {
                    self.saved(self.store.mark_unsent(&id));
                    let op = Operation {
                        r#type: op_type::DESTROY_MESSAGE,
                        param1: Some(chat),
                        param2: Some(id),
                        ..Default::default()
                    };
                    self.on_operation(op);
                    self.status = "message unsent".into();
                }
                Err(e) => self.status = format!("could not unsend: {e}"),
            },
            AppEvent::Left(chat, res) => match res {
                Ok(()) => {
                    self.status = format!("left {}", self.name(&chat));
                    self.remove_chat(&chat);
                }
                Err(e) => self.status = format!("could not leave: {e}"),
            },
            AppEvent::Muted(chat, muted, res) => match res {
                Ok(()) => {
                    if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                        c.muted = muted;
                    }
                    self.persist_chat(&chat);
                    self.sort_chats(self.selected_mid());
                    let verb = if muted { "muted" } else { "notifications on for" };
                    self.status = format!("{verb} {}", self.name(&chat));
                }
                Err(e) => self.status = format!("could not change notifications: {e}"),
            },
            AppEvent::GroupCreated(res, invited) => match res {
                Ok(chat) => {
                    let mid = chat.chat_mid.clone();
                    self.names.insert(mid.clone(), chat.chat_name.clone());
                    self.learn_pictures(HashMap::from([(mid.clone(), chat.picture())]));
                    // Prefer what the server says; it may already list members/invitees.
                    let mut members = chat.member_mids();
                    if !members.contains(&self.me) {
                        members.insert(0, self.me.clone());
                    }
                    let invitees = match chat.invitee_mids() {
                        v if v.is_empty() => invited,
                        v => v,
                    };
                    let invitees: Vec<String> =
                        invitees.into_iter().filter(|m| !members.contains(m)).collect();
                    self.members.insert(mid.clone(), members);
                    self.invitees.insert(mid.clone(), invitees);
                    let selected = self.selected_mid();
                    // A stream op or reload may have added it already: never list it twice.
                    if !self.chats.iter().any(|c| c.mid == mid) {
                        self.chats.push(ChatEntry {
                            mid: mid.clone(),
                            preview: "You created the group".into(),
                            last_time: chat.created_time().unwrap_or_else(now_ms),
                            ..Default::default()
                        });
                    }
                    self.sort_chats(selected);
                    self.messages.entry(mid.clone()).or_default();
                    self.persist_chat(&mid);
                    self.status = format!("created {}", chat.chat_name);
                    self.enter_chat(mid);
                }
                Err(e) => self.status = format!("could not create the group: {e}"),
            },
            AppEvent::Invited(chat, mids, res) => match res {
                Ok(()) => {
                    let n = mids.len();
                    self.invitees.entry(chat.clone()).or_default().extend(mids);
                    self.status = format!("invited {n} to {}", self.name(&chat));
                }
                Err(e) => self.status = format!("could not invite: {e}"),
            },
            AppEvent::Renamed(chat, name, res) => match res {
                Ok(()) => {
                    self.names.insert(chat.clone(), name.clone());
                    self.persist_chat(&chat);
                    self.status = format!("renamed to {name}");
                }
                Err(e) => self.status = format!("could not rename: {e}"),
            },
            AppEvent::StickerPackages(res) => self.on_sticker_packages(res),
            AppEvent::EmojiPackages(res) => self.on_emoji_packages(res),
            AppEvent::StickerPic(id, res) => {
                let slot = match res {
                    Ok(img) => Load::Ready(Picture::new(img, self.picker.as_ref())),
                    Err(e) => Load::Failed(e),
                };
                self.sticker_pics.insert(id, slot);
            }
            AppEvent::FilePreview(path, res) => {
                let slot = match res {
                    Ok(img) => Load::Ready(Picture::new(img, self.picker.as_ref())),
                    Err(e) => Load::Failed(e),
                };
                self.file_previews.insert(path, slot);
            }
            AppEvent::UploadDone(chat, label, err) => {
                if let Some(i) = self.uploads.iter().position(|u| u.0 == chat && u.1 == label) {
                    self.uploads.remove(i);
                }
                if let Some(e) = err {
                    self.status = format!("upload failed: {e}");
                }
            }
            AppEvent::ServerSettings(res) => {
                self.server_settings = Some(match res {
                    Ok(v) => {
                        self.loaded(&Job::ServerSettings);
                        Load::Ready(v)
                    }
                    Err(e) => {
                        self.retry_later(Job::ServerSettings);
                        Load::Failed(e)
                    }
                });
            }
            AppEvent::ServerSettingSaved(field, value, res) => match res {
                Ok(()) => self.status = "saved to your LINE account".into(),
                Err(e) => {
                    if let Some(Load::Ready(v)) = self.server_settings.as_mut() {
                        v[field] = serde_json::Value::Bool(!value);
                    }
                    self.status = format!("LINE refused the change: {e}");
                }
            },
            AppEvent::MyProfile(res) => {
                self.my_profile = Some(match res {
                    Ok(p) => {
                        self.loaded(&Job::MyProfile);
                        Load::Ready((p.display_name, p.status_message))
                    }
                    Err(e) => {
                        self.retry_later(Job::MyProfile);
                        Load::Failed(e)
                    }
                });
            }
            AppEvent::CoverSet(res) => match res {
                Ok(()) => {
                    self.status = "profile cover updated".into();
                    let me = self.me.clone();
                    self.reload_profile(&me);
                }
                Err(e) => self.status = format!("could not change the cover: {e}"),
            },
            AppEvent::ProfileChanged => {
                self.my_profile = None;
                self.profiles.remove(&self.me);
                if matches!(&self.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Account) {
                    self.fetch_tab_data(settings::Tab::Account);
                }
            }
            AppEvent::Receipt(s, outcome) => self.on_receipt(s, outcome),
            AppEvent::Tick => {
                let now = std::time::Instant::now();
                self.typing.retain(|_, (_, until)| *until > now);
            }
            AppEvent::Thumb(id, res) => {
                self.media.done(&MediaKey::Thumb(id.clone()));
                let slot = match res {
                    Ok((img, qr)) => {
                        if !qr.is_empty() {
                            self.qr.insert(id.clone(), qr);
                        }
                        self.loaded(&Job::Thumb(id.clone()));
                        Load::Ready(Picture::new(img, self.picker.as_ref()))
                    }
                    Err(e) => {
                        self.retry_later(Job::Thumb(id.clone()));
                        Load::Failed(e)
                    }
                };
                self.thumbs.insert(id, slot);
            }
            AppEvent::AnimFrame(id) => {
                let frame = self
                    .anim
                    .as_ref()
                    .filter(|p| p.id == id)
                    .and_then(anim::Player::take_frame);
                if let (Some(img), Some(Load::Ready(media))) = (frame, self.full.get_mut(&id)) {
                    // Replace the picture on screen with the new frame.
                    media.picture = Some(Picture::new(img, self.picker.as_ref()));
                }
            }
            AppEvent::Full(id, res) => match res {
                Ok(Downloaded {
                    data,
                    file_name,
                    picture,
                    note,
                    qr,
                }) => {
                    // The original may show a code the smaller picture missed.
                    if !qr.is_empty() {
                        self.qr.insert(id.clone(), qr);
                    }
                    let picture = picture.map(|p| Picture::new(p, self.picker.as_ref()));
                    if let Some(n) = &note {
                        self.status = n.clone();
                    }
                    self.remember_full(
                        id.clone(),
                        FullMedia {
                            data,
                            file_name,
                            picture,
                            note,
                        },
                    );
                    if self.status == "downloading…" {
                        self.status.clear();
                    }
                    if let Some(after) = self.pending.remove(&id) {
                        self.finish(&id, after);
                    }
                }
                Err(e) => {
                    self.pending.remove(&id);
                    self.status = format!("download failed: {e}");
                    self.full.insert(id, Load::Failed(e));
                }
            },
        }
    }

    /// The server's chat list arrived: merge it with what the store had.
    fn on_server_chats(&mut self, server: Vec<ChatEntry>) {
        let selected = self.selected_mid();
        for mut s in server {
            let stale = s
                .last_id
                .as_ref()
                .is_some_and(|id| !self.store.has_message(id).unwrap_or(false));
            if stale {
                self.stale.insert(s.mid.clone());
            } else {
                self.stale.remove(&s.mid);
            }
            if self.read_receipts() && self.open.as_deref() == Some(s.mid.as_str()) {
                s.unread = 0;
            }
            match self.chats.iter_mut().find(|c| c.mid == s.mid) {
                Some(c) => {
                    s.muted = c.muted;
                    // A message that arrived live after the list was fetched wins.
                    if c.last_time > s.last_time {
                        s.preview = c.preview.clone();
                        s.last_time = c.last_time;
                        s.last_id = c.last_id.clone();
                    }
                    *c = s.clone();
                }
                None => self.chats.push(s.clone()),
            }
            self.persist_chat(&s.mid);
        }
        self.sort_chats(selected);
        let visible_len = self.list_rows().len();
        if self.list.selected().is_none() && visible_len > 0 {
            self.list.select(Some(0));
        }
        self.status = format!("{} chats · live", self.chats.len());
        // The open chat shows what arrived without being opened again.
        if let Some(open) = self.open.clone()
            && (self.stale.contains(&open) || self.resync.contains(&open))
            && self.loading.insert(open.clone())
        {
            spawn_history(self.client.clone(), self.tx.clone(), self.store.clone(), open);
        }
    }

    /// The selection reached the last chat: fetch the next page of older chats.
    /// Not while a query filters the list (typing must not page through every
    /// chat): there ↓ at the end asks for it ([`App::search_older_chats`]).
    fn maybe_more_chats(&mut self) {
        if self.filtering() {
            return;
        }
        let visible_len = self.list_rows().len();
        let at_end = self.list.selected().is_some_and(|i| i + 1 >= visible_len);
        if at_end && visible_len > 0 {
            self.load_more_chats();
        }
    }

    /// ↓ past the last search result (or with none): look through the older
    /// chats, page after page until one matches or all are listed.
    fn search_older_chats(&mut self) {
        match self.more_chats {
            MoreChats::Ready(_) => self.load_more_chats(),
            MoreChats::Loading(_) => self.status = "searching older chats…".into(),
            MoreChats::Done => self.status = format!("all {} chats searched", self.chats.len()),
        }
    }

    fn load_more_chats(&mut self) {
        let MoreChats::Ready(after) = &self.more_chats else {
            return;
        };
        let after = after.clone();
        self.more_chats = MoreChats::Loading(after.clone());
        self.status = if self.filtering() {
            "searching older chats…"
        } else {
            "loading more chats…"
        }
        .into();
        let known = self.chats.iter().map(|c| c.mid.clone()).collect();
        spawn_more_chats(self.client.clone(), self.tx.clone(), after, known);
    }

    /// Chat rows of the list (search results that are chats).
    fn chat_rows(&self) -> usize {
        self.list_rows()
            .iter()
            .filter(|r| matches!(r, ListRow::Chat(_)))
            .count()
    }

    fn on_more_chats(&mut self, res: Result<(Vec<ChatEntry>, Option<String>), String>) {
        let matches_before = self.filtering().then(|| self.chat_rows());
        let (mut chats, next) = match res {
            Ok(v) => v,
            Err(e) => {
                // Back to where it was: the next time the end is reached tries again.
                if let MoreChats::Loading(after) = &self.more_chats {
                    self.more_chats = MoreChats::Ready(after.clone());
                }
                self.status = format!("could not load more chats: {e}");
                return;
            }
        };
        // Chats that showed up meanwhile (live) keep what they have.
        chats.retain(|c| !self.chats.iter().any(|k| k.mid == c.mid));
        let n = chats.len();
        let done = next.is_none();
        self.more_chats = match next {
            Some(after) => MoreChats::Ready(Some(after)),
            None => MoreChats::Done,
        };
        if n > 0 {
            self.on_server_chats(chats);
        }
        self.status = match (n, done) {
            (0, _) => format!("{} chats · all loaded", self.chats.len()),
            (_, true) => format!("{n} more chats · {} chats · all loaded", self.chats.len()),
            (_, false) => format!("{n} more chats · {} chats", self.chats.len()),
        };
        // Searching older chats goes on until one matches (or the search ends).
        if let Some(before) = matches_before
            && self.filtering()
        {
            let found = self.chat_rows().saturating_sub(before);
            match (found, done) {
                (0, false) => self.load_more_chats(),
                (0, true) => {
                    self.status = format!("no other chat matches · all {} chats searched", self.chats.len())
                }
                (1, _) => self.status = "1 more chat matches".into(),
                (k, _) => self.status = format!("{k} more chats match"),
            }
        }
    }

    fn clear_cache(&mut self) {
        if let Err(e) = self.store.clear() {
            self.status = format!("could not clear: {e}");
            return;
        }
        self.saved(self.store.bind_account(&self.me));
        for ticket in self.receipts.clear() {
            if let Some(task) = self.receipt_tasks.remove(&ticket) {
                task.abort();
            }
        }
        // Copies of media opened from the cleared chats go too.
        if let Err(e) = prune_dir(&view_dir(), Duration::ZERO, 0, std::time::SystemTime::now()) {
            tracing::warn!("emptying {} failed: {e}", view_dir().display());
        }
        self.messages.clear();
        self.read_marks.clear();
        self.no_older.clear();
        self.stale.clear();
        self.reload_pins();
        self.chats.clear();
        self.more_chats = MoreChats::Ready(None);
        // The store dropped the drafts with the chats.
        self.drafts.clear();
        self.input.clear();
        self.replying = None;
        self.open = None;
        self.focus = Focus::Chats;
        self.list.select(None);
        self.status = "local data cleared — reloading…".into();
        spawn_chat_list(self.client.clone(), self.tx.clone());
        spawn_contacts(self.client.clone(), self.tx.clone(), self.store.clone(), true);
    }

    fn submit(&mut self) {
        let text = self.input.take();
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(chat) = &self.open {
            self.saved(self.store.remove(&format!("{DRAFT_KEY}{chat}")));
        }
        // Several lines are a message, never a command.
        let (cmd, arg) = match text.split_once(char::is_whitespace) {
            _ if text.contains('\n') => ("", ""),
            Some((c, a)) => (c, a.trim()),
            None => (text.as_str(), ""),
        };
        // Plugin commands (never one of rune's own: see `keymap::CORE_COMMANDS`).
        if let Some(name) = cmd.strip_prefix('/')
            && !name.is_empty()
            && !name.starts_with('/')
            && self.plugin_command(name, arg)
        {
            return;
        }
        let Some(chat) = self.open.clone() else {
            self.status = "open a chat first".into();
            return;
        };
        let is_group = MidType::of(&chat) != MidType::User;
        match cmd {
            "/save" if arg.is_empty() => {
                self.input.set(text.clone());
                self.status = "usage: /save PATH (or s on a selected message)".into();
                return;
            }
            "/save" => return self.save_latest_media(&chat, arg.to_owned()),
            "/attach" | "/image" | "/file" | "/send" => {
                let path = (!arg.is_empty()).then(|| attach::expand_home(arg));
                return match path {
                    Some(p) if p.is_file() => self.open_attach(Some(p)),
                    Some(p) if p.is_dir() => {
                        self.open_attach(None);
                        if let Some(Modal::Attach(a)) = self.modal.as_mut() {
                            *a = attach::Attach::new(p);
                        }
                    }
                    Some(p) => self.status = format!("no such file: {}", p.display()),
                    None => self.open_attach(None),
                };
            }
            "/sticker" | "/stickers" => return self.open_stickers(),
            "/location" => return self.run_action(Action::Location),
            "/contact" => return self.run_action(Action::ShareContact),
            "/invite" if is_group => return self.run_action(Action::Invite(chat)),
            "/leave" if is_group => return self.run_action(Action::ConfirmLeave(chat)),
            "/rename" if is_group => return self.run_action(Action::Rename(chat)),
            "/albums" | "/album" if is_group => return self.run_action(Action::Albums(chat)),
            "/invite" | "/leave" | "/rename" | "/albums" | "/album" => {
                self.input.set(text.clone());
                self.status = format!("{cmd} is for groups (type //{} to send it as text)", &cmd[1..]);
                return;
            }
            "/mute" => return self.run_action(Action::Mute(chat, true)),
            "/unmute" => return self.run_action(Action::Mute(chat, false)),
            _ => {}
        }
        // "//text" sends "/text"; an unknown command is never sent by accident,
        // whatever follows it (a path such as "/usr/bin …" is text).
        let text = if let Some(rest) = text.strip_prefix("//") {
            format!("/{rest}")
        } else if is_command_word(cmd) {
            self.input.set(text.clone());
            self.status =
                format!("unknown command {cmd} (start with // to send it as text; F1 lists the commands)");
            return;
        } else {
            text
        };
        let out = match self.replying.take() {
            Some(id) => OutgoingMessage::reply(&chat, &text, &id),
            None => OutgoingMessage::text(&chat, &text),
        };
        self.send_outgoing(chat, out, text);
    }

    /// `/save PATH`: the newest media of a chat to a file.
    fn save_latest_media(&mut self, chat: &str, path: String) {
        let media = self
            .messages
            .get(chat)
            .and_then(|l| l.iter().rev().filter(|m| !m.unsent).find_map(|m| m.raw.clone()));
        let Some(m) = media else {
            self.status = "no media in this chat".into();
            return;
        };
        let path = attach::expand_home(&path);
        if std::fs::symlink_metadata(&path).is_ok() {
            self.status = format!("{} already exists; choose another name", path.display());
            return;
        }
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.status = "downloading…".into();
        tokio::spawn(async move {
            let s = match client.download_media(&m, false).await {
                Ok(data) => {
                    let len = data.len();
                    let p = path.clone();
                    // A file created there meanwhile is still never replaced.
                    let written = tokio::task::spawn_blocking(move || write_new(&p, &data))
                        .await
                        .map_err(std::io::Error::other)
                        .and_then(|r| r);
                    match written {
                        Ok(()) => format!("saved {len} bytes to {}", path.display()),
                        Err(e) => format!("write failed: {e}"),
                    }
                }
                Err(e) => format!("download failed: {e}"),
            };
            let _ = tx.send(AppEvent::Status(s));
        });
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Press {
            self.expire_status();
        }
        self.handle_key(k);
        self.settle_modal();
    }

    /// Remember when the status message changed (called before each frame).
    fn note_status(&mut self) {
        if self.status_since.as_ref().is_none_or(|(s, _)| *s != self.status) {
            self.status_since = Some((self.status.clone(), Instant::now()));
        }
    }

    /// Before a key, click or paste is handled: a status message that has
    /// been on screen long enough goes, so old news does not crowd out the
    /// keys. It only goes when someone is there to have read it — never
    /// while the keyboard is idle — and errors stay longer.
    fn expire_status(&mut self) {
        self.note_status();
        if let Some((shown, since)) = &self.status_since
            && !shown.is_empty()
        {
            let keep = if status_is_error(shown) {
                STATUS_ERROR_KEEP
            } else {
                STATUS_KEEP
            };
            if since.elapsed() >= keep {
                self.status.clear();
            }
        }
    }

    fn handle_key(&mut self, k: KeyEvent) {
        use keymap::Cmd;
        if k.kind != KeyEventKind::Press {
            return;
        }
        // Waiting for a new shortcut: whatever is pressed is it.
        if matches!(&self.modal, Some(Modal::Shortcuts(p)) if p.capturing()) {
            return self.modal_key(k);
        }
        let ctx = self.key_context();
        let cmd = keymap::resolve(ctx, &k);
        if cmd == Some(Cmd::Quit) {
            self.quit = true;
            return;
        }
        if matches!(self.modal, Some(Modal::Help { .. })) {
            return self.modal_key(k);
        }
        if cmd == Some(Cmd::Help) {
            return self.open_help(ctx);
        }
        if self.modal.is_some() {
            return self.modal_key(k);
        }
        if cmd == Some(Cmd::Settings) {
            return self.open_settings();
        }
        if cmd == Some(Cmd::MainMenu) {
            return self.main_menu();
        }
        if cmd == Some(Cmd::ReadReceipts) {
            return self.toggle_read_receipts();
        }
        if !self.profile.is_empty() {
            return self.profile_key(k);
        }
        // Chords for the open chat, also while typing.
        match (cmd, self.open.clone()) {
            (Some(Cmd::Search), _) => return self.start_search(),
            (Some(Cmd::ComposeMenu), _) => return self.compose_menu(),
            (Some(Cmd::ChatMenu), Some(mid)) => return self.chat_menu(&mid),
            (Some(Cmd::ChatProfile), Some(mid)) => return self.open_profile(mid),
            (Some(Cmd::ChatMenu | Cmd::ChatProfile), None) => {
                self.status = "open a chat first".into();
                return;
            }
            _ => {}
        }
        // Alt-<letter or digit>: plugin keys (rune binds none of them).
        if cmd.is_none()
            && let Some(c) = keymap::is_plugin_key(&k)
            && matches!(
                ctx,
                keymap::Ctx::ChatList
                    | keymap::Ctx::ChatSearch
                    | keymap::Ctx::Compose
                    | keymap::Ctx::Messages
                    | keymap::Ctx::Viewer
            )
            && self.plugin_key(c)
        {
            return;
        }
        match ctx {
            keymap::Ctx::Viewer => self.viewer_key(cmd),
            keymap::Ctx::ChatSearch => self.search_key(cmd, &k),
            keymap::Ctx::ChatList => self.chat_list_key(cmd),
            keymap::Ctx::Compose => self.compose_key(cmd, &k),
            keymap::Ctx::Messages => self.messages_key(cmd, &k),
            _ => {}
        }
        self.left_search();
    }

    fn viewer_key(&mut self, cmd: Option<keymap::Cmd>) {
        use keymap::Cmd;
        match cmd {
            Some(Cmd::Confirm | Cmd::PlayPause) if self.viewer_plays() => self.play_selected(),
            Some(Cmd::PlayPause) if self.viewer_animates() => {
                if let Some(player) = &self.anim {
                    player.toggle_pause();
                    self.status = if player.is_paused() { "paused" } else { "playing" }.into();
                }
            }
            Some(Cmd::PlayPause) => self.status = "nothing to play in this picture".into(),
            Some(Cmd::Back | Cmd::Confirm) => self.close_viewer(),
            Some(Cmd::Left) => self.step_media(false),
            Some(Cmd::Right) => self.step_media(true),
            Some(Cmd::Menu) => self.message_menu(),
            Some(Cmd::Save) => self.act_on_selected(After::Save),
            Some(Cmd::OpenExternal) => self.act_on_selected(After::Open),
            Some(Cmd::OpenLink) if !self.open_selected_link() => {
                self.status = "no link or QR code in this picture".into();
            }
            _ => {}
        }
    }

    fn search_key(&mut self, cmd: Option<keymap::Cmd>, k: &KeyEvent) {
        use keymap::Cmd;
        match cmd {
            Some(Cmd::Back) => self.end_search(),
            Some(Cmd::Confirm) => {
                if let Some(mid) = self.selected_mid() {
                    self.enter_chat(mid);
                }
            }
            Some(Cmd::Up) => self.move_chat_sel(-1),
            Some(Cmd::Down) => {
                let n = self.list_rows().len();
                if self.filtering() && self.list.selected().is_none_or(|i| i + 1 >= n) {
                    self.search_older_chats();
                }
                self.move_chat_sel(1);
            }
            Some(Cmd::PageUp) => self.move_chat_sel(-self.chat_page()),
            Some(Cmd::PageDown) => self.move_chat_sel(self.chat_page()),
            // The query stays for coming back (see `App::left_search`).
            Some(Cmd::SwitchFocus) if self.open.is_some() => self.focus = Focus::Input,
            cmd => {
                let selected = self.selected_mid();
                match self.chat_query.edit(cmd, k) {
                    Edit::Changed => self.chat_query_changed(selected),
                    // Only → past the end leaves; End or Delete there stay in the field.
                    Edit::MovedHitRight if cmd == Some(Cmd::CursorRight) && self.open.is_some() => {
                        self.focus = Focus::Input;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Chat-list rows that fit on screen (for PgUp / PgDn).
    fn chat_page(&self) -> isize {
        (self.hit.chats.height / CHAT_ROW_H).max(1) as isize
    }

    /// List positions of the chats that want attention (see
    /// [`ChatEntry::wants_attention`]).
    fn unread_rows(&self) -> Vec<usize> {
        self.list_rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, ListRow::Chat(i) if self.chats[*i].wants_attention()))
            .map(|(k, _)| k)
            .collect()
    }

    /// List positions shown in full by the last frame.
    fn chat_view(&self) -> std::ops::Range<usize> {
        let first = self.list.offset();
        first..first + (self.hit.chats.height / CHAT_ROW_H).max(1) as usize
    }

    /// Select an unread chat: the nearest to the selection, or the nearest
    /// above / below what is on screen. Says so when there is none.
    fn jump_to_unread(&mut self, toward: Toward) {
        let rows = self.unread_rows();
        let view = self.chat_view();
        let target = match toward {
            Toward::Nearest => nearest_unread(&rows, self.list.selected().unwrap_or(view.start)),
            Toward::Above => rows.iter().copied().rfind(|&k| k < view.start),
            Toward::Below => rows.iter().copied().find(|&k| k >= view.end),
        };
        match target {
            Some(k) => self.list.select(Some(k)),
            None => self.status = "no other unread chat".into(),
        }
    }

    fn chat_list_key(&mut self, cmd: Option<keymap::Cmd>) {
        use keymap::Cmd;
        let selected = self.selected_mid();
        match cmd {
            Some(Cmd::Up) => self.move_chat_sel(-1),
            Some(Cmd::Down) => self.move_chat_sel(1),
            Some(Cmd::PageUp) => self.move_chat_sel(-self.chat_page()),
            Some(Cmd::PageDown) => self.move_chat_sel(self.chat_page()),
            Some(Cmd::Home) => {
                let first = (!self.list_rows().is_empty()).then_some(0);
                self.list.select(first);
            }
            Some(Cmd::End) => {
                let last = self.list_rows().len().checked_sub(1);
                self.list.select(last);
            }
            Some(Cmd::Confirm) => {
                if let Some(mid) = selected {
                    self.enter_chat(mid);
                }
            }
            Some(Cmd::SwitchFocus) if self.open.is_some() => self.focus = Focus::Input,
            Some(Cmd::Menu) => {
                if let Some(mid) = selected {
                    self.chat_menu(&mid);
                }
            }
            Some(Cmd::Profile) => {
                if let Some(mid) = selected {
                    self.open_profile(mid);
                }
            }
            Some(Cmd::NewGroup) => self.run_action(Action::NewGroup),
            Some(Cmd::NextUnread) => self.jump_to_unread(Toward::Nearest),
            Some(Cmd::PinUp | Cmd::PinDown) => {
                if let Some(mid) = selected {
                    self.move_pin(&mid, if cmd == Some(Cmd::PinUp) { -1 } else { 1 });
                }
            }
            _ => {}
        }
    }

    fn compose_key(&mut self, cmd: Option<keymap::Cmd>, k: &KeyEvent) {
        use keymap::Cmd;
        let width = self.input_width();
        match cmd {
            // A pasted line break is not a request to send half of the paste.
            Some(Cmd::Confirm) if self.pasting => {
                self.input.insert('\n');
                self.status = format!(
                    "pasted text keeps its line breaks — {} it",
                    keymap::with_keys(keymap::Ctx::Compose, Cmd::Confirm, "sends")
                );
            }
            Some(Cmd::Confirm) => self.submit(),
            Some(Cmd::NewLine) => self.input.insert('\n'),
            Some(Cmd::Back) if self.replying.take().is_some() => {
                self.status = "reply cancelled".into();
            }
            Some(Cmd::Back | Cmd::SwitchFocus) => self.focus = Focus::Chats,
            Some(Cmd::Up) if self.input.move_row(-1, width) => {}
            Some(Cmd::Down) => {
                self.input.move_row(1, width);
            }
            Some(Cmd::Up) => self.select_msg(self.open_messages().len().checked_sub(1)),
            Some(Cmd::PageUp) => self.scroll_by(10),
            Some(Cmd::PageDown) => self.scroll_by(-10),
            cmd => {
                // Only ← past the start leaves; Home, Backspace or Delete there stay in the line.
                if self.input.edit(cmd, k) == Edit::MovedHitLeft && cmd == Some(Cmd::CursorLeft) {
                    self.focus = Focus::Chats;
                }
            }
        }
    }

    /// Text columns of the composer as last drawn (0 before the first frame: no wrapping).
    fn input_width(&self) -> usize {
        self.hit.input.width.saturating_sub(2) as usize
    }

    fn messages_key(&mut self, cmd: Option<keymap::Cmd>, k: &KeyEvent) {
        use keymap::Cmd;
        match cmd {
            Some(Cmd::Up) => {
                if self.sel == Some(0) {
                    self.load_older();
                }
                self.select_msg(Some(self.sel.map_or(0, |i| i.saturating_sub(1))));
            }
            Some(Cmd::Down) => {
                let n = self.open_messages().len();
                match self.sel {
                    Some(i) if i + 1 < n => self.select_msg(Some(i + 1)),
                    _ => self.focus = Focus::Input,
                }
            }
            Some(Cmd::Left | Cmd::SwitchFocus) => self.focus = Focus::Chats,
            Some(Cmd::Right | Cmd::Back) => self.focus = Focus::Input,
            Some(Cmd::PageUp) => self.scroll_by(10),
            Some(Cmd::PageDown) => self.scroll_by(-10),
            Some(Cmd::Home) => self.select_msg((!self.open_messages().is_empty()).then_some(0)),
            Some(Cmd::End) => self.select_msg(self.open_messages().len().checked_sub(1)),
            Some(Cmd::Confirm) if self.selected_msg().is_some_and(UiMsg::plays) => self.play_selected(),
            Some(Cmd::Confirm) => {
                let media = self.selected_msg().is_some_and(|m| m.media_kind().is_some());
                if media || !self.open_selected_link() {
                    self.open_viewer();
                }
            }
            Some(Cmd::Reply) => self.start_reply(),
            Some(Cmd::Menu) => self.message_menu(),
            Some(Cmd::React) => {
                if let Some(id) = self
                    .selected_msg()
                    .filter(|m| !m.id.is_empty())
                    .map(|m| m.id.clone())
                {
                    self.run_action(Action::ReactMenu(id));
                }
            }
            Some(Cmd::Unsend) => match self.selected_msg() {
                Some(m) if self.can_unsend(m) => {
                    let id = m.id.clone();
                    self.run_action(Action::ConfirmUnsend(id));
                }
                Some(_) => self.status = "you can only unsend your own messages".into(),
                None => {}
            },
            Some(Cmd::Profile) => {
                if let Some(from) = self.selected_msg().map(|m| m.from.clone()) {
                    self.open_profile(from);
                }
            }
            Some(Cmd::Save) => self.act_on_selected(After::Save),
            Some(Cmd::OpenExternal) => self.act_on_selected(After::Open),
            Some(Cmd::Copy) => self.copy_selected(),
            Some(Cmd::FindInChat) => {
                if let Some(chat) = self.open.clone() {
                    self.open_find(chat);
                }
            }
            None => {
                if let Some(c) = typed_char(k) {
                    self.focus = Focus::Input;
                    self.input.insert(c);
                }
            }
            _ => {}
        }
    }

    fn can_unsend(&self, m: &UiMsg) -> bool {
        m.from == self.me && !m.unsent && !m.id.is_empty()
    }

    /// Bracketed paste: into the search field or an overlay's text field; a
    /// file path in a chat offers to send the file; other text is typed.
    fn on_paste(&mut self, text: &str) {
        self.expire_status();
        self.handle_paste(text);
        self.settle_modal();
    }

    fn handle_paste(&mut self, text: &str) {
        if self.modal.is_some() {
            return self.modal_paste(text);
        }
        // The profile page and the viewer have no text field.
        if !self.profile.is_empty() || self.viewer.is_some() {
            return;
        }
        // One line: newlines would send nothing useful.
        let line = text.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " ");
        if self.searching && self.focus == Focus::Chats {
            let selected = self.selected_mid();
            self.chat_query.insert_str(&line);
            return self.chat_query_changed(selected);
        }
        if self.open.is_some()
            && self.focus != Focus::Chats
            && let Some(path) = attach::pasted_path(text)
        {
            return self.open_attach(Some(path));
        }
        if self.open.is_some() {
            // The message line keeps line breaks; nothing is sent until Enter.
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            let text = text.trim_end_matches('\n');
            self.focus = Focus::Input;
            self.input.insert_str(text);
            let lines = text.lines().count();
            if lines > 1 {
                self.status = format!(
                    "pasted {lines} lines — {} them as one message",
                    keymap::with_keys(keymap::Ctx::Compose, keymap::Cmd::Confirm, "sends")
                );
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if matches!(
            m.kind,
            MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            self.expire_status();
        }
        self.handle_mouse(m);
        self.settle_modal();
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
        let at = Position::new(m.column, m.row);
        if self.modal.is_some() {
            return self.modal_mouse(m);
        }
        if !self.profile.is_empty() {
            return self.profile_mouse(m);
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some(url) = self
                .hit
                .links
                .iter()
                .find(|(r, _)| r.contains(at))
                .map(|l| l.1.clone())
        {
            return self.open_link(&url);
        }
        if let Some(area) = self.hit.viewer {
            match m.kind {
                MouseEventKind::ScrollUp => self.step_media(false),
                MouseEventKind::ScrollDown => self.step_media(true),
                MouseEventKind::Down(MouseButton::Right) => self.close_viewer(),
                MouseEventKind::Down(MouseButton::Left) if !area.contains(at) => self.close_viewer(),
                _ => {}
            }
            return;
        }
        let in_chats = self.hit.chats.contains(at);
        let in_conv = self.hit.conv.contains(at);
        match m.kind {
            MouseEventKind::ScrollUp if in_chats => self.move_chat_sel(-1),
            MouseEventKind::ScrollDown if in_chats => self.move_chat_sel(1),
            MouseEventKind::ScrollUp if in_conv => self.scroll_by(3),
            MouseEventKind::ScrollDown if in_conv => self.scroll_by(-3),
            MouseEventKind::Down(MouseButton::Left)
                if self.hit.unread_above.is_some_and(|r| r.contains(at)) =>
            {
                self.focus = Focus::Chats;
                self.jump_to_unread(Toward::Above);
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.hit.unread_below.is_some_and(|r| r.contains(at)) =>
            {
                self.focus = Focus::Chats;
                self.jump_to_unread(Toward::Below);
            }
            MouseEventKind::Down(MouseButton::Left) if in_chats => {
                let i = self.list.offset() + ((m.row - self.hit.chats.y) / CHAT_ROW_H) as usize;
                if let Some(mid) = self.list_rows().get(i).map(|r| self.row_mid(r).to_owned()) {
                    self.list.select(Some(i));
                    self.enter_chat(mid);
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.hit.conv_title.contains(at) && self.open.is_some() =>
            {
                let mid = self.open.clone().unwrap_or_default();
                self.open_profile(mid);
            }
            MouseEventKind::Down(MouseButton::Left)
                if in_conv
                    && let Some(from) = self
                        .hit
                        .heads
                        .iter()
                        .find(|(y, _)| *y == m.row)
                        .map(|h| h.1.clone()) =>
            {
                self.open_profile(from);
            }
            MouseEventKind::Down(MouseButton::Left)
                if in_conv
                    && let Some(id) = self
                        .hit
                        .reactions
                        .iter()
                        .find(|(y, _)| *y == m.row)
                        .map(|(_, id)| id.clone()) =>
            {
                self.run_action(Action::Reactions(id));
            }
            MouseEventKind::Down(MouseButton::Left) if in_conv => {
                let hit = self
                    .hit
                    .msgs
                    .iter()
                    .find(|(y0, y1, _)| (*y0..*y1).contains(&m.row))
                    .map(|h| h.2);
                if let Some(i) = hit {
                    self.select_msg(Some(i));
                    if self.selected_msg().is_some_and(UiMsg::plays) {
                        self.play_selected();
                    } else if self.selected_msg().is_some_and(|m| m.media_kind().is_some()) {
                        self.open_viewer();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.hit.input.contains(at) && self.open.is_some() => {
                self.focus = Focus::Input;
            }
            _ => {}
        }
        self.left_search();
    }

    /// `c` on a selected message: its text to the clipboard.
    fn copy_selected(&mut self) {
        let Some(m) = self.selected_msg() else { return };
        if !m.copyable() {
            self.status = "only a text message can be copied".into();
            return;
        }
        let text = m.text.clone();
        let n = text.chars().count();
        let what = if n == 1 { "character" } else { "characters" };
        self.status = match clipboard::copy(&text, &self.tx) {
            clipboard::Via::Program(_) => format!("copied {n} {what}"),
            clipboard::Via::Terminal => {
                format!("copied {n} {what} through the terminal (if it allows that: OSC 52)")
            }
        };
    }

    /// `r` on a selected message: the next text message quotes it.
    fn start_reply(&mut self) {
        // From the viewer's menu: typing happens in the message line.
        if self.viewer.is_some() {
            self.close_viewer();
        }
        let Some(m) = self.selected_msg().filter(|m| !m.id.is_empty()) else {
            self.status = "this message cannot be replied to yet".into();
            return;
        };
        self.replying = Some(m.id.clone());
        self.focus = Focus::Input;
        self.status = format!(
            "replying — {}{}{}",
            keymap::with_keys(keymap::Ctx::Compose, keymap::Cmd::Confirm, "send"),
            keymap::HINT_SEP,
            keymap::with_keys(keymap::Ctx::Compose, keymap::Cmd::Back, "cancel")
        );
    }

    /// `g` on a reply: select the message it quotes.
    fn goto_quoted(&mut self) {
        let Some(id) = self.selected_msg().and_then(|m| m.reply_to.clone()) else {
            self.status = "not a reply".into();
            return;
        };
        match self.open_messages().iter().position(|m| m.id == id) {
            Some(i) => {
                // The viewer would keep showing the reply while the quote is selected.
                if self.viewer.is_some() {
                    self.close_viewer();
                }
                self.select_msg(Some(i));
            }
            None => self.status = "the quoted message is older than the loaded history".into(),
        }
    }

    /// "Name: text" of a quoted message in `chat`, if it is loaded.
    fn quote(&self, chat: &str, id: &str, max: usize) -> String {
        let q = self
            .messages
            .get(chat)
            .and_then(|l| l.iter().find(|m| m.id == id));
        self.quote_line(q, max)
    }

    fn quote_line(&self, q: Option<&UiMsg>, max: usize) -> String {
        match q {
            Some(q) => {
                let flat = q.text.replace(['\r', '\n'], " ");
                ellipsize(&format!("{}: {}", self.name(&q.from), flat.trim()), max)
            }
            None => "an earlier message".into(),
        }
    }

    /// Select a message and keep it in view (`None` changes nothing).
    fn select_msg(&mut self, i: Option<usize>) {
        if i.is_none() {
            return;
        }
        self.sel = i;
        self.focus = Focus::Messages;
        self.follow_sel = true;
    }

    /// Scroll the conversation; positive is towards older messages.
    fn scroll_by(&mut self, lines: isize) {
        if lines > 0 && self.at_top {
            self.load_older();
        }
        self.scroll = self.scroll.saturating_add_signed(lines);
        self.follow_sel = false;
    }

    fn act_on_selected(&mut self, after: After) {
        if let Some(m) = self.selected_msg().cloned() {
            self.fetch_full(&m, after);
        }
    }

    /// Web links of a message: in its text, then in QR codes in its picture.
    fn links_of(&self, m: &UiMsg) -> Vec<String> {
        let mut out: Vec<String> = links::find_links(&m.text)
            .into_iter()
            .map(|r| m.text[r].to_owned())
            .collect();
        for code in self.qr.get(&m.id).into_iter().flatten() {
            if links::is_web_link(code) {
                out.push(code.trim().to_owned());
            } else {
                out.extend(links::find_links(code).into_iter().map(|r| code[r].to_owned()));
            }
        }
        out
    }

    fn open_link(&mut self, url: &str) {
        self.status = match links::open_url(url) {
            Ok(()) => format!("opened {url}"),
            Err(e) => format!("could not open link: {e}"),
        };
    }

    /// Open the first link of the selected message (text or QR code).
    fn open_selected_link(&mut self) -> bool {
        let Some(m) = self.selected_msg().cloned() else {
            return false;
        };
        match self.links_of(&m).first() {
            Some(url) => {
                let url = url.clone();
                self.open_link(&url);
                true
            }
            None => false,
        }
    }

    fn close_viewer(&mut self) {
        self.viewer = None;
        self.needs_clear = true;
    }

    fn key_context(&self) -> keymap::Ctx {
        use keymap::Ctx;
        if let Some(modal) = &self.modal {
            return match modal {
                Modal::Menu(_) => Ctx::Menu,
                Modal::Confirm(_) => Ctx::Confirm,
                Modal::Reactions { .. } | Modal::ReadBy { .. } => Ctx::Info,
                Modal::Help { .. } => Ctx::Help,
                Modal::Form(_) => Ctx::Form,
                Modal::Pick(_) => Ctx::Pick,
                Modal::Attach(a) if a.chosen.is_some() => Ctx::AttachSend,
                Modal::Attach(a) if a.filter.is_empty() => Ctx::AttachBrowse,
                Modal::Attach(_) => Ctx::AttachFilter,
                Modal::Stickers(s) => s.ctx(),
                Modal::Settings(_) => Ctx::Settings,
                Modal::Plugins(_) => Ctx::Plugins,
                Modal::PluginView(_) => Ctx::PluginView,
                Modal::Shortcuts(_) => Ctx::Keys,
                Modal::Find(_) => Ctx::Find,
            };
        }
        if !self.profile.is_empty() {
            return Ctx::Profile;
        }
        if self.viewer.is_some() {
            return Ctx::Viewer;
        }
        match self.focus {
            Focus::Chats if self.searching => Ctx::ChatSearch,
            Focus::Chats => Ctx::ChatList,
            Focus::Input => Ctx::Compose,
            Focus::Messages => Ctx::Messages,
        }
    }

    /// Whether `cmd` can do something right now; hints leave out what cannot.
    /// (The keys still answer, with a word on why nothing happened.)
    fn available(&self, cmd: keymap::Cmd) -> bool {
        use keymap::Cmd;
        let chat = self.list.selected().and_then(|i| self.chat_at(i));
        let msg = self.selected_msg();
        let real_msg = msg.filter(|m| !m.id.is_empty());
        match cmd {
            Cmd::ComposeMenu | Cmd::ChatMenu | Cmd::ChatProfile => self.open.is_some(),
            Cmd::Confirm if self.focus == Focus::Chats && self.modal.is_none() => {
                self.selected_mid().is_some()
            }
            Cmd::Menu | Cmd::Profile if self.focus == Focus::Chats => chat.is_some(),
            Cmd::PinUp | Cmd::PinDown => chat.is_some_and(|c| self.is_pinned(&c.mid)),
            Cmd::NextUnread => {
                let selected = self.list.selected();
                self.unread_rows().into_iter().any(|k| Some(k) != selected)
            }
            Cmd::Menu | Cmd::Reply | Cmd::React => real_msg.is_some(),
            Cmd::Profile => msg.is_some(),
            Cmd::Unsend => msg.is_some_and(|m| self.can_unsend(m)),
            Cmd::Save | Cmd::OpenExternal => msg.is_some_and(|m| m.media_kind().is_some()),
            Cmd::Copy => msg.is_some_and(UiMsg::copyable),
            Cmd::FindInChat => self.open.is_some(),
            Cmd::OpenLink => msg.is_some_and(|m| !self.links_of(m).is_empty()),
            Cmd::PlayPause => self.viewer_plays() || self.viewer_animates(),
            Cmd::ResetKey => matches!(
                &self.modal,
                Some(Modal::Shortcuts(p)) if p.selected().is_some_and(|(t, c)| self.keys.is_custom(t, c))
            ),
            Cmd::AsFile | Cmd::OriginalQuality => match &self.modal {
                Some(Modal::Attach(a)) => {
                    !a.cover
                        && (cmd == Cmd::AsFile
                            || a.chosen
                                .as_ref()
                                .is_some_and(|c| c.kind == rune::MediaKind::Image))
                }
                _ => false,
            },
            _ => true,
        }
    }

    /// The status-line hint for what has the keys now.
    fn hint(&self) -> String {
        keymap::hint(self.key_context(), |cmd| self.available(cmd))
    }

    fn open_help(&mut self, ctx: keymap::Ctx) {
        let under = self.modal.take().map(Box::new);
        self.modal = Some(Modal::Help {
            ctx,
            under,
            scroll: 0,
        });
    }

    // -- drawing ---------------------------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        // Pictures not drawn lately make room once over their budgets.
        self.thumbs.new_frame();
        self.sticker_pics.new_frame();
        self.file_previews.new_frame();
        // Pictures under an open overlay must know what covers them before
        // they are drawn (only graphics care: text cells stack by themselves).
        let layered = self.picker.is_some() && self.overlay_open();
        picture::compose(f, layered, |f| self.draw_layers(f));
        split_spacing_marks(f.buffer_mut());
    }

    fn overlay_open(&self) -> bool {
        self.modal.is_some() || !self.profile.is_empty() || self.viewer.is_some()
    }

    /// Everything on screen, bottom layer first. Runs twice per frame while
    /// an overlay is open (see [`picture::compose`]).
    fn draw_layers(&mut self, f: &mut Frame) {
        picture::layer(Layer::Base);
        let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        // The chat list gives up some width on narrow terminals so messages keep room.
        let list_w = (main.width * 2 / 5).clamp(CHAT_LIST_MIN_W, CHAT_LIST_W);
        let [left, right] = Layout::horizontal([Constraint::Length(list_w), Constraint::Min(20)]).areas(main);
        self.draw_chats(f, left);
        let rows = self.input.rows(right.width.saturating_sub(2) as usize).len();
        let max_rows = (right.height / 3).clamp(1, INPUT_MAX_ROWS);
        let input_h = (rows as u16).clamp(1, max_rows) + 2;
        let [conv, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(input_h)]).areas(right);
        self.draw_conversation(f, conv);
        self.draw_input(f, input);
        // Overlays show their own keys on their frame; the status line then
        // only says how to get help, so nothing is listed twice.
        let overlay = self.overlay_open();
        let help = keymap::help_key(self.key_context());
        let hint = match self.hint() {
            keys if !overlay && !keys.is_empty() => format!("{help}{}{keys}", keymap::HINT_SEP),
            _ => help.to_owned(),
        };
        let (message, hint) = status_line(&self.status, &hint, status.width as usize);
        let mut spans = Vec::with_capacity(3);
        if let Some(message) = message {
            // Failures stand out from progress and confirmations.
            let t = theme::t();
            let (fg, bg) = if status_is_error(&self.status) {
                (t.on_danger, t.danger)
            } else {
                (t.on_accent, t.accent)
            };
            spans.push(Span::styled(message, Style::new().fg(fg).bg(bg)));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(hint, theme::muted()));
        let line = Line::from(spans);
        f.render_widget(Paragraph::new(line), status);
        self.hit.viewer = None;
        self.hit.profile = None;
        if self.viewer.is_some() {
            picture::layer(Layer::Viewer);
            self.draw_viewer(f, main);
        }
        if !self.profile.is_empty() {
            picture::layer(Layer::Profile);
            self.draw_profile(f, main);
        }
        if self.modal.is_some() {
            picture::layer(Layer::Modal);
            self.draw_modal(f, main);
        } else {
            self.hit.modal = None;
        }
    }

    fn draw_viewer(&mut self, f: &mut Frame, area: Rect) {
        let Some(id) = self.viewer.clone() else { return };
        let [area] = Layout::horizontal([Constraint::Percentage(94)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Percentage(94)])
            .flex(Flex::Center)
            .areas(area);
        picture::cover(f, area);
        self.hit.viewer = Some(area);
        // Links behind the viewer are covered now.
        self.hit.links.clear();
        let qr = self.qr.get(&id).cloned().unwrap_or_default();
        let msg = self.open_messages().iter().find(|m| m.id == id).cloned();
        let title = match &msg {
            Some(m) => {
                let text = m.text.replace(['\r', '\n'], " ");
                format!(" {} · {} · {} ", self.name(&m.from), hhmm(m.time), text.trim())
            }
            None => " media ".into(),
        };
        let block = Block::bordered()
            .border_type(BorderType::Double)
            .border_style(Style::new().fg(theme::t().accent))
            .title(ellipsize(&title, area.width.saturating_sub(2) as usize))
            .title_bottom(frame_hint(
                &keymap::hint(keymap::Ctx::Viewer, |cmd| self.available(cmd)),
                area,
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        match self.full.get_mut(&id) {
            Some(Load::Ready(media)) => {
                let mut info = format!("{} · {:.1} KB", media.file_name, media.data.len() as f64 / 1024.0);
                if let Some(n) = &media.note {
                    info = format!("{info} · {n}");
                }
                let qr_h = qr.len().min(3) as u16;
                let [head, codes, body] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(qr_h),
                    Constraint::Min(1),
                ])
                .areas(inner);
                f.render_widget(Paragraph::new(info).style(theme::muted()), head);
                for (i, code) in qr.iter().take(3).enumerate() {
                    let y = codes.y + i as u16;
                    let label = "▣ QR  ";
                    let line = if links::is_web_link(code) {
                        let x = codes.x + label.width() as u16;
                        let w = (code.width() as u16).min(codes.width.saturating_sub(label.width() as u16));
                        self.hit
                            .links
                            .push((Rect::new(x, y, w, 1), code.trim().to_owned()));
                        Line::from(vec![
                            Span::styled(label, Style::new().fg(theme::t().accent).bold()),
                            Span::styled(code.clone(), Style::new().fg(theme::t().link).underlined()),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(label, Style::new().fg(theme::t().accent).bold()),
                            Span::raw(code.replace('\n', " ")),
                        ])
                    };
                    f.render_widget(Paragraph::new(line), Rect::new(codes.x, y, codes.width, 1));
                }
                match media.picture.as_mut() {
                    Some(pic) => {
                        let size = pic.size_for(body.as_size());
                        let [h] = Layout::horizontal([Constraint::Length(size.width)])
                            .flex(Flex::Center)
                            .areas(body);
                        let [r] = Layout::vertical([Constraint::Length(size.height)])
                            .flex(Flex::Center)
                            .areas(h);
                        pic.render(f, r, size, 0, self.cell_mode, VIEW_FILTER);
                    }
                    None => {
                        let text = "No preview for this kind of media.\n\nPress s to save it to ~/Downloads/rune, or o to open it with your system's app.";
                        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), body);
                    }
                }
            }
            Some(Load::Failed(e)) => {
                f.render_widget(
                    Paragraph::new(format!("Could not download: {e}")).wrap(Wrap { trim: false }),
                    inner,
                );
            }
            _ => f.render_widget(Paragraph::new("downloading…").style(theme::muted()), inner),
        }
    }

    /// Right-aligned on the chat list and the message line while read
    /// receipts are off, so reading never marks anything by surprise.
    fn receipts_badge(&self) -> Option<Line<'static>> {
        (!self.read_receipts()).then(|| {
            Line::styled(
                " ◌ receipts off ",
                Style::new().fg(theme::t().on_warn).bg(theme::warn_fill()).bold(),
            )
            .right_aligned()
        })
    }

    fn block(&self, title: String, focused: bool) -> Block<'static> {
        let t = theme::t();
        let (border, title_style) = if focused {
            (Style::new().fg(t.accent), Style::new().fg(t.text).bold())
        } else {
            (Style::new().fg(t.border), Style::new().fg(t.muted))
        };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title_style(title_style)
            .title(title)
    }

    /// A search result for a friend who has no chat listed: Enter starts one.
    fn friend_item(
        &self,
        mid: &str,
        words: &[String],
        is_sel: bool,
        focused: bool,
        text_w: usize,
        avatars: &HashSet<String>,
    ) -> ListItem<'static> {
        let name = self.name(mid);
        let bar = row_bar(is_sel, focused);
        let (tile_top, tile_bottom) = row_tile(mid, &name, avatars.contains(mid));
        let mut head = vec![bar.clone(), tile_top, Span::raw(" ")];
        head.extend(highlighted(&ellipsize(&name, text_w), words, Style::new().bold()));
        let note = ellipsize("friend · no messages yet", text_w);
        let body = vec![
            bar,
            tile_bottom,
            Span::raw(" "),
            Span::styled(note, theme::muted()),
        ];
        let (mut head, mut body) = (Line::from(head), Line::from(body));
        if is_sel {
            let sel_bg = Style::new().bg(theme::t().surface_hi);
            head = head.style(sel_bg);
            body = body.style(sel_bg);
        }
        ListItem::new(vec![head, body, Line::default()])
    }

    fn draw_chats(&mut self, f: &mut Frame, area: Rect) {
        let list_area = if self.searching {
            let search_area = Rect::new(area.x, area.y, area.width, area.height.min(3));
            let block = self.block(" Search ".into(), self.focus == Focus::Chats);
            let inner = block.inner(search_area);
            let (shown, cursor) = self.chat_query.view(inner.width as usize);
            f.render_widget(Paragraph::new(shown.to_owned()).block(block), search_area);
            if self.focus == Focus::Chats
                && self.modal.is_none()
                && self.profile.is_empty()
                && self.viewer.is_none()
                && inner.height > 0
            {
                f.set_cursor_position(Position::new(inner.x + cursor, inner.y));
            }
            Rect::new(
                area.x,
                area.y.saturating_add(search_area.height),
                area.width,
                area.height.saturating_sub(search_area.height),
            )
        } else {
            area
        };
        let list_rows = self.list_rows();
        let words = self.search_words();
        let inner_w = list_area.width.saturating_sub(2) as usize;
        // Text starts after the accent bar, the avatar tile and a gap.
        let text_w = inner_w.saturating_sub(CHAT_TEXT_X as usize + 1);
        let rows_h = CHAT_ROW_H as usize;
        // Avatars of the chats that can be on screen (fetched on first sight).
        let rows = list_area.height.saturating_sub(2) as usize / rows_h + 1;
        let first = self.list.offset().min(self.list.selected().unwrap_or(0));
        let near: Vec<String> = list_rows
            .iter()
            .skip(first.saturating_sub(rows))
            .take(rows * 3)
            .map(|r| self.row_mid(r).to_owned())
            .collect();
        let near_from = first.saturating_sub(rows);
        let mut avatars = HashSet::new();
        for (k, mid) in near.into_iter().enumerate() {
            let away = rows_away(near_from + k, first, rows) * CHAT_ROW_H as u32;
            if self.avatar_ready(&mid) {
                avatars.insert(mid);
            } else {
                self.media.nearer(&MediaKey::Avatar(mid), away);
            }
        }
        let focused = self.focus == Focus::Chats;
        let selected = self.list.selected();
        let sel_bg = Style::new().bg(theme::t().surface_hi);
        // Search results are ranked, not in pin order: no pinned divider (📌 stays).
        // A row position (hidden chats have no row).
        let pinned_row = |r: &ListRow| match r {
            ListRow::Chat(i) => !self.chats[*i].invited && self.is_pinned(&self.chats[*i].mid),
            ListRow::Friend(_) => false,
        };
        let pinned_end = if words.is_empty()
            && list_rows
                .iter()
                .any(|r| matches!(r, ListRow::Chat(i) if !self.chats[*i].invited) && !pinned_row(r))
        {
            list_rows.iter().rposition(pinned_row)
        } else {
            None
        };
        let friends_start = list_rows.iter().position(|r| matches!(r, ListRow::Friend(_)));
        let mut items: Vec<ListItem> = list_rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let chat_i = match row {
                    ListRow::Chat(chat_i) => *chat_i,
                    ListRow::Friend(mid) => {
                        return self.friend_item(mid, &words, selected == Some(i), focused, text_w, &avatars);
                    }
                };
                let c = &self.chats[chat_i];
                let name = self.name(&c.mid);
                let is_sel = selected == Some(i);
                let bar = row_bar(is_sel, focused);
                let (tile_top, tile_bottom) = row_tile(&c.mid, &name, avatars.contains(&c.mid));

                // Line 1: name ··· time
                let time = chat_time(c.last_time);
                let bell = if c.muted { " 🔕" } else { "" };
                let pin_mark = if self.is_pinned(&c.mid) && !c.invited {
                    "📌 "
                } else {
                    ""
                };
                let name_w = text_w.saturating_sub(time.width() + 1 + bell.width() + pin_mark.width());
                let shown_name = ellipsize(&name, name_w);
                let gap = text_w
                    .saturating_sub(pin_mark.width() + shown_name.width() + bell.width() + time.width());
                let unread = c.unread > 0;
                let name_style = if unread {
                    Style::new().bold().fg(theme::t().text)
                } else {
                    Style::new().bold()
                };
                let time_style = if unread {
                    Style::new().fg(theme::t().accent)
                } else {
                    theme::muted()
                };
                let mut head = vec![
                    bar.clone(),
                    tile_top,
                    Span::raw(" "),
                    Span::styled(pin_mark, name_style),
                ];
                head.extend(highlighted(&shown_name, &words, name_style));
                head.extend([
                    Span::styled(bell, name_style),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(time, time_style),
                ]);
                let head = Line::from(head);

                // Line 2: preview ··· unread badge
                let badge = if unread {
                    let n = if c.unread > 99 {
                        "99+".to_owned()
                    } else {
                        c.unread.to_string()
                    };
                    format!(" {n} ")
                } else {
                    String::new()
                };
                // A search result found by its members' names says whose.
                let mut members: Vec<&str> = Vec::new();
                if !words.is_empty() && name_rank(&name, &words).is_none() {
                    for m in words.iter().flat_map(|w| self.members_matching(c, w)) {
                        if !members.contains(&m) {
                            members.push(m);
                        }
                    }
                }
                let draft = self.drafts.get(&c.mid).map(|d| d.input.as_str());
                let flat: String = match draft {
                    _ if !members.is_empty() => format!("👥 {}", members.join(", ")),
                    Some(d) => format!("✎ {d}"),
                    None => c.preview.clone(),
                }
                .chars()
                .map(|ch| if ch == '\n' { ' ' } else { ch })
                .collect();
                let preview_w = text_w.saturating_sub(if unread { badge.width() + 1 } else { 0 });
                let preview = ellipsize(flat.trim(), preview_w);
                let gap = text_w.saturating_sub(preview.width() + badge.width());
                let preview_style = if c.invited {
                    Style::new().fg(theme::t().warn)
                } else if !members.is_empty() {
                    theme::muted()
                } else if draft.is_some() {
                    Style::new().fg(theme::t().draft).italic()
                } else if unread {
                    Style::new()
                } else {
                    theme::muted()
                };
                let mut body = vec![bar, tile_bottom, Span::raw(" ")];
                if members.is_empty() {
                    body.push(Span::styled(preview, preview_style));
                } else {
                    body.extend(highlighted(&preview, &words, preview_style));
                }
                body.push(Span::raw(" ".repeat(gap)));
                if unread {
                    body.push(Span::styled(
                        badge,
                        Style::new().fg(theme::t().on_accent).bg(theme::t().accent).bold(),
                    ));
                }
                let (mut head, mut body) = (head, Line::from(body));
                if is_sel {
                    head = head.style(sel_bg);
                    body = body.style(sel_bg);
                }
                // The pinned section ends in a dim divider; other entries
                // keep the usual blank spacer row.
                let spacer = if pinned_end == Some(i) {
                    let label = match &self.pin_list {
                        Some(list) => format!("── pinned · {list} ──"),
                        None => "── pinned ──".into(),
                    };
                    Line::from(Span::styled(label, theme::muted()))
                } else if friends_start == Some(i + 1) {
                    Line::from(Span::styled("── friends ──", theme::muted()))
                } else {
                    Line::default()
                };
                ListItem::new(vec![head, body, spacer])
            })
            .collect();
        if items.is_empty() && !words.is_empty() {
            let more = match self.more_chats {
                MoreChats::Ready(_) => "↓ to search older chats",
                MoreChats::Loading(_) => "searching older chats…",
                MoreChats::Done => "all chats searched",
            };
            let query = ellipsize(self.chat_query.as_str().trim(), inner_w.saturating_sub(22));
            items.push(ListItem::new(vec![
                Line::styled(format!(" no chat or friend matches “{query}”"), theme::muted()),
                Line::styled(format!(" {more}"), theme::muted()),
            ]));
        }
        let total_unread: u32 = self.chats.iter().map(|c| c.unread).sum();
        let title = if !words.is_empty() {
            match list_rows.len() {
                1 => " Chats · 1 match ".to_owned(),
                n => format!(" Chats · {n} matches "),
            }
        } else if total_unread > 0 {
            format!(" Chats · {total_unread} unread ")
        } else {
            " Chats ".into()
        };
        let title_w = title.width() as u16;
        let mut block = self.block(title, focused);
        let mut receipts_w = 0;
        if let Some(badge) = self.receipts_badge() {
            receipts_w = badge.width() as u16;
            block = block.title(badge);
        }
        let (more, loading) = if words.is_empty() {
            (" ↓ at the end: more chats ", " loading more chats… ")
        } else {
            (" ↓ at the end: search older chats ", " searching older chats… ")
        };
        let bottom = match self.more_chats {
            _ if list_rows.is_empty() => None,
            MoreChats::Ready(_) => Some(more),
            MoreChats::Loading(_) => Some(loading),
            MoreChats::Done => None,
        };
        if let Some(bottom) = bottom {
            block = block.title_bottom(Line::styled(bottom, theme::muted()));
        }
        let bottom_w = bottom.map_or(0, |b| b.width() as u16);
        self.hit.chats = block.inner(list_area);
        let list = List::new(items).block(block);
        f.render_stateful_widget(list, list_area, &mut self.list);
        self.draw_unread_indicators(f, list_area, (title_w, bottom_w), receipts_w);
        let inner = self.hit.chats;
        let visible: Vec<String> = list_rows
            .iter()
            .skip(self.list.offset())
            .take(inner.height as usize / rows_h + 1)
            .map(|r| self.row_mid(r).to_owned())
            .collect();
        for (k, mid) in visible.iter().enumerate() {
            if avatars.contains(mid) {
                let at = Position::new(inner.x + 1, inner.y + CHAT_ROW_H * k as u16);
                self.draw_avatar(f, mid, at, LIST_AVATAR, inner);
            }
        }
    }

    /// Unread chats out of view: "▲ n unread" on the list's top border
    /// (left of the receipts badge) and "▼ n unread" on its bottom one,
    /// from where the last render scrolled to. Each is a click target
    /// leading to the nearest such chat. `titles` are the widths of what the
    /// borders already say on the left (top, bottom) and `receipts_w` of
    /// the badge on the top right: free border space is used first, then a
    /// shorter label, and only then the end of a title (never " Chats ").
    fn draw_unread_indicators(&mut self, f: &mut Frame, area: Rect, titles: (u16, u16), receipts_w: u16) {
        self.hit.unread_above = None;
        self.hit.unread_below = None;
        if area.height < 2 {
            return;
        }
        let view = self.chat_view();
        let rows = self.unread_rows();
        let above = rows.iter().filter(|&&k| k < view.start).count();
        let below = rows.iter().filter(|&&k| k >= view.end).count();
        let style = Style::new().fg(theme::t().on_accent).bg(theme::t().accent).bold();
        let right = area.right().saturating_sub(1);
        let mut place = |count: usize, arrow: &str, y: u16, title_w: u16, right: u16| -> Option<Rect> {
            if count == 0 {
                return None;
            }
            let labels = [format!(" {arrow} {count} unread "), format!(" {arrow} {count} ")];
            let lefts = [
                area.x + 1 + title_w + 1,
                area.x + 1 + title_w.min(" Chats ".len() as u16),
            ];
            let (label, left) = lefts
                .iter()
                .flat_map(|&left| labels.iter().map(move |l| (l, left)))
                .find(|(l, left)| right >= left + l.width() as u16)?;
            let w = label.width() as u16;
            let r = Rect::new(right - w, y, w, 1);
            debug_assert!(r.x >= left);
            f.render_widget(Paragraph::new(Line::styled(label.clone(), style)), r);
            Some(r)
        };
        let top_right = if receipts_w > 0 {
            right.saturating_sub(receipts_w + 1)
        } else {
            right
        };
        self.hit.unread_above = place(above, "▲", area.y, titles.0, top_right);
        self.hit.unread_below = place(below, "▼", area.bottom() - 1, titles.1, right);
    }

    fn draw_conversation(&mut self, f: &mut Frame, area: Rect) {
        let title = match &self.open {
            Some(mid) => {
                let muted = self.chats.iter().any(|c| &c.mid == mid && c.muted);
                let mut t = format!(" {}{} ", self.name(mid), if muted { " 🔕" } else { "" });
                if let Some((who, until)) = self.typing.get(mid)
                    && *until > std::time::Instant::now()
                {
                    let who = if MidType::of(mid) == MidType::User {
                        String::new()
                    } else {
                        format!("{} ", self.name(who))
                    };
                    t.push_str(&format!("· ✎ {who}typing… "));
                }
                t
            }
            None => " rune ".into(),
        };
        let block = self.block(title, self.focus == Focus::Messages);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.hit.conv = inner;
        self.hit.conv_title = Rect::new(area.x, area.y, area.width, 1);
        self.hit.msgs.clear();
        self.hit.heads.clear();
        self.hit.links.clear();
        self.hit.reactions.clear();
        let mut heads: Vec<(usize, String)> = Vec::new();
        // Header avatars: row, column offset, sender.
        let mut head_avatars: Vec<(usize, u16, String)> = Vec::new();
        // Pictures asked for during layout, by row: placed once the window is known.
        let mut placed: Vec<(usize, MediaKey)> = Vec::new();
        // Links per row: row index, right-aligned, row width, links.
        let mut link_rows: Vec<(usize, bool, u16, Vec<LinkSpan>)> = Vec::new();
        let mut push_bubble = |rows: &mut Vec<Row>, b: Vec<BubbleRow>| {
            for (line, links) in b {
                if !links.is_empty() {
                    link_rows.push((rows.len(), mine_of(&line), line.width() as u16, links));
                }
                rows.push(Row::Text(line));
            }
        };
        let Some(chat) = self.open.clone() else {
            use keymap::{Cmd, Ctx};
            let key = Style::new().fg(theme::t().accent);
            let (help, typing_help) = (
                keymap::shortcut(Ctx::ChatList, Cmd::Help),
                keymap::global(Cmd::Help),
            );
            let help_what = if typing_help.is_empty() || typing_help == help {
                "all shortcuts".to_owned()
            } else {
                format!("all shortcuts ({typing_help} while typing)")
            };
            // The keys as they are now; one without a key is left out.
            let tips: Vec<(String, String)> = [
                (
                    keymap::shortcut(Ctx::ChatList, Cmd::Search),
                    "search chats and friends".to_owned(),
                ),
                (
                    keymap::shortcut(Ctx::ChatList, Cmd::NewGroup),
                    "new group".to_owned(),
                ),
                (
                    keymap::global(Cmd::MainMenu),
                    "menu: profile, pin lists, settings, plugins…".to_owned(),
                ),
                (if help.is_empty() { typing_help } else { help }, help_what),
            ]
            .into_iter()
            .filter(|(k, _)| !k.is_empty())
            .collect();
            let key_w = tips.iter().map(|(k, _)| k.width()).max().unwrap_or(0).max(7);
            let mut lines = vec![
                Line::raw(""),
                Line::raw("Pick a chat on the left to open it."),
                Line::raw(""),
            ];
            lines.extend(tips.into_iter().map(|(k, what)| {
                let pad = " ".repeat(key_w - k.width());
                Line::from(vec![Span::styled(format!("{pad}{k}  "), key), Span::raw(what)])
            }));
            let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
            let x = inner.x + inner.width.saturating_sub(width) / 2;
            let area = Rect::new(x, inner.y, width.min(inner.width), inner.height);
            f.render_widget(Paragraph::new(lines).style(theme::muted()), area);
            return;
        };
        let width = inner.width as usize;
        let height = inner.height as usize;
        // Borrowed out of `self` while drawing (and put back at the end)
        // rather than cloned on every frame.
        let stored = self.messages.remove(&chat);
        let msgs = stored.as_deref().unwrap_or_default();
        let mut rows: Vec<Row> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(msgs.len());
        if msgs.is_empty() && self.loading.contains(&chat) {
            rows.push(Row::Text(Line::from(Span::styled("loading…", theme::muted()))));
        } else if self.older_loading.contains(&chat) {
            rows.push(Row::Text(
                Line::from(Span::styled("loading older messages…", theme::muted())).centered(),
            ));
        } else if self.no_older.contains(&chat) {
            rows.push(Row::Text(
                Line::from(Span::styled("— beginning of the conversation —", theme::muted())).centered(),
            ));
        } else if !msgs.is_empty() {
            rows.push(Row::Text(
                Line::from(Span::styled("↑ scroll up for older messages", theme::muted())).centered(),
            ));
        } else {
            rows.push(Row::Text(
                Line::from(Span::styled(
                    "no messages yet — type below to start the conversation",
                    theme::muted(),
                ))
                .centered(),
            ));
        }
        let group_chat = MidType::of(&chat) != MidType::User;
        let read_state = self.read_state(&chat);
        let selecting = self.focus == Focus::Messages && self.viewer.is_none();
        let unread_index = self.unread_index(&chat, msgs);
        let mut unread_row = None;
        for (i, m) in msgs.iter().enumerate() {
            if unread_index == Some(i) {
                unread_row = Some(rows.len());
                rows.push(Row::Text(
                    Line::from(Span::styled(
                        "── Unread Messages ──",
                        Style::new().fg(theme::t().warn).bold(),
                    ))
                    .centered(),
                ));
            }
            let start = rows.len();
            let mine = m.from == self.me;
            let selected = selecting && self.sel == Some(i);
            let place = |l: Line<'static>| if mine { l.right_aligned() } else { l };
            let mut head = Vec::new();
            if selected {
                head.push(Span::styled("▶ ", Style::new().fg(theme::t().accent).bold()));
            }
            // Same one-column margin as the bubbles and thumbnails.
            if !mine {
                head.push(Span::raw(" "));
                placed.push((rows.len(), MediaKey::Avatar(m.from.clone())));
                // Others get their picture before the name, as in LINE.
                if self.avatar_ready(&m.from) {
                    let off = head.iter().map(Span::width).sum::<usize>() as u16;
                    head_avatars.push((rows.len(), off, m.from.clone()));
                    head.push(Span::raw("   "));
                }
            }
            head.push(Span::styled(
                self.name(&m.from),
                Style::new()
                    .fg(if mine {
                        theme::t().accent
                    } else {
                        sender_color(&m.from)
                    })
                    .bold(),
            ));
            head.push(Span::styled(format!("  {}", hhmm(m.time)), theme::muted()));
            if mine && !m.id.is_empty() && !m.unsent {
                let n = readers_in(&read_state, &m.id).len();
                let read = match (n, group_chat) {
                    (0, _) => String::new(),
                    (_, false) => "  ✓ Read".into(),
                    (n, true) => format!("  ✓ Read by {n}"),
                };
                head.push(Span::styled(read, Style::new().fg(theme::t().accent)));
            }
            if mine {
                head.push(Span::raw(" "));
            }
            heads.push((rows.len(), m.from.clone()));
            rows.push(Row::Text(place(Line::from(head))));
            let quote_w = (width * 3 / 4)
                .max(24)
                .min(width.saturating_sub(1))
                .saturating_sub(4);
            let quote = match &m.reply_to {
                Some(id) => Some(format!(
                    "↩ {}",
                    self.quote_line(msgs.iter().find(|q| q.id == *id), quote_w)
                )),
                None if m.forwarded => Some("↪ Forwarded".to_owned()),
                None => None,
            };
            if let Some(q) = quote {
                let q = Span::styled(q, theme::muted().italic());
                let line = if mine {
                    Line::from(vec![q, Span::raw(" ")])
                } else {
                    Line::from(vec![Span::raw(" "), q])
                };
                rows.push(Row::Text(place(line)));
            }
            let mut body_style = if m.unsent {
                theme::muted().italic()
            } else {
                Style::new()
            };
            let mut border = Style::new().fg(if mine {
                theme::t().accent
            } else {
                sender_color(&m.from)
            });
            if m.unsent {
                border = theme::muted();
            }
            if selected {
                body_style = body_style.bg(theme::t().surface_hi);
                border = Style::new().fg(theme::t().text).bold();
            }
            // Bubbles take at most three quarters of the pane (but stay readable when narrow).
            let bubble_w = (width * 3 / 4).max(24).min(width.saturating_sub(1));
            let mut thumb = None;
            if m.is_visual() && (self.prefs.auto_pictures || self.thumbs.contains_key(&m.id)) {
                match self.thumbs.get(&m.id) {
                    Some(Load::Ready(p)) => {
                        let (tw, th) = THUMB_SIZES[self.thumb_size];
                        let max = Size::new(tw.min(inner.width.saturating_sub(2)), th);
                        let sz = p.size_for(max);
                        thumb = Some(sz);
                    }
                    Some(Load::Loading) => {}
                    Some(Load::Failed(_)) => {}
                    None => {
                        if let Some(raw) = m.raw.clone() {
                            let key = MediaKey::Thumb(m.id.clone());
                            self.media
                                .want(key.clone(), fetchq::UNPLACED, MediaStart::Thumb(Box::new(raw)));
                            placed.push((start, key));
                        }
                    }
                }
            }
            match thumb {
                Some(sz) if sz.height > 0 && sz.width > 0 => {
                    rows.push(Row::Image {
                        id: m.id.clone(),
                        w: sz.width,
                        h: sz.height,
                        right: mine,
                    });
                    for _ in 1..sz.height {
                        rows.push(Row::Pad);
                    }
                    if m.raw.as_ref().is_some_and(|r| media_type(r) == ct::VIDEO) {
                        let mut text = m.text.clone();
                        if selected {
                            text.push_str("   ⏎ play");
                        }
                        push_bubble(&mut rows, bubble(&text, bubble_w, mine, border, body_style));
                    }
                    if m.raw.as_ref().is_some_and(Message::is_animated_sticker) {
                        let hint = if selected {
                            "▶ animated · ⏎ play"
                        } else {
                            "▶ animated"
                        };
                        let span = Span::styled(hint, theme::muted());
                        let line = if mine {
                            Line::from(vec![span, Span::raw(" ")]).right_aligned()
                        } else {
                            Line::from(vec![Span::raw(" "), span])
                        };
                        rows.push(Row::Text(line));
                    }
                    for code in self.qr.get(&m.id).into_iter().flatten() {
                        push_bubble(
                            &mut rows,
                            bubble(&format!("▣ QR  {code}"), bubble_w, mine, border, body_style),
                        );
                    }
                }
                _ => {
                    let mut text = m.text.clone();
                    if matches!(self.thumbs.get(&m.id), Some(Load::Loading))
                        || self.media.is_waiting(&MediaKey::Thumb(m.id.clone()))
                    {
                        text.push_str(" …");
                    }
                    if selected && m.plays() {
                        text.push_str("   ⏎ play · v view · s save");
                    } else if selected && m.media_kind().is_some() {
                        text.push_str("   ⏎ view · s save · o open");
                    }
                    if selected && m.media_kind().is_none() && !links::find_links(&text).is_empty() {
                        text.push_str("   ⏎/u open link");
                    }
                    push_bubble(&mut rows, bubble(&text, bubble_w, mine, border, body_style));
                }
            }
            if !m.reactions.is_empty() && !m.unsent {
                rows.push(Row::Reaction {
                    id: m.id.clone(),
                    line: place(self.reaction_line(&m.reactions, mine)),
                });
            }
            spans.push((start, rows.len()));
            rows.push(Row::Text(Line::raw("")));
        }
        for (c, label) in &self.uploads {
            if *c == chat {
                rows.push(Row::Text(
                    Line::from(vec![
                        Span::styled(format!("⏫ uploading {label}…"), theme::muted().italic()),
                        Span::raw(" "),
                    ])
                    .right_aligned(),
                ));
            }
        }

        // Window: `scroll` rows up from the bottom, but keep the selection visible.
        let total = rows.len();
        let max_scroll = total.saturating_sub(height);
        let follow = std::mem::take(&mut self.follow_sel);
        if let (true, Some((s0, s1))) = (selecting && follow, self.sel.and_then(|i| spans.get(i).copied())) {
            let win_start = total.saturating_sub(height + self.scroll);
            if s0 < win_start {
                self.scroll = total.saturating_sub(height + s0);
            } else if s1 > win_start + height {
                self.scroll = total.saturating_sub(s1.max(height));
            }
        }
        if self.jump_unread
            && let Some(divider_row) = unread_row
        {
            let current_start = total.saturating_sub(height + self.scroll.min(max_scroll));
            if !(current_start..current_start.saturating_add(height)).contains(&divider_row) {
                let target_start = divider_row.saturating_sub(1);
                self.scroll = total.saturating_sub(height + target_start);
            }
            self.jump_unread = false;
        }
        self.scroll = self.scroll.min(max_scroll);
        if self.scroll > 0 && area.height > 2 && area.width > 4 {
            let key = if self.focus == Focus::Messages {
                "End"
            } else {
                "PgDn"
            };
            let label = Line::styled(format!(" ↓ newer messages below · {key} "), Style::new().yellow())
                .right_aligned();
            let row = Rect::new(area.x + 2, area.y + area.height - 1, area.width - 4, 1);
            f.render_widget(Paragraph::new(label), row);
        }
        let start = total.saturating_sub(height + self.scroll);
        self.at_top = start == 0 && !msgs.is_empty();
        for (r, key) in &placed {
            self.media.nearer(key, rows_away(*r, start, height));
        }
        for (i, &(s0, s1)) in spans.iter().enumerate() {
            let (y0, y1) = (s0.max(start), s1.min(start + height));
            if y0 < y1 {
                let y = |r: usize| inner.y + (r - start) as u16;
                self.hit.msgs.push((y(y0), y(y1), i));
            }
        }
        for (r, from) in heads {
            if (start..start + height).contains(&r) {
                self.hit.heads.push((inner.y + (r - start) as u16, from));
            }
        }
        for (r, row) in rows.iter().enumerate() {
            if (start..start + height).contains(&r)
                && let Row::Reaction { id, .. } = row
            {
                self.hit
                    .reactions
                    .push((inner.y + (r - start) as u16, id.clone()));
            }
        }
        for (r, right, line_w, links) in link_rows {
            if !(start..start + height).contains(&r) {
                continue;
            }
            let x0 = if right {
                inner.x + inner.width.saturating_sub(line_w)
            } else {
                inner.x
            };
            for l in links {
                let rect = Rect::new(x0 + l.off, inner.y + (r - start) as u16, l.w, 1).intersection(inner);
                self.hit.links.push((rect, l.url));
            }
        }
        let visible: Vec<&Row> = rows.iter().skip(start).take(height).collect();
        let lines: Vec<Line> = visible
            .iter()
            .map(|r| match r {
                Row::Text(l) => l.clone(),
                Row::Reaction { line, .. } => line.clone(),
                _ => Line::raw(""),
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
        for (r, off, from) in head_avatars {
            if (start..start + height).contains(&r) {
                let at = Position::new(inner.x + off, inner.y + (r - start) as u16);
                self.draw_avatar(f, &from, at, HEAD_AVATAR, inner);
            }
        }
        // Thumbnails, cropped to the rows inside the window.
        for (r0, r) in rows.iter().enumerate() {
            let Row::Image { id, w, h, right } = r else {
                continue;
            };
            let (top, bottom) = (r0.max(start), (r0 + *h as usize).min(start + height));
            if top >= bottom {
                continue;
            }
            let Some(Load::Ready(p)) = self.thumbs.get_mut(id) else {
                continue;
            };
            let x = if *right {
                inner.x + inner.width.saturating_sub(*w + 1)
            } else {
                inner.x + 1
            };
            let rect = Rect::new(x, inner.y + (top - start) as u16, *w, (bottom - top) as u16);
            let skip = (top - r0) as u16;
            p.render(f, rect, Size::new(*w, *h), skip, self.cell_mode, THUMB_FILTER);
        }
        if let Some(list) = stored {
            self.messages.insert(chat, list);
        }
    }

    /// " 👍 2  ❤️ 1 " under a message; our own reaction is highlighted.
    fn reaction_line(&self, reactions: &[MessageReaction], mine_msg: bool) -> Line<'static> {
        let mut counts: Vec<(&'static str, usize, bool)> = Vec::new();
        for r in reactions {
            let emoji = reaction_glyph(&r.kind);
            let me = r.from == self.me;
            match counts.iter_mut().find(|c| c.0 == emoji) {
                Some(c) => {
                    c.1 += 1;
                    c.2 |= me;
                }
                None => counts.push((emoji, 1, me)),
            }
        }
        let mut spans = Vec::new();
        if !mine_msg {
            spans.push(Span::raw("  "));
        }
        for (emoji, n, me) in counts {
            let style = if me {
                Style::new().bg(theme::t().accent_soft).bold()
            } else {
                Style::new().bg(theme::t().surface)
            };
            let label = if n > 1 {
                format!(" {emoji} {n} ")
            } else {
                format!(" {emoji} ")
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        if mine_msg {
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    }

    fn draw_input(&mut self, f: &mut Frame, area: Rect) {
        self.hit.input = area;
        let title = match (&self.replying, &self.open) {
            (Some(id), Some(chat)) => {
                let max = (area.width as usize).saturating_sub(30).max(10);
                format!(
                    " ↩ Reply to {} · {} ",
                    self.quote(chat, id, max),
                    keymap::with_keys(keymap::Ctx::Compose, keymap::Cmd::Back, "cancel")
                )
            }
            _ => match self.input.line_count() {
                1 => " Message ".into(),
                n => format!(" Message · {n} lines "),
            },
        };
        let mut block = self.block(title, self.focus == Focus::Input);
        if let Some(badge) = self.receipts_badge() {
            block = block.title(badge);
        }
        let inner = block.inner(area);
        let (rows, (x, y)) = self.input.view_rows(inner.width as usize, inner.height as usize);
        let lines: Vec<Line> = rows.into_iter().map(|r| Line::raw(r.to_owned())).collect();
        f.render_widget(Paragraph::new(lines).block(block), area);
        if self.focus == Focus::Input
            && self.modal.is_none()
            && self.profile.is_empty()
            && self.viewer.is_none()
            && inner.width > 0
            && inner.height > 0
        {
            let at = Position::new(
                inner.x + x.min(inner.width - 1),
                inner.y + y.min(inner.height - 1),
            );
            f.set_cursor_position(at);
        }
    }
}

// -- startup ------------------------------------------------------------------------------

async fn login(path: &str) -> Result<Session, Box<dyn std::error::Error>> {
    println!("No session at {path}; logging in by QR code.");
    let client = Client::new(Config::default())?;
    let res = client
        .qr_login(QrLoginOptions::default(), |ev| match ev {
            QrLoginEvent::QrUrl(url) => {
                if let Ok(code) = QrCode::new(url.as_bytes()) {
                    println!("{}", code.render::<Dense1x2>().quiet_zone(true).build());
                }
                println!("Scan with the LINE app.");
            }
            QrLoginEvent::Pin(pin) => println!("Enter PIN {pin} on your phone."),
        })
        .await?;
    let s = Session {
        tokens: client.tokens(),
        e2ee_keys: res.e2ee_keys,
    };
    s.save(path)?;
    Ok(s)
}

/// The full-screen page shown when `ltsm.wasm` is missing: what is wrong and how
/// to fix it, until a key is pressed. Without a terminal (or if drawing fails)
/// the explanation goes to stderr instead.
fn show_missing_wasm(problem: &rune::ltsm::WasmProblem) {
    use crossterm::event::{Event, KeyEventKind, read};
    let help = problem.help();
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        eprintln!("{help}");
        return;
    }
    let mut terminal = ratatui::init();
    let lines: Vec<Line> = help
        .lines()
        .enumerate()
        .map(|(i, l)| match i {
            0 => Line::styled(
                l.to_owned(),
                Style::default()
                    .fg(theme::t().danger)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            _ if l.starts_with("  ") => Line::styled(l.to_owned(), theme::muted()),
            _ => Line::raw(l.to_owned()),
        })
        .chain([
            Line::raw(""),
            Line::styled("Press any key to quit.", theme::muted()),
        ])
        .collect();
    let drawn = terminal.draw(|f| {
        let block = Block::bordered()
            .title(" rune — secure module needed ")
            .border_style(Style::default().fg(theme::t().border));
        f.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(ratatui::widgets::Wrap { trim: false }),
            f.area(),
        );
    });
    if drawn.is_ok() {
        while let Ok(ev) = read() {
            if matches!(ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                break;
            }
        }
    }
    ratatui::restore();
    // Kept on screen after the alternate screen is gone, so the paths can be copied.
    eprintln!("{help}");
}

/// The saved session at `path`, or `None` when there is none yet (log in).
/// An unreadable or damaged file is an error, never replaced by a new login:
/// it holds the device certificate and the Letter Sealing keys.
fn read_session(path: &str) -> Result<Option<Session>, String> {
    match Session::load(path) {
        Ok(s) => Ok(Some(s)),
        Err(rune::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "cannot read the session {path}: {e}\n\
             Fix it, or move it away to log in again by QR code."
        )),
    }
}

async fn run(
    mut terminal: DefaultTerminal,
    mut app: App,
    mut rx: mpsc::UnboundedReceiver<AppEvent>,
) -> std::io::Result<()> {
    let mut keys = EventStream::new();
    let mut last_burst: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut had_overlay = (false, false, false);
    // A terminal error (it hung up) ends the loop like quitting, so the
    // draft is still saved and plugins still shut down.
    let mut failure = None;
    while !app.quit {
        // Pictures under an overlay are drawn as text cells (see
        // `picture::compose`), but terminals differ in whether text written
        // over Sixel pixels erases them: repaint everything when an overlay
        // opens, closes or gives way to another.
        let overlay = (app.modal.is_some(), !app.profile.is_empty(), app.viewer.is_some());
        if overlay != had_overlay {
            had_overlay = overlay;
            if app.picker.is_some() {
                app.needs_clear = true;
            }
        }
        if app.needs_clear {
            // Wipe graphics (Sixel/Kitty) left behind by a closed viewer or overlay.
            // Not `Terminal::clear`: it saves and restores the cursor by asking
            // the terminal where it is (`ESC[6n`), and a terminal busy drawing
            // pictures may not answer within crossterm's 2 s, which ended the
            // app. `resize` to the same size clears the whole screen and
            // forces a full redraw without asking; every frame places the
            // cursor itself anyway.
            if let Err(e) = terminal.size().and_then(|size| terminal.resize(size.into())) {
                failure = Some(e);
                break;
            }
            app.needs_clear = false;
        }
        app.report_store_failure();
        app.run_retries();
        app.run_plugin_timers();
        app.flush_receipts();
        app.sync_anim();
        app.maybe_more_chats();
        app.note_status();
        if let Err(e) = terminal.draw(|f| app.draw(f)) {
            failure = Some(e);
            break;
        }
        app.start_media();
        app.request_missing_names();
        let next_retry = app.retry.next_due();
        let next_plugin_timer = app.plugins.next_timer();
        tokio::select! {
            ev = keys.next() => {
                // Everything the terminal has already sent is handled together,
                // so a paste typed out as keys can be told from typing. `poll!`
                // polls with this task's waker: crossterm's `EventStream` keeps
                // the waker of the first poll that found nothing (until it fires),
                // so a throwaway waker (`now_or_never`) would leave `select!`
                // never woken for later keys.
                let mut batch = vec![ev];
                while batch.len() < MAX_BATCH && matches!(batch.last(), Some(Some(_))) {
                    match futures_util::poll!(keys.next()) {
                        std::task::Poll::Ready(ev) => batch.push(ev),
                        std::task::Poll::Pending => break,
                    }
                }
                let continued = last_burst.is_some_and(|t| t.elapsed() < BURST_GAP);
                let pasting = is_paste_burst(&batch, continued);
                if pasting {
                    last_burst = Some(Instant::now());
                }
                for ev in batch {
                    if app.quit {
                        break;
                    }
                    app.pasting = pasting;
                    match ev {
                        Some(Ok(TermEvent::Key(k))) => app.on_key(k),
                        Some(Ok(TermEvent::Mouse(m))) => app.on_mouse(m),
                        Some(Ok(TermEvent::Paste(text))) => app.on_paste(&text),
                        Some(Ok(TermEvent::FocusGained)) => app.on_focus(true),
                        Some(Ok(TermEvent::FocusLost)) => app.on_focus(false),
                        // A resize just redraws.
                        Some(Ok(_)) => {}
                        Some(Err(e)) => app.status = format!("terminal input error: {e}"),
                        // The terminal went away: nothing can reach the app any more.
                        None => app.quit = true,
                    }
                }
                app.pasting = false;
            },
            Some(ev) = rx.recv() => app.on_event(ev),
            _ = tick.tick() => {} // refresh relative timestamps
            // A queued retry is due (run at the top of the loop).
            _ = async {
                match next_retry {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending().await,
                }
            } => {}
            // A plugin timer is due (run at the top of the loop).
            _ = async {
                match next_plugin_timer {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending().await,
                }
            } => {}
        }
    }
    app.save_open_draft();
    app.plugins.shutdown_all();
    failure.map_or(Ok(()), Err)
}

/// SIGTERM (`kill`, a system shutdown) and SIGHUP (the terminal window
/// closed) end the app the way quitting does: the draft is saved, plugins
/// shut down and the terminal is restored if it is still there. A second
/// signal ends it at once, in case the event loop is stuck.
fn spawn_signal_watch(tx: mpsc::UnboundedSender<AppEvent>) {
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup()))
        else {
            tracing::warn!("cannot watch for SIGTERM/SIGHUP; they end rune-tui without saving the draft");
            return;
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM: quitting"),
            _ = hup.recv() => tracing::info!("SIGHUP: quitting"),
        }
        let _ = tx.send(AppEvent::Terminate);
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
        tracing::warn!("second signal: exiting at once");
        std::process::exit(1);
    });
    #[cfg(not(unix))]
    drop(tx);
}

/// Columns of the chat list (narrower on small terminals, down to the minimum).
const CHAT_LIST_W: u16 = 38;
const CHAT_LIST_MIN_W: u16 = 26;

/// Events read in one go, at most (a long paste takes several reads).
const MAX_BATCH: usize = 4096;

/// How long a status message stays at least (see `App::expire_status`).
const STATUS_KEEP: Duration = Duration::from_secs(8);
/// ... and one that reports a failure.
const STATUS_ERROR_KEEP: Duration = Duration::from_secs(30);
/// This many key presses read in one go are a paste, not typing: a person
/// gets that far ahead of the screen only when it stalls.
const BURST_KEYS: usize = 8;
/// A read this soon after a paste burst is the rest of it (a paste longer
/// than one read), whatever its size.
const BURST_GAP: Duration = Duration::from_millis(15);

/// Whether events read in one go are a paste that a terminal without
/// bracketed paste typed out as keys: then its Enters are line breaks, so
/// pasting text that has (or ends in) a line break never sends a message.
fn is_paste_burst(batch: &[Option<std::io::Result<TermEvent>>], continued: bool) -> bool {
    let presses = batch
        .iter()
        .filter(|ev| matches!(ev, Some(Ok(TermEvent::Key(k))) if k.kind == KeyEventKind::Press))
        .count();
    presses > 0 && (continued || presses >= BURST_KEYS)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match start().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Displayed, not debug-printed: these messages tell the user what to do.
            eprintln!("rune-tui: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn start() -> Result<(), Box<dyn std::error::Error>> {
    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Run(o)) => o,
        Ok(cli::Command::Help) => {
            print!("{}", cli::USAGE);
            return Ok(());
        }
        Ok(cli::Command::Version) => {
            println!("rune-tui {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    if let Some(log) = &opts.log {
        init_log(log).map_err(|e| format!("cannot open the log file {}: {e}", log.display()))?;
    }
    // rune's data directory (`~/.local/share/rune`): the default session, the
    // message store and plugins live there, and it is where `ltsm.wasm` goes, so it
    // exists from the first start on. It will hold keys and messages: owner-only.
    let data_dir = cli::data_home().map(|h| h.join("rune"));
    let data_dir_error = data_dir.as_deref().and_then(|d| {
        make_private_dir(d)
            .err()
            .map(|e| format!("cannot create {}: {e}", d.display()))
    });
    if let Some(e) = &data_dir_error {
        tracing::warn!("{e}");
    }
    // LINE's secure module is the user's own copy; without it nothing can be signed.
    if let Err(problem) = rune::ltsm::load_wasm() {
        show_missing_wasm(&problem);
        return Err(problem.to_string().into());
    }
    let session_path = cli::session_path(opts.session.as_deref(), cli::data_home());
    let path = session_path.to_string_lossy().into_owned();
    let session = match read_session(&path)? {
        Some(s) => s,
        None => {
            // A login is written to the data directory unless a path was given.
            if let Some(e) = data_dir_error.filter(|_| data_dir.as_deref() == session_path.parent()) {
                return Err(e.into());
            }
            login(&path).await?
        }
    };
    let client = session.into_client(Config::default())?;
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_signal_watch(tx.clone());
    // Persist refreshed tokens; a failure is shown, since the next start
    // would otherwise begin with a token LINE has already replaced.
    let (saver, save_path, save_tx) = (client.clone(), path.clone(), tx.clone());
    let mut token_rx = client.subscribe_tokens();
    tokio::spawn(async move {
        while token_rx.changed().await.is_ok() {
            if let Err(e) = Session::from_client(&saver).save(&save_path) {
                tracing::error!("saving the session to {save_path} failed: {e}");
                let _ = save_tx.send(AppEvent::Status(format!(
                    "could not save the renewed login to {save_path}: {e}"
                )));
            }
        }
    });
    let me = match client.my_mid() {
        Some(m) => m,
        None => client.profile().await?.mid,
    };

    // The local store: next to the session unless told otherwise.
    let store = if opts.no_store {
        Store::open_in_memory()?
    } else {
        let db = opts
            .db
            .clone()
            .unwrap_or_else(|| std::path::Path::new(&path).with_extension("db"));
        Store::open(&db).map_err(|e| format!("cannot open the message store {}: {e}", db.display()))?
    };
    if store.bind_account(&me)? {
        eprintln!("The message store belonged to another account; it was cleared.");
    }
    // Names used to be kept here before the store had a user table.
    let _ = store.remove("cache.user_names");
    let _ = store.prune_images(IMAGE_CACHE_BYTES);
    // Files written for the system viewer and mpv; they are copies of (decrypted) media.
    match prune_dir(
        &view_dir(),
        VIEW_CACHE_MAX_AGE,
        VIEW_CACHE_BYTES,
        std::time::SystemTime::now(),
    ) {
        Ok(n) if n > 0 => tracing::info!("removed {n} old files from {}", view_dir().display()),
        Ok(_) => {}
        Err(e) => tracing::warn!("pruning {} failed: {e}", view_dir().display()),
    }
    let resume_at = store.stream_revision()?;

    spawn_chat_list(client.clone(), tx.clone());
    spawn_contacts(client.clone(), tx.clone(), store.clone(), false);
    spawn_stream(client.clone(), tx.clone(), resume_at);
    // Renew the token ahead of time (not only when LINE rejects it).
    let (refresher, rtx) = (client.clone(), tx.clone());
    tokio::spawn(async move {
        if let Err(e) = refresher.run_token_refresher().await {
            let _ = rtx.send(AppEvent::Status(format!("session expired — log in again ({e})")));
        }
    });
    let terminal = ratatui::init();
    // Query the terminal's image protocol (after entering the alternate
    // screen, before reading key events, as ratatui-image requires).
    let picker = if opts.graphics {
        Picker::from_query_stdio()
            .ok()
            .filter(|p| p.protocol_type() != ProtocolType::Halfblocks)
    } else {
        None
    };
    // Failing to turn these on is not worth leaving the terminal in raw mode for.
    let keyboard_enhancement = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )
        .is_ok();
    let mouse = opts.mouse && crossterm::execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste, EnableFocusChange);
    // ratatui's panic hook restores raw mode and the screen; switch the
    // extra modes off too, or a panic leaves the shell receiving mouse codes.
    let restore_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        disable_terminal_modes(keyboard_enhancement, mouse);
        restore_hook(info);
    }));
    let plugin_dir = opts.plugins.clone().or_else(plugins::default_dir);
    let mut app = App::new(client, me, tx, store.clone());
    app.plugins = plugins::Plugins::new(store, plugin_dir, opts.no_plugins, keymap::CORE_COMMANDS);
    app.start_plugins();
    app.no_read_receipts_flag = !opts.read_receipts;
    app.use_default_keys(opts.default_keys);
    if let Some(file) = shortcuts::default_file() {
        app.use_keys_file(file);
    }
    app.cell_mode = opts.cells;
    app.picker = picker;
    app.status = if app.chats.is_empty() {
        "loading chats…".into()
    } else {
        format!("{} chats · syncing…", app.chats.len())
    };
    let result = run(terminal, app, rx).await;
    disable_terminal_modes(keyboard_enhancement, mouse);
    ratatui::restore();
    Ok(result?)
}

/// Send `tracing` output (the library's and ours) to `path`, appending,
/// created owner-only. The level comes from `RUST_LOG` (default `info`).
/// Tokens and message text are never logged, at any level.
fn init_log(path: &std::path::Path) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("rune=info,rune_tui=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init()
        .map_err(std::io::Error::other)?;
    tracing::info!("rune-tui {} starting", env!("CARGO_PKG_VERSION"));
    Ok(())
}

/// Undo the terminal modes `main` turned on (on exit and on a panic).
fn disable_terminal_modes(keyboard_enhancement: bool, mouse: bool) {
    let mut out = std::io::stdout();
    let _ = crossterm::execute!(out, DisableBracketedPaste, DisableFocusChange);
    if keyboard_enhancement {
        let _ = crossterm::execute!(out, crossterm::event::PopKeyboardEnhancementFlags);
    }
    if mouse {
        let _ = crossterm::execute!(out, DisableMouseCapture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::MediaKind;

    #[tokio::test]
    async fn unknown_names_are_looked_up_once_per_retry_window() {
        let mut a = app();
        let stranger = "u0123456789abcdef0123456789abcdef".to_owned();
        assert!(a.name(&stranger).ends_with('…'));
        assert!(a.name_misses.borrow().contains(&stranger));
        a.request_missing_names();
        assert!(a.name_requests.contains_key(&stranger));
        // Asked again within the window: no new request.
        let first = a.name_requests[&stranger];
        a.want_names([stranger.clone()]);
        assert_eq!(a.name_requests[&stranger], first);
        // Squares, junk and our own mid are never sent.
        a.want_names(["".into(), "s0123456789".into(), "Ume".into()]);
        assert_eq!(a.name_requests.len(), 1);
    }

    #[test]
    fn stranger_names_survive_a_restart() {
        let a = app();
        let mid = "u0123456789abcdef0123456789abcdef".to_owned();
        let mut a = a;
        a.names.insert(mid.clone(), "Carol".into());
        a.picture_paths.insert(mid.clone(), "/p/x".into());
        a.persist_users(std::slice::from_ref(&mid));
        let u = a.store.user(&mid).unwrap().unwrap();
        assert_eq!((u.name.as_str(), u.picture.as_deref()), ("Carol", Some("/p/x")));
        // What a profile page stored is kept when the name changes.
        a.store
            .upsert_user(&UserRow {
                status: "busy".into(),
                ..u
            })
            .unwrap();
        a.names.insert(mid.clone(), "Caz".into());
        a.persist_users(std::slice::from_ref(&mid));
        let u = a.store.user(&mid).unwrap().unwrap();
        assert_eq!((u.name.as_str(), u.status.as_str()), ("Caz", "busy"));
        // A fresh App reads the names back.
        let (tx, _rx) = mpsc::unbounded_channel();
        let b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            a.store.clone(),
        );
        assert_eq!(b.name(&mid), "Caz");
    }

    #[test]
    fn failed_history_stays_stale_and_is_queued() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.stale.insert("Ua".into());
        a.loading.insert("Ua".into());
        a.on_event(AppEvent::Failed(
            Job::History("Ua".into()),
            "history failed: offline".into(),
        ));
        assert!(!a.loading.contains("Ua"));
        // Still marked: the retry (or opening it again) fetches it.
        assert!(a.stale.contains("Ua"));
        assert!(a.retry.contains(&Job::History("Ua".into())));
        assert_eq!(a.status, "history failed: offline — retrying in 2s");
        // Messages already shown stay.
        assert_eq!(a.messages["Ua"].len(), 3);
    }

    #[test]
    fn failed_avatars_are_retried_on_their_own_schedule() {
        let mut a = app();
        a.on_avatar("Ua".into(), None);
        assert!(matches!(a.avatars.get("Ua"), Some(Load::Failed(_))));
        a.on_event(AppEvent::Failed(Job::ChatList, "chat list failed".into()));
        // The chat list comes through: that does not move the avatar's retry.
        a.on_event(AppEvent::Chats(Vec::new()));
        assert!(!a.retry.contains(&Job::ChatList));
        a.run_retries();
        assert!(matches!(a.avatars.get("Ua"), Some(Load::Failed(_))));
        assert!(a.retry.contains(&Job::Avatar("Ua".into())));
        // When its own time comes, the failure is dropped and the next draw asks again.
        let due = a.retry.next_due().unwrap();
        assert_eq!(a.retry.take_due(due), vec![Job::Avatar("Ua".into())]);
        a.retry
            .failed(Job::Avatar("Ua".into()), Instant::now() - Duration::from_secs(60));
        a.run_retries();
        assert!(!a.avatars.contains_key("Ua"));
        // It loads: only its own entry goes.
        a.on_avatar("Ua".into(), Some(image::DynamicImage::new_rgba8(4, 4)));
        assert!(!a.retry.contains(&Job::Avatar("Ua".into())));
    }

    #[test]
    fn nameless_groups_are_hidden_from_the_chat_list() {
        let mut a = app();
        for mid in ["Cgone", "Cnamed", "Rroom", "Cinvite", "Copen"] {
            a.chats.push(ChatEntry {
                mid: mid.into(),
                invited: mid == "Cinvite",
                ..Default::default()
            });
        }
        a.names.insert("Cnamed".into(), "Club".into());
        a.open = Some("Copen".into());
        let listed: Vec<&str> = a
            .visible_chats()
            .iter()
            .map(|&i| a.chats[i].mid.as_str())
            .collect();
        // Users without a name stay (shown by mid until it arrives).
        assert_eq!(listed, ["Ua", "Ub", "Cnamed", "Cinvite", "Copen"]);
        // Not found by a search either.
        a.chat_query = TextInput::from("c".to_owned());
        assert!(a.visible_chats().iter().all(|&i| a.chats[i].mid != "Cgone"));
        a.chat_query = TextInput::default();
        // Its name arrives: listed again, the selection kept on its chat.
        a.list.select(Some(2));
        a.learn_names(HashMap::from([("Cgone".to_owned(), "Back".to_owned())]));
        assert_eq!(a.selected_mid().as_deref(), Some("Cnamed"));
        assert!(a.visible_chats().iter().any(|&i| a.chats[i].mid == "Cgone"));
    }

    #[tokio::test]
    async fn hidden_groups_are_looked_up_once() {
        let mut a = app();
        let gone = "CdpW4R3F0NeM_h2mSKjJxSVY4aHmqIiqH1_PpkKKcLkk".to_owned();
        a.chats.push(ChatEntry {
            mid: gone.clone(),
            ..Default::default()
        });
        // Never drawn, still asked for.
        a.request_missing_names();
        assert!(a.name_requests.contains_key(&gone));
        // LINE answered without it (a group we left): not asked again this run.
        a.on_event(AppEvent::LookedUp(vec![gone.clone()]));
        a.name_requests.clear();
        a.request_missing_names();
        assert!(a.name_requests.is_empty());
    }

    #[test]
    fn pinned_divider_follows_rows_not_chat_indices() {
        let mut a = app();
        a.chats.insert(
            0,
            ChatEntry {
                mid: "Cgone".into(),
                ..Default::default()
            },
        );
        a.pinned = vec!["Cgone".into(), "Ua".into()];
        let rows = a.list_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(a.row_mid(&rows[0]), "Ua");
        // The hidden pinned group has no row: the divider goes under "Ua",
        // not under the second row (its index among all chats).
        let s = screen(&mut a);
        let (div, ua, ub) = (
            s.find("── pinned ──").expect("divider"),
            s.find("Ua…").unwrap(),
            s.find("Ub…").unwrap(),
        );
        assert!(ua < div && div < ub, "{s}");
    }

    /// A 44-character base64url mid, the format most mids have now (live).
    const B64_USER: &str = "Ud_x-7Kq2mZ0vN3pR8sT1uW4yA6bC9eF5gH0jL2nP4rs";

    #[test]
    fn base64url_mids_are_looked_up() {
        assert_eq!(B64_USER.len(), 44);
        assert!(resolvable(B64_USER));
        assert!(resolvable("CdpW4R3F0NeM_h2mSKjJxSVY4aHmqIiqH1_PpkKKcLkk"));
        assert!(resolvable("u0123456789abcdef0123456789abcdef"));
        // Squares, short ids and anything that is not a mid stay out.
        for bad in [
            "",
            "Ume",
            "s0123456789",
            "Uabc/def0123",
            "Uabc def0123",
            "Uäöü01234567",
        ] {
            assert!(!resolvable(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn drawn_avatar_without_picture_is_looked_up_once() {
        let mut a = app();
        let mid = B64_USER.to_owned();
        // A name from the store, but no picture: the old code never asked.
        a.names.insert(mid.clone(), "Dee".into());
        assert!(!a.avatar_ready(&mid));
        assert!(a.name_misses.borrow().contains(&mid));
        a.request_missing_names();
        assert!(a.name_requests.contains_key(&mid));
        // The lookup answers: the picture is loaded where it is drawn.
        a.on_event(AppEvent::Pictures(HashMap::from([(
            mid.clone(),
            Some("/0hpic".into()),
        )])));
        a.on_event(AppEvent::LookedUp(vec![mid.clone()]));
        assert!(!a.avatar_ready(&mid));
        a.start_media();
        assert!(matches!(a.avatars.get(&mid), Some(Load::Loading)));
        // Not asked again this run, even after the retry window.
        assert!(a.name_misses.borrow().is_empty());
        a.name_requests.clear();
        a.want_names([mid.clone()]);
        assert!(a.name_requests.is_empty());
        // Kept for the next start.
        assert_eq!(
            a.store.user(&mid).unwrap().unwrap().picture.as_deref(),
            Some("/0hpic")
        );
    }

    #[tokio::test]
    async fn stored_picture_is_refreshed_once_per_run() {
        let mut a = app();
        let mid = B64_USER.to_owned();
        a.names.insert(mid.clone(), "Dee".into());
        a.picture_paths.insert(mid.clone(), "/0hold".into());
        // The stored picture is shown at once and checked in the next round.
        assert!(!a.avatar_ready(&mid));
        a.start_media();
        assert!(matches!(a.avatars.get(&mid), Some(Load::Loading)));
        assert!(a.name_misses.borrow().contains(&mid));
        a.request_missing_names();
        assert!(a.name_requests.contains_key(&mid));
    }

    #[test]
    fn removed_picture_drops_the_stale_one_everywhere() {
        let mut a = app();
        let mid = B64_USER.to_owned();
        a.names.insert(mid.clone(), "Dee".into());
        a.picture_paths.insert(mid.clone(), "/0hold".into());
        a.persist_users(std::slice::from_ref(&mid));
        a.avatars.insert(mid.clone(), Load::Failed("no picture".into()));
        a.on_event(AppEvent::Pictures(HashMap::from([(mid.clone(), None)])));
        assert!(!a.picture_paths.contains_key(&mid));
        assert!(!a.avatars.contains_key(&mid));
        assert_eq!(a.store.user(&mid).unwrap().unwrap().picture, None);
        // Nothing to load now, and nothing marked as loading forever.
        assert!(!a.avatar_ready(&mid));
        assert!(!a.avatars.contains_key(&mid));
    }

    #[test]
    fn profile_page_picture_reaches_the_chat_list() {
        let mut a = app();
        let mid = B64_USER.to_owned();
        a.names.insert(mid.clone(), "Dee".into());
        a.avatars.insert(mid.clone(), Load::Failed("no picture".into()));
        a.on_event(AppEvent::ProfilePicture(mid.clone(), Some("/0hnew".into())));
        assert_eq!(a.picture_paths[&mid], "/0hnew");
        // The avatar is fetched again with the new path where it is drawn.
        assert!(!a.avatars.contains_key(&mid));
        assert!(a.looked_up.contains(&mid));
        assert_eq!(
            a.store.user(&mid).unwrap().unwrap().picture.as_deref(),
            Some("/0hnew")
        );
    }

    #[test]
    fn invitation_picture_replaces_the_stale_avatar() {
        let mut a = app();
        a.picture_paths.insert("Cgroup".into(), "/0hold".into());
        a.avatars.insert("Cgroup".into(), Load::Loading);
        let chat: Chat = serde_json::from_value(serde_json::json!({
            "chatMid": "Cgroup", "chatName": "Club", "picturePath": "/0hnew"
        }))
        .unwrap();
        a.on_invitations(vec![chat], true);
        assert_eq!(a.picture_paths["Cgroup"], "/0hnew");
        assert!(!a.avatars.contains_key("Cgroup"));
    }

    #[test]
    fn friend_list_refresh_reaches_every_view() {
        let mut a = app();
        a.picture_paths.insert("Ua".into(), "/old".into());
        a.avatars.insert("Ua".into(), Load::Loading);
        let c: Contact = serde_json::from_value(serde_json::json!({
            "mid": "Ua", "displayName": "Alice", "statusMessage": "", "picturePath": "/new"
        }))
        .unwrap();
        a.on_event(AppEvent::Contacts(vec![c]));
        assert_eq!(a.name("Ua"), "Alice");
        assert_eq!(a.picture_paths["Ua"], "/new");
        // The old picture's avatar is dropped, to be fetched with the new path.
        assert!(!a.avatars.contains_key("Ua"));
        // Kept for the next start.
        assert_eq!(a.store.user("Ua").unwrap().unwrap().name, "Alice");
    }

    fn app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut a = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            Store::open_in_memory().unwrap(),
        );
        a.no_read_receipts_flag = true;
        a.thumb_size = 0;
        for (i, mid) in ["Ua", "Ub"].iter().enumerate() {
            a.chats.push(ChatEntry {
                mid: (*mid).into(),
                ..Default::default()
            });
            let msgs = (0..3)
                .map(|j| UiMsg {
                    id: format!("{i}{j}"),
                    from: "Ume".into(),
                    time: 0,
                    text: format!("m{j}"),
                    raw: None,
                    reply_to: None,
                    forwarded: false,
                    reactions: Vec::new(),
                    unsent: false,
                    delivered: 0,
                    written: true,
                    plain: true,
                })
                .collect();
            a.messages.insert((*mid).into(), msgs);
        }
        a
    }

    fn unread_app(last_seen: Option<&str>, unread: u32) -> App {
        let mut a = app();
        let chat = a.chats.iter_mut().find(|c| c.mid == "Ua").unwrap();
        chat.last_id = Some("3".into());
        chat.last_seen = last_seen.map(str::to_owned);
        chat.unread = unread;
        a.messages.insert(
            "Ua".into(),
            [
                ("1", "last read message"),
                ("2", "first unread message"),
                ("3", "second unread message"),
            ]
            .into_iter()
            .map(|(id, text)| UiMsg {
                id: id.into(),
                from: "Ub".into(),
                time: 0,
                text: text.into(),
                raw: None,
                reply_to: None,
                forwarded: false,
                reactions: Vec::new(),
                unsent: false,
                delivered: 0,
                written: true,
                plain: true,
            })
            .collect(),
        );
        a.open_chat("Ua".into());
        a
    }

    fn screen(a: &mut App) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn key(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn key_with_modifiers(a: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        a.on_key(KeyEvent::new(code, modifiers));
    }

    fn type_text(a: &mut App, text: &str) {
        for c in text.chars() {
            key(a, KeyCode::Char(c));
        }
    }

    #[test]
    fn thai_rows_keep_every_cell_one_column() {
        let mut a = app();
        a.names.insert("Ua".into(), "กำนันทำนาน้ำท่วม".into());
        a.names.insert("Ub".into(), "ครูเปิ้ล".into());
        a.chats[0].preview = "ครูบอกให้ซันทักไปหาครูเปิ้ลครูเคมี เดี๋ยวทำงานส่งนะ น้ำท่วมบ้านกำนันแล้ว".into();
        a.chats[1].preview = "ทำ".into();
        let s = screen(&mut a);
        // SARA AM would join its consonant into one two-column cell, which terminals
        // draw differently (shifted borders, a lone `ำ` left behind on repaint).
        assert!(!s.contains('\u{0E33}'), "SARA AM is decomposed: {s}");
        assert!(s.contains("ก\u{0E4D}านันท\u{0E4D}านาน\u{0E49}\u{0E4D}า"), "{s}");
        assert!(s.contains("ครูบอกให้ซันทักไปหาครูเปิ้ล"), "{s}");
        assert!(s.contains('…'), "the long preview is cut: {s}");
        // No cell is wider than one column, so every row's border lands in the same place.
        assert_eq!(s.width(), 100 * 30, "{s}");
        // Search highlights still find words typed with SARA AM.
        let spans = highlighted(&ellipsize("ทำนา", 10), &["ทำ".into()], Style::new());
        assert_eq!(spans[0].content, "ท\u{0E4D}า");
    }

    /// Every cell of the screen one column wide with no SARA AM left, and no
    /// vowel or tone mark alone at the start of a cell (where a terminal would
    /// put it on whatever came before).
    fn assert_thai_cells(a: &mut App, w: u16, h: u16) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let cells = term.backend().buffer().content();
        for (i, c) in cells.iter().enumerate() {
            let s = c.symbol();
            assert!(!s.contains(['\u{0E33}', '\u{0EB3}']), "cell {i}: {s:?}");
            assert!(
                s.chars().next().is_none_or(|ch| ch.width().unwrap_or(0) > 0),
                "cell {i} starts with a mark: {s:?}"
            );
        }
        let s: String = cells.iter().map(|c| c.symbol()).collect();
        assert_eq!(s.width(), usize::from(w) * usize::from(h), "{s}");
        s
    }

    #[test]
    fn thai_messages_and_drafts_keep_every_cell_one_column() {
        let mut a = app();
        a.open = Some("Ua".into());
        let text = "น้ำท่วมบ้านกำนันแล้ว ทำงานส่งนะ กำลังทำอยู่ ญี่ปุ่น ขอบคุณครับ";
        let thai: Vec<UiMsg> = (0..3)
            .map(|j| UiMsg {
                id: format!("{j}"),
                text: text.repeat(j + 1),
                ..a.messages["Ua"][0].clone()
            })
            .collect();
        a.messages.insert("Ua".into(), thai);
        a.focus = Focus::Input;
        type_text(&mut a, "ทำไมน้ำถึงท่วมบ้านกำนันทุกปี");
        for (w, h) in [(100, 30), (63, 20), (41, 20), (120, 40)] {
            let s = assert_thai_cells(&mut a, w, h);
            assert!(s.contains("น\u{0E49}\u{0E4D}าท\u{0E48}วม"), "{s}");
        }
        // A profile page draws the name as it is.
        a.names.insert("Ub".into(), "คุณน้ำ".into());
        a.profile.push("Ub".into());
        assert_thai_cells(&mut a, 100, 30);
    }

    #[test]
    fn wrapping_keeps_sara_am_with_its_letter() {
        // `ทำ` is two columns: it moves to the next row whole.
        assert_eq!(wrap("กทำ", 2), vec!["ก", "ทำ"]);
        assert_eq!(wrap("กน้ำ", 2), vec!["ก", "น้ำ"]);
        for width in 2..12 {
            for row in wrap("กำนันทำนาน้ำท่วมบ้านทำงาน", width)
            {
                assert!(!row.starts_with('\u{0E33}'), "{width}: {row}");
                assert!(row.width() <= width, "{width}: {row}");
            }
            let input = input::TextInput::from("กำนันทำนาน้ำท่วมบ้านทำงาน");
            for (s, e) in input.rows(width + 1) {
                assert!(!input.as_str()[s..e].starts_with('\u{0E33}'), "{width}");
            }
        }
    }

    #[test]
    fn chat_search_filters_names_and_backspace_clears_the_query() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha Group".into());
        a.names.insert("Ub".into(), "Bravo".into());
        a.list.select(Some(0));

        key(&mut a, KeyCode::Char('/'));
        assert!(a.searching);
        type_text(&mut a, "ALPHA");
        assert_eq!(a.visible_chats(), vec![0]);
        assert_eq!(a.selected_mid().as_deref(), Some("Ua"));

        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.chat_query, "ALPH");
        assert_eq!(a.visible_chats(), vec![0]);
        for _ in 0..3 {
            key(&mut a, KeyCode::Backspace);
        }
        assert_eq!(a.chat_query, "A");
        assert_eq!(a.visible_chats(), vec![0, 1]);
        key(&mut a, KeyCode::Backspace);
        assert!(a.chat_query.is_empty());
        assert_eq!(a.visible_chats(), vec![0, 1]);
    }

    #[test]
    fn thumbnails_are_kept_no_bigger_than_the_largest_size_shows() {
        let mut a = app();
        a.picker = None;
        let cap = a.thumb_cap();
        let (cols, rows) = THUMB_SIZES[THUMB_SIZES.len() - 1];
        let (gw, gh) = a.cell_mode.grid();
        assert_eq!(cap, (u32::from(cols) * gw * 2, u32::from(rows) * gh * 2));
        let big = shrink_now(image::DynamicImage::new_rgba8(4000, 1000), (400, 300));
        assert_eq!((big.width(), big.height()), (400, 100), "aspect ratio is kept");
        let small = shrink_now(image::DynamicImage::new_rgba8(20, 10), (400, 300));
        assert_eq!((small.width(), small.height()), (20, 10), "never scaled up");
        // The weight counts the pixels, so the budget sees the shrink.
        use piccache::Weigh;
        let p = Picture::new(big, None);
        assert!(p.weight() >= 400 * 100 * 4 && p.weight() < 4000 * 1000 * 4);
    }

    #[test]
    fn chat_search_edits_at_the_cursor_and_refilters() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Bravo".into());
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "avo");
        assert_eq!(a.visible_chats(), vec![1]);
        // "avo" → "Bavo" → "Bravo": typing in the middle refilters.
        key(&mut a, KeyCode::Home);
        type_text(&mut a, "B");
        assert_eq!(a.chat_query, "Bavo");
        assert!(a.visible_chats().is_empty());
        type_text(&mut a, "r");
        assert_eq!(a.chat_query, "Bravo");
        assert_eq!(a.visible_chats(), vec![1]);
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));
        // ← → move without leaving the search; Delete removes after the cursor.
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Delete);
        assert_eq!(a.chat_query, "Bavo");
        assert!(a.searching && a.focus == Focus::Chats);
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Right);
        key(&mut a, KeyCode::Delete);
        assert_eq!(a.chat_query, "Bavo");
        // Backspace at the start does nothing.
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.chat_query, "Bavo");
        // A paste lands at the cursor too.
        a.on_paste("Br");
        assert_eq!(a.chat_query, "BrBavo");
        key(&mut a, KeyCode::Esc);
        assert!(a.chat_query.is_empty());
    }

    #[test]
    fn people_picker_filter_edits_at_the_cursor() {
        let mut a = app();
        a.modal = Some(Modal::Pick(modal::Pick {
            title: String::new(),
            kind: modal::PickKind::ShareContact,
            options: vec![
                ("U1".into(), "Alice".into()),
                ("U2".into(), "Bob".into()),
                ("U3".into(), "อลิส".into()),
            ],
            chosen: vec![],
            filter: TextInput::default(),
            sel: 0,
            single: false,
        }));
        let pick = |a: &App| match &a.modal {
            Some(Modal::Pick(p)) => (p.filter.as_str().to_owned(), p.visible(), p.sel),
            _ => panic!("the picker closed"),
        };
        type_text(&mut a, "ace");
        assert_eq!(pick(&a).1, Vec::<usize>::new());
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Left);
        type_text(&mut a, "li");
        assert_eq!(pick(&a), ("alice".into(), vec![0], 0));
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Delete);
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(pick(&a).0, "lic");
        // Thai with combining marks: one step over each cluster.
        for _ in 0..3 {
            key(&mut a, KeyCode::Backspace);
        }
        type_text(&mut a, "ลิส");
        assert_eq!(pick(&a).1, vec![2]);
        // Two steps left is the start; Delete takes ล with its vowel mark.
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Delete);
        assert_eq!(pick(&a).0, "ส");
    }

    #[test]
    fn file_filter_edits_at_the_cursor_and_backspace_leaves_the_folder_when_empty() {
        let dir = std::env::temp_dir().join(format!("rune-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["photo.png", "notes.txt"] {
            std::fs::write(dir.join("sub").join(f), b"1").unwrap();
        }
        let mut a = app();
        a.modal = Some(Modal::Attach(attach::Attach::new(dir.join("sub"))));
        let at = |a: &App| match &a.modal {
            Some(Modal::Attach(at)) => (at.filter.as_str().to_owned(), at.visible().len(), at.dir.clone()),
            _ => panic!("the browser closed"),
        };
        type_text(&mut a, "pg");
        assert_eq!(a.key_context(), keymap::Ctx::AttachFilter);
        assert_eq!(at(&a).1, 0);
        // ← moves the cursor (not to the parent folder) while a filter is typed.
        key(&mut a, KeyCode::Left);
        type_text(&mut a, "n");
        assert_eq!(at(&a), ("png".into(), 1, dir.join("sub")));
        key(&mut a, KeyCode::Home);
        type_text(&mut a, ".");
        assert_eq!(at(&a).0, ".png", "`.` is text inside a filter");
        key(&mut a, KeyCode::Delete);
        key(&mut a, KeyCode::End);
        for _ in 0..3 {
            key(&mut a, KeyCode::Backspace);
        }
        assert_eq!(at(&a), (String::new(), 2, dir.join("sub")));
        assert_eq!(a.key_context(), keymap::Ctx::AttachBrowse);
        // Empty filter: Backspace is the parent folder again.
        key(&mut a, KeyCode::Backspace);
        assert_eq!(at(&a).2, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn album_post(id: &str, t: i64, album: &str, name: &str) -> Message {
        let mut m = Message {
            id: id.into(),
            from: "Ua".into(),
            to: "Ua".into(),
            content_type: rune::album::POST_NOTIFICATION,
            created_time: Some(t),
            ..Default::default()
        };
        for (k, v) in [
            ("serviceType", "AB"),
            ("albumId", album),
            ("albumName", name),
            ("locKey", "BT"),
            ("mediaOid", "o1"),
            ("mediaType", "I"),
        ] {
            m.content_metadata.insert(k.into(), v.into());
        }
        m
    }

    #[test]
    fn albums_come_from_indexed_posts_and_stay_current() {
        let mut a = app();
        let mut msgs: Vec<Message> = (1..=500)
            .map(|i| Message {
                id: (1000 + i).to_string(),
                from: "Ua".into(),
                to: "Ua".into(),
                created_time: Some(i * 10),
                text: Some("hi".into()),
                ..Default::default()
            })
            .collect();
        msgs.push(album_post("2001", 15, "A1", "Old name"));
        msgs.push(album_post("2002", 25, "A2", "Beach"));
        msgs.push(album_post("2003", 35, "A1", "Trip"));
        a.store.put_messages("Ua", &msgs, None).unwrap();
        let albums = a.albums_of("Ua");
        let summary: Vec<(&str, &str, usize)> = albums
            .iter()
            .map(|x| (x.id.as_str(), x.name.as_str(), x.posts.len()))
            .collect();
        assert_eq!(summary, [("A1", "Trip", 2), ("A2", "Beach", 1)]);
        // A post only in memory (not stored yet) counts; an unsent one does not.
        let mut ui = to_ui(&album_post("2004", 45, "A3", "New"), None).unwrap();
        ui.raw = Some(album_post("2004", 45, "A3", "New"));
        a.messages.get_mut("Ua").unwrap().push(ui);
        a.store.mark_unsent("2002").unwrap();
        let ids: Vec<String> = a.albums_of("Ua").into_iter().map(|x| x.id).collect();
        assert_eq!(ids, ["A3", "A1"]);
    }

    #[tokio::test]
    async fn viewing_an_album_photo_loads_only_the_history_up_to_it() {
        let mut a = app();
        let mut msgs: Vec<Message> = (1..=300)
            .map(|i| Message {
                id: (1000 + i).to_string(),
                from: "Ua".into(),
                to: "Ua".into(),
                created_time: Some(i * 10),
                text: Some("hi".into()),
                ..Default::default()
            })
            .collect();
        msgs.insert(100, album_post("1100", 1000, "A1", "Trip"));
        msgs.retain(|m| m.id != "1100" || m.content_type != 0);
        a.store.put_messages("Ua", &msgs, None).unwrap();
        // The open chat shows the newest 50 stored messages.
        let newest: Vec<UiMsg> = a
            .store
            .messages("Ua", 50, None)
            .unwrap()
            .iter()
            .filter_map(stored_to_ui)
            .collect();
        a.messages.insert("Ua".into(), newest);
        a.open = Some("Ua".into());
        a.view_message("Ua", "1100");
        let loaded = a.open_messages();
        assert_eq!(loaded.first().map(|m| m.id.as_str()), Some("1100"));
        // Contiguous and no more than needed: 1100..=1300.
        assert_eq!(loaded.len(), 201);
        assert!(
            loaded
                .windows(2)
                .all(|w| message_id_cmp(&w[0].id, &w[1].id).is_lt())
        );
    }

    #[test]
    fn chat_search_matches_group_member_names() {
        let mut a = app();
        a.names.insert("Ua".into(), "Project".into());
        a.names.insert("Ub".into(), "Other".into());
        a.names.insert("Uc".into(), "Caroline".into());
        a.members.insert("Ua".into(), vec!["Ume".into(), "Uc".into()]);
        a.list.select(Some(0));

        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "carol");
        assert_eq!(a.visible_chats(), vec![0]);
        assert_eq!(a.selected_mid().as_deref(), Some("Ua"));
    }

    #[test]
    fn chat_search_escape_keeps_selection_and_enter_opens_a_filtered_chat() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Beta".into());
        a.list.select(Some(1));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "bet");
        key(&mut a, KeyCode::Esc);
        assert!(!a.searching);
        assert!(a.chat_query.is_empty());
        assert_eq!(a.visible_chats(), vec![0, 1]);
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));

        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "beta");
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));
        key(&mut a, KeyCode::Enter);
        assert!(!a.searching);
        assert_eq!(a.open.as_deref(), Some("Ub"));
    }

    #[tokio::test]
    async fn chat_search_without_matches_has_no_selection_and_shows_empty_state() {
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "missing");
        assert!(a.list_rows().is_empty());
        assert_eq!(a.list.selected(), None);
        key(&mut a, KeyCode::Enter);
        assert!(a.searching);
        assert_eq!(a.open, None);
        let s = screen(&mut a);
        assert!(s.contains("no chat or friend matches “missing”"), "{s}");
        assert!(s.contains("↓ to search older chats"), "{s}");
        // Typing never pages through older chats; ↓ asks for it.
        a.maybe_more_chats();
        assert_eq!(a.more_chats, MoreChats::Ready(None));
        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.list.selected(), None);
        assert_eq!(a.more_chats, MoreChats::Loading(None));
        assert!(screen(&mut a).contains("searching older chats…"));
    }

    #[tokio::test]
    async fn searching_older_chats_goes_on_until_one_matches() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Bravo".into());
        let entry = |mid: &str| ChatEntry {
            mid: mid.into(),
            ..Default::default()
        };
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "carl");
        key(&mut a, KeyCode::Down);
        assert_eq!(a.more_chats, MoreChats::Loading(None));
        // A page without a match: the next one is asked for at once.
        a.on_event(AppEvent::MoreChats(Ok((
            vec![entry("Cold")],
            Some("Cold".into()),
        ))));
        assert_eq!(a.more_chats, MoreChats::Loading(Some("Cold".into())));
        a.names.insert("Ucarl".into(), "Carla".into());
        a.on_event(AppEvent::MoreChats(Ok((
            vec![entry("Ucarl")],
            Some("Ucarl".into()),
        ))));
        assert_eq!(
            a.more_chats,
            MoreChats::Ready(Some("Ucarl".into())),
            "found: it stops"
        );
        assert_eq!(a.selected_mid().as_deref(), Some("Ucarl"));
        assert_eq!(a.status, "1 more chat matches");
        // A failure leaves ↓ to try again.
        key(&mut a, KeyCode::Down);
        a.on_event(AppEvent::MoreChats(Err("boom".into())));
        assert_eq!(a.more_chats, MoreChats::Ready(Some("Ucarl".into())));
        assert!(a.status.contains("could not load more chats"), "{}", a.status);
        // The last page holds none: everything was searched.
        key(&mut a, KeyCode::Down);
        a.on_event(AppEvent::MoreChats(Ok((vec![entry("Colder")], None))));
        assert_eq!(a.more_chats, MoreChats::Done);
        assert!(a.status.contains("all 5 chats searched"), "{}", a.status);
        type_text(&mut a, "zzz");
        assert!(screen(&mut a).contains("all chats searched"));
    }

    #[tokio::test]
    async fn a_query_with_one_match_does_not_page_through_older_chats() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Bravo".into());
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "bravo");
        // The only match is selected, so it is the last row too.
        assert_eq!(a.list.selected(), Some(0));
        a.maybe_more_chats();
        assert_eq!(a.more_chats, MoreChats::Ready(None));
        key(&mut a, KeyCode::Esc);
        a.list.select(Some(1));
        a.maybe_more_chats();
        assert_eq!(
            a.more_chats,
            MoreChats::Loading(None),
            "the plain list still pages"
        );
    }

    #[test]
    fn chat_search_ranks_name_matches_before_member_matches() {
        let mut a = app();
        for (mid, name) in [
            ("Ua", "Project"),
            ("Ub", "Kabob"),
            ("Uc", "Bob"),
            ("Ud", "Bobby Lee"),
        ] {
            a.names.insert(mid.into(), name.into());
        }
        for mid in ["Uc", "Ud"] {
            a.chats.push(ChatEntry {
                mid: mid.into(),
                ..Default::default()
            });
        }
        a.members.insert("Ua".into(), vec!["Ume".into(), "Uc".into()]);
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "bob");
        let order: Vec<String> = a.list_rows().iter().map(|r| a.row_mid(r).to_owned()).collect();
        // Word starts (list order among them), then inside a word, then members.
        assert_eq!(order, ["Uc", "Ud", "Ub", "Ua"]);
        assert_eq!(
            a.selected_mid().as_deref(),
            Some("Uc"),
            "the best match is selected"
        );
        let s = screen(&mut a);
        // (A wide emoji takes two cells in the test buffer.)
        assert!(
            s.contains("👥") && s.contains(" Bob "),
            "a member match says whose: {s}"
        );
        // Every word has to match, in any order.
        a.set_chat_query("lee bob".into());
        assert_eq!(a.list_rows(), vec![ListRow::Chat(3)]);
        // Our own name never makes every group match.
        a.names.insert("Ume".into(), "Me Myself".into());
        a.set_chat_query("myself".into());
        assert!(a.list_rows().is_empty());
    }

    #[tokio::test]
    async fn chat_search_finds_friends_without_a_chat() {
        let mut a = app();
        a.names.insert("Ua".into(), "Dan's group".into());
        a.names.insert("Ub".into(), "Other".into());
        for (mid, name) in [("Udana", "Dana"), ("Ub", "Other"), ("Ume", "Me")] {
            a.contacts.insert(mid.into(), name.into());
            a.names.insert(mid.into(), name.into());
        }
        a.open = None;
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "dan");
        assert_eq!(
            a.list_rows(),
            vec![ListRow::Chat(0), ListRow::Friend("Udana".into())],
            "friends with a chat show as the chat"
        );
        let s = screen(&mut a);
        assert!(
            s.contains("── friends ──") && s.contains("friend · no messages yet"),
            "{s}"
        );
        assert!(s.contains("Chats · 2 matches"), "{s}");
        key(&mut a, KeyCode::Down);
        assert_eq!(a.selected_mid().as_deref(), Some("Udana"));
        assert!(a.hint().contains("Enter open"), "{}", a.hint());
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.open.as_deref(), Some("Udana"));
        assert!(!a.searching);
        assert_eq!(a.focus, Focus::Input);
    }

    #[test]
    fn chat_search_query_stays_when_the_focus_leaves_without_a_choice() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Bravo".into());
        a.open = Some("Ua".into());
        a.focus = Focus::Chats;
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "bra");
        key(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Focus::Input);
        assert!(a.searching && a.chat_query == "bra");
        assert_eq!(a.list_rows(), vec![ListRow::Chat(1)], "the list stays filtered");
        assert!(screen(&mut a).contains("bra"));
        // Back in the list: in the search field again, the query as it was.
        key(&mut a, KeyCode::Tab);
        assert_eq!(a.key_context(), keymap::Ctx::ChatSearch);
        type_text(&mut a, "v");
        assert_eq!(a.chat_query, "brav");
        // Ctrl-f from the message line comes back to it too.
        a.focus = Focus::Input;
        key_with_modifiers(&mut a, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!((a.focus, a.chat_query.as_str()), (Focus::Chats, "brav"));
        // Esc ends it; an empty search does not outlive the focus.
        key(&mut a, KeyCode::Esc);
        assert!(!a.searching);
        key(&mut a, KeyCode::Char('/'));
        key(&mut a, KeyCode::Tab);
        assert!(!a.searching);
    }

    #[test]
    fn ctrl_f_searches_from_the_messages_but_not_the_viewer() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        key_with_modifiers(&mut a, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(a.searching);
        assert_eq!(a.key_context(), keymap::Ctx::ChatSearch);
        type_text(&mut a, "x");
        assert_eq!(a.chat_query, "x", "letters go into the field");
        assert_eq!(
            keymap::resolve(
                keymap::Ctx::Viewer,
                &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)
            ),
            None
        );
    }

    #[test]
    fn chat_search_pages_through_results_and_hides_the_pinned_divider() {
        let mut a = app();
        for i in 0..30 {
            let mid = format!("Ux{i:02}");
            a.names.insert(mid.clone(), format!("Person {i}"));
            a.chats.push(ChatEntry {
                mid,
                ..Default::default()
            });
        }
        a.pinned = vec!["Ux05".into()];
        a.sort_chats(None);
        assert!(screen(&mut a).contains("── pinned ──"));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "person");
        let s = screen(&mut a);
        assert!(!s.contains("── pinned ──") && s.contains("📌"), "{s}");
        assert_eq!(a.list.selected(), Some(0));
        key(&mut a, KeyCode::PageDown);
        assert_eq!(a.list.selected(), Some(a.chat_page() as usize));
        key(&mut a, KeyCode::PageUp);
        assert_eq!(a.list.selected(), Some(0));
    }

    #[test]
    fn search_matching_ignores_case_and_marks_word_starts() {
        assert_eq!(find_word("Alpha Group", "gro"), vec![(6..9, true)]);
        assert_eq!(find_word("ALPHA", "lph"), vec![(1..4, false)]);
        assert_eq!(find_word("Ünal", "ün"), vec![(0..3, true)]);
        assert!(find_word("Alpha", "").is_empty());
        assert_eq!(name_rank("Alpha Group", &["gr".into(), "al".into()]), Some(0));
        assert_eq!(name_rank("Alpha Group", &["roup".into()]), Some(1));
        assert_eq!(name_rank("Alpha Group", &["x".into()]), None);
        let spans = highlighted("Alpha Group", &["gro".into()], Style::new());
        let text: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, ["Alpha ", "Gro", "up"]);
    }

    #[test]
    fn chat_search_right_at_the_end_moves_to_the_message_line() {
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "ab");
        // No chat open: there is no message line to move to.
        key(&mut a, KeyCode::Right);
        assert!(a.searching && a.focus == Focus::Chats);

        a.open = Some("Ua".into());
        key(&mut a, KeyCode::Left);
        assert!(
            a.searching && a.focus == Focus::Chats,
            "→ inside the query only moves the cursor"
        );
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Delete);
        assert!(
            a.searching && a.focus == Focus::Chats,
            "End and Delete at the end stay in the field"
        );
        key(&mut a, KeyCode::Right);
        assert_eq!(a.focus, Focus::Input);
        assert!(
            a.searching && a.chat_query == "ab",
            "the query stays for coming back"
        );
    }

    #[test]
    fn chat_search_letters_do_not_trigger_chat_shortcuts() {
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "nm");
        assert_eq!(a.chat_query, "nm");
        assert!(a.modal.is_none());
        assert!(a.searching);
        key(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Focus::Chats);
        assert!(a.searching);
    }

    #[test]
    fn mouse_click_opens_the_filtered_chat_at_its_visible_position() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Beta".into());
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "beta");
        screen(&mut a);

        let chats = a.hit.chats;
        click(&mut a, chats.x + 1, chats.y + 1);
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));
        assert_eq!(a.open.as_deref(), Some("Ub"));
    }

    #[test]
    fn unread_divider_is_rendered_once_at_the_read_boundary() {
        let mut a = unread_app(Some("1"), 2);
        let screen = screen(&mut a);
        assert_eq!(screen.matches("Unread Messages").count(), 1, "{screen}");
        let last_read = screen.find("last read message").unwrap();
        let divider = screen.find("Unread Messages").unwrap();
        let first_unread = screen.find("first unread message").unwrap();
        assert!(last_read < divider && divider < first_unread, "{screen}");
    }

    #[test]
    fn unread_divider_stays_until_another_chat_is_opened() {
        let mut a = unread_app(Some("1"), 2);
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 40, "param1": "Ua", "param2": "3", "param3": "0"
        })));
        a.on_operation(op(serde_json::json!({
            "revision": 2, "type": 25, "message": sent("4", "Ume", "Ua", "my reply")
        })));
        assert_eq!(a.unread_index("Ua", a.open_messages()), Some(1));
        assert!(screen(&mut a).contains("Unread Messages"));

        a.open_chat("Ub".into());
        a.open_chat("Ua".into());
        assert!(a.unread_divider.is_none());
    }

    #[test]
    fn ctrl_r_toggles_read_receipts_unless_the_flag_forbids_them() {
        let mut a = unread_app(Some("1"), 2);
        a.no_read_receipts_flag = false;
        a.prefs.read_receipts = true;
        key_with_modifiers(&mut a, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(!a.read_receipts());
        let shown = screen(&mut a);
        assert_eq!(
            shown.matches("◌ receipts off").count(),
            2,
            "chat list and message line: {shown}"
        );
        a.focus = Focus::Input;
        key_with_modifiers(&mut a, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(a.read_receipts());
        assert_eq!(a.input.as_str(), "", "the chord is not typed");
        assert!(!screen(&mut a).contains("receipts off"));

        a.no_read_receipts_flag = true;
        key_with_modifiers(&mut a, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(a.prefs.read_receipts && !a.read_receipts());
    }

    #[tokio::test]
    async fn with_receipts_off_nothing_clears_unread() {
        let mut a = receipts_app(false);
        let chat = |a: &App| {
            let c = a.chats.iter().find(|c| c.mid == "Ua").unwrap();
            (c.unread, c.last_seen.clone())
        };
        let still = (2, Some("1".to_owned()));
        a.open_chat("Ua".into());
        assert_eq!(chat(&a), still, "opening does not read it");
        assert!(
            a.unread_divider.is_some(),
            "the divider still shows where unread starts"
        );
        a.on_focus(false);
        a.on_focus(true);
        assert_eq!(chat(&a), still, "nor does coming back to it");
        // A message arriving in the open chat counts as unread too.
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 26, "message": sent("4", "Ua", "Ume", "more")
        })));
        assert_eq!(chat(&a).0, 3);
        // Neither our own reply nor the server's chat list clears it.
        a.on_operation(op(serde_json::json!({
            "revision": 2, "type": 25, "message": sent("5", "Ume", "Ua", "reply")
        })));
        a.on_event(AppEvent::Chats(vec![ChatEntry {
            mid: "Ua".into(),
            last_id: Some("5".into()),
            unread: 3,
            last_seen: Some("1".into()),
            ..Default::default()
        }]));
        assert_eq!(chat(&a), (3, Some("1".to_owned())));
        assert_eq!(a.store.chat("Ua").unwrap().unwrap().unread, 3, "nor in the store");
        assert!(take_sent(&mut a).is_empty(), "nothing marked read");
        // Turned on while in the chat: read now.
        toggle_receipts(&mut a);
        assert_eq!(chat(&a), (0, Some("5".to_owned())));
    }

    /// `unread_app` with read receipts allowed for this run and set to `on`;
    /// `Ua` (messages 1–3, read up to 1) is not open yet.
    fn receipts_app(on: bool) -> App {
        let mut a = unread_app(Some("1"), 2);
        a.open_chat("Ub".into());
        let c = a.chats.iter_mut().find(|c| c.mid == "Ua").unwrap();
        c.last_seen = Some("1".into());
        c.unread = 2;
        a.no_read_receipts_flag = false;
        a.prefs.read_receipts = on;
        a
    }

    fn stored_receipts(a: &App) -> Vec<(String, Option<String>, Option<String>)> {
        a.store.pending_receipts().unwrap()
    }

    fn row(
        chat: &str,
        approved: Option<&str>,
        held: Option<&str>,
    ) -> (String, Option<String>, Option<String>) {
        (chat.into(), approved.map(Into::into), held.map(Into::into))
    }

    fn take_sent(a: &mut App) -> Vec<(String, String)> {
        a.sent_receipts.drain(..).map(|s| (s.chat, s.id)).collect()
    }

    fn toggle_receipts(a: &mut App) {
        key_with_modifiers(a, KeyCode::Char('r'), KeyModifiers::CONTROL);
    }

    #[test]
    fn reading_with_receipts_on_sends_and_clears_on_acknowledgement() {
        let mut a = receipts_app(true);
        a.open_chat("Ua".into());
        let s = a.sent_receipts.clone();
        assert_eq!(take_sent(&mut a), vec![("Ua".into(), "3".into())]);
        assert_eq!(
            stored_receipts(&a),
            vec![row("Ua", Some("3"), None)],
            "kept until acknowledged"
        );
        a.on_event(AppEvent::Receipt(s[0].clone(), receipts::Outcome::Acked));
        assert!(stored_receipts(&a).is_empty());
        // Opening it again sends nothing: no duplicate.
        a.open_chat("Ub".into());
        a.open_chat("Ua".into());
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty());
    }

    #[test]
    fn read_while_off_is_sent_when_turned_back_on_in_that_chat() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty(), "never sent while off");
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("3"))]);
        toggle_receipts(&mut a);
        assert_eq!(take_sent(&mut a), vec![("Ua".into(), "3".into())]);
        assert_eq!(stored_receipts(&a), vec![row("Ua", Some("3"), None)]);
    }

    #[test]
    fn turned_on_in_another_chat_it_waits_until_the_chat_is_opened_again() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.open_chat("Ub".into());
        toggle_receipts(&mut a);
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty(), "not in Ua any more");
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("3"))]);
        // Nothing new in Ua, but being there approves what was held.
        a.open_chat("Ua".into());
        assert_eq!(take_sent(&mut a), vec![("Ua".into(), "3".into())]);
    }

    #[test]
    fn turned_on_while_the_terminal_is_unfocused_it_waits_for_focus() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.on_focus(false);
        toggle_receipts(&mut a);
        assert!(take_sent(&mut a).is_empty());
        a.on_focus(true);
        assert_eq!(take_sent(&mut a), vec![("Ua".into(), "3".into())]);
    }

    #[test]
    fn only_messages_actually_seen_are_held() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        // Arrives while the terminal is in the background: not read.
        a.on_focus(false);
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 26, "message": sent("4", "Ua", "Ume", "away")
        })));
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("3"))]);
        // Coming back shows it.
        a.on_focus(true);
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("4"))]);
        // Arrives while looking at the chat: read.
        a.on_operation(op(serde_json::json!({
            "revision": 2, "type": 26, "message": sent("5", "Ua", "Ume", "here")
        })));
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("5"))]);
        // Arrives in another chat: not read.
        a.on_operation(op(serde_json::json!({
            "revision": 3, "type": 26, "message": sent("6", "Ub", "Ume", "elsewhere")
        })));
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("5"))]);
        assert!(take_sent(&mut a).is_empty());
    }

    #[test]
    fn pending_receipts_survive_a_restart() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        let mut b = receipts_app(true);
        b.open_chat("Ua".into());
        let msgs = a.messages["Ua"].clone();
        // Held in one store, approved but never acknowledged in another.
        for (store, sent) in [(a.store.clone(), false), (b.store.clone(), true)] {
            let (tx, _rx) = mpsc::unbounded_channel();
            let mut c = App::new(Client::new(Config::default()).unwrap(), "Ume".into(), tx, store);
            c.prefs.read_receipts = true;
            c.chats.push(ChatEntry {
                mid: "Ua".into(),
                last_id: Some("3".into()),
                last_seen: Some("3".into()),
                ..Default::default()
            });
            c.messages.insert("Ua".into(), msgs.clone());
            c.flush_receipts();
            if sent {
                assert_eq!(
                    take_sent(&mut c),
                    vec![("Ua".into(), "3".into())],
                    "retried at start"
                );
            } else {
                assert!(take_sent(&mut c).is_empty(), "held until the chat is opened");
                c.open_chat("Ua".into());
                assert_eq!(take_sent(&mut c), vec![("Ua".into(), "3".into())]);
            }
        }
    }

    #[test]
    fn failed_receipts_retry_with_backoff_and_only_while_on() {
        let mut a = receipts_app(true);
        a.open_chat("Ua".into());
        let s = a.sent_receipts.drain(..).next().unwrap();
        a.on_event(AppEvent::Receipt(s, receipts::Outcome::Retry));
        let job = Job::Receipt("Ua".into());
        assert!(a.retry.contains(&job));
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty(), "waits out the backoff");
        assert_eq!(stored_receipts(&a), vec![row("Ua", Some("3"), None)]);
        let now = Instant::now();
        a.run_retries_at(now + Duration::from_secs(3));
        let s = a.sent_receipts.drain(..).next().unwrap();
        assert_eq!((s.chat.as_str(), s.id.as_str()), ("Ua", "3"));
        a.on_event(AppEvent::Receipt(s, receipts::Outcome::Retry));
        // Turned off before the next attempt: the retry sends nothing.
        toggle_receipts(&mut a);
        a.run_retries_at(now + Duration::from_secs(600));
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty());
        // Back on (still in the chat): sent, and acknowledged at last.
        toggle_receipts(&mut a);
        let s = a.sent_receipts.drain(..).next().unwrap();
        a.on_event(AppEvent::Receipt(s, receipts::Outcome::Acked));
        assert!(stored_receipts(&a).is_empty());
        assert!(!a.retry.contains(&job));
    }

    #[test]
    fn a_request_cancelled_by_turning_off_is_not_retried() {
        let mut a = receipts_app(true);
        a.open_chat("Ua".into());
        let s = a.sent_receipts.drain(..).next().unwrap();
        toggle_receipts(&mut a);
        // Its failure arrives after all.
        a.on_event(AppEvent::Receipt(s.clone(), receipts::Outcome::Retry));
        assert!(!a.retry.contains(&Job::Receipt("Ua".into())));
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty());
        assert_eq!(stored_receipts(&a), vec![row("Ua", Some("3"), None)]);
        // Or its success: the server has it, so it is done.
        a.on_event(AppEvent::Receipt(s, receipts::Outcome::Acked));
        assert!(stored_receipts(&a).is_empty());
    }

    #[test]
    fn a_read_on_another_device_settles_pending_receipts() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 40, "param1": "Ua", "param2": "3"
        })));
        assert!(stored_receipts(&a).is_empty());
        toggle_receipts(&mut a);
        assert!(take_sent(&mut a).is_empty(), "nothing left to send");
    }

    #[test]
    fn an_unsent_pending_message_moves_the_receipt_back() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 65, "param1": "Ua", "param2": "3"
        })));
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("2"))]);
        toggle_receipts(&mut a);
        assert_eq!(take_sent(&mut a), vec![("Ua".into(), "2".into())]);
    }

    #[test]
    fn a_refused_receipt_is_dropped() {
        let mut a = receipts_app(true);
        a.open_chat("Ua".into());
        let s = a.sent_receipts.drain(..).next().unwrap();
        a.on_event(AppEvent::Receipt(s, receipts::Outcome::Refused));
        assert!(stored_receipts(&a).is_empty());
        assert!(!a.retry.contains(&Job::Receipt("Ua".into())));
    }

    #[test]
    fn leaving_a_chat_forgets_its_receipt() {
        let mut a = receipts_app(false);
        a.open_chat("Ua".into());
        a.remove_chat("Ua");
        assert!(stored_receipts(&a).is_empty());
        toggle_receipts(&mut a);
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty());
    }

    #[test]
    fn the_no_read_receipts_flag_holds_reads_for_a_later_run() {
        let mut a = receipts_app(true);
        a.no_read_receipts_flag = true;
        a.open_chat("Ua".into());
        a.on_focus(false);
        a.on_focus(true);
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty(), "never sent in this run");
        assert_eq!(stored_receipts(&a), vec![row("Ua", None, Some("3"))]);
        // Ctrl-r cannot turn them on this run either.
        toggle_receipts(&mut a);
        a.flush_receipts();
        assert!(take_sent(&mut a).is_empty());
        // A later run without the flag sends it once the chat is open there.
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            a.store.clone(),
        );
        b.prefs.read_receipts = true;
        b.chats.push(ChatEntry {
            mid: "Ua".into(),
            last_id: Some("3".into()),
            last_seen: Some("3".into()),
            ..Default::default()
        });
        b.messages.insert("Ua".into(), a.messages["Ua"].clone());
        b.flush_receipts();
        assert!(take_sent(&mut b).is_empty(), "not before the chat is open");
        b.open_chat("Ua".into());
        assert_eq!(take_sent(&mut b), vec![("Ua".into(), "3".into())]);
    }

    #[test]
    fn receipt_failures_are_retried_unless_the_server_refuses_the_position() {
        let api = |status: u16, gateway: Option<i64>, talk: Option<i64>| {
            rune::Error::Api(Box::new(rune::ApiError {
                path: "/api/talk/thrift/Talk/TalkService/sendChatChecked".into(),
                status,
                gateway_code: gateway,
                talk_code: talk,
                reason: None,
                raw: serde_json::Value::Null,
            }))
        };
        use receipts::Outcome::{Refused, Retry};
        assert_eq!(receipt_outcome(&rune::Error::Timeout("x".into())), Retry);
        assert_eq!(receipt_outcome(&rune::Error::LoginRequired), Retry);
        assert_eq!(receipt_outcome(&api(500, Some(10052), None)), Retry);
        assert_eq!(
            receipt_outcome(&api(200, Some(10051), Some(115))),
            Retry,
            "SHOULD_RETRY"
        );
        assert_eq!(
            receipt_outcome(&api(200, Some(10051), Some(119))),
            Retry,
            "token refresh"
        );
        assert_eq!(receipt_outcome(&api(200, Some(10051), Some(1))), Retry, "auth");
        assert_eq!(receipt_outcome(&api(200, Some(10051), Some(5))), Refused);
    }

    #[test]
    fn count_placed_unread_divider_keeps_its_message_as_others_arrive() {
        let mut a = unread_app(None, 2);
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 26, "message": sent("4", "Ua", "Ume", "another")
        })));
        assert_eq!(a.unread_index("Ua", a.open_messages()), Some(1));
    }

    #[test]
    fn unread_divider_is_omitted_when_read_and_uses_count_without_an_anchor() {
        let mut a = unread_app(Some("3"), 0);
        assert!(a.unread_divider.is_none());
        assert!(!screen(&mut a).contains("Unread Messages"));

        let a = unread_app(None, 2);
        assert_eq!(a.unread_divider, Some(("Ua".into(), Unread::Last(2))));
        assert_eq!(a.unread_index("Ua", a.open_messages()), Some(1));
    }

    #[test]
    fn compose_types_shortcut_characters_verbatim() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Input;
        for c in "?mjkq".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        assert_eq!(a.input, "?mjkq");
    }

    #[test]
    fn help_opens_from_chat_list_and_f1_works_while_typing() {
        let mut a = app();
        key(&mut a, KeyCode::Char('?'));
        assert!(matches!(
            a.modal,
            Some(Modal::Help {
                ctx: keymap::Ctx::ChatList,
                under: None,
                ..
            })
        ));
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        a.open = Some("Ua".into());
        a.focus = Focus::Input;
        a.input = "draft ?".into();
        key(&mut a, KeyCode::F(1));
        assert!(matches!(
            a.modal,
            Some(Modal::Help {
                ctx: keymap::Ctx::Compose,
                under: None,
                ..
            })
        ));
        assert_eq!(a.input, "draft ?");
        key(&mut a, KeyCode::F(1));
        assert!(a.modal.is_none());
    }

    #[test]
    fn help_restores_the_menu_and_its_selection() {
        let mut a = app();
        a.chat_menu("Ua");
        let Some(Modal::Menu(menu)) = a.modal.as_mut() else {
            panic!("chat menu")
        };
        menu.sel = 1;

        key(&mut a, KeyCode::Char('?'));
        let Some(Modal::Help { ctx, under, .. }) = &a.modal else {
            panic!("contextual help")
        };
        assert_eq!(*ctx, keymap::Ctx::Menu);
        let Some(Modal::Menu(menu)) = under.as_deref() else {
            panic!("menu retained below help")
        };
        assert_eq!(menu.sel, 1);

        key(&mut a, KeyCode::Esc);
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("menu restored")
        };
        assert_eq!(menu.sel, 1);
    }

    #[test]
    fn help_closes_with_enter_question_mark_and_f1() {
        for close in [KeyCode::Enter, KeyCode::Char('?'), KeyCode::F(1)] {
            let mut a = app();
            key(&mut a, KeyCode::Char('?'));
            key(&mut a, close);
            assert!(a.modal.is_none(), "help did not close with {close:?}");
        }
    }

    #[test]
    fn help_scrolls_with_arrows_pages_and_home_end() {
        let mut a = app();
        a.open_help(keymap::Ctx::Messages);
        key(&mut a, KeyCode::Down);
        assert!(matches!(a.modal, Some(Modal::Help { scroll: 1, .. })));
        key(&mut a, KeyCode::PageDown);
        assert!(matches!(a.modal, Some(Modal::Help { scroll: 11, .. })));
        key(&mut a, KeyCode::Home);
        assert!(matches!(a.modal, Some(Modal::Help { scroll: 0, .. })));
        key(&mut a, KeyCode::End);
        assert!(matches!(a.modal, Some(Modal::Help { scroll, .. }) if scroll > 0));
    }

    #[test]
    fn ctrl_c_quits_even_with_an_overlay_open() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.modal = Some(Modal::Form(modal::Form {
            title: " Test ".into(),
            kind: modal::FormKind::DisplayName,
            fields: vec![modal::Field {
                label: "Name".into(),
                value: TextInput::default(),
                hint: "".into(),
            }],
            sel: 0,
            error: None,
        }));
        key_with_modifiers(&mut a, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(a.quit);
    }

    #[test]
    fn m_opens_the_menu_for_the_selected_chat_message_and_viewer_message() {
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('m'));
        assert!(matches!(a.modal, Some(Modal::Menu(_))));

        a.modal = None;
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.sel = Some(0);
        key(&mut a, KeyCode::Char('m'));
        assert!(matches!(a.modal, Some(Modal::Menu(_))));

        a.modal = None;
        a.viewer = Some("00".into());
        key(&mut a, KeyCode::Char('m'));
        assert!(matches!(a.modal, Some(Modal::Menu(_))));
    }

    #[tokio::test]
    async fn ctrl_s_opens_settings_from_every_main_view() {
        for focus in [Focus::Chats, Focus::Input, Focus::Messages] {
            let mut a = app();
            a.open = Some("Ua".into());
            a.focus = focus;
            a.sel = Some(0);
            key_with_modifiers(&mut a, KeyCode::Char('s'), KeyModifiers::CONTROL);
            assert!(matches!(a.modal, Some(Modal::Settings(_))), "{focus:?}");
        }
        // One key for it: `S` is just a letter.
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('S'));
        assert!(a.modal.is_none());
    }

    #[tokio::test]
    async fn settings_j_and_k_move_between_rows() {
        let mut a = app();
        a.open_settings_at(settings::Tab::Notifications);
        let Some(Modal::Settings(page)) = &a.modal else {
            panic!("settings dialog")
        };
        let first = page.sel;
        key(&mut a, KeyCode::Char('j'));
        let Some(Modal::Settings(page)) = &a.modal else {
            panic!("settings dialog remains open")
        };
        assert_ne!(page.sel, first);
        key(&mut a, KeyCode::Char('k'));
        let Some(Modal::Settings(page)) = &a.modal else {
            panic!("settings dialog remains open")
        };
        assert_eq!(page.sel, first);
    }

    /// The main menu's item whose label contains `what`, chosen.
    fn choose(a: &mut App, what: &str) {
        let Some(Modal::Menu(m)) = &mut a.modal else {
            panic!("a menu, not {:?}", a.key_context())
        };
        m.sel = m
            .items
            .iter()
            .position(|(l, _)| l.contains(what))
            .unwrap_or_else(|| {
                panic!(
                    "no {what:?} in {:?}",
                    m.items.iter().map(|i| &i.0).collect::<Vec<_>>()
                )
            });
        key(a, KeyCode::Enter);
    }

    fn ctrl_x(a: &mut App) {
        key_with_modifiers(a, KeyCode::Char('x'), KeyModifiers::CONTROL);
    }

    #[test]
    fn ctrl_x_opens_the_main_menu_from_every_main_view_and_while_typing() {
        for focus in [Focus::Chats, Focus::Input, Focus::Messages] {
            let mut a = app();
            a.open_chat("Ua".into());
            a.focus = focus;
            ctrl_x(&mut a);
            let Some(Modal::Menu(m)) = &a.modal else {
                panic!("main menu from {focus:?}")
            };
            let labels: Vec<&str> = m.items.iter().map(|i| i.0.as_str()).collect();
            // The open chat comes first; every screen is reachable, with its key.
            assert!(labels[0].contains("Ctrl-g"), "{labels:?}");
            for want in [
                "Settings\tCtrl-s",
                "Plugins",
                "Pin lists",
                "My profile",
                "New group",
                "Quit\tCtrl-c",
            ] {
                assert!(labels.iter().any(|l| l.contains(want)), "{want} in {labels:?}");
            }
            assert_eq!(a.input.as_str(), "", "Ctrl-x types nothing");
        }
        // Without an open chat, no chat entries.
        let mut a = app();
        ctrl_x(&mut a);
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("main menu")
        };
        assert!(
            m.items
                .iter()
                .all(|(l, _)| !l.contains("Ctrl-g") && !l.contains("Ctrl-o"))
        );
        // It draws, shortcuts included, wide and narrow.
        for (w, h) in [(100, 30), (30, 10)] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            term.draw(|f| a.draw(f)).unwrap();
        }
        assert!(screen(&mut a).contains("Ctrl-s"), "{}", screen(&mut a));
        choose(&mut a, "Quit");
        assert!(a.quit);
    }

    #[tokio::test]
    async fn main_menu_items_go_where_they_say() {
        let mut a = app();
        ctrl_x(&mut a);
        choose(&mut a, "Plugins");
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Plugins));
        a.modal = None;
        ctrl_x(&mut a);
        choose(&mut a, "Settings");
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Notifications));
        a.modal = None;
        ctrl_x(&mut a);
        choose(&mut a, "Pin lists");
        assert!(matches!(&a.modal, Some(Modal::Menu(m)) if m.title.contains("Pin lists")));
        a.modal = None;
        ctrl_x(&mut a);
        choose(&mut a, "Find a chat");
        assert!(a.searching && a.modal.is_none());
        a.end_search();
        ctrl_x(&mut a);
        choose(&mut a, "Keyboard shortcuts");
        assert!(matches!(
            &a.modal,
            Some(Modal::Help {
                ctx: keymap::Ctx::ChatList,
                ..
            })
        ));
        a.modal = None;
        a.no_read_receipts_flag = false;
        let before = a.read_receipts();
        ctrl_x(&mut a);
        choose(&mut a, "Read receipts");
        assert_ne!(a.read_receipts(), before);
    }

    #[tokio::test]
    async fn screens_opened_from_settings_come_back_to_it() {
        let mut a = app();
        // Settings › Chats › Pin lists: Esc, and choosing a list, return to Settings.
        a.open_settings_at(settings::Tab::Chats);
        let rows = a.settings_rows_for_test(settings::Tab::Chats);
        let Some(Modal::Settings(s)) = &mut a.modal else {
            panic!("settings")
        };
        s.sel = rows.iter().position(|r| r.starts_with("Pin lists")).unwrap();
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Menu(_))));
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Chats));
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Enter); // the active list again
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Chats));

        // Settings › Account › Display name: Esc returns to the Account tab.
        a.open_settings_at(settings::Tab::Account);
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Form(_))));
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Account));

        // Settings › Storage › Clear local data…: cancelling returns too.
        a.open_settings_at(settings::Tab::Storage);
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Confirm(_))));
        key(&mut a, KeyCode::Char('n'));
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Storage));

        // Closing Settings itself closes it, and nothing comes back later.
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('m'));
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        // The profile page is not an overlay: Settings goes away for it.
        a.open_settings_at(settings::Tab::Account);
        let rows = a.settings_rows_for_test(settings::Tab::Account);
        let Some(Modal::Settings(s)) = &mut a.modal else {
            panic!("settings")
        };
        s.sel = rows.iter().position(|r| r.starts_with("Profile page")).unwrap();
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none() && a.profile == ["Ume"]);
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none() && a.profile.is_empty());
    }

    #[tokio::test]
    async fn screens_are_not_commands_and_unknown_commands_are_never_sent() {
        let mut a = app();
        a.open_chat("Ua".into());
        a.focus = Focus::Input;
        // The screens are in the menus; their old names are unknown commands.
        for typed in [
            "/settings",
            "/plugins",
            "/help",
            "/quit",
            "/group",
            "/newgroup",
            "/menu",
            "/join",
            "/flex",
        ] {
            submit(&mut a, typed);
            assert!(
                a.status.contains(&format!("unknown command {typed}")),
                "{typed}: {}",
                a.status
            );
            assert_eq!(a.input.as_str(), typed, "never sent");
            assert!(a.modal.is_none() && !a.quit, "{typed}");
            a.input.take();
        }
        // Unknown commands stay in the line, whatever follows them.
        submit(&mut a, "/shrug that is a long sentence after a typo");
        assert!(a.status.contains("unknown command /shrug"), "{}", a.status);
        assert_eq!(a.input.as_str(), "/shrug that is a long sentence after a typo");
        a.input.take();
        // Group commands in a 1:1 chat say so instead of sending.
        submit(&mut a, "/invite");
        assert!(a.status.contains("for groups"), "{}", a.status);
        assert_eq!(a.input.as_str(), "/invite");
        a.input.take();
        submit(&mut a, "/save");
        assert!(a.status.contains("usage: /save PATH"), "{}", a.status);
        a.input.take();
        // Paths and `//` are text.
        submit(&mut a, "/usr/bin is where it is");
        assert_eq!(a.input.as_str(), "");
        submit(&mut a, "//settings");
        assert_eq!(a.input.as_str(), "");
        assert!(is_command_word("/mute") && !is_command_word("/usr/bin") && !is_command_word("/"));
    }

    #[test]
    fn plain_q_does_not_quit_or_close_the_viewer() {
        let mut a = app();
        key(&mut a, KeyCode::Char('q'));
        assert!(!a.quit);

        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.sel = Some(0);
        a.viewer = Some("00".into());
        key(&mut a, KeyCode::Char('q'));
        assert_eq!(a.viewer.as_deref(), Some("00"));
        assert!(!a.quit);
    }

    fn click(a: &mut App, x: u16, y: u16) {
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn invitations_sit_on_top_and_need_answering() {
        let mut a = app();
        a.list.select(Some(1));
        let chat: Chat = serde_json::from_value(serde_json::json!({
            "chatMid": "Cinv", "chatName": "Party", "createdTime": "5"
        }))
        .unwrap();
        a.on_event(AppEvent::Invitations(vec![chat], false));
        assert_eq!(a.chats[0].mid, "Cinv", "invitations are listed first");
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"), "selection is kept");
        assert_eq!(a.name("Cinv"), "Party");

        a.list.select(Some(0));
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.open.as_deref(), a.focus), (None, Focus::Chats), "not opened");
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("Enter on an invitation asks what to do")
        };
        assert!(menu.title.contains("Party"), "{}", menu.title);
        assert_eq!(menu.items[0].1, Action::Accept("Cinv".into()));

        // Declining asks again, starting on the safe button.
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        assert!(
            matches!(&a.modal, Some(Modal::Confirm(c)) if !c.yes && c.action == Action::Decline("Cinv".into()))
        );
        key(&mut a, KeyCode::Enter);
        assert!(
            a.modal.is_none() && a.chats[0].invited,
            "Enter on Cancel keeps it"
        );
        // Letters do not answer invitations by accident.
        for c in ['a', 'd', 'd'] {
            key(&mut a, KeyCode::Char(c));
        }
        assert!(a.modal.is_none() && a.chats[0].invited);
        // The menu key offers the same choices.
        key(&mut a, KeyCode::Char('m'));
        assert!(matches!(&a.modal, Some(Modal::Menu(m)) if m.items[0].1 == Action::Accept("Cinv".into())));
        a.modal = None;

        a.on_event(AppEvent::InviteAnswered("Cinv".into(), false, Err("boom".into())));
        assert!(a.chats[0].invited, "a failure keeps the invitation");
        a.on_event(AppEvent::InviteAnswered("Cinv".into(), true, Ok(())));
        let c = a.chats.iter().find(|c| c.mid == "Cinv").unwrap();
        assert!(!c.invited && c.preview == "You joined");
        a.on_event(AppEvent::Invitations(
            vec![serde_json::from_value(serde_json::json!({"chatMid": "Cx", "chatName": "X"})).unwrap()],
            false,
        ));
        a.on_event(AppEvent::InviteAnswered("Cx".into(), false, Ok(())));
        assert!(
            a.chats.iter().all(|c| c.mid != "Cx"),
            "declined invitations vanish"
        );
        assert!(a.list.selected().is_some_and(|i| i < a.chats.len()));
    }

    #[test]
    fn pin_lists_switch_create_rename_delete_and_persist() {
        let mut a = app();
        a.set_pinned("Ub", true);
        assert_eq!(a.pinned, ["Ub"]);
        assert_eq!(a.pin_list, None, "a single list is not named on screen");

        // "Pin lists…" (main menu, Settings › Chats) opens the switcher;
        // create a list from it (it becomes active, empty).
        a.run_action(Action::PinLists);
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("pin list menu")
        };
        assert!(menu.items[menu.sel].0.starts_with("● Default · 1 pin"));
        a.run_action(Action::NewPinList);
        key(&mut a, KeyCode::Enter);
        assert!(
            matches!(&a.modal, Some(Modal::Form(f)) if f.error.is_some()),
            "blank name refused"
        );
        type_text(&mut a, "default");
        key(&mut a, KeyCode::Enter);
        assert!(
            matches!(&a.modal, Some(Modal::Form(f)) if f.error.is_some()),
            "duplicate refused"
        );
        a.modal = None;
        a.run_action(Action::NewPinList);
        type_text(&mut a, "Work");
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        assert!(a.pinned.is_empty());
        assert_eq!(a.pin_list.as_deref(), Some("Work"));
        a.set_pinned("Ua", true);
        let screen = screen(&mut a);
        assert!(screen.contains("── pinned · Work ──"), "{screen}");

        // Switching back restores the other list and its order.
        let lists = a.store.pin_profiles().unwrap();
        let (default, work) = (lists[0].id, lists[1].id);
        a.run_action(Action::SwitchPinList(default));
        assert_eq!(a.pinned, ["Ub"]);
        assert_eq!(a.chats[0].mid, "Ub");
        a.run_action(Action::SwitchPinList(work));
        assert_eq!(a.pinned, ["Ua"]);
        assert_eq!(a.chats[0].mid, "Ua");

        a.run_action(Action::RenamePinList(work));
        let Some(Modal::Form(f)) = &mut a.modal else {
            panic!("rename form")
        };
        assert_eq!(f.fields[0].value, "Work");
        f.fields[0].value = "Office".into();
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.pin_list.as_deref(), Some("Office"));

        // The active list survives restarting the app.
        let (tx, _rx) = mpsc::unbounded_channel();
        let b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            a.store.clone(),
        );
        assert_eq!(b.pinned, ["Ua"]);
        assert_eq!(b.pin_list.as_deref(), Some("Office"));

        a.run_action(Action::ConfirmDeletePinList(work));
        assert!(matches!(&a.modal, Some(Modal::Confirm { .. })));
        a.modal = None;
        a.run_action(Action::DeletePinList(work));
        assert_eq!(a.pinned, ["Ub"]);
        assert_eq!(a.pin_list, None);
        a.pin_lists_menu();
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("pin list menu")
        };
        assert!(
            !menu
                .items
                .iter()
                .any(|(_, action)| matches!(action, Action::ConfirmDeletePinList(_))),
            "the last list cannot be deleted"
        );
    }

    #[test]
    fn pinning_keeps_the_selection_at_the_same_list_position() {
        let mut a = app();
        a.chats.push(ChatEntry {
            mid: "Uc".into(),
            ..Default::default()
        });
        a.list.select(Some(2));
        a.set_pinned("Uc", true);
        assert_eq!(
            a.chats.iter().map(|c| c.mid.as_str()).collect::<Vec<_>>(),
            ["Uc", "Ua", "Ub"]
        );
        assert_eq!(a.list.selected(), Some(2));
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));

        a.list.select(Some(0));
        a.set_pinned("Ub", true);
        assert_eq!(
            a.selected_mid().as_deref(),
            Some("Uc"),
            "pinning another chat keeps the selected chat"
        );
    }

    #[tokio::test]
    async fn unread_pinned_chats_float_to_the_top_until_read() {
        let mut a = app();
        a.no_read_receipts_flag = false;
        a.prefs.read_receipts = true;
        // Opening a chat fetches history: LINE is a closed local port.
        a.client = Client::new(Config {
            gateway: "http://127.0.0.1:9".into(),
            ..Config::default()
        })
        .unwrap();
        for mid in ["Uc", "Ud"] {
            a.chats.push(ChatEntry {
                mid: mid.into(),
                ..Default::default()
            });
        }
        for mid in ["Ua", "Ub", "Uc"] {
            a.set_pinned(mid, true);
        }
        let order = |a: &App| a.chats.iter().map(|c| c.mid.clone()).collect::<Vec<_>>();
        assert_eq!(order(&a), ["Ua", "Ub", "Uc", "Ud"]);
        let receive = |a: &mut App, rev: i64, id: &str, from: &str| {
            a.on_operation(op(serde_json::json!({
                "revision": rev, "type": 26, "message": sent(id, from, "Ume", "hi")
            })));
        };

        a.select_chat_mid(Some("Ua"));
        receive(&mut a, 1, "100", "Uc");
        assert_eq!(order(&a), ["Uc", "Ua", "Ub", "Ud"], "an unread pin floats up");
        assert_eq!(
            a.selected_mid().as_deref(),
            Some("Ua"),
            "the selection stays on its chat"
        );
        // An unpinned chat's unread message does not pass the pins.
        receive(&mut a, 2, "101", "Ud");
        assert_eq!(order(&a), ["Uc", "Ua", "Ub", "Ud"]);
        // Several unread pins keep their pin order among themselves.
        receive(&mut a, 3, "102", "Ub");
        assert_eq!(order(&a), ["Ub", "Uc", "Ua", "Ud"]);
        receive(&mut a, 4, "103", "Uc");
        assert_eq!(
            order(&a),
            ["Ub", "Uc", "Ua", "Ud"],
            "newer messages do not reshuffle them"
        );

        // Reading here (opening the chat) returns it to its pin position.
        a.open_chat("Uc".into());
        assert_eq!(order(&a), ["Ub", "Ua", "Uc", "Ud"]);
        // Reading elsewhere (another device) does too.
        a.on_operation(op(serde_json::json!({
            "revision": 5, "type": 40, "param1": "Ub", "param2": "102", "param3": "0"
        })));
        assert_eq!(order(&a), ["Ua", "Ub", "Uc", "Ud"]);
        assert_eq!(a.pinned, ["Ua", "Ub", "Uc"], "the pin order itself never changes");
        assert_eq!(a.store.pinned_chats().unwrap(), ["Ua", "Ub", "Uc"]);
    }

    /// `app()` plus unpinned chats `U00`… (`U00` newest) with nothing unread.
    fn many_chats(n: usize) -> App {
        let mut a = app();
        a.chats.clear();
        for i in 0..n {
            a.chats.push(ChatEntry {
                mid: format!("U{i:02}"),
                last_time: 1_000 - i as i64,
                ..Default::default()
            });
        }
        a.open = None;
        a.focus = Focus::Chats;
        a.sort_chats(None);
        a
    }

    fn set_unread(a: &mut App, mid: &str, unread: u32) {
        a.chats.iter_mut().find(|c| c.mid == mid).unwrap().unread = unread;
        a.sort_chats(a.selected_mid());
    }

    #[test]
    fn unread_chats_sit_under_the_pins_until_read() {
        let mut a = many_chats(6);
        let order = |a: &App| {
            a.chats
                .iter()
                .map(|c| c.mid.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        a.set_pinned("U05", true);
        assert_eq!(order(&a), "U05 U00 U01 U02 U03 U04");
        a.select_chat_mid(Some("U01"));
        set_unread(&mut a, "U03", 1);
        set_unread(&mut a, "U02", 4);
        // Under the pin, among themselves by activity; the rest by activity.
        assert_eq!(order(&a), "U05 U02 U03 U00 U01 U04");
        assert_eq!(
            a.selected_mid().as_deref(),
            Some("U01"),
            "the selection follows its chat"
        );
        // An unread pin stays in the pinned section.
        set_unread(&mut a, "U05", 1);
        assert_eq!(order(&a), "U05 U02 U03 U00 U01 U04");
        // Muted chats do not float; muting a floating one drops it back.
        a.chats.iter_mut().find(|c| c.mid == "U04").unwrap().muted = true;
        set_unread(&mut a, "U04", 9);
        assert_eq!(order(&a), "U05 U02 U03 U00 U01 U04");
        a.on_event(AppEvent::Mutes(HashMap::from([("U02".to_owned(), true)])));
        assert_eq!(order(&a), "U05 U03 U00 U01 U02 U04");
        // Read: back to its activity position, nothing about it is kept.
        set_unread(&mut a, "U03", 0);
        assert_eq!(order(&a), "U05 U00 U01 U02 U03 U04");
        assert_eq!(a.pinned, ["U05"]);
    }

    #[test]
    fn nearest_unread_prefers_the_closest_then_the_one_above() {
        assert_eq!(nearest_unread(&[], 3), None);
        assert_eq!(nearest_unread(&[3], 3), None, "never the selection itself");
        assert_eq!(nearest_unread(&[0, 9], 3), Some(0));
        assert_eq!(nearest_unread(&[0, 5], 3), Some(5));
        assert_eq!(nearest_unread(&[1, 3, 5], 3), Some(1), "a tie goes up");
    }

    #[test]
    fn unread_chats_out_of_view_are_counted_and_reachable() {
        let mut a = many_chats(30);
        set_unread(&mut a, "U10", 1);
        set_unread(&mut a, "U20", 2);
        let s = screen(&mut a);
        assert!(
            !s.contains('▲') && !s.contains('▼'),
            "both on screen: no indicator\n{s}"
        );
        assert!(a.hit.unread_above.is_none() && a.hit.unread_below.is_none());

        // Scrolled to the end, both are above.
        key(&mut a, KeyCode::End);
        let s = screen(&mut a);
        assert!(s.contains("▲ 2 unread"), "{s}");
        assert!(!s.contains('▼'), "{s}");
        assert!(a.available(keymap::Cmd::NextUnread));
        let r = a.hit.unread_above.expect("indicator area");
        click(&mut a, r.x + 1, r.y);
        assert_eq!(
            a.selected_mid().as_deref(),
            Some("U20"),
            "the nearest one above the view"
        );
        // The view scrolled to it; the other one is still above.
        let s = screen(&mut a);
        assert!(s.contains("▲ 1 unread") && s.contains("U20…"), "{s}");

        // `u` goes from one to the other, and says so when there is no other.
        key(&mut a, KeyCode::Char('u'));
        assert_eq!(a.selected_mid().as_deref(), Some("U10"));
        set_unread(&mut a, "U20", 0);
        assert!(!a.available(keymap::Cmd::NextUnread));
        key(&mut a, KeyCode::Char('u'));
        assert_eq!(a.selected_mid().as_deref(), Some("U10"));
        assert_eq!(a.status, "no other unread chat");
        assert!(!screen(&mut a).contains('▲'));
    }

    #[test]
    fn unread_indicator_keeps_the_list_title_readable() {
        let mut a = many_chats(30);
        set_unread(&mut a, "U10", 1);
        key(&mut a, KeyCode::End);
        // Receipts on: room for the whole title and label.
        a.no_read_receipts_flag = false;
        a.prefs.read_receipts = true;
        let s = screen(&mut a);
        assert!(s.contains("Chats · 1 unread") && s.contains("▲ 1 unread"), "{s}");
        // Receipts off (the badge takes the space): " Chats " is never covered.
        a.no_read_receipts_flag = true;
        let s = screen(&mut a);
        assert!(s.contains("╭ Chats ") && s.contains('▲'), "{s}");
    }

    #[test]
    fn unread_chats_below_many_pins_are_counted_below() {
        let mut a = many_chats(30);
        for i in 0..15 {
            a.set_pinned(&format!("U{i:02}"), true);
        }
        set_unread(&mut a, "U25", 1);
        key(&mut a, KeyCode::Home);
        let s = screen(&mut a);
        assert!(s.contains("▼ 1 "), "{s}");
        let r = a.hit.unread_below.expect("indicator area");
        click(&mut a, r.x + 1, r.y);
        assert_eq!(a.selected_mid().as_deref(), Some("U25"));
        assert!(!screen(&mut a).contains('▼'));
    }

    #[test]
    fn pinned_chats_survive_updates_and_app_recreation() {
        let mut a = app();
        let invite: Chat = serde_json::from_value(serde_json::json!({
            "chatMid": "Cinv", "chatName": "Party", "createdTime": "5"
        }))
        .unwrap();
        a.on_event(AppEvent::Invitations(vec![invite], false));

        a.chat_menu("Cinv");
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("invitation menu")
        };
        assert!(
            !menu
                .items
                .iter()
                .any(|(_, action)| matches!(action, Action::Pin(..)))
        );
        a.set_pinned("Cinv", true);
        assert!(!a.store.is_pinned("Cinv").unwrap());

        a.chat_menu("Ua");
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("chat menu")
        };
        let pin = menu
            .items
            .iter()
            .find_map(|(_, action)| match action {
                Action::Pin(mid, true) if mid == "Ua" => Some(action.clone()),
                _ => None,
            })
            .expect("pin action");
        a.run_action(pin);
        assert_eq!(a.chats.iter().position(|c| c.mid == "Ua"), Some(1));
        assert_eq!(a.chats[0].mid, "Cinv", "invitations remain ahead of pins");
        assert!(a.store.is_pinned("Ua").unwrap());
        a.persist_chat("Ua");
        a.set_pinned("Ub", true);

        a.chat_menu("Ua");
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("pinned chat menu")
        };
        let move_down = menu
            .items
            .iter()
            .find_map(|(_, action)| match action {
                Action::MovePin(mid, 1) if mid == "Ua" => Some(action.clone()),
                _ => None,
            })
            .expect("move pin down action");
        a.run_action(move_down);
        assert_eq!(a.pinned, ["Ub", "Ua"]);
        assert_eq!(a.chats.iter().position(|c| c.mid == "Ub"), Some(1));
        a.modal = None;
        a.select_chat_mid(Some("Ua"));
        key(&mut a, KeyCode::Char('K'));
        assert_eq!(a.pinned, ["Ua", "Ub"], "K moves the selected pin up");

        a.on_event(AppEvent::Chats(vec![ChatEntry {
            mid: "Uc".into(),
            last_time: 100,
            ..Default::default()
        }]));
        assert_eq!(
            a.chats.iter().map(|c| c.mid.as_str()).collect::<Vec<_>>(),
            ["Cinv", "Ua", "Ub", "Uc"]
        );
        let screen = screen(&mut a);
        assert!(screen.contains("📌"), "pinned chat marker: {screen}");
        assert!(
            screen.contains("── pinned ──"),
            "pinned section divider: {screen}"
        );

        let (tx, _rx) = mpsc::unbounded_channel();
        let b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            a.store.clone(),
        );
        assert_eq!(b.pinned, ["Ua", "Ub"]);
        assert_eq!(b.chats.iter().position(|c| c.mid == "Ua"), Some(1));

        a.chat_menu("Ua");
        let Some(Modal::Menu(menu)) = &a.modal else {
            panic!("chat menu")
        };
        let unpin = menu
            .items
            .iter()
            .find_map(|(_, action)| match action {
                Action::Pin(mid, false) if mid == "Ua" => Some(action.clone()),
                _ => None,
            })
            .expect("unpin action");
        a.run_action(unpin);
        assert_eq!(a.chats.iter().position(|c| c.mid == "Ub"), Some(1));
        assert_eq!(a.chats.iter().position(|c| c.mid == "Uc"), Some(2));
        assert_eq!(a.chats.iter().position(|c| c.mid == "Ua"), Some(3));
        assert!(!a.store.is_pinned("Ua").unwrap());
    }

    #[tokio::test]
    async fn reply_to_selected_message() {
        let mut a = app();
        a.names.insert("Ume".into(), "Me".into());
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter); // open "Ua"
        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Up); // select m1
        key(&mut a, KeyCode::Char('r'));
        assert_eq!((a.replying.as_deref(), a.focus), (Some("01"), Focus::Input));
        assert_eq!(a.quote("Ua", "01", 40), "You: m1");
        assert_eq!(a.quote("Ua", "nope", 40), "an earlier message");
        key(&mut a, KeyCode::Esc);
        assert_eq!(
            (a.replying.as_deref(), a.focus),
            (None, Focus::Input),
            "Esc cancels first"
        );

        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Char('r'));
        for c in "ok".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        assert_eq!(
            (a.replying.as_deref(), a.input.as_str()),
            (None, ""),
            "sending consumes it"
        );

        // "Go to quoted message" on a reply selects the message it quotes.
        let msgs = a.messages.get_mut("Ua").unwrap();
        msgs[2].reply_to = Some("00".into());
        a.focus = Focus::Messages;
        a.select_msg(Some(2));
        a.run_action(Action::GotoQuoted);
        assert_eq!(a.sel, Some(0));
        a.run_action(Action::GotoQuoted);
        assert_eq!(a.status, "not a reply");

        // Switching chats drops a pending reply.
        key(&mut a, KeyCode::Char('r'));
        a.open_chat("Ub".into());
        assert_eq!(a.replying, None);
    }

    #[test]
    fn enter_forward_esc_back() {
        let mut a = app();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.open.as_deref(), a.focus), (Some("Ua"), Focus::Input));
        key(&mut a, KeyCode::Up);
        assert_eq!((a.focus, a.sel), (Focus::Messages, Some(2)));
        key(&mut a, KeyCode::Up);
        assert_eq!(a.sel, Some(1));
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.focus, Focus::Input, "↓ past the newest returns to typing");
        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Char('z'));
        assert_eq!(
            (a.focus, a.input.as_str()),
            (Focus::Input, "z"),
            "typing leaves the list"
        );
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.focus, Focus::Chats);
        key(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Focus::Input);
    }

    #[tokio::test]
    async fn pictures_on_screen_download_first() {
        let mut a = app();
        let n = 40;
        let msgs: Vec<UiMsg> = (0..n)
            .map(|j| {
                let m = serde_json::json!({"id": format!("{}", 100 + j), "from": "Ume", "contentType": 1});
                to_ui(&serde_json::from_value::<Message>(m).unwrap(), None).unwrap()
            })
            .collect();
        a.messages.insert("Ua".into(), msgs);
        a.open = Some("Ua".into());
        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        a.start_media();
        // The newest messages are at the bottom, on screen: they go first, and
        // only a few at once — not the whole history, oldest first.
        let loading: Vec<usize> = (0..n)
            .filter(|j| matches!(a.thumbs.get(&format!("{}", 100 + j)), Some(Load::Loading)))
            .collect();
        assert_eq!(loading, ((n - fetchq::THUMB_SLOTS)..n).collect::<Vec<_>>());
        // What waits shows as loading and is started when a slot frees.
        term.draw(|f| a.draw(f)).unwrap();
        a.start_media();
        a.on_event(AppEvent::Thumb(format!("{}", 100 + n - 1), Err("x".into())));
        term.draw(|f| a.draw(f)).unwrap();
        a.start_media();
        let next = format!("{}", 100 + n - 1 - fetchq::THUMB_SLOTS);
        assert!(matches!(a.thumbs.get(&next), Some(Load::Loading)));
    }

    #[test]
    fn rows_away_from_the_window() {
        assert_eq!(rows_away(5, 10, 4), 5);
        assert_eq!(rows_away(10, 10, 4), fetchq::VISIBLE);
        assert_eq!(rows_away(13, 10, 4), fetchq::VISIBLE);
        assert_eq!(rows_away(14, 10, 4), 1);
    }

    #[tokio::test]
    async fn mouse_clicks_and_wheel() {
        let mut a = app();
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        // Second chat: rows 0-2 are the first chat, 3-5 the second.
        let r = a.hit.chats;
        click(&mut a, r.x + 2, r.y + CHAT_ROW_H);
        assert_eq!(a.open.as_deref(), Some("Ub"));
        term.draw(|f| a.draw(f)).unwrap();
        let (y0, _, i) = a.hit.msgs[0];
        let r = a.hit.conv;
        // Row 0 is the sender's name (opens their profile), row 2 the text.
        click(&mut a, r.x + 3, y0 + 2);
        assert_eq!((a.focus, a.sel), (Focus::Messages, Some(i)));
        let r = a.hit.input;
        click(&mut a, r.x + 3, r.y + 1);
        assert_eq!(a.focus, Focus::Input);
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: a.hit.conv.x + 1,
            row: a.hit.conv.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(a.scroll, 3);
    }

    #[test]
    fn bubbles_wrap_and_align() {
        let b = bubble("hello world", 11, false, Style::new(), Style::new());
        let text: Vec<String> = b.iter().map(|(l, _)| l.to_string()).collect();
        assert_eq!(text, [" ╭───────╮", " │ hello │", " │ world │", " ╰───────╯"]);
        let b = bubble("日本", 20, true, Style::new(), Style::new());
        assert_eq!(b[1].0.to_string(), "│ 日本 │ ");
        assert_eq!(b[1].0.alignment, Some(ratatui::layout::Alignment::Right));
    }

    #[test]
    fn bubbles_mark_links() {
        let b = bubble("go https://a.bc/xyz now", 14, false, Style::new(), Style::new());
        let text: Vec<String> = b.iter().map(|(l, _)| l.to_string()).collect();
        assert_eq!(
            &text[1..4],
            [" │ go         │", " │ https://a. │", " │ bc/xyz now │"]
        );
        // The link wraps across rows; every piece opens the whole URL, and
        // "now" after it on the same row is not part of it.
        let url = "https://a.bc/xyz".to_owned();
        assert!(b[1].1.is_empty());
        assert_eq!(
            b[2].1,
            [LinkSpan {
                off: 3,
                w: 10,
                url: url.clone()
            }]
        );
        assert_eq!(b[3].1, [LinkSpan { off: 3, w: 6, url }]);
    }

    #[tokio::test]
    async fn profile_from_name_and_title() {
        let mut a = app();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        term.draw(|f| a.draw(f)).unwrap();
        let (y, from) = a.hit.heads[0].clone();
        let x = a.hit.conv.x + 2;
        click(&mut a, x, y);
        assert_eq!(a.profile, [from]);
        // Back closes it; the title bar opens the chat's profile.
        key(&mut a, KeyCode::Esc);
        assert!(a.profile.is_empty());
        term.draw(|f| a.draw(f)).unwrap();
        let t = a.hit.conv_title;
        click(&mut a, t.x + 3, t.y);
        assert_eq!(a.profile, ["Ua"]);
        term.draw(|f| a.draw(f)).unwrap();
        assert!(a.hit.profile.is_some());
        // Clicking outside the page closes it.
        click(&mut a, 0, 0);
        assert!(a.profile.is_empty());
    }

    #[test]
    fn opens_links_from_selected_message() {
        let mut a = app();
        a.messages.get_mut("Ua").unwrap()[2].text = "see https://example.com/x".into();
        a.qr.insert(
            "01".into(),
            vec!["https://qr.example/y".into(), "WIFI:S:home;;".into()],
        );
        let m2 = a.messages["Ua"][2].clone();
        let m1 = a.messages["Ua"][1].clone();
        assert_eq!(a.links_of(&m2), ["https://example.com/x"]);
        assert_eq!(
            a.links_of(&m1),
            ["https://qr.example/y"],
            "non-web QR text is not a link"
        );
    }

    #[test]
    fn repeated_downloads_reuse_the_file() {
        let dir = std::env::temp_dir().join(format!("rune-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = store_download_in(&dir, "pic.jpg", b"one").unwrap();
        let b = store_download_in(&dir, "pic.jpg", b"one").unwrap();
        let c = store_download_in(&dir, "pic.jpg", b"two").unwrap();
        let d = store_download_in(&dir, "pic.jpg", b"two").unwrap();
        assert_eq!(a, b);
        assert_eq!(c, d);
        assert_ne!(a, c);
        assert_eq!(c.file_name().unwrap(), "pic (1).jpg");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sender_file_names_are_made_safe() {
        assert_eq!(safe_file_name("../../.bashrc"), "_.._.bashrc");
        assert_eq!(safe_file_name(".hidden.txt"), "hidden.txt");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name(""), "file");
        assert_eq!(safe_file_name("a\x1b[2Jb\n.jpg"), "a_[2Jb_.jpg");
        let long = format!("{}.jpeg", "é".repeat(300));
        let s = safe_file_name(&long);
        assert!(s.len() <= MAX_FILE_NAME && s.ends_with(".jpeg"), "{s}");
        let no_ext = "x".repeat(400);
        assert_eq!(safe_file_name(&no_ext).len(), MAX_FILE_NAME);
    }

    #[test]
    fn downloads_never_replace_or_write_through_existing_entries() {
        let dir = std::env::temp_dir().join(format!("rune-dl-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A symlink under the wanted name is skipped, its target untouched.
        let target = dir.join("target");
        std::fs::write(&target, b"keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dir.join("pic.jpg")).unwrap();
        let p = store_download_in(&dir, "pic.jpg", b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        // Concurrent saves of different files under one name each get their own.
        let paths: Vec<PathBuf> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8u8)
                .map(|i| {
                    let dir = &dir;
                    s.spawn(move || store_download_in(dir, "same.bin", &[i; 4096]).unwrap())
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (i, p) in paths.iter().enumerate() {
            assert_eq!(std::fs::read(p).unwrap(), vec![i as u8; 4096]);
        }
        // `write_new` refuses an existing path.
        let e = write_new(&target, b"x").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wraps_by_display_width() {
        assert_eq!(wrap("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap("日本語です", 4), vec!["日本", "語で", "す"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap("hello big world", 9), vec!["hello big", "world"]);
        assert_eq!(wrap("hi wonderful", 6), vec!["hi", "wonder", "ful"]);
    }

    #[test]
    fn describes_messages() {
        let mut m = Message {
            content_type: ct::FILE,
            ..Default::default()
        };
        m.content_metadata.insert("FILE_NAME".into(), "a.pdf".into());
        assert_eq!(describe(&m, None).unwrap(), "📎 [file a.pdf]");
        m.content_type = 18;
        assert!(describe(&m, None).is_none());

        let mut call = Message {
            content_type: ct::CALL,
            ..Default::default()
        };
        call.content_metadata.insert("TYPE".into(), "V".into());
        call.content_metadata.insert("DURATION".into(), "75000".into());
        assert_eq!(describe(&call, None).unwrap(), "📹 Video call · 1:15");
        call.content_metadata.insert("RESULT".into(), "REJECTED".into());
        assert_eq!(describe(&call, None).unwrap(), "📹 Video call · declined ✗");
    }

    fn op(v: serde_json::Value) -> Operation {
        serde_json::from_value(v).unwrap()
    }

    fn sent(id: &str, from: &str, to: &str, text: &str) -> Message {
        serde_json::from_value(serde_json::json!({"id": id, "from": from, "to": to,
            "toType": if to.starts_with('C') { 2 } else { 0 }, "contentType": 0, "text": text,
            "createdTime": now_ms().to_string()}))
        .unwrap()
    }

    /// The notices `f` would show (tests deliver none).
    fn notices(f: impl FnOnce()) -> Vec<(String, String)> {
        notice::SHOWN.with(|s| s.borrow_mut().clear());
        f();
        notice::SHOWN.with(|s| s.borrow_mut().drain(..).map(|n| (n.title, n.body)).collect())
    }

    #[test]
    fn notifications_name_the_sender_and_the_chat() {
        let mut a = app();
        a.prefs.notify = NotifyMode::Desktop;
        a.names.insert("Ua".into(), "Alice".into());
        a.names.insert("Cg".into(), "Climbing".into());
        let receive = |a: &mut App, m: Message| {
            a.on_operation(op(serde_json::json!({"revision": 1, "type": 26, "message": m})))
        };
        let shown = notices(|| receive(&mut a, sent("10", "Ua", "Ume", "hi")));
        assert_eq!(shown, [("Alice".into(), "hi".into())]);
        let shown = notices(|| receive(&mut a, sent("11", "Ua", "Cg", "hi all")));
        assert_eq!(shown, [("Climbing".into(), "Alice: hi all".into())]);
        // An unknown sender is a word, not a mid.
        let shown = notices(|| receive(&mut a, sent("12", "Uzz", "Cg", "yo")));
        assert_eq!(shown, [("Climbing".into(), "Someone: yo".into())]);

        a.prefs.notify_preview = false;
        let shown = notices(|| receive(&mut a, sent("13", "Ua", "Ume", "secret")));
        assert_eq!(shown, [("Alice".into(), "New message".into())]);

        // Muted chats, replayed history and our own messages stay quiet.
        a.chats.iter_mut().find(|c| c.mid == "Ua").unwrap().muted = true;
        assert!(notices(|| receive(&mut a, sent("14", "Ua", "Ume", "x"))).is_empty());
        let mut old = sent("15", "Ub", "Ume", "x");
        old.created_time = Some(now_ms() - NOTIFY_MAX_AGE_MS - 1);
        assert!(notices(|| receive(&mut a, old)).is_empty());
        assert!(notices(|| receive(&mut a, sent("16", "Ume", "Ub", "x"))).is_empty());
    }

    /// Regression: a message the stream delivers again (a replay, a full
    /// sync) was notified again, though it was stored and shown only once.
    #[test]
    fn a_message_delivered_twice_notifies_once() {
        let mut a = app();
        a.prefs.notify = NotifyMode::Desktop;
        a.names.insert("Ua".into(), "Alice".into());
        let m = sent("20", "Ua", "Ume", "hi");
        let receive = |a: &mut App, rev: i64| {
            a.on_operation(op(
                serde_json::json!({"revision": rev, "type": 26, "message": m.clone()}),
            ))
        };
        assert_eq!(notices(|| receive(&mut a, 1)).len(), 1);
        assert!(
            notices(|| receive(&mut a, 2)).is_empty(),
            "already stored: not news"
        );
        // Also in a chat whose messages are loaded.
        a.open_chat("Ua".into());
        let again = notices(|| receive(&mut a, 3));
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn invitations_notify_with_the_group_name() {
        let mut a = app();
        a.prefs.notify = NotifyMode::Desktop;
        let chat: Chat = serde_json::from_value(serde_json::json!({
            "chatMid": "Cinv", "chatName": "Party", "createdTime": "5"
        }))
        .unwrap();
        let shown = notices(|| a.on_event(AppEvent::Invitations(vec![chat.clone()], false)));
        assert_eq!(
            shown,
            [("Party".into(), "You're invited to join this group".into())]
        );
        // Known already: no second notification.
        assert!(notices(|| a.on_event(AppEvent::Invitations(vec![chat], false))).is_empty());
    }

    #[tokio::test]
    async fn starts_from_the_store_and_resumes() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_chat(&ChatRow {
                mid: "Cg".into(),
                name: "Friends".into(),
                preview: "see you".into(),
                last_time: 5,
                unread: 2,
                muted: true,
                ..Default::default()
            })
            .unwrap();
        store
            .put_message("Cg", &sent("100", "Ua", "Cg", "see you"), None)
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut a = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            store.clone(),
        );
        a.no_read_receipts_flag = true;
        assert_eq!(a.chats.len(), 1, "chat list shown before any request");
        assert_eq!(
            (a.name("Cg"), a.chats[0].unread, a.chats[0].muted),
            ("Friends".into(), 2, true)
        );
        a.open_chat("Cg".into());
        assert_eq!(a.open_messages().len(), 1, "history from the store");
        assert!(a.loading.is_empty(), "no server request for a fresh chat");
        assert_eq!(
            store.chat("Cg").unwrap().unwrap().unread,
            2,
            "read receipts off: still unread"
        );

        // The server's list says there is a newer message we do not have.
        a.on_event(AppEvent::Chats(vec![ChatEntry {
            mid: "Cg".into(),
            last_id: Some("101".into()),
            last_time: 9,
            preview: "new".into(),
            unread: 1,
            ..Default::default()
        }]));
        assert!(a.stale.contains("Cg"));
        assert!(a.loading.contains("Cg"), "the open stale chat is refetched");
        assert!(a.chats[0].muted, "mute survives the merge");
        assert_eq!(a.chats[0].unread, 1, "receipts off: the server's count stands");
    }

    #[tokio::test]
    async fn a_live_message_in_an_unopened_chat_keeps_its_stored_history() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_chat(&ChatRow {
                mid: "Ua".into(),
                last_message_id: Some("102".into()),
                last_time: 3,
                ..Default::default()
            })
            .unwrap();
        let history: Vec<Message> = (100..103)
            .map(|id| sent(&id.to_string(), "Ua", "Ume", "old"))
            .collect();
        store.put_messages("Ua", &history, None).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut a = App::new(Client::new(Config::default()).unwrap(), "Ume".into(), tx, store);
        a.no_read_receipts_flag = true;
        // A message arrives while another chat (or none) is open.
        a.on_operation(op(serde_json::json!({"revision": 5, "type": 26,
            "message": sent("103", "Ua", "Ume", "new")})));
        assert_eq!(a.chats[0].unread, 1);
        assert_eq!(a.chats[0].preview, "new");
        // The replayed copy of the same message is not counted twice.
        a.on_operation(op(serde_json::json!({"revision": 6, "type": 26,
            "message": sent("103", "Ua", "Ume", "new")})));
        assert_eq!(a.chats[0].unread, 1);
        a.open_chat("Ua".into());
        let texts: Vec<&str> = a.open_messages().iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            ["old", "old", "old", "new"],
            "the stored history is shown too"
        );
    }

    #[tokio::test]
    async fn inactive_message_boxes_parse() {
        // Shape of an inactive box (live): no lastMessages, ids as an object, counts as strings.
        let page = serde_json::json!({"hasNext": false, "messageBoxes": [{
            "id": "Cold", "midType": 2, "lastMessages": [], "unreadCount": "3",
            "lastDeliveredMessageId": {"deliveredTime": "1768177972896", "messageId": "596172781734330507"},
            "lastSeenMessageId": "596172781734330500"}]});
        let c = parse_boxes(&Client::new(Config::default()).unwrap(), &page).await;
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].last_time, 1_768_177_972_896);
        assert_eq!(c[0].last_id.as_deref(), Some("596172781734330507"));
        assert_eq!(c[0].last_seen.as_deref(), Some("596172781734330500"));
        assert_eq!(c[0].unread, 3, "string counts are read");
        assert_eq!(box_message_id(&serde_json::json!("0")), None);
        assert_eq!(box_message_id(&serde_json::json!(5)).as_deref(), Some("5"));
    }

    #[tokio::test]
    async fn selecting_the_last_chat_loads_more() {
        let mut a = app();
        a.list.select(Some(0));
        a.maybe_more_chats();
        assert_eq!(a.more_chats, MoreChats::Ready(None), "not at the end yet");
        a.list.select(Some(a.chats.len() - 1));
        a.maybe_more_chats();
        assert_eq!(a.more_chats, MoreChats::Loading(None));
        let entry = |mid: &str, t: i64| ChatEntry {
            mid: mid.into(),
            last_time: t,
            ..Default::default()
        };
        a.on_event(AppEvent::MoreChats(Err("boom".into())));
        assert_eq!(a.more_chats, MoreChats::Ready(None), "a failure can be retried");
        a.more_chats = MoreChats::Loading(None);
        a.on_event(AppEvent::MoreChats(Ok((
            vec![entry("Ua", 1), entry("Cold", 1)],
            Some("Cold".into()),
        ))));
        assert_eq!(a.more_chats, MoreChats::Ready(Some("Cold".into())));
        assert_eq!(
            a.chats.iter().filter(|c| c.mid == "Ua").count(),
            1,
            "no duplicates"
        );
        assert!(a.chats.iter().any(|c| c.mid == "Cold"));
        a.more_chats = MoreChats::Loading(Some("Cold".into()));
        a.on_event(AppEvent::MoreChats(Ok((vec![entry("Colder", 0)], None))));
        assert_eq!(a.more_chats, MoreChats::Done);
        assert!(a.status.contains("all loaded"));
        a.list.select(Some(a.chats.len() - 1));
        a.maybe_more_chats();
        assert_eq!(a.more_chats, MoreChats::Done, "nothing left to load");
    }

    #[tokio::test]
    async fn a_refused_reaction_gives_way_to_the_previous_one() {
        use rune::Reaction;
        let mut a = app();
        let id = "00".to_owned(); // a message of chat Ua
        let mine = |a: &App| {
            a.find_msg(&id).and_then(|m| {
                m.reactions
                    .iter()
                    .find(|r| r.from == "Ume")
                    .map(|r| r.kind.clone())
            })
        };
        let nice = Some(ReactionKind::Predefined(Reaction::Nice as i64));
        let love = Some(ReactionKind::Predefined(Reaction::Love as i64));
        a.react(&id, Some(modal::NewReaction::Builtin(Reaction::Nice)));
        a.react(&id, Some(modal::NewReaction::Builtin(Reaction::Love)));
        assert_eq!(mine(&a), love, "shown at once");
        // LINE refuses ❤️: 👍 is back, on screen and in the store.
        a.on_reaction_failed(&id, nice.clone(), love.clone(), "refused".into());
        assert_eq!(mine(&a), nice);
        assert!(a.status.contains("reaction failed"));
        // A refusal for a reaction that was replaced meanwhile changes nothing.
        a.on_reaction_failed(&id, None, love, "late".into());
        assert_eq!(mine(&a), nice);
    }

    #[tokio::test]
    async fn messages_arriving_while_away_are_marked_read_on_return() {
        let mut a = app();
        a.no_read_receipts_flag = false;
        a.open_chat("Ua".into());
        a.on_focus(false);
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 7, "type": 26, "message": sent("900", "Ua", "Ume", "are you there?")})))));
        let chat = |a: &App| a.chats.iter().find(|c| c.mid == "Ua").cloned().unwrap();
        assert_eq!(chat(&a).last_id.as_deref(), Some("900"));
        assert_ne!(
            chat(&a).last_seen.as_deref(),
            Some("900"),
            "not read while the terminal is unfocused"
        );
        a.on_focus(true);
        assert_eq!(
            chat(&a).last_seen.as_deref(),
            Some("900"),
            "read once you are back"
        );
        // Focused: read as it arrives.
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 8, "type": 26, "message": sent("901", "Ua", "Ume", "ok")})))));
        assert_eq!(chat(&a).last_seen.as_deref(), Some("901"));
    }

    #[test]
    fn only_a_missing_session_leads_to_a_new_login() {
        let dir = std::env::temp_dir().join(format!("rune-session-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let p = path.to_str().unwrap();
        assert!(read_session(p).unwrap().is_none(), "missing: log in");
        std::fs::write(&path, b"{ truncated").unwrap();
        let err = read_session(p).unwrap_err();
        assert!(err.contains("cannot read the session"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{ truncated", "left as it was");
        Session::default().save(&path).unwrap();
        assert!(read_session(p).unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_store_writes_reach_the_status_line() {
        let mut a = app();
        assert_eq!(a.saved(Ok::<_, rune::Error>(1)), Some(1));
        a.report_store_failure();
        assert!(!a.status.contains("local store"));
        assert_eq!(
            a.saved::<()>(Err(rune::Error::Protocol("store: disk is full".into()))),
            None
        );
        a.report_store_failure();
        assert!(a.status.contains("could not save to the local store") && a.status.contains("disk is full"));
        // Reported once.
        a.status.clear();
        a.report_store_failure();
        assert!(a.status.is_empty());
    }

    #[test]
    fn view_cache_prune_drops_old_files_then_the_oldest_over_budget() {
        let dir = std::env::temp_dir().join(format!("rune-view-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let now = std::time::SystemTime::now();
        let put = |name: &str, len: usize, age_secs: u64| {
            let p = dir.join(name);
            std::fs::write(&p, vec![0u8; len]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(now - Duration::from_secs(age_secs)).unwrap();
        };
        put("ancient.jpg", 10, 30 * 24 * 3600);
        put("old.mp4", 60, 300);
        put("mid.png", 30, 200);
        put("new.gif", 30, 100);
        let week = Duration::from_secs(7 * 24 * 3600);
        assert_eq!(prune_dir(&dir, week, 100, now).unwrap(), 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        // Too old: gone; over the 100-byte budget: the oldest goes; directories stay.
        assert_eq!(left, ["mid.png", "new.gif", "sub"]);
        // Emptying (clear local data).
        assert_eq!(prune_dir(&dir, Duration::ZERO, 0, now).unwrap(), 2);
        assert_eq!(prune_dir(&dir.join("missing"), week, 0, now).unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn read_by_combines_receipts_with_replies_and_reactions() {
        let mut a = app();
        // Stored from an earlier run, then a read arrives before the chat is
        // opened: both must count.
        a.store.set_read_mark("Cg", "Ua", "500", Some(1_000)).unwrap();
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 1, "type": 55, "param1": "Cg", "param2": "Ub", "param3": "500",
            "createdTime": "2000"})))));
        let msg = |id: &str, from: &str, content_type: i64| {
            to_ui(
                &serde_json::from_value::<Message>(serde_json::json!({"id": id, "from": from,
                    "to": "Cg", "toType": 2, "contentType": content_type, "text": "x"}))
                .unwrap(),
                None,
            )
            .unwrap()
        };
        let mut later = msg("503", "Ume", ct::TEXT);
        later.reactions = vec![MessageReaction {
            from: "Ue".into(),
            at: 0,
            kind: ReactionKind::Predefined(2),
        }];
        a.messages.insert(
            "Cg".into(),
            vec![
                msg("500", "Ume", ct::TEXT),
                msg("501", "Uc", ct::TEXT),
                // A call record says nothing about reading.
                msg("502", "Ud", ct::CALL),
                later,
            ],
        );
        let members = ["Ume", "Ua", "Ub", "Uc", "Ud", "Ue"];
        a.members
            .insert("Cg".into(), members.iter().map(|m| (*m).to_owned()).collect());
        for (mid, name) in [
            ("Ua", "Alice"),
            ("Ub", "Bob"),
            ("Uc", "Carol"),
            ("Ud", "Dave"),
            ("Ue", "Eve"),
        ] {
            a.names.insert(mid.into(), name.into());
        }
        a.open_chat("Cg".into());
        let seen = |v: &[(&str, SeenBy)]| -> Vec<(String, SeenBy)> {
            v.iter().map(|(m, how)| ((*m).to_owned(), *how)).collect()
        };
        assert_eq!(
            a.readers("Cg", "500"),
            seen(&[
                ("Ua", SeenBy::Receipt),
                ("Ub", SeenBy::Receipt),
                ("Uc", SeenBy::WroteLater),
                ("Ue", SeenBy::Reacted),
            ])
        );
        assert_eq!(a.readers("Cg", "503"), seen(&[("Ue", SeenBy::Reacted)]));

        let (title, lines) = a.read_by_lines("500");
        assert_eq!(title, " Read by 4 of 5 ");
        assert_eq!(
            lines[..4],
            [
                format!("✓ Alice  {}", hhmm(1_000)),
                format!("✓ Bob    {}", hhmm(2_000)),
                "✓ Carol  wrote later".to_owned(),
                "✓ Eve    reacted".to_owned(),
            ],
            "receipts in reading order, then the rest by name"
        );
        assert_eq!(lines[5..7], ["Not yet (1):".to_owned(), "  Dave".to_owned()]);

        // The box follows new reads while it is open.
        a.run_action(Action::ReadBy("500".into()));
        assert!(screen(&mut a).contains("Read by 4 of 5"));
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 2, "type": 55, "param1": "Cg", "param2": "Ud", "param3": "503",
            "createdTime": "3000"})))));
        let rendered = screen(&mut a);
        assert!(rendered.contains("Read by 5 of 5"), "{rendered}");
        assert!(!rendered.contains("Not yet"), "{rendered}");
    }

    #[tokio::test]
    async fn operations_update_reads_reactions_and_unsend() {
        let mut a = app();
        a.open_chat("Cg".into());
        a.messages.insert("Cg".into(), Vec::new());
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 7, "type": 25, "message": sent("500", "Ume", "Cg", "hello all")})))));
        assert_eq!(a.open_messages().len(), 1);
        assert_eq!(a.store.stream_revision().unwrap(), Some(7), "position saved");
        for (rev, reader) in [(8, "Ua"), (9, "Ub")] {
            a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
                "revision": rev, "type": 55, "param1": "Cg", "param2": reader, "param3": "500"})))));
        }
        assert_eq!(
            a.readers("Cg", "500"),
            [
                ("Ua".to_owned(), SeenBy::Receipt),
                ("Ub".to_owned(), SeenBy::Receipt)
            ]
        );
        assert!(a.readers("Cg", "501").is_empty());
        a.names.insert("Ua".into(), "Alice".into());
        a.names.insert("Ub".into(), "Bob".into());
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 10, "type": 140, "param1": "500", "param3": "Ua",
            "param2": "{\"chatMid\":\"Cg\",\"curr\":{\"predefinedReactionType\":2}}"})))));
        assert_eq!(a.open_messages()[0].reactions.len(), 1);
        let line = a.reaction_line(&a.open_messages()[0].reactions, true).to_string();
        assert!(line.contains("👍"), "{line}");

        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let rendered: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("Read by 2"), "group read count shown");

        a.run_action(Action::Reactions("500".into()));
        let rendered = screen(&mut a);
        assert!(
            rendered.contains("Alice") && rendered.contains("👍"),
            "reaction user and emoji shown"
        );
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 11, "type": 140, "param1": "500", "param3": "Ub",
            "param2": "{\"chatMid\":\"Cg\",\"curr\":{\"predefinedReactionType\":2}}"})))));
        let rendered = screen(&mut a);
        assert!(
            rendered.contains("Alice") && rendered.contains("Bob"),
            "open reaction list refreshes live"
        );

        a.modal = None;
        screen(&mut a);
        let (row, id) = a
            .hit
            .reactions
            .iter()
            .find(|(_, id)| id == "500")
            .unwrap()
            .clone();
        assert_eq!(id, "500");
        let x = a.hit.conv.x;
        click(&mut a, x, row);
        assert!(matches!(a.modal, Some(Modal::Reactions { ref msg, .. }) if msg == "500"));

        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 12, "type": 65, "param1": "Cg", "param2": "500", "param3": "0"})))));
        assert!(a.open_messages()[0].unsent);
        assert!(a.store.message("500").unwrap().unwrap().unsent);
        assert_eq!(a.open_messages()[0].media_kind(), None);
        assert_eq!(a.reaction_lines("500").1, ["No reactions"]);

        // Read on the phone clears unread; leaving on the phone removes the chat.
        a.chats.push(ChatEntry {
            mid: "Uz".into(),
            unread: 3,
            last_id: Some("9".into()),
            ..Default::default()
        });
        a.on_event(AppEvent::Incoming(Box::new(op(serde_json::json!({
            "revision": 13, "type": 40, "param1": "Uz", "param2": "9", "param3": "0"})))));
        assert_eq!(a.chats.iter().find(|c| c.mid == "Uz").unwrap().unread, 0);
        a.on_event(AppEvent::Incoming(Box::new(op(
            serde_json::json!({"revision": 14, "type": 127, "param1": "Cg"}),
        ))));
        assert!(a.open.is_none() && a.chats.iter().all(|c| c.mid != "Cg"));
    }

    #[tokio::test]
    async fn menus_drive_group_actions() {
        let mut a = app();
        a.contacts.insert("Ua".into(), "Alice".into());
        a.contacts.insert("Ub".into(), "Bob".into());
        a.chats.insert(
            0,
            ChatEntry {
                mid: "Cg".into(),
                ..Default::default()
            },
        );
        a.names.insert("Cg".into(), "Trip".into());
        a.members.insert("Cg".into(), vec!["Ume".into(), "Ua".into()]);
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('m'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("chat menu")
        };
        let labels: Vec<&str> = m.items.iter().map(|i| i.0.as_str()).collect();
        assert!(labels.iter().any(|l| l.contains("Invite")) && labels.iter().any(|l| l.contains("Leave")));
        // "Invite friends…" lists only people who are not members yet.
        choose(&mut a, "Invite");
        let Some(Modal::Pick(p)) = &a.modal else {
            panic!("invite picker")
        };
        assert_eq!(p.options, [("Ub".to_owned(), "Bob".to_owned())]);
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        // Leave asks first; "n" cancels.
        a.run_action(Action::ConfirmLeave("Cg".into()));
        assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
        key(&mut a, KeyCode::Char('n'));
        assert!(a.modal.is_none() && a.chats.iter().any(|c| c.mid == "Cg"));
        a.on_event(AppEvent::Left("Cg".into(), Ok(())));
        assert!(a.chats.iter().all(|c| c.mid != "Cg"));

        // New group: name form, then member picker.
        key(&mut a, KeyCode::Char('n'));
        for c in "Pals".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        let Some(Modal::Pick(p)) = &a.modal else {
            panic!("member picker")
        };
        assert_eq!(p.kind, modal::PickKind::NewGroup("Pals".into()));
        key(&mut a, KeyCode::Esc);
        let chat: Chat =
            serde_json::from_value(serde_json::json!({"chatMid": "Cnew", "chatName": "Pals"})).unwrap();
        a.on_event(AppEvent::GroupCreated(Ok(chat.clone()), vec!["Ua".into()]));
        assert_eq!(a.open.as_deref(), Some("Cnew"));
        assert_eq!(a.store.chat("Cnew").unwrap().unwrap().name, "Pals");
        assert_eq!(a.members["Cnew"], [a.me.clone()]);
        assert_eq!(a.invitees["Cnew"], ["Ua".to_owned()]);
        // A repeated/late confirmation never lists the group twice.
        a.on_event(AppEvent::GroupCreated(Ok(chat), vec!["Ua".into()]));
        assert_eq!(a.chats.iter().filter(|c| c.mid == "Cnew").count(), 1);
        // Failure keeps the list unchanged and says why.
        a.on_event(AppEvent::GroupCreated(Err("boom".into()), vec![]));
        assert!(a.status.contains("could not create the group: boom"));

        // Blank or over-long names are refused in the form, keeping it open.
        a.modal = None;
        a.run_action(Action::NewGroup);
        key(&mut a, KeyCode::Char(' '));
        key(&mut a, KeyCode::Enter);
        let Some(Modal::Form(f)) = &a.modal else {
            panic!("name form stays open")
        };
        assert!(f.error.is_some());
        for _ in 0..60 {
            key(&mut a, KeyCode::Char('x'));
        }
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Form(f)) if f.error.is_some()));
        // Nobody to invite: the form (and the typed name) survives.
        a.modal = None;
        a.contacts.clear();
        a.run_action(Action::NewGroup);
        for c in " Pals ".chars() {
            key(&mut a, KeyCode::Char(c));
        }
        key(&mut a, KeyCode::Enter);
        let Some(Modal::Form(f)) = &a.modal else {
            panic!("name form stays open")
        };
        assert_eq!(f.fields[0].value, " Pals ");
        assert!(f.error.as_deref().is_some_and(|e| e.contains("loading")));
        a.modal = None;
        a.on_event(AppEvent::Muted("Cnew".into(), true, Ok(())));
        assert!(a.store.chat("Cnew").unwrap().unwrap().muted);
    }

    #[tokio::test]
    async fn react_with_a_line_emoji() {
        let mut a = app();
        let basic = rune::models::BASIC_EMOJI_PACKAGE;
        a.on_emoji_packages(Ok(vec![rune::EmojiPackage {
            id: basic.into(),
            name: "LINE Basic Emoji".into(),
            version: 3,
            resource_type: 1,
            emojis: vec![
                ("001".into(), "heart eyes Moon".into()),
                ("143".into(), "thumbs up".into()),
            ],
        }]));
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter); // open "Ua"
        key(&mut a, KeyCode::Up); // select our message m2
        key(&mut a, KeyCode::Char('e'));
        key(&mut a, KeyCode::Char('7')); // ✨ More LINE emoji…
        let Some(Modal::Stickers(s)) = &a.modal else {
            panic!("emoji picker")
        };
        assert_eq!(s.ctx(), keymap::Ctx::Emoji);
        assert_eq!(s.tab, 1, "nothing recent yet: starts on the package");
        key(&mut a, KeyCode::Enter); // the first emoji
        assert!(a.modal.is_none());
        let kind = ReactionKind::Paid {
            product_id: basic.into(),
            emoji_id: "001".into(),
        };
        assert_eq!(a.open_messages()[2].reactions[0].kind, kind);
        assert!(a.status.contains("heart eyes Moon"), "{}", a.status);
        let id = a.open_messages()[2].id.clone();
        assert!(a.reaction_lines(&id).1[0].contains("(heart eyes Moon)"));
        let line = a.reaction_line(&a.open_messages()[2].reactions, true).to_string();
        assert!(line.contains('✨'), "{line}");

        // The menu now offers it from "Recent", ticked; a Unicode stand-in is drawn where there is one.
        key(&mut a, KeyCode::Char('e'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("reaction menu")
        };
        assert_eq!(m.items.len(), 9, "six, the recent emoji, the picker, remove");
        assert_eq!(m.items[6].0, "✨ heart eyes Moon  ✓");
        assert_eq!(m.items[7].0, "✨ More LINE emoji…");
        key(&mut a, KeyCode::Char('8'));
        assert!(
            matches!(&a.modal, Some(Modal::Stickers(s)) if s.tab == 0),
            "opens on Recent"
        );
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Right);
        key(&mut a, KeyCode::Enter); // thumbs up
        assert_eq!(a.status, "reacted 👍 thumbs up");
        let line = a.reaction_line(&a.open_messages()[2].reactions, true).to_string();
        assert!(line.contains('👍') && !line.contains('✨'), "{line}");
    }

    #[tokio::test]
    async fn message_keys_and_pasted_files() {
        let mut a = app();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter); // open "Ua"
        key(&mut a, KeyCode::Up); // select our message m2
        key(&mut a, KeyCode::Char('e'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("reaction menu")
        };
        assert_eq!(m.items.len(), 7, "six reactions and the LINE emoji picker");
        key(&mut a, KeyCode::Char('2')); // ❤️
        assert!(a.modal.is_none());
        assert_eq!(
            a.open_messages()[2].reactions[0].kind,
            ReactionKind::Predefined(3)
        );
        key(&mut a, KeyCode::Char('x'));
        assert!(
            matches!(a.modal, Some(Modal::Confirm { .. })),
            "unsend asks first"
        );
        key(&mut a, KeyCode::Esc);

        let dir = std::env::temp_dir().join(format!("rune-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("photo.png");
        std::fs::write(&file, b"png").unwrap();
        a.focus = Focus::Input;
        a.on_paste(&format!("'{}'", file.display()));
        let Some(Modal::Attach(at)) = &a.modal else {
            panic!("attach dialog")
        };
        assert_eq!(at.chosen.as_ref().map(|c| c.path.clone()), Some(file.clone()));
        key(&mut a, KeyCode::Char('o'));
        let Some(Modal::Attach(at)) = &a.modal else {
            panic!("attach dialog remains open")
        };
        assert!(at.chosen.as_ref().is_some_and(|c| c.original));
        key(&mut a, KeyCode::Char('f'));
        let Some(Modal::Attach(at)) = &a.modal else {
            panic!("attach dialog remains open")
        };
        assert!(at.chosen.as_ref().is_some_and(|c| c.kind == MediaKind::File));
        key(&mut a, KeyCode::Char('f'));
        let Some(Modal::Attach(at)) = &a.modal else {
            panic!("attach dialog remains open")
        };
        assert!(at.chosen.as_ref().is_some_and(|c| c.kind == MediaKind::Image));
        a.modal = None;
        a.on_paste("just text");
        assert_eq!(a.input, "just text");
        std::fs::remove_dir_all(&dir).unwrap();

        // Unknown commands are not sent by accident; "//" escapes.
        a.input = "/nope".into();
        a.submit();
        assert!(a.status.contains("unknown command"));
        assert_eq!(a.input, "/nope");
    }

    #[tokio::test]
    async fn every_overlay_draws_at_any_size() {
        let mut a = app();
        a.open_chat("Ua".into());
        a.contacts.insert("Ub".into(), "Bob".into());
        a.names.insert("Ub".into(), "Bob".into());
        a.messages.get_mut("Ua").unwrap()[0].reactions = vec![MessageReaction {
            from: "Ub".into(),
            at: 0,
            kind: ReactionKind::Predefined(2),
        }];
        type Opener = Box<dyn Fn(&mut App)>;
        let overlays: Vec<Opener> = vec![
            Box::new(|a| a.chat_menu("Ua")),
            Box::new(|a| a.open_help(keymap::Ctx::ChatList)),
            Box::new(|a| a.compose_menu()),
            Box::new(|a| a.run_action(Action::Location)),
            Box::new(|a| a.run_action(Action::ShareContact)),
            Box::new(|a| a.run_action(Action::ConfirmLeave("Ua".into()))),
            Box::new(|a| a.run_action(Action::ReadBy("02".into()))),
            Box::new(|a| a.run_action(Action::Reactions("00".into()))),
            Box::new(|a| a.open_attach(None)),
            Box::new(|a| {
                a.store
                    .set(
                        "cache.sticker_packages",
                        r#"{"at": 99999999999999, "packages": []}"#,
                    )
                    .unwrap();
                a.open_stickers();
            }),
            Box::new(|a| a.main_menu()),
            Box::new(|a| a.open_settings_at(settings::Tab::Notifications)),
            Box::new(|a| a.open_settings_at(settings::Tab::Chats)),
            Box::new(|a| a.open_settings_at(settings::Tab::Account)),
            Box::new(|a| a.open_settings_at(settings::Tab::Plugins)),
            Box::new(|a| a.open_settings_at(settings::Tab::Storage)),
            Box::new(|a| {
                a.modal = Some(Modal::PluginView(plugin_ui::PluginView::new(plugins::View {
                    plugin: "p".into(),
                    id: "v".into(),
                    title: "A plugin view".into(),
                    lines: (0..50)
                        .map(|i| rune_plugin::proto::ViewLine::new(format!("line {i} สวัสดี 👋")))
                        .collect(),
                    selected: Some(40),
                    keys: vec![rune_plugin::proto::KeyHelp::new("a", "add")],
                })))
            }),
        ];
        for open in &overlays {
            for (w, h) in [(100, 30), (40, 12), (24, 8)] {
                open(&mut a);
                assert!(a.modal.is_some());
                let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
                term.draw(|f| a.draw(f)).unwrap();
                key(&mut a, KeyCode::Esc);
                a.modal = None;
            }
        }
    }

    /// The app with plugins from `dir`, the enabled ones started.
    fn plugin_app(dir: &std::path::Path) -> App {
        let mut a = app();
        a.plugins = plugins::Plugins::new(
            a.store.clone(),
            Some(dir.to_owned()),
            false,
            keymap::CORE_COMMANDS,
        );
        for job in a.plugins.scan() {
            let fx = a.plugins.loaded(job.run());
            a.apply_plugin_effects(fx);
        }
        a
    }

    fn enable_now(a: &mut App, id: &str) {
        let job = a.plugins.enable(id).unwrap();
        let fx = a.plugins.loaded(job.run());
        a.apply_plugin_effects(fx);
    }

    fn submit(a: &mut App, text: &str) {
        type_text(a, text);
        key(a, KeyCode::Enter);
    }

    #[tokio::test]
    async fn without_plugins_nothing_changes() {
        let mut a = app();
        assert!(a.plugins.entries.is_empty());
        a.open_chat("Ua".into());
        a.focus = Focus::Input;
        submit(&mut a, "/snip");
        assert!(a.status.contains("unknown command /snip"), "{}", a.status);
        a.input.take();
        key_with_modifiers(&mut a, KeyCode::Char('s'), KeyModifiers::ALT);
        assert!(a.modal.is_none());
        open_plugins(&mut a);
        assert!(screen(&mut a).contains("No plugins installed"));
    }

    /// Ctrl-x › Plugins, by name.
    fn open_plugins(a: &mut App) {
        key_with_modifiers(a, KeyCode::Char('x'), KeyModifiers::CONTROL);
        let Some(Modal::Menu(m)) = &mut a.modal else {
            panic!("main menu")
        };
        m.sel = m.items.iter().position(|(l, _)| l.contains("Plugins")).unwrap();
        key(a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Settings(_))), "Settings › Plugins");
    }

    #[tokio::test]
    async fn the_sample_plugin_works_through_the_ui() {
        let dir = plugins::tests::TempDir::new("app");
        plugins::tests::snippets(&dir.0);
        let mut a = plugin_app(&dir.0);
        enable_now(&mut a, "snippets");
        a.open_chat("Ua".into());
        a.focus = Focus::Input;

        // A command opens the plugin's form; submitting it opens its view.
        submit(&mut a, "/snip add hi");
        assert!(matches!(&a.modal, Some(Modal::Form(f)) if f.fields[0].value.as_str() == "hi"));
        key(&mut a, KeyCode::Enter);
        type_text(&mut a, "Hello there");
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::PluginView(_))));
        let s = screen(&mut a);
        assert!(s.contains("Snippets (1)") && s.contains("Hello there"), "{s}");
        assert!(
            s.contains("Enter insert"),
            "the view's keys are on its frame: {s}"
        );

        // Enter in the view types the snippet into the message line.
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.input.as_str(), "Hello there");
        a.input.take();

        // Its key opens the view while typing; `?` lists the view's keys.
        key_with_modifiers(&mut a, KeyCode::Char('s'), KeyModifiers::ALT);
        assert!(matches!(&a.modal, Some(Modal::PluginView(_))));
        key(&mut a, KeyCode::Char('?'));
        assert!(screen(&mut a).contains("insert"));
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::PluginView(_))));
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        // The message line's help lists the plugin's command and key.
        let help = a.help_lines(keymap::Ctx::Compose, None);
        assert!(
            help.iter()
                .any(|(k, h)| k.starts_with("/snip") && h.contains("(Snippets)")),
            "{help:?}"
        );
        assert!(help.iter().any(|(k, _)| k == "Alt-s"), "{help:?}");
        assert!(
            a.help_lines(keymap::Ctx::Menu, None)
                .iter()
                .all(|(k, _)| k != "Alt-s")
        );

        // Settings › Plugins lists it on; its page says what it may do and
        // has its settings; Esc goes back to the list, then closes.
        open_plugins(&mut a);
        assert!(screen(&mut a).contains("Snippets"));
        key(&mut a, KeyCode::Enter);
        let page = screen(&mut a);
        assert!(
            page.contains("type into your message line") && page.contains("Preview length"),
            "{page}"
        );
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Plugins));
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        // Off: its command and key are gone, the rest of the app is not.
        a.plugins.disable("snippets");
        submit(&mut a, "/snip hi");
        assert!(a.status.contains("which is off"), "{}", a.status);
        a.input.take();
        submit(&mut a, "/plugins");
        assert!(
            a.status.contains("unknown command /plugins") && a.modal.is_none(),
            "{}",
            a.status
        );
    }

    #[tokio::test]
    async fn enabling_from_the_manager_asks_first() {
        let dir = plugins::tests::TempDir::new("enable");
        plugins::tests::snippets(&dir.0);
        let mut a = plugin_app(&dir.0);
        a.open_plugin_page("snippets".into());
        // The first row to act on is "Enabled".
        key(&mut a, KeyCode::Enter);
        let Some(Modal::Confirm(c)) = &a.modal else {
            panic!("expected a confirmation")
        };
        assert!(
            c.text.contains("type into your message line") && c.text.contains("trust"),
            "{}",
            c.text
        );
        key(&mut a, KeyCode::Char('y'));
        assert_eq!(a.plugins.get("snippets").unwrap().state, plugins::State::Loading);
        assert!(matches!(a.modal, Some(Modal::Plugins(_))));
    }

    #[tokio::test]
    async fn a_crashing_plugin_is_stopped_and_the_app_goes_on() {
        let dir = plugins::tests::TempDir::new("crash");
        let wasm = plugins::tests::wat_plugin(plugins::tests::QUIET, 1, "unreachable");
        plugins::tests::install(
            &dir.0,
            "crashy",
            r#","commands":[{"name":"go","help":"x"}]"#,
            &wasm,
        );
        let mut a = plugin_app(&dir.0);
        enable_now(&mut a, "crashy");
        a.open_chat("Ua".into());
        a.focus = Focus::Input;
        submit(&mut a, "/go");
        assert!(a.status.contains("plugin crashy stopped"), "{}", a.status);
        assert!(matches!(
            a.plugins.get("crashy").unwrap().state,
            plugins::State::Failed(_)
        ));
        open_plugins(&mut a);
        assert!(screen(&mut a).contains("⚠ failed"), "{}", screen(&mut a));
        key(&mut a, KeyCode::Esc);
        type_text(&mut a, "still typing");
        assert_eq!(a.input.as_str(), "still typing");
    }

    /// Select the row of the open plugin page whose label has `label`.
    fn select_page_row(a: &mut App, id: &str, label: &str) {
        let i = a
            .plugin_row_for_test(id, label)
            .unwrap_or_else(|| panic!("no {label:?} on the page:\n{}", screen(a)));
        let Some(Modal::Plugins(m)) = &mut a.modal else {
            panic!("a plugin page")
        };
        m.select(i);
    }

    #[tokio::test]
    async fn plugins_install_update_and_uninstall_from_git_through_settings() {
        use plugins::install;
        use plugins::tests::{commit, have_git, manifest, repo_with, says};
        if !have_git() {
            return;
        }
        let work = plugins::tests::TempDir::new("app-git-src");
        let dir = plugins::tests::TempDir::new("app-git");
        let repo = repo_with(
            &work.0,
            &[
                ("plugin.json", &manifest("gitty", "1.0")),
                ("plugin.wasm", &says("v1")),
            ],
        );
        let mut a = plugin_app(&dir.0);
        open_plugins(&mut a);
        let rows = a.settings_rows_for_test(settings::Tab::Plugins);
        let at = rows
            .iter()
            .position(|r| r.contains("Install from a Git repository"))
            .unwrap();
        if let Some(Modal::Settings(s)) = &mut a.modal {
            s.sel = at;
        }
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Form(f)) if f.kind == modal::FormKind::PluginInstall));

        // Addresses are checked in the form.
        type_text(&mut a, "http://example.com/r");
        for _ in 0..3 {
            key(&mut a, KeyCode::Enter);
        }
        let Some(Modal::Form(f)) = &a.modal else {
            panic!("the form stays open")
        };
        assert!(
            f.error.as_deref().is_some_and(|e| e.contains("unencrypted")),
            "{:?}",
            f.error
        );
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Plugins));

        // One install at a time.
        a.install_plugin(install::Request::new("/nonexistent/rune-test-repo", "", "").unwrap());
        assert!(
            a.status.starts_with("installing from /nonexistent"),
            "{}",
            a.status
        );
        assert!(screen(&mut a).contains("installing…"));
        a.install_plugin(install::Request::new(&repo.display().to_string(), "", "").unwrap());
        assert_eq!(a.status, "wait: installing…");
        a.on_plugin_install(plugin_ui::InstallEvent::Installed(Err(
            "could not clone: nope".into()
        )));
        assert_eq!(a.status, "install failed: could not clone: nope");

        // A finished install (run here instead of on a blocking task): listed,
        // selected, off.
        let req = install::Request::new(&repo.display().to_string(), "", "").unwrap();
        let o = install::install(&dir.0, &req, a.plugins.limits(), &|_| {}).unwrap();
        a.on_plugin_install(plugin_ui::InstallEvent::Installed(Ok(o)));
        assert!(a.status.starts_with("installed gitty 1.0"), "{}", a.status);
        assert_eq!(a.plugins.get("gitty").unwrap().state, plugins::State::Disabled);
        key(&mut a, KeyCode::Enter);
        let page = screen(&mut a);
        assert!(
            page.contains("Source") && page.contains("Check for a new version") && page.contains("Uninstall"),
            "{page}"
        );

        // An update while it runs waits for approval on its page.
        enable_now(&mut a, "gitty");
        commit(
            &repo,
            &[
                ("plugin.json", &manifest("gitty", "1.1")),
                ("plugin.wasm", &says("v2")),
            ],
            "v2",
        );
        let o = install::update(&dir.0, "gitty", None, a.plugins.limits(), &|_| {}).unwrap();
        a.on_plugin_install(plugin_ui::InstallEvent::Updated("gitty".into(), Ok(o)));
        assert!(
            a.status
                .contains("updated gitty 1.0 → 1.1; approve the new version"),
            "{}",
            a.status
        );
        assert!(screen(&mut a).contains("Version 1.1 is on disk"));
        select_page_row(&mut a, "gitty", "Load the new version");
        key(&mut a, KeyCode::Enter);
        let Some(Modal::Confirm(c)) = &a.modal else {
            panic!("loading a changed module asks first")
        };
        assert!(c.text.contains("Load gitty 1.1"), "{}", c.text);
        key(&mut a, KeyCode::Char('y'));
        assert!(
            a.status.starts_with("loading the new version of gitty"),
            "{}",
            a.status
        );

        // Turning on automatic loading asks first too.
        select_page_row(&mut a, "gitty", "Load changed versions");
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Confirm(c)) if c.text.contains("without asking")));
        key(&mut a, KeyCode::Char('y'));
        assert!(a.plugins.autoload("gitty"));

        // Uninstall: asked, files gone, back to the list.
        select_page_row(&mut a, "gitty", "Uninstall");
        key(&mut a, KeyCode::Enter);
        assert!(matches!(&a.modal, Some(Modal::Confirm(c)) if c.text.contains("Uninstall gitty")));
        key(&mut a, KeyCode::Char('y'));
        assert!(!dir.0.join("gitty").exists());
        assert!(a.status.starts_with("uninstalled gitty"), "{}", a.status);
    }

    #[tokio::test]
    async fn plugin_versions_are_chosen_pinned_and_checked_from_settings() {
        use plugins::install;
        use plugins::tests::{have_git, installed_version, released};
        use plugins::versions::Track;
        if !have_git() {
            return;
        }
        let work = plugins::tests::TempDir::new("app-ver-src");
        let dir = plugins::tests::TempDir::new("app-ver");
        let repo = released(&work.0);
        let mut a = plugin_app(&dir.0);
        let req = install::Request::new(&repo.display().to_string(), "^1", "").unwrap();
        let o = install::install(&dir.0, &req, a.plugins.limits(), &|_| {}).unwrap();
        a.on_plugin_install(plugin_ui::InstallEvent::Installed(Ok(o)));
        a.open_plugin_page("rel".into());
        let page = screen(&mut a);
        assert!(
            page.contains("newest ^1 release") && page.contains("v1.1.0"),
            "{page}"
        );

        // Updates: the version menu (looked up, here synchronously).
        select_page_row(&mut a, "rel", "Updates");
        let v = install::versions(&dir.0, "rel").unwrap();
        a.on_plugin_install(plugin_ui::InstallEvent::Versions("rel".into(), Ok(v)));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("the version menu over its page")
        };
        let labels: Vec<&str> = m.items.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels[0].starts_with("Newest release\tnow v2.0.0"), "{labels:?}");
        assert!(
            labels[1].starts_with("Newest compatible (^1) release  ✓"),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.starts_with("Newest commit on main")),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.starts_with("Pin to v1.1.0\tinstalled")),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.starts_with("Pin to v1.0.0\tolder")),
            "{labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("Pin to v2.0.0-beta\tpre-release")),
            "{labels:?}"
        );
        // Pinning the installed version: right away, no network.
        choose(&mut a, "Pin to the installed version");
        assert!(a.status.contains("rel is pinned to v1.1.0"), "{}", a.status);
        assert!(matches!(&a.modal, Some(Modal::Plugins(_))));
        assert!(screen(&mut a).contains("pinned to v1.1.0"));

        // A versions answer arriving after the page closed opens nothing.
        a.modal = None;
        let v = install::versions(&dir.0, "rel").unwrap();
        a.on_plugin_install(plugin_ui::InstallEvent::Versions("rel".into(), Ok(v)));
        assert!(
            a.modal.is_none() && a.status.contains("choose on its page"),
            "{}",
            a.status
        );

        // Check all: pinned ones report what they leave out.
        a.open_settings_at(settings::Tab::Plugins);
        assert!(
            a.settings_rows_for_test(settings::Tab::Plugins)
                .iter()
                .any(|r| r == "Check all for new versions")
        );
        let res = install::update(&dir.0, "rel", None, a.plugins.limits(), &|_| {});
        a.on_plugin_install(plugin_ui::InstallEvent::CheckedAll(vec![("rel".into(), res)]));
        assert_eq!(a.status, "1 up to date · available but not followed: rel v2.0.0");
        a.open_plugin_page("rel".into());
        assert!(screen(&mut a).contains("v2.0.0 is available"));

        // Switching to an older version installs it (its module then waits
        // for approval like any changed module).
        let res = install::update(
            &dir.0,
            "rel",
            Some(Track::Pinned("v1.0.0".into())),
            a.plugins.limits(),
            &|_| {},
        );
        a.on_plugin_install(plugin_ui::InstallEvent::Updated("rel".into(), res));
        assert!(a.status.starts_with("switched rel 1.1.0 → 1.0.0"), "{}", a.status);
        assert_eq!(installed_version(&dir.0, "rel"), "1.0.0");
    }

    #[tokio::test]
    async fn changed_plugin_files_reload_while_the_app_runs() {
        let dir = plugins::tests::TempDir::new("app-hot");
        let d = plugins::tests::install(&dir.0, "hot", "", &plugins::tests::says("v1"));
        let mut a = plugin_app(&dir.0);
        enable_now(&mut a, "hot");
        a.open_plugin_page("hot".into());
        // Rebuilt: the page offers the new version; removed: back to the list.
        std::fs::write(d.join("plugin.wasm"), plugins::tests::says("v2")).unwrap();
        a.handle_event(AppEvent::PluginFiles(vec!["hot".into()]));
        assert!(a.status.contains("approve the new version"), "{}", a.status);
        assert!(screen(&mut a).contains("Load the new version"));
        std::fs::remove_dir_all(&d).unwrap();
        a.handle_event(AppEvent::PluginFiles(vec!["hot".into()]));
        assert!(a.status.contains("removed from disk; stopped"), "{}", a.status);
        assert!(matches!(
            &a.modal,
            Some(Modal::Plugins(_)) | Some(Modal::Settings(_))
        ));
        // A new one appears in Settings › Plugins.
        plugins::tests::install(&dir.0, "newbie", "", &plugins::tests::says("hi"));
        a.handle_event(AppEvent::PluginFiles(vec!["newbie".into()]));
        assert!(a.status.starts_with("found plugin newbie"), "{}", a.status);
        a.open_settings_at(settings::Tab::Plugins);
        assert!(screen(&mut a).contains("newbie"));
    }

    #[test]
    fn motion_media_goes_to_mpv() {
        assert!(is_motion_file("clip.MP4") && is_motion_file("voice.m4a"));
        assert!(
            !is_motion_file("a.gif") && !is_motion_file("s.apng"),
            "animations play in the viewer"
        );
        assert!(!is_motion_file("photo.jpg") && !is_motion_file("notes.pdf") && !is_motion_file("README"));
        let args = |p: &str| -> Vec<String> {
            mpv_args(std::path::Path::new(p))
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let mp4 = args("/d/line-1.mp4");
        assert!(
            mp4.contains(&"--no-terminal".to_owned()),
            "mpv must leave rune's terminal alone"
        );
        assert_eq!(
            mp4[mp4.len() - 2..],
            ["--".to_owned(), "/d/line-1.mp4".to_owned()]
        );
        // A file name starting with "-" is never read as an option.
        assert_eq!(args("-x.mp4").last().unwrap(), "-x.mp4");

        let msg = |v: serde_json::Value| -> UiMsg {
            to_ui(&serde_json::from_value::<Message>(v).unwrap(), None).unwrap()
        };
        assert!(msg(serde_json::json!({"id": "1", "contentType": 2})).plays());
        assert!(msg(serde_json::json!({"id": "1", "contentType": 3})).plays());
        assert!(
            !msg(serde_json::json!({"id": "1", "contentType": 1})).plays(),
            "GIFs play from the viewer"
        );
        assert!(
            !msg(serde_json::json!({"id": "1", "contentType": 7, "contentMetadata": {"STKID": "9", "STKOPT": "A"}})).plays(),
            "animated stickers play in the viewer"
        );
        assert!(
            !msg(serde_json::json!({"id": "1", "contentType": 7, "contentMetadata": {"STKID": "9"}})).plays()
        );
        assert!(
            msg(serde_json::json!({"id": "1", "contentType": 14, "contentMetadata": {"FILE_NAME": "a.mkv"}}))
                .plays()
        );
        assert!(
            !msg(
                serde_json::json!({"id": "1", "contentType": 14, "contentMetadata": {"FILE_NAME": "a.zip"}})
            )
            .plays()
        );
        let mut unsent = msg(serde_json::json!({"id": "1", "contentType": 2}));
        unsent.unsent = true;
        assert!(!unsent.plays());
    }
    #[test]
    fn fast_chat_list_keys_stay_on_the_list() {
        let mut a = app();
        a.list.select(Some(0));
        // Several ↓ before the next frame used to leave the selection past
        // the end, so Enter opened nothing.
        for _ in 0..5 {
            key(&mut a, KeyCode::Down);
        }
        assert_eq!(a.list.selected(), Some(1));
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.open.as_deref(), a.focus), (Some("Ub"), Focus::Input));
        a.focus = Focus::Chats;
        key(&mut a, KeyCode::PageUp);
        assert_eq!(a.list.selected(), Some(0));
        key(&mut a, KeyCode::PageDown);
        assert_eq!(a.list.selected(), Some(1));
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_chat_and_message() {
        let mut a = app();
        a.focus = Focus::Chats;
        key(&mut a, KeyCode::End);
        assert_eq!(a.list.selected(), Some(1));
        key(&mut a, KeyCode::Home);
        assert_eq!(a.list.selected(), Some(0));
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.sel = Some(1);
        key(&mut a, KeyCode::Home);
        assert_eq!((a.sel, a.focus), (Some(0), Focus::Messages));
        key(&mut a, KeyCode::End);
        assert_eq!((a.sel, a.focus), (Some(2), Focus::Messages));
    }

    #[test]
    fn the_message_line_deletes_by_word() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Input;
        type_text(&mut a, "hello big world");
        key_with_modifiers(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(a.input.as_str(), "hello big ");
        key_with_modifiers(&mut a, KeyCode::Left, KeyModifiers::CONTROL);
        key_with_modifiers(&mut a, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(a.input.as_str(), "hello ");
        key_with_modifiers(&mut a, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(a.input.is_empty());
        assert_eq!(a.focus, Focus::Input);
    }

    #[test]
    fn the_status_line_hides_an_empty_status_and_marks_failures() {
        let mut a = app();
        a.status.clear();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        // The status line starts straight with the help key, no empty chip.
        assert_eq!(term.backend().buffer()[(0, 29)].symbol(), "?");
        a.status = "could not unsend: gone".into();
        term.draw(|f| a.draw(f)).unwrap();
        assert_eq!(term.backend().buffer()[(1, 29)].bg, theme::t().danger);
        a.status = "saved".into();
        term.draw(|f| a.draw(f)).unwrap();
        assert_eq!(term.backend().buffer()[(1, 29)].bg, theme::t().accent);
    }

    #[test]
    fn scrolled_up_conversations_say_newer_messages_are_below() {
        let mut a = app();
        a.open = Some("Ua".into());
        let long: Vec<UiMsg> = (0..40)
            .map(|j| UiMsg {
                id: format!("{j}"),
                text: format!("message {j}"),
                ..a.messages["Ua"][0].clone()
            })
            .collect();
        a.messages.insert("Ua".into(), long);
        assert!(!screen(&mut a).contains("newer messages below"));
        a.scroll = 10;
        assert!(screen(&mut a).contains("newer messages below · PgDn"));
    }

    #[tokio::test]
    async fn sending_scrolls_back_to_the_newest_message() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Input;
        a.scroll = 10;
        type_text(&mut a, "hi");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn empty_and_welcome_screens_say_what_to_do() {
        let mut a = app();
        let s = screen(&mut a);
        assert!(s.contains("Pick a chat on the left"), "{s}");
        assert!(s.contains("search chats and friends"), "{s}");
        a.open = Some("Unew".into());
        assert!(screen(&mut a).contains("no messages yet"));
    }

    #[test]
    fn chat_search_counts_its_matches() {
        let mut a = app();
        a.focus = Focus::Chats;
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "Ub");
        assert!(screen(&mut a).contains("Chats · 1 match "));
    }

    #[test]
    fn the_message_line_help_lists_its_commands() {
        let lines = keymap::help_lines(keymap::Ctx::Compose);
        assert!(lines.iter().any(|(k, _)| k == "/attach [PATH]"));
        // The messages have the `/` key (search), not the `/commands`.
        let messages = keymap::help_lines(keymap::Ctx::Messages);
        assert!(
            !messages
                .iter()
                .any(|(k, _)| is_command_word(k.split(' ').next().unwrap_or("")))
        );
        assert!(messages.contains(&("/".into(), "search this chat's messages")));
    }

    #[test]
    fn the_message_line_edits_at_a_cursor_and_keeps_the_focus() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Input;
        type_text(&mut a, "helo");
        key(&mut a, KeyCode::Left);
        type_text(&mut a, "l");
        key(&mut a, KeyCode::Home);
        type_text(&mut a, "> ");
        key(&mut a, KeyCode::Delete);
        assert_eq!(a.input, "> ello");
        assert_eq!(
            a.focus,
            Focus::Input,
            "Home and Delete at the start keep the focus"
        );
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.input, "> ell");
        // A paste lands at the cursor too, line breaks and all.
        key(&mut a, KeyCode::Home);
        a.on_paste("a\nb ");
        assert_eq!(a.input, "a\nb > ell");
        let screen = screen(&mut a);
        assert!(screen.contains("Message · 2 lines"), "{screen}");
        assert!(screen.contains("│a ") && screen.contains("│b > ell"), "{screen}");
        // Home goes to the start of the cursor's line; Backspace there joins lines.
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.input, "ab > ell");
        assert_eq!(a.focus, Focus::Input);
        // Only ← at the start moves to the chat list.
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.focus, Focus::Input);
        key(&mut a, KeyCode::Left);
        assert_eq!(a.focus, Focus::Chats);
    }

    #[tokio::test]
    async fn the_message_line_takes_several_lines_and_sends_them_as_one() {
        let mut a = app();
        a.enter_chat("Ua".into());
        type_text(&mut a, "hi");
        key_with_modifiers(&mut a, KeyCode::Enter, KeyModifiers::SHIFT);
        type_text(&mut a, "there");
        key_with_modifiers(&mut a, KeyCode::Enter, KeyModifiers::ALT);
        key_with_modifiers(&mut a, KeyCode::Char('j'), KeyModifiers::CONTROL);
        type_text(&mut a, "/mute");
        assert_eq!(a.input, "hi\nthere\n\n/mute");
        let shown = screen(&mut a);
        assert!(shown.contains("Message · 4 lines"), "{shown}");
        assert!(shown.contains("│there "), "the composer grew: {shown}");
        // ↑ walks up the lines, and only above the first selects messages.
        for _ in 0..3 {
            key(&mut a, KeyCode::Up);
            assert_eq!((a.focus, a.sel), (Focus::Input, None));
        }
        key(&mut a, KeyCode::Down);
        type_text(&mut a, "!");
        assert_eq!(a.input, "hi\nthere!\n\n/mute");
        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Up);
        assert_eq!(a.focus, Focus::Messages);
        key(&mut a, KeyCode::Esc);
        // Several lines are a message even when one looks like a command.
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.input.as_str(), a.status.as_str()), ("", "sending…"));
    }

    #[tokio::test]
    async fn pasted_line_breaks_never_send() {
        let mut a = app();
        a.enter_chat("Ua".into());
        // A terminal without bracketed paste types the paste out, Enters and all.
        a.pasting = true;
        type_text(&mut a, "one");
        key(&mut a, KeyCode::Enter);
        type_text(&mut a, "two");
        key(&mut a, KeyCode::Enter);
        a.pasting = false;
        assert_eq!(a.input, "one\ntwo\n");
        assert!(a.status.contains("Enter sends"), "{}", a.status);
        // A bracketed paste ending in a line break does not send either.
        a.input.clear();
        a.on_paste("three\r\nfour\r\n");
        assert_eq!(a.input, "three\nfour");
        assert_eq!(a.status, "pasted 2 lines — Enter sends them as one message");
        // The person's own Enter sends.
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.input.as_str(), a.status.as_str()), ("", "sending…"));
    }

    #[test]
    fn paste_bursts_are_told_from_typing() {
        let press = |c| {
            Some(Ok(TermEvent::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            ))))
        };
        let typed: Vec<_> = "ok".chars().map(press).collect();
        assert!(!is_paste_burst(&typed, false), "a few keys are typing");
        let pasted: Vec<_> = "a pasted line".chars().map(press).collect();
        assert!(is_paste_burst(&pasted, false));
        assert!(is_paste_burst(&typed, true), "the rest of a long paste");
        assert!(!is_paste_burst(&[Some(Ok(TermEvent::FocusGained))], true));
    }

    #[test]
    fn drafts_stay_with_their_chat() {
        let mut a = app();
        a.enter_chat("Ua".into());
        type_text(&mut a, "for A");
        a.replying = Some("01".into());
        a.enter_chat("Ub".into());
        assert_eq!((a.input.as_str(), a.replying.as_deref()), ("", None));
        let shown = screen(&mut a);
        assert!(
            shown.contains("✎ for A"),
            "the chat list shows the draft: {shown}"
        );
        type_text(&mut a, "for B");
        // Enter in another chat never sends A's draft there.
        a.enter_chat("Ua".into());
        assert_eq!((a.input.as_str(), a.replying.as_deref()), ("for A", Some("01")));
        assert_eq!(
            a.store.get("cache.draft.Ub").unwrap().as_deref(),
            Some("for B"),
            "kept across restarts"
        );
        // At exit the open chat's draft is kept too; an emptied one is forgotten.
        a.save_open_draft();
        let (tx, _rx) = mpsc::unbounded_channel();
        let b = App::new(a.client.clone(), "Ume".into(), tx, a.store.clone());
        assert_eq!(b.drafts.get("Ua").map(|d| d.input.as_str()), Some("for A"));
        a.input.clear();
        a.enter_chat("Ub".into());
        assert_eq!(a.store.get("cache.draft.Ua").unwrap(), None);
        assert!(!a.drafts.contains_key("Ua"));
        // Removing a chat drops its draft.
        a.enter_chat("Ua".into());
        a.remove_chat("Ub");
        assert!(!a.drafts.contains_key("Ub"));
        assert_eq!(a.store.get("cache.draft.Ub").unwrap(), None);
    }

    #[test]
    fn reply_and_quote_from_the_viewer_menu_close_the_viewer() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.messages.get_mut("Ua").unwrap()[2].reply_to = Some("00".into());
        a.sel = Some(2);
        a.viewer = Some("02".into());
        a.run_action(Action::Reply("02".into()));
        assert_eq!((a.viewer.as_deref(), a.focus), (None, Focus::Input));
        type_text(&mut a, "ok");
        assert_eq!(a.input, "ok", "typing reaches the message line");

        a.sel = Some(2);
        a.viewer = Some("02".into());
        a.run_action(Action::GotoQuoted);
        assert_eq!((a.viewer.as_deref(), a.sel), (None, Some(0)));
    }

    /// Regression: the notification icon writer created the view cache with
    /// the default mode (0755), and the private mode was applied only when
    /// the directory did not exist yet, so decrypted media copies could land
    /// in a directory everyone can list and read.
    #[cfg(unix)]
    #[test]
    fn the_view_cache_is_made_private_even_if_it_already_exists() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("rune-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("rune/view");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private_dir(&dir).unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        let fresh = base.join("new/view");
        make_private_dir(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        // A file where the directory should be is an error, not written through.
        let file = base.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(make_private_dir(&file).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_termination_signal_quits_like_the_quit_key() {
        let mut a = app();
        a.on_event(AppEvent::Terminate);
        assert!(a.quit, "run then saves the draft and shuts plugins down");
    }

    #[test]
    fn file_types_that_run_programs_are_recognised_by_their_name_on_disk() {
        for risky in [
            "setup.desktop",
            "Install.SH",
            "invoice.pdf.exe",
            "tool.jar",
            "a.AppImage",
            "x.desktop ",  // trailing space is trimmed when written
            "x.desktop..", // so are trailing dots, for this check
            "..hidden.sh",
        ] {
            assert!(runs_programs(risky), "{risky}");
        }
        for safe in [
            "photo.jpg",
            "notes.txt",
            "report.pdf",
            "song.mp3",
            "README",
            "desktop",
            ".sh",
        ] {
            assert!(!runs_programs(safe), "{safe}");
        }
    }

    #[test]
    fn opening_a_file_that_can_run_programs_is_asked_first() {
        let mut a = app();
        a.remember_full(
            "01".into(),
            FullMedia {
                data: b"[Desktop Entry]".to_vec(),
                file_name: "photos.desktop".into(),
                picture: None,
                note: None,
            },
        );
        a.finish("01", After::Open);
        let Some(Modal::Confirm(c)) = &a.modal else {
            panic!("expected a confirmation, got status {:?}", a.status);
        };
        assert!(!c.yes, "starts on Cancel");
        assert_eq!(c.action, Action::OpenAnyway("01".into()));
        assert!(c.text.contains("photos.desktop") && c.text.contains("run programs"));
        assert!(!a.status.starts_with("opening"), "nothing was opened");
        // Agreeing after the download was dropped says so instead of doing nothing.
        a.modal = None;
        a.run_action(Action::OpenAnyway("gone".into()));
        assert!(a.status.contains("no longer kept"), "{}", a.status);
    }

    #[test]
    fn unsent_media_is_dropped_and_leaves_the_viewer() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.sel = Some(0);
        a.viewer = Some("00".into());
        a.remember_full(
            "00".into(),
            FullMedia {
                data: vec![1, 2, 3],
                file_name: "a.jpg".into(),
                picture: None,
                note: None,
            },
        );
        a.qr.insert("00".into(), vec!["https://x.y".into()]);
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 65, "param1": "Ua", "param2": "00", "param3": "0"
        })));
        assert!(a.viewer.is_none());
        assert!(!a.full.contains_key("00") && a.full_lru.is_empty() && !a.qr.contains_key("00"));
        assert!(a.open_messages()[0].unsent);
    }

    #[test]
    fn downloaded_originals_are_dropped_oldest_first_but_not_the_one_on_screen() {
        let mut a = app();
        let media = |n: usize| FullMedia {
            data: vec![0; n],
            file_name: "x".into(),
            picture: None,
            note: None,
        };
        for id in ["a", "b", "c"] {
            a.remember_full(id.into(), media(10));
        }
        a.viewer = Some("a".into());
        a.trim_full(20);
        assert_eq!(a.full_lru, ["a", "c"], "the viewed one and the newest stay");
        assert!(!a.full.contains_key("b"));
        a.viewer = None;
        a.trim_full(5);
        assert_eq!(a.full_lru, ["c"], "the newest always stays");
    }

    #[tokio::test]
    async fn destructive_confirmations_start_on_cancel() {
        let mut a = app();
        a.chats.push(ChatEntry {
            mid: "Cg".into(),
            ..Default::default()
        });
        a.run_action(Action::ConfirmLeave("Cg".into()));
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none() && a.status == "cancelled");
        let screen_text = {
            a.run_action(Action::ConfirmLeave("Cg".into()));
            screen(&mut a)
        };
        assert!(
            screen_text.contains("Leave (y)") && screen_text.contains("Cancel (n)"),
            "{screen_text}"
        );
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Enter);
        assert!(a.status.starts_with("leaving"), "{}", a.status);
        // `y` answers directly; Tab moves between the buttons.
        a.run_action(Action::ConfirmLeave("Cg".into()));
        key(&mut a, KeyCode::Tab);
        assert!(matches!(&a.modal, Some(Modal::Confirm(c)) if c.yes));
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Char('y'));
        assert!(a.modal.is_none() && a.status.starts_with("leaving"));
        // The buttons can be clicked.
        a.run_action(Action::ConfirmLeave("Cg".into()));
        screen(&mut a);
        let (r, _) = a
            .hit
            .confirm_buttons
            .iter()
            .copied()
            .find(|(_, yes)| !yes)
            .unwrap();
        click(&mut a, r.x, r.y);
        assert!(a.modal.is_none() && a.status == "cancelled");
    }

    #[test]
    fn pasting_while_searching_filters_the_chat_list() {
        let mut a = app();
        a.names.insert("Ua".into(), "Alpha".into());
        a.names.insert("Ub".into(), "Beta".into());
        a.open = Some("Ua".into());
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        a.on_paste("bet\n");
        assert_eq!(a.chat_query, "bet");
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"));
        assert!(a.input.as_str().is_empty(), "nothing went to the message line");
    }

    #[test]
    fn the_status_line_fits_any_width() {
        let hint = "? help · Enter open · Tab focus · / search · Ctrl-q quit";
        for status in ["", "sent", &"a very long failure message ".repeat(6)] {
            for w in 0..120 {
                let (message, keys) = status_line(status, hint, w);
                let message_w = message.as_ref().map_or(0, |m| m.width() + 1);
                assert!(message_w + keys.width() <= w.max(message_w), "{status:?} at {w}");
                if w >= 40 {
                    // A long message gives way to the help key.
                    assert!(keys.starts_with("? help"), "{status:?} at {w}: {keys:?}");
                    assert!(message_w + keys.width() <= w, "{status:?} at {w}");
                }
            }
        }
    }

    #[test]
    fn hints_only_offer_what_the_selection_allows() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.sel = Some(0);
        let text_hint = a.hint();
        assert!(
            text_hint.contains("r reply") && !text_hint.contains("s save"),
            "{text_hint}"
        );
        a.messages.get_mut("Ua").unwrap()[0].raw =
            Some(serde_json::from_value(serde_json::json!({"id": "00", "contentType": 14})).unwrap());
        assert!(a.hint().contains("s save"));
        // No chat open: no chords for it.
        let mut a = app();
        assert!(!a.hint().contains("Ctrl-o"));
        a.list.select(Some(0));
        assert!(a.hint().contains("Enter/→ open"));
        // An overlay lists its keys on its frame, not in the status line too.
        a.chat_menu("Ua");
        let screen = screen(&mut a);
        assert_eq!(screen.matches("Esc close").count(), 1, "{screen}");
        assert_eq!(screen.matches("? help").count(), 1, "{screen}");
    }

    #[test]
    fn help_is_titled_by_context_and_lists_numbered_items_once() {
        let mut a = app();
        a.chat_menu("Ua");
        key(&mut a, KeyCode::Char('?'));
        let screen = screen(&mut a);
        assert!(screen.contains("Menu shortcuts"), "{screen}");
        assert!(screen.contains("1…9"), "{screen}");
    }

    #[tokio::test]
    async fn a_profiles_chat_button_opens_the_chat_for_typing() {
        let mut a = app();
        a.names.insert("Ub".into(), "Bob".into());
        a.open_profile("Ub".into());
        assert_eq!(a.key_context(), keymap::Ctx::Profile);
        key(&mut a, KeyCode::Right);
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Enter);
        assert!(a.profile.is_empty());
        assert_eq!((a.open.as_deref(), a.focus), (Some("Ub"), Focus::Input));
        assert_eq!(a.selected_mid().as_deref(), Some("Ub"), "the list follows");
    }

    #[test]
    fn leaving_the_open_chat_clears_what_belonged_to_it() {
        let mut a = app();
        a.open = Some("Ua".into());
        a.focus = Focus::Messages;
        a.sel = Some(0);
        a.viewer = Some("00".into());
        a.replying = Some("01".into());
        a.typing
            .insert("Ua".into(), ("Ub".into(), std::time::Instant::now()));
        a.no_older.insert("Ua".into());
        a.on_event(AppEvent::Left("Ua".into(), Ok(())));
        assert!(a.open.is_none() && a.viewer.is_none() && a.replying.is_none());
        assert!(!a.typing.contains_key("Ua") && !a.no_older.contains("Ua"));
        assert_eq!(a.focus, Focus::Chats);
    }

    // -- shortcuts ---------------------------------------------------------------------

    /// Settings › Shortcuts › `section`, with the binding of `cmd` selected.
    fn shortcut_page(a: &mut App, section: &str, cmd: keymap::Cmd) {
        a.open_settings_at(settings::Tab::Shortcuts);
        let rows = a.settings_rows_for_test(settings::Tab::Shortcuts);
        let Some(Modal::Settings(s)) = &mut a.modal else {
            panic!("settings")
        };
        s.sel = rows.iter().position(|r| r == section).expect(section);
        key(a, KeyCode::Enter);
        let Some(Modal::Shortcuts(p)) = &mut a.modal else {
            panic!("the {section} shortcuts, not {:?}", a.key_context())
        };
        assert_eq!(keymap::SECTIONS[p.section].title, section);
        p.sel = keymap::SECTIONS[p.section]
            .entries()
            .iter()
            .position(|(_, b)| b.cmd == cmd)
            .expect("listed");
    }

    fn shortcut_state(a: &App) -> shortcuts::State {
        match &a.modal {
            Some(Modal::Shortcuts(p)) => p.state.clone(),
            _ => panic!("the shortcut page, not {:?}", a.key_context()),
        }
    }

    fn ctrl(a: &mut App, c: char) {
        key_with_modifiers(a, KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    #[tokio::test]
    async fn a_changed_shortcut_answers_at_once_everywhere_it_is_named_and_is_kept() {
        use keymap::Cmd;
        let mut a = app();
        a.open_chat("Ua".into());
        shortcut_page(&mut a, "Messages", Cmd::Reply);
        key(&mut a, KeyCode::Enter);
        assert!(matches!(
            shortcut_state(&a),
            shortcuts::State::Capture { add: false }
        ));
        assert!(screen(&mut a).contains("press the new key"), "{}", screen(&mut a));
        // Esc gives up waiting; the page then names the shortcut for keys.json.
        key(&mut a, KeyCode::Esc);
        assert_eq!(shortcut_state(&a), shortcuts::State::Browse);
        let Some(Modal::Shortcuts(p)) = &mut a.modal else {
            panic!()
        };
        p.note = None;
        assert!(
            screen(&mut a).contains("“messages.reply” in keys.json"),
            "{}",
            screen(&mut a)
        );
        key(&mut a, KeyCode::Enter);
        ctrl(&mut a, 't');
        assert_eq!(shortcut_state(&a), shortcuts::State::Browse);
        assert!(screen(&mut a).contains("(default r)"), "{}", screen(&mut a));
        // Back to Settings, which counts the change, then out.
        key(&mut a, KeyCode::Esc);
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Shortcuts));
        assert!(screen(&mut a).contains("1 changed"), "{}", screen(&mut a));
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());

        // In the messages: Ctrl-t replies, r does nothing.
        a.select_msg(Some(1));
        key(&mut a, KeyCode::Char('r'));
        assert_eq!(a.replying, None);
        a.select_msg(Some(1));
        ctrl(&mut a, 't');
        assert_eq!(a.replying.as_deref(), Some("01"));
        assert!(a.status.contains("Esc cancel"), "{}", a.status);
        // The message menu and the hint name the new key.
        a.select_msg(Some(1));
        assert!(a.hint().contains("Ctrl-t reply"), "{}", a.hint());
        key(&mut a, KeyCode::Char('m'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("message menu")
        };
        assert!(
            m.items.iter().any(|(l, _)| l == "↩ Reply\tCtrl-t"),
            "{:?}",
            m.items
        );
        a.modal = None;

        // Stored, and the next start answers with it.
        let stored = a.store.get("setting.keys").unwrap().unwrap();
        assert!(stored.contains(r#""messages.reply":["ctrl-t"]"#), "{stored}");
        keymap::install(keymap::Keymap::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            a.store.clone(),
        );
        assert!(b.key_problems.is_empty());
        let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(keymap::resolve(keymap::Ctx::Messages, &ctrl_t), Some(Cmd::Reply));

        // Delete puts the default back; nothing is stored then.
        shortcut_page(&mut a, "Messages", Cmd::Reply);
        key(&mut a, KeyCode::Delete);
        assert!(a.keys.is_default());
        assert_eq!(a.store.get("setting.keys").unwrap(), None);
        assert_eq!(keymap::resolve(keymap::Ctx::Messages, &ctrl_t), None);
    }

    #[tokio::test]
    async fn a_key_in_use_moves_only_when_asked() {
        use keymap::{Cmd, Ctx, Table};
        let mut a = app();
        shortcut_page(&mut a, "Messages", Cmd::Reply);
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Char('e'));
        assert!(matches!(shortcut_state(&a), shortcuts::State::Conflict { .. }));
        assert!(
            screen(&mut a).contains("already does “react”"),
            "{}",
            screen(&mut a)
        );
        key(&mut a, KeyCode::Char('n'));
        assert!(a.keys.is_default(), "kept");
        // Asked again and moved: React has no key left, and says so.
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Char('e'));
        key(&mut a, KeyCode::Enter);
        let messages = Table::Own(Ctx::Messages);
        assert_eq!(keymap::display_keys(a.keys.keys(messages, Cmd::Reply)), "e");
        assert!(a.keys.keys(messages, Cmd::React).is_empty());
        assert!(
            screen(&mut a).contains("“react” now has no key"),
            "{}",
            screen(&mut a)
        );
        assert!(keymap::help_lines(Ctx::Messages).contains(&("—".into(), "react")));
        // React back to its default clashes with Reply: both rows say so.
        let Some(Modal::Shortcuts(p)) = &mut a.modal else {
            panic!()
        };
        p.sel += 1;
        key(&mut a, KeyCode::Backspace);
        assert!(a.keys.clash(messages, Cmd::React).is_some());
        let text = screen(&mut a);
        assert!(
            text.contains("change one of them") && text.contains("⚠ e"),
            "{text}"
        );
        key(&mut a, KeyCode::Esc);
        assert!(screen(&mut a).contains("⚠ 1 changed"), "{}", screen(&mut a));
        // Putting back every default asks first.
        let rows = a.settings_rows_for_test(settings::Tab::Shortcuts);
        let Some(Modal::Settings(s)) = &mut a.modal else {
            panic!()
        };
        s.sel = rows
            .iter()
            .position(|r| r.starts_with("Put back every default"))
            .unwrap();
        key(&mut a, KeyCode::Enter);
        assert!(matches!(a.modal, Some(Modal::Confirm(_))));
        key(&mut a, KeyCode::Char('y'));
        assert!(a.keys.is_default());
        assert!(matches!(&a.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Shortcuts));
    }

    #[tokio::test]
    async fn the_new_key_is_checked_and_waiting_takes_every_key() {
        use keymap::Cmd;
        let mut a = app();
        shortcut_page(&mut a, "Message line", Cmd::Back);
        key(&mut a, KeyCode::Enter);
        key_with_modifiers(&mut a, KeyCode::Char('q'), KeyModifiers::ALT);
        assert!(screen(&mut a).contains("kept for plugins"), "{}", screen(&mut a));
        key(&mut a, KeyCode::Char('q'));
        assert!(screen(&mut a).contains("typed as text"), "{}", screen(&mut a));
        assert!(
            matches!(shortcut_state(&a), shortcuts::State::Capture { .. }),
            "still waiting"
        );
        // Even Ctrl-c is a candidate, not quitting (Quit has it: asked first).
        ctrl(&mut a, 'c');
        assert!(!a.quit);
        assert!(matches!(shortcut_state(&a), shortcuts::State::Conflict { .. }));
        key(&mut a, KeyCode::Esc);
        // And F1 is not help while waiting.
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::F(1));
        assert!(matches!(shortcut_state(&a), shortcuts::State::Conflict { .. }));
        key(&mut a, KeyCode::Esc);
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Esc);
        assert_eq!(shortcut_state(&a), shortcuts::State::Browse);
        assert!(a.keys.is_default());
        // `a` adds a key beside the others; Tab moves to the next screen.
        key(&mut a, KeyCode::Char('a'));
        ctrl(&mut a, 'q');
        let line = keymap::Table::Own(keymap::Ctx::Compose);
        assert_eq!(keymap::display_keys(a.keys.keys(line, Cmd::Back)), "Esc/Ctrl-q");
        key(&mut a, KeyCode::Tab);
        assert!(
            screen(&mut a).contains("Shortcuts › Text fields"),
            "{}",
            screen(&mut a)
        );
        // The editor's own help says its keys stay.
        key(&mut a, KeyCode::Char('?'));
        assert!(
            screen(&mut a).contains("These keys cannot be changed"),
            "{}",
            screen(&mut a)
        );
    }

    #[tokio::test]
    async fn default_keys_run_ignores_the_saved_shortcuts_but_keeps_them() {
        use keymap::{Cmd, Ctx, Table};
        let store = Store::open_in_memory().unwrap();
        store
            .set(
                "setting.keys",
                r#"{"messages.reply":["ctrl-t"],"messages.bogus":["x"]}"#,
            )
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut a = App::new(Client::new(Config::default()).unwrap(), "Ume".into(), tx, store);
        assert_eq!(a.key_problems.len(), 1, "{:?}", a.key_problems);
        let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(keymap::resolve(Ctx::Messages, &ctrl_t), Some(Cmd::Reply));
        a.use_default_keys(true);
        assert_eq!(keymap::resolve(Ctx::Messages, &ctrl_t), None);
        let r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(keymap::resolve(Ctx::Messages, &r), Some(Cmd::Reply));
        // The page shows (and edits) the saved ones, saying when they apply.
        let rows = a.settings_rows_for_test(settings::Tab::Shortcuts);
        assert!(rows.iter().any(|r| r.contains("--default-keys")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("messages.bogus")), "{rows:?}");
        shortcut_page(&mut a, "Messages", Cmd::React);
        key(&mut a, KeyCode::Enter);
        ctrl(&mut a, 'e');
        assert_eq!(
            keymap::display_keys(a.keys.keys(Table::Own(Ctx::Messages), Cmd::React)),
            "Ctrl-e"
        );
        let ctrl_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(
            keymap::resolve(Ctx::Messages, &ctrl_e),
            None,
            "from the next start"
        );
        assert!(a.store.get("setting.keys").unwrap().unwrap().contains("ctrl-e"));
        assert!(a.key_problems.is_empty(), "the bad entry is not stored any more");
    }

    #[tokio::test]
    async fn clicks_and_the_wheel_do_what_they_did_whatever_the_keys() {
        use keymap::{Cmd, Ctx, Table};
        let mut a = app();
        let mut map = keymap::Keymap::default();
        map.set(Table::Own(Ctx::Menu), Cmd::Confirm, vec![]);
        map.set(Table::Own(Ctx::Menu), Cmd::Down, vec![]);
        map.set(
            Table::Global,
            Cmd::Settings,
            vec![keymap::Key::of(&KeyEvent::new(
                KeyCode::Char('e'),
                KeyModifiers::CONTROL,
            ))],
        );
        keymap::install(map);
        ctrl_x(&mut a);
        let text = screen(&mut a);
        assert!(text.contains("Ctrl-e") && !text.contains("Ctrl-s"), "{text}");
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("main menu")
        };
        let i = m.items.iter().position(|(l, _)| l.contains("Settings")).unwrap();
        // Enter chooses nothing now, the wheel still moves, a click chooses.
        key(&mut a, KeyCode::Enter);
        assert!(matches!(a.modal, Some(Modal::Menu(_))));
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(&a.modal, Some(Modal::Menu(m)) if m.sel == 1));
        let y = a.hit.modal_rows.iter().find(|r| r.1 == i).unwrap().0;
        let x = a.hit.modal.unwrap().x + 2;
        click(&mut a, x, y);
        assert!(
            matches!(a.modal, Some(Modal::Settings(_))),
            "{:?}",
            a.key_context()
        );
        // The welcome screen and plugin notices name the keys as they are.
        a.modal = None;
        assert!(screen(&mut a).contains("Ctrl-x"));
        assert_eq!(keymap::via_menu("Plugins"), "Ctrl-x › Plugins");
    }

    #[test]
    fn shift_tab_switches_focus_as_terminals_send_it() {
        let mut a = app();
        a.open_chat("Ua".into());
        a.focus = Focus::Chats;
        key_with_modifiers(&mut a, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(a.focus, Focus::Input);
        key_with_modifiers(&mut a, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(a.focus, Focus::Chats);
    }

    #[tokio::test]
    async fn copy_puts_a_text_message_on_the_clipboard_and_nothing_else() {
        let copied = || clipboard::COPIED.with(|c| c.borrow().clone());
        let mut a = app();
        a.open_chat("Ua".into());
        a.select_msg(Some(1));
        key(&mut a, KeyCode::Char('c'));
        assert_eq!(copied(), ["m1"]);
        assert_eq!(a.status, "copied 2 characters");
        assert_eq!(a.focus, Focus::Messages, "c is not typed");
        // The message menu offers it too, with its key.
        key(&mut a, KeyCode::Char('m'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("message menu")
        };
        assert!(
            m.items.iter().any(|(l, _)| l == "📋 Copy text\tc"),
            "{:?}",
            m.items
        );
        choose(&mut a, "Copy text");
        assert_eq!(copied(), ["m1", "m1"]);
        // Nothing to copy from an unsent message (or media, sealed text, …).
        a.messages.get_mut("Ua").unwrap()[1].unsent = true;
        a.select_msg(Some(1));
        key(&mut a, KeyCode::Char('c'));
        assert!(a.status.contains("only a text message"), "{}", a.status);
        key(&mut a, KeyCode::Char('m'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("message menu")
        };
        assert!(!m.items.iter().any(|(l, _)| l.contains("Copy")));
        assert_eq!(copied().len(), 2);
        let sealed: Message = serde_json::from_value(serde_json::json!({
            "id": "9", "contentType": 0, "chunks": ["a", "b", "c", "d", "e"],
            "contentMetadata": {"e2eeVersion": "2"}
        }))
        .unwrap();
        assert!(sealed.is_e2ee() && !to_ui(&sealed, None).unwrap().copyable());
        let image: Message =
            serde_json::from_value(serde_json::json!({"id": "8", "contentType": 1})).unwrap();
        assert!(!to_ui(&image, None).unwrap().copyable());
        let text: Message =
            serde_json::from_value(serde_json::json!({"id": "7", "contentType": 0, "text": "hi"})).unwrap();
        assert!(to_ui(&text, None).unwrap().copyable());
        assert!(!to_ui(&text, Some("no key")).unwrap().copyable());
    }

    #[test]
    fn old_status_messages_go_with_the_next_key_but_not_by_themselves() {
        let mut a = app();
        let aged = |a: &mut App, text: &str, secs: u64| {
            a.status = text.into();
            a.status_since = Some((text.into(), Instant::now() - Duration::from_secs(secs)));
        };
        // Fresh: kept through keys.
        a.status = "copied 2 characters".into();
        a.note_status();
        key(&mut a, KeyCode::Down);
        assert_eq!(a.status, "copied 2 characters");
        // Old: the next key clears it first.
        aged(&mut a, "copied 2 characters", 9);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.status, "");
        // Failures stay longer.
        aged(&mut a, "send failed: timeout", 9);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.status, "send failed: timeout");
        aged(&mut a, "send failed: timeout", 31);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.status, "");
        // A key that says the same thing again says it afresh.
        a.list.select(Some(0));
        aged(&mut a, "no other unread chat", 9);
        key(&mut a, KeyCode::Char('u'));
        assert_eq!(a.status, "no other unread chat");
        // Clicks and the wheel count; the pointer merely moving does not.
        aged(&mut a, "saved", 9);
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(moved);
        assert_eq!(a.status, "saved");
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            ..moved
        });
        assert_eq!(a.status, "");
    }

    #[tokio::test]
    async fn long_lists_page_by_what_the_screen_shows() {
        let mut a = app();
        a.modal = Some(Modal::Pick(modal::Pick {
            title: String::new(),
            kind: modal::PickKind::ShareContact,
            options: (0..100)
                .map(|i| (format!("U{i}"), format!("Friend {i:02}")))
                .collect(),
            chosen: vec![],
            filter: TextInput::default(),
            sel: 0,
            single: false,
        }));
        screen(&mut a);
        let shown = a.hit.modal_rows.len();
        assert!(shown > 3, "{shown}");
        let sel = |a: &App| match &a.modal {
            Some(Modal::Pick(p)) => p.sel,
            _ => panic!("picker"),
        };
        key(&mut a, KeyCode::PageDown);
        assert_eq!(sel(&a), shown);
        key(&mut a, KeyCode::PageUp);
        assert_eq!(sel(&a), 0);
        // Settings › Shortcuts lists more screens than fit.
        a.open_settings_at(settings::Tab::Shortcuts);
        screen(&mut a);
        let Some(Modal::Settings(s)) = &a.modal else {
            panic!()
        };
        let first = s.sel;
        key(&mut a, KeyCode::PageDown);
        let Some(Modal::Settings(s)) = &a.modal else {
            panic!()
        };
        assert!(s.sel > first + 3, "{} after {first}", s.sel);
        key(&mut a, KeyCode::PageUp);
        let Some(Modal::Settings(s)) = &a.modal else {
            panic!()
        };
        assert_eq!(s.sel, first);
    }

    #[tokio::test]
    async fn shortcuts_are_also_kept_in_a_file_that_wins_at_start() {
        use keymap::{Cmd, Ctx, Table};
        let dir = std::env::temp_dir().join(format!("rune-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("rune").join("keys.json");
        let reply = |a: &App| keymap::display_keys(a.keys.keys(Table::Own(Ctx::Messages), Cmd::Reply));
        // A change made before there was a file goes into it.
        let mut a = app();
        shortcut_page(&mut a, "Messages", Cmd::Reply);
        key(&mut a, KeyCode::Enter);
        ctrl(&mut a, 't');
        a.use_keys_file(file.clone());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"_about\"") && text.contains("\"ctrl-t\""),
            "{text}"
        );
        // Every change is written to it.
        shortcut_page(&mut a, "Messages", Cmd::React);
        key(&mut a, KeyCode::Enter);
        ctrl(&mut a, 'e');
        assert!(std::fs::read_to_string(&file).unwrap().contains("messages.react"));
        // Edited by hand, then read again from Settings › Shortcuts.
        std::fs::write(&file, r#"{"messages.reply": ["ctrl-y"], "messages.nope": ["z"]}"#).unwrap();
        a.modal = None;
        a.open_settings_at(settings::Tab::Shortcuts);
        let rows = a.settings_rows_for_test(settings::Tab::Shortcuts);
        assert!(
            rows.iter().any(|r| r.contains("rune/keys.json: edit it")),
            "{rows:?}"
        );
        let Some(Modal::Settings(s)) = &mut a.modal else {
            panic!()
        };
        s.sel = rows.iter().position(|r| r == "Read keys.json again").unwrap();
        key(&mut a, KeyCode::Enter);
        assert!(a.status.contains("1 changed shortcut"), "{}", a.status);
        assert_eq!(reply(&a), "Ctrl-y");
        assert!(a.keys.keys(Table::Own(Ctx::Messages), Cmd::React) == [keymap::parse_key("e").unwrap()]);
        assert!(
            a.settings_rows_for_test(settings::Tab::Shortcuts)
                .iter()
                .any(|r| r.contains("messages.nope"))
        );
        let ctrl_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert_eq!(keymap::resolve(Ctx::Messages, &ctrl_y), Some(Cmd::Reply));

        // At the next start the file wins over the store.
        let store = a.store.clone();
        store
            .set("setting.keys", r#"{"messages.reply":["ctrl-q"]}"#)
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut b = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            store.clone(),
        );
        assert_eq!(reply(&b), "Ctrl-q");
        b.use_keys_file(file.clone());
        assert_eq!(reply(&b), "Ctrl-y");
        assert!(
            store.get("setting.keys").unwrap().unwrap().contains("ctrl-y"),
            "the store follows"
        );

        // A file half-way through an edit is neither used nor overwritten.
        std::fs::write(&file, "{ \"messages.reply\": [").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut c = App::new(
            Client::new(Config::default()).unwrap(),
            "Ume".into(),
            tx,
            store.clone(),
        );
        c.use_keys_file(file.clone());
        assert_eq!(reply(&c), "Ctrl-y", "the stored ones are used");
        assert!(
            c.key_problems.iter().any(|p| p.contains("unreadable")),
            "{:?}",
            c.key_problems
        );
        shortcut_page(&mut c, "Messages", Cmd::Reply);
        key(&mut c, KeyCode::Delete);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ \"messages.reply\": [");
        assert!(
            c.key_problems.iter().any(|p| p.contains("unreadable")),
            "still said"
        );

        // A symlinked file (dotfile managers) is written where it points.
        #[cfg(unix)]
        {
            let real = dir.join("dotfiles-keys.json");
            std::fs::write(&real, "{}").unwrap();
            let link = dir.join("linked").join("keys.json");
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let mut d = app();
            d.use_keys_file(link.clone());
            shortcut_page(&mut d, "Messages", Cmd::Reply);
            key(&mut d, KeyCode::Enter);
            ctrl(&mut d, 't');
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert!(std::fs::read_to_string(&real).unwrap().contains("ctrl-t"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The next search result the app posts (other events are dropped).
    async fn found(rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> Box<find::Found> {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(AppEvent::Found(f))) => return f,
                Ok(Some(_)) => continue,
                other => panic!("no search result: {}", other.is_ok()),
            }
        }
    }

    #[tokio::test]
    async fn search_in_a_chat_lists_stored_matches_and_shows_the_chosen_one() {
        let store = Store::open_in_memory().unwrap();
        let text = |i: i64| match i {
            150 => "the Secret word",
            390 => "secret again",
            _ => "chatter",
        };
        let msgs: Vec<Message> = (1..=400)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "id": i.to_string(), "from": if i % 2 == 0 { "Ume" } else { "Ua" }, "to": "Ua",
                    "toType": 0, "contentType": 0, "text": text(i),
                    "createdTime": (1_000_000 + i * 1000).to_string()
                }))
                .unwrap()
            })
            .collect();
        store.put_messages("Ua", &msgs, None).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut a = App::new(Client::new(Config::default()).unwrap(), "Ume".into(), tx, store);
        a.names.insert("Ua".into(), "Alice".into());
        a.open_chat("Ua".into());
        assert!(
            !a.open_messages().iter().any(|m| m.id == "150"),
            "older than what is loaded"
        );
        a.select_msg(Some(a.open_messages().len() - 1));
        key(&mut a, KeyCode::Char('/'));
        assert_eq!(a.key_context(), keymap::Ctx::Find);
        assert!(screen(&mut a).contains("type to search"), "{}", screen(&mut a));
        type_text(&mut a, "SECRET");
        // Every change searched; only the answer to the last one is shown.
        while matches!(&a.modal, Some(Modal::Find(f)) if f.busy) {
            let f = found(&mut rx).await;
            a.on_event(AppEvent::Found(f));
        }
        let Some(Modal::Find(f)) = &a.modal else {
            panic!("search")
        };
        assert_eq!(f.searched.as_deref(), Some("SECRET"));
        assert_eq!(
            f.hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["390", "150"]
        );
        let text = screen(&mut a);
        assert!(
            text.contains("2 matches") && text.contains("Secret word"),
            "{text}"
        );
        // A stale result does not replace it.
        a.on_found(find::Found {
            chat: "Ua".into(),
            seq: 1,
            query: "S".into(),
            hits: Ok(vec![]),
        });
        assert!(matches!(&a.modal, Some(Modal::Find(f)) if f.hits.len() == 2));
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.selected_msg().map(|m| m.id.as_str()), Some("150"));
        assert_eq!(a.focus, Focus::Messages);
        // History stays contiguous: everything from 150 to the newest is loaded.
        let ids: Vec<i64> = a.open_messages().iter().map(|m| m.id.parse().unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "{ids:?}");
        // The query comes back for this chat, from the chat menu too.
        a.run_action(Action::FindInChat("Ua".into()));
        let Some(Modal::Find(f)) = &a.modal else {
            panic!("search")
        };
        assert_eq!(f.query.as_str(), "SECRET");
        while matches!(&a.modal, Some(Modal::Find(f)) if f.busy) {
            let f = found(&mut rx).await;
            a.on_event(AppEvent::Found(f));
        }
        assert!(matches!(&a.modal, Some(Modal::Find(f)) if f.hits.len() == 2));
        // Esc closes; the chat menu lists it.
        key(&mut a, KeyCode::Esc);
        assert!(a.modal.is_none());
        a.chat_menu("Ua");
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("chat menu")
        };
        assert!(m.items.iter().any(|(l, _)| l.contains("Search in chat")));
    }
}
