//! Settings: this app's preferences (kept in the local store), LINE's
//! account-wide notification settings, keyboard shortcuts (each screen's
//! page is `shortcuts`), your account, plugins and storage; plus desktop
//! notifications for new messages (worded and shown by `notice`).
//!
//! Screens opened from a settings row (a form, the pin lists, a plugin's
//! page, a confirmation) come back to Settings when they close: see
//! `App::open_from_settings` and `App::settle_modal`.

use crate::*;
use rune::models::settings_attribute as sa;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMode {
    Off,
    /// Ring the terminal bell.
    Bell,
    /// A desktop notification (`notify-send` / `osascript` / a PowerShell
    /// toast on Windows, else the terminal's OSC 777 notification).
    Desktop,
}

impl NotifyMode {
    fn as_str(self) -> &'static str {
        match self {
            NotifyMode::Off => "off",
            NotifyMode::Bell => "bell",
            NotifyMode::Desktop => "desktop",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => NotifyMode::Off,
            "bell" => NotifyMode::Bell,
            "desktop" => NotifyMode::Desktop,
            _ => return None,
        })
    }
    fn next(self) -> Self {
        match self {
            NotifyMode::Off => NotifyMode::Bell,
            NotifyMode::Bell => NotifyMode::Desktop,
            NotifyMode::Desktop => NotifyMode::Off,
        }
    }
    fn label(self) -> &'static str {
        match self {
            NotifyMode::Off => "Off",
            NotifyMode::Bell => "Terminal bell",
            NotifyMode::Desktop => "Desktop notification",
        }
    }
}

/// This app's preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub notify: NotifyMode,
    /// Put the message text in notifications.
    pub notify_preview: bool,
    pub notify_groups: bool,
    /// Also notify while the chat is open and the terminal focused.
    pub notify_focused: bool,
    pub read_receipts: bool,
    /// Load pictures in chats automatically.
    pub auto_pictures: bool,
    pub thumb_size: usize,
    pub theme: theme::ThemeMode,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            notify: NotifyMode::Desktop,
            notify_preview: true,
            notify_groups: true,
            notify_focused: false,
            read_receipts: true,
            auto_pictures: true,
            thumb_size: 2,
            theme: theme::ThemeMode::Auto,
        }
    }
}

impl Prefs {
    pub fn load(store: &Store) -> Self {
        let d = Prefs::default();
        let get = |k: &str| store.get(&format!("setting.{k}")).ok().flatten();
        let flag = |k: &str, dflt: bool| get(k).map_or(dflt, |v| v == "true");
        Prefs {
            notify: get("notify")
                .and_then(|v| NotifyMode::parse(&v))
                .unwrap_or(d.notify),
            notify_preview: flag("notify_preview", d.notify_preview),
            notify_groups: flag("notify_groups", d.notify_groups),
            notify_focused: flag("notify_focused", d.notify_focused),
            read_receipts: flag("read_receipts", d.read_receipts),
            auto_pictures: flag("auto_pictures", d.auto_pictures),
            thumb_size: get("thumb_size")
                .and_then(|v| v.parse().ok())
                .filter(|&n| n < THUMB_SIZES.len())
                .unwrap_or(d.thumb_size),
            theme: get("theme")
                .and_then(|v| theme::ThemeMode::parse(&v))
                .unwrap_or(d.theme),
        }
    }

    pub fn save(&self, store: &Store) {
        let pairs = [
            ("notify", self.notify.as_str().to_owned()),
            ("notify_preview", self.notify_preview.to_string()),
            ("notify_groups", self.notify_groups.to_string()),
            ("notify_focused", self.notify_focused.to_string()),
            ("read_receipts", self.read_receipts.to_string()),
            ("auto_pictures", self.auto_pictures.to_string()),
            ("thumb_size", self.thumb_size.to_string()),
            ("theme", self.theme.as_str().to_owned()),
        ];
        for (k, v) in pairs {
            let _ = store.set(&format!("setting.{k}"), &v);
        }
    }
}

