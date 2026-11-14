//! Pictures drawn with text: every terminal cell shows a block glyph in two
//! colours, chosen so the cell best matches a small grid of image pixels.
//!
//! ratatui-image's half-blocks give each cell 1×2 pixels. Quadrants (`▖▞▟…`)
//! give 2×2 and sextants (`🬀🬔🬻…`) 2×3 — twice and three times the detail —
//! at the cost of two colours per cell instead of one per pixel. Quadrants
//! are in every terminal font; sextants need a terminal that draws Unicode's
//! "Symbols for Legacy Computing" (kitty, WezTerm, foot, Ghostty, recent VTE
//! and Konsole).

use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::style::Color;
use ratatui::widgets::StatefulWidget;

/// How each cell is split into pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellMode {
    /// 1×2 pixels: `▀ ▄`.
    Half,
    /// 2×2 pixels: `▘ ▝ ▖ ▗ ▌ ▐ ▀ ▄ ▚ ▞ ▛ ▜ ▙ ▟`.
    Quadrant,
    /// 2×3 pixels: U+1FB00 sextants.
    Sextant,
}

impl CellMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "half" | "halfblocks" => Some(Self::Half),
            "quad" | "quadrant" | "quadrants" => Some(Self::Quadrant),
            "sextant" | "sextants" => Some(Self::Sextant),
            _ => None,
        }
    }

    /// Pixels per cell (columns, rows).
    fn grid(self) -> (u32, u32) {
        match self {
            Self::Half => (1, 2),
            Self::Quadrant => (2, 2),
            Self::Sextant => (2, 3),
        }
    }

    /// Glyph whose foreground covers the pixels set in `mask` (bit `y*w + x`).
    fn glyph(self, mask: u8) -> char {
        match self {
            Self::Half => [' ', '▀', '▄', '█'][mask as usize],
            Self::Quadrant => [
                ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
            ][mask as usize],
            Self::Sextant => match mask {
                0 => ' ',
                21 => '▌',
                42 => '▐',
                63 => '█',
                // U+1FB00.. lists the other 60 patterns in mask order.
                n => {
                    let skip = u32::from(n > 21) + u32::from(n > 42);
                    char::from_u32(0x1FB00 + u32::from(n) - 1 - skip).unwrap_or('█')
                }
            },
        }
    }
}

/// Largest originals are shrunk once to this many pixels per side; no
/// terminal shows more, and it keeps re-encoding on resize fast.
const MAX_SOURCE: u32 = 1200;
/// Pixels below this alpha show the terminal's own background.
const ALPHA_CUTOFF: u8 = 128;

/// One cell: glyph, foreground, background.
type Glyph = (char, Color, Color);

/// A picture plus its encoding for the last size it was drawn at.
pub struct CellImage {
    source: RgbaImage,
    cache: Option<(Size, CellMode, Vec<Glyph>)>,
}

impl CellImage {
    pub fn new(img: DynamicImage) -> Self {
        let img = if img.width() > MAX_SOURCE || img.height() > MAX_SOURCE {
            img.resize(MAX_SOURCE, MAX_SOURCE, FilterType::Triangle)
        } else {
            img
        };
        Self {
            source: img.to_rgba8(),
            cache: None,
        }
    }

    /// Cells the picture takes when scaled (up or down) to fit `max`,
    /// assuming cells twice as tall as wide.
    pub fn size_for(&self, max: Size) -> Size {
        let (w, h) = self.source.dimensions();
        crate::picture::fit(w, h, max, ratatui_image::FontSize { width: 1, height: 2 })
    }

    fn encode(&self, size: Size, mode: CellMode, filter: FilterType) -> Vec<Glyph> {
        let (gw, gh) = mode.grid();
        let (pw, ph) = (u32::from(size.width) * gw, u32::from(size.height) * gh);
        let px = image::imageops::resize(&self.source, pw, ph, filter);
        let n = (gw * gh) as usize;
        let mut out = Vec::with_capacity(usize::from(size.width) * usize::from(size.height));
        let mut sub = [[0f32; 3]; 6];
        for cy in 0..u32::from(size.height) {
            for cx in 0..u32::from(size.width) {
                let mut opaque = 0u8;
                for i in 0..n as u32 {
                    let p = px.get_pixel(cx * gw + i % gw, cy * gh + i / gw).0;
                    sub[i as usize] = [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])];
                    if p[3] >= ALPHA_CUTOFF {
                        opaque |= 1 << i;
                    }
                }
                out.push(fit_cell(&sub[..n], opaque, mode));
            }
        }
        out
    }
}

