//! Where rune keeps its files, per platform.
//!
//! Unix (Linux, macOS): the XDG base directories — `$XDG_DATA_HOME/rune`
//! (`~/.local/share/rune`), `$XDG_CACHE_HOME/rune` (`~/.cache/rune`),
//! `$XDG_CONFIG_HOME/rune` (`~/.config/rune`).
//!
//! Windows: `%LOCALAPPDATA%\rune` for data (session, store, `ltsm.wasm`,
//! plugins: never roamed to other machines), `%LOCALAPPDATA%\rune\cache`,
//! `%APPDATA%\rune` for configuration. An absolute `XDG_*` variable still
//! wins there, so a setup that sets them keeps working. Secret files rely on
//! the per-user ACLs of the profile folders (Unix permissions do not exist).
//!
//! Every function returns `None` when the variables it needs are unset or
//! empty; callers decide the fallback.

use std::ffi::OsString;
use std::path::PathBuf;

/// The user's home directory: `$HOME`, on Windows `%USERPROFILE%`.
pub fn home() -> Option<PathBuf> {
    home_in(&env)
}

/// rune's data directory (session, store, `ltsm.wasm`, plugins).
pub fn data() -> Option<PathBuf> {
    data_in(&env)
}

/// rune's cache directory (files opened in viewers, notification icons).
pub fn cache() -> Option<PathBuf> {
    cache_in(&env)
}

/// rune's configuration directory (`keys.json`).
pub fn config() -> Option<PathBuf> {
    config_in(&env)
}

/// The user's download folder (`~/Downloads`).
pub fn downloads() -> Option<PathBuf> {
    Some(home()?.join("Downloads"))
}

fn env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// A variable holding a non-empty path.
fn var(get: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    get(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// An XDG variable: only an absolute path counts, as the spec requires.
fn xdg(get: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    var(get, name).filter(|p| p.is_absolute())
}

pub(crate) fn home_in(get: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if cfg!(windows) {
        var(get, "USERPROFILE").or_else(|| var(get, "HOME"))
    } else {
        var(get, "HOME")
    }
}

pub(crate) fn data_in(get: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(d) = xdg(get, "XDG_DATA_HOME") {
        return Some(d.join("rune"));
    }
    if cfg!(windows) {
        return var(get, "LOCALAPPDATA")
            .or_else(|| Some(home_in(get)?.join("AppData").join("Local")))
            .map(|d| d.join("rune"));
    }
    Some(home_in(get)?.join(".local").join("share").join("rune"))
}

pub(crate) fn cache_in(get: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(d) = xdg(get, "XDG_CACHE_HOME") {
        return Some(d.join("rune"));
    }
    if cfg!(windows) {
        return var(get, "LOCALAPPDATA")
            .or_else(|| Some(home_in(get)?.join("AppData").join("Local")))
            .map(|d| d.join("rune").join("cache"));
    }
    Some(home_in(get)?.join(".cache").join("rune"))
}

pub(crate) fn config_in(get: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(d) = xdg(get, "XDG_CONFIG_HOME") {
        return Some(d.join("rune"));
    }
    if cfg!(windows) {
        return var(get, "APPDATA")
            .or_else(|| Some(home_in(get)?.join("AppData").join("Roaming")))
            .map(|d| d.join("rune"));
    }
    Some(home_in(get)?.join(".config").join("rune"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn xdg_wins_only_when_absolute() {
        let get = with(&[
            ("XDG_DATA_HOME", "relative"),
            ("HOME", "/h"),
            ("USERPROFILE", "/h"),
        ]);
        let d = data_in(&get).unwrap();
        assert!(d.ends_with("rune"), "{d:?}");
        assert!(!d.starts_with("relative"), "{d:?}");
        let get = with(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h"), ("USERPROFILE", "/h")]);
        assert_eq!(data_in(&get), Some(PathBuf::from("/x/rune")));
        let get = with(&[("XDG_CONFIG_HOME", "/c"), ("XDG_CACHE_HOME", "/k")]);
        assert_eq!(config_in(&get), Some(PathBuf::from("/c/rune")));
        assert_eq!(cache_in(&get), Some(PathBuf::from("/k/rune")));
    }

    #[test]
    fn nothing_known_is_none() {
        let get = with(&[("HOME", "")]);
        assert_eq!(home_in(&get), None);
        assert_eq!(data_in(&get), None);
        assert_eq!(cache_in(&get), None);
        assert_eq!(config_in(&get), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_defaults() {
        let get = with(&[("HOME", "/h")]);
        assert_eq!(data_in(&get), Some(PathBuf::from("/h/.local/share/rune")));
        assert_eq!(cache_in(&get), Some(PathBuf::from("/h/.cache/rune")));
        assert_eq!(config_in(&get), Some(PathBuf::from("/h/.config/rune")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_defaults() {
        let get = with(&[
            ("USERPROFILE", r"C:\Users\u"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
        ]);
        assert_eq!(
            data_in(&get),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\rune"))
        );
        assert_eq!(
            cache_in(&get),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\rune\cache"))
        );
        assert_eq!(
            config_in(&get),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming\rune"))
        );
        let get = with(&[("USERPROFILE", r"C:\Users\u")]);
        assert_eq!(
            data_in(&get),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\rune"))
        );
    }
}