/// The tabs, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// This computer's notifications, then LINE's (which apply to the phone too).
    Notifications,
    Chats,
    /// The keys of every screen (each opens its own page).
    Shortcuts,
    /// Your display name, status message and profile page.
    Account,
    Plugins,
    Storage,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Notifications,
        Tab::Chats,
        Tab::Shortcuts,
        Tab::Account,
        Tab::Plugins,
        Tab::Storage,
    ];

    fn title(self) -> &'static str {
        match self {
            Tab::Notifications => "Notifications",
            Tab::Chats => "Chats",
            Tab::Shortcuts => "Shortcuts",
            Tab::Account => "Account",
            Tab::Plugins => "Plugins",
            Tab::Storage => "Storage",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    fn step(self, forward: bool) -> Tab {
        let n = Tab::ALL.len();
        let i = if forward {
            self.index() + 1
        } else {
            self.index() + n - 1
        };
        Tab::ALL[i % n]
    }
}

/// LINE's account notification switches: attribute, `Settings` field, label.
const SERVER_TOGGLES: [(i64, &str, &str); 7] = [
    (sa::NOTIFICATION_ENABLE, "notificationEnable", "Notifications"),
    (
        sa::NOTIFICATION_NEW_MESSAGE,
        "notificationNewMessage",
        "New messages",
    ),
    (
        sa::NOTIFICATION_SHOW_MESSAGE,
        "notificationShowMessage",
        "Show message text",
    ),
    (
        sa::NOTIFICATION_THUMBNAIL,
        "notificationThumbnail",
        "Show photo thumbnails",
    ),
    (
        sa::NOTIFICATION_GROUP_INVITATION,
        "notificationGroupInvitation",
        "Group invitations",
    ),
    (sa::NOTIFICATION_MENTION, "notificationMention", "Mentions"),
    (sa::NOTIFICATION_REACTION, "notificationReaction", "Reactions"),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowAct {
    NotifyMode,
    NotifyPreview,
    NotifyGroups,
    NotifyFocused,
    TestNotify,
    Server(usize),
    ProfilePage,
    ReadReceipts,
    AutoPictures,
    ThumbSize,
    Theme,
    DisplayName,
    StatusMessage,
    ClearCache,
    PinLists,
    /// One plugin's page, by index in the plugin list.
    Plugin(usize),
    /// Install a plugin from a Git repository.
    InstallPlugin,
    /// Check every Git-installed plugin for a new version.
    CheckAll,
    /// One screen's shortcuts, by index in `keymap::SECTIONS`.
    Shortcuts(usize),
    ResetShortcuts,
    /// Read `keys.json` again (after editing it by hand).
    ReloadKeys,
    None,
}

pub struct SettingsPage {
    pub tab: Tab,
    pub sel: usize,
}

fn on_off(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

impl App {
    pub fn open_settings(&mut self) {
        self.open_settings_at(Tab::Notifications);
    }

    pub fn open_settings_at(&mut self, tab: Tab) {
        self.modal_parent = None;
        // On the first row that does something (the tab may start with a note).
        let sel = self
            .settings_rows(tab)
            .iter()
            .position(|r| r.2 != RowAct::None)
            .unwrap_or(0);
        self.modal = Some(Modal::Settings(SettingsPage { tab, sel }));
        self.fetch_tab_data(tab);
    }

    /// Settings › Plugins with `plugin`'s row selected (where its page goes back to).
    pub fn open_settings_on_plugin(&mut self, plugin: &str) {
        self.open_settings_at(Tab::Plugins);
        let at = self.settings_rows(Tab::Plugins).iter().position(|r| {
            matches!(r.2, RowAct::Plugin(i) if self.plugins.entries.get(i).is_some_and(|e| e.id == plugin))
        });
        if let (Some(Modal::Settings(s)), Some(i)) = (self.modal.as_mut(), at) {
            s.sel = i;
        }
    }

    /// Open a screen over Settings (by `open`); Settings comes back when it closes.
    fn open_from_settings(&mut self, open: impl FnOnce(&mut App)) {
        self.modal_parent = self.modal.take().map(Box::new);
        open(self);
    }

    pub fn fetch_tab_data(&mut self, tab: Tab) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        if tab == Tab::Notifications
            && !matches!(self.server_settings, Some(Load::Ready(_)) | Some(Load::Loading))
        {
            self.server_settings = Some(Load::Loading);
            tokio::spawn(async move {
                let attrs: Vec<i64> = SERVER_TOGGLES.iter().map(|t| t.0).collect();
                let res = client
                    .settings_attributes(&attrs)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::ServerSettings(res));
            });
        } else if tab == Tab::Account && self.my_profile.is_none() {
            self.my_profile = Some(Load::Loading);
            tokio::spawn(async move {
                let res = client.profile().await.map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::MyProfile(res));
            });
        }
    }

    /// The rows of a tab: label, value, what Enter does.
    fn settings_rows(&self, tab: Tab) -> Vec<(String, String, RowAct)> {
        let p = &self.prefs;
        let note = |text: &str| (text.to_owned(), String::new(), RowAct::None);
        match tab {
            Tab::Notifications => {
                let mut rows = vec![
                    note("On this computer"),
                    ("New messages".into(), p.notify.label().into(), RowAct::NotifyMode),
                    (
                        "Show message text".into(),
                        on_off(p.notify_preview),
                        RowAct::NotifyPreview,
                    ),
                    (
                        "Group chats".into(),
                        on_off(p.notify_groups),
                        RowAct::NotifyGroups,
                    ),
                    (
                        "Even for the open chat".into(),
                        on_off(p.notify_focused),
                        RowAct::NotifyFocused,
                    ),
                    ("Send a test notification".into(), "↵".into(), RowAct::TestNotify),
                    note("Muted chats never notify (mute one from its menu)."),
                    note(""),
                    note("On LINE (these apply to your phone too)"),
                ];
                match &self.server_settings {
                    Some(Load::Ready(v)) => {
                        for (i, (_, field, label)) in SERVER_TOGGLES.iter().enumerate() {
                            let val = v.get(*field).and_then(|b| b.as_bool());
                            rows.push(((*label).into(), val.map_or("?".into(), on_off), RowAct::Server(i)));
                        }
                        let muted = v.get("notificationMuteExpiration").and_then(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .or_else(|| x.as_i64().map(|n| n.to_string()))
                        });
                        if let Some(m) = muted
                            .and_then(|m| m.parse::<i64>().ok())
                            .filter(|&m| m > now_ms())
                        {
                            rows.push(("Paused until".into(), hhmm(m), RowAct::None));
                        }
                    }
                    Some(Load::Failed(e)) => {
                        rows.push((format!("could not load: {e}"), String::new(), RowAct::None))
                    }
                    _ => rows.push(("loading…".into(), String::new(), RowAct::None)),
                }
                rows
            }
            Tab::Chats => {
                let receipts = if self.no_read_receipts_flag {
                    "Off (--no-read-receipts)".into()
                } else {
                    on_off(p.read_receipts)
                };
                vec![
                    ("Send read receipts".into(), receipts, RowAct::ReadReceipts),
                    (
                        "Load pictures automatically".into(),
                        on_off(p.auto_pictures),
                        RowAct::AutoPictures,
                    ),
                    (
                        "Picture size".into(),
                        format!("{}/{}", self.thumb_size + 1, THUMB_SIZES.len()),
                        RowAct::ThumbSize,
                    ),
                    ("Colour theme".into(), p.theme.label().into(), RowAct::Theme),
                    (
                        "Pin lists…".into(),
                        self.store
                            .active_pin_profile()
                            .map(|p| p.name)
                            .unwrap_or_default(),
                        RowAct::PinLists,
                    ),
                    (
                        "Typing indicators are shown when LINE sends them;".into(),
                        String::new(),
                        RowAct::None,
                    ),
                    (
                        "the Chrome protocol cannot send yours.".into(),
                        String::new(),
                        RowAct::None,
                    ),
                ]
            }
            Tab::Shortcuts => {
                let mut rows = Vec::new();
                if self.default_keys {
                    rows.push(note("This run uses the default keys (--default-keys);"));
                    rows.push(note("changes made here apply from the next start."));
                }
                rows.extend(keymap::SECTIONS.iter().enumerate().map(|(i, s)| {
                    (
                        s.title.to_owned(),
                        self.shortcuts_summary(s),
                        RowAct::Shortcuts(i),
                    )
                }));
                if !self.keys.is_default() {
                    rows.push((
                        "Put back every default shortcut…".into(),
                        "↵".into(),
                        RowAct::ResetShortcuts,
                    ));
                }
                if let Some(file) = self.keys_file_shown() {
                    rows.push(("Read keys.json again".into(), "↵".into(), RowAct::ReloadKeys));
                    rows.push(note(""));
                    rows.push(note(&format!(
                        "Also kept in {file}: edit it, then read it again."
                    )));
                } else {
                    rows.push(note(""));
                }
                for problem in &self.key_problems {
                    rows.push(note(&format!("Not used: {problem}")));
                }
                rows.push(note(
                    "Enter shows a screen's shortcuts: change, add or reset each key.",
                ));
                rows.push(note("Alt with a letter or digit is kept for plugins."));
                rows
            }
            Tab::Account => {
                let (name, status) = match &self.my_profile {
                    Some(Load::Ready((n, s))) => (n.clone(), s.clone()),
                    Some(Load::Failed(e)) => (format!("? ({e})"), String::new()),
                    _ => ("…".into(), "…".into()),
                };
                vec![
                    ("Display name".into(), name, RowAct::DisplayName),
                    ("Status message".into(), status, RowAct::StatusMessage),
                    (
                        "Profile page (photo, cover)".into(),
                        "↵".into(),
                        RowAct::ProfilePage,
                    ),
                    (
                        "Friends see changes to your profile.".into(),
                        String::new(),
                        RowAct::None,
                    ),
                ]
            }
            Tab::Plugins => {
                let plugins = &self.plugins;
                let mut rows = Vec::new();
                if plugins.safe_mode {
                    rows.push(note("Plugins are off for this run (--no-plugins)."));
                }
                rows.extend(plugins.entries.iter().enumerate().map(|(i, e)| {
                    let version = e
                        .manifest
                        .as_ref()
                        .map(|m| format!("  {}", m.version))
                        .unwrap_or_default();
                    let state = match e.state.is_problem() {
                        true => format!("⚠ {}", e.state.label()),
                        false => e.state.label().to_owned(),
                    };
                    (format!("{}{version}", e.name()), state, RowAct::Plugin(i))
                }));
                if plugins.entries.is_empty() {
                    rows.push(note("No plugins installed."));
                }
                if plugins.dir.is_some() {
                    let busy = self.plugin_job.clone().unwrap_or_else(|| "↵".into());
                    rows.push((
                        "Install from a Git repository…".into(),
                        busy.clone(),
                        RowAct::InstallPlugin,
                    ));
                    if plugins.entries.iter().any(|e| e.source.is_some()) {
                        rows.push(("Check all for new versions".into(), busy, RowAct::CheckAll));
                    }
                }
                rows.push(note(""));
                rows.push(match &plugins.dir {
                    Some(d) => note(&format!("Installed in {}", d.display())),
                    None => note("No plugin directory (set --plugins DIR)."),
                });
                rows.push(note("Plugins add commands, keys and views. Each runs only once"));
                rows.push(note(
                    "you enable it on its page (Enter) and allow what it asks for.",
                ));
                rows.push(note("Changed files are picked up while rune runs."));
                rows
            }
            Tab::Storage => {
                let (chats, msgs, contacts) = self.store.stats().unwrap_or_default();
                let path = self
                    .store
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "in memory (not saved)".into());
                let size = self
                    .store
                    .path()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
                    .unwrap_or_default();
                let rev = self
                    .store
                    .stream_revision()
                    .ok()
                    .flatten()
                    .map_or("—".into(), |r| r.to_string());
                vec![
                    ("Database".into(), path, RowAct::None),
                    ("Size".into(), size, RowAct::None),
                    (
                        "Chats · messages · contacts".into(),
                        format!("{chats} · {msgs} · {contacts}"),
                        RowAct::None,
                    ),
                    ("Stream position".into(), rev, RowAct::None),
                    (
                        "Messages are stored decrypted; keep this file private.".into(),
                        String::new(),
                        RowAct::None,
                    ),
                    ("Clear local data…".into(), "↵".into(), RowAct::ClearCache),
                ]
            }
        }
    }

    /// The labels of a tab's rows.
    #[cfg(test)]
    pub fn settings_rows_for_test(&self, tab: Tab) -> Vec<String> {
        self.settings_rows(tab).into_iter().map(|r| r.0).collect()
    }

    pub fn settings_key(&mut self, mut s: SettingsPage, cmd: Option<keymap::Cmd>) {
        let rows = self.settings_rows(s.tab);
        let selectable: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].2 != RowAct::None).collect();
        match cmd {
            Some(keymap::Cmd::Back) => return,
            Some(cmd @ (keymap::Cmd::PrevTab | keymap::Cmd::NextTab)) => {
                s.tab = s.tab.step(cmd == keymap::Cmd::NextTab);
                s.sel = 0;
                self.fetch_tab_data(s.tab);
            }
            Some(keymap::Cmd::Up) => {
                let pos = selectable.iter().position(|&i| i == s.sel).unwrap_or(0);
                s.sel = selectable.get(pos.saturating_sub(1)).copied().unwrap_or(0);
            }
            Some(keymap::Cmd::Down) => {
                let pos = selectable.iter().position(|&i| i == s.sel);
                s.sel = match pos {
                    Some(p) => selectable.get(p + 1).copied().unwrap_or(s.sel),
                    None => selectable.first().copied().unwrap_or(0),
                };
            }
            Some(cmd @ (keymap::Cmd::PageUp | keymap::Cmd::PageDown)) => {
                // A page is as many rows as the last frame could choose from.
                let page = self.hit.modal_rows.len().max(1);
                let pos = selectable.iter().position(|&i| i == s.sel).unwrap_or(0);
                let to = if cmd == keymap::Cmd::PageUp {
                    pos.saturating_sub(page)
                } else {
                    (pos + page).min(selectable.len().saturating_sub(1))
                };
                s.sel = selectable.get(to).copied().unwrap_or(0);
            }
            Some(keymap::Cmd::Home) => s.sel = selectable.first().copied().unwrap_or(0),
            Some(keymap::Cmd::End) => s.sel = selectable.last().copied().unwrap_or(0),
            Some(keymap::Cmd::Confirm) => {
                let act = rows.get(s.sel).map_or(RowAct::None, |r| r.2);
                let act = if act == RowAct::None {
                    // Nothing selected yet: act on the first real row.
                    selectable.first().map_or(RowAct::None, |&i| {
                        s.sel = i;
                        rows[i].2
                    })
                } else {
                    act
                };
                self.modal = Some(Modal::Settings(s));
                self.settings_act(act);
                return;
            }
            _ => {}
        }
        if rows.get(s.sel).is_none_or(|r| r.2 == RowAct::None)
            && let Some(&first) = selectable.first()
        {
            s.sel = first;
        }
        self.modal = Some(Modal::Settings(s));
    }

    fn settings_act(&mut self, act: RowAct) {
        match act {
            RowAct::NotifyMode => self.prefs.notify = self.prefs.notify.next(),
            RowAct::NotifyPreview => self.prefs.notify_preview ^= true,
            RowAct::NotifyGroups => self.prefs.notify_groups ^= true,
            RowAct::NotifyFocused => self.prefs.notify_focused ^= true,
            RowAct::TestNotify => {
                let mode = if self.prefs.notify == NotifyMode::Off {
                    NotifyMode::Desktop
                } else {
                    self.prefs.notify
                };
                // With your own picture, so the icon is tested too.
                let me = vec![self.me.clone()];
                self.notify_later_as(mode, &[], me, None, |_| {
                    Some(notice::Notice::new(
                        "rune",
                        "Notifications are working",
                        notice::Category::Other,
                    ))
                });
                self.status = format!("sent a test ({})", mode.label());
            }
            RowAct::Server(i) => {
                let (attr, field, label) = SERVER_TOGGLES[i];
                let Some(Load::Ready(v)) = self.server_settings.as_mut() else {
                    return;
                };
                let new = !v.get(field).and_then(|b| b.as_bool()).unwrap_or(false);
                // Optimistic; reverted if LINE refuses.
                v[field] = serde_json::Value::Bool(new);
                self.status = format!("{label}: {}…", on_off(new));
                let (client, tx) = (self.client.clone(), self.tx.clone());
                tokio::spawn(async move {
                    let res = client
                        .update_settings(&[attr], serde_json::json!({ field: new }))
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppEvent::ServerSettingSaved(field, new, res));
                });
            }
            RowAct::ReadReceipts if self.no_read_receipts_flag => {
                self.status = "read receipts are off for this run (--no-read-receipts)".into();
            }
            RowAct::ReadReceipts => {
                self.prefs.read_receipts ^= true;
                self.receipts_switched();
            }
            RowAct::AutoPictures => self.prefs.auto_pictures ^= true,
            RowAct::ThumbSize => {
                self.thumb_size = (self.thumb_size + 1) % THUMB_SIZES.len();
                self.prefs.thumb_size = self.thumb_size;
            }
            RowAct::Theme => {
                self.prefs.theme = self.prefs.theme.next();
                theme::apply(self.prefs.theme);
            }
            RowAct::DisplayName | RowAct::StatusMessage => {
                let (name, status) = match &self.my_profile {
                    Some(Load::Ready(p)) => p.clone(),
                    _ => (String::new(), String::new()),
                };
                let (kind, title, label, value, hint) = if act == RowAct::DisplayName {
                    (
                        modal::FormKind::DisplayName,
                        " Display name ",
                        "Name",
                        name,
                        "how friends see you",
                    )
                } else {
                    (
                        modal::FormKind::StatusMessage,
                        " Status message ",
                        "Status",
                        status,
                        "shown on your profile; empty clears it",
                    )
                };
                let form = modal::Form {
                    title: title.into(),
                    kind,
                    fields: vec![modal::Field {
                        label: label.into(),
                        value: value.into(),
                        hint: hint.into(),
                    }],
                    sel: 0,
                    error: None,
                };
                return self.open_from_settings(|app| app.modal = Some(Modal::Form(form)));
            }
            RowAct::ProfilePage => {
                // The profile page is not an overlay: Settings closes.
                self.modal = None;
                self.modal_parent = None;
                let me = self.me.clone();
                return self.open_profile(me);
            }
            RowAct::ClearCache => {
                return self.open_from_settings(|app| {
                    app.confirm(
                        "Delete all locally stored chats, messages and contacts? They are fetched \
                         from LINE again (only as much history as LINE still serves).",
                        modal::Action::ClearCache,
                    )
                });
            }
            RowAct::PinLists => return self.open_from_settings(App::pin_lists_menu),
            RowAct::Plugin(i) => {
                let Some(id) = self.plugins.entries.get(i).map(|e| e.id.clone()) else {
                    return;
                };
                return self.open_from_settings(|app| app.open_plugin_page(id));
            }
            RowAct::InstallPlugin => {
                if let Some(job) = &self.plugin_job {
                    self.status = format!("wait: {job}");
                    return;
                }
                return self.open_from_settings(App::install_plugin_form);
            }
            RowAct::CheckAll => return self.check_all_plugins(),
            RowAct::Shortcuts(i) => return self.open_from_settings(|app| app.open_shortcuts(i)),
            RowAct::ReloadKeys => {
                self.status = self.read_keys_file();
                return;
            }
            RowAct::ResetShortcuts => {
                let n: usize = keymap::SECTIONS.iter().map(|s| self.keys.changed_in(s)).sum();
                return self.open_from_settings(|app| {
                    app.confirm(
                        format!("Put back the default keys for every screen? Your {n} changed shortcuts are forgotten."),
                        modal::Action::ResetShortcuts,
                    )
                });
            }
            RowAct::None => return,
        }
        self.prefs.save(&self.store);
    }

    pub fn draw_settings(&mut self, f: &mut Frame, area: Rect, s: &mut SettingsPage, hint: String) {
        let r = modal::popup(area, 80, 18);
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(" ⚙ Settings ", Style::new().bold()))
            .title_bottom(frame_hint(&hint, r));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [tabs, _, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        let mut spans = Vec::new();
        for t in Tab::ALL {
            let style = if t == s.tab {
                Style::new().bg(theme::t().accent).fg(theme::t().on_accent).bold()
            } else {
                theme::muted()
            };
            spans.push(Span::styled(format!(" {} ", t.title()), style));
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), tabs);
        let rows = self.settings_rows(s.tab);
        if rows.get(s.sel).is_none_or(|r| r.2 == RowAct::None)
            && let Some(i) = rows.iter().position(|r| r.2 != RowAct::None)
        {
            s.sel = i;
        }
        let first = modal::scroll_first(s.sel, body.height as usize);
        for (row, (label, value, act)) in rows.iter().skip(first).take(body.height as usize).enumerate() {
            let i = first + row;
            let y = body.y + row as u16;
            let note = *act == RowAct::None && value.is_empty();
            // Only rows that do something can be clicked (and selected).
            if *act != RowAct::None {
                self.hit.modal_rows.push((y, i));
            }
            let sel = i == s.sel && !note;
            let vw = value.width().min(body.width as usize / 2);
            let value = ellipsize(value, vw);
            let lw = (body.width as usize).saturating_sub(vw + 3);
            let label = ellipsize(label, lw);
            let pad = (body.width as usize).saturating_sub(label.width() + value.width() + 2);
            let (ls, vs) = if sel {
                (
                    Style::new().bg(theme::t().accent_soft).fg(theme::t().text),
                    Style::new().bg(theme::t().accent_soft).fg(theme::t().text).bold(),
                )
            } else if note {
                (theme::muted().italic(), theme::muted())
            } else {
                (Style::new(), Style::new().fg(theme::t().accent))
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {label}{}", " ".repeat(pad)), ls),
                    Span::styled(format!("{value} "), vs),
                ])),
                Rect::new(body.x, y, body.width, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(Prefs::load(&store), Prefs::default());
        let p = Prefs {
            notify: NotifyMode::Bell,
            notify_preview: false,
            read_receipts: false,
            thumb_size: 1,
            theme: theme::ThemeMode::Light,
            ..Prefs::default()
        };
        p.save(&store);
        assert_eq!(Prefs::load(&store), p);
        assert_eq!(NotifyMode::Desktop.next(), NotifyMode::Off);
    }

    #[test]
    fn tabs_wrap_around_both_ways() {
        assert_eq!(Tab::Storage.step(true), Tab::Notifications);
        assert_eq!(Tab::Notifications.step(false), Tab::Storage);
        assert_eq!(Tab::Chats.step(true), Tab::Shortcuts);
        assert_eq!(Tab::Shortcuts.step(true), Tab::Account);
    }
}
