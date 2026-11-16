//! Every key binding, per context, in one place: what `on_key` and the
//! overlays resolve keys with, and what the status-line hints, the help
//! screen, menus and messages show.
//!
//! The tables below are the defaults. Any binding can be changed in
//! Settings › Shortcuts ([`Keymap`] holds the changes, stored as
//! `setting.keys`); text never names a key itself but asks [`label`], so
//! what the screen says is always what the keys do. `--default-keys` runs
//! with the defaults. The shortcut editor's own keys (`Ctx::Keys`) and the
//! menus' numbers cannot be changed, so a mistake can always be undone.
//!
//! The rules the defaults follow (and changes are held to, where marked *):
//! - Global chords work in every view where they make sense, also while
//!   typing: `Ctrl-c` quits, `F1` shows help, `Ctrl-x` opens the main menu
//!   (everything that is not about one chat or message: search, new group,
//!   your profile, pin lists, read receipts, settings, plugins, quit),
//!   `Ctrl-s` opens settings, and `Ctrl-r` turns read receipts on or off,
//!   and with a chat open `Ctrl-o` (send something), `Ctrl-g` (chat menu)
//!   and `Ctrl-p` (chat profile).
//! - Navigation is never typed into the message line: the `/` commands act
//!   on the open chat; screens are opened by keys and menus.
//! - `?` shows help wherever printable keys are not text.
//! - Plain letters are only bound where nothing is typed (*), and only for
//!   things done often to the selected item. Everything else lives in the
//!   item's menu (`m`), which lists it by name.
//! - `Enter` confirms or opens, `Esc` goes back; consequential actions ask
//!   with labelled buttons.
//! - Text fields edit at a cursor (`←` `→` `Home` `End` `Delete`), and by
//!   word the way shells do (`Ctrl-←` `Ctrl-→` `Ctrl-w` `Ctrl-u` `Ctrl-k`).
//! - Lists jump to their first and last item with `Home` and `End`.
//! - `Alt-<letter or digit>` is never bound here (*): those keys belong to
//!   plugins (`is_plugin_key`), so a plugin can never shadow a rune key.
//! - One key means one thing wherever it answers (*: a change that would
//!   break this offers to move the key).
//! - On the message line `Enter` sends and `Shift-Enter` (or `Alt-Enter`,
//!   or `Ctrl-j` where the terminal cannot tell `Shift-Enter` apart) starts
//!   a new line; `↑` `↓` move between its lines.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ctx {
    ChatList,
    ChatSearch,
    Compose,
    Messages,
    Viewer,
    Profile,
    Menu,
    Confirm,
    Info,
    Help,
    Form,
    Pick,
    AttachBrowse,
    /// The file browser while a filter is typed: keys edit it.
    AttachFilter,
    AttachSend,
    Stickers,
    Emoji,
    Settings,
    /// One plugin's page (Settings › Plugins › a plugin).
    Plugins,
    /// A plugin's view: keys not bound here go to the plugin.
    PluginView,
    /// One screen's shortcuts (Settings › Shortcuts › a screen). Its keys
    /// cannot be changed.
    Keys,
    /// Search in a chat: typing is the query.
    Find,
}

impl Ctx {
    pub const ALL: [Ctx; 22] = [
        Ctx::ChatList,
        Ctx::ChatSearch,
        Ctx::Compose,
        Ctx::Messages,
        Ctx::Viewer,
        Ctx::Profile,
        Ctx::Menu,
        Ctx::Confirm,
        Ctx::Info,
        Ctx::Help,
        Ctx::Form,
        Ctx::Pick,
        Ctx::AttachBrowse,
        Ctx::AttachFilter,
        Ctx::AttachSend,
        Ctx::Stickers,
        Ctx::Emoji,
        Ctx::Settings,
        Ctx::Plugins,
        Ctx::PluginView,
        Ctx::Keys,
        Ctx::Find,
    ];

    /// Shown as the help screen's title.
    pub fn title(self) -> &'static str {
        match self {
            Ctx::ChatList => "Chat list",
            Ctx::ChatSearch => "Chat search",
            Ctx::Compose => "Message line",
            Ctx::Messages => "Messages",
            Ctx::Viewer => "Media viewer",
            Ctx::Profile => "Profile",
            Ctx::Menu => "Menu",
            Ctx::Confirm => "Confirmation",
            Ctx::Info => "Information",
            Ctx::Help => "Help",
            Ctx::Form => "Form",
            Ctx::Pick => "People picker",
            Ctx::AttachBrowse => "File browser",
            Ctx::AttachFilter => "File filter",
            Ctx::AttachSend => "Send dialog",
            Ctx::Stickers => "Stickers",
            Ctx::Emoji => "LINE emoji reactions",
            Ctx::Settings => "Settings",
            Ctx::Plugins => "Plugin page",
            Ctx::PluginView => "Plugin view",
            Ctx::Keys => "Shortcut editor",
            Ctx::Find => "Search in chat",
        }
    }

    /// Its name in stored shortcuts (never changes).
    fn id(self) -> &'static str {
        match self {
            Ctx::ChatList => "chat-list",
            Ctx::ChatSearch => "chat-search",
            Ctx::Compose => "message-line",
            Ctx::Messages => "messages",
            Ctx::Viewer => "viewer",
            Ctx::Profile => "profile",
            Ctx::Menu => "menu",
            Ctx::Confirm => "confirm",
            Ctx::Info => "info",
            Ctx::Help => "help",
            Ctx::Form => "form",
            Ctx::Pick => "people-picker",
            Ctx::AttachBrowse => "files",
            Ctx::AttachFilter => "file-filter",
            Ctx::AttachSend => "send-dialog",
            Ctx::Stickers => "stickers",
            Ctx::Emoji => "emoji",
            Ctx::Settings => "settings",
            Ctx::Plugins => "plugin-page",
            Ctx::PluginView => "plugin-view",
            Ctx::Keys => "shortcuts",
            Ctx::Find => "chat-search-messages",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cmd {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Confirm,
    Back,
    Search,
    Toggle,
    SwitchFocus,
    // Text editing.
    CursorLeft,
    CursorRight,
    LineStart,
    LineEnd,
    DeleteBackward,
    DeleteForward,
    WordLeft,
    WordRight,
    DeleteWordBackward,
    DeleteToLineStart,
    DeleteToLineEnd,
    NewLine,
    // Global.
    Help,
    MainMenu,
    Settings,
    Quit,
    ComposeMenu,
    ChatMenu,
    ChatProfile,
    ReadReceipts,
    // On the selected chat / message / media.
    Menu,
    Profile,
    NewGroup,
    PinUp,
    PinDown,
    NextUnread,
    Reply,
    React,
    Unsend,
    OpenLink,
    Save,
    OpenExternal,
    Copy,
    FindInChat,
    PlayPause,
    // Overlays.
    Yes,
    No,
    Digit(u8),
    NextTab,
    PrevTab,
    AsFile,
    OriginalQuality,
    ShowHidden,
    HomeDir,
    ParentDir,
    // The shortcut editor.
    AddKey,
    ResetKey,
}

impl Cmd {
    /// Its name in stored shortcuts (never changes).
    fn id(self) -> &'static str {
        match self {
            Cmd::Up => "up",
            Cmd::Down => "down",
            Cmd::Left => "left",
            Cmd::Right => "right",
            Cmd::PageUp => "page-up",
            Cmd::PageDown => "page-down",
            Cmd::Home => "home",
            Cmd::End => "end",
            Cmd::Confirm => "confirm",
            Cmd::Back => "back",
            Cmd::Search => "search",
            Cmd::Toggle => "toggle",
            Cmd::SwitchFocus => "switch-focus",
            Cmd::CursorLeft => "cursor-left",
            Cmd::CursorRight => "cursor-right",
            Cmd::LineStart => "line-start",
            Cmd::LineEnd => "line-end",
            Cmd::DeleteBackward => "delete-backward",
            Cmd::DeleteForward => "delete-forward",
            Cmd::WordLeft => "word-left",
            Cmd::WordRight => "word-right",
            Cmd::DeleteWordBackward => "delete-word-backward",
            Cmd::DeleteToLineStart => "delete-to-line-start",
            Cmd::DeleteToLineEnd => "delete-to-line-end",
            Cmd::NewLine => "new-line",
            Cmd::Help => "help",
            Cmd::MainMenu => "main-menu",
            Cmd::Settings => "settings",
            Cmd::Quit => "quit",
            Cmd::ComposeMenu => "compose-menu",
            Cmd::ChatMenu => "chat-menu",
            Cmd::ChatProfile => "chat-profile",
            Cmd::ReadReceipts => "read-receipts",
            Cmd::Menu => "menu",
            Cmd::Profile => "profile",
            Cmd::NewGroup => "new-group",
            Cmd::PinUp => "pin-up",
            Cmd::PinDown => "pin-down",
            Cmd::NextUnread => "next-unread",
            Cmd::Reply => "reply",
            Cmd::React => "react",
            Cmd::Unsend => "unsend",
            Cmd::OpenLink => "open-link",
            Cmd::Save => "save",
            Cmd::OpenExternal => "open-external",
            Cmd::Copy => "copy",
            Cmd::FindInChat => "find-in-chat",
            Cmd::PlayPause => "play-pause",
            Cmd::Yes => "yes",
            Cmd::No => "no",
            Cmd::Digit(_) => "digit",
            Cmd::NextTab => "next-tab",
            Cmd::PrevTab => "prev-tab",
            Cmd::AsFile => "as-file",
            Cmd::OriginalQuality => "original-quality",
            Cmd::ShowHidden => "show-hidden",
            Cmd::HomeDir => "home-dir",
            Cmd::ParentDir => "parent-dir",
            Cmd::AddKey => "add-key",
            Cmd::ResetKey => "reset-key",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    /// The key as bindings compare it: a character says Shift itself (`K`,
    /// `?`), and Shift-Tab is its own key whether or not the terminal adds
    /// Shift to it (crossterm does).
    pub fn normalized(self) -> Key {
        match self.code {
            KeyCode::Char(_) | KeyCode::BackTab => Key {
                code: self.code,
                mods: self.mods.difference(KeyModifiers::SHIFT),
            },
            _ => self,
        }
    }

    pub fn of(event: &KeyEvent) -> Key {
        Key {
            code: event.code,
            mods: event.modifiers,
        }
        .normalized()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub keys: &'static [Key],
    pub cmd: Cmd,
    pub help: &'static str,
    /// Listed in the status-line hint (when available).
    pub hint: bool,
}

const fn key(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::NONE,
    }
}

const fn ch(c: char) -> Key {
    key(KeyCode::Char(c))
}

const fn shift(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::SHIFT,
    }
}

const fn alt(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::ALT,
    }
}

const fn ctrl_key(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::CONTROL,
    }
}

