//! Profile page, laid out like LINE's: the cover photo fills the whole page
//! as its background (for groups, and users with LINE's default cover, the
//! profile picture blurred instead), darkening towards the bottom so the
//! text stays readable, with the round avatar set into it, the name and
//! status message, Chat / Photo / Cover buttons, and members for groups.
//!
//! Text cannot sit on a picture, so the background is drawn the same way
//! everywhere: every cell of the page takes the backdrop's colour there.
//! Only the avatar is a picture (with the terminal's graphics when it has
//! them); the corners around its circle are those same cell colours.

use crate::*;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use ratatui_image::FontSize;
use rune::PictureSize;

/// What background work hands the UI.
pub struct ProfileLoaded {
    name: String,
    status: String,
    members: Vec<String>,
    photo: Option<Vec<u8>>,
    /// The cover photo as downloaded (users with a cover of their own).
    cover_photo: Option<Vec<u8>>,
    avatar: Option<RgbaImage>,
    backdrop: Option<RgbaImage>,
}

pub struct ProfileData {
    pub name: String,
    status: String,
    /// Member mids, for groups.
    members: Vec<String>,
    /// The picture as downloaded, for `o`.
    photo: Option<Vec<u8>>,
    /// The cover photo as downloaded, for `v`.
    cover_photo: Option<Vec<u8>>,
    /// The round profile picture.
    avatar: Option<RgbaImage>,
    /// What fills the page: the cover photo, or the blurred profile picture.
    backdrop: Option<RgbaImage>,
    /// The page as last laid out, rebuilt when its size changes.
    scene: Option<(SceneKey, Scene)>,
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

/// Largest backdrop kept, per side; no terminal page shows more.
const MAX_BACKDROP: u32 = 1200;

/// A cover photo as the page's backdrop (shrunk once, cropped when drawn).
fn backdrop_from_cover(img: &DynamicImage) -> RgbaImage {
    if img.width() > MAX_BACKDROP || img.height() > MAX_BACKDROP {
        img.resize(MAX_BACKDROP, MAX_BACKDROP, FilterType::Triangle)
            .to_rgba8()
    } else {
        img.to_rgba8()
    }
}

/// The profile picture, blurred, to stand in for a cover.
fn backdrop_from_picture(img: &DynamicImage) -> RgbaImage {
    let small = img.resize_exact(96, 96, FilterType::Triangle).to_rgba8();
    image::imageops::blur(&small, 4.0)
}

/// Plain backdrop for a profile with neither cover nor picture.
fn plain_backdrop() -> RgbaImage {
    RgbaImage::from_pixel(8, 8, Rgba([52, 58, 70, 255]))
}

/// Size of the page and where things sit on it, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneKey {
    page: Size,
    /// Where the text starts; the backdrop darkens from there on.
    top_rows: u16,
    /// The avatar's box, relative to the page.
    avatar: Rect,
    font: (u16, u16),
}

/// The page's background, laid out.
struct Scene {
    /// One colour per cell of the page, row by row.
    cells: Vec<Color>,
    /// The avatar over its box's cell colours.
    avatar: Option<Picture>,
}

/// How much of the backdrop's brightness is kept `y` pixels down a page
/// `h` pixels tall whose text starts at `text_y`: bright at the top, then
/// dark enough behind the text that white reads well on any photo.
fn shade(y: u32, text_y: u32, h: u32) -> f32 {
    if y < text_y {
        0.95 - 0.4 * y as f32 / text_y.max(1) as f32
    } else {
        0.55 - 0.2 * (y - text_y) as f32 / (h - text_y).max(1) as f32
    }
}

/// One colour per cell: the backdrop scaled to fill the page (cells twice
/// as tall as wide) and shaded, row by row.
fn page_cells(backdrop: &RgbaImage, key: SceneKey) -> Vec<[u8; 3]> {
    let (cols, rows) = (u32::from(key.page.width), u32::from(key.page.height));
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    // 2×4 samples per cell, averaged down to one.
    let (w, h) = (cols * 2, rows * 4);
    let text_y = u32::from(key.top_rows) * 4;
    let mut page = DynamicImage::ImageRgba8(backdrop.clone())
        .resize_to_fill(w, h, FilterType::Triangle)
        .to_rgba8();
    for (_, y, p) in page.enumerate_pixels_mut() {
        let k = shade(y, text_y, h);
        for c in &mut p.0[..3] {
            *c = (f32::from(*c) * k) as u8;
        }
    }
    image::imageops::resize(&page, cols, rows, FilterType::Triangle)
        .pixels()
        .map(|p| [p.0[0], p.0[1], p.0[2]])
        .collect()
}

