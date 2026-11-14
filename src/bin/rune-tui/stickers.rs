//! The sticker picker: recently seen stickers plus the packages this account
//! owns, as a grid of pictures. "Recent" only offers stickers from owned
//! packages: LINE rejects sending one you do not own (HTTP 400, reported
//! live), and stickers you merely received are in the history too.

use super::*;
use rune::StickerPackage;
use rune::store::StickerRef;

/// Grid cell size in terminal cells.
const CELL_W: u16 = 12;
const CELL_H: u16 = 6;

pub struct Stickers {
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
                if let Some(Modal::Stickers(s)) = self.modal.as_mut() {
                    let tab_name = s.tabs.get(s.tab).map(|t| t.0.clone());
                    s.tabs = tabs_from(&s.seen, &p);
                    s.tab = tab_name
                        .and_then(|n| s.tabs.iter().position(|t| t.0 == n))
                        .unwrap_or(if s.tabs[0].1.is_empty() && s.tabs.len() > 1 {
                            1
                        } else {
                            0
                        });
                    s.loading = false;
                }
            }
            Err(e) => {
                if let Some(Modal::Stickers(s)) = self.modal.as_mut() {
                    s.loading = false;
                }
                self.status = format!("could not list your stickers: {e}");
            }
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

    pub fn stickers_key(&mut self, mut s: Stickers, k: KeyEvent) {
        let n = s.current().len();
        let cols = s.cols.max(1);
        match keymap::resolve(keymap::Ctx::Stickers, &k) {
            Some(keymap::Cmd::Back) => return,
            Some(keymap::Cmd::Left) => s.sel = s.sel.saturating_sub(1),
            Some(keymap::Cmd::Right) => s.sel = (s.sel + 1).min(n.saturating_sub(1)),
            Some(keymap::Cmd::Up) => s.sel = s.sel.saturating_sub(cols),
            Some(keymap::Cmd::Down) => {
                if s.sel + cols < n {
                    s.sel += cols;
                }
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
                    self.send_sticker(&st);
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
            let st = s.current().get(i).cloned();
            self.modal = None;
            if let Some(st) = st {
                self.send_sticker(&st);
            }
        } else {
            s.sel = i;
        }
    }

    pub fn draw_stickers(&mut self, f: &mut Frame, area: Rect, s: &mut Stickers, hint: String) {
        let r = modal::popup(area, 100, area.height.saturating_sub(2).max(14));
        f.render_widget(Clear, r);
        self.hit.modal = Some(r);
        let title = match &self.open {
            Some(chat) => format!(" 😀 Sticker to {} ", self.name(chat)),
            None => " 😀 Sticker ".into(),
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(Color::Green))
            .title(Span::styled(title, Style::new().bold()))
            .title_bottom(Span::styled(hint, Style::new().dim()));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let [tabs_row, grid] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        let name = s.tabs.get(s.tab).map(|t| t.0.clone()).unwrap_or_default();
        let mut tab_line = format!("◀ {name} ▶  ({}/{})", s.tab + 1, s.tabs.len());
        if s.loading {
            tab_line.push_str("  · loading your packs…");
        }
        f.render_widget(
            Paragraph::new(ellipsize(&tab_line, tabs_row.width as usize)).style(Style::new().bold()),
            tabs_row,
        );
        let items = s.current().to_vec();
        if items.is_empty() {
            let msg = if s.tab == 0 {
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
                    .style(Style::new().dim())
                    .wrap(Wrap { trim: true }),
                grid,
            );
            s.cells.clear();
            return;
        }
        let cols = (grid.width / CELL_W).max(1) as usize;
        let rows = (grid.height / CELL_H).max(1) as usize;
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
                grid.x + (pos % cols) as u16 * CELL_W,
                grid.y + (pos / cols) as u16 * CELL_H,
                CELL_W,
                CELL_H,
            );
            s.cells.push((cell, k));
            let selected = k == s.sel;
            if selected {
                let b = Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(Color::Green).bold());
                f.render_widget(b, cell);
            }
            let pic_area = Rect::new(cell.x + 1, cell.y + 1, cell.width - 2, cell.height - 2);
            match self.sticker_pics.get_mut(&st.1) {
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
                    f.render_widget(Paragraph::new("…").style(Style::new().dim()).centered(), pic_area);
                }
                Some(Load::Failed(_)) => {
                    f.render_widget(Paragraph::new("✖").style(Style::new().dim()).centered(), pic_area);
                }
                None => {
                    self.sticker_pics.insert(st.1.clone(), Load::Loading);
                    let (client, tx, id) = (self.client.clone(), self.tx.clone(), st.1.clone());
                    tokio::spawn(async move {
                        let res = match client.download_sticker(&id, StickerImage::Static).await {
                            Ok(b) => decode_picture(b).await,
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(AppEvent::StickerPic(id, res));
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
}
