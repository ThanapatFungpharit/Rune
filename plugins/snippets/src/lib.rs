//! Snippets — a sample rune-tui plugin.
//!
//! Saves bits of text under a name and types them into the message line:
//! `/snip NAME` inserts one, `/snip` or `Alt-s` opens the list (`Enter`
//! inserts, `a` adds, `e` edits, `d` deletes), `/snip add [NAME]` and
//! `/snip rm NAME` work from the message line. Uses the `compose` and
//! `storage` permissions and two settings (`sort`, `preview`).
//!
//! The logic is plain Rust, so it is unit-tested natively (`cargo test`);
//! `export_plugin!` adds the WebAssembly entry points.

use std::collections::BTreeMap;

use rune_plugin::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const VIEW: &str = "list";
/// Storage keys: `s.<name>`.
const PREFIX: &str = "s.";
const NAME_MAX: usize = 32;
const TEXT_MAX: usize = 4000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Snip {
    text: String,
    added: i64,
}

#[derive(Debug, Default)]
pub struct Snippets {
    snips: BTreeMap<String, Snip>,
    newest_first: bool,
    preview: usize,
    view_open: bool,
    sel: usize,
}

fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Snippets {
    fn configure(&mut self, config: &Map<String, Value>) {
        self.newest_first = config.get("sort").and_then(Value::as_str) == Some("newest");
        self.preview = config.get("preview").and_then(Value::as_u64).unwrap_or(60) as usize;
    }

    /// Names in list order.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<&String> = self.snips.keys().collect();
        if self.newest_first {
            names.sort_by_key(|n| std::cmp::Reverse(self.snips[*n].added));
        }
        names.into_iter().cloned().collect()
    }

    fn view(&mut self) -> Action {
        let names = self.names();
        self.sel = self.sel.min(names.len().saturating_sub(1));
        let lines = if names.is_empty() {
            vec![
                ViewLine::styled("No snippets yet.", LineStyle::Muted),
                ViewLine::new(""),
                ViewLine::new("Press a to add one, or type /snip add NAME."),
            ]
        } else {
            let w = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
            names
                .iter()
                .map(|n| {
                    let one_line = self.snips[n].text.replace('\n', " ⏎ ");
                    let mut preview: String = one_line.chars().take(self.preview).collect();
                    if one_line.chars().count() > self.preview {
                        preview.push('…');
                    }
                    ViewLine::new(format!("{n:<w$}  {preview}"))
                })
                .collect()
        };
        self.view_open = true;
        Action::OpenView {
            id: VIEW.into(),
            title: format!("Snippets ({})", names.len()),
            selected: (!names.is_empty()).then_some(self.sel),
            lines,
            keys: vec![
                KeyHelp::new("Enter", "insert"),
                KeyHelp::new("a", "add"),
                KeyHelp::new("e", "edit"),
                KeyHelp::new("d", "delete"),
            ],
        }
    }

    fn form(id: &str, title: &str, name: &str, text: &str) -> Action {
        Action::Form {
            id: id.into(),
            title: title.into(),
            fields: vec![
                FormField::new("Name", name, "letters, digits, - and _"),
                FormField::new("Text", text, "what /snip NAME types"),
            ],
        }
    }

    fn insert(&mut self, name: &str) -> Vec<Action> {
        match self.snips.get(name) {
            Some(s) => {
                let mut out = vec![Action::InsertText { text: s.text.clone() }];
                if self.view_open {
                    self.view_open = false;
                    out.push(Action::CloseView { id: VIEW.into() });
                }
                out
            }
            None => vec![Action::status(format!("no snippet {name:?} (/snip add {name})"))],
        }
    }

    fn remove(&mut self, name: &str) -> Vec<Action> {
        if self.snips.remove(name).is_none() {
            return vec![Action::status(format!("no snippet {name:?}"))];
        }
        let mut out = vec![
            Action::StoreDelete {
                key: format!("{PREFIX}{name}"),
            },
            Action::status(format!("deleted {name}")),
        ];
        if self.view_open {
            out.push(self.view());
        }
        out
    }

    fn save(&mut self, now: i64, old: Option<&str>, values: &[String]) -> Vec<Action> {
        let name = values.first().map_or("", |s| s.trim()).to_owned();
        let text = values.get(1).map_or("", |s| s.as_str()).to_owned();
        if !valid_name(&name) {
            return vec![
                Action::status("a name is 1–32 letters, digits, - or _"),
                Self::form(
                    &old.map_or("add".into(), |o| format!("edit:{o}")),
                    "Snippet",
                    &name,
                    &text,
                ),
            ];
        }
        if text.trim().is_empty() || text.chars().count() > TEXT_MAX {
            return vec![Action::status(format!(
                "the text must be 1–{TEXT_MAX} characters"
            ))];
        }
        let mut out = Vec::new();
        if let Some(old) = old.filter(|o| *o != name) {
            self.snips.remove(old);
            out.push(Action::StoreDelete {
                key: format!("{PREFIX}{old}"),
            });
        }
        let added = self.snips.get(&name).map_or(now, |s| s.added);
        let snip = Snip { text, added };
        out.push(Action::StoreSet {
            key: format!("{PREFIX}{name}"),
            value: serde_json::to_string(&snip).unwrap_or_default(),
        });
        self.snips.insert(name.clone(), snip);
        self.sel = self.names().iter().position(|n| *n == name).unwrap_or(0);
        out.push(Action::status(format!("saved {name} — /snip {name} types it")));
        out.push(self.view());
        out
    }

    fn view_key(&mut self, key: &str) -> Vec<Action> {
        let names = self.names();
        let current = names.get(self.sel).cloned();
        match key {
            "Up" | "k" => self.sel = self.sel.saturating_sub(1),
            "Down" | "j" => self.sel = (self.sel + 1).min(names.len().saturating_sub(1)),
            "Home" => self.sel = 0,
            "End" => self.sel = names.len().saturating_sub(1),
            "Enter" => return current.map_or_else(Vec::new, |n| self.insert(&n)),
            "a" => return vec![Self::form("add", "New snippet", "", "")],
            "e" => {
                return current.map_or_else(Vec::new, |n| {
                    let text = self.snips[&n].text.clone();
                    vec![Self::form(&format!("edit:{n}"), "Edit snippet", &n, &text)]
                });
            }
            "d" => return current.map_or_else(Vec::new, |n| self.remove(&n)),
            _ => return Vec::new(),
        }
        vec![self.view()]
    }
}

