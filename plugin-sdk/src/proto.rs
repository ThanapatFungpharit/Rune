//! The messages between host and plugin (JSON, `type`-tagged).
//!
//! Compatibility rules: a minor API version may add event types, action
//! types and optional fields. Plugins ignore events they do not know
//! ([`Event::Unknown`]); the host refuses actions it does not know.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One host → plugin message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Wall-clock time, milliseconds since the Unix epoch.
    pub now: i64,
    pub event: Event,
}

/// What happened. Events marked *interactive* come from something the user
/// just did; only replies to those may open views or forms or edit the
/// message line, so a plugin never takes the keyboard by surprise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The plugin was activated (at start-up or when enabled).
    Init {
        /// Every setting from the manifest, validated (defaults filled in).
        config: Map<String, Value>,
        /// The plugin's stored data (needs the `storage` permission).
        #[serde(default)]
        data: BTreeMap<String, String>,
        /// The host's name and version, e.g. `rune-tui 0.1.0`.
        host: String,
    },
    /// The plugin is about to be deactivated (disabled or the app quits).
    /// Only `status` and `store_*` actions still take effect.
    Shutdown,
    /// The user changed a setting (the whole new set).
    ConfigChanged { config: Map<String, Value> },
    /// *Interactive*: `/name ARGS` typed on the message line.
    Command {
        name: String,
        args: String,
        /// The open chat, if any.
        chat: Option<ChatRef>,
    },
    /// *Interactive*: one of the manifest's keys was pressed.
    Key { id: String, chat: Option<ChatRef> },
    /// *Interactive*: a key pressed in the plugin's open view. `key` is a
    /// character (`a`, `A`, `?` is reserved) or one of `Enter`, `Up`,
    /// `Down`, `Left`, `Right`, `Home`, `End`, `Tab`, `Shift-Tab`,
    /// `Backspace`, `Delete`, `Space`.
    ViewKey {
        view: String,
        key: String,
        /// The highlighted line, if the view has a selection.
        selected: Option<usize>,
    },
    /// The user closed the plugin's view.
    ViewClosed { view: String },
    /// *Interactive*: a form opened with [`Action::Form`] was submitted.
    FormSubmitted { form: String, values: Vec<String> },
    /// The form was cancelled.
    FormCancelled { form: String },
    /// A new message in any chat (needs `read_messages`).
    Message { chat: ChatRef, message: MessageInfo },
    /// The user opened a chat (needs `read_messages`).
    ChatOpened { chat: ChatRef },
    /// A timer set with [`Action::SetTimer`] went off.
    Timer { id: String },
    /// An event from a newer host: ignore it.
    #[serde(other)]
    Unknown,
}

impl Event {
    /// Whether the event comes straight from something the user did.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Event::Command { .. } | Event::Key { .. } | Event::ViewKey { .. } | Event::FormSubmitted { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    User,
    Group,
    Room,
    Other,
}

/// A chat, as plugins see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRef {
    pub mid: String,
    pub name: String,
    pub kind: ChatKind,
}

/// A message, as plugins see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    pub from: String,
    pub from_name: String,
    /// The text of a text message (decrypted), `None` for other kinds.
    pub text: Option<String>,
    /// How rune shows it in one line (`[photo]`, `[sticker]`, the text…).
    pub summary: String,
    /// Milliseconds since the Unix epoch.
    pub time: i64,
    /// Sent by the user (from this or another device).
    pub mine: bool,
}

/// One plugin → host reply.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// Something the plugin wants done. The permission each needs is listed;
/// actions without one are always allowed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// A line on the status bar.
    Status {
        text: String,
    },
    /// A desktop notification, as set up in Settings (`notify`).
    Notify {
        title: String,
        body: String,
    },
    /// Show (or, with the same `id`, replace) the plugin's view: a box of
    /// lines over the chat. Interactive replies only, unless that view is
    /// already open.
    OpenView {
        id: String,
        title: String,
        lines: Vec<ViewLine>,
        /// Highlight this line (kept on screen); keys then report it.
        #[serde(default)]
        selected: Option<usize>,
        /// The view's keys, for the hint line and the help screen.
        #[serde(default)]
        keys: Vec<KeyHelp>,
    },
    CloseView {
        id: String,
    },
    /// Ask for text; answered with [`Event::FormSubmitted`] or
    /// [`Event::FormCancelled`]. Interactive replies only.
    Form {
        id: String,
        title: String,
        fields: Vec<FormField>,
    },
    /// Insert text at the message line's cursor (`compose`). Interactive
    /// replies only.
    InsertText {
        text: String,
    },
    /// Send a text message (`send_messages`): to the open chat or a chat in
    /// the chat list. Limited to a few per minute.
    SendMessage {
        chat: String,
        text: String,
    },
    /// Keep a value (`storage`); handed back in [`Event::Init`].
    StoreSet {
        key: String,
        value: String,
    },
    StoreDelete {
        key: String,
    },
    /// Get a [`Event::Timer`] after `after_ms` (at least 1 s), once or
    /// every `after_ms`. Setting an existing id replaces it.
    SetTimer {
        id: String,
        after_ms: u64,
        #[serde(default)]
        repeat: bool,
    },
    CancelTimer {
        id: String,
    },
}

impl Action {
    pub fn status(text: impl Into<String>) -> Self {
        Action::Status { text: text.into() }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStyle {
    #[default]
    Normal,
    Muted,
    Accent,
    Danger,
    Bold,
    Heading,
}

/// A line in a plugin view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewLine {
    pub text: String,
    #[serde(default)]
    pub style: LineStyle,
}

impl ViewLine {
    pub fn new(text: impl Into<String>) -> Self {
        ViewLine {
            text: text.into(),
            style: LineStyle::Normal,
        }
    }
    pub fn styled(text: impl Into<String>, style: LineStyle) -> Self {
        ViewLine {
            text: text.into(),
            style,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyHelp {
    pub key: String,
    pub help: String,
}

impl KeyHelp {
    pub fn new(key: impl Into<String>, help: impl Into<String>) -> Self {
        KeyHelp {
            key: key.into(),
            help: help.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    pub label: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub hint: String,
}

impl FormField {
    pub fn new(label: impl Into<String>, value: impl Into<String>, hint: impl Into<String>) -> Self {
        FormField {
            label: label.into(),
            value: value.into(),
            hint: hint.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_type_tagged_snake_case() {
        let a = Action::SetTimer {
            id: "t".into(),
            after_ms: 1000,
            repeat: false,
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({"type": "set_timer", "id": "t", "after_ms": 1000, "repeat": false})
        );
        let e: Event =
            serde_json::from_str(r#"{"type":"view_key","view":"v","key":"Enter","selected":2}"#).unwrap();
        assert!(e.is_interactive());
        // Optional fields may be left out.
        let a: Action =
            serde_json::from_str(r#"{"type":"open_view","id":"v","title":"T","lines":[]}"#).unwrap();
        assert!(matches!(a, Action::OpenView { selected: None, .. }));
        // The host refuses actions it does not know.
        assert!(serde_json::from_str::<Action>(r#"{"type":"format_disk"}"#).is_err());
    }
}
