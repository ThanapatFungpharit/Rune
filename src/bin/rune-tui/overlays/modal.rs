//! Overlays: menus, forms, pickers, confirmations and info boxes, and the
//! actions they run (reactions, unsend, group management, other message types).

use std::borrow::Cow;

use crate::*;
use rune::{EmojiPackage, Reaction};

/// Emoji the react menu offers from "Recent" before "More LINE emoji…".
const MENU_RECENT_EMOJI: usize = 4;

/// A reaction we leave.
pub enum NewReaction {
    Builtin(Reaction),
    /// A LINE emoji: its package and emoji id.
    Emoji(EmojiPackage, String),
}

/// An overlay on top of the main screen; it gets every key while open.
pub enum Modal {
    Menu(Menu),
    Form(Form),
    Pick(Pick),
    Confirm(Confirm),
    /// The people and emoji on a message, refreshed from message state each draw.
    Reactions {
        msg: String,
        scroll: usize,
    },
    /// Who has read one of our messages (open chat), refreshed each draw.
    ReadBy {
        msg: String,
        scroll: usize,
    },
    Help {
        ctx: keymap::Ctx,
        under: Option<Box<Modal>>,
        scroll: usize,
    },
    Attach(attach::Attach),
    Stickers(stickers::Stickers),
    Settings(settings::SettingsPage),
    /// One plugin's page (from Settings › Plugins).
    Plugins(plugin_ui::Manager),
    /// A plugin's own view.
    PluginView(plugin_ui::PluginView),
    /// One screen's shortcuts (from Settings › Shortcuts).
    Shortcuts(shortcuts::KeyPage),
    /// Search in a chat.
    Find(find::Find),
}

/// Something the user can ask for from a menu, key or form.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// React to a message (`None` takes our reaction back).
    React(String, Option<Reaction>),
    /// React with a LINE emoji: message, package, emoji.
    ReactEmoji(String, String, String),
    /// The LINE emoji picker for a message.
    EmojiPicker(String),
    ReactMenu(String),
    Reactions(String),
    ConfirmUnsend(String),
    Unsend(String),
    Reply(String),
    ReadBy(String),
    ViewMedia,
    Save,
    Open,
    /// The selected message's text to the clipboard.
    CopyText,
    /// Open a downloaded file of a type that can run programs (asked first).
    OpenAnyway(String),
    GotoQuoted,
    Profile(String),
    Attach,
    Stickers,
    Location,
    ShareContact,
    NewGroup,
    Invite(String),
    Rename(String),
    ConfirmLeave(String),
    Leave(String),
    Mute(String, bool),
    Pin(String, bool),
    MovePin(String, i32),
    /// The account's pin lists: switch, create, rename, delete.
    PinLists,
    SwitchPinList(i64),
    NewPinList,
    RenamePinList(i64),
    ConfirmDeletePinList(i64),
    DeletePinList(i64),
    Accept(String),
    ConfirmDecline(String),
    Decline(String),
    ClearCache,
    /// Every shortcut back to its default (asked first).
    ResetShortcuts,
    /// Search a chat's stored messages.
    FindInChat(String),
    /// The albums a group's history mentions.
    Albums(String),
    /// The posts of one album: `(chat, album id)`.
    AlbumPhotos(String, String),
    /// Show a message's media in the viewer: `(chat, message id)`.
    ViewMessage(String, String),
    /// Pick a picture for your profile cover.
    ChangeCover,
    ConfirmCover(std::path::PathBuf),
    SetCover(std::path::PathBuf),
    /// Turn a plugin on (asked first, listing its permissions).
    EnablePlugin(String),
    ClearPluginData(String),
    ForgetPlugin(String),
    /// Delete a plugin installed from a repository (asked first).
    UninstallPlugin(String),
    /// Load the newer version waiting beside a running plugin (asked first).
    ApprovePluginUpdate(String),
    /// Let a plugin's new versions load without asking (asked first).
    AutoloadPlugin(String),
    /// Make a Git-installed plugin follow a branch, a range or one version.
    PluginTrack(String, plugins::versions::Track),
    /// Pin a Git-installed plugin to the version it has.
    PinPlugin(String),
    // The main menu's own entries.
    Search,
    ChatMenu(String),
    ComposeMenu,
    Settings(settings::Tab),
    ToggleReadReceipts,
    Help,
    Quit,
}

/// A menu item's label may end in `\t` and a shortcut, shown right-aligned.
pub struct Menu {
    pub title: String,
    pub items: Vec<(String, Action)>,
    pub sel: usize,
    /// Indices of items drawn with a divider line above them (groups).
    pub breaks: Vec<usize>,
}

/// A menu label with its shortcut (see [`Menu`]).
fn with_key(label: impl Into<String>, key: &str) -> String {
    let label = label.into();
    if key.is_empty() {
        label
    } else {
        format!("{label}\t{key}")
    }
}

/// A question with two labelled buttons. Every confirmation starts on
/// "Cancel", so a stray `Enter` never does them.
pub struct Confirm {
    pub text: String,
    pub action: Action,
    /// The highlighted button is the one that runs `action`.
    pub yes: bool,
}

impl Action {
    /// The label of the button that runs the action.
    fn verb(&self) -> &'static str {
        match self {
            Action::Unsend(_) => "Unsend",
            Action::Leave(_) => "Leave",
            Action::Decline(_) => "Decline",
            Action::DeletePinList(_) => "Delete",
            Action::ClearCache => "Clear",
            Action::ResetShortcuts => "Reset",
            Action::OpenAnyway(_) => "Open",
            Action::SetCover(_) => "Replace cover",
            Action::EnablePlugin(_) => "Enable",
            Action::ClearPluginData(_) => "Delete",
            Action::ForgetPlugin(_) => "Forget",
            Action::UninstallPlugin(_) => "Uninstall",
            Action::ApprovePluginUpdate(_) => "Load it",
            Action::AutoloadPlugin(_) => "Allow",
            _ => "OK",
        }
    }
}

/// LINE's limit on a group name, in characters.
pub const GROUP_NAME_MAX: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub enum FormKind {
    NewGroup,
    Rename(String),
    Location,
    DisplayName,
    StatusMessage,
    NewPinList,
    RenamePinList(i64),
    /// A form a plugin asked for (`Action::Form`).
    Plugin {
        plugin: String,
        form: String,
    },
    /// One of a plugin's settings.
    PluginSetting {
        plugin: String,
        key: String,
    },
    /// Install a plugin from a Git repository.
    PluginInstall,
}

pub struct Field {
    pub label: Cow<'static, str>,
    pub value: TextInput,
    pub hint: Cow<'static, str>,
}

pub struct Form {
    pub title: String,
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub sel: usize,
    pub error: Option<String>,
}

