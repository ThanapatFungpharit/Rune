//! Settings › Shortcuts › a screen: that screen's shortcuts, each changed
//! by pressing the new key. A change is held to `keymap`'s rules (no plugin
//! keys, no characters where they are text); a key that already does
//! something else there is moved only when the user says so.
//!
//! Changes are stored as `setting.keys` (see [`keymap::Keymap::to_json`])
//! and answer at once — or, with `--default-keys`, from the next start.
//! They are also kept in a file to edit by hand or copy elsewhere
//! (`keys.json`, [`default_file`]): read at start, where it wins over the
//! store, and rewritten on every change — unless it could not be read,
//! in which case it is left alone until it is fixed.
//! This page's own keys cannot be changed (`keymap::Ctx::Keys`), so a
//! mistake can always be undone here.

use crate::*;
use keymap::{Cmd, Key, Keymap, Table};

/// Where the user's shortcuts are stored.
const SETTING: &str = "setting.keys";

/// Where the shortcuts are also kept as a file:
/// `$XDG_CONFIG_HOME/rune/keys.json`, else `~/.config/rune/keys.json`.
pub fn default_file() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("rune").join("keys.json"))
}

/// The file the shortcuts are also kept in.
pub struct KeysFile {
    pub path: PathBuf,
    /// Read (or absent), so rewriting it loses nothing. One that could not
    /// be read is left alone until it is fixed.
    pub writable: bool,
}