/// The avatar's box in pixels: each cell's colour as a flat block, the
/// round avatar over it, so the box's corners match the cells around it.
fn avatar_tile(cells: &[[u8; 3]], key: SceneKey, avatar: &RgbaImage) -> RgbaImage {
    let (fw, fh) = (u32::from(key.font.0), u32::from(key.font.1));
    let b = key.avatar;
    let cols = usize::from(key.page.width);
    let mut tile = RgbaImage::from_fn(u32::from(b.width) * fw, u32::from(b.height) * fh, |x, y| {
        let cx = usize::from(b.x) + (x / fw) as usize;
        let cy = usize::from(b.y) + (y / fh) as usize;
        let [r, g, bl] = cells.get(cy * cols + cx).copied().unwrap_or([0, 0, 0]);
        Rgba([r, g, bl, 255])
    });
    let a = image::imageops::resize(avatar, tile.width(), tile.height(), FilterType::Lanczos3);
    image::imageops::overlay(&mut tile, &a, 0, 0);
    tile
}

/// Lay the page out: a colour for every cell, the avatar set into them.
fn build_scene(
    backdrop: &RgbaImage,
    avatar: Option<&RgbaImage>,
    key: SceneKey,
    picker: Option<&Picker>,
) -> Scene {
    let cells = page_cells(backdrop, key);
    let avatar = avatar
        .filter(|_| !key.avatar.is_empty() && !cells.is_empty())
        .map(|a| Picture::new(DynamicImage::ImageRgba8(avatar_tile(&cells, key, a)), picker));
    Scene {
        cells: cells.into_iter().map(|[r, g, b]| Color::Rgb(r, g, b)).collect(),
        avatar,
    }
}

/// Decode a profile's pictures (off the UI thread) into what the page shows.
/// `small` stands in for the avatar while the full picture is not there.
async fn decode_profile(
    name: String,
    status: String,
    members: Vec<String>,
    photo: Option<Vec<u8>>,
    small: Option<Vec<u8>>,
    cover_photo: Option<Vec<u8>>,
) -> ProfileLoaded {
    let (p, c) = (photo.clone().or(small), cover_photo.clone());
    let (avatar, backdrop) = tokio::task::spawn_blocking(move || {
        let pic = p.and_then(|b| image::load_from_memory(&b).ok());
        let backdrop = c
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|img| backdrop_from_cover(&img))
            .or_else(|| pic.as_ref().map(backdrop_from_picture));
        (pic.as_ref().map(|p| round_avatar(p).to_rgba8()), backdrop)
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
        backdrop,
    }
}