const fn ctrl(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

macro_rules! binding {
    ($cmd:expr, $help:expr, $hint:expr; $($key:expr),+ $(,)?) => {
        Binding { keys: &[$($key),+], cmd: $cmd, help: $help, hint: $hint }
    };
}

use KeyCode::{
    BackTab, Backspace, Delete, Down, End, Enter, Esc, Home, Left, PageDown, PageUp, Right, Tab, Up,
};

static GLOBAL: &[Binding] = &[
    binding!(Cmd::Quit, "quit", false; ctrl('c')),
    binding!(Cmd::Help, "show the shortcuts", false; key(KeyCode::F(1))),
    binding!(Cmd::MainMenu, "menu", true; ctrl('x')),
    binding!(Cmd::Settings, "settings", false; ctrl('s')),
    binding!(Cmd::Search, "search chats and friends", false; ctrl('f')),
    binding!(Cmd::ComposeMenu, "attach a photo, file, sticker, location or contact", true; ctrl('o')),
    binding!(Cmd::ChatMenu, "the open chat's menu", false; ctrl('g')),
    binding!(Cmd::ChatProfile, "the open chat's profile", false; ctrl('p')),
    binding!(Cmd::ReadReceipts, "turn read receipts on / off", false; ctrl('r')),
];

/// `?` where printable keys are not text.
static QUESTION_HELP: &[Binding] =
    &[binding!(Cmd::Help, "show the shortcuts (not while typing)", false; ch('?'))];

/// Editing keys of every text field with a cursor.
static TEXT_EDIT: &[Binding] = &[
    binding!(Cmd::CursorLeft, "move the cursor left", false; key(Left)),
    binding!(Cmd::CursorRight, "move the cursor right", false; key(Right)),
    binding!(Cmd::LineStart, "start of the line", false; key(Home)),
    binding!(Cmd::LineEnd, "end of the line", false; key(End)),
    binding!(Cmd::DeleteBackward, "delete before the cursor", false; key(Backspace)),
    binding!(Cmd::DeleteForward, "delete after the cursor", false; key(Delete)),
    // Alt-← / Alt-→ is what macOS terminals send for Option-arrows.
    binding!(Cmd::WordLeft, "previous word", false; ctrl_key(Left), alt(Left)),
    binding!(Cmd::WordRight, "next word", false; ctrl_key(Right), alt(Right)),
    binding!(Cmd::DeleteWordBackward, "delete the word before the cursor", false; ctrl('w'), alt(Backspace)),
    binding!(Cmd::DeleteToLineStart, "delete to the start of the line", false; ctrl('u')),
    binding!(Cmd::DeleteToLineEnd, "delete to the end of the line", false; ctrl('k')),
];

static CHAT_LIST: &[Binding] = &[
    binding!(Cmd::Up, "previous chat", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next chat (at the end: load more)", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Home, "first chat", false; key(Home)),
    binding!(Cmd::End, "last chat", false; key(End)),
    binding!(Cmd::Confirm, "open (invitations: join or decline)", true; key(Enter), key(Right)),
    binding!(Cmd::SwitchFocus, "go to the open chat", false; key(Tab), key(BackTab)),
    binding!(Cmd::Search, "search chats and friends", true; ch('/')),
    binding!(Cmd::Menu, "chat menu", true; ch('m')),
    binding!(Cmd::Profile, "profile", true; ch('p')),
    binding!(Cmd::NewGroup, "new group", true; ch('n')),
    binding!(Cmd::PinUp, "move the pinned chat up", false; ch('K')),
    binding!(Cmd::PinDown, "move the pinned chat down", false; ch('J')),
    binding!(Cmd::NextUnread, "nearest unread chat", true; ch('u')),
];

static CHAT_SEARCH: &[Binding] = &[
    binding!(Cmd::Back, "stop searching", true; key(Esc)),
    binding!(Cmd::Confirm, "open the chat", true; key(Enter)),
    binding!(Cmd::Up, "previous match", true; key(Up)),
    binding!(Cmd::Down, "next match (at the end: search older chats)", false; key(Down)),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::SwitchFocus, "go to the open chat (the search stays until Esc)", false; key(Tab), key(BackTab)),
];

static COMPOSE: &[Binding] = &[
    binding!(Cmd::Confirm, "send", true; key(Enter)),
    // Ctrl-j is the line feed every terminal sends; Shift-Enter needs the
    // keyboard protocol or modifyOtherKeys, and some terminals keep Alt-Enter.
    binding!(Cmd::NewLine, "new line", true; shift(Enter), alt(Enter), ctrl('j')),
    binding!(Cmd::Back, "back (a reply is cancelled first)", true; key(Esc)),
    binding!(Cmd::Up, "previous line (on the first: select messages)", true; key(Up)),
    binding!(Cmd::Down, "next line", false; key(Down)),
    binding!(Cmd::PageUp, "scroll up", false; key(PageUp)),
    binding!(Cmd::PageDown, "scroll down", false; key(PageDown)),
    binding!(Cmd::SwitchFocus, "go to the chat list", false; key(Tab), key(BackTab)),
];

static MESSAGES: &[Binding] = &[
    binding!(Cmd::Up, "previous message (at the top: load older)", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next message (past the newest: type)", false; key(Down), ch('j')),
    binding!(Cmd::Left, "go to the chat list", false; key(Left)),
    binding!(Cmd::Right, "go to the message line", false; key(Right)),
    binding!(Cmd::PageUp, "scroll up", false; key(PageUp)),
    binding!(Cmd::PageDown, "scroll down", false; key(PageDown)),
    binding!(Cmd::Home, "oldest loaded message", false; key(Home)),
    binding!(Cmd::End, "newest message", false; key(End)),
    binding!(Cmd::SwitchFocus, "go to the chat list", false; key(Tab), key(BackTab)),
    binding!(Cmd::Confirm, "view / play / open the link", true; key(Enter)),
    binding!(Cmd::Reply, "reply", true; ch('r')),
    binding!(Cmd::React, "react", true; ch('e')),
    binding!(Cmd::Menu, "all actions", true; ch('m')),
    binding!(Cmd::Save, "save to ~/Downloads/rune", true; ch('s')),
    binding!(Cmd::OpenExternal, "open with the system app", false; ch('o')),
    binding!(Cmd::Copy, "copy the text", false; ch('c')),
    binding!(Cmd::FindInChat, "search this chat's messages", false; ch('/')),
    binding!(Cmd::Profile, "the sender's profile", false; ch('p')),
    binding!(Cmd::Unsend, "unsend (asks first)", false; ch('x')),
    binding!(Cmd::Back, "back to typing", false; key(Esc)),
];

static VIEWER: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Confirm, "play a video or audio clip, else close", false; key(Enter)),
    binding!(Cmd::PlayPause, "play / pause", true; ch(' ')),
    binding!(Cmd::Left, "previous", true; key(Left), ch('h')),
    binding!(Cmd::Right, "next", true; key(Right), ch('l')),
    binding!(Cmd::Save, "save to ~/Downloads/rune", true; ch('s')),
    binding!(Cmd::OpenExternal, "open with the system app", true; ch('o')),
    binding!(Cmd::OpenLink, "open the QR code's link", true; ch('u')),
    binding!(Cmd::Menu, "all actions", true; ch('m')),
];

static PROFILE: &[Binding] = &[
    binding!(Cmd::Back, "back (or close)", true; key(Esc)),
    binding!(Cmd::Left, "previous button", false; key(Left), ch('h')),
    binding!(Cmd::Right, "next button", false; key(Right), ch('l')),
    binding!(Cmd::Up, "previous member (above the first: the buttons)", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next member", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "a page of members up", false; key(PageUp)),
    binding!(Cmd::PageDown, "a page of members down", false; key(PageDown)),
    binding!(Cmd::Home, "the buttons", false; key(Home)),
    binding!(Cmd::End, "last member", false; key(End)),
    binding!(Cmd::Confirm, "press the button / open the member", true; key(Enter)),
];

static MENU: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Up, "previous item", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next item", false; key(Down), ch('j')),
    binding!(Cmd::Home, "first item", false; key(Home)),
    binding!(Cmd::End, "last item", false; key(End)),
    binding!(Cmd::Confirm, "choose", true; key(Enter)),
    binding!(Cmd::Digit(1), "choose by number", true; ch('1')),
    binding!(Cmd::Digit(2), "choose item 2", false; ch('2')),
    binding!(Cmd::Digit(3), "choose item 3", false; ch('3')),
    binding!(Cmd::Digit(4), "choose item 4", false; ch('4')),
    binding!(Cmd::Digit(5), "choose item 5", false; ch('5')),
    binding!(Cmd::Digit(6), "choose item 6", false; ch('6')),
    binding!(Cmd::Digit(7), "choose item 7", false; ch('7')),
    binding!(Cmd::Digit(8), "choose item 8", false; ch('8')),
    binding!(Cmd::Digit(9), "choose item 9", false; ch('9')),
];

static CONFIRM: &[Binding] = &[
    binding!(Cmd::Back, "cancel", true; key(Esc)),
    binding!(Cmd::Left, "highlight the left button", false; key(Left)),
    binding!(Cmd::Right, "highlight the right button", false; key(Right)),
    binding!(Cmd::SwitchFocus, "highlight the other button", false; key(Tab), key(BackTab)),
    binding!(Cmd::Confirm, "press the highlighted button", true; key(Enter)),
    binding!(Cmd::Yes, "yes", true; ch('y'), ch('Y')),
    binding!(Cmd::No, "no", true; ch('n'), ch('N')),
];

static INFO: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc), key(Enter)),
    binding!(Cmd::Up, "scroll up", false; key(Up), ch('k')),
    binding!(Cmd::Down, "scroll down", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Home, "top", false; key(Home)),
    binding!(Cmd::End, "bottom", false; key(End)),
];

static HELP: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc), key(Enter)),
    binding!(Cmd::Help, "close", false; ch('?')),
    binding!(Cmd::Up, "scroll up", false; key(Up), ch('k')),
    binding!(Cmd::Down, "scroll down", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Home, "top", false; key(Home)),
    binding!(Cmd::End, "bottom", false; key(End)),
];

static FORM: &[Binding] = &[
    binding!(Cmd::Back, "cancel", true; key(Esc)),
    binding!(Cmd::Confirm, "next field / submit", true; key(Enter)),
    binding!(Cmd::Down, "next field", true; key(Tab), key(Down)),
    binding!(Cmd::Up, "previous field", false; key(BackTab), key(Up)),
];

static PICK: &[Binding] = &[
    binding!(Cmd::Back, "cancel", true; key(Esc)),
    binding!(Cmd::Up, "previous person", false; key(Up)),
    binding!(Cmd::Down, "next person", false; key(Down)),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Toggle, "tick / untick", true; ch(' ')),
    binding!(Cmd::Confirm, "done (nobody ticked: the highlighted one)", true; key(Enter)),
];

static ATTACH_BROWSE: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Up, "previous file", false; key(Up)),
    binding!(Cmd::Down, "next file", false; key(Down)),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Home, "first file", false; key(Home)),
    binding!(Cmd::End, "last file", false; key(End)),
    binding!(Cmd::Confirm, "open the folder / choose the file", true; key(Enter)),
    binding!(Cmd::Right, "open the folder", false; key(Right)),
    binding!(Cmd::ParentDir, "parent folder", true; key(Left), key(Backspace)),
    binding!(Cmd::ShowHidden, "show / hide hidden files", true; ch('.')),
    binding!(Cmd::HomeDir, "home folder", true; ch('~')),
];

/// The file browser with a filter typed: arrows and Home/End edit it.
static ATTACH_FILTER: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Up, "previous file", false; key(Up)),
    binding!(Cmd::Down, "next file", false; key(Down)),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Confirm, "open the folder / choose the file / go to the typed path", true; key(Enter)),
];

static ATTACH_SEND: &[Binding] = &[
    binding!(Cmd::Back, "choose another file", true; key(Esc)),
    binding!(Cmd::Confirm, "send", true; key(Enter)),
    binding!(Cmd::AsFile, "send as a plain file (or not)", true; ch('f')),
    binding!(Cmd::OriginalQuality, "photo in original quality (or not)", true; ch('o')),
];

