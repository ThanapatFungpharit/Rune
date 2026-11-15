//! Noticing changes in the plugin directory, for hot reload.
//!
//! Polling, not file-system notifications: a plugin directory holds a few
//! small directories, so looking at the size and modification time of each
//! plugin's files every second costs nothing and works on every platform
//! without another dependency. A change is reported only once the files have
//! stayed the same for one more look, so a module that is still being
//! written (a build, a copy, an unpacked update) is not reloaded half-done.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

/// One file: name, size, modification time.
type Stamp = (String, u64, Option<SystemTime>);

/// Plugin directory name → the files directly in it.
pub type Snapshot = BTreeMap<String, Vec<Stamp>>;

/// The plugin directory as it is now (hidden entries skipped, like
/// discovery skips them; links followed, so a linked development folder is
/// watched too).
pub fn snapshot(dir: &Path) -> Snapshot {
    let mut snap = Snapshot::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return snap;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let mut files: Vec<Stamp> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| {
                let m = std::fs::metadata(f.path()).ok().filter(|m| m.is_file())?;
                Some((
                    f.file_name().to_string_lossy().into_owned(),
                    m.len(),
                    m.modified().ok(),
                ))
            })
            .collect();
        files.sort();
        snap.insert(name, files);
    }
    snap
}

/// Directories added, removed or with different files between two looks.
pub fn changed(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let mut ids: Vec<String> = before
        .iter()
        .filter(|(id, files)| after.get(*id) != Some(files))
        .map(|(id, _)| id.clone())
        .chain(after.keys().filter(|id| !before.contains_key(*id)).cloned())
        .collect();
    ids.sort();
    ids
}

/// Compares each look with the last settled one.
pub struct Watcher {
    settled: Snapshot,
    /// A look that differed from `settled`, waiting to be seen again.
    pending: Option<Snapshot>,
}

impl Watcher {
    pub fn new(base: Snapshot) -> Self {
        Watcher {
            settled: base,
            pending: None,
        }
    }

    /// The plugins that changed, once the directory has stopped changing.
    pub fn look(&mut self, now: Snapshot) -> Vec<String> {
        if now == self.settled {
            self.pending = None;
            return Vec::new();
        }
        if self.pending.as_ref() != Some(&now) {
            self.pending = Some(now);
            return Vec::new();
        }
        self.pending = None;
        let ids = changed(&self.settled, &now);
        self.settled = now;
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::tests::TempDir;

    #[test]
    fn changes_are_reported_once_they_settle() {
        let dir = TempDir::new("watch");
        let a = dir.0.join("a");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("plugin.json"), "{}").unwrap();
        std::fs::create_dir_all(dir.0.join(".staging/x")).unwrap();
        std::fs::write(dir.0.join("stray"), "x").unwrap();
        let base = snapshot(&dir.0);
        assert_eq!(
            base.keys().collect::<Vec<_>>(),
            ["a"],
            "hidden entries and files are skipped"
        );
        let mut w = Watcher::new(base);
        assert!(w.look(snapshot(&dir.0)).is_empty(), "nothing changed");

        // A module being written: reported only when two looks agree.
        std::fs::write(a.join("plugin.wasm"), "half").unwrap();
        assert!(w.look(snapshot(&dir.0)).is_empty());
        std::fs::write(a.join("plugin.wasm"), "the whole module").unwrap();
        assert!(w.look(snapshot(&dir.0)).is_empty(), "still changing");
        assert_eq!(w.look(snapshot(&dir.0)), ["a"]);
        assert!(w.look(snapshot(&dir.0)).is_empty(), "reported once");

        // Added and removed directories; a change that is undone is no change.
        std::fs::create_dir_all(dir.0.join("b")).unwrap();
        std::fs::remove_dir_all(&a).unwrap();
        w.look(snapshot(&dir.0));
        assert_eq!(w.look(snapshot(&dir.0)), ["a", "b"]);
        std::fs::create_dir_all(dir.0.join("c")).unwrap();
        assert!(w.look(snapshot(&dir.0)).is_empty());
        std::fs::remove_dir_all(dir.0.join("c")).unwrap();
        assert!(w.look(snapshot(&dir.0)).is_empty());
        assert!(w.look(snapshot(&dir.0)).is_empty());
    }
}
