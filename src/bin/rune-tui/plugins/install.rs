//! Installing plugins from Git repositories, updating and removing them.
//!
//! Blocking functions (run them off the UI thread) that only touch the
//! file system and the `git` program; the [`Plugins`](super::Plugins) host
//! picks the result up with [`refresh`](super::Plugins::refresh), like any
//! other change in the plugin directory.
//!
//! Installing runs nothing: rune does not build plugins (a build runs the
//! author's build scripts and macros), so a repository must hold the plugin
//! *built* — `plugin.json` next to its `.wasm`. The repository is cloned
//! into a hidden staging directory inside the plugin directory, the plugin
//! folder in it is found and checked exactly as discovery checks it (and the
//! module is compiled, never instantiated), and only `plugin.json`, the
//! module and a record of where they came from ([`SOURCE_FILE`]) are moved
//! into `<plugins dir>/<id>/` with a single rename. An update swaps the
//! directory the same way; a module that changed still needs the user's
//! approval before it runs.
//!
//! Each install follows a [`Track`]: a branch, the newest release in a
//! range, or one pinned tag or commit. `git ls-remote` tells where it points
//! now (no download when nothing changed), and exactly that commit is
//! fetched (`git fetch --depth 1`).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use rune_plugin::Manifest;
use rune_plugin::manifest::MANIFEST_FILE;
use serde::{Deserialize, Serialize};

use super::runtime::{Limits, Sandbox};
use super::versions::{Remote, Target, Track, Version, VersionReq};
use super::{MANIFEST_MAX, State, clean, inspect};

/// Where an installed plugin came from, kept next to its manifest.
pub const SOURCE_FILE: &str = ".rune-source.json";
/// Clones and half-finished installs, inside the plugin directory (so the
/// final rename stays on one file system); hidden, so discovery skips it.
const STAGING: &str = ".staging";
/// A git command taking longer than this is stopped.
const GIT_TIMEOUT: Duration = Duration::from_secs(300);
/// How deep in a repository `plugin.json` is looked for.
const SEARCH_DEPTH: usize = 3;
const SOURCE_MAX: u64 = 16 << 10;

/// The repository a plugin was installed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub url: String,
    /// What updates follow.
    #[serde(default)]
    pub track: Track,
    /// The plugin's folder in the repository; empty: found by itself.
    #[serde(default)]
    pub subdir: String,
    /// The commit installed.
    pub commit: String,
    /// Its version tag, if it has one.
    #[serde(default)]
    pub tag: String,
    /// When (ms since the epoch).
    #[serde(default)]
    pub installed: i64,
    /// Written by the first version of this feature: a branch or tag.
    #[serde(default, rename = "git_ref", skip_serializing)]
    legacy_ref: String,
}

impl Source {
    /// The record in an installed plugin's directory, if it has a usable one.
    pub fn read(dir: &Path) -> Option<Source> {
        let path = dir.join(SOURCE_FILE);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > SOURCE_MAX {
            return None;
        }
        let mut s: Source = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        if !s.legacy_ref.is_empty() && s.track == Track::default() {
            s.track = Track::Branch(std::mem::take(&mut s.legacy_ref));
        }
        Some(s)
    }

    /// Replace the record, atomically (written next to it, then renamed).
    fn write(&self, dir: &Path) -> Result<(), String> {
        let tmp = dir.join(format!("{SOURCE_FILE}.new"));
        write_file(&tmp, &serde_json::to_vec_pretty(self).expect("sources serialize"))?;
        std::fs::rename(&tmp, dir.join(SOURCE_FILE))
            .map_err(|e| format!("cannot save where it came from: {e}"))
    }

    pub fn short_commit(&self) -> &str {
        self.commit.get(..7).unwrap_or(&self.commit)
    }

    /// The installed version as Git knows it: `v1.2.0 (abc1234)` or `abc1234`.
    pub fn version_label(&self) -> String {
        if self.tag.is_empty() {
            self.short_commit().to_owned()
        } else {
            format!("{} ({})", self.tag, self.short_commit())
        }
    }

    /// `url`, with the folder when there is one.
    pub fn describe(&self) -> String {
        if self.subdir.is_empty() {
            self.url.clone()
        } else {
            format!("{} › {}", self.url, self.subdir)
        }
    }
}

