//! The sticker picker: recently seen stickers plus the packages this account
//! owns, as a grid of pictures. "Recent" only offers stickers from owned
//! packages: LINE rejects sending one you do not own (HTTP 400, reported
//! live), and stickers you merely received are in the history too.
//!
//! The same grid picks a LINE emoji to react with: "Recent" (emoji we
//! reacted with) and the emoji packages this account owns.

use crate::*;
use rune::store::StickerRef;
use rune::{EmojiPackage, StickerPackage};

/// Grid cell size in terminal cells: stickers, emoji.
const CELL: (u16, u16) = (12, 6);
const EMOJI_CELL: (u16, u16) = (10, 5);

/// What the picker is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// A sticker to send to the open chat.
    Sticker,
    /// A LINE emoji to react to this message with.
    Emoji(String),
}

pub struct Stickers {
    pub pick: Pick,
    /// `(title, stickers)`; the first tab is "Recent".
    pub tabs: Vec<(String, Vec<StickerRef>)>,
    pub tab: usize,
    pub sel: usize,
    /// Columns in the last drawn grid (for ↑/↓).
    pub cols: usize,
    /// First visible row.
    pub top: usize,
    /// Cells on screen: rect and index.
    pub cells: Vec<(Rect, usize)>,
    pub loading: bool,
    /// Every sticker seen in the history, newest first, before the
    /// ownership filter (re-applied when the owned list arrives).
    pub seen: Vec<StickerRef>,
}

/// Stickers shown in "Recent".
const RECENT_MAX: usize = 48;

/// Cache key for owned packages in the store.
const CACHE_KEY: &str = "cache.sticker_packages";
/// Refetch owned packages after a day.
const CACHE_MAX_AGE_MS: i64 = 24 * 3600 * 1000;
/// Cache key for owned emoji packages in the store.
const EMOJI_CACHE_KEY: &str = "cache.emoji_packages";
/// Emoji we reacted with, newest first (`["<package>/<emoji>", …]`).
const RECENT_EMOJI_KEY: &str = "cache.recent_emoji";
/// Emoji kept in "Recent".
const RECENT_EMOJI_MAX: usize = 24;

/// The emoji tabs: "Recent" (from owned packages only), then one per package.
fn emoji_tabs(recent: &[String], owned: &[EmojiPackage]) -> Vec<(String, Vec<StickerRef>)> {
    let recent = recent
        .iter()
        .filter_map(|k| {
            let (pkg, id) = k.split_once('/')?;
            let p = owned.iter().find(|p| p.id == pkg)?;
            p.emojis
                .iter()
                .any(|e| e.0 == id)
                .then(|| (p.id.clone(), id.to_owned(), p.version))
        })
        .collect();
    let mut tabs = vec![("🕘 Recent".to_owned(), recent)];
    for p in owned {
        let refs = p
            .emojis
            .iter()
            .map(|e| (p.id.clone(), e.0.clone(), p.version))
            .collect();
        tabs.push((p.name.clone(), refs));
    }
    tabs
}

fn encode_emoji_packages(p: &[EmojiPackage]) -> String {
    serde_json::json!({
        "at": now_ms(),
        "packages": p.iter().map(|p| serde_json::json!({
            "id": p.id, "name": p.name, "version": p.version, "resourceType": p.resource_type,
            "emojis": p.emojis,
        })).collect::<Vec<_>>()
    })
    .to_string()
}

pub(crate) fn decode_emoji_packages(s: &str) -> Option<(i64, Vec<EmojiPackage>)> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let at = v["at"].as_i64()?;
    let list = v["packages"]
        .as_array()?
        .iter()
        .map(|p| EmojiPackage {
            id: p["id"].as_str().unwrap_or_default().into(),
            name: p["name"].as_str().unwrap_or_default().into(),
            version: p["version"].as_u64().unwrap_or(1) as u32,
            resource_type: p["resourceType"].as_i64().unwrap_or(1),
            emojis: serde_json::from_value(p["emojis"].clone()).unwrap_or_default(),
        })
        .collect();
    Some((at, list))
}

