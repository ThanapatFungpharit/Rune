# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`rune` is an unofficial Rust client for LINE that speaks the protocol of the
LINE **Chrome extension** (`CHROMEOS` 3.7.2): a library (`src/`), a terminal chat
app (`src/bin/rune-tui/`, feature `tui`) and a dev CLI (`examples/cli.rs`).
The reference implementation is OkLine (Python, github.com/NiceDayZc/OkLine);
`docs/PROTOCOL.md` records every protocol fact with its provenance — keep it
updated, and keep the README's status table honest about what is live-verified
vs. mock-tested.

## Commands

```sh
cargo build                                    # library + CLI example
cargo build --release --features tui --bin rune-tui
cargo clippy --all-targets --features tui      # kept at zero warnings
cargo fmt                                      # rustfmt.toml: max_width = 110
cargo test --features tui                      # unit, mock-gateway, signing vectors (no network)
cargo test --test client_mock must_refresh     # one integration test by name filter
cargo test --lib media::                       # unit tests of one module
```

Opt-in (`#[ignore]`) tests need a real logged-in `session.json` (never committed)
because `ltsm.wasm` rejects made-up mids — real-format mids are MAC-protected:

```sh
RUNE_SESSION=session.json cargo test --test e2ee_lifecycle -- --ignored
RUNE_SESSION=session.json cargo test --test media_mock -- --ignored
RUNE_SESSION=session.json RUNE_TEST_GROUP=<group mid> cargo test --test e2ee_mock -- --ignored
OKLINE_LTSM_DIR=<OkLine>/okline/ltsm cargo test --test ltsm v1_interop -- --ignored   # needs node
```

Running things against the real account: `cargo run --release --example cli -- <cmd>`
(`login`, `whoami`, `chats`, `recent`, `watch [REV]`, `call METHOD JSON`, `download`, `cover`, `albums`,
`send-image`, …) and `./target/release/rune-tui [--no-read-receipts] session.json`.
Anything that sends messages, marks chats read (opening a chat in the TUI does,
unless `--no-read-receipts`) or registers group keys is visible to other people.

Dev profile builds all dependencies with `opt-level = 2` because Cranelift compiling
the 2.3 MB wasm unoptimized takes ~15 s per `Ltsm::new()`.

## Architecture

