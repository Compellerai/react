//! Video sync module with database-backed state and non-blocking operations
//!
//! Features:
//! - Database persistence for sync state (survives app restart)
//! - Non-blocking UI (all operations on background threads)
//! - Resume interrupted downloads using HTTP Range headers
//! - Per-video pause/resume/cancel
//! - Real-time progress tracking

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, SyncVideoStatus};

const SYNC_API_URL: &str = "https://compeller.ai/c0mp3ls/public/";
const CHUNK_SIZE: usize = 256 * 1024; // 256KB chunks
const MAX_RETRIES: u32 = 5;

/// Remote video info from API
#[derive(Debug, Clone, Deserialize, Serialize)]
struct RemoteVideoInfo {
    name: String,
    size: u64,
    #[serde(default)]
    mtime: String,
    #[serde(default, rename = "type")]
    file_type: String,
}

/// URL-encode a filename for use in URLs
fn url_encode_filename(filename: &str) -> String {
    let mut result = String::with_capacity(filename.len() * 3);
    for c in filename.chars() {
        match c {
            ' ' => result.push_str("%20"),
            '!' => result.push_str("%21"),
            '#' => result.push_str("%23"),
            '$' => result.push_str("%24"),
            '&' => result.push_str("%26"),
            '\'' => result.push_str("%27"),
            '(' => result.push_str("%28"),
            ')' => result.push_str("%29"),
            '+' => result.push_str("%2B"),
            ',' => result.push_str("%2C"),
            ';' => result.push_str("%3B"),
            '=' => result.push_str("%3D"),
            '@' => result.push_str("%40"),
            '[' => result.push_str("%5B"),
            ']' => result.push_str("%5D"),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' => result.push(c),
            c => {
                for byte in c.to_string().as_bytes() {
                    result.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    result
}

/// Fetch video list from server (blocking - run on background thread)
fn fetch_video_list() -> Result<Vec<RemoteVideoInfo>> {
    let response = ureq::get(SYNC_API_URL)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| anyhow!("Failed to fetch: {}", e))?;

    let mut files: Vec<RemoteVideoInfo> = response.into_json()
        .map_err(|e| anyhow!("Failed to parse: {}", e))?;

    // Filter to mp4 only
    files.retain(|f| f.name.ends_with(".mp4"));

    log::info!("[Sync] Found {} videos on server", files.len());
    Ok(files)
}

/// Download a single video with resume support (blocking - run on background thread)
fn download_video(
    url: &str,
    dest_path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<()> {
    let part_path = dest_path.with_extension("mp4.part");

    // Check existing progress
    let existing_size = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
    progress.store(existing_size, Ordering::Relaxed);

    // Open/create file for append
    let mut file = if existing_size > 0 {
        log::info!("[Sync] Resuming from {} bytes", existing_size);
        OpenOptions::new().write(true).append(true).open(&part_path)?
    } else {
        File::create(&part_path)?
    };

    // Build request with Range header for resume
    let mut request = ureq::get(url).timeout(Duration::from_secs(120));
    if existing_size > 0 {
        request = request.set("Range", &format!("bytes={}-", existing_size));
    }

    let response = request.call().map_err(|e| anyhow!("Download failed: {}", e))?;

    // Handle resume not supported
    if existing_size > 0 && response.status() != 206 {
        log::warn!("[Sync] Server doesn't support resume, restarting");
        drop(file);
        file = File::create(&part_path)?;
        progress.store(0, Ordering::Relaxed);
    }

    let mut reader = response.into_reader();
    let mut buffer = [0u8; CHUNK_SIZE];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Cancelled"));
        }

        match reader.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                file.write_all(&buffer[..n])?;
                progress.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(e) => return Err(anyhow!("Read error: {}", e)),
        }
    }

    file.flush()?;
    drop(file);

    // Rename to final path
    fs::rename(&part_path, dest_path)?;

    Ok(())
}

/// Sync manager - coordinates background sync operations
pub struct VideoSync {
    /// Is a sync operation currently running?
    pub is_running: Arc<AtomicBool>,
    /// Should the current operation be cancelled?
    pub cancel: Arc<AtomicBool>,
    /// Current download progress (bytes)
    pub current_progress: Arc<AtomicU64>,
    /// Current download speed (bytes/sec)
    pub current_speed: Arc<AtomicU64>,
    /// Name of video currently being downloaded
    pub current_video: Arc<parking_lot::Mutex<Option<String>>>,
    /// Status message
    pub status_message: Arc<parking_lot::Mutex<String>>,
    /// Counter incremented each time a download completes (UI polls this to refresh)
    pub completed_count: Arc<AtomicU64>,
    /// Video directory
    video_dir: PathBuf,
}

impl VideoSync {
    pub fn new(video_dir: PathBuf) -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            current_progress: Arc::new(AtomicU64::new(0)),
            current_speed: Arc::new(AtomicU64::new(0)),
            current_video: Arc::new(parking_lot::Mutex::new(None)),
            status_message: Arc::new(parking_lot::Mutex::new(String::new())),
            completed_count: Arc::new(AtomicU64::new(0)),
            video_dir,
        }
    }

    /// Get the completed count (UI polls this to know when to refresh video list)
    pub fn get_completed_count(&self) -> u64 {
        self.completed_count.load(Ordering::Relaxed)
    }

    /// Get current status message (non-blocking)
    pub fn get_status(&self) -> String {
        self.status_message.try_lock()
            .map(|s| s.clone())
            .unwrap_or_else(|| "Working...".to_string())
    }

    /// Get current download info (non-blocking)
    pub fn get_current_download(&self) -> (Option<String>, u64, u64) {
        let name = self.current_video.try_lock().map(|v| v.clone()).unwrap_or(None);
        let progress = self.current_progress.load(Ordering::Relaxed);
        let speed = self.current_speed.load(Ordering::Relaxed);
        (name, progress, speed)
    }

    /// Is sync currently running?
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Refresh video list from server (background thread)
    pub fn refresh_list(&self, db: Database) {
        if self.is_running.load(Ordering::Relaxed) {
            return;
        }

        self.is_running.store(true, Ordering::Release);
        self.cancel.store(false, Ordering::Release);

        *self.status_message.lock() = "Fetching video list...".to_string();

        let is_running = self.is_running.clone();
        let status = self.status_message.clone();
        let video_dir = self.video_dir.clone();

        std::thread::spawn(move || {
            match fetch_video_list() {
                Ok(videos) => {
                    let base_url = SYNC_API_URL.trim_end_matches('/');

                    for video in &videos {
                        let url = format!("{}/{}", base_url, url_encode_filename(&video.name));
                        let _ = db.upsert_sync_video(&video.name, &url, video.size);
                    }

                    // Mark videos that already exist locally as completed
                    let _ = db.mark_existing_videos_completed(video_dir.to_str().unwrap_or(""));

                    let (pending, _, completed, _, paused, _, _) = db.get_sync_stats();
                    if paused > 0 {
                        *status.lock() = format!("{} paused, {} pending, {} done", paused, pending, completed);
                    } else {
                        *status.lock() = format!("{} pending, {} done", pending, completed);
                    }
                }
                Err(e) => {
                    *status.lock() = format!("Error: {}", e);
                }
            }

            is_running.store(false, Ordering::Release);
        });
    }

    /// Start syncing pending videos (background thread)
    pub fn start_sync(&self, db: Database) {
        if self.is_running.load(Ordering::Relaxed) {
            return;
        }

        self.is_running.store(true, Ordering::Release);
        self.cancel.store(false, Ordering::Release);

        let is_running = self.is_running.clone();
        let cancel = self.cancel.clone();
        let progress = self.current_progress.clone();
        let speed = self.current_speed.clone();
        let current_video = self.current_video.clone();
        let status = self.status_message.clone();
        let completed_count = self.completed_count.clone();
        let video_dir = self.video_dir.clone();

        std::thread::spawn(move || {
            fs::create_dir_all(&video_dir).ok();

            loop {
                if cancel.load(Ordering::Relaxed) {
                    *status.lock() = "Sync cancelled".to_string();
                    break;
                }

                // Get next pending video
                let video = match db.get_next_sync_video() {
                    Some(v) => v,
                    None => {
                        *status.lock() = "All videos synced!".to_string();
                        break;
                    }
                };

                // Update status
                *current_video.lock() = Some(video.name.clone());
                *status.lock() = format!("Downloading: {}", video.name);
                progress.store(0, Ordering::Relaxed);
                speed.store(0, Ordering::Relaxed);

                // Mark as downloading
                let _ = db.set_sync_video_status(&video.name, &SyncVideoStatus::Downloading, None);

                // Start progress tracker thread
                let progress_clone = progress.clone();
                let speed_clone = speed.clone();
                let db_clone = db.clone();
                let name_clone = video.name.clone();
                let cancel_clone = cancel.clone();

                let progress_handle = std::thread::spawn(move || {
                    let mut last_progress = 0u64;
                    let mut last_time = std::time::Instant::now();

                    while !cancel_clone.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(500));

                        let current = progress_clone.load(Ordering::Relaxed);
                        let elapsed = last_time.elapsed().as_secs_f64();

                        if elapsed > 0.0 {
                            let bytes_diff = current.saturating_sub(last_progress);
                            let speed = (bytes_diff as f64 / elapsed) as u64;
                            speed_clone.store(speed, Ordering::Relaxed);
                            last_progress = current;
                            last_time = std::time::Instant::now();
                        }

                        // Update DB progress occasionally
                        let _ = db_clone.set_sync_video_progress(&name_clone, current);
                    }
                });

                // Download
                let dest_path = video_dir.join(&video.name);
                let result = download_video(&video.remote_url, &dest_path, &cancel, &progress);

                // Stop progress tracker
                cancel.store(true, Ordering::Release);
                let _ = progress_handle.join();
                cancel.store(false, Ordering::Release);

                // Update status
                match result {
                    Ok(()) => {
                        let final_size = progress.load(Ordering::Relaxed);
                        let _ = db.set_sync_video_status(&video.name, &SyncVideoStatus::Completed, None);
                        let _ = db.set_sync_video_progress(&video.name, final_size);
                        // Increment counter so UI knows to refresh video list
                        completed_count.fetch_add(1, Ordering::Release);
                        log::info!("[Sync] Completed: {}", video.name);
                    }
                    Err(e) => {
                        if e.to_string().contains("Cancelled") {
                            let _ = db.set_sync_video_status(&video.name, &SyncVideoStatus::Paused, None);
                        } else {
                            let _ = db.set_sync_video_status(&video.name, &SyncVideoStatus::Failed, Some(&e.to_string()));
                            log::warn!("[Sync] Failed {}: {}", video.name, e);
                        }
                        break;
                    }
                }
            }

            *current_video.lock() = None;
            progress.store(0, Ordering::Relaxed);
            speed.store(0, Ordering::Relaxed);
            is_running.store(false, Ordering::Release);
        });
    }

    /// Cancel current sync
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

// Helper functions for UI
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}