static STICKERS: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Confirm, "send the sticker", true; key(Enter)),
    binding!(Cmd::Left, "previous sticker", false; key(Left), ch('h')),
    binding!(Cmd::Right, "next sticker", false; key(Right), ch('l')),
    binding!(Cmd::Up, "row up", false; key(Up), ch('k')),
    binding!(Cmd::Down, "row down", false; key(Down), ch('j')),
    binding!(Cmd::NextTab, "next pack", true; key(Tab)),
    binding!(Cmd::PrevTab, "previous pack", false; key(BackTab)),
];

static EMOJI: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Confirm, "react with the emoji", true; key(Enter)),
    binding!(Cmd::Left, "previous emoji", false; key(Left), ch('h')),
    binding!(Cmd::Right, "next emoji", false; key(Right), ch('l')),
    binding!(Cmd::Up, "row up", false; key(Up), ch('k')),
    binding!(Cmd::Down, "row down", false; key(Down), ch('j')),
    binding!(Cmd::NextTab, "next pack", true; key(Tab)),
    binding!(Cmd::PrevTab, "previous pack", false; key(BackTab)),
];

static SETTINGS: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::NextTab, "next tab", true; key(Right), key(Tab)),
    binding!(Cmd::PrevTab, "previous tab", false; key(Left), key(BackTab)),
    binding!(Cmd::Up, "previous setting", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next setting", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Home, "first setting", false; key(Home)),
    binding!(Cmd::End, "last setting", false; key(End)),
    binding!(Cmd::Confirm, "change", true; key(Enter), ch(' ')),
];

static PLUGINS: &[Binding] = &[
    binding!(Cmd::Back, "back to Settings › Plugins", true; key(Esc)),
    binding!(Cmd::Up, "previous row", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next row", false; key(Down), ch('j')),
    binding!(Cmd::Home, "first row", false; key(Home)),
    binding!(Cmd::End, "last row", false; key(End)),
    binding!(Cmd::Confirm, "open / change", true; key(Enter), ch(' ')),
];

/// A plugin's view takes every other key itself (it lists them on its frame).
static PLUGIN_VIEW: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::PageUp, "scroll up", false; key(PageUp)),
    binding!(Cmd::PageDown, "scroll down", false; key(PageDown)),
];

/// Search in a chat: printable keys are the query.
static FIND: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Confirm, "show the message in the chat", true; key(Enter)),
    binding!(Cmd::Up, "newer match", false; key(Up)),
    binding!(Cmd::Down, "older match", false; key(Down)),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
];

/// The shortcut editor: fixed, so a shortcut changed by mistake can always
/// be changed back.
static KEYS: &[Binding] = &[
    binding!(Cmd::Back, "back", true; key(Esc)),
    binding!(Cmd::Up, "previous shortcut", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next shortcut", false; key(Down), ch('j')),
    binding!(Cmd::Home, "first shortcut", false; key(Home)),
    binding!(Cmd::End, "last shortcut", false; key(End)),
    binding!(Cmd::NextTab, "next screen", false; key(Right), key(Tab)),
    binding!(Cmd::PrevTab, "previous screen", false; key(Left), key(BackTab)),
    binding!(Cmd::Confirm, "change (then press the new key)", true; key(Enter)),
    binding!(Cmd::AddKey, "add a key", true; ch('a')),
    binding!(Cmd::ResetKey, "reset to the default", true; key(Delete), key(Backspace)),
];

/// A table of bindings: one context's own, or one several contexts share.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Table {
    Global,
    /// `?` for help where printable keys are not text.
    QuestionHelp,
    TextEdit,
    Own(Ctx),
}

impl Table {
    fn bindings(self) -> &'static [Binding] {
        match self {
            Table::Global => GLOBAL,
            Table::QuestionHelp => QUESTION_HELP,
            Table::TextEdit => TEXT_EDIT,
            Table::Own(ctx) => match ctx {
                Ctx::ChatList => CHAT_LIST,
                Ctx::ChatSearch => CHAT_SEARCH,
                Ctx::Compose => COMPOSE,
                Ctx::Messages => MESSAGES,
                Ctx::Viewer => VIEWER,
                Ctx::Profile => PROFILE,
                Ctx::Menu => MENU,
                Ctx::Confirm => CONFIRM,
                Ctx::Info => INFO,
                Ctx::Help => HELP,
                Ctx::Form => FORM,
                Ctx::Pick => PICK,
                Ctx::AttachBrowse => ATTACH_BROWSE,
                Ctx::AttachFilter => ATTACH_FILTER,
                Ctx::AttachSend => ATTACH_SEND,
                Ctx::Stickers => STICKERS,
                Ctx::Emoji => EMOJI,
                Ctx::Settings => SETTINGS,
                Ctx::Plugins => PLUGINS,
                Ctx::PluginView => PLUGIN_VIEW,
                Ctx::Keys => KEYS,
                Ctx::Find => FIND,
            },
        }
    }

    fn id(self) -> &'static str {
        match self {
            Table::Global => "global",
            Table::QuestionHelp => "help-key",
            Table::TextEdit => "text",
            Table::Own(ctx) => ctx.id(),
        }
    }

    /// Every table.
    fn all() -> impl Iterator<Item = Table> {
        [Table::Global, Table::QuestionHelp, Table::TextEdit]
            .into_iter()
            .chain(Ctx::ALL.into_iter().map(Table::Own))
    }

    /// Where the table's bindings are listed in Settings › Shortcuts.
    pub fn section_title(self) -> &'static str {
        SECTIONS
            .iter()
            .find(|s| s.tables.contains(&self))
            .map_or("Shortcut editor", |s| s.title)
    }
}

/// A screen of Settings › Shortcuts: the tables it lists.
pub struct Section {
    pub title: &'static str,
    pub tables: &'static [Table],
}

/// Every table whose keys can be changed, in the order Settings › Shortcuts
/// lists them (the shortcut editor's own is left out).
pub const SECTIONS: &[Section] = &[
    Section {
        title: "Everywhere",
        tables: &[Table::Global, Table::QuestionHelp],
    },
    Section {
        title: "Chat list",
        tables: &[Table::Own(Ctx::ChatList)],
    },
    Section {
        title: "Chat search",
        tables: &[Table::Own(Ctx::ChatSearch)],
    },
    Section {
        title: "Message line",
        tables: &[Table::Own(Ctx::Compose)],
    },
    Section {
        title: "Text fields",
        tables: &[Table::TextEdit],
    },
    Section {
        title: "Messages",
        tables: &[Table::Own(Ctx::Messages)],
    },
    Section {
        title: "Media viewer",
        tables: &[Table::Own(Ctx::Viewer)],
    },
    Section {
        title: "Profile",
        tables: &[Table::Own(Ctx::Profile)],
    },
    Section {
        title: "Menus",
        tables: &[Table::Own(Ctx::Menu)],
    },
    Section {
        title: "Confirmations",
        tables: &[Table::Own(Ctx::Confirm)],
    },
    Section {
        title: "Information boxes",
        tables: &[Table::Own(Ctx::Info)],
    },
    Section {
        title: "Help screen",
        tables: &[Table::Own(Ctx::Help)],
    },
    Section {
        title: "Forms",
        tables: &[Table::Own(Ctx::Form)],
    },
    Section {
        title: "People picker",
        tables: &[Table::Own(Ctx::Pick)],
    },
    Section {
        title: "File browser",
        tables: &[Table::Own(Ctx::AttachBrowse)],
    },
    Section {
        title: "File filter",
        tables: &[Table::Own(Ctx::AttachFilter)],
    },
    Section {
        title: "Send dialog",
        tables: &[Table::Own(Ctx::AttachSend)],
    },
    Section {
        title: "Stickers",
        tables: &[Table::Own(Ctx::Stickers)],
    },
    Section {
        title: "LINE emoji reactions",
        tables: &[Table::Own(Ctx::Emoji)],
    },
    Section {
        title: "Settings",
        tables: &[Table::Own(Ctx::Settings)],
    },
    Section {
        title: "Search in chat",
        tables: &[Table::Own(Ctx::Find)],
    },
    Section {
        title: "Plugin page",
        tables: &[Table::Own(Ctx::Plugins)],
    },
    Section {
        title: "Plugin view",
        tables: &[Table::Own(Ctx::PluginView)],
    },
];

impl Section {
    /// The bindings it lists (those that can be changed).
    pub fn entries(&self) -> Vec<(Table, &'static Binding)> {
        self.tables
            .iter()
            .flat_map(|&t| t.bindings().iter().map(move |b| (t, b)))
            .filter(|(t, b)| customizable(*t, b.cmd))
            .collect()
    }
}

/// The menus' numbers and the shortcut editor's keys stay as they are.
fn customizable(table: Table, cmd: Cmd) -> bool {
    table != Table::Own(Ctx::Keys) && !matches!(cmd, Cmd::Digit(_))
}

/// The tables answering in `ctx`, in the order they are asked: its own,
/// the shared ones it takes, the global chords last.
fn tables(ctx: Ctx) -> impl Iterator<Item = Table> {
    let edit = matches!(
        ctx,
        Ctx::Compose | Ctx::Form | Ctx::ChatSearch | Ctx::Pick | Ctx::AttachFilter | Ctx::Find
    );
    let question = !(is_typing(ctx) || ctx == Ctx::Help);
    [
        Some(Table::Own(ctx)),
        edit.then_some(Table::TextEdit),
        question.then_some(Table::QuestionHelp),
        Some(Table::Global),
    ]
    .into_iter()
    .flatten()
}

/// Every binding active in `ctx`, the context's first, globals last.
fn all(ctx: Ctx) -> impl Iterator<Item = (Table, &'static Binding)> {
    tables(ctx)
        .flat_map(|t| t.bindings().iter().map(move |b| (t, b)))
        .filter(move |(t, b)| *t != Table::Global || global_allowed(ctx, b.cmd))
}

/// The contexts where `cmd` of `table` answers.
fn contexts_of(table: Table, cmd: Cmd) -> impl Iterator<Item = Ctx> {
    Ctx::ALL.into_iter().filter(move |&ctx| {
        tables(ctx).any(|t| t == table) && (table != Table::Global || global_allowed(ctx, cmd))
    })
}

fn global_allowed(ctx: Ctx, cmd: Cmd) -> bool {
    use Ctx::*;
    let main_view = matches!(ctx, ChatList | ChatSearch | Compose | Messages | Viewer);
    match cmd {
        Cmd::Quit | Cmd::Help => true,
        Cmd::Settings | Cmd::MainMenu => main_view || ctx == Profile,
        Cmd::ComposeMenu | Cmd::ChatMenu | Cmd::ChatProfile | Cmd::ReadReceipts => main_view,
        // Not from the viewer: the chat list is behind it.
        Cmd::Search => main_view && ctx != Viewer,
        _ => false,
    }
}

fn binding(table: Table, cmd: Cmd) -> Option<&'static Binding> {
    table.bindings().iter().find(|b| b.cmd == cmd)
}

/// The binding's name in stored shortcuts and `keys.json`: `messages.reply`.
pub fn name(table: Table, cmd: Cmd) -> String {
    format!("{}.{}", table.id(), cmd.id())
}

/// What `cmd` of `table` does, as the help screen says it.
pub fn help(table: Table, cmd: Cmd) -> &'static str {
    binding(table, cmd).map_or("", |b| b.help)
}

