//! Importing a chat exported from the LINE phone app (chat → ☰ → Other
//! settings → Export chat history) into the store, so history LINE no
//! longer serves shows above the fetched messages. Chosen from the chat
//! menu; the file is picked with the attach browser in import mode.

use super::*;
use chrono::{Local, TimeZone};
use rune::import::{ChatExport, LocalTime, parse_chat_export};
use std::path::{Path, PathBuf};

/// Exports bigger than this are refused (years of a busy group are a few MB).
const MAX_EXPORT: u64 = 256 * 1024 * 1024;

/// The exporting phone's local time, taken to be this computer's time zone.
fn local_ms(t: &LocalTime) -> i64 {
    Local
        .with_ymd_and_hms(t.year, t.month, t.day, t.hour, t.minute, 0)
        .earliest()
        .map(|d| d.timestamp_millis())
        // A time skipped by a DST change: use today's offset.
        .unwrap_or_else(|| t.utc_ms(Local::now().offset().local_minus_utc() / 60))
}

fn read_export(path: &Path) -> Result<ChatExport, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    if size > MAX_EXPORT {
        return Err(format!("{} is too big for a chat export", path.display()));
    }
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_chat_export(&String::from_utf8_lossy(&data)).map_err(|e| e.to_string())
}

fn norm(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Match the export's sender names to mids: you, then the chat's members,
/// then friends; a 1:1 chat's one leftover name is the other person (or you,
/// after a rename). Names that stay ambiguous or unknown are left out.
pub fn match_senders(
    export: &ChatExport,
    me: &str,
    my_names: &[String],
    partner: Option<&str>,
    participants: &[(String, String)],
    friends: &[(String, String)],
) -> HashMap<String, String> {
    let unique = |pool: &[(String, String)], name: &str| -> Option<String> {
        let mut hits = pool.iter().filter(|(_, n)| norm(n) == norm(name)).map(|(m, _)| m);
        let first = hits.next()?;
        hits.all(|m| m == first).then(|| first.clone())
    };
    let mut out = HashMap::new();
    let mut left = Vec::new();
    for (name, _) in export.senders() {
        let mid = if my_names.iter().any(|n| norm(n) == norm(&name)) {
            Some(me.to_owned())
        } else {
            unique(participants, &name).or_else(|| unique(friends, &name))
        };
        match mid {
            Some(m) => drop(out.insert(name, m)),
            None => left.push(name),
        }
    }
    if let (Some(partner), [name]) = (partner, left.as_slice()) {
        let used = |mid: &str| out.values().any(|m| m == mid);
        if !used(partner) {
            out.insert(name.clone(), partner.to_owned());
        } else if !used(me) {
            out.insert(name.clone(), me.to_owned());
        }
    }
    out
}

impl App {
    /// The file browser, picking a chat export to import into `chat`.
    pub fn open_import_picker(&mut self, chat: String) {
        let mut a = attach::Attach::new(self.browse_dir_for_import());
        a.import = Some(chat);
        self.modal = Some(Modal::Attach(a));
    }

    fn browse_dir_for_import(&self) -> PathBuf {
        self.store
            .get("setting.import_dir")
            .ok()
            .flatten()
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .or_else(|| dirs_downloads().filter(|d| d.is_dir()))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn sender_map(&self, chat: &str, export: &ChatExport) -> HashMap<String, String> {
        let mut my_names: Vec<String> = Vec::new();
        if let Some(n) = self.names.get(&self.me) {
            my_names.push(n.clone());
        }
        if let Some(Load::Ready((n, _))) = &self.my_profile {
            my_names.push(n.clone());
        }
        if let Ok(p) = self.store.profile() {
            my_names.push(p.name);
        }
        my_names.retain(|n| !n.trim().is_empty());
        let user = MidType::of(chat) == MidType::User;
        let members: Vec<String> = if user {
            vec![chat.to_owned()]
        } else {
            self.members
                .get(chat)
                .cloned()
                .or_else(|| self.store.user(chat).ok().flatten().map(|u| u.members))
                .unwrap_or_default()
        };
        let participants: Vec<(String, String)> = members
            .into_iter()
            .filter(|m| *m != self.me)
            .filter_map(|m| {
                let n = self.names.get(&m).cloned().filter(|n| !n.is_empty())?;
                Some((m, n))
            })
            .collect();
        let friends: Vec<(String, String)> = self
            .contacts
            .iter()
            .map(|(m, n)| (m.clone(), n.clone()))
            .collect();
        match_senders(
            export,
            &self.me,
            &my_names,
            user.then_some(chat),
            &participants,
            &friends,
        )
    }

    /// A file was picked: show what would be imported, and ask.
    pub fn preview_import(&mut self, chat: String, path: PathBuf) {
        let _ = self.store.set(
            "setting.import_dir",
            &path.parent().unwrap_or(Path::new(".")).to_string_lossy(),
        );
        let export = match read_export(&path) {
            Ok(e) => e,
            Err(e) => {
                self.modal = Some(Modal::Info {
                    title: " Import chat history ".into(),
                    lines: vec![e],
                    scroll: 0,
                });
                return;
            }
        };
        let map = self.sender_map(&chat, &export);
        let (first, last) = export.span().expect("a parsed export has messages");
        let mut text = format!(
            "Import {} messages ({} – {}) into {}?",
            export.messages.len(),
            first,
            last,
            self.name(&chat)
        );
        if let Some(t) = &export.title {
            text.push_str(&format!("\nThe file is the chat with “{t}”."));
        }
        text.push('\n');
        for (name, n) in export.senders().into_iter().take(8) {
            let who = match map.get(&name) {
                Some(m) if *m == self.me => "you".to_owned(),
                Some(m) => self.name(m),
                None => "not matched (shown by name)".to_owned(),
            };
            text.push_str(&format!("\n{name} → {who} · {n}"));
        }
        text.push_str(
            "\n\nTimes are read in this computer's time zone. Photos and stickers are only \
             placeholders. Imported messages stay on this computer.",
        );
        self.confirm(text, modal::Action::Import(chat, path));
    }

    pub fn run_import(&mut self, chat: String, path: PathBuf) {
        let export = match read_export(&path) {
            Ok(e) => e,
            Err(e) => return self.status = e,
        };
        let map = self.sender_map(&chat, &export);
        let msgs = export.to_messages(&chat, |n| map.get(n).cloned(), local_ms);
        match self.store.import_messages(&chat, &msgs) {
            Ok(st) => {
                self.no_older.remove(&chat);
                if self.messages.contains_key(&chat) {
                    let keep = self.messages.get(&chat).map_or(0, Vec::len).max(200);
                    self.reload_from_store(&chat, keep);
                }
                let mut s = format!("imported {} messages", st.added);
                if st.already > 0 {
                    s.push_str(&format!(" · {} already imported", st.already));
                }
                if st.duplicates > 0 {
                    s.push_str(&format!(" · {} already here from LINE", st.duplicates));
                }
                self.status = s;
            }
            Err(e) => self.status = format!("import failed: {e}"),
        }
    }

    pub fn delete_imported(&mut self, chat: String) {
        match self.store.delete_imported(&chat) {
            Ok(n) => {
                if self.messages.contains_key(&chat) {
                    let keep = self.messages.get(&chat).map_or(0, Vec::len).max(200);
                    self.reload_from_store(&chat, keep);
                }
                self.status = format!("removed {n} imported messages");
            }
            Err(e) => self.status = format!("could not remove: {e}"),
        }
    }
}

/// `~/Downloads`, where phones' exports usually end up after a transfer.
fn dirs_downloads() -> Option<PathBuf> {
    std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senders_match_by_name_and_leftover() {
        let e = parse_chat_export(
            "2024/01/15 Mon\n09:00\tMe Old\thi\n09:01\tAli\tyo\n09:02\tBob\they\n09:03\tCat\t!\n",
        )
        .unwrap();
        let friends = vec![
            ("Ub".to_owned(), "bob".to_owned()),
            ("Uc1".to_owned(), "Cat".to_owned()),
            ("Uc2".to_owned(), "Cat".to_owned()),
        ];
        // Group: Bob by friend name; Cat ambiguous; Ali and "Me Old" unknown.
        let m = match_senders(&e, "Ume", &["Me".into()], None, &[], &friends);
        assert_eq!(m.get("Bob").map(String::as_str), Some("Ub"));
        assert_eq!(m.len(), 1);
        // 1:1 with Ali, me renamed: the single leftover after Ali is me.
        let e = parse_chat_export("2024/01/15 Mon\n09:00\tMe Old\thi\n09:01\tAli\tyo\n").unwrap();
        let parts = vec![("Uali".to_owned(), "Ali".to_owned())];
        let m = match_senders(&e, "Ume", &["Me".into()], Some("Uali"), &parts, &[]);
        assert_eq!(m.get("Ali").map(String::as_str), Some("Uali"));
        assert_eq!(m.get("Me Old").map(String::as_str), Some("Ume"));
    }
}
