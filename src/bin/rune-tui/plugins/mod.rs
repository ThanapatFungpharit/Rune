//! Plugins: discovery, lifecycle, permissions, settings and storage.
//!
//! This module knows nothing about the screen. The app hands it events
//! (`command`, `key`, `message`, …) and applies the [`Effect`]s it returns;
//! every plugin action has been checked against the plugin's granted
//! permissions by then. Plugins are WebAssembly modules run by
//! [`runtime::Sandbox`]; the wire types and the manifest live in the
//! `rune-plugin` crate (`plugin-sdk/`), shared with plugin authors.
//!
//! Lifecycle of an installed plugin (a directory `<plugins dir>/<id>/`
//! holding `plugin.json` and its module):
//!
//! ```text
//! discovered ─┬─ Invalid / Incompatible          (never run)
//!             └─ Disabled ── enable (user approves permissions) ──▶ Loading
//!                Loading ── compiled + init ──▶ Active ── disable ──▶ Disabled
//!                Loading / Active ── error ──▶ Failed ── restart ──▶ Loading
//!                enabled, but module or permissions changed ──▶ NeedsApproval
//!                Active ── files change (hot reload) ──▶ new version loaded beside
//!                  the old one, then swapped in; or kept waiting (`Update`)
//! ```
//!
//! Nothing is executed unless the user enabled the plugin and approved
//! exactly this module (by SHA-256) with exactly these permissions — or
//! turned on *Load new versions automatically* for it, which approves a
//! changed module as long as it asks for no new permissions.
//!
//! [`install`] adds, updates and removes plugins from Git repositories;
//! [`watch`] notices changed files. Both end in [`Plugins::refresh`].

pub mod install;
pub mod runtime;
#[cfg(test)]
pub(crate) mod tests;
pub mod versions;
pub mod watch;

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rune::store::Store;
pub use rune_plugin::Event;
use rune_plugin::manifest::{MANIFEST_FILE, is_key};
use rune_plugin::proto::{Envelope, FormField, KeyHelp, ViewLine};
use rune_plugin::{Action, ApiVersion, Manifest, Permission};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use rune_plugin::proto::{ChatKind, ChatRef, LineStyle, MessageInfo};
use runtime::{Limits, Sandbox};

/// The API this host speaks.
pub const HOST_API: ApiVersion = rune_plugin::API_VERSION;

/// Largest `plugin.json`.
const MANIFEST_MAX: u64 = 256 << 10;
/// Actions taken from one reply; the rest are dropped.
const ACTIONS_PER_REPLY: usize = 64;
/// Storage quota per plugin.
const DATA_KEYS: usize = 256;
const DATA_KEY_LEN: usize = 64;
const DATA_VALUE_LEN: usize = 16 << 10;
/// Timers per plugin and their range.
const TIMERS: usize = 16;
const TIMER_MIN: Duration = Duration::from_secs(1);
const TIMER_MAX: Duration = Duration::from_secs(7 * 24 * 3600);
const TIMER_ID_CHARS: usize = 64;
/// `send_message` actions per plugin per minute.
const SENDS_PER_MINUTE: usize = 5;
/// View size limits.
const VIEW_LINES: usize = 2000;
const LINE_CHARS: usize = 500;
const FORM_FIELDS: usize = 8;
const MESSAGE_CHARS: usize = 5000;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Installed, not enabled.
    Disabled,
    /// Enabled, but the module or its permissions changed since the user
    /// approved them: enabling it again asks again.
    NeedsApproval(String),
    /// Enabled, but not started this run (`--no-plugins`).
    Paused,
    /// Being compiled and started.
    Loading,
    Active,
    /// Stopped by an error this run (still enabled: restart or disable it).
    Failed(String),
    /// The directory or manifest is unusable.
    Invalid(String),
    /// Built for an API this host does not speak.
    Incompatible(String),
    /// Settings or data remain for a plugin that is no longer installed.
    Missing,
}

impl State {
    /// One word or two for lists.
    pub fn label(&self) -> &'static str {
        match self {
            State::Disabled => "off",
            State::NeedsApproval(_) => "needs approval",
            State::Paused => "paused",
            State::Loading => "starting…",
            State::Active => "on",
            State::Failed(_) => "failed",
            State::Invalid(_) => "invalid",
            State::Incompatible(_) => "incompatible",
            State::Missing => "not installed",
        }
    }

    /// The reason behind the state, if it has one.
    pub fn detail(&self) -> Option<&str> {
        match self {
            State::NeedsApproval(s) | State::Failed(s) | State::Invalid(s) | State::Incompatible(s) => {
                Some(s)
            }
            State::Paused => Some("plugins are off for this run (--no-plugins)"),
            _ => None,
        }
    }

    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            State::Failed(_) | State::Invalid(_) | State::Incompatible(_) | State::NeedsApproval(_)
        )
    }
}

struct Timer {
    id: String,
    at: Instant,
    every: Option<Duration>,
}

/// One installed (or remembered) plugin.
pub struct Entry {
    pub id: String,
    pub dir: PathBuf,
    /// `None` when the manifest is unusable or the plugin is gone.
    pub manifest: Option<Manifest>,
    pub state: State,
    /// SHA-256 (hex) of the module as discovered.
    hash: Option<String>,
    /// The repository it was installed from (`None`: copied in by hand).
    pub source: Option<install::Source>,
    /// A newer version on disk while this one keeps running.
    update: Option<Update>,
    /// The files as last read from disk, to tell a real change from a
    /// repeated look ([`Plugins::refresh`]).
    seen: Option<Seen>,
    sandbox: Option<Sandbox>,
    config: Map<String, Value>,
    timers: Vec<Timer>,
    sends: VecDeque<Instant>,
    /// Bumped whenever a load starts or the plugin stops, so the result of
    /// a load that was overtaken is dropped.
    generation: u64,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }

    fn has(&self, p: Permission) -> bool {
        self.manifest.as_ref().is_some_and(|m| m.has(p))
    }

    /// What the user approves: this module with these permissions.
    fn fingerprint(&self) -> Option<String> {
        Some(fingerprint_of(self.manifest.as_ref()?, self.hash.as_ref()?))
    }

    /// A newer version on disk that is not running (yet).
    pub fn update(&self) -> Option<&Update> {
        self.update.as_ref()
    }

    fn from_found(f: Found) -> Entry {
        let mut entry = Entry {
            seen: Some(Seen::of(&f)),
            id: f.id,
            dir: f.dir,
            manifest: None,
            state: State::Disabled,
            hash: f.hash,
            source: f.source,
            update: None,
            sandbox: None,
            config: Map::new(),
            timers: Vec::new(),
            sends: VecDeque::new(),
            generation: 0,
        };
        match f.manifest {
            Ok(m) => entry.manifest = Some(m),
            Err(state) => entry.state = state,
        }
        entry
    }

    fn missing(id: String, dir: PathBuf) -> Entry {
        Entry {
            id,
            dir,
            manifest: None,
            state: State::Missing,
            hash: None,
            source: None,
            update: None,
            seen: None,
            sandbox: None,
            config: Map::new(),
            timers: Vec::new(),
            sends: VecDeque::new(),
            generation: 0,
        }
    }

    /// Take what is on disk as this (stopped) plugin's files.
    fn adopt(&mut self, f: Found) {
        self.seen = Some(Seen::of(&f));
        self.hash = f.hash;
        self.source = f.source;
        self.update = None;
        match f.manifest {
            Ok(m) => {
                self.manifest = Some(m);
                self.state = State::Disabled;
            }
            Err(state) => {
                self.manifest = None;
                self.state = state;
            }
        }
    }

    fn stop(&mut self) {
        self.sandbox = None;
        self.timers.clear();
        self.sends.clear();
        self.generation += 1;
    }
}

