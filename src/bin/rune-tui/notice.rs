//! Notifications: what they say (pure, tested) and how they are shown
//! (terminal bell, `notify-send` with the sender's picture as the icon,
//! `osascript`, else the terminal's OSC 777).
//!
//! Wording rules, the same for every event:
//! - a 1:1 chat is titled with the sender, a group with the group, and the
//!   body then says who: `Alice: hi`, `Alice sent a photo`;
//! - text is quoted as sent, everything else is a short sentence about what
//!   happened (`Sent a photo`, `Missed voice call`), never the chat list's
//!   bracketed summaries;
//! - an unknown name is looked up first and, failing that, reads `Someone` /
//!   `Group chat` rather than a raw mid; a missing picture just means no icon.

use crate::*;
use rune::models::format_call_duration;
use settings::NotifyMode;

/// Longest title / body shown, in characters (notification popups cut
/// long text off anyway; this keeps the cut readable).
const TITLE_MAX: usize = 80;
const BODY_MAX: usize = 240;
/// Lines of a multi-line message shown.
const BODY_LINES: usize = 4;
/// How long a notification may wait for a name or picture lookup.
const LOOKUP_WAIT: Duration = Duration::from_secs(3);

/// Stand-ins for names that cannot be found.
pub const UNKNOWN_SENDER: &str = "Someone";
pub const UNKNOWN_GROUP: &str = "Group chat";

/// Freedesktop notification category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A message or call (`im.received`).
    Received,
    /// Anything else about chats, e.g. an invitation (`im`).
    Chat,
    /// Not about a chat (a plugin, the test).
    Other,
}

/// One notification, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub category: Category,
}

impl Notice {
    pub fn new(title: &str, body: &str, category: Category) -> Self {
        Notice {
            title: clean_line(title, TITLE_MAX),
            body: clean_body(body),
            category,
        }
    }
}

/// What a message says, for a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gist {
    /// Words to quote (prefixed `Sender: ` in groups).
    Text(String),
    /// A lower-case predicate with the sender as subject: `sent a photo`.
    Did(String),
}

/// The gist of a message for its notification, or `None` when it is not
/// worth one (chat events other than album changes, calls that connected,
/// that we declined or that ended).
pub fn gist(m: &Message, e2ee_error: Option<&str>) -> Option<Gist> {
    use Gist::{Did, Text};
    if e2ee_error.is_some() {
        return Some(Did("sent a message that could not be decrypted".into()));
    }
    let meta = &m.content_metadata;
    let named = |what: &str, key: &str| match meta.get(key).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(n) => format!("{what}: {n}"),
        None => what.to_owned(),
    };
    let timed = |what: &str| match meta
        .get("DURATION")
        .and_then(|d| d.parse::<u64>().ok())
        .filter(|d| *d >= 1000)
    {
        Some(d) => format!("{what} ({})", format_call_duration(d)),
        None => what.to_owned(),
    };
    Some(match m.content_type {
        ct::TEXT if m.is_e2ee() => Did("sent an encrypted message".into()),
        ct::TEXT => match m.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => Text(t.to_owned()),
            None => Did("sent a message".into()),
        },
        ct::IMAGE => Did("sent a photo".into()),
        ct::VIDEO => Did(timed("sent a video")),
        ct::AUDIO => Did(timed("sent a voice message")),
        ct::STICKER => match meta
            .get("STKTXT")
            .map(|t| t.trim().trim_start_matches('[').trim_end_matches(']').trim())
        {
            Some(t) if !t.is_empty() && !t.eq_ignore_ascii_case("sticker") => {
                Did(format!("sent a sticker ({t})"))
            }
            _ => Did("sent a sticker".into()),
        },
        ct::CONTACT => Did(named("shared a contact", "displayName")),
        ct::FILE => Did(named("sent a file", "FILE_NAME")),
        ct::LOCATION => {
            let title = m
                .extra
                .get("location")
                .and_then(|l| l.get("title").or_else(|| l.get("address")))
                .and_then(|t| t.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty());
            Did(match title {
                Some(t) => format!("shared a location: {t}"),
                None => "shared a location".into(),
            })
        }
        ct::FLEX => match meta.get("ALT_TEXT").map(|t| t.trim()).filter(|t| !t.is_empty()) {
            Some(t) => Text(t.to_owned()),
            None => Did("sent a message".into()),
        },
        ct::CALL => call_gist(m)?,
        rune::album::POST_NOTIFICATION => match m.album_post() {
            Some(p) => Did(match (p.kind, p.media.len()) {
                (rune::AlbumPostKind::Created, _) => format!("created the album \"{}\"", p.album_name),
                (_, 0 | 1) => format!("added a photo to the album \"{}\"", p.album_name),
                (_, n) => format!("added {n} items to the album \"{}\"", p.album_name),
            }),
            None => Did("shared a post".into()),
        },
        rune::album::CHAT_EVENT => Did(m.album_event()?.describe()),
        _ => Did("sent a message".into()),
    })
}

