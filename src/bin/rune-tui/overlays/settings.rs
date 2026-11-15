//! Settings: this app's preferences (kept in the local store), LINE's
//! account-wide notification settings, profile, and storage; plus desktop
//! notifications for new messages.

use crate::*;
use rune::models::settings_attribute as sa;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMode {
    Off,
    /// Ring the terminal bell.
    Bell,
    /// A desktop notification (`notify-send` / `osascript`, else the
    /// terminal's OSC 777 notification).
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
        ];
        for (k, v) in pairs {
            let _ = store.set(&format!("setting.{k}"), &v);
        }
    }
}

/// Show a notification (off the UI thread for external commands).
pub fn notify(mode: NotifyMode, title: &str, body: &str) {
    use std::io::Write;
    match mode {
        NotifyMode::Off => {}
        NotifyMode::Bell => {
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\x07");
            let _ = out.flush();
        }
        NotifyMode::Desktop => {
            let spawned = if cfg!(target_os = "macos") {
                let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                std::process::Command::new("osascript")
                    .arg("-e")
                    .arg(format!(
                        "display notification \"{}\" with title \"{}\"",
                        esc(body),
                        esc(title)
                    ))
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            } else {
                std::process::Command::new("notify-send")
                    .args(["--app-name=rune", "--", title, body])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            };
            if spawned.is_err() {
                // OSC 777 (urxvt, foot, WezTerm, Ghostty, …).
                let clean = |s: &str| s.replace([';', '\x07', '\x1b', '\n'], " ");
                let mut out = std::io::stdout();
                let _ = write!(out, "\x1b]777;notify;{};{}\x07", clean(title), clean(body));
                let _ = out.flush();
            }
        }
    }
}

const TABS: [&str; 5] = ["Notifications", "LINE account", "Chats", "Profile", "Storage"];

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
    DisplayName,
    StatusMessage,
    ClearCache,
    None,
}

pub struct SettingsPage {
    pub tab: usize,
    pub sel: usize,
}

fn on_off(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

impl App {
    pub fn open_settings(&mut self) {
        self.open_settings_at(0);
    }

    pub fn open_settings_at(&mut self, tab: usize) {
        self.modal = Some(Modal::Settings(SettingsPage { tab, sel: 0 }));
        self.fetch_tab_data(tab);
    }

    fn fetch_tab_data(&mut self, tab: usize) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        if tab == 1 && !matches!(self.server_settings, Some(Load::Ready(_)) | Some(Load::Loading)) {
            self.server_settings = Some(Load::Loading);
            tokio::spawn(async move {
                let attrs: Vec<i64> = SERVER_TOGGLES.iter().map(|t| t.0).collect();
                let res = client
                    .settings_attributes(&attrs)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::ServerSettings(res));
            });
        } else if tab == 3 && self.my_profile.is_none() {
            self.my_profile = Some(Load::Loading);
            tokio::spawn(async move {
                let res = client.profile().await.map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::MyProfile(res));
            });
        }
    }

    /// The rows of a tab: label, value, what Enter does.
    fn settings_rows(&self, tab: usize) -> Vec<(String, String, RowAct)> {
        let p = &self.prefs;
        match tab {
            0 => vec![
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
                (
                    "Muted chats never notify (mute one from its menu).".into(),
                    String::new(),
                    RowAct::None,
                ),
            ],
            1 => {
                let mut rows = vec![(
                    "These are LINE's settings: they apply to your phone too.".into(),
                    String::new(),
                    RowAct::None,
                )];
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
            2 => {
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
            3 => {
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
            _ => {
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

    pub fn settings_key(&mut self, mut s: SettingsPage, k: KeyEvent) {
        let rows = self.settings_rows(s.tab);
        let selectable: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].2 != RowAct::None).collect();
        match keymap::resolve(keymap::Ctx::Settings, &k) {
            Some(keymap::Cmd::Back) => return,
            Some(keymap::Cmd::PrevTab) => {
                s.tab = s.tab.checked_sub(1).unwrap_or(TABS.len() - 1);
                s.sel = 0;
                self.fetch_tab_data(s.tab);
            }
            Some(keymap::Cmd::NextTab) => {
                s.tab = (s.tab + 1) % TABS.len();
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
                notify(mode, "rune", "Notifications work 🎉");
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
            RowAct::ReadReceipts => self.prefs.read_receipts ^= true,
            RowAct::AutoPictures => self.prefs.auto_pictures ^= true,
            RowAct::ThumbSize => {
                self.thumb_size = (self.thumb_size + 1) % THUMB_SIZES.len();
                self.prefs.thumb_size = self.thumb_size;
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
                self.modal = Some(Modal::Form(modal::Form {
                    title: title.into(),
                    kind,
                    fields: vec![modal::Field {
                        label,
                        value: value.into(),
                        hint,
                    }],
                    sel: 0,
                    error: None,
                }));
                return;
            }
            RowAct::ProfilePage => {
                self.modal = None;
                let me = self.me.clone();
                return self.open_profile(me);
            }
            RowAct::ClearCache => {
                return self.confirm(
                    "Delete all locally stored chats, messages and contacts? They are fetched \
                     from LINE again (only as much history as LINE still serves).",
                    modal::Action::ClearCache,
                );
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
            .border_style(Style::new().fg(Color::Green))
            .title(Span::styled(" ⚙ Settings ", Style::new().bold()))
            .title_bottom(Span::styled(hint, Style::new().dim()));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [tabs, _, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        let mut spans = Vec::new();
        for (i, t) in TABS.iter().enumerate() {
            let style = if i == s.tab {
                Style::new().bg(Color::Green).fg(Color::Black).bold()
            } else {
                Style::new().dim()
            };
            spans.push(Span::styled(format!(" {t} "), style));
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
                    Style::new().bg(Color::Green).fg(Color::Black),
                    Style::new().bg(Color::Green).fg(Color::Black).bold(),
                )
            } else if note {
                (Style::new().dim().italic(), Style::new().dim())
            } else {
                (Style::new(), Style::new().fg(Color::Green))
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
            ..Prefs::default()
        };
        p.save(&store);
        assert_eq!(Prefs::load(&store), p);
        assert_eq!(NotifyMode::Desktop.next(), NotifyMode::Off);
    }
}
