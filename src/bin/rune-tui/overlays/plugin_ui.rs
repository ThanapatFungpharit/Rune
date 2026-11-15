//! Plugins on screen: a plugin's page (Settings › Plugins lists them; `Esc`
//! goes back there), a plugin's view, and the app's side of the plugin host —
//! turning plugin effects into app actions and app events into plugin events,
//! installing and updating plugins from Git (off the UI thread) and watching
//! the plugin directory for changed files (hot reload).

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use crate::*;
use plugins::versions::{Track, Version, VersionReq};
use plugins::{ChatKind, ChatRef, Effect, LineStyle, MessageInfo, State, Update, View, install, watch};
use rune_plugin::manifest::{SettingSpec, SettingType};

/// How often the plugin directory is looked at for changed files.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// A Git install or update, as it goes.
pub enum InstallEvent {
    Progress(String),
    Installed(Result<install::Outcome, String>),
    /// Plugin id, outcome.
    Updated(String, Result<install::Outcome, String>),
    /// A plugin's repository's versions, to choose from.
    Versions(String, Result<install::Versions, String>),
    /// *Check all for new versions*: each Git-installed plugin's outcome.
    CheckedAll(Vec<(String, Result<install::Outcome, String>)>),
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// A plugin's view while it is open.
pub struct PluginView {
    pub view: View,
    /// First line shown, when the view has no selection to follow.
    scroll: usize,
}

/// One plugin's page: its state, permissions, settings and data.
pub struct Manager {
    plugin: String,
    sel: usize,
}

impl PluginView {
    #[cfg(test)]
    pub fn new(view: View) -> Self {
        PluginView { view, scroll: 0 }
    }
}

impl Manager {
    pub fn sel(&self) -> usize {
        self.sel
    }

    pub fn select(&mut self, i: usize) {
        self.sel = i;
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RowAct {
    None,
    Toggle,
    Restart,
    /// Load the newer version waiting for approval.
    ApproveUpdate,
    /// Load new versions automatically / ask first.
    AutoLoad,
    /// Fetch the newest commit of its repository.
    Update,
    /// Choose a version, or what updates follow.
    Versions,
    Uninstall,
    Setting(String),
    ResetSettings,
    ClearData,
    Forget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowStyle {
    Normal,
    Note,
    Heading,
    Problem,
}

struct Row {
    label: String,
    value: String,
    act: RowAct,
    style: RowStyle,
}

fn row(label: impl Into<String>, value: impl Into<String>, act: RowAct) -> Row {
    Row {
        label: label.into(),
        value: value.into(),
        act,
        style: RowStyle::Normal,
    }
}

fn note(label: impl Into<String>, style: RowStyle) -> Row {
    Row {
        label: label.into(),
        value: String::new(),
        act: RowAct::None,
        style,
    }
}

fn setting_value(spec: &SettingSpec, v: Option<&serde_json::Value>) -> String {
    match (spec.kind, v) {
        (SettingType::Bool, Some(v)) => if v.as_bool() == Some(true) { "On" } else { "Off" }.into(),
        (_, Some(serde_json::Value::String(s))) => s.clone(),
        (_, Some(v)) => v.to_string(),
        (_, None) => "?".into(),
    }
}

/// The name a key has in [`plugins::Event::ViewKey`].
pub fn key_name(k: &KeyEvent) -> Option<String> {
    if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return None;
    }
    Some(match k.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift-Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        _ => return None,
    })
}

impl App {
    // -- the host side -------------------------------------------------------------

    /// Find the installed plugins, start the enabled ones (off the UI
    /// thread) and watch the plugin directory for changes.
    pub fn start_plugins(&mut self) {
        // The first look comes before discovery, so nothing changed in
        // between goes unnoticed.
        let base = self.plugins.dir.clone().map(|d| {
            install::clean_staging(&d);
            (watch::snapshot(&d), d)
        });
        for job in self.plugins.scan() {
            self.spawn_plugin_load(job);
        }
        if let Some((base, dir)) = base {
            self.spawn_plugin_watch(dir, base);
        }
    }

    /// Look at the plugin directory every [`WATCH_EVERY`]; settled changes
    /// arrive as [`AppEvent::PluginFiles`].
    fn spawn_plugin_watch(&self, dir: PathBuf, base: watch::Snapshot) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut watcher = watch::Watcher::new(base);
            let mut every = tokio::time::interval(WATCH_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while !tx.is_closed() {
                every.tick().await;
                let d = dir.clone();
                let Ok(snap) = tokio::task::spawn_blocking(move || watch::snapshot(&d)).await else {
                    break;
                };
                let ids = watcher.look(snap);
                if !ids.is_empty() && tx.send(AppEvent::PluginFiles(ids)).is_err() {
                    break;
                }
            }
        });
    }

    /// Plugin directories changed (or an install finished): reload what
    /// changed. Whether a plugin is now loading.
    pub fn refresh_plugins(&mut self, ids: &[String]) -> bool {
        let (fx, jobs) = self.plugins.refresh(ids);
        self.apply_plugin_effects(fx);
        let loading = !jobs.is_empty();
        for job in jobs {
            self.spawn_plugin_load(job);
        }
        // A page of a plugin that is gone goes back to the list.
        if let Some(Modal::Plugins(m)) = &self.modal
            && self.plugins.get(&m.plugin).is_none()
        {
            let id = m.plugin.clone();
            self.open_settings_on_plugin(&id);
        }
        loading
    }

    // -- installing from Git ------------------------------------------------------------

    pub fn install_plugin_form(&mut self) {
        let field = |label: &'static str, hint: &'static str| modal::Field {
            label: label.into(),
            value: String::new().into(),
            hint: hint.into(),
        };
        self.modal = Some(Modal::Form(modal::Form {
            title: " Install a plugin from Git ".into(),
            kind: modal::FormKind::PluginInstall,
            fields: vec![
                field("Repository", "https://…, git@host:path or a local path"),
                field(
                    "Version",
                    "empty: default branch · latest · ^1.2 · v1.2.0 · a branch or commit",
                ),
                field("Folder", "where its plugin.json is; empty: found by itself"),
            ],
            sel: 0,
            error: None,
        }));
    }

