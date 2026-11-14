//! `rune-tui` — a terminal chat client built on the `rune` library.
//!
//! ```sh
//! cargo run --release --features tui --bin rune-tui [session.json]
//! ```
//!
//! Options: `--no-read-receipts` (don't mark chats as read when opened),
//! `--no-graphics` (draw pictures with text instead of the terminal's image protocol),
//! `--no-mouse` (leave the mouse to the terminal, e.g. for text selection),
//! `--db PATH` (local message store; default: the session path with `.db`),
//! `--no-store` (keep nothing on disk: an in-memory store).
//!
//! Chats, messages (decrypted), contacts, read marks and the event-stream
//! position live in a local SQLite store, so start-up shows the chat list at
//! once and catches up on what happened while rune was closed.
//!
//! Keys are context-aware and listed in full by `?` (or `F1`, which also
//! works while typing); the status line hints at the common ones. Arrows
//! navigate (`j`/`k` are aliases in lists), `Enter` confirms or opens, `Esc`
//! goes back and `m` opens the selected item's menu, which lists everything
//! that can be done with it. `Ctrl-C` quits, `Ctrl-S` opens settings; with a
//! chat open, `Ctrl-O` sends a photo/file/sticker/location/contact, `Ctrl-G`
//! opens the chat's menu and `Ctrl-P` its profile — these work while typing.
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
//! kept in order per pin list (`K`/`J` move the selected pin). In the chat
//! list `/` searches chats, `m` opens the chat's menu (pin, pin lists,
//! invite, rename, mute, leave, albums, import), `n` creates a group and `p`
//! shows the chat's profile.
//!
//! Mouse: click a reaction line to see who reacted; click a chat to open it,
//! click a picture to view it, wheel to
//! scroll; in the viewer the wheel steps through media and a click outside
//! (or right-click) closes it. Shift-drag still selects text in most terminals.
//! Links are underlined; click one to open it in the browser. Pictures are
//! scanned for QR codes, shown under the picture and in the viewer (`u`
//! or a click opens a QR link).
//!
//! Profiles: `p` on a chat or message, `Ctrl-P` for the open chat, your own
//! from Settings › Profile, or click the chat title or a sender's name. The
//! page shows the picture, name, status message, buttons (`←`/`→` and
//! `Enter`, or click) and, for groups, members; a member opens their profile,
//! `Esc` goes back.
//!
//! Commands in the input line: `/attach [PATH]` (also `/image`, `/file`),
//! `/sticker`, `/location`, `/contact`, `/group`, `/invite`,
//! `/leave`, `/settings`, `/save PATH` (latest media in this chat), `/quit`.
//! Pasting (or dropping) a file path into the input line offers to send it.
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
use std::time::Duration;

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
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::FilterType;
use ratatui_image::picker::Picker;
use ratatui_image::picker::ProtocolType;

#[path = "rune-tui/albums.rs"]
mod albums;
#[path = "rune-tui/anim.rs"]
mod anim;
#[path = "rune-tui/attach.rs"]
mod attach;
#[path = "rune-tui/avatar.rs"]
mod avatar;
#[path = "rune-tui/cells.rs"]
mod cells;
#[path = "rune-tui/history_import.rs"]
mod history_import;
#[path = "rune-tui/input.rs"]
mod input;
#[path = "rune-tui/keymap.rs"]
mod keymap;
#[path = "rune-tui/links.rs"]
mod links;
#[path = "rune-tui/modal.rs"]
mod modal;
#[path = "rune-tui/picture.rs"]
mod picture;
#[path = "rune-tui/profile.rs"]
mod profile;
#[path = "rune-tui/settings.rs"]
mod settings;
#[path = "rune-tui/stickers.rs"]
mod stickers;
use albums::media_type;
use avatar::{Avatar, HEAD_AVATAR, LIST_AVATAR};
use cells::CellMode;
use input::TextInput;
use modal::{Action, Modal};
use picture::Picture;
use profile::{ProfileAction, ProfileData, ProfileLoaded};
use rune::import::{imported_sender_name, is_imported_id};
use rune::models::{Operation, content_type as ct, op_type};
use rune::ops::message_id_cmp;
use rune::store::{ChatRow, UserRow};
use rune::{
    Chat, Client, Config, Contact, MediaUpload, Message, MessageReaction, MidType, OpEvent, OutgoingMessage,
    PictureSize, Profile, QrLoginEvent, QrLoginOptions, Reaction, ReactionKind, Session, StickerImage,
    StickerPackage, Store,
};
use settings::{NotifyMode, Prefs};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// -- model -----------------------------------------------------------------------------

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
}

/// Chat-list preview of a pending invitation.
const INVITE_PREVIEW: &str = "Invitation · Enter to join or decline";

