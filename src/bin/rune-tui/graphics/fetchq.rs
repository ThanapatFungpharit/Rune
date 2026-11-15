//! Which pictures to download next: what is on screen first, then what is
//! nearest to it.
//!
//! Drawing says what it wants each frame (`want`, with how far the item is
//! from the visible area); after the frame, `take` hands out the most urgent
//! wants that fit in their kind's free download slots and forgets the rest,
//! which the next frame asks for again — with the distance it then has. So
//! scrolling re-orders what is waiting, and what is no longer drawn (a closed
//! chat, a chat list scrolled away) is not fetched. `done` frees a slot.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Rows between an item and the visible area: 0 is on screen.
pub type Distance = u32;

/// On screen.
pub const VISIBLE: Distance = 0;
/// Asked for without knowing where it is drawn (until `nearer` says).
pub const UNPLACED: Distance = 10_000;

/// A kind of download, each with its own number of slots (so quick avatar
/// loads never wait behind large pictures).
pub trait Kind {
    /// Downloads of this kind that may run at once.
    fn limit(&self) -> usize;
}

pub struct FetchQueue<K, T> {
    /// This frame's wants: distance and what to start, per key.
    wanted: HashMap<K, (Distance, u64, T)>,
    running: HashSet<K>,
    /// Order of first want this frame: ties go to what was asked first.
    seq: u64,
}

impl<K: Kind + Eq + Hash + Clone, T> Default for FetchQueue<K, T> {
    fn default() -> Self {
        Self {
            wanted: HashMap::new(),
            running: HashSet::new(),
            seq: 0,
        }
    }
}

impl<K: Kind + Eq + Hash + Clone, T> FetchQueue<K, T> {
    /// Ask for `key` this frame. Asking again keeps the nearest distance;
    /// a key already downloading is ignored.
    pub fn want(&mut self, key: K, distance: Distance, start: T) {
        if self.running.contains(&key) {
            return;
        }
        match self.wanted.get_mut(&key) {
            Some(w) => w.0 = w.0.min(distance),
            None => {
                self.seq += 1;
                self.wanted.insert(key, (distance, self.seq, start));
            }
        }
    }

    /// Lower the distance of a key wanted this frame (once its place is known).
    pub fn nearer(&mut self, key: &K, distance: Distance) {
        if let Some(w) = self.wanted.get_mut(key) {
            w.0 = w.0.min(distance);
        }
    }

    /// Waiting (wanted but not started) this frame.
    pub fn is_waiting(&self, key: &K) -> bool {
        self.wanted.contains_key(key)
    }

    /// The downloads to start now, most urgent first, marked running; every
    /// other want is dropped until the next frame asks again.
    pub fn take(&mut self) -> Vec<(K, T)> {
        let mut wants: Vec<_> = self.wanted.drain().collect();
        self.seq = 0;
        wants.sort_by_key(|(_, (d, seq, _))| (*d, *seq));
        let mut busy: Vec<(K, usize)> = Vec::new();
        for k in &self.running {
            match busy.iter_mut().find(|(b, _)| same_kind(b, k)) {
                Some((_, n)) => *n += 1,
                None => busy.push((k.clone(), 1)),
            }
        }
        let mut out = Vec::new();
        for (key, (_, _, start)) in wants {
            let n = match busy.iter_mut().find(|(b, _)| same_kind(b, &key)) {
                Some((_, n)) => n,
                None => {
                    busy.push((key.clone(), 0));
                    &mut busy.last_mut().expect("just pushed").1
                }
            };
            if *n < key.limit() {
                *n += 1;
                self.running.insert(key.clone());
                out.push((key, start));
            }
        }
        out
    }

    /// A download finished (or failed): its slot is free.
    pub fn done(&mut self, key: &K) {
        self.running.remove(key);
    }
}

fn same_kind<K>(a: &K, b: &K) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// What the TUI downloads through the queue.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum MediaKey {
    /// A message's inline picture, by message id.
    Thumb(String),
    /// A profile/group picture, by mid.
    Avatar(String),
}

/// Inline pictures are large downloads (≤1280 px originals). They share one
/// HTTP/2 connection to OBS and decrypt/decode off the async workers, so six at
/// a time fill a screen quickly while leaving bandwidth for originals opened
/// by hand.
pub const THUMB_SLOTS: usize = 6;
/// Avatars are small (and usually come from the store): many at once.
pub const AVATAR_SLOTS: usize = 12;

impl Kind for MediaKey {
    fn limit(&self) -> usize {
        match self {
            MediaKey::Thumb(_) => THUMB_SLOTS,
            MediaKey::Avatar(_) => AVATAR_SLOTS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(id: &str) -> MediaKey {
        MediaKey::Thumb(id.into())
    }

    fn keys(v: Vec<(MediaKey, ())>) -> Vec<MediaKey> {
        v.into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn visible_first_then_nearest() {
        let mut q = FetchQueue::default();
        q.want(thumb("old"), 40, ());
        q.want(thumb("near"), 3, ());
        q.want(thumb("seen"), VISIBLE, ());
        q.want(thumb("far"), 90, ());
        assert_eq!(
            keys(q.take()),
            [thumb("seen"), thumb("near"), thumb("old"), thumb("far")]
        );
    }

    #[test]
    fn slots_are_per_kind_and_freed_by_done() {
        let mut q = FetchQueue::default();
        for i in 0..THUMB_SLOTS + 2 {
            q.want(thumb(&i.to_string()), i as Distance, ());
        }
        q.want(MediaKey::Avatar("Ua".into()), 50, ());
        let started = keys(q.take());
        assert_eq!(
            started.len(),
            THUMB_SLOTS + 1,
            "the avatar does not wait for thumbnails"
        );
        assert!(started.contains(&MediaKey::Avatar("Ua".into())));
        // Nothing free: the next frame's wants wait (and are dropped after `take`).
        q.want(thumb("late"), VISIBLE, ());
        assert!(q.take().is_empty());
        assert!(!q.is_waiting(&thumb("late")));
        q.done(&thumb("0"));
        q.want(thumb("late"), VISIBLE, ());
        q.want(thumb("later"), 5, ());
        assert_eq!(keys(q.take()), [thumb("late")]);
    }

    #[test]
    fn running_is_not_started_twice_and_place_can_come_later() {
        let mut q = FetchQueue::default();
        q.want(thumb("a"), VISIBLE, ());
        assert_eq!(q.take().len(), 1);
        q.want(thumb("a"), VISIBLE, ());
        assert!(q.take().is_empty());
        // Asked for during layout, placed on screen afterwards.
        q.want(thumb("b"), UNPLACED, ());
        q.want(thumb("c"), 7, ());
        q.nearer(&thumb("b"), VISIBLE);
        q.want(thumb("c"), 2, ());
        assert_eq!(keys(q.take()), [thumb("b"), thumb("c")]);
    }
}
