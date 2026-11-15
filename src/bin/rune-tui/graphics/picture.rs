//! A picture drawn either with the terminal's image protocol or with text
//! cells, and drawable in part: a thumbnail scrolled half out of the pane is
//! cropped to the rows that are still visible.
//!
//! Pictures are also stacked under overlays (see [`compose`]): terminal
//! graphics are not cells, so the usual "draw the overlay last" does not hide
//! them on its own.

use std::cell::RefCell;

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Clear;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};
use ratatui_image::{FontSize, Resize};

use crate::cells::{CellImage, CellMode, CellView};
use crate::piccache::Weigh;

pub enum Picture {
    Cells(CellImage),
    Graphics(GraphicsImage),
}

/// A picture for the terminal's image protocol, encoded row by row so any
/// band of rows can be shown.
pub struct GraphicsImage {
    source: DynamicImage,
    picker: Picker,
    /// Encoding for the last size drawn (`None` if encoding failed).
    cache: Option<(Size, Option<SlicedProtocol>)>,
    /// Text-cell version, made the first time the picture is partly covered
    /// by an overlay.
    cells: Option<CellImage>,
}

/// Cells an image of `w`×`h` pixels takes when scaled (up or down) to fit
/// `max`, for cells of `font` pixels.
pub fn fit(w: u32, h: u32, max: Size, font: FontSize) -> Size {
    if w == 0 || h == 0 || max.width == 0 || max.height == 0 || font.width == 0 || font.height == 0 {
        return Size::new(0, 0);
    }
    let (w, h) = (f64::from(w), f64::from(h));
    let (fw, fh) = (f64::from(font.width), f64::from(font.height));
    let s = (f64::from(max.width) * fw / w).min(f64::from(max.height) * fh / h);
    let cols = (w * s / fw).round().clamp(1.0, f64::from(max.width)) as u16;
    let rows = (h * s / fh).round().clamp(1.0, f64::from(max.height)) as u16;
    Size::new(cols, rows)
}

impl Weigh for Picture {
    /// Source pixels plus the encoding for the last size drawn. The
    /// protocol's encoding is estimated as base64 of the scaled RGBA pixels
    /// (what Kitty sends; Sixel and iTerm2 are smaller).
    fn weight(&self) -> usize {
        match self {
            Picture::Cells(c) => c.weight(),
            Picture::Graphics(g) => {
                let encoded = match &g.cache {
                    Some((size, Some(_))) => {
                        let font = g.picker.font_size();
                        let px = usize::from(size.width)
                            * usize::from(font.width)
                            * usize::from(size.height)
                            * usize::from(font.height);
                        px * 4 * 4 / 3
                    }
                    _ => 0,
                };
                g.source.as_bytes().len() + encoded + g.cells.as_ref().map_or(0, CellImage::weight)
            }
        }
    }
}

impl Picture {
    pub fn new(img: DynamicImage, picker: Option<&Picker>) -> Self {
        match picker {
            Some(p) => Picture::Graphics(GraphicsImage {
                source: img,
                picker: p.clone(),
                cache: None,
                cells: None,
            }),
            None => Picture::Cells(CellImage::new(img)),
        }
    }

    /// Cells the picture takes when scaled to fit `max`.
    pub fn size_for(&self, max: Size) -> Size {
        match self {
            Picture::Cells(c) => c.size_for(max),
            Picture::Graphics(g) => fit(g.source.width(), g.source.height(), max, g.picker.font_size()),
        }
    }

