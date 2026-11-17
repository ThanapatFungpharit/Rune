//! Small development CLI.
//!
//! ```sh
//! cargo run --release --example cli -- login            # scan QR, writes session.json
//! cargo run --release --example cli -- whoami
//! cargo run --release --example cli -- chats
//! cargo run --release --example cli -- invites          # pending group invitations
//! cargo run --release --example cli -- accept <chat-mid> # or: decline <chat-mid>
//! cargo run --release --example cli -- send <mid> "hello"
//! cargo run --release --example cli -- recent <chat-mid> [count]   # [time] id from: (↩ quoted-id) text
//! cargo run --release --example cli -- reply <chat-mid> <message-id> "text"
//! cargo run --release --example cli -- watch            # print incoming operations
//! cargo run --release --example cli -- call talk/thrift/Talk/TalkService/getServerTime '[]'
//! cargo run --release --example cli -- cover <mid>|me [out.jpg]   # profile cover
//! cargo run --release --example cli -- set-cover photo.jpg        # replace your cover (public!)
//! cargo run --release --example cli -- albums <group-mid> [count] [dir]  # albums in recent history
//! ```
//!
//! `RUNE_SESSION` overrides the session path; `RUST_LOG=rune=debug` for logs.

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use rune::models::op_type;
use rune::{Client, Config, QrLoginEvent, QrLoginOptions, Session};

fn session_path() -> String {
    std::env::var("RUNE_SESSION").unwrap_or_else(|_| "session.json".into())
}

async fn load() -> Result<(Client, Session), Box<dyn std::error::Error>> {
    let s = Session::load_async(session_path())
        .await
        .map_err(|e| format!("{e} (run `login` first)"))?;
    let client = s.clone().into_client(Config::default())?;
    // Persist refreshed tokens automatically.
    let mut rx = client.subscribe_tokens();
    let saver = client.clone();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if let Err(e) = Session::from_client(&saver).save_async(session_path()).await {
                eprintln!("failed to save session: {e}");
            }
        }
    });
    Ok((client, s))
}

/// Message text, decrypting Letter-Sealed messages.
async fn body(c: &Client, m: &rune::Message) -> String {
    if let Some(call) = m.call() {
        return format!("📞 {call}");
    }
    let text = match c.decrypt_message(m).await {
        Ok(d) if m.is_e2ee() => format!("🔒 {}", d.text.unwrap_or_default()),
        Ok(d) => d.text.unwrap_or_default(),
        Err(e) => format!("<sealed, cannot decrypt: {e}>"),
    };
    relation(m) + &text
}

