//! A text field with a cursor: the message line and form fields.
//! The cursor moves by character, stepping over zero-width ones (Thai vowel
//! and tone marks, emoji modifiers) so it never lands inside a cluster.
//!
//! Form fields show one scrolling line ([`TextInput::view`]); the message
//! line may hold several lines, soft-wrapped at word boundaries
//! ([`TextInput::rows`], [`TextInput::view_rows`]), and its cursor moves
//! between the wrapped rows ([`TextInput::move_row`]).

use crossterm::event::KeyEvent;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keymap::Cmd;
use crate::typed_char;

/// What [`TextInput::edit`] did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key: the caller handles it.
    Ignored,
    /// The cursor moved, or the key did nothing away from the ends.
    Moved,
    /// A leftward key did nothing because the cursor already was at the
    /// start (`←` at the start): callers may leave the field.
    MovedHitLeft,
    /// A rightward key did nothing because the cursor already was at the end.
    MovedHitRight,
    /// The text changed: filters depending on it must update.
    Changed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset, always on a character boundary.
    cursor: usize,
    /// The column ↑/↓ aim for, and the cursor it was set at: moving through
    /// a short row keeps the column for the next one. Stale once the cursor
    /// moves otherwise.
    goal: Option<(usize, usize)>,
}

fn zero_width(c: char) -> bool {
    c.width() == Some(0)
}

impl TextInput {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Apply a text-editing key: the cursor and delete commands of the
    /// shared `TEXT_EDIT` bindings, or (`cmd` is `None`) a typed character.
    /// Every text field edits through this, so they all behave alike.
    pub fn edit(&mut self, cmd: Option<Cmd>, k: &KeyEvent) -> Edit {
        let (len, cursor) = (self.text.len(), self.cursor);
        match cmd {
            Some(Cmd::CursorLeft) => self.left(),
            Some(Cmd::CursorRight) => self.right(),
            Some(Cmd::LineStart) => self.home(),
            Some(Cmd::LineEnd) => self.end(),
            Some(Cmd::DeleteBackward) => self.backspace(),
            Some(Cmd::DeleteForward) => self.delete(),
            Some(Cmd::WordLeft) => self.cursor = self.word_start(),
            Some(Cmd::WordRight) => self.cursor = self.word_end(),
            Some(Cmd::DeleteWordBackward) => {
                let start = self.word_start();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            Some(Cmd::DeleteToLineStart) => {
                let end = self.cursor;
                self.home();
                self.text.drain(self.cursor..end);
            }
            Some(Cmd::DeleteToLineEnd) => {
                let start = self.cursor;
                self.end();
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            None => match typed_char(k) {
                Some(c) => self.insert(c),
                None => return Edit::Ignored,
            },
            Some(_) => return Edit::Ignored,
        }
        if self.text.len() != len {
            return Edit::Changed;
        }
        // A boundary is hit only by a key that could not move the cursor
        // toward it: arriving at an end is an ordinary move.
        let leftward = matches!(
            cmd,
            Some(
                Cmd::CursorLeft
                    | Cmd::LineStart
                    | Cmd::WordLeft
                    | Cmd::DeleteBackward
                    | Cmd::DeleteWordBackward
                    | Cmd::DeleteToLineStart
            )
        );
        match self.cursor {
            c if c != cursor => Edit::Moved,
            0 if leftward => Edit::MovedHitLeft,
            c if c == len && !leftward => Edit::MovedHitRight,
            _ => Edit::Moved,
        }
    }

    /// Replace the text; the cursor goes to the end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    /// Delete the character after the cursor (and the marks on it).
    pub fn delete(&mut self) {
        let end = self.next_stop(self.cursor);
        self.text.drain(self.cursor..end);
    }

    pub fn left(&mut self) {
        for (i, c) in self.text[..self.cursor].char_indices().rev() {
            self.cursor = i;
            if !zero_width(c) {
                break;
            }
        }
    }

    pub fn right(&mut self) {
        self.cursor = self.next_stop(self.cursor);
    }

    /// Where the word before the cursor starts: back over spaces, then over
    /// everything up to the previous space (as shells do).
    fn word_start(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        trimmed
            .rfind(char::is_whitespace)
            .map_or(0, |i| i + trimmed[i..].chars().next().map_or(1, char::len_utf8))
    }

    /// Where the word after the cursor ends: over spaces, then to the next space.
    fn word_end(&self) -> usize {
        let after = &self.text[self.cursor..];
        let skipped = after.len() - after.trim_start().len();
        let rest = &after[skipped..];
        self.cursor + skipped + rest.find(char::is_whitespace).unwrap_or(rest.len())
    }

    /// Start of the cursor's line (the whole text when it has one line).
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    /// End of the cursor's line.
    pub fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
    }

    /// Lines in the text (an empty text has one).
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// Visual rows of the text wrapped at `width` columns (one is kept for
    /// the cursor), as byte ranges without the line breaks. Rows break after
    /// the last space that fits, or mid-word when a word is wider than the
    /// row; marks stay with their character. `width` 0 does not wrap.
    pub fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let room = if width == 0 {
            usize::MAX
        } else {
            width.saturating_sub(1).max(1)
        };
        let mut rows = Vec::new();
        let mut line_start = 0;
        for line in self.text.split('\n') {
            let mut start = line_start;
            let mut used = 0;
            // Just after the row's last space.
            let mut brk = None;
            for (i, c) in line.char_indices() {
                let at = line_start + i;
                let w = c.width().unwrap_or(0);
                if used + w > room && at > start {
                    // After the last space; else here, but never between a
                    // letter and its SARA AM.
                    let letter = || {
                        crate::letter_start(&self.text[start..at])
                            .map(|i| start + i)
                            .filter(|&i| i > start && crate::joins_previous(c))
                    };
                    let cut = brk.filter(|&b| b > start).or_else(letter).unwrap_or(at);
                    rows.push((start, cut));
                    start = cut;
                    used = self.text[start..at].width();
                    brk = None;
                }
                used += w;
                if c == ' ' {
                    brk = Some(at + 1);
                }
            }
            let end = line_start + line.len();
            rows.push((start, end));
            line_start = end + 1;
        }
        rows
    }