/// `path` with the home directory written `~`.
fn shown(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.starts_with(&home) && home.as_os_str().len() > 1 => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

/// Replace `path`'s contents in one step (a symlinked file, as dotfile
/// managers make, is written where it points).
fn write_whole(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = target.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The stored shortcuts, and what in them could not be used.
pub fn load(store: &Store) -> (Keymap, Vec<String>) {
    match store.get(SETTING) {
        Ok(Some(json)) => Keymap::from_json(&json),
        Ok(None) => (Keymap::default(), Vec::new()),
        Err(e) => (
            Keymap::default(),
            vec![format!("could not read your shortcuts: {e}")],
        ),
    }
}

pub struct KeyPage {
    /// Index into `keymap::SECTIONS`.
    pub section: usize,
    pub sel: usize,
    pub state: State,
    /// What the last change did, or why it could not be made.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Browse,
    /// The next key pressed is the new one (`add`: besides the others).
    Capture {
        add: bool,
    },
    /// That key does something else where this one answers: move it here?
    Conflict {
        add: bool,
        key: Key,
        with: Vec<(Table, Cmd)>,
    },
}

impl KeyPage {
    fn section(&self) -> &'static keymap::Section {
        &keymap::SECTIONS[self.section]
    }

    /// The selected binding.
    pub fn selected(&self) -> Option<(Table, Cmd)> {
        self.section().entries().get(self.sel).map(|(t, b)| (*t, b.cmd))
    }

    /// Waiting for the new key: every key goes to the page.
    pub fn capturing(&self) -> bool {
        matches!(self.state, State::Capture { .. })
    }

    fn step_section(&mut self, forward: bool) {
        let n = keymap::SECTIONS.len();
        self.section = if forward {
            (self.section + 1) % n
        } else {
            (self.section + n - 1) % n
        };
        self.sel = 0;
        self.note = None;
    }
}

/// `“what”` for a binding.
fn quoted(table: Table, cmd: Cmd) -> String {
    format!("“{}”", keymap::help(table, cmd))
}

impl App {
    /// One screen's shortcuts, over Settings (which comes back when it closes).
    pub fn open_shortcuts(&mut self, section: usize) {
        self.modal = Some(Modal::Shortcuts(KeyPage {
            section: section.min(keymap::SECTIONS.len() - 1),
            sel: 0,
            state: State::Browse,
            note: None,
        }));
    }

    pub fn shortcuts_key(&mut self, mut p: KeyPage, cmd: Option<Cmd>, k: KeyEvent) {
        match p.state.clone() {
            State::Capture { add } => self.capture_key(&mut p, add, k),
            State::Conflict { add, key, with } => match k.code {
                KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                    if let Some((table, cmd)) = p.selected() {
                        p.note = Some(self.change_keys(table, cmd, key, add, &with));
                    }
                    p.state = State::Browse;
                }
                KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                    p.note = Some("not changed".into());
                    p.state = State::Browse;
                }
                _ => {}
            },
            State::Browse => {
                let n = p.section().entries().len();
                match cmd {
                    Some(Cmd::Back) => return,
                    Some(Cmd::Up) => {
                        p.sel = p.sel.saturating_sub(1);
                        p.note = None;
                    }
                    Some(Cmd::Down) => {
                        p.sel = (p.sel + 1).min(n.saturating_sub(1));
                        p.note = None;
                    }
                    Some(Cmd::Home) => {
                        p.sel = 0;
                        p.note = None;
                    }
                    Some(Cmd::End) => {
                        p.sel = n.saturating_sub(1);
                        p.note = None;
                    }
                    Some(Cmd::NextTab) => p.step_section(true),
                    Some(Cmd::PrevTab) => p.step_section(false),
                    Some(Cmd::Confirm) => {
                        p.state = State::Capture { add: false };
                        p.note = None;
                    }
                    Some(Cmd::AddKey) => {
                        p.state = State::Capture { add: true };
                        p.note = None;
                    }
                    Some(Cmd::ResetKey) => {
                        if let Some((table, cmd)) = p.selected() {
                            p.note = Some(self.reset_keys(table, cmd));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.modal = Some(Modal::Shortcuts(p));
    }

    /// The key pressed while waiting for one.
    fn capture_key(&mut self, p: &mut KeyPage, add: bool, k: KeyEvent) {
        if k.code == KeyCode::Esc && k.modifiers.is_empty() {
            p.state = State::Browse;
            p.note = Some("not changed".into());
            return;
        }
        let Some((table, cmd)) = p.selected() else {
            p.state = State::Browse;
            return;
        };
        let key = Key::of(&k);
        let name = keymap::display_key(key);
        if let Some(why) = self.keys.refusal(table, cmd, key) {
            // Still waiting: another key may do.
            p.note = Some(format!("{why}. Press another key (Esc cancels)."));
            return;
        }
        p.state = State::Browse;
        let current = self.keys.keys(table, cmd);
        if current.iter().any(|k| k.normalized() == key) && (add || current.len() == 1) {
            p.note = Some(format!("{name} already does {}", quoted(table, cmd)));
            return;
        }
        let with = self.keys.conflicts(table, cmd, key);
        if with.is_empty() {
            p.note = Some(self.change_keys(table, cmd, key, add, &[]));
        } else {
            p.state = State::Conflict { add, key, with };
            p.note = None;
        }
    }

    /// Bind `key` to `cmd` of `table` (`add`: besides its other keys),
    /// taking it from `take_from`. Says what changed.
    fn change_keys(
        &mut self,
        table: Table,
        cmd: Cmd,
        key: Key,
        add: bool,
        take_from: &[(Table, Cmd)],
    ) -> String {
        let mut map = self.keys.clone();
        for &(t, c) in take_from {
            map.take(t, c, key);
        }
        if add {
            map.add(table, cmd, key);
        } else {
            map.set(table, cmd, vec![key]);
        }
        let mut note = format!(
            "{}: {}",
            quoted(table, cmd),
            keymap::display_keys(map.keys(table, cmd))
        );
        for &(t, c) in take_from {
            let left = match map.keys(t, c) {
                [] => "now has no key".to_owned(),
                keys => format!("keeps {}", keymap::display_keys(keys)),
            };
            note.push_str(&format!("; {} {left}", quoted(t, c)));
        }
        self.shortcuts_changed(map);
        note
    }

    /// `cmd` of `table` back to its default keys. Says what changed.
    fn reset_keys(&mut self, table: Table, cmd: Cmd) -> String {
        if !self.keys.is_custom(table, cmd) {
            return format!("{} already has its default keys", quoted(table, cmd));
        }
        let mut map = self.keys.clone();
        map.reset(table, cmd);
        let mut note = format!(
            "{} is back to {}",
            quoted(table, cmd),
            keymap::display_keys(map.keys(table, cmd))
        );
        if let Some((key, t, c)) = map.clash(table, cmd) {
            note.push_str(&format!(
                " — but {} also does {}: change one of them",
                keymap::display_key(key),
                quoted(t, c)
            ));
        }
        self.shortcuts_changed(map);
        note
    }

    /// Every shortcut back to its default (Settings › Shortcuts).
    pub fn reset_all_shortcuts(&mut self) {
        self.shortcuts_changed(Keymap::default());
        self.status = "every shortcut is back to its default".into();
    }

    /// Keep `map` as the user's shortcuts: stored, written to the keys
    /// file, and answering keys unless this run uses the defaults.
    fn shortcuts_changed(&mut self, map: Keymap) {
        self.adopt_keys(map);
        // What could not be read is gone from what is stored now (unless it
        // is the keys file itself, which stays as it is until fixed).
        if self.keys_file.as_ref().is_none_or(|f| f.writable) {
            self.key_problems.clear();
        }
        self.write_keys_file();
    }

    /// `map` as the user's shortcuts: in the store and answering keys.
    fn adopt_keys(&mut self, map: Keymap) {
        let stored = if map.is_default() {
            self.store.remove(SETTING)
        } else {
            self.store.set(SETTING, &map.to_json())
        };
        self.saved(stored);
        self.keys = map;
        if !self.default_keys {
            keymap::install(self.keys.clone());
        }
    }

    /// Also keep the shortcuts in `path`: read now (it wins over the
    /// store), written on every change.
    pub fn use_keys_file(&mut self, path: PathBuf) {
        self.keys_file = Some(KeysFile {
            path,
            writable: false,
        });
        let said = self.read_keys_file();
        tracing::info!("shortcuts: {said}");
    }

    /// (Re)read the keys file into the user's shortcuts. Says what happened.
    pub fn read_keys_file(&mut self) -> String {
        let Some(path) = self.keys_file.as_ref().map(|f| f.path.clone()) else {
            return "no keys file".into();
        };
        let name = shown(&path);
        let (writable, said) = match std::fs::read_to_string(&path) {
            Ok(text) => match Keymap::parse(&text) {
                Ok((map, problems)) => {
                    let n = map.changed();
                    for problem in &problems {
                        tracing::warn!("{name}: {problem}");
                    }
                    self.adopt_keys(map);
                    self.key_problems = problems.into_iter().map(|p| format!("{name}: {p}")).collect();
                    let s = if n == 1 { "" } else { "s" };
                    (true, format!("read {name}: {n} changed shortcut{s}"))
                }
                Err(e) => {
                    let why =
                        format!("{name} is unreadable ({e}): not used, and left alone until it is fixed");
                    tracing::warn!("{why}");
                    self.key_problems = vec![why.clone()];
                    (false, why)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                true,
                format!("{name} does not exist: it is written when a shortcut changes"),
            ),
            Err(e) => {
                let why = format!("could not read {name} ({e}): left alone");
                tracing::warn!("{why}");
                self.key_problems = vec![why.clone()];
                (false, why)
            }
        };
        if let Some(file) = self.keys_file.as_mut() {
            file.writable = writable;
        }
        // Changes made before there was a file go into it.
        if writable && !path.exists() && !self.keys.is_default() {
            self.write_keys_file();
        }
        said
    }

    /// Rewrite the keys file with the user's shortcuts (if it may be).
    fn write_keys_file(&mut self) {
        let Some(file) = self.keys_file.as_ref().filter(|f| f.writable) else {
            return;
        };
        if let Err(e) = write_whole(&file.path, &self.keys.to_file_json()) {
            self.status = format!("could not write {}: {e}", shown(&file.path));
        }
    }

    /// Settings › Shortcuts: where the keys file is (if any).
    pub fn keys_file_shown(&self) -> Option<String> {
        self.keys_file.as_ref().map(|f| shown(&f.path))
    }

    /// Answer keys with the defaults this run (`--default-keys`), or with
    /// the user's own.
    pub fn use_default_keys(&mut self, on: bool) {
        self.default_keys = on;
        keymap::install(if on { Keymap::default() } else { self.keys.clone() });
    }

    /// Settings › Shortcuts: one row per screen, then resetting everything.
    pub fn shortcuts_summary(&self, section: &keymap::Section) -> String {
        let changed = self.keys.changed_in(section);
        let clash = section
            .entries()
            .iter()
            .any(|(t, b)| self.keys.clash(*t, b.cmd).is_some());
        match (changed, clash) {
            (0, false) => String::new(),
            (n, false) => format!("{n} changed"),
            (0, true) => "⚠ clash".into(),
            (n, true) => format!("⚠ {n} changed"),
        }
    }

    pub fn draw_shortcuts(&mut self, f: &mut Frame, area: Rect, p: &mut KeyPage, hint: String) {
        let section = p.section();
        let entries = section.entries();
        p.sel = p.sel.min(entries.len().saturating_sub(1));
        let r = modal::popup(area, 80, entries.len() as u16 + 6);
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let hint = match &p.state {
            State::Browse => hint,
            State::Capture { .. } => " press the new key · Esc cancel ".into(),
            State::Conflict { .. } => " Enter/y move it · Esc/n keep it ".into(),
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(
                format!(" ⌨ Shortcuts › {} ", section.title),
                Style::new().bold(),
            ))
            .title_bottom(frame_hint(&hint, r));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [body, _, note_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(2)]).areas(inner);
        let t = theme::t();
        let first = modal::scroll_first(p.sel, body.height as usize);
        for (row, (table, b)) in entries.iter().enumerate().skip(first).take(body.height as usize) {
            let y = body.y + (row - first) as u16;
            self.hit.modal_rows.push((y, row));
            let sel = row == p.sel;
            let custom = self.keys.is_custom(*table, b.cmd);
            let clash = self.keys.clash(*table, b.cmd).is_some();
            let keys = self.keys.keys(*table, b.cmd);
            let value = match (&p.state, sel) {
                (State::Capture { add: false }, true) => "press the new key…".to_owned(),
                (State::Capture { add: true }, true) => {
                    format!("{} + press a key…", keymap::display_keys(keys))
                }
                _ if keys.is_empty() => "—".to_owned(),
                _ => keymap::display_keys(keys),
            };
            let value = if clash { format!("⚠ {value}") } else { value };
            let was = if custom {
                format!("  (default {})", keymap::display_keys(b.keys))
            } else {
                String::new()
            };
            let width = body.width as usize;
            let vw = value.width().min(width / 2);
            let value = ellipsize(&value, vw);
            let ww = was.width().min(width.saturating_sub(vw + 12));
            let was = ellipsize(&was, ww);
            let lw = width.saturating_sub(vw + ww + 3);
            let label = ellipsize(b.help, lw);
            let pad = width.saturating_sub(label.width() + value.width() + was.width() + 2);
            let base = if sel {
                Style::new().bg(t.accent_soft).fg(t.text)
            } else {
                Style::new()
            };
            let value_style = match (clash, custom) {
                (true, _) => base.fg(t.warn).bold(),
                (false, true) => base.fg(t.accent).bold(),
                (false, false) if sel => base.bold(),
                (false, false) => base.fg(t.accent),
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {label}{}", " ".repeat(pad)), base),
                    Span::styled(value, value_style),
                    Span::styled(format!("{was} "), base.fg(t.muted)),
                ])),
                Rect::new(body.x, y, body.width, 1),
            );
        }
        let (text, style) = self.shortcuts_note(p);
        f.render_widget(
            Paragraph::new(text).style(style).wrap(Wrap { trim: true }),
            note_area,
        );
    }

    /// The line under the list: the question being asked, what the last
    /// change did, a clash on the selected shortcut, or how this works.
    fn shortcuts_note(&self, p: &KeyPage) -> (String, Style) {
        let t = theme::t();
        let selected = p.selected();
        match (&p.state, selected) {
            (State::Conflict { key, with, .. }, Some((table, cmd))) => {
                let others: Vec<String> = with
                    .iter()
                    .map(|&(t, c)| format!("{} ({})", quoted(t, c), t.section_title().to_lowercase()))
                    .collect();
                let text = format!(
                    "{} already does {}. Move it to {}? Enter/y moves it, Esc/n keeps it there.",
                    keymap::display_key(*key),
                    others.join(", "),
                    quoted(table, cmd)
                );
                (text, Style::new().fg(t.warn))
            }
            (State::Capture { .. }, Some((table, cmd))) => {
                let text = p.note.clone().unwrap_or_else(|| {
                    format!("Press the new key for {} (Esc cancels).", quoted(table, cmd))
                });
                (text, Style::new().fg(t.accent))
            }
            (_, _) if p.note.is_some() => (p.note.clone().unwrap_or_default(), Style::new().fg(t.accent)),
            (_, Some((table, cmd)))
                if let Some((key, other_table, other_cmd)) = self.keys.clash(table, cmd) =>
            {
                let text = format!(
                    "⚠ {} also does {} in the {}: change one of them.",
                    keymap::display_key(key),
                    quoted(other_table, other_cmd),
                    other_table.section_title().to_lowercase()
                );
                (text, Style::new().fg(t.warn))
            }
            _ if self.default_keys => (
                "This run uses the default keys (--default-keys): changes apply from the next start.".into(),
                theme::muted(),
            ),
            (_, selected) => {
                // Its name, for editing keys.json by hand.
                let name = selected.map_or(String::new(), |(t, c)| {
                    format!("“{}” in keys.json · ", keymap::name(t, c))
                });
                (
                    format!("{name}Enter: press a new key · a: add a key · Delete: reset to the default"),
                    theme::muted(),
                )
            }
        }
    }
}
