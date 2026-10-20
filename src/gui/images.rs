//! Official card images (full art) from Scryfall, the standard Magic card database.
//!
//! * Each image is downloaded once (high-resolution "large" scan) and saved in the
//!   `images` folder next to the program as "Card Name (SET 123).jpg"; after that it is
//!   always read from disk, so the app works offline.
//! * Grid thumbnails are decoded at a reduced size on background threads and drawn with
//!   mipmaps, so they look smooth when small; the full-resolution image is only loaded for
//!   the card you're zooming into or inspecting.
//! * Only a few textures are sent to the GPU per frame, and least-recently-used images are
//!   released, so scrolling stays smooth even with the whole set loaded.
//! * If an image can't be fetched, the app draws a stand-in card face instead.
use eframe::egui;

use crate::card::Card;
use egui::{ColorImage, TextureFilter, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Width the grid thumbnails are decoded at (sharp up to ~300 px on screen).
const THUMB_WIDTH: u32 = 340;
/// Thumbnails kept on the GPU at once (~0.6 MB each).
const MAX_THUMBS: usize = 220;
/// Full-resolution images kept at once (~2.5 MB each).
const MAX_FULL: usize = 16;
/// Textures uploaded per frame, so a burst of finished images doesn't stall a frame.
const UPLOADS_PER_FRAME: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Want {
    Thumb,
    Full,
    /// Just make sure the file is on disk (used for "download everything" before a draft).
    DownloadOnly,
}

struct Job {
    key: String,
    want: Want,
    /// Tried in order: by set + collector number, then by exact card name.
    urls: Vec<String>,
    path: PathBuf,
}

type Done = (String, Want, Result<Option<ColorImage>, String>);

pub struct ImageCache {
    thumbs: HashMap<String, TextureHandle>,
    full: HashMap<String, TextureHandle>,
    full_order: VecDeque<String>,
    last_used: HashMap<String, u64>,
    frame: u64,
    requested: HashSet<(String, Want)>,
    failed: HashSet<String>,
    on_disk: HashSet<String>,
    ready: VecDeque<(String, Want, ColorImage)>,
    /// Downloads for cards on screen go first...
    net: Sender<Job>,
    /// ...ahead of background "download everything" jobs.
    net_bulk: Sender<Job>,
    disk: Sender<Job>,
    done: Receiver<Done>,
    dir: PathBuf,
    pub enabled: bool,
    pub last_error: Option<String>,
    downloads_queued: usize,
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
            "https://api.scryfall.com/cards/{}/{}?format=image&version=large",
            set, n
        ));
    }
    urls.push(format!(
        "https://api.scryfall.com/cards/named?exact={}&format=image&version=large",
        percent_encode(name)
    ));
    urls
}

/// "Red Hulk (MSH 149).jpg" - readable names so the folder is easy to browse.
pub fn file_name(card: &Card) -> String {
    let clean: String = card
        .name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let set = if card.set.is_empty() {
        "MSH"
    } else {
        &card.set
    };
    match &card.collector_number {
        Some(n) => format!("{} ({} {}).jpg", clean, set.to_uppercase(), n),
        None => format!("{} ({}).jpg", clean, set.to_uppercase()),
    }
}