/// A newer version of a running plugin, found on disk.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// Waiting for the user to approve its module or permissions.
    Approval {
        manifest: Box<Manifest>,
        hash: String,
        source: Option<install::Source>,
        /// Permissions it asks for that the running version does not have.
        new_permissions: Vec<Permission>,
    },
    /// Its files are not usable (or it would not start): the running
    /// version stays.
    Broken(String),
}

/// What one look at a plugin's directory found, for comparison.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    manifest: Result<Manifest, State>,
    hash: Option<String>,
    source: Option<install::Source>,
}

impl Seen {
    fn of(f: &Found) -> Seen {
        Seen {
            manifest: f.manifest.clone(),
            hash: f.hash.clone(),
            source: f.source.clone(),
        }
    }
}

/// A plugin view, as the app shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub lines: Vec<ViewLine>,
    pub selected: Option<usize>,
    pub keys: Vec<KeyHelp>,
}

/// What the app should do; permissions are already checked.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Status(String),
    Notify {
        title: String,
        body: String,
    },
    /// Show or replace a view. When not `interactive`, only replace it if
    /// that very view is open.
    OpenView {
        view: View,
        interactive: bool,
    },
    /// Close the plugin's view (`id: None`: whichever is open).
    CloseView {
        plugin: String,
        id: Option<String>,
    },
    Form {
        plugin: String,
        id: String,
        title: String,
        fields: Vec<FormField>,
    },
    InsertText(String),
    /// Send a text message (to the open chat or one in the chat list: the
    /// app checks).
    SendMessage {
        plugin: String,
        chat: String,
        text: String,
    },
}

/// Compile and start one plugin, off the UI thread ([`LoadJob::run`]).
pub struct LoadJob {
    pub id: String,
    wasm: PathBuf,
    hash: String,
    limits: Limits,
    generation: u64,
    /// Hot reload: a new version, loaded while the old one keeps running
    /// and swapped in once it compiled.
    next: Option<Box<Next>>,
}

struct Next {
    manifest: Manifest,
    hash: String,
    source: Option<install::Source>,
}

pub enum LoadFailure {
    /// The module on disk is not the one the user approved.
    Changed,
    Error(String),
}

pub struct Loaded {
    pub id: String,
    generation: u64,
    result: Result<Sandbox, LoadFailure>,
    next: Option<Box<Next>>,
}

impl LoadJob {
    /// Read, verify and compile the module (blocking).
    pub fn run(self) -> Loaded {
        let result = std::fs::read(&self.wasm)
            .map_err(|e| LoadFailure::Error(format!("cannot read {}: {e}", self.wasm.display())))
            .and_then(|bytes| {
                // Checked on the bytes compiled, not on an earlier read.
                if sha256_hex(&bytes) != self.hash {
                    return Err(LoadFailure::Changed);
                }
                Sandbox::load(&self.id, &bytes, self.limits).map_err(LoadFailure::Error)
            });
        Loaded {
            id: self.id,
            generation: self.generation,
            result,
            next: self.next,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Store keys. Settings live under `setting.` (kept by *clear local
/// data*); a plugin's data under `plugin.<id>.data.`.
fn enabled_key(id: &str) -> String {
    format!("setting.plugin.{id}.enabled")
}
fn approved_key(id: &str) -> String {
    format!("setting.plugin.{id}.approved")
}
fn config_prefix(id: &str) -> String {
    format!("setting.plugin.{id}.config.")
}
fn data_prefix(id: &str) -> String {
    format!("plugin.{id}.data.")
}
fn autoload_key(id: &str) -> String {
    format!("setting.plugin.{id}.autoload")
}

/// The permission part of an approval fingerprint (`hash|perms`).
fn perms_of(fingerprint: &str) -> &str {
    fingerprint.split_once('|').map_or("", |(_, p)| p)
}

fn fingerprint_of(m: &Manifest, hash: &str) -> String {
    let mut perms: Vec<&str> = m.permissions.iter().map(|p| p.as_str()).collect();
    perms.sort_unstable();
    format!("{hash}|{}", perms.join(","))
}

/// Text from a plugin made safe to draw: no control characters (which could
/// carry terminal escapes), at most `max` characters.
pub fn clean(s: &str, max: usize, newlines: bool) -> String {
    let mut out: String = s
        .chars()
        .filter(|&c| c != '\r')
        .map(|c| match c {
            '\n' if newlines => '\n',
            '\t' => ' ',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .take(max + 1)
        .collect();
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

/// The default plugin directory: `$XDG_DATA_HOME/rune/plugins`, else
/// `~/.local/share/rune/plugins`.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("rune/plugins"))
}

/// One directory found in the plugin directory.
pub struct Found {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, State>,
    pub hash: Option<String>,
    pub source: Option<install::Source>,
}

/// Look through `dir` for plugins (reads files, runs nothing).
pub fn discover(dir: &Path, limits: Limits) -> Vec<Found> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'))
        })
        .collect();
    dirs.sort();
    dirs.into_iter().map(|d| inspect(&d, limits)).collect()
}

/// Read one plugin directory.
pub fn inspect(dir: &Path, limits: Limits) -> Found {
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut found = Found {
        id: id.clone(),
        dir: dir.to_owned(),
        manifest: Err(State::Invalid(String::new())),
        hash: None,
        source: None,
    };
    let invalid = |s: String| Err(State::Invalid(s));
    if !rune_plugin::manifest::is_ident(&id, 32) {
        found.manifest =
            invalid("the directory name is not a plugin id (lowercase letters, digits, '-')".into());
        return found;
    }
    let path = dir.join(MANIFEST_FILE);
    let text = match std::fs::metadata(&path) {
        Ok(m) if m.len() > MANIFEST_MAX => Err("plugin.json is too large".to_owned()),
        Ok(_) => std::fs::read_to_string(&path).map_err(|e| format!("cannot read plugin.json: {e}")),
        Err(_) => Err("no plugin.json".to_owned()),
    };
    let manifest = match text.and_then(|t| Manifest::parse(&t)) {
        Ok(m) => m,
        Err(e) => {
            found.manifest = invalid(e);
            return found;
        }
    };
    if manifest.id != id {
        found.manifest = invalid(format!(
            "plugin.json says id {:?}, but it is in {id}/",
            manifest.id
        ));
        return found;
    }
    match manifest.api() {
        Some(api) if api.runs_on(HOST_API) => {}
        _ => {
            found.manifest = Err(State::Incompatible(format!(
                "built for plugin API {}; this rune speaks {HOST_API}",
                manifest.api_version
            )));
            return found;
        }
    }
    let wasm = dir.join(&manifest.entry);
    if let Some(e) = shared_writable(&[dir, &path, &wasm]) {
        found.manifest = invalid(e);
        return found;
    }
    match std::fs::metadata(&wasm) {
        Ok(m) if m.len() > limits.module as u64 => {
            found.manifest = invalid(format!("{} is too large", manifest.entry));
            return found;
        }
        Ok(_) => match std::fs::read(&wasm) {
            Ok(bytes) => found.hash = Some(sha256_hex(&bytes)),
            Err(e) => {
                found.manifest = invalid(format!("cannot read {}: {e}", manifest.entry));
                return found;
            }
        },
        Err(_) => {
            found.manifest = invalid(format!("{} is missing (build the plugin first)", manifest.entry));
            return found;
        }
    }
    found.manifest = Ok(manifest);
    found.source = install::Source::read(dir);
    found
}

/// Files anyone else could swap are refused: running them would run
/// whatever another user put there.
fn shared_writable(paths: &[&Path]) -> Option<String> {
    #[cfg(unix)]
    for p in paths {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(p)
            && m.permissions().mode() & 0o002 != 0
        {
            return Some(format!("{} is writable by other users", p.display()));
        }
    }
    #[cfg(not(unix))]
    let _ = paths;
    None
}

pub struct Plugins {
    store: Store,
    /// Where plugins are installed; `None`: no plugins this run.
    pub dir: Option<PathBuf>,
    /// `--no-plugins`: enabled plugins are not started.
    pub safe_mode: bool,
    pub entries: Vec<Entry>,
    limits: Limits,
    /// `/name`s rune itself handles.
    reserved: &'static [&'static str],
    host: String,
}

impl Plugins {
    pub fn new(
        store: Store,
        dir: Option<PathBuf>,
        safe_mode: bool,
        reserved: &'static [&'static str],
    ) -> Self {
        Plugins {
            store,
            dir,
            safe_mode,
            entries: Vec::new(),
            limits: Limits::default(),
            reserved,
            host: format!("rune-tui {}", env!("CARGO_PKG_VERSION")),
        }
    }

    #[cfg(test)]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    fn is_enabled(&self, id: &str) -> bool {
        self.store.get(&enabled_key(id)).ok().flatten().as_deref() == Some("true")
    }

