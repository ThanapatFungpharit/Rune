//! Every key binding, per context, in one place: what `on_key` and the
//! overlays resolve keys with, and what the status-line hints and the help
//! screen show.
//!
//! The rules the tables follow:
//! - Global chords work in every view where they make sense, also while
//!   typing: `Ctrl-C` quits, `F1` shows help, `Ctrl-S` opens settings, and
//!   with a chat open `Ctrl-O` (send something), `Ctrl-G` (chat menu) and
//!   `Ctrl-P` (chat profile).
//! - `?` shows help wherever printable keys are not text.
//! - Plain letters are only bound where nothing is typed, and only for
//!   things done often to the selected item. Everything else lives in the
//!   item's menu (`m`), which lists it by name.
//! - `Enter` confirms or opens, `Esc` goes back; consequential actions ask
//!   with labelled buttons.
//! - Text fields edit at a cursor (`←` `→` `Home` `End` `Delete`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    AttachSend,
    Stickers,
    Settings,
}

impl Ctx {
    #[cfg(test)]
    pub const ALL: [Ctx; 16] = [
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
        Ctx::AttachSend,
        Ctx::Stickers,
        Ctx::Settings,
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
            Ctx::AttachSend => "Send dialog",
            Ctx::Stickers => "Stickers",
            Ctx::Settings => "Settings",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    // Global.
    Help,
    Settings,
    Quit,
    ComposeMenu,
    ChatMenu,
    ChatProfile,
    // On the selected chat / message / media.
    Menu,
    Profile,
    NewGroup,
    PinUp,
    PinDown,
    Reply,
    React,
    Unsend,
    OpenLink,
    Save,
    OpenExternal,
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
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

pub const GLOBAL: &[Binding] = &[
    binding!(Cmd::Quit, "quit", false; ctrl('c')),
    binding!(Cmd::Help, "show the shortcuts", false; key(KeyCode::F(1))),
    binding!(Cmd::Settings, "settings", false; ctrl('s')),
    binding!(Cmd::ComposeMenu, "attach a photo, file, sticker, location or contact", true; ctrl('o')),
    binding!(Cmd::ChatMenu, "the open chat's menu", false; ctrl('g')),
    binding!(Cmd::ChatProfile, "the open chat's profile", false; ctrl('p')),
];

/// `?` where printable keys are not text.
static QUESTION_HELP: &[Binding] = &[binding!(Cmd::Help, "show the shortcuts", false; ch('?'))];

/// Editing keys of every text field with a cursor.
static TEXT_EDIT: &[Binding] = &[
    binding!(Cmd::CursorLeft, "move the cursor left", false; key(Left)),
    binding!(Cmd::CursorRight, "move the cursor right", false; key(Right)),
    binding!(Cmd::LineStart, "start of the line", false; key(Home)),
    binding!(Cmd::LineEnd, "end of the line", false; key(End)),
    binding!(Cmd::DeleteBackward, "delete before the cursor", false; key(Backspace)),
    binding!(Cmd::DeleteForward, "delete after the cursor", false; key(Delete)),
];

static CHAT_LIST: &[Binding] = &[
    binding!(Cmd::Up, "previous chat", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next chat (at the end: load more)", false; key(Down), ch('j')),
    binding!(Cmd::PageUp, "page up", false; key(PageUp)),
    binding!(Cmd::PageDown, "page down", false; key(PageDown)),
    binding!(Cmd::Confirm, "open (invitations: join or decline)", true; key(Enter), key(Right)),
    binding!(Cmd::SwitchFocus, "go to the open chat", false; key(Tab), key(BackTab)),
    binding!(Cmd::Search, "search", true; ch('/')),
    binding!(Cmd::Menu, "menu", true; ch('m')),
    binding!(Cmd::Profile, "profile", true; ch('p')),
    binding!(Cmd::NewGroup, "new group", true; ch('n')),
    binding!(Cmd::PinUp, "move the pinned chat up", false; ch('K')),
    binding!(Cmd::PinDown, "move the pinned chat down", false; ch('J')),
];

static CHAT_SEARCH: &[Binding] = &[
    binding!(Cmd::Back, "stop searching", true; key(Esc)),
    binding!(Cmd::Confirm, "open the chat", true; key(Enter)),
    binding!(Cmd::Up, "previous match", true; key(Up)),
    binding!(Cmd::Down, "next match", true; key(Down)),
    binding!(Cmd::SwitchFocus, "go to the open chat", false; key(Tab), key(BackTab)),
    binding!(Cmd::DeleteBackward, "delete a character", false; key(Backspace)),
];

static COMPOSE: &[Binding] = &[
    binding!(Cmd::Confirm, "send", true; key(Enter)),
    binding!(Cmd::Back, "back (a reply is cancelled first)", true; key(Esc)),
    binding!(Cmd::Up, "select messages", true; key(Up)),
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
    binding!(Cmd::SwitchFocus, "go to the chat list", false; key(Tab), key(BackTab)),
    binding!(Cmd::Confirm, "view / play / open the link", true; key(Enter)),
    binding!(Cmd::Reply, "reply", true; ch('r')),
    binding!(Cmd::React, "react", true; ch('e')),
    binding!(Cmd::Menu, "all actions", true; ch('m')),
    binding!(Cmd::Save, "save to ~/Downloads/rune", true; ch('s')),
    binding!(Cmd::OpenExternal, "open with the system app", false; ch('o')),
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
    binding!(Cmd::Confirm, "press the button / open the member", true; key(Enter)),
];

static MENU: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::Up, "previous item", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next item", false; key(Down), ch('j')),
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
    binding!(Cmd::Toggle, "tick / untick", true; ch(' ')),
    binding!(Cmd::Confirm, "done (nobody ticked: the highlighted one)", true; key(Enter)),
    binding!(Cmd::DeleteBackward, "delete a filter character", false; key(Backspace)),
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
    binding!(Cmd::ParentDir, "parent folder", true; key(Left)),
    binding!(Cmd::ParentDir, "delete a filter character, else parent folder", false; key(Backspace)),
    binding!(Cmd::ShowHidden, "show / hide hidden files (empty filter)", true; ch('.')),
    binding!(Cmd::HomeDir, "home folder (empty filter)", true; ch('~')),
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

static SETTINGS: &[Binding] = &[
    binding!(Cmd::Back, "close", true; key(Esc)),
    binding!(Cmd::NextTab, "next tab", true; key(Right), key(Tab)),
    binding!(Cmd::PrevTab, "previous tab", false; key(Left), key(BackTab)),
    binding!(Cmd::Up, "previous setting", false; key(Up), ch('k')),
    binding!(Cmd::Down, "next setting", false; key(Down), ch('j')),
    binding!(Cmd::Confirm, "change", true; key(Enter), ch(' ')),
];

/// The context's own bindings, then the shared ones it takes.
fn tables(ctx: Ctx) -> [&'static [Binding]; 3] {
    let own = match ctx {
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
        Ctx::AttachSend => ATTACH_SEND,
        Ctx::Stickers => STICKERS,
        Ctx::Settings => SETTINGS,
    };
    let edit = if matches!(ctx, Ctx::Compose | Ctx::Form) {
        TEXT_EDIT
    } else {
        &[]
    };
    let question = if is_typing(ctx) || ctx == Ctx::Help {
        &[]
    } else {
        QUESTION_HELP
    };
    [own, edit, question]
}

/// Every binding active in `ctx`, the context's first, globals last.
fn all(ctx: Ctx) -> impl Iterator<Item = &'static Binding> {
    tables(ctx)
        .into_iter()
        .flatten()
        .chain(GLOBAL.iter().filter(move |b| global_allowed(ctx, b.cmd)))
}

pub fn resolve(ctx: Ctx, event: &KeyEvent) -> Option<Cmd> {
    all(ctx)
        .find(|binding| binding.keys.iter().any(|key| matches_key(*key, event)))
        .map(|binding| binding.cmd)
}

fn global_allowed(ctx: Ctx, cmd: Cmd) -> bool {
    use Ctx::*;
    let main_view = matches!(ctx, ChatList | ChatSearch | Compose | Messages | Viewer);
    match cmd {
        Cmd::Quit | Cmd::Help => true,
        Cmd::Settings => main_view || ctx == Profile,
        Cmd::ComposeMenu | Cmd::ChatMenu | Cmd::ChatProfile => main_view,
        _ => false,
    }
}

fn matches_key(key: Key, event: &KeyEvent) -> bool {
    let mut modifiers = event.modifiers;
    let mut expected = key.mods;
    // Terminals differ in whether `?` or `K` come with Shift; the character says it.
    if matches!(event.code, KeyCode::Char(_)) && matches!(key.code, KeyCode::Char(_)) {
        modifiers = modifiers.difference(KeyModifiers::SHIFT);
        expected = expected.difference(KeyModifiers::SHIFT);
    }
    key.code == event.code && expected == modifiers
}

/// Printable keys are text here.
pub fn is_typing(ctx: Ctx) -> bool {
    matches!(
        ctx,
        Ctx::ChatSearch | Ctx::Compose | Ctx::Form | Ctx::Pick | Ctx::AttachBrowse
    )
}

/// How to get help in `ctx`: the status line starts with it, so it is never cut off.
pub fn help_key(ctx: Ctx) -> &'static str {
    if is_typing(ctx) { "F1 help" } else { "? help" }
}

/// The hinted bindings of `ctx` whose command `available` allows right now.
pub fn hint(ctx: Ctx, available: impl Fn(Cmd) -> bool) -> String {
    all(ctx)
        .filter(|b| b.hint && available(b.cmd))
        .map(|b| {
            let keys = match b.cmd {
                Cmd::Digit(_) => "1…9".to_owned(),
                _ => display_keys(b.keys),
            };
            format!("{keys} {}", b.help)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Every binding of `ctx`, for the help screen.
pub fn help_lines(ctx: Ctx) -> Vec<(String, &'static str)> {
    let mut lines: Vec<(String, &'static str)> = Vec::new();
    for b in all(ctx) {
        // The numbered menu items share one line.
        if matches!(b.cmd, Cmd::Digit(2..)) {
            continue;
        }
        let keys = match b.cmd {
            Cmd::Digit(_) => "1…9".to_owned(),
            _ => display_keys(b.keys),
        };
        lines.push((keys, b.help));
    }
    lines
}

fn display_keys(keys: &[Key]) -> String {
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

fn display_key(key: Key) -> String {
    let mut label = String::new();
    if key.mods.contains(KeyModifiers::CONTROL) {
        label.push_str("Ctrl-");
        if let KeyCode::Char(c) = key.code {
            label.push(c.to_ascii_uppercase());
            return label;
        }
    }
    if key.mods.contains(KeyModifiers::ALT) {
        label.push_str("Alt-");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        event(code, KeyModifiers::NONE)
    }

    fn normalized(key: Key) -> (KeyCode, KeyModifiers) {
        if matches!(key.code, KeyCode::Char(_)) {
            (key.code, key.mods.difference(KeyModifiers::SHIFT))
        } else {
            (key.code, key.mods)
        }
    }

    #[test]
    fn every_key_means_one_thing_per_context_and_is_documented() {
        for ctx in Ctx::ALL {
            let mut keys = Vec::new();
            for binding in all(ctx) {
                assert!(!binding.help.is_empty(), "empty help text in {ctx:?}");
                for &key in binding.keys {
                    let key = normalized(key);
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
            for b in all(ctx) {
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
        for ctx in [Ctx::ChatSearch, Ctx::Compose, Ctx::Form] {
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
        for ctx in [Ctx::Compose, Ctx::Form] {
            assert_eq!(resolve(ctx, &plain(KeyCode::Left)), Some(Cmd::CursorLeft));
            assert_eq!(resolve(ctx, &plain(KeyCode::Right)), Some(Cmd::CursorRight));
            assert_eq!(resolve(ctx, &plain(KeyCode::Home)), Some(Cmd::LineStart));
            assert_eq!(resolve(ctx, &plain(KeyCode::Delete)), Some(Cmd::DeleteForward));
        }
        // Outside text, ← and → move between panes.
        assert_eq!(resolve(Ctx::Messages, &plain(KeyCode::Left)), Some(Cmd::Left));
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
            ('o', Cmd::ComposeMenu),
            ('g', Cmd::ChatMenu),
            ('p', Cmd::ChatProfile),
        ] {
            assert_eq!(resolve(Ctx::Compose, &chord(c)), Some(cmd));
            assert_eq!(resolve(Ctx::Menu, &chord(c)), None);
        }
        assert_eq!(resolve(Ctx::Profile, &chord('s')), Some(Cmd::Settings));
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
    fn hints_skip_unavailable_commands() {
        let all_on = hint(Ctx::Messages, |_| true);
        assert!(all_on.contains("r reply"), "{all_on}");
        assert!(hint(Ctx::Messages, |_| false).is_empty());
        assert!(hint(Ctx::Menu, |_| true).contains("1…9 choose by number"));
        assert_eq!(help_key(Ctx::Compose), "F1 help");
        assert_eq!(help_key(Ctx::Messages), "? help");
        assert_eq!(display_key(ctrl('o')), "Ctrl-O");
    }

    #[test]
    fn help_lists_numbered_items_once_and_capitals_once() {
        let menu = help_lines(Ctx::Menu);
        assert_eq!(menu.iter().filter(|(k, _)| k == "1…9").count(), 1);
        assert!(!menu.iter().any(|(k, _)| k == "2"));
        let confirm = help_lines(Ctx::Confirm);
        assert!(confirm.iter().any(|(k, _)| k == "y"));
    }
}
