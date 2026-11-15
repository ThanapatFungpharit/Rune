//! Profile page, laid out like LINE's: the cover photo as a band across the
//! top (for groups, and users with LINE's default cover, the profile picture
//! blurred and darkened instead), a round avatar, the name and status
//! message, Chat / Photo / Cover buttons, and members for groups.

use crate::*;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use rune::PictureSize;

/// What background work hands the UI.
pub struct ProfileLoaded {
    name: String,
    status: String,
    members: Vec<String>,
    photo: Option<Vec<u8>>,
    /// The cover photo as downloaded (users with a cover of their own).
    cover_photo: Option<Vec<u8>>,
    avatar: Option<DynamicImage>,
    cover: Option<DynamicImage>,
}

pub struct ProfileData {
    name: String,
    status: String,
    /// Member mids, for groups.
    members: Vec<String>,
    /// The picture as downloaded, for `o`.
    photo: Option<Vec<u8>>,
    /// The cover photo as downloaded, for `v`.
    cover_photo: Option<Vec<u8>>,
    avatar: Option<Picture>,
    cover: Option<Picture>,
}

/// Clickable things on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileAction {
    Chat,
    Photo,
    Cover,
    /// Your own profile: pick a new cover.
    ChangeCover,
}

/// Square crop with a round, anti-aliased alpha mask.
pub fn round_avatar(img: &DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    let side = w.min(h);
    let sq = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let n = 512.min(side.max(64));
    let mut out: RgbaImage = sq.resize_exact(n, n, FilterType::Lanczos3).to_rgba8();
    let r = n as f32 / 2.0;
    for (x, y, p) in out.enumerate_pixels_mut() {
        let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
        let a = (r - d).clamp(0.0, 1.0);
        p.0[3] = (f32::from(p.0[3]) * a) as u8;
    }
    DynamicImage::ImageRgba8(out)
}

/// The middle 4:1 band of a cover photo, as LINE crops it behind the avatar.
fn cover_band(img: &DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    let ch = (w / 4).min(h).max(1);
    let cw = (ch * 4).min(w);
    let band = img.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch);
    if cw > MAX_PICTURE {
        band.resize(MAX_PICTURE, MAX_PICTURE, FilterType::Triangle)
    } else {
        band
    }
}

/// A 4:1 band of the picture, blurred and darkened, to stand in for a cover.
fn cover_from(img: &DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    let ch = (w / 4).min(h).max(1);
    let band = img.crop_imm(0, (h - ch) / 2, w, ch);
    let small = band.resize_exact(160, 40, FilterType::Triangle).to_rgba8();
    let mut out = image::imageops::blur(&small, 3.0);
    for p in out.pixels_mut() {
        let Rgba([r, g, b, _]) = *p;
        *p = Rgba([
            (f32::from(r) * 0.55) as u8,
            (f32::from(g) * 0.55) as u8,
            (f32::from(b) * 0.55) as u8,
            255,
        ]);
    }
    DynamicImage::ImageRgba8(out)
}

/// Decode a profile's pictures (off the UI thread) into what the page shows.
async fn decode_profile(
    name: String,
    status: String,
    members: Vec<String>,
    photo: Option<Vec<u8>>,
    cover_photo: Option<Vec<u8>>,
) -> ProfileLoaded {
    let (p, c) = (photo.clone(), cover_photo.clone());
    let (avatar, cover) = tokio::task::spawn_blocking(move || {
        let pic = p.and_then(|b| image::load_from_memory(&b).ok());
        let cover = c
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|img| cover_band(&img))
            .or_else(|| pic.as_ref().map(cover_from));
        (pic.as_ref().map(round_avatar), cover)
    })
    .await
    .unwrap_or((None, None));
    ProfileLoaded {
        name,
        status,
        members,
        photo,
        cover_photo,
        avatar,
        cover,
    }
}