    /// Find the installed plugins and decide what to start. Run once at
    /// start-up; the jobs go off the UI thread, their results to [`loaded`](Self::loaded).
    pub fn scan(&mut self) -> Vec<LoadJob> {
        let found = self
            .dir
            .as_deref()
            .map(|d| discover(d, self.limits))
            .unwrap_or_default();
        let mut jobs = Vec::new();
        let mut ids = HashSet::new();
        for f in found {
            ids.insert(f.id.clone());
            self.entries.push(Entry::from_found(f));
            let i = self.entries.len() - 1;
            if self.entries[i].manifest.is_some() {
                jobs.extend(self.decide(i));
            }
        }
        // Plugins that were removed but left settings or data behind.
        let mut gone: Vec<String> = ["setting.plugin.", "plugin."]
            .iter()
            .flat_map(|p| self.store.values_with_prefix(p).unwrap_or_default())
            .filter_map(|(k, _)| {
                let rest = k
                    .strip_prefix("setting.plugin.")
                    .or_else(|| k.strip_prefix("plugin."))?;
                Some(rest.split('.').next()?.to_owned())
            })
            .filter(|id| !ids.contains(id))
            .collect();
        gone.sort();
        gone.dedup();
        for id in gone {
            let dir = self.dir.clone().unwrap_or_default().join(&id);
            self.entries.push(Entry::missing(id, dir));
        }
        jobs
    }

    /// Ids that left settings or data in the store.
    fn remembered(&self, id: &str) -> bool {
        [format!("setting.plugin.{id}."), format!("plugin.{id}.")]
            .iter()
            .any(|p| !self.store.values_with_prefix(p).unwrap_or_default().is_empty())
    }

    /// The state of a discovered plugin with a usable manifest, and its job
    /// if it is to start.
    fn decide(&mut self, i: usize) -> Option<LoadJob> {
        let id = self.entries[i].id.clone();
        if !self.is_enabled(&id) {
            self.entries[i].state = State::Disabled;
            return None;
        }
        let approved = self.store.get(&approved_key(&id)).ok().flatten();
        let fp = self.entries[i].fingerprint();
        let auto = || fp.as_deref().is_some_and(|fp| self.auto_approve(&id, fp));
        if approved.is_none() || (approved != fp && !auto()) {
            self.entries[i].state =
                State::NeedsApproval("its module or permissions changed since you enabled it".into());
            return None;
        }
        if self.safe_mode {
            self.entries[i].state = State::Paused;
            return None;
        }
        Some(self.start(i))
    }

    fn start(&mut self, i: usize) -> LoadJob {
        let e = &mut self.entries[i];
        e.stop();
        e.state = State::Loading;
        let m = e.manifest.as_ref().expect("started plugins have a manifest");
        LoadJob {
            id: e.id.clone(),
            wasm: e.dir.join(&m.entry),
            hash: e.hash.clone().unwrap_or_default(),
            limits: self.limits,
            generation: e.generation,
            next: None,
        }
    }

    /// Load a new version of a running plugin beside it (hot reload); the
    /// old one keeps running until [`loaded`](Self::loaded) swaps them.
    fn swap(
        &mut self,
        i: usize,
        manifest: Manifest,
        hash: String,
        source: Option<install::Source>,
    ) -> LoadJob {
        let e = &mut self.entries[i];
        // Overtakes an earlier swap still loading.
        e.generation += 1;
        e.update = None;
        LoadJob {
            id: e.id.clone(),
            wasm: e.dir.join(&manifest.entry),
            hash: hash.clone(),
            limits: self.limits,
            generation: e.generation,
            next: Some(Box::new(Next {
                manifest,
                hash,
                source,
            })),
        }
    }

    /// A load finished: start the plugin (its `init`) or record why not.
    pub fn loaded(&mut self, done: Loaded) -> Vec<Effect> {
        let Some(i) = self.index(&done.id) else {
            return Vec::new();
        };
        let e = &mut self.entries[i];
        if e.generation != done.generation {
            return Vec::new(); // overtaken (disabled, restarted or reloaded meanwhile)
        }
        if let Some(next) = done.next {
            return self.swapped(i, next, done.result);
        }
        if e.state != State::Loading {
            return Vec::new();
        }
        let name = e.name().to_owned();
        let sandbox = match done.result {
            Ok(s) => s,
            Err(LoadFailure::Changed) => {
                e.state = State::NeedsApproval("its module changed on disk since you approved it".into());
                return vec![Effect::Status(format!(
                    "plugin {name} changed: approve it again in Settings › Plugins (Ctrl-x › Plugins)"
                ))];
            }
            Err(LoadFailure::Error(why)) => {
                tracing::warn!("{}: {why}", e.id);
                e.state = State::Failed(why.clone());
                return vec![Effect::Status(format!("plugin {name} failed to start: {why}"))];
            }
        };
        let declared = e.manifest.as_ref().and_then(Manifest::api);
        if !sandbox.api.runs_on(HOST_API) || declared.is_none_or(|d| d.major != sandbox.api.major) {
            e.state = State::Incompatible(format!(
                "its module speaks plugin API {}; this rune speaks {HOST_API}",
                sandbox.api
            ));
            return vec![Effect::Status(format!("plugin {name} is incompatible"))];
        }
        e.sandbox = Some(sandbox);
        e.state = State::Active;
        self.init(i)
    }

