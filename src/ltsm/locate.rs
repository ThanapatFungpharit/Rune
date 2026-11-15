//! Finding the user's own copy of `ltsm.wasm`.
//!
//! The module is LINE's code, so rune does not ship it: each user supplies the
//! copy from their own LINE Chrome extension. Only the exact build the import
//! shims were written for (extension 3.7.2) is accepted.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use sha2::{Digest, Sha256};

/// SHA-256 of the one `ltsm.wasm` build rune can host (LINE Chrome 3.7.2).
pub const LTSM_SHA256: &str = "58bb4e189ab9bbd7d72ed415258da36afd50e306cb019bab75329d4f5f1b65b3";

/// Environment variable naming the module's path; when set, only that file is tried.
pub const LTSM_ENV: &str = "RUNE_LTSM";

/// The extension's id in the Chrome Web Store.
const EXTENSION_ID: &str = "ophjlpahpchlmihnnnihgmmeilfjmjjc";

/// Why no usable `ltsm.wasm` was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmProblem {
    /// No candidate file exists; `searched` lists the files looked for (browser
    /// extension folders are searched too, unless `RUNE_LTSM` is set).
    Missing { searched: Vec<PathBuf> },
    /// Files were found, but none is the supported build (or readable).
    /// Each entry is a path and what was wrong with it.
    Unusable {
        found: Vec<(PathBuf, String)>,
        searched: Vec<PathBuf>,
    },
}

impl fmt::Display for WasmProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmProblem::Missing { .. } => write!(f, "LINE's ltsm.wasm was not found"),
            WasmProblem::Unusable { found, .. } => {
                write!(f, "no usable ltsm.wasm (")?;
                for (i, (p, why)) in found.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}: {why}", p.display())?;
                }
                write!(f, ")")
            }
        }
    }
}

impl std::error::Error for WasmProblem {}

impl WasmProblem {
    /// The places that were searched, in order.
    pub fn searched(&self) -> &[PathBuf] {
        match self {
            WasmProblem::Missing { searched } | WasmProblem::Unusable { searched, .. } => searched,
        }
    }

    /// A multi-line explanation for people: what is wrong and how to fix it.
    pub fn help(&self) -> String {
        let mut s = format!("{self}.\n\n");
        s.push_str(
            "rune runs LINE's own secure module (ltsm.wasm) but does not ship it, because it is\n\
             LINE's code. Use the copy from your own LINE Chrome extension (version 3.7.2):\n\n",
        );
        s.push_str(&format!("  sha256 {LTSM_SHA256}\n\n"));
        if let Some(p) = default_install_path() {
            s.push_str(&format!("Copy it to {}\n", p.display()));
        }
        s.push_str(&format!("or point {LTSM_ENV} at it.\n\nSearched:\n"));
        for p in self.searched() {
            s.push_str(&format!("  {}\n", p.display()));
        }
        if self.searched().len() > 1 {
            s.push_str("  the LINE extension in Chrome, Chromium, Brave, Edge and Vivaldi profiles\n");
        }
        s
    }
}

/// Where rune looks for the module by default: `$XDG_DATA_HOME/rune/ltsm.wasm`
/// (`~/.local/share/rune/ltsm.wasm`).
pub fn default_install_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("rune").join("ltsm.wasm"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local/share")))
}

/// Browser profile roots that may hold the extension, relative to `$HOME`.
const BROWSER_DIRS: &[&str] = &[
    ".config/google-chrome",
    ".config/google-chrome-beta",
    ".config/google-chrome-unstable",
    ".config/chromium",
    ".config/BraveSoftware/Brave-Browser",
    ".config/microsoft-edge",
    ".config/vivaldi",
    "snap/chromium/common/chromium",
    ".var/app/com.google.Chrome/config/google-chrome",
    ".var/app/org.chromium.Chromium/config/chromium",
    ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Chromium",
    "Library/Application Support/BraveSoftware/Brave-Browser",
    "Library/Application Support/Microsoft Edge",
];

/// Installed copies of the extension: `<browser>/<profile>/Extensions/<id>/<version>`.
fn extension_dirs(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for b in BROWSER_DIRS {
        let Ok(profiles) = std::fs::read_dir(home.join(b)) else {
            continue;
        };
        for p in profiles.flatten() {
            let Ok(versions) = std::fs::read_dir(p.path().join("Extensions").join(EXTENSION_ID)) else {
                continue;
            };
            out.extend(versions.flatten().map(|v| v.path()).filter(|v| v.is_dir()));
        }
    }
    out.sort();
    out
}

