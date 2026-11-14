//! Animations (GIF, APNG — animated stickers are APNGs — and animated WebP)
//! played inside the viewer: a thread decodes one frame at a time and hands
//! it over to replace the picture on screen on a fixed 12 fps tick, and starts
//! decoding again from the top after the last frame, until it is stopped.
//! Frames keep their own timing on that grid (faster ones are dropped).
//! Only the newest frame is kept, so a slow terminal skips frames instead of
//! queueing them.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, Frames};
use tokio::sync::mpsc;

use crate::{AppEvent, MAX_PICTURE};

/// Browsers show frames with a delay of 10 ms or less for 100 ms.
const DEFAULT_DELAY: Duration = Duration::from_millis(100);
/// How often a waiting player checks whether it was stopped or paused.
const POLL: Duration = Duration::from_millis(25);

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Set once a second frame was decoded: it really is an animation.
    animated: AtomicBool,
    /// The newest decoded frame, not yet drawn.
    frame: Mutex<Option<DynamicImage>>,
}

/// A running animation for one message; dropping it stops the thread.
pub struct Player {
    pub id: String,
    shared: Arc<Shared>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Player {
    /// Start playing `data` (any image; stills end after their only frame)
    /// for message `id`. Every new frame posts `AppEvent::AnimFrame(id)`.
    pub fn start(id: String, data: Vec<u8>, tx: mpsc::UnboundedSender<AppEvent>) -> Player {
        let shared = Arc::new(Shared::default());
        let (s, msg) = (shared.clone(), id.clone());
        std::thread::spawn(move || play(&data, &s, || tx.send(AppEvent::AnimFrame(msg.clone())).is_ok()));
        Player { id, shared }
    }

    /// The newest frame, if one arrived since the last call.
    pub fn take_frame(&self) -> Option<DynamicImage> {
        self.shared.frame.lock().ok()?.take()
    }

    /// More than one frame: worth showing pause controls for.
    pub fn is_animated(&self) -> bool {
        self.shared.animated.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn toggle_pause(&self) {
        self.shared.paused.fetch_xor(true, Ordering::Relaxed);
    }
}

/// Could `data` hold an animation (by its magic bytes)?
pub fn may_animate(data: &[u8]) -> bool {
    matches!(
        data,
        [b'G', b'I', b'F', ..]
            | [0x89, b'P', b'N', b'G', ..]
            | [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..]
    )
}

/// A frame decoder for `data`, or `None` for a still or unknown format.
fn frames(data: &[u8]) -> Option<Frames<'_>> {
    match data {
        [b'G', b'I', b'F', ..] => Some(GifDecoder::new(Cursor::new(data)).ok()?.into_frames()),
        [0x89, b'P', b'N', b'G', ..] => {
            let d = PngDecoder::new(Cursor::new(data)).ok()?;
            if !d.is_apng().ok()? {
                return None;
            }
            Some(d.apng().ok()?.into_frames())
        }
        [b'R', b'I', b'F', b'F', ..] => {
            let d = WebPDecoder::new(Cursor::new(data)).ok()?;
            if !d.has_animation() {
                return None;
            }
            Some(d.into_frames())
        }
        _ => None,
    }
}

/// Frames reach the screen on a fixed grid of this many per second.
pub const FPS: u32 = 12;

/// The grid tick a frame shown from `at` for `delay` (animation time) is
/// handed over on: the first tick at or after `at`, or `None` when the frame
/// is over before that tick (a frame shorter than a tick is dropped, so the
/// animation keeps its speed).
fn tick_for(at: Duration, delay: Duration) -> Option<Duration> {
    const SEC: u128 = 1_000_000_000;
    let k = (at.as_nanos() * u128::from(FPS)).div_ceil(SEC);
    let tick = Duration::from_nanos(u64::try_from(k * SEC / u128::from(FPS)).ok()?);
    (tick < at + delay).then_some(tick)
}

/// Sleep until `base + at`, in short slices; false once stopped. Time spent
/// paused moves `base` on, so the animation resumes where it was.
fn wait(s: &Shared, base: &mut Instant, at: Duration) -> bool {
    loop {
        if s.stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if s.paused.load(Ordering::Relaxed) {
            std::thread::sleep(POLL);
            *base += Instant::now() - now;
            continue;
        }
        let until = *base + at;
        if now >= until {
            return true;
        }
        std::thread::sleep((until - now).min(POLL));
    }
}

/// Decode → hand over on the next 12 fps tick → decode the next one, from
/// the top again after the last frame. Frames keep their own timing: each
/// shows on the first tick inside its time slot, frames whose slot holds no
/// tick (or that decoding fell behind on) are skipped. Returns when stopped,
/// when `notify` fails (the app is gone), or at once for a still.
fn play(data: &[u8], s: &Shared, notify: impl Fn() -> bool) {
    let mut base = Instant::now();
    // Position on the animation's timeline; it runs on across rounds so the
    // tick grid stays even.
    let mut at = Duration::ZERO;
    loop {
        let Some(decoder) = frames(data) else { return };
        let mut decoded = 0usize;
        for frame in decoder {
            if s.stop.load(Ordering::Relaxed) {
                return;
            }
            // A broken frame ends this round; the next round starts over.
            let Ok(frame) = frame else { break };
            let (num, den) = frame.delay().numer_denom_ms();
            let ms = num.checked_div(den).unwrap_or(0);
            let delay = if ms <= 10 {
                DEFAULT_DELAY
            } else {
                Duration::from_millis(u64::from(ms))
            };
            decoded += 1;
            if decoded == 2 {
                s.animated.store(true, Ordering::Relaxed);
            }
            let start = at;
            at += delay;
            // The first round's first frame is already on screen (the still).
            if !s.animated.load(Ordering::Relaxed) {
                continue;
            }
            let Some(tick) = tick_for(start, delay) else {
                continue;
            };
            // Decoding fell behind: this frame's slot is already over.
            if Instant::now() >= base + at {
                continue;
            }
            let mut img = DynamicImage::ImageRgba8(frame.into_buffer());
            if img.width() > MAX_PICTURE || img.height() > MAX_PICTURE {
                img = img.resize(MAX_PICTURE, MAX_PICTURE, FilterType::Triangle);
            }
            if !wait(s, &mut base, tick) {
                return;
            }
            if let Ok(mut slot) = s.frame.lock() {
                *slot = Some(img);
            }
            if !notify() {
                return;
            }
        }
        // No frame, or a single one: nothing to animate.
        if !s.animated.load(Ordering::Relaxed) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::sync::atomic::AtomicUsize;

    fn gif(n: usize, ms: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for i in 0..n {
                let px = Rgba([(i * 80) as u8, 0, 0, 255]);
                let f = Frame::from_parts(
                    RgbaImage::from_pixel(4, 4, px),
                    0,
                    0,
                    Delay::from_numer_denom_ms(ms, 1),
                );
                enc.encode_frame(f).unwrap();
            }
        }
        out
    }

    #[test]
    fn stills_and_unknown_data_do_not_play() {
        assert!(frames(b"not an image").is_none());
        let mut png = Vec::new();
        RgbaImage::new(2, 2)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(
            may_animate(&png) && frames(&png).is_none(),
            "plain PNG is a still"
        );
        let s = Shared::default();
        let calls = AtomicUsize::new(0);
        play(&gif(1, 50), &s, || calls.fetch_add(1, Ordering::Relaxed) < 100);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "a one-frame GIF never replaces the still"
        );
        assert!(!s.animated.load(Ordering::Relaxed));
    }