/// What the user asked to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    /// Empty (default branch), `latest`, a range, a tag, a version, a branch
    /// or a commit: see [`Track::from_spec`].
    pub version: String,
    pub subdir: String,
}

impl Request {
    /// Checked and normalised user input.
    pub fn new(url: &str, version: &str, subdir: &str) -> Result<Request, String> {
        let url = normalize_url(url)?;
        let version = version.trim();
        check_version(version)?;
        let subdir = subdir.trim().trim_matches('/');
        check_subdir(subdir)?;
        Ok(Request {
            url,
            version: version.to_owned(),
            subdir: subdir.to_owned(),
        })
    }
}

/// What an install or update did (or found).
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub name: String,
    /// The plugin's own version (its manifest's).
    pub version: String,
    pub commit: String,
    /// The version tag of `commit`, if any.
    pub tag: String,
    /// What it follows now.
    pub track: Track,
    /// An update: the version and commit it replaced.
    pub previous: Option<(String, String)>,
    /// Nothing new to install (the track may still have changed).
    pub up_to_date: bool,
    /// A newer release than the installed one that its track does not move
    /// to (pinned, or outside its range).
    pub available: Option<String>,
}

/// A repository's releases, for choosing a version.
#[derive(Debug, Clone, PartialEq)]
pub struct Versions {
    /// Tag, version; newest first.
    pub releases: Vec<(String, Version)>,
    pub default_branch: Option<String>,
}

/// A repository address rune is willing to hand to git: `https://`,
/// `ssh://`, `user@host:path`, `file://` or an absolute local path. A bare
/// `host.tld/owner/repo` means `https://`. Anything else — plain `http`,
/// `git://`, `ext::` (which runs commands), options — is refused.
pub fn normalize_url(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("give the repository's address".into());
    }
    let lower = s.to_ascii_lowercase();
    // Local paths may contain spaces; addresses may not.
    let local = s.starts_with('/') || lower.starts_with("file://");
    if s.starts_with('-') || s.chars().any(|c| c.is_control() || (c.is_whitespace() && !local)) {
        return Err("that is not a repository address".into());
    }
    for scheme in ["https://", "ssh://", "file://"] {
        if lower.starts_with(scheme) {
            // The address is saved in the plugin's folder and shown on screen,
            // so it must not carry a password or token (`https://TOKEN@host`).
            let authority = s[scheme.len()..].split(['/', '?', '#']).next().unwrap_or("");
            let secret = match scheme {
                "https://" => authority.contains('@'),
                "ssh://" => authority
                    .split_once('@')
                    .is_some_and(|(user, _)| user.contains(':')),
                _ => false,
            };
            if secret {
                return Err(
                    "the address holds a user name or password: leave it out and let git's credential \
                     helper or your ssh key sign in"
                        .into(),
                );
            }
            return if s.len() > scheme.len() {
                Ok(s.to_owned())
            } else {
                Err("the address has no host or path".into())
            };
        }
    }
    if lower.starts_with("http://") || lower.starts_with("git://") {
        return Err("unencrypted addresses (http://, git://) are refused: use https:// or ssh".into());
    }
    if s.contains("::") || lower.contains("://") {
        return Err("only https://, ssh://, user@host:path, file:// or a local path".into());
    }
    if s.starts_with('/') {
        return Ok(s.to_owned());
    }
    if s.starts_with('~') || s.starts_with('.') {
        return Err("give a local repository's full path (starting with /)".into());
    }
    // scp-like ssh: user@host:path
    if let Some((user_host, path)) = s.split_once(':')
        && let Some((user, host)) = user_host.split_once('@')
        && !user.is_empty()
        && !host.is_empty()
        && !path.is_empty()
        && !host.contains('/')
    {
        return Ok(s.to_owned());
    }
    // host.tld/owner/repo
    if let Some((host, path)) = s.split_once('/')
        && host.contains('.')
        && !path.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Ok(format!("https://{s}"));
    }
    Err("only https://, ssh://, user@host:path, file:// or a local path".into())
}

/// What may be typed as the version to install: a branch, tag, commit or
/// version (as [`check_ref`]), or a range (`^1.2`, `~1.2`, `1.x`, `*`).
pub fn check_version(v: &str) -> Result<(), String> {
    let range = v.starts_with(['^', '~']) || v.ends_with(".x") || v.ends_with(".*") || v == "*";
    if range {
        return VersionReq::parse(v)
            .map(drop)
            .ok_or_else(|| format!("{v:?} is not a version range (like ^1.2, ~1.2 or 1.x)"));
    }
    check_ref(v).map_err(|_| "not a branch, tag, version or commit".into())
}

