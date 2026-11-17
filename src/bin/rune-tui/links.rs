//! Links in message text, opening them, and QR codes in pictures.

use std::ops::Range;

use image::DynamicImage;

/// Byte ranges of `http://` / `https://` links in `text`. A link ends at
/// whitespace or the first non-ASCII character (Thai or Japanese text often
/// runs straight on after a link); trailing sentence punctuation is left out.
pub fn find_links(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = ["https://", "http://"]
        .iter()
        .filter_map(|p| text[from..].find(p).map(|i| from + i))
        .min()
    {
        let end = text[i..]
            .find(|c: char| c.is_whitespace() || !c.is_ascii() || matches!(c, '<' | '>' | '"'))
            .map_or(text.len(), |e| i + e);
        let mut url = &text[i..end];
        loop {
            let trimmed = url.trim_end_matches(['.', ',', '!', '?', ':', ';', '\'']);
            // Drop an unmatched closing bracket: "(see https://x.y/a)".
            let trimmed = match trimmed.chars().last() {
                Some(c @ (')' | ']')) => {
                    let open = if c == ')' { '(' } else { '[' };
                    if trimmed.matches(open).count() < trimmed.matches(c).count() {
                        &trimmed[..trimmed.len() - 1]
                    } else {
                        trimmed
                    }
                }
                _ => trimmed,
            };
            if trimmed.len() == url.len() {
                break;
            }
            url = trimmed;
        }
        // A bare scheme is not a link.
        if url.len() > "https://".len() && !url.ends_with("//") {
            out.push(i..i + url.len());
        }
        from = end.max(i + 1);
    }
    out
}

/// Whether `s` is a web link we are willing to hand to the system opener.
pub fn is_web_link(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && !s.chars().any(char::is_whitespace)
}

/// Open a web link in the default browser. Anything else is refused, so a
/// crafted message or QR code cannot make the opener run a local file.
pub fn open_url(url: &str) -> std::io::Result<()> {
    if !is_web_link(url) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a web link",
        ));
    }
    crate::system_open(std::ffi::OsStr::new(&opener_arg(url)))
}

/// The link as handed to the opener. `explorer.exe` splits its command line
/// by rules of its own, so on Windows a `"` (never valid in a URL; the
/// argument quoting escapes it in a way explorer may not honour) is sent
/// percent-encoded, as a browser would send it anyway.
fn opener_arg(url: &str) -> String {
    let url = url.trim();
    if cfg!(windows) {
        url.replace('"', "%22")
    } else {
        url.to_owned()
    }
}

/// Contents of every QR code found in a picture (blocking; run off the UI
/// thread). Large pictures are scanned at ≤1200 px, which still finds codes
/// that take up a sensible share of the picture.
pub fn scan_qr(img: &DynamicImage) -> Vec<String> {
    let img = if img.width() > 1200 || img.height() > 1200 {
        img.resize(1200, 1200, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let luma = img.to_luma8();
    let (w, h) = (luma.width() as usize, luma.height() as usize);
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma.get_pixel(x as u32, y as u32).0[0]);
    let mut out: Vec<String> = Vec::new();
    for grid in prepared.detect_grids() {
        if let Ok((_, content)) = grid.decode()
            && !content.is_empty()
            && !out.contains(&content)
        {
            out.push(content);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(s: &str) -> Vec<&str> {
        find_links(s).into_iter().map(|r| &s[r]).collect()
    }

    #[test]
    fn finds_links() {
        assert_eq!(
            links("see https://example.com/a?b=1."),
            ["https://example.com/a?b=1"]
        );
        assert_eq!(
            links("(at http://x.y/p) and https://z.w"),
            ["http://x.y/p", "https://z.w"]
        );
        assert_eq!(
            links("wiki https://en.wikipedia.org/wiki/Rust_(language)!"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(
            links("日本語https://line.me/R/ti/p/abcです"),
            ["https://line.me/R/ti/p/abc"]
        );
        assert!(links("no link, just https:// here").is_empty());
    }

    #[test]
    fn refuses_non_web_links() {
        assert!(is_web_link("https://a.b"));
        assert!(!is_web_link("file:///etc/passwd"));
        assert!(!is_web_link("/home/me/evil.desktop"));
        assert!(open_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn the_opener_gets_the_link_as_one_argument() {
        let url = "https://a.b/x?q=1&r=%41^b";
        assert_eq!(opener_arg(&format!(" {url}\n")), url, "no shell reads & ^ %");
        let quoted = opener_arg("https://a.b/\"x");
        if cfg!(windows) {
            assert_eq!(quoted, "https://a.b/%22x");
        } else {
            assert_eq!(quoted, "https://a.b/\"x");
        }
    }

    #[test]
    fn reads_a_qr_code() {
        let code = qrcode::QrCode::new("https://example.com/qr").unwrap();
        let (n, colors) = (code.width() as u32, code.to_colors());
        // 8 px modules with a 4-module quiet zone, as a photo of a code would have.
        let img = image::GrayImage::from_fn((n + 8) * 8, (n + 8) * 8, |x, y| {
            let (mx, my) = ((x / 8).wrapping_sub(4), (y / 8).wrapping_sub(4));
            let dark = mx < n && my < n && colors[(my * n + mx) as usize] == qrcode::Color::Dark;
            image::Luma([if dark { 0 } else { 255 }])
        });
        assert_eq!(
            scan_qr(&DynamicImage::ImageLuma8(img)),
            ["https://example.com/qr"]
        );
    }
}