    /// The cursor's row in `rows` and its column there. At a soft wrap the
    /// cursor shows at the start of the next row.
    fn row_col(&self, rows: &[(usize, usize)]) -> (usize, usize) {
        let r = rows.iter().rposition(|&(s, _)| s <= self.cursor).unwrap_or(0);
        let (s, e) = rows[r];
        (r, self.text[s..self.cursor.min(e)].width())
    }

    /// Move the cursor `delta` wrapped rows up (negative) or down, keeping
    /// its column as well as the row allows. False (and nothing moves) when
    /// there is no such row: the caller may then use the key otherwise.
    pub fn move_row(&mut self, delta: isize, width: usize) -> bool {
        let rows = self.rows(width);
        let (r, col) = self.row_col(&rows);
        let col = match self.goal {
            Some((at, goal)) if at == self.cursor => goal,
            _ => col,
        };
        let Some(t) = r.checked_add_signed(delta).filter(|&t| t < rows.len()) else {
            return false;
        };
        let (s, e) = rows[t];
        let mut at = s;
        let mut used = 0;
        for (i, c) in self.text[s..e].char_indices() {
            let w = c.width().unwrap_or(0);
            if w > 0 && used + w > col {
                break;
            }
            used += w;
            at = s + i + c.len_utf8();
        }
        // The end of a soft-wrapped row shows on the next one: stay before it.
        if at == e && rows.get(t + 1).is_some_and(|&(next, _)| next == e) && at > s {
            self.cursor = at;
            self.left();
        } else {
            self.cursor = at;
        }
        self.goal = Some((self.cursor, col));
        true
    }

