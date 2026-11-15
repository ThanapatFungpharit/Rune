//! Typed views of operations from the event stream.
//!
//! [`Operation`] keeps the raw Thrift fields (`type`, `param1..3`); the
//! meaning of the params depends on the type. [`Operation::event`] decodes
//! the ones a chat client needs. The layouts were read off the live stream
//! (2026-11-14) unless noted; see `docs/PROTOCOL.md`.

use serde_json::Value;

use crate::models::{Message, Operation, ReactionKind, op_type, value_i64};

/// What an operation means for a chat client.
#[derive(Debug, Clone, PartialEq)]
pub enum OpEvent<'a> {
    /// A message was sent (by us, on any device) or received.
    Message(&'a Message),
    /// `reader` has read `chat` up to `message_id`. In 1:1 chats the server
    /// puts the reader's mid in `param1`, which is the chat from our side.
    Read {
        chat: &'a str,
        reader: &'a str,
        message_id: &'a str,
    },
    /// We read `chat` up to `message_id` (this or another device).
    ChatChecked { chat: &'a str, message_id: &'a str },
    /// A message was unsent (`by_me`: on one of our devices).
    Unsent {
        chat: &'a str,
        message_id: &'a str,
        by_me: bool,
    },
    /// `from` reacted to a message (`reaction: None` = took it back).
    Reaction {
        chat: Option<String>,
        message_id: &'a str,
        from: &'a str,
        reaction: Option<ReactionKind>,
    },
    /// `from` is typing in `chat` (op 62; layout inferred, never seen live).
    Typing { chat: &'a str, from: &'a str },
    /// A group was renamed / changed (`attribute`: [`crate::models::chat_attribute`]).
    ChatUpdated { chat: &'a str, attribute: Option<i64> },
    /// `member` joined (accepted an invitation to) `chat`.
    MemberJoined { chat: &'a str, member: &'a str },
    /// `member` left `chat` or was removed from it.
    MemberLeft { chat: &'a str, member: &'a str },
    /// We left `chat` (this or another device).
    LeftChat { chat: &'a str },
    /// Someone was invited into `chat` (possibly us; check the invitees).
    Invited { chat: &'a str },
    /// We answered an invitation on some device.
    InvitationAnswered { chat: &'a str, accepted: bool },
    /// A user's profile changed.
    ProfileUpdated { mid: &'a str },
    /// Anything else (see [`op_type`]).
    Other,
}

impl Operation {
    /// When the server recorded this operation (epoch ms, `createdTime`;
    /// sent as a string, live 2026-11-15). For op 55 that is when the read
    /// happened.
    pub fn created_time(&self) -> Option<i64> {
        self.extra
            .get("createdTime")
            .and_then(value_i64)
            .filter(|t| *t > 0)
    }

    /// Decode this operation (see [`OpEvent`]).
    pub fn event(&self) -> OpEvent<'_> {
        let p1 = self.param1.as_deref().filter(|s| !s.is_empty());
        let p2 = self.param2.as_deref().filter(|s| !s.is_empty());
        let p3 = self.param3.as_deref().filter(|s| !s.is_empty());
        match (self.r#type, p1, p2, p3) {
            (op_type::SEND_MESSAGE | op_type::RECEIVE_MESSAGE, ..) => match &self.message {
                Some(m) => OpEvent::Message(m),
                None => OpEvent::Other,
            },
            (op_type::NOTIFIED_READ_MESSAGE, Some(chat), Some(reader), Some(message_id)) => OpEvent::Read {
                chat,
                reader,
                message_id,
            },
            (op_type::SEND_CHAT_CHECKED, Some(chat), Some(message_id), _) => {
                OpEvent::ChatChecked { chat, message_id }
            }
            (t @ (op_type::DESTROY_MESSAGE | op_type::NOTIFIED_DESTROY_MESSAGE), Some(chat), Some(id), _) => {
                OpEvent::Unsent {
                    chat,
                    message_id: id,
                    by_me: t == op_type::DESTROY_MESSAGE,
                }
            }
            (op_type::NOTIFIED_SEND_REACTION, Some(message_id), meta, Some(from)) => {
                let meta: Value = meta
                    .and_then(|m| serde_json::from_str(m).ok())
                    .unwrap_or(Value::Null);
                OpEvent::Reaction {
                    chat: meta.get("chatMid").and_then(Value::as_str).map(str::to_owned),
                    message_id,
                    from,
                    reaction: meta.get("curr").and_then(ReactionKind::from_json),
                }
            }
            (op_type::NOTIFIED_TYPING, Some(chat), Some(from), _) => OpEvent::Typing { chat, from },
            (op_type::UPDATE_CHAT | op_type::NOTIFIED_UPDATE_CHAT, Some(chat), attr, _) => {
                OpEvent::ChatUpdated {
                    chat,
                    attribute: attr.and_then(|a| value_i64(&Value::String(a.to_owned()))),
                }
            }
            (
                op_type::NOTIFIED_ACCEPT_CHAT_INVITATION | op_type::NOTIFIED_JOIN_CHAT,
                Some(chat),
                Some(m),
                _,
            ) => OpEvent::MemberJoined { chat, member: m },
            (
                op_type::NOTIFIED_DELETE_SELF_FROM_CHAT | op_type::NOTIFIED_LEAVE_CHAT,
                Some(chat),
                Some(member),
                _,
            ) => OpEvent::MemberLeft { chat, member },
            // Kicks name the removed member(s) in param3 (reference layout).
            (op_type::NOTIFIED_DELETE_OTHER_FROM_CHAT, Some(chat), _, Some(member)) => {
                OpEvent::MemberLeft { chat, member }
            }
            (op_type::DELETE_SELF_FROM_CHAT, Some(chat), ..) => OpEvent::LeftChat { chat },
            (op_type::NOTIFIED_INVITE_INTO_CHAT, Some(chat), ..) => OpEvent::Invited { chat },
            (t @ (op_type::ACCEPT_CHAT_INVITATION | op_type::REJECT_CHAT_INVITATION), Some(chat), ..) => {
                OpEvent::InvitationAnswered {
                    chat,
                    accepted: t == op_type::ACCEPT_CHAT_INVITATION,
                }
            }
            (op_type::NOTIFIED_UPDATE_PROFILE, Some(mid), ..) => OpEvent::ProfileUpdated { mid },
            _ => OpEvent::Other,
        }
    }
}

/// Compare two message ids. LINE message ids are increasing 64-bit numbers
/// (sent as decimal strings), so they order messages within a chat.
pub fn message_id_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => (a.len(), a).cmp(&(b.len(), b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(t: i64, p1: Option<&str>, p2: Option<&str>, p3: Option<&str>) -> Operation {
        serde_json::from_value(json!({"revision": 1, "type": t, "param1": p1, "param2": p2, "param3": p3}))
            .unwrap()
    }

    #[test]
    fn live_layouts() {
        assert_eq!(
            op(55, Some("C1"), Some("U2"), Some("640000000000000001")).event(),
            OpEvent::Read {
                chat: "C1",
                reader: "U2",
                message_id: "640000000000000001"
            }
        );
        assert_eq!(
            op(40, Some("U9"), Some("7"), Some("0")).event(),
            OpEvent::ChatChecked {
                chat: "U9",
                message_id: "7"
            }
        );
        assert_eq!(
            op(65, Some("C1"), Some("7"), Some("0")).event(),
            OpEvent::Unsent {
                chat: "C1",
                message_id: "7",
                by_me: false
            }
        );
        assert!(matches!(
            op(64, Some("U1"), Some("7"), Some("0")).event(),
            OpEvent::Unsent { by_me: true, .. }
        ));
        let r = op(
            140,
            Some("7"),
            Some(r#"{"chatMid":"U0","curr":{"predefinedReactionType":7}}"#),
            Some("U0"),
        );
        assert_eq!(
            r.event(),
            OpEvent::Reaction {
                chat: Some("U0".into()),
                message_id: "7",
                from: "U0",
                reaction: Some(ReactionKind::Predefined(7))
            }
        );
        let cancel = op(140, Some("7"), Some(r#"{"chatMid":"C1"}"#), Some("U0"));
        assert!(matches!(cancel.event(), OpEvent::Reaction { reaction: None, .. }));
        assert_eq!(
            op(121, Some("C1"), Some("1"), None).event(),
            OpEvent::ChatUpdated {
                chat: "C1",
                attribute: Some(1)
            }
        );
        assert_eq!(
            op(128, Some("C1"), Some("U2"), None).event(),
            OpEvent::MemberLeft {
                chat: "C1",
                member: "U2"
            }
        );
        assert_eq!(
            op(131, Some("C1"), None, None).event(),
            OpEvent::InvitationAnswered {
                chat: "C1",
                accepted: false
            }
        );
        assert_eq!(op(48, None, None, None).event(), OpEvent::Other);
        // Missing params never panic.
        assert_eq!(op(55, Some("C1"), None, None).event(), OpEvent::Other);
    }

    #[test]
    fn ids_order_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(message_id_cmp("99", "100"), Less);
        assert_eq!(message_id_cmp("640442637832946311", "640442637832946311"), Equal);
        assert_eq!(message_id_cmp("b", "a"), Greater);
    }
}