/// Fetch a profile (yours, a friend's or a group's) and its pictures.
///
/// What the store remembers of it is shown first (when `show_cached`), then
/// it is refreshed from LINE; pictures already in the store are not
/// downloaded again, and the page is only redrawn when something changed.
pub fn spawn_profile(
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    store: Store,
    mid: String,
    me: String,
    show_cached: bool,
) {
    tokio::spawn(async move {
        let cached = store.user(&mid).ok().flatten().filter(|u| !u.name.is_empty());
        if show_cached && let Some(u) = &cached {
            let photo = u
                .picture
                .as_deref()
                .and_then(|p| store.image(&picture_key(p, PictureSize::Full)).ok().flatten());
            let cover = u
                .cover
                .as_deref()
                .and_then(|c| store.image(&cover_key(c)).ok().flatten());
            let loaded =
                decode_profile(u.name.clone(), u.status.clone(), u.members.clone(), photo, cover).await;
            let _ = tx.send(AppEvent::Profile(mid.clone(), Ok(Box::new(loaded))));
        }
        let info = async {
            if mid == me {
                let p = client.profile().await.map_err(|e| e.to_string())?;
                Ok((p.display_name, p.status_message, p.picture_path, Vec::new()))
            } else if MidType::of(&mid) == MidType::User {
                let mut cs = client
                    .contacts(std::slice::from_ref(&mid))
                    .await
                    .map_err(|e| e.to_string())?;
                let c = cs.remove(&mid).ok_or("no such user")?;
                Ok((c.name().to_owned(), c.status_message, c.picture_path, Vec::new()))
            } else {
                let c = client
                    .chats(std::slice::from_ref(&mid))
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .next()
                    .ok_or("no such group")?;
                let mut members = c.member_mids();
                // You first, then by mid (names arrive later).
                members.sort_by_key(|m| (*m != me, m.clone()));
                let path = c.picture();
                Ok::<_, String>((c.chat_name, String::new(), path, members))
            }
        };
        // Users have a cover photo (`None` for LINE's default one); groups do not.
        // `Err` when it could not be checked (the stored one is kept then).
        let cover_info = async {
            if MidType::of(&mid) != MidType::User {
                return Ok(None);
            }
            client.profile_cover(&mid).await.map(Some).map_err(|e| {
                tracing::debug!("getCover failed: {e}");
            })
        };
        let (info, cover_info) = tokio::join!(info, cover_info);
        let (name, status, path, members) = match info {
            Ok(i) => i,
            Err(e) => {
                // A stored copy is on screen already; keep it.
                if !(show_cached && cached.is_some()) {
                    let _ = tx.send(AppEvent::Profile(mid, Err(e)));
                } else {
                    tracing::debug!("profile refresh for {mid} failed: {e}");
                }
                return;
            }
        };
        let path = path.filter(|p| !p.trim().is_empty());
        let mut fresh = false;
        let photo = match path.as_deref() {
            Some(p) => cached_picture(&client, &store, p, PictureSize::Full)
                .await
                .map(|(b, new)| {
                    fresh |= new;
                    b
                }),
            None => None,
        };
        let (cover_path, cover_photo) = match cover_info {
            Ok(Some(c)) => match c.custom_image().map(|o| o.path()) {
                Some(cp) => {
                    let key = cover_key(&cp);
                    let bytes = match store.image(&key).ok().flatten() {
                        Some(b) => Some(b),
                        None => match client.download_cover(&c).await {
                            Ok(Some(b)) => {
                                let _ = store.put_image(&key, &b);
                                fresh = true;
                                Some(b)
                            }
                            Ok(None) => None,
                            Err(e) => {
                                tracing::debug!("cover download failed: {e}");
                                None
                            }
                        },
                    };
                    (Some(cp), bytes)
                }
                None => (None, None),
            },
            Ok(None) => (None, None),
            Err(()) => {
                let cp = cached.as_ref().and_then(|u| u.cover.clone());
                let bytes = cp
                    .as_deref()
                    .and_then(|c| store.image(&cover_key(c)).ok().flatten());
                (cp, bytes)
            }
        };
        let row = UserRow {
            mid: mid.clone(),
            name: name.clone(),
            status: status.clone(),
            picture: path,
            cover: cover_path,
            members: members.clone(),
            updated: 0,
        };
        let unchanged = cached.as_ref().is_some_and(|u| {
            UserRow {
                updated: 0,
                ..u.clone()
            } == row
        });
        if !unchanged {
            let _ = store.upsert_user(&row);
        }
        if unchanged && !fresh && show_cached {
            return;
        }
        let loaded = decode_profile(name, status, members, photo, cover_photo).await;
        let _ = tx.send(AppEvent::Profile(mid, Ok(Box::new(loaded))));
    });
}