impl ChatEntry {
    fn invitation(chat: &Chat) -> Self {
        ChatEntry {
            mid: chat.chat_mid.clone(),
            preview: INVITE_PREVIEW.into(),
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
    /// Read from a LINE chat export: only stored locally, the server does not know it.
    imported: bool,
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
    /// Play it in mpv (videos, audio).
    Play,
}

/// Inline thumbnail sizes (columns × rows), picked in Settings › Chats.
const THUMB_SIZES: [(u16, u16); 4] = [(32, 8), (48, 14), (64, 20), (96, 30)];
/// Scaling for inline thumbnails: Lanczos keeps block-glyph pictures sharp
/// (nearest-neighbour makes them noisy).
const THUMB_FILTER: FilterType = FilterType::Lanczos3;
/// The viewer re-scales large originals on resize; Catmull-Rom is a good
/// quality/speed trade-off there.
const VIEW_FILTER: FilterType = FilterType::CatmullRom;

/// Results of background work, delivered to the UI loop.
enum AppEvent {
    Chats(Vec<ChatEntry>),
    /// A page of older (inactive) chats and the cursor for the next page
    /// (`None`: that was the last).
    MoreChats(Result<(Vec<ChatEntry>, Option<String>), String>),
    Names(HashMap<String, String>),
    /// Profile/group picture paths, per mid (only mids that have one).
    Pictures(HashMap<String, String>),
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
    /// The open overlay, if any.
    modal: Option<Modal>,
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
    sticker_pics: HashMap<String, Load<Picture>>,
    file_previews: HashMap<String, Load<Picture>>,
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
    /// Chats whose server history is loaded to its start (imported messages may be older).
    server_done: HashSet<String>,
    /// The conversation is scrolled to its oldest loaded row.
    at_top: bool,
    /// Boundary between read and unread messages in one chat.
    unread_divider: Option<(String, Unread)>,
    /// Scroll the divider into view after the unread chat is opened.
    jump_unread: bool,
    /// The terminal has focus (for notifications).
    focused: bool,
    chats: Vec<ChatEntry>,
    /// The chat list is filtering its entries.
    searching: bool,
    /// Name or participant query shown in the chat-list search field.
    chat_query: String,
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
    /// Mids drawn without a known name since the last lookup round.
    name_misses: std::cell::RefCell<HashSet<String>>,
    /// Picture paths learned with names, per mid.
    picture_paths: HashMap<String, String>,
    /// Avatars beside names, per mid.
    avatars: HashMap<String, Load<Avatar>>,
    messages: HashMap<String, Vec<UiMsg>>,
    loading: HashSet<String>,
    open: Option<String>,
    focus: Focus,
    input: TextInput,
    /// Lines scrolled up from the bottom of the conversation.
    scroll: usize,
    status: String,
    quit: bool,
    /// Selected message (index into the open chat's messages).
    sel: Option<usize>,
    thumbs: HashMap<String, Load<Picture>>,
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
}

/// Screen areas from the last frame.
#[derive(Default)]
struct HitAreas {
    /// Inside the chat list's border.
    chats: Rect,
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
fn ellipsize(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_owned();
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
    const PALETTE: [u8; 8] = [24, 30, 61, 64, 95, 131, 133, 166];
    let h = mid
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
    Color::Indexed(PALETTE[h as usize % PALETTE.len()])
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

/// Why imported messages cannot be replied to, reacted to, unsent…
const IMPORTED_ONLY_LOCAL: &str = "imported from a chat export: this message exists only on this computer";

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
        imported: is_imported_id(&m.id),
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
                // Break after the last space, if any; long words split anywhere.
                match line.rfind(' ') {
                    Some(i) if i > 0 => {
                        let rest = line.split_off(i + 1);
                        line.pop();
                        out.push(std::mem::replace(&mut line, rest));
                    }
                    _ => out.push(std::mem::take(&mut line)),
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
    let link_style = body.fg(Color::LightBlue).underlined();
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

fn store_download_in(dir: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    // Never let a server-supplied name escape the directory.
    let safe: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = if safe.trim_matches('.').is_empty() {
        "file".to_owned()
    } else {
        safe
    };
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (safe.clone(), String::new()),
    };
    let mut p = dir.join(&safe);
    let mut n = 1;
    while let Ok(meta) = std::fs::symlink_metadata(&p) {
        // Cheap length check before reading the file back.
        if meta.is_file() && meta.len() == data.len() as u64 && std::fs::read(&p).is_ok_and(|b| b == data) {
            return Ok(p);
        }
        p = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&p, data)?;
    Ok(p)
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
    std::process::Command::new(opener)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// Fetch the picture shown inline for a message. LINE's previews are tiny
/// (≤384 px, JPEG quality 0.3), so images use the main picture (≤1280 px)
/// and fall back to the preview; videos only have the preview frame.
fn spawn_thumb(client: Client, tx: mpsc::UnboundedSender<AppEvent>, m: Message) {
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
                Ok(img) if media_type(&m) == ct::IMAGE => Ok(scan_qr(img).await),
                Ok(img) => Ok((img, Vec::new())),
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
    const PALETTE: [Color; 6] = [
        Color::Cyan,
        Color::Magenta,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
    ];
    let h = mid
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[(h as usize) % PALETTE.len()]
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
            Err(e) => return drop(tx.send(AppEvent::Status(format!("chat list failed: {e}")))),
        };
        let chats = parse_boxes(&client, &boxes).await;
        let mids: Vec<String> = chats.iter().map(|c| c.mid.clone()).collect();
        let _ = tx.send(AppEvent::Chats(chats));
        match client.invited_chats().await {
            Ok(inv) => drop(tx.send(AppEvent::Invitations(inv, true))),
            Err(e) => drop(tx.send(AppEvent::Status(format!("invitations failed: {e}")))),
        }
        resolve_names(&client, &tx, mids).await;
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
            Err(e) => drop(tx.send(AppEvent::Status(format!("friend list failed: {e}")))),
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
    let b = client.download_picture(path, size).await.ok()?;
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

/// Write `data` to [`view_dir`] for opening (private to this user).
fn store_for_viewing(name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let dir = view_dir();
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(&dir)?;
    store_download_in(&dir, name, data)
}

/// A mid `getContactsV2` / `getChats` can look up (not empty, not a square).
fn resolvable(mid: &str) -> bool {
    mid.len() >= 8
        && mid.bytes().all(|b| b.is_ascii_alphanumeric())
        && matches!(MidType::of(mid), MidType::User | MidType::Group | MidType::Room)
}

/// Contacts for `mids`; when a batch is refused (one bad mid can fail it),
/// split it so the rest still resolve.
async fn contacts_lenient(client: &Client, mids: &[String]) -> HashMap<String, Contact> {
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
            Err(e) => tracing::debug!("getContactsV2 {}: {e}", part[0]),
        }
    }
    out
}

/// Chats for `mids`, split like [`contacts_lenient`] when a batch is refused.
async fn chats_lenient(client: &Client, mids: &[String]) -> Vec<Chat> {
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
            Err(e) => tracing::debug!("getChats {}: {e}", part[0]),
        }
    }
    out
}

/// Look up display names for users (contacts) and groups (chats).
async fn resolve_names(client: &Client, tx: &mpsc::UnboundedSender<AppEvent>, mids: Vec<String>) {
    let mut mids: Vec<String> = mids.into_iter().filter(|m| resolvable(m)).collect();
    mids.sort();
    mids.dedup();
    let (groups, users): (Vec<String>, Vec<String>) =
        mids.into_iter().partition(|m| MidType::of(m) != MidType::User);
    let mut names = HashMap::new();
    let mut pictures = HashMap::new();
    let mut mutes = HashMap::new();
    let mut members = HashMap::new();
    if !users.is_empty() {
        let cs = contacts_lenient(client, &users).await;
        for (mid, c) in cs {
            if let Some(p) = c.picture_path.as_deref().filter(|p| !p.trim().is_empty()) {
                pictures.insert(mid.clone(), p.to_owned());
            }
            mutes.insert(mid.clone(), c.notifications_disabled());
            names.insert(mid, c.name().to_owned());
        }
    }
    if !groups.is_empty() {
        for c in chats_lenient(client, &groups).await {
            if let Some(p) = c.picture() {
                pictures.insert(c.chat_mid.clone(), p);
            }
            mutes.insert(c.chat_mid.clone(), c.notifications_disabled());
            members.insert(c.chat_mid.clone(), (c.member_mids(), c.invitee_mids()));
            names.insert(c.chat_mid.clone(), c.chat_name.clone());
        }
    }
    if !members.is_empty() {
        let _ = tx.send(AppEvent::Members(members));
    }
    if !mutes.is_empty() {
        let _ = tx.send(AppEvent::Mutes(mutes));
    }
    if !pictures.is_empty() {
        let _ = tx.send(AppEvent::Pictures(pictures));
    }
    if !names.is_empty() {
        let _ = tx.send(AppEvent::Names(names));
    }
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
            // Imported messages are not on the server; join up with what came from it.
            let newest_stored = store.newest_fetched_message(&chat).ok().flatten();
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
                    let _ = store.clear_fetched_messages(&chat);
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
                let _ = tx.send(AppEvent::History(chat, Vec::new()));
                let _ = tx.send(AppEvent::Status(format!("history failed: {e}")));
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
                    let _ = tx.send(AppEvent::Status(format!("event stream stopped: {e}")));
                    return;
                }
            };
            if tx.send(ev).is_err() {
                return;
            }
        }
    });
}

// -- app logic ---------------------------------------------------------------------------

impl App {
    /// A fresh app showing what the store already knows.
    fn new(client: Client, me: String, tx: mpsc::UnboundedSender<AppEvent>, store: Store) -> App {
        let prefs = Prefs::load(&store);
        let mut app = App {
            client,
            me,
            tx,
            thumb_size: prefs.thumb_size,
            store,
            prefs,
            no_read_receipts_flag: false,
            modal: None,
            contacts: HashMap::new(),
            members: HashMap::new(),
            invitees: HashMap::new(),
            read_marks: HashMap::new(),
            typing: HashMap::new(),
            uploads: Vec::new(),
            sticker_pics: HashMap::new(),
            file_previews: HashMap::new(),
            server_settings: None,
            my_profile: None,
            stale: HashSet::new(),
            resync: HashSet::new(),
            older_loading: HashSet::new(),
            no_older: HashSet::new(),
            server_done: HashSet::new(),
            at_top: false,
            unread_divider: None,
            jump_unread: false,
            focused: true,
            chats: Vec::new(),
            searching: false,
            chat_query: String::new(),
            pinned: Vec::new(),
            pin_list: None,
            list: ListState::default(),
            more_chats: MoreChats::Ready(None),
            names: HashMap::new(),
            name_requests: HashMap::new(),
            name_misses: Default::default(),
            picture_paths: HashMap::new(),
            avatars: HashMap::new(),
            messages: HashMap::new(),
            loading: HashSet::new(),
            open: None,
            focus: Focus::Chats,
            input: TextInput::default(),
            scroll: 0,
            status: String::new(),
            quit: false,
            sel: None,
            thumbs: HashMap::new(),
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
        };
        app.reload_pins();
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
            if let Some(p) = c.picture_path.as_deref().filter(|p| !p.trim().is_empty()) {
                app.picture_paths
                    .entry(c.mid.clone())
                    .or_insert_with(|| p.to_owned());
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
        if !app.chats.is_empty() {
            app.list.select(Some(0));
        }
        app
    }

    fn name(&self, mid: &str) -> String {
        if mid == self.me {
            return "You".into();
        }
        if let Some(n) = imported_sender_name(mid) {
            return if n.is_empty() { "LINE".into() } else { n.to_owned() };
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

    /// Start a lookup for the names (and pictures) of `mids` that are not
    /// known yet and were not asked for in the last `NAME_RETRY`.
    fn want_names(&mut self, mids: impl IntoIterator<Item = String>) {
        let now = std::time::Instant::now();
        let mut ask: Vec<String> = Vec::new();
        for mid in mids {
            if !resolvable(&mid)
                || mid == self.me
                || self.names.get(&mid).is_some_and(|n| !n.is_empty())
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
        let misses = std::mem::take(&mut *self.name_misses.borrow_mut());
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
                let _ = self.store.upsert_user(&row);
            }
        }
    }

    fn selected_mid(&self) -> Option<String> {
        self.list
            .selected()
            .and_then(|i| self.chat_at(i))
            .map(|c| c.mid.clone())
    }

    fn visible_chats(&self) -> Vec<usize> {
        if !self.searching || self.chat_query.trim().is_empty() {
            return (0..self.chats.len()).collect();
        }
        self.chats
            .iter()
            .enumerate()
            .filter(|(_, chat)| self.chat_matches(chat))
            .map(|(i, _)| i)
            .collect()
    }

    fn chat_matches(&self, chat: &ChatEntry) -> bool {
        let query = self.chat_query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        if self.name(&chat.mid).to_lowercase().contains(&query) {
            return true;
        }
        self.members
            .get(&chat.mid)
            .into_iter()
            .chain(self.invitees.get(&chat.mid))
            .flatten()
            .any(|mid| self.name(mid).to_lowercase().contains(&query))
    }

    fn chat_at(&self, pos: usize) -> Option<&ChatEntry> {
        self.chats.get(*self.visible_chats().get(pos)?)
    }

    fn select_chat_mid(&mut self, mid: Option<&str>) {
        let visible = self.visible_chats();
        self.list
            .select(mid.and_then(|mid| visible.iter().position(|&i| self.chats[i].mid == mid)));
    }

    fn set_chat_query(&mut self, query: String) {
        let selected = self.selected_mid();
        self.chat_query = query;
        let visible = self.visible_chats();
        let selected = selected
            .as_deref()
            .and_then(|mid| visible.iter().position(|&i| self.chats[i].mid == mid))
            .or_else(|| (!visible.is_empty()).then_some(0));
        self.list.select(selected);
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

    fn unread_index(&self, chat: &str, msgs: &[UiMsg]) -> Option<usize> {
        let (divider_chat, unread) = self.unread_divider.as_ref()?;
        if divider_chat != chat {
            return None;
        }
        match unread {
            Unread::After(anchor) => msgs.iter().position(|m| {
                !m.id.is_empty() && !m.imported && m.from != self.me && message_id_cmp(&m.id, anchor).is_gt()
            }),
            Unread::Last(n) => msgs
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, m)| !m.id.is_empty() && !m.imported && m.from != self.me)
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
            if self.visible_chats().iter().any(|&i| self.chats[i].mid == mid) {
                self.select_chat_mid(Some(&mid));
            }
            self.focus = Focus::Input;
        }
    }

    /// Move the chat-list selection by `delta` rows, staying on the list
    /// (ratatui only clamps a selection when drawing, which is too late for
    /// keys that arrive before the next frame).
    fn move_chat_sel(&mut self, delta: isize) {
        let n = self.visible_chats().len();
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
            self.status = format!("invitation to {} — Enter to join or decline", self.name(&mid));
            return;
        }
        let mut mark = None;
        let mut has_unread = false;
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
            c.unread = 0;
            c.last_seen = c.last_id.clone();
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
        // Only imported messages: LINE's own history still has to be fetched.
        let empty = self
            .messages
            .get(&mid)
            .is_none_or(|v| v.iter().all(|m| m.imported));
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
        if self.read_receipts()
            && let Some(id) = mark
        {
            let (c, chat) = (self.client.clone(), mid.clone());
            tokio::spawn(async move {
                let _ = c.mark_read(&chat, &id).await;
            });
        }
        self.persist_chat(&mid);
        if self.open.as_deref() != Some(mid.as_str()) {
            self.replying = None;
        }
        self.open = Some(mid);
        self.scroll = 0;
        self.sel = None;
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
        let _ = self.store.upsert_chat(&row);
    }