    /// A new version of a running plugin loaded (or not): swap it in, or
    /// keep the old one and say why.
    fn swapped(&mut self, i: usize, next: Box<Next>, result: Result<Sandbox, LoadFailure>) -> Vec<Effect> {
        let e = &mut self.entries[i];
        if e.sandbox.is_none() {
            return Vec::new();
        }
        let name = e.name().to_owned();
        let broken = |e: &mut Entry, why: String| {
            tracing::warn!("{}: new version: {why}", e.id);
            let fx = Effect::Status(format!(
                "plugin {name}: the new version {why}; the running one stays"
            ));
            e.update = Some(Update::Broken(format!("the new version {why}")));
            vec![fx]
        };
        let sandbox = match result {
            Ok(s) => s,
            // Changed again while loading: the next look reloads it.
            Err(LoadFailure::Changed) => return Vec::new(),
            Err(LoadFailure::Error(why)) => return broken(e, format!("does not start: {why}")),
        };
        if !sandbox.api.runs_on(HOST_API) || next.manifest.api().is_none_or(|d| d.major != sandbox.api.major)
        {
            return broken(
                e,
                format!("speaks plugin API {}, this rune {HOST_API}", sandbox.api),
            );
        }
        // The old one's `shutdown` (it may save its data), then the new one.
        let mut fx = self.deactivate(i);
        let e = &mut self.entries[i];
        let Next {
            manifest,
            hash,
            source,
        } = *next;
        let version = manifest.version.clone();
        e.manifest = Some(manifest);
        e.hash = Some(hash);
        e.source = source;
        e.update = None;
        e.sandbox = Some(sandbox);
        e.state = State::Active;
        fx.push(Effect::Status(format!("reloaded {} {version}", e.name())));
        fx.extend(self.init(i));
        fx
    }

    /// A plugin just started: its settings, its `init`.
    fn init(&mut self, i: usize) -> Vec<Effect> {
        let config = self.load_config(i);
        self.entries[i].config = config;
        let mut fx = self.shadow_warnings(i);
        let data = if self.entries[i].has(Permission::Storage) {
            self.data(&self.entries[i].id)
        } else {
            BTreeMap::new()
        };
        let init = Event::Init {
            config: self.entries[i].config.clone(),
            data,
            host: self.host.clone(),
        };
        fx.extend(self.send(i, init));
        fx
    }

    /// Commands and keys another plugin (earlier by id) already has, or rune itself.
    fn shadow_warnings(&self, i: usize) -> Vec<Effect> {
        let Some(m) = &self.entries[i].manifest else {
            return Vec::new();
        };
        let mut fx = Vec::new();
        for c in &m.commands {
            if self.reserved.contains(&c.name.as_str()) {
                fx.push(Effect::Status(format!(
                    "plugin {}: /{} is rune's own; ignored",
                    m.name, c.name
                )));
            } else if let Some(o) = self.command_owner(&c.name).filter(|&o| o != i) {
                let other = self.entries[o].name();
                fx.push(Effect::Status(format!(
                    "plugin {}: /{} belongs to {other}",
                    m.name, c.name
                )));
            }
        }
        for k in &m.keys {
            if let Some(o) = k.alt_char().and_then(|c| self.key_owner(c)).filter(|&o| o != i) {
                let other = self.entries[o].name();
                fx.push(Effect::Status(format!(
                    "plugin {}: {} belongs to {other}",
                    m.name, k.key
                )));
            }
        }
        fx
    }

    /// The user enabled the plugin, having been shown its permissions.
    pub fn enable(&mut self, id: &str) -> Result<LoadJob, String> {
        let i = self.index(id).ok_or("no such plugin")?;
        let e = &self.entries[i];
        let fp = match (&e.state, e.fingerprint()) {
            (State::Invalid(s) | State::Incompatible(s), _) => return Err(s.clone()),
            (State::Missing, _) | (_, None) => return Err("it is not installed".into()),
            (State::Active | State::Loading, _) => return Err("it is already on".into()),
            (_, Some(fp)) => fp,
        };
        self.store
            .set(&approved_key(id), &fp)
            .map_err(|e| e.to_string())?;
        self.store
            .set(&enabled_key(id), "true")
            .map_err(|e| e.to_string())?;
        Ok(self.start(i))
    }

    /// Turn a plugin off (its `shutdown` runs first) and forget the approval.
    pub fn disable(&mut self, id: &str) -> Vec<Effect> {
        let Some(i) = self.index(id) else {
            return Vec::new();
        };
        let fx = self.deactivate(i);
        let _ = self.store.remove(&enabled_key(id));
        let _ = self.store.remove(&approved_key(id));
        let e = &mut self.entries[i];
        if e.update.is_some() {
            // It ran an older version than the one on disk: show that one.
            let found = inspect(&e.dir, self.limits);
            e.adopt(found);
        } else if e.manifest.is_some() && !matches!(e.state, State::Invalid(_) | State::Incompatible(_)) {
            e.state = State::Disabled;
        }
        fx
    }

