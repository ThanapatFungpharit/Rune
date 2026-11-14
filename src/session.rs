//! Saving and restoring a logged-in session.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::auth::E2eeKey;
use crate::client::Client;
use crate::error::Result;
use crate::transport::{Config, Tokens};

/// Everything needed to resume without scanning a QR code again.
///
/// Contains the access/refresh tokens and Letter Sealing private keys —
/// store it like a password. [`Session::save`] writes the file with
/// owner-only permissions on Unix.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub tokens: Tokens,
    #[serde(default)]
    pub e2ee_keys: Vec<E2eeKey>,
}

impl Session {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    /// Write atomically (temp file + rename), mode 0600 on Unix.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        // Unique temp name: concurrent saves (e.g. a token-refresh watcher and
        // the main task) must not rename each other's file away.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{}.{n}.tmp", std::process::id()));
        let tmp = path.with_file_name(name);
        let data = serde_json::to_vec_pretty(self)?;
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            use std::io::Write;
            let mut f = opts.open(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    /// Snapshot a client's current tokens and Letter Sealing keys.
    pub fn from_client(client: &Client) -> Self {
        Session {
            tokens: client.tokens(),
            e2ee_keys: client.e2ee_keys(),
        }
    }

    /// Build a client from this session (tokens + Letter Sealing keys).
    pub fn into_client(self, config: Config) -> Result<Client> {
        let client = Client::with_tokens(config, self.tokens)?;
        client.load_e2ee_keys(&self.e2ee_keys)?;
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_saves_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("rune-session-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut s = Session::default();
                    s.tokens.mid = Some(format!("u{i}"));
                    s.save(&path).unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(Session::load(&path).unwrap().tokens.mid.is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1, "temp files left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
