//! Loads that failed, waiting to be tried again.
//!
//! Each failed load is a [`Job`] with its own schedule: exponential backoff
//! from [`FIRST_WAIT`] up to [`MAX_WAIT`] (per-item loads — one avatar, one
//! thumbnail, one profile — top out at [`ITEM_MAX_WAIT`], since some media is
//! simply gone). A job's success or failure never moves another job's
//! schedule: a burst of loads that failed together is not retried together
//! the moment one of them comes through. Every job is tried for as long as
//! rune runs; per-item ones only while something still shows them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Something that can be loaded again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Job {
    /// The start-up chat list (`getMessageBoxes`).
    ChatList,
    /// Pending group invitations.
    Invitations,
    /// The friend list.
    Contacts,
    /// The operation stream (it reconnects by itself; this is for when it gave up).
    Stream,
    /// A chat's recent history.
    History(String),
    /// A profile/group picture beside names.
    Avatar(String),
    /// A message's inline picture.
    Thumb(String),
    /// A profile page.
    Profile(String),
    /// Our own profile on the settings page.
    MyProfile,
    /// Account settings on the settings page.
    ServerSettings,
    /// A chat's read receipt (see `receipts.rs`).
    Receipt(String),
}

impl Job {
    /// Loads of one item, which may be gone for good.
    fn is_item(&self) -> bool {
        matches!(self, Job::Avatar(_) | Job::Thumb(_) | Job::Profile(_))
    }
}

/// Wait after the first failure; doubled per further failure.
const FIRST_WAIT: Duration = Duration::from_secs(2);
/// Longest wait between attempts of a background load.
const MAX_WAIT: Duration = Duration::from_secs(300);
/// Longest wait between attempts of a per-item load.
const ITEM_MAX_WAIT: Duration = Duration::from_secs(600);

struct Entry {
    failures: u32,
    /// When to try again; `None` while an attempt is running.
    due: Option<Instant>,
}

#[derive(Default)]
pub struct RetryQueue {
    entries: HashMap<Job, Entry>,
}

impl RetryQueue {
    /// Note a failure; returns how long until the next attempt.
    pub fn failed(&mut self, job: Job, now: Instant) -> Duration {
        let cap = if job.is_item() { ITEM_MAX_WAIT } else { MAX_WAIT };
        let e = self.entries.entry(job).or_insert(Entry {
            failures: 0,
            due: None,
        });
        e.failures += 1;
        let wait = FIRST_WAIT.saturating_mul(1 << (e.failures - 1).min(16)).min(cap);
        e.due = Some(now + wait);
        wait
    }

    /// Note a success: the job's schedule is dropped (only its own).
    /// Returns `true` when it had been failing.
    pub fn succeeded(&mut self, job: &Job) -> bool {
        self.entries.remove(job).is_some()
    }

    /// Forget a job (its subject went away).
    pub fn forget(&mut self, job: &Job) {
        self.entries.remove(job);
    }

    /// The jobs due by `now`, marked as running (not handed out again until
    /// they fail or succeed).
    pub fn take_due(&mut self, now: Instant) -> Vec<Job> {
        let mut due: Vec<(Instant, Job)> = Vec::new();
        for (job, e) in &mut self.entries {
            if let Some(at) = e.due.filter(|at| *at <= now) {
                e.due = None;
                due.push((at, job.clone()));
            }
        }
        due.sort_by_key(|d| d.0);
        due.into_iter().map(|d| d.1).collect()
    }

    /// When the next job is due.
    pub fn next_due(&self) -> Option<Instant> {
        self.entries.values().filter_map(|e| e.due).min()
    }

    /// Is this job waiting to be tried again (or being tried)?
    #[cfg(test)]
    pub fn contains(&self, job: &Job) -> bool {
        self.entries.contains_key(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let mut q = RetryQueue::default();
        let t = Instant::now();
        let waits: Vec<_> = (0..12).map(|_| q.failed(Job::ChatList, t)).collect();
        assert_eq!(waits[0], Duration::from_secs(2));
        assert_eq!(waits[1], Duration::from_secs(4));
        assert_eq!(waits[2], Duration::from_secs(8));
        assert_eq!(*waits.last().unwrap(), MAX_WAIT);
    }

    #[test]
    fn due_jobs_are_handed_out_once() {
        let mut q = RetryQueue::default();
        let t = Instant::now();
        q.failed(Job::Contacts, t);
        assert!(q.take_due(t).is_empty());
        let later = t + Duration::from_secs(3);
        assert_eq!(q.take_due(later), vec![Job::Contacts]);
        assert!(q.take_due(later).is_empty());
        assert_eq!(q.next_due(), None);
        // Still known (running) until it succeeds.
        assert!(q.contains(&Job::Contacts));
        assert!(q.succeeded(&Job::Contacts));
        assert!(!q.contains(&Job::Contacts));
    }

    #[test]
    fn items_keep_their_own_backoff_with_a_longer_cap() {
        let mut q = RetryQueue::default();
        let t = Instant::now();
        let thumb = Job::Thumb("m1".into());
        let waits: Vec<_> = (0..12).map(|_| q.failed(thumb.clone(), t)).collect();
        assert_eq!(waits[0], FIRST_WAIT);
        // Never given up: still scheduled, at most `ITEM_MAX_WAIT` apart.
        assert_eq!(*waits.last().unwrap(), ITEM_MAX_WAIT);
        assert_eq!(q.next_due(), Some(t + ITEM_MAX_WAIT));
    }

    #[test]
    fn a_success_moves_no_other_schedule() {
        let mut q = RetryQueue::default();
        let t = Instant::now();
        let (a, b) = (Job::Avatar("Ua".into()), Job::Avatar("Ub".into()));
        q.failed(a.clone(), t);
        q.failed(b.clone(), t);
        q.failed(b.clone(), t);
        q.failed(Job::Stream, t);
        // Unrelated success: nothing changes.
        assert!(!q.succeeded(&Job::Contacts));
        // One of them comes through: the others stay on their own times.
        assert!(q.succeeded(&a));
        assert!(q.take_due(t).is_empty());
        assert_eq!(q.take_due(t + FIRST_WAIT), vec![Job::Stream]);
        assert_eq!(q.take_due(t + FIRST_WAIT * 2), vec![b.clone()]);
        // And its own backoff carries on from where it was.
        assert_eq!(q.failed(b, t), FIRST_WAIT * 4);
    }
}