impl Form {
    fn new(
        title: impl Into<String>,
        kind: FormKind,
        fields: Vec<(&'static str, String, &'static str)>,
    ) -> Self {
        Form {
            title: title.into(),
            kind,
            fields: fields
                .into_iter()
                .map(|(label, value, hint)| Field {
                    label: label.into(),
                    value: value.into(),
                    hint: hint.into(),
                })
                .collect(),
            sel: 0,
            error: None,
        }
    }
    fn value(&self, i: usize) -> &str {
        self.fields[i].value.as_str().trim()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PickKind {
    /// Members for a new group with this name.
    NewGroup(String),
    Invite(String),
    ShareContact,
}

/// Choose people from the contact list (type to filter).
pub struct Pick {
    pub title: String,
    pub kind: PickKind,
    /// `(mid, name)`.
    pub options: Vec<(String, String)>,
    pub chosen: Vec<String>,
    pub filter: TextInput,
    pub sel: usize,
    pub single: bool,
}

impl Pick {
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.as_str().to_lowercase();
        (0..self.options.len())
            .filter(|&i| f.is_empty() || self.options[i].1.to_lowercase().contains(&f))
            .collect()
    }
}

/// A centred box of at most `w`×`h` inside `area`.
pub fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2)).max(10.min(area.width));
    let h = h.min(area.height.saturating_sub(2)).max(3.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Parse "13.75, 100.50" (as copied from a map) into a coordinate pair.
pub fn parse_lat_lon(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

impl App {
    pub fn open_menu(&mut self, title: impl Into<String>, items: Vec<(String, Action)>) {
        self.modal = Some(Modal::Menu(Menu {
            title: title.into(),
            items,
            sel: 0,
            breaks: Vec::new(),
        }));
    }

    /// A menu in groups separated by divider lines; empty groups are skipped.
    pub fn open_grouped_menu(&mut self, title: impl Into<String>, groups: Vec<Vec<(String, Action)>>) {
        let mut items = Vec::new();
        let mut breaks = Vec::new();
        for group in groups.into_iter().filter(|g| !g.is_empty()) {
            if !items.is_empty() {
                breaks.push(items.len());
            }
            items.extend(group);
        }
        self.open_menu(title, items);
        if let Some(Modal::Menu(menu)) = &mut self.modal {
            menu.breaks = breaks;
        }
    }

    pub fn confirm(&mut self, text: impl Into<String>, action: Action) {
        self.modal = Some(Modal::Confirm(Confirm {
            text: text.into(),
            action,
            yes: false,
        }));
    }

    /// `Enter` on an invitation (or its menu): join, decline or look first.
    pub fn invitation_menu(&mut self, mid: &str) {
        let items = vec![
            ("✅ Join the group".to_owned(), Action::Accept(mid.into())),
            ("✖ Decline…".to_owned(), Action::ConfirmDecline(mid.into())),
            ("👥 Group profile".to_owned(), Action::Profile(mid.into())),
        ];
        self.open_menu(format!(" Invitation to {} ", self.name(mid)), items);
    }

    /// `Ctrl-x`: everything that is not about one chat or message, each
    /// with its shortcut, so the menu also teaches the keys.
    pub fn main_menu(&mut self) {
        // This chat, then finding and starting chats, then the account,
        // then the app itself, and quitting alone at the end.
        let mut chat = Vec::new();
        if let Some(open) = self.open.clone() {
            chat.push((
                with_key(
                    format!("💬 Chat menu: {}", self.name(&open)),
                    &keymap::global(keymap::Cmd::ChatMenu),
                ),
                Action::ChatMenu(open),
            ));
            chat.push((
                with_key(
                    "📎 Send a photo, file, sticker…",
                    &keymap::global(keymap::Cmd::ComposeMenu),
                ),
                Action::ComposeMenu,
            ));
        }
        let receipts = if self.read_receipts() { "On" } else { "Off" };
        let groups = vec![
            chat,
            vec![
                (
                    with_key("🔍 Find a chat or friend", &keymap::global(keymap::Cmd::Search)),
                    Action::Search,
                ),
                (
                    with_key(
                        "👥 New group…",
                        &keymap::shortcut(keymap::Ctx::ChatList, keymap::Cmd::NewGroup),
                    ),
                    Action::NewGroup,
                ),
            ],
            vec![
                ("👤 My profile".to_owned(), Action::Profile(self.me.clone())),
                ("📌 Pin lists…".to_owned(), Action::PinLists),
                (
                    with_key(
                        format!("👀 Read receipts: {receipts}"),
                        &keymap::global(keymap::Cmd::ReadReceipts),
                    ),
                    Action::ToggleReadReceipts,
                ),
            ],
            vec![
                (
                    with_key("🔧 Settings", &keymap::global(keymap::Cmd::Settings)),
                    Action::Settings(settings::Tab::Notifications),
                ),
                ("🧩 Plugins".to_owned(), Action::Settings(settings::Tab::Plugins)),
                (
                    with_key("❓ Keyboard shortcuts", &keymap::global(keymap::Cmd::Help)),
                    Action::Help,
                ),
            ],
            vec![(
                with_key("👋 Quit", &keymap::global(keymap::Cmd::Quit)),
                Action::Quit,
            )],
        ];
        self.open_grouped_menu(" ☰ rune ", groups);
    }

    /// `m` in the chat list / `Ctrl-g` in a chat: what can be done with it.
    pub fn chat_menu(&mut self, mid: &str) {
        let name = self.name(mid);
        let entry = self.chats.iter().find(|c| c.mid == mid);
        if entry.is_some_and(|c| c.invited) {
            return self.invitation_menu(mid);
        }
        let muted = entry.is_some_and(|c| c.muted);
        // The same order for every kind of chat: look, send, organise, then
        // what changes the chat for others, and leaving alone at the end.
        let group = MidType::of(mid) != MidType::User;
        let profile = if group {
            "👥 Members & profile"
        } else {
            "👤 Profile"
        };
        let key = |cmd| keymap::shortcut(keymap::Ctx::ChatList, cmd);
        let mut look = vec![
            (
                with_key(profile, &key(keymap::Cmd::Profile)),
                Action::Profile(mid.into()),
            ),
            // No key shown: `/` searches the chat from its messages, but the
            // chat list from the list.
            ("🔎 Search in chat…".to_owned(), Action::FindInChat(mid.into())),
        ];
        if group {
            look.push(("🖼 Albums".to_owned(), Action::Albums(mid.into())));
        }
        let mut send = Vec::new();
        if self.open.as_deref() == Some(mid) {
            send.push((
                with_key(
                    "📎 Send a photo, file, sticker…",
                    &keymap::global(keymap::Cmd::ComposeMenu),
                ),
                Action::ComposeMenu,
            ));
        }
        let mut organise = vec![mute_item(mid, muted), pin_item(mid, self.is_pinned(mid))];
        if let Some(position) = self.pinned.iter().position(|pinned| pinned == mid) {
            if position > 0 {
                organise.push((
                    with_key("⬆ Move pin up", &key(keymap::Cmd::PinUp)),
                    Action::MovePin(mid.into(), -1),
                ));
            }
            if position + 1 < self.pinned.len() {
                organise.push((
                    with_key("⬇ Move pin down", &key(keymap::Cmd::PinDown)),
                    Action::MovePin(mid.into(), 1),
                ));
            }
        }
        let (mut manage, mut leave) = (Vec::new(), Vec::new());
        if group {
            manage.push(("➕ Invite friends…".to_owned(), Action::Invite(mid.into())));
            manage.push(("✏ Rename group…".to_owned(), Action::Rename(mid.into())));
            leave.push(("🚪 Leave group…".to_owned(), Action::ConfirmLeave(mid.into())));
        }
        self.open_grouped_menu(format!(" {name} "), vec![look, send, organise, manage, leave]);
    }

    /// The account's pin lists (main menu, Settings › Chats), the active one selected.
    pub fn pin_lists_menu(&mut self) {
        let lists = match self.store.pin_profiles() {
            Ok(lists) => lists,
            Err(e) => {
                self.status = format!("could not load pin lists: {e}");
                return;
            }
        };
        let mut items: Vec<(String, Action)> = lists
            .iter()
            .map(|p| {
                let pins = match p.pins {
                    1 => "1 pin".to_owned(),
                    n => format!("{n} pins"),
                };
                let mark = if p.active { "●" } else { "○" };
                (format!("{mark} {} · {pins}", p.name), Action::SwitchPinList(p.id))
            })
            .collect();
        let sel = lists.iter().position(|p| p.active).unwrap_or(0);
        items.push(("➕ New pin list…".to_owned(), Action::NewPinList));
        if let Some(active) = lists.iter().find(|p| p.active) {
            items.push((
                format!("✏ Rename “{}”…", active.name),
                Action::RenamePinList(active.id),
            ));
            if lists.len() > 1 {
                items.push((
                    format!("🗑 Delete “{}”…", active.name),
                    Action::ConfirmDeletePinList(active.id),
                ));
            }
        }
        self.open_menu(" 📌 Pin lists ", items);
        if let Some(Modal::Menu(menu)) = &mut self.modal {
            menu.sel = sel;
        }
    }

    fn pin_list(&self, id: i64) -> Option<rune::PinProfile> {
        self.store
            .pin_profiles()
            .ok()
            .and_then(|lists| lists.into_iter().find(|p| p.id == id))
    }

    /// Why `name` cannot name a pin list (other than `except`), if it cannot.
    fn pin_list_name_error(&self, name: &str, except: Option<i64>) -> Option<String> {
        if name.is_empty() {
            return Some("give the pin list a name".into());
        }
        if name.chars().count() > rune::store::PIN_PROFILE_NAME_MAX {
            return Some(format!(
                "pin list names are at most {} characters",
                rune::store::PIN_PROFILE_NAME_MAX
            ));
        }
        let lower = name.to_lowercase();
        let taken = self
            .store
            .pin_profiles()
            .unwrap_or_default()
            .into_iter()
            .any(|p| Some(p.id) != except && p.name.to_lowercase() == lower);
        taken.then(|| format!("there is already a pin list named “{name}”"))
    }

    /// Show the active pin list (after switching, creating or deleting).
    fn pin_list_changed(&mut self, status: String) {
        let selected = self.selected_mid();
        if self.reload_pins() {
            self.status = status;
        }
        self.sort_chats(selected);
    }

    /// `m` on a selected message.
    pub fn message_menu(&mut self) {
        let Some(m) = self.selected_msg().cloned() else {
            return;
        };
        if m.id.is_empty() {
            return;
        }
        // The keys of where the menu was opened (the viewer has fewer).
        let ctx = if self.viewer.is_some() {
            keymap::Ctx::Viewer
        } else {
            keymap::Ctx::Messages
        };
        let key = |cmd| keymap::shortcut(ctx, cmd);
        // Act on it, then look into it, then the destructive unsend alone at the end.
        let mut act = vec![
            (
                with_key("↩ Reply", &key(keymap::Cmd::Reply)),
                Action::Reply(m.id.clone()),
            ),
            (
                with_key("😀 React…", &key(keymap::Cmd::React)),
                Action::ReactMenu(m.id.clone()),
            ),
        ];
        if m.media_kind().is_some() && !m.unsent {
            let view = if ctx == keymap::Ctx::Messages {
                key(keymap::Cmd::Confirm)
            } else {
                String::new()
            };
            act.push((with_key("🖼 View", &view), Action::ViewMedia));
            act.push((
                with_key("💾 Save to ~/Downloads/rune", &key(keymap::Cmd::Save)),
                Action::Save,
            ));
            act.push((
                with_key("↗ Open with system app", &key(keymap::Cmd::OpenExternal)),
                Action::Open,
            ));
        }
        if m.copyable() {
            act.push((
                with_key("📋 Copy text", &key(keymap::Cmd::Copy)),
                Action::CopyText,
            ));
        }
        let mut info = Vec::new();
        if m.reply_to.is_some() {
            info.push(("⤴ Go to quoted message".to_owned(), Action::GotoQuoted));
        }
        if !m.unsent && !m.reactions.is_empty() {
            info.push(("👥 Reactions…".to_owned(), Action::Reactions(m.id.clone())));
        }
        let mut unsend = Vec::new();
        if m.from == self.me && !m.unsent {
            info.push(("👁 Read by…".to_owned(), Action::ReadBy(m.id.clone())));
            unsend.push((
                with_key("🚫 Unsend…", &key(keymap::Cmd::Unsend)),
                Action::ConfirmUnsend(m.id.clone()),
            ));
        }
        info.push((
            with_key("👤 Sender's profile", &key(keymap::Cmd::Profile)),
            Action::Profile(m.from.clone()),
        ));
        self.open_grouped_menu(" Message ", vec![act, info, unsend]);
    }

    /// `Ctrl-o`: things to send besides text.
    pub fn compose_menu(&mut self) {
        if self.open.is_none() {
            self.status = "open a chat first".into();
            return;
        }
        self.open_menu(
            " Send ",
            vec![
                ("📎 Photo, video or file…".to_owned(), Action::Attach),
                ("😀 Sticker…".to_owned(), Action::Stickers),
                ("📍 Location…".to_owned(), Action::Location),
                ("👤 Contact card…".to_owned(), Action::ShareContact),
            ],
        );
    }

    fn react_menu(&mut self, id: &str) {
        let mine = self.find_msg(id).and_then(|m| {
            m.reactions
                .iter()
                .find(|r| r.from == self.me)
                .map(|r| r.kind.clone())
        });
        let tick = |on: bool| if on { "  ✓" } else { "" };
        let mut items: Vec<(String, Action)> = Reaction::ALL
            .iter()
            .map(|r| {
                let on = mine == Some(ReactionKind::Predefined(*r as i64));
                let label = format!("{} {}{}", r.emoji(), r.label(), tick(on));
                (label, Action::React(id.into(), Some(*r)))
            })
            .collect();
        let mut mine_listed = matches!(mine, None | Some(ReactionKind::Predefined(_)));
        for (pkg, emoji, _) in self.recent_emoji_refs(MENU_RECENT_EMOJI) {
            let kind = ReactionKind::Paid {
                product_id: pkg.clone(),
                emoji_id: emoji.clone(),
            };
            let on = mine.as_ref() == Some(&kind);
            mine_listed |= on;
            let label = format!("{}{}", self.reaction_label(&kind), tick(on));
            items.push((label, Action::ReactEmoji(id.into(), pkg, emoji)));
        }
        let more = match mine.as_ref().filter(|_| !mine_listed) {
            Some(k) => format!("✨ More LINE emoji…  ✓ {}", self.reaction_label(k)),
            None => "✨ More LINE emoji…".into(),
        };
        items.push((more, Action::EmojiPicker(id.into())));
        if mine.is_some() {
            items.push(("✖ Remove my reaction".into(), Action::React(id.into(), None)));
        }
        self.open_menu(" React ", items);
    }

    /// Alt text of a LINE emoji reaction from an owned package.
    fn paid_alt(&self, kind: &ReactionKind) -> Option<&str> {
        match kind {
            ReactionKind::Paid { product_id, emoji_id } => {
                self.emoji_alt(product_id, emoji_id).filter(|a| !a.is_empty())
            }
            ReactionKind::Predefined(_) => None,
        }
    }

    /// "👍 Nice", "🫶 heart hands", "✨ heart eyes Moon": a reaction for menus and the status line.
    pub(crate) fn reaction_label(&self, kind: &ReactionKind) -> String {
        let name = match kind {
            ReactionKind::Predefined(n) => Reaction::from_i64(*n).map(Reaction::label),
            ReactionKind::Paid { .. } => self.paid_alt(kind),
        };
        match name {
            Some(n) => format!("{} {n}", reaction_glyph(kind)),
            None => reaction_glyph(kind).to_owned(),
        }
    }

    fn contact_options(&self, exclude: &HashSet<String>) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .contacts
            .iter()
            .filter(|(mid, _)| !exclude.contains(*mid) && *mid != &self.me)
            .map(|(mid, name)| (mid.clone(), name.clone()))
            .collect();
        // Names are display-only and may collide: break ties by mid so
        // same-named contacts keep a stable order (the map's order is not).
        v.sort_by(|(a_mid, a), (b_mid, b)| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a_mid.cmp(b_mid))
        });
        v
    }

    fn open_pick(&mut self, title: &str, kind: PickKind, exclude: HashSet<String>, single: bool) {
        let options = self.contact_options(&exclude);
        if options.is_empty() {
            self.status = if self.contacts.is_empty() {
                "your friend list is still loading…".into()
            } else {
                "no one else to choose".into()
            };
            return;
        }
        self.modal = Some(Modal::Pick(Pick {
            title: title.into(),
            kind,
            options,
            chosen: Vec::new(),
            filter: TextInput::default(),
            sel: 0,
            single,
        }));
    }

    /// Run an action (closing the overlay it came from first).
    pub fn run_action(&mut self, action: Action) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        match action {
            Action::React(id, r) => self.react(&id, r.map(NewReaction::Builtin)),
            Action::ReactEmoji(id, pkg, emoji) => self.react_emoji(&id, &pkg, &emoji),
            Action::EmojiPicker(id) => self.open_emoji_picker(id),
            Action::ReactMenu(id) => self.react_menu(&id),
            Action::Reactions(id) => self.modal = Some(Modal::Reactions { msg: id, scroll: 0 }),
            Action::ConfirmUnsend(id) => self.confirm(
                "Unsend this message? It disappears for everyone in the chat \
                 (LINE only allows it for 24 hours).",
                Action::Unsend(id),
            ),
            Action::Unsend(id) => {
                let Some(chat) = self.open.clone() else { return };
                self.status = "unsending…".into();
                tokio::spawn(async move {
                    let res = client.unsend_message(&id).await.map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::Unsent(chat, id, res));
                });
            }
            Action::Reply(id) => {
                if let Some(i) = self.open_messages().iter().position(|m| m.id == id) {
                    self.sel = Some(i);
                }
                self.start_reply();
            }
            Action::ReadBy(id) => self.modal = Some(Modal::ReadBy { msg: id, scroll: 0 }),
            Action::ViewMedia => self.open_viewer(),
            Action::Save => self.act_on_selected(After::Save),
            Action::CopyText => self.copy_selected(),
            Action::Open => self.act_on_selected(After::Open),
            Action::OpenAnyway(id) => self.finish(&id, After::OpenAnyway),
            Action::GotoQuoted => self.goto_quoted(),
            Action::Profile(mid) => self.open_profile(mid),
            Action::Attach => self.open_attach(None),
            Action::Stickers => self.open_stickers(),
            Action::Location => {
                self.modal = Some(Modal::Form(Form::new(
                    " 📍 Send a location ",
                    FormKind::Location,
                    vec![
                        ("Title", String::new(), "e.g. Siam Paragon"),
                        ("Address", String::new(), "optional"),
                        (
                            "Latitude",
                            String::new(),
                            "e.g. 13.7466 — or paste \"13.7466, 100.5347\"",
                        ),
                        ("Longitude", String::new(), "e.g. 100.5347"),
                    ],
                )))
            }
            Action::ShareContact => self.open_pick(
                " 👤 Share a contact ",
                PickKind::ShareContact,
                HashSet::new(),
                true,
            ),
            Action::NewGroup => {
                self.modal = Some(Modal::Form(Form::new(
                    " 👥 New group ",
                    FormKind::NewGroup,
                    vec![("Group name", String::new(), "next you pick who to invite")],
                )))
            }
            Action::Invite(chat) => {
                let mut exclude: HashSet<String> = self
                    .members
                    .get(&chat)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                exclude.extend(self.invitees.get(&chat).cloned().unwrap_or_default());
                let title = format!(" ➕ Invite to {} ", self.name(&chat));
                self.open_pick(&title, PickKind::Invite(chat), exclude, false);
            }
            Action::Rename(chat) => {
                let name = self.names.get(&chat).cloned().unwrap_or_default();
                self.modal = Some(Modal::Form(Form::new(
                    " ✏ Rename group ",
                    FormKind::Rename(chat),
                    vec![("Group name", name, "members see the new name")],
                )));
            }
            Action::ConfirmLeave(chat) => {
                let text = format!(
                    "Leave {}? Its members will see that you left, and its history disappears here.",
                    self.name(&chat)
                );
                self.confirm(text, Action::Leave(chat));
            }
            Action::Leave(chat) => {
                self.status = format!("leaving {}…", self.name(&chat));
                tokio::spawn(async move {
                    let res = client.leave_chat(&chat).await.map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::Left(chat, res));
                });
            }
            Action::Mute(chat, muted) => {
                self.status = if muted { "muting…" } else { "unmuting…" }.into();
                tokio::spawn(async move {
                    let res = client
                        .set_chat_muted(&chat, muted)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::Muted(chat, muted, res));
                });
            }
            Action::Pin(chat, pinned) => self.set_pinned(&chat, pinned),
            Action::MovePin(chat, delta) => self.move_pin(&chat, delta),
            Action::PinLists => self.pin_lists_menu(),
            Action::SwitchPinList(id) => match self.store.switch_pin_profile(id) {
                Ok(()) => {
                    let name = self.pin_list(id).map(|p| p.name).unwrap_or_default();
                    self.pin_list_changed(format!("pin list: {name}"));
                }
                Err(e) => self.status = format!("could not switch pin list: {e}"),
            },
            Action::NewPinList => {
                self.modal = Some(Modal::Form(Form::new(
                    " 📌 New pin list ",
                    FormKind::NewPinList,
                    vec![("Name", String::new(), "a new, empty list of pinned chats")],
                )));
            }
            Action::RenamePinList(id) => {
                let name = self.pin_list(id).map(|p| p.name).unwrap_or_default();
                self.modal = Some(Modal::Form(Form::new(
                    " ✏ Rename pin list ",
                    FormKind::RenamePinList(id),
                    vec![("Name", name, "only stored on this computer")],
                )));
            }
            Action::ConfirmDeletePinList(id) => {
                let Some(list) = self.pin_list(id) else { return };
                let text = format!(
                    "Delete the pin list “{}” ({} pinned)? The chats themselves and your other \
                     pin lists are not affected.",
                    list.name, list.pins
                );
                self.confirm(text, Action::DeletePinList(id));
            }
            Action::DeletePinList(id) => {
                let name = self.pin_list(id).map(|p| p.name).unwrap_or_default();
                match self.store.delete_pin_profile(id) {
                    Ok(()) => {
                        let now = self
                            .store
                            .active_pin_profile()
                            .map(|p| p.name)
                            .unwrap_or_default();
                        self.pin_list_changed(format!("deleted pin list {name} · now {now}"));
                    }
                    Err(e) => self.status = format!("could not delete pin list: {e}"),
                }
            }
            Action::Accept(chat) => self.answer_invite_for(chat, true),
            Action::ConfirmDecline(chat) => {
                let text = format!("Decline the invitation to {}?", self.name(&chat));
                self.confirm(text, Action::Decline(chat));
            }
            Action::Decline(chat) => self.answer_invite_for(chat, false),
            Action::ClearCache => self.clear_cache(),
            Action::ResetShortcuts => self.reset_all_shortcuts(),
            Action::FindInChat(chat) => self.open_find(chat),
            Action::Search => {
                self.close_profile();
                self.start_search();
            }
            Action::ChatMenu(chat) => {
                self.close_profile();
                self.chat_menu(&chat);
            }
            Action::ComposeMenu => {
                self.close_profile();
                self.compose_menu();
            }
            Action::Settings(tab) => self.open_settings_at(tab),
            Action::ToggleReadReceipts => self.toggle_read_receipts(),
            Action::Help => {
                let ctx = self.key_context();
                self.open_help(ctx);
            }
            Action::Quit => self.quit = true,
            Action::EnablePlugin(id) => self.enable_plugin(&id),
            Action::ClearPluginData(id) => self.clear_plugin_data(&id),
            Action::ForgetPlugin(id) => self.forget_plugin(&id),
            Action::UninstallPlugin(id) => self.uninstall_plugin(&id),
            Action::ApprovePluginUpdate(id) => self.approve_plugin_update(&id),
            Action::AutoloadPlugin(id) => self.set_plugin_autoload(&id, true),
            Action::PluginTrack(id, track) => self.switch_plugin_track(&id, track),
            Action::PinPlugin(id) => self.pin_plugin(&id),
            Action::Albums(chat) => self.albums_menu(&chat),
            Action::AlbumPhotos(chat, album) => self.album_photos_menu(&chat, &album),
            Action::ViewMessage(chat, id) => self.view_message(&chat, &id),
            Action::ChangeCover => self.open_cover_picker(),
            Action::ConfirmCover(path) => self.confirm(
                "Replace your profile cover with this picture? Everyone who can see \
                 your profile will see it.",
                Action::SetCover(path),
            ),
            Action::SetCover(path) => {
                self.status = "uploading the new cover…".into();
                tokio::spawn(async move {
                    let res = async {
                        let data = tokio::fs::read(&path)
                            .await
                            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("cover.jpg")
                            .to_owned();
                        client
                            .set_profile_cover(&data, &name)
                            .await
                            .map(drop)
                            .map_err(|e| e.to_string())
                    }
                    .await;
                    let _ = tx.send(AppEvent::CoverSet(res));
                });
            }
        }
    }

    /// React (or take a reaction back), showing it at once.
    pub(crate) fn react(&mut self, id: &str, r: Option<NewReaction>) {
        let kind = r.as_ref().map(|r| match r {
            NewReaction::Builtin(b) => ReactionKind::Predefined(*b as i64),
            NewReaction::Emoji(p, e) => ReactionKind::Paid {
                product_id: p.id.clone(),
                emoji_id: e.clone(),
            },
        });
        self.status = match &kind {
            Some(k) => format!("reacted {}", self.reaction_label(k)),
            None => "reaction removed".into(),
        };
        if let Some(NewReaction::Emoji(p, e)) = &r {
            self.note_recent_emoji(&p.id, e);
        }
        let me = self.me.clone();
        let mut before = None;
        if let Some(m) = self.find_msg_mut(id) {
            before = m.reactions.iter().find(|x| x.from == me).map(|x| x.kind.clone());
            rune::models::apply_reaction(&mut m.reactions, &me, kind.clone(), now_ms());
        }
        self.saved(self.store.set_reaction(id, &me, kind.clone(), now_ms()));
        let (client, tx, id) = (self.client.clone(), self.tx.clone(), id.to_owned());
        tokio::spawn(async move {
            let res = match r {
                Some(NewReaction::Builtin(r)) => client.react(&id, r).await,
                Some(NewReaction::Emoji(p, e)) => client.react_emoji(&id, &p, &e).await,
                None => client.cancel_reaction(&id).await,
            };
            if let Err(e) = res {
                let _ = tx.send(AppEvent::ReactionFailed(id, before, kind, e.to_string()));
            }
        });
    }

    /// LINE refused a reaction shown ahead of its answer: show (and keep)
    /// the previous one again, unless another reaction replaced it meanwhile.
    pub(crate) fn on_reaction_failed(
        &mut self,
        id: &str,
        before: Option<ReactionKind>,
        tried: Option<ReactionKind>,
        e: String,
    ) {
        self.status = format!("reaction failed: {e}");
        let me = self.me.clone();
        let Some(m) = self.find_msg_mut(id) else {
            return;
        };
        let current = m.reactions.iter().find(|x| x.from == me).map(|x| x.kind.clone());
        if current != tried {
            return;
        }
        rune::models::apply_reaction(&mut m.reactions, &me, before.clone(), now_ms());
        self.saved(self.store.set_reaction(id, &me, before, now_ms()));
    }

    /// Title and lines of the *Read by…* box for message `id` of the open
    /// chat: readers with when they read it (or how we know), then the
    /// group members not known to have read it.
    pub(crate) fn read_by_lines(&self, id: &str) -> (String, Vec<String>) {
        let Some(chat) = self.open.as_deref() else {
            return (" Read by ".into(), Vec::new());
        };
        let readers = self.readers(chat, id);
        let times = self.store.read_times(chat, id).unwrap_or_default();
        let mut rows: Vec<(Option<i64>, String, &str, String)> = readers
            .iter()
            .map(|(mid, how)| {
                let at = match how {
                    SeenBy::Receipt => times.get(mid).copied(),
                    _ => None,
                };
                let detail = match (how, at) {
                    (SeenBy::Receipt, Some(at)) => hhmm(at),
                    (SeenBy::Receipt, None) => String::new(),
                    (SeenBy::WroteLater, _) => "wrote later".into(),
                    (SeenBy::Reacted, _) => "reacted".into(),
                };
                (at, self.name(mid), mid.as_str(), detail)
            })
            .collect();
        // In reading order where known, then by name (mid breaks ties).
        rows.sort_by(|a, b| (a.0.is_none(), a.0, &a.1, a.2).cmp(&(b.0.is_none(), b.0, &b.1, b.2)));
        let name_w = rows.iter().map(|r| r.1.width()).max().unwrap_or(0).min(28);
        let mut lines: Vec<String> = rows
            .iter()
            .map(|(_, name, _, detail)| {
                let name = ellipsize(name, name_w);
                let pad = " ".repeat(name_w.saturating_sub(name.width()));
                format!("✓ {name}{pad}  {detail}").trim_end().to_owned()
            })
            .collect();
        if lines.is_empty() {
            lines.push("No one is known to have read it yet.".into());
        }
        let group = MidType::of(chat) != MidType::User;
        let others: Vec<&String> = self
            .members
            .get(chat)
            .filter(|_| group)
            .into_iter()
            .flatten()
            .filter(|m| **m != self.me)
            .collect();
        let mut not_yet: Vec<(String, &str)> = others
            .iter()
            .filter(|m| !readers.iter().any(|(r, _)| r == **m))
            .map(|m| (self.name(m), m.as_str()))
            .collect();
        not_yet.sort();
        if !not_yet.is_empty() {
            lines.push(String::new());
            lines.push(format!("Not yet ({}):", not_yet.len()));
            lines.extend(not_yet.into_iter().map(|(name, _)| format!("  {name}")));
        }
        lines.push(String::new());
        lines.push(
            "Times are when LINE reported the read. LINE's Chrome protocol returns no read \
             history, so reads from before rune was listening are known only when the reader \
             wrote or reacted later."
                .into(),
        );
        let title = match (group, others.len()) {
            (false, _) if readers.is_empty() => " Not read yet ".into(),
            (false, _) => " Read ".into(),
            (true, 0) => format!(" Read by {} ", readers.len()),
            (true, n) => format!(" Read by {} of {n} ", readers.len().min(n)),
        };
        (title, lines)
    }

    pub(crate) fn reaction_lines(&self, id: &str) -> (String, Vec<String>) {
        let Some(msg) = self.find_msg(id).filter(|m| !m.unsent) else {
            return (" Reactions ".into(), vec!["No reactions".into()]);
        };
        let lines: Vec<String> = msg
            .reactions
            .iter()
            .map(|r| match &r.kind {
                ReactionKind::Paid { .. } => {
                    format!("{}  {}", reaction_glyph(&r.kind), self.name(&r.from))
                        + &self
                            .paid_alt(&r.kind)
                            .map(|a| format!("  ({a})"))
                            .unwrap_or_default()
                }
                ReactionKind::Predefined(_) => format!("{}  {}", reaction_glyph(&r.kind), self.name(&r.from)),
            })
            .collect();
        if lines.is_empty() {
            return (" Reactions ".into(), vec!["No reactions".into()]);
        }
        (format!(" Reactions ({}) ", lines.len()), lines)
    }

    fn submit_form(&mut self, mut form: Form) {
        let chat = self.open.clone();
        let (client, tx) = (self.client.clone(), self.tx.clone());
        let fail = |app: &mut App, mut form: Form, e: String| {
            form.error = Some(e);
            app.modal = Some(Modal::Form(form));
        };
        match form.kind.clone() {
            FormKind::NewGroup => {
                self.close_profile();
                let name = form.value(0).trim().to_owned();
                if name.is_empty() {
                    return fail(self, form, "give the group a name".into());
                }
                if name.chars().count() > GROUP_NAME_MAX {
                    return fail(
                        self,
                        form,
                        format!("group names are at most {GROUP_NAME_MAX} characters"),
                    );
                }
                self.open_pick(
                    &format!(" 👥 Invite to “{name}” "),
                    PickKind::NewGroup(name),
                    HashSet::new(),
                    false,
                );
                // No one to invite (yet): keep the typed name instead of losing it.
                if self.modal.is_none() {
                    let e = self.status.clone();
                    fail(self, form, e);
                }
            }
            FormKind::Rename(chat) => {
                let name = form.value(0).trim().to_owned();
                if name.is_empty() {
                    return fail(self, form, "the name cannot be empty".into());
                }
                if name.chars().count() > GROUP_NAME_MAX {
                    return fail(
                        self,
                        form,
                        format!("group names are at most {GROUP_NAME_MAX} characters"),
                    );
                }
                self.status = "renaming…".into();
                tokio::spawn(async move {
                    let res = client.rename_chat(&chat, &name).await.map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::Renamed(chat, name, res));
                });
            }
            FormKind::Location => {
                let Some(chat) = chat else { return };
                // "lat, lon" pasted into the latitude field fills both.
                if let Some((lat, lon)) = parse_lat_lon(form.value(2)) {
                    form.fields[2].value.set(lat.to_string());
                    form.fields[3].value.set(lon.to_string());
                }
                let (lat, lon) = match (form.value(2).parse::<f64>(), form.value(3).parse::<f64>()) {
                    (Ok(a), Ok(b)) if (-90.0..=90.0).contains(&a) && (-180.0..=180.0).contains(&b) => (a, b),
                    _ => {
                        return fail(
                            self,
                            form,
                            "latitude/longitude must be numbers (−90…90, −180…180)".into(),
                        );
                    }
                };
                let title = match form.value(0) {
                    "" => "Location".to_owned(),
                    t => t.to_owned(),
                };
                let out = OutgoingMessage::location(&chat, lat, lon, &title, form.value(1));
                self.send_outgoing(chat, out, format!("📍 [location {title}]"));
            }
            FormKind::NewPinList => {
                let name = form.value(0).to_owned();
                if let Some(e) = self.pin_list_name_error(&name, None) {
                    return fail(self, form, e);
                }
                let created = self.store.create_pin_profile(&name);
                match created.and_then(|p| self.store.switch_pin_profile(p.id).map(|()| p)) {
                    Ok(p) => self.pin_list_changed(format!("pin list: {} (new, empty)", p.name)),
                    Err(e) => fail(self, form, e.to_string()),
                }
            }
            FormKind::RenamePinList(id) => {
                let name = form.value(0).to_owned();
                if let Some(e) = self.pin_list_name_error(&name, Some(id)) {
                    return fail(self, form, e);
                }
                match self.store.rename_pin_profile(id, &name) {
                    Ok(()) => self.pin_list_changed(format!("pin list renamed to {name}")),
                    Err(e) => fail(self, form, e.to_string()),
                }
            }
            FormKind::Plugin { plugin, form: id } => {
                let values = form.fields.iter().map(|f| f.value.as_str().to_owned()).collect();
                self.plugin_event(&plugin, plugins::Event::FormSubmitted { form: id, values });
            }
            FormKind::PluginInstall => {
                match plugins::install::Request::new(form.value(0), form.value(1), form.value(2)) {
                    Ok(req) => self.install_plugin(req),
                    Err(e) => fail(self, form, e),
                }
            }
            FormKind::PluginSetting { plugin, key } => {
                let value = serde_json::Value::from(form.value(0));
                match self.plugins.set_setting(&plugin, &key, value) {
                    Ok(fx) => {
                        self.apply_plugin_effects(fx);
                        self.open_plugin_page(plugin);
                    }
                    Err(e) => fail(self, form, e),
                }
            }
            FormKind::DisplayName | FormKind::StatusMessage => {
                let value = form.value(0).to_owned();
                let is_name = form.kind == FormKind::DisplayName;
                if is_name && value.is_empty() {
                    return fail(self, form, "the name cannot be empty".into());
                }
                self.status = "saving…".into();
                tokio::spawn(async move {
                    let res = if is_name {
                        client.set_display_name(&value).await
                    } else {
                        client.set_status_message(&value).await
                    };
                    let s = match res {
                        Ok(()) if is_name => format!("display name is now {value}"),
                        Ok(()) => "status message saved".into(),
                        Err(e) => format!("could not save: {e}"),
                    };
                    let _ = tx.send(AppEvent::Status(s));
                    let _ = tx.send(AppEvent::ProfileChanged);
                });
                // Settings › Account comes back (it was opened from there).
            }
        }
    }

    fn submit_pick(&mut self, pick: Pick) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        match pick.kind.clone() {
            PickKind::NewGroup(name) => {
                if pick.chosen.is_empty() {
                    self.status = "pick at least one friend (Space)".into();
                    self.modal = Some(Modal::Pick(pick));
                    return;
                }
                self.status = format!("creating {name}…");
                let members = pick.chosen;
                tokio::spawn(async move {
                    let res = client
                        .create_group(&name, &members)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::GroupCreated(res, members));
                });
            }
            PickKind::Invite(chat) => {
                if pick.chosen.is_empty() {
                    self.status = "nobody selected".into();
                    return;
                }
                let mids = pick.chosen;
                self.status = format!("inviting {}…", mids.len());
                tokio::spawn(async move {
                    let res = client
                        .invite_into_chat(&chat, &mids)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::Invited(chat, mids, res));
                });
            }
            PickKind::ShareContact => {
                let Some(chat) = self.open.clone() else { return };
                let Some(mid) = pick.chosen.first().cloned() else {
                    return;
                };
                let name = pick
                    .options
                    .iter()
                    .find(|o| o.0 == mid)
                    .map(|o| o.1.clone())
                    .unwrap_or_default();
                let out = OutgoingMessage::contact(&chat, &mid, &name);
                self.send_outgoing(chat, out, format!("👤 [contact {name}]"));
            }
        }
    }

    // -- keys ----------------------------------------------------------------------------

    fn close_modal(&mut self) {
        self.modal = match self.modal.take() {
            Some(Modal::Help { under, .. }) => under.map(|modal| *modal),
            // The plugin hears that its view went (as with Esc).
            Some(Modal::PluginView(v)) => {
                let view = plugins::Event::ViewClosed { view: v.view.id };
                return self.plugin_event(&v.view.plugin, view);
            }
            _ => None,
        };
    }

    pub fn modal_key(&mut self, k: KeyEvent) {
        let cmd = keymap::resolve(self.key_context(), &k);
        self.modal_input(cmd, k);
    }

    /// What a click or the wheel does on the overlay: `cmd`, whichever key
    /// it is bound to.
    pub fn modal_cmd(&mut self, cmd: keymap::Cmd) {
        self.modal_input(Some(cmd), KeyEvent::new(KeyCode::Null, KeyModifiers::NONE));
    }

    /// `cmd` (what `k` means on the open overlay, if anything) for the overlay.
    fn modal_input(&mut self, cmd: Option<keymap::Cmd>, k: KeyEvent) {
        use keymap::Cmd;
        let Some(modal) = self.modal.take() else { return };
        let keep = |app: &mut App, m: Modal| app.modal = Some(m);
        match modal {
            Modal::Menu(mut m) => match cmd {
                Some(Cmd::Back) => {}
                Some(Cmd::Up) => {
                    m.sel = m.sel.checked_sub(1).unwrap_or(m.items.len().saturating_sub(1));
                    keep(self, Modal::Menu(m));
                }
                Some(Cmd::Down) => {
                    m.sel = (m.sel + 1) % m.items.len().max(1);
                    keep(self, Modal::Menu(m));
                }
                Some(Cmd::Home) => {
                    m.sel = 0;
                    keep(self, Modal::Menu(m));
                }
                Some(Cmd::End) => {
                    m.sel = m.items.len().saturating_sub(1);
                    keep(self, Modal::Menu(m));
                }
                Some(Cmd::Confirm) => {
                    if let Some((_, a)) = m.items.into_iter().nth(m.sel) {
                        self.run_action(a);
                    }
                }
                Some(Cmd::Digit(i @ 1..=9)) => {
                    let i = i as usize - 1;
                    if i < m.items.len() {
                        let a = m.items.swap_remove(i).1;
                        self.run_action(a);
                    } else {
                        keep(self, Modal::Menu(m));
                    }
                }
                _ => keep(self, Modal::Menu(m)),
            },
            Modal::Confirm(mut c) => match cmd {
                Some(Cmd::Yes) => self.run_action(c.action),
                Some(Cmd::Confirm) if c.yes => self.run_action(c.action),
                Some(Cmd::Back | Cmd::No | Cmd::Confirm) => self.status = "cancelled".into(),
                Some(Cmd::Left) => {
                    c.yes = true;
                    keep(self, Modal::Confirm(c));
                }
                Some(Cmd::Right) => {
                    c.yes = false;
                    keep(self, Modal::Confirm(c));
                }
                Some(Cmd::SwitchFocus) => {
                    c.yes = !c.yes;
                    keep(self, Modal::Confirm(c));
                }
                _ => keep(self, Modal::Confirm(c)),
            },
            Modal::Reactions { msg, scroll } => {
                if let Some(scroll) = text_scroll(cmd, scroll) {
                    keep(self, Modal::Reactions { msg, scroll });
                }
            }
            Modal::ReadBy { msg, scroll } => {
                if let Some(scroll) = text_scroll(cmd, scroll) {
                    keep(self, Modal::ReadBy { msg, scroll });
                }
            }
            Modal::Help { ctx, under, scroll } => match cmd {
                Some(Cmd::Back | Cmd::Help) => self.modal = under.map(|modal| *modal),
                cmd => {
                    let scroll = scrolled(cmd, scroll);
                    keep(self, Modal::Help { ctx, under, scroll });
                }
            },
            Modal::Form(mut f) => match cmd {
                Some(Cmd::Back) => {
                    self.status = "cancelled".into();
                    if let FormKind::Plugin { plugin, form } = f.kind {
                        self.plugin_event(&plugin, plugins::Event::FormCancelled { form });
                    } else if let FormKind::PluginSetting { plugin, .. } = f.kind {
                        self.open_plugin_page(plugin);
                    }
                }
                Some(Cmd::Down) => {
                    f.sel = (f.sel + 1) % f.fields.len();
                    keep(self, Modal::Form(f));
                }
                Some(Cmd::Up) => {
                    f.sel = f.sel.checked_sub(1).unwrap_or(f.fields.len() - 1);
                    keep(self, Modal::Form(f));
                }
                Some(Cmd::Confirm) if f.sel + 1 < f.fields.len() => {
                    // A pasted "lat, lon" can be submitted right away.
                    if f.kind == FormKind::Location && f.sel == 2 && parse_lat_lon(f.value(2)).is_some() {
                        return self.submit_form(f);
                    }
                    f.sel += 1;
                    keep(self, Modal::Form(f));
                }
                Some(Cmd::Confirm) => self.submit_form(f),
                cmd => {
                    if f.fields[f.sel].value.edit(cmd, &k) == Edit::Changed {
                        f.error = None;
                    }
                    keep(self, Modal::Form(f));
                }
            },
            Modal::Pick(mut p) => {
                let vis = p.visible();
                match cmd {
                    Some(Cmd::Back) => self.status = "cancelled".into(),
                    Some(Cmd::Up) => {
                        p.sel = p.sel.saturating_sub(1);
                        keep(self, Modal::Pick(p));
                    }
                    Some(Cmd::Down) => {
                        p.sel = (p.sel + 1).min(vis.len().saturating_sub(1));
                        keep(self, Modal::Pick(p));
                    }
                    // A page is as many people as the last frame showed.
                    Some(Cmd::PageUp) => {
                        p.sel = p.sel.saturating_sub(self.hit.modal_rows.len().max(1));
                        keep(self, Modal::Pick(p));
                    }
                    Some(Cmd::PageDown) => {
                        let page = self.hit.modal_rows.len().max(1);
                        p.sel = (p.sel + page).min(vis.len().saturating_sub(1));
                        keep(self, Modal::Pick(p));
                    }
                    Some(Cmd::Toggle) if p.single => {
                        if let Some(&i) = vis.get(p.sel) {
                            p.chosen = vec![p.options[i].0.clone()];
                            self.submit_pick(p);
                        } else {
                            keep(self, Modal::Pick(p));
                        }
                    }
                    Some(Cmd::Toggle) => {
                        if let Some(&i) = vis.get(p.sel) {
                            let mid = p.options[i].0.clone();
                            match p.chosen.iter().position(|m| *m == mid) {
                                Some(j) => drop(p.chosen.remove(j)),
                                None => p.chosen.push(mid),
                            }
                        }
                        keep(self, Modal::Pick(p));
                    }
                    Some(Cmd::Confirm) => {
                        // Enter with nothing ticked picks the highlighted person.
                        if p.chosen.is_empty()
                            && let Some(&i) = vis.get(p.sel)
                        {
                            p.chosen.push(p.options[i].0.clone());
                        }
                        self.submit_pick(p);
                    }
                    cmd => {
                        if p.filter.edit(cmd, &k) == Edit::Changed {
                            p.sel = 0;
                        }
                        keep(self, Modal::Pick(p));
                    }
                }
            }
            Modal::Attach(a) => self.attach_key(a, cmd, k),
            Modal::Stickers(s) => self.stickers_key(s, cmd),
            Modal::Settings(s) => self.settings_key(s, cmd),
            Modal::Plugins(m) => self.plugins_key(m, cmd),
            Modal::PluginView(v) => self.plugin_view_key(v, cmd, k),
            Modal::Shortcuts(p) => self.shortcuts_key(p, cmd, k),
            Modal::Find(f) => self.find_key(f, cmd, k),
        }
    }

    /// Pasted text goes to the overlay's text field, if it has one.
    pub fn modal_paste(&mut self, text: &str) {
        match self.modal.as_mut() {
            Some(Modal::Form(f)) => {
                let line = text.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " ");
                f.fields[f.sel].value.insert_str(&line);
                f.error = None;
            }
            Some(Modal::Pick(p)) => {
                p.filter.insert_str(text.trim());
                p.sel = 0;
            }
            Some(Modal::Attach(_)) => {
                if let Some(Modal::Attach(a)) = self.modal.take() {
                    self.attach_paste(a, text);
                }
            }
            Some(Modal::Find(_)) => {
                if let Some(Modal::Find(f)) = self.modal.take() {
                    self.find_paste(f, text);
                }
            }
            _ => {}
        }
    }

    pub fn modal_mouse(&mut self, m: MouseEvent) {
        use keymap::Cmd;
        let at = Position::new(m.column, m.row);
        let inside = self.hit.modal.is_some_and(|r| r.contains(at));
        // Waiting for a new shortcut: the mouse gives up waiting.
        if let Some(Modal::Shortcuts(p)) = self.modal.as_mut()
            && p.state != shortcuts::State::Browse
            && !matches!(
                m.kind,
                MouseEventKind::Moved | MouseEventKind::Up(_) | MouseEventKind::Drag(_)
            )
        {
            p.state = shortcuts::State::Browse;
            p.note = Some("not changed".into());
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => self.close_modal(),
            MouseEventKind::Down(MouseButton::Left) if !inside => self.close_modal(),
            MouseEventKind::Down(MouseButton::Left) => {
                let row = self.hit.modal_rows.iter().find(|(y, _)| *y == m.row).map(|r| r.1);
                if matches!(self.modal, Some(Modal::Stickers(_))) {
                    return self.stickers_click(at);
                }
                if let Some(&(_, yes)) = self.hit.confirm_buttons.iter().find(|(r, _)| r.contains(at))
                    && let Some(Modal::Confirm(c)) = self.modal.as_mut()
                {
                    c.yes = yes;
                    return self.modal_cmd(Cmd::Confirm);
                }
                let Some(i) = row else { return };
                match self.modal.as_mut() {
                    Some(Modal::Menu(menu)) => {
                        menu.sel = i;
                        self.modal_cmd(Cmd::Confirm);
                    }
                    Some(Modal::Pick(p)) => {
                        p.sel = i;
                        self.modal_cmd(Cmd::Toggle);
                    }
                    Some(Modal::Form(f)) => f.sel = i.min(f.fields.len() - 1),
                    Some(Modal::Attach(a)) => {
                        if a.sel == i {
                            self.modal_cmd(Cmd::Confirm);
                        } else {
                            a.sel = i;
                        }
                    }
                    Some(Modal::Settings(s)) => {
                        if s.sel == i {
                            self.modal_cmd(Cmd::Confirm);
                        } else {
                            s.sel = i;
                        }
                    }
                    Some(Modal::Plugins(m)) => {
                        if m.sel() == i {
                            self.modal_cmd(Cmd::Confirm);
                        } else {
                            m.select(i);
                        }
                    }
                    Some(Modal::Shortcuts(p)) => {
                        if p.sel == i {
                            self.modal_cmd(Cmd::Confirm);
                        } else {
                            p.sel = i;
                            p.note = None;
                        }
                    }
                    Some(Modal::Find(f)) => {
                        f.sel = i;
                        self.modal_cmd(Cmd::Confirm);
                    }
                    _ => {}
                }
            }
            // The wheel stops at the ends of a menu instead of wrapping around.
            MouseEventKind::ScrollUp => match self.modal.as_mut() {
                Some(Modal::Menu(menu)) => menu.sel = menu.sel.saturating_sub(1),
                // A plugin's view hears the arrow itself.
                Some(Modal::PluginView(_)) => {
                    self.modal_input(None, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                }
                _ => self.modal_cmd(Cmd::Up),
            },
            MouseEventKind::ScrollDown => match self.modal.as_mut() {
                Some(Modal::Menu(menu)) => menu.sel = (menu.sel + 1).min(menu.items.len().saturating_sub(1)),
                Some(Modal::PluginView(_)) => {
                    self.modal_input(None, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                }
                _ => self.modal_cmd(Cmd::Down),
            },
            _ => {}
        }
    }

    // -- drawing ------------------------------------------------------------------------

    fn modal_frame(&mut self, f: &mut Frame, area: Rect, title: &str, help: &str) -> Rect {
        crate::picture::cover(f, area);
        self.hit.modal = Some(area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(title.to_owned(), Style::new().bold()))
            .title_bottom(frame_hint(help, area));
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    }

    fn draw_text_modal(
        &mut self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        lines: &[String],
        scroll: &mut usize,
        hint: String,
    ) {
        let want = lines.iter().map(|l| l.width()).max().unwrap_or(20).clamp(24, 70) + 4;
        let text_w = popup(area, want as u16, 1).width.saturating_sub(2) as usize;
        let body: Vec<String> = lines.iter().flat_map(|l| wrap(l, text_w)).collect();
        let r = popup(area, want as u16, body.len() as u16 + 2);
        let h = r.height.saturating_sub(2) as usize;
        *scroll = (*scroll).min(body.len().saturating_sub(h));
        let help = if body.len() > h {
            format!(" {}/{} · {hint} ", *scroll + h, body.len())
        } else {
            hint
        };
        let inner = self.modal_frame(f, r, title, &help);
        let shown: Vec<Line> = body.into_iter().skip(*scroll).take(h).map(Line::raw).collect();
        f.render_widget(Paragraph::new(shown), inner);
    }

    pub fn draw_modal(&mut self, f: &mut Frame, area: Rect) {
        self.hit.modal = None;
        self.hit.modal_rows.clear();
        self.hit.confirm_buttons.clear();
        // Nothing behind the overlay is clickable.
        self.hit.links.clear();
        // Worked out while the overlay is still in place: it says what is available.
        let hint = self.hint();
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        match &mut modal {
            Modal::Menu(m) => {
                // A shortcut (after `\t`) sits right-aligned, 2 columns from the label.
                let w = m
                    .items
                    .iter()
                    .map(|(l, _)| match l.split_once('\t') {
                        Some((label, key)) => label.width() + key.width() + 2,
                        None => l.width(),
                    })
                    .max()
                    .unwrap_or(10)
                    .max(m.title.width())
                    + 8;
                let rows = m.items.len() + m.breaks.len();
                let r = popup(area, w as u16, rows as u16 + 2);
                let inner = self.modal_frame(f, r, &m.title, &hint);
                // Dividers only when everything fits; a scrolling menu is a plain list.
                let dividers = rows <= inner.height as usize;
                let first = if dividers {
                    0
                } else {
                    scroll_first(m.sel, inner.height as usize)
                };
                let label_w = inner.width.saturating_sub(2) as usize;
                let mut y = inner.y;
                for (i, (label, _)) in m.items.iter().enumerate().skip(first) {
                    if dividers && m.breaks.contains(&i) {
                        let rule = "─".repeat(inner.width as usize);
                        f.render_widget(
                            Paragraph::new(Span::styled(rule, theme::muted())),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                        y += 1;
                    }
                    if y >= inner.y + inner.height {
                        break;
                    }
                    self.hit.modal_rows.push((y, i));
                    let sel = i == m.sel;
                    let style = if sel {
                        Style::new().bg(theme::t().accent_soft).fg(theme::t().text).bold()
                    } else {
                        Style::new()
                    };
                    let key = if i < 9 { format!("{} ", i + 1) } else { "  ".into() };
                    let (label, shortcut) = label.split_once('\t').unwrap_or((label, ""));
                    // The shortcut gives way first when the box is narrow.
                    let shortcut = if label.width() + shortcut.width() + 2 <= label_w {
                        format!(" {shortcut} ")
                    } else {
                        String::new()
                    };
                    let text_w = label_w.saturating_sub(shortcut.width());
                    let key_style = if sel {
                        style.remove_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        theme::muted()
                    };
                    let line = Line::from(vec![
                        Span::styled(key, theme::muted()),
                        Span::styled(pad_to(&ellipsize(label, text_w), text_w), style),
                        Span::styled(shortcut, key_style),
                    ]);
                    f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
                    y += 1;
                }
            }
            Modal::Confirm(c) => {
                // Wrap to the box as it fits on screen, not as it would like to be.
                let text_w = popup(area, 58, 1).width.saturating_sub(4) as usize;
                let lines = wrap(&c.text, text_w);
                let r = popup(area, 58, lines.len() as u16 + 4);
                let inner = self.modal_frame(f, r, " Are you sure? ", &hint);
                // Keep the buttons visible even when the text has to be cut.
                let room = (inner.height as usize).saturating_sub(2);
                let shown = lines.len().min(room);
                let body: Vec<Line> = lines.into_iter().take(room).map(Line::raw).collect();
                f.render_widget(Paragraph::new(body), inner);
                let y = inner.y + shown as u16 + u16::from(inner.height as usize > shown + 1);
                if y < inner.y + inner.height {
                    let button = |what: &str, cmd| match keymap::shortcut(keymap::Ctx::Confirm, cmd) {
                        key if key.is_empty() => format!(" {what} "),
                        key => format!(" {what} ({key}) "),
                    };
                    let buttons = [
                        (button(c.action.verb(), keymap::Cmd::Yes), true),
                        (button("Cancel", keymap::Cmd::No), false),
                    ];
                    let mut x = inner.x;
                    for (label, yes) in buttons {
                        let focused = c.yes == yes;
                        let style = match (focused, yes) {
                            (true, true) => {
                                Style::new().bg(theme::t().danger).fg(theme::t().on_danger).bold()
                            }
                            (true, false) => {
                                Style::new().bg(theme::t().accent).fg(theme::t().on_accent).bold()
                            }
                            (false, _) => Style::new().bg(theme::t().surface_hi),
                        };
                        let w = (label.width() as u16).min((inner.x + inner.width).saturating_sub(x));
                        let rect = Rect::new(x, y, w, 1);
                        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
                        self.hit.confirm_buttons.push((rect, yes));
                        x = (x + w + 3).min(inner.x + inner.width);
                    }
                }
            }
            Modal::Reactions { msg, scroll } => {
                let (title, lines) = self.reaction_lines(msg);
                self.draw_text_modal(f, area, &title, &lines, scroll, hint.clone());
            }
            Modal::ReadBy { msg, scroll } => {
                let (title, lines) = self.read_by_lines(msg);
                self.draw_text_modal(f, area, &title, &lines, scroll, hint.clone());
            }
            Modal::Help { ctx, scroll, under } => {
                let view = match under.as_deref() {
                    Some(Modal::PluginView(v)) => Some(v),
                    _ => None,
                };
                let lines = self.help_lines(*ctx, view);
                // Every key is padded to the widest (up to 24 columns): room for
                // that, a space and the longest description, inside the border.
                let widest_key = lines
                    .iter()
                    .map(|(key, _)| key.width())
                    .max()
                    .unwrap_or(1)
                    .min(24);
                let widest_description = lines.iter().map(|(_, d)| d.width()).max().unwrap_or(0);
                let want = (widest_key + 1 + widest_description + 2).clamp(36, 100);
                let r = popup(area, want as u16, lines.len() as u16 + 2);
                let h = r.height.saturating_sub(2) as usize;
                *scroll = (*scroll).min(lines.len().saturating_sub(h));
                let title = match ctx {
                    keymap::Ctx::Keys => " Shortcut editor keys ".to_owned(),
                    ctx => format!(" {} shortcuts ", ctx.title()),
                };
                let inner = self.modal_frame(f, r, &title, &hint);
                let key_width = lines
                    .iter()
                    .map(|(key, _)| key.width())
                    .max()
                    .unwrap_or(1)
                    .min((inner.width as usize).saturating_sub(2).min(24))
                    .max(1);
                let description_width = (inner.width as usize).saturating_sub(key_width + 1);
                let shown: Vec<Line> = lines
                    .iter()
                    .skip(*scroll)
                    .take(h)
                    .map(|(key, description)| {
                        Line::from(vec![
                            Span::styled(
                                pad_to(&ellipsize(key, key_width), key_width),
                                Style::new().fg(theme::t().accent).bold(),
                            ),
                            Span::raw(" "),
                            Span::raw(ellipsize(description, description_width)),
                        ])
                    })
                    .collect();
                f.render_widget(Paragraph::new(shown), inner);
            }
            Modal::Form(form) => {
                let h = form.fields.len() as u16 * 3 + 2 + u16::from(form.error.is_some()) * 2;
                let r = popup(area, 72, h);
                let inner = self.modal_frame(f, r, &form.title, &hint);
                let mut y = inner.y;
                for (i, field) in form.fields.iter().enumerate() {
                    if y + 2 > inner.y + inner.height {
                        break;
                    }
                    let sel = i == form.sel;
                    let label_style = if sel {
                        Style::new().fg(theme::t().accent).bold()
                    } else {
                        Style::new().bold()
                    };
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(field.label.clone(), label_style),
                            Span::styled(format!("  {}", field.hint), theme::muted()),
                        ])),
                        Rect::new(inner.x, y, inner.width, 1),
                    );
                    self.hit.modal_rows.push((y, i));
                    self.hit.modal_rows.push((y + 1, i));
                    let (shown, cursor) = field.value.view(inner.width.saturating_sub(2) as usize);
                    let bg = if sel {
                        theme::t().surface_hi
                    } else {
                        theme::t().surface
                    };
                    f.render_widget(
                        Paragraph::new(format!(" {shown}")).style(Style::new().bg(bg)),
                        Rect::new(inner.x, y + 1, inner.width, 1),
                    );
                    if sel && inner.width > 1 {
                        let x = inner.x + 1 + cursor.min(inner.width - 2);
                        f.set_cursor_position(Position::new(x, y + 1));
                    }
                    y += 3;
                }
                if let Some(e) = &form.error {
                    f.render_widget(
                        Paragraph::new(e.as_str())
                            .style(Style::new().fg(theme::t().danger))
                            .wrap(Wrap { trim: true }),
                        Rect::new(
                            inner.x,
                            y,
                            inner.width,
                            inner.y + inner.height - y.min(inner.y + inner.height),
                        ),
                    );
                }
            }
            Modal::Pick(p) => {
                let r = popup(area, 60, (area.height * 4 / 5).max(10));
                let inner = self.modal_frame(f, r, &p.title, &hint);
                let [search, count, list] =
                    Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
                        .areas(inner);
                let (shown, cursor) = p.filter.view(search.width.saturating_sub(3) as usize);
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled("🔍 ", Style::new()),
                        Span::raw(shown.to_owned()),
                    ]))
                    .style(Style::new().bg(theme::t().surface)),
                    search,
                );
                f.set_cursor_position(Position::new(search.x + 3 + cursor, search.y));
                if !p.single {
                    let names: Vec<String> = p
                        .chosen
                        .iter()
                        .filter_map(|m| p.options.iter().find(|o| &o.0 == m).map(|o| o.1.clone()))
                        .collect();
                    let txt = if names.is_empty() {
                        "nobody selected yet".to_owned()
                    } else {
                        format!("{} selected: {}", names.len(), names.join(", "))
                    };
                    f.render_widget(
                        Paragraph::new(ellipsize(&txt, count.width as usize))
                            .style(Style::new().fg(theme::t().accent)),
                        count,
                    );
                }
                let vis = p.visible();
                let h = list.height as usize;
                let first = p.sel.saturating_sub(h.saturating_sub(1));
                for (row, &i) in vis.iter().skip(first).take(h).enumerate() {
                    let (mid, name) = &p.options[i];
                    let y = list.y + row as u16;
                    self.hit.modal_rows.push((y, first + row));
                    let ticked = p.chosen.contains(mid);
                    let mark = match (p.single, ticked) {
                        (true, _) => "  ",
                        (false, true) => "☑ ",
                        (false, false) => "☐ ",
                    };
                    let style = if first + row == p.sel {
                        Style::new().bg(theme::t().accent_soft).fg(theme::t().text).bold()
                    } else if ticked {
                        Style::new().fg(theme::t().accent)
                    } else {
                        Style::new()
                    };
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::raw(mark),
                            Span::styled(name.clone(), style),
                        ])),
                        Rect::new(list.x, y, list.width, 1),
                    );
                }
                if vis.is_empty() {
                    f.render_widget(Paragraph::new("no match").style(theme::muted()), list);
                }
            }
            Modal::Attach(_)
            | Modal::Stickers(_)
            | Modal::Settings(_)
            | Modal::Plugins(_)
            | Modal::PluginView(_)
            | Modal::Shortcuts(_)
            | Modal::Find(_) => {}
        }
        self.modal = Some(modal);
        match self.modal.take() {
            Some(Modal::Attach(mut a)) => {
                self.draw_attach(f, area, &mut a, hint);
                self.modal = Some(Modal::Attach(a));
            }
            Some(Modal::Stickers(mut s)) => {
                self.draw_stickers(f, area, &mut s, hint);
                self.modal = Some(Modal::Stickers(s));
            }
            Some(Modal::Settings(mut s)) => {
                self.draw_settings(f, area, &mut s, hint);
                self.modal = Some(Modal::Settings(s));
            }
            Some(Modal::Plugins(mut m)) => {
                self.draw_plugins(f, area, &mut m, hint);
                self.modal = Some(Modal::Plugins(m));
            }
            Some(Modal::PluginView(mut v)) => {
                self.draw_plugin_view(f, area, &mut v, hint);
                self.modal = Some(Modal::PluginView(v));
            }
            Some(Modal::Shortcuts(mut p)) => {
                self.draw_shortcuts(f, area, &mut p, hint);
                self.modal = Some(Modal::Shortcuts(p));
            }
            Some(Modal::Find(mut find)) => {
                self.draw_find(f, area, &mut find, hint);
                self.modal = Some(Modal::Find(find));
            }
            other => self.modal = other,
        }
    }
}

