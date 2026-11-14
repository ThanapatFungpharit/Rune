//! Importing chat history exported from the official LINE apps.
//!
//! LINE's phone apps can save a chat as a text file (chat → ☰ → *Other
//! settings* → *Export chat history*). That file is the only way to get
//! messages the server no longer serves, so `rune` can read it into the
//! local [`Store`](crate::Store) and show it above the server's history.
//!
//! The file looks like this (the wording and the date format follow the
//! phone's language; the layout is the same everywhere; `⇥` is a tab):
//!
//! ```text
//! [LINE] Chat history with Alice
//! Saved on: 2024/05/02, 21:07
//!
//! Mon, 1/15/2024
//! 09:12⇥Alice⇥Good morning
//! 09:13⇥Bob⇥"Two lines,
//! the second one"
//! 09:14⇥Alice⇥[Sticker]
//! 09:20⇥⇥Alice unsent a message.
//! ```
//!
//! - each day starts with a date line (year first like `2024/01/15(Mon)`,
//!   or last like `Mon, 15/01/2024`; Buddhist-era years are converted);
//! - a message is `time⇥sender⇥text`, the time in 24-hour or 12-hour form
//!   (`AM`/`PM`, `午前`/`午後`, `上午`/`下午`, `오전`/`오후`);
//! - text with line breaks is wrapped in `"…"` with `""` for a quote;
//! - photos, stickers and the like are only placeholders such as `[Photo]`;
//! - events (joins, unsends…) have an empty sender.
//!
//! The file has no message ids, user mids or seconds, and its times are in
//! the exporting phone's time zone. Imported messages therefore get
//! synthetic **negative** ids ([`is_imported_id`]): they sort before every
//! real message, never reach the server, and importing the same file again
//! is a no-op. Senders are matched to mids by name by the caller;
//! unmatched ones become [`imported_sender_mid`] pseudo-mids. The store
//! drops an imported message once the real one is fetched
//! ([`fingerprint`] decides what counts as the same message).

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use sha2::{Digest, Sha256};

use crate::models::{Message, MidType, content_type as ct};

/// `contentMetadata` key set on every imported message.
pub const META_IMPORTED: &str = "RUNE_IMPORTED";
/// `contentMetadata` key with the sender's name as the file gave it.
pub const META_SENDER: &str = "RUNE_IMPORT_SENDER";
/// `contentMetadata` key with the [`ExportedKind`] label (absent for text).
pub const META_KIND: &str = "RUNE_IMPORT_KIND";

/// Prefix of the pseudo-mid of a sender the import could not match to a user.
pub const IMPORTED_SENDER_PREFIX: &str = "import:";

/// Is this the id of an imported message (only stored locally)?
pub fn is_imported_id(id: &str) -> bool {
    id.starts_with('-')
}

/// The pseudo-mid for a sender known only by `name` (empty for events).
pub fn imported_sender_mid(name: &str) -> String {
    format!("{IMPORTED_SENDER_PREFIX}{name}")
}

/// The name in a pseudo-mid made by [`imported_sender_mid`].
pub fn imported_sender_name(mid: &str) -> Option<&str> {
    mid.strip_prefix(IMPORTED_SENDER_PREFIX)
}

/// A calendar date and minute as the file shows it (the exporting phone's local time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl LocalTime {
    /// Milliseconds since the epoch, for a zone `offset_minutes` east of UTC.
    pub fn utc_ms(&self, offset_minutes: i32) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        let minutes =
            days * 1440 + i64::from(self.hour) * 60 + i64::from(self.minute) - i64::from(offset_minutes);
        minutes * 60_000
    }
}

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// What a line stood for, from its placeholder text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportedKind {
    Text,
    Sticker,
    Photo,
    Video,
    Audio,
    File,
    Album,
    Contact,
    Location,
    /// An event without a sender (joins, leaves, unsends, call notices…).
    Event,
}