    /// Stop a running plugin after its `shutdown`.
    fn deactivate(&mut self, i: usize) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.entries[i].sandbox.is_some() {
            fx = self.send_as(i, Event::Shutdown, Mode::ShuttingDown);
        }
        let e = &mut self.entries[i];
        e.stop();
        fx.push(Effect::CloseView {
            plugin: e.id.clone(),
            id: None,
        });
        fx
    }

    /// Reload a plugin from disk (after rebuilding it, or after a failure).
    /// A changed module or permission list needs approving again.
    pub fn restart(&mut self, id: &str) -> (Vec<Effect>, Option<LoadJob>) {
        let Some(i) = self.index(id) else {
            return (Vec::new(), None);
        };
        let fx = self.deactivate(i);
        let found = inspect(&self.entries[i].dir, self.limits);
        let e = &mut self.entries[i];
        e.adopt(found);
        if e.manifest.is_none() {
            return (fx, None);
        }
        let job = self.decide(i);
        (fx, job)
    }

    /// Quitting: every plugin's `shutdown`; only storage writes count.
    pub fn shutdown_all(&mut self) {
        for i in 0..self.entries.len() {
            if self.entries[i].sandbox.is_some() {
                self.send_as(i, Event::Shutdown, Mode::ShuttingDown);
                self.entries[i].stop();
            }
        }
    }

    /// Delete a plugin's settings, approval and data. An installed plugin
    /// is turned off first; a missing one disappears from the list.
    pub fn forget(&mut self, id: &str) -> Vec<Effect> {
        let fx = self.disable(id);
        for prefix in [format!("setting.plugin.{id}."), format!("plugin.{id}.")] {
            for (k, _) in self.store.values_with_prefix(&prefix).unwrap_or_default() {
                let _ = self.store.remove(&k);
            }
        }
        if let Some(i) = self.index(id)
            && self.entries[i].state == State::Missing
        {
            self.entries.remove(i);
        }
        fx
    }

    // -- changes on disk: hot reload, installs, updates ----------------------------

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Whether a changed module of this plugin loads without asking (as
    /// long as it asks for no new permissions).
    pub fn autoload(&self, id: &str) -> bool {
        self.store.get(&autoload_key(id)).ok().flatten().as_deref() == Some("true")
    }

    /// Approve `fp` in place of the approved fingerprint if the user lets
    /// this plugin's new versions load by themselves and the permissions
    /// are the same.
    fn auto_approve(&self, id: &str, fp: &str) -> bool {
        if !self.autoload(id) || !self.is_enabled(id) {
            return false;
        }
        let Some(approved) = self.store.get(&approved_key(id)).ok().flatten() else {
            return false;
        };
        perms_of(&approved) == perms_of(fp) && self.store.set(&approved_key(id), fp).is_ok()
    }

    /// Turn *Load new versions automatically* on or off. Turning it on
    /// loads a version that was waiting for approval, if it asks for no new
    /// permissions.
    pub fn set_autoload(&mut self, id: &str, on: bool) -> Result<Option<LoadJob>, String> {
        let i = self.index(id).ok_or("no such plugin")?;
        if on {
            self.store.set(&autoload_key(id), "true")
        } else {
            self.store.remove(&autoload_key(id))
        }
        .map_err(|e| e.to_string())?;
        if !on {
            return Ok(None);
        }
        let e = &self.entries[i];
        if let Some(Update::Approval {
            manifest,
            hash,
            source,
            ..
        }) = e.update.clone()
        {
            let fp = fingerprint_of(&manifest, &hash);
            return Ok(self
                .auto_approve(id, &fp)
                .then(|| self.swap(i, *manifest, hash, source)));
        }
        if matches!(e.state, State::NeedsApproval(_)) {
            return Ok(self.decide(i));
        }
        Ok(None)
    }

    /// The user approved the newer version waiting beside a running plugin.
    pub fn approve_update(&mut self, id: &str) -> Result<LoadJob, String> {
        let i = self.index(id).ok_or("no such plugin")?;
        let Some(Update::Approval {
            manifest,
            hash,
            source,
            ..
        }) = self.entries[i].update.clone()
        else {
            return Err("no new version is waiting".into());
        };
        self.store
            .set(&approved_key(id), &fingerprint_of(&manifest, &hash))
            .map_err(|e| e.to_string())?;
        if self.entries[i].sandbox.is_some() {
            Ok(self.swap(i, *manifest, hash, source))
        } else {
            self.entries[i].manifest = Some(*manifest);
            self.entries[i].hash = Some(hash);
            self.entries[i].source = source;
            self.entries[i].update = None;
            Ok(self.start(i))
        }
    }

    /// Look again at these plugin directories (the watcher saw them change,
    /// or an install, update or removal just finished): new plugins appear,
    /// removed ones stop, changed ones reload. Looking again at files that
    /// did not change does nothing, so a change reported twice is handled once.
    pub fn refresh(&mut self, ids: &[String]) -> (Vec<Effect>, Vec<LoadJob>) {
        let mut fx = Vec::new();
        let mut jobs = Vec::new();
        for id in ids {
            let (f, job) = self.refresh_one(id);
            fx.extend(f);
            jobs.extend(job);
        }
        (fx, jobs)
    }

    fn refresh_one(&mut self, id: &str) -> (Vec<Effect>, Option<LoadJob>) {
        let Some(root) = self.dir.clone() else {
            return (Vec::new(), None);
        };
        if id.starts_with('.') || id.contains(['/', '\\']) {
            return (Vec::new(), None);
        }
        let dir = root.join(id);
        let i = self.index(id);
        if !dir.is_dir() {
            return match i {
                Some(i) if self.entries[i].state != State::Missing => (self.removed(i), None),
                _ => (Vec::new(), None),
            };
        }
        let found = inspect(&dir, self.limits);
        let Some(i) = i else {
            // A new plugin.
            self.entries.push(Entry::from_found(found));
            let i = self.entries.len() - 1;
            let job = self.entries[i]
                .manifest
                .is_some()
                .then(|| self.decide(i))
                .flatten();
            let e = &self.entries[i];
            let msg = match &e.state {
                State::Disabled => format!(
                    "found plugin {} {}: enable it in Settings › Plugins (Ctrl-x › Plugins)",
                    e.name(),
                    e.manifest.as_ref().map_or("", |m| m.version.as_str())
                ),
                State::Loading => format!("starting {}…", e.name()),
                s => format!("plugin folder {id}: {}", s.detail().unwrap_or(s.label())),
            };
            return (vec![Effect::Status(msg)], job);
        };
        let seen = Seen::of(&found);
        if self.entries[i].state != State::Missing
            && let Some(old) = &self.entries[i].seen
            && old.manifest == seen.manifest
            && old.hash == seen.hash
        {
            // The same plugin; at most its source record changed (pinned,
            // or following something else at the same commit).
            let e = &mut self.entries[i];
            if e.update.is_none() || e.sandbox.is_none() {
                e.source = seen.source.clone();
            }
            e.seen = Some(seen);
            return (Vec::new(), None);
        }
        if self.entries[i].sandbox.is_some() {
            return self.changed_while_running(i, found);
        }
        // Not running: take the new files and decide again (a failed plugin
        // gets another try, a waiting one may now match its approval).
        let was = self.entries[i].state.clone();
        let e = &mut self.entries[i];
        e.stop();
        let name = e.name().to_owned();
        e.adopt(found);
        let job = if e.manifest.is_some() {
            self.decide(i)
        } else {
            None
        };
        let e = &self.entries[i];
        let msg = match &e.state {
            State::Loading if matches!(was, State::Missing) => format!("starting {}…", e.name()),
            State::Loading => format!("reloading {}…", e.name()),
            State::NeedsApproval(_) => format!(
                "plugin {} changed on disk: approve it again on its page (Settings › Plugins)",
                e.name()
            ),
            State::Disabled if was == State::Missing => format!(
                "found plugin {} again: enable it in Settings › Plugins (Ctrl-x › Plugins)",
                e.name()
            ),
            State::Disabled => format!("plugin {} changed on disk (it is off)", e.name()),
            State::Paused => return (Vec::new(), job),
            s => format!("plugin {name}: {}", s.detail().unwrap_or(s.label())),
        };
        (vec![Effect::Status(msg)], job)
    }

    /// New files under a running plugin: load them beside it when they are
    /// approved (or may be approved automatically), else keep it running
    /// and let the user decide.
    fn changed_while_running(&mut self, i: usize, found: Found) -> (Vec<Effect>, Option<LoadJob>) {
        let id = self.entries[i].id.clone();
        let name = self.entries[i].name().to_owned();
        self.entries[i].seen = Some(Seen::of(&found));
        let (m, hash) = match (found.manifest, found.hash) {
            (Ok(m), Some(h)) => (m, h),
            (Err(state), _) => {
                let why = state.detail().unwrap_or(state.label()).to_owned();
                self.entries[i].update =
                    Some(Update::Broken(format!("its files on disk are not usable: {why}")));
                let msg =
                    format!("plugin {name}: its new files are not usable ({why}); the running version stays");
                return (vec![Effect::Status(msg)], None);
            }
            (Ok(_), None) => return (Vec::new(), None),
        };
        let fp = fingerprint_of(&m, &hash);
        let e = &self.entries[i];
        if e.fingerprint().as_deref() == Some(fp.as_str()) && e.manifest.as_ref() == Some(&m) {
            // Same module and manifest (touched, or only its source record changed).
            self.entries[i].source = found.source;
            self.entries[i].update = None;
            return (Vec::new(), None);
        }
        let approved = self.store.get(&approved_key(&id)).ok().flatten();
        if approved.as_deref() == Some(fp.as_str()) || self.auto_approve(&id, &fp) {
            let job = self.swap(i, m, hash, found.source);
            return (vec![Effect::Status(format!("reloading {name}…"))], Some(job));
        }
        let had = e
            .manifest
            .as_ref()
            .map(|m| m.permissions.clone())
            .unwrap_or_default();
        let new_permissions: Vec<Permission> = m
            .permissions
            .iter()
            .copied()
            .filter(|p| !had.contains(p))
            .collect();
        let msg = format!(
            "plugin {name} changed on disk: approve the new version on its page (Settings › Plugins); \
             the running one stays until then"
        );
        self.entries[i].update = Some(Update::Approval {
            manifest: Box::new(m),
            hash,
            source: found.source,
            new_permissions,
        });
        (vec![Effect::Status(msg)], None)
    }

    /// Its directory is gone: stop it; keep an entry only for leftover
    /// settings or data.
    fn removed(&mut self, i: usize) -> Vec<Effect> {
        let name = self.entries[i].name().to_owned();
        let running = self.entries[i].sandbox.is_some();
        let mut fx = self.deactivate(i);
        let id = self.entries[i].id.clone();
        if self.remembered(&id) {
            let dir = self.entries[i].dir.clone();
            self.entries[i] = Entry::missing(id, dir);
        } else {
            self.entries.remove(i);
        }
        fx.push(Effect::Status(if running {
            format!("plugin {name} was removed from disk; stopped it")
        } else {
            format!("plugin {name} was removed from disk")
        }));
        fx
    }

    /// Delete a plugin rune installed from a repository: stopped first, its
    /// approval forgotten; its settings and data stay for a reinstall
    /// (*Forget* deletes them).
    pub fn uninstall(&mut self, id: &str) -> Result<Vec<Effect>, String> {
        let root = self.dir.clone().ok_or("no plugin directory")?;
        let i = self.index(id).ok_or("no such plugin")?;
        if self.entries[i].source.is_none() {
            return Err(format!(
                "it was not installed from a repository; delete {} yourself",
                self.entries[i].dir.display()
            ));
        }
        let mut fx = self.deactivate(i);
        let _ = self.store.remove(&enabled_key(id));
        let _ = self.store.remove(&approved_key(id));
        if let Err(e) = install::uninstall(&root, id) {
            // Still there: show it as it is now (off).
            let found = inspect(&self.entries[i].dir, self.limits);
            self.entries[i].adopt(found);
            return Err(e);
        }
        if let Some(i) = self.index(id) {
            if self.remembered(id) {
                let dir = self.entries[i].dir.clone();
                self.entries[i] = Entry::missing(id.to_owned(), dir);
            } else {
                self.entries.remove(i);
            }
        }
        fx.retain(|e| !matches!(e, Effect::Status(_)));
        Ok(fx)
    }

    // -- settings ------------------------------------------------------------------

    /// The plugin's settings: stored values that still pass the manifest's
    /// checks, defaults for the rest.
    fn load_config(&self, i: usize) -> Map<String, Value> {
        let e = &self.entries[i];
        let Some(m) = &e.manifest else {
            return Map::new();
        };
        let prefix = config_prefix(&e.id);
        m.settings
            .iter()
            .map(|s| {
                let stored = self.store.get(&format!("{prefix}{}", s.key)).ok().flatten();
                let v = stored
                    .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                    .and_then(|v| s.check(&v).ok())
                    .unwrap_or_else(|| s.default.clone());
                (s.key.clone(), v)
            })
            .collect()
    }

    /// Current settings of any installed plugin (running or not).
    pub fn config(&self, id: &str) -> Map<String, Value> {
        match self.index(id) {
            Some(i) if self.entries[i].sandbox.is_some() => self.entries[i].config.clone(),
            Some(i) => self.load_config(i),
            None => Map::new(),
        }
    }

    /// Change one setting (checked against the manifest); a running plugin
    /// hears about it.
    pub fn set_setting(&mut self, id: &str, key: &str, value: Value) -> Result<Vec<Effect>, String> {
        let i = self.index(id).ok_or("no such plugin")?;
        let spec = self.entries[i]
            .manifest
            .as_ref()
            .and_then(|m| m.settings.iter().find(|s| s.key == key))
            .ok_or("no such setting")?;
        let v = spec.check(&value)?;
        self.store
            .set(&format!("{}{key}", config_prefix(id)), &v.to_string())
            .map_err(|e| e.to_string())?;
        self.config_changed(i)
    }

    pub fn reset_settings(&mut self, id: &str) -> Result<Vec<Effect>, String> {
        let i = self.index(id).ok_or("no such plugin")?;
        for (k, _) in self
            .store
            .values_with_prefix(&config_prefix(id))
            .unwrap_or_default()
        {
            self.store.remove(&k).map_err(|e| e.to_string())?;
        }
        self.config_changed(i)
    }

    fn config_changed(&mut self, i: usize) -> Result<Vec<Effect>, String> {
        let config = self.load_config(i);
        self.entries[i].config = config.clone();
        if self.entries[i].sandbox.is_none() {
            return Ok(Vec::new());
        }
        Ok(self.send(i, Event::ConfigChanged { config }))
    }

    // -- data ---------------------------------------------------------------------

    /// The plugin's stored data, key → value.
    pub fn data(&self, id: &str) -> BTreeMap<String, String> {
        let prefix = data_prefix(id);
        self.store
            .values_with_prefix(&prefix)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?.to_owned(), v)))
            .collect()
    }

    /// Delete the plugin's stored data (a running plugin is restarted
    /// afterwards so it does not keep a stale copy).
    pub fn clear_data(&mut self, id: &str) -> (Vec<Effect>, Option<LoadJob>) {
        let running = self.get(id).is_some_and(|e| e.sandbox.is_some());
        let mut fx = Vec::new();
        if running && let Some(i) = self.index(id) {
            fx = self.deactivate(i);
        }
        for (k, _) in self
            .store
            .values_with_prefix(&data_prefix(id))
            .unwrap_or_default()
        {
            let _ = self.store.remove(&k);
        }
        let job = match self.index(id) {
            Some(i) if running => self.decide(i),
            _ => None,
        };
        (fx, job)
    }

    // -- events -------------------------------------------------------------------

    fn command_owner(&self, name: &str) -> Option<usize> {
        if self.reserved.contains(&name) {
            return None;
        }
        self.entries.iter().position(|e| {
            e.sandbox.is_some()
                && e.manifest
                    .as_ref()
                    .is_some_and(|m| m.commands.iter().any(|c| c.name == name))
        })
    }

    fn key_owner(&self, c: char) -> Option<usize> {
        self.entries.iter().position(|e| {
            e.sandbox.is_some()
                && e.manifest
                    .as_ref()
                    .is_some_and(|m| m.keys.iter().any(|k| k.alt_char() == Some(c)))
        })
    }

    /// `/name args` from the message line, if a running plugin has it; a
    /// command of a plugin that is not running says so.
    pub fn command(&mut self, name: &str, args: &str, chat: Option<ChatRef>) -> Option<Vec<Effect>> {
        if let Some(i) = self.command_owner(name) {
            let ev = Event::Command {
                name: name.to_owned(),
                args: args.to_owned(),
                chat,
            };
            return Some(self.send(i, ev));
        }
        if self.reserved.contains(&name) {
            return None;
        }
        let idle = self.entries.iter().find(|e| {
            e.manifest
                .as_ref()
                .is_some_and(|m| m.commands.iter().any(|c| c.name == name))
        })?;
        Some(vec![Effect::Status(format!(
            "/{name} comes from the plugin {}, which is {} (see Ctrl-x › Plugins)",
            idle.name(),
            idle.state.label()
        ))])
    }

    /// `Alt-<c>`, if a running plugin binds it.
    pub fn key(&mut self, c: char, chat: Option<ChatRef>) -> Option<Vec<Effect>> {
        let i = self.key_owner(c)?;
        let id = self.entries[i]
            .manifest
            .as_ref()?
            .keys
            .iter()
            .find(|k| k.alt_char() == Some(c))?
            .id
            .clone();
        Some(self.send(i, Event::Key { id, chat }))
    }

    /// To one plugin, by id, if it is running.
    pub fn deliver(&mut self, id: &str, ev: Event) -> Vec<Effect> {
        match self.index(id) {
            Some(i) if self.entries[i].sandbox.is_some() => self.send(i, ev),
            _ => Vec::new(),
        }
    }

    /// A new message, to every running plugin allowed to read messages.
    pub fn message(&mut self, chat: ChatRef, message: MessageInfo) -> Vec<Effect> {
        self.broadcast(Permission::ReadMessages, Event::Message { chat, message })
    }

    pub fn chat_opened(&mut self, chat: ChatRef) -> Vec<Effect> {
        self.broadcast(Permission::ReadMessages, Event::ChatOpened { chat })
    }

    fn broadcast(&mut self, need: Permission, ev: Event) -> Vec<Effect> {
        let mut fx = Vec::new();
        for i in 0..self.entries.len() {
            if self.entries[i].sandbox.is_some() && self.entries[i].has(need) {
                fx.extend(self.send(i, ev.clone()));
            }
        }
        fx
    }

    /// The soonest timer, for the event loop to wake up at.
    pub fn next_timer(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter(|e| e.sandbox.is_some())
            .flat_map(|e| e.timers.iter().map(|t| t.at))
            .min()
    }

    /// Fire the timers due by `now`.
    pub fn run_timers(&mut self, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        for i in 0..self.entries.len() {
            let mut due = Vec::new();
            let e = &mut self.entries[i];
            e.timers.retain_mut(|t| {
                if t.at > now {
                    return true;
                }
                due.push(t.id.clone());
                match t.every {
                    Some(every) => {
                        t.at = now + every;
                        true
                    }
                    None => false,
                }
            });
            for id in due {
                if self.entries[i].sandbox.is_none() {
                    break; // failed on an earlier timer
                }
                fx.extend(self.send(i, Event::Timer { id }));
            }
        }
        fx
    }

    /// Commands of running plugins, for the help screen: usage, help.
    pub fn commands_help(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(m) = e.manifest.as_ref().filter(|_| e.sandbox.is_some()) else {
                continue;
            };
            for c in &m.commands {
                if self.command_owner(&c.name) == Some(i) {
                    let usage = if c.usage.is_empty() {
                        format!("/{}", c.name)
                    } else {
                        clean(&c.usage, 40, false)
                    };
                    out.push((usage, format!("{} ({})", c.help, m.name)));
                }
            }
        }
        out
    }

    /// Keys of running plugins, for the help screen: key, help.
    pub fn keys_help(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(m) = e.manifest.as_ref().filter(|_| e.sandbox.is_some()) else {
                continue;
            };
            for k in &m.keys {
                if let Some(c) = k.alt_char().filter(|&c| self.key_owner(c) == Some(i)) {
                    out.push((format!("Alt-{c}"), format!("{} ({})", k.help, m.name)));
                }
            }
        }
        out
    }

    // -- calling a plugin ------------------------------------------------------------

    fn send(&mut self, i: usize, ev: Event) -> Vec<Effect> {
        let mode = if ev.is_interactive() {
            Mode::Interactive
        } else {
            Mode::Background
        };
        self.send_as(i, ev, mode)
    }

    fn send_as(&mut self, i: usize, ev: Event, mode: Mode) -> Vec<Effect> {
        let env = Envelope {
            now: chrono::Utc::now().timestamp_millis(),
            event: ev,
        };
        let input = serde_json::to_vec(&env).expect("events serialize");
        let Some(sandbox) = self.entries[i].sandbox.as_mut() else {
            return Vec::new();
        };
        let reply = match sandbox.call(&input) {
            Ok(r) => r,
            Err(why) => return self.fail(i, why),
        };
        let actions = match serde_json::from_slice::<Value>(&reply) {
            Ok(Value::Object(mut o)) => match o.remove("actions") {
                Some(Value::Array(a)) => a,
                None => Vec::new(),
                Some(_) => return self.fail(i, "replied without an action list".into()),
            },
            _ => return self.fail(i, "replied with something that is not JSON".into()),
        };
        let mut fx = Vec::new();
        if actions.len() > ACTIONS_PER_REPLY {
            fx.push(self.refused(
                i,
                &format!(
                    "sent {} actions; only {ACTIONS_PER_REPLY} were taken",
                    actions.len()
                ),
            ));
        }
        for raw in actions.into_iter().take(ACTIONS_PER_REPLY) {
            match serde_json::from_value::<Action>(raw) {
                Ok(a) => {
                    if let Some(e) = self.act(i, a, mode) {
                        fx.push(e);
                    }
                }
                Err(e) => fx.push(self.refused(i, &format!("sent an action rune does not know ({e})"))),
            }
        }
        fx
    }

    /// Stop a plugin that misbehaved; the app keeps going.
    fn fail(&mut self, i: usize, why: String) -> Vec<Effect> {
        let e = &mut self.entries[i];
        tracing::warn!("{} stopped: {why}", e.id);
        e.stop();
        e.state = State::Failed(why.clone());
        vec![
            Effect::CloseView {
                plugin: e.id.clone(),
                id: None,
            },
            Effect::Status(format!("plugin {} stopped: {why}", e.name())),
        ]
    }

    fn refused(&self, i: usize, why: &str) -> Effect {
        let e = &self.entries[i];
        tracing::warn!("{}: {why}", e.id);
        Effect::Status(format!("plugin {}: {why}", e.name()))
    }

    fn only_interactive(&self, i: usize, what: &str) -> Option<Effect> {
        Some(self.refused(i, &format!("{what} only answers something you did; ignored")))
    }

    /// A refusal, unless the plugin has `p`.
    fn need(&self, i: usize, p: Permission, what: &str) -> Option<Effect> {
        (!self.entries[i].has(p))
            .then(|| self.refused(i, &format!("{what} needs the {} permission; refused", p.as_str())))
    }

    /// One action, checked: an effect for the app, or done here (storage,
    /// timers), or refused with a word on the status line.
    fn act(&mut self, i: usize, a: Action, mode: Mode) -> Option<Effect> {
        let interactive = mode == Mode::Interactive;
        let plugin = self.entries[i].id.clone();
        let name = self.entries[i].name().to_owned();
        if mode == Mode::ShuttingDown
            && !matches!(
                a,
                Action::Status { .. } | Action::StoreSet { .. } | Action::StoreDelete { .. }
            )
        {
            return None;
        }
        match a {
            Action::Status { text } => Some(Effect::Status(format!("{name}: {}", clean(&text, 200, false)))),
            Action::Notify { title, body } => {
                self.need(i, Permission::Notify, "a notification").or_else(|| {
                    Some(Effect::Notify {
                        title: clean(&title, 100, false),
                        body: clean(&body, 300, false),
                    })
                })
            }
            Action::OpenView {
                id,
                title,
                lines,
                selected,
                keys,
            } => {
                let lines: Vec<ViewLine> = lines
                    .into_iter()
                    .take(VIEW_LINES)
                    .map(|l| ViewLine {
                        text: clean(&l.text, LINE_CHARS, false),
                        style: l.style,
                    })
                    .collect();
                let keys = keys
                    .into_iter()
                    .take(16)
                    .map(|k| KeyHelp {
                        key: clean(&k.key, 16, false),
                        help: clean(&k.help, 60, false),
                    })
                    .collect();
                Some(Effect::OpenView {
                    view: View {
                        plugin,
                        id: clean(&id, 64, false),
                        title: clean(&title, 60, false),
                        selected: selected.filter(|&s| s < lines.len()),
                        lines,
                        keys,
                    },
                    interactive,
                })
            }
            Action::CloseView { id } => Some(Effect::CloseView { plugin, id: Some(id) }),
            Action::Form { .. } if !interactive => self.only_interactive(i, "a form"),
            Action::Form { id, title, fields } => Some(Effect::Form {
                plugin,
                id: clean(&id, 64, false),
                title: clean(&title, 60, false),
                fields: fields
                    .into_iter()
                    .take(FORM_FIELDS)
                    .map(|f| FormField {
                        label: clean(&f.label, 40, false),
                        value: clean(&f.value, 1000, false),
                        hint: clean(&f.hint, 60, false),
                    })
                    .collect(),
            }),
            Action::InsertText { text } => {
                if let Some(e) = self.need(i, Permission::Compose, "typing into the message line") {
                    return Some(e);
                }
                if !interactive {
                    return self.only_interactive(i, "typing into the message line");
                }
                Some(Effect::InsertText(clean(&text, MESSAGE_CHARS, true)))
            }
            Action::SendMessage { chat, text } => {
                if let Some(e) = self.need(i, Permission::SendMessages, "sending a message") {
                    return Some(e);
                }
                let text = clean(text.trim(), MESSAGE_CHARS, true);
                if text.is_empty() {
                    return None;
                }
                let now = Instant::now();
                let sends = &mut self.entries[i].sends;
                while sends
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
                {
                    sends.pop_front();
                }
                if sends.len() >= SENDS_PER_MINUTE {
                    return Some(self.refused(
                        i,
                        &format!("more than {SENDS_PER_MINUTE} messages a minute; refused"),
                    ));
                }
                sends.push_back(now);
                Some(Effect::SendMessage { plugin, chat, text })
            }
            Action::StoreSet { key, value } => {
                if let Some(e) = self.need(i, Permission::Storage, "storing data") {
                    return Some(e);
                }
                if !is_key(&key, DATA_KEY_LEN) || value.len() > DATA_VALUE_LEN {
                    return Some(self.refused(
                        i,
                        &format!(
                            "storage keys are 1–{DATA_KEY_LEN} of A–Z a–z 0–9 _ . - and values at most {} KB",
                            DATA_VALUE_LEN >> 10
                        ),
                    ));
                }
                let full = format!("{}{key}", data_prefix(&plugin));
                let exists = self.store.get(&full).ok().flatten().is_some();
                if !exists && self.data(&plugin).len() >= DATA_KEYS {
                    return Some(self.refused(i, &format!("storage is full ({DATA_KEYS} keys)")));
                }
                self.store
                    .set(&full, &value)
                    .err()
                    .map(|e| self.refused(i, &format!("could not store {key}: {e}")))
            }
            Action::StoreDelete { key } => {
                if let Some(e) = self.need(i, Permission::Storage, "storing data") {
                    return Some(e);
                }
                let _ = self.store.remove(&format!("{}{key}", data_prefix(&plugin)));
                None
            }
            Action::SetTimer { id, after_ms, repeat } => {
                let after = Duration::from_millis(after_ms).clamp(TIMER_MIN, TIMER_MAX);
                // Stored (and compared) as cleaned, so setting the same id
                // again replaces the timer and cancelling it finds it.
                let id = clean(&id, TIMER_ID_CHARS, false);
                let timers = &mut self.entries[i].timers;
                timers.retain(|t| t.id != id);
                if timers.len() >= TIMERS {
                    return Some(self.refused(i, &format!("at most {TIMERS} timers")));
                }
                timers.push(Timer {
                    id,
                    at: Instant::now() + after,
                    every: repeat.then_some(after),
                });
                None
            }
            Action::CancelTimer { id } => {
                let id = clean(&id, TIMER_ID_CHARS, false);
                self.entries[i].timers.retain(|t| t.id != id);
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Answering something the user just did.
    Interactive,
    Background,
    /// Answering `shutdown`: only status and storage.
    ShuttingDown,
}
