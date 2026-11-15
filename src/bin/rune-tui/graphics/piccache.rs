//! Decoded pictures kept in memory up to a byte budget: inline thumbnails,
//! the sticker picker's stickers and the file browser's previews. Past the
//! budget the least recently drawn pictures go (they are fetched again if
//! they come back on screen), but never one drawn in the current or the
//! previous frame, so what is on screen never flickers or reloads in a loop.
//! `Loading` slots always stay (their result is on its way); failures are
//! kept (so they are not retried on every frame) and cost a little.

use std::cell::Cell;
use std::collections::HashMap;

use crate::Load;

/// Memory a cached value holds, in bytes (an estimate is fine).
pub trait Weigh {
    fn weight(&self) -> usize;
}

/// What a remembered failure counts as, besides its message.
const FAILED_WEIGHT: usize = 64;

struct Slot<V> {
    load: Load<V>,
    /// Clock value of the last use.
    used: Cell<u64>,
}

pub struct PicCache<V> {
    map: HashMap<String, Slot<V>>,
    budget: usize,
    /// Bumped on every use: a total order of uses.
    clock: Cell<u64>,
    /// Clock value when the previous frame started: slots used since are
    /// on screen (or about to be) and are never evicted.
    keep_since: u64,
    /// Clock value when the current frame started.
    frame_start: u64,
}

impl<V: Weigh> PicCache<V> {
    pub fn new(budget: usize) -> Self {
        PicCache {
            map: HashMap::new(),
            budget,
            clock: Cell::new(0),
            keep_since: 0,
            frame_start: 0,
        }
    }

    fn tick(&self) -> u64 {
        let t = self.clock.get() + 1;
        self.clock.set(t);
        t
    }

    /// Look a picture up, marking it used.
    pub fn get(&self, key: &str) -> Option<&Load<V>> {
        let slot = self.map.get(key)?;
        slot.used.set(self.tick());
        Some(&slot.load)
    }

    /// Look a picture up to draw it, marking it used.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Load<V>> {
        let t = self.tick();
        let slot = self.map.get_mut(key)?;
        slot.used.set(t);
        Some(&mut slot.load)
    }

    /// Whether `key` has a slot, without counting as a use.
    pub fn contains_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub fn insert(&mut self, key: String, load: Load<V>) {
        let used = Cell::new(self.tick());
        self.map.insert(key, Slot { load, used });
        self.trim();
    }

    pub fn remove(&mut self, key: &str) {
        self.map.remove(key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Call once per frame, before drawing: drops what went over budget
    /// (pictures grow when they are encoded for a new size) and starts
    /// tracking which slots the new frame draws.
    pub fn new_frame(&mut self) {
        self.trim();
        self.keep_since = self.frame_start;
        self.frame_start = self.clock.get();
    }

    fn slot_weight(load: &Load<V>) -> usize {
        match load {
            Load::Ready(v) => v.weight(),
            Load::Failed(e) => FAILED_WEIGHT + e.len(),
            Load::Loading => 0,
        }
    }

    /// Memory held by everything cached.
    pub fn weight(&self) -> usize {
        self.map.values().map(|s| Self::slot_weight(&s.load)).sum()
    }

    /// Evict least recently used slots until the cache fits its budget.
    fn trim(&mut self) {
        let mut total = self.weight();
        if total <= self.budget {
            return;
        }
        let mut old: Vec<(u64, String, usize)> = self
            .map
            .iter()
            .filter(|(_, s)| !matches!(s.load, Load::Loading) && s.used.get() <= self.keep_since)
            .map(|(k, s)| (s.used.get(), k.clone(), Self::slot_weight(&s.load)))
            .collect();
        old.sort_unstable_by_key(|(used, ..)| *used);
        for (_, key, w) in old {
            if total <= self.budget {
                break;
            }
            self.map.remove(&key);
            total -= w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Blob(usize);
    impl Weigh for Blob {
        fn weight(&self) -> usize {
            self.0
        }
    }

    fn ready(c: &PicCache<Blob>) -> Vec<&str> {
        let mut v: Vec<&str> = c
            .map
            .iter()
            .filter(|(_, s)| matches!(s.load, Load::Ready(_)))
            .map(|(k, _)| k.as_str())
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn evicts_least_recently_drawn_beyond_the_budget() {
        let mut c = PicCache::new(300);
        for k in ["a", "b", "c"] {
            c.insert(k.into(), Load::Ready(Blob(100)));
        }
        // Two frames later none of them is on screen; "a" is drawn again.
        c.new_frame();
        c.new_frame();
        c.get_mut("a");
        c.new_frame();
        c.new_frame();
        c.insert("d".into(), Load::Ready(Blob(100)));
        assert_eq!(ready(&c), ["a", "c", "d"], "b was used least recently");
        assert!(c.weight() <= 300);
    }

    #[test]
    fn never_evicts_what_is_on_screen_or_loading() {
        let mut c = PicCache::new(100);
        c.insert("loading".into(), Load::Loading);
        // A frame that draws more than the budget keeps all of it.
        c.new_frame();
        for k in ["a", "b", "c"] {
            c.insert(k.into(), Load::Ready(Blob(100)));
        }
        c.new_frame();
        assert_eq!(ready(&c), ["a", "b", "c"]);
        // Still drawn in the next frame: kept.
        for k in ["a", "b", "c"] {
            c.get("a");
            c.get_mut(k);
        }
        c.new_frame();
        assert_eq!(c.len(), 4);
        // Scrolled away: trimmed back to the budget, oldest first.
        c.get_mut("c");
        c.new_frame();
        c.new_frame();
        assert_eq!(ready(&c), ["c"]);
        assert!(c.contains_key("loading"), "a pending download stays");
    }

    #[test]
    fn failures_are_remembered_and_bounded() {
        let mut c: PicCache<Blob> = PicCache::new(1_000);
        for i in 0..100 {
            c.insert(i.to_string(), Load::Failed("gone".into()));
        }
        c.new_frame();
        c.new_frame();
        c.new_frame();
        assert!(c.weight() <= 1_000);
        assert!(c.len() < 100 && c.contains_key("99"), "the newest failure stays");
        c.remove("99");
        assert!(!c.contains_key("99"));
    }
}