/// Printable keys are text here.
pub fn is_typing(ctx: Ctx) -> bool {
    matches!(
        ctx,
        Ctx::ChatSearch
            | Ctx::Compose
            | Ctx::Form
            | Ctx::Pick
            | Ctx::AttachBrowse
            | Ctx::AttachFilter
            | Ctx::Find
    )
}

/// Whether typing `c` in `ctx` is text: everywhere keys are text, except
/// the file browser, where a letter or digit starts a filter and the rest
/// (`.`, `~`) are commands.
fn typed_there(ctx: Ctx, c: char) -> bool {
    is_typing(ctx) && (ctx != Ctx::AttachBrowse || c.is_alphanumeric())
}

/// The bindings as the user has them: the defaults with Settings ›
/// Shortcuts' changes on top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    /// Changed bindings and their keys (empty: no key at all).
    custom: HashMap<(Table, Cmd), Vec<Key>>,
}

impl Keymap {
    /// The keys `b` of `table` answers to.
    fn keys_for(&self, table: Table, b: &'static Binding) -> &[Key] {
        self.custom.get(&(table, b.cmd)).map_or(b.keys, Vec::as_slice)
    }

    /// The keys of `cmd` in `table` (none if the table has no such command).
    pub fn keys(&self, table: Table, cmd: Cmd) -> &[Key] {
        binding(table, cmd).map_or(&[], |b| self.keys_for(table, b))
    }

    pub fn is_custom(&self, table: Table, cmd: Cmd) -> bool {
        self.custom.contains_key(&(table, cmd))
    }

    pub fn is_default(&self) -> bool {
        self.custom.is_empty()
    }

    /// How many bindings are changed.
    pub fn changed(&self) -> usize {
        self.custom.len()
    }

    /// How many of `section`'s bindings are changed.
    pub fn changed_in(&self, section: &Section) -> usize {
        section
            .entries()
            .iter()
            .filter(|(t, b)| self.is_custom(*t, b.cmd))
            .count()
    }

    pub fn resolve(&self, ctx: Ctx, event: &KeyEvent) -> Option<Cmd> {
        let pressed = Key::of(event);
        all(ctx)
            .find(|&(t, b)| self.keys_for(t, b).iter().any(|k| k.normalized() == pressed))
            .map(|(_, b)| b.cmd)
    }

    /// The keys of what `cmd` does in `ctx`, as the screen names them
    /// (empty when nothing there does it).
    pub fn label(&self, ctx: Ctx, cmd: Cmd) -> String {
        all(ctx)
            .find(|(_, b)| b.cmd == cmd)
            .map(|(t, b)| display_keys(self.keys_for(t, b)))
            .unwrap_or_default()
    }

    /// The hinted bindings of `ctx` whose command `available` allows right
    /// now (the main menu first: everything else can be found from it).
    pub fn hint(&self, ctx: Ctx, available: impl Fn(Cmd) -> bool) -> String {
        match held() {
            m if m.is_empty() => self.plain_hint(ctx, available),
            m => self.held_hint(ctx, m, available),
        }
    }

    /// While `mods` are held: every binding of `ctx` whose key uses all of
    /// them (see [`mods_used`]), whether hinted normally or not.
    pub fn held_hint(&self, ctx: Ctx, mods: KeyModifiers, available: impl Fn(Cmd) -> bool) -> String {
        held_text(mods, self.held_entries(ctx, mods, available))
    }

    /// The entries of [`Keymap::held_hint`] (`"Ctrl-t reply"`), in table order.
    pub fn held_entries(&self, ctx: Ctx, mods: KeyModifiers, available: impl Fn(Cmd) -> bool) -> Vec<String> {
        let mut entries: Vec<String> = Vec::new();
        for (t, b) in all(ctx) {
            if !available(b.cmd) {
                continue;
            }
            let keys: Vec<Key> = self
                .keys_for(t, b)
                .iter()
                .copied()
                .filter(|&k| mods_used(k).contains(mods))
                // A key an earlier binding takes (a stored clash) does not do this.
                .filter(|k| self.resolve(ctx, &KeyEvent::new(k.code, k.mods)) == Some(b.cmd))
                .collect();
            if keys.is_empty() {
                continue;
            }
            let entry = format!("{} {}", display_keys(&keys), b.help);
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        }
        entries
    }

    fn plain_hint(&self, ctx: Ctx, available: impl Fn(Cmd) -> bool) -> String {
        let mut shown: Vec<(Table, &Binding)> = all(ctx)
            .filter(|&(t, b)| b.hint && available(b.cmd) && !self.keys_for(t, b).is_empty())
            .collect();
        shown.sort_by_key(|(_, b)| b.cmd != Cmd::MainMenu);
        shown
            .into_iter()
            .map(|(t, b)| {
                let keys = self.keys_for(t, b);
                let keys = match b.cmd {
                    Cmd::Digit(_) => "1…9".to_owned(),
                    // The full list is on the help screen. Shift-Enter once the
                    // terminal is known to tell it from Enter; until then Ctrl-j,
                    // which always arrives (Alt-Enter is left out: some terminals,
                    // WezTerm among them, keep it for themselves).
                    Cmd::NewLine if keys == b.keys && shift_enter() => display_key(shift(Enter)),
                    Cmd::NewLine if keys == b.keys => display_key(ctrl('j')),
                    _ => display_keys(keys),
                };
                format!("{keys} {}", b.help)
            })
            .collect::<Vec<_>>()
            .join(HINT_SEP)
    }

    /// How to get help in `ctx`, for the start of the status line.
    pub fn help_key(&self, ctx: Ctx) -> String {
        match self.label(ctx, Cmd::Help) {
            keys if keys.is_empty() => String::new(),
            keys => format!("{keys} help"),
        }
    }