    /// Draw rows `skip..skip + visible.height` of the picture at `full` size
    /// into `visible`.
    pub fn render(
        &mut self,
        f: &mut Frame,
        visible: Rect,
        full: Size,
        skip: u16,
        mode: CellMode,
        filter: FilterType,
    ) {
        if visible.width == 0 || visible.height == 0 || full.width == 0 || full.height == 0 {
            return;
        }
        match self {
            Picture::Cells(c) => f.render_stateful_widget(
                CellView {
                    mode,
                    filter,
                    full,
                    skip,
                },
                visible,
                c,
            ),
            Picture::Graphics(g) => {
                match STACK.with_borrow(|s| s.hidden(visible)) {
                    Hidden::Fully => return,
                    Hidden::Partly => {
                        // Drawn as text, which the overlay's `Clear` then
                        // cuts cleanly where it covers the picture.
                        let cells = g.cells.get_or_insert_with(|| CellImage::new(g.source.clone()));
                        let view = CellView {
                            mode,
                            filter,
                            full,
                            skip,
                        };
                        f.render_stateful_widget(view, visible, cells);
                        return;
                    }
                    Hidden::No => {}
                }
                if !matches!(&g.cache, Some((s, _)) if *s == full) {
                    // Scale with a good filter here: the protocol's own
                    // slicing resizes with nearest-neighbour, a no-op once
                    // the picture is already exactly the right size.
                    let font = g.picker.font_size();
                    let img = g.source.resize_exact(
                        u32::from(full.width) * u32::from(font.width),
                        u32::from(full.height) * u32::from(font.height),
                        filter,
                    );
                    let sliced =
                        SlicedProtocol::new_with_resize(&g.picker, img, full, Resize::Fit(None)).ok();
                    g.cache = Some((full, sliced));
                }
                if let Some((_, Some(sliced))) = &g.cache {
                    let pos = SignedPosition::from((0, -i16::try_from(skip).unwrap_or(i16::MAX)));
                    f.render_widget(SlicedImage::new(sliced, pos), visible);
                }
            }
        }
    }
}

// -- stacking under overlays ------------------------------------------------------------
//
// Terminal graphics do not stack like text. Sixel and iTerm2 put the whole
// picture in its top-left cell and mark the others "skip", so an overlay drawn
// over part of a picture either leaves that cell in place (the terminal then
// paints the picture over the overlay) or keeps the overlay's own cells marked
// skip (they are never written, and the picture shows through). Kitty stacks
// per cell, but sends the picture only once, in its first cell: an overlay
// covering that cell loses the picture for good.
//
// So pictures are drawn knowing what covers them. Every overlay announces its
// box with `cover`; while one is open the frame is first laid out without any
// graphics, and in the real pass a picture under a higher layer's box is drawn
// with text cells (or not at all when fully covered).

/// Drawing order of the screen's layers: each is drawn over the ones before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    #[default]
    Base,
    Viewer,
    Profile,
    Modal,
}

/// How much of a picture's area the overlays above it cover.
#[derive(Debug, PartialEq, Eq)]
enum Hidden {
    No,
    Partly,
    Fully,
}

#[derive(Default)]
struct Stack {
    /// The layer being drawn.
    layer: Layer,
    /// Laying out: graphics draw nothing (nothing is encoded or sent).
    measuring: bool,
    /// Overlay boxes found by the layout pass, with their layers.
    covers: Vec<(Layer, Rect)>,
    /// Boxes announced in the pass being drawn.
    seen: Vec<(Layer, Rect)>,
}

impl Stack {
    fn hidden(&self, area: Rect) -> Hidden {
        if self.measuring {
            return Hidden::Fully;
        }
        let mut hidden = Hidden::No;
        for &(_, r) in self.covers.iter().filter(|(l, _)| *l > self.layer) {
            let common = r.intersection(area);
            if common == area {
                return Hidden::Fully;
            }
            if !common.is_empty() {
                hidden = Hidden::Partly;
            }
        }
        hidden
    }
}

thread_local! {
    static STACK: RefCell<Stack> = RefCell::default();
}

/// Draw a frame with `draw`, which calls [`layer`] before each overlay and
/// [`cover`] for each overlay's box. `layered` says pictures may be covered
/// (an overlay is open and pictures use the terminal's graphics): `draw` then
/// runs twice, once to find the boxes and once for real, so it must not
/// depend on having run before in the same frame.
pub fn compose<F: FnMut(&mut Frame)>(f: &mut Frame, layered: bool, mut draw: F) {
    let reset = |measuring: bool, covers: Vec<(Layer, Rect)>| {
        STACK.with_borrow_mut(|s| {
            *s = Stack {
                measuring,
                covers,
                ..Stack::default()
            }
        })
    };
    if layered {
        reset(true, Vec::new());
        draw(f);
        f.buffer_mut().reset();
        let covers = STACK.with_borrow_mut(|s| std::mem::take(&mut s.seen));
        reset(false, covers);
    } else {
        reset(false, Vec::new());
    }
    draw(f);
    reset(false, Vec::new());
}

