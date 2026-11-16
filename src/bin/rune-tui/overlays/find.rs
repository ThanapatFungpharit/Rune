//! Search in a chat (chat menu › *Search in chat…*, `/` on the messages):
//! the stored, decrypted text messages that contain what is typed, newest
//! first, updated as the query changes; `Enter` shows the message in the
//! chat. Only what this computer has stored is searched (the Chrome gateway
//! has no search), through `Store::search_messages` off the UI thread;
//! results of a query that has since changed are dropped (`seq`).

use crate::*;
use keymap::Cmd;

/// At most this many matches are listed (the newest).
pub const LIMIT: usize = 200;

pub struct Find {
    pub chat: String,
    pub query: TextInput,
    pub hits: Vec<Hit>,
    pub sel: usize,
    /// The query `hits` answer (`None`: nothing searched yet).
    pub searched: Option<String>,
    /// The last search started; results of earlier ones are dropped.
    pub seq: u64,
    /// A search is running.
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub from: String,
    pub time: i64,
    pub text: String,
}

/// A search's results, for `AppEvent::Found`.
pub struct Found {
    pub chat: String,
    pub seq: u64,
    pub query: String,
    pub hits: Result<Vec<Hit>, String>,
}

/// Where `needle` (already lower-case) first matches `text` without case:
/// the byte range in `text`.
pub fn match_range(text: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let wanted: Vec<char> = needle.chars().collect();
    text.char_indices().find_map(|(start, _)| {
        let mut lowered = text[start..].chars().flat_map(char::to_lowercase);
        if !wanted.iter().all(|&w| lowered.next() == Some(w)) {
            return None;
        }
        // The end: as many characters of `text` as it took to cover the needle.
        let mut covered = 0;
        let mut end = start;
        for (i, c) in text[start..].char_indices() {
            if covered >= wanted.len() {
                break;
            }
            covered += c.to_lowercase().count();
            end = start + i + c.len_utf8();
        }
        Some(start..end)
    })
}

/// `text` on one line, fitted to `width` columns around `range`: some
/// context before the match (cut with `…`), the match, then what fits.
/// Returns (before, match, after).
fn snippet(text: &str, range: std::ops::Range<usize>, width: usize) -> (String, String, String) {
    let flat = |s: &str| s.replace(['\n', '\r', '\t'], " ");
    let (before, hit, after) = (
        flat(&text[..range.start]),
        flat(&text[range.clone()]),
        flat(&text[range.end..]),
    );
    // Up to a third of the row leads in to the match.
    let lead = (width / 3).max(4);
    let before = if before.width() > lead {
        let mut kept = String::new();
        let mut w = 1;
        for c in before.chars().rev() {
            let cw = c.width().unwrap_or(0);
            if w + cw > lead {
                break;
            }
            kept.insert(0, c);
            w += cw;
        }
        format!("…{kept}")
    } else {
        before
    };
    let hit = ellipsize(&hit, width.saturating_sub(before.width()));
    let after = ellipsize(&after, width.saturating_sub(before.width() + hit.width()));
    (before, hit, after)
}

impl App {
    /// Search the stored messages of `chat` (the last query there comes back).
    pub fn open_find(&mut self, chat: String) {
        let query = match &self.last_find {
            Some((c, q)) if *c == chat => q.clone(),
            _ => String::new(),
        };
        let mut f = Find {
            chat,
            query: TextInput::from(query.as_str()),
            hits: Vec::new(),
            sel: 0,
            searched: None,
            seq: 0,
            busy: false,
        };
        self.start_find(&mut f);
        self.modal = Some(Modal::Find(f));
    }

