//! Copying text to the clipboard: through the desktop's clipboard program
//! when there is one (`wl-copy`, `xclip`, `xsel`, `pbcopy`), else through
//! the terminal (OSC 52, which also reaches the local clipboard over SSH
//! where the terminal allows it).
//!
//! Under `cfg(test)` nothing is copied; the text lands in [`COPIED`].

use std::io::Write;

use tokio::sync::mpsc;

use crate::AppEvent;

/// How the text went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Program(&'static str),
    /// OSC 52: the terminal may ignore it, and nothing says whether it did.
    #[cfg_attr(test, allow(dead_code))]
    Terminal,
}

#[cfg(test)]
thread_local! {
    pub static COPIED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Copy `text`. A clipboard program that starts but fails says so later,
/// as a status message through `tx`.
pub fn copy(text: &str, tx: &mpsc::UnboundedSender<AppEvent>) -> Via {
    #[cfg(test)]
    {
        let _ = tx;
        COPIED.with(|c| c.borrow_mut().push(text.to_owned()));
        Via::Program("test")
    }
    #[cfg(not(test))]
    {
        for (program, args) in programs() {
            if start(program, args, text, tx).is_ok() {
                return Via::Program(program);
            }
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(osc52(text).as_bytes());
        let _ = out.flush();
        Via::Terminal
    }
}

/// The clipboard programs worth trying here, best first.
#[cfg_attr(test, allow(dead_code))]
fn programs() -> Vec<(&'static str, &'static [&'static str])> {
    let mut found: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if cfg!(target_os = "macos") {
        found.push(("pbcopy", &[]));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        found.push(("wl-copy", &[]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        found.push(("xclip", &["-selection", "clipboard", "-in"]));
        found.push(("xsel", &["--clipboard", "--input"]));
    }
    found
}

/// Start `program` with `text` on its standard input (written, and the
/// program waited for, on a thread of its own).
#[cfg_attr(test, allow(dead_code))]
fn start(
    program: &'static str,
    args: &[&str],
    text: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> std::io::Result<()> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take();
    let (text, tx) = (text.to_owned(), tx.clone());
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            let written = input.as_mut().map(|i| i.write_all(text.as_bytes()));
            drop(input);
            let status = child.wait();
            let failed = match (written, status) {
                (Some(Err(e)), _) => Some(e.to_string()),
                (_, Err(e)) => Some(e.to_string()),
                (_, Ok(st)) if !st.success() => {
                    Some(st.code().map_or("a signal".into(), |c| format!("exit {c}")))
                }
                _ => None,
            };
            if let Some(why) = failed {
                let _ = tx.send(AppEvent::Status(format!(
                    "could not copy: {program} failed ({why})"
                )));
            }
        })?;
    Ok(())
}

/// The OSC 52 sequence that puts `text` on the clipboard.
fn osc52(text: &str) -> String {
    use base64::Engine;
    format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_gets_base64() {
        assert_eq!(osc52("hi ✓"), "\x1b]52;c;aGkg4pyT\x07");
    }

    #[test]
    fn programs_follow_the_session() {
        // Only what the environment offers is tried (none in a bare test run
        // is fine): never an empty program name.
        assert!(programs().iter().all(|(p, _)| !p.is_empty()));
    }
}