/// Every `*.wasm` under `dir`, a few levels deep.
fn wasm_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth > 0 {
            wasm_files(&p, depth - 1, out);
        } else if t.is_file() && p.extension().is_some_and(|x| x == "wasm") {
            out.push(p);
        }
    }
}

/// The places searched, and the candidate files in the order they are tried.
/// A file is `named` when the user put it there (as opposed to one of the
/// extension's other wasm modules), so a wrong build there is worth reporting.
fn candidates() -> (Vec<PathBuf>, Vec<(PathBuf, bool)>) {
    if let Some(p) = std::env::var_os(LTSM_ENV).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return (vec![p.clone()], vec![(p, true)]);
    }
    let mut searched = Vec::new();
    let mut files = Vec::new();
    for p in default_install_path()
        .into_iter()
        .chain([PathBuf::from("assets/ltsm.wasm")])
    {
        searched.push(p.clone());
        files.push((p, true));
    }
    if let Some(h) = home() {
        let mut found = Vec::new();
        for d in extension_dirs(&h) {
            wasm_files(&d, 3, &mut found);
        }
        files.extend(found.into_iter().map(|p| {
            let named = p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains("ltsm"));
            (p, named)
        }));
    }
    (searched, files)
}

/// Read `path` and check that it is the supported build.
pub fn check(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if sha == LTSM_SHA256 {
        Ok(bytes)
    } else {
        Err(format!("a different build (sha256 {}…)", &sha[..8]))
    }
}

fn find() -> Result<Vec<u8>, WasmProblem> {
    let (searched, files) = candidates();
    let mut found = Vec::new();
    for (p, named) in files {
        if !p.exists() {
            continue;
        }
        match check(&p) {
            Ok(bytes) => {
                tracing::debug!(path = %p.display(), "using ltsm.wasm");
                return Ok(bytes);
            }
            Err(why) if named => found.push((p, why)),
            Err(_) => {}
        }
    }
    if found.is_empty() {
        Err(WasmProblem::Missing { searched })
    } else {
        Err(WasmProblem::Unusable { found, searched })
    }
}

/// The supported `ltsm.wasm`, found once per process (see the module docs for
/// where it is looked for). A failed search is retried on the next call.
pub fn load() -> Result<Arc<[u8]>, WasmProblem> {
    static FOUND: OnceLock<Arc<[u8]>> = OnceLock::new();
    if let Some(b) = FOUND.get() {
        return Ok(b.clone());
    }
    let bytes: Arc<[u8]> = find()?.into();
    Ok(FOUND.get_or_init(|| bytes).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rune-locate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_wasm_in_installed_extension_versions() {
        let home = temp_home("ext");
        let v = home
            .join(".config/chromium/Profile 1/Extensions")
            .join(EXTENSION_ID)
            .join("3.7.2_0");
        std::fs::create_dir_all(v.join("static/wasm")).unwrap();
        std::fs::write(v.join("static/wasm/ltsm.wasm"), b"x").unwrap();
        std::fs::write(v.join("manifest.json"), b"{}").unwrap();
        let dirs = extension_dirs(&home);
        assert_eq!(dirs, vec![v.clone()]);
        let mut files = Vec::new();
        wasm_files(&dirs[0], 3, &mut files);
        assert_eq!(files, vec![v.join("static/wasm/ltsm.wasm")]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rejects_other_builds() {
        let home = temp_home("check");
        let f = home.join("ltsm.wasm");
        std::fs::write(&f, b"\0asm not this one").unwrap();
        assert!(check(&f).unwrap_err().starts_with("a different build"));
        assert!(check(&home.join("absent.wasm")).is_err());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn help_says_what_to_do() {
        let p = WasmProblem::Missing {
            searched: vec![PathBuf::from("/a/ltsm.wasm"), "assets/ltsm.wasm".into()],
        };
        let h = p.help();
        assert!(h.contains(LTSM_SHA256) && h.contains(LTSM_ENV) && h.contains("/a/ltsm.wasm"));
    }
}
