//! Cover art, off the render path (SCOPE section 5).
//!
//! A panel asks for an album's cover every frame; fetching one can mean reading a JPEG off
//! a spinning disk and decoding it. So a miss enqueues a request, the panel draws a
//! placeholder, and the decoded thumbnail arrives on a later frame. One worker thread owns
//! its own database connection (WAL: safe alongside the UI's), and the cache keeps at most
//! `CAPACITY` thumbnails in memory so a 18k-album library cannot eat the budget.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use fbp_index::{album_key, Art, Library};

const CAPACITY: usize = 64;

struct Request {
    key: String,
    albumartist: Option<String>,
    album: String,
}

pub struct ArtCache {
    requests: Sender<Request>,
    results: Receiver<(String, Option<Art>)>,
    memory: HashMap<String, Option<Arc<Art>>>,
    pending: HashSet<String>,
    order: VecDeque<String>,
    enabled: bool,
    fetched: usize,
    missing: usize,
}

impl ArtCache {
    pub fn spawn(db: PathBuf, enabled: bool) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (result_tx, result_rx) = channel::<(String, Option<Art>)>();

        std::thread::spawn(move || {
            let library = match Library::open(&db) {
                Ok(library) => library,
                Err(_) => return,
            };
            while let Ok(request) = request_rx.recv() {
                let art = library
                    .fetch_album_art(request.albumartist.as_deref(), &request.album)
                    .ok()
                    .flatten();
                if result_tx.send((request.key, art)).is_err() {
                    break;
                }
            }
        });

        Self {
            requests: request_tx,
            results: result_rx,
            memory: HashMap::new(),
            pending: HashSet::new(),
            order: VecDeque::new(),
            enabled,
            fetched: 0,
            missing: 0,
        }
    }

    /// The cover for an album, or None while it is being fetched (or if there is none).
    pub fn get(&mut self, albumartist: Option<&str>, album: &str) -> Option<Arc<Art>> {
        let key = album_key(albumartist, album);
        if let Some(entry) = self.memory.get(&key) {
            return entry.clone();
        }
        if self.enabled && self.pending.insert(key.clone()) {
            let request = Request {
                key,
                albumartist: albumartist.map(|v| v.to_string()),
                album: album.to_string(),
            };
            let _ = self.requests.send(request);
        }
        None
    }

    /// Drain finished fetches. Returns true when anything arrived, so the caller can redraw.
    pub fn poll(&mut self) -> bool {
        let mut arrived = false;
        while let Ok((key, art)) = self.results.try_recv() {
            arrived = true;
            self.pending.remove(&key);
            if art.is_some() {
                self.fetched += 1;
            } else {
                self.missing += 1;
            }
            self.order.push_back(key.clone());
            self.memory.insert(key, art.map(Arc::new));
        }
        while self.order.len() > CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                if !self.pending.contains(&oldest) {
                    self.memory.remove(&oldest);
                }
            }
        }
        arrived
    }

    /// Is this album already resolved (art or a known miss)?
    #[cfg(feature = "shots")]
    pub fn contains(&self, albumartist: Option<&str>, album: &str) -> bool {
        self.memory.contains_key(&album_key(albumartist, album))
    }

    /// Insert a result fetched synchronously -- used by the shot path, which cannot wait for
    /// a frame to arrive.
    #[cfg(feature = "shots")]
    pub fn insert(&mut self, albumartist: Option<&str>, album: &str, art: Option<Art>) {
        let key = album_key(albumartist, album);
        self.pending.remove(&key);
        self.order.push_back(key.clone());
        self.memory.insert(key, art.map(Arc::new));
    }

    pub fn is_pending(&self, albumartist: Option<&str>, album: &str) -> bool {
        self.pending.contains(&album_key(albumartist, album))
    }
}