**Signing / crypto via LINE's own wasm.** Every gateway request needs an
`X-Hmac` computed inside `assets/ltsm.wasm` (embedded with `include_bytes!`).
`src/ltsm/runtime.rs` hosts it in wasmtime by re-implementing the 40 minified
Emscripten imports from `ltsmSandbox.js` (`a.a`…`a.N`): syscall stubs,
`EM_ASM` random snippets (addresses hard-coded), a fake `window.location.origin`,
Embind class/method registration and a small `emval` table. `src/ltsm/mod.rs`
is the typed API (`hmac`, Curve25519 keys, ECDH channels, V1/V2 Letter
Sealing, keychain unwrap/export). The import mapping is specific to this wasm
build — a new `ltsm.wasm` means re-deriving it. Byte-exactness is pinned by
`tests/fixtures/hmac_vectors.json` (from `scripts/gen_hmac_vectors.js`, run
against OkLine's Node bridge). After a trap, `invoke` drops emval handles
created during the call (C++ destructors never run).

**Request path.** `Client` (cheap `Clone`, `Arc<Transport>`) → `Transport::call`:
`POST /api/<ns>/thrift/<Ns>/<Svc>/<method>` with a JSON array of positional args,
signed over the exact bytes sent, re-signed on every retry/replay because the
key depends on the access token. Responses must be `{"message":"OK","data":…}`;
errors carry an outer gateway code wrapping an inner `TalkException` code (`error.rs::parse_error`). TalkException 119 →
`/api/auth/tokenRefresh` → replay
once; token changes are broadcast on a `watch` channel (`subscribe_tokens`) so
callers persist `Session`. int64s often arrive as JSON strings — models use
lenient deserializers and keep unknown fields in `extra`.

**Letter Sealing (`src/e2ee.rs`).** Keys only arrive with the keychain at QR
login (`auth.rs` unwraps and exports them into `Session.e2ee_keys`). 1:1 uses
`negotiateE2EEPublicKey`; groups use a shared key unwrapped per member (and
registered via `registerE2EEGroupKey` if missing). `send_message` sends plain
first; TalkException 82 marks the chat sealed and re-sends sealed. Decrypt
never mutates its input and returns typed `E2eeErrorKind` failures; the event
stream attaches failures to `Operation::e2ee_error` instead of dropping events.
The server stops serving retired public keys, so our own keys' public halves
are served locally.

**Media (`src/media.rs`).** Everything follows the extension code bundled in
`ltsmSandbox.js` (minified names cited in comments). OBS requests use an
encrypted access token (`acquireEncryptedAccessToken(2)`) plus
`X-Line-Application`, no `X-Hmac`. Sealed media: HKDF/AES-CTR/HMAC blob (`tests/fixtures/blob_vectors.json`, generated
by `scripts/gen_blob_vectors.js`
from the extension's WebCrypto algorithm), uploaded to `/r/talk/em{i,v,a,f}/reqid-…`;
the server's `x-obs-oid` header becomes `OID`; `ENC_KM`/`FILE_NAME` move inside
the ciphertext. `determineMediaMessageFlow` decides sealed (V2) vs plain (V1).
`src/exif.rs` scrubs metadata losslessly from images sent as originals; a file it
cannot parse (`scrub` → `None`) is re-encoded, never sent with its metadata.

**Timeline / albums (`src/timeline.rs`, `src/album.rs`).** `/api/timeline/`
gateway calls and `/r/myhome/` OBS objects use a channel token (`issueChannelToken("1341209850")`) instead of the
encrypted access token;
the gateway serves only `getCover`/`updateCover`/`homeId`. Albums have no
API: album posts (content type 16, `serviceType` `AB`) are parsed from
chats, and their photos come from `/r/album/a/{oid}` with the channel token
plus `X-Line-Mid: <chatId>` (found live; `download_media` handles them).

**Events (`src/events.rs`).** SSE `GET /api/operation/receive` with a `localRev`
cursor; reconnect backoff, 30 s silence watchdog (server pings every 20 s),
auto-decrypt, `note_operation` cache invalidation. Futures must stay `Send`
(a test asserts it) so the stream can be `tokio::spawn`ed.

**Typed ops (`src/ops.rs`).** `Operation::event()` decodes `param1..3` into
`OpEvent` (read marks, unsend, reactions, membership…); layouts were read off
the live stream and are tabled in PROTOCOL.md. Message ids compare numerically (`message_id_cmp`).

**Store (`src/store.rs`, feature `store`, bundled SQLite).** Chats, decrypted
messages (JSON of `Message`), contacts, `users` (last-seen name/status/picture/cover/members
of any mid, friend or not — names and profile pages load from it first), `images`
(profile pictures and covers by picture path / cover OBS path, pruned to 256 MB at
start). Opening or playing anything (profile picture, cover, message media,
sticker animation) writes to `~/.cache/rune/view`; only `s`/`/save` write to Downloads, read marks, `meta` key/values
(`stream_rev`, `owner`, `setting.*`, `cache.*`). Created 0600; `bind_account`
wipes another account's chat data but preserves every profile's pins. `profiles`
stores account names and last-use times. Pins live in named pin lists (`pin_profiles`, schema 3): each account has ≥1,
exactly one `active`, and the
pin methods act on the bound account's active list; pins are ordered per list.
Account profiles and pin lists are separate concepts.
`apply(op)` folds an operation in and advances
the stream position (never backwards). Writes are idempotent by message id.

**Timed refresh.** `Client::run_token_refresher` sleeps in ≤60 s slices
against the wall clock until shortly before the token's refresh time, and
restarts its wait whenever `subscribe_tokens` fires.

**TUI (`src/bin/rune-tui/`).** `main.rs` holds the `App` and the event loop;
`graphics/` holds picture code (`picture`, `piccache`, `cells`, `anim`, `avatar`),
`overlays/` the screens drawn over the chat (`modal`, `attach`, `stickers`,
`settings`, `profile`, `albums`); `input`, `keymap`, `links` sit
beside `main.rs`. Child modules `use crate::*` and `main.rs` re-imports each by
its short name, so `picture::…`, `modal::…` paths work everywhere. Single `App` state
driven by a `tokio::select!` over crossterm events and an `mpsc` of `AppEvent`s;
all network work runs in spawned tasks that post results back. Pictures use
ratatui-image with the detected terminal protocol (Kitty/Sixel/iTerm2) or,
with `--no-graphics`, custom block-glyph rendering in `cells.rs`; `avatar.rs`,
`profile.rs`, `picture.rs`, `links.rs`, `albums.rs` hold avatars, profile
pages (with covers), picture decoding, link/QR handling and the per-group
album list; overlays blank their box with `picture::cover` (not a bare
`Clear`): Sixel/iTerm2 graphics live in one anchor cell with the rest marked
skip, so while an overlay is open `picture::compose` lays the frame out once
without graphics, then draws pictures under a higher `Layer`'s box as text
cells (or not at all when fully covered); `modal.rs` (menus, forms, pickers, confirms and
the `Action` dispatcher), `attach.rs` (file browser + send dialog),
`stickers.rs`, `settings.rs` (prefs in the store, notifications) are the
overlays — an open `App::modal` takes every key. `keymap.rs` holds every binding
per `Ctx` (one per screen/overlay, plus shared text-editing and `?` tables);
`App::available` filters the status-line hints by what the selection allows, and
the keymap tests pin one meaning per key and no redundant alternatives. `keymap::COMMANDS`
lists the message line's `/` commands for its help screen — keep it in step with `App::submit`.
`input.rs` is the cursor-editing field (multi-line on the message line: word-wrapped
rows, `↑`/`↓` between them; Shift/Alt-Enter or Ctrl-j insert a line break); every text field (message line, forms,
chat search, people picker, file filter) edits through `TextInput::edit` with the
shared `TEXT_EDIT` keys (the file browser switches to `Ctx::AttachFilter` while a
filter is typed). `piccache.rs` bounds decoded thumbnails, sticker-picker pictures
and file previews by bytes (LRU, never what the last two frames drew); thumbnails
are shrunk to what the largest thumbnail size shows. Albums read only type-16 posts (`Store::album_posts`, indexed) and
`view_message` loads just the stretch up to the
photo (`Store::messages_from`). Start-up builds the UI from
the store (`App::new`), then merges `getMessageBoxes` (chats whose newest id
is not stored become `stale` and are fetched on open) and replays the stream
from the stored revision. Each chat keeps its own draft (`App::drafts`, stored as
`cache.draft.<mid>`); `run` reads all ready terminal events in one batch and
marks a batch of ≥ 8 key presses as a paste (`App::pasting`: Enter inserts a
line break instead of sending). `ratatui-image` is built without its default
`chafa` feature (needs a system C library).

## Repo notes

- The enclosing git repository is the user's home directory; this project has no
  repo of its own, so don't commit unless asked (and suggest `git init` here).
- `session.json` holds tokens and private keys (written 0600, gitignored);
  the TUI's `session.db` holds decrypted messages (0600, gitignored).
- The live service is the source of truth: the reference was wrong in places (e.g. `getE2EEPublicKey(mid, 1, 0)` is
  rejected; documented in PROTOCOL.md).