/// Where a scrollable box's first line is after `cmd` (clamped when drawn).
fn scrolled(cmd: Option<keymap::Cmd>, scroll: usize) -> usize {
    use keymap::Cmd;
    match cmd {
        Some(Cmd::Up) => scroll.saturating_sub(1),
        Some(Cmd::Down) => scroll + 1,
        Some(Cmd::PageUp) => scroll.saturating_sub(10),
        Some(Cmd::PageDown) => scroll + 10,
        Some(Cmd::Home) => 0,
        Some(Cmd::End) => usize::MAX / 2,
        _ => scroll,
    }
}

/// A key in a read-only text box: the new scroll position, `None` to close.
fn text_scroll(cmd: Option<keymap::Cmd>, scroll: usize) -> Option<usize> {
    match cmd {
        Some(keymap::Cmd::Back) => None,
        cmd => Some(scrolled(cmd, scroll)),
    }
}

/// The first row to draw of a list `h` rows high so that row `sel` is visible.
pub fn scroll_first(sel: usize, h: usize) -> usize {
    sel.saturating_sub(h.saturating_sub(1))
}

/// Pad `s` with spaces to `w` display columns (`format!` pads by chars, which
/// is wrong for emoji, CJK and Thai).
fn pad_to(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(s.width())))
}

