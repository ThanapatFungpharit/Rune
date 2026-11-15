//! Small round profile pictures shown beside names: in message headers
//! (2×1 cells) and in the chat list (4×2 cells).

use crate::*;
use image::DynamicImage;
use rune::PictureSize;

/// Header avatar size, in cells (about square with 1:2 cells).
pub const HEAD_AVATAR: Size = Size::new(2, 1);
/// Chat-list avatar size, in cells.
pub const LIST_AVATAR: Size = Size::new(4, 2);

/// One picture per size drawn, so each keeps its own encoding and neither
/// re-encodes every frame.
pub struct Avatar {
    head: Picture,
    list: Picture,
}

/// Load (from the store, else the CDN) and round a profile/group picture
/// off the UI thread.
pub fn spawn_avatar(
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    store: Store,
    mid: String,
    path: String,
) {
    tokio::spawn(async move {
        let img = match cached_picture(&client, &store, &path, PictureSize::Small).await {
            Some((b, _)) => tokio::task::spawn_blocking(move || {
                image::load_from_memory(&b).ok().map(|i| {
                    let r = profile::round_avatar(&i);
                    // Plenty for a few cells; keeps encoding cheap.
                    r.resize(96, 96, FilterType::Lanczos3)
                })
            })
            .await
            .ok()
            .flatten(),
            None => None,
        };
        let _ = tx.send(AppEvent::Avatar(mid, img));
    });
}

impl App {
    /// The avatar for `mid` if it is ready; queues the download (see
    /// `App::start_media`) while a mid with a known picture is asked for. A mid not looked up this
    /// run is queued for the next lookup round (its stored picture may be
    /// stale, or missing), so every view gets pictures the same way.
    pub fn avatar_ready(&mut self, mid: &str) -> bool {
        if !self.looked_up.contains(mid) {
            self.name_misses.get_mut().insert(mid.to_owned());
        }
        match self.avatars.get(mid) {
            Some(Load::Ready(_)) => true,
            Some(_) => false,
            None => {
                if let Some(path) = self.picture_paths.get(mid).cloned() {
                    // Started after the frame, by how near the screen it is drawn.
                    self.media.want(
                        MediaKey::Avatar(mid.to_owned()),
                        fetchq::UNPLACED,
                        MediaStart::Avatar(path),
                    );
                }
                false
            }
        }
    }

    pub fn on_avatar(&mut self, mid: String, img: Option<DynamicImage>) {
        self.media.done(&MediaKey::Avatar(mid.clone()));
        let slot = match img {
            Some(i) => {
                self.loaded(&Job::Avatar(mid.clone()));
                Load::Ready(Avatar {
                    head: Picture::new(i.clone(), self.picker.as_ref()),
                    list: Picture::new(i, self.picker.as_ref()),
                })
            }
            None => {
                // Not fetched or not decodable: tried again while it is drawn.
                self.retry_later(Job::Avatar(mid.clone()));
                Load::Failed("no picture".into())
            }
        };
        self.avatars.insert(mid, slot);
    }

    /// Draw `mid`'s avatar at `size` with its top-left cell at `at`.
    pub fn draw_avatar(&mut self, f: &mut Frame, mid: &str, at: Position, size: Size, clip: Rect) {
        let mode = self.cell_mode;
        let Some(Load::Ready(a)) = self.avatars.get_mut(mid) else {
            return;
        };
        let rect = Rect::new(at.x, at.y, size.width, size.height).intersection(clip);
        if rect.width < size.width {
            return;
        }
        let skip = 0;
        let pic = if size == LIST_AVATAR {
            &mut a.list
        } else {
            &mut a.head
        };
        pic.render(f, rect, size, skip, mode, FilterType::Lanczos3);
    }
}
