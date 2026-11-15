//! Which commit of a repository a Git-installed plugin follows: a branch, the
//! newest release (version tag) within a range, or one pinned tag or commit.
//!
//! Pure functions over what `git ls-remote` lists ([`Remote`]); no I/O here.
//! Releases are tags that read as versions (`v1.2.3`, `1.2`, `v2.0.0-beta.1`);
//! ranges follow Cargo's rules (`^1.2` = 1.2 up to, not including, 2.0;
//! `~1.2` = 1.2.x; `1.x` = `^1`). Pre-releases are only ever installed by
//! pinning them.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A release version read from a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `-beta.1` etc., without the dash.
    pub pre: Option<String>,
}

impl Version {
    /// `v1.2.3`, `1.2`, `V2`, `1.0.0-rc.1+build` (the build part is ignored).
    pub fn parse(tag: &str) -> Option<Version> {
        let s = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
        let s = s.split('+').next()?;
        let (nums, pre) = match s.split_once('-') {
            Some((n, p)) if !p.is_empty() => (n, Some(p.to_owned())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = nums.split('.');
        let mut num = || -> Option<Option<u64>> {
            match parts.next() {
                None => Some(None),
                Some(p) if !p.is_empty() && p.len() <= 18 && p.bytes().all(|b| b.is_ascii_digit()) => {
                    Some(Some(p.parse().ok()?))
                }
                Some(_) => None,
            }
        };
        let major = num()??;
        let minor = num()?.unwrap_or(0);
        let patch = num()?.unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl Ord for Version {
    /// By number; a pre-release comes before its release.
    fn cmp(&self, other: &Self) -> Ordering {
        self.triple()
            .cmp(&other.triple())
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// Which releases an update may move to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionReq {
    /// Any release.
    Any,
    /// `^M[.m[.p]]`: at least that, below the next incompatible version.
    Caret(u64, Option<u64>, Option<u64>),
    /// `~M[.m[.p]]`: at least that, below the next minor (or major) version.
    Tilde(u64, Option<u64>, Option<u64>),
}

impl VersionReq {
    /// `*`, `latest`, `^1.2`, `~1.2.3`, `1.x`, `1.2.x`.
    pub fn parse(s: &str) -> Option<VersionReq> {
        let s = s.trim();
        if matches!(s, "*" | "latest") {
            return Some(VersionReq::Any);
        }
        let parts = |s: &str| -> Option<(u64, Option<u64>, Option<u64>)> {
            let v: Vec<&str> = s.split('.').collect();
            if v.is_empty() || v.len() > 3 {
                return None;
            }
            let n =
                |p: &str| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok())?;
            let opt = |i: usize| -> Option<Option<u64>> {
                match v.get(i) {
                    None => Some(None),
                    Some(p) => n(p).map(Some),
                }
            };
            Some((n(v[0])?, opt(1)?, opt(2)?))
        };
        if let Some(rest) = s.strip_prefix('^') {
            let (a, b, c) = parts(rest.strip_prefix(['v', 'V']).unwrap_or(rest))?;
            return Some(VersionReq::Caret(a, b, c));
        }
        if let Some(rest) = s.strip_prefix('~') {
            let (a, b, c) = parts(rest.strip_prefix(['v', 'V']).unwrap_or(rest))?;
            return Some(VersionReq::Tilde(a, b, c));
        }
        let wild = s.strip_suffix(".x").or_else(|| s.strip_suffix(".*"))?;
        match parts(wild)? {
            (a, None, None) => Some(VersionReq::Caret(a, None, None)),
            (a, Some(b), None) => Some(VersionReq::Tilde(a, Some(b), None)),
            _ => None,
        }
    }

    /// Releases only: a pre-release never matches a range.
    pub fn matches(&self, v: &Version) -> bool {
        if v.pre.is_some() {
            return false;
        }
        let at = v.triple();
        let (lower, upper) = match *self {
            VersionReq::Any => return true,
            VersionReq::Caret(a, b, c) => {
                let upper = match (a, b, c) {
                    (a, _, _) if a > 0 => (a + 1, 0, 0),
                    (_, None, _) => (1, 0, 0),
                    (_, Some(b), _) if b > 0 => (0, b + 1, 0),
                    (_, Some(_), None) => (0, 1, 0),
                    (_, Some(_), Some(c)) => (0, 0, c + 1),
                };
                ((a, b.unwrap_or(0), c.unwrap_or(0)), upper)
            }
            VersionReq::Tilde(a, b, c) => {
                let upper = match b {
                    None => (a + 1, 0, 0),
                    Some(b) => (a, b + 1, 0),
                };
                ((a, b.unwrap_or(0), c.unwrap_or(0)), upper)
            }
        };
        at >= lower && at < upper
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = |a: u64, b: Option<u64>, c: Option<u64>| {
            let mut s = a.to_string();
            for n in [b, c].into_iter().flatten() {
                s.push_str(&format!(".{n}"));
            }
            s
        };
        match *self {
            VersionReq::Any => write!(f, "*"),
            VersionReq::Caret(a, b, c) => write!(f, "^{}", parts(a, b, c)),
            VersionReq::Tilde(a, b, c) => write!(f, "~{}", parts(a, b, c)),
        }
    }
}

/// What a Git-installed plugin follows, and so what *Check for a new
/// version* may move it to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    /// The newest commit of a branch (empty: the default branch).
    Branch(String),
    /// The newest release within a range ([`VersionReq`], as written).
    Releases(String),
    /// One tag or commit: never updated (newer releases are reported).
    Pinned(String),
}

impl Default for Track {
    fn default() -> Self {
        Track::Branch(String::new())
    }
}

impl Track {
    pub fn is_pinned(&self) -> bool {
        matches!(self, Track::Pinned(_))
    }

    /// For the plugin's page: what updates do.
    pub fn describe(&self) -> String {
        match self {
            Track::Branch(b) if b.is_empty() => "newest commit (default branch)".into(),
            Track::Branch(b) => format!("newest commit on {b}"),
            Track::Releases(r) if r == "*" => "newest release".into(),
            Track::Releases(r) => format!("newest {r} release"),
            Track::Pinned(at) => format!("pinned to {}", short(at)),
        }
    }

    /// What the user typed as the version to install: empty (the default
    /// branch), `latest`, a range (`^1.2`, `~1.2`, `1.x`), a tag (pinned), a
    /// version that names a tag (`1.2` for `v1.2.0`, pinned), a branch, or
    /// a commit (pinned).
    pub fn from_spec(spec: &str, remote: &Remote) -> Result<Track, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Ok(Track::default());
        }
        if spec.starts_with(['^', '~'])
            || spec.ends_with(".x")
            || spec.ends_with(".*")
            || spec == "*"
            || spec == "latest"
        {
            let req = VersionReq::parse(spec).ok_or_else(|| format!("{spec:?} is not a version range"))?;
            return Ok(Track::Releases(req.to_string()));
        }
        if remote.tags.contains_key(spec) {
            return Ok(Track::Pinned(spec.to_owned()));
        }
        if remote.branches.contains_key(spec) {
            return Ok(Track::Branch(spec.to_owned()));
        }
        if let Some(v) = Version::parse(spec)
            && let Some((tag, _)) = remote.releases().into_iter().find(|(_, rv)| *rv == v)
        {
            return Ok(Track::Pinned(tag));
        }
        if is_hex(spec) && spec.len() >= 7 {
            return Ok(Track::Pinned(spec.to_lowercase()));
        }
        Err(format!("the repository has no branch, tag or version {spec}"))
    }
}

pub fn short(s: &str) -> &str {
    if s.len() == 40 && is_hex(s) { &s[..7] } else { s }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The branches and tags of a repository (`git ls-remote --symref`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Remote {
    /// The default branch's name.
    pub head: Option<String>,
    /// Name → commit.
    pub branches: BTreeMap<String, String>,
    /// Name → commit (annotated tags peeled).
    pub tags: BTreeMap<String, String>,
}

impl Remote {
    /// The output of `git ls-remote --symref <url> HEAD refs/heads/* refs/tags/*`.
    pub fn parse(listing: &str) -> Remote {
        let mut r = Remote::default();
        let mut peeled = BTreeMap::new();
        for line in listing.lines() {
            let Some((left, name)) = line.split_once('\t') else {
                continue;
            };
            if let Some(target) = left.strip_prefix("ref: ") {
                if name == "HEAD" {
                    r.head = target.strip_prefix("refs/heads/").map(str::to_owned);
                }
                continue;
            }
            if !is_hex(left) {
                continue;
            }
            let commit = left.to_lowercase();
            if let Some(b) = name.strip_prefix("refs/heads/") {
                r.branches.insert(b.to_owned(), commit);
            } else if let Some(t) = name.strip_prefix("refs/tags/") {
                match t.strip_suffix("^{}") {
                    Some(t) => {
                        peeled.insert(t.to_owned(), commit);
                    }
                    None => {
                        r.tags.insert(t.to_owned(), commit);
                    }
                }
            }
        }
        r.tags.extend(peeled);
        r
    }

    /// Tags that read as versions, newest first.
    pub fn releases(&self) -> Vec<(String, Version)> {
        let mut v: Vec<(String, Version)> = self
            .tags
            .keys()
            .filter_map(|t| Some((t.clone(), Version::parse(t)?)))
            .collect();
        // Newest first; of `v1.0` and `1.0.0`, the first by name.
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// The newest release matching `req`.
    pub fn newest(&self, req: VersionReq) -> Option<(String, Version)> {
        self.releases().into_iter().find(|(_, v)| req.matches(v))
    }

    /// The best name for a commit: its newest version tag, else any tag.
    pub fn tag_at(&self, commit: &str) -> Option<String> {
        let at = |t: &&String| self.tags.get(*t).is_some_and(|c| c == commit);
        self.releases()
            .into_iter()
            .map(|(t, _)| t)
            .find(|t| at(&t))
            .or_else(|| self.tags.keys().find(at).cloned())
    }

    /// Where a track points now.
    pub fn resolve(&self, track: &Track) -> Result<Target, String> {
        let tagged = |tag: &str, commit: &str| Target {
            commit: commit.to_owned(),
            fetch: format!("refs/tags/{tag}"),
            tag: tag.to_owned(),
        };
        match track {
            Track::Branch(b) => {
                let name = if b.is_empty() {
                    self.head.clone().ok_or("the repository has no default branch")?
                } else {
                    b.clone()
                };
                if let Some(c) = self.branches.get(&name) {
                    return Ok(Target {
                        commit: c.clone(),
                        fetch: format!("refs/heads/{name}"),
                        tag: self.tag_at(c).unwrap_or_default(),
                    });
                }
                // Installed by an older rune with a tag as its "branch".
                match self.tags.get(&name) {
                    Some(c) => Ok(tagged(&name, c)),
                    None => Err(format!("the repository has no branch {name}")),
                }
            }
            Track::Releases(r) => {
                let req = VersionReq::parse(r).ok_or_else(|| format!("{r:?} is not a version range"))?;
                if self.releases().is_empty() {
                    return Err("the repository has no releases (version tags)".into());
                }
                let (tag, _) = self
                    .newest(req)
                    .ok_or_else(|| format!("no release matches {req}"))?;
                Ok(tagged(&tag, &self.tags[&tag]))
            }
            Track::Pinned(at) => {
                if let Some(c) = self.tags.get(at) {
                    return Ok(tagged(at, c));
                }
                if !is_hex(at) || at.len() < 7 {
                    return Err(format!("the repository has no tag {at}"));
                }
                let at = at.to_lowercase();
                // A commit some branch or tag points at is fetched by that name.
                let named = self
                    .branches
                    .iter()
                    .map(|(n, c)| (format!("refs/heads/{n}"), c))
                    .chain(self.tags.iter().map(|(n, c)| (format!("refs/tags/{n}"), c)))
                    .find(|(_, c)| c.starts_with(&at));
                match named {
                    Some((fetch, c)) => Ok(Target {
                        commit: c.clone(),
                        tag: self.tag_at(c).unwrap_or_default(),
                        fetch,
                    }),
                    None if at.len() == 40 => Ok(Target {
                        tag: String::new(),
                        fetch: at.clone(),
                        commit: at,
                    }),
                    None => Err(format!(
                        "no branch or tag points at {at}: give the full 40-character commit"
                    )),
                }
            }
        }
    }
}

/// A commit to install and how to fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub commit: String,
    /// What to ask the remote for: a ref name, or the commit itself.
    pub fetch: String,
    /// Its version tag, if it has one (empty otherwise).
    pub tag: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_are_read_from_tags_and_ordered() {
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("2").to_string(), "2.0.0");
        assert_eq!(v("V1.2-rc.1+abc").to_string(), "1.2.0-rc.1");
        for bad in [
            "",
            "v",
            "x1",
            "1..2",
            "1.2.3.4",
            "1.-2",
            "1.2-",
            "release-1",
            "1.a",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("2.0.0-beta") < v("2.0.0") && v("2.0.0-beta") > v("1.9.0"));
        assert!(v("1.0.0-alpha") < v("1.0.0-beta"));
    }

    #[test]
    fn ranges_follow_cargos_rules() {
        let req = |s: &str| VersionReq::parse(s).unwrap();
        let yes = |r: &str, ver: &str| assert!(req(r).matches(&v(ver)), "{r} should match {ver}");
        let no = |r: &str, ver: &str| assert!(!req(r).matches(&v(ver)), "{r} should not match {ver}");
        yes("^1.2", "1.2.0");
        yes("^1.2", "1.9.3");
        no("^1.2", "1.1.9");
        no("^1.2", "2.0.0");
        yes("^0.2", "0.2.5");
        no("^0.2", "0.3.0");
        yes("^0.0.3", "0.0.3");
        no("^0.0.3", "0.0.4");
        yes("~1.2", "1.2.7");
        no("~1.2", "1.3.0");
        yes("~1", "1.9.0");
        yes("1.x", "1.4.0");
        no("1.x", "2.0.0");
        yes("1.2.x", "1.2.9");
        no("1.2.x", "1.3.0");
        yes("*", "9.0.0");
        no("*", "9.0.0-rc.1");
        no("^1", "1.5.0-beta");
        assert_eq!(req("latest"), VersionReq::Any);
        assert_eq!(req("^v1.2").to_string(), "^1.2");
        assert_eq!(req("1.x").to_string(), "^1");
        assert_eq!(req("1.2.x").to_string(), "~1.2");
        for bad in ["", "^", "^a", "1", "1.2.3.x", "~1.2.3.4", ">=1"] {
            assert!(VersionReq::parse(bad).is_none(), "{bad}");
        }
    }

    const C1: &str = "1111111111111111111111111111111111111111";
    const C2: &str = "2222222222222222222222222222222222222222";
    const C3: &str = "3333333333333333333333333333333333333333";
    const C4: &str = "4444444444444444444444444444444444444444";
    const TAGOBJ: &str = "9999999999999999999999999999999999999999";

    fn remote() -> Remote {
        Remote::parse(&format!(
            "ref: refs/heads/main\tHEAD\n{C4}\tHEAD\n{C4}\trefs/heads/main\n{C3}\trefs/heads/next\n\
             {C1}\trefs/tags/v1.0.0\n{C2}\trefs/tags/v1.1.0\n{TAGOBJ}\trefs/tags/v2.0.0\n{C3}\trefs/tags/v2.0.0^{{}}\n\
             {C4}\trefs/tags/v3.0.0-beta\n{C4}\trefs/tags/nightly\nwarning: junk\n"
        ))
    }

    #[test]
    fn listings_are_parsed_with_annotated_tags_peeled() {
        let r = remote();
        assert_eq!(r.head.as_deref(), Some("main"));
        assert_eq!(r.branches.len(), 2);
        assert_eq!(r.tags["v2.0.0"], C3, "the commit, not the tag object");
        let names: Vec<String> = r.releases().into_iter().map(|(t, _)| t).collect();
        assert_eq!(names, ["v3.0.0-beta", "v2.0.0", "v1.1.0", "v1.0.0"]);
        assert_eq!(r.tag_at(C4).as_deref(), Some("v3.0.0-beta"));
        assert_eq!(r.tag_at(C3).as_deref(), Some("v2.0.0"));
    }

    #[test]
    fn tracks_resolve_to_the_right_commit() {
        let r = remote();
        let at = |t: Track| r.resolve(&t).map(|x| (x.commit, x.fetch, x.tag));
        assert_eq!(
            at(Track::default()),
            Ok((C4.into(), "refs/heads/main".into(), "v3.0.0-beta".into()))
        );
        assert_eq!(at(Track::Branch("next".into())).unwrap().0, C3);
        assert_eq!(
            at(Track::Releases("*".into())),
            Ok((C3.into(), "refs/tags/v2.0.0".into(), "v2.0.0".into())),
            "pre-releases are skipped"
        );
        assert_eq!(at(Track::Releases("^1".into())).unwrap().2, "v1.1.0");
        assert_eq!(at(Track::Releases("~1.0".into())).unwrap().2, "v1.0.0");
        assert!(
            at(Track::Releases("^4".into()))
                .unwrap_err()
                .contains("no release matches ^4")
        );
        assert_eq!(at(Track::Pinned("v3.0.0-beta".into())).unwrap().0, C4);
        // A commit: by the ref that points at it, or by itself when full.
        assert_eq!(
            at(Track::Pinned("2222222".into())),
            Ok((C2.into(), "refs/tags/v1.1.0".into(), "v1.1.0".into()))
        );
        let lone = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
        assert_eq!(
            at(Track::Pinned(lone.into())),
            Ok((lone.into(), lone.into(), String::new()))
        );
        assert!(
            at(Track::Pinned("abcdef1".into()))
                .unwrap_err()
                .contains("full 40-character")
        );
        assert!(at(Track::Pinned("v9".into())).unwrap_err().contains("no tag v9"));
        assert!(
            at(Track::Branch("gone".into()))
                .unwrap_err()
                .contains("no branch gone")
        );
        // An older install's "branch" that is really a tag.
        assert_eq!(at(Track::Branch("v1.0.0".into())).unwrap().0, C1);
        assert!(
            Remote::default()
                .resolve(&Track::Releases("*".into()))
                .unwrap_err()
                .contains("no releases")
        );
    }

    #[test]
    fn what_the_user_types_becomes_a_track() {
        let r = remote();
        let t = |s: &str| Track::from_spec(s, &r);
        assert_eq!(t(""), Ok(Track::Branch(String::new())));
        assert_eq!(t("latest"), Ok(Track::Releases("*".into())));
        assert_eq!(t("^1.0"), Ok(Track::Releases("^1.0".into())));
        assert_eq!(t("1.x"), Ok(Track::Releases("^1".into())));
        assert_eq!(t("v1.1.0"), Ok(Track::Pinned("v1.1.0".into())));
        assert_eq!(
            t("1.1"),
            Ok(Track::Pinned("v1.1.0".into())),
            "a version names its tag"
        );
        assert_eq!(t("nightly"), Ok(Track::Pinned("nightly".into())));
        assert_eq!(t("next"), Ok(Track::Branch("next".into())));
        assert_eq!(t("ABCDEF1"), Ok(Track::Pinned("abcdef1".into())));
        assert!(t("1.7").unwrap_err().contains("no branch, tag or version 1.7"));
        assert!(t("^x").is_err());
        assert_eq!(Track::Pinned(C1.into()).describe(), "pinned to 1111111");
        assert_eq!(Track::Releases("^1".into()).describe(), "newest ^1 release");
        let json = serde_json::to_string(&Track::Releases("^1".into())).unwrap();
        assert_eq!(json, r#"{"releases":"^1"}"#);
    }
}
