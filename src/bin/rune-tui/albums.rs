//! Group albums. LINE's Chrome protocol cannot list albums, so this works
//! from the album posts a chat receives ("X added photos to <album>"): they
//! are grouped by album, and picking a photo shows it in the viewer. Album
//! posts are ordinary media messages otherwise (inline picture, `s`/`o`).

use super::*;

/// The media kind an album post stands for (`IMAGE`, or `VIDEO` for a video).
fn album_media_type(m: &Message) -> Option<i64> {
    let post = m.album_post()?;
    let first = post.media.first()?;
    Some(if first.is_video() { ct::VIDEO } else { ct::IMAGE })
}

/// The content type to treat a message's media as: album posts count as
/// pictures (or videos), everything else is what it says.
pub fn media_type(m: &Message) -> i64 {
    album_media_type(m).unwrap_or(m.content_type)
}

/// One album seen in a chat.
struct AlbumSeen {
    id: String,
    name: String,
    /// `(message id, sender, time, photos in the post)`, newest first.
    posts: Vec<(String, String, i64, usize)>,
}

impl App {
    /// Album posts of `chat`: loaded messages plus the stored history.
    fn albums_of(&self, chat: &str) -> Vec<AlbumSeen> {
        let mut seen: HashMap<String, (Message, i64)> = HashMap::new();
        for sm in self.store.messages(chat, 20_000, None).unwrap_or_default() {
            if sm.unsent {
                continue;
            }
            let t = sm.message.created_time.unwrap_or(0);
            seen.insert(sm.message.id.clone(), (sm.message, t));
        }
        for m in self.messages.get(chat).into_iter().flatten() {
            if let Some(raw) = m.raw.as_ref().filter(|_| !m.unsent) {
                seen.entry(m.id.clone()).or_insert_with(|| (raw.clone(), m.time));
            }
        }
        let mut posts: Vec<(rune::AlbumPost, String, String, i64)> = seen
            .into_iter()
            .filter_map(|(id, (m, t))| Some((m.album_post()?, id, m.from, t)))
            .collect();
        posts.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| message_id_cmp(&b.1, &a.1)));
        let mut albums: Vec<AlbumSeen> = Vec::new();
        for (p, id, from, t) in posts {
            let entry = match albums.iter().position(|a| a.id == p.album_id) {
                Some(i) => &mut albums[i],
                None => {
                    albums.push(AlbumSeen {
                        id: p.album_id.clone(),
                        name: String::new(),
                        posts: Vec::new(),
                    });
                    albums.last_mut().unwrap()
                }
            };
            // Newest first, so the first name seen is the current one.
            if entry.name.is_empty() {
                entry.name = p.album_name.clone();
            }
            entry.posts.push((id, from, t, p.media.len().max(1)));
        }
        albums
    }

    /// The chat menu's "Albums": every album the chat's history mentions.
    pub fn albums_menu(&mut self, chat: &str) {
        if self.open.as_deref() != Some(chat) {
            self.open_chat(chat.to_owned());
        }
        let albums = self.albums_of(chat);
        if albums.is_empty() {
            self.status = "no album posts in this chat's history (LINE's Chrome protocol cannot \
                           list albums; they show up here once someone adds photos)"
                .into();
            return;
        }
        let items = albums
            .into_iter()
            .map(|a| {
                let photos: usize = a.posts.iter().map(|p| p.3).sum();
                let newest = a.posts.first().map_or(0, |p| p.2);
                let name = if a.name.is_empty() {
                    "(unnamed album)".to_owned()
                } else {
                    a.name
                };
                (
                    format!(
                        "🖼 {} · {photos} photo{} · {}",
                        ellipsize(&name, 36),
                        if photos == 1 { "" } else { "s" },
                        chat_time(newest)
                    ),
                    Action::AlbumPhotos(chat.to_owned(), a.id),
                )
            })
            .collect();
        self.open_menu(" Albums ", items);
    }

    /// The posts of one album; picking one opens it in the viewer.
    pub fn album_photos_menu(&mut self, chat: &str, album_id: &str) {
        let Some(album) = self.albums_of(chat).into_iter().find(|a| a.id == album_id) else {
            self.status = "that album is no longer in the history".into();
            return;
        };
        let items = album
            .posts
            .iter()
            .map(|(id, from, t, n)| {
                let more = if *n > 1 {
                    format!(" (+{} not shown)", n - 1)
                } else {
                    String::new()
                };
                (
                    format!("📷 {} · {}{more}", self.name(from), hhmm(*t)),
                    Action::ViewMessage(chat.to_owned(), id.clone()),
                )
            })
            .collect();
        let title = if album.name.is_empty() {
            " Album ".to_owned()
        } else {
            format!(" {} ", ellipsize(&album.name, 40))
        };
        self.open_menu(title, items);
    }

    /// Select a message of `chat` (loading older stored history if needed)
    /// and show its media in the viewer.
    pub fn view_message(&mut self, chat: &str, id: &str) {
        if self.open.as_deref() != Some(chat) {
            self.open_chat(chat.to_owned());
        }
        if !self.open_messages().iter().any(|m| m.id == id) {
            let oldest = self.open_messages().first().map(|m| m.id.clone());
            let older: Vec<UiMsg> = self
                .store
                .messages(chat, 20_000, oldest.as_deref())
                .unwrap_or_default()
                .iter()
                .filter_map(stored_to_ui)
                .collect();
            // Keep the history contiguous: everything from that message on.
            if let Some(i) = older.iter().position(|m| m.id == id) {
                self.prepend(chat, older[i..].to_vec());
            }
        }
        match self.open_messages().iter().position(|m| m.id == id) {
            Some(i) => {
                self.select_msg(Some(i));
                self.open_viewer();
            }
            None => self.status = "that photo's message is not in the loaded history".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_posts_count_as_pictures() {
        let mut m = Message {
            content_type: 16,
            ..Default::default()
        };
        for (k, v) in [
            ("serviceType", "AB"),
            ("albumId", "1"),
            ("albumName", "Trip"),
            ("locKey", "BT"),
            ("mediaOid", "o1"),
            ("mediaType", "I"),
        ] {
            m.content_metadata.insert(k.into(), v.into());
        }
        assert_eq!(media_type(&m), ct::IMAGE);
        m.content_metadata.insert("mediaType".into(), "V".into());
        assert_eq!(media_type(&m), ct::VIDEO);
        m.content_metadata.insert("serviceType".into(), "GB".into());
        assert_eq!(media_type(&m), 16);
    }
}