    /// The wrapped rows that fit in `height` rows of `width` columns, and the
    /// cursor's (column, row) among them. The view shows the end of the text
    /// unless the cursor is above it, then starts at the cursor's row.
    pub fn view_rows(&self, width: usize, height: usize) -> (Vec<&str>, (u16, u16)) {
        let rows = self.rows(width);
        let (r, col) = self.row_col(&rows);
        let height = height.max(1);
        let first = r.min(rows.len().saturating_sub(height));
        let shown = rows[first..]
            .iter()
            .take(height)
            .map(|&(s, e)| &self.text[s..e])
            .collect();
        (shown, (col as u16, (r - first) as u16))
    }

    /// Where the cursor lands after stepping right from `at`.
    fn next_stop(&self, at: usize) -> usize {
        let mut rest = self.text[at..].char_indices();
        match rest.next() {
            None => at,
            Some((_, first)) => rest
                .find(|&(_, c)| !zero_width(c))
                .map_or(self.text.len(), |(i, _)| at + i)
                .max(at + first.len_utf8()),
        }
    }

    /// What fits in `width` columns (one is kept for the cursor) and the
    /// cursor's column in it. The view scrolls just enough to show the cursor.
    pub fn view(&self, width: usize) -> (&str, u16) {
        let room = width.saturating_sub(1);
        let mut start = 0;
        while self.text[start..self.cursor].width() > room {
            start += self.text[start..].chars().next().map_or(1, char::len_utf8);
        }
        let mut end = start;
        let mut used = 0;
        for c in self.text[start..].chars() {
            let w = c.width().unwrap_or(0);
            if used + w > room.max(1) {
                break;
            }
            used += w;
            end += c.len_utf8();
        }
        let end = end.max(self.cursor);
        (
            &self.text[start..end],
            self.text[start..self.cursor].width() as u16,
        )
    }
}

impl From<&str> for TextInput {
    fn from(s: &str) -> Self {
        let mut t = TextInput::default();
        t.set(s);
        t
    }
}

impl From<String> for TextInput {
    fn from(s: String) -> Self {
        let mut t = TextInput::default();
        t.set(s);
        t
    }
}