/// A branch or tag name (empty: the default branch).
pub fn check_ref(r: &str) -> Result<(), String> {
    let ok = r.is_empty()
        || (r.len() <= 200
            && !r.starts_with(['-', '/', '.'])
            && !r.ends_with(['/', '.'])
            && !r.contains("..")
            && !r.ends_with(".lock")
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'+')));
    if ok {
        Ok(())
    } else {
        Err("not a branch or tag name".into())
    }
}

/// A folder inside the repository (empty: look for it).
pub fn check_subdir(d: &str) -> Result<(), String> {
    let ok = d.len() <= 200
        && !d.contains('\\')
        && Path::new(d).components().all(|c| match c {
            Component::Normal(p) => !p.to_string_lossy().starts_with('.'),
            _ => false,
        });
    if ok {
        Ok(())
    } else {
        Err("the folder must be a path inside the repository (no .., no leading /)".into())
    }
}

/// A scratch directory under the staging area, removed when dropped.
struct Staging(PathBuf);

impl Staging {
    fn new(plugins: &Path) -> Result<Staging, String> {
        let root = plugins.join(STAGING);
        create_dir(plugins)?;
        create_dir(&root)?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = root.join(format!("{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(Staging(dir))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn create_dir(d: &Path) -> Result<(), String> {
    match std::fs::create_dir_all(d) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("cannot create {}: {e}", d.display())),
    }
}

/// Remove staging directories a crash left behind (older than an hour, so
/// another running rune's install is not pulled from under it).
pub fn clean_staging(plugins: &Path) {
    let Ok(rd) = std::fs::read_dir(plugins.join(STAGING)) else {
        return;
    };
    let hour = Duration::from_secs(3600);
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > hour);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Install a plugin from a repository into `<plugins>/<id>/`.
pub fn install(
    plugins: &Path,
    req: &Request,
    limits: Limits,
    progress: &dyn Fn(String),
) -> Result<Outcome, String> {
    progress(format!("looking at {}…", req.url));
    let remote = ls_remote(&req.url)?;
    let track = Track::from_spec(&req.version, &remote)?;
    let at = remote.resolve(&track)?;
    let staging = Staging::new(plugins)?;
    let repo = staging.0.join("repo");
    progress(format!("fetching {}…", label(&at)));
    let commit = fetch(&req.url, &at, &repo)?;
    progress("checking the plugin…".into());
    let (folder, subdir) = locate(&repo, &req.subdir)?;
    let source = Source {
        url: req.url.clone(),
        track: track.clone(),
        subdir,
        tag: remote.tag_at(&commit).unwrap_or_default(),
        commit: commit.clone(),
        installed: now_ms(),
        legacy_ref: String::new(),
    };
    let (out, m) = prepare(&folder, &staging.0, &source, limits)?;
    let target = plugins.join(&m.id);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(match Source::read(&target) {
            Some(s) if s.url == req.url => format!(
                "{} is already installed from there: use Check for a new version or Updates on its page",
                m.name
            ),
            Some(s) => format!(
                "a plugin with the id {} is already installed (from {}): uninstall it first",
                m.id, s.url
            ),
            None => format!(
                "a plugin with the id {} is already installed (copied into {} by hand): remove that folder first",
                m.id,
                target.display()
            ),
        });
    }
    std::fs::rename(&out, &target).map_err(|e| format!("cannot move it into {}: {e}", target.display()))?;
    Ok(Outcome {
        available: newer_release(&remote, &source.tag, &m.version, &track),
        id: m.id,
        name: m.name,
        version: m.version,
        commit,
        tag: source.tag,
        track,
        previous: None,
        up_to_date: false,
    })
}

fn label(at: &Target) -> String {
    if at.tag.is_empty() {
        super::versions::short(&at.commit).to_owned()
    } else {
        at.tag.clone()
    }
}

/// The newest release that is newer than the installed version but not
/// where `track` goes (so the user hears of it).
fn newer_release(remote: &Remote, tag: &str, manifest_version: &str, track: &Track) -> Option<String> {
    let installed = Version::parse(tag).or_else(|| Version::parse(manifest_version))?;
    let (newest, v) = remote.newest(VersionReq::Any)?;
    let followed = match track {
        Track::Releases(r) => VersionReq::parse(r).is_some_and(|r| r.matches(&v)),
        _ => false,
    };
    (v > installed && !followed).then_some(newest)
}