    /// Run one Git job off the UI thread (one at a time): `job` gets the
    /// plugin directory, the limits and a progress callback.
    fn spawn_git_job<F>(&mut self, busy: String, status: String, job: F)
    where
        F: FnOnce(&std::path::Path, plugins::runtime::Limits, &dyn Fn(String)) -> InstallEvent
            + Send
            + 'static,
    {
        let Some(dir) = self.plugins.dir.clone() else {
            self.status = "no plugin directory (start rune with --plugins DIR)".into();
            return;
        };
        if let Some(job) = &self.plugin_job {
            self.status = format!("wait: {job}");
            return;
        }
        self.plugin_job = Some(busy);
        self.status = status;
        let (tx, limits) = (self.tx.clone(), self.plugins.limits());
        tokio::task::spawn_blocking(move || {
            let progress = |s: String| {
                let _ = tx.send(AppEvent::PluginInstall(InstallEvent::Progress(s)));
            };
            let done = job(&dir, limits, &progress);
            let _ = tx.send(AppEvent::PluginInstall(done));
        });
    }

    /// Fetch, check and install (off the UI thread); the plugin is off
    /// until the user enables it.
    pub fn install_plugin(&mut self, req: install::Request) {
        let status = format!("installing from {}…", req.url);
        self.spawn_git_job("installing…".into(), status, move |dir, limits, progress| {
            InstallEvent::Installed(install::install(dir, &req, limits, progress))
        });
    }

    /// Bring a plugin up to date with what it follows (`to: None`), or make
    /// it follow something else (a version, a range, a branch).
    fn update_plugin(&mut self, id: String, to: Option<Track>) {
        let name = self.plugin_name(&id);
        let status = match &to {
            None => format!("checking {name} for a new version…"),
            Some(t) => format!("switching {name} to {}…", t.describe()),
        };
        self.spawn_git_job(
            format!("checking {name}…"),
            status,
            move |dir, limits, progress| {
                let res = install::update(dir, &id, to, limits, progress);
                InstallEvent::Updated(id, res)
            },
        );
    }

    /// The menu of a plugin's versions (looked up first).
    fn choose_plugin_version(&mut self, id: String) {
        let name = self.plugin_name(&id);
        let status = format!("looking up the versions of {name}…");
        self.spawn_git_job("looking up versions…".into(), status, move |dir, _, _| {
            let res = install::versions(dir, &id);
            InstallEvent::Versions(id, res)
        });
    }

    /// Settings › Plugins › *Check all for new versions*: every plugin
    /// installed from Git, one after another (pinned ones only report).
    pub fn check_all_plugins(&mut self) {
        let ids: Vec<String> = self
            .plugins
            .entries
            .iter()
            .filter(|e| e.source.is_some())
            .map(|e| e.id.clone())
            .collect();
        if ids.is_empty() {
            self.status = "no plugin was installed from Git".into();
            return;
        }
        let status = format!("checking {} plugins for new versions…", ids.len());
        self.spawn_git_job("checking all…".into(), status, move |dir, limits, progress| {
            let n = ids.len();
            let results = ids
                .into_iter()
                .enumerate()
                .map(|(k, id)| {
                    progress(format!("checking {id} ({}/{n})…", k + 1));
                    let res = install::update(dir, &id, None, limits, progress);
                    (id, res)
                })
                .collect();
            InstallEvent::CheckedAll(results)
        });
    }

    /// Pin a plugin to its installed version (no network).
    pub fn pin_plugin(&mut self, id: &str) {
        let name = self.plugin_name(id);
        let Some(dir) = self.plugins.dir.clone() else {
            return;
        };
        match install::pin(&dir, id) {
            Ok(track) => {
                self.refresh_plugins(&[id.to_owned()]);
                self.status = format!(
                    "{name} is {}: checks report newer versions but install nothing",
                    track.describe()
                );
            }
            Err(e) => self.status = format!("cannot pin {name}: {e}"),
        }
        self.open_plugin_page(id.to_owned());
    }

    /// The status line for one update outcome (after refreshing the plugin).
    fn update_outcome(&mut self, id: &str, res: Result<install::Outcome, String>) -> String {
        let name = self.plugin_name(id);
        let before = self
            .plugins
            .get(id)
            .and_then(|e| e.source.as_ref())
            .map(|s| s.track.clone());
        let o = match res {
            Ok(o) => o,
            Err(e) => return format!("{name} not updated: {e}"),
        };
        let switched = before.as_ref() != Some(&o.track);
        let loading = self.refresh_plugins(&[id.to_owned()]);
        match &o.available {
            Some(a) => self.plugin_available.insert(id.to_owned(), a.clone()),
            None => self.plugin_available.remove(id),
        };
        let at = if o.tag.is_empty() {
            short(&o.commit).to_owned()
        } else {
            o.tag.clone()
        };
        let available = |o: &install::Outcome| match &o.available {
            Some(a) if o.track.is_pinned() => format!("; {a} is available (Updates on its page)"),
            Some(a) => format!("; {a} is available but outside what it follows"),
            None => String::new(),
        };
        if o.up_to_date {
            return match &o.track {
                _ if switched => format!("{name} now follows the {} (already at {at})", o.track.describe()),
                Track::Pinned(_) => format!("{name} is pinned to {at}{}", available(&o)),
                t => format!("{name} is up to date ({at}, {}){}", t.describe(), available(&o)),
            };
        }
        let (old_version, old_commit) = o.previous.clone().unwrap_or_default();
        let change = if old_version != o.version && !old_version.is_empty() {
            format!("{old_version} → {}", o.version)
        } else {
            format!("{} → {at}", short(&old_commit))
        };
        let next = match self.plugins.get(id) {
            _ if loading => "; loading it…".to_owned(),
            Some(e)
                if matches!(e.update(), Some(Update::Approval { .. }))
                    || matches!(e.state, State::NeedsApproval(_)) =>
            {
                "; approve the new version on its page to run it".to_owned()
            }
            Some(e) => match (e.update(), e.state.detail()) {
                (Some(Update::Broken(why)), _) => format!("; but {why}"),
                (None, Some(why)) if e.state.is_problem() => format!("; but {why}"),
                _ => String::new(),
            },
            None => String::new(),
        };
        let verb = if switched { "switched" } else { "updated" };
        format!("{verb} {name} {change}{next}{}", available(&o))
    }