impl ExportedKind {
    pub fn label(self) -> &'static str {
        match self {
            ExportedKind::Text => "text",
            ExportedKind::Sticker => "sticker",
            ExportedKind::Photo => "photo",
            ExportedKind::Video => "video",
            ExportedKind::Audio => "audio",
            ExportedKind::File => "file",
            ExportedKind::Album => "album",
            ExportedKind::Contact => "contact",
            ExportedKind::Location => "location",
            ExportedKind::Event => "event",
        }
    }

    /// The kind of a real message, as far as an export could tell it apart.
    fn of_content_type(t: i64) -> Option<ExportedKind> {
        Some(match t {
            ct::TEXT => ExportedKind::Text,
            ct::STICKER => ExportedKind::Sticker,
            ct::IMAGE => ExportedKind::Photo,
            ct::VIDEO => ExportedKind::Video,
            ct::AUDIO => ExportedKind::Audio,
            ct::FILE => ExportedKind::File,
            ct::CONTACT => ExportedKind::Contact,
            ct::LOCATION => ExportedKind::Location,
            _ => return None,
        })
    }

    /// Placeholders of the English, Thai, Japanese, Chinese and Korean apps.
    fn of_placeholder(text: &str) -> ExportedKind {
        let t = text.trim();
        let Some(inner) = t.strip_prefix('[').and_then(|t| t.strip_suffix(']')) else {
            return ExportedKind::Text;
        };
        match inner.trim() {
            "Sticker" | "สติกเกอร์" | "スタンプ" | "貼圖" | "贴图" | "이모티콘" => {
                ExportedKind::Sticker
            }
            "Photo" | "Photos" | "รูป" | "รูปภาพ" | "写真" | "照片" | "相片" | "사진" => {
                ExportedKind::Photo
            }
            "Video" | "Videos" | "วิดีโอ" | "動画" | "影片" | "视频" | "동영상" => {
                ExportedKind::Video
            }
            "Voice message"
            | "Audio"
            | "ข้อความเสียง"
            | "ボイスメッセージ"
            | "語音訊息"
            | "语音信息"
            | "음성메시지" => ExportedKind::Audio,
            "File" | "ไฟล์" | "ファイル" | "檔案" | "文件" | "파일" => ExportedKind::File,
            "Album" | "Albums" | "อัลบั้ม" | "アルバム" | "相簿" | "相册" | "앨범" => {
                ExportedKind::Album
            }
            "Contact" | "Contacts" | "ผู้ติดต่อ" | "連絡先" | "聯絡資訊" | "联系人" | "연락처" => {
                ExportedKind::Contact
            }
            "Location" | "ตำแหน่งที่ตั้ง" | "位置情報" | "位置資訊" | "位置信息" | "위치정보" => {
                ExportedKind::Location
            }
            _ => ExportedKind::Text,
        }
    }
}

/// One message (or event) of an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedMessage {
    pub time: LocalTime,
    /// The sender's name as the exporting phone showed it; `None` for events.
    pub sender: Option<String>,
    pub text: String,
    pub kind: ExportedKind,
}

/// A parsed export.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatExport {
    /// The chat's name from the `[LINE] …` header, when there is one.
    pub title: Option<String>,
    /// Oldest first, in file order.
    pub messages: Vec<ExportedMessage>,
}