impl App {
    /// Show a profile (on top of any profile already open).
    pub fn open_profile(&mut self, mid: String) {
        if mid.is_empty() {
            self.status = "no profile for this message".into();
            return;
        }
        match self.profiles.get(&mid) {
            Some(Load::Loading) => {}
            // Shown as it is while it is checked for changes.
            Some(Load::Ready(_)) => self.fetch_profile(&mid, false),
            // A failed load is tried again rather than shown failed forever.
            _ => {
                self.profiles.insert(mid.clone(), Load::Loading);
                self.fetch_profile(&mid, true);
            }
        }
        self.viewer = None;
        // Clicking the profile that is already open does not stack it twice.
        if self.profile.last() != Some(&mid) {
            self.profile.push(mid);
        }
        self.profile_sel = None;
        self.profile_btn = 0;
        self.needs_clear = true;
    }

    fn fetch_profile(&self, mid: &str, show_cached: bool) {
        spawn_profile(
            self.client.clone(),
            self.tx.clone(),
            self.store.clone(),
            mid.to_owned(),
            self.me.clone(),
            show_cached,
        );
    }

    /// Fetch a profile again (e.g. after changing it), refreshing the page if open.
    pub fn reload_profile(&mut self, mid: &str) {
        self.profiles.remove(mid);
        if self.profile.iter().any(|m| m == mid) {
            self.profiles.insert(mid.to_owned(), Load::Loading);
            // Straight from LINE: the stored copy is what just changed.
            self.fetch_profile(mid, false);
            self.needs_clear = true;
        }
    }

    /// Back to the previous profile, or close the page.
    pub fn profile_back(&mut self) {
        self.profile.pop();
        self.profile_sel = None;
        self.profile_btn = 0;
        self.needs_clear = true;
    }

    pub fn close_profile(&mut self) {
        self.profile.clear();
        self.profile_sel = None;
        self.profile_btn = 0;
        self.needs_clear = true;
    }

    /// The buttons the open page has, left to right.
    fn profile_buttons(&self) -> Vec<(&'static str, ProfileAction)> {
        let Some(mid) = self.profile.last() else {
            return Vec::new();
        };
        let mine = *mid == self.me;
        let data = match self.profiles.get(mid) {
            Some(Load::Ready(p)) => Some(p),
            _ => None,
        };
        let mut btns = Vec::new();
        if !mine {
            btns.push((" 💬 Chat ", ProfileAction::Chat));
        }
        if data.is_some_and(|p| p.photo.is_some()) {
            btns.push((" 🖼 Photo ", ProfileAction::Photo));
        }
        if data.is_some_and(|p| p.cover_photo.is_some()) {
            btns.push((" 🏞 Cover ", ProfileAction::Cover));
        }
        if mine {
            btns.push((" ✏ Change cover ", ProfileAction::ChangeCover));
        }
        btns
    }

