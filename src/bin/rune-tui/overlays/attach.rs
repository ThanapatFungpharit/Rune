//! Sending photos, videos, audio and files: a file browser with a preview,
//! then a send dialog. Opened with `Ctrl-o` (or by pasting / dropping a file
//! path into the input line). The same browser, in cover mode, picks a new
//! profile cover from your own profile page.

use crate::*;
use rune::MediaKind;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// The file chosen for sending, with its options.
pub struct Chosen {
    pub path: PathBuf,
    pub size: u64,
    pub kind: MediaKind,
    /// Images: send the original pixels (metadata scrubbed) instead of LINE's ≤1280 px JPEG.
    pub original: bool,
    pub duration_ms: Option<u64>,
}

pub struct Attach {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub filter: TextInput,
    pub sel: usize,
    pub show_hidden: bool,
    pub chosen: Option<Chosen>,
    pub error: Option<String>,
    /// Picking a new profile cover (pictures only) instead of something to send.
    pub cover: bool,
}

/// LINE's file limit (the library refuses bigger uploads).
const MAX_SEND: u64 = 1 << 30;
/// Cover pictures above this are refused before uploading (the extension
/// crops covers in the browser; LINE's own limit is not known).
const MAX_COVER: u64 = 20 * 1024 * 1024;

/// `p` with a leading `~/` (on Windows also `~\`), or a lone `~`, replaced by
/// the home directory.
pub fn expand_home(p: &str) -> PathBuf {
    let rest = p
        .strip_prefix("~/")
        .or_else(|| p.strip_prefix("~\\").filter(|_| cfg!(windows)))
        .or_else(|| (p == "~").then_some(""));
    match rest {
        Some(rest) => rune::dirs::home().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// A path from pasted text: drops quotes, `file://` and shell escapes that
/// terminals add when a file is dragged in.
pub fn pasted_path(text: &str) -> Option<PathBuf> {
    let t = text.trim().trim_matches(|c| c == '\'' || c == '"');
    let t = t.strip_prefix("file://").unwrap_or(t);
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    let t = percent_decode(t).replace("\\ ", " ");
    let p = expand_home(&t);
    p.is_file().then_some(p)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What to send a file as, from its extension.
pub fn guess_kind(path: &Path) -> MediaKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" => MediaKind::Image,
        // LINE plays MP4 / M4A; anything else is sent as a file.
        "mp4" | "m4v" => MediaKind::Video,
        "m4a" => MediaKind::Audio,
        _ => MediaKind::File,
    }
}

pub fn kind_label(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Image => "🖼  photo",
        MediaKind::Video => "🎞  video",
        MediaKind::Audio => "🎤 voice/audio",
        MediaKind::File => "📎 file",
    }
}

/// Length of an MP4/M4A from its `moov/mvhd` box, in milliseconds.
pub fn mp4_duration_ms(data: &[u8]) -> Option<u64> {
    fn boxes(mut d: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
        std::iter::from_fn(move || {
            if d.len() < 8 {
                return None;
            }
            let size32 = u32::from_be_bytes(d[0..4].try_into().ok()?) as u64;
            let kind = &d[4..8];
            let (header, size) = match size32 {
                1 if d.len() >= 16 => (16, u64::from_be_bytes(d[8..16].try_into().ok()?)),
                0 => (8, d.len() as u64),
                n => (8, n),
            };
            let size = usize::try_from(size).ok()?.min(d.len());
            if size < header {
                return None;
            }
            let body = &d[header..size];
            d = &d[size..];
            Some((kind, body))
        })
    }
    let (_, moov) = boxes(data).find(|(k, _)| *k == b"moov")?;
    let (_, mvhd) = boxes(moov).find(|(k, _)| *k == b"mvhd")?;
    let be32 = |o: usize| {
        mvhd.get(o..o + 4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()) as u64)
    };
    let be64 = |o: usize| {
        mvhd.get(o..o + 8)
            .map(|b| u64::from_be_bytes(b.try_into().unwrap()))
    };
    let (scale, dur) = match mvhd.first()? {
        0 => (be32(12)?, be32(16)?),
        1 => (be32(20)?, be64(24)?),
        _ => return None,
    };
    (scale > 0).then(|| dur.saturating_mul(1000) / scale)
}