    #[test]
    fn frames_land_on_the_12_fps_grid() {
        let ms = Duration::from_millis;
        assert_eq!(tick_for(Duration::ZERO, ms(100)), Some(Duration::ZERO));
        // 1/12 s = 83.33 ms: a 100 ms frame from 50 ms shows on tick 1.
        assert_eq!(tick_for(ms(50), ms(100)), Some(Duration::from_nanos(83_333_333)));
        // A 20 ms frame from 20 ms (or 40 ms) is over before tick 1.
        assert_eq!(tick_for(ms(20), ms(20)), None);
        assert_eq!(tick_for(ms(40), ms(20)), None);
        assert_eq!(tick_for(ms(80), ms(20)), Some(Duration::from_nanos(83_333_333)));
    }

    #[test]
    fn frames_replace_each_other_and_loop() {
        let s = Shared::default();
        let calls = AtomicUsize::new(0);
        let started = Instant::now();
        // 3 frames of 100 ms (slower than a tick): every frame shows, round
        // after round; stop after 7 hand-overs, i.e. into the third round.
        play(&gif(3, 100), &s, || {
            let n = calls.fetch_add(1, Ordering::Relaxed) + 1;
            assert!(
                s.frame.lock().unwrap().take().is_some(),
                "each notice carries a frame"
            );
            n < 7
        });
        assert_eq!(calls.load(Ordering::Relaxed), 7);
        assert!(s.animated.load(Ordering::Relaxed));
        assert!(
            started.elapsed() >= Duration::from_millis(700),
            "frame delays are kept"
        );
    }

    #[test]
    fn fast_animations_are_capped_at_12_fps() {
        let s = Shared::default();
        let times = Mutex::new(Vec::new());
        // 50 fps GIF: hand-overs still come at most every 1/12 s.
        play(&gif(5, 20), &s, || {
            let mut t = times.lock().unwrap();
            t.push(Instant::now());
            t.len() < 8
        });
        let t = times.into_inner().unwrap();
        let span = t[t.len() - 1] - t[0];
        let tick = Duration::from_secs(1) / FPS;
        assert!(span >= tick * 6, "8 frames over {span:?}: faster than {FPS} fps");
    }

    #[test]
    fn stop_ends_the_thread() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let p = Player::start("m".into(), gif(2, 20), tx);
        assert!(matches!(rx.blocking_recv(), Some(AppEvent::AnimFrame(id)) if id == "m"));
        assert!(p.take_frame().is_some() && p.is_animated());
        drop(p);
        // The thread drops its sender once it notices the stop.
        while rx.blocking_recv().is_some() {}
    }
}