impl ChatExport {
    /// Distinct sender names, most messages first.
    pub fn senders(&self) -> Vec<(String, usize)> {
        let mut n: HashMap<&str, usize> = HashMap::new();
        for m in &self.messages {
            if let Some(s) = &m.sender {
                *n.entry(s).or_default() += 1;
            }
        }
        let mut v: Vec<(String, usize)> = n.into_iter().map(|(s, c)| (s.to_owned(), c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// The first and last message times.
    pub fn span(&self) -> Option<(LocalTime, LocalTime)> {
        Some((self.messages.first()?.time, self.messages.last()?.time))
    }

    /// Turn the export into messages of `chat` for [`Store::import_messages`].
    ///
    /// `sender_mid` maps a sender name to a mid (unmatched names become
    /// [`imported_sender_mid`] pseudo-mids); `to_utc_ms` converts the
    /// exporting phone's local time (e.g. `|t| t.utc_ms(420)` for UTC+7).
    /// Ids are derived from the content, so the same file gives the same ids.
    ///
    /// [`Store::import_messages`]: crate::Store::import_messages
    pub fn to_messages(
        &self,
        chat: &str,
        sender_mid: impl Fn(&str) -> Option<String>,
        to_utc_ms: impl Fn(&LocalTime) -> i64,
    ) -> Vec<Message> {
        let mut per_minute: HashMap<i64, i64> = HashMap::new();
        let mut seen: HashMap<(i64, &str, &str), u32> = HashMap::new();
        let mut out = Vec::with_capacity(self.messages.len());
        for m in &self.messages {
            let minute = to_utc_ms(&m.time);
            // Keep the file's order inside a minute: the store sorts by time.
            let k = per_minute.entry(minute).or_default();
            let time = minute + (*k).min(59_999);
            *k += 1;
            let name = m.sender.as_deref().unwrap_or("");
            let nth = seen.entry((minute, name, &m.text)).or_default();
            let id = synthetic_id(chat, minute, name, &m.text, *nth);
            *nth += 1;
            let from = m
                .sender
                .as_deref()
                .and_then(&sender_mid)
                .unwrap_or_else(|| imported_sender_mid(name));
            let mut meta = BTreeMap::new();
            meta.insert(META_IMPORTED.to_owned(), "1".to_owned());
            if let Some(s) = &m.sender {
                meta.insert(META_SENDER.to_owned(), s.clone());
            }
            if m.kind != ExportedKind::Text {
                meta.insert(META_KIND.to_owned(), m.kind.label().to_owned());
            }
            out.push(Message {
                id,
                from,
                to: chat.to_owned(),
                to_type: MidType::of(chat) as i64,
                text: Some(m.text.clone()),
                content_type: ct::TEXT,
                content_metadata: meta,
                created_time: Some(time),
                ..Message::default()
            });
        }
        out
    }
}

/// A negative decimal id, unique per (chat, minute, sender, text, repeat).
fn synthetic_id(chat: &str, minute: i64, sender: &str, text: &str, nth: u32) -> String {
    let mut h = Sha256::new();
    for part in [
        chat.as_bytes(),
        &minute.to_be_bytes(),
        sender.as_bytes(),
        text.as_bytes(),
        &nth.to_be_bytes(),
    ] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part);
    }
    let d = h.finalize();
    let n = u64::from_be_bytes(d[..8].try_into().expect("8 bytes")) >> 2;
    format!("-{}", n + 1)
}

/// What decides that an imported message and a real one are the same:
/// `(kind, normalized text)` (text only for text messages). `None` when a
/// message cannot be compared (e.g. still sealed, or an event).
pub fn fingerprint(m: &Message) -> Option<(ExportedKind, String)> {
    if m.content_metadata.contains_key(META_IMPORTED) {
        let kind = match m.content_metadata.get(META_KIND).map(String::as_str) {
            None => ExportedKind::Text,
            Some(k) => [
                ExportedKind::Sticker,
                ExportedKind::Photo,
                ExportedKind::Video,
                ExportedKind::Audio,
                ExportedKind::File,
                ExportedKind::Album,
                ExportedKind::Contact,
                ExportedKind::Location,
            ]
            .into_iter()
            .find(|x| x.label() == k)?,
        };
        let text = if kind == ExportedKind::Text {
            normalize(m.text.as_deref()?)
        } else {
            String::new()
        };
        return Some((kind, text));
    }
    let kind = ExportedKind::of_content_type(m.content_type)?;
    if kind == ExportedKind::Text {
        let t = m.text.as_deref().filter(|t| !t.trim().is_empty())?;
        Some((kind, normalize(t)))
    } else {
        Some((kind, String::new()))
    }
}

fn normalize(t: &str) -> String {
    t.replace("\r\n", "\n").trim().to_owned()
}

/// Why a file could not be read as an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parse an exported chat (the `.txt` file's contents).
pub fn parse_chat_export(text: &str) -> Result<ChatExport, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let order = date_order(&lines);
    let mut out = ChatExport::default();
    if let Some(first) = lines.first().map(|l| l.trim())
        && let Some(rest) = first.strip_prefix("[LINE]")
    {
        out.title = Some(chat_title(rest.trim())).filter(|t| !t.is_empty());
    }

