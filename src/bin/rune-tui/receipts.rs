//! Read receipts that have not been acknowledged yet.
//!
//! `sendChatChecked` is a watermark ("read up to this message"), so one
//! position per chat is enough: a receipt for a newer message covers every
//! older one. Each chat keeps two positions:
//!
//! - `approved`: read with receipts on. Sent as soon as possible and retried
//!   (with backoff) until the server acknowledges it, whichever chat is open.
//! - `held`: read with receipts off. Never sent by itself: it becomes
//!   `approved` only when receipts are on *and* the user is in that chat
//!   (turning receipts back on there, coming back to it, opening it again).
//!
//! Positions are only ever raised, and cleared only by an acknowledgement
//! covering them: our own request succeeding, or the stream reporting that
//! the chat was read that far (this or another device). One request per chat
//! is in flight at a time; a newer position read meanwhile goes out after it.
//! Every request carries a ticket, so the result of one cancelled by turning
//! receipts off (or by leaving the chat) changes nothing but what it
//! acknowledged.
//!
//! This is pure state; `App` persists [`Receipts::row`] after every change,
//! performs the [`Send`]s and reports back with [`Receipts::finished`].

use std::cmp::Ordering;
use std::collections::HashMap;

use rune::ops::message_id_cmp;

/// A request to make: mark `chat` read up to `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Send {
    pub chat: String,
    pub id: String,
    pub ticket: u64,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The server acknowledged it.
    Acked,
    /// Network trouble, a server hiccup, a token being refreshed: try again later.
    Retry,
    /// The server refused this position for good (e.g. the message is gone):
    /// drop it, keep anything newer.
    Refused,
}

/// What [`Receipts::finished`] wants done next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Nothing (the request was stale, or everything is acknowledged).
    Nothing,
    /// Back off before trying this chat again (the caller schedules it).
    BackOff,
    /// More is pending for this chat: send it now.
    Flush,
}

#[derive(Debug, Default)]
struct Entry {
    approved: Option<String>,
    held: Option<String>,
    /// The request in flight: (position, ticket).
    flight: Option<(String, u64)>,
    /// Waiting out a backoff after a failure.
    backing_off: bool,
}

impl Entry {
    fn is_empty(&self) -> bool {
        self.approved.is_none() && self.held.is_none() && self.flight.is_none()
    }
}

/// `slot` = the newer of `slot` and `id`. True when it changed.
fn raise(slot: &mut Option<String>, id: &str) -> bool {
    if slot
        .as_deref()
        .is_some_and(|cur| message_id_cmp(id, cur) != Ordering::Greater)
    {
        return false;
    }
    *slot = Some(id.to_owned());
    true
}

/// Clear `slot` if `id` covers it. True when it changed.
fn cover(slot: &mut Option<String>, id: &str) -> bool {
    if slot
        .as_deref()
        .is_some_and(|cur| message_id_cmp(cur, id) != Ordering::Greater)
    {
        *slot = None;
        return true;
    }
    false
}

#[derive(Debug, Default)]
pub struct Receipts {
    entries: HashMap<String, Entry>,
    /// Newest acknowledged position per chat this run, so a position already
    /// acknowledged is never queued (or sent) again.
    acked: HashMap<String, String>,
    next_ticket: u64,
}

impl Receipts {
    /// Rebuild from stored rows: (chat, approved, held).
    pub fn load(rows: impl IntoIterator<Item = (String, Option<String>, Option<String>)>) -> Receipts {
        let mut r = Receipts::default();
        for (chat, approved, held) in rows {
            let e = Entry {
                approved,
                held,
                ..Entry::default()
            };
            if !e.is_empty() {
                r.entries.insert(chat, e);
            }
        }
        r
    }

    fn already_acked(&self, chat: &str, id: &str) -> bool {
        self.acked
            .get(chat)
            .is_some_and(|a| message_id_cmp(id, a) != Ordering::Greater)
    }

    /// The user read `chat` up to `id` while in it. With receipts on
    /// (`enabled`) that is approved together with anything held there;
    /// otherwise it is held. True when something changed (persist the row).
    pub fn read(&mut self, chat: &str, id: &str, enabled: bool) -> bool {
        if self.already_acked(chat, id) {
            return enabled && self.promote(chat);
        }
        let e = self.entries.entry(chat.to_owned()).or_default();
        let changed = if enabled {
            raise(&mut e.approved, id)
        } else {
            raise(&mut e.held, id)
        };
        let promoted = enabled && self.promote(chat);
        self.tidy(chat);
        changed || promoted
    }