/// Someone else's call record: worth a notification only when it asks
/// something of us (a missed call, a group call to join). rune cannot
/// take calls, so a call to join points at the phone.
fn call_gist(m: &Message) -> Option<Gist> {
    use rune::{CallMedia, CallOutcome};
    let call = m.call()?;
    let kind = match call.media {
        CallMedia::Voice => "voice call",
        CallMedia::Video => "video call",
        CallMedia::Live => "live broadcast",
    };
    match call.outcome {
        CallOutcome::GroupStarted => Some(Gist::Did(format!("started a group {kind} — join on your phone"))),
        CallOutcome::GroupInvited => Some(Gist::Did(format!(
            "invited you to a group {kind} — join on your phone"
        ))),
        CallOutcome::Unanswered | CallOutcome::Cancelled | CallOutcome::Busy => {
            Some(Gist::Text(format!("Missed {kind}")))
        }
        CallOutcome::Other(_) if !call.group => Some(Gist::Text(call.to_string())),
        _ => None,
    }
}

/// Who and where a message notification is about (names already looked
/// up; `None` when still unknown).
pub struct About<'a> {
    pub sender: Option<&'a str>,
    /// `Some` for a group or room (its name, if known).
    pub group: Option<Option<&'a str>>,
}

/// The notification for a message: a 1:1 chat is titled with the sender,
/// a group with the group. Without `preview` only who wrote is shown (call
/// alerts have no private text and always show).
pub fn message_notice(about: &About, gist: &Gist, preview: bool, is_call: bool) -> Notice {
    let sender = about
        .sender
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(UNKNOWN_SENDER);
    let show = preview || is_call;
    match about.group {
        None => {
            let body = match gist {
                _ if !show => "New message".to_owned(),
                Gist::Text(t) => t.clone(),
                Gist::Did(d) => capitalize(d),
            };
            Notice::new(sender, &body, Category::Received)
        }
        Some(name) => {
            let title = name.filter(|s| !s.trim().is_empty()).unwrap_or(UNKNOWN_GROUP);
            let body = match gist {
                _ if !show => format!("New message from {sender}"),
                Gist::Text(t) => format!("{sender}: {t}"),
                Gist::Did(d) => format!("{sender} {d}"),
            };
            Notice::new(title, &body, Category::Received)
        }
    }
}

/// A call ringing now (op 50).
pub fn ringing_notice(caller: Option<&str>) -> Notice {
    match caller.filter(|c| !c.trim().is_empty()) {
        Some(who) => Notice::new(who, "Incoming call — answer on your phone", Category::Received),
        None => Notice::new("Incoming call", "Answer on your phone", Category::Received),
    }
}

/// An invitation to a group.
pub fn invitation_notice(group: Option<&str>) -> Notice {
    match group.filter(|g| !g.trim().is_empty()) {
        Some(g) => Notice::new(g, "You're invited to join this group", Category::Chat),
        None => Notice::new(
            "Group invitation",
            "You're invited to join a group",
            Category::Chat,
        ),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// One line of at most `max` characters, without control characters,
/// whitespace runs collapsed; cut with `…`.
fn clean_line(s: &str, max: usize) -> String {
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect();
    truncate(&words.join(" "), max)
}

/// At most `BODY_LINES` non-blank lines of at most `BODY_MAX` characters
/// in all, without control characters other than the line breaks.
fn clean_body(s: &str) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|l| clean_line(l, BODY_MAX))
        .filter(|l| !l.is_empty())
        .collect();
    let more = lines.len() > BODY_LINES;
    let mut body = lines.into_iter().take(BODY_LINES).collect::<Vec<_>>().join("\n");
    if more && !body.ends_with('…') {
        body.push('…');
    }
    truncate(&body, BODY_MAX)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Body text for a server that reads it as markup (the freedesktop spec's
/// `body-markup`): `&`, `<` and `>` must not start tags or entities.
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Text that is safe inside an OSC sequence: no control character
/// (C0 or C1, which could end the sequence and start another) and no `;`
/// (the field separator).
fn osc_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() || c == ';' { ' ' } else { c })
        .collect()
}

