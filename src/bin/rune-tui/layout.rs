//! Screen geometry shared by every view: the main split, overlay boxes and
//! bubble widths, so spacing and behaviour change together at the same
//! window sizes everywhere.

use ratatui::layout::Rect;

/// Below this many columns (too narrow for the chat list beside a
/// conversation) one pane is shown at a time.
pub const SPLIT_MIN_W: u16 = 64;
/// Below this many columns an overlay's two halves (the file browser's list
/// and preview) are stacked rather than side by side.
pub const SIDE_BY_SIDE_MIN_W: u16 = 70;
/// Below this the window cannot show anything useful.
pub const MIN_W: u16 = 20;
pub const MIN_H: u16 = 6;

/// The chat list's width beside a conversation.
pub const CHAT_LIST_W: u16 = 38;
pub const CHAT_LIST_MIN_W: u16 = 26;

/// Message bubbles never grow past this, so lines stay readable on wide windows.
pub const BUBBLE_MAX_W: usize = 100;

pub fn too_small(area: Rect) -> bool {
    area.width < MIN_W || area.height < MIN_H
}

/// Which panes the main view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    pub list: Option<Rect>,
    pub chat: Option<Rect>,
}

/// The main view's panes in `area`: side by side when there is room, else
/// only the one that has the keys (`chat_first`: a chat is open and focused).
pub fn panes(area: Rect, chat_first: bool) -> Panes {
    if area.width < SPLIT_MIN_W {
        return Panes {
            list: (!chat_first).then_some(area),
            chat: chat_first.then_some(area),
        };
    }
    let list_w = (area.width * 2 / 5).clamp(CHAT_LIST_MIN_W, CHAT_LIST_W);
    Panes {
        list: Some(Rect {
            width: list_w,
            ..area
        }),
        chat: Some(Rect {
            x: area.x + list_w,
            width: area.width - list_w,
            ..area
        }),
    }
}

/// The space an overlay keeps free around itself: none when the window is
/// narrow or short (every cell counts), a little more as it grows.
fn margin(len: u16, small: u16) -> u16 {
    match len {
        l if l < small => 0,
        l if l < small * 2 => 1,
        _ => 2,
    }
}

/// A centred overlay box of at most `w`×`h` inside `area`, with the same
/// margins for every overlay.
pub fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let room_w = area.width.saturating_sub(2 * margin(area.width, SPLIT_MIN_W));
    let room_h = area.height.saturating_sub(2 * margin(area.height, 16));
    let w = w.min(room_w).max(10.min(area.width));
    let h = h.min(room_h).max(3.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// The largest overlay `area` takes (pictures, profiles): all the room
/// [`popup`] leaves, up to `max_w` wide.
pub fn sheet(area: Rect, max_w: u16) -> Rect {
    popup(area, max_w, u16::MAX)
}

/// How wide a message bubble may be in a conversation `width` columns wide.
pub fn bubble_width(width: usize) -> usize {
    let w = if width < usize::from(SPLIT_MIN_W) {
        // A phone-sized pane: nearly all of it.
        width.saturating_sub(2)
    } else {
        (width * 3 / 4).min(BUBBLE_MAX_W)
    };
    w.max(24).min(width.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_windows_show_one_pane() {
        let a = Rect::new(0, 0, 50, 20);
        assert_eq!(
            panes(a, false),
            Panes {
                list: Some(a),
                chat: None
            }
        );
        assert_eq!(
            panes(a, true),
            Panes {
                list: None,
                chat: Some(a)
            }
        );
    }

    #[test]
    fn panes_fill_the_width_without_overlap() {
        for w in SPLIT_MIN_W..300 {
            let a = Rect::new(3, 1, w, 20);
            let p = panes(a, true);
            let (l, c) = (p.list.unwrap(), p.chat.unwrap());
            assert_eq!(l.x, a.x);
            assert_eq!(l.right(), c.x);
            assert_eq!(c.right(), a.right());
            assert!((CHAT_LIST_MIN_W..=CHAT_LIST_W).contains(&l.width));
        }
    }

    #[test]
    fn popups_stay_inside_and_centred() {
        for w in 1..200 {
            for h in 1..60 {
                let a = Rect::new(2, 1, w, h);
                for (pw, ph) in [(10, 3), (80, 18), (u16::MAX, u16::MAX)] {
                    let r = popup(a, pw, ph);
                    assert!(a.contains(r.as_position()) || r.area() == 0);
                    assert!(r.right() <= a.right() && r.bottom() <= a.bottom(), "{a:?} {r:?}");
                    let (l, rr) = (r.x - a.x, a.right() - r.right());
                    assert!(l.abs_diff(rr) <= 1, "{a:?} {r:?}");
                }
            }
        }
    }

    #[test]
    fn bubbles_are_capped_and_fit() {
        assert_eq!(bubble_width(300), BUBBLE_MAX_W);
        assert_eq!(bubble_width(40), 38);
        for w in 2..300 {
            assert!(bubble_width(w) < w);
        }
    }
}
