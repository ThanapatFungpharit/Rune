//! A one-line text field with a cursor: the message line and form fields.
//! The cursor moves by character, stepping over zero-width ones (Thai vowel
//! and tone marks, emoji modifiers) so it never lands inside a cluster.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset, always on a character boundary.
    cursor: usize,
}

fn zero_width(c: char) -> bool {
    c.width() == Some(0)
}

impl TextInput {
    pub fn as_str(&self) -> &str {
        &self.text
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

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
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