    /// The version menu, over the plugin's page.
    fn version_menu(&mut self, id: &str, v: install::Versions) {
        let Some(e) = self.plugins.get(id) else {
            return;
        };
        let Some(src) = e.source.clone() else {
            return;
        };
        let installed =
            Version::parse(&src.tag).or_else(|| e.manifest.as_ref().and_then(|m| Version::parse(&m.version)));
        let mark = |t: &Track| if *t == src.track { "  ✓" } else { "" };
        let mut follow = Vec::new();
        let newest = |req: VersionReq| {
            v.releases
                .iter()
                .find(|(_, ver)| req.matches(ver))
                .map(|(t, _)| t.clone())
        };
        if let Some(n) = newest(VersionReq::Any) {
            let t = Track::Releases("*".into());
            follow.push((
                format!("Newest release{}\tnow {n}", mark(&t)),
                Action::PluginTrack(id.to_owned(), t),
            ));
        }
        if let Some(have) = &installed {
            let req = if have.major > 0 {
                VersionReq::Caret(have.major, None, None)
            } else {
                VersionReq::Caret(0, Some(have.minor), None)
            };
            if let Some(n) = newest(req) {
                let t = Track::Releases(req.to_string());
                follow.push((
                    format!("Newest compatible ({req}) release{}\tnow {n}", mark(&t)),
                    Action::PluginTrack(id.to_owned(), t),
                ));
            }
        }
        let branch = v
            .default_branch
            .clone()
            .unwrap_or_else(|| "the default branch".into());
        let t = Track::default();
        follow.push((
            format!("Newest commit on {branch}{}", mark(&t)),
            Action::PluginTrack(id.to_owned(), t),
        ));
        if let Track::Branch(b) = &src.track
            && !b.is_empty()
            && Some(b) != v.default_branch.as_ref()
        {
            follow.push((
                format!("Newest commit on {b}  ✓"),
                Action::PluginTrack(id.to_owned(), src.track.clone()),
            ));
        }
        if !src.track.is_pinned() {
            follow.push((
                format!("Pin to the installed version\t{}", src.version_label()),
                Action::PinPlugin(id.to_owned()),
            ));
        }
        let pins = v
            .releases
            .iter()
            .take(20)
            .map(|(tag, ver)| {
                let what = match () {
                    _ if *tag == src.tag => "installed",
                    _ if ver.pre.is_some() => "pre-release",
                    _ if installed.as_ref().is_some_and(|i| ver < i) => "older",
                    _ => "",
                };
                let t = Track::Pinned(tag.clone());
                (
                    format!("Pin to {tag}{}\t{what}", mark(&t)),
                    Action::PluginTrack(id.to_owned(), t),
                )
            })
            .collect();
        let title = format!(
            " {} › version (now {}) ",
            self.plugin_name(id),
            src.version_label()
        );
        self.open_grouped_menu(title, vec![follow, pins]);
    }

    pub fn on_plugin_install(&mut self, ev: InstallEvent) {
        match ev {
            InstallEvent::Progress(s) => {
                if self.plugin_job.is_some() {
                    self.status = s;
                }
            }
            InstallEvent::Installed(res) => {
                self.plugin_job = None;
                let o = match res {
                    Ok(o) => o,
                    Err(e) => {
                        self.status = format!("install failed: {e}");
                        return;
                    }
                };
                self.refresh_plugins(std::slice::from_ref(&o.id));
                self.status = format!(
                    "installed {} {} ({}): it is off until you enable it on its page",
                    o.name,
                    o.version,
                    short(&o.commit)
                );
                // Waiting on Settings › Plugins: show where it is.
                if matches!(&self.modal, Some(Modal::Settings(s)) if s.tab == settings::Tab::Plugins) {
                    self.open_settings_on_plugin(&o.id);
                }
            }
            InstallEvent::Updated(id, res) => {
                self.plugin_job = None;
                self.status = self.update_outcome(&id, res);
            }
            InstallEvent::Versions(id, res) => {
                self.plugin_job = None;
                let on_page = matches!(&self.modal, Some(Modal::Plugins(m)) if m.plugin == id);
                match res {
                    Ok(v) if on_page => {
                        self.status = format!("{} releases", v.releases.len());
                        self.version_menu(&id, v);
                    }
                    Ok(_) => {
                        self.status =
                            format!("versions of {} found: choose on its page", self.plugin_name(&id))
                    }
                    Err(e) => {
                        self.status = format!("cannot list the versions of {}: {e}", self.plugin_name(&id))
                    }
                }
            }
            InstallEvent::CheckedAll(results) => {
                self.plugin_job = None;
                let (mut updated, mut current, mut failed) = (Vec::new(), 0, Vec::new());
                let mut available = Vec::new();
                for (id, res) in results {
                    let ok = res
                        .as_ref()
                        .map(|o| (o.up_to_date, o.available.clone()))
                        .map_err(drop);
                    let line = self.update_outcome(&id, res);
                    match ok {
                        Ok((true, a)) => {
                            current += 1;
                            if let Some(a) = a {
                                available.push(format!("{} {a}", self.plugin_name(&id)));
                            }
                        }
                        Ok((false, _)) => updated.push(line),
                        Err(_) => failed.push(line),
                    }
                }
                let mut parts = updated;
                if current > 0 {
                    parts.push(format!("{current} up to date"));
                }
                if !available.is_empty() {
                    parts.push(format!("available but not followed: {}", available.join(", ")));
                }
                parts.extend(failed);
                self.status = parts.join(" · ");
            }
        }
    }