    let mut date: Option<(i32, u32, u32)> = None;
    let mut pending_blank = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() {
            pending_blank += 1;
            continue;
        }
        if !line.contains('\t')
            && let Some(d) = date_line(line, order)
            && date.is_none_or(|prev| d >= prev)
        {
            date = Some(d);
            pending_blank = 0;
            continue;
        }
        let Some((y, mo, d)) = date else {
            // The header ("[LINE] …", "Saved on: …").
            continue;
        };
        if let Some((hour, minute, rest)) = message_line(line) {
            let (sender, body) = match rest.split_once('\t') {
                Some((s, b)) => (Some(s.trim()).filter(|s| !s.is_empty()), b),
                None => (None, rest),
            };
            let body = match unquote(body, &lines[i..]) {
                Some((t, used)) => {
                    i += used;
                    t
                }
                None => body.to_owned(),
            };
            let kind = match sender {
                None => ExportedKind::Event,
                Some(_) => ExportedKind::of_placeholder(&body),
            };
            out.messages.push(ExportedMessage {
                time: LocalTime {
                    year: y,
                    month: mo,
                    day: d,
                    hour,
                    minute,
                },
                sender: sender.map(str::to_owned),
                text: body,
                kind,
            });
            pending_blank = 0;
        } else if let Some(last) = out.messages.last_mut() {
            // An unquoted line break (older exports).
            for _ in 0..pending_blank {
                last.text.push('\n');
            }
            last.text.push('\n');
            last.text.push_str(line);
            pending_blank = 0;
        }
    }
    if out.messages.is_empty() {
        return Err(ParseError(
            "no messages found: expected a chat exported from the LINE app \
             (lines like \"09:12<Tab>Name<Tab>Text\" under date lines)"
                .into(),
        ));
    }
    Ok(out)
}

/// The chat's name from the header after `[LINE]`.
fn chat_title(s: &str) -> String {
    const PREFIXES: &[&str] = &[
        "Chat history with ",
        "Chat history in ",
        "Chat history of ",
        "ประวัติการแชทกับ",
        "ประวัติการแชทใน",
        "ประวัติแชทกับ",
        "與",
        "与",
    ];
    const SUFFIXES: &[&str] = &[
        "とのトーク履歴",
        "のトーク履歴",
        "的聊天記錄",
        "的聊天记录",
        "의 대화",
        "님과의 대화",
        "와의 대화",
        "과의 대화",
        "'s chat history",
    ];
    let mut t = s.trim();
    for p in PREFIXES {
        if let Some(r) = t.strip_prefix(p) {
            t = r;
            break;
        }
    }
    for x in SUFFIXES {
        if let Some(r) = t.strip_suffix(x) {
            t = r;
            break;
        }
    }
    t.trim().to_owned()
}

/// Day/month order of dates written year-last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateOrder {
    DayMonth,
    MonthDay,
}