/// The tabs: "Recent" (seen stickers from owned packages only), then one per pack.
fn tabs_from(seen: &[StickerRef], owned: &[StickerPackage]) -> Vec<(String, Vec<StickerRef>)> {
    let recent = seen
        .iter()
        .filter(|st| owned.iter().any(|p| p.id == st.0))
        .take(RECENT_MAX)
        .cloned()
        .collect();
    let mut tabs = vec![("🕘 Recent".to_owned(), recent)];
    for p in owned {
        let refs = p
            .sticker_ids
            .iter()
            .map(|id| (p.id.clone(), id.clone(), p.version))
            .collect();
        tabs.push((p.name.clone(), refs));
    }
    tabs
}

fn encode_packages(p: &[StickerPackage]) -> String {
    serde_json::json!({
        "at": now_ms(),
        "packages": p.iter().map(|p| serde_json::json!({
            "id": p.id, "name": p.name, "version": p.version, "ids": p.sticker_ids, "animated": p.animated
        })).collect::<Vec<_>>()
    })
    .to_string()
}

fn decode_packages(s: &str) -> Option<(i64, Vec<StickerPackage>)> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let at = v["at"].as_i64()?;
    let list = v["packages"]
        .as_array()?
        .iter()
        .map(|p| StickerPackage {
            id: p["id"].as_str().unwrap_or_default().into(),
            name: p["name"].as_str().unwrap_or_default().into(),
            version: p["version"].as_u64().unwrap_or(1) as u32,
            sticker_ids: p["ids"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
            animated: p["animated"].as_bool().unwrap_or(false),
        })
        .collect();
    Some((at, list))
}

impl Stickers {
    fn current(&self) -> &[StickerRef] {
        self.tabs.get(self.tab).map(|t| t.1.as_slice()).unwrap_or(&[])
    }

    pub fn ctx(&self) -> keymap::Ctx {
        match self.pick {
            Pick::Sticker => keymap::Ctx::Stickers,
            Pick::Emoji(_) => keymap::Ctx::Emoji,
        }
    }

    /// Key of an item's picture in `App::sticker_pics` (emoji ids repeat across packages).
    fn pic_key(&self, st: &StickerRef) -> String {
        match self.pick {
            Pick::Sticker => st.1.clone(),
            Pick::Emoji(_) => format!("{}/{}", st.0, st.1),
        }
    }

    /// Rebuild the tabs, staying on the same one when it still exists.
    fn set_tabs(&mut self, tabs: Vec<(String, Vec<StickerRef>)>) {
        let tab_name = self.tabs.get(self.tab).map(|t| t.0.clone());
        self.tabs = tabs;
        self.tab = tab_name
            .and_then(|n| self.tabs.iter().position(|t| t.0 == n))
            .unwrap_or(if self.tabs[0].1.is_empty() && self.tabs.len() > 1 {
                1
            } else {
                0
            });
        self.sel = self.sel.min(self.current().len().saturating_sub(1));
        self.loading = false;
    }

    /// The nearest non-empty tab after (`fwd`) or before the current one, wrapping
    /// around the tab list; the current tab itself when every other one is empty.
    fn neighbour_tab(&self, fwd: bool) -> Option<usize> {
        let len = self.tabs.len();
        (1..=len)
            .map(|d| {
                if fwd {
                    (self.tab + d) % len
                } else {
                    (self.tab + len - d) % len
                }
            })
            .find(|&t| !self.tabs[t].1.is_empty())
    }