    /// A choice from the version menu.
    pub fn switch_plugin_track(&mut self, id: &str, track: Track) {
        self.open_plugin_page(id.to_owned());
        self.update_plugin(id.to_owned(), Some(track));
    }

    pub fn uninstall_plugin(&mut self, id: &str) {
        let name = self.plugin_name(id);
        if let Some(job) = &self.plugin_job {
            self.status = format!("wait: {job}");
            return self.open_plugin_page(id.to_owned());
        }
        match self.plugins.uninstall(id) {
            Ok(fx) => {
                self.apply_plugin_effects(fx);
                self.status = if self.plugins.get(id).is_some() {
                    format!("uninstalled {name}; its settings and data are kept (Forget it deletes them)")
                } else {
                    format!("uninstalled {name}")
                };
            }
            Err(e) => self.status = format!("cannot uninstall {name}: {e}"),
        }
        self.open_plugin_page(id.to_owned());
    }

    /// Loading a changed module runs new code: say what it may do and ask.
    fn ask_approve_update(&mut self, id: &str) {
        let Some(Update::Approval {
            manifest,
            new_permissions,
            ..
        }) = self.plugins.get(id).and_then(|e| e.update())
        else {
            return;
        };
        let may = if manifest.permissions.is_empty() {
            "show its own view and status lines".to_owned()
        } else {
            manifest
                .permissions
                .iter()
                .map(|p| {
                    let new = if new_permissions.contains(p) { " (new)" } else { "" };
                    format!("{}{new}", p.describe())
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        let text = format!(
            "Load {} {}? Its module changed on disk. It will be able to: {may}. Only load versions you trust.",
            manifest.name, manifest.version
        );
        self.confirm(text, Action::ApprovePluginUpdate(id.to_owned()));
    }

    pub fn approve_plugin_update(&mut self, id: &str) {
        match self.plugins.approve_update(id) {
            Ok(job) => {
                self.spawn_plugin_load(job);
                self.status = format!("loading the new version of {}…", self.plugin_name(id));
            }
            Err(e) => self.status = format!("{}: {e}", self.plugin_name(id)),
        }
        self.open_plugin_page(id.to_owned());
    }

    pub fn set_plugin_autoload(&mut self, id: &str, on: bool) {
        let name = self.plugin_name(id);
        match self.plugins.set_autoload(id, on) {
            Ok(Some(job)) => {
                self.spawn_plugin_load(job);
                self.status = format!("loading the new version of {name}…");
            }
            Ok(None) if on => {
                self.status =
                    format!("{name}: new versions load by themselves unless they ask for more permissions")
            }
            Ok(None) => self.status = format!("{name}: new versions wait for your approval"),
            Err(e) => self.status = format!("{name}: {e}"),
        }
        self.open_plugin_page(id.to_owned());
    }

    fn spawn_plugin_load(&self, job: plugins::LoadJob) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(done) = tokio::task::spawn_blocking(move || job.run()).await {
                let _ = tx.send(AppEvent::PluginLoaded(Box::new(done)));
            }
        });
    }

    /// How plugins see a chat.
    pub fn chat_ref(&self, mid: &str) -> ChatRef {
        let kind = match MidType::of(mid) {
            MidType::User => ChatKind::User,
            MidType::Group => ChatKind::Group,
            MidType::Room => ChatKind::Room,
            _ => ChatKind::Other,
        };
        ChatRef {
            mid: mid.to_owned(),
            name: self.name(mid),
            kind,
        }
    }

    fn open_chat_ref(&self) -> Option<ChatRef> {
        self.open.as_deref().map(|m| self.chat_ref(m))
    }

    /// A new message, for plugins that may read messages.
    pub fn plugins_hear(&mut self, chat: &str, m: &Message) {
        let info = MessageInfo {
            id: m.id.clone(),
            from: m.from.clone(),
            from_name: self.name(&m.from),
            text: (m.content_type == ct::TEXT).then(|| m.text.clone()).flatten(),
            summary: preview_of(m),
            time: m.created_time.unwrap_or_else(now_ms),
            mine: m.from == self.me,
        };
        let fx = self.plugins.message(self.chat_ref(chat), info);
        self.apply_plugin_effects(fx);
    }

    pub fn plugins_chat_opened(&mut self, mid: &str) {
        let fx = self.plugins.chat_opened(self.chat_ref(mid));
        self.apply_plugin_effects(fx);
    }

    /// `/name args` for a plugin; `false` if no plugin knows the command.
    pub fn plugin_command(&mut self, name: &str, args: &str) -> bool {
        let chat = self.open_chat_ref();
        match self.plugins.command(name, args, chat) {
            Some(fx) => {
                self.apply_plugin_effects(fx);
                true
            }
            None => false,
        }
    }

    /// `Alt-<c>` for a plugin; `false` if none binds it.
    pub fn plugin_key(&mut self, c: char) -> bool {
        let chat = self.open_chat_ref();
        match self.plugins.key(c, chat) {
            Some(fx) => {
                self.apply_plugin_effects(fx);
                true
            }
            None => false,
        }
    }

    pub fn plugin_event(&mut self, plugin: &str, ev: plugins::Event) {
        let fx = self.plugins.deliver(plugin, ev);
        self.apply_plugin_effects(fx);
    }

    pub fn run_plugin_timers(&mut self) {
        if self
            .plugins
            .next_timer()
            .is_some_and(|t| t <= std::time::Instant::now())
        {
            let fx = self.plugins.run_timers(std::time::Instant::now());
            self.apply_plugin_effects(fx);
        }
    }