    /// Search for what `f.query` holds now.
    fn start_find(&mut self, f: &mut Find) {
        f.seq += 1;
        f.sel = 0;
        let query = f.query.as_str().trim().to_owned();
        self.last_find = Some((f.chat.clone(), f.query.as_str().to_owned()));
        if query.is_empty() {
            f.hits.clear();
            f.searched = None;
            f.busy = false;
            return;
        }
        f.busy = true;
        let (store, tx, chat, seq) = (self.store.clone(), self.tx.clone(), f.chat.clone(), f.seq);
        tokio::task::spawn_blocking(move || {
            let hits = store
                .search_messages(&chat, &query, LIMIT)
                .map(|found| {
                    found
                        .into_iter()
                        .map(|sm| Hit {
                            id: sm.message.id.clone(),
                            from: sm.message.from.clone(),
                            time: sm.message.created_time.unwrap_or(0),
                            text: sm.message.text.unwrap_or_default(),
                        })
                        .collect()
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::Found(Box::new(Found {
                chat,
                seq,
                query,
                hits,
            })));
        });
    }

    /// A search finished: shown if it answers what is typed now.
    pub fn on_found(&mut self, found: Found) {
        let Some(Modal::Find(f)) = self.modal.as_mut() else {
            return;
        };
        if f.chat != found.chat || f.seq != found.seq {
            return;
        }
        f.busy = false;
        match found.hits {
            Ok(hits) => {
                f.hits = hits;
                f.searched = Some(found.query);
            }
            Err(e) => {
                f.hits.clear();
                f.searched = None;
                self.status = format!("could not search: {e}");
            }
        }
    }

    pub fn find_key(&mut self, mut f: Find, cmd: Option<Cmd>, k: KeyEvent) {
        let last = f.hits.len().saturating_sub(1);
        let page = self.hit.modal_rows.len().max(1);
        match cmd {
            Some(Cmd::Back) => return,
            Some(Cmd::Up) => f.sel = f.sel.saturating_sub(1),
            Some(Cmd::Down) => f.sel = (f.sel + 1).min(last),
            Some(Cmd::PageUp) => f.sel = f.sel.saturating_sub(page),
            Some(Cmd::PageDown) => f.sel = (f.sel + page).min(last),
            Some(Cmd::Confirm) => match f.hits.get(f.sel) {
                Some(hit) => {
                    let (chat, id, time) = (f.chat.clone(), hit.id.clone(), hit.time);
                    if self.show_message(&chat, &id) {
                        self.status = format!(
                            "message of {} · {}",
                            chat_time(time),
                            keymap::with_keys(keymap::Ctx::Messages, Cmd::FindInChat, "searches again")
                        );
                    } else {
                        self.status = "that message is not in the loaded history".into();
                    }
                    return;
                }
                None if f.busy => {}
                None => self.status = "nothing to show: type what to look for".into(),
            },
            cmd => {
                if f.query.edit(cmd, &k) == Edit::Changed {
                    self.start_find(&mut f);
                }
            }
        }
        self.modal = Some(Modal::Find(f));
    }

    /// Pasted text goes into the query.
    pub fn find_paste(&mut self, mut f: Find, text: &str) {
        f.query.insert_str(&text.replace(['\r', '\n'], " "));
        self.start_find(&mut f);
        self.modal = Some(Modal::Find(f));
    }

    pub fn draw_find(&mut self, f: &mut Frame, area: Rect, find: &mut Find, hint: String) {
        let t = theme::t();
        let r = modal::popup(area, 90, (area.height * 4 / 5).max(10));
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.accent))
            .title(Span::styled(
                format!(" 🔎 Search in {} ", self.name(&find.chat)),
                Style::new().bold(),
            ))
            .title_bottom(frame_hint(&hint, r));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [field, count, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        let (shown, cursor) = find.query.view(field.width.saturating_sub(3) as usize);
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::raw("🔍 "), Span::raw(shown.to_owned())]))
                .style(Style::new().bg(t.surface)),
            field,
        );
        f.set_cursor_position(Position::new(field.x + 3 + cursor, field.y));
        let n = find.hits.len();
        let summary = match (&find.searched, find.busy) {
            (_, true) => "searching…".to_owned(),
            (None, false) => "type to search the messages stored on this computer".to_owned(),
            (Some(_), false) if n == 0 => "no match".to_owned(),
            (Some(_), false) if n >= LIMIT => format!("the newest {LIMIT} matches"),
            (Some(_), false) => format!("{n} match{}", if n == 1 { "" } else { "es" }),
        };
        f.render_widget(Paragraph::new(summary).style(theme::muted()), count);
        let needle = find.searched.as_deref().unwrap_or("").to_lowercase();
        let h = list.height as usize;
        find.sel = find.sel.min(n.saturating_sub(1));
        let first = modal::scroll_first(find.sel, h);
        for (row, (i, hit)) in find.hits.iter().enumerate().skip(first).take(h).enumerate() {
            let y = list.y + row as u16;
            self.hit.modal_rows.push((y, i));
            let sel = i == find.sel;
            let base = if sel {
                Style::new().bg(t.accent_soft).fg(t.text)
            } else {
                Style::new()
            };
            let when = format!("{:>9} ", chat_time(hit.time));
            let who = if hit.from == self.me {
                "You".to_owned()
            } else {
                self.name(&hit.from)
            };
            let who = format!("{} ", ellipsize(&who, 14));
            let room = (list.width as usize).saturating_sub(when.width() + who.width());
            let (before, matched, after) = match match_range(&hit.text, &needle) {
                Some(range) => snippet(&hit.text, range, room),
                None => (
                    ellipsize(&hit.text.replace('\n', " "), room),
                    String::new(),
                    String::new(),
                ),
            };
            let used = when.width() + who.width() + before.width() + matched.width() + after.width();
            let pad = " ".repeat((list.width as usize).saturating_sub(used));
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(when, base.fg(t.muted)),
                    Span::styled(who, base.fg(sender_color(&hit.from)).bold()),
                    Span::styled(before, base),
                    Span::styled(matched, base.fg(t.warn).bold()),
                    Span::styled(format!("{after}{pad}"), base),
                ])),
                Rect::new(list.x, y, list.width, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_found_without_case_and_cut_around() {
        let text = "Meet at the CAFÉ at noon";
        let r = match_range(text, "café").unwrap();
        assert_eq!(&text[r], "CAFÉ");
        assert_eq!(match_range("ไปร้านกาแฟกัน", "กาแฟ").map(|r| r.start), Some(18));
        assert_eq!(match_range("abc", "x"), None);
        assert_eq!(match_range("abc", ""), None);
        // Context before a late match is cut, the match stays whole.
        let long = format!("{} the word here and more after it", "x".repeat(100));
        let r = match_range(&long, "word").unwrap();
        let (before, hit, after) = snippet(&long, r, 40);
        assert!(before.starts_with('…') && before.width() <= 13, "{before:?}");
        assert_eq!(hit, "word");
        assert!(before.width() + hit.width() + after.width() <= 40);
        // Line breaks do not break the row.
        let (b, h, a) = snippet("one\ntwo three", 4..7, 40);
        assert_eq!((b.as_str(), h.as_str(), a.as_str()), ("one ", "two", " three"));
    }
}