/// Show a notification now (external programs run detached). `icon` is a
/// picture file, shown where the desktop supports it.
pub fn show(mode: NotifyMode, n: &Notice, icon: Option<&std::path::Path>) {
    use std::io::Write;
    if cfg!(test) {
        return;
    }
    match mode {
        NotifyMode::Off => {}
        NotifyMode::Bell => {
            let mut out = std::io::stdout();
            let _ = out.write_all(b"\x07");
            let _ = out.flush();
        }
        NotifyMode::Desktop => {
            let spawned = if cfg!(target_os = "macos") {
                let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
                crate::spawn_reaped(std::process::Command::new("osascript").arg("-e").arg(format!(
                    "display notification \"{}\" with title \"{}\"",
                    esc(&n.body),
                    esc(&n.title)
                )))
            } else {
                let mut cmd = std::process::Command::new("notify-send");
                cmd.arg("--app-name=rune");
                match n.category {
                    Category::Received => drop(cmd.arg("--category=im.received")),
                    Category::Chat => drop(cmd.arg("--category=im")),
                    Category::Other => {}
                }
                if let Some(icon) = icon {
                    cmd.arg(format!("--icon={}", icon.display()));
                }
                crate::spawn_reaped(cmd.arg("--").arg(&n.title).arg(escape_markup(&n.body)))
            };
            if spawned.is_err() {
                // OSC 777 (urxvt, foot, WezTerm, Ghostty, …).
                let mut out = std::io::stdout();
                let _ = write!(
                    out,
                    "\x1b]777;notify;{};{}\x07",
                    osc_text(&n.title),
                    osc_text(&n.body)
                );
                let _ = out.flush();
            }
        }
    }
}

/// The picture behind `path` (a profile/group picture path) as a file the
/// notifier can read: written once into [`view_dir`] under a name derived
/// from the path (a changed picture gets a new path, hence a new file).
/// `None` when it cannot be had in time; the notification then goes
/// without an icon.
async fn icon_file(client: &Client, store: &Store, path: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(path.as_bytes());
    let hex: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    let dir = view_dir();
    for ext in ["jpg", "png", "gif", "webp"] {
        let p = dir.join(format!("icon-{hex}.{ext}"));
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Some(p);
        }
    }
    let (bytes, _) = tokio::time::timeout(
        LOOKUP_WAIT,
        cached_picture(client, store, path, PictureSize::Small),
    )
    .await
    .ok()??;
    let ext = match image::guess_format(&bytes).ok()? {
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Png => "png",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::WebP => "webp",
        _ => return None,
    };
    tokio::task::spawn_blocking(move || {
        let p = private_view_dir()?.join(format!("icon-{hex}.{ext}"));
        match write_new(&p, &bytes) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
            _ => Ok(p),
        }
    })
    .await
    .ok()?
    .inspect_err(|e| tracing::debug!("cannot write a notification icon: {e}"))
    .ok()
}

/// A notification that may still need names or a picture: what is known
/// when the event arrives, and what to look up before showing it.
pub struct Pending {
    pub mode: NotifyMode,
    /// Mids whose names are unknown (looked up, at most `LOOKUP_WAIT`).
    pub lookup: Vec<String>,
    /// Picture paths already known, by mid.
    pub pictures: HashMap<String, String>,
    /// Mids whose pictures to try for the icon, in order.
    pub icon_from: Vec<String>,
    /// The chat (or caller) that, if a lookup finds it muted, cancels this.
    pub mute_key: Option<String>,
    /// Builds the notice from the names known by then (a mid absent from
    /// the map is unknown).
    pub build: Build,
    /// Names already known, by mid.
    pub names: HashMap<String, String>,
}

/// Makes a notice from the names known when it is shown.
pub type Build = Box<dyn FnOnce(&HashMap<String, String>) -> Option<Notice> + Send>;