    /// Carry out what plugins asked for (already checked against their
    /// permissions; this checks it against the screen).
    pub fn apply_plugin_effects(&mut self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Status(s) => self.status = s,
                Effect::Notify { title, body } => {
                    let n = notice::Notice::new(&title, &body, notice::Category::Other);
                    notice::show(self.prefs.notify, &n, None);
                }
                Effect::OpenView { view, interactive } => match self.modal.as_mut() {
                    Some(Modal::PluginView(v)) if v.view.plugin == view.plugin && v.view.id == view.id => {
                        v.view = view;
                    }
                    // A new view only when the user asked for it, and never
                    // over another overlay (other than this plugin's view).
                    Some(Modal::PluginView(v)) if interactive && v.view.plugin == view.plugin => {
                        self.modal = Some(Modal::PluginView(PluginView { view, scroll: 0 }));
                    }
                    None if interactive => {
                        self.modal = Some(Modal::PluginView(PluginView { view, scroll: 0 }));
                    }
                    _ => {}
                },
                Effect::CloseView { plugin, id } => {
                    if let Some(Modal::PluginView(v)) = &self.modal
                        && v.view.plugin == plugin
                        && id.as_ref().is_none_or(|id| *id == v.view.id)
                    {
                        self.modal = None;
                    }
                }
                Effect::Form {
                    plugin,
                    id,
                    title,
                    fields,
                } => {
                    if fields.is_empty() {
                        continue;
                    }
                    self.modal = Some(Modal::Form(modal::Form {
                        title: format!(" {title} "),
                        kind: modal::FormKind::Plugin { plugin, form: id },
                        fields: fields
                            .into_iter()
                            .map(|f| modal::Field {
                                label: Cow::Owned(f.label),
                                value: f.value.into(),
                                hint: Cow::Owned(f.hint),
                            })
                            .collect(),
                        sel: 0,
                        error: None,
                    }));
                }
                Effect::InsertText(text) => {
                    if self.open.is_none() {
                        self.status = "open a chat first".into();
                        continue;
                    }
                    self.input.insert_str(&text);
                    self.focus = Focus::Input;
                    self.sel = None;
                }
                Effect::SendMessage { plugin, chat, text } => {
                    let known = self.open.as_deref() == Some(chat.as_str())
                        || self.chats.iter().any(|c| c.mid == chat && !c.invited);
                    if !known {
                        self.status =
                            format!("plugin {plugin} tried to send to a chat that is not in your list");
                        continue;
                    }
                    let out = OutgoingMessage::text(&chat, &text);
                    self.send_outgoing(chat, out, text);
                }
            }
        }
    }

    // -- the manager ----------------------------------------------------------------

    /// A plugin's page (Settings › Plugins when it is not installed any more).
    pub fn open_plugin_page(&mut self, plugin: String) {
        if self.plugins.get(&plugin).is_none() {
            return self.open_settings_at(settings::Tab::Plugins);
        }
        self.modal = Some(Modal::Plugins(Manager { plugin, sel: 0 }));
    }

    fn plugin_rows(&self, id: &str) -> Vec<Row> {
        let Some(e) = self.plugins.get(id) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut status = row("Status", e.state.label(), RowAct::None);
        status.style = if e.state.is_problem() {
            RowStyle::Problem
        } else {
            RowStyle::Normal
        };
        rows.push(status);
        if let Some(why) = e.state.detail() {
            for line in wrap(why, 72) {
                rows.push(note(line, RowStyle::Problem));
            }
        }
        match e.update() {
            Some(Update::Approval {
                manifest,
                new_permissions,
                ..
            }) => {
                rows.push(note(
                    format!(
                        "Version {} is on disk, waiting for your approval",
                        manifest.version
                    ),
                    RowStyle::Problem,
                ));
                for p in new_permissions {
                    rows.push(note(
                        format!("  new: it asks to {}", p.describe()),
                        RowStyle::Problem,
                    ));
                }
                rows.push(row("Load the new version…", "↵", RowAct::ApproveUpdate));
            }
            Some(Update::Broken(why)) => {
                for line in wrap(why, 72) {
                    rows.push(note(line, RowStyle::Problem));
                }
                rows.push(note("The running version stays.", RowStyle::Problem));
            }
            None => {}
        }
        let can_enable = e.manifest.is_some()
            && !matches!(
                e.state,
                State::Invalid(_) | State::Incompatible(_) | State::Missing
            );
        if can_enable {
            let on = matches!(
                e.state,
                State::Active | State::Loading | State::Failed(_) | State::Paused
            );
            rows.push(row("Enabled", if on { "On" } else { "Off" }, RowAct::Toggle));
        }
        if matches!(e.state, State::Active | State::Failed(_)) {
            rows.push(row("Restart (reload it from disk)", "↵", RowAct::Restart));
        }
        if can_enable {
            let auto = if self.plugins.autoload(id) {
                "Automatically"
            } else {
                "Ask first"
            };
            rows.push(row("Load changed versions", auto, RowAct::AutoLoad));
        }
        if let Some(m) = &e.manifest {
            if !m.description.is_empty() {
                rows.push(note("", RowStyle::Note));
                for line in wrap(&m.description, 72) {
                    rows.push(note(line, RowStyle::Note));
                }
            }
            rows.push(note("", RowStyle::Note));
            rows.push(note("It may", RowStyle::Heading));
            if m.permissions.is_empty() {
                rows.push(note("  only show its own view and status lines", RowStyle::Note));
            }
            for perm in &m.permissions {
                rows.push(note(format!("  • {}", perm.describe()), RowStyle::Note));
            }
            if !m.commands.is_empty() || !m.keys.is_empty() {
                rows.push(note("It adds", RowStyle::Heading));
                for c in &m.commands {
                    let usage = if c.usage.is_empty() {
                        format!("/{}", c.name)
                    } else {
                        c.usage.clone()
                    };
                    rows.push(note(format!("  {usage}  {}", c.help), RowStyle::Note));
                }
                for k in &m.keys {
                    rows.push(note(format!("  {}  {}", k.key, k.help), RowStyle::Note));
                }
            }
            if !m.settings.is_empty() {
                rows.push(note("Settings", RowStyle::Heading));
                let config = self.plugins.config(id);
                for s in &m.settings {
                    rows.push(row(
                        format!("  {}", s.label),
                        setting_value(s, config.get(&s.key)),
                        RowAct::Setting(s.key.clone()),
                    ));
                }
                rows.push(row("  Reset to defaults", "↵", RowAct::ResetSettings));
            }
            rows.push(note("", RowStyle::Note));
            let meta = [
                ("Version", m.version.clone()),
                ("Author", m.author.clone()),
                ("Plugin API", m.api_version.clone()),
            ];
            for (k, v) in meta.into_iter().filter(|(_, v)| !v.is_empty()) {
                rows.push(row(k, v, RowAct::None));
            }
        }
        rows.push(row("Folder", e.dir.display().to_string(), RowAct::None));
        match &e.source {
            Some(src) => {
                rows.push(row("Source", src.describe(), RowAct::None));
                let when = chrono::DateTime::from_timestamp_millis(src.installed)
                    .map(|d| d.with_timezone(&chrono::Local).format(" · %Y-%m-%d").to_string())
                    .unwrap_or_default();
                rows.push(row(
                    "Version",
                    format!("{}{when}", src.version_label()),
                    RowAct::None,
                ));
                rows.push(row("Updates", src.track.describe(), RowAct::Versions));
                if let Some(a) = self.plugin_available.get(id) {
                    rows.push(note(
                        format!("  {a} is available (Updates › choose it)"),
                        RowStyle::Note,
                    ));
                }
                let busy = self.plugin_job.clone().unwrap_or_else(|| "↵".into());
                rows.push(row("Check for a new version", busy, RowAct::Update));
                rows.push(row("Uninstall (delete its files)…", "↵", RowAct::Uninstall));
            }
            None if e.state != State::Missing => {
                rows.push(row("Source", "copied in by hand", RowAct::None));
            }
            None => {}
        }
        let stored = self.plugins.data(id).len();
        if stored > 0 {
            rows.push(row(
                format!("Delete its stored data ({stored} items)…"),
                "↵",
                RowAct::ClearData,
            ));
        }
        if e.state == State::Missing || e.manifest.is_none() || e.state == State::Disabled {
            rows.push(row(
                "Forget it (delete its settings and data)…",
                "↵",
                RowAct::Forget,
            ));
        }
        rows
    }

    /// The position of the first row of a plugin's page whose label has `label`.
    #[cfg(test)]
    pub fn plugin_row_for_test(&self, id: &str, label: &str) -> Option<usize> {
        self.plugin_rows(id).iter().position(|r| r.label.contains(label))
    }

    pub fn plugins_key(&mut self, mut m: Manager, k: KeyEvent) {
        let rows = self.plugin_rows(&m.plugin);
        let selectable: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].act != RowAct::None).collect();
        let mut sel = m.sel();
        match keymap::resolve(keymap::Ctx::Plugins, &k) {
            Some(keymap::Cmd::Back) => return self.open_settings_on_plugin(&m.plugin),
            Some(keymap::Cmd::Up) => {
                let pos = selectable.iter().position(|&i| i == sel).unwrap_or(0);
                sel = selectable.get(pos.saturating_sub(1)).copied().unwrap_or(0);
            }
            Some(keymap::Cmd::Down) => {
                sel = match selectable.iter().position(|&i| i == sel) {
                    Some(p) => selectable.get(p + 1).copied().unwrap_or(sel),
                    None => selectable.first().copied().unwrap_or(0),
                };
            }
            Some(keymap::Cmd::Home) => sel = selectable.first().copied().unwrap_or(0),
            Some(keymap::Cmd::End) => sel = selectable.last().copied().unwrap_or(0),
            Some(keymap::Cmd::Confirm) => {
                if rows.get(sel).is_none_or(|r| r.act == RowAct::None) {
                    sel = selectable.first().copied().unwrap_or(0);
                }
                let act = rows.get(sel).map_or(RowAct::None, |r| r.act.clone());
                m.select(sel);
                let id = m.plugin.clone();
                self.modal = Some(Modal::Plugins(m));
                return self.manager_act(id, act);
            }
            _ => {}
        }
        m.select(sel);
        self.modal = Some(Modal::Plugins(m));
    }

    fn manager_act(&mut self, id: String, act: RowAct) {
        match act {
            RowAct::None => {}
            RowAct::Toggle => {
                let on = self.plugins.get(&id).is_some_and(|e| {
                    matches!(
                        e.state,
                        State::Active | State::Loading | State::Failed(_) | State::Paused
                    )
                });
                if on {
                    let fx = self.plugins.disable(&id);
                    self.apply_plugin_effects(fx);
                    self.status = format!("plugin {} is off", self.plugin_name(&id));
                    self.open_plugin_page(id);
                } else {
                    self.ask_enable_plugin(&id);
                }
            }
            RowAct::Restart => {
                let (fx, job) = self.plugins.restart(&id);
                self.apply_plugin_effects(fx);
                match job {
                    Some(job) => {
                        self.spawn_plugin_load(job);
                        self.status = format!("restarting {}…", self.plugin_name(&id));
                    }
                    None => {
                        let state = self.plugins.get(&id).map_or("gone", |e| e.state.label());
                        self.status = format!("{} not restarted: {state}", self.plugin_name(&id));
                    }
                }
                self.open_plugin_page(id);
            }
            RowAct::ApproveUpdate => self.ask_approve_update(&id),
            RowAct::AutoLoad if self.plugins.autoload(&id) => self.set_plugin_autoload(&id, false),
            RowAct::AutoLoad => self.confirm(
                format!(
                    "Load new versions of {} without asking? Whenever its module changes on disk (a rebuild, \
                     an update), the new one runs right away, as long as it asks for no new permissions. \
                     Meant for plugins you build yourself or fully trust.",
                    self.plugin_name(&id)
                ),
                Action::AutoloadPlugin(id),
            ),
            RowAct::Update => self.update_plugin(id, None),
            RowAct::Versions => self.choose_plugin_version(id),
            RowAct::Uninstall => self.confirm(
                format!(
                    "Uninstall {}? Its files are deleted; its settings and stored data are kept for a \
                     reinstall (Forget it deletes them).",
                    self.plugin_name(&id)
                ),
                Action::UninstallPlugin(id),
            ),
            RowAct::Setting(key) => self.edit_plugin_setting(&id, &key),
            RowAct::ResetSettings => match self.plugins.reset_settings(&id) {
                Ok(fx) => {
                    self.apply_plugin_effects(fx);
                    self.status = "settings reset to their defaults".into();
                }
                Err(e) => self.status = format!("could not reset: {e}"),
            },
            RowAct::ClearData => self.confirm(
                format!(
                    "Delete everything {} has stored? This cannot be undone.",
                    self.plugin_name(&id)
                ),
                Action::ClearPluginData(id),
            ),
            RowAct::Forget => self.confirm(
                format!(
                    "Delete {}'s settings, approval and stored data? The plugin's files stay where they are.",
                    self.plugin_name(&id)
                ),
                Action::ForgetPlugin(id),
            ),
        }
    }

    fn plugin_name(&self, id: &str) -> String {
        self.plugins.get(id).map_or(id, |e| e.name()).to_owned()
    }

    /// Enabling runs third-party code: say what it may do and ask.
    fn ask_enable_plugin(&mut self, id: &str) {
        let Some(m) = self.plugins.get(id).and_then(|e| e.manifest.as_ref()) else {
            return;
        };
        let may = if m.permissions.is_empty() {
            "show its own view and status lines".to_owned()
        } else {
            m.permissions
                .iter()
                .map(|p| p.describe())
                .collect::<Vec<_>>()
                .join("; ")
        };
        let author = if m.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", m.author)
        };
        self.confirm(
            format!(
                "Enable {} {}{author}? It will be able to: {may}. Only enable plugins you trust.",
                m.name, m.version
            ),
            Action::EnablePlugin(id.to_owned()),
        );
    }

    pub fn enable_plugin(&mut self, id: &str) {
        match self.plugins.enable(id) {
            Ok(job) => {
                self.spawn_plugin_load(job);
                self.status = format!("starting {}…", self.plugin_name(id));
            }
            Err(e) => self.status = format!("cannot enable {}: {e}", self.plugin_name(id)),
        }
        self.open_plugin_page(id.to_owned());
    }

    pub fn clear_plugin_data(&mut self, id: &str) {
        let (fx, job) = self.plugins.clear_data(id);
        self.apply_plugin_effects(fx);
        if let Some(job) = job {
            self.spawn_plugin_load(job);
        }
        self.status = format!("deleted {}'s data", self.plugin_name(id));
        self.open_plugin_page(id.to_owned());
    }

    pub fn forget_plugin(&mut self, id: &str) {
        let name = self.plugin_name(id);
        let fx = self.plugins.forget(id);
        self.apply_plugin_effects(fx);
        self.status = format!("forgot {name}");
        // Its page if it is still installed, else the plugin list.
        self.open_plugin_page(id.to_owned());
    }

    fn edit_plugin_setting(&mut self, id: &str, key: &str) {
        let Some(spec) = self
            .plugins
            .get(id)
            .and_then(|e| e.manifest.as_ref())
            .and_then(|m| m.settings.iter().find(|s| s.key == key))
            .cloned()
        else {
            return;
        };
        let current = self
            .plugins
            .config(id)
            .get(key)
            .cloned()
            .unwrap_or(spec.default.clone());
        let next = match spec.kind {
            SettingType::Bool => Some(serde_json::Value::Bool(current.as_bool() != Some(true))),
            SettingType::Choice => {
                let at = spec
                    .choices
                    .iter()
                    .position(|c| current.as_str() == Some(c))
                    .unwrap_or(0);
                Some(serde_json::Value::from(
                    spec.choices[(at + 1) % spec.choices.len()].clone(),
                ))
            }
            SettingType::Int | SettingType::Text => None,
        };
        if let Some(v) = next {
            match self.plugins.set_setting(id, key, v) {
                Ok(fx) => self.apply_plugin_effects(fx),
                Err(e) => self.status = format!("{}: {e}", spec.label),
            }
            return;
        }
        let hint = match (spec.kind, spec.min, spec.max) {
            (SettingType::Int, Some(lo), Some(hi)) => format!("a number from {lo} to {hi}"),
            (SettingType::Int, ..) => "a whole number".to_owned(),
            _ => spec.help.clone(),
        };
        let value = match &current {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        self.modal = Some(Modal::Form(modal::Form {
            title: format!(" {} › {} ", self.plugin_name(id), spec.label),
            kind: modal::FormKind::PluginSetting {
                plugin: id.to_owned(),
                key: key.to_owned(),
            },
            fields: vec![modal::Field {
                label: Cow::Owned(spec.label.clone()),
                value: value.into(),
                hint: Cow::Owned(hint),
            }],
            sel: 0,
            error: None,
        }));
    }

    pub fn draw_plugins(&mut self, f: &mut Frame, area: Rect, m: &mut Manager, hint: String) {
        let r = modal::popup(area, 84, 24);
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let title = format!(" ⚙ Settings › Plugins › {} ", self.plugin_name(&m.plugin));
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(title, Style::new().bold()))
            .title_bottom(frame_hint(&hint, r));
        let body = block.inner(r);
        f.render_widget(block, r);
        let rows = self.plugin_rows(&m.plugin);
        if rows.get(m.sel()).is_none_or(|r| r.act == RowAct::None)
            && let Some(i) = rows.iter().position(|r| r.act != RowAct::None)
        {
            m.select(i);
        }
        let sel = m.sel();
        let first = modal::scroll_first(sel, body.height as usize);
        let t = theme::t();
        for (n, r) in rows.iter().skip(first).take(body.height as usize).enumerate() {
            let i = first + n;
            let y = body.y + n as u16;
            if r.act != RowAct::None {
                self.hit.modal_rows.push((y, i));
            }
            let selected = i == sel && r.act != RowAct::None;
            let vw = r.value.width().min(body.width as usize / 2);
            let value = ellipsize(&r.value, vw);
            let label = ellipsize(&r.label, (body.width as usize).saturating_sub(vw + 3));
            let pad = (body.width as usize).saturating_sub(label.width() + value.width() + 2);
            let (ls, vs) = match (selected, r.style) {
                (true, _) => (
                    Style::new().bg(t.accent_soft).fg(t.text),
                    Style::new().bg(t.accent_soft).fg(t.text).bold(),
                ),
                (false, RowStyle::Problem) => (Style::new().fg(t.danger), Style::new().fg(t.danger)),
                (false, RowStyle::Heading) => (Style::new().fg(t.accent).bold(), Style::new()),
                (false, RowStyle::Note) => (theme::muted(), theme::muted()),
                (false, RowStyle::Normal) => (Style::new(), Style::new().fg(t.accent)),
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {label}{}", " ".repeat(pad)), ls),
                    Span::styled(format!("{value} "), vs),
                ])),
                Rect::new(body.x, y, body.width, 1),
            );
        }
    }

    // -- a plugin's view ----------------------------------------------------------------

    pub fn plugin_view_key(&mut self, mut v: PluginView, k: KeyEvent) {
        let page = (self.hit.modal.map_or(10, |r| r.height.saturating_sub(2)) as usize).max(1);
        match keymap::resolve(keymap::Ctx::PluginView, &k) {
            Some(keymap::Cmd::Back) => {
                let (plugin, view) = (v.view.plugin, v.view.id);
                self.plugin_event(&plugin, plugins::Event::ViewClosed { view });
                return;
            }
            Some(keymap::Cmd::PageUp) => v.scroll = v.scroll.saturating_sub(page),
            Some(keymap::Cmd::PageDown) => v.scroll += page,
            _ => {
                let Some(key) = key_name(&k) else {
                    self.modal = Some(Modal::PluginView(v));
                    return;
                };
                let ev = plugins::Event::ViewKey {
                    view: v.view.id.clone(),
                    key,
                    selected: v.view.selected,
                };
                let plugin = v.view.plugin.clone();
                // Back in place first: the reply may replace or close it.
                self.modal = Some(Modal::PluginView(v));
                return self.plugin_event(&plugin, ev);
            }
        }
        self.modal = Some(Modal::PluginView(v));
    }

    pub fn draw_plugin_view(&mut self, f: &mut Frame, area: Rect, v: &mut PluginView, hint: String) {
        let view = &v.view;
        let keys: Vec<String> = view
            .keys
            .iter()
            .map(|k| format!("{} {}", k.key, k.help))
            .collect();
        let hint = if keys.is_empty() {
            hint
        } else {
            format!(" {} · {} ", keys.join(" · "), hint.trim())
        };
        let want_w = view
            .lines
            .iter()
            .map(|l| l.text.width())
            .chain([view.title.width() + 6, hint.width()])
            .max()
            .unwrap_or(30)
            .clamp(30, 100)
            + 4;
        let want_h = (view.lines.len() as u16 + 2).max(5);
        let r = modal::popup(area, want_w as u16, want_h.min(area.height * 4 / 5));
        crate::picture::cover(f, r);
        self.hit.modal = Some(r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::t().accent))
            .title(Span::styled(format!(" {} ", view.title), Style::new().bold()))
            .title_bottom(frame_hint(&hint, r));
        let body = block.inner(r);
        f.render_widget(block, r);
        let h = body.height as usize;
        let max_scroll = view.lines.len().saturating_sub(h);
        v.scroll = match view.selected {
            // Follow the selection.
            Some(s) if s < v.scroll => s,
            Some(s) if s >= v.scroll + h => s + 1 - h,
            _ => v.scroll,
        }
        .min(max_scroll);
        let t = theme::t();
        for (n, line) in v.view.lines.iter().skip(v.scroll).take(h).enumerate() {
            let i = v.scroll + n;
            let style = match line.style {
                LineStyle::Normal => Style::new(),
                LineStyle::Muted => theme::muted(),
                LineStyle::Accent => Style::new().fg(t.accent),
                LineStyle::Danger => Style::new().fg(t.danger),
                LineStyle::Bold => Style::new().bold(),
                LineStyle::Heading => Style::new().fg(t.accent).bold(),
            };
            let style = if v.view.selected == Some(i) {
                style.bg(t.accent_soft).fg(t.text)
            } else {
                style
            };
            let w = body.width as usize;
            let text = ellipsize(&line.text, w.saturating_sub(1));
            let pad = w.saturating_sub(text.width() + 1);
            f.render_widget(
                Paragraph::new(Span::styled(format!(" {text}{}", " ".repeat(pad)), style)),
                Rect::new(body.x, body.y + n as u16, body.width, 1),
            );
        }
    }

    /// The help screen's lines: the context's keys, plus what plugins add
    /// there (or, over a plugin's view, the keys it lists).
    pub fn help_lines(&self, ctx: keymap::Ctx, view: Option<&PluginView>) -> Vec<(String, String)> {
        use keymap::Ctx;
        let mut lines: Vec<(String, String)> = keymap::help_lines(ctx)
            .into_iter()
            .map(|(k, h)| (k, h.to_owned()))
            .collect();
        if ctx == Ctx::PluginView
            && let Some(v) = view
        {
            lines.extend(v.view.keys.iter().map(|k| (k.key.clone(), k.help.clone())));
        }
        if matches!(
            ctx,
            Ctx::ChatList | Ctx::ChatSearch | Ctx::Compose | Ctx::Messages | Ctx::Viewer
        ) {
            lines.extend(self.plugins.keys_help());
        }
        if ctx == Ctx::Compose {
            lines.extend(self.plugins.commands_help());
        }
        lines
    }
}
