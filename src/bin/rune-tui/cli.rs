//! Command-line options.
//!
//! Unknown options are refused rather than ignored: a mistyped
//! `--no-read-receipts` must not quietly send read receipts.

use std::path::PathBuf;

use crate::cells::CellMode;

pub const USAGE: &str = "\
Usage: rune-tui [OPTIONS] [SESSION]

A terminal chat client for LINE. SESSION is the saved login (default:
session.json); when it does not exist, rune-tui logs in by QR code and
writes it (mode 0600). It holds your tokens and Letter Sealing keys.

Options:
  --no-read-receipts  never mark chats as read (others see no read receipt)
  --no-graphics       draw pictures with text instead of the terminal's
                      image protocol (Kitty, Sixel, iTerm2)
  --cells MODE        glyphs for text pictures: quadrant (default),
                      sextant or half
  --no-mouse          leave the mouse to the terminal (text selection)
  --db PATH           local message store (default: SESSION with .db)
  --no-store          keep nothing on disk (in-memory store)
  --log PATH          append diagnostics to PATH (mode 0600; RUST_LOG sets
                      the level, default info); never tokens or messages
  --plugins DIR       where plugins are installed (default:
                      $XDG_DATA_HOME/rune/plugins, else ~/.local/share/rune/plugins)
  --no-plugins        start no plugin this run (they stay enabled)
  --default-keys      use the default keyboard shortcuts this run (yours
                      stay saved; change them in Settings › Shortcuts)
  -h, --help          show this help
  -V, --version       show the version
";

/// What to run with.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub session: String,
    pub read_receipts: bool,
    pub graphics: bool,
    pub mouse: bool,
    pub cells: CellMode,
    pub db: Option<PathBuf>,
    pub no_store: bool,
    pub log: Option<PathBuf>,
    pub plugins: Option<PathBuf>,
    pub no_plugins: bool,
    pub default_keys: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            session: "session.json".into(),
            read_receipts: true,
            graphics: true,
            mouse: true,
            cells: CellMode::Quadrant,
            db: None,
            no_store: false,
            log: None,
            plugins: None,
            no_plugins: false,
            default_keys: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Options),
    Help,
    Version,
}

/// Parse the arguments after the program name. `--opt VALUE` and
/// `--opt=VALUE` both work; `--` ends the options.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut o = Options::default();
    let mut session: Option<String> = None;
    let mut args = args.into_iter();
    let mut options_done = false;
    while let Some(arg) = args.next() {
        if options_done || !arg.starts_with('-') || arg == "-" {
            if session.replace(arg.clone()).is_some() {
                return Err(format!(
                    "unexpected argument {arg:?}: only one session path is taken"
                ));
            }
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs {what}"))
        };
        let flag = |inline: &Option<String>| match inline {
            Some(_) => Err(format!("{name} takes no value")),
            None => Ok(()),
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--" => options_done = true,
            "--no-read-receipts" => {
                flag(&inline)?;
                o.read_receipts = false;
            }
            "--no-graphics" => {
                flag(&inline)?;
                o.graphics = false;
            }
            "--no-mouse" => {
                flag(&inline)?;
                o.mouse = false;
            }
            "--no-store" => {
                flag(&inline)?;
                o.no_store = true;
            }
            "--cells" => {
                let v = value("a mode")?;
                o.cells = CellMode::parse(&v)
                    .ok_or_else(|| format!("--cells: expected half, quadrant or sextant, got {v:?}"))?;
            }
            "--db" => o.db = Some(PathBuf::from(value("a path")?)),
            "--log" => o.log = Some(PathBuf::from(value("a path")?)),
            "--plugins" => o.plugins = Some(PathBuf::from(value("a directory")?)),
            "--no-plugins" => {
                flag(&inline)?;
                o.no_plugins = true;
            }
            "--default-keys" => {
                flag(&inline)?;
                o.default_keys = true;
            }
            _ => return Err(format!("unknown option {name} (see --help)")),
        }
    }
    if o.no_plugins && o.plugins.is_some() {
        return Err("--plugins and --no-plugins contradict each other".into());
    }
    if o.no_store && o.db.is_some() {
        return Err("--db and --no-store contradict each other".into());
    }
    if let Some(s) = session {
        o.session = s;
    }
    Ok(Command::Run(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Options, String> {
        match parse(args.iter().map(|s| s.to_string()))? {
            Command::Run(o) => Ok(o),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn defaults_and_every_option() {
        assert_eq!(run(&[]).unwrap(), Options::default());
        let o = run(&[
            "--no-read-receipts",
            "--no-graphics",
            "--no-mouse",
            "--cells",
            "sextant",
            "--db=/tmp/x.db",
            "--log",
            "/tmp/rune.log",
            "--plugins=/tmp/plugins",
            "me.json",
        ])
        .unwrap();
        assert_eq!(o.plugins, Some(PathBuf::from("/tmp/plugins")));
        assert!(run(&["--no-plugins"]).unwrap().no_plugins);
        assert!(run(&["--default-keys"]).unwrap().default_keys);
        assert!(!o.read_receipts && !o.graphics && !o.mouse);
        assert_eq!(o.cells, CellMode::Sextant);
        assert_eq!(o.db, Some(PathBuf::from("/tmp/x.db")));
        assert_eq!(o.log, Some(PathBuf::from("/tmp/rune.log")));
        assert_eq!(o.session, "me.json");
        // A value that looks like the session path is the option's value.
        assert_eq!(run(&["--db", "a.json"]).unwrap().session, "session.json");
    }

    #[test]
    fn mistakes_are_refused_not_ignored() {
        assert!(
            run(&["--no-read-reciepts"])
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(run(&["--cells"]).unwrap_err().contains("needs"));
        assert!(run(&["--cells", "dots"]).is_err());
        assert!(run(&["--no-mouse=yes"]).unwrap_err().contains("no value"));
        assert!(run(&["a.json", "b.json"]).unwrap_err().contains("only one"));
        assert!(run(&["--no-store", "--db", "x.db"]).is_err());
        assert!(run(&["--no-plugins", "--plugins", "d"]).is_err());
        assert!(run(&["--no-plugins=1"]).is_err());
        assert!(run(&["--default-keys=no"]).is_err());
    }

    #[test]
    fn help_version_and_double_dash() {
        let p = |a: &[&str]| parse(a.iter().map(|s| s.to_string())).unwrap();
        assert_eq!(p(&["-h"]), Command::Help);
        assert_eq!(p(&["--no-mouse", "--help"]), Command::Help);
        assert_eq!(p(&["--version"]), Command::Version);
        assert_eq!(
            run(&["--", "--odd-name.json"]).unwrap().session,
            "--odd-name.json"
        );
    }
}