/// Decodes, optionally shrinks with a high-quality filter, and rounds the corners.
fn decode(bytes: &[u8], want: Want) -> Result<ColorImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if want == Want::Thumb && img.width() > THUMB_WIDTH {
        let h = (img.height() as f32 * THUMB_WIDTH as f32 / img.width() as f32).round() as u32;
        img.resize_exact(THUMB_WIDTH, h, image::imageops::FilterType::CatmullRom)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    round_corners(&mut rgba);
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Makes the square corners of a scan transparent, with a soft edge, like a real card.
fn round_corners(img: &mut image::RgbaImage) {
    let (w, h) = (img.width(), img.height());
    let r = w as f32 * 0.045;
    let n = (r.ceil() as u32).min(w / 2).min(h / 2);
    // (corner circle centre, x range, y range) for the four corners.
    let corners = [
        (r, r, 0, 0),
        (w as f32 - r, r, w - n, 0),
        (r, h as f32 - r, 0, h - n),
        (w as f32 - r, h as f32 - r, w - n, h - n),
    ];
    for (cx, cy, x0, y0) in corners {
        for y in y0..y0 + n {
            for x in x0..x0 + n {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                // Only pixels beyond the centre on both axes are in the rounded-off corner.
                if (dx < 0.0) == (cx < w as f32 / 2.0) && (dy < 0.0) == (cy < h as f32 / 2.0) {
                    let a = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                    let p = img.get_pixel_mut(x, y);
                    p[3] = (p[3] as f32 * a) as u8;
                }
            }
        }
    }
}

fn download(job: &Job) -> Result<Vec<u8>, String> {
    let mut last_err = String::from("no image address");
    for url in &job.urls {
        let attempt = ureq::get(url)
            .header("User-Agent", "MSHDeckbuilder/0.4 (personal deck builder)")
            .header("Accept", "image/*")
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut resp| {
                resp.body_mut()
                    .with_config()
                    .limit(12 * 1024 * 1024)
                    .read_to_vec()
                    .map_err(|e| e.to_string())
            });
        match attempt {
            Ok(b) => return Ok(b),
            Err(e) => last_err = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(last_err)
}

fn save(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Write to a temp file first so a crash never leaves a half image behind.
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Linear,
        minification: TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        // Mipmaps keep shrunken scans smooth instead of grainy.
        mipmap_mode: Some(TextureFilter::Linear),
    }
}

impl ImageCache {
    pub fn new(dir: &Path, ctx: &egui::Context, enabled: bool) -> ImageCache {
        let (net, net_rx) = channel::<Job>();
        let (net_bulk, bulk_rx) = channel::<Job>();
        let (disk, disk_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();

        // One network thread: Scryfall asks for requests one at a time, 50-100 ms apart.
        {
            let done_tx = done_tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let job = match net_rx.try_recv() {
                        Ok(j) => j,
                        Err(_) => match bulk_rx.try_recv() {
                            Ok(j) => j,
                            Err(_) => {
                                match net_rx.recv_timeout(std::time::Duration::from_millis(150)) {
                                    Ok(j) => j,
                                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                    Err(_) => break,
                                }
                            }
                        },
                    };
                    let result = if job.path.exists() {
                        std::fs::read(&job.path).map_err(|e| e.to_string())
                    } else {
                        let r = download(&job);
                        if let Ok(bytes) = &r {
                            save(&job.path, bytes);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        r
                    };
                    let out = result.and_then(|bytes| match job.want {
                        Want::DownloadOnly => Ok(None),
                        w => decode(&bytes, w).map(Some),
                    });
                    let _ = done_tx.send((job.key, job.want, out));
                    ctx.request_repaint();
                }
            });
        }
        // A few disk threads: decoding saved images is CPU work, so spread it out.
        let disk_rx = Arc::new(Mutex::new(disk_rx));
        for _ in 0..3 {
            let rx = disk_rx.clone();
            let done_tx = done_tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let job = match rx.lock() {
                        Ok(r) => match r.recv() {
                            Ok(j) => j,
                            Err(_) => break,
                        },
                        Err(_) => break,
                    };
                    let out = std::fs::read(&job.path)
                        .map_err(|e| e.to_string())
                        .and_then(|b| decode(&b, job.want))
                        .map(Some);
                    let _ = done_tx.send((job.key, job.want, out));
                    ctx.request_repaint();
                }
            });
        }
        let on_disk = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.ends_with(".jpg"))
                    .collect()
            })
            .unwrap_or_default();
        ImageCache {
            thumbs: HashMap::new(),
            full: HashMap::new(),
            full_order: VecDeque::new(),
            last_used: HashMap::new(),
            frame: 0,
            requested: HashSet::new(),
            failed: HashSet::new(),
            on_disk,
            ready: VecDeque::new(),
            net,
            net_bulk,
            disk,
            done,
            dir: dir.to_path_buf(),
            enabled,
            last_error: None,
            downloads_queued: 0,
        }
    }

    /// Collects finished work and uploads a few textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((key, want, result)) = self.done.try_recv() {
            match result {
                Ok(Some(img)) => {
                    self.on_disk.insert(key.clone());
                    self.ready.push_back((key, want, img));
                }
                Ok(None) => {
                    self.on_disk.insert(key);
                    self.downloads_queued = self.downloads_queued.saturating_sub(1);
                }
                Err(e) => {
                    if want == Want::DownloadOnly {
                        self.downloads_queued = self.downloads_queued.saturating_sub(1);
                    }
                    self.failed.insert(key);
                    self.last_error = Some(e);
                }
            }
        }
        for _ in 0..UPLOADS_PER_FRAME {
            let Some((key, want, img)) = self.ready.pop_front() else {
                break;
            };
            let tex = ctx.load_texture(format!("{}#{:?}", key, want), img, texture_options());
            match want {
                Want::Full => {
                    self.full.insert(key.clone(), tex);
                    self.full_order.push_back(key);
                    while self.full_order.len() > MAX_FULL {
                        if let Some(old) = self.full_order.pop_front() {
                            self.full.remove(&old);
                            self.requested.remove(&(old, Want::Full));
                        }
                    }
                }
                _ => {
                    self.last_used.insert(key.clone(), self.frame);
                    self.thumbs.insert(key, tex);
                }
            }
        }
        if !self.ready.is_empty() {
            ctx.request_repaint();
        }
        self.evict_thumbs();
    }

    /// Frees the least-recently-drawn thumbnails beyond the cap (they reload from disk).
    fn evict_thumbs(&mut self) {
        if self.thumbs.len() <= MAX_THUMBS {
            return;
        }
        let mut by_age: Vec<(u64, String)> = self
            .thumbs
            .keys()
            .map(|k| (*self.last_used.get(k).unwrap_or(&0), k.clone()))
            .collect();
        by_age.sort();
        let excess = self.thumbs.len() - MAX_THUMBS;
        for (used, key) in by_age.into_iter().take(excess) {
            if used + 2 >= self.frame {
                break; // still on screen
            }
            self.thumbs.remove(&key);
            self.requested.remove(&(key.clone(), Want::Thumb));
            self.last_used.remove(&key);
        }
    }

    fn job(&self, card: &Card, want: Want) -> Job {
        let set = if card.set.is_empty() {
            "msh".to_string()
        } else {
            card.set.to_lowercase()
        };
        let key = file_name(card);
        Job {
            urls: image_urls(&set, card.collector_number.as_deref(), &card.name),
            path: self.dir.join(&key),
            key,
            want,
        }
    }

    fn request(&mut self, card: &Card, want: Want) -> String {
        let key = file_name(card);
        if self.failed.contains(&key) || !self.requested.insert((key.clone(), want)) {
            return key;
        }
        let job = self.job(card, want);
        if self.on_disk.contains(&key) && want != Want::DownloadOnly {
            let _ = self.disk.send(job);
        } else if want == Want::DownloadOnly && self.on_disk.contains(&key) {
            // Already saved; nothing to do.
        } else {
            if want == Want::DownloadOnly {
                self.downloads_queued += 1;
                let _ = self.net_bulk.send(job);
            } else {
                let _ = self.net.send(job);
            }
        }
        key
    }

    /// Small, smooth image for grids. Starts loading (from disk, or a first-time download)
    /// the first time a card is drawn.
    pub fn thumb(&mut self, card: &Card) -> Option<TextureHandle> {
        if !self.enabled {
            return None;
        }
        let key = self.request(card, Want::Thumb);
        self.last_used.insert(key.clone(), self.frame);
        self.thumbs.get(&key).cloned()
    }

    /// Full-resolution image for zoom and the inspector (falls back to the thumbnail while
    /// it loads).
    pub fn full(&mut self, card: &Card) -> Option<TextureHandle> {
        if !self.enabled {
            return None;
        }
        let key = file_name(card);
        if let Some(t) = self.full.get(&key) {
            return Some(t.clone());
        }
        if self.on_disk.contains(&key) {
            self.request(card, Want::Full);
        }
        self.thumb(card)
    }

    /// Makes sure images are saved on disk (no GPU memory used) - e.g. the whole set
    /// before a draft. Returns how many downloads were queued.
    pub fn prefetch<'a>(&mut self, cards: impl Iterator<Item = &'a Card>) -> usize {
        let before = self.downloads_queued;
        for c in cards {
            if !self.on_disk.contains(&file_name(c)) {
                self.request(c, Want::DownloadOnly);
            }
        }
        self.downloads_queued - before
    }

    pub fn loaded(&self) -> usize {
        self.on_disk.len()
    }

    /// Images still being downloaded for the first time.
    pub fn pending(&self) -> usize {
        self.downloads_queued
    }

    pub fn failures(&self) -> usize {
        self.failed.len()
    }

    pub fn folder(&self) -> &Path {
        &self.dir
    }

    /// Lets failed images be tried again (e.g. after reconnecting).
    pub fn retry_failed(&mut self) {
        for k in self.failed.drain() {
            for w in [Want::Thumb, Want::Full, Want::DownloadOnly] {
                self.requested.remove(&(k.clone(), w));
            }
        }
        self.last_error = None;
    }
}