fn mute_item(mid: &str, muted: bool) -> (String, Action) {
    if muted {
        (
            "🔔 Turn notifications on".to_owned(),
            Action::Mute(mid.into(), false),
        )
    } else {
        ("🔕 Mute notifications".to_owned(), Action::Mute(mid.into(), true))
    }
}

fn pin_item(mid: &str, pinned: bool) -> (String, Action) {
    (
        if pinned {
            "📌 Unpin chat"
        } else {
            "📌 Pin chat"
        }
        .to_owned(),
        Action::Pin(mid.into(), !pinned),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lists_scroll_to_the_selection() {
        assert_eq!(scroll_first(0, 5), 0);
        assert_eq!(scroll_first(4, 5), 0);
        assert_eq!(scroll_first(9, 5), 5);
        assert_eq!(scroll_first(3, 0), 3);
        // Padding counts display columns: 🔔 is two wide.
        assert_eq!(pad_to("🔔 a", 6).width(), 6);
        assert_eq!(pad_to("toolong", 3), "toolong");
    }

    #[test]
    fn coordinates() {
        assert_eq!(parse_lat_lon("13.7466, 100.5347"), Some((13.7466, 100.5347)));
        assert_eq!(parse_lat_lon("abc"), None);
    }

    #[test]
    fn pick_filters_by_name() {
        let p = Pick {
            title: String::new(),
            kind: PickKind::ShareContact,
            options: vec![
                ("U1".into(), "Alice".into()),
                ("U2".into(), "Bob".into()),
                ("U3".into(), "alicia".into()),
            ],
            chosen: vec![],
            filter: "ali".into(),
            sel: 0,
            single: true,
        };
        assert_eq!(p.visible(), [0, 2]);
    }
}