/// Fetch a profile (yours, a friend's or a group's) and its pictures.
///
/// What the store remembers of it is shown first (when `show_cached`; the
/// small picture the chat list uses stands in until the full one is there),
/// then it is refreshed from LINE, picture and cover at once; pictures
/// already in the store are not downloaded again. When something is on
/// screen already (`shown`, or the stored copy), the page is only redrawn
/// when something changed.
pub fn spawn_profile(
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    store: Store,
    mid: String,
    me: String,
    show_cached: bool,
    shown: bool,
) {
    tokio::spawn(async move {
        // Store reads run on the blocking pool, not on this task's thread.
        let stored = |key: String| {
            let store = store.clone();
            async move { store.run(move |s| s.image(&key)).await.ok().flatten() }
        };
        let m = mid.clone();
        let cached = store
            .run(move |s| s.user(&m))
            .await
            .ok()
            .flatten()
            .filter(|u| !u.name.is_empty());
        let shown = shown || (show_cached && cached.is_some());
        if show_cached && let Some(u) = &cached {
            let pic = u.picture.as_deref();
            let photo = match pic {
                Some(p) => stored(picture_key(p, PictureSize::Full)).await,
                None => None,
            };
            let small = match (&photo, pic) {
                (None, Some(p)) => stored(picture_key(p, PictureSize::Small)).await,
                _ => None,
            };
            let cover = match u.cover.as_deref() {
                Some(c) => stored(cover_key(c)).await,
                None => None,
            };
            let loaded = decode_profile(
                u.name.clone(),
                u.status.clone(),
                u.members.clone(),
                photo,
                small,
                cover,
            )
            .await;
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
                if !shown {
                    let _ = tx.send(AppEvent::Profile(mid, Err(e)));
                } else {
                    tracing::debug!("profile refresh for {mid} failed: {e}");
                }
                return;
            }
        };
        let path = path.filter(|p| !p.trim().is_empty());
        // The chat list and message headers use the same picture: tell them
        // (even when this page has nothing new to show).
        let _ = tx.send(AppEvent::ProfilePicture(mid.clone(), path.clone()));
        // Picture and cover download side by side; each says whether it was new.
        let picture = async {
            match path.as_deref() {
                Some(p) => match cached_picture(&client, &store, p, PictureSize::Full).await {
                    Some((b, new)) => (Some(b), new),
                    None => (None, false),
                },
                None => (None, false),
            }
        };
        let cover = async {
            match cover_info {
                Ok(Some(c)) => match c.custom_image().map(|o| o.path()) {
                    Some(cp) => {
                        let key = cover_key(&cp);
                        let (bytes, new) = match stored(key.clone()).await {
                            Some(b) => (Some(b), false),
                            None => match client.download_cover(&c).await {
                                Ok(Some(b)) => {
                                    let (_, b) = store.run(move |s| (s.put_image(&key, &b), b)).await;
                                    (Some(b), true)
                                }
                                Ok(None) => (None, false),
                                Err(e) => {
                                    tracing::debug!("cover download failed: {e}");
                                    (None, false)
                                }
                            },
                        };
                        (Some(cp), bytes, new)
                    }
                    None => (None, None, false),
                },
                Ok(None) => (None, None, false),
                Err(()) => {
                    let cp = cached.as_ref().and_then(|u| u.cover.clone());
                    let bytes = match cp.as_deref() {
                        Some(c) => stored(cover_key(c)).await,
                        None => None,
                    };
                    (cp, bytes, false)
                }
            }
        };
        let ((photo, new_photo), (cover_path, cover_photo, new_cover)) = tokio::join!(picture, cover);
        let fresh = new_photo || new_cover;
        // A full picture that would not download: the small one the avatars
        // use (stored, else downloaded).
        let small = match (&photo, path.as_deref()) {
            (None, Some(p)) => cached_picture(&client, &store, p, PictureSize::Small)
                .await
                .map(|(b, _)| b),
            _ => None,
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
            let _ = store.run(move |s| s.upsert_user(&row)).await;
        }
        if unchanged && !fresh && shown {
            return;
        }
        let loaded = decode_profile(name, status, members, photo, small, cover_photo).await;
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

    pub fn fetch_profile(&self, mid: &str, show_cached: bool) {
        spawn_profile(
            self.client.clone(),
            self.tx.clone(),
            self.store.clone(),
            mid.to_owned(),
            self.me.clone(),
            show_cached,
            matches!(self.profiles.get(mid), Some(Load::Ready(_))),
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
                self.loaded(&Job::Profile(mid.clone()));
                let p = *p;
                // Member names come in through the usual name lookup.
                self.want_names(p.members.iter().cloned());
                if !p.name.is_empty() && mid != self.me {
                    self.learn_names(HashMap::from([(mid.clone(), p.name.clone())]));
                }
                Load::Ready(ProfileData {
                    name: p.name,
                    status: p.status,
                    members: p.members,
                    photo: p.photo,
                    cover_photo: p.cover_photo,
                    avatar: p.avatar,
                    backdrop: p.backdrop,
                    scene: None,
                })
            }
            Err(e) => {
                self.retry_later(Job::Profile(mid.clone()));
                Load::Failed(e)
            }
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
                // Writing (synced to disk, compared with an earlier copy) runs
                // off the UI thread and reports on the status line.
                self.status = format!("opening {}…", safe_file_name(&name));
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    let status = match store_for_viewing(&name, &bytes).and_then(|path| {
                        open_external(&path)?;
                        Ok(path)
                    }) {
                        Ok(path) => format!("opened {}", path.display()),
                        Err(e) => format!("could not open: {e}"),
                    };
                    let _ = tx.send(AppEvent::Status(status));
                });
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
        self.profile_cmd(keymap::resolve(keymap::Ctx::Profile, &k));
    }

    fn profile_cmd(&mut self, cmd: Option<keymap::Cmd>) {
        use keymap::Cmd;
        let n = self.profile_members();
        let buttons = self.profile_buttons();
        // A page is as many members as the last frame showed.
        let page = self.hit.profile_rows.len().max(1);
        match cmd {
            Some(Cmd::Back) => self.profile_back(),
            Some(cmd @ (Cmd::Up | Cmd::Down | Cmd::PageUp | Cmd::PageDown | Cmd::Home | Cmd::End)) => {
                self.profile_sel = member_step(self.profile_sel, n, cmd, page);
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
            MouseEventKind::ScrollDown => self.profile_cmd(Some(keymap::Cmd::Down)),
            MouseEventKind::ScrollUp => self.profile_cmd(Some(keymap::Cmd::Up)),
            _ => {}
        }
    }

    pub fn draw_profile(&mut self, f: &mut Frame, area: Rect) {
        self.hit.profile_buttons.clear();
        self.hit.profile_rows.clear();
        let Some(mid) = self.profile.last().cloned() else {
            return;
        };
        let area = crate::layout::sheet(area, 76);
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
            .border_style(Style::new().fg(theme::t().accent))
            .title(kind)
            // Its own keys, also under an overlay opened from it.
            .title_bottom(frame_hint(&keymap::hint(keymap::Ctx::Profile, |_| true), area));
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
                f.render_widget(Paragraph::new("loading profile…").style(theme::muted()), inner);
                return;
            }
        };

        // The backdrop under the whole page, the avatar set into its upper
        // part; both one picture, re-laid out only when the page changes size.
        let picker = self.picker.as_ref();
        let font = picker.map_or(
            FontSize {
                width: 10,
                height: 20,
            },
            Picker::font_size,
        );
        let top_pad = (inner.height / 4).clamp(3, 8).min(inner.height);
        let avatar_max = Size::new(20, (inner.height.saturating_sub(top_pad) / 3).clamp(3, 10));
        let avatar_size = data
            .avatar
            .as_ref()
            .map(|a| picture::fit(a.width(), a.height(), avatar_max, font));
        let top_rows = (top_pad + avatar_size.map_or(0, |s| s.height)).min(inner.height);
        let key = SceneKey {
            page: inner.as_size(),
            top_rows,
            avatar: avatar_size.map_or(Rect::default(), |s| {
                Rect::new(
                    inner.width.saturating_sub(s.width) / 2,
                    top_pad,
                    s.width,
                    s.height,
                )
            }),
            font: (font.width, font.height),
        };
        if data.scene.as_ref().is_none_or(|(k, _)| *k != key) {
            let plain;
            let backdrop = match &data.backdrop {
                Some(b) => b,
                None => {
                    plain = plain_backdrop();
                    &plain
                }
            };
            data.scene = Some((key, build_scene(backdrop, data.avatar.as_ref(), key, picker)));
        }
        if let Some((_, scene)) = data.scene.as_mut() {
            let buf = f.buffer_mut();
            let width = usize::from(inner.width.max(1));
            for (i, c) in scene.cells.iter().enumerate() {
                let (x, y) = (i % width, i / width);
                if let Some(cell) = buf.cell_mut((inner.x + x as u16, inner.y + y as u16)) {
                    cell.set_bg(*c);
                }
            }
            if let Some(a) = scene.avatar.as_mut() {
                let r = key.avatar;
                let at = Rect::new(inner.x + r.x, inner.y + r.y, r.width, r.height);
                a.render(f, at, at.as_size(), 0, cell_mode, FilterType::Lanczos3);
            }
        }
        let mut text = Rect::new(inner.x, inner.y + top_rows, inner.width, inner.height - top_rows);
        if avatar_size.is_none() && text.height > 6 {
            // No picture (yet): a placeholder where it goes.
            let [row, rest] = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(text);
            let [avatar] = Layout::horizontal([Constraint::Length(10)])
                .flex(Flex::Center)
                .areas(row);
            f.render_widget(
                Paragraph::new(vec![Line::raw(""), Line::raw("👤")])
                    .centered()
                    .block(
                        Block::bordered()
                            .border_type(BorderType::Rounded)
                            .border_style(Style::new().fg(accent)),
                    ),
                avatar,
            );
            text = rest;
        }

        // Name, status, buttons.
        let name = if data.name.is_empty() {
            fallback_name
        } else {
            data.name.clone()
        };
        let mut lines = vec![
            Line::raw(""),
            Line::styled(name, Style::new().fg(theme::t().text).bold()).centered(),
        ];
        let tw = usize::from(text.width.saturating_sub(4)).max(10);
        for l in wrap(data.status.trim(), tw).into_iter().take(3) {
            if !l.is_empty() {
                lines.push(Line::styled(l, Style::new().fg(theme::t().muted)).centered());
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
                Style::new().fg(theme::t().accent).bold()
            } else {
                Style::new().fg(theme::t().muted)
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
                Line::styled(
                    format!(" Members · {}", names.len()),
                    Style::new().fg(theme::t().text).bold(),
                ),
            ]),
            title,
        );
        let height = usize::from(rows.height);
        let first = sel.map_or(0, |s| (s + 1).saturating_sub(height));
        for (row, (i, n)) in names.iter().enumerate().skip(first).take(height).enumerate() {
            let y = rows.y + row as u16;
            let style = if sel == Some(i) {
                Style::new().bg(theme::t().accent_soft).fg(theme::t().text).bold()
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

/// The member selected after `cmd` (`None`: the buttons above the list of `n`).
fn member_step(sel: Option<usize>, n: usize, cmd: keymap::Cmd, page: usize) -> Option<usize> {
    use keymap::Cmd;
    let last = n.checked_sub(1)?;
    match (cmd, sel) {
        (Cmd::Home, _) | (Cmd::Up | Cmd::PageUp, None | Some(0)) => None,
        (Cmd::Up, Some(i)) => Some(i - 1),
        (Cmd::PageUp, Some(i)) => Some(i.saturating_sub(page)),
        (Cmd::Down, sel) => Some(sel.map_or(0, |i| (i + 1).min(last))),
        (Cmd::PageDown, sel) => Some(sel.map_or(page - 1, |i| i + page).min(last)),
        (Cmd::End, _) => Some(last),
        (_, sel) => sel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_stepped_through_by_one_by_page_and_to_the_ends() {
        use keymap::Cmd;
        let step = |sel, cmd| member_step(sel, 30, cmd, 10);
        assert_eq!(step(None, Cmd::Down), Some(0));
        assert_eq!(step(Some(0), Cmd::Up), None, "above the first: the buttons");
        assert_eq!(step(None, Cmd::PageDown), Some(9));
        assert_eq!(step(Some(9), Cmd::PageDown), Some(19));
        assert_eq!(step(Some(25), Cmd::PageDown), Some(29));
        assert_eq!(step(Some(15), Cmd::PageUp), Some(5));
        assert_eq!(step(Some(5), Cmd::PageUp), Some(0));
        assert_eq!(step(Some(0), Cmd::PageUp), None);
        assert_eq!(step(Some(12), Cmd::End), Some(29));
        assert_eq!(step(Some(12), Cmd::Home), None);
        assert_eq!(step(Some(29), Cmd::Down), Some(29));
        // No members: always the buttons.
        assert_eq!(member_step(None, 0, Cmd::Down, 10), None);
        assert_eq!(member_step(None, 0, Cmd::End, 10), None);
    }

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
    fn large_covers_are_shrunk_once() {
        let huge = DynamicImage::ImageRgb8(image::RgbImage::new(4000, 3000));
        assert_eq!(backdrop_from_cover(&huge).dimensions(), (MAX_BACKDROP, 900));
        let small = DynamicImage::ImageRgb8(image::RgbImage::new(300, 200));
        assert_eq!(backdrop_from_cover(&small).dimensions(), (300, 200));
    }

    fn key() -> SceneKey {
        SceneKey {
            page: Size::new(40, 30),
            top_rows: 12,
            avatar: Rect::new(15, 6, 10, 5),
            font: (10, 20),
        }
    }

    fn red_avatar() -> RgbaImage {
        round_avatar(&DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([250, 0, 0]),
        )))
        .to_rgba8()
    }

    #[test]
    fn every_cell_gets_a_colour_darkening_downwards() {
        let backdrop = RgbaImage::from_pixel(300, 100, Rgba([200, 200, 200, 255]));
        let scene = build_scene(&backdrop, Some(&red_avatar()), key(), None);
        assert_eq!(scene.cells.len(), 40 * 30);
        let red = |c: Color| match c {
            Color::Rgb(r, _, _) => r,
            _ => panic!("cells are RGB"),
        };
        let top = red(scene.cells[0]);
        let text = red(scene.cells[12 * 40]);
        let bottom = red(scene.cells[40 * 30 - 1]);
        assert!(top > text && text > bottom, "{top} {text} {bottom}");
        assert!(
            matches!(scene.avatar, Some(Picture::Cells(_))),
            "no picker: drawn with cells"
        );
    }

    #[test]
    fn avatar_corners_are_the_cells_colours() {
        let backdrop = RgbaImage::from_pixel(300, 100, Rgba([0, 0, 200, 255]));
        let cells = page_cells(&backdrop, key());
        let tile = avatar_tile(&cells, key(), &red_avatar());
        assert_eq!(tile.dimensions(), (100, 100));
        assert_eq!(tile.get_pixel(50, 50).0, [250, 0, 0, 255]);
        // Top-left corner: exactly the colour of the cell it lies in.
        let [r, g, b] = cells[6 * 40 + 15];
        assert_eq!(tile.get_pixel(1, 1).0, [r, g, b, 255]);
    }

    #[test]
    fn shade_darkens_downwards() {
        assert!(shade(0, 100, 300) > shade(99, 100, 300));
        assert!(shade(99, 100, 300) >= shade(100, 100, 300) - 0.01);
        assert!(shade(100, 100, 300) > shade(299, 100, 300));
    }
}