    /// Receipts are on and the user is in `chat`: what was held there is
    /// approved. True when something changed.
    pub fn promote(&mut self, chat: &str) -> bool {
        let Some(e) = self.entries.get_mut(chat) else {
            return false;
        };
        let Some(held) = e.held.take() else {
            return false;
        };
        raise(&mut e.approved, &held);
        true
    }

    /// The requests to start now. Nothing while receipts are off: held
    /// positions are never sent, and approved ones wait for receipts to be
    /// turned back on.
    pub fn take_due(&mut self, enabled: bool) -> Vec<Send> {
        if !enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (chat, e) in &mut self.entries {
            if e.flight.is_some() || e.backing_off {
                continue;
            }
            if let Some(id) = &e.approved {
                self.next_ticket += 1;
                e.flight = Some((id.clone(), self.next_ticket));
                out.push(Send {
                    chat: chat.clone(),
                    id: id.clone(),
                    ticket: self.next_ticket,
                });
            }
        }
        out.sort_by(|a, b| a.chat.cmp(&b.chat));
        out
    }

    /// A request ended. Returns whether the row changed and what to do next.
    pub fn finished(&mut self, s: &Send, outcome: Outcome) -> (bool, Next) {
        let current = self
            .entries
            .get(&s.chat)
            .and_then(|e| e.flight.as_ref())
            .is_some_and(|(_, t)| *t == s.ticket);
        if current && let Some(e) = self.entries.get_mut(&s.chat) {
            e.flight = None;
        }
        let changed = match outcome {
            // An acknowledgement counts even for a cancelled request: the
            // server has it.
            Outcome::Acked => self.acked(&s.chat, &s.id),
            Outcome::Refused if current => self.entries.get_mut(&s.chat).is_some_and(|e| {
                let a = e.approved.as_deref() == Some(s.id.as_str()) && e.approved.take().is_some();
                let h = e.held.as_deref() == Some(s.id.as_str()) && e.held.take().is_some();
                a || h
            }),
            _ => false,
        };
        let next = if !current {
            Next::Nothing
        } else if outcome == Outcome::Retry {
            if let Some(e) = self.entries.get_mut(&s.chat) {
                e.backing_off = true;
            }
            Next::BackOff
        } else if self.entries.get(&s.chat).is_some_and(|e| e.approved.is_some()) {
            Next::Flush
        } else {
            Next::Nothing
        };
        self.tidy(&s.chat);
        (changed, next)
    }

    /// The server has `chat` read up to `id` (our request, or the stream
    /// reporting a read on this or another device): everything it covers
    /// is done. True when the row changed.
    pub fn acked(&mut self, chat: &str, id: &str) -> bool {
        let newest = self.acked.entry(chat.to_owned()).or_insert_with(|| id.to_owned());
        if message_id_cmp(id, newest) == Ordering::Greater {
            *newest = id.to_owned();
        }
        let changed = self
            .entries
            .get_mut(chat)
            .is_some_and(|e| cover(&mut e.approved, id) | cover(&mut e.held, id));
        self.tidy(chat);
        changed
    }

    /// A backoff ran out: `chat` may be sent again.
    pub fn ready(&mut self, chat: &str) {
        if let Some(e) = self.entries.get_mut(chat) {
            e.backing_off = false;
        }
    }

    /// Is `chat` waiting out a backoff?
    pub fn backing_off(&self, chat: &str) -> bool {
        self.entries.get(chat).is_some_and(|e| e.backing_off)
    }

    /// Receipts were turned off: requests in flight are cancelled (their
    /// tickets are returned to abort them); their positions stay pending.
    pub fn disable(&mut self) -> Vec<u64> {
        let tickets = self
            .entries
            .values_mut()
            .filter_map(|e| e.flight.take().map(|(_, t)| t))
            .collect();
        self.entries.retain(|_, e| !e.is_empty());
        tickets
    }

    /// Message `id` of `chat` was unsent. A position on it moves back to
    /// `fallback` (the newest earlier message still there), or is dropped.
    /// True when the row changed.
    pub fn unsent(&mut self, chat: &str, id: &str, fallback: Option<&str>) -> bool {
        let Some(e) = self.entries.get_mut(chat) else {
            return false;
        };
        let mut changed = false;
        for slot in [&mut e.approved, &mut e.held] {
            if slot.as_deref() == Some(id) {
                *slot = fallback
                    .filter(|f| message_id_cmp(f, id) == Ordering::Less)
                    .map(str::to_owned);
                changed = true;
            }
        }
        self.tidy(chat);
        changed
    }