/// "↩ <id> " for replies, "↪ " for forwards.
fn relation(m: &rune::Message) -> String {
    match m.reply_to() {
        Some(id) => format!("↩ {id} "),
        None if m.is_forwarded() => "↪ ".into(),
        None => String::new(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).ok_or("missing argument");

    match args.first().map(String::as_str) {
        Some("login") => {
            let client = Client::new(Config::default())?;
            let res = client
                .qr_login(QrLoginOptions::default(), |ev| match ev {
                    QrLoginEvent::QrUrl(url) => match QrCode::new(url.as_bytes()) {
                        Ok(code) => println!(
                            "{}\nScan with the LINE app (or open: {url})",
                            code.render::<Dense1x2>().quiet_zone(true).build()
                        ),
                        Err(_) => println!("Open on your phone: {url}"),
                    },
                    QrLoginEvent::Pin(pin) => println!("Enter PIN {pin} on your phone"),
                })
                .await?;
            if let Some(e) = &res.e2ee_error {
                eprintln!("note: no Letter Sealing keys captured: {e}");
            }
            Session {
                tokens: client.tokens(),
                e2ee_keys: res.e2ee_keys,
            }
            .save_async(session_path())
            .await?;
            println!(
                "logged in as {} — session saved to {}",
                res.mid.unwrap_or_default(),
                session_path()
            );
        }
        Some("refresh") => {
            let (c, s) = load().await?;
            c.refresh_token().await?;
            // Save here too (awaited); the background saver may not have run yet.
            Session {
                tokens: c.tokens(),
                e2ee_keys: s.e2ee_keys,
            }
            .save_async(session_path())
            .await?;
            let due = c.tokens().refresh_due_epoch_sec().unwrap_or(0);
            println!("token refreshed; next refresh due at epoch {due}");
        }
        Some("download") => {
            // download CHAT MESSAGE_ID OUT [preview]
            let (c, _) = load().await?;
            let (chat, id, out) = (arg(1)?, arg(2)?, arg(3)?);
            let msgs = c.recent_messages(chat, 200).await?;
            let m = msgs
                .iter()
                .find(|m| m.id == id)
                .ok_or("message not in the last 200 of that chat")?;
            let data = c
                .download_media(m, args.get(4).is_some_and(|a| a == "preview"))
                .await?;
            tokio::fs::write(out, &data).await?;
            println!("saved {} bytes to {out}", data.len());
        }
        Some(cmd @ ("send-image" | "send-file" | "send-video" | "send-audio")) => {
            // send-image MID PATH | send-video MID PATH DURATION_MS
            let (c, _) = load().await?;
            let path = std::path::Path::new(arg(2)?);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_owned();
            let data = tokio::fs::read(path).await?;
            let dur = || {
                args.get(3)
                    .and_then(|d| d.parse().ok())
                    .ok_or("duration in ms required")
            };
            let up = match cmd {
                "send-image" => rune::MediaUpload {
                    original: true,
                    ..rune::MediaUpload::image(data, name)
                },
                "send-video" => rune::MediaUpload::video(data, name, dur()?),
                "send-audio" => rune::MediaUpload::audio(data, name, dur()?),
                _ => rune::MediaUpload::file(data, name),
            };
            let m = c.send_media(arg(1)?, up).await?;
            println!(
                "sent message {} ({})",
                m.id,
                if m.is_e2ee() { "sealed" } else { "plain" }
            );
        }
        Some("picture") => {
            // picture MID OUT  (profile picture of a contact, or of yourself with "me")
            let (c, _) = load().await?;
            let path = match arg(1)? {
                "me" => c.profile().await?.picture_path.unwrap_or_default(),
                mid => c
                    .contacts(&[mid.to_owned()])
                    .await?
                    .remove(mid)
                    .and_then(|k| k.picture_path)
                    .unwrap_or_default(),
            };
            let data = c.download_picture(&path, rune::PictureSize::Full).await?;
            tokio::fs::write(arg(2)?, &data).await?;
            println!("saved {} bytes", data.len());
        }
        Some("cover") => {
            // cover MID|me [OUT]  (profile cover; saves the picture when OUT is given)
            let (c, _) = load().await?;
            let mid = match arg(1)? {
                "me" => c.my_mid().ok_or("not logged in")?,
                mid => mid.to_owned(),
            };
            let cover = c.profile_cover(&mid).await?;
            match cover.custom_image() {
                Some(o) => println!("cover {}", o.path()),
                None => println!("default cover"),
            }
            if let Some(v) = &cover.video {
                println!("video cover {}", v.path());
            }
            if let Some(out) = args.get(2) {
                match c.download_cover(&cover).await? {
                    Some(data) => {
                        tokio::fs::write(out, &data).await?;
                        println!("saved {} bytes to {out}", data.len());
                    }
                    None => println!("nothing to save (default cover)"),
                }
            }
        }
        Some("set-cover") => {
            // set-cover PATH  (replaces YOUR profile cover; everyone sees it)
            let (c, _) = load().await?;
            let path = std::path::Path::new(arg(1)?);
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("cover.jpg");
            let o = c.set_profile_cover(&tokio::fs::read(path).await?, name).await?;
            println!("cover set: {}", o.path());
        }
        Some("albums") => {
            // albums CHAT [COUNT] [DIR]  (albums seen in the chat's recent history;
            // with DIR, saves every photo they show)
            let (c, _) = load().await?;
            let chat = arg(1)?;
            let count = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(200);
            // (album id, name, [(post, sender)])
            type Seen = (String, String, Vec<(rune::AlbumPost, String)>);
            let mut albums: Vec<Seen> = Vec::new();
            for m in c.recent_messages(chat, count).await? {
                if let Some(ev) = m.album_event() {
                    println!("event: {} {}", m.from, ev.describe());
                }
                let Some(p) = m.album_post() else { continue };
                match albums.iter_mut().find(|(id, ..)| *id == p.album_id) {
                    Some((_, name, posts)) => {
                        if name.is_empty() {
                            name.clone_from(&p.album_name);
                        }
                        posts.push((p, m.from.clone()));
                    }
                    None => albums.push((
                        p.album_id.clone(),
                        p.album_name.clone(),
                        vec![(p, m.from.clone())],
                    )),
                }
            }
            if albums.is_empty() {
                println!("no album posts in the last {count} messages");
            }
            // Photos download several at a time; lines print in album order.
            use futures_util::StreamExt;
            let jobs: Vec<_> = albums
                .iter()
                .flat_map(|(id, name, posts)| {
                    let photos: usize = posts.iter().map(|(p, _)| p.media.len()).sum();
                    let header = format!("album {id} \"{name}\": {} posts, {photos} photos", posts.len());
                    std::iter::once((Some(header), None)).chain(
                        posts
                            .iter()
                            .flat_map(|(p, from)| p.media.iter().map(move |m| (None, Some((p, from, m))))),
                    )
                })
                .collect();
            let dir = args.get(3).map(std::path::Path::new);
            let c = &c;
            let mut lines = futures_util::stream::iter(jobs)
                .map(|(header, item)| async move {
                    if let Some(h) = header {
                        return Ok::<_, Box<dyn std::error::Error>>(h);
                    }
                    let (p, from, media) = item.expect("header or item");
                    let mut line = format!("  {} {} from {from}", media.media_type, media.oid);
                    if let Some(dir) = dir {
                        let data = c.download_album_media(&p.chat, media).await?;
                        let ext = if media.is_video() { "mp4" } else { "jpg" };
                        let out = dir.join(format!("{}.{ext}", media.oid));
                        tokio::fs::write(&out, &data).await?;
                        line += &format!(" -> {} ({} bytes)", out.display(), data.len());
                    }
                    Ok(line)
                })
                .buffered(6);
            while let Some(line) = lines.next().await {
                println!("{}", line?);
            }
        }
        Some("sticker") => {
            // sticker STKID OUT
            let (c, _) = load().await?;
            let data = c.download_sticker(arg(1)?, rune::StickerImage::Static2x).await?;
            tokio::fs::write(arg(2)?, &data).await?;
            println!("saved {} bytes", data.len());
        }
        Some("e2ee-status") => {
            let (c, _) = load().await?;
            let st = c.e2ee_status().await?;
            println!(
                "local keys {:?}, server keys {:?}, current: {}",
                st.local_key_ids,
                st.server_key_ids,
                st.is_current()
            );
        }
        Some("whoami") => {
            let (c, s) = load().await?;
            let p = c.profile().await?;
            println!(
                "{} ({}) — {} E2EE key(s) stored",
                p.display_name,
                p.mid,
                s.e2ee_keys.len()
            );
        }
        Some("chats") => {
            let (c, _) = load().await?;
            let (member, invited) = c.chat_mids().await?;
            for chat in c.chats(&member).await? {
                println!(
                    "{}  {}  ({} members)",
                    chat.chat_mid,
                    chat.chat_name,
                    chat.member_mids().len()
                );
            }
            if !invited.is_empty() {
                println!("{} pending invitation(s) — see `invites`", invited.len());
            }
        }
        Some("invites") => {
            let (c, _) = load().await?;
            let chats = c.invited_chats().await?;
            if chats.is_empty() {
                println!("no pending invitations");
            }
            for chat in chats {
                println!(
                    "{}  {}  ({} members)",
                    chat.chat_mid,
                    chat.chat_name,
                    chat.member_mids().len()
                );
            }
        }
        Some("accept") => {
            let (c, _) = load().await?;
            c.accept_chat_invitation(arg(1)?).await?;
            println!("joined {}", arg(1)?);
        }
        Some("decline") => {
            let (c, _) = load().await?;
            c.reject_chat_invitation(arg(1)?).await?;
            println!("declined {}", arg(1)?);
        }
        Some("send") => {
            let (c, _) = load().await?;
            let m = c.send_text(arg(1)?, arg(2)?).await?;
            println!("sent message {}", m.id);
        }
        Some("reply") => {
            // reply CHAT MESSAGE_ID TEXT (ids: see `recent`)
            let (c, _) = load().await?;
            let m = c.send_reply(arg(1)?, arg(3)?, arg(2)?).await?;
            println!("sent reply {}", m.id);
        }
        Some("recent") => {
            let (c, _) = load().await?;
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
            let mut msgs = c.recent_messages(arg(1)?, n).await?;
            msgs.reverse();
            for m in msgs {
                println!(
                    "[{}] {} {}: {}",
                    m.created_time.unwrap_or(0),
                    m.id,
                    m.from,
                    body(&c, &m).await
                );
            }
        }
        Some("watch") => {
            let (c, _) = load().await?;
            let from_rev = args.get(1).and_then(|s| s.parse().ok());
            let mut ops = c.operations(from_rev);
            loop {
                let op = ops.next_operation().await?;
                match (op.r#type, &op.message) {
                    (op_type::RECEIVE_MESSAGE | op_type::SEND_MESSAGE, Some(m)) => {
                        // The stream already decrypted it; a failure is kept, not dropped.
                        let text = match &op.e2ee_error {
                            Some(f) => format!("<sealed, cannot decrypt: {f}>"),
                            None if m.is_e2ee() => "<sealed>".into(),
                            // Raw metadata too: call records are not live-verified yet.
                            None => match m.call() {
                                Some(call) => format!("📞 {call} {:?}", m.content_metadata),
                                None => m.text.clone().unwrap_or_default(),
                            },
                        };
                        println!("#{} {} -> {}: {}{}", op.revision, m.from, m.to, relation(m), text);
                    }
                    _ => println!(
                        "#{} op type {} {:?} {:?} {:?}",
                        op.revision, op.r#type, op.param1, op.param2, op.param3
                    ),
                }
            }
        }
        Some("emojis") => {
            // LINE emoji packages usable as reactions.
            let (c, _) = load().await?;
            for p in c.owned_emoji_packages().await? {
                let sample: Vec<_> = p
                    .emojis
                    .iter()
                    .take(3)
                    .map(|e| format!("{} {}", e.0, e.1))
                    .collect();
                println!(
                    "{} v{} type {} — {} ({} emoji: {}, …)",
                    p.id,
                    p.version,
                    p.resource_type,
                    p.name,
                    p.emojis.len(),
                    sample.join(", ")
                );
            }
        }
        Some("call") => {
            let (c, _) = load().await?;
            let params: serde_json::Value = serde_json::from_str(args.get(2).map_or("[]", String::as_str))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&c.call(arg(1)?, params).await?)?
            );
        }
        _ => {
            eprintln!(
                "usage: cli <login|refresh|e2ee-status|download CHAT ID OUT [preview]|send-image|send-file|send-video|send-audio MID PATH [MS]|picture MID|me OUT|sticker STKID OUT|emojis|whoami|chats|invites|accept CHAT|decline CHAT|send MID TEXT|reply CHAT MSGID TEXT|recent CHAT [N]|watch [REV]|call METHOD JSON>"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}
