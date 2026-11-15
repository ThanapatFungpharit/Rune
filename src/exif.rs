//! Lossless metadata scrubbing for images sent as originals.
//!
//! The pixels are never re-encoded. Only metadata containers are rewritten:
//!
//! * **EXIF** is rebuilt from a whitelist ([`IFD0_KEEP`], [`EXIF_KEEP`]):
//!   orientation, camera/lens model, capture time and exposure settings stay;
//!   everything else goes — GPS, serial numbers, owner/artist names, maker
//!   notes (which often carry serials or location), comments, the interop IFD
//!   and the embedded thumbnail (which can show the uncropped picture).
//! * **XMP**, **IPTC/Photoshop** and comments are dropped (XMP regularly
//!   duplicates GPS data).
//! * JPEG: data after the primary image's EOI (MPF secondary images such as
//!   HDR gain maps or depth maps, each with their own EXIF) is cut, and the
//!   `MPF` segment pointing at them is removed.
//! * PNG: `eXIf` is scrubbed, `tEXt`/`zTXt`/`iTXt` are dropped.
//! * WebP: the `EXIF` chunk is scrubbed, `XMP ` is dropped.
//!
//! Colour profiles (ICC) are kept. Other formats are returned unchanged.
//! Anything malformed is dropped rather than passed through; a JPEG, PNG or
//! WebP whose structure cannot be walked at all gives `None`, never the
//! original bytes with their metadata.

/// IFD0 tags kept: Make, Model, Orientation, X/YResolution, ResolutionUnit,
/// Software, DateTime, YCbCrPositioning.
const IFD0_KEEP: &[u16] = &[
    0x010F, 0x0110, 0x0112, 0x011A, 0x011B, 0x0128, 0x0131, 0x0132, 0x0213,
];

/// Exif sub-IFD tags kept: exposure settings, capture times, colour space,
/// pixel dimensions, lens make/model/specification.
const EXIF_KEEP: &[u16] = &[
    0x829A, 0x829D, 0x8822, 0x8827, 0x8830, 0x9000, 0x9003, 0x9004, 0x9010, 0x9011, 0x9012, 0x9101, 0x9201,
    0x9202, 0x9203, 0x9204, 0x9205, 0x9207, 0x9208, 0x9209, 0x920A, 0x9290, 0x9291, 0x9292, 0xA000, 0xA001,
    0xA002, 0xA003, 0xA402, 0xA403, 0xA404, 0xA405, 0xA406, 0xA432, 0xA433, 0xA434,
];

const TAG_EXIF_IFD: u16 = 0x8769;

/// Strip privacy-sensitive metadata from a JPEG, PNG or WebP file (other
/// formats are returned as they are). `None` when the file claims to be one
/// of these but cannot be parsed: its metadata could not be removed, so the
/// caller must not send it as is (re-encode the pixels instead).
pub(crate) fn scrub(data: &[u8]) -> Option<Vec<u8>> {
    if data.starts_with(&[0xFF, 0xD8]) {
        scrub_jpeg(data)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        scrub_png(data)
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        scrub_webp(data)
    } else {
        Some(data.to_vec())
    }
}

// -- TIFF/EXIF ----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Order(bool); // true = little-endian

impl Order {
    fn u16(self, b: &[u8], at: usize) -> Option<u16> {
        let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(if self.0 {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }
    fn u32(self, b: &[u8], at: usize) -> Option<u32> {
        let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if self.0 {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }
    fn put16(self, out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&if self.0 { v.to_le_bytes() } else { v.to_be_bytes() });
    }
    fn put32(self, out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&if self.0 { v.to_le_bytes() } else { v.to_be_bytes() });
    }
}

/// One IFD entry with its value bytes (in the file's byte order).
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    value: Vec<u8>,
}

