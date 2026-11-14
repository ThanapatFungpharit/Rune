//! A picture drawn either with the terminal's image protocol or with text
//! cells, and drawable in part: a thumbnail scrolled half out of the pane is
//! cropped to the rows that are still visible.

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::Picker;
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};
use ratatui_image::{FontSize, Resize};

use crate::cells::{CellImage, CellMode, CellView};

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

impl Picture {
    pub fn new(img: DynamicImage, picker: Option<&Picker>) -> Self {
        match picker {
            Some(p) => Picture::Graphics(GraphicsImage {
                source: img,
                picker: p.clone(),
                cache: None,
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
