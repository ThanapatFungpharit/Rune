//! The colour palette: every colour the UI draws with, by what it means.
//!
//! Roles, not hues, are what drawing code asks for (`theme::t().accent`):
//! - `accent` marks what is interactive or focused (focused borders, the
//!   selection marker, key names, our own messages); `on_accent` is text on it.
//! - `text` is emphasised text (names, titles); body text keeps the
//!   terminal's own colour. `muted` is secondary text (times, hints,
//!   previews of read chats); `border` frames what is not focused.
//! - `surface` / `surface_hi` are quiet backgrounds (input fields, the
//!   selected row); `accent_soft` is a tinted one (the highlighted menu item,
//!   our own reactions).
//! - `warn` draws attention without alarm (unread divider, invitations,
//!   search matches, the receipts-off badge); `danger` is for failures and
//!   destructive buttons; `link` for web links; `draft` for unsent drafts.
//!
//! Two palettes, dark and light, both muted rather than saturated; the
//! terminal's background shows through (nothing paints a page colour).
//! Terminals without 24-bit colour get the nearest xterm-256 colours.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::{Color, Style};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThemeMode {
    /// Dark or light from the terminal's `COLORFGBG`, dark if unknown.
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(ThemeMode::Auto),
            "dark" => Some(ThemeMode::Dark),
            "light" => Some(ThemeMode::Light),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::Auto => "auto",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Auto => "Automatic",
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }
    pub fn next(self) -> Self {
        match self {
            ThemeMode::Auto => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub accent_soft: Color,
    pub surface: Color,
    pub surface_hi: Color,
    pub link: Color,
    pub warn: Color,
    pub on_warn: Color,
    pub danger: Color,
    pub on_danger: Color,
    pub draft: Color,
    /// Other people's names and bubble borders, picked per mid.
    pub senders: [Color; 6],
    /// Monogram tiles (white text on them), picked per mid.
    pub tiles: [Color; 8],
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// Tiles are mid-tones that carry white text on either background.
const TILES: [Color; 8] = [
    rgb(0x3d6a8f),
    rgb(0x2f7a6d),
    rgb(0x5b5f9a),
    rgb(0x667a3a),
    rgb(0x80566e),
    rgb(0x8f5a45),
    rgb(0x765089),
    rgb(0x86692f),
];

const DARK: Theme = Theme {
    text: rgb(0xe6e9ef),
    muted: rgb(0x8b93a3),
    border: rgb(0x4a5263),
    accent: rgb(0x5ec29a),
    on_accent: rgb(0x0e1a14),
    accent_soft: rgb(0x23423a),
    surface: rgb(0x252a33),
    surface_hi: rgb(0x2f3542),
    link: rgb(0x7aa7f0),
    warn: rgb(0xe3bd72),
    on_warn: rgb(0x1c1608),
    danger: rgb(0xe2717a),
    on_danger: rgb(0x1c0d0f),
    draft: rgb(0xbf96e6),
    senders: [
        rgb(0x6cb0ee),
        rgb(0xc58ae0),
        rgb(0xe3bd72),
        rgb(0x5cc0c8),
        rgb(0xe8838b),
        rgb(0xdb9d68),
    ],
    tiles: TILES,
};

const LIGHT: Theme = Theme {
    text: rgb(0x1f2328),
    muted: rgb(0x656d7a),
    border: rgb(0xc3c9d2),
    accent: rgb(0x14774f),
    on_accent: rgb(0xffffff),
    accent_soft: rgb(0xd5ebdf),
    surface: rgb(0xeef1f4),
    surface_hi: rgb(0xe1e6ec),
    link: rgb(0x2a5bc7),
    warn: rgb(0x8a5a00),
    on_warn: rgb(0x2e2300),
    danger: rgb(0xbf2a37),
    on_danger: rgb(0xffffff),
    draft: rgb(0x7848c9),
    senders: [
        rgb(0x1f65c0),
        rgb(0x7b45c4),
        rgb(0x8a5a00),
        rgb(0x16737b),
        rgb(0xbb2531),
        rgb(0xa8470c),
    ],
    tiles: TILES,
};

// The badge on `warn` needs a lighter fill on light backgrounds than the
// text colour `warn` is.
const LIGHT_WARN_FILL: Color = rgb(0xf3d27a);

struct Palettes {
    dark: Theme,
    light: Theme,
    light_warn_fill: Color,
}

static PALETTES: OnceLock<Palettes> = OnceLock::new();
static LIGHT_ON: AtomicBool = AtomicBool::new(false);

fn palettes() -> &'static Palettes {
    PALETTES.get_or_init(|| {
        let truecolor = std::env::var("COLORTERM")
            .is_ok_and(|v| v.eq_ignore_ascii_case("truecolor") || v.eq_ignore_ascii_case("24bit"));
        let fit = |c: Color| if truecolor { c } else { nearest_256(c) };
        let adapt = |t: &Theme| Theme {
            text: fit(t.text),
            muted: fit(t.muted),
            border: fit(t.border),
            accent: fit(t.accent),
            on_accent: fit(t.on_accent),
            accent_soft: fit(t.accent_soft),
            surface: fit(t.surface),
            surface_hi: fit(t.surface_hi),
            link: fit(t.link),
            warn: fit(t.warn),
            on_warn: fit(t.on_warn),
            danger: fit(t.danger),
            on_danger: fit(t.on_danger),
            draft: fit(t.draft),
            senders: t.senders.map(fit),
            tiles: t.tiles.map(fit),
        };
        Palettes {
            dark: adapt(&DARK),
            light: adapt(&LIGHT),
            light_warn_fill: fit(LIGHT_WARN_FILL),
        }
    })
}

/// The palette in use.
pub fn t() -> &'static Theme {
    let p = palettes();
    if LIGHT_ON.load(Ordering::Relaxed) {
        &p.light
    } else {
        &p.dark
    }
}