fn type_size(typ: u16) -> Option<usize> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// Read the IFD at `off`, keeping only whitelisted tags (plus the Exif pointer
/// when `keep` is IFD0's list; its value is returned separately).
fn read_ifd(
    t: &[u8],
    o: Order,
    off: usize,
    keep: &[u16],
    is_ifd0: bool,
) -> Option<(Vec<Entry>, Option<usize>)> {
    let n = o.u16(t, off)? as usize;
    let mut kept = Vec::new();
    let mut exif_ptr = None;
    for i in 0..n {
        let e = off + 2 + i * 12;
        let tag = o.u16(t, e)?;
        let typ = o.u16(t, e + 2)?;
        let count = o.u32(t, e + 4)?;
        if is_ifd0 && tag == TAG_EXIF_IFD {
            exif_ptr = Some(o.u32(t, e + 8)? as usize);
            continue;
        }
        if !keep.contains(&tag) {
            continue;
        }
        let Some(size) = type_size(typ) else { continue };
        let len = size.checked_mul(count as usize)?;
        let value = if len <= 4 {
            t.get(e + 8..e + 8 + len)?.to_vec()
        } else {
            let at = o.u32(t, e + 8)? as usize;
            t.get(at..at.checked_add(len)?)?.to_vec()
        };
        kept.push(Entry {
            tag,
            typ,
            count,
            value,
        });
    }
    kept.sort_by_key(|e| e.tag);
    Some((kept, exif_ptr))
}

/// Append an IFD (entries sorted by tag) at the current end of `out` (offsets are relative to the
/// TIFF header at `out[0]`). Returns the position of the entry whose value
/// must be patched to point at the Exif IFD, if `exif_ptr` is set.
fn write_ifd(out: &mut Vec<u8>, o: Order, entries: &[Entry], exif_ptr: bool) -> Option<usize> {
    let n = entries.len() + exif_ptr as usize;
    let start = out.len();
    let mut data_at = start + 2 + n * 12 + 4;
    let mut data = Vec::new();
    let mut patch = None;
    o.put16(out, n as u16);
    let mut wrote_ptr = !exif_ptr;
    for e in entries {
        if !wrote_ptr && e.tag > TAG_EXIF_IFD {
            o.put16(out, TAG_EXIF_IFD);
            o.put16(out, 4);
            o.put32(out, 1);
            patch = Some(out.len());
            o.put32(out, 0);
            wrote_ptr = true;
        }
        o.put16(out, e.tag);
        o.put16(out, e.typ);
        o.put32(out, e.count);
        if e.value.len() <= 4 {
            let mut v = e.value.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            o.put32(out, data_at as u32);
            data.extend_from_slice(&e.value);
            if data.len() % 2 == 1 {
                data.push(0);
            }
            data_at = start + 2 + n * 12 + 4 + data.len();
        }
    }
    if !wrote_ptr {
        o.put16(out, TAG_EXIF_IFD);
        o.put16(out, 4);
        o.put32(out, 1);
        patch = Some(out.len());
        o.put32(out, 0);
    }
    o.put32(out, 0); // no IFD1 (thumbnail)
    out.extend_from_slice(&data);
    patch
}

/// Rebuild a TIFF structure keeping only whitelisted tags. `None` when it is
/// malformed or nothing is left worth keeping.
fn scrub_tiff(t: &[u8]) -> Option<Vec<u8>> {
    let o = match t.get(0..4)? {
        b"II*\0" => Order(true),
        b"MM\0*" => Order(false),
        _ => return None,
    };
    let ifd0 = o.u32(t, 4)? as usize;
    let (e0, exif_ptr) = read_ifd(t, o, ifd0, IFD0_KEEP, true)?;
    let ex = match exif_ptr {
        Some(p) => read_ifd(t, o, p, EXIF_KEEP, false)?.0,
        None => Vec::new(),
    };
    if e0.is_empty() && ex.is_empty() {
        return None;
    }
    let mut out = t[0..4].to_vec();
    o.put32(&mut out, 8);
    let patch = write_ifd(&mut out, o, &e0, !ex.is_empty());
    if let Some(at) = patch {
        let here = out.len() as u32;
        let b = if o.0 {
            here.to_le_bytes()
        } else {
            here.to_be_bytes()
        };
        out[at..at + 4].copy_from_slice(&b);
        write_ifd(&mut out, o, &ex, false);
    }
    Some(out)
}

