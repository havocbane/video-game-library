//! Keeps a copy of every cover image in `<data dir>/covers`, so covers load
//! offline and survive a store changing or removing its image URL.
//!
//! The UI loads covers through the `cover://` protocol (see `main.rs`). A cover
//! that isn't cached yet is downloaded on first view, and `warm` downloads the
//! rest of the library in the background. Files are named by a hash of the
//! cover URL, so changing a game's cover simply points it at a new file;
//! `prune` deletes files no game uses any more.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Larger files aren't box art; refuse them rather than fill the disk.
const MAX_BYTES: usize = 15 * 1024 * 1024;
/// Downloads at once while warming, to be gentle with the image CDNs.
const WORKERS: usize = 4;
/// A partial download this old was abandoned (the app closed mid-download).
const STALE_TMP: Duration = Duration::from_secs(10 * 60);

pub struct CoverCache {
    dir: PathBuf,
    http: reqwest::Client,
    /// URLs being warmed right now, so overlapping warm runs don't fetch twice.
    in_flight: Mutex<HashSet<String>>,
}

pub struct Cover {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

impl CoverCache {
    pub fn new(dir: PathBuf, http: reqwest::Client) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, http, in_flight: Mutex::default() })
    }

    /// Returns the cached cover for `url`, downloading it first if needed.
    pub async fn get(&self, url: &str) -> Result<Cover> {
        if let Ok(bytes) = std::fs::read(self.path_for(url)) {
            if let Some(mime) = sniff(&bytes) {
                return Ok(Cover { bytes, mime });
            }
        }
        self.download(url).await
    }

    /// Downloads every cover in `urls` that isn't cached yet, a few at a time.
    pub async fn warm(self: &Arc<Self>, urls: Vec<String>) {
        let queue: Vec<String> = {
            let mut in_flight = self.in_flight.lock().unwrap();
            urls.into_iter()
                .filter(|u| cacheable(u) && !self.path_for(u).exists())
                .filter(|u| in_flight.insert(u.clone()))
                .collect()
        };
        if queue.is_empty() {
            return;
        }
        let queue = Arc::new(Mutex::new(queue));
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                let (cache, queue) = (Arc::clone(self), Arc::clone(&queue));
                tauri::async_runtime::spawn(async move {
                    loop {
                        let Some(url) = queue.lock().unwrap().pop() else { break };
                        // A cover that fails now is retried when it's next shown.
                        let _ = cache.download(&url).await;
                        cache.in_flight.lock().unwrap().remove(&url);
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.await;
        }
    }

    /// Deletes cached files that none of `urls` points to.
    pub fn prune(&self, urls: &[String]) {
        let keep: HashSet<String> = urls.iter().map(|u| file_name(u)).collect();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let unused = if name.ends_with(".tmp") {
                is_older_than(&entry.path(), STALE_TMP)
            } else {
                !keep.contains(&name)
            };
            if unused {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    async fn download(&self, url: &str) -> Result<Cover> {
        if !cacheable(url) {
            return Err(Error::msg("not a web image"));
        }
        let mut response = self.http.get(url).send().await?.error_for_status()?;
        if response.content_length().is_some_and(|n| n as usize > MAX_BYTES) {
            return Err(Error::msg("image is too large"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_BYTES {
                return Err(Error::msg("image is too large"));
            }
        }
        // Don't trust the Content-Type header: some CDNs send error pages as 200s.
        let mime = sniff(&bytes).ok_or_else(|| Error::msg("not an image"))?;

        // Write under a unique temporary name and rename, so a reader never sees
        // half a file and two downloads of the same cover can't collide.
        let path = self.path_for(url);
        let tmp = path.with_extension(format!("{}.tmp", unique_suffix()));
        std::fs::write(&tmp, &bytes)?;
        if std::fs::rename(&tmp, &path).is_err() {
            // Another download finished first (and may be open for reading).
            let _ = std::fs::remove_file(&tmp);
        }
        Ok(Cover { bytes, mime })
    }

    fn path_for(&self, url: &str) -> PathBuf {
        self.dir.join(file_name(url))
    }
}

/// Only web images are cached; anything else (a typo, a local path) is left to the browser.
fn cacheable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn file_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// Identifies the image formats browsers display, from the file's first bytes.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let mime = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [_, _, _, _, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', ..] => "image/avif",
        [b'B', b'M', ..] => "image/bmp",
        _ => return None,
    };
    Some(mime)
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn is_older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() > age)
}

/// Undoes the percent-encoding the UI applies to put a URL in a `cover://` path.
pub fn decode_path(path: &str) -> Option<String> {
    let path = path.strip_prefix('/').unwrap_or(path).as_bytes();
    let mut out = Vec::with_capacity(path.len());
    let mut i = 0;
    while i < path.len() {
        if path[i] == b'%' {
            let hex = std::str::from_utf8(path.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(path[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_ui_encoded_urls() {
        // What encodeURIComponent produces for a typical cover URL.
        let encoded = "/https%3A%2F%2Fcdn.example.com%2Fa%20b%2Fcover.jpg%3Fv%3D2%26t%3D%C3%A9";
        assert_eq!(decode_path(encoded).as_deref(), Some("https://cdn.example.com/a b/cover.jpg?v=2&t=é"));
        assert_eq!(decode_path("/bad%2"), None);
        assert_eq!(decode_path("/bad%zz"), None);
    }

    #[test]
    fn recognizes_images_only() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0...."), Some("image/jpeg"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"<!DOCTYPE html>"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn names_are_stable_and_distinct() {
        assert_eq!(file_name("https://a/1.jpg"), file_name("https://a/1.jpg"));
        assert_ne!(file_name("https://a/1.jpg"), file_name("https://a/2.jpg"));
        assert_eq!(file_name("https://a/1.jpg").len(), 32);
        assert!(cacheable("HTTPS://a/1.jpg"));
        assert!(!cacheable("C:\\covers\\1.jpg"));
        assert!(!cacheable("data:image/png;base64,AAAA"));
    }

    #[test]
    fn prune_keeps_covers_in_use() {
        let dir = std::env::temp_dir().join(format!("vgl-cover-test-{}", unique_suffix()));
        let cache = CoverCache::new(dir.clone(), reqwest::Client::new()).unwrap();
        let (kept, dropped) = ("https://a/kept.jpg".to_string(), "https://a/old.jpg");
        std::fs::write(cache.path_for(&kept), b"x").unwrap();
        std::fs::write(cache.path_for(dropped), b"x").unwrap();
        let fresh_tmp = dir.join("abc.1-1.tmp");
        std::fs::write(&fresh_tmp, b"x").unwrap();

        cache.prune(&[kept.clone()]);
        assert!(cache.path_for(&kept).exists());
        assert!(!cache.path_for(dropped).exists());
        assert!(fresh_tmp.exists(), "a download in progress isn't removed");
        let _ = std::fs::remove_dir_all(dir);
    }
}
