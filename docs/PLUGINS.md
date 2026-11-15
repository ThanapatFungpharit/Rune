# Plugins

`rune-tui` can be extended with **plugins**: sandboxed WebAssembly modules
that add `/commands`, keys, views, settings and reactions to messages without
any change to rune's own code. This document is the whole contract: the
design and why it looks the way it does, the API, the manifest, the
permissions and limits, and how to build one.

- SDK crate (wire types, manifest checks, the export macro): [`plugin-sdk/`](../plugin-sdk)
- Sample plugin: [`plugins/snippets/`](../plugins/snippets)
- Host: [`src/bin/rune-tui/plugins/`](../src/bin/rune-tui/plugins) (lifecycle, permissions,
  sandbox, Git installs in `install.rs`, change watching in `watch.rs`) and
  [`overlays/plugin_ui.rs`](../src/bin/rune-tui/overlays/plugin_ui.rs) (screens)
- Tooling: [`scripts/plugin.sh`](../scripts/plugin.sh) (`new`, `build`, `install`)

## Quick start

```sh
rustup target add wasm32-unknown-unknown          # once
scripts/plugin.sh install plugins/snippets         # build + copy to ~/.local/share/rune/plugins/snippets
./target/release/rune-tui                          # then: Ctrl-x › Plugins → Snippets → Enabled → y
```

Then `/snip add hi` saves a snippet, `/snip hi` types it into the message line,
and `Alt-s` opens the list.