    pub fn on_profile_loaded(&mut self, mid: String, res: Result<Box<ProfileLoaded>, String>) {
        // New pictures over old ones on the open page: wipe leftover graphics.
        if self.profile.last() == Some(&mid) && matches!(self.profiles.get(&mid), Some(Load::Ready(_))) {
            self.needs_clear = true;
        }
        let slot = match res {
            Ok(p) => {
                let p = *p;
                // Member names come in through the usual name lookup.
                self.want_names(p.members.iter().cloned());
                if !p.name.is_empty() && mid != self.me {
                    self.names.insert(mid.clone(), p.name.clone());
                }
                Load::Ready(ProfileData {
                    name: p.name,
                    status: p.status,
                    members: p.members,
                    photo: p.photo,
                    cover_photo: p.cover_photo,
                    avatar: p.avatar.map(|i| Picture::new(i, self.picker.as_ref())),
                    cover: p.cover.map(|i| Picture::new(i, self.picker.as_ref())),
                })
            }
            Err(e) => Load::Failed(e),
        };
        self.profiles.insert(mid, slot);
    }

    pub fn profile_action(&mut self, action: ProfileAction) {
        let Some(mid) = self.profile.last().cloned() else {
            return;
        };
        match action {
            ProfileAction::ChangeCover => {
                if mid == self.me {
                    self.run_action(Action::ChangeCover);
                }
            }
            ProfileAction::Chat => {
                if mid == self.me {
                    return;
                }
                self.close_profile();
                self.enter_chat(mid);
            }
            ProfileAction::Photo | ProfileAction::Cover => {
                let Some(Load::Ready(p)) = self.profiles.get(&mid) else {
                    return;
                };
                let (bytes, what) = if action == ProfileAction::Photo {
                    (p.photo.clone(), "profile picture")
                } else {
                    (p.cover_photo.clone(), "cover photo")
                };
                let Some(bytes) = bytes else {
                    self.status = format!("no {what}");
                    return;
                };
                let stem = if p.name.is_empty() { "profile" } else { &p.name };
                let suffix = if action == ProfileAction::Cover {
                    "-cover"
                } else {
                    ""
                };
                let name = format!("{stem}{suffix}.{}", sniff_ext(&bytes));
                self.status = match store_for_viewing(&name, &bytes).and_then(|path| {
                    open_external(&path)?;
                    Ok(path)
                }) {
                    Ok(path) => format!("opened {}", path.display()),
                    Err(e) => format!("could not open: {e}"),
                };
            }
        }
    }

    fn profile_members(&self) -> usize {
        match self.profile.last().and_then(|m| self.profiles.get(m)) {
            Some(Load::Ready(p)) => p.members.len(),
            _ => 0,
        }
    }

    /// Keys while the page is open: `←`/`→` pick a button, `↑`/`↓` a member
    /// (above the first member are the buttons), `Enter` presses or opens.
    pub fn profile_key(&mut self, k: KeyEvent) {
        use keymap::Cmd;
        let n = self.profile_members();
        let buttons = self.profile_buttons();
        match keymap::resolve(keymap::Ctx::Profile, &k) {
            Some(Cmd::Back) => self.profile_back(),
            Some(Cmd::Up) => {
                self.profile_sel = match self.profile_sel {
                    Some(0) | None => None,
                    Some(i) => Some(i - 1),
                }
            }
            Some(Cmd::Down) if n > 0 => {
                self.profile_sel = Some(self.profile_sel.map_or(0, |i| (i + 1).min(n - 1)));
            }
            Some(Cmd::Left) => {
                self.profile_sel = None;
                self.profile_btn = self.profile_btn.saturating_sub(1);
            }
            Some(Cmd::Right) => {
                self.profile_sel = None;
                self.profile_btn = (self.profile_btn + 1).min(buttons.len().saturating_sub(1));
            }
            Some(Cmd::Confirm) => match self.profile_sel {
                Some(i) => self.open_member(i),
                None => {
                    // The buttons may have changed (a picture arrived) since the last move.
                    let pick = buttons.get(self.profile_btn.min(buttons.len().saturating_sub(1)));
                    if let Some(&(_, action)) = pick {
                        self.profile_action(action);
                    }
                }
            },
            _ => {}
        }
    }

    fn open_member(&mut self, i: usize) {
        let member = match self.profile.last().and_then(|m| self.profiles.get(m)) {
            Some(Load::Ready(p)) => p.members.get(i).cloned(),
            _ => None,
        };
        if let Some(m) = member {
            self.open_profile(m);
        }
    }

