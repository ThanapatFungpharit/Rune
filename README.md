# rune

An **unofficial** Rust client for LINE that uses the protocol of the LINE
Chrome extension (`CHROMEOS` 3.7.2). It logs in to a personal account by QR
code, calls the extension's Thrift-over-JSON gateway, and streams incoming
operations.

The mandatory `X-Hmac` request signature, and the Letter Sealing
primitives, come from LINE's own `ltsm.wasm`. `rune` runs that module natively
with [wasmtime](https://wasmtime.dev), so you need neither Node.js nor a
browser. Its output matches the reference implementation byte for byte (see
[docs/PROTOCOL.md](docs/PROTOCOL.md)).

> ⚠️ **Read this first.** `rune` is an unofficial, third-party LINE client. It is not an official LINE API, is not
> developed by, affiliated with, or endorsed by LY Corporation.
>
> `rune` communicates with LINE using undocumented protocols derived from the LINE ChromeOS extension. These protocols
> are not officially supported for third-party client development and may change or stop working without notice.
>
> Using unofficial clients and reverse-engineered protocols may conflict with LINE's Terms of Use, particularly
> provisions concerning unauthorized or improper service access, automated activity, and reverse engineering.
>
> LY Corporation may restrict access, invalidate sessions, suspend functionality, or take action against accounts in
> accordance with its terms. Account restrictions or loss of access are possible, and no compatibility or account-safety
> guarantees are provided.
>
> **Security and privacy:** `rune` handles sensitive authentication credentials, E2EE private keys, and decrypted
> messages. Its local session and database files should be treated as confidential. You are also responsible for handling
> other people's messages and personal information appropriately.
>
> Use at your own risk. Only authenticate accounts you are authorized to use, avoid excessive requests, protect your
> local data, and review [LINE's current Terms of Use](https://terms.line.me/line_terms?lang=en) before using this client.

## Status

| Feature                                                                                                                                                                                                                                     | Status                                                                                                                                                                                                                                |
|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `X-Hmac` signing (native `ltsm.wasm`)                                                                                                                                                                                                       | ✅ verified against reference vectors                                                                                                                                                                                                 |
| QR login (incl. PIN step), certificate reuse                                                                                                                                                                                                | ✅ live-tested                                                                                                                                                                                                                        |
| Token refresh (code 119 / 401, retry policy, 10201 kickout)                                                                                                                                                                                 | ✅ refresh endpoint live-tested; 119/10201 paths mock-tested                                                                                                                                                                          |
| Timed token refresh (`spawn_token_refresher`: renews before `durationUntilRefreshInSec` runs out, wall-clock aware)                                                                                                                         | ✅ unit + mock-tested; the TUI runs it                                                                                                                                                                                                |
| Session save/restore (tokens + E2EE key blobs)                                                                                                                                                                                              | ✅                                                                                                                                                                                                                                    |
| Profile, contacts, friends, block/unblock                                                                                                                                                                                                   | ✅ profile + contact list live; the rest shapes from reference                                                                                                                                                                        |
| Chat list paging, including inactive (hidden or old) chats (`message_boxes_page`); TUI loads more when the last chat is selected                                                                                                            | ✅ page shapes and cursor checked live (read-only); TUI paging unit-tested, not yet clicked through live                                                                                                                              |
| Group chats: list, create, invite, kick, leave, rename, mute (`create_group`, `rename_chat`, `set_chat_muted`); TUI menus                                                                                                                   | ✅ listing + mute state live; create/invite/leave/rename/mute calls mock-tested only                                                                                                                                                  |
| Joining a group by invitation link / QR ticket                                                                                                                                                                                              | ❌ the gateway answers `REQUEST_NOT_FOUND` for `findChatByTicket` / `acceptChatInvitationByTicket` (live)                                                                                                                             |
| Group invitations: list pending (`invited_chats`), accept/decline (CLI + TUI)                                                                                                                                                               | ✅ listing + decline live; accept mock-tested only                                                                                                                                                                                    |
| Replies: send (`send_reply`, `OutgoingMessage::reply_to`), detect (`Message::reply_to`, `is_forwarded`), TUI quotes                                                                                                                         | ✅ detection live on real history; sending mock/unit-tested only                                                                                                                                                                      |
| OpenChat (Square)                                                                                                                                                                                                                           | ❌ not served by the Chrome gateway (see `docs/PROTOCOL.md`)                                                                                                                                                                          |
| Messages: text, sticker, contact, location, reply, unsend, reactions, read marks, history                                                                                                                                                   | ✅ history + sealed text send live; other types shapes from reference                                                                                                                                                                 |
| Sending Flex messages                                                                                                                                                                                                                       | ❌ a feature for official accounts / bots (LINE Messaging API); personal accounts get TalkException 11 "Incompatible app version" (live). Received ones show their alt text                                                           |
| TUI: stickers (recent ones from owned packs + owned packs; LINE rejects unowned stickers with HTTP 400), location, contact card, reactions, unsend, read receipts                                                                           | ✅ owned-pack listing live; sending these types mock/unit-tested, not sent live                                                                                                                                                       |
| LINE emoji reactions (`Client::owned_emoji_packages`, `react_emoji`, `ReactionKind::emoji`); TUI recent emoji + picker                                                                                                                       | ✅ pack listing and an emoji reaction sent live (own chat, 2026-11-15); TUI flow unit-tested                                                                                                                                          |
| Typed operations (`Operation::event()`): read marks, reads on other devices, unsend, reactions, membership                                                                                                                                  | ✅ layouts read off the live stream (2026-11-14)                                                                                                                                                                                      |
| Read status ("Read" / "Read by N")                                                                                                                                                                                                          | ✅ from live `NOTIFIED_READ_MESSAGE` ops (read times from their `createdTime`); `getMessageReadRange` is served but returned `[]` live, so earlier reads are inferred from later messages and reactions (unit-tested)                                                                                                        |
| Typing indicators                                                                                                                                                                                                                           | ⚠️ shown when op 62 arrives (never observed live); the Chrome protocol has no method to send them                                                                                                                                     |
| Local store (`rune::store`, SQLite): chats, decrypted messages, contacts, read marks, stream position, settings                                                                                                                             | ✅ unit-tested; TUI start-up from the store and resume live-tested                                                                                                                                                                    |
| Store: user cache (any mid's name/status/picture/cover/members) and profile picture/cover images                                                                                                                                            | ✅ unit-tested; not yet live-tested                                                                                                                                                                                                   |
| Notification settings: LINE's account switches (`settings_attributes` / `update_settings`), per-chat mute; desktop notifications in the TUI                                                                                                 | ✅ reading live; writing mock-tested only                                                                                                                                                                                             |
| Operation stream (SSE, resume cursor, reconnect)                                                                                                                                                                                            | ✅ live (connect, pings, replay from a cursor, auto-decrypt)                                                                                                                                                                          |
| Any of the 77 extension methods via `Client::call`                                                                                                                                                                                          | ✅ (`rune::endpoints::THRIFT_METHODS`)                                                                                                                                                                                                |
| Letter Sealing primitives (ECDH, V1, V2)                                                                                                                                                                                                    | ✅ V1 interop with the reference; V2 round-trip with real keys                                                                                                                                                                        |
| Keychain capture at login                                                                                                                                                                                                                   | ✅ live (keys persist and reload)                                                                                                                                                                                                     |
| Reading sealed (E2EE) messages, 1:1 and group                                                                                                                                                                                               | ✅ live: history 37/37, and live stream (official-app and phone messages)                                                                                                                                                             |
| E2EE failures typed, never dropped; key status/rotation handling; restart                                                                                                                                                                   | ✅ mock-tested with real keys; `e2ee-status` live                                                                                                                                                                                     |
| Sending sealed text/location, 1:1 and group (auto on code 82)                                                                                                                                                                               | ✅ 1:1 and group sent live (existing group key); creating a new group key mock-tested only                                                                                                                                            |
| Media download: images, video, audio, files (plain, sealed, bot URLs)                                                                                                                                                                       | ✅ live, incl. official-app sealed media                                                                                                                                                                                              |
| Media upload (sealed V2 + plain V1), image previews/resizing                                                                                                                                                                                | ✅ sealed image sent live; video/audio/file and plain flow mock-tested                                                                                                                                                                |
| Profile/group pictures, sticker images                                                                                                                                                                                                      | ✅ live                                                                                                                                                                                                                               |
| Profile covers (`profile_cover`, `download_cover`; video covers reported); TUI profile pages show them                                                                                                                                      | ✅ live (own + friends'); groups have none                                                                                                                                                                                            |
| Changing your cover (`set_profile_cover`, CLI `set-cover`, TUI `e` on your profile)                                                                                                                                                         | ⚠️ mock-tested only (would change the live profile); TUI picker and confirmation checked live, upload not run                                                                                                                         |
| Group albums: album posts (`Message::album_post`) and album events (`album_event`: rename/delete/items removed), album photo download (`download_album_media`, also via `download_media`); TUI album list per group                         | ✅ posts parsed and all photos downloaded live; events unit-tested from the extension's code                                                                                                                                          |
| Listing, creating or editing albums                                                                                                                                                                                                         | ❌ the gateway serves no album API (the extension has no album browser)                                                                                                                                                               |
| Animated stickers (`Message::is_animated_sticker`, `StickerImage::Animation` APNG); TUI plays them, GIFs and animated WebPs in the viewer (frame-by-frame decode at a fixed 12 fps, looping), videos and audio in mpv                       | ✅ metadata and APNG URL checked live; in-viewer playback unit-tested (frame decode, replace, loop, stop), not yet watched live against a real animated sticker; mpv playback unit-tested (mpv was not installed on the test machine) |
| Call history: `Message::call()` → `CallRecord` (1:1 + group, voice/video/live, duration); TUI/CLI show `📞 Voice call · 3:12`                                                                                                               | ⚠️ unit-tested only: no call record was left in the account's server-side history; metadata keys from the official apps, parsed leniently                                                                                             |
| Placing or answering voice/video calls                                                                                                                                                                                                      | ❌ not possible: the extension protocol has no call methods and LINE's call media stack is proprietary (TUI points incoming calls at the phone)                                                                                       |
| TUI plugins: sandboxed WebAssembly plugins add commands, `Alt` keys, views, forms, settings and storage; permissions approved per module (SHA-256); managed in Settings › Plugins ([docs/PLUGINS.md](docs/PLUGINS.md)) | ✅ unit- and UI-tested with the sample plugin and hand-written failing modules; not yet used in a live session |
| TUI plugins from Git (install, follow a branch or release range, pin a version, check for updates, uninstall) and hot reload of changed plugin files | ✅ tested against local repositories (needs `git`) and on-disk changes; not yet tried against a hosted repository or in a live session |
| E-mail/password login                                                                                                                                                                                                                       | ❌ not planned                                                                                                                                                                                                                        |

"Shapes from reference" means the request and response formats come from
OkLine, which reports testing them live; `rune` has not exercised those calls
itself yet. The first live run (2026-11-13) is summarized in
[docs/PROTOCOL.md](docs/PROTOCOL.md).

## Install

`rune-tui`, the terminal chat app, installs like any other command-line
program. You need a Rust toolchain (1.97 or newer; `rustup update`) and a C
compiler (SQLite is compiled in); Linux and macOS are supported.

```sh
git clone https://github.com/ThanapatFungpharit/Rune.git && cd Rune
make install                          # into ~/.local: no root needed
```

That installs

| File                                                  | What                                      |
|-------------------------------------------------------|-------------------------------------------|
| `~/.local/bin/rune-tui`                               | the program                               |
| `~/.local/share/man/man1/rune-tui.1`                  | `man rune-tui`                            |
| `~/.local/share/applications/rune-tui.desktop`        | *Rune* in your desktop's app launcher (opens in a terminal) |
| `~/.local/share/icons/hicolor/scalable/apps/rune-tui.svg` | its icon                              |

Then run `rune-tui` from any directory (if `~/.local/bin` is not on your
`PATH`, `make install` says so). The first start shows a QR code to scan
with the LINE app on your phone.

Other ways:

```sh
sudo make install PREFIX=/usr/local   # for every user (run `make` first, as yourself)
make uninstall                        # with the same PREFIX as the install
cargo install --locked --path . --features tui --bin rune-tui   # just the binary, into ~/.cargo/bin
```

Packagers: `make build`, then `make install PREFIX=/usr DESTDIR="$pkgdir"`
(installs the license to `share/licenses/rune/`; desktop and icon caches are
refreshed only when `DESTDIR` is empty).

rune-tui needs LINE's `ltsm.wasm`, which is not distributed with it: when the
LINE Chrome extension is installed in Chrome, Chromium, Brave, Edge or Vivaldi
it is found there; otherwise see [Getting `ltsm.wasm`](#getting-ltsmwasm).

Your data lives outside the install and is never touched by `make install`
or `make uninstall`:

| Path                                   | What                                                    |
|----------------------------------------|---------------------------------------------------------|
| `~/.local/share/rune/session.json`     | your login: tokens and Letter Sealing keys (`0600`)    |
| `~/.local/share/rune/session.db`       | decrypted messages, contacts, settings (`0600`)        |
| `~/.local/share/rune/plugins/`         | plugins                                                 |
| `~/.config/rune/keys.json`             | your keyboard shortcuts                                 |
| `~/.cache/rune/view/`                  | media you opened (pruned at start)                      |

(`$XDG_DATA_HOME`, `$XDG_CONFIG_HOME` and `$XDG_CACHE_HOME` move them; the
`rune` folder in the data directory is created owner-only at the first start.) A session path
given on the command line overrides the default, and a `./session.json` in
the current directory is still used while the default does not exist, so
setups from before rune could be installed keep their login.

## Usage

```rust
use rune::{Client, Config, QrLoginEvent, QrLoginOptions, Session};

#[tokio::main]
async fn main() -> rune::Result<()> {
    // First run: log in by QR and save the session.
    let client = Client::new(Config::default())?;
    let login = client
        .qr_login(QrLoginOptions::default(), |ev| match ev {
            QrLoginEvent::QrUrl(url) => println!("render as QR and scan: {url}"),
            QrLoginEvent::Pin(pin) => println!("enter PIN {pin} on your phone"),
        })
        .await?;
    Session { tokens: client.tokens(), e2ee_keys: login.e2ee_keys }.save("session.json")?;

    // Later runs:
    let client = Session::load("session.json")?.into_client(Config::default())?;
    let me = client.profile().await?;
    client.send_text(&me.mid, "note to self").await?;

    // Incoming operations, resuming after reconnects. Letter-Sealed
    // messages are decrypted with the keys stored in the session.
    let mut ops = client.operations(None);
    loop {
        let op = ops.next_operation().await?;
        if let Some(m) = &op.message {
            let m = client.decrypt_message(m).await?;
            println!("{} -> {}: {:?}", m.from, m.to, m.text);
        }
    }
}
```

`Client` is cheap to clone. Tokens can change on refresh, so call
`client.subscribe_tokens()` to persist the new ones (the CLI example shows
how). Long-running programs should also call `client.spawn_token_refresher()`
to renew the token ahead of time.

With the `store` feature, `rune::Store` keeps chats, decrypted messages,
contacts, read marks and the stream position in SQLite:

```rust
let me = client.my_mid().unwrap();
let store = rune::Store::open("rune.db")?; // created 0600
store.bind_account(&me)?; // wipes another account's data
let mut ops = client.operations(store.stream_revision()?); // resume where we stopped
loop {
    let op = ops.next_operation().await?;
    // One transaction: the message, the chat row and the stream position.
    store.apply(&op, &me, |m| m.text.clone().unwrap_or_default())?;
    match op.event() {
        rune::OpEvent::Read { chat, reader, message_id } => { /* "Read by" */ }
        rune::OpEvent::Reaction { message_id, from, reaction, .. } => { /* … */ }
        _ => {}
    }
}
```

### Terminal chat app (`rune-tui`)

```sh
cargo run --release --features tui --bin rune-tui            # default session (see Install), QR login if missing
cargo run --release --features tui --bin rune-tui -- --no-read-receipts
cargo run --release --features tui --bin rune-tui -- --no-graphics   # draw pictures with text only
cargo run --release --features tui --bin rune-tui -- --no-mouse   # keyboard only
cargo run --release --features tui --bin rune-tui -- --no-graphics --cells sextant   # sharper text pictures
cargo run --release --features tui --bin rune-tui -- --log rune.log   # diagnostics (RUST_LOG=rune=debug for more)
cargo run --release --features tui --bin rune-tui -- --help   # every option
```

Unknown or misspelt options are refused rather than ignored (a mistyped
`--no-read-receipts` must not quietly send read receipts). Only a missing
session file starts a QR login; a damaged or unreadable one is reported and
left alone, since it holds the device certificate and your keys. If saving a
renewed token fails, the status line says so. `--log PATH` appends
diagnostics to a 0600 file: no tokens or message text, but chat and user
ids (and request paths at `debug` level).

Chat list (invitations first, then the active pin list's chats in its order,
then recent activity, with unread counts and decrypted previews) on the left, the
conversation on the right, an input line at the bottom; new messages arrive
live from the operation stream. Sealed chats work transparently (decrypt on
receive, seal on send). Images, video previews and stickers appear as inline
thumbnails; a viewer shows them full-size, and any media can be saved or opened.
Text messages sit in rounded bubbles: yours on the right, everyone else's on
the left, word-wrapped to three quarters of the pane.
Pinned chats live in named **pin lists**: each LINE account has its own lists (a `Default` list holds pins made before
lists existed), one of them is active,
and each keeps its own order. *Pin lists…* in the main menu (`Ctrl-x`) or
Settings › Chats switches between them, creates, renames and deletes them; pinning, unpinning and
`K`/`J` act on the active list only. A pinned chat with unread messages
moves to the top of the pinned section until it is read, then returns to its
place (several unread pins keep their pin order). Other unread chats (not
muted ones) likewise sit right under the pins, newest first, until read, then
drop back to where their last activity puts them. Unread chats scrolled out of
view are counted on the list's border (`▲ n unread` above, `▼ n unread` below;
click one to jump there), and `u` selects the nearest unread chat. Lists and the
active choice are stored locally and survive restarts.
Links are underlined and open in your browser on click (or `Enter` on
the selected message). Pictures are scanned for QR codes; what a code says is
shown under the picture and in the viewer, and QR links open the same way.

Profiles look like LINE's: the cover photo fills the page as its
background (darkening towards the bottom so text stays readable) with the
round avatar set into it, name and status message, Chat, Photo and Cover buttons, and
members for groups (pick one to see their profile). Open one with `p` on a
chat or message, `Ctrl-p` for the open chat, *My profile* in the main menu
(`Ctrl-x`) or Settings › Account for your own, or by clicking the chat title or a sender's name. `←`/`→` pick a button
and `Enter` presses it (or click): *Cover* opens the full cover, and on your
own profile *Change cover* picks a new cover picture in the file browser and
asks before replacing it. Groups (which have no
cover) and users with LINE's default cover get the profile picture, blurred,
instead. A profile seen before opens at once from the store and is then
refreshed; picture and cover download side by side.

**Albums.** When someone creates an album or adds photos to one, the chat
shows "🖼 added to the album «…»" with the photo inline; it opens in the
viewer and saves like any picture. The group menu's *Albums* (or `/albums`)
lists every album the chat's history mentions with its photos; picking one
shows it in the viewer. LINE's Chrome protocol cannot list an album's full
contents, so an album shows the photos that were announced in the chat (LINE announces the first few photos of a batch).

Pictures, inline and in the viewer, use the terminal's own image protocol at
full resolution: Kitty, Sixel or iTerm2, detected at start-up (WezTerm and
Konsole get iTerm2). Inline images load LINE's main picture (≤1280 px) rather
than its preview, which is at most 384 px and heavily compressed; videos show
the preview frame. If a terminal misreports support and draws garbage, run
with `--no-graphics`.

Without an image protocol (or with `--no-graphics`), pictures are drawn with
text: each cell gets the block glyph and pair of colours that best match its
pixels. `--cells` picks the glyphs: `quadrant` (default, 2×2 pixels per cell,
works in every terminal font), `sextant` (2×3, for terminals that draw
Unicode's legacy-computing sextants) or `half` (1×2 half-blocks). Transparent
sticker pixels show the terminal background. For the original file, press
`o` to open it in your image viewer. Pictures
are scaled with Lanczos (thumbnails) / Catmull-Rom (viewer) rather than
nearest-neighbour, which looked noisy.

When the original of an old picture or video is no longer on LINE's server,
the viewer falls back to its preview and says so; `s`/`o` then save the
preview as `…-preview.jpg`. Expired files (LINE deletes attachments after a
while) are reported as such. On this device LINE served about two weeks of
history, in which every image still had its original, so the preview
fallback has not been seen live yet.

Navigation is "Enter forward, Esc back": `Enter` opens a chat, after which
you type straight away; `↑` steps up into the messages and `↓` past the
newest one (or just typing) returns to the input line. The mouse works too —
click a chat or a picture, scroll with the wheel. Pass `--no-mouse` to leave
the mouse to the terminal (Shift-drag selects text in most terminals anyway);
`c` on a selected message copies its text.

`Enter` sends and `Shift-Enter` starts a new line (`Alt-Enter` or `Ctrl-j`
where the terminal does not report `Shift-Enter`). Nothing half-written
goes out by accident: pasted text keeps its line breaks and waits for your
`Enter` — also in terminals without bracketed paste, which type a paste out
as keys (a burst of keys read at once is a paste, and its `Enter`s are line
breaks); several lines are never taken for a `/command`; a `/word` that is
not a command is never sent, whatever follows it (start with `//` to send
it as text; a path like `/usr/bin` is text); and each chat
keeps its own draft (and reply), shown as `✎ …` in the chat list and kept
across restarts, so switching chats never sends one chat's text to another.

Navigation has three levels, each one key away from anywhere in the main
view (also while typing):

- **Main menu** (`Ctrl-x`): everything that is not about one chat or message
  — the open chat's menu, send something, find a chat or friend, new group,
  your profile, pin lists, read receipts, settings, plugins, the shortcuts,
  quit. Every entry shows its own shortcut, so the menu also teaches them.
- **The chat's menu** (`m` in the chat list, `Ctrl-g` in a chat): profile,
  search in chat, albums, send, mute, pin, then invite, rename and leave for groups.
- **The message's menu** (`m` on a selected message or in the viewer).

**Settings** (`Ctrl-s`) is where preferences and management live, plugins
included; a screen opened from a settings row (a form, the pin lists, a
plugin's page, a confirmation) comes back to Settings when it closes.
The message line's `/` commands only act on the open chat — screens are
opened with keys and menus, so typing never navigates by accident.

`?` (or `F1`, which also works while typing) lists every key of the screen
you are on; the status line starts with `Ctrl-x menu` and shows the common
keys that apply right now. A message on the status line stays while you are
away; once it has been up for 8 seconds (30 for a failure), your next key
clears it. Letters are only bound where nothing is typed and
only for frequent actions; everything else is in a menu, by name.
Consequential actions (unsend, leave, decline, delete) ask first, with the
*Cancel* button highlighted.

Every key below is a default: **Settings › Shortcuts** lists each screen, and
on a screen's page `Enter` takes the next key you press as the new one (`a`
adds a key beside the others, `Delete` puts the default back). A key that
already does something else there is moved only when you say so; `Alt` with a
letter or digit stays with plugins, and letters are refused where they would be
typed as text. Menus, hints, help screens and messages always name the keys
as you have them. Changes are kept in the local store and in
`~/.config/rune/keys.json` (`$XDG_CONFIG_HOME` if set) — only what differs from
the defaults, as `"messages.reply": ["ctrl-t"]` (each shortcut's name is shown in
the editor). Edit that file by hand or copy it to another machine: it is read at
start, where it wins over the store, or with *Read keys.json again*; a file
rune cannot read is left alone, never overwritten. `--default-keys` starts
with the defaults for one run (yours stay saved), and the editor's own keys
cannot be changed, so a mistake can always be undone.

| Key / mouse / command                                                            | Action                                                                                                                                                          |
|----------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Ctrl-c`                                                                         | quit                                                                                                                                                            |
| `?`, `F1`                                                                        | the keys of the current screen                                                                                                                                  |
| `Ctrl-x`                                                                         | main menu: chat menu, send, find, new group, my profile, pin lists, read receipts, settings, plugins, shortcuts, quit                                           |
| `Ctrl-s`                                                                         | settings: notifications (this computer and LINE's), chats, shortcuts, account, plugins, storage                                                                |
| `↑` `↓` / `j` `k`, wheel                                                         | move in the chat list / message list                                                                                                                            |
| `PgUp` `PgDn` (chat list, people picker, settings, a group's members)            | a page up / down                                                                                                                                                |
| `Home` `End` (chat list, messages, menus, settings, a group's members)           | first / last item; in messages `End` selects the newest message; on a profile `Home` goes back to the buttons                                                   |
| chat list: `↓` / wheel to the last chat                                          | load more chats: older and hidden ones the start-up list leaves out (100 per page, until all are listed)                                                        |
| `Enter`, click                                                                   | open the chat · send the message · view the selected media (GIFs and animated stickers play in the viewer); videos, audio and audio/video files **play in mpv** |
| chat list: `Enter` on an invitation                                              | join or decline (invitations are listed first, in yellow)                                                                                                       |
| chat list: `/`, `Ctrl-f` anywhere                                                | search chats (by name or member) and friends, best match first; `↓` at the end searches older chats; `Esc` stops                                                |
| chat list: `m`, `Ctrl-g` in a chat                                               | the chat's menu: profile/members, search in chat, albums, send, mute, pin/unpin, move pin, invite, rename, leave                                               |
| chat list: `p`, `Ctrl-p` in a chat                                               | the chat's profile                                                                                                                                              |
| `Ctrl-r`                                                                         | read receipts on / off (saved; read chats without others seeing it)                                                                                             |
| chat list: `n`                                                                   | new group (name, then pick friends)                                                                                                                             |
| chat list: `K` / `J`                                                             | move the selected pinned chat up / down                                                                                                                         |
| `Tab`                                                                            | jump between the chat list and the open chat                                                                                                                    |
| `Esc`                                                                            | back: viewer → messages → typing → chat list                                                                                                                    |
| typing: `←` `→` `Home` `End` `Delete` `Backspace`                                | edit the message at the cursor (`Home` / `End`: start / end of the line)                                                                                        |
| typing: `Ctrl-←` `Ctrl-→` `Ctrl-w` `Ctrl-u` `Ctrl-k`                             | previous / next word, delete the word before the cursor, delete to the start / end of the line (every text field)                                               |
| typing: `Shift-Enter` / `Alt-Enter` / `Ctrl-j`                                   | new line: the message line grows with the text and `Enter` sends all of it as one message                                                                         |
| typing: `↑` `↓`                                                                  | move between the lines; `↑` on the first line selects messages (starts at the newest)                                                                             |
| messages: `↓` past the newest, or any other letter                               | back to typing                                                                                                                                                  |
| typing / messages: `Ctrl-o`                                                      | send a photo/video/file, sticker, location or contact card                                                                                                      |
| paste or drop a file path                                                        | offers to send that file                                                                                                                                        |
| messages: `r`                                                                    | reply to the selected message (input shows the quote; `Esc` cancels)                                                                                            |
| messages: `e`                                                                    | react: 👍 ❤️ 😂 🤩 😢 😱, recent LINE emoji, or *More LINE emoji…* for every emoji of your packs; again to change, "remove" to take it back |
| messages / viewer: `s`                                                           | save to `~/Downloads/rune/` (original file name, never overwrites)                                                                                              |
| messages / viewer: `o`                                                           | open (from `~/.cache/rune/view/`, not Downloads; emptied of files older than 7 days or beyond 1 GiB at start-up): motion media in mpv, everything else with the system app; file types that can run programs (`.desktop`, scripts, `.jar`, executables, installers) are asked about first |
| messages: `p`                                                                    | the sender's profile                                                                                                                                            |
| messages: `x`                                                                    | unsend one of your messages (asks first)                                                                                                                        |
| messages: `/`, chat menu › *Search in chat…*                                     | search the chat's messages stored on this computer (text, without case; newest first, as you type); `Enter` shows the message in the chat (`/` again: the same query) |
| messages: `c`                                                                    | copy the message's text (through `wl-copy`, `xclip`, `xsel` or `pbcopy`, else the terminal's OSC 52 — which also works over SSH where the terminal allows it)  |
| messages / viewer: `m`                                                           | every action for the message: reply, react, reactions, read by, unsend, view, save, open, copy, go to the quoted message, sender's profile                      |
| viewer: `←` `→` / `h` `l`, wheel                                                 | previous / next media in the chat                                                                                                                               |
| viewer: `Space`                                                                  | play a video or audio clip in mpv; pause / resume a GIF or animated sticker                                                                                     |
| viewer: `u`                                                                      | open the link in the picture's QR code                                                                                                                          |
| viewer: click outside, right-click                                               | close                                                                                                                                                           |
| `PgUp` `PgDn`, wheel                                                             | scroll the conversation                                                                                                                                         |
| `PgUp` / wheel at the top                                                        | load older messages (store first, then LINE)                                                                                                                    |
| `/save PATH`                                                                     | save the most recent picture/video/audio/file of the chat (`~` expands; an existing PATH is refused, never overwritten)                                         |
| `/attach [PATH]` (`/image`, `/file`), `/sticker`, `/location`, `/contact`        | open the matching dialog                                                                                                                                        |
| `/invite`, `/rename`, `/albums`, `/leave` (groups), `/mute`, `/unmute`           | the open chat's actions                                                                                                                                         |
| `F1` while typing                                                                | the shortcuts, followed by every `/` command (plugins' included)                                                                                                |
| `Ctrl-x` › *Plugins*, `Alt-<letter>`                                             | manage plugins in Settings › Plugins (install from Git, enable, configure, update, uninstall); plugin keys (rune itself never binds `Alt` letters)               |

**Plugins.** Plugins are WebAssembly modules in `~/.local/share/rune/plugins/<id>/`
(`--plugins DIR` elsewhere, `--no-plugins` to start none). Installing one runs
nothing: Settings › Plugins (`Ctrl-x` › *Plugins*) lists them, and a plugin starts only
once you enable it and approve what it asks for (read messages, send messages,
type into the message line, notify, store data). They run sandboxed with
memory and time limits; one that crashes is stopped and marked failed while
rune carries on. *Install from a Git repository…* (Settings › Plugins) clones a
repository holding a built plugin (`plugin.json` next to its `.wasm`; rune never
builds one), checks it without running it and installs it, off; its page then
offers *Check for a new version*, *Uninstall* and *Updates*: follow a branch, the newest
release (or the newest compatible one, `^1`), or pin it to one tag or commit — any listed
version can be chosen, older ones too (*Check all for new versions* does every plugin at
once; pinned ones only report newer releases). Changed plugin files are
noticed while rune runs: a running plugin's new version loads beside it once
approved (or by itself, if you allow that on its page and it asks for no new
permissions), and files that do not work leave the running version alone.
The sample `plugins/snippets` adds `/snip` and `Alt-s` for
reusable text (`scripts/plugin.sh install plugins/snippets`). Writing one:
[docs/PLUGINS.md](docs/PLUGINS.md).

**Sending things.** `Ctrl-o` → *Photo, video or file* opens a file browser (type to filter, `~` home, `.` hidden files)
with a preview of pictures;
choosing a file shows a send dialog that picks photo/video/audio/file from
the extension (`f` sends it as a plain file instead, `o` sends photos in
original quality with location and personal metadata removed). MP4/M4A
lengths are read from the file; videos go without a preview frame. The
sticker picker shows stickers seen in your chats plus the packs this
account owns; the arrow keys carry on into the next (or previous) pack
past a pack's edge, in the LINE emoji picker too. Locations take a title, an address and coordinates (paste
"13.7466, 100.5347" into the latitude field). Replies work for text and
stickers.

**Read status, reactions, typing.** Your messages show `✓ Read` (1:1) or
`✓ Read by N` (groups) as read receipts arrive. LINE returns no read
history, so for earlier messages rune also counts a member who wrote a
message or reacted afterwards as having read them. *Read by…* in the
message's menu lists the readers (with when they read it, or "wrote later" /
"reacted") and the group members who have not, and updates while open.
Reactions appear as chips under messages, yours highlighted. A reaction shows at once; if LINE refuses it, the previous one comes back and the status line says why. Besides
LINE's six built-in reactions you can react with any emoji of the LINE
emoji packs you own (every account has the 211 of "LINE Basic Emoji"):
`e` lists your recent ones, and *More LINE emoji…* opens a picture
picker like the sticker one. LINE emoji are drawn as their Unicode
look-alike where there is one (👍, 🔥, 🥹…) and as ✨ otherwise (the LINE
characters); *Reactions…* names them ("✨ Alice (heart eyes Moon)"). Unsent
messages turn into "message unsent". When LINE sends a typing notification
the chat title says so; rune cannot send one (the Chrome protocol has no
such method).

**Local store and sync.** The TUI keeps chats, messages (decrypted),
friends, read marks, settings and the event-stream position in a SQLite
file next to the session (`session.db`, mode 0600; `--db PATH` to move it,
`--no-store` for none). If a write to it fails (a full disk, say), the status
line says so. Start-up shows the stored chat list immediately,
then refreshes it from LINE and replays every operation since the stored
position, so messages, reads, reactions and unsends that happened while it
was closed are caught up. Opening a chat reads the store and only asks
LINE when the chat has messages the store lacks (joining up gaps of up to
400 messages; larger gaps drop the stale part rather than leave a hole).
The access token is renewed on a timer ahead of expiry, not only when LINE
rejects it.

**Groups.** Create a group (`n`), invite friends, rename, mute, look at
members and leave from the chat menu; `Enter` on an invitation joins or
declines it. Joining by invitation link or QR code is not
possible: the Chrome gateway does not serve the ticket methods.

**Settings** (`Ctrl-s`, or `Ctrl-x` › *Settings*): *Notifications* — on this
computer (desktop notification, terminal bell
or off; message text; groups; muted chats never notify; a test button; a 1:1 chat
is titled with the sender, a group with the group and `Sender: text` / `Sender sent a photo`,
with the sender's profile picture as the icon under `notify-send`), then LINE's own notification
switches (they apply to your phone too); *Chats* (read receipts, automatic pictures, inline picture size,
colour theme: automatic, dark or light, pin lists); *Shortcuts* (every screen's keys, each
changeable; see above); *Account* (display name, status message,
your profile page); *Plugins* (every installed plugin and its state, and installing from Git;
`Enter` opens a plugin's page, `Esc` comes back); *Storage* (database path,
counts, stream position, clear local data). Clearing deletes the stored chats
and messages from the database file itself (SQLite `secure_delete` plus a
log checkpoint) and empties `~/.cache/rune/view/`; what you saved to
Downloads stays.

Opening a chat marks it as read (others see the read receipt) unless you
pass `--no-read-receipts` or turn read receipts off (`Ctrl-r` anywhere, or in
the settings or the main menu; a yellow *receipts off* badge sits on the chat list and the message line while they
are). With them off a chat also stays unread here — its count, bold name and
*Unread Messages* divider remain, as they do on your phone — until it is read
with receipts on or on another device.
Messages that arrive in the open chat while the terminal is in the
background are marked read when you return to it (in terminals that report
focus changes; others count as always focused).
What you read with read receipts turned off is remembered (per chat, across
restarts) and marked read once you turn them back on *while in that chat*, or
next open it with them on; a read on another device, or the message being
unsent, settles it. Read receipts that fail to send are retried with backoff
until LINE acknowledges them, but never while receipts are off.
`--no-read-receipts` counts as off: reads are remembered the same way and
sent only in a later run with receipts on, once you are in that chat.

**GIFs, animated stickers (APNG) and animated WebPs play in the viewer**:
a background thread decodes one frame at a time and replaces the picture on
screen with it on a fixed 12 fps tick, starting over after the last frame.
Frames keep their own timing on that grid: a frame shorter than a tick is
dropped, so fast GIFs keep their speed (and a slow terminal skips frames).
`Space` pauses.

**Video and audio play in [mpv](https://mpv.io)** (install it
from your package manager): the file is written to `~/.cache/rune/view/`
(Downloads only gets what you save with `s`) and opened in its own mpv window (audio gets one too, so it can be closed);
mpv never touches rune's terminal. Without
mpv, rune falls back to the system app and says so. Not there yet:
video inside the terminal itself; video preview frames when sending.

### Development CLI

```sh
cargo run --release --example cli -- login      # QR in the terminal, writes session.json
cargo run --release --example cli -- whoami
cargo run --release --example cli -- refresh    # renew the token now
cargo run --release --example cli -- chats
cargo run --release --example cli -- invites    # pending group invitations
cargo run --release --example cli -- accept <chat-mid>   # or: decline <chat-mid>
cargo run --release --example cli -- send <mid> "hello"
cargo run --release --example cli -- reply <chat-mid> <message-id> "text"   # ids from `recent`
cargo run --release --example cli -- recent <chat-mid> 20
cargo run --release --example cli -- watch [REV]       # decrypts sealed messages; REV replays from a revision
cargo run --release --example cli -- e2ee-status      # are our keys still current?
cargo run --release --example cli -- send-image <mid> photo.jpg
cargo run --release --example cli -- download <chat> <message-id> out.jpg [preview]
cargo run --release --example cli -- picture me me.jpg
cargo run --release --example cli -- sticker 403 sticker.png
cargo run --release --example cli -- emojis           # LINE emoji packs usable as reactions
cargo run --release --example cli -- call talk/thrift/Talk/TalkService/getServerTime '[]'
```

Set `RUST_LOG=rune=debug` to see request logs. Logs never include tokens,
and `Tokens`' `Debug` output is redacted.

## Security notes

- `session.json` holds the access and refresh tokens, the device certificate
  and your Letter Sealing **private keys**. `Session::save` writes it with mode
  `0600`. Never commit it (it is in `.gitignore`).
- The TUI's `session.db` (next to the session) holds your messages decrypted (mode `0600`).
  Deleted rows are overwritten (`secure_delete`), and `~/.cache/rune/view/`
  (copies of media you opened, folder `0700`) is pruned at start-up. Media
  you save goes to `~/Downloads/rune/` with your usual file permissions.
- Media links in messages (`DOWNLOAD_URL` / `PREVIEW_URL`) are chosen by
  their sender. rune fetches them only over `https` from public hosts (not
  `localhost` or private, loopback or link-local addresses), never with
  credentials, and caps every download (20 MiB for pictures, 1 GiB plus
  overhead for files). Requests carrying the access token follow redirects
  to other hosts only within `*.line-apps.com` / `*.line-scdn.net`. A host
  name that *resolves* to a private address is not caught.
- The secure module is LINE's binary (`ltsm.wasm`, from extension 3.7.2,
  sha256 `58bb4e18…`). rune does not ship it; see
  [Getting `ltsm.wasm`](#getting-ltsmwasm). It runs sandboxed in wasmtime.
  Its only host access is to random bytes and to a fixed origin string.

## Getting `ltsm.wasm`

rune signs every request with LINE's own secure module but does not
distribute it, because it is LINE's code. Each user supplies the copy from
their own LINE Chrome extension, version 3.7.2 (sha256
`58bb4e189ab9bbd7d72ed415258da36afd50e306cb019bab75329d4f5f1b65b3`; other
builds are refused, since the host is written for this one). rune looks, in
order, at:

1. `$RUNE_LTSM`, if set (then nowhere else);
2. `$XDG_DATA_HOME/rune/ltsm.wasm` (`~/.local/share/rune/ltsm.wasm`);
3. `assets/ltsm.wasm` in the current directory (development; gitignored);
4. the installed extension in Chrome, Chromium, Brave, Edge and Vivaldi profiles.

Without it, `rune-tui` shows where it looked and exits; the library's
`Ltsm::new` / `Client::new` return an error (`rune::ltsm::load_wasm` gives
the typed `WasmProblem`). The tests need it too, as `assets/ltsm.wasm`.

## Tests

```sh
cargo test --features tui                    # unit (library + TUI), mock-gateway, signing vectors
cargo clippy --all-targets --features tui    # kept at zero warnings
OKLINE_LTSM_DIR=/path/to/OkLine/okline/ltsm \
  cargo test --test ltsm v1_interop -- --ignored   # Node <-> Rust Letter Sealing interop
RUNE_SESSION=session.json \
  cargo test --test ltsm real_keys -- --ignored    # V2 with your real keys (local only)
RUNE_SESSION=session.json RUNE_TEST_GROUP=<a group mid> \
  cargo test --test e2ee_mock -- --ignored          # sealed send/decrypt flows, mock gateway
RUNE_SESSION=session.json \
  cargo test --test e2ee_lifecycle -- --ignored     # failures, rotation, restart
RUNE_SESSION=session.json \
  cargo test --test media_mock -- --ignored         # sealed media upload/download
```

## Layout

| Path                  | What                                                                      |
|-----------------------|---------------------------------------------------------------------------|
| `src/ltsm/`           | wasmtime host for `ltsm.wasm` (`runtime.rs`) and its typed API (`mod.rs`) |
| `src/transport.rs`    | HTTP, headers, signing, envelope, retries, token refresh                  |
| `src/client.rs`       | typed operations                                                          |
| `src/auth.rs`         | QR login                                                                  |
| `src/e2ee.rs`         | Letter Sealing: framing, 1:1/group keys, seal/decrypt                     |
| `src/media.rs`        | media upload/download, blob encryption, pictures, stickers                |
| `src/events.rs`       | SSE operation stream                                                      |
| `src/models.rs`       | message builders, typed responses                                         |
| `src/session.rs`      | session persistence                                                       |
| `src/store.rs`        | SQLite message store (feature `store`)                                    |
| `src/ops.rs`          | typed operations (`Operation::event()`)                                   |
| `src/timeline.rs`, `src/album.rs` | profile covers, group album posts and photos                  |
| `src/exif.rs`         | lossless metadata scrubbing for images sent as originals                  |
| `src/bin/rune-tui/`   | terminal chat app (feature `tui`): `main.rs`, `graphics/`, `overlays/`    |
| `docs/PROTOCOL.md`    | protocol findings and what is verified                                    |

## Why is it called a `rune`?

It's because Ru(st) + (Li)ne = Rune.