/// Bring an installed plugin up to date with what it follows (`to: None`,
/// *Check for a new version*), or make it follow something else and install
/// where that points (`to: Some`: choosing a version, a range or a branch;
/// older versions too). A pinned plugin is never moved by a check; newer
/// releases are reported in [`Outcome::available`].
pub fn update(
    plugins: &Path,
    id: &str,
    to: Option<Track>,
    limits: Limits,
    progress: &dyn Fn(String),
) -> Result<Outcome, String> {
    let target = plugins.join(id);
    let old = Source::read(&target).ok_or("it was not installed from a Git repository")?;
    let old_manifest = std::fs::read_to_string(target.join(MANIFEST_FILE))
        .ok()
        .and_then(|t| Manifest::parse(&t).ok());
    let (old_name, old_version) =
        old_manifest.map_or((id.to_owned(), String::new()), |m| (m.name, m.version));
    let track = to.clone().unwrap_or_else(|| old.track.clone());
    progress(format!("looking at {}…", old.url));
    let remote = ls_remote(&old.url)?;
    let stay = |track: Track, tag: String| Outcome {
        id: id.to_owned(),
        name: old_name.clone(),
        version: old_version.clone(),
        commit: old.commit.clone(),
        available: newer_release(&remote, &tag, &old_version, &track),
        tag,
        track,
        previous: None,
        up_to_date: true,
    };
    if to.is_none() && track.is_pinned() {
        return Ok(stay(track, old.tag.clone()));
    }
    let at = remote.resolve(&track)?;
    // A range only moves forward (a newer release may have been pinned before).
    let older = match (&track, Version::parse(&old.tag), Version::parse(&at.tag)) {
        (Track::Releases(_), Some(have), Some(next)) => to.is_none() && next < have,
        _ => false,
    };
    if at.commit == old.commit || older {
        let tag = if at.commit == old.commit && !at.tag.is_empty() {
            at.tag.clone()
        } else {
            old.tag.clone()
        };
        if track != old.track || tag != old.tag {
            Source {
                track: track.clone(),
                tag: tag.clone(),
                ..old.clone()
            }
            .write(&target)?;
        }
        return Ok(stay(track, tag));
    }
    let staging = Staging::new(plugins)?;
    let repo = staging.0.join("repo");
    progress(format!("fetching {}…", label(&at)));
    let commit = fetch(&old.url, &at, &repo)?;
    progress("checking the new version…".into());
    let (folder, subdir) = locate(&repo, &old.subdir)?;
    let source = Source {
        track: track.clone(),
        tag: remote.tag_at(&commit).unwrap_or_default(),
        commit: commit.clone(),
        installed: now_ms(),
        subdir,
        ..old.clone()
    };
    let (out, m) = prepare(&folder, &staging.0, &source, limits)?;
    if m.id != id {
        return Err(format!(
            "that version holds the plugin {:?}, not {id:?}; not installed",
            m.id
        ));
    }
    // Swap: the old directory goes into staging (removed with it), the new
    // one takes its place; put the old one back if that fails.
    let aside = staging.0.join("old");
    std::fs::rename(&target, &aside).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
    if let Err(e) = std::fs::rename(&out, &target) {
        let _ = std::fs::rename(&aside, &target);
        return Err(format!("cannot replace {}: {e}", target.display()));
    }
    Ok(Outcome {
        available: newer_release(&remote, &source.tag, &m.version, &track),
        id: m.id,
        name: m.name,
        version: m.version,
        commit,
        tag: source.tag,
        track,
        previous: Some((old_version, old.commit)),
        up_to_date: false,
    })
}

/// Pin a plugin to the version it has (no network): checks then leave it
/// where it is.
pub fn pin(plugins: &Path, id: &str) -> Result<Track, String> {
    let dir = plugins.join(id);
    let old = Source::read(&dir).ok_or("it was not installed from a Git repository")?;
    let at = if old.tag.is_empty() {
        old.commit.clone()
    } else {
        old.tag.clone()
    };
    let track = Track::Pinned(at);
    Source {
        track: track.clone(),
        ..old
    }
    .write(&dir)?;
    Ok(track)
}