#[cfg(test)]
thread_local! {
    /// What tests would have shown (nothing is delivered from tests).
    pub static SHOWN: std::cell::RefCell<Vec<Notice>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Look up what `p` lacks (telling the app what was found, as any lookup
/// does), then show it. Runs as its own task, so a slow lookup never holds
/// up the UI; whatever is still missing after `LOOKUP_WAIT` falls back.
pub fn spawn(client: Client, tx: mpsc::UnboundedSender<AppEvent>, store: Store, p: Pending) {
    #[cfg(test)]
    {
        // No lookups and no desktop: record what the known names give.
        let _ = (&client, &tx, &store);
        if p.mode != NotifyMode::Off
            && let Some(n) = (p.build)(&p.names)
        {
            SHOWN.with(|s| s.borrow_mut().push(n));
        }
        return;
    }
    #[allow(unreachable_code)]
    tokio::spawn(async move {
        let Pending {
            mode,
            lookup,
            mut pictures,
            icon_from,
            mute_key,
            build,
            mut names,
        } = p;
        if !lookup.is_empty()
            && let Ok(found) = tokio::time::timeout(LOOKUP_WAIT, lookup_names(&client, lookup)).await
        {
            if mute_key
                .as_ref()
                .is_some_and(|k| found.mutes.get(k) == Some(&true))
            {
                found.post(&tx);
                return;
            }
            for (mid, name) in &found.names {
                if !name.trim().is_empty() {
                    names.insert(mid.clone(), name.clone());
                }
            }
            for (mid, pic) in &found.pictures {
                match pic {
                    Some(path) if !path.trim().is_empty() => drop(pictures.insert(mid.clone(), path.clone())),
                    _ => drop(pictures.remove(mid)),
                }
            }
            found.post(&tx);
        }
        let Some(notice) = build(&names) else {
            return;
        };
        let mut icon = None;
        if mode == NotifyMode::Desktop && !cfg!(target_os = "macos") {
            for path in icon_from.iter().filter_map(|mid| pictures.get(mid)) {
                icon = icon_file(&client, &store, path).await;
                if icon.is_some() {
                    break;
                }
            }
        }
        show(mode, &notice, icon.as_deref());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(content_type: i64, meta: &[(&str, &str)]) -> Message {
        Message {
            content_type,
            content_metadata: meta.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
            ..Default::default()
        }
    }

    fn text(t: &str) -> Message {
        Message {
            text: Some(t.into()),
            ..msg(ct::TEXT, &[])
        }
    }

    const DM: About = About {
        sender: Some("Alice"),
        group: None,
    };
    const GROUP: About = About {
        sender: Some("Alice"),
        group: Some(Some("Climbing")),
    };

    fn say(about: &About, m: &Message, preview: bool) -> (String, String) {
        let g = gist(m, None).unwrap();
        let n = message_notice(about, &g, preview, m.content_type == ct::CALL);
        (n.title, n.body)
    }

    #[test]
    fn one_to_one_and_group_wording() {
        assert_eq!(say(&DM, &text("hi"), true), ("Alice".into(), "hi".into()));
        assert_eq!(
            say(&GROUP, &text("hi"), true),
            ("Climbing".into(), "Alice: hi".into())
        );
        let photo = msg(ct::IMAGE, &[]);
        assert_eq!(say(&DM, &photo, true).1, "Sent a photo");
        assert_eq!(say(&GROUP, &photo, true).1, "Alice sent a photo");
    }

    #[test]
    fn previews_off_hide_the_content_but_not_who() {
        assert_eq!(
            say(&DM, &text("secret"), false),
            ("Alice".into(), "New message".into())
        );
        assert_eq!(say(&GROUP, &text("secret"), false).1, "New message from Alice");
        // A missed call holds no private text.
        let missed = msg(ct::CALL, &[("TYPE", "A"), ("RESULT", "NO_ANSWER")]);
        assert_eq!(say(&DM, &missed, false).1, "Missed voice call");
    }

    #[test]
    fn missing_names_fall_back_to_words_not_mids() {
        let about = About {
            sender: None,
            group: Some(None),
        };
        assert_eq!(
            say(&about, &text("hi"), true),
            (UNKNOWN_GROUP.into(), format!("{UNKNOWN_SENDER}: hi"))
        );
        let blank = About {
            sender: Some("  "),
            group: None,
        };
        assert_eq!(say(&blank, &text("hi"), true).0, UNKNOWN_SENDER);
        assert_eq!(ringing_notice(None).title, "Incoming call");
        assert_eq!(
            ringing_notice(Some("Bob")).body,
            "Incoming call — answer on your phone"
        );
        assert_eq!(invitation_notice(Some("")).title, "Group invitation");
        assert_eq!(invitation_notice(Some("Climbing")).title, "Climbing");
    }

    #[test]
    fn each_kind_reads_as_a_sentence() {
        let g = |m: &Message| match gist(m, None).unwrap() {
            Gist::Did(d) => d,
            Gist::Text(t) => t,
        };
        assert_eq!(
            g(&msg(ct::VIDEO, &[("DURATION", "12500")])),
            "sent a video (0:12)"
        );
        assert_eq!(g(&msg(ct::VIDEO, &[])), "sent a video");
        assert_eq!(
            g(&msg(ct::AUDIO, &[("DURATION", "65000")])),
            "sent a voice message (1:05)"
        );
        assert_eq!(
            g(&msg(ct::STICKER, &[("STKTXT", "[Thanks]")])),
            "sent a sticker (Thanks)"
        );
        assert_eq!(g(&msg(ct::STICKER, &[("STKTXT", "[Sticker]")])), "sent a sticker");
        assert_eq!(g(&msg(ct::FILE, &[("FILE_NAME", "a.pdf")])), "sent a file: a.pdf");
        assert_eq!(g(&msg(ct::FILE, &[])), "sent a file");
        assert_eq!(
            g(&msg(ct::CONTACT, &[("displayName", "Bob")])),
            "shared a contact: Bob"
        );
        assert_eq!(g(&msg(ct::FLEX, &[("ALT_TEXT", "Your order")])), "Your order");
        assert_eq!(g(&msg(ct::FLEX, &[])), "sent a message");
        assert_eq!(g(&text("   ")), "sent a message");
        assert_eq!(g(&msg(999, &[])), "sent a message");
        let sealed = Message {
            chunks: Some(vec!["a".into(), "b".into(), "c".into()]),
            ..msg(ct::TEXT, &[])
        };
        assert_eq!(g(&sealed), "sent an encrypted message");
        assert_eq!(
            gist(&text("x"), Some("no key")),
            Some(Gist::Did("sent a message that could not be decrypted".into()))
        );
    }

    #[test]
    fn calls_and_chat_events_notify_only_when_they_ask_something() {
        let call = |meta: &[(&str, &str)]| gist(&msg(ct::CALL, meta), None);
        assert_eq!(
            call(&[("GC_EVT_TYPE", "S"), ("GC_MEDIA_TYPE", "VIDEO")]),
            Some(Gist::Did(
                "started a group video call — join on your phone".into()
            ))
        );
        assert_eq!(
            call(&[("TYPE", "V"), ("RESULT", "NO_ANSWER")]),
            Some(Gist::Text("Missed video call".into()))
        );
        // Connected, declined and ended calls are no news.
        assert_eq!(call(&[("TYPE", "A"), ("DURATION", "5000")]), None);
        assert_eq!(call(&[("RESULT", "REJECTED")]), None);
        assert_eq!(call(&[("GC_EVT_TYPE", "E")]), None);
        // A chat event that is not an album change (a join, a rename) has no body.
        assert_eq!(gist(&msg(rune::album::CHAT_EVENT, &[]), None), None);
    }

    #[test]
    fn text_is_cleaned_and_bounded() {
        let n = Notice::new("Ali\x1bce\n\tB", "a\x07b\n\n\nc\nd\ne\nf", Category::Other);
        assert_eq!(n.title, "Ali ce B");
        assert_eq!(n.body, "a b\nc\nd\ne…");
        let long = Notice::new(&"x".repeat(500), &"y".repeat(500), Category::Other);
        assert_eq!(long.title.chars().count(), TITLE_MAX);
        assert!(long.title.ends_with('…'));
        assert_eq!(long.body.chars().count(), BODY_MAX);
        assert_eq!(Notice::new("สวัสดี 👋", "", Category::Other).title, "สวัสดี 👋");
    }

    #[test]
    fn body_markup_and_osc_cannot_be_injected() {
        assert_eq!(escape_markup("<b>a</b> & b"), "&lt;b&gt;a&lt;/b&gt; &amp; b");
        // BEL, ESC, the 8-bit string terminator (U+009C), CR/LF and `;`.
        let hostile = "hi\x07\x1b]0;pwned\x1b\\\u{9c}\r\nx;y";
        let clean = osc_text(hostile);
        assert!(!clean.chars().any(|c| c.is_control() || c == ';'), "{clean:?}");
        assert_eq!(osc_text("สวัสดี 👋"), "สวัสดี 👋");
    }
}