/// Best glyph and colours for one cell's pixels.
fn fit_cell(sub: &[[f32; 3]], opaque: u8, mode: CellMode) -> Glyph {
    let n = sub.len();
    let full = ((1u16 << n) - 1) as u8;
    let mean = |mask: u8| {
        let (mut s, mut c) = ([0f32; 3], 0f32);
        for (i, p) in sub.iter().enumerate() {
            if mask & (1 << i) != 0 {
                for k in 0..3 {
                    s[k] += p[k];
                }
                c += 1.0;
            }
        }
        let c = c.max(1.0);
        Color::Rgb((s[0] / c) as u8, (s[1] / c) as u8, (s[2] / c) as u8)
    };
    if opaque == 0 {
        return (' ', Color::Reset, Color::Reset);
    }
    if opaque != full {
        // Transparent pixels keep the terminal background.
        return (mode.glyph(opaque), mean(opaque), Color::Reset);
    }
    // Split the pixels into two groups with the least squared error:
    // error = Σ|p|² − |Σ₁|²/n₁ − |Σ₀|²/n₀, so maximise the last two terms.
    let total: [f32; 3] = sub
        .iter()
        .fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let sq = |v: [f32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let (mut best, mut best_score) = (0u8, sq(total) / n as f32);
    // Masks and their complements are the same split: keep the last pixel unset.
    for mask in 1..(1u8 << (n - 1)) {
        let (mut s1, mut c1) = ([0f32; 3], 0f32);
        for (i, p) in sub.iter().enumerate() {
            if mask & (1 << i) != 0 {
                s1 = [s1[0] + p[0], s1[1] + p[1], s1[2] + p[2]];
                c1 += 1.0;
            }
        }
        let s0 = [total[0] - s1[0], total[1] - s1[1], total[2] - s1[2]];
        let score = sq(s1) / c1 + sq(s0) / (n as f32 - c1);
        if score > best_score {
            (best, best_score) = (mask, score);
        }
    }
    if best == 0 {
        let c = mean(full);
        return (' ', c, c);
    }
    (mode.glyph(best), mean(best), mean(full & !best))
}

/// Draws rows `skip..skip + area.height` of a [`CellImage`] stretched to
/// `full` cells (size it with [`CellImage::size_for`] first).
pub struct CellView {
    pub mode: CellMode,
    pub filter: FilterType,
    pub full: Size,
    pub skip: u16,
}

impl StatefulWidget for CellView {
    type State = CellImage;

    fn render(self, area: Rect, buf: &mut Buffer, img: &mut CellImage) {
        let size = self.full;
        if size.width == 0 || size.height == 0 {
            return;
        }
        if !matches!(&img.cache, Some((s, m, _)) if *s == size && *m == self.mode) {
            img.cache = Some((size, self.mode, img.encode(size, self.mode, self.filter)));
        }
        let Some((_, _, cells)) = &img.cache else { return };
        let w = usize::from(size.width);
        let rows = cells
            .chunks(w)
            .skip(usize::from(self.skip))
            .take(usize::from(area.height));
        for (dy, row) in rows.enumerate() {
            for (dx, &(ch, fg, bg)) in row.iter().take(usize::from(area.width)).enumerate() {
                if let Some(cell) = buf.cell_mut((area.x + dx as u16, area.y + dy as u16)) {
                    cell.set_char(ch).set_fg(fg).set_bg(bg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sextant_glyphs() {
        let m = CellMode::Sextant;
        assert_eq!(m.glyph(1), '\u{1FB00}');
        assert_eq!(m.glyph(20), '\u{1FB13}');
        assert_eq!(m.glyph(22), '\u{1FB14}');
        assert_eq!(m.glyph(62), '\u{1FB3B}');
        assert_eq!((m.glyph(21), m.glyph(42)), ('▌', '▐'));
    }

    #[test]
    fn splits_a_two_colour_cell_exactly() {
        // Top-left and bottom-right white, the rest black: `▚`.
        let w = [255.0; 3];
        let b = [0.0; 3];
        let (ch, fg, bg) = fit_cell(&[w, b, b, w], 0b1111, CellMode::Quadrant);
        let (ch, fg, bg) = if fg == Color::Rgb(0, 0, 0) {
            (ch, bg, fg)
        } else {
            (ch, fg, bg)
        };
        assert!(ch == '▚' || ch == '▞');
        assert_eq!((fg, bg), (Color::Rgb(255, 255, 255), Color::Rgb(0, 0, 0)));
    }

    #[test]
    fn keeps_transparency() {
        let p = [10.0; 3];
        assert_eq!(
            fit_cell(&[p, p], 0b01, CellMode::Half),
            ('▀', Color::Rgb(10, 10, 10), Color::Reset)
        );
        assert_eq!(fit_cell(&[p, p], 0, CellMode::Half).0, ' ');
    }

    #[test]
    fn draws_only_the_visible_rows() {
        // Top half red, bottom half blue; show only the bottom row of two.
        let img = image::RgbImage::from_fn(4, 8, |_, y| {
            if y < 4 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let mut c = CellImage::new(DynamicImage::ImageRgb8(img));
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        let view = CellView {
            mode: CellMode::Half,
            filter: FilterType::Nearest,
            full: Size::new(2, 2),
            skip: 1,
        };
        view.render(buf.area, &mut buf, &mut c);
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(0, 0, 255));
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn fits_wide_and_tall_pictures() {
        let img = CellImage::new(DynamicImage::new_rgb8(400, 100));
        assert_eq!(img.size_for(Size::new(40, 40)), Size::new(40, 5));
        let img = CellImage::new(DynamicImage::new_rgb8(100, 400));
        assert_eq!(img.size_for(Size::new(40, 20)), Size::new(10, 20));
    }
}