/// The releases of the repository a plugin came from.
pub fn versions(plugins: &Path, id: &str) -> Result<Versions, String> {
    let old = Source::read(&plugins.join(id)).ok_or("it was not installed from a Git repository")?;
    let remote = ls_remote(&old.url)?;
    Ok(Versions {
        releases: remote.releases(),
        default_branch: remote.head,
    })
}

/// Delete a plugin rune installed (one with a [`SOURCE_FILE`]). Plugins
/// copied in by hand are left alone: their folder may be someone's work.
pub fn uninstall(plugins: &Path, id: &str) -> Result<(), String> {
    let target = plugins.join(id);
    let meta = std::fs::symlink_metadata(&target).map_err(|_| "it is not installed".to_owned())?;
    if !meta.is_dir() || Source::read(&target).is_none() {
        return Err(format!(
            "it was not installed from a repository; delete {} yourself",
            target.display()
        ));
    }
    let staging = Staging::new(plugins)?;
    // One rename takes it out of the plugin directory; the rest goes with
    // the staging directory.
    std::fs::rename(&target, staging.0.join(id))
        .map_err(|e| format!("cannot remove {}: {e}", target.display()))
}

/// The plugin folder in a clone, and its path in the repository.
fn locate(repo: &Path, subdir: &str) -> Result<(PathBuf, String), String> {
    if !subdir.is_empty() {
        let dir = repo.join(subdir);
        // core.symlinks=false checks symlinks out as plain files, so a
        // directory here is a real one inside the clone.
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(format!("the repository has no folder {subdir}"));
        }
        if !is_file(&dir.join(MANIFEST_FILE)) {
            return Err(format!("{subdir} has no plugin.json"));
        }
        return Ok((dir, subdir.to_owned()));
    }
    let mut found = Vec::new();
    find_manifests(repo, repo, 0, &mut found);
    found.sort();
    match found.as_slice() {
        [] => Err(
            "the repository has no plugin.json (a plugin is a plugin.json next to its built .wasm)".into(),
        ),
        [one] => Ok((repo.join(one), one.clone())),
        many => {
            let list: Vec<&str> = many.iter().map(|s| if s.is_empty() { "." } else { s }).collect();
            Err(format!(
                "the repository holds several plugins; give the folder: {}",
                list.join(", ")
            ))
        }
    }
}

fn find_manifests(repo: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if is_file(&dir.join(MANIFEST_FILE)) {
        let rel = dir.strip_prefix(repo).unwrap_or(dir);
        out.push(rel.to_string_lossy().into_owned());
        return;
    }
    if depth >= SEARCH_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            find_manifests(repo, &e.path(), depth + 1, out);
        }
    }
}

fn is_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file())
}

/// Copy the plugin out of the clone into `<staging>/out/<id>/` (manifest,
/// module, source record), check it as discovery would, and compile the
/// module without running it.
fn prepare(
    folder: &Path,
    staging: &Path,
    source: &Source,
    limits: Limits,
) -> Result<(PathBuf, Manifest), String> {
    let text = read_regular(&folder.join(MANIFEST_FILE), MANIFEST_MAX)
        .map_err(|e| format!("plugin.json: {e}"))
        .and_then(|b| String::from_utf8(b).map_err(|_| "plugin.json is not UTF-8 text".to_owned()))?;
    let m = Manifest::parse(&text).map_err(|e| format!("its plugin.json is invalid: {e}"))?;
    let module = read_regular(&folder.join(&m.entry), limits.module as u64).map_err(|e| {
        if e == "missing" {
            format!(
                "the repository has no {} next to plugin.json: plugins are installed built (rune does not \
                 build them, building runs code), so its author has to publish the .wasm",
                m.entry
            )
        } else {
            format!("{}: {e}", m.entry)
        }
    })?;
    if module.starts_with(b"version https://git-lfs") {
        return Err(format!(
            "{} is kept in Git LFS, which rune does not download; ask the author to commit it directly",
            m.entry
        ));
    }
    let out = staging.join("out").join(&m.id);
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755));
    }
    write_file(&out.join(MANIFEST_FILE), text.as_bytes())?;
    write_file(&out.join(&m.entry), &module)?;
    let record = serde_json::to_vec_pretty(source).expect("sources serialize");
    write_file(&out.join(SOURCE_FILE), &record)?;
    let found = inspect(&out, limits);
    if let Err(state) = found.manifest {
        return Err(match state {
            State::Incompatible(why) => format!("it is incompatible: {why}"),
            s => format!("it is not a usable plugin: {}", s.detail().unwrap_or(s.label())),
        });
    }
    Sandbox::check(&module, limits).map_err(|e| format!("its module cannot run in rune: {e}"))?;
    Ok((out, m))
}