/// Three numbers of a date, with the year's position: `(a, b, c)`.
fn date_numbers(line: &str) -> Option<(i64, i64, i64)> {
    // Too long for a date line; also skips "Saved on: …" which has a time.
    if line.chars().count() > 48 || line.contains(':') {
        return None;
    }
    let mut nums: Vec<(i64, usize)> = Vec::new();
    let mut cur = String::new();
    let mut seps = 0usize;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            cur.push(c);
            if !chars.peek().is_some_and(char::is_ascii_digit) {
                nums.push((cur.parse().ok()?, cur.len()));
                cur.clear();
                if nums.len() == 3 {
                    break;
                }
                // Digits must be joined by a separator to be one date.
                let sep = chars.peek().copied();
                if matches!(sep, Some('/' | '.' | '-' | '年' | '月' | ' ')) {
                    seps += 1;
                    chars.next();
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }
                    if !chars.peek().is_some_and(char::is_ascii_digit) {
                        nums.clear();
                    }
                } else {
                    nums.clear();
                }
            }
        }
    }
    if nums.len() != 3 || seps < 2 {
        return None;
    }
    let year_first = nums[0].1 == 4;
    let year_last = nums[2].1 == 4 || (nums[2].1 == 2 && nums[0].1 <= 2 && nums[1].1 <= 2);
    if !year_first && !year_last {
        return None;
    }
    // Year-last dates are marked by a negative first number.
    let (a, b, c) = (nums[0].0, nums[1].0, nums[2].0);
    Some(if year_first { (a, b, c) } else { (-a, b, c) })
}

fn fix_year(y: i64) -> i64 {
    match y {
        0..=99 => 2000 + y,
        // Thai Buddhist era.
        2400.. => y - 543,
        _ => y,
    }
}

fn valid(y: i64, m: i64, d: i64) -> Option<(i32, u32, u32)> {
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => return None,
    };
    ((1..=dim).contains(&d) && (1900..=2200).contains(&y)).then_some((y as i32, m as u32, d as u32))
}

fn date_line(line: &str, order: DateOrder) -> Option<(i32, u32, u32)> {
    let (a, b, c) = date_numbers(line)?;
    if a >= 0 {
        return valid(fix_year(a), b, c);
    }
    let (first, second, y) = (-a, b, fix_year(c));
    match order {
        DateOrder::DayMonth => valid(y, second, first),
        DateOrder::MonthDay => valid(y, first, second),
    }
}

/// Weekday named in a date line (0 = Monday).
fn weekday_of_line(line: &str) -> Option<u32> {
    const NAMES: &[(&str, u32)] = &[
        ("Mon", 0),
        ("Tue", 1),
        ("Wed", 2),
        ("Thu", 3),
        ("Fri", 4),
        ("Sat", 5),
        ("Sun", 6),
        ("อา.", 6),
        ("พฤ.", 3),
        ("จ.", 0),
        ("อ.", 1),
        ("พ.", 2),
        ("ศ.", 4),
        ("ส.", 5),
    ];
    NAMES.iter().find(|(n, _)| line.contains(n)).map(|&(_, d)| d)
}

fn weekday(y: i32, m: u32, d: u32) -> u32 {
    // 1970-01-01 was a Thursday.
    (days_from_civil(y, m, d) + 3).rem_euclid(7) as u32
}

/// How the file writes year-last dates: a day above 12 or the weekday names
/// settle it; otherwise English files are taken as month-first.
fn date_order(lines: &[&str]) -> DateOrder {
    let (mut dm, mut md) = (0usize, 0usize);
    let mut english = false;
    for l in lines.iter().filter(|l| !l.contains('\t')) {
        let Some((a, b, c)) = date_numbers(l) else {
            continue;
        };
        if a >= 0 {
            continue;
        }
        let (first, second, y) = (-a, b, fix_year(c));
        english |= l.chars().any(|c| c.is_ascii_alphabetic());
        let as_dm = valid(y, second, first);
        let as_md = valid(y, first, second);
        match (as_dm, as_md) {
            (Some(_), None) => dm += 10,
            (None, Some(_)) => md += 10,
            (Some(x), Some(z)) => {
                if let Some(w) = weekday_of_line(l) {
                    dm += usize::from(weekday(x.0, x.1, x.2) == w);
                    md += usize::from(weekday(z.0, z.1, z.2) == w);
                }
            }
            (None, None) => {}
        }
    }
    match dm.cmp(&md) {
        std::cmp::Ordering::Greater => DateOrder::DayMonth,
        std::cmp::Ordering::Less => DateOrder::MonthDay,
        std::cmp::Ordering::Equal if english => DateOrder::MonthDay,
        std::cmp::Ordering::Equal => DateOrder::DayMonth,
    }
}