impl Plugin for Snippets {
    fn handle(&mut self, cx: &Context, event: Event) -> Vec<Action> {
        match event {
            Event::Init { config, data, .. } => {
                self.configure(&config);
                self.snips = data
                    .into_iter()
                    .filter_map(|(k, v)| {
                        Some((k.strip_prefix(PREFIX)?.to_owned(), serde_json::from_str(&v).ok()?))
                    })
                    .collect();
                log::info(&format!("{} snippets loaded", self.snips.len()));
                Vec::new()
            }
            Event::ConfigChanged { config } => {
                self.configure(&config);
                if self.view_open {
                    vec![self.view()]
                } else {
                    Vec::new()
                }
            }
            Event::Command { args, .. } => {
                let args = args.trim();
                let (verb, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
                match (verb, rest.trim()) {
                    ("", _) => vec![self.view()],
                    ("add", name) => vec![Self::form("add", "New snippet", name, "")],
                    ("rm" | "del", "") => vec![Action::status("which one? /snip rm NAME")],
                    ("rm" | "del", name) => self.remove(name),
                    (name, _) => self.insert(name),
                }
            }
            Event::Key { .. } => vec![self.view()],
            Event::ViewKey { key, .. } => self.view_key(&key),
            Event::ViewClosed { .. } => {
                self.view_open = false;
                Vec::new()
            }
            Event::FormSubmitted { form, values } => {
                let old = form.strip_prefix("edit:");
                self.save(cx.now, old, &values)
            }
            _ => Vec::new(),
        }
    }
}

rune_plugin::export_plugin!(Snippets);

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Context {
        Context { now: 100 }
    }