    /// Mouse while the page is open. Returns after handling every event.
    pub fn profile_mouse(&mut self, m: MouseEvent) {
        let at = Position::new(m.column, m.row);
        let inside = self.hit.profile.is_some_and(|r| r.contains(at));
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => self.profile_back(),
            MouseEventKind::Down(MouseButton::Left) if !inside => self.close_profile(),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, a)) = self.hit.profile_buttons.iter().find(|(r, _)| r.contains(at)) {
                    self.profile_action(a);
                } else if let Some(&(_, i)) = self.hit.profile_rows.iter().find(|(y, _)| *y == m.row) {
                    self.open_member(i);
                }
            }
            MouseEventKind::ScrollDown => self.profile_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            MouseEventKind::ScrollUp => self.profile_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            _ => {}
        }
    }

    pub fn draw_profile(&mut self, f: &mut Frame, area: Rect) {
        self.hit.profile_buttons.clear();
        self.hit.profile_rows.clear();
        let Some(mid) = self.profile.last().cloned() else {
            return;
        };
        let w = (area.width * 94 / 100).min(76);
        let [area] = Layout::horizontal([Constraint::Length(w)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Percentage(94)])
            .flex(Flex::Center)
            .areas(area);
        crate::picture::cover(f, area);
        self.hit.profile = Some(area);
        let kind = if mid == self.me {
            " My profile "
        } else if MidType::of(&mid) == MidType::User {
            " Profile "
        } else {
            " Group "
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(Color::Green))
            .title(kind)
            // Its own keys, also under an overlay opened from it.
            .title_bottom(Span::styled(
                keymap::hint(keymap::Ctx::Profile, |_| true),
                Style::new().dim(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let accent = sender_color(&mid);
        let fallback_name = self.name(&mid);
        let btns = self.profile_buttons();
        let focused_btn = self.profile_btn.min(btns.len().saturating_sub(1));
        let names: Vec<String> = match self.profiles.get(&mid) {
            Some(Load::Ready(p)) => p.members.iter().map(|m| self.name(m)).collect(),
            _ => Vec::new(),
        };
        let (cell_mode, sel) = (self.cell_mode, self.profile_sel);
        let data = match self.profiles.get_mut(&mid) {
            Some(Load::Ready(p)) => p,
            Some(Load::Failed(e)) => {
                let text = format!("{fallback_name}\n\nCould not load the profile: {e}");
                f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
                return;
            }
            _ => {
                f.render_widget(
                    Paragraph::new("loading profile…").style(Style::new().dim()),
                    inner,
                );
                return;
            }
        };

        // Cover band, then the avatar centred beneath it.
        let cover_h = (inner.height / 4).clamp(3, 8);
        let [cover, rest] = Layout::vertical([Constraint::Length(cover_h), Constraint::Min(0)]).areas(inner);
        match data.cover.as_mut() {
            Some(c) => c.render(f, cover, cover.as_size(), 0, cell_mode, FilterType::Triangle),
            None => f.render_widget(Block::new().style(Style::new().bg(accent)), cover),
        }
        let avatar_max = Size::new(20, (rest.height / 3).clamp(3, 10));
        let avatar_size = data
            .avatar
            .as_ref()
            .map_or(Size::new(10, 5), |a| a.size_for(avatar_max));
        let [_, avatar_row, text] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(avatar_size.height),
            Constraint::Min(0),
        ])
        .areas(rest);
        let [avatar] = Layout::horizontal([Constraint::Length(avatar_size.width)])
            .flex(Flex::Center)
            .areas(avatar_row);
        match data.avatar.as_mut() {
            Some(a) => a.render(f, avatar, avatar_size, 0, cell_mode, FilterType::Lanczos3),
            None => f.render_widget(
                Paragraph::new(vec![Line::raw(""), Line::raw(""), Line::raw("👤")])
                    .centered()
                    .block(
                        Block::bordered()
                            .border_type(BorderType::Rounded)
                            .border_style(Style::new().fg(accent)),
                    ),
                avatar,
            ),
        }

        // Name, status, buttons.
        let name = if data.name.is_empty() {
            fallback_name
        } else {
            data.name.clone()
        };
        let mut lines = vec![Line::raw(""), Line::styled(name, Style::new().bold()).centered()];
        let tw = usize::from(text.width.saturating_sub(4)).max(10);
        for l in wrap(data.status.trim(), tw).into_iter().take(3) {
            if !l.is_empty() {
                lines.push(Line::styled(l, Style::new().dim()).centered());
            }
        }
        lines.push(Line::raw(""));
        let head_h = lines.len() as u16;
        let [head, buttons, list] = Layout::vertical([
            Constraint::Length(head_h),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(text);
        f.render_widget(Paragraph::new(lines), head);

        let widths: Vec<Constraint> = btns
            .iter()
            .map(|(l, _)| Constraint::Length(l.width() as u16 + 2))
            .collect();
        let cells = Layout::horizontal(widths)
            .flex(Flex::Center)
            .spacing(2)
            .split(buttons);
        for (i, (r, (label, action))) in cells.iter().zip(&btns).enumerate() {
            let focused = i == focused_btn && sel.is_none();
            let style = if focused {
                Style::new().fg(Color::Green).bold()
            } else {
                Style::new()
            };
            f.render_widget(
                Paragraph::new(*label).style(style).block(
                    Block::bordered()
                        .border_type(BorderType::Rounded)
                        .border_style(style),
                ),
                *r,
            );
            self.hit.profile_buttons.push((*r, *action));
        }

        // Members.
        if names.is_empty() || list.height < 3 {
            return;
        }
        let [title, rows] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(list);
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::styled(format!(" Members · {}", names.len()), Style::new().bold()),
            ]),
            title,
        );
        let height = usize::from(rows.height);
        let first = sel.map_or(0, |s| (s + 1).saturating_sub(height));
        for (row, (i, n)) in names.iter().enumerate().skip(first).take(height).enumerate() {
            let y = rows.y + row as u16;
            let style = if sel == Some(i) {
                Style::new().bg(Color::DarkGray).bold()
            } else {
                Style::new()
            };
            let line = Line::from(vec![
                Span::raw("  👤 "),
                Span::styled(n.clone(), style.fg(sender_color(&data.members[i]))),
            ]);
            f.render_widget(
                Paragraph::new(line).style(style),
                Rect::new(rows.x, y, rows.width, 1),
            );
            self.hit.profile_rows.push((y, i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_is_round() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 100, image::Rgb([200, 10, 10])));
        let a = round_avatar(&img).to_rgba8();
        assert_eq!(a.width(), a.height());
        assert_eq!(a.get_pixel(0, 0).0[3], 0, "corner is transparent");
        let c = a.width() / 2;
        assert_eq!(a.get_pixel(c, c).0[3], 255, "centre is opaque");
    }

    #[test]
    fn cover_band_is_a_centred_4_to_1_crop() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1200, 900, image::Rgb([9, 9, 9])));
        let c = cover_band(&img);
        assert_eq!((c.width(), c.height()), (1200, 300));
        let tall = DynamicImage::ImageRgb8(image::RgbImage::new(100, 1000));
        assert_eq!(cover_band(&tall).dimensions(), (100, 25));
        let huge = DynamicImage::ImageRgb8(image::RgbImage::new(4000, 3000));
        assert_eq!(cover_band(&huge).dimensions(), (MAX_PICTURE, MAX_PICTURE / 4));
    }

    #[test]
    fn cover_is_a_dark_band() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(300, 300, image::Rgb([200, 200, 200])));
        let c = cover_from(&img).to_rgba8();
        assert_eq!((c.width(), c.height()), (160, 40));
        assert!(c.get_pixel(80, 20).0[0] < 120);
    }
}