// -- JPEG -----------------------------------------------------------------------------------

fn scrub_jpeg(d: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        // Skip fill bytes before a marker.
        while d.get(i)? == &0xFF && d.get(i + 1)? == &0xFF {
            i += 1;
        }
        if d.get(i)? != &0xFF {
            return None;
        }
        let m = *d.get(i + 1)?;
        if m == 0xD9 {
            out.extend_from_slice(&[0xFF, 0xD9]);
            return Some(out); // anything after EOI is dropped
        }
        if (0xD0..=0xD7).contains(&m) || m == 0x01 {
            out.extend_from_slice(&d[i..i + 2]);
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([*d.get(i + 2)?, *d.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        let seg = d.get(i..i + 2 + len)?;
        let body = &seg[4..];
        match m {
            0xE1 if body.starts_with(b"Exif\0\0") => {
                if let Some(t) = scrub_tiff(&body[6..]) {
                    let new_len = 2 + 6 + t.len();
                    if new_len <= 0xFFFF {
                        out.extend_from_slice(&[0xFF, 0xE1]);
                        out.extend_from_slice(&(new_len as u16).to_be_bytes());
                        out.extend_from_slice(b"Exif\0\0");
                        out.extend_from_slice(&t);
                    }
                }
            }
            0xE1 => {}                               // XMP / extended XMP
            0xE2 if body.starts_with(b"MPF\0") => {} // multi-picture index
            0xED | 0xFE => {}                        // IPTC/Photoshop, comment
            _ => out.extend_from_slice(seg),
        }
        i += 2 + len;
        if m == 0xDA {
            // Entropy-coded data: copy up to the next real marker.
            let start = i;
            loop {
                let Some(&b) = d.get(i) else {
                    // A truncated file (no EOI): its metadata segments are
                    // behind us already, so keep the scan data and end it.
                    out.extend_from_slice(&d[start..]);
                    out.extend_from_slice(&[0xFF, 0xD9]);
                    return Some(out);
                };
                if b == 0xFF {
                    let n = *d.get(i + 1).unwrap_or(&0x00);
                    if n != 0x00 && !(0xD0..=0xD7).contains(&n) && n != 0xFF {
                        break;
                    }
                }
                i += 1;
            }
            out.extend_from_slice(&d[start..i]);
        }
    }
}

// -- PNG ------------------------------------------------------------------------------------

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for p in parts {
        for &b in *p {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
    }
    !c
}

fn scrub_png(d: &[u8]) -> Option<Vec<u8>> {
    let mut out = d[..8].to_vec();
    let mut i = 8;
    while i < d.len() {
        let len = u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?) as usize;
        let typ = d.get(i + 4..i + 8)?;
        let body = d.get(i + 8..i + 8 + len)?;
        let whole = d.get(i..i + 12 + len)?;
        match typ {
            b"eXIf" => {
                if let Some(t) = scrub_tiff(body) {
                    out.extend_from_slice(&(t.len() as u32).to_be_bytes());
                    out.extend_from_slice(b"eXIf");
                    out.extend_from_slice(&t);
                    out.extend_from_slice(&crc32(&[b"eXIf", &t]).to_be_bytes());
                }
            }
            b"tEXt" | b"zTXt" | b"iTXt" => {}
            _ => out.extend_from_slice(whole),
        }
        i += 12 + len;
        if typ == b"IEND" {
            break;
        }
    }
    Some(out)
}

// -- WebP -----------------------------------------------------------------------------------

fn scrub_webp(d: &[u8]) -> Option<Vec<u8>> {
    let mut chunks = Vec::new();
    let mut has_exif = false;
    let mut i = 12;
    while i + 8 <= d.len() {
        let id: [u8; 4] = d[i..i + 4].try_into().ok()?;
        let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().ok()?) as usize;
        let body = d.get(i + 8..i + 8 + len)?;
        match &id {
            b"EXIF" => {
                let (prefix, tiff) = match body.strip_prefix(b"Exif\0\0") {
                    Some(t) => (&b"Exif\0\0"[..], t),
                    None => (&b""[..], body),
                };
                if let Some(t) = scrub_tiff(tiff) {
                    chunks.push((id, [prefix, &t].concat()));
                    has_exif = true;
                }
            }
            b"XMP " => {}
            _ => chunks.push((id, body.to_vec())),
        }
        i += 8 + len + (len & 1);
    }
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    for (id, mut body) in chunks {
        if &id == b"VP8X" && !body.is_empty() {
            body[0] &= !0x04; // XMP flag
            if !has_exif {
                body[0] &= !0x08; // EXIF flag
            }
        }
        out.extend_from_slice(&id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
    }
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian TIFF with Make, Orientation, Artist (dropped), a GPS
    /// pointer (dropped) and an Exif IFD with DateTimeOriginal and
    /// BodySerialNumber (dropped).
    fn sample_tiff() -> Vec<u8> {
        let o = Order(true);
        let mut t = b"II*\0".to_vec();
        o.put32(&mut t, 8);
        // IFD0 at 8: 5 entries -> 2 + 60 + 4 = 66 bytes -> data at 74.
        o.put16(&mut t, 5);
        let ent = |t: &mut Vec<u8>, tag: u16, typ: u16, count: u32, v: u32| {
            o.put16(t, tag);
            o.put16(t, typ);
            o.put32(t, count);
            o.put32(t, v);
        };
        ent(&mut t, 0x010F, 2, 6, 74); // Make "Canon\0"
        ent(&mut t, 0x0112, 3, 1, 6); // Orientation
        ent(&mut t, 0x013B, 2, 4, u32::from_le_bytes(*b"Bob\0")); // Artist
        ent(&mut t, 0x8769, 4, 1, 80); // Exif IFD
        ent(&mut t, 0x8825, 4, 1, 200); // GPS IFD
        o.put32(&mut t, 0);
        t.extend_from_slice(b"Canon\0");
        assert_eq!(t.len(), 80);
        // Exif IFD at 80: 2 entries -> 2 + 24 + 4 = 30 -> data at 110.
        o.put16(&mut t, 2);
        ent(&mut t, 0x9003, 2, 20, 110);
        ent(&mut t, 0xA431, 2, 8, 130);
        o.put32(&mut t, 0);
        t.extend_from_slice(b"2026:11:14 10:00:00\0");
        t.extend_from_slice(b"SN12345\0");
        t
    }

    fn tags(t: &[u8], off: usize) -> Vec<(u16, u32)> {
        let o = Order(true);
        let n = o.u16(t, off).unwrap() as usize;
        (0..n)
            .map(|i| {
                (
                    o.u16(t, off + 2 + i * 12).unwrap(),
                    o.u32(t, off + 10 + i * 12).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn tiff_whitelist() {
        let t = scrub_tiff(&sample_tiff()).unwrap();
        let ifd0 = tags(&t, 8);
        assert_eq!(
            ifd0.iter().map(|e| e.0).collect::<Vec<_>>(),
            [0x010F, 0x0112, 0x8769]
        );
        let make_at = ifd0[0].1 as usize;
        assert_eq!(&t[make_at..make_at + 6], b"Canon\0");
        assert_eq!(ifd0[1].1 & 0xFFFF, 6);
        let ex = tags(&t, ifd0[2].1 as usize);
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].0, 0x9003);
        let at = ex[0].1 as usize;
        assert_eq!(&t[at..at + 19], b"2026:11:14 10:00:00");
        let s = String::from_utf8_lossy(&t);
        assert!(!s.contains("SN12345") && !s.contains("Bob"));
    }

    #[test]
    fn jpeg_strips_and_truncates() {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 10, 10]));
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90)
            .encode_image(&img)
            .unwrap();
        // Insert EXIF + XMP after SOI, and junk after EOI.
        let tiff = sample_tiff();
        let mut exif = vec![0xFF, 0xE1];
        exif.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend_from_slice(&tiff);
        let xmp_body = b"http://ns.adobe.com/xap/1.0/\0<x:gps lat=1/>";
        let mut xmp = vec![0xFF, 0xE1];
        xmp.extend_from_slice(&((2 + xmp_body.len()) as u16).to_be_bytes());
        xmp.extend_from_slice(xmp_body);
        let input = [&plain[..2], &exif, &xmp, &plain[2..], b"SECONDIMAGE-SN12345"].concat();

        let out = scrub(&input).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Exif") && s.contains("Canon"));
        assert!(!s.contains("SN12345") && !s.contains("gps") && !s.contains("Bob"));
        assert!(out.ends_with(&[0xFF, 0xD9]));
        let a = image::load_from_memory(&out).unwrap().to_rgb8();
        let b = image::load_from_memory(&plain).unwrap().to_rgb8();
        assert_eq!(a, b, "pixels untouched");
    }

    /// A small JPEG with the sample EXIF (Artist, serial number, GPS pointer) after SOI.
    pub(crate) fn jpeg_with_exif() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 10, 10]));
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90)
            .encode_image(&img)
            .unwrap();
        let tiff = sample_tiff();
        let mut exif = vec![0xFF, 0xE1];
        exif.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend_from_slice(&tiff);
        [&plain[..2], &exif, &plain[2..]].concat()
    }

    #[test]
    fn truncated_jpeg_is_still_scrubbed() {
        let full = jpeg_with_exif();
        // Cut inside the scan data: no EOI.
        let cut = &full[..full.len() - 6];
        let out = scrub(cut).expect("a truncated JPEG still loses its metadata");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Canon") && !s.contains("SN12345") && !s.contains("Bob"));
        assert!(out.ends_with(&[0xFF, 0xD9]));
    }

    #[test]
    fn unparsable_files_never_come_back_with_their_metadata() {
        let full = jpeg_with_exif();
        // Garbage where the next marker should be, right after the EXIF segment.
        let exif_end = 4 + u16::from_be_bytes([full[4], full[5]]) as usize;
        let broken = [&full[..exif_end], b"\x00\x00junk", &full[exif_end..]].concat();
        assert_eq!(scrub(&broken), None);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 40, b't', b'E', b'X', b't']); // chunk longer than the file
        assert_eq!(scrub(&png), None);
        // Formats without a scrubber pass through.
        assert_eq!(scrub(b"GIF89a...").as_deref(), Some(&b"GIF89a..."[..]));
    }

    #[test]
    fn png_strips_text_and_scrubs_exif() {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]));
        let mut plain = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut plain), image::ImageFormat::Png)
            .unwrap();
        let chunk = |typ: &[u8], body: &[u8]| {
            let mut c = (body.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(typ);
            c.extend_from_slice(body);
            c.extend_from_slice(&crc32(&[typ, body]).to_be_bytes());
            c
        };
        // After IHDR (8 + 25 bytes).
        let input = [
            &plain[..33],
            &chunk(b"eXIf", &sample_tiff()),
            &chunk(b"tEXt", b"Comment\0GPS 13.7,100.5"),
            &plain[33..],
        ]
        .concat();
        let out = scrub(&input).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("eXIf") && s.contains("Canon"));
        assert!(!s.contains("GPS 13.7") && !s.contains("SN12345"));
        assert_eq!(image::load_from_memory(&out).unwrap().to_rgb8(), img);
    }
}