/// A regular file (not a link) of at most `max` bytes.
fn read_regular(p: &Path, max: u64) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(p).map_err(|_| "missing".to_owned())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > max {
        return Err(format!("larger than {} KB", max >> 10));
    }
    let mut buf = Vec::new();
    std::fs::File::open(p)
        .and_then(|f| f.take(max + 1).read_to_end(&mut buf))
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > max {
        return Err(format!("larger than {} KB", max >> 10));
    }
    Ok(buf)
}

/// Written readable by all, writable only by the owner (discovery refuses
/// files others can write).
fn write_file(p: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(p, bytes).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("cannot set permissions of {}: {e}", p.display()))?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

// -- git -----------------------------------------------------------------------------

/// The repository's branches and tags, without downloading anything.
fn ls_remote(url: &str) -> Result<Remote, String> {
    let args: Vec<OsString> = [
        "ls-remote",
        "--symref",
        "--",
        url,
        "HEAD",
        "refs/heads/*",
        "refs/tags/*",
    ]
    .map(OsString::from)
    .into();
    let out = git(&args, None).map_err(|e| format!("cannot reach {url}: {e}"))?;
    Ok(Remote::parse(&out))
}

/// Fetch exactly `at` (one commit, no history) into a new repository at
/// `into` and check it out; the commit checked out.
fn fetch(url: &str, at: &Target, into: &Path) -> Result<String, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {e}", into.display()))?;
    let run = |args: &[&str]| {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        git(&args, Some(into))
    };
    run(&["init", "--quiet"])?;
    run(&[
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "--no-tags",
        "--no-recurse-submodules",
        "--",
        url,
        &at.fetch,
    ])
    .map_err(|e| format!("could not fetch {}: {e}", label(at)))?;
    run(&["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
    let commit = head(into)?;
    // A ref may have moved since it was listed; a commit may not.
    if at.fetch == at.commit && commit != at.commit {
        return Err(format!("fetched {commit}, not the commit asked for"));
    }
    Ok(commit)
}

fn head(repo: &Path) -> Result<String, String> {
    let out = git(&["rev-parse".into(), "HEAD".into()], Some(repo))?;
    let commit = out.trim().to_owned();
    if commit.len() >= 7 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(commit)
    } else {
        Err("git did not report the commit it fetched".into())
    }
}

/// Run git without a terminal: it must never prompt (rune owns the screen),
/// may only use encrypted transports or local files, checks symlinks out as
/// plain files and is stopped after [`GIT_TIMEOUT`]. Returns its output.
fn git(args: &[OsString], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    for c in [
        "protocol.allow=never",
        "protocol.https.allow=always",
        "protocol.ssh.allow=always",
        "protocol.file.allow=always",
        "core.symlinks=false",
        "core.fsmonitor=false",
        "submodule.recurse=false",
    ] {
        cmd.arg("-c").arg(c);
    }
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let ssh = ssh_command(std::env::var("GIT_SSH_COMMAND").ok(), configured_ssh_command());
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_SSH_COMMAND", ssh)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            "the git program is not installed (it is needed to install plugins from a repository)".to_owned()
        }
        _ => format!("cannot run git: {e}"),
    })?;
    // Read both pipes on threads, so a chatty git never blocks on a full pipe.
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.by_ref().take(1 << 20).read_to_end(&mut buf);
                let _ = std::io::copy(&mut r, &mut std::io::sink());
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() > GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "git took longer than {} minutes; stopped",
                    GIT_TIMEOUT.as_secs() / 60
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("git: {e}")),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if status.success() {
        return Ok(String::from_utf8_lossy(&out).into_owned());
    }
    Err(git_error(&String::from_utf8_lossy(&err)))
}

/// The ssh command git is to use, never prompting (`BatchMode`). Setting
/// `GIT_SSH_COMMAND` overrides `core.sshCommand`, so the user's own choice
/// (environment first, then configuration, as git itself ranks them) is
/// kept and only made non-interactive.
fn ssh_command(env: Option<String>, configured: Option<String>) -> String {
    let base = [env, configured]
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_owned())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "ssh".into());
    format!("{base} -o BatchMode=yes")
}