    /// Move the selection by an arrow key. Past the edge of a pack it continues in the
    /// next pack (→ from the last item, ↓ from the last row) or the previous one (← from
    /// the first item, ↑ from the first row), keeping the column on ↑/↓.
    fn step(&mut self, cmd: keymap::Cmd) {
        use keymap::Cmd;
        let n = self.current().len();
        let cols = self.cols.max(1);
        let col = self.sel % cols;
        match cmd {
            Cmd::Left if self.sel > 0 => self.sel -= 1,
            Cmd::Right if self.sel + 1 < n => self.sel += 1,
            Cmd::Up if self.sel >= cols => self.sel -= cols,
            Cmd::Down if self.sel + cols < n => self.sel += cols,
            // Below is a shorter last row: go to its last item before leaving the pack.
            Cmd::Down if n > 0 && self.sel / cols < (n - 1) / cols => self.sel = n - 1,
            Cmd::Left => self.wrap_to(false, |m| m - 1),
            Cmd::Right => self.wrap_to(true, |_| 0),
            Cmd::Up => self.wrap_to(false, |m| ((m - 1) / cols * cols + col).min(m - 1)),
            Cmd::Down => self.wrap_to(true, |_| col),
            _ => {}
        }
    }

    /// Enter the neighbouring non-empty pack at `place(its length)` (clamped to it).
    fn wrap_to(&mut self, fwd: bool, place: impl FnOnce(usize) -> usize) {
        if let Some(t) = self.neighbour_tab(fwd) {
            let m = self.tabs[t].1.len();
            self.tab = t;
            self.sel = place(m).min(m - 1);
            self.top = 0;
        }
    }
}