    /// Forget a chat (left, declined, removed).
    fn remove_chat(&mut self, mid: &str) {
        let selected = self.selected_mid();
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
            &mut self.server_done,
        ] {
            set.remove(mid);
        }
        self.read_marks.remove(mid);
        self.typing.remove(mid);
        let _ = self.store.delete_chat(mid);
        if self.open.as_deref() == Some(mid) {
            self.open = None;
            self.sel = None;
            self.replying = None;
            self.focus = Focus::Chats;
            // The viewer only ever shows the open chat's media.
            if self.viewer.is_some() {
                self.close_viewer();
            }
        }
        self.sort_chats(selected);
        let visible_len = self.visible_chats().len();
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
        self.sort_chats(selected);
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

    /// Who (other than us) has read message `id` in `chat`, as far as we know.
    fn readers(&self, chat: &str, id: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .read_marks
            .get(chat)
            .into_iter()
            .flatten()
            .filter(|(reader, mark)| **reader != self.me && message_id_cmp(mark, id).is_ge())
            .map(|(reader, _)| reader.clone())
            .collect();
        v.sort();
        v
    }

    fn find_msg(&self, id: &str) -> Option<&UiMsg> {
        self.messages.values().flatten().find(|m| m.id == id)
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
    /// Imported messages wait until LINE has nothing older, so server
    /// history between them and the loaded messages is not skipped.
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
        let oldest_real = list.iter().find(|m| !m.id.is_empty() && !m.imported).cloned();
        let server_open = !self.server_done.contains(&chat) && oldest_real.is_some();
        let mut stored = self
            .store
            .messages(&chat, 100, Some(&oldest.id))
            .unwrap_or_default();
        if server_open {
            // Only the stored messages newer than the first imported one.
            let keep_from = stored
                .iter()
                .rposition(|m| is_imported_id(&m.message.id))
                .map_or(0, |i| i + 1);
            stored.drain(..keep_from);
        }
        if !stored.is_empty() {
            let ui = stored.iter().filter_map(stored_to_ui).collect();
            let n = self.prepend(&chat, ui);
            self.status = format!("{n} older messages");
            return;
        }
        match oldest_real.filter(|_| server_open) {
            Some(real) => {
                self.older_loading.insert(chat.clone());
                self.status = "loading older messages…".into();
                spawn_older(
                    self.client.clone(),
                    self.tx.clone(),
                    chat,
                    real.id,
                    real.delivered,
                );
            }
            None => {
                self.no_older.insert(chat);
                self.status = "this is the beginning of the conversation".into();
            }
        }
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
                        imported: false,
                    });
                    ui.from = me;
                    let _ = tx.send(AppEvent::Sent(chat, Box::new(ui), Some(Box::new(m))));
                }
                Err(e) => drop(tx.send(AppEvent::Status(format!("send failed: {e}")))),
            }
        });
    }

    /// Desktop notification / bell for an incoming message, per the settings.
    fn maybe_notify(&self, chat: &str, m: &Message) {
        let p = &self.prefs;
        if p.notify == NotifyMode::Off || m.from == self.me {
            return;
        }
        // Replayed history after a restart is not news.
        if m.created_time.is_some_and(|t| now_ms() - t > 120_000) {
            return;
        }
        let group = MidType::of(chat) != MidType::User;
        if group && !p.notify_groups {
            return;
        }
        if self.chats.iter().any(|c| c.mid == chat && c.muted) {
            return;
        }
        if self.open.as_deref() == Some(chat) && self.focused && !p.notify_focused {
            return;
        }
        let title = if group {
            format!("{} · {}", self.name(&m.from), self.name(chat))
        } else {
            self.name(&m.from)
        };
        let body = if p.notify_preview {
            preview_of(m)
        } else {
            "New message".into()
        };
        settings::notify(p.notify, &title, &body);
    }

    /// Apply one stream operation to the store and the screen.
    fn on_operation(&mut self, op: Operation) {
        let _ = self.store.apply(&op, &self.me, preview_of);
        match op.event() {
            OpEvent::Message(m) => {
                let chat = m.chat_mid(&self.me).to_owned();
                if self.typing.get(&chat).is_some_and(|(who, _)| *who == m.from) {
                    self.typing.remove(&chat);
                }
                if let Some(ui) = to_ui(m, op.e2ee_error.as_ref().map(|e| e.detail.as_str())) {
                    let mine = m.from == self.me;
                    self.push_message(&chat, ui, !mine);
                    if mine {
                        if self
                            .unread_divider
                            .as_ref()
                            .is_some_and(|(divider_chat, _)| divider_chat == &chat)
                        {
                            self.unread_divider = None;
                            self.jump_unread = false;
                        }
                        if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                            c.unread = 0;
                            c.last_seen = Some(m.id.clone());
                        }
                    } else {
                        self.maybe_notify(&chat, m);
                        if self.read_receipts() && self.open.as_deref() == Some(chat.as_str()) {
                            let (c, id, to) = (self.client.clone(), m.id.clone(), chat.clone());
                            tokio::spawn(async move {
                                let _ = c.mark_read(&to, &id).await;
                            });
                            if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                                c.last_seen = Some(m.id.clone());
                            }
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
                if reader != self.me {
                    let marks = self.read_marks.entry(chat.to_owned()).or_default();
                    let newer = marks
                        .get(reader)
                        .is_none_or(|cur| message_id_cmp(message_id, cur).is_gt());
                    if newer {
                        marks.insert(reader.to_owned(), message_id.to_owned());
                    }
                }
            }
            OpEvent::ChatChecked { chat, message_id } => {
                if let Some((divider_chat, unread)) = self.unread_divider.as_mut()
                    && divider_chat == chat
                {
                    match unread {
                        Unread::After(anchor) if message_id_cmp(message_id, anchor).is_gt() => {
                            *anchor = message_id.to_owned();
                            self.jump_unread = false;
                        }
                        Unread::Last(_) => {
                            *unread = Unread::After(message_id.to_owned());
                            self.jump_unread = false;
                        }
                        Unread::After(_) => {}
                    }
                }
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
            }
            OpEvent::Unsent { chat, message_id, .. } => {
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
            return;
        };
        if after == After::Nothing {
            return;
        }
        let motion = is_motion_file(&media.file_name);
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
        // Only `s` keeps a copy in Downloads; opening and playing use the cache folder.
        let written = if after == After::Save {
            store_download(&media.file_name, &media.data)
        } else {
            store_for_viewing(&media.file_name, &media.data)
        };
        let result = written.and_then(|p| {
            if after == After::Open && !play {
                open_external(&p)?;
            }
            Ok(p)
        });
        self.status = match (result, after) {
            (Ok(p), _) if play => play_media(&self.tx, &p),
            (Ok(p), After::Open) => format!("opened {}", p.display()),
            (Ok(p), _) => format!("saved to {}", p.display()),
            (Err(e), _) => format!("could not save: {e}"),
        };
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
    fn push_message(&mut self, chat: &str, msg: UiMsg, unread: bool) {
        let list = self.messages.entry(chat.to_owned()).or_default();
        if list.iter().any(|m| m.id == msg.id && !msg.id.is_empty()) {
            return;
        }
        let preview = msg.text.clone();
        let time = msg.time;
        let id = (!msg.id.is_empty()).then(|| msg.id.clone());
        list.push(msg);
        let selected = self.selected_mid();
        match self.chats.iter_mut().find(|c| c.mid == chat) {
            Some(c) => {
                if time >= c.last_time {
                    c.preview = preview;
                    c.last_time = time;
                    c.last_id = id.or(c.last_id.take());
                }
                if unread && self.open.as_deref() != Some(chat) {
                    c.unread += 1;
                }
            }
            None => {
                self.chats.push(ChatEntry {
                    mid: chat.to_owned(),
                    preview,
                    last_time: time,
                    unread: u32::from(unread && self.open.as_deref() != Some(chat)),
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
        let caller = [&op.param1, &op.param2]
            .into_iter()
            .flatten()
            .find(|p| p.len() > 1 && p.starts_with(['u', 'U']))
            .map(|mid| self.name(mid));
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
                let _ = self.store.delete_chat(&mid);
            }
        }
        let new: Vec<String> = chats
            .iter()
            .filter(|c| !self.chats.iter().any(|e| e.mid == c.chat_mid && e.invited))
            .map(|c| c.chat_mid.clone())
            .collect();
        for chat in chats {
            if let Some(p) = chat.picture() {
                self.picture_paths.insert(chat.chat_mid.clone(), p);
            }
            self.names.insert(chat.chat_mid.clone(), chat.chat_name.clone());
            match self.chats.iter_mut().find(|c| c.mid == chat.chat_mid) {
                Some(c) => *c = ChatEntry::invitation(&chat),
                None => self.chats.push(ChatEntry::invitation(&chat)),
            }
            self.persist_chat(&chat.chat_mid);
        }
        if !full && self.prefs.notify != NotifyMode::Off {
            for mid in &new {
                settings::notify(
                    self.prefs.notify,
                    "rune",
                    &format!("Invitation to {}", self.name(mid)),
                );
            }
        }
        self.sort_chats(selected);
        let visible_len = self.visible_chats().len();
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
            let _ = self.store.delete_chat(&mid);
            self.status = format!("declined {name}");
        }
        self.sort_chats(selected);
        let visible_len = self.visible_chats().len();
        if self.list.selected().is_none_or(|i| i >= visible_len) {
            self.list.select((visible_len > 0).then_some(0));
        }
    }

    fn sort_chats(&mut self, keep_selected: Option<String>) {
        // Invitations stay first, then pinned chats, then recent activity.
        self.chats.sort_by_key(|c| {
            let pin_position = self.pinned.iter().position(|mid| mid == &c.mid);
            (
                std::cmp::Reverse(c.invited),
                std::cmp::Reverse(pin_position.is_some() && !c.invited),
                pin_position.unwrap_or(usize::MAX),
                std::cmp::Reverse(c.last_time),
            )
        });
        if let Some(sel) = keep_selected {
            self.select_chat_mid(Some(&sel));
        }
    }

    fn on_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Chats(server) => self.on_server_chats(server),
            AppEvent::MoreChats(res) => self.on_more_chats(res),
            AppEvent::Names(n) => {
                let selected = self.selected_mid();
                let profile_name = n.get(&self.me).cloned();
                let changed: Vec<String> = n
                    .iter()
                    .filter(|(mid, name)| self.names.get(*mid) != Some(name))
                    .map(|(mid, _)| mid.clone())
                    .collect();
                self.names.extend(n);
                if let Some(name) = profile_name {
                    let _ = self.store.set_profile_name(&name);
                }
                for mid in &changed {
                    self.persist_chat(mid);
                }
                self.persist_users(&changed);
                self.select_chat_mid(selected.as_deref());
            }
            AppEvent::Pictures(p) => {
                for (mid, path) in p {
                    if self.picture_paths.get(&mid) != Some(&path) {
                        self.avatars.remove(&mid);
                        self.picture_paths.insert(mid.clone(), path);
                        self.persist_chat(&mid);
                        self.persist_users(std::slice::from_ref(&mid));
                    }
                }
            }
            AppEvent::Avatar(mid, img) => self.on_avatar(mid, img),
            AppEvent::History(chat, msgs) => {
                self.loading.remove(&chat);
                self.stale.remove(&chat);
                self.resync.remove(&chat);
                let _ = self.store.put_messages(&chat, &msgs, None);
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
                if done {
                    self.server_done.insert(chat.clone());
                }
                let _ = self.store.put_messages(&chat, &msgs, None);
                let n = if self.store.imported_count(&chat).unwrap_or(0) > 0 {
                    // Fetched messages may land between imported ones (and replace copies of them).
                    let before = self.messages.get(&chat).map_or(0, Vec::len);
                    self.reload_from_store(&chat, before + msgs.len());
                    self.messages
                        .get(&chat)
                        .map_or(0, Vec::len)
                        .saturating_sub(before)
                } else {
                    let ui: Vec<UiMsg> = msgs.iter().filter_map(|m| to_ui(m, None)).collect();
                    self.prepend(&chat, ui)
                };
                if n == 0 && done {
                    // Nothing more from LINE: go on with imported messages, if any.
                    self.load_older();
                } else {
                    self.status = format!("{n} older messages");
                }
            }
            AppEvent::Incoming(op) => self.on_operation(*op),
            AppEvent::Control(name, data) => {
                if let Some(r) = data.get("nextRevision").and_then(|v| match v {
                    serde_json::Value::String(s) => s.parse().ok(),
                    v => v.as_i64(),
                }) {
                    let _ = self.store.set_stream_revision(r);
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
                    let _ = self.store.put_message(&chat, &m, None);
                }
                self.push_message(&chat, *msg, false);
                if let Some(c) = self.chats.iter_mut().find(|c| c.mid == chat) {
                    c.unread = 0;
                    c.last_seen = c.last_id.clone();
                }
                self.persist_chat(&chat);
                self.status = "sent".into();
            }
            AppEvent::Status(s) => self.status = s,
            AppEvent::Invitations(chats, full) => self.on_invitations(chats, full),
            AppEvent::InviteAnswered(mid, accepted, res) => self.on_invite_answered(mid, accepted, res),
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
            }
            AppEvent::Contacts(list) => {
                let _ = self.store.upsert_contacts(&list);
                let mut mutes = HashMap::new();
                for c in &list {
                    self.contacts.insert(c.mid.clone(), c.name().to_owned());
                    self.names.insert(c.mid.clone(), c.name().to_owned());
                    if let Some(p) = c.picture_path.as_deref().filter(|p| !p.trim().is_empty()) {
                        self.picture_paths.insert(c.mid.clone(), p.to_owned());
                    }
                    mutes.insert(c.mid.clone(), c.notifications_disabled());
                }
                self.on_event(AppEvent::Mutes(mutes));
            }
            AppEvent::Unsent(chat, id, res) => match res {
                Ok(()) => {
                    let _ = self.store.mark_unsent(&id);
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
                    let verb = if muted { "muted" } else { "notifications on for" };
                    self.status = format!("{verb} {}", self.name(&chat));
                }
                Err(e) => self.status = format!("could not change notifications: {e}"),
            },
            AppEvent::GroupCreated(res, invited) => match res {
                Ok(chat) => {
                    let mid = chat.chat_mid.clone();
                    self.names.insert(mid.clone(), chat.chat_name.clone());
                    if let Some(p) = chat.picture() {
                        self.picture_paths.insert(mid.clone(), p);
                    }
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
                    Ok(v) => Load::Ready(v),
                    Err(e) => Load::Failed(e),
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
                    Ok(p) => Load::Ready((p.display_name, p.status_message)),
                    Err(e) => Load::Failed(e),
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
                if matches!(&self.modal, Some(Modal::Settings(s)) if s.tab == 3) {
                    let tab = 3;
                    self.modal = None;
                    self.open_settings_at(tab);
                }
            }
            AppEvent::Tick => {
                let now = std::time::Instant::now();
                self.typing.retain(|_, (_, until)| *until > now);
            }
            AppEvent::Thumb(id, res) => {
                let slot = match res {
                    Ok((img, qr)) => {
                        if !qr.is_empty() {
                            self.qr.insert(id.clone(), qr);
                        }
                        Load::Ready(Picture::new(img, self.picker.as_ref()))
                    }
                    Err(e) => Load::Failed(e),
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
            if self.open.as_deref() == Some(s.mid.as_str()) {
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
        let visible_len = self.visible_chats().len();
        if self.list.selected().is_none() && visible_len > 0 {
            self.list.select(Some(0));
        }
        self.status = format!("{} chats · live", self.chats.len());
        if let Some(open) = self.open.clone()
            && self.stale.contains(&open)
            && self.loading.insert(open.clone())
        {
            spawn_history(self.client.clone(), self.tx.clone(), self.store.clone(), open);
        }
    }

    /// The selection reached the last chat: fetch the next page of older chats.
    fn maybe_more_chats(&mut self) {
        let MoreChats::Ready(after) = &self.more_chats else {
            return;
        };
        let visible_len = self.visible_chats().len();
        let at_end = self.list.selected().is_some_and(|i| i + 1 >= visible_len);
        if !at_end || visible_len == 0 {
            return;
        }
        let after = after.clone();
        self.more_chats = MoreChats::Loading(after.clone());
        self.status = "loading more chats…".into();
        let known = self.chats.iter().map(|c| c.mid.clone()).collect();
        spawn_more_chats(self.client.clone(), self.tx.clone(), after, known);
    }

    fn on_more_chats(&mut self, res: Result<(Vec<ChatEntry>, Option<String>), String>) {
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
    }

    fn clear_cache(&mut self) {
        if let Err(e) = self.store.clear() {
            self.status = format!("could not clear: {e}");
            return;
        }
        let _ = self.store.bind_account(&self.me);
        self.messages.clear();
        self.read_marks.clear();
        self.no_older.clear();
        self.server_done.clear();
        self.stale.clear();
        self.reload_pins();
        self.chats.clear();
        self.more_chats = MoreChats::Ready(None);
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
        let (cmd, arg) = match text.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (text.as_str(), ""),
        };
        // Commands that work without an open chat.
        match cmd {
            "/quit" => return self.quit = true,
            "/settings" => return self.open_settings(),
            "/group" | "/newgroup" => return self.run_action(Action::NewGroup),
            "/join" => {
                self.status = "LINE's Chrome protocol cannot join by invitation link or QR code \
                               (no ticket methods); invitations are listed first in the chat list"
                    .into();
                return;
            }
            _ => {}
        }
        let Some(chat) = self.open.clone() else {
            self.status = "open a chat first".into();
            return;
        };
        let is_group = MidType::of(&chat) != MidType::User;
        match cmd {
            "/save" if !arg.is_empty() => return self.save_latest_media(&chat, arg.to_owned()),
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
            "/flex" => return self.status = modal::FLEX_UNSUPPORTED.into(),
            "/invite" if is_group => return self.run_action(Action::Invite(chat)),
            "/leave" if is_group => return self.run_action(Action::ConfirmLeave(chat)),
            "/rename" if is_group => return self.run_action(Action::Rename(chat)),
            "/albums" | "/album" if is_group => return self.run_action(Action::Albums(chat)),
            "/mute" => return self.run_action(Action::Mute(chat, true)),
            "/unmute" => return self.run_action(Action::Mute(chat, false)),
            "/menu" => return self.chat_menu(&chat),
            _ => {}
        }
        // "//text" sends "/text"; other unknown commands are not sent by accident.
        let text = if let Some(rest) = text.strip_prefix("//") {
            format!("/{rest}")
        } else if text.starts_with('/') && !text.contains(char::is_whitespace) && text.len() < 20 {
            self.input.set(text.clone());
            self.status = format!("unknown command {cmd} (start with // to send it as text)");
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
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.status = "downloading…".into();
        tokio::spawn(async move {
            let s = match client.download_media(&m, false).await {
                Ok(data) => match std::fs::write(&path, &data) {
                    Ok(()) => format!("saved {} bytes to {path}", data.len()),
                    Err(e) => format!("write failed: {e}"),
                },
                Err(e) => format!("download failed: {e}"),
            };
            let _ = tx.send(AppEvent::Status(s));
        });
    }

    fn on_key(&mut self, k: KeyEvent) {
        use keymap::Cmd;
        if k.kind != KeyEventKind::Press {
            return;
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
        if !self.profile.is_empty() {
            return self.profile_key(k);
        }
        // Chords for the open chat, also while typing.
        match (cmd, self.open.clone()) {
            (Some(Cmd::ComposeMenu), _) => return self.compose_menu(),
            (Some(Cmd::ChatMenu), Some(mid)) => return self.chat_menu(&mid),
            (Some(Cmd::ChatProfile), Some(mid)) => return self.open_profile(mid),
            (Some(Cmd::ChatMenu | Cmd::ChatProfile), None) => {
                self.status = "open a chat first".into();
                return;
            }
            _ => {}
        }
        match ctx {
            keymap::Ctx::Viewer => self.viewer_key(cmd),
            keymap::Ctx::ChatSearch => self.search_key(cmd, &k),
            keymap::Ctx::ChatList => self.chat_list_key(cmd),
            keymap::Ctx::Compose => self.compose_key(cmd, &k),
            keymap::Ctx::Messages => self.messages_key(cmd, &k),
            _ => {}
        }
        if self.searching && self.focus != Focus::Chats {
            self.end_search();
        }
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
            Some(Cmd::DeleteBackward) => {
                let mut query = self.chat_query.clone();
                query.pop();
                self.set_chat_query(query);
            }
            Some(Cmd::Confirm) => {
                if let Some(mid) = self.selected_mid() {
                    self.enter_chat(mid);
                }
            }
            Some(Cmd::Up) => self.move_chat_sel(-1),
            Some(Cmd::Down) => self.move_chat_sel(1),
            Some(Cmd::SwitchFocus) if self.open.is_some() => {
                self.end_search();
                self.focus = Focus::Input;
            }
            None => {
                if let Some(c) = typed_char(k) {
                    let mut query = self.chat_query.clone();
                    query.push(c);
                    self.set_chat_query(query);
                }
            }
            _ => {}
        }
    }

    /// Chat-list rows that fit on screen (for PgUp / PgDn).
    fn chat_page(&self) -> isize {
        (self.hit.chats.height / CHAT_ROW_H).max(1) as isize
    }

    fn chat_list_key(&mut self, cmd: Option<keymap::Cmd>) {
        use keymap::Cmd;
        let selected = self.selected_mid();
        match cmd {
            Some(Cmd::Up) => self.move_chat_sel(-1),
            Some(Cmd::Down) => self.move_chat_sel(1),
            Some(Cmd::PageUp) => self.move_chat_sel(-self.chat_page()),
            Some(Cmd::PageDown) => self.move_chat_sel(self.chat_page()),
            Some(Cmd::Confirm) => {
                if let Some(mid) = selected {
                    self.enter_chat(mid);
                }
            }
            Some(Cmd::SwitchFocus) if self.open.is_some() => self.focus = Focus::Input,
            Some(Cmd::Search) => {
                self.searching = true;
                self.set_chat_query(String::new());
            }
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
        match cmd {
            Some(Cmd::Confirm) => self.submit(),
            Some(Cmd::Back) if self.replying.take().is_some() => {
                self.status = "reply cancelled".into();
            }
            Some(Cmd::Back | Cmd::SwitchFocus) => self.focus = Focus::Chats,
            Some(Cmd::Up) => self.select_msg(self.open_messages().len().checked_sub(1)),
            Some(Cmd::PageUp) => self.scroll_by(10),
            Some(Cmd::PageDown) => self.scroll_by(-10),
            Some(Cmd::CursorLeft) => self.input.left(),
            Some(Cmd::CursorRight) => self.input.right(),
            Some(Cmd::LineStart) => self.input.home(),
            Some(Cmd::LineEnd) => self.input.end(),
            Some(Cmd::DeleteBackward) => self.input.backspace(),
            Some(Cmd::DeleteForward) => self.input.delete(),
            None => {
                if let Some(c) = typed_char(k) {
                    self.input.insert(c);
                }
            }
            _ => {}
        }
    }

    fn messages_key(&mut self, cmd: Option<keymap::Cmd>, k: &KeyEvent) {
        use keymap::Cmd;
        let imported = self.selected_msg().is_some_and(|m| m.imported);
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
            Some(Cmd::Confirm) if self.selected_msg().is_some_and(UiMsg::plays) => self.play_selected(),
            Some(Cmd::Confirm) => {
                let media = self.selected_msg().is_some_and(|m| m.media_kind().is_some());
                if media || !self.open_selected_link() {
                    self.open_viewer();
                }
            }
            Some(Cmd::Reply) => self.start_reply(),
            Some(Cmd::Menu) => self.message_menu(),
            Some(Cmd::React | Cmd::Unsend) if imported => self.status = IMPORTED_ONLY_LOCAL.into(),
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
        m.from == self.me && !m.unsent && !m.id.is_empty() && !m.imported
    }

    /// Bracketed paste: into the search field or an overlay's text field; a
    /// file path in a chat offers to send the file; other text is typed.
    fn on_paste(&mut self, text: &str) {
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
            let query = format!("{}{line}", self.chat_query);
            return self.set_chat_query(query);
        }
        if self.open.is_some()
            && self.focus != Focus::Chats
            && let Some(path) = attach::pasted_path(text)
        {
            return self.open_attach(Some(path));
        }
        if self.open.is_some() {
            self.focus = Focus::Input;
            self.input.insert_str(&line);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
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
            MouseEventKind::Down(MouseButton::Left) if in_chats => {
                let i = self.list.offset() + ((m.row - self.hit.chats.y) / CHAT_ROW_H) as usize;
                if let Some(mid) = self.chat_at(i).map(|c| c.mid.clone()) {
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
        if self.searching && self.focus != Focus::Chats {
            self.end_search();
        }
    }

    /// `r` on a selected message: the next text message quotes it.
    fn start_reply(&mut self) {
        // From the viewer's menu: typing happens in the message line.
        if self.viewer.is_some() {
            self.close_viewer();
        }
        if self.selected_msg().is_some_and(|m| m.imported) {
            self.status = IMPORTED_ONLY_LOCAL.into();
            return;
        }
        let Some(m) = self.selected_msg().filter(|m| !m.id.is_empty()) else {
            self.status = "this message cannot be replied to yet".into();
            return;
        };
        self.replying = Some(m.id.clone());
        self.focus = Focus::Input;
        self.status = "replying — Enter send · Esc cancel".into();
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
                Modal::Info { .. } | Modal::Reactions { .. } => Ctx::Info,
                Modal::Help { .. } => Ctx::Help,
                Modal::Form(_) => Ctx::Form,
                Modal::Pick(_) => Ctx::Pick,
                Modal::Attach(a) if a.chosen.is_some() => Ctx::AttachSend,
                Modal::Attach(_) => Ctx::AttachBrowse,
                Modal::Stickers(_) => Ctx::Stickers,
                Modal::Settings(_) => Ctx::Settings,
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
        let real_msg = msg.filter(|m| !m.id.is_empty() && !m.imported);
        match cmd {
            Cmd::ComposeMenu | Cmd::ChatMenu | Cmd::ChatProfile => self.open.is_some(),
            Cmd::Confirm if self.focus == Focus::Chats && self.modal.is_none() => chat.is_some(),
            Cmd::Menu | Cmd::Profile if self.focus == Focus::Chats => chat.is_some(),
            Cmd::PinUp | Cmd::PinDown => chat.is_some_and(|c| self.is_pinned(&c.mid)),
            Cmd::Menu | Cmd::Reply | Cmd::React => real_msg.is_some(),
            Cmd::Profile => msg.is_some_and(|m| imported_sender_name(&m.from).is_none()),
            Cmd::Unsend => msg.is_some_and(|m| self.can_unsend(m)),
            Cmd::Save | Cmd::OpenExternal => msg.is_some_and(|m| m.media_kind().is_some()),
            Cmd::OpenLink => msg.is_some_and(|m| !self.links_of(m).is_empty()),
            Cmd::PlayPause => self.viewer_plays() || self.viewer_animates(),
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
        let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        let [left, right] = Layout::horizontal([Constraint::Length(38), Constraint::Min(20)]).areas(main);
        self.draw_chats(f, left);
        let [conv, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(right);
        self.draw_conversation(f, conv);
        self.draw_input(f, input);
        // Overlays show their own keys on their frame; the status line then
        // only says how to get help, so nothing is listed twice.
        let overlay = self.modal.is_some() || !self.profile.is_empty() || self.viewer.is_some();
        let help = keymap::help_key(self.key_context());
        let hint = match self.hint() {
            keys if !overlay && !keys.is_empty() => format!("{help} · {keys}"),
            _ => help.to_owned(),
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.status),
                Style::new().fg(Color::Black).bg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(hint, Style::new().dim()),
        ]);
        f.render_widget(Paragraph::new(line), status);
        self.hit.viewer = None;
        self.hit.profile = None;
        if self.viewer.is_some() {
            self.draw_viewer(f, main);
        }
        if !self.profile.is_empty() {
            self.draw_profile(f, main);
        }
        if self.modal.is_some() {
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
        f.render_widget(Clear, area);
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
            .border_style(Style::new().fg(Color::Green))
            .title(ellipsize(&title, area.width.saturating_sub(2) as usize))
            .title_bottom(Span::styled(
                keymap::hint(keymap::Ctx::Viewer, |cmd| self.available(cmd)),
                Style::new().dim(),
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
                f.render_widget(Paragraph::new(info).style(Style::new().dim()), head);
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
                            Span::styled(label, Style::new().fg(Color::Green).bold()),
                            Span::styled(code.clone(), Style::new().fg(Color::LightBlue).underlined()),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(label, Style::new().fg(Color::Green).bold()),
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
            _ => f.render_widget(Paragraph::new("downloading…").style(Style::new().dim()), inner),
        }
    }

    fn block(&self, title: String, focused: bool) -> Block<'static> {
        let style = if focused {
            Style::new().fg(Color::Green)
        } else {
            Style::new().dim()
        };
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style)
            .title(title)
    }

    fn draw_chats(&mut self, f: &mut Frame, area: Rect) {
        let list_area = if self.searching {
            let search_area = Rect::new(area.x, area.y, area.width, area.height.min(3));
            let block = self.block(" Search ".into(), self.focus == Focus::Chats);
            let inner = block.inner(search_area);
            let max = inner.width.saturating_sub(1) as usize;
            let mut shown = self.chat_query.as_str();
            while shown.width() > max {
                let mut chars = shown.chars();
                chars.next();
                shown = chars.as_str();
            }
            f.render_widget(Paragraph::new(shown.to_owned()).block(block), search_area);
            if self.focus == Focus::Chats
                && self.modal.is_none()
                && self.profile.is_empty()
                && self.viewer.is_none()
                && inner.height > 0
            {
                f.set_cursor_position(Position::new(inner.x + shown.width() as u16, inner.y));
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
        let visible_chats = self.visible_chats();
        let inner_w = list_area.width.saturating_sub(2) as usize;
        // Text starts after the accent bar, the avatar tile and a gap.
        let text_w = inner_w.saturating_sub(CHAT_TEXT_X as usize + 1);
        let rows_h = CHAT_ROW_H as usize;
        // Avatars of the chats that can be on screen (fetched on first sight).
        let rows = list_area.height.saturating_sub(2) as usize / rows_h + 1;
        let first = self.list.offset().min(self.list.selected().unwrap_or(0));
        let near: Vec<String> = visible_chats
            .iter()
            .skip(first.saturating_sub(rows))
            .take(rows * 3)
            .filter_map(|&i| self.chats.get(i).map(|c| c.mid.clone()))
            .collect();
        let avatars: HashSet<String> = near.into_iter().filter(|m| self.avatar_ready(m)).collect();
        let focused = self.focus == Focus::Chats;
        let selected = self.list.selected();
        let sel_bg = Style::new().bg(Color::Indexed(236));
        let pinned_end = if visible_chats.iter().any(|&i| {
            let c = &self.chats[i];
            !c.invited && !self.is_pinned(&c.mid)
        }) {
            visible_chats.iter().enumerate().rposition(|(_, &i)| {
                let c = &self.chats[i];
                !c.invited && self.is_pinned(&c.mid)
            })
        } else {
            None
        };
        let mut items: Vec<ListItem> = visible_chats
            .iter()
            .enumerate()
            .map(|(i, &chat_i)| {
                let c = &self.chats[chat_i];
                let name = self.name(&c.mid);
                let is_sel = selected == Some(i);
                let bar = match (is_sel, focused) {
                    (true, true) => Span::styled("▌", Style::new().fg(Color::Green)),
                    (true, false) => Span::styled("▌", Style::new().fg(Color::Gray)),
                    _ => Span::raw(" "),
                };
                // Avatar tile: drawn over by the picture when one is loaded,
                // otherwise a coloured monogram.
                let (tile_top, tile_bottom) = if avatars.contains(&c.mid) {
                    (Span::raw("    "), Span::raw("    "))
                } else {
                    let tile = Style::new().fg(Color::White).bg(monogram_color(&c.mid)).bold();
                    let initial: String = name
                        .chars()
                        .find(|ch| ch.width().unwrap_or(0) > 0)
                        .map(|ch| ch.to_uppercase().collect())
                        .unwrap_or_else(|| "?".into());
                    let iw = initial.width().min(4);
                    let left = (4 - iw) / 2;
                    let top = format!("{}{initial}{}", " ".repeat(left), " ".repeat(4 - iw - left));
                    (Span::styled(top, tile), Span::styled("    ", tile))
                };

                // Line 1: name ··· time
                let time = chat_time(c.last_time);
                let bell = if c.muted { " 🔕" } else { "" };
                let pin_mark = if self.is_pinned(&c.mid) && !c.invited {
                    "📌 "
                } else {
                    ""
                };
                let name_w = text_w.saturating_sub(time.width() + 1 + bell.width() + pin_mark.width());
                let name = format!("{pin_mark}{}{bell}", ellipsize(&name, name_w));
                let gap = text_w.saturating_sub(name.width() + time.width());
                let unread = c.unread > 0;
                let name_style = if unread {
                    Style::new().bold().fg(Color::White)
                } else {
                    Style::new().bold()
                };
                let time_style = if unread {
                    Style::new().fg(Color::Green)
                } else {
                    Style::new().dim()
                };
                let head = Line::from(vec![
                    bar.clone(),
                    tile_top,
                    Span::raw(" "),
                    Span::styled(name, name_style),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(time, time_style),
                ]);

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
                let flat: String = c
                    .preview
                    .chars()
                    .map(|ch| if ch == '\n' { ' ' } else { ch })
                    .collect();
                let preview_w = text_w.saturating_sub(if unread { badge.width() + 1 } else { 0 });
                let preview = ellipsize(flat.trim(), preview_w);
                let gap = text_w.saturating_sub(preview.width() + badge.width());
                let preview_style = if c.invited {
                    Style::new().fg(Color::Yellow)
                } else if unread {
                    Style::new()
                } else {
                    Style::new().dim()
                };
                let mut body = vec![
                    bar,
                    tile_bottom,
                    Span::raw(" "),
                    Span::styled(preview, preview_style),
                    Span::raw(" ".repeat(gap)),
                ];
                if unread {
                    body.push(Span::styled(
                        badge,
                        Style::new().fg(Color::Black).bg(Color::Green).bold(),
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
                    Line::from(Span::styled(label, Style::new().dim()))
                } else {
                    Line::default()
                };
                ListItem::new(vec![head, body, spacer])
            })
            .collect();
        if items.is_empty() && self.searching {
            items.push(ListItem::new(Line::styled(
                "no matching chats",
                Style::new().dim(),
            )));
        }
        let total_unread: u32 = self.chats.iter().map(|c| c.unread).sum();
        let title = if total_unread > 0 {
            format!(" Chats · {total_unread} unread ")
        } else {
            " Chats ".into()
        };
        let mut block = self.block(title, focused);
        match self.more_chats {
            _ if visible_chats.is_empty() => {}
            MoreChats::Ready(_) => {
                block = block.title_bottom(Line::styled(" ↓ at the end: more chats ", Style::new().dim()))
            }
            MoreChats::Loading(_) => {
                block = block.title_bottom(Line::styled(" loading more chats… ", Style::new().dim()))
            }
            MoreChats::Done => {}
        }
        self.hit.chats = block.inner(list_area);
        let list = List::new(items).block(block);
        f.render_stateful_widget(list, list_area, &mut self.list);
        let inner = self.hit.chats;
        let visible: Vec<String> = visible_chats
            .iter()
            .skip(self.list.offset())
            .take(inner.height as usize / rows_h + 1)
            .filter_map(|&i| self.chats.get(i).map(|c| c.mid.clone()))
            .collect();
        for (k, mid) in visible.iter().enumerate() {
            if avatars.contains(mid) {
                let at = Position::new(inner.x + 1, inner.y + CHAT_ROW_H * k as u16);
                self.draw_avatar(f, mid, at, LIST_AVATAR, inner);
            }
        }
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
            let msg = Paragraph::new("Pick a chat on the left (Enter or click).").style(Style::new().dim());
            f.render_widget(msg, inner);
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
            rows.push(Row::Text(Line::from(Span::styled(
                "loading…",
                Style::new().dim(),
            ))));
        } else if self.older_loading.contains(&chat) {
            rows.push(Row::Text(
                Line::from(Span::styled("loading older messages…", Style::new().dim())).centered(),
            ));
        } else if self.no_older.contains(&chat) {
            rows.push(Row::Text(
                Line::from(Span::styled(
                    "— beginning of the conversation —",
                    Style::new().dim(),
                ))
                .centered(),
            ));
        } else if !msgs.is_empty() {
            rows.push(Row::Text(
                Line::from(Span::styled("↑ scroll up for older messages", Style::new().dim())).centered(),
            ));
        }
        let group_chat = MidType::of(&chat) != MidType::User;
        let selecting = self.focus == Focus::Messages && self.viewer.is_none();
        let unread_index = self.unread_index(&chat, msgs);
        let mut unread_row = None;
        for (i, m) in msgs.iter().enumerate() {
            if unread_index == Some(i) {
                unread_row = Some(rows.len());
                rows.push(Row::Text(
                    Line::from(Span::styled(
                        "── Unread Messages ──",
                        Style::new().fg(Color::Yellow).bold(),
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
                head.push(Span::styled("▶ ", Style::new().fg(Color::Green).bold()));
            }
            // Same one-column margin as the bubbles and thumbnails.
            if !mine {
                head.push(Span::raw(" "));
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
                    .fg(if mine { Color::Green } else { sender_color(&m.from) })
                    .bold(),
            ));
            head.push(Span::styled(format!("  {}", hhmm(m.time)), Style::new().dim()));
            if mine && !m.id.is_empty() && !m.unsent {
                let n = self.readers(&chat, &m.id).len();
                let read = match (n, group_chat) {
                    (0, _) => String::new(),
                    (_, false) => "  ✓ Read".into(),
                    (n, true) => format!("  ✓ Read by {n}"),
                };
                head.push(Span::styled(read, Style::new().fg(Color::Green)));
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
                let q = Span::styled(q, Style::new().dim().italic());
                let line = if mine {
                    Line::from(vec![q, Span::raw(" ")])
                } else {
                    Line::from(vec![Span::raw(" "), q])
                };
                rows.push(Row::Text(place(line)));
            }
            let mut body_style = if m.unsent {
                Style::new().dim().italic()
            } else {
                Style::new()
            };
            let mut border = Style::new().fg(if mine { Color::Green } else { sender_color(&m.from) });
            if m.unsent {
                border = Style::new().dim();
            }
            if selected {
                body_style = body_style.bg(Color::DarkGray);
                border = Style::new().fg(Color::White).bold();
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
                            self.thumbs.insert(m.id.clone(), Load::Loading);
                            spawn_thumb(self.client.clone(), self.tx.clone(), raw);
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
                        let span = Span::styled(hint, Style::new().dim());
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
                    if matches!(self.thumbs.get(&m.id), Some(Load::Loading)) {
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
                        Span::styled(format!("⏫ uploading {label}…"), Style::new().dim().italic()),
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
        let start = total.saturating_sub(height + self.scroll);
        self.at_top = start == 0 && !msgs.is_empty();
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
        let mut counts: Vec<(String, usize, bool)> = Vec::new();
        for r in reactions {
            let emoji = match &r.kind {
                ReactionKind::Predefined(n) => Reaction::from_i64(*n).map_or("•", Reaction::emoji).to_owned(),
                ReactionKind::Paid { .. } => "✨".to_owned(),
            };
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
                Style::new().bg(Color::Indexed(22)).bold()
            } else {
                Style::new().bg(Color::Indexed(236))
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
                format!(" ↩ Reply to {} · Esc cancel ", self.quote(chat, id, max))
            }
            _ => " Message ".into(),
        };
        let block = self.block(title, self.focus == Focus::Input);
        let inner = block.inner(area);
        let (shown, cursor) = self.input.view(inner.width as usize);
        f.render_widget(Paragraph::new(shown.to_owned()).block(block), area);
        if self.focus == Focus::Input
            && self.modal.is_none()
            && self.profile.is_empty()
            && self.viewer.is_none()
            && inner.width > 0
            && inner.height > 0
        {
            f.set_cursor_position(Position::new(inner.x + cursor.min(inner.width - 1), inner.y));
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

async fn run(
    mut terminal: DefaultTerminal,
    mut app: App,
    mut rx: mpsc::UnboundedReceiver<AppEvent>,
) -> std::io::Result<()> {
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut had_modal = false;
    while !app.quit {
        // Opening or closing an overlay over inline pictures can leave
        // Sixel/Kitty graphics on top of (or behind) it.
        if app.modal.is_some() != had_modal {
            had_modal = app.modal.is_some();
            if app.picker.is_some() {
                app.needs_clear = true;
            }
        }
        if app.needs_clear {
            // Wipe graphics (Sixel/Kitty) left behind by a closed viewer or overlay.
            terminal.clear()?;
            app.needs_clear = false;
        }
        app.sync_anim();
        app.maybe_more_chats();
        terminal.draw(|f| app.draw(f))?;
        app.request_missing_names();
        tokio::select! {
            ev = keys.next() => match ev {
                Some(Ok(TermEvent::Key(k))) => app.on_key(k),
                Some(Ok(TermEvent::Mouse(m))) => app.on_mouse(m),
                Some(Ok(TermEvent::Paste(text))) => app.on_paste(&text),
                Some(Ok(TermEvent::FocusGained)) => app.focused = true,
                Some(Ok(TermEvent::FocusLost)) => app.focused = false,
                // A resize just redraws.
                Some(Ok(_)) => {}
                Some(Err(e)) => app.status = format!("terminal input error: {e}"),
                // The terminal went away: nothing can reach the app any more.
                None => app.quit = true,
            },
            Some(ev) = rx.recv() => app.on_event(ev),
            _ = tick.tick() => {} // refresh relative timestamps
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read_receipts = !args.iter().any(|a| a == "--no-read-receipts");
    let graphics = !args.iter().any(|a| a == "--no-graphics");
    let mouse = !args.iter().any(|a| a == "--no-mouse");
    let cells_arg = args
        .iter()
        .position(|a| a == "--cells")
        .and_then(|i| args.get(i + 1));
    let cell_mode = match cells_arg {
        Some(v) => {
            CellMode::parse(v).ok_or(format!("--cells: expected half, quadrant or sextant, got {v}"))?
        }
        None => CellMode::Quadrant,
    };
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let path = args
        .iter()
        .enumerate()
        .find(|(i, a)| {
            !a.starts_with("--") && !(*i > 0 && matches!(args[i - 1].as_str(), "--cells" | "--db"))
        })
        .map(|(_, a)| a)
        .cloned()
        .unwrap_or_else(|| "session.json".into());
    let session = match Session::load(&path) {
        Ok(s) => s,
        Err(_) => login(&path).await?,
    };
    let client = session.into_client(Config::default())?;
    // Persist refreshed tokens.
    let (saver, save_path) = (client.clone(), path.clone());
    let mut token_rx = client.subscribe_tokens();
    tokio::spawn(async move {
        while token_rx.changed().await.is_ok() {
            let _ = Session::from_client(&saver).save(&save_path);
        }
    });
    let me = match client.my_mid() {
        Some(m) => m,
        None => client.profile().await?.mid,
    };

    // The local store: next to the session unless told otherwise.
    let store = if args.iter().any(|a| a == "--no-store") {
        Store::open_in_memory()?
    } else {
        let db = value_of("--db").map(PathBuf::from).unwrap_or_else(|| {
            let p = std::path::Path::new(&path);
            p.with_extension("db")
        });
        Store::open(&db).map_err(|e| format!("cannot open the message store {}: {e}", db.display()))?
    };
    if store.bind_account(&me)? {
        eprintln!("The message store belonged to another account; it was cleared.");
    }
    // Names used to be kept here before the store had a user table.
    let _ = store.remove("cache.user_names");
    let _ = store.prune_images(IMAGE_CACHE_BYTES);
    let resume_at = store.stream_revision()?;

    let (tx, rx) = mpsc::unbounded_channel();
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
    let picker = if graphics {
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
    let mouse = mouse && crossterm::execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste, EnableFocusChange);
    // ratatui's panic hook restores raw mode and the screen; switch the
    // extra modes off too, or a panic leaves the shell receiving mouse codes.
    let restore_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        disable_terminal_modes(keyboard_enhancement, mouse);
        restore_hook(info);
    }));
    let mut app = App::new(client, me, tx, store);
    app.no_read_receipts_flag = !read_receipts;
    app.cell_mode = cell_mode;
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
                    imported: false,
                })
                .collect();
            a.messages.insert((*mid).into(), msgs);
        }
        a
    }

    #[tokio::test]
    async fn imported_history_pages_after_line_history() {
        let mut a = app();
        // LINE is never reached: the gateway is a closed local port.
        a.client = Client::new(Config {
            gateway: "http://127.0.0.1:9".into(),
            max_retries: 0,
            ..Config::default()
        })
        .unwrap();
        let day = 86_400_000;
        let real = |id: &str, t: i64| -> Message {
            serde_json::from_value(serde_json::json!({
                "id": id, "from": "Uc", "to": "Ume", "contentType": 0, "text": id, "createdTime": t.to_string()
            }))
            .unwrap()
        };
        a.store
            .put_messages("Uc", &[real("100", 10 * day), real("101", 11 * day)], None)
            .unwrap();
        let export =
            rune::parse_chat_export("1970/01/02 Fri\n09:00\tCat\told one\n09:01\tCat\told two\n").unwrap();
        let msgs = export.to_messages("Uc", |_| None, |t| t.utc_ms(0));
        assert_eq!(a.store.import_messages("Uc", &msgs).unwrap().added, 2);
        a.chats.push(ChatEntry {
            mid: "Uc".into(),
            ..Default::default()
        });
        let shown = a.store.messages("Uc", 200, None).unwrap();
        assert_eq!(shown.len(), 4);
        // Only the fetched ones are loaded (as after a fresh fetch).
        a.messages
            .insert("Uc".into(), shown[2..].iter().filter_map(stored_to_ui).collect());
        a.open = Some("Uc".into());

        // LINE's older history comes before the imported messages.
        a.load_older();
        assert!(a.older_loading.contains("Uc"));
        assert_eq!(a.open_messages().len(), 2);
        a.on_event(AppEvent::Older("Uc".into(), vec![real("99", 5 * day)], true));
        let ids: Vec<_> = a.open_messages().iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, ["99", "100", "101"]);
        // LINE has nothing older: now the imported ones.
        a.load_older();
        let list = a.open_messages();
        assert_eq!(list.len(), 5);
        assert!(list[0].imported && list[1].imported);
        assert_eq!(a.name(&list[0].from), "Cat");
        a.load_older();
        assert!(a.no_older.contains("Uc"));
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
                imported: false,
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

    #[test]
    fn chat_search_without_matches_has_no_selection_and_shows_empty_state() {
        let mut a = app();
        a.list.select(Some(0));
        key(&mut a, KeyCode::Char('/'));
        type_text(&mut a, "missing");
        assert!(a.visible_chats().is_empty());
        assert_eq!(a.list.selected(), None);
        key(&mut a, KeyCode::Up);
        key(&mut a, KeyCode::Down);
        assert_eq!(a.list.selected(), None);
        key(&mut a, KeyCode::Enter);
        assert!(a.searching);
        assert_eq!(a.open, None);
        assert!(screen(&mut a).contains("no matching chats"));
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
    fn unread_divider_tracks_reads_and_clears_on_our_message() {
        let mut a = unread_app(Some("1"), 2);
        a.on_operation(op(serde_json::json!({
            "revision": 1, "type": 40, "param1": "Ua", "param2": "2", "param3": "0"
        })));
        assert_eq!(a.unread_index("Ua", a.open_messages()), Some(2));
        assert!(!a.jump_unread);
        a.on_operation(op(serde_json::json!({
            "revision": 2, "type": 40, "param1": "Ua", "param2": "3", "param3": "0"
        })));
        assert_eq!(a.unread_index("Ua", a.open_messages()), None);
        assert!(!screen(&mut a).contains("Unread Messages"));

        let mut a = unread_app(Some("1"), 2);
        a.on_operation(op(serde_json::json!({
            "revision": 3, "type": 25, "message": sent("4", "Ume", "Ua", "my reply")
        })));
        assert!(a.unread_divider.is_none());
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
                label: "Name",
                value: TextInput::default(),
                hint: "",
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

    #[test]
    fn ctrl_s_opens_settings_from_every_main_view() {
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

    #[test]
    fn settings_j_and_k_move_between_rows() {
        let mut a = app();
        a.open_settings_at(0);
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

        // The chat menu's "Pin lists…" opens the switcher; create a list from
        // it (it becomes active, empty).
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
            0,
            "read state persisted"
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
        assert_eq!(a.chats[0].unread, 0, "the open chat stays read");
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
        assert_eq!(a.readers("Cg", "500"), ["Ua", "Ub"]);
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
        key(&mut a, KeyCode::Enter);
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
    async fn message_keys_and_pasted_files() {
        let mut a = app();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter); // open "Ua"
        key(&mut a, KeyCode::Up); // select our message m2
        key(&mut a, KeyCode::Char('e'));
        let Some(Modal::Menu(m)) = &a.modal else {
            panic!("reaction menu")
        };
        assert_eq!(m.items.len(), 6);
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
            Box::new(|a| a.open_settings_at(0)),
            Box::new(|a| a.open_settings_at(2)),
            Box::new(|a| a.open_settings_at(4)),
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
        assert_eq!(a.focus, Focus::Input, "← no longer jumps to the chat list");
        key(&mut a, KeyCode::End);
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.input, "> ell");
        // A paste lands at the cursor too, as one line.
        key(&mut a, KeyCode::Home);
        a.on_paste("a\nb ");
        assert_eq!(a.input, "a b > ell");
        let screen = screen(&mut a);
        assert!(screen.contains("a b > ell"), "{screen}");
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
        assert!(!a.hint().contains("Ctrl-O"));
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
}