/// The user's `core.sshCommand` (global or system configuration), if set.
fn configured_ssh_command() -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", "core.sshCommand"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The gist of git's complaint: its last `fatal:`/`error:` line.
fn git_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or(lines.last())
        .copied()
        .unwrap_or("git failed");
    let line = line
        .trim_start_matches("fatal:")
        .trim_start_matches("error:")
        .trim();
    clean(line, 200, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_checked_and_normalised() {
        for ok in [
            "https://github.com/o/r",
            "ssh://git@host/o/r.git",
            "git@github.com:o/r.git",
            "file:///srv/repo",
            "/home/me/repo",
        ] {
            assert_eq!(normalize_url(ok).as_deref(), Ok(ok), "{ok}");
        }
        assert_eq!(
            normalize_url(" github.com/o/r ").as_deref(),
            Ok("https://github.com/o/r")
        );
        for bad in [
            "",
            "http://github.com/o/r",
            "git://host/r",
            "ext::sh -c touch% /tmp/x",
            "-uhttps://x",
            "--upload-pack=x",
            "fd::3",
            "ftp://host/r",
            "https://",
            "~/repo",
            "./repo",
            "repo",
            "a b",
            // Credentials would be saved in .rune-source.json and shown on screen.
            "https://ghp_token@github.com/o/r",
            "https://user:pass@github.com/o/r",
            "ssh://git:pass@host/o/r",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad:?}");
        }
        assert!(
            normalize_url("https://github.com/o/r@v1").is_ok(),
            "an @ in the path is fine"
        );
    }

    #[test]
    fn ssh_never_prompts_and_keeps_the_users_command() {
        let cmd =
            |env: Option<&str>, conf: Option<&str>| ssh_command(env.map(Into::into), conf.map(Into::into));
        assert_eq!(cmd(None, None), "ssh -o BatchMode=yes");
        assert_eq!(
            cmd(None, Some("ssh -i ~/.ssh/work")),
            "ssh -i ~/.ssh/work -o BatchMode=yes",
            "core.sshCommand is kept (GIT_SSH_COMMAND would override it)"
        );
        assert_eq!(
            cmd(Some("ssh -p 2222"), Some("ssh -i k")),
            "ssh -p 2222 -o BatchMode=yes",
            "the environment wins, as in git"
        );
        assert_eq!(cmd(Some("  "), Some("ssh -i k")), "ssh -i k -o BatchMode=yes");
    }

    #[test]
    fn refs_and_folders_are_checked() {
        for ok in ["", "main", "v1.2.0", "release/1.x", "feature_a-b"] {
            assert!(check_ref(ok).is_ok(), "{ok}");
        }
        for ok in [
            "", "latest", "^1.2", "~1", "1.x", "*", "v1.2.0", "abc1234", "main",
        ] {
            assert!(check_version(ok).is_ok(), "{ok}");
        }
        for bad in ["^", "^x", "1.2.3.x", "-x", "a b", ">=1"] {
            assert!(check_version(bad).is_err(), "{bad}");
        }
        for bad in [
            "-x",
            "--upload-pack=x",
            "a..b",
            "a b",
            "/a",
            "a/",
            "x.lock",
            "a~1",
        ] {
            assert!(check_ref(bad).is_err(), "{bad}");
        }
        for ok in ["", "plugins/snippets", "a"] {
            assert!(check_subdir(ok).is_ok(), "{ok}");
        }
        for bad in ["../x", "/abs", "a/../../b", ".git", "a/.hidden", "a\\b"] {
            assert!(check_subdir(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Request::new("github.com/o/r", " ^1.2 ", "/plugins/x/").unwrap(),
            Request {
                url: "https://github.com/o/r".into(),
                version: "^1.2".into(),
                subdir: "plugins/x".into()
            }
        );
    }

    #[test]
    fn git_errors_are_summed_up() {
        assert_eq!(
            git_error("Cloning into 'x'...\nfatal: repository 'https://h/r/' not found\n"),
            "repository 'https://h/r/' not found"
        );
        assert_eq!(git_error("warning: a\nerror: b\nhint: c"), "b");
        assert_eq!(git_error(""), "git failed");
        assert_eq!(git_error("fatal: \x1b[31mred"), "\u{FFFD}[31mred");
    }
}