impl App {
    pub fn open_stickers(&mut self) {
        if self.open.is_none() {
            self.status = "open a chat first".into();
            return;
        }
        // Look further back than we show: some of these are not ours.
        let seen = self.store.recent_stickers(RECENT_MAX * 5).unwrap_or_default();
        let cached = self
            .store
            .get(CACHE_KEY)
            .ok()
            .flatten()
            .and_then(|s| decode_packages(&s));
        let fresh = cached
            .as_ref()
            .is_some_and(|(at, _)| now_ms() - at < CACHE_MAX_AGE_MS);
        let owned = cached.map(|c| c.1).unwrap_or_default();
        let mut s = Stickers {
            pick: Pick::Sticker,
            tabs: tabs_from(&seen, &owned),
            tab: 0,
            sel: 0,
            cols: 1,
            top: 0,
            cells: Vec::new(),
            loading: !fresh,
            seen,
        };
        // No recent stickers yet: start on the first owned package.
        if s.tabs[0].1.is_empty() && s.tabs.len() > 1 {
            s.tab = 1;
        }
        if !fresh {
            let (client, tx) = (self.client.clone(), self.tx.clone());
            tokio::spawn(async move {
                let res = client.owned_sticker_packages().await.map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::StickerPackages(res));
            });
        }
        self.modal = Some(Modal::Stickers(s));
    }

    pub fn on_sticker_packages(&mut self, res: Result<Vec<StickerPackage>, String>) {
        match res {
            Ok(p) => {
                let _ = self.store.set(CACHE_KEY, &encode_packages(&p));
                if let Some(Modal::Stickers(s)) = self.modal.as_mut()
                    && s.pick == Pick::Sticker
                {
                    let tabs = tabs_from(&s.seen, &p);
                    s.set_tabs(tabs);
                }
            }
            Err(e) => {
                if let Some(Modal::Stickers(s)) = self.modal.as_mut()
                    && s.pick == Pick::Sticker
                {
                    s.loading = false;
                }
                self.status = format!("could not list your stickers: {e}");
            }
        }
    }

    /// The emoji packages from the last run (for alt texts before any request).
    pub fn load_emoji_packages(&mut self) {
        if let Some((_, p)) = self
            .store
            .get(EMOJI_CACHE_KEY)
            .ok()
            .flatten()
            .and_then(|s| decode_emoji_packages(&s))
        {
            self.emoji_packs = p;
        }
    }

    fn recent_emoji(&self) -> Vec<String> {
        self.store
            .get(RECENT_EMOJI_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Up to `n` recently used emoji that are still in an owned package.
    pub fn recent_emoji_refs(&self, n: usize) -> Vec<StickerRef> {
        let mut tabs = emoji_tabs(&self.recent_emoji(), &self.emoji_packs);
        tabs.swap_remove(0).1.into_iter().take(n).collect()
    }

    /// Put an emoji first in "Recent".
    pub fn note_recent_emoji(&mut self, package: &str, emoji: &str) {
        let key = format!("{package}/{emoji}");
        let mut recent = self.recent_emoji();
        recent.retain(|k| *k != key);
        recent.insert(0, key);
        recent.truncate(RECENT_EMOJI_MAX);
        if let Ok(s) = serde_json::to_string(&recent) {
            let _ = self.store.set(RECENT_EMOJI_KEY, &s);
        }
    }

    /// The emoji picker, reacting to message `msg`.
    pub fn open_emoji_picker(&mut self, msg: String) {
        let fresh = self
            .store
            .get(EMOJI_CACHE_KEY)
            .ok()
            .flatten()
            .and_then(|s| decode_emoji_packages(&s))
            .is_some_and(|(at, _)| now_ms() - at < CACHE_MAX_AGE_MS);
        let mut s = Stickers {
            pick: Pick::Emoji(msg),
            tabs: emoji_tabs(&self.recent_emoji(), &self.emoji_packs),
            tab: 0,
            sel: 0,
            cols: 1,
            top: 0,
            cells: Vec::new(),
            loading: !fresh,
            seen: Vec::new(),
        };
        if s.tabs[0].1.is_empty() && s.tabs.len() > 1 {
            s.tab = 1;
        }
        if !fresh {
            let (client, tx) = (self.client.clone(), self.tx.clone());
            tokio::spawn(async move {
                let res = client.owned_emoji_packages().await.map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::EmojiPackages(res));
            });
        }
        self.modal = Some(Modal::Stickers(s));
    }

    pub fn on_emoji_packages(&mut self, res: Result<Vec<EmojiPackage>, String>) {
        match res {
            Ok(p) => {
                let _ = self.store.set(EMOJI_CACHE_KEY, &encode_emoji_packages(&p));
                self.emoji_packs = p;
                let tabs = emoji_tabs(&self.recent_emoji(), &self.emoji_packs);
                if let Some(Modal::Stickers(s)) = self.modal.as_mut()
                    && matches!(s.pick, Pick::Emoji(_))
                {
                    s.set_tabs(tabs);
                }
            }
            Err(e) => {
                if let Some(Modal::Stickers(s)) = self.modal.as_mut()
                    && matches!(s.pick, Pick::Emoji(_))
                {
                    s.loading = false;
                }
                self.status = format!("could not list your emoji: {e}");
            }
        }
    }

    /// Alt text of a LINE emoji from an owned package ("thumbs up").
    pub fn emoji_alt(&self, package: &str, emoji: &str) -> Option<&str> {
        self.emoji_packs
            .iter()
            .find(|p| p.id == package)?
            .emojis
            .iter()
            .find(|e| e.0 == emoji)
            .map(|e| e.1.as_str())
    }

    /// React with a LINE emoji from an owned package.
    pub fn react_emoji(&mut self, msg: &str, package: &str, emoji: &str) {
        match self.emoji_packs.iter().find(|p| p.id == package).cloned() {
            Some(p) => self.react(msg, Some(modal::NewReaction::Emoji(p, emoji.to_owned()))),
            None => self.status = "that emoji package is no longer yours".into(),
        }
    }

    /// Send the sticker, or react with the emoji.
    fn choose(&mut self, pick: &Pick, st: &StickerRef) {
        match pick {
            Pick::Sticker => self.send_sticker(st),
            Pick::Emoji(msg) => self.react_emoji(msg, &st.0, &st.1),
        }
    }

    fn send_sticker(&mut self, st: &StickerRef) {
        let Some(chat) = self.open.clone() else { return };
        let mut out = OutgoingMessage::sticker(&chat, &st.0, &st.1, st.2);
        let reply_to = self.replying.take();
        if let Some(id) = &reply_to {
            out = out.reply_to(id);
        }
        self.send_outgoing(chat, out, "[sticker]".into());
    }

    pub fn stickers_key(&mut self, mut s: Stickers, cmd: Option<keymap::Cmd>) {
        match cmd {
            Some(keymap::Cmd::Back) => return,
            Some(cmd @ (keymap::Cmd::Left | keymap::Cmd::Right | keymap::Cmd::Up | keymap::Cmd::Down)) => {
                s.step(cmd)
            }
            Some(keymap::Cmd::NextTab) => {
                s.tab = (s.tab + 1) % s.tabs.len();
                s.sel = 0;
                s.top = 0;
            }
            Some(keymap::Cmd::PrevTab) => {
                s.tab = s.tab.checked_sub(1).unwrap_or(s.tabs.len() - 1);
                s.sel = 0;
                s.top = 0;
            }
            Some(keymap::Cmd::Confirm) => {
                if let Some(st) = s.current().get(s.sel).cloned() {
                    self.choose(&s.pick, &st);
                    return;
                }
            }
            _ => {}
        }
        self.modal = Some(Modal::Stickers(s));
    }

    /// A click selects a sticker; clicking the selected one sends it.
    pub fn stickers_click(&mut self, at: Position) {
        let Some(Modal::Stickers(s)) = self.modal.as_mut() else {
            return;
        };
        let Some(&(_, i)) = s.cells.iter().find(|(r, _)| r.contains(at)) else {
            return;
        };
        if s.sel == i {
            let (st, pick) = (s.current().get(i).cloned(), s.pick.clone());
            self.modal = None;
            if let Some(st) = st {
                self.choose(&pick, &st);
            }
        } else {
            s.sel = i;
        }
    }

    pub fn draw_stickers(&mut self, f: &mut Frame, area: Rect, s: &mut Stickers, hint: String) {
        let r = modal::popup(area, 100, area.height.saturating_sub(2).max(14));
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let emoji = matches!(s.pick, Pick::Emoji(_));
        let title = match &self.open {
            _ if emoji => " ✨ React with a LINE emoji ".into(),
            Some(chat) => format!(" 😀 Sticker to {} ", self.name(chat)),
            None => " 😀 Sticker ".into(),
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(title, Style::new().bold()))
            .title_bottom(frame_hint(&hint, r));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [tabs_row, grid] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        let name = s.tabs.get(s.tab).map(|t| t.0.clone()).unwrap_or_default();
        let mut tab_line = format!("◀ {name} ▶  ({}/{})", s.tab + 1, s.tabs.len());
        if s.loading {
            tab_line.push_str("  · loading your packs…");
        } else if emoji
            && let Some(st) = s.current().get(s.sel)
            && let Some(alt) = self.emoji_alt(&st.0, &st.1).filter(|a| !a.is_empty())
        {
            tab_line.push_str(&format!("  · {alt}"));
        }
        f.render_widget(
            Paragraph::new(ellipsize(&tab_line, tabs_row.width as usize)).style(Style::new().bold()),
            tabs_row,
        );
        let items = s.current().to_vec();
        if items.is_empty() {
            let msg = if emoji {
                if s.loading {
                    "Checking which emoji packs you own…"
                } else if s.tab == 0 {
                    "No recent emoji yet — emoji you react with show up here.\nTab: your emoji packs."
                } else {
                    "This pack is empty."
                }
            } else if s.tab == 0 {
                if s.loading {
                    "Checking which sticker packs you own…"
                } else {
                    "No recent stickers from your packs yet — stickers sent or received in your chats show up here when you own their pack.\nTab: your sticker packs."
                }
            } else {
                "This pack is empty."
            };
            f.render_widget(
                Paragraph::new(msg)
                    .style(theme::muted())
                    .wrap(Wrap { trim: true }),
                grid,
            );
            s.cells.clear();
            return;
        }
        let (cell_w, cell_h) = if emoji { EMOJI_CELL } else { CELL };
        let cols = (grid.width / cell_w).max(1) as usize;
        let rows = (grid.height / cell_h).max(1) as usize;
        s.cols = cols;
        let sel_row = s.sel / cols;
        if sel_row < s.top {
            s.top = sel_row;
        } else if sel_row >= s.top + rows {
            s.top = sel_row + 1 - rows;
        }
        s.cells.clear();
        for (k, st) in items.iter().enumerate().skip(s.top * cols).take(rows * cols) {
            let pos = k - s.top * cols;
            let cell = Rect::new(
                grid.x + (pos % cols) as u16 * cell_w,
                grid.y + (pos / cols) as u16 * cell_h,
                cell_w,
                cell_h,
            );
            s.cells.push((cell, k));
            let selected = k == s.sel;
            if selected {
                let b = Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(theme::t().accent).bold());
                f.render_widget(b, cell);
            }
            let pic_area = Rect::new(cell.x + 1, cell.y + 1, cell.width - 2, cell.height - 2);
            let key = s.pic_key(st);
            match self.sticker_pics.get_mut(&key) {
                Some(Load::Ready(p)) => {
                    let size = p.size_for(pic_area.as_size());
                    let [h] = Layout::horizontal([Constraint::Length(size.width)])
                        .flex(Flex::Center)
                        .areas(pic_area);
                    let [v] = Layout::vertical([Constraint::Length(size.height)])
                        .flex(Flex::Center)
                        .areas(h);
                    p.render(f, v, size, 0, self.cell_mode, THUMB_FILTER);
                }
                Some(Load::Loading) => {
                    f.render_widget(Paragraph::new("…").style(theme::muted()).centered(), pic_area);
                }
                Some(Load::Failed(_)) => {
                    f.render_widget(Paragraph::new("✖").style(theme::muted()).centered(), pic_area);
                }
                None => {
                    self.sticker_pics.insert(key.clone(), Load::Loading);
                    let (client, tx, st) = (self.client.clone(), self.tx.clone(), st.clone());
                    tokio::spawn(async move {
                        let bytes = if emoji {
                            client.download_emoji(&st.0, &st.1).await
                        } else {
                            client.download_sticker(&st.1, StickerImage::Static).await
                        };
                        let res = match bytes {
                            Ok(b) => decode_picture(b).await,
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(AppEvent::StickerPic(key, res));
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_cache_round_trips() {
        let p = vec![StickerPackage {
            id: "13325".into(),
            name: "Crayon".into(),
            version: 2,
            sticker_ids: vec!["1".into(), "2".into()],
            animated: true,
        }];
        let (at, back) = decode_packages(&encode_packages(&p)).unwrap();
        assert!(at > 0);
        assert_eq!(back, p);
        let tabs = tabs_from(&[], &p);
        assert_eq!(
            tabs[1].1,
            [("13325".into(), "1".into(), 2), ("13325".into(), "2".into(), 2)]
        );
    }

    #[test]
    fn emoji_tabs_and_cache() {
        let p = vec![EmojiPackage {
            id: "670e".into(),
            name: "Basic".into(),
            version: 3,
            resource_type: 1,
            emojis: vec![("001".into(), "Moon".into()), ("143".into(), "thumbs up".into())],
        }];
        let (at, back) = decode_emoji_packages(&encode_emoji_packages(&p)).unwrap();
        assert!(at > 0);
        assert_eq!(back, p);
        // Recent keeps order and drops emoji we no longer own.
        let recent = [
            "670e/143".to_owned(),
            "dead/001".into(),
            "670e/999".into(),
            "670e/001".into(),
        ];
        let tabs = emoji_tabs(&recent, &p);
        assert_eq!(
            tabs[0].1,
            [("670e".into(), "143".into(), 3), ("670e".into(), "001".into(), 3)]
        );
        assert_eq!(tabs[1].0, "Basic");
        assert_eq!(tabs[1].1.len(), 2);
    }

    #[test]
    fn recent_only_offers_owned_packs() {
        let owned = vec![StickerPackage {
            id: "1".into(),
            name: "Moon".into(),
            version: 1,
            sticker_ids: vec!["10".into()],
            animated: false,
        }];
        // A sticker someone sent from a pack we do not own is left out.
        let seen = vec![("31578466".into(), "900".into(), 1), ("1".into(), "10".into(), 1)];
        let tabs = tabs_from(&seen, &owned);
        assert_eq!(tabs[0].1, [("1".into(), "10".into(), 1)]);
        // Until the owned list is known, nothing is offered.
        assert!(tabs_from(&seen, &[])[0].1.is_empty());
    }

    /// A picker with tabs of the given sizes (sticker ids are "<tab>.<index>"), 3 columns.
    fn picker(sizes: &[usize]) -> Stickers {
        let tabs = sizes
            .iter()
            .enumerate()
            .map(|(t, &n)| {
                (
                    format!("t{t}"),
                    (0..n).map(|i| ("p".into(), format!("{t}.{i}"), 1)).collect(),
                )
            })
            .collect();
        Stickers {
            pick: Pick::Sticker,
            tabs,
            tab: 1,
            sel: 0,
            cols: 3,
            top: 5,
            cells: Vec::new(),
            loading: false,
            seen: Vec::new(),
        }
    }

    fn at(s: &Stickers) -> (usize, usize) {
        (s.tab, s.sel)
    }

    #[test]
    fn arrows_move_within_a_pack() {
        use keymap::Cmd::*;
        // Tab 1: 8 items in rows [0 1 2] [3 4 5] [6 7].
        let mut s = picker(&[2, 8, 4]);
        s.sel = 4;
        s.step(Left);
        assert_eq!(at(&s), (1, 3));
        s.step(Right);
        s.step(Right);
        assert_eq!(at(&s), (1, 5));
        s.step(Up);
        assert_eq!(at(&s), (1, 2));
        s.step(Down);
        assert_eq!(at(&s), (1, 5));
        // Nothing below in the shorter last row: go to its last item, not to the next pack.
        s.step(Down);
        assert_eq!(at(&s), (1, 7));
        assert_eq!(s.top, 5, "moving within a pack keeps the scroll position");
    }

    #[test]
    fn arrows_wrap_into_neighbouring_packs() {
        use keymap::Cmd::*;
        let mut s = picker(&[2, 8, 4]);
        // → from the last item: first item of the next pack.
        s.sel = 7;
        s.step(Right);
        assert_eq!(at(&s), (2, 0));
        assert_eq!(s.top, 0);
        // ← from the first item: last item of the previous pack.
        s.step(Left);
        assert_eq!(at(&s), (1, 7));
        // ↓ from the last row keeps the column (clamped to the next pack's length).
        s.step(Down);
        assert_eq!(at(&s), (2, 1));
        s.step(Down);
        assert_eq!(at(&s), (2, 3), "rows [0 1 2] [3]: shorter last row first");
        s.step(Down);
        assert_eq!(at(&s), (0, 0), "past the last pack wraps to the first");
        // ↑ from the first row: same column in the previous pack's last row, clamped.
        s.sel = 1;
        s.step(Up);
        assert_eq!(at(&s), (2, 3));
        s.tab = 1;
        s.sel = 2;
        s.step(Up);
        assert_eq!(at(&s), (0, 1));
        s.step(Left);
        s.step(Left);
        assert_eq!(at(&s), (2, 3), "before the first pack wraps to the last");
    }

    #[test]
    fn wrapping_skips_empty_packs() {
        use keymap::Cmd::*;
        // An empty "Recent" and an empty pack in the middle.
        let mut s = picker(&[0, 2, 0, 3]);
        s.sel = 1;
        s.step(Right);
        assert_eq!(at(&s), (3, 0));
        s.step(Left);
        assert_eq!(at(&s), (1, 1));
        s.step(Left);
        s.step(Left);
        assert_eq!(at(&s), (3, 2));
        // Arrows on an empty pack move into the nearest non-empty one.
        s.tab = 2;
        s.sel = 0;
        s.step(Down);
        assert_eq!(at(&s), (3, 0));
        s.tab = 0;
        s.step(Up);
        assert_eq!(at(&s), (3, 0));
        // A single non-empty pack wraps onto itself.
        let mut s = picker(&[0, 4]);
        s.sel = 3;
        s.step(Right);
        assert_eq!(at(&s), (1, 0));
        s.step(Up);
        assert_eq!(at(&s), (1, 3));
        // Nothing to select anywhere: nothing moves.
        let mut s = picker(&[0, 0]);
        s.step(Right);
        assert_eq!(at(&s), (1, 0));
    }
}