/// Use `mode` from now on (the next frame draws with it).
pub fn apply(mode: ThemeMode) {
    let light = match mode {
        ThemeMode::Dark => false,
        ThemeMode::Light => true,
        ThemeMode::Auto => std::env::var("COLORFGBG")
            .ok()
            .and_then(|v| bg_is_light(&v))
            .unwrap_or(false),
    };
    LIGHT_ON.store(light, Ordering::Relaxed);
}

/// `COLORFGBG` is `fg;bg` (sometimes `fg;default;bg`) in ANSI colour numbers.
fn bg_is_light(colorfgbg: &str) -> Option<bool> {
    let bg: u8 = colorfgbg.rsplit(';').next()?.trim().parse().ok()?;
    Some(matches!(bg, 7 | 9..=15))
}

/// Secondary text: times, hints, read previews.
pub fn muted() -> Style {
    Style::new().fg(t().muted)
}

/// Fill for a `warn` badge (text `on_warn`).
pub fn warn_fill() -> Color {
    let p = palettes();
    if LIGHT_ON.load(Ordering::Relaxed) {
        p.light_warn_fill
    } else {
        p.dark.warn
    }
}

/// The xterm-256 colour closest to `c` (6×6×6 cube or grey ramp).
fn nearest_256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c;
    };
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let step = |v: u8| {
        (0..6)
            .min_by_key(|&i| (i32::from(STEPS[i]) - i32::from(v)).abs())
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (step(r), step(g), step(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gi_ramp = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let grey = 8 + 10 * gi_ramp;
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if dist((grey, grey, grey)) < dist(cube) {
        Color::Indexed(232 + gi_ramp)
    } else {
        Color::Indexed(16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance contrast of two colours.
    fn contrast(a: Color, b: Color) -> f64 {
        let lum = |c: Color| {
            let Color::Rgb(r, g, b) = c else { panic!("{c:?}") };
            let ch = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_roles_are_readable_on_their_background() {
        // Typical terminal backgrounds for each palette.
        for (t, page) in [(&DARK, rgb(0x1a1d23)), (&LIGHT, rgb(0xffffff))] {
            for (role, c) in [
                ("text", t.text),
                ("accent", t.accent),
                ("link", t.link),
                ("warn", t.warn),
                ("danger", t.danger),
                ("draft", t.draft),
            ]
            .into_iter()
            .chain(t.senders.iter().map(|&c| ("sender", c)))
            {
                assert!(contrast(c, page) >= 4.5, "{role} {c:?} on {page:?}");
                assert!(
                    contrast(c, t.surface_hi) >= 3.0,
                    "{role} {c:?} on the selected row"
                );
            }
            assert!(contrast(t.muted, page) >= 3.0, "muted on {page:?}");
            assert!(contrast(t.on_accent, t.accent) >= 4.5);
            assert!(contrast(t.on_danger, t.danger) >= 4.5);
            for tile in t.tiles {
                assert!(contrast(rgb(0xffffff), tile) >= 4.5, "{tile:?}");
            }
        }
        assert!(contrast(DARK.on_warn, DARK.warn) >= 4.5);
        assert!(contrast(LIGHT.on_warn, LIGHT_WARN_FILL) >= 4.5);
    }

    #[test]
    fn colorfgbg_tells_light_from_dark() {
        assert_eq!(bg_is_light("15;0"), Some(false));
        assert_eq!(bg_is_light("0;15"), Some(true));
        assert_eq!(bg_is_light("0;default;7"), Some(true));
        assert_eq!(bg_is_light("garbage"), None);
    }

    #[test]
    fn without_truecolor_colours_map_into_the_256_palette() {
        assert_eq!(nearest_256(rgb(0x000000)), Color::Indexed(16));
        assert_eq!(nearest_256(rgb(0xffffff)), Color::Indexed(231));
        assert_eq!(nearest_256(rgb(0x808080)), Color::Indexed(244));
        assert_eq!(nearest_256(Color::Reset), Color::Reset);
    }
}