/// `time⇥rest` → (hour, minute, rest).
fn message_line(line: &str) -> Option<(u32, u32, &str)> {
    let (time, rest) = line.split_once('\t')?;
    let (h, m) = parse_time(time.trim())?;
    Some((h, m, rest))
}

fn parse_time(t: &str) -> Option<(u32, u32)> {
    const PM: &[&str] = &["PM", "pm", "P.M.", "p.m.", "午後", "下午", "오후"];
    const AM: &[&str] = &["AM", "am", "A.M.", "a.m.", "午前", "上午", "오전"];
    let mut s = t;
    let mut pm = None;
    for (marks, is_pm) in [(PM, true), (AM, false)] {
        for m in marks {
            if let Some(r) = s.strip_prefix(m).or_else(|| s.strip_suffix(m)) {
                s = r.trim();
                pm = Some(is_pm);
            }
        }
    }
    let (h, m) = s.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (mut h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if m > 59 {
        return None;
    }
    match pm {
        Some(true) if (1..12).contains(&h) => h += 12,
        Some(false) if h == 12 => h = 0,
        Some(_) if !(1..=12).contains(&h) => return None,
        _ => {}
    }
    (h < 24).then_some((h, m))
}

/// A `"…"`-quoted text that may continue over the following lines.
/// Returns the text and how many extra lines it used; `None` when `first`
/// is not a quoted field (then it is taken literally).
fn unquote(first: &str, rest: &[&str]) -> Option<(String, usize)> {
    const MAX_LINES: usize = 5000;
    let body = first.strip_prefix('"')?;
    let mut text = String::new();
    let mut line = body;
    let mut used = 0usize;
    loop {
        let mut chars = line.char_indices().peekable();
        while let Some((idx, c)) = chars.next() {
            if c != '"' {
                text.push(c);
                continue;
            }
            if chars.peek().map(|&(_, c)| c) == Some('"') {
                chars.next();
                text.push('"');
                continue;
            }
            // A closing quote: only a quoted field if it ends the line.
            return (line[idx + 1..].trim().is_empty()).then_some((text, used));
        }
        if used >= rest.len() || used >= MAX_LINES {
            return None;
        }
        text.push('\n');
        line = rest[used];
        used += 1;
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "\u{feff}[LINE] Chat history with Alice\r\nSaved on: 5/2/2024, 21:07\r\n\r\n\
Mon, 1/15/2024\r\n\
09:12\tAlice\tGood morning\r\n\
09:13\tBob\t\"Two lines,\r\nthe \"\"second\"\" one\"\r\n\
09:13\tAlice\t[Sticker]\r\n\
21:20\t\tAlice unsent a message.\r\n\r\n\
Tue, 1/16/2024\r\n\
10:00\tBob\tok\r\n";

    #[test]
    fn english_export() {
        let e = parse_chat_export(EN).unwrap();
        assert_eq!(e.title.as_deref(), Some("Alice"));
        assert_eq!(e.messages.len(), 5);
        let m = &e.messages[1];
        assert_eq!(m.sender.as_deref(), Some("Bob"));
        assert_eq!(m.text, "Two lines,\nthe \"second\" one");
        assert_eq!(
            m.time,
            LocalTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 9,
                minute: 13
            }
        );
        assert_eq!(e.messages[2].kind, ExportedKind::Sticker);
        assert_eq!(e.messages[3].kind, ExportedKind::Event);
        assert_eq!(e.messages[3].sender, None);
        assert_eq!(e.messages[4].time.day, 16);
        assert_eq!(e.senders(), vec![("Alice".into(), 2), ("Bob".into(), 2)]);
    }

    #[test]
    fn japanese_year_first_and_12_hour() {
        let t = "[LINE] Aliceとのトーク履歴\n保存日時：2024/01/20 10:00\n\n2024/01/15(月)\n午後1:05\tAlice\t[写真]\n午前12:30\tBob\tこんにちは\n";
        let e = parse_chat_export(t).unwrap();
        assert_eq!(e.title.as_deref(), Some("Alice"));
        assert_eq!((e.messages[0].time.hour, e.messages[0].time.minute), (13, 5));
        assert_eq!(e.messages[0].kind, ExportedKind::Photo);
        assert_eq!(e.messages[1].time.hour, 0);
    }

    #[test]
    fn thai_buddhist_day_first() {
        let t = "[LINE] ประวัติการแชทกับ Somchai\nบันทึกเมื่อ: 02/05/2567 21:07\n\nจ., 15/01/2567\n09:00\tSomchai\tสวัสดี\n\nอ., 16/01/2567\n09:01\tSomchai\tok\n";
        let e = parse_chat_export(t).unwrap();
        assert_eq!(e.title.as_deref(), Some("Somchai"));
        assert_eq!(
            (
                e.messages[0].time.year,
                e.messages[0].time.month,
                e.messages[0].time.day
            ),
            (2024, 1, 15)
        );
    }

    #[test]
    fn weekday_settles_ambiguous_order() {
        // 2024-03-04 is a Monday; 2024-04-03 a Wednesday.
        let t = "Mon, 04/03/2024\n09:00\tA\thi\n";
        assert_eq!(parse_chat_export(t).unwrap().messages[0].time.month, 3);
        let t = "Mon, 03/04/2024\n09:00\tA\thi\n";
        assert_eq!(parse_chat_export(t).unwrap().messages[0].time.month, 3);
    }

    #[test]
    fn unquoted_continuations_and_literal_quotes() {
        let t = "2024/01/15 Mon\n09:00\tA\tfirst\nsecond\n\nthird\n09:01\tA\t\"quoted\" word\n2023/01/01\n";
        let e = parse_chat_export(t).unwrap();
        assert_eq!(e.messages[0].text, "first\nsecond\n\nthird");
        // A date going backwards is text, not a new day.
        assert_eq!(e.messages[1].text, "\"quoted\" word\n2023/01/01");
    }

    #[test]
    fn not_an_export() {
        assert!(parse_chat_export("hello\nworld\n").is_err());
    }

    #[test]
    fn utc_conversion() {
        let t = LocalTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 7,
            minute: 0,
        };
        assert_eq!(t.utc_ms(420), 0);
        assert_eq!(weekday(2024, 1, 15), 0);
    }

    #[test]
    fn messages_are_stable_and_ordered() {
        let e = parse_chat_export(EN).unwrap();
        let map = |n: &str| (n == "Alice").then(|| "Ualice".to_owned());
        let a = e.to_messages("Ualice", map, |t| t.utc_ms(0));
        let b = e.to_messages("Ualice", map, |t| t.utc_ms(0));
        assert_eq!(a, b);
        assert!(
            a.iter()
                .all(|m| is_imported_id(&m.id) && m.id.parse::<i64>().is_ok())
        );
        assert_eq!(a[0].from, "Ualice");
        assert_eq!(a[1].from, "import:Bob");
        assert_eq!(imported_sender_name(&a[1].from), Some("Bob"));
        // Same minute: file order kept.
        assert!(a[2].created_time > a[1].created_time);
        assert_eq!(fingerprint(&a[2]), Some((ExportedKind::Sticker, String::new())));
        assert_eq!(fingerprint(&a[3]), None);
        let real = Message {
            text: Some("Good morning\r\n".into()),
            ..Message::default()
        };
        assert_eq!(fingerprint(&real), fingerprint(&a[0]));
    }
}
