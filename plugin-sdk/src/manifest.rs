//! `plugin.json`: what a plugin is, what it asks for and what it adds.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// File name of the manifest in a plugin's directory.
pub const MANIFEST_FILE: &str = "plugin.json";

/// An API version, `MAJOR.MINOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.trim().split_once('.')?;
        let num =
            |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?;
        Some(ApiVersion {
            major: num(a)?,
            minor: num(b)?,
        })
    }

    pub const fn from_packed(v: i32) -> Self {
        ApiVersion {
            major: (v >> 16) as u16,
            minor: v as u16,
        }
    }

    /// Whether a plugin built for `self` runs in a host speaking `host`:
    /// same major version, and nothing newer than the host knows.
    pub fn runs_on(self, host: ApiVersion) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// What a plugin may do beyond drawing its own view and status lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Receive every new message and which chat is opened.
    ReadMessages,
    /// Send text messages on the user's behalf.
    SendMessages,
    /// Insert text into the message line.
    Compose,
    /// Show desktop notifications.
    Notify,
    /// Keep data in rune's local store.
    Storage,
}

impl Permission {
    pub const ALL: [Permission; 5] = [
        Permission::ReadMessages,
        Permission::SendMessages,
        Permission::Compose,
        Permission::Notify,
        Permission::Storage,
    ];

    /// Shown when the user is asked to grant it.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::ReadMessages => "read every message you receive or send, in all chats",
            Permission::SendMessages => "send messages in your name (others see them)",
            Permission::Compose => "type into your message line",
            Permission::Notify => "show desktop notifications",
            Permission::Storage => "keep its own data in rune's local store",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadMessages => "read_messages",
            Permission::SendMessages => "send_messages",
            Permission::Compose => "compose",
            Permission::Notify => "notify",
            Permission::Storage => "storage",
        }
    }
}

/// A `/name` command for the message line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// Without the slash: lowercase letters, digits and `-`.
    pub name: String,
    /// How to type it, e.g. `/snip [NAME]` (default: `/name`).
    #[serde(default)]
    pub usage: String,
    pub help: String,
}

/// A key that works in the main views, also while typing. Plugins get the
/// `Alt-<letter or digit>` chords, which rune itself never binds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySpec {
    /// Reported in [`crate::Event::Key`].
    pub id: String,
    /// `alt+s`, `alt+3`, …
    pub key: String,
    pub help: String,
}

impl KeySpec {
    /// The character of an `alt+<c>` key.
    pub fn alt_char(&self) -> Option<char> {
        let rest = self.key.trim().to_ascii_lowercase();
        let c = rest.strip_prefix("alt+")?;
        let mut chars = c.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_lowercase() || c.is_ascii_digit() => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingType {
    Bool,
    Int,
    Text,
    Choice,
}

/// A user-editable setting, shown in the plugin's page of the manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[serde(rename = "type")]
    pub kind: SettingType,
    pub default: Value,
    /// `int`: inclusive bounds.
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    /// `text`: the longest value, in characters (default and at most 1000).
    #[serde(default)]
    pub max_len: Option<usize>,
    /// `choice`: the values to choose from.
    #[serde(default)]
    pub choices: Vec<String>,
}

/// Longest `text` setting value.
pub const TEXT_SETTING_MAX: usize = 1000;

impl SettingSpec {
    /// `value` as this setting holds it, or why it cannot.
    pub fn check(&self, value: &Value) -> Result<Value, String> {
        match self.kind {
            SettingType::Bool => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| "must be on or off".into()),
            SettingType::Int => {
                let n = value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
                    .ok_or("must be a whole number")?;
                match (self.min, self.max) {
                    (Some(lo), _) if n < lo => Err(format!("must be at least {lo}")),
                    (_, Some(hi)) if n > hi => Err(format!("must be at most {hi}")),
                    _ => Ok(Value::from(n)),
                }
            }
            SettingType::Text => {
                let s = value.as_str().ok_or("must be text")?;
                let max = self.max_len.unwrap_or(TEXT_SETTING_MAX).min(TEXT_SETTING_MAX);
                if s.chars().count() > max {
                    return Err(format!("must be at most {max} characters"));
                }
                if s.chars().any(char::is_control) {
                    return Err("must be one line".into());
                }
                Ok(Value::from(s))
            }
            SettingType::Choice => {
                let s = value.as_str().ok_or("must be one of the choices")?;
                if self.choices.iter().any(|c| c == s) {
                    Ok(Value::from(s))
                } else {
                    Err(format!("must be one of: {}", self.choices.join(", ")))
                }
            }
        }
    }
}

