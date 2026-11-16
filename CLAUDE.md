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
cargo test --features tui plugins::            # plugin host: discovery, lifecycle, sandbox failures
(cd plugin-sdk && cargo test)                  # plugin API crate (not built by the commands above)
(cd plugins/snippets && cargo test)            # the sample plugin's logic, natively
scripts/plugin.sh build plugins/snippets       # rebuild snippets.wasm (host tests use it; needs wasm32-unknown-unknown)
make install [PREFIX=~/.local] [DESTDIR=]      # rune-tui + man page + desktop entry + icon (packaging/); make uninstall
```

`packaging/rune-tui.1` is the man page: a test (`cli::tests::man_page_lists_every_option`) fails when
an option in `cli::USAGE` is missing from it. The default session is `cli::session_path`:
`$XDG_DATA_HOME/rune/session.json` (dir created 0700), falling back to `./session.json` while only that exists.

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
`send-image`, …) and `./target/release/rune-tui [--no-read-receipts] [--log rune.log] session.json`
(`--help` lists the options; unknown ones are refused).
Anything that sends messages, marks chats read (opening a chat in the TUI does,
unless `--no-read-receipts`) or registers group keys is visible to other people.

Dev profile builds all dependencies with `opt-level = 2` because Cranelift compiling
the 2.3 MB wasm unoptimized takes ~15 s per `Ltsm::new()`.

## Architecture

**Signing / crypto via LINE's own wasm.** Every gateway request needs an
`X-Hmac` computed inside LINE's `ltsm.wasm`, which is never shipped: `src/ltsm/locate.rs` finds the
user's own copy at run time (`$RUNE_LTSM`, `~/.local/share/rune/`, `./assets/` — gitignored, the tests use it —
then installed Chrome-family extensions), accepting only sha256 `58bb4e18…`; `rune-tui` shows `WasmProblem::help` and exits without it.
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
`X-Line-Application`, no `X-Hmac`; the token is cached and fetched once for all concurrent downloads (re-fetched only when the token a request used is refused), and blob crypto runs in `spawn_blocking`. Public `DOWNLOAD_URL`/`PREVIEW_URL` (sender-chosen) are
fetched only from `https` URLs with a host name, never an IP literal (`transport::is_public_https`;
the configured OBS origin is exempt, which the mock tests rely on), through `Transport::untrusted_http`
(`transport::untrusted_client`: no proxy, `PinnedResolver` refuses a name if any A/AAAA answer fails
`is_public_ip` and the connection goes to exactly the checked addresses, every redirect re-checked);
every body goes through `media::read_body` with a size cap;
the reqwest client's redirect policy keeps token-bearing (gateway/OBS) requests on LINE hosts.
Sealed media: HKDF/AES-CTR/HMAC blob (`tests/fixtures/blob_vectors.json`, generated
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
of any mid, friend or not — names and profile pages load from it first), `read_marks`
(newest position per reader) and `read_log` (when each position was first read, op 55's
`createdTime`; newest 200 per reader kept), `images`
(profile pictures and covers by picture path / cover OBS path, pruned to 256 MB at
start), `pending_receipts`, `meta` key/values
(`stream_rev`, `owner`, `setting.*`, `cache.*`). (Media files are not in the store: opening or playing
anything — profile picture, cover, message media, sticker animation, notification icons — writes to `~/.cache/rune/view`, owner-only via `private_view_dir`;
only `s` writes to `~/Downloads/rune` and `/save PATH` to the path given, through `safe_file_name` /
`write_new`, which never replace an existing file; opening a file `runs_programs` flags — launchers,
scripts, executables, installers, by the name as written — asks first, `Action::OpenAnyway`.) Created 0600; `bind_account`
wipes another account's chat data but preserves every profile's pins. `profiles`
stores account names and last-use times. Pins live in named pin lists (`pin_profiles`, schema 3): each account has ≥1,
exactly one `active`, and the
pin methods act on the bound account's active list; pins are ordered per list.
Account profiles and pin lists are separate concepts.
`apply(op)` folds an operation in and advances
the stream position (never backwards) in one transaction: internal `*_in(&Connection, …)` helpers
let it combine the per-table writes. Writes are idempotent by message id. File-backed stores run
with `secure_delete`; `clear` and an account switch checkpoint the WAL so deleted messages leave
no copy.

**Timed refresh.** `Client::run_token_refresher` sleeps in ≤60 s slices
against the wall clock until shortly before the token's refresh time, and
restarts its wait whenever `subscribe_tokens` fires.

**TUI (`src/bin/rune-tui/`).** `main.rs` holds the `App` and the event loop;
`graphics/` holds picture code (`picture`, `piccache`, `cells`, `anim`, `avatar`, `fetchq`),
`overlays/` the screens drawn over the chat (`modal`, `attach`, `stickers`,
`settings`, `profile`, `albums`, `plugin_ui`); `input`, `keymap`, `links` sit
beside `main.rs`, as do `cli`, `clipboard` (`c` copies a text message: `wl-copy`/`xclip`/`xsel`/`pbcopy`,
else OSC 52; under `cfg(test)` into `clipboard::COPIED`), `notice`, `receipts`, `retry` and `theme` (every colour by role — `theme::t().accent`, `theme::muted()` — dark and light palettes, Auto from `COLORFGBG`, xterm-256 fallback without `COLORTERM=truecolor`; never hard-code a `Color` in drawing code). Child modules `use crate::*` and `main.rs` re-imports each by
its short name, so `picture::…`, `modal::…` paths work everywhere. Single `App` state
driven by a `tokio::select!` over crossterm events and an `mpsc` of `AppEvent`s;
all network work runs in spawned tasks that post results back. A failed load posts
`AppEvent::Failed(Job, …)` (or the view's own `Load::Failed` calls `retry_later`);
`retry.rs` backs each `Job` off exponentially on its own schedule (capped at 5 min,
10 min for per-item jobs, never given up) and `App::run_retries` restarts due ones; one
job's success or failure never reschedules another. Writes of user data to the store go through `App::saved`, which
puts a failure on the status line (cache writes stay best effort). A status message clears on
the next key, click or paste once it has been drawn for 8 s (30 s if `status_is_error`):
`App::expire_status`, never on a timer, so nothing goes while the user is away. `cli.rs` parses the
options (strict; `--log` sets up `tracing-subscriber`); `main::read_session` only logs
in when the session file is missing. SIGTERM/SIGHUP quit like the quit key (`spawn_signal_watch` →
`AppEvent::Terminate`; a second one exits at once), and a terminal error ends `run`'s loop rather
than returning early, so the open draft is saved and plugins shut down either way. Notifications go through `notice.rs`: pure wording (`gist`, `message_notice`, …:
1:1 titled by sender, group by group with `Sender: …` in the body, `Someone`/`Group chat` for
names still unknown) and `App::notify_later`, which looks up missing names (≤3 s, the app learns
them; a lookup finding the chat muted cancels) and writes the sender's picture into the view cache
as `notify-send --icon`; under `cfg(test)` nothing is delivered, notices land in `notice::SHOWN`.
Openers and notifiers start via `spawn_reaped`
(no zombies); `prune_dir` empties `~/.cache/rune/view` of files older than 7 days or
beyond 1 GiB at start-up, and all of it on *clear local data*. Read receipts for
messages arriving in the open chat wait for terminal focus (`App::on_focus`). Every read goes
through `App::note_read` into `receipts::Receipts` (pure state, mirrored in the store's
`pending_receipts`): per chat an `approved` position (read with receipts on; sent with one request
per chat in flight, retried via `Job::Receipt` until acknowledged) and a `held` one (read with them
off; approved only once receipts are on *and* the user is in that chat). Positions clear only on an
acknowledgement (our request, or op 40 from any device); tickets make results of requests cancelled
by turning receipts off harmless; `--no-read-receipts` counts as off (reads are held, never sent that run); with receipts off nothing clears a chat's unread count or `last_seen` — `App::mark_seen`, gated opens/sends/server merges — only op 40 or reading with them on does). Names and pictures from any source go through
`App::learn_names`/`learn_pictures`, which update the chat list, avatars, profile
pages and the store together. Every drawn avatar (`App::avatar_ready`) and unnamed mid
queues one `resolve_names` lookup per run (`App::looked_up`), so stored names and
pictures are refreshed and missing ones fetched; profile pages post the path they
fetch (`AppEvent::ProfilePicture`). Thumbnails and avatars are not fetched where they are laid out:
drawing `want`s them in `App::media` (`fetchq::FetchQueue`) with their distance in rows from the
visible area, and `App::start_media` (after each frame) starts the nearest ones within per-kind slot
limits (`THUMB_SLOTS`, `AVATAR_SLOTS`); unstarted wants are dropped and re-asked by the next frame,
and each `AppEvent::Thumb`/`Avatar` must call `media.done`. Pictures use
ratatui-image with the detected terminal protocol (Kitty/Sixel/iTerm2) or,
with `--no-graphics`, custom block-glyph rendering in `cells.rs`; `avatar.rs`,
`profile.rs`, `picture.rs`, `links.rs`, `albums.rs` hold avatars, profile
pages (with covers), picture decoding, link/QR handling and the per-group
album list; overlays blank their box with `picture::cover` (not a bare
`Clear`): Sixel/iTerm2 graphics live in one anchor cell with the rest marked
skip, so while an overlay is open `picture::compose` lays the frame out once
without graphics, then draws pictures under a higher `Layer`'s box as text
cells (or not at all when fully covered); `modal.rs` (menus — the main menu `Ctrl-x`, chat and message menus; a label's
`\t` suffix is its right-aligned shortcut — forms, pickers, confirms and
the `Action` dispatcher), `attach.rs` (file browser + send dialog),
`stickers.rs` (also the LINE emoji reaction picker, `Pick::Emoji`), `settings.rs` (prefs in the store, notifications; tabs are `settings::Tab`),
`find.rs` (search in a chat, `Modal::Find`/`Ctx::Find`: `Store::search_messages` — stored, decrypted text
messages only, `str::to_lowercase` matching — runs in `spawn_blocking` per change of the query and posts
`AppEvent::Found`; a result whose `seq` is not the latest is dropped; `Enter` goes through
`App::show_message`, which album photos share: it loads the stored stretch from the message to the
loaded history) are the overlays — an open `App::modal` takes every key. A screen opened from a settings row goes through
`open_from_settings`, which keeps Settings in `App::modal_parent`; `settle_modal` (after every key,
mouse, paste and app event) puts it back once that screen closes. Navigation is keys and menus,
never message-line commands: `/` commands act on the open chat, and an unknown `/word` is never sent. `keymap.rs` holds every binding
per `Ctx` (one per screen/overlay, plus shared text-editing and `?` tables);
Those tables are the defaults: `keymap::Keymap` layers the user's changes on them (stored as JSON in
`setting.keys`, per `Table` — a context's own, or the shared `Global`/`TextEdit`/`QuestionHelp` — and
`Cmd`, with stable ids; also mirrored to `keys.json` in the config dir, `shortcuts::default_file` —
read at start where it wins, rewritten on every change through symlinks, and left alone while
`Keymap::parse` refuses it), installed per UI thread (`keymap::install`; `--default-keys` installs the
defaults while `App::keys` keeps the saved set). Nothing names a key in text: menus, confirm buttons,
status messages, the welcome tips and plugin notices ask `keymap::shortcut`/`global`/`with_keys`/
`via_menu`. Settings › Shortcuts lists `keymap::SECTIONS`; `overlays/shortcuts.rs` is one section's
page (`Modal::Shortcuts`, `Ctx::Keys`, whose keys and the menus' digits cannot change): `Enter`
captures the next key (routed there before global keys), refused by `Keymap::refusal` (plugin `Alt`
keys, printable keys where `typed_there`), and a key in use elsewhere (`Keymap::conflicts`, across
every context the binding answers in) moves only after a yes; `Keymap::clash` flags stored clashes.
Mouse clicks and the wheel act through `App::modal_cmd(Cmd)`, never a synthesized key, so they work
whatever the keys are. `App::available` filters the status-line hints by what the selection allows; every hint is
fitted to its width by `keymap::fit_hint` (whole entries dropped from the end, then `…`; the help
key always kept — `main::status_line`, `frame_hint` for overlay frames), and
the keymap tests pin one meaning per key and no redundant alternatives. `keymap::COMMANDS`
lists the message line's `/` commands for its help screen — keep it in step with `App::submit`.
`Ctrl-x` (main menu) is hinted first in every main view.
`input.rs` is the cursor-editing field (multi-line on the message line: word-wrapped
rows, `↑`/`↓` between them; Shift/Alt-Enter or Ctrl-j insert a line break); every text field (message line, forms,
chat search, people picker, file filter) edits through `TextInput::edit` with the
shared `TEXT_EDIT` keys (the file browser switches to `Ctx::AttachFilter` while a
filter is typed). The chat list's positions are `App::list_rows` (groups/rooms with no name are
left out, `App::hidden`, except an invitation or the open chat; `ListRow::Chat` index or,
while a search query is typed, `ListRow::Friend` for friends with no chat listed): never index
`App::chats` by list position. Search results are ranked (`name_rank`: word start, inside a word,
then member names); a typed query survives the focus leaving (`App::left_search`) until `Esc` or a
chat is opened, and filtering never pages older chats by itself (`↓` past the end does,
`App::search_older_chats`). `piccache.rs` bounds decoded thumbnails, sticker-picker pictures
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

**Plugins (`src/bin/rune-tui/plugins/`, `overlays/plugin_ui.rs`, `plugin-sdk/`;
contract in `docs/PLUGINS.md`).** WebAssembly modules run by their own wasmtime
`Engine` (fuel metering, `StoreLimits`), not `ltsm`'s. `runtime::Sandbox` hosts one
(exports `memory`/`rune_api_version`/`rune_alloc`/`rune_call`, sole import `rune.log`;
every failure, even a wasmtime panic, is an `Err`). Settings › Plugins lists them and `plugin_ui::Manager` is one plugin's page
(`Esc` back to that list). `Plugins` (no screen code) does
discovery (`<dir>/<id>/plugin.json`, id = directory name, no shared-writable files),
approval (`setting.plugin.<id>.approved` = module SHA-256 + sorted permissions; re-checked
on the bytes compiled), lifecycle (`LoadJob::run` in `spawn_blocking` →
`AppEvent::PluginLoaded` → `loaded` sends `init`), settings (`setting.plugin.<id>.config.*`,
checked by the manifest's `SettingSpec`), storage (`plugin.<id>.data.*`), timers and the
permission check of every action, returning `Effect`s that `App::apply_plugin_effects`
carries out. Only replies to interactive events (command, key, view key, form) may open
views/forms or type; background ones may update an open view. `plugins/install.rs` installs,
updates and uninstalls from Git (blocking, via the `git` program with no prompts, https/ssh/file
only, `core.symlinks=false`; never builds: a repo must hold `plugin.json` + the built `.wasm`;
`git ls-remote` resolves a `versions::Track` — branch, release range (Cargo-style `^`/`~`,
pre-releases only by pinning) or pinned tag/commit — to one commit, fetched with
`git fetch --depth 1` into `<dir>/.staging/`, checked by `inspect` + `Sandbox::check` (compiles,
never instantiates), moved in with one rename; `.rune-source.json` records url/track/folder/
commit/tag; `update(…, to)` follows the track (pinned: never moves, reports
`Outcome::available`) or switches it (older versions too); `pin` rewrites only the record;
only those installs can be uninstalled). `plugins/watch.rs` polls the directory every second
(size + mtime, reported once two looks agree) → `AppEvent::PluginFiles` → `Plugins::refresh`,
which is idempotent (`Entry::seen`): new/removed/changed plugins; a running one's new version is
loaded beside it (`LoadJob::next`) and swapped in only if approved — or auto-approved
(`setting.plugin.<id>.autoload`, same permissions) — else kept as `Update::Approval`; unusable
files or a module that fails to load leave the old one (`Update::Broken`). One Git job at a
time (`App::plugin_job`). Plugin text goes through
`plugins::clean` (no control characters). Wire types and manifest validation live in the
`rune-plugin` crate (`plugin-sdk/`, path dependency of the `tui` feature) shared with
plugin authors; bump `API_VERSION`'s minor for additions, major for breaking changes.
Core hooks: `submit` (after rune's commands; `keymap::CORE_COMMANDS` are never
dispatched to plugins — keep it in step with `submit`), `on_key` (`Alt-<letter/digit>`,
which `keymap` never binds — a test pins it), `on_operation` (new messages), `open_chat`,
the run loop (timers; `shutdown_all` on quit). The sample `plugins/snippets` is its own
cargo workspace; its prebuilt `snippets.wasm` is checked in for the host tests.

## Repo notes

- Own git repo (`origin` = private GitHub `ThanapatFungpharit/Rune`); commit or push only when asked.
  `ltsm.wasm` must never enter history (it was purged with `git filter-repo`).
- `session.json` holds tokens and private keys (written 0600, gitignored);
  the TUI's `session.db` holds decrypted messages (0600, gitignored).
- The live service is the source of truth: the reference was wrong in places (e.g. `getE2EEPublicKey(mid, 1, 0)` is
  rejected; documented in PROTOCOL.md).