From a Git repository instead (it must hold the plugin built — see
[Installing from Git](#installing-from-git)): Settings › Plugins › *Install from
a Git repository…*, give its address (and, if needed, a branch or tag and the
plugin's folder in it), then enable it on its page.

A new plugin of your own:

```sh
scripts/plugin.sh new my-plugin                    # plugins/my-plugin: Cargo.toml, plugin.json, src/lib.rs
scripts/plugin.sh install plugins/my-plugin
```

A running rune notices rebuilt or copied files within a couple of seconds
([hot reload](#hot-reload)): a changed module waits on the plugin's page for
your approval, or loads by itself if you set *Load changed versions* to
*Automatically* there.

## Design

### Where rune can be extended

Auditing the TUI turned up these extension points, which the plugin API is
built on (each one is a single, narrow hook in the core):

| Extension point | In the core | What plugins get |
|---|---|---|
| Message-line commands | `App::submit` matches `/name`; unknown single words are refused | `/name` from the manifest, dispatched after rune's own (`keymap::CORE_COMMANDS` can never be taken) |
| Keys | `keymap.rs` resolves every key per `Ctx`; the keymap tests pin one meaning per key | `Alt-<letter or digit>`, which rune never binds (a keymap test keeps it that way), in the main views, also while typing |
| Overlays | `App::modal` takes every key while open | a plugin *view* (`Modal::PluginView`) and plugin *forms* (the existing `modal::Form`) |
| Events | `App::on_operation` (new messages), `App::open_chat` | `message`, `chat_opened` (with `read_messages`), timers |
| Settings | `Prefs` in the store's `meta` table; Settings tabs | per-plugin settings declared in the manifest, edited on its page in Settings › Plugins, stored under `setting.plugin.<id>.config.*` |
| Storage | `Store::get`/`set`/`values_with_prefix` | a small key/value store per plugin (`plugin.<id>.data.*`) |
| Help | `keymap::help_lines` per context | plugin commands and keys listed on the help screens |
| Main loop | `run`'s `select!` over keys, `AppEvent`s, ticks, retries | a branch for plugin timers; plugin loading off the UI thread (`AppEvent::PluginLoaded`) |

Everything plugin-specific sits behind those hooks: the core calls
`self.plugins.<event>(…)` and applies the returned effects; it holds no
per-plugin code and runs unchanged with no plugins installed.

### Why WebAssembly

The options were native libraries (`dlopen`), embedded scripting, external
processes, and WebAssembly. rune already embeds **wasmtime** (it runs LINE's
`ltsm.wasm`), and WebAssembly is the only option that gives every
requirement at once:

- **Isolation.** A plugin owns only its linear memory. It cannot read files,
  open sockets or touch rune's memory; a trap, an infinite loop (fuel runs
  out) or a memory blow-up (capped) becomes an error value, never a crash.
  Native libraries share the address space and any panic or bug takes rune
  down; external processes can do anything the user can.
- **Enforceable permissions.** The module imports exactly one host function
  (`rune.log`). Everything else is an *action* in its reply, which the host
  checks against the permissions the user granted. A permission is not a
  promise, it is the only way in.
- **A stable ABI.** JSON over linear memory does not depend on Rust's ABI or
  compiler version; plugins can be written in any language that targets
  `wasm32-unknown-unknown`.
- **No new dependencies.** `wasmtime` and `serde_json` are already in the tree.

### Shape

```text
 core (main.rs)                 plugins/mod.rs (Plugins)               plugin (wasm)
 ───────────────                ─────────────────────────              ──────────────
 submit "/snip x"  ─command──▶  find owner, build Event  ──JSON──▶  handle(event)
 on_key Alt-s      ─key──────▶  call Sandbox (fuel, mem)              │
 on_operation msg  ─message──▶  ◀──────── JSON {actions:[…]} ─────────┘
                                check each action:
                                  permission? interactive? limits? sanitize
 apply_plugin_effects ◀─Effect─ storage + timers done here
   status, notify, view, form,
   insert text, send message
```

- **Plugins** (`plugins/mod.rs`) owns discovery, lifecycle, approval,
  settings, storage, timers and permission checks. It knows nothing about the
  screen and is unit-tested without the app.
- **Sandbox** (`plugins/runtime.rs`) runs one module: compile, link,
  instantiate, call with a fuel budget, read the reply. Every failure,
  including a panic inside wasmtime, comes back as `Err`.
- **plugin_ui** (`overlays/plugin_ui.rs`) is the app side: it builds events
  from app state (`ChatRef`, `MessageInfo`), applies `Effect`s to the screen
  and draws plugin pages and plugin views.
- **rune-plugin** (`plugin-sdk/`) holds the wire types and manifest checks
  used by both sides, so host and plugins cannot drift apart.

## Lifecycle

```text
discovered ─┬─ Invalid / Incompatible                  never run; its page says why
            └─ Disabled ── enable (user approves) ──▶ Loading ── init ok ──▶ Active
               Active ── disable ──▶ Disabled          (shutdown event first)
               Loading / Active ── error ──▶ Failed ── restart ──▶ Loading
               enabled, but module or permissions changed ──▶ NeedsApproval
               enabled, run with --no-plugins ──▶ Paused
               settings/data left, directory gone ──▶ Missing ── forget ──▶ (gone)
               Active ── files change ──┬─ approved ──▶ new version loads beside it ──▶ swapped in
                                        ├─ not approved ──▶ waits (old one runs) ── approve ──▶ …
                                        └─ unusable / won't load ──▶ old one keeps running
```

1. **Discovery** (start-up). Every directory in the plugin directory
   (`--plugins DIR`, default `$XDG_DATA_HOME/rune/plugins`, else
   `~/.local/share/rune/plugins`) is read: `plugin.json` is parsed and
   validated, the directory name must equal the plugin id, the API version
   must be compatible, the module must exist (and is hashed with SHA-256), and
   on Unix no file may be writable by other users. Nothing is executed.
2. **Loading.** Only plugins that are enabled *and* whose current module hash
   and permission list equal what the user approved. The module is read once,
   its hash checked again on exactly those bytes, compiled and instantiated in
   a blocking task (off the UI thread) and posted back as
   `AppEvent::PluginLoaded`.
3. **Initialization.** The module's own `rune_api_version` is checked, then
   the plugin gets `init` with its settings (validated, defaults filled in)
   and its stored data.
4. **Active.** Events flow in, actions flow out (below).
5. **Deactivation.** Disabling sends `shutdown` (only `status` and
   `store_*` take effect), drops the instance, cancels its timers and closes
   its view. Quitting rune sends `shutdown` to every plugin.
6. **Unloading** is dropping the wasmtime `Store`: all of the plugin's
   memory goes with it.
7. **Errors.** A trap, running out of fuel, a pointer outside its memory, a
   reply that is not JSON or too big: the plugin is stopped and marked
   *Failed* (with the reason, on its page and the status line); the rest of
   rune, and other plugins, carry on. *Restart* on its page reloads it from
   disk, and so do new files ([hot reload](#hot-reload)).

## Managing plugins

Settings › Plugins (`Ctrl-s`, or `Ctrl-x` › *Plugins*) lists every installed
plugin with its state (problems marked ⚠). `Enter` opens a plugin's page;
`Esc` there goes back to the list. A plugin's page shows its description, what it may do (permissions),
the commands and keys it adds, its settings, version, author, API version and
folder, and offers:

- **Enabled** — turning it on lists the permissions and asks for
  confirmation; turning it off also forgets the approval.
- **Restart** — reload from disk (after rebuilding, or after a failure).
- **Settings** — on/off and choices change in place; numbers and text open a
  form, checked against the manifest. **Reset to defaults**.
- **Load changed versions** — *Ask first* (default) or *Automatically*: a
  changed module (a rebuild, an update) loads without asking as long as it
  asks for no new permissions. Turning it on asks first; meant for plugins
  you build yourself or fully trust.
- **Load the new version…** — when a newer version is waiting (it lists what
  that version may do, marking new permissions).
- For a plugin installed from Git: its **Source** (address, folder),
  **Version** (tag and commit, install date), **Updates** (what it follows;
  `Enter` chooses a version — see [Updates and pinning](#updates-and-pinning)),
  **Check for a new version** and **Uninstall…**.
- **Delete its stored data…**, **Forget it…** (settings, approval and data of
  a disabled or uninstalled plugin).

### Installing from Git

Settings › Plugins › *Install from a Git repository…* asks for:

| Field | |
|---|---|
| Repository | `https://host/owner/repo`, `ssh://…`, `git@host:owner/repo`, `file://…` or an absolute local path; `host.tld/owner/repo` means `https://`. No user name or password in an `https://` address (nor a password in `ssh://`): it is saved in `.rune-source.json` and shown on screen, so sign in through git's credential helper or an ssh key. git never prompts: ssh runs with `BatchMode=yes` added to your `GIT_SSH_COMMAND` or `core.sshCommand` |
| Version | optional: empty follows the default branch; `latest` the newest release; a range (`^1.2`, `~1.2`, `1.x`) the newest release in it; a tag (`v1.2.0`), a version naming one (`1.2`) or a commit pins it; a branch name follows that branch |
| Folder | optional: where `plugin.json` is, for repositories holding several plugins (otherwise found by itself, up to three folders deep) |

The repository must hold the plugin **built**: `plugin.json` next to its
`.wasm` (committed, not in Git LFS). rune never builds plugins — a build runs
the author's build scripts and macros, which is exactly the code nothing may
run unasked. What happens, off the UI thread, with progress on the status line:

1. `git ls-remote` lists the branches and tags, the version is resolved to one
   commit, and exactly that commit is fetched (`git fetch --depth 1`) into
   `<plugin dir>/.staging/…` (hidden; leftovers of a crash are removed at the
   next start).
2. The plugin folder is found and checked exactly like discovery checks an
   installed one (manifest, id, API version, module size), and the module is
   **compiled, never instantiated**: imports other than `rune.log` and a
   missing or mistyped ABI export are refused here, not after installing.
3. Only `plugin.json`, the module and `.rune-source.json` (address, what it
   follows, folder, commit, tag, date) are moved into `<plugin dir>/<id>/`, by one rename,
   readable by all and writable only by you. The clone is deleted.
4. The plugin is listed **off**; enabling it asks as always.

Refused, with the reason on the status line: an id that is already installed
(from the same repository: use *Check for a new version*; from elsewhere or
copied in by hand: remove that one first), a repository with no plugin, with
several (it lists their folders), a missing or LFS-stored module, an invalid,
incompatible or unloadable plugin, an unknown branch, tag or version, a range
no release matches, git missing or failing (its message), and git commands
over five minutes. Nothing is left behind by a failed
install. One install or update runs at a time.

### Updates and pinning

Every plugin installed from Git **follows** something, shown as *Updates* on its
page:

| Follows | *Check for a new version* moves it to |
|---|---|
| a branch (*newest commit on main*) | the branch's newest commit |
| releases (*newest release*, *newest ^1 release*) | the newest version tag in the range, never back to an older one; pre-releases never |
| a tag or commit (*pinned to v1.2.0*) | nowhere: a pinned plugin stays put, and the check says which newer release exists |

Releases are tags that read as versions (`v1.2.3`, `1.2`, `v2.0.0-beta.1`),
ordered by number. Ranges follow Cargo's rules: `^1.2` is 1.2 up to (not
including) 2.0, `^0.2` is 0.2.x, `~1.2` is 1.2.x, `1.x` means `^1`.

*Updates* (`Enter`) looks up the repository's versions and offers: *Newest
release*, *Newest compatible (^M) release* (same major version as the
installed one), *Newest commit on* the default branch, *Pin to the installed
version* (no network needed), and *Pin to* each release, newest first, marked
*installed*, *older* or *pre-release*. Choosing one installs where it points —
an older version too, to roll back — and makes the plugin follow it from then
on. Choosing something that points at the installed commit only changes what
it follows; nothing reloads.

A check first asks the remote where the branch or tags point (`git ls-remote`);
the same commit means up to date and nothing is downloaded. Otherwise exactly
the new commit is fetched, checked the same way as an install (it must still be
the same id) and swapped in with two renames (the old one is put back if that
fails); a running plugin then reloads as in [hot reload](#hot-reload) — so a new
module still waits for approval unless you allowed otherwise. A newer release
that a plugin does not follow (pinned, or outside its range) is reported on the
status line and noted on its page.

Settings › Plugins › *Check all for new versions* checks every plugin installed
from Git, one after another, and sums up: what was updated, how many are up to
date, which newer releases are not followed, and what failed.

*Uninstall…* stops the plugin, forgets its approval and deletes its folder;
its settings and data stay for a reinstall (*Forget it…* deletes them). Only
plugins installed from Git can be uninstalled from rune: a folder copied in by
hand may be someone's work, so rune leaves deleting it to you.

**Dependencies.** A plugin is one self-contained module: plugins cannot call
each other or load libraries, so there is nothing else to fetch (and
submodules are not). What a plugin needs from rune is its API version,
checked at install, at discovery and again from the module when it starts.

### Hot reload

rune looks at the plugin directory every second (sizes and modification times,
not contents) and reacts once a change has held still for one more look, so a
module still being written is never loaded half-done. Polling keeps this free
of platform-specific file-watching code and new dependencies; with a handful
of plugins it costs nothing. Then, per changed plugin directory:

- **New**: listed, off (or started, if it was enabled and approved before —
  same module, same permissions — and had only been removed).
- **Removed**: stopped (`shutdown` first); an entry stays only for leftover
  settings or data.
- **Changed, not running**: its new files are read; a failed or starting
  plugin gets another try, a disabled one shows its new version.
- **Changed, running**: rune checks the new files first. Usable and approved
  (or *Load changed versions: Automatically* with the same permissions): the
  new module is compiled and started beside the running one; only once it
  loaded does the old one get `shutdown` and the new one `init`. Not approved:
  the old one keeps running and the page offers *Load the new version…*.
  Files that are not usable, or a module that does not compile, start or
  speak this API: the old one keeps running and the page says why. (A new
  version that loads but crashes in `init` is stopped like any crash; fixing
  the files retries it.)

The same check runs after rune's own installs and updates, and looking at
files that did not change does nothing, so a change is handled once however
it is noticed.

`--no-plugins` starts rune without running any plugin (they stay enabled for
the next run). A plugin's command typed while it is off answers with where it
went ("/snip comes from the plugin Snippets, which is off").

## Security model

- **Nothing runs unasked.** Installing copies files; a plugin runs only after
  the user enables it and approves its permissions, and only that exact module
  (SHA-256) with exactly those permissions. A changed module or permission list
  is not started until approved again — also when the file is swapped between
  approval and compilation, and also when it arrives by hot reload or an
  update. The one exception is the user's own choice: *Load changed versions:
  Automatically* approves a changed module that asks for no new permissions.
- **Installing from Git runs no plugin code and no build.** rune runs `git`
  with no terminal prompts (`GIT_TERMINAL_PROMPT=0`, ssh in batch mode),
  only the https, ssh and file transports (no `ext::`, no plain `http`/`git`),
  symlinks checked out as plain files, no submodules, and a time limit;
  addresses and branch names are checked first (no options smuggled in). The
  module is compiled to check it, never instantiated.
- **No ambient authority.** One import (`rune.log`); modules importing
  anything else are refused before linking. No WASI: no files, clock,
  network, environment or randomness except what the host passes in.
- **Permissions** gate every action with effects outside the plugin's view
  (table below). Refused actions are reported on the status line and in the
  log; they do not stop the plugin.
- **No surprise input.** Only replies to *interactive* events (a command, a
  key, a key in its view, a submitted form) may open views or forms or type
  into the message line. Background events (messages, timers) can update a
  view that is already open, but never take the keyboard.
- **Terminal safety.** Every string a plugin sends is stripped of control
  characters before it is drawn or passed on, so a plugin cannot emit terminal
  escape sequences; lengths are capped.
- **Resource limits** (per plugin): 200 M fuel units per event (roughly
  instructions: an endless loop is stopped after about 0.1 s), 64 MiB memory,
  1 MiB reply, 16 MiB module, 64 actions per reply, 2000 view lines, 16 timers
  (1 s – 7 days), 5 sent messages a minute, storage of 256 keys × 16 KiB, 200
  log lines per event.
- **Shared-writable files are refused** (Unix): a plugin directory, manifest
  or module writable by other users could be swapped for anything.
- **Storage is separated** by prefix: a plugin sees only its own data. Plugin
  settings live under `setting.` and survive *clear local data*; plugin data
  under `plugin.<id>.data.`; *Forget* deletes both.

## The manifest (`plugin.json`)

```json
{
  "id": "snippets",
  "name": "Snippets",
  "version": "1.0.0",
  "api_version": "1.0",
  "description": "Reusable bits of text …",
  "author": "…",
  "entry": "snippets.wasm",
  "permissions": ["compose", "storage"],
  "commands": [{ "name": "snip", "usage": "/snip [NAME | add [NAME] | rm NAME]", "help": "insert, add or remove a snippet" }],
  "keys": [{ "id": "open", "key": "alt+s", "help": "the snippet list" }],
  "settings": [
    { "key": "sort", "label": "Order", "type": "choice", "default": "name", "choices": ["name", "newest"] },
    { "key": "preview", "label": "Preview length", "type": "int", "default": 60, "min": 10, "max": 200 }
  ]
}
```

| Field | Required | Rules |
|---|---|---|
| `id` | yes | `[a-z][a-z0-9-]*`, ≤ 32; equals the directory name |
| `name` | yes | 1–48 characters, one line |
| `version` | yes | the plugin's own version, ≤ 32 characters |
| `api_version` | yes | `MAJOR.MINOR` of the plugin API it was built for |
| `description`, `author` | no | ≤ 500 / ≤ 100 characters, one line |
| `entry` | no | the module's file name next to the manifest (default `plugin.wasm`) |
| `permissions` | no | from the table below, each once |
| `commands` | no | `name` (`[a-z][a-z0-9-]*`, ≤ 24, not one of rune's own), optional `usage` (≤ 60), `help` (≤ 120) |
| `keys` | no | `id`, `key` (`alt+<a-z or 0-9>`), `help` (≤ 120) |
| `settings` | no | `key`, `label`, optional `help`, `type` (`bool`, `int`, `text`, `choice`), `default`, and `min`/`max` (int), `max_len` (text, ≤ 1000), `choices` (choice, each ≤ 48) |

Every text field is one line: control characters (newlines included) are refused.

Unknown fields are refused (typos are caught, not ignored). Commands rune
handles itself (`keymap::CORE_COMMANDS`: `/save`, `/mute`, `/attach`, …), and
keys or commands an earlier plugin (by id) already has are not added; the
status line says so when the plugin starts.

### Permissions

| Permission | Allows |
|---|---|
| `read_messages` | `message` events for every new message (text decrypted) and `chat_opened` |
| `send_messages` | `send_message`: a text message in the user's name, to the open chat or one in the chat list (5 a minute) |
| `compose` | `insert_text` into the message line (interactive replies only) |
| `notify` | `notify`: a desktop notification, as set up in Settings (off means off) |
| `storage` | `store_set` / `store_delete`; the data comes back in `init` |

Always allowed: `status`, `open_view` / `close_view`, `form` (interactive
only), `set_timer` / `cancel_timer`, logging.

## The API (version 1.0)

### ABI

A plugin is a `wasm32-unknown-unknown` module exporting:

| Export | Signature | Meaning |
|---|---|---|
| `memory` | memory | its linear memory |
| `rune_api_version` | `() -> i32` | `major << 16 \| minor` |
| `rune_alloc` | `(len: i32) -> i32` | a buffer of `len` bytes for the next event |
| `rune_call` | `(ptr: i32, len: i32) -> i64` | handle the event (JSON) in that buffer; return `ptr << 32 \| len` of the reply (JSON), valid until the next call |

and may import only `rune.log(level: i32, ptr: i32, len: i32)` (0 error, 1
warn, 2 info, 3 debug), which writes to rune's `--log` file. Calls are
sequential; the plugin keeps its state in its memory between them.
`rune_plugin::export_plugin!(Type)` generates all of this for a Rust type
implementing `Plugin + Default`.

### Events (host → plugin)

Each call gets `{"now": <ms since epoch>, "event": {"type": …, …}}`.
*Interactive* events answer something the user just did.

| `type` | Fields | When |
|---|---|---|
| `init` | `config`, `data`, `host` | started (at start-up or when enabled) |
| `shutdown` | — | about to stop (disable or quit) |
| `config_changed` | `config` | the user changed a setting |
| `command` *(interactive)* | `name`, `args`, `chat` | `/name args` typed (`chat`: the open chat or `null`) |
| `key` *(interactive)* | `id`, `chat` | one of its keys pressed |
| `view_key` *(interactive)* | `view`, `key`, `selected` | a key pressed in its view: a character, or `Enter` `Up` `Down` `Left` `Right` `Home` `End` `Tab` `Shift-Tab` `Backspace` `Delete` `Space` (`Esc`, `PgUp`/`PgDn`, `?` and `F1` stay rune's) |
| `view_closed` | `view` | the user closed its view |
| `form_submitted` *(interactive)* | `form`, `values` | a form it opened was submitted |
| `form_cancelled` | `form` | … or cancelled |
| `message` | `chat`, `message` | a new message (`read_messages`) |
| `chat_opened` | `chat` | the user opened a chat (`read_messages`) |
| `timer` | `id` | a timer went off |

`chat` is `{"mid", "name", "kind": "user"|"group"|"room"|"other"}`;
`message` is `{"id", "from", "from_name", "text" (text messages only), "summary", "time", "mine"}`.

### Actions (plugin → host)

The reply is `{"actions": [{"type": …, …}, …]}`.

| `type` | Fields | Needs |
|---|---|---|
| `status` | `text` | — |
| `notify` | `title`, `body` | `notify` |
| `open_view` | `id`, `title`, `lines: [{text, style}]`, `selected`?, `keys: [{key, help}]`? | — (new views: interactive only; an open view can always be updated) |
| `close_view` | `id` | — |
| `form` | `id`, `title`, `fields: [{label, value?, hint?}]` (≤ 8) | interactive |
| `insert_text` | `text` | `compose`, interactive |
| `send_message` | `chat`, `text` | `send_messages` |
| `store_set` / `store_delete` | `key` (`[A-Za-z0-9_.-]`, ≤ 64), `value` (≤ 16 KiB) | `storage` |
| `set_timer` / `cancel_timer` | `id`, `after_ms`, `repeat`? | — |

Line styles: `normal`, `muted`, `accent`, `danger`, `bold`, `heading` (drawn
in the user's theme). A view with `selected` highlights that line and keeps it
on screen; `keys` appear on its frame and on its `?` help screen.

### Versioning and compatibility

The API version is `MAJOR.MINOR` (`rune_plugin::API_VERSION`, now `1.0`). A
plugin built for `M.m` runs on a host speaking `M.n` when `n ≥ m`:

- **Minor** versions only add: new event types, new action types, new
  optional fields. Plugins must ignore events they do not know (the SDK maps
  them to `Event::Unknown`); hosts refuse actions they do not know (reported,
  not fatal).
- **Major** versions may change or remove anything; plugins for another
  major version are not loaded.

The version is checked twice: from `plugin.json` at discovery (an
incompatible plugin is listed as *Incompatible* and never run) and from the
module's `rune_api_version` after instantiation (a module that disagrees with
its manifest is refused too).

## Writing a plugin in Rust

```rust
use rune_plugin::prelude::*;

#[derive(Default)]
struct Hello {
    greeted: u32,
}

impl Plugin for Hello {
    fn handle(&mut self, _cx: &Context, event: Event) -> Vec<Action> {
        match event {
            Event::Command { args, chat, .. } => {
                self.greeted += 1;
                let place = chat.map_or("nowhere".to_owned(), |c| c.name);
                vec![Action::status(format!("hello #{} from {place}: {args}", self.greeted))]
            }
            _ => Vec::new(),
        }
    }
}

rune_plugin::export_plugin!(Hello);
```

Build it as a `cdylib` for `wasm32-unknown-unknown` (`scripts/plugin.sh new`
sets up `Cargo.toml` with a small release profile and `panic = "abort"`; a
panic then traps, and the host marks the plugin failed). Because `handle` is
ordinary Rust, test plugins natively with `cargo test` — see
`plugins/snippets/src/lib.rs`, whose tests drive it with events and check the
actions. `rune_plugin::Manifest::parse` validates a `plugin.json` the same way
rune does.

Other languages work too: anything that produces a module with the exports
above and speaks the JSON. The host's tests (`plugins/tests.rs`) use plugins
written directly in the WebAssembly text format.

## Tests

- `plugin-sdk`: manifest validation, version compatibility, wire format, dispatch
  (`cd plugin-sdk && cargo test`).
- `plugins/snippets`: the sample's logic (`cd plugins/snippets && cargo test`).
- Host (`cargo test --features tui plugins::`): discovery (invalid, incompatible,
  renamed, missing module, shared-writable), the sample's full lifecycle
  (enable, command, form, storage, key, settings, restart across runs,
  disable), approval of changed modules and permissions (including a swap
  between approval and compilation), `--no-plugins`, module API checks, broken
  modules (invalid, unknown import, missing export, too much memory, a start
  function that loops, a trap in `init`), plugins that misbehave later (trap,
  endless loop, wild pointer, huge reply, non-JSON, memory growth) while others
  keep working, permission checks, background events vs. the keyboard, the send
  rate limit, timers, reserved commands, settings validation and fallback,
  forgetting removed plugins, text sanitising, hot reload (approval of a
  changed module beside the running one, automatic loading only with the same
  permissions, unusable files and modules that fail to load leaving the old
  one running, a failed plugin retried with new files, plugins appearing and
  disappearing, changes handled once), the watcher (changes reported once
  settled), module checks without running them, and Git installs from local
  repositories (installed files and modes, duplicates, updates, uninstall,
  hand-installed plugins left alone, branch and folder, and every refusal:
  no plugin, several, no module, LFS, invalid, incompatible, unloadable,
  extra imports, unknown branch, nothing left behind), versions (tags read and
  ordered, Cargo-style ranges, annotated tags, what each track resolves to,
  what typed versions mean) and following, pinning and switching against a
  repository with releases (latest, compatible ranges taking new releases,
  pinned plugins reporting newer ones, rolling back, branch tips, commits by
  id, records of the first version still read). The Git tests are skipped
  when `git` is not installed.
- App (`cargo test --features tui` in `main.rs`): no plugins changes nothing,
  the sample works through the real UI (command → form → view → message line,
  `Alt-s`, help, Settings › Plugins), enabling from its page asks first, a crashing
  plugin is stopped while the app goes on, installing from Git through Settings
  (form checks, one job at a time, the result listed, an update waiting for
  approval, automatic loading and uninstalling asked first), the version menu,
  pinning, *Check all* and switching to an older version, changed files
  reloading while the app runs, and every overlay (plugin page, plugin view)
  draws at any size.

The host tests use the prebuilt `plugins/snippets/snippets.wasm`; rebuild it
with `scripts/plugin.sh build plugins/snippets` after changing the sample or
the SDK.
