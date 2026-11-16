# crossterm 0.29.0, patched for rune

An unmodified copy of crossterm 0.29.0 from crates.io (MIT, see `LICENSE`)
with one change, marked `rune patch` in `src/event/sys/unix/parse.rs`:
`parse_csi_special_key_code` reads xterm's modifyOtherKeys sequences,
`ESC [ 27 ; <modifiers> ; <code point> ~`, as the key with its modifiers
(crossterm otherwise drops them). `rune-tui` turns modifyOtherKeys on
(`CSI > 4 ; 1 m`) when the terminal lacks the kitty keyboard protocol;
WezTerm (with `enable_kitty_keyboard` off), xterm and tmux (with
`extended-keys on`) then send
Shift-Enter as `ESC [ 27 ; 2 ; 13 ~` instead of a plain `\r`.

Also one clippy fix (needless parentheses in `src/terminal/sys/unix.rs`), as
clippy lints path dependencies.

Drop this copy (and the `[patch.crates-io]` entry in the root
`Cargo.toml`) once crossterm parses these sequences itself.