    /// The chat is gone (left, declined, data cleared). Returns the ticket
    /// of its request in flight, to abort.
    pub fn forget(&mut self, chat: &str) -> Option<u64> {
        self.acked.remove(chat);
        self.entries.remove(chat).and_then(|e| e.flight.map(|(_, t)| t))
    }

    /// Forget everything (local data cleared). Returns the tickets to abort.
    pub fn clear(&mut self) -> Vec<u64> {
        let tickets = self
            .entries
            .values()
            .filter_map(|e| e.flight.as_ref().map(|f| f.1))
            .collect();
        *self = Receipts {
            next_ticket: self.next_ticket,
            ..Receipts::default()
        };
        tickets
    }

    /// What to store for `chat`: (approved, held); both `None` = no row.
    pub fn row(&self, chat: &str) -> (Option<&str>, Option<&str>) {
        self.entries
            .get(chat)
            .map_or((None, None), |e| (e.approved.as_deref(), e.held.as_deref()))
    }

    /// Chats with anything pending.
    #[cfg(test)]
    pub fn chats(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        v.sort();
        v
    }

    fn tidy(&mut self, chat: &str) {
        if self.entries.get(chat).is_some_and(Entry::is_empty) {
            self.entries.remove(chat);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(sends: &[Send]) -> Vec<(&str, &str)> {
        sends.iter().map(|s| (s.chat.as_str(), s.id.as_str())).collect()
    }

    #[test]
    fn nothing_is_sent_while_receipts_are_off() {
        let mut r = Receipts::default();
        assert!(r.read("Ca", "10", false));
        assert!(r.take_due(false).is_empty());
        // Turned on elsewhere: held positions still wait for the chat.
        assert!(r.take_due(true).is_empty());
        assert_eq!(r.row("Ca"), (None, Some("10")));
        // Approved ones wait for receipts to be on too.
        r.read("Cb", "5", true);
        assert!(r.take_due(false).is_empty());
        assert_eq!(ids(&r.take_due(true)), vec![("Cb", "5")]);
    }

    #[test]
    fn being_in_the_chat_with_receipts_on_sends_what_was_held() {
        let mut r = Receipts::default();
        r.read("Ca", "10", false);
        r.read("Ca", "12", false);
        r.read("Ca", "11", false);
        assert_eq!(r.row("Ca"), (None, Some("12")), "positions only rise");
        assert!(r.promote("Ca"));
        assert!(!r.promote("Ca"));
        let s = r.take_due(true);
        assert_eq!(ids(&s), vec![("Ca", "12")]);
        assert_eq!(r.finished(&s[0], Outcome::Acked), (true, Next::Nothing));
        assert!(r.chats().is_empty(), "cleared once acknowledged");
    }

    #[test]
    fn one_request_per_chat_and_a_newer_read_follows_it() {
        let mut r = Receipts::default();
        r.read("Ca", "10", true);
        let first = r.take_due(true);
        r.read("Ca", "20", true);
        assert!(
            r.take_due(true).is_empty(),
            "no second request while one is in flight"
        );
        assert_eq!(r.finished(&first[0], Outcome::Acked), (false, Next::Flush));
        let second = r.take_due(true);
        assert_eq!(ids(&second), vec![("Ca", "20")]);
        r.finished(&second[0], Outcome::Acked);
        // Re-reading an acknowledged position queues nothing (no duplicate).
        assert!(!r.read("Ca", "20", true));
        assert!(!r.read("Ca", "15", false));
        assert!(r.take_due(true).is_empty());
    }

    #[test]
    fn failures_back_off_and_keep_the_position() {
        let mut r = Receipts::default();
        r.read("Ca", "10", true);
        let s = r.take_due(true);
        assert_eq!(r.finished(&s[0], Outcome::Retry), (false, Next::BackOff));
        assert!(r.backing_off("Ca"));
        r.read("Ca", "11", true);
        assert!(r.take_due(true).is_empty(), "waits out its backoff");
        assert_eq!(r.row("Ca"), (Some("11"), None));
        r.ready("Ca");
        let s = r.take_due(true);
        assert_eq!(ids(&s), vec![("Ca", "11")]);
        r.finished(&s[0], Outcome::Acked);
        assert!(r.chats().is_empty());
    }

    #[test]
    fn a_refused_position_is_dropped_but_not_a_newer_one() {
        let mut r = Receipts::default();
        r.read("Ca", "10", true);
        let s = r.take_due(true);
        assert_eq!(r.finished(&s[0], Outcome::Refused), (true, Next::Nothing));
        assert!(r.chats().is_empty());

        r.read("Cb", "10", true);
        let s = r.take_due(true);
        r.read("Cb", "11", true);
        assert_eq!(r.finished(&s[0], Outcome::Refused), (false, Next::Flush));
        assert_eq!(ids(&r.take_due(true)), vec![("Cb", "11")]);
    }

    #[test]
    fn turning_off_cancels_requests_and_their_late_results_change_nothing() {
        let mut r = Receipts::default();
        r.read("Ca", "10", true);
        let s = r.take_due(true);
        assert_eq!(r.disable(), vec![s[0].ticket]);
        assert_eq!(r.row("Ca"), (Some("10"), None), "still pending");
        // The cancelled request's failure arrives late: no backoff, no change.
        assert_eq!(r.finished(&s[0], Outcome::Retry), (false, Next::Nothing));
        assert!(!r.backing_off("Ca"));
        // Back on: sent again under a new ticket.
        let again = r.take_due(true);
        assert_eq!(ids(&again), vec![("Ca", "10")]);
        assert_ne!(again[0].ticket, s[0].ticket);
        // A cancelled request that did reach the server still counts.
        assert_eq!(r.finished(&s[0], Outcome::Acked), (true, Next::Nothing));
        assert_eq!(r.finished(&again[0], Outcome::Acked), (false, Next::Nothing));
        assert!(r.chats().is_empty());
    }

    #[test]
    fn rapid_toggling_never_has_two_requests_for_a_chat() {
        let mut r = Receipts::default();
        r.read("Ca", "10", true);
        let mut tickets = Vec::new();
        for _ in 0..5 {
            let s = r.take_due(true);
            assert_eq!(s.len(), 1);
            assert!(r.take_due(true).is_empty());
            tickets.push(s[0].clone());
            r.disable();
            assert!(r.take_due(false).is_empty());
        }
        // Results of all the cancelled ones: only acknowledgements count.
        for s in &tickets[..4] {
            r.finished(s, Outcome::Retry);
        }
        assert_eq!(r.row("Ca"), (Some("10"), None));
        r.finished(&tickets[4], Outcome::Acked);
        assert!(r.chats().is_empty());
    }

    #[test]
    fn a_read_reported_by_the_stream_covers_pending_positions() {
        let mut r = Receipts::default();
        r.read("Ca", "10", false);
        r.read("Cb", "10", true);
        let s = r.take_due(true);
        // Another device read both chats further.
        assert!(r.acked("Ca", "12"));
        assert!(r.acked("Cb", "10"));
        assert!(r.chats().contains(&"Cb"), "its request is still in flight");
        assert_eq!(r.finished(&s[0], Outcome::Acked), (false, Next::Nothing));
        assert!(r.chats().is_empty());
        // An older report covers nothing newer.
        r.read("Cc", "20", true);
        assert!(!r.acked("Cc", "19"));
        assert_eq!(r.row("Cc"), (Some("20"), None));
    }

    #[test]
    fn unsent_messages_move_the_position_back_or_drop_it() {
        let mut r = Receipts::default();
        r.read("Ca", "10", false);
        r.read("Cb", "20", true);
        assert!(r.unsent("Ca", "10", Some("9")));
        assert_eq!(r.row("Ca"), (None, Some("9")));
        assert!(!r.unsent("Ca", "5", Some("4")), "not a pending position");
        assert!(r.unsent("Cb", "20", None));
        assert!(r.chats() == vec!["Ca"]);
        // A "fallback" that is not older is ignored.
        assert!(r.unsent("Ca", "9", Some("30")));
        assert!(r.chats().is_empty());
    }

    #[test]
    fn forgetting_a_chat_returns_its_request_to_abort() {
        let mut r = Receipts::load([
            ("Ca".into(), Some("10".into()), None),
            ("Cb".into(), None, Some("3".into())),
            ("Cc".into(), None, None),
        ]);
        assert_eq!(r.chats(), vec!["Ca", "Cb"]);
        let s = r.take_due(true);
        assert_eq!(ids(&s), vec![("Ca", "10")]);
        assert_eq!(r.forget("Ca"), Some(s[0].ticket));
        assert_eq!(r.forget("Cb"), None);
        assert!(r.chats().is_empty());
        // Its late result does not bring it back.
        assert_eq!(r.finished(&s[0], Outcome::Retry), (false, Next::Nothing));
        assert!(r.chats().is_empty());
    }

    #[test]
    fn ids_compare_numerically() {
        let mut r = Receipts::default();
        r.read("Ca", "999", true);
        assert!(r.read("Ca", "1000", true));
        assert_eq!(r.row("Ca"), (Some("1000"), None));
    }
}
