//! Official card images (full art) from Scryfall, the standard Magic card database:
//! downloaded in a background thread (one at a time, as Scryfall asks), cached under
//! data/images so they work offline afterwards, and turned into textures on the UI thread.
//! Only if an image can't be fetched does the app draw a stand-in card face.

use crate::card::Card;
use eframe::egui;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

struct Job {
    key: String,
    /// Tried in order: by set + collector number, then by exact card name.
    urls: Vec<String>,
    cache: PathBuf,
}

pub struct ImageCache {
    textures: HashMap<String, TextureHandle>,
    requested: HashSet<String>,
    failed: HashSet<String>,
    jobs: Sender<Job>,
    done: Receiver<(String, Result<ColorImage, String>)>,
    dir: PathBuf,
    pub enabled: bool,
    pub last_error: Option<String>,
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Override for testing: MSH_IMAGE_BASE=http://127.0.0.1:8000 serves /{set}/{number}.jpg.
fn image_urls(set: &str, number: Option<&str>, name: &str) -> Vec<String> {
    if let Ok(base) = std::env::var("MSH_IMAGE_BASE") {
        return vec![format!(
            "{}/{}/{}.jpg",
            base.trim_end_matches('/'),
            set,
            number.unwrap_or("0")
        )];
    }
    let mut urls = Vec::new();
    if let Some(n) = number {
        urls.push(format!(
            "https://api.scryfall.com/cards/{}/{}?format=image&version=normal",
            set, n
        ));
    }
    urls.push(format!(
        "https://api.scryfall.com/cards/named?exact={}&format=image&version=normal",
        percent_encode(name)
    ));
    urls
}

fn decode(bytes: &[u8]) -> Result<ColorImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn fetch(job: &Job) -> Result<ColorImage, String> {
    if let Ok(bytes) = std::fs::read(&job.cache) {
        if let Ok(img) = decode(&bytes) {
            return Ok(img);
        }
    }
    let mut last_err = String::from("no image address");
    let mut bytes = None;
    for url in &job.urls {
        let attempt = ureq::get(url)
            .header("User-Agent", "MSHDeckbuilder/0.3 (personal deck builder)")
            .header("Accept", "image/*")
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut resp| {
                resp.body_mut()
                    .with_config()
                    .limit(8 * 1024 * 1024)
                    .read_to_vec()
                    .map_err(|e| e.to_string())
            });
        match attempt {
            Ok(b) => {
                bytes = Some(b);
                break;
            }
            Err(e) => last_err = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let bytes = bytes.ok_or(last_err)?;
    let img = decode(&bytes)?;
    if let Some(dir) = job.cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&job.cache, &bytes);
    Ok(img)
}

impl ImageCache {
    pub fn new(dir: &Path, ctx: &egui::Context, enabled: bool) -> ImageCache {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let cached = job.cache.exists();
                let result = fetch(&job);
                let _ = done_tx.send((job.key, result));
                ctx.request_repaint();
                if !cached {
                    // Scryfall asks for 50-100 ms between requests.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
        ImageCache {
            textures: HashMap::new(),
            requested: HashSet::new(),
            failed: HashSet::new(),
            jobs,
            done,
            dir: dir.to_path_buf(),
            enabled,
            last_error: None,
        }
    }

    /// Moves finished downloads into textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, result)) = self.done.try_recv() {
            match result {
                Ok(img) => {
                    let tex = ctx.load_texture(&key, img, TextureOptions::LINEAR);
                    self.textures.insert(key, tex);
                }
                Err(e) => {
                    self.failed.insert(key);
                    self.last_error = Some(e);
                }
            }
        }
    }

    fn key(card: &Card) -> (String, String) {
        let set = if card.set.is_empty() {
            "msh".to_string()
        } else {
            card.set.to_lowercase()
        };
        let id = match &card.collector_number {
            Some(n) => n.clone(),
            None => card
                .name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect(),
        };
        (set.clone(), format!("{}-{}", set, id))
    }

    fn request(&mut self, card: &Card) -> String {
        let (set, key) = Self::key(card);
        if !self.requested.contains(&key) && !self.failed.contains(&key) {
            self.requested.insert(key.clone());
            let _ = self.jobs.send(Job {
                urls: image_urls(&set, card.collector_number.as_deref(), &card.name),
                cache: self.dir.join(format!("{}.jpg", key)),
                key: key.clone(),
            });
        }
        key
    }

    /// The image for a card if it's ready; starts a download the first time it's asked for.
    pub fn get(&mut self, card: &Card) -> Option<&TextureHandle> {
        if !self.enabled {
            return None;
        }
        let key = self.request(card);
        self.textures.get(&key)
    }

    /// Queues downloads for many cards at once (e.g. the whole set before a draft).
    pub fn prefetch<'a>(&mut self, cards: impl Iterator<Item = &'a Card>) -> usize {
        let before = self.requested.len();
        for c in cards {
            self.request(c);
        }
        self.requested.len() - before
    }

    /// Downloads still waiting or in progress.
    pub fn pending(&self) -> usize {
        self.requested
            .len()
            .saturating_sub(self.textures.len() + self.failed.len())
    }

    pub fn loaded(&self) -> usize {
        self.textures.len()
    }

    pub fn failures(&self) -> usize {
        self.failed.len()
    }

    /// Lets failed pictures be tried again (e.g. after reconnecting).
    pub fn retry_failed(&mut self) {
        for k in self.failed.drain() {
            self.requested.remove(&k);
        }
        self.last_error = None;
    }
}
