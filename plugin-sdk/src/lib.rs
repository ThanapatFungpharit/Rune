//! The plugin API of `rune-tui`, shared by the host and by plugins.
//!
//! A plugin is a WebAssembly module (built for `wasm32-unknown-unknown`)
//! plus a `plugin.json` [manifest](manifest::Manifest). The host sends it
//! one [`Event`](proto::Event) at a time and the plugin answers with
//! [`Action`](proto::Action)s, both as JSON; the host checks every action
//! against the permissions the user granted. A plugin never calls into the
//! host except to log.
//!
//! Writing one in Rust:
//!
//! ```ignore
//! use rune_plugin::prelude::*;
//!
//! #[derive(Default)]
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn handle(&mut self, _cx: &Context, event: Event) -> Vec<Action> {
//!         match event {
//!             Event::Command { args, .. } => vec![Action::status(format!("hello, {args}!"))],
//!             _ => vec![],
//!         }
//!     }
//! }
//!
//! rune_plugin::export_plugin!(Hello);
//! ```
//!
//! See `docs/PLUGINS.md` for the whole contract (ABI, lifecycle, limits).

pub mod manifest;
pub mod proto;

pub use manifest::{ApiVersion, Manifest, Permission};
pub use proto::{Action, Event};

/// The API version this crate speaks. A plugin built against `M.m` loads
/// in a host speaking `M.n` with `n ≥ m`: minor versions only add things.
pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

/// What a plugin sees besides the event itself.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// Wall-clock time, milliseconds since the Unix epoch (wasm has no clock).
    pub now: i64,
}

/// A plugin: one value, kept alive between events while the plugin is active.
pub trait Plugin {
    fn handle(&mut self, cx: &Context, event: Event) -> Vec<Action>;
}

/// Everything a plugin usually needs.
pub mod prelude {
    pub use crate::proto::{
        Action, ChatKind, ChatRef, Event, FormField, KeyHelp, LineStyle, MessageInfo, ViewLine,
    };
    pub use crate::{Context, Plugin, log};
}

/// Logging to the host's diagnostics (`rune-tui --log PATH`). Messages are
/// capped at 4 KiB and a few hundred per event.
pub mod log {
    #[cfg(target_arch = "wasm32")]
    #[link(wasm_import_module = "rune")]
    unsafe extern "C" {
        #[link_name = "log"]
        fn host_log(level: i32, ptr: *const u8, len: usize);
    }

    fn write(level: i32, msg: &str) {
        #[cfg(target_arch = "wasm32")]
        // SAFETY: the host only reads `len` bytes at `ptr`, within our memory.
        unsafe {
            host_log(level, msg.as_ptr(), msg.len())
        };
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (level, msg);
    }

    pub fn error(msg: &str) {
        write(0, msg)
    }
    pub fn warn(msg: &str) {
        write(1, msg)
    }
    pub fn info(msg: &str) {
        write(2, msg)
    }
    pub fn debug(msg: &str) {
        write(3, msg)
    }
}

/// Decode one host message, run the plugin, encode its reply. The host
/// sends an [`proto::Envelope`]; a reply that cannot be built becomes a
/// status line rather than a failure.
pub fn dispatch<P: Plugin>(plugin: &mut P, input: &[u8]) -> Vec<u8> {
    let reply = match serde_json::from_slice::<proto::Envelope>(input) {
        Ok(env) => proto::Reply {
            actions: plugin.handle(&Context { now: env.now }, env.event),
        },
        Err(e) => {
            log::error(&format!("unreadable event: {e}"));
            proto::Reply::default()
        }
    };
    serde_json::to_vec(&reply).unwrap_or_else(|_| b"{\"actions\":[]}".to_vec())
}

/// The packed API version the `rune_api_version` export returns.
pub const fn packed_api_version() -> i32 {
    ((API_VERSION.major as i32) << 16) | API_VERSION.minor as i32
}

/// Export `$ty` (a [`Plugin`] + [`Default`]) as the plugin's entry points:
/// `rune_api_version`, `rune_alloc` and `rune_call` (see `docs/PLUGINS.md`).
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        #[cfg(target_arch = "wasm32")]
        mod __rune_plugin_exports {
            use super::*;
            use std::cell::RefCell;

            thread_local! {
                static PLUGIN: RefCell<Option<$ty>> = const { RefCell::new(None) };
                static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
                static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn rune_api_version() -> i32 {
                $crate::packed_api_version()
            }

            /// A buffer of `len` bytes for the next event; valid until the next call.
            #[unsafe(no_mangle)]
            pub extern "C" fn rune_alloc(len: i32) -> i32 {
                INPUT.with_borrow_mut(|b| {
                    b.clear();
                    b.resize(len.max(0) as usize, 0);
                    b.as_mut_ptr() as i32
                })
            }

            /// Handle the event in the input buffer; returns `ptr << 32 | len`
            /// of the reply, valid until the next call.
            #[unsafe(no_mangle)]
            pub extern "C" fn rune_call(ptr: i32, len: i32) -> i64 {
                let input = INPUT.with_borrow_mut(std::mem::take);
                debug_assert_eq!((input.as_ptr() as i32, input.len() as i32), (ptr, len));
                let out = PLUGIN.with_borrow_mut(|p| {
                    let plugin = p.get_or_insert_with(<$ty as Default>::default);
                    $crate::dispatch(plugin, &input)
                });
                OUTPUT.with_borrow_mut(|o| {
                    *o = out;
                    ((o.as_ptr() as u32 as i64) << 32) | o.len() as u32 as i64
                })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::*;

    #[derive(Default)]
    struct Echo(u32);

    impl Plugin for Echo {
        fn handle(&mut self, cx: &Context, event: Event) -> Vec<Action> {
            self.0 += 1;
            match event {
                Event::Command { args, .. } => vec![Action::status(format!("{} {args} {}", self.0, cx.now))],
                _ => vec![],
            }
        }
    }

    #[test]
    fn dispatch_round_trips_json() {
        let mut p = Echo::default();
        let input = br#"{"now":5,"event":{"type":"command","name":"echo","args":"hi","chat":null}}"#;
        let out: Reply = serde_json::from_slice(&dispatch(&mut p, input)).unwrap();
        assert_eq!(out.actions, vec![Action::status("1 hi 5")]);
        // Unknown events from a newer host are ignored, garbage is survived.
        let out: Reply =
            serde_json::from_slice(&dispatch(&mut p, br#"{"now":1,"event":{"type":"x_new"}}"#)).unwrap();
        assert!(out.actions.is_empty());
        let out: Reply = serde_json::from_slice(&dispatch(&mut p, b"nope")).unwrap();
        assert!(out.actions.is_empty());
        assert_eq!(p.0, 2);
    }

    #[test]
    fn packed_version() {
        assert_eq!(packed_api_version(), 1 << 16);
        assert_eq!(ApiVersion::from_packed(packed_api_version()), API_VERSION);
    }
}