/// Length of an MP4/M4A file without reading all of it: the top-level boxes
/// are skipped over until `moov`, which is read alone (it may sit at the end
/// of a large video).
pub fn mp4_file_duration_ms(path: &Path) -> Option<u64> {
    /// `moov` holds the sample tables; even hours of video keep it far below this.
    const MAX_MOOV: u64 = 64 * 1024 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        let mut head = [0u8; 16];
        f.seek(SeekFrom::Start(at)).ok()?;
        f.read_exact(&mut head[..8]).ok()?;
        let size32 = u64::from(u32::from_be_bytes(head[0..4].try_into().ok()?));
        let (header, size) = match size32 {
            1 => {
                f.read_exact(&mut head[8..16]).ok()?;
                (16, u64::from_be_bytes(head[8..16].try_into().ok()?))
            }
            0 => (8, len - at),
            n => (8, n),
        };
        if size < header {
            return None;
        }
        if &head[4..8] == b"moov" {
            if size > MAX_MOOV {
                return None;
            }
            let mut boxed = vec![0u8; size as usize];
            f.seek(SeekFrom::Start(at)).ok()?;
            f.read_exact(&mut boxed).ok()?;
            return mp4_duration_ms(&boxed);
        }
        at += size;
    }
    None
}

fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} KB", n as f64 / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

impl Attach {
    pub fn new(dir: PathBuf) -> Self {
        let mut a = Attach {
            dir,
            entries: Vec::new(),
            filter: TextInput::default(),
            sel: 0,
            show_hidden: false,
            chosen: None,
            error: None,
            cover: false,
        };
        a.reload();
        a
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        self.sel = 0;
        self.error = None;
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') && !self.show_hidden {
                        continue;
                    }
                    // Follow symlinks so linked folders open.
                    let meta = std::fs::metadata(e.path()).ok();
                    self.entries.push(Entry {
                        dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                        size: meta.map_or(0, |m| m.len()),
                        name,
                    });
                }
            }
            Err(e) => self.error = Some(format!("cannot list {}: {e}", self.dir.display())),
        }
        self.entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.as_str().to_lowercase();
        (0..self.entries.len())
            .filter(|&i| f.is_empty() || self.entries[i].name.to_lowercase().contains(&f))
            .collect()
    }

    fn selected(&self) -> Option<&Entry> {
        self.visible().get(self.sel).map(|&i| &self.entries[i])
    }

    fn cd(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.filter.clear();
        self.reload();
    }

    fn up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_owned) {
            let from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
            self.cd(parent);
            if let Some(i) = from.and_then(|n| self.visible().iter().position(|&i| self.entries[i].name == n))
            {
                self.sel = i;
            }
        }
    }

    /// Pick `path` for sending (reads its size and, for MP4, its length).
    pub fn choose(&mut self, path: PathBuf) {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_SEND {
            self.error = Some(format!("{} is larger than LINE's 1 GB limit", human_size(size)));
            return;
        }
        let kind = guess_kind(&path);
        if self.cover {
            self.error = if kind != MediaKind::Image {
                Some("a cover has to be a picture (JPEG or PNG)".into())
            } else if size > MAX_COVER {
                Some(format!(
                    "{} is too big for a cover (20 MB at most)",
                    human_size(size)
                ))
            } else {
                None
            };
            if self.error.is_some() {
                return;
            }
        }
        let duration_ms = match kind {
            MediaKind::Video | MediaKind::Audio => mp4_file_duration_ms(&path),
            _ => None,
        };
        self.chosen = Some(Chosen {
            path,
            size,
            kind,
            original: false,
            duration_ms,
        });
    }
}

impl App {
    /// Open the file browser (in the last folder used), or go straight to
    /// the send dialog for `file`.
    pub fn open_attach(&mut self, file: Option<PathBuf>) {
        if self.open.is_none() {
            self.status = "open a chat first".into();
            return;
        }
        let dir = self.browse_dir(file.as_deref().and_then(Path::parent));
        let mut a = Attach::new(dir);
        if let Some(f) = file {
            if let Some(i) = a
                .visible()
                .iter()
                .position(|&i| Some(a.entries[i].name.as_str()) == f.file_name().and_then(|n| n.to_str()))
            {
                a.sel = i;
            }
            a.choose(f);
        }
        self.modal = Some(Modal::Attach(a));
    }