impl PartialEq<&str> for TextInput {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_by_word_and_to_the_line_ends() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let k = KeyEvent::new(KeyCode::Null, KeyModifiers::NONE);
        let mut t = TextInput::from("send the  photo");
        assert_eq!(t.edit(Some(Cmd::WordLeft), &k), Edit::Moved);
        assert_eq!(&t.as_str()[t.cursor..], "photo");
        t.edit(Some(Cmd::WordLeft), &k);
        assert_eq!(&t.as_str()[t.cursor..], "the  photo");
        t.edit(Some(Cmd::WordRight), &k);
        assert_eq!(&t.as_str()[t.cursor..], "  photo");
        t.edit(Some(Cmd::WordRight), &k);
        assert_eq!(t.cursor, t.as_str().len());
        assert_eq!(t.edit(Some(Cmd::DeleteWordBackward), &k), Edit::Changed);
        assert_eq!(t, "send the  ");
        t.edit(Some(Cmd::DeleteWordBackward), &k);
        assert_eq!(t, "send ");
        t.set("สวัสดี ครับ");
        t.edit(Some(Cmd::DeleteWordBackward), &k);
        assert_eq!(t, "สวัสดี ");
        // Line-wise deletes stop at line breaks.
        t.set("first\nsecond line");
        t.edit(Some(Cmd::WordLeft), &k);
        t.edit(Some(Cmd::DeleteToLineStart), &k);
        assert_eq!(t, "first\nline");
        t.edit(Some(Cmd::DeleteToLineEnd), &k);
        assert_eq!(t, "first\n");
        t.home();
        t.left();
        t.edit(Some(Cmd::DeleteToLineStart), &k);
        assert_eq!(t, "\n");
        assert_eq!(t.edit(Some(Cmd::WordLeft), &k), Edit::MovedHitLeft);
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut t = TextInput::from("helo");
        t.left();
        t.insert('l');
        assert_eq!(t, "hello");
        t.home();
        t.delete();
        t.insert('H');
        assert_eq!(t, "Hello");
        t.end();
        t.backspace();
        assert_eq!(t, "Hell");
        t.home();
        t.backspace();
        t.left();
        assert_eq!(t, "Hell");
        t.end();
        t.right();
        t.delete();
        t.insert_str(" no");
        assert_eq!(t, "Hell no");
        assert_eq!(t.take(), "Hell no");
        assert!(t.as_str().is_empty());
    }

    #[test]
    fn edit_reports_moves_and_changes() {
        use crossterm::event::{KeyCode, KeyModifiers};

        let ch = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let none = KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE);

        let mut t = TextInput::default();

        // Empty input: each key hits the boundary in its own direction.
        assert_eq!(t.edit(Some(Cmd::DeleteBackward), &none), Edit::MovedHitLeft);
        assert_eq!(t.edit(Some(Cmd::DeleteForward), &none), Edit::MovedHitRight);
        assert_eq!(t.edit(Some(Cmd::CursorLeft), &none), Edit::MovedHitLeft);
        assert_eq!(t.edit(Some(Cmd::CursorRight), &none), Edit::MovedHitRight);

        // Insertions and cursor movement: reaching an end is a plain move.
        assert_eq!(t.edit(None, &ch('b')), Edit::Changed);
        assert_eq!(t.edit(Some(Cmd::LineStart), &none), Edit::Moved);
        assert_eq!(t.edit(None, &ch('a')), Edit::Changed);
        assert_eq!(t.edit(Some(Cmd::LineEnd), &none), Edit::Moved);
        assert_eq!(t.edit(None, &ch('😀')), Edit::Changed);
        assert_eq!(t, "ab😀");

        // Middle movement.
        assert_eq!(t.edit(Some(Cmd::CursorLeft), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorLeft), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorRight), &none), Edit::Moved);
        // Stepping onto the end is a move; only pressing past it hits.
        assert_eq!(t.edit(Some(Cmd::CursorRight), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorRight), &none), Edit::MovedHitRight);
        assert_eq!(t.edit(Some(Cmd::LineEnd), &none), Edit::MovedHitRight);

        // Likewise at the start: ← from the second character only moves.
        assert_eq!(t.edit(Some(Cmd::LineStart), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorRight), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorLeft), &none), Edit::Moved);
        assert_eq!(t.edit(Some(Cmd::CursorLeft), &none), Edit::MovedHitLeft);
        assert_eq!(t.edit(Some(Cmd::LineStart), &none), Edit::MovedHitLeft);

        // Deletion changes the input (Backspace at the start deletes nothing).
        assert_eq!(t.edit(Some(Cmd::DeleteBackward), &none), Edit::MovedHitLeft);
        assert_eq!(t.edit(Some(Cmd::DeleteForward), &none), Edit::Changed);
        assert_eq!(t, "b😀");

        // Not editing: keys handled by the caller and control chords.
        assert_eq!(t.edit(Some(Cmd::Confirm), &none), Edit::Ignored);
        assert_eq!(t.edit(None, &none), Edit::Ignored);

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(t.edit(None, &ctrl_a), Edit::Ignored);
        assert_eq!(t, "b😀");

        t.clear();
        assert!(t.is_empty());
        assert_eq!(t.view(10), ("", 0));
    }

    #[test]
    fn steps_over_combining_marks() {
        // "กี่" is ก + two zero-width marks; "ข" follows.
        let mut t = TextInput::from("กี่ข");
        t.left();
        t.left();
        assert_eq!(t.cursor, 0);
        t.right();
        assert_eq!(&t.as_str()[..t.cursor], "กี่");
        t.home();
        t.delete();
        assert_eq!(t, "ข");
    }

    #[test]
    fn lines_have_their_own_home_and_end() {
        let mut t = TextInput::from("one\ntwo\nthree");
        assert_eq!(t.line_count(), 3);
        t.home();
        assert_eq!(&t.as_str()[t.cursor..], "three");
        t.left();
        t.home();
        assert_eq!(&t.as_str()[t.cursor..], "two\nthree");
        t.end();
        assert_eq!(&t.as_str()[t.cursor..], "\nthree");
        assert_eq!(TextInput::default().line_count(), 1);
    }

    #[test]
    fn rows_wrap_at_spaces_and_line_breaks() {
        let t = TextInput::from("hello big world\n\nab");
        let text = |rows: Vec<(usize, usize)>| -> Vec<String> {
            rows.into_iter()
                .map(|(s, e)| t.as_str()[s..e].to_owned())
                .collect()
        };
        // 9 columns: 8 for text, one for the cursor.
        assert_eq!(text(t.rows(9)), ["hello ", "big ", "world", "", "ab"]);
        assert_eq!(text(t.rows(0)), ["hello big world", "", "ab"]);
        // A word wider than the row breaks mid-word.
        let t = TextInput::from("abcdefgh");
        assert_eq!(t.rows(4), [(0, 3), (3, 6), (6, 8)]);
        // Wide characters and marks: "กี่" is one column.
        let t = TextInput::from("日本語กี่");
        assert_eq!(t.rows(5), [(0, 6), (6, 18)]);
        assert_eq!(TextInput::default().rows(10), [(0, 0)]);
    }

    #[test]
    fn cursor_moves_between_rows_keeping_its_column() {
        let mut t = TextInput::from("abcdef\nxy\nlonger line");
        t.home();
        t.right();
        t.right();
        t.right();
        // In "longer line" at column 3; up goes to the end of "xy".
        assert!(t.move_row(-1, 0));
        assert_eq!(&t.as_str()[t.cursor..], "\nlonger line");
        assert!(t.move_row(-1, 0));
        assert_eq!(&t.as_str()[t.cursor..], "def\nxy\nlonger line");
        assert!(!t.move_row(-1, 0), "no row above the first");
        assert!(t.move_row(2, 0));
        assert_eq!(&t.as_str()[t.cursor..], "ger line");
        assert!(!t.move_row(1, 0), "no row below the last");
        // Any other move forgets the column.
        t.home();
        assert!(t.move_row(-1, 0));
        assert_eq!(&t.as_str()[t.cursor..], "xy\nlonger line");
        // Soft-wrapped rows: the cursor never lands on the hidden wrap point.
        let mut t = TextInput::from("aaaa bbbb");
        t.home();
        assert!(t.move_row(1, 6));
        assert_eq!(&t.as_str()[t.cursor..], "bbbb");
        t.end();
        assert!(t.move_row(-1, 6));
        assert_eq!(&t.as_str()[t.cursor..], " bbbb");
    }

    #[test]
    fn view_rows_follow_the_cursor() {
        let mut t = TextInput::from("1\n2\n3\n4");
        assert_eq!(t.view_rows(10, 2), (vec!["3", "4"], (1, 1)));
        t.move_row(-3, 10);
        assert_eq!(t.view_rows(10, 2), (vec!["1", "2"], (1, 0)));
        t.move_row(1, 10);
        assert_eq!(t.view_rows(10, 2), (vec!["2", "3"], (1, 0)));
        // A cursor after a trailing break sits on the empty last row.
        let t = TextInput::from("ab\n");
        assert_eq!(t.view_rows(10, 3), (vec!["ab", ""], (0, 1)));
        // At a soft wrap the cursor shows at the start of the next row.
        let t = TextInput::from("abcdef");
        assert_eq!(t.view_rows(4, 3), (vec!["abc", "def"], (3, 1)));
    }

    #[test]
    fn view_keeps_the_cursor_visible() {
        let t = TextInput::from("abcdefghij");
        assert_eq!(t.view(5), ("ghij", 4));
        let mut t = t;
        t.home();
        assert_eq!(t.view(5), ("abcd", 0));
        let t = TextInput::from("日本語");
        assert_eq!(t.view(4), ("語", 2));
        assert_eq!(TextInput::default().view(0), ("", 0));
        assert_eq!(TextInput::from("ab").view(10), ("ab", 2));
    }
}