    fn init(p: &mut Snippets, data: &[(&str, &str)]) {
        let data = data.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let config = serde_json::json!({"sort": "name", "preview": 10})
            .as_object()
            .unwrap()
            .clone();
        p.handle(
            &cx(),
            Event::Init {
                config,
                data,
                host: "test".into(),
            },
        );
    }

    fn command(p: &mut Snippets, args: &str) -> Vec<Action> {
        p.handle(
            &cx(),
            Event::Command {
                name: "snip".into(),
                args: args.into(),
                chat: None,
            },
        )
    }

    #[test]
    fn inserts_saved_snippets_and_keeps_new_ones() {
        let mut p = Snippets::default();
        init(
            &mut p,
            &[("s.hi", r#"{"text":"hello there","added":1}"#), ("junk", "x")],
        );
        assert_eq!(
            command(&mut p, "hi"),
            vec![Action::InsertText {
                text: "hello there".into()
            }]
        );
        assert!(matches!(&command(&mut p, "nope")[..], [Action::Status { .. }]));
        let out = p.handle(
            &cx(),
            Event::FormSubmitted {
                form: "add".into(),
                values: vec!["addr".into(), "1 Main St".into()],
            },
        );
        assert!(out.contains(&Action::StoreSet {
            key: "s.addr".into(),
            value: r#"{"text":"1 Main St","added":100}"#.into()
        }));
        let Some(Action::OpenView { lines, selected, .. }) = out.last() else {
            panic!("{out:?}")
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(*selected, Some(0), "the new snippet is selected");
        assert_eq!(
            lines[1].text, "hi    hello ther…",
            "previews are cut to the setting"
        );
    }

    #[test]
    fn view_keys_move_insert_edit_and_delete() {
        let mut p = Snippets::default();
        init(
            &mut p,
            &[
                ("s.a", r#"{"text":"A","added":1}"#),
                ("s.b", r#"{"text":"B","added":2}"#),
            ],
        );
        command(&mut p, "");
        let key = |p: &mut Snippets, k: &str| {
            p.handle(
                &cx(),
                Event::ViewKey {
                    view: VIEW.into(),
                    key: k.into(),
                    selected: None,
                },
            )
        };
        key(&mut p, "Down");
        assert_eq!(
            key(&mut p, "Enter"),
            vec![
                Action::InsertText { text: "B".into() },
                Action::CloseView { id: VIEW.into() }
            ]
        );
        command(&mut p, "");
        assert!(matches!(&key(&mut p, "e")[..], [Action::Form { id, .. }] if id == "edit:b"));
        // Renaming b to c moves its storage key.
        let out = p.handle(
            &cx(),
            Event::FormSubmitted {
                form: "edit:b".into(),
                values: vec!["c".into(), "B2".into()],
            },
        );
        assert!(out.contains(&Action::StoreDelete { key: "s.b".into() }));
        assert!(matches!(&key(&mut p, "d")[..], [Action::StoreDelete { key }, ..] if key == "s.c"));
        assert_eq!(p.names(), ["a"]);
    }

    #[test]
    fn bad_names_reopen_the_form() {
        let mut p = Snippets::default();
        init(&mut p, &[]);
        let out = p.handle(
            &cx(),
            Event::FormSubmitted {
                form: "add".into(),
                values: vec!["no spaces".into(), "x".into()],
            },
        );
        assert!(matches!(&out[..], [Action::Status { .. }, Action::Form { .. }]));
        assert!(p.snips.is_empty());
    }
}