    /// The browser in cover mode: pick a picture to become your profile cover.
    pub fn open_cover_picker(&mut self) {
        let mut a = Attach::new(self.browse_dir(None));
        a.cover = true;
        self.modal = Some(Modal::Attach(a));
    }

    /// Where the browser starts: `hint`, else the last folder used, else home.
    fn browse_dir(&self, hint: Option<&Path>) -> PathBuf {
        hint.map(Path::to_owned)
            .or_else(|| {
                self.store
                    .get("setting.attach_dir")
                    .ok()
                    .flatten()
                    .map(PathBuf::from)
            })
            .filter(|d| d.is_dir())
            .or_else(rune::dirs::home)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn attach_paste(&mut self, mut a: Attach, text: &str) {
        match pasted_path(text) {
            Some(p) => {
                a.cd(p.parent().map(Path::to_owned).unwrap_or_default());
                a.choose(p);
            }
            None => {
                a.filter.insert_str(text.trim());
                a.sel = 0;
            }
        }
        self.keep_attach(a);
    }

    /// Put the browser back.
    fn keep_attach(&mut self, a: Attach) {
        self.modal = Some(Modal::Attach(a));
    }

    /// `cmd` is what `k` means in the dialog's context now (send dialog,
    /// browser or filter: see `App::key_context`).
    pub fn attach_key(&mut self, mut a: Attach, cmd: Option<keymap::Cmd>, k: KeyEvent) {
        if let Some(c) = a.chosen.as_mut() {
            match cmd {
                Some(keymap::Cmd::Back) => a.chosen = None,
                Some(keymap::Cmd::Confirm) if a.cover => {
                    let _ = self.store.set("setting.attach_dir", &a.dir.to_string_lossy());
                    return self.run_action(Action::ConfirmCover(c.path.clone()));
                }
                Some(keymap::Cmd::Confirm) => return self.send_chosen(a),
                // A cover is a picture, sent as it is.
                Some(keymap::Cmd::AsFile | keymap::Cmd::OriginalQuality) if a.cover => {}
                // Send as a plain file instead (or back).
                Some(keymap::Cmd::AsFile) => {
                    c.kind = if c.kind == MediaKind::File {
                        guess_kind(&c.path)
                    } else {
                        MediaKind::File
                    };
                }
                Some(keymap::Cmd::OriginalQuality) if c.kind == MediaKind::Image => c.original = !c.original,
                _ => {}
            }
            self.modal = Some(Modal::Attach(a));
            return;
        }
        let vis = a.visible();
        // With a filter typed, the arrows and Home/End edit it (`AttachFilter`).
        match a.filter.edit(cmd, &k) {
            Edit::Changed => {
                a.sel = 0;
                return self.keep_attach(a);
            }
            Edit::Moved | Edit::MovedHitLeft | Edit::MovedHitRight => return self.keep_attach(a),
            Edit::Ignored => {}
        }
        match cmd {
            Some(keymap::Cmd::Back) => return,
            Some(keymap::Cmd::Up) => a.sel = a.sel.saturating_sub(1),
            Some(keymap::Cmd::Down) => a.sel = (a.sel + 1).min(vis.len().saturating_sub(1)),
            Some(keymap::Cmd::PageUp) => a.sel = a.sel.saturating_sub(10),
            Some(keymap::Cmd::PageDown) => a.sel = (a.sel + 10).min(vis.len().saturating_sub(1)),
            Some(keymap::Cmd::Home) => a.sel = 0,
            Some(keymap::Cmd::End) => a.sel = vis.len().saturating_sub(1),
            Some(keymap::Cmd::ParentDir) => a.up(),
            Some(keymap::Cmd::Confirm | keymap::Cmd::Right) => {
                // A typed path ("~/Pictures", "/tmp/a.png") goes there.
                let typed = a.filter.as_str();
                let typed = (typed.starts_with('/') || typed.starts_with('~')).then(|| expand_home(typed));
                if let Some(p) = typed {
                    if p.is_dir() {
                        a.cd(p);
                    } else if p.is_file() {
                        a.choose(p);
                    } else {
                        a.error = Some(format!("no such file or folder: {}", p.display()));
                    }
                } else if let Some(e) = a.selected() {
                    let p = a.dir.join(&e.name);
                    if e.dir {
                        a.cd(p);
                    } else if cmd == Some(keymap::Cmd::Confirm) {
                        a.choose(p);
                    }
                }
            }
            Some(keymap::Cmd::ShowHidden) => {
                a.show_hidden = !a.show_hidden;
                a.reload();
            }
            Some(keymap::Cmd::HomeDir) => a.cd(expand_home("~")),
            _ => {}
        }
        self.keep_attach(a);
    }

    fn send_chosen(&mut self, a: Attach) {
        let Some(c) = a.chosen else { return };
        let Some(chat) = self.open.clone() else { return };
        let _ = self.store.set("setting.attach_dir", &a.dir.to_string_lossy());
        let name = c
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();
        let label = format!("{} {name} ({})", kind_label(c.kind), human_size(c.size));
        self.uploads.push((chat.clone(), label.clone()));
        self.status = format!("uploading {name}…");
        let (client, tx, me) = (self.client.clone(), self.tx.clone(), self.me.clone());
        tokio::spawn(async move {
            let res = async {
                let data = tokio::fs::read(&c.path)
                    .await
                    .map_err(|e| format!("cannot read {}: {e}", c.path.display()))?;
                let up = match c.kind {
                    MediaKind::Image => MediaUpload {
                        original: c.original,
                        ..MediaUpload::image(data, name.clone())
                    },
                    MediaKind::Video => MediaUpload::video(data, name.clone(), c.duration_ms.unwrap_or(0)),
                    MediaKind::Audio => MediaUpload::audio(data, name.clone(), c.duration_ms.unwrap_or(0)),
                    MediaKind::File => MediaUpload::file(data, name.clone()),
                };
                let m = client.send_media(&chat, up).await.map_err(|e| e.to_string())?;
                Ok::<_, String>(client.decrypt_message(&m).await.unwrap_or(m))
            }
            .await;
            match res {
                Ok(m) => {
                    if let Some(mut ui) = to_ui(&m, None) {
                        ui.from = me;
                        let _ = tx.send(AppEvent::Sent(chat.clone(), Box::new(ui), Some(Box::new(m))));
                    }
                    let _ = tx.send(AppEvent::UploadDone(chat, label, None));
                }
                Err(e) => drop(tx.send(AppEvent::UploadDone(chat, label, Some(e)))),
            }
        });
    }

    pub fn draw_attach(&mut self, f: &mut Frame, area: Rect, a: &mut Attach, hint: String) {
        let r = modal::popup(area, 110, area.height.saturating_sub(2).max(12));
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let title = match &self.open {
            _ if a.cover => " 🏞 New profile cover ".into(),
            Some(chat) => format!(" 📎 Send to {} ", self.name(chat)),
            None => " 📎 Send ".into(),
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(title, Style::new().bold()))
            .title_bottom(frame_hint(&hint, r));
        let inner = block.inner(r);
        f.render_widget(block, r);
        // Side by side when wide enough, else the preview (and send dialog) under the list.
        let [list_area, preview_area] = if inner.width < crate::layout::SIDE_BY_SIDE_MIN_W {
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner)
        } else {
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(inner)
        };
        let [path_row, filter_row, rows] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
                .areas(list_area);
        f.render_widget(
            Paragraph::new(ellipsize(
                &format!("📁 {}", a.dir.display()),
                path_row.width as usize,
            ))
            .style(Style::new().bold()),
            path_row,
        );
        let (shown, cursor) = a.filter.view(filter_row.width.saturating_sub(3) as usize);
        let filter_line = if a.filter.is_empty() {
            Line::from(Span::styled("🔍 type a name or a path…", theme::muted()))
        } else {
            Line::from(vec![Span::raw("🔍 "), Span::raw(shown.to_owned())])
        };
        f.render_widget(
            Paragraph::new(filter_line).style(Style::new().bg(theme::t().surface)),
            filter_row,
        );
        if a.chosen.is_none() {
            f.set_cursor_position(Position::new(filter_row.x + 3 + cursor, filter_row.y));
        }
        let vis = a.visible();
        let h = rows.height as usize;
        let first = a.sel.saturating_sub(h.saturating_sub(1));
        for (row, &i) in vis.iter().skip(first).take(h).enumerate() {
            let e = &a.entries[i];
            let y = rows.y + row as u16;
            self.hit.modal_rows.push((y, first + row));
            let icon = if e.dir {
                "📁"
            } else {
                match guess_kind(Path::new(&e.name)) {
                    MediaKind::Image => "🖼 ",
                    MediaKind::Video => "🎞 ",
                    MediaKind::Audio => "🎤",
                    MediaKind::File => "📄",
                }
            };
            let size = if e.dir { String::new() } else { human_size(e.size) };
            let name_w = (rows.width as usize).saturating_sub(size.width() + 4);
            let name = ellipsize(&format!("{}{}", e.name, if e.dir { "/" } else { "" }), name_w);
            let pad = (rows.width as usize).saturating_sub(name.width() + size.width() + 3);
            let style = if first + row == a.sel {
                Style::new().bg(theme::t().accent_soft).fg(theme::t().text).bold()
            } else if e.dir {
                Style::new().fg(theme::t().link)
            } else {
                Style::new()
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(format!("{icon} ")),
                    Span::styled(format!("{name}{}", " ".repeat(pad)), style),
                    Span::styled(size, theme::muted()),
                ])),
                Rect::new(rows.x, y, rows.width, 1),
            );
        }
        if vis.is_empty() {
            let msg = a.error.clone().unwrap_or_else(|| "nothing here".into());
            f.render_widget(
                Paragraph::new(msg)
                    .style(theme::muted())
                    .wrap(Wrap { trim: true }),
                rows,
            );
        }

        // Right: preview of the highlighted file, or the send dialog.
        let preview_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::muted());
        let pv = preview_block.inner(preview_area);
        f.render_widget(preview_block, preview_area);
        let target = match &a.chosen {
            Some(c) => Some(c.path.clone()),
            None => a.selected().filter(|e| !e.dir).map(|e| a.dir.join(&e.name)),
        };
        let mut text_h = 0u16;
        if let Some(c) = &a.chosen {
            let mut lines = vec![
                Line::from(Span::styled(
                    c.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    Style::new().bold(),
                )),
                Line::from(format!("{} · {}", kind_label(c.kind), human_size(c.size))),
            ];
            match c.kind {
                _ if a.cover => {
                    lines.push(Line::from(
                        "Replaces your profile cover; everyone who can see your profile sees it.",
                    ));
                    lines.push(Line::from(Span::styled(
                        "Location & personal metadata are removed first.",
                        theme::muted(),
                    )));
                }
                MediaKind::Image => lines.push(Line::from(if c.original {
                    "Original quality (location & personal metadata removed)"
                } else {
                    "Standard quality (LINE's ≤1280 px JPEG) — o for original"
                })),
                MediaKind::Video | MediaKind::Audio => lines.push(Line::from(match c.duration_ms {
                    Some(ms) => format!("length {}", rune::models::format_call_duration(ms)),
                    None => "length unknown (not an MP4/M4A file?)".into(),
                })),
                MediaKind::File => {}
            }
            if c.kind == MediaKind::Video {
                lines.push(Line::from(Span::styled(
                    "Sent without a preview frame (rune has no video decoder).",
                    theme::muted(),
                )));
            }
            if self.replying.is_some() && !a.cover {
                lines.push(Line::from(Span::styled(
                    "Media is sent without the reply quote (the reply stays pending for your next text).",
                    theme::muted(),
                )));
            }
            lines.push(Line::raw(""));
            lines.push(if a.cover {
                Line::from(Span::styled(
                    " Set as cover (Enter) ",
                    Style::new().bg(theme::t().accent).fg(theme::t().on_accent).bold(),
                ))
            } else {
                Line::from(vec![
                    Span::styled(
                        " Send (Enter) ",
                        Style::new().bg(theme::t().accent).fg(theme::t().on_accent).bold(),
                    ),
                    Span::raw("  "),
                    Span::styled(" as file (f) ", Style::new().bg(theme::t().surface_hi)),
                ])
            });
            text_h = lines.len() as u16 + 1;
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), pv);
        } else if let Some(e) = &a.error {
            f.render_widget(
                Paragraph::new(e.as_str()).style(Style::new().fg(theme::t().danger)),
                pv,
            );
            text_h = 2;
        }
        let pic_area = Rect::new(pv.x, pv.y + text_h, pv.width, pv.height.saturating_sub(text_h));
        if let Some(p) = target.filter(|p| guess_kind(p) == MediaKind::Image) {
            let key = p.to_string_lossy().into_owned();
            match self.file_previews.get_mut(&key) {
                Some(Load::Ready(pic)) => {
                    let size = pic.size_for(pic_area.as_size());
                    let [h] = Layout::horizontal([Constraint::Length(size.width)])
                        .flex(Flex::Center)
                        .areas(pic_area);
                    let [v] = Layout::vertical([Constraint::Length(size.height)])
                        .flex(Flex::Center)
                        .areas(h);
                    pic.render(f, v, size, 0, self.cell_mode, VIEW_FILTER);
                }
                Some(Load::Failed(e)) => {
                    f.render_widget(
                        Paragraph::new(format!("no preview: {e}")).style(theme::muted()),
                        pic_area,
                    );
                }
                Some(Load::Loading) => {
                    f.render_widget(Paragraph::new("loading preview…").style(theme::muted()), pic_area);
                }
                None => {
                    self.file_previews.insert(key.clone(), Load::Loading);
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let res = match tokio::fs::read(&key).await {
                            Ok(b) => decode_picture(b).await,
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(AppEvent::FilePreview(key, res));
                    });
                }
            }
        } else if a.chosen.is_none() && a.error.is_none() {
            f.render_widget(
                Paragraph::new(if a.cover {
                    "Pick a picture (JPEG or PNG) for your profile cover. It is previewed here; LINE shows the middle of it as a wide band."
                } else {
                    "Pick a file. Photos are previewed here.\n\nTip: you can also drag a file onto the terminal or paste its path into the message line."
                })
                    .style(theme::muted())
                    .wrap(Wrap { trim: true }),
                pv,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_pasted_paths() {
        assert_eq!(guess_kind(Path::new("a/B.JPG")), MediaKind::Image);
        assert_eq!(guess_kind(Path::new("clip.mp4")), MediaKind::Video);
        assert_eq!(guess_kind(Path::new("memo.m4a")), MediaKind::Audio);
        assert_eq!(guess_kind(Path::new("song.mp3")), MediaKind::File);
        assert_eq!(guess_kind(Path::new("README")), MediaKind::File);
        let dir = std::env::temp_dir().join(format!("rune attach {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("my pic.png");
        std::fs::write(&file, b"x").unwrap();
        let s = file.to_string_lossy().to_string();
        assert_eq!(pasted_path(&format!("'{s}'\n")), Some(file.clone()));
        assert_eq!(pasted_path(&s.replace(' ', "\\ ")), Some(file.clone()));
        assert_eq!(
            pasted_path(&format!("file://{}", s.replace(' ', "%20"))),
            Some(file.clone())
        );
        assert_eq!(pasted_path("hello there"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mp4_duration_from_mvhd() {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd[16..20].copy_from_slice(&12_345u32.to_be_bytes()); // duration
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        };
        let mut file = boxed(b"ftyp", b"isom0000");
        file.extend(boxed(b"moov", &boxed(b"mvhd", &mvhd)));
        assert_eq!(mp4_duration_ms(&file), Some(12_345));
        assert_eq!(mp4_duration_ms(b"not an mp4 at all"), None);
    }

    #[test]
    fn mp4_duration_from_a_file_with_moov_at_the_end() {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&1_800u32.to_be_bytes());
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        };
        let mut file = boxed(b"ftyp", b"isom0000");
        file.extend(boxed(b"mdat", &vec![7u8; 100_000]));
        file.extend(boxed(b"moov", &boxed(b"mvhd", &mvhd)));
        let path = std::env::temp_dir().join(format!("rune-mp4-{}.mp4", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        assert_eq!(mp4_file_duration_ms(&path), Some(3_000));
        std::fs::write(&path, b"not an mp4").unwrap();
        assert_eq!(mp4_file_duration_ms(&path), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn browser_lists_dirs_first_and_filters() {
        let dir = std::env::temp_dir().join(format!("rune-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zdir")).unwrap();
        std::fs::write(dir.join("a.txt"), b"1").unwrap();
        std::fs::write(dir.join(".hidden"), b"1").unwrap();
        let mut a = Attach::new(dir.clone());
        let names: Vec<&str> = a.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "a.txt"]);
        a.filter.set("TXT");
        assert_eq!(a.visible(), [1]);
        a.show_hidden = true;
        a.reload();
        assert_eq!(a.entries.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