    /// Every binding of `ctx`, for the help screen (unbound ones say so).
    pub fn help_lines(&self, ctx: Ctx) -> Vec<(String, &'static str)> {
        let mut lines: Vec<(String, &'static str)> = Vec::new();
        for (t, b) in all(ctx) {
            // The numbered menu items share one line.
            if matches!(b.cmd, Cmd::Digit(2..)) {
                continue;
            }
            let keys = match b.cmd {
                Cmd::Digit(_) => "1…9".to_owned(),
                _ => match self.keys_for(t, b) {
                    [] => "—".to_owned(),
                    keys => display_keys(keys),
                },
            };
            lines.push((keys, b.help));
        }
        if ctx == Ctx::Compose {
            lines.extend(COMMANDS.iter().map(|&(cmd, what)| (cmd.to_owned(), what)));
        }
        lines
    }

    /// The other bindings that already answer to `key` somewhere `cmd` of
    /// `table` does: binding it there takes the key from them.
    pub fn conflicts(&self, table: Table, cmd: Cmd, key: Key) -> Vec<(Table, Cmd)> {
        let key = key.normalized();
        let mut out = Vec::new();
        for ctx in contexts_of(table, cmd) {
            for (t, b) in all(ctx) {
                if (t, b.cmd) != (table, cmd)
                    && self.keys_for(t, b).iter().any(|k| k.normalized() == key)
                    && !out.contains(&(t, b.cmd))
                {
                    out.push((t, b.cmd));
                }
            }
        }
        out
    }

    /// The keys of `cmd` in `table` that something else also answers to
    /// where it does (only possible through stored changes made before a
    /// default changed, or edited by hand): the first `(key, table, cmd)`.
    pub fn clash(&self, table: Table, cmd: Cmd) -> Option<(Key, Table, Cmd)> {
        self.keys(table, cmd).iter().find_map(|&k| {
            self.conflicts(table, cmd, k)
                .first()
                .map(|&(t, c)| (k.normalized(), t, c))
        })
    }

    /// Why `key` cannot be bound to `cmd` of `table` as things are, if it cannot:
    /// a rule ([`refusal`]) or a binding that cannot give the key up.
    pub fn refusal(&self, table: Table, cmd: Cmd, key: Key) -> Option<String> {
        if let Some(why) = refusal(table, cmd, key) {
            return Some(why);
        }
        self.conflicts(table, cmd, key)
            .into_iter()
            .find(|&(t, c)| !customizable(t, c))
            .map(|(t, c)| {
                let what = binding(t, c).map_or("", |b| b.help);
                format!(
                    "{} cannot be used: it is “{what}” in the {}",
                    display_key(key.normalized()),
                    t.section_title().to_lowercase()
                )
            })
    }

    /// Give `cmd` of `table` exactly `keys` (normalized, without repeats).
    pub fn set(&mut self, table: Table, cmd: Cmd, keys: Vec<Key>) {
        let Some(b) = binding(table, cmd) else { return };
        let mut wanted: Vec<Key> = Vec::new();
        for k in keys.into_iter().map(Key::normalized) {
            if !wanted.contains(&k) {
                wanted.push(k);
            }
        }
        let defaults: Vec<Key> = b.keys.iter().map(|k| k.normalized()).collect();
        let same = wanted.len() == defaults.len() && wanted.iter().all(|k| defaults.contains(k));
        if same {
            self.custom.remove(&(table, cmd));
        } else {
            self.custom.insert((table, cmd), wanted);
        }
    }

    /// Bind `key` to `cmd` of `table` too.
    pub fn add(&mut self, table: Table, cmd: Cmd, key: Key) {
        let mut keys = self.keys(table, cmd).to_vec();
        keys.push(key);
        self.set(table, cmd, keys);
    }

    /// Unbind `key` from `cmd` of `table`.
    pub fn take(&mut self, table: Table, cmd: Cmd, key: Key) {
        let key = key.normalized();
        let keys = self
            .keys(table, cmd)
            .iter()
            .copied()
            .filter(|k| k.normalized() != key)
            .collect();
        self.set(table, cmd, keys);
    }

    /// Back to the default keys.
    pub fn reset(&mut self, table: Table, cmd: Cmd) {
        self.custom.remove(&(table, cmd));
    }

    /// Stored as `setting.keys`: `{"messages.reply": ["ctrl-t"], …}`.
    pub fn to_json(&self) -> String {
        let mut entries: Vec<(String, Vec<String>)> = self
            .custom
            .iter()
            .map(|((t, c), keys)| {
                let names = keys.iter().filter_map(|k| key_name(*k)).collect();
                (format!("{}.{}", t.id(), c.id()), names)
            })
            .collect();
        entries.sort();
        let map: serde_json::Map<String, serde_json::Value> = entries
            .into_iter()
            .map(|(id, keys)| (id, serde_json::json!(keys)))
            .collect();
        serde_json::Value::Object(map).to_string()
    }

    /// Read what [`to_json`](Self::to_json) wrote, leaving out (and naming)
    /// what is unknown, unreadable or against the rules.
    pub fn from_json(json: &str) -> (Keymap, Vec<String>) {
        Keymap::parse(json).unwrap_or_else(|e| {
            (
                Keymap::default(),
                vec![format!("custom shortcuts unreadable: {e}")],
            )
        })
    }

    /// [`from_json`](Self::from_json), but `Err` when the whole text is not
    /// a JSON object (a file that is mid-edit is not taken as "no changes").
    /// Entries named `_…` are comments.
    pub fn parse(json: &str) -> Result<(Keymap, Vec<String>), String> {
        let mut map = Keymap::default();
        let mut problems = Vec::new();
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let entries = value.as_object().ok_or("not a JSON object")?;
        for (id, keys) in entries.iter().filter(|(id, _)| !id.starts_with('_')) {
            let found = Table::all().find_map(|t| {
                t.bindings()
                    .iter()
                    .find(|b| format!("{}.{}", t.id(), b.cmd.id()) == *id)
                    .map(|b| (t, b.cmd))
            });
            let Some((table, cmd)) = found.filter(|&(t, c)| customizable(t, c)) else {
                problems.push(format!("unknown shortcut {id} (ignored)"));
                continue;
            };
            let Some(names) = keys.as_array() else {
                problems.push(format!("{id}: not a list of keys (ignored)"));
                continue;
            };
            let mut parsed = Vec::new();
            for name in names {
                match name.as_str().and_then(parse_key) {
                    Some(k) => match refusal(table, cmd, k) {
                        None => parsed.push(k),
                        Some(why) => problems.push(format!("{id}: {why} (ignored)")),
                    },
                    None => problems.push(format!("{id}: cannot read key {name} (ignored)")),
                }
            }
            map.set(table, cmd, parsed);
        }
        Ok((map, problems))
    }

    /// The changes as a file to read and edit by hand (`keys.json`): what
    /// [`to_json`](Self::to_json) holds, one entry per line, under a note.
    pub fn to_file_json(&self) -> String {
        let mut value: serde_json::Value = serde_json::from_str(&self.to_json()).unwrap_or_default();
        if let Some(entries) = value.as_object_mut() {
            entries.insert("_about".into(), serde_json::json!(FILE_NOTE));
        }
        let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// The note at the top of `keys.json`.
const FILE_NOTE: &str = "rune's keyboard shortcuts that differ from the defaults: \"screen.command\": [keys]. \
     Settings › Shortcuts shows each one's name. Keys: ctrl-x, alt-enter, shift-tab, f1, pgup, \
     space, up, K, ? … ([] for none). Read when rune starts (or with Settings › Shortcuts › \
     Read keys.json again); rune rewrites this file when you change a shortcut there.";

/// Why `key` can never be bound to `cmd` of `table`, if it cannot: keys
/// that are not keys, plugin keys, and characters where they are text.
pub fn refusal(table: Table, cmd: Cmd, key: Key) -> Option<String> {
    let key = key.normalized();
    let name = display_key(key);
    if !bindable(key.code) {
        return Some(format!("{name} cannot be used as a shortcut"));
    }
    if is_plugin_key(&KeyEvent::new(key.code, key.mods)).is_some() {
        return Some(format!("{name} is kept for plugins (Alt with a letter or digit)"));
    }
    if let KeyCode::Char(c) = key.code
        && !key.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && let Some(ctx) = contexts_of(table, cmd).find(|&ctx| typed_there(ctx, c))
    {
        return Some(format!(
            "{name} is typed as text in the {}",
            ctx.title().to_lowercase()
        ));
    }
    None
}

fn bindable(code: KeyCode) -> bool {
    match code {
        KeyCode::Char(c) => !c.is_control(),
        KeyCode::F(n) => (1..=24).contains(&n),
        Enter
        | Esc
        | Tab
        | BackTab
        | Backspace
        | Delete
        | Home
        | End
        | PageUp
        | PageDown
        | Up
        | Down
        | Left
        | Right
        | KeyCode::Insert => true,
        _ => false,
    }
}

thread_local! {
    /// The keymap the UI (which runs on one thread) answers keys with.
    static ACTIVE: RefCell<Rc<Keymap>> = RefCell::new(Rc::new(Keymap::default()));
}

thread_local! {
    /// The modifiers (Ctrl, Alt, Shift) held down on their own right now, as far as the
    /// terminal reports it: hints then list what those modifiers reach.
    static HELD: std::cell::Cell<KeyModifiers> = const { std::cell::Cell::new(KeyModifiers::NONE) };
    /// Whether the terminal reports Shift-Enter apart from Enter (it speaks the
    /// kitty keyboard protocol, or a Shift-Enter has arrived through
    /// modifyOtherKeys). Until then the hints name `Ctrl-j`, which always works.
    static SHIFT_ENTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether the terminal reports Ctrl, Alt and Shift pressed on their own
    /// (the kitty keyboard protocol), which the held-key hints need.
    static LONE_MODIFIERS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Note whether the terminal reports modifiers pressed on their own.
pub fn set_lone_modifiers(reported: bool) {
    LONE_MODIFIERS.with(|s| s.set(reported));
}

pub fn lone_modifiers() -> bool {
    LONE_MODIFIERS.with(|s| s.get())
}

/// Note whether the terminal tells Shift-Enter apart from Enter.
pub fn set_shift_enter(known: bool) {
    SHIFT_ENTER.with(|s| s.set(known));
}

pub fn shift_enter() -> bool {
    SHIFT_ENTER.with(|s| s.get())
}

/// Ctrl and Alt: the modifiers that make a key a chord rather than typing.
pub const CHORD_MODS: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT);

/// The modifiers bindings use, so holding one reveals its shortcuts.
pub const HELD_MODS: KeyModifiers = CHORD_MODS.union(KeyModifiers::SHIFT);

/// The modifiers pressing `key` takes: its own, plus Shift for a key that says
/// Shift itself (a capital letter, Shift-Tab). Shifted symbols (`?`) depend on
/// the keyboard layout and are left out.
pub fn mods_used(key: Key) -> KeyModifiers {
    let mut mods = key.mods & HELD_MODS;
    match key.code {
        KeyCode::BackTab => mods |= KeyModifiers::SHIFT,
        KeyCode::Char(c) if c.is_uppercase() => mods |= KeyModifiers::SHIFT,
        _ => {}
    }
    mods
}

/// Note which modifiers are held (Ctrl, Alt and Shift count).
pub fn set_held(mods: KeyModifiers) {
    HELD.with(|h| h.set(mods & HELD_MODS));
}

pub fn held() -> KeyModifiers {
    HELD.with(|h| h.get())
}

/// See [`Keymap::held_entries`].
pub fn held_entries(ctx: Ctx, mods: KeyModifiers, available: impl Fn(Cmd) -> bool) -> Vec<String> {
    active().held_entries(ctx, mods, available)
}

/// The hint while `mods` are held: `entries`, or that there are none.
pub fn held_text(mods: KeyModifiers, entries: Vec<String>) -> String {
    if !entries.is_empty() {
        return entries.join(HINT_SEP);
    }
    let name: Vec<&str> = [
        (KeyModifiers::CONTROL, "Ctrl"),
        (KeyModifiers::ALT, "Alt"),
        (KeyModifiers::SHIFT, "Shift"),
    ]
    .into_iter()
    .filter(|&(m, _)| mods.contains(m))
    .map(|(_, n)| n)
    .collect();
    format!("no {} shortcuts here", name.join("-"))
}

/// Answer keys with `map` from now on.
pub fn install(map: Keymap) {
    ACTIVE.with(|a| *a.borrow_mut() = Rc::new(map));
}

/// The keymap keys are answered with.
pub fn active() -> Rc<Keymap> {
    ACTIVE.with(|a| a.borrow().clone())
}

pub fn resolve(ctx: Ctx, event: &KeyEvent) -> Option<Cmd> {
    active().resolve(ctx, event)
}

/// The first key of what `cmd` does in `ctx`, for a menu or a sentence
/// (empty when it has none).
pub fn shortcut(ctx: Ctx, cmd: Cmd) -> String {
    let map = active();
    all(ctx)
        .find(|(_, b)| b.cmd == cmd)
        .and_then(|(t, b)| map.keys_for(t, b).first().copied())
        .map(display_key)
        .unwrap_or_default()
}

/// A global chord's first key (empty when it has none).
pub fn global(cmd: Cmd) -> String {
    active()
        .keys(Table::Global, cmd)
        .first()
        .map(|&k| display_key(k))
        .unwrap_or_default()
}

/// `"{key} {what}"`, or just `what` when `cmd` has no key in `ctx`.
pub fn with_keys(ctx: Ctx, cmd: Cmd, what: &str) -> String {
    match shortcut(ctx, cmd) {
        key if key.is_empty() => what.to_owned(),
        key => format!("{key} {what}"),
    }
}

/// Where the main menu's item `item` is, for a sentence: `Ctrl-x › Plugins`.
pub fn via_menu(item: &str) -> String {
    match global(Cmd::MainMenu) {
        key if key.is_empty() => format!("main menu › {item}"),
        key => format!("{key} › {item}"),
    }
}

/// How to get help in `ctx`: the status line starts with it, so it is never cut off.
pub fn help_key(ctx: Ctx) -> String {
    active().help_key(ctx)
}

/// See [`Keymap::hint`].
pub fn hint(ctx: Ctx, available: impl Fn(Cmd) -> bool) -> String {
    active().hint(ctx, available)
}

/// See [`Keymap::help_lines`].
pub fn help_lines(ctx: Ctx) -> Vec<(String, &'static str)> {
    active().help_lines(ctx)
}

/// Separates the entries of a hint.
pub const HINT_SEP: &str = " · ";

/// `hint` (entries joined by [`HINT_SEP`], optionally padded with spaces) made to
/// fit `width` columns: whole entries are dropped from the end and a final `…`
/// says more are on the help screen (left out when only the first entry fits);
/// the first entry (how to get help) is never dropped, only cut when it alone
/// does not fit.
pub fn fit_hint(hint: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if hint.width() <= width {
        return hint.to_owned();
    }
    // Keep the padding only while there is room for it.
    let inner = hint.trim();
    let (lead, trail) = if hint.starts_with(' ') && hint.ends_with(' ') && width >= inner.width().min(8) + 2 {
        (" ", " ")
    } else {
        ("", "")
    };
    let room = width.saturating_sub(lead.len() + trail.len());
    let more = format!("{HINT_SEP}…");
    let mut out = String::new();
    for (i, part) in inner.split(HINT_SEP).enumerate() {
        let candidate = if i == 0 {
            part.to_owned()
        } else {
            format!("{out}{HINT_SEP}{part}")
        };
        // Something always follows an entry that is not the last: leave room for the `…`.
        if candidate.width() + more.width() > room {
            break;
        }
        out = candidate;
    }
    let first = inner.split(HINT_SEP).next().unwrap_or("");
    if out.is_empty() && first.width() <= room {
        // Only the first entry fits: it goes alone.
        out = first.to_owned();
    } else if out.is_empty() {
        // Not even the first entry fits: cut it.
        if room == 0 {
            return String::new();
        }
        let mut w = 0;
        for ch in inner.chars() {
            let cw = ch.width().unwrap_or(0);
            if w + cw + 1 > room {
                break;
            }
            out.push(ch);
            w += cw;
        }
        out.push('…');
    } else {
        out.push_str(&more);
    }
    format!("{lead}{out}{trail}")
}

/// The message line's commands (typed and sent with `Enter`), listed after
/// its keys on the help screen. They act on the open chat; screens
/// (settings, plugins, help, …) are opened with keys and the main menu.
const COMMANDS: &[(&str, &str)] = &[
    (
        "/attach [PATH]",
        "send a photo, video or file (also /image, /file)",
    ),
    ("/sticker", "send a sticker"),
    ("/location", "send a location"),
    ("/contact", "share a contact"),
    ("/save PATH", "save the chat's newest media to PATH"),
    ("/mute, /unmute", "turn the chat's notifications off / on"),
    ("/invite, /rename", "invite to / rename the group"),
    ("/albums, /leave", "the group's albums / leave the group"),
    ("//text", "send \"/text\" as it is"),
];

/// The `/name`s the message line handles itself (see `App::submit`):
/// plugins cannot take them.
pub const CORE_COMMANDS: &[&str] = &[
    "save", "attach", "image", "file", "send", "sticker", "stickers", "location", "contact", "invite",
    "leave", "rename", "albums", "album", "mute", "unmute",
];

/// Whether plugins may bind `Alt-<c>` (rune never does).
pub fn is_plugin_key(event: &KeyEvent) -> Option<char> {
    match event.code {
        KeyCode::Char(c) if event.modifiers == KeyModifiers::ALT => {
            let c = c.to_ascii_lowercase();
            (c.is_ascii_lowercase() || c.is_ascii_digit()).then_some(c)
        }
        _ => None,
    }
}

pub fn display_keys(keys: &[Key]) -> String {
    let mut shown: Vec<String> = Vec::new();
    for k in keys {
        let label = display_key(*k);
        // `y/Y`, `n/N`: one is enough.
        if !shown.iter().any(|s| s.eq_ignore_ascii_case(&label)) {
            shown.push(label);
        }
    }
    shown.join("/")
}

pub fn display_key(key: Key) -> String {
    let mut label = String::new();
    if key.mods.contains(KeyModifiers::CONTROL) {
        label.push_str("Ctrl-");
        if key.mods.contains(KeyModifiers::ALT) {
            label.push_str("Alt-");
        }
        if let KeyCode::Char(c) = key.code {
            // Ctrl takes letters without case; a capital one came with Shift.
            if c.is_ascii_uppercase() {
                label.push_str("Shift-");
            }
            label.push(c.to_ascii_lowercase());
            return label;
        }
    } else if key.mods.contains(KeyModifiers::ALT) {
        label.push_str("Alt-");
    }
    // A character says Shift itself (`K`, `?`).
    if key.mods.contains(KeyModifiers::SHIFT) && !matches!(key.code, KeyCode::Char(_) | BackTab) {
        label.push_str("Shift-");
    }
    label.push_str(&match key.code {
        Backspace => "Backspace".into(),
        BackTab => "Shift-Tab".into(),
        Delete => "Delete".into(),
        Down => "↓".into(),
        End => "End".into(),
        Enter => "Enter".into(),
        Esc => "Esc".into(),
        KeyCode::F(n) => format!("F{n}"),
        Home => "Home".into(),
        KeyCode::Insert => "Insert".into(),
        Left => "←".into(),
        PageDown => "PgDn".into(),
        PageUp => "PgUp".into(),
        Right => "→".into(),
        Tab => "Tab".into(),
        Up => "↑".into(),
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    });
    label
}

/// A key's name in stored shortcuts: `ctrl-x`, `shift-enter`, `alt-left`,
/// `f1`, `space`, `K`, `?` (`None` for keys that cannot be bound).
pub fn key_name(key: Key) -> Option<String> {
    let key = key.normalized();
    let code = match key.code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(c) if !c.is_control() => c.to_string(),
        KeyCode::F(n) if (1..=24).contains(&n) => format!("f{n}"),
        Enter => "enter".into(),
        Esc => "esc".into(),
        Tab => "tab".into(),
        BackTab => "backtab".into(),
        Backspace => "backspace".into(),
        Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        Home => "home".into(),
        End => "end".into(),
        PageUp => "pgup".into(),
        PageDown => "pgdn".into(),
        Up => "up".into(),
        Down => "down".into(),
        Left => "left".into(),
        Right => "right".into(),
        _ => return None,
    };
    let mut name = String::new();
    for (m, prefix) in [
        (KeyModifiers::CONTROL, "ctrl-"),
        (KeyModifiers::ALT, "alt-"),
        (KeyModifiers::SHIFT, "shift-"),
    ] {
        if key.mods.contains(m) {
            name.push_str(prefix);
        }
    }
    name.push_str(&code);
    Some(name)
}

/// Read a [`key_name`] (modifier names in any case).
pub fn parse_key(name: &str) -> Option<Key> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = name;
    loop {
        let lower = rest.to_ascii_lowercase();
        let found = [
            ("ctrl-", KeyModifiers::CONTROL),
            ("alt-", KeyModifiers::ALT),
            ("shift-", KeyModifiers::SHIFT),
        ]
        .into_iter()
        .find(|(p, _)| lower.starts_with(p) && rest.len() > p.len());
        match found {
            Some((p, m)) => {
                mods |= m;
                rest = &rest[p.len()..];
            }
            None => break,
        }
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => match rest.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" => Enter,
            "esc" => Esc,
            "tab" => Tab,
            "backtab" => BackTab,
            "backspace" => Backspace,
            "delete" => Delete,
            "insert" => KeyCode::Insert,
            "home" => Home,
            "end" => End,
            "pgup" => PageUp,
            "pgdn" => PageDown,
            "up" => Up,
            "down" => Down,
            "left" => Left,
            "right" => Right,
            f => KeyCode::F(
                f.strip_prefix('f')?
                    .parse()
                    .ok()
                    .filter(|n| (1..=24).contains(n))?,
            ),
        },
    };
    bindable(code).then_some(Key { code, mods }.normalized())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        event(code, KeyModifiers::NONE)
    }

    #[test]
    fn every_key_means_one_thing_per_context_and_is_documented() {
        for ctx in Ctx::ALL {
            let mut keys = Vec::new();
            for (_, binding) in all(ctx) {
                assert!(!binding.help.is_empty(), "empty help text in {ctx:?}");
                for &key in binding.keys {
                    let key = key.normalized();
                    assert!(!keys.contains(&key), "{key:?} is bound twice in {ctx:?}");
                    keys.push(key);
                }
            }
            assert!(!help_lines(ctx).is_empty());
            assert!(!ctx.title().is_empty());
        }
    }

    #[test]
    fn no_command_has_two_unrelated_keys_of_the_same_kind() {
        // Alternatives are allowed only as arrow + vi letter, Tab + Shift-Tab,
        // or a key plus its capital; never two chords or two letters for one action.
        for ctx in Ctx::ALL {
            for (_, b) in all(ctx) {
                let chords = b
                    .keys
                    .iter()
                    .filter(|k| k.mods.contains(KeyModifiers::CONTROL))
                    .count();
                assert!(chords <= 1, "{:?} has {chords} chords in {ctx:?}", b.cmd);
                let letters: Vec<char> = b
                    .keys
                    .iter()
                    .filter_map(|k| match k.code {
                        KeyCode::Char(c) if c.is_alphabetic() && k.mods.is_empty() => {
                            Some(c.to_ascii_lowercase())
                        }
                        _ => None,
                    })
                    .collect();
                assert!(
                    letters.windows(2).all(|w| w[0] == w[1]),
                    "{:?} has several letters in {ctx:?}",
                    b.cmd
                );
            }
        }
    }

    #[test]
    fn printable_keys_are_text_while_typing() {
        for ctx in [Ctx::ChatSearch, Ctx::Compose, Ctx::Form, Ctx::Find] {
            for c in ' '..='~' {
                assert_eq!(resolve(ctx, &plain(KeyCode::Char(c))), None, "{ctx:?} {c:?}");
            }
        }
        for c in ' '..='~' {
            let pick = resolve(Ctx::Pick, &plain(KeyCode::Char(c)));
            assert_eq!(pick, (c == ' ').then_some(Cmd::Toggle), "{c:?}");
            let browse = resolve(Ctx::AttachBrowse, &plain(KeyCode::Char(c)));
            assert_eq!(browse.is_some(), matches!(c, '.' | '~'), "{c:?}");
        }
    }

    #[test]
    fn text_fields_edit_at_the_cursor() {
        // Every text field edits with the same keys.
        for ctx in [
            Ctx::Compose,
            Ctx::Form,
            Ctx::ChatSearch,
            Ctx::Pick,
            Ctx::AttachFilter,
            Ctx::Find,
        ] {
            assert_eq!(resolve(ctx, &plain(KeyCode::Left)), Some(Cmd::CursorLeft));
            assert_eq!(resolve(ctx, &plain(KeyCode::Right)), Some(Cmd::CursorRight));
            assert_eq!(resolve(ctx, &plain(KeyCode::Home)), Some(Cmd::LineStart));
            assert_eq!(resolve(ctx, &plain(KeyCode::End)), Some(Cmd::LineEnd));
            assert_eq!(
                resolve(ctx, &plain(KeyCode::Backspace)),
                Some(Cmd::DeleteBackward)
            );
            assert_eq!(resolve(ctx, &plain(KeyCode::Delete)), Some(Cmd::DeleteForward));
        }
        // An empty file filter leaves the arrows and Backspace to browsing.
        assert_eq!(
            resolve(Ctx::AttachBrowse, &plain(KeyCode::Left)),
            Some(Cmd::ParentDir)
        );
        assert_eq!(
            resolve(Ctx::AttachBrowse, &plain(KeyCode::Backspace)),
            Some(Cmd::ParentDir)
        );
        assert_eq!(resolve(Ctx::AttachBrowse, &plain(KeyCode::Home)), Some(Cmd::Home));
        // Outside text, ← and → move between panes.
        assert_eq!(resolve(Ctx::Messages, &plain(KeyCode::Left)), Some(Cmd::Left));
    }

    #[test]
    fn enter_sends_and_modified_enter_starts_a_line() {
        let enter = |mods| resolve(Ctx::Compose, &event(KeyCode::Enter, mods));
        assert_eq!(enter(KeyModifiers::NONE), Some(Cmd::Confirm));
        assert_eq!(enter(KeyModifiers::SHIFT), Some(Cmd::NewLine));
        assert_eq!(enter(KeyModifiers::ALT), Some(Cmd::NewLine));
        assert_eq!(
            resolve(Ctx::Compose, &event(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Some(Cmd::NewLine)
        );
        assert_eq!(resolve(Ctx::Compose, &plain(KeyCode::Down)), Some(Cmd::Down));
        // Only the message line takes several lines.
        assert_eq!(
            resolve(Ctx::Form, &event(KeyCode::Enter, KeyModifiers::SHIFT)),
            None
        );
        // The hint names a key the terminal can send.
        set_shift_enter(false);
        let legacy = hint(Ctx::Compose, |_| true);
        assert!(legacy.contains("Enter send · Ctrl-j new line"), "{legacy}");
        set_shift_enter(true);
        let full = hint(Ctx::Compose, |_| true);
        set_shift_enter(false);
        assert!(full.contains("Enter send · Shift-Enter new line"), "{full}");
        assert!(
            help_lines(Ctx::Compose)
                .iter()
                .any(|(k, _)| k == "Shift-Enter/Alt-Enter/Ctrl-j"),
            "{:?}",
            help_lines(Ctx::Compose)
        );
    }

    #[test]
    fn shift_is_ignored_for_character_bindings() {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            assert_eq!(
                resolve(Ctx::ChatList, &event(KeyCode::Char('K'), modifiers)),
                Some(Cmd::PinUp)
            );
            assert_eq!(
                resolve(Ctx::ChatList, &event(KeyCode::Char('?'), modifiers)),
                Some(Cmd::Help)
            );
        }
    }

    #[test]
    fn global_chords_work_while_typing_but_not_in_overlays() {
        let chord = |c| event(KeyCode::Char(c), KeyModifiers::CONTROL);
        for (c, cmd) in [
            ('s', Cmd::Settings),
            ('x', Cmd::MainMenu),
            ('o', Cmd::ComposeMenu),
            ('g', Cmd::ChatMenu),
            ('p', Cmd::ChatProfile),
            ('r', Cmd::ReadReceipts),
        ] {
            assert_eq!(resolve(Ctx::Compose, &chord(c)), Some(cmd));
            assert_eq!(resolve(Ctx::Menu, &chord(c)), None);
        }
        assert_eq!(resolve(Ctx::Profile, &chord('s')), Some(Cmd::Settings));
        assert_eq!(resolve(Ctx::Profile, &chord('x')), Some(Cmd::MainMenu));
        assert_eq!(resolve(Ctx::Settings, &chord('x')), None);
        // Settings has one key only.
        assert_eq!(resolve(Ctx::ChatList, &plain(KeyCode::Char('S'))), None);
        assert_eq!(resolve(Ctx::Compose, &chord(',')), None);
    }

    #[test]
    fn quit_and_help_work_everywhere_and_plain_q_nowhere() {
        for ctx in Ctx::ALL {
            let ctrl_c = event(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert_eq!(resolve(ctx, &ctrl_c), Some(Cmd::Quit));
            assert_eq!(resolve(ctx, &plain(KeyCode::F(1))), Some(Cmd::Help));
            assert_eq!(resolve(ctx, &plain(KeyCode::Char('q'))), None, "{ctx:?}");
            let question = resolve(ctx, &plain(KeyCode::Char('?')));
            assert_eq!(question == Some(Cmd::Help), !is_typing(ctx), "{ctx:?}");
        }
    }

    #[test]
    fn fitted_hints_keep_whole_entries_and_the_help_key() {
        use unicode_width::UnicodeWidthStr;
        let hint = "? help · Enter open · Tab focus · / search";
        assert_eq!(fit_hint(hint, 80), hint);
        assert_eq!(fit_hint(hint, hint.width()), hint);
        assert_eq!(fit_hint(hint, 30), "? help · Enter open · …");
        assert_eq!(fit_hint(hint, 10), "? help · …");
        // Too narrow for anything but a shortened help key.
        assert_eq!(fit_hint(hint, 5), "? he…");
        assert_eq!(fit_hint(hint, 0), "");
        // Frame titles keep their padding while it fits.
        assert_eq!(fit_hint(" a · bb · cc ", 10), " a · … ");
        for ctx in [
            Ctx::ChatList,
            Ctx::Messages,
            Ctx::Compose,
            Ctx::Viewer,
            Ctx::Profile,
        ] {
            let full = format!("{}{HINT_SEP}{}", help_key(ctx), hint_text(ctx));
            for w in 0..=full.width() + 2 {
                let fitted = fit_hint(&full, w);
                assert!(fitted.width() <= w, "{ctx:?} at {w}: {fitted:?}");
                if w >= help_key(ctx).width() + 4 {
                    assert!(fitted.starts_with(&help_key(ctx)), "{ctx:?} at {w}: {fitted:?}");
                }
                // Never an entry cut in the middle.
                for part in fitted.trim_end_matches('…').split(HINT_SEP) {
                    let part = part.trim_end_matches(" ·");
                    assert!(
                        part.is_empty()
                            || full.split(HINT_SEP).any(|p| p == part)
                            || w < help_key(ctx).width() + 4,
                        "{ctx:?} at {w}: {fitted:?}"
                    );
                }
            }
        }
    }

    fn hint_text(ctx: Ctx) -> String {
        hint(ctx, |_| true)
    }

    #[test]
    fn hints_skip_unavailable_commands() {
        let all_on = hint(Ctx::Messages, |_| true);
        assert!(all_on.contains("r reply"), "{all_on}");
        assert!(hint(Ctx::Messages, |_| false).is_empty());
        assert!(hint(Ctx::Menu, |_| true).contains("1…9 choose by number"));
        assert_eq!(help_key(Ctx::Compose), "F1 help");
        assert_eq!(help_key(Ctx::Messages), "? help");
        assert_eq!(display_key(ctrl('o')), "Ctrl-o");
    }

    #[test]
    fn alt_letters_and_digits_are_left_to_plugins() {
        for ctx in Ctx::ALL {
            for c in ('a'..='z').chain('0'..='9') {
                let alt = event(KeyCode::Char(c), KeyModifiers::ALT);
                assert_eq!(resolve(ctx, &alt), None, "{ctx:?} binds Alt-{c}");
                assert_eq!(is_plugin_key(&alt), Some(c));
            }
        }
        assert_eq!(
            is_plugin_key(&event(KeyCode::Char('S'), KeyModifiers::ALT)),
            Some('s')
        );
        assert_eq!(
            is_plugin_key(&event(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(is_plugin_key(&event(KeyCode::Enter, KeyModifiers::ALT)), None);
    }

    #[test]
    fn every_listed_command_is_reserved_from_plugins() {
        for (usage, _) in COMMANDS {
            for name in usage.split(", ").filter_map(|c| c.strip_prefix('/')) {
                let name = name.split_whitespace().next().unwrap();
                if !name.is_empty() && !name.starts_with('/') {
                    assert!(CORE_COMMANDS.contains(&name), "/{name} is not in CORE_COMMANDS");
                }
            }
        }
    }

    #[test]
    fn the_main_menu_is_hinted_first_in_every_main_view() {
        for ctx in [
            Ctx::ChatList,
            Ctx::ChatSearch,
            Ctx::Compose,
            Ctx::Messages,
            Ctx::Viewer,
        ] {
            assert!(hint(ctx, |_| true).starts_with("Ctrl-x menu"), "{ctx:?}");
        }
        assert!(!hint(Ctx::Menu, |_| true).contains("Ctrl-x"));
    }

    #[test]
    fn screens_are_not_message_line_commands() {
        for name in [
            "settings", "plugins", "help", "quit", "group", "newgroup", "menu", "join", "flex",
        ] {
            assert!(!CORE_COMMANDS.contains(&name), "/{name} is not a command");
            assert!(
                !COMMANDS
                    .iter()
                    .any(|(usage, _)| usage.contains(&format!("/{name}"))),
                "/{name} is still listed as a command"
            );
        }
    }

    #[test]
    fn help_lists_numbered_items_once_and_capitals_once() {
        let menu = help_lines(Ctx::Menu);
        assert_eq!(menu.iter().filter(|(k, _)| k == "1…9").count(), 1);
        assert!(!menu.iter().any(|(k, _)| k == "2"));
        let confirm = help_lines(Ctx::Confirm);
        assert!(confirm.iter().any(|(k, _)| k == "y"));
    }

    #[test]
    fn shift_tab_matches_whether_or_not_the_terminal_adds_shift() {
        // crossterm reports Shift-Tab as BackTab with Shift.
        for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let back_tab = event(KeyCode::BackTab, mods);
            assert_eq!(resolve(Ctx::ChatList, &back_tab), Some(Cmd::SwitchFocus));
            assert_eq!(resolve(Ctx::Settings, &back_tab), Some(Cmd::PrevTab));
            assert_eq!(resolve(Ctx::Form, &back_tab), Some(Cmd::Up));
            assert_eq!(resolve(Ctx::Stickers, &back_tab), Some(Cmd::PrevTab));
        }
        assert_eq!(
            display_key(Key::of(&event(KeyCode::BackTab, KeyModifiers::SHIFT))),
            "Shift-Tab"
        );
    }

    #[test]
    fn key_names_round_trip() {
        for t in Table::all() {
            for b in t.bindings() {
                for &k in b.keys {
                    let name = key_name(k).unwrap_or_else(|| panic!("{k:?} has no name"));
                    assert_eq!(parse_key(&name), Some(k.normalized()), "{name}");
                }
            }
        }
        assert_eq!(parse_key("ctrl--"), Some(ctrl('-')));
        assert_eq!(parse_key("CTRL-x"), Some(ctrl('x')));
        assert_eq!(parse_key("Shift-Enter"), Some(shift(Enter)));
        assert_eq!(parse_key("f12"), Some(key(KeyCode::F(12))));
        assert_eq!(parse_key("shift-K"), Some(ch('K')));
        assert_eq!(parse_key("space"), Some(ch(' ')));
        assert_eq!(parse_key("f"), Some(ch('f')));
        for bad in ["", "ctrl-", "f25", "f0", "foo", "ctrl-foo", "\u{7}"] {
            assert_eq!(parse_key(bad), None, "{bad:?}");
        }
        assert_eq!(key_name(key(KeyCode::Null)), None);
    }

    #[test]
    fn changed_keys_answer_and_the_old_ones_do_not() {
        let mut map = Keymap::default();
        let reply = (Table::Own(Ctx::Messages), Cmd::Reply);
        map.set(reply.0, reply.1, vec![ctrl('t')]);
        assert!(map.is_custom(reply.0, reply.1));
        let ctrl_t = event(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(map.resolve(Ctx::Messages, &ctrl_t), Some(Cmd::Reply));
        assert_eq!(map.resolve(Ctx::Messages, &plain(KeyCode::Char('r'))), None);
        // Only there: the chat list never had it.
        assert_eq!(map.resolve(Ctx::ChatList, &ctrl_t), None);
        assert_eq!(map.label(Ctx::Messages, Cmd::Reply), "Ctrl-t");
        assert!(map.hint(Ctx::Messages, |_| true).contains("Ctrl-t reply"));
        assert!(
            map.help_lines(Ctx::Messages)
                .contains(&("Ctrl-t".into(), "reply"))
        );
        // Giving it its default keys back is no change at all.
        map.set(reply.0, reply.1, vec![ch('r')]);
        assert!(map.is_default());
        // Several keys, repeats dropped, Shift on characters ignored.
        map.add(reply.0, reply.1, ctrl('t'));
        map.add(reply.0, reply.1, ctrl('t'));
        assert_eq!(map.keys(reply.0, reply.1), [ch('r'), ctrl('t')]);
        map.take(reply.0, reply.1, ch('r'));
        assert_eq!(map.keys(reply.0, reply.1), [ctrl('t')]);
        // No key at all: unhinted, and the help screen says so.
        map.take(reply.0, reply.1, ctrl('t'));
        assert!(map.keys(reply.0, reply.1).is_empty());
        assert!(!map.hint(Ctx::Messages, |_| true).contains("reply"));
        assert!(map.help_lines(Ctx::Messages).contains(&("—".into(), "reply")));
        map.reset(reply.0, reply.1);
        assert!(map.is_default());
        // A changed new-line key is hinted as it is.
        map.set(Table::Own(Ctx::Compose), Cmd::NewLine, vec![ctrl('n')]);
        assert!(map.hint(Ctx::Compose, |_| true).contains("Ctrl-n new line"));
    }

    #[test]
    fn conflicts_are_found_wherever_both_answer() {
        let map = Keymap::default();
        let messages = Table::Own(Ctx::Messages);
        assert_eq!(
            map.conflicts(messages, Cmd::Reply, ch('e')),
            [(messages, Cmd::React)]
        );
        assert!(map.conflicts(messages, Cmd::Reply, ctrl('t')).is_empty());
        // A key of the chat list only is free in the messages.
        assert!(
            map.conflicts(Table::Own(Ctx::ChatList), Cmd::Menu, ch('r'))
                .is_empty()
        );
        // Global chords answer while typing: the text keys are in the way.
        assert!(
            map.conflicts(Table::Global, Cmd::Search, ctrl('u'))
                .contains(&(Table::TextEdit, Cmd::DeleteToLineStart))
        );
        assert!(
            map.conflicts(Table::Global, Cmd::MainMenu, ctrl('w'))
                .contains(&(Table::TextEdit, Cmd::DeleteWordBackward))
        );
        // ...and other chords.
        assert_eq!(
            map.conflicts(Table::Global, Cmd::Settings, ctrl('r')),
            [(Table::Global, Cmd::ReadReceipts)]
        );
        // `?` answers wherever keys are not text.
        assert!(
            map.conflicts(Table::Own(Ctx::Settings), Cmd::Up, ch('?'))
                .contains(&(Table::QuestionHelp, Cmd::Help))
        );
        // Shift does not make a character another key.
        assert_eq!(
            map.conflicts(
                Table::Own(Ctx::ChatList),
                Cmd::NewGroup,
                shift(KeyCode::Char('K'))
            ),
            [(Table::Own(Ctx::ChatList), Cmd::PinUp)]
        );
        assert_eq!(map.clash(messages, Cmd::Reply), None);
        // Stored changes can clash (a default changed since): it is found.
        let mut map = map;
        map.set(messages, Cmd::Reply, vec![ch('e')]);
        assert_eq!(
            map.clash(messages, Cmd::Reply),
            Some((ch('e'), messages, Cmd::React))
        );
        assert_eq!(
            map.clash(messages, Cmd::React),
            Some((ch('e'), messages, Cmd::Reply))
        );
    }

    #[test]
    fn rules_refuse_plugin_keys_text_and_what_cannot_move() {
        let map = Keymap::default();
        let messages = Table::Own(Ctx::Messages);
        assert!(
            refusal(messages, Cmd::Reply, alt(KeyCode::Char('r')))
                .unwrap()
                .contains("plugins")
        );
        assert!(refusal(messages, Cmd::Reply, alt(KeyCode::Char('7'))).is_some());
        assert_eq!(refusal(messages, Cmd::Reply, alt(Enter)), None);
        assert_eq!(refusal(messages, Cmd::Reply, ch('t')), None);
        assert!(
            refusal(Table::Own(Ctx::Compose), Cmd::Back, ch('q'))
                .unwrap()
                .contains("typed as text in the message line")
        );
        // Global chords and text keys answer while typing.
        assert!(refusal(Table::Global, Cmd::Search, ch('/')).is_some());
        assert!(refusal(Table::TextEdit, Cmd::WordLeft, ch('b')).is_some());
        assert_eq!(refusal(Table::Global, Cmd::Search, ctrl('/')), None);
        // The file browser types letters and digits only.
        let files = Table::Own(Ctx::AttachBrowse);
        assert!(refusal(files, Cmd::ShowHidden, ch('h')).is_some());
        assert_eq!(refusal(files, Cmd::ShowHidden, ch('!')), None);
        assert!(refusal(Table::TextEdit, Cmd::WordLeft, key(KeyCode::Null)).is_some());
        assert!(refusal(messages, Cmd::Reply, key(KeyCode::CapsLock)).is_some());
        // Menu numbers and the shortcut editor's keys stay.
        assert!(
            map.refusal(Table::Own(Ctx::Menu), Cmd::Up, ch('1'))
                .unwrap()
                .contains("menus")
        );
        assert!(map.refusal(Table::Global, Cmd::Quit, key(Delete)).is_some());
        assert_eq!(
            map.refusal(messages, Cmd::Reply, ch('e')),
            None,
            "React can give it up"
        );
    }

    #[test]
    fn stored_shortcuts_round_trip_and_bad_entries_are_dropped() {
        let mut map = Keymap::default();
        map.set(
            Table::Own(Ctx::Messages),
            Cmd::Reply,
            vec![ctrl('t'), key(KeyCode::F(5))],
        );
        map.set(Table::Global, Cmd::MainMenu, vec![ctrl('b')]);
        map.set(Table::Own(Ctx::ChatList), Cmd::PinUp, vec![]);
        map.set(Table::QuestionHelp, Cmd::Help, vec![ch('h')]);
        let json = map.to_json();
        assert!(json.contains(r#""messages.reply":["ctrl-t","f5"]"#), "{json}");
        assert!(json.contains(r#""chat-list.pin-up":[]"#), "{json}");
        assert_eq!(Keymap::from_json(&json), (map, vec![]));
        let (read, problems) = Keymap::from_json(
            r#"{"messages.reply": ["ctrl-t", "bogus", "alt-a"], "messages.fly": ["f"],
                "menu.digit": ["x"], "shortcuts.add-key": ["b"], "global.quit": "ctrl-q",
                "message-line.back": ["q"]}"#,
        );
        assert_eq!(read.keys(Table::Own(Ctx::Messages), Cmd::Reply), [ctrl('t')]);
        assert_eq!(read.keys(Table::Own(Ctx::Compose), Cmd::Back), &[] as &[Key]);
        assert!(
            read.keys(Table::Global, Cmd::Quit) == [ctrl('c')],
            "a bad entry keeps the default"
        );
        assert_eq!(problems.len(), 7, "{problems:?}");
        assert!(!Keymap::from_json("[1]").1.is_empty());
        assert!(
            Keymap::parse("{\"messages.reply\": [").is_err(),
            "mid-edit is not empty"
        );
        // The file form reads back the same, its note ignored.
        let mut map = Keymap::default();
        map.set(Table::Own(Ctx::Messages), Cmd::Reply, vec![ctrl('t')]);
        let file = map.to_file_json();
        assert!(file.starts_with("{\n  \"_about\""), "{file}");
        assert!(
            file.contains("\n  \"messages.reply\": [\n    \"ctrl-t\"\n  ]"),
            "{file}"
        );
        assert_eq!(Keymap::parse(&file), Ok((map, vec![])));
        assert!(!Keymap::from_json("{").1.is_empty());
        assert_eq!(Keymap::from_json("{}"), (Keymap::default(), vec![]));
    }

    #[test]
    fn every_changeable_binding_is_listed_once_under_a_unique_name() {
        let mut ids = Vec::new();
        for t in Table::all() {
            for b in t.bindings().iter().filter(|b| customizable(t, b.cmd)) {
                let id = format!("{}.{}", t.id(), b.cmd.id());
                assert!(!ids.contains(&id), "{id} twice");
                ids.push(id);
                let listed = SECTIONS
                    .iter()
                    .filter(|s| s.entries().iter().any(|(st, sb)| *st == t && sb.cmd == b.cmd))
                    .count();
                assert_eq!(listed, 1, "{t:?} {:?} listed {listed} times", b.cmd);
            }
        }
        assert!(
            !SECTIONS.iter().any(|s| s.tables.contains(&Table::Own(Ctx::Keys))),
            "the editor's own keys are fixed"
        );
        assert!(SECTIONS.iter().all(|s| !s.entries().is_empty()));
    }

    #[test]
    fn text_names_the_keys_the_installed_keymap_has() {
        assert_eq!(global(Cmd::MainMenu), "Ctrl-x");
        assert_eq!(with_keys(Ctx::Compose, Cmd::Back, "cancel"), "Esc cancel");
        let mut map = Keymap::default();
        map.set(Table::Global, Cmd::MainMenu, vec![ctrl('b')]);
        map.set(Table::Own(Ctx::Compose), Cmd::Back, vec![]);
        map.set(Table::QuestionHelp, Cmd::Help, vec![ch('h')]);
        install(map);
        assert_eq!(global(Cmd::MainMenu), "Ctrl-b");
        assert_eq!(with_keys(Ctx::Compose, Cmd::Back, "cancel"), "cancel");
        assert_eq!(help_key(Ctx::Messages), "h help");
        assert!(hint(Ctx::ChatList, |_| true).starts_with("Ctrl-b menu"));
        assert_eq!(
            resolve(Ctx::Compose, &event(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Cmd::MainMenu)
        );
        install(Keymap::default());
        assert_eq!(global(Cmd::MainMenu), "Ctrl-x");
    }
}

#[cfg(test)]
mod held_tests {
    use super::*;

    #[test]
    fn holding_a_modifier_lists_what_it_reaches() {
        let map = Keymap::default();
        let ctrl = map.held_hint(Ctx::Compose, KeyModifiers::CONTROL, |_| true);
        assert!(!ctrl.is_empty());
        for entry in ctrl.split(HINT_SEP) {
            assert!(entry.starts_with("Ctrl-"), "{entry}");
        }
        set_held(KeyModifiers::CONTROL);
        assert_eq!(map.hint(Ctx::Compose, |_| true), ctrl);
        set_held(KeyModifiers::SUPER);
        assert_eq!(held(), KeyModifiers::NONE, "bindings never use Super");
        assert_eq!(
            map.hint(Ctx::Compose, |_| true),
            map.plain_hint(Ctx::Compose, |_| true)
        );

        // Shift: keys that name it, and capital letters, which say it themselves.
        let shift = map.held_hint(Ctx::Compose, KeyModifiers::SHIFT, |_| true);
        assert!(shift.contains("Shift-Enter"), "{shift}");
        let messages = map.held_hint(Ctx::Messages, KeyModifiers::SHIFT, |_| true);
        for entry in shift.split(HINT_SEP).chain(messages.split(HINT_SEP)) {
            let keys = entry.split(' ').next().unwrap();
            assert!(
                keys.split('/')
                    .all(|k| k.contains("Shift-") || k.chars().any(char::is_uppercase)),
                "{entry}"
            );
        }
    }

    #[test]
    fn held_modifiers_reach_every_key_that_uses_them() {
        assert_eq!(
            mods_used(Key::of(&KeyEvent::from(KeyCode::Char('K')))),
            KeyModifiers::SHIFT
        );
        assert_eq!(
            mods_used(Key::of(&KeyEvent::from(KeyCode::BackTab))),
            KeyModifiers::SHIFT
        );
        assert_eq!(
            mods_used(Key::of(&KeyEvent::from(KeyCode::Char('?')))),
            KeyModifiers::NONE
        );
        let ctrl_shift_x = Key {
            code: KeyCode::Char('X'),
            mods: KeyModifiers::CONTROL,
        };
        assert_eq!(
            mods_used(ctrl_shift_x),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        );

        // Holding Ctrl reaches Ctrl-Shift keys too; holding both, only those.
        let mut map = Keymap::default();
        let reply = (Table::Own(Ctx::Messages), Cmd::Reply);
        map.set(reply.0, reply.1, vec![ctrl_shift_x]);
        assert!(
            map.held_hint(Ctx::Messages, KeyModifiers::CONTROL, |_| true)
                .contains("Ctrl-Shift-x reply")
        );
        let both = map.held_hint(Ctx::Messages, KeyModifiers::CONTROL | KeyModifiers::SHIFT, |_| {
            true
        });
        assert!(both.contains("Ctrl-Shift-x reply"), "{both}");
        assert!(!both.contains("Ctrl-x "), "{both}");
        // A stored clash: the key goes to the binding found first, so only it lists the key.
        let mut clash = Keymap::default();
        let first = all(Ctx::Messages)
            .find(|(t, _)| *t == Table::Own(Ctx::Messages))
            .unwrap();
        let quit_keys = clash.keys(Table::Global, Cmd::Quit).to_vec();
        clash.set(Table::Own(Ctx::Messages), first.1.cmd, quit_keys.clone());
        let ctrl = clash.held_hint(Ctx::Messages, KeyModifiers::CONTROL, |_| true);
        let quit = display_keys(&quit_keys);
        assert!(ctrl.contains(&format!("{quit} {}", first.1.help)), "{ctrl}");
        assert!(!ctrl.contains(&format!("{quit} quit")), "{ctrl}");

        assert_eq!(
            held_text(KeyModifiers::CONTROL | KeyModifiers::SHIFT, vec![]),
            "no Ctrl-Shift shortcuts here"
        );
    }
}