/// `plugin.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Lowercase letters, digits and `-`; the plugin's directory has this name.
    pub id: String,
    pub name: String,
    /// The plugin's own version (free-form, e.g. `1.2.0`).
    pub version: String,
    /// The API it was built for, `MAJOR.MINOR`.
    pub api_version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// The WebAssembly module, a file next to the manifest.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    #[serde(default)]
    pub keys: Vec<KeySpec>,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

fn default_entry() -> String {
    "plugin.wasm".into()
}

/// A plain identifier: `[a-z][a-z0-9-]*`, at most `max` long.
pub fn is_ident(s: &str, max: usize) -> bool {
    let mut b = s.bytes();
    s.len() <= max
        && b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// A setting or storage key: `[A-Za-z0-9_.-]`, 1 to `max` long.
pub fn is_key(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

impl Manifest {
    /// Parse and [`validate`](Self::validate) a manifest.
    pub fn parse(json: &str) -> Result<Self, String> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| format!("plugin.json: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn api(&self) -> Option<ApiVersion> {
        ApiVersion::parse(&self.api_version)
    }

    /// Everything that can be checked without the host: identifiers,
    /// duplicates, keys, setting defaults.
    pub fn validate(&self) -> Result<(), String> {
        if !is_ident(&self.id, 32) {
            return Err(format!(
                "id {:?}: use 1–32 lowercase letters, digits and '-', starting with a letter",
                self.id
            ));
        }
        let text = |what: &str, s: &str, max: usize| {
            if s.trim().is_empty() || s.chars().count() > max || s.chars().any(char::is_control) {
                Err(format!("{what}: 1–{max} characters on one line"))
            } else {
                Ok(())
            }
        };
        // Optional text: may be empty, otherwise one line of at most `max`.
        let optional = |what: &str, s: &str, max: usize| {
            if s.is_empty() { Ok(()) } else { text(what, s, max) }
        };
        text("name", &self.name, 48)?;
        text("version", &self.version, 32)?;
        optional("description", &self.description, 500)?;
        optional("author", &self.author, 100)?;
        if self.api().is_none() {
            return Err(format!(
                "api_version {:?}: expected MAJOR.MINOR",
                self.api_version
            ));
        }
        let entry_ok = self.entry.ends_with(".wasm")
            && self.entry.len() > 5
            && !self.entry.contains(['/', '\\'])
            && !self.entry.starts_with('.');
        if !entry_ok {
            return Err(format!(
                "entry {:?}: a .wasm file name next to plugin.json",
                self.entry
            ));
        }
        let mut seen = HashSet::new();
        for p in &self.permissions {
            if !seen.insert(*p) {
                return Err(format!("permission {} is listed twice", p.as_str()));
            }
        }
        let mut seen = HashSet::new();
        for c in &self.commands {
            if !is_ident(&c.name, 24) {
                return Err(format!(
                    "command {:?}: use lowercase letters, digits and '-'",
                    c.name
                ));
            }
            if !seen.insert(c.name.as_str()) {
                return Err(format!("command /{} is listed twice", c.name));
            }
            text(&format!("help of /{}", c.name), &c.help, 120)?;
            optional(&format!("usage of /{}", c.name), &c.usage, 60)?;
        }
        let mut ids = HashSet::new();
        let mut chars = HashSet::new();
        for k in &self.keys {
            if !is_key(&k.id, 32) || !ids.insert(k.id.as_str()) {
                return Err(format!("key id {:?}: missing, malformed or listed twice", k.id));
            }
            let c = k
                .alt_char()
                .ok_or_else(|| format!("key {:?}: plugins may bind alt+<letter or digit>", k.key))?;
            if !chars.insert(c) {
                return Err(format!("key {} is listed twice", k.key));
            }
            text(&format!("help of {}", k.key), &k.help, 120)?;
        }
        let mut seen = HashSet::new();
        for s in &self.settings {
            if !is_key(&s.key, 32) || !seen.insert(s.key.as_str()) {
                return Err(format!(
                    "setting key {:?}: missing, malformed or listed twice",
                    s.key
                ));
            }
            text(&format!("label of {}", s.key), &s.label, 48)?;
            optional(&format!("help of {}", s.key), &s.help, 120)?;
            for c in &s.choices {
                text(&format!("a choice of {}", s.key), c, 48)?;
            }
            if s.kind == SettingType::Choice && s.choices.is_empty() {
                return Err(format!("setting {}: a choice needs choices", s.key));
            }
            if let (Some(lo), Some(hi)) = (s.min, s.max)
                && lo > hi
            {
                return Err(format!("setting {}: min is above max", s.key));
            }
            s.check(&s.default)
                .map_err(|e| format!("setting {}: default {e}", s.key))?;
        }
        Ok(())
    }

    /// Every setting at its default.
    pub fn default_config(&self) -> Map<String, Value> {
        self.settings
            .iter()
            .map(|s| (s.key.clone(), s.default.clone()))
            .collect()
    }

    pub fn has(&self, p: Permission) -> bool {
        self.permissions.contains(&p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: &str) -> Result<Manifest, String> {
        Manifest::parse(&format!(
            r#"{{"id":"demo","name":"Demo","version":"1.0","api_version":"1.0"{extra}}}"#
        ))
    }

    #[test]
    fn minimal_manifest_gets_defaults() {
        let m = manifest("").unwrap();
        assert_eq!(m.entry, "plugin.wasm");
        assert!(m.permissions.is_empty() && m.default_config().is_empty());
        assert_eq!(m.api(), Some(ApiVersion { major: 1, minor: 0 }));
    }

    #[test]
    fn versions_are_compatible_within_a_major_up_to_the_hosts_minor() {
        let host = ApiVersion { major: 1, minor: 2 };
        assert!(ApiVersion::parse("1.0").unwrap().runs_on(host));
        assert!(ApiVersion::parse("1.2").unwrap().runs_on(host));
        assert!(!ApiVersion::parse("1.3").unwrap().runs_on(host));
        assert!(!ApiVersion::parse("2.0").unwrap().runs_on(host));
        assert!(!ApiVersion::parse("0.9").unwrap().runs_on(host));
        for bad in ["1", "1.", ".1", "1.0.0", "a.b", "-1.0", "+1.0", "1.99999"] {
            assert_eq!(ApiVersion::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn malformed_manifests_say_what_is_wrong() {
        let bad = [
            (r#","id2":1"#, "unknown field"),
            (r#","entry":"../x.wasm""#, "entry"),
            (r#","permissions":["root"]"#, "unknown variant"),
            (r#","permissions":["notify","notify"]"#, "twice"),
            (r#","commands":[{"name":"Bad","help":"x"}]"#, "command"),
            (r#","keys":[{"id":"k","key":"ctrl+s","help":"x"}]"#, "alt+"),
            (
                r#","keys":[{"id":"a","key":"alt+s","help":"x"},{"id":"b","key":"Alt+S","help":"y"}]"#,
                "twice",
            ),
            (
                r#","settings":[{"key":"n","label":"N","type":"int","default":5,"max":3}]"#,
                "at most 3",
            ),
            (
                r#","settings":[{"key":"c","label":"C","type":"choice","default":"a"}]"#,
                "choices",
            ),
            (
                r#","settings":[{"key":"b","label":"B","type":"bool","default":"yes"}]"#,
                "on or off",
            ),
            // Text the plugin manager draws must be one line.
            (r#","description":"two\nlines""#, "description"),
            (r#","author":"a\u001b[31m""#, "author"),
            (
                r#","commands":[{"name":"x","usage":"/x\r","help":"x"}]"#,
                "usage of /x",
            ),
            (
                r#","settings":[{"key":"b","label":"B","type":"bool","default":true,"help":"\u0007"}]"#,
                "help of b",
            ),
            (
                r#","settings":[{"key":"c","label":"C","type":"choice","default":"a","choices":["a","b\n"]}]"#,
                "a choice of c",
            ),
        ];
        for (extra, want) in bad {
            let e = manifest(extra).unwrap_err();
            assert!(e.contains(want), "{extra}: {e}");
        }
        let e = Manifest::parse(r#"{"id":"Demo","name":"D","version":"1","api_version":"1.0"}"#).unwrap_err();
        assert!(e.contains("id"), "{e}");
    }

    #[test]
    fn settings_are_checked_and_normalised() {
        let m = manifest(
            r#","settings":[
                {"key":"n","label":"N","type":"int","default":5,"min":1,"max":9},
                {"key":"t","label":"T","type":"text","default":"","max_len":3},
                {"key":"c","label":"C","type":"choice","default":"a","choices":["a","b"]}]"#,
        )
        .unwrap();
        let [n, t, c] = &m.settings[..] else { panic!() };
        assert_eq!(n.check(&Value::from("7")), Ok(Value::from(7)));
        assert!(n.check(&Value::from(10)).is_err());
        assert!(t.check(&Value::from("abcd")).is_err());
        assert!(t.check(&Value::from("a\nb")).is_err());
        assert_eq!(c.check(&Value::from("b")), Ok(Value::from("b")));
        assert!(c.check(&Value::from("z")).is_err());
        assert_eq!(m.default_config()["n"], 5);
    }
}