/// What is drawn from now on is on `layer`, over everything drawn before.
pub fn layer(layer: Layer) {
    STACK.with_borrow_mut(|s| s.layer = layer);
}

/// Blank `area` for an overlay and note that it covers the layers below.
pub fn cover(f: &mut Frame, area: Rect) {
    f.render_widget(Clear, area);
    STACK.with_borrow_mut(|s| {
        let layer = s.layer;
        s.seen.push((layer, area));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::CellDiffOption;
    use ratatui_image::picker::ProtocolType;

    fn sixel_picker() -> Picker {
        let mut p = Picker::halfblocks();
        p.set_protocol_type(ProtocolType::Sixel);
        p
    }

    fn photo() -> DynamicImage {
        let mut img = image::RgbaImage::new(80, 80);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 3) as u8, (y * 3) as u8, 128, 255]);
        }
        DynamicImage::ImageRgba8(img)
    }

    /// A picture at (2, 2) of 10×5 cells, then (if `overlay`) a box over
    /// part of it, as the app draws them.
    fn frame(pic: &mut Picture, overlay: Option<Rect>) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(30, 12)).unwrap();
        term.draw(|f| {
            compose(f, overlay.is_some(), |f| {
                layer(Layer::Base);
                let full = Size::new(10, 5);
                pic.render(
                    f,
                    Rect::new(2, 2, 10, 5),
                    full,
                    0,
                    CellMode::Quadrant,
                    FilterType::Nearest,
                );
                if let Some(r) = overlay {
                    layer(Layer::Modal);
                    cover(f, r);
                    f.render_widget(ratatui::widgets::Paragraph::new("overlay text"), r);
                }
            })
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn graphics_cells(buf: &ratatui::buffer::Buffer) -> usize {
        buf.content()
            .iter()
            .filter(|c| c.symbol().contains('\x1b') || c.diff_option == CellDiffOption::Skip)
            .count()
    }

    #[test]
    fn uncovered_pictures_use_graphics() {
        let mut pic = Picture::new(photo(), Some(&sixel_picker()));
        assert!(graphics_cells(&frame(&mut pic, None)) > 0);
    }

    #[test]
    fn partly_covered_pictures_are_drawn_as_cells() {
        let mut pic = Picture::new(photo(), Some(&sixel_picker()));
        // Covers the picture's lower right, not its anchor cell.
        let buf = frame(&mut pic, Some(Rect::new(6, 4, 15, 5)));
        assert_eq!(graphics_cells(&buf), 0, "graphics leak past the overlay");
        let row: String = (6..18).map(|x| buf[(x, 4)].symbol().to_owned()).collect();
        assert_eq!(row, "overlay text");
        // The part left uncovered still shows the picture.
        assert_ne!(buf[(2, 2)].symbol(), " ");
    }

    #[test]
    fn fully_covered_pictures_draw_nothing() {
        let mut pic = Picture::new(photo(), Some(&sixel_picker()));
        let buf = frame(&mut pic, Some(Rect::new(0, 0, 30, 12)));
        assert_eq!(graphics_cells(&buf), 0);
        let Picture::Graphics(g) = &pic else {
            unreachable!()
        };
        assert!(
            g.cells.is_none() && g.cache.is_none(),
            "a hidden picture is not encoded"
        );
    }

    #[test]
    fn pictures_on_top_of_their_overlay_keep_graphics() {
        let mut pic = Picture::new(photo(), Some(&sixel_picker()));
        let mut term = Terminal::new(TestBackend::new(30, 12)).unwrap();
        term.draw(|f| {
            compose(f, true, |f| {
                layer(Layer::Modal);
                cover(f, Rect::new(0, 0, 30, 12));
                let full = Size::new(10, 5);
                pic.render(
                    f,
                    Rect::new(2, 2, 10, 5),
                    full,
                    0,
                    CellMode::Quadrant,
                    FilterType::Nearest,
                );
            })
        })
        .unwrap();
        assert!(graphics_cells(term.backend().buffer()) > 0);
    }
}
