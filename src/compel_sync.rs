//! Compel video sync module - downloads authenticated user's compel videos
//!
//! Mirrors sync.rs architecture: background thread, SQLite state, atomic progress,
//! resume via .mp4.part + Range headers. Downloads go into the same compels/ directory
//! and auto-register video_tracks entries on completion for picker matching.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config;
use crate::db::{Database, SyncVideoStatus};

const CHUNK_SIZE: usize = 256 * 1024; // 256KB chunks

/// Compel info from API
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct CompelInfo {
    #[serde(rename = "compelId")]
    compel_id: i64,
    title: Option<String>,
    artist: Option<String>,
    #[serde(rename = "aspectRatio")]
    aspect_ratio: Option<String>,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<i64>,
    videos: Vec<CompelVideoEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct CompelVideoEntry {
    #[serde(rename = "videoId")]
    video_id: i64,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct CompelsResponse {
    success: bool,
    compels: Vec<CompelInfo>,
}

/// Sanitize a string for use in filenames
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Build a display name like "Artist - Title.mp4" for a compel
fn compel_filename(artist: Option<&str>, title: Option<&str>, compel_id: i64) -> String {
    let name = match (artist, title) {
        (Some(a), Some(t)) if !a.is_empty() && !t.is_empty() => format!("{} - {}", a, t),
        (None, Some(t)) | (Some(""), Some(t)) if !t.is_empty() => t.to_string(),
        (Some(a), None) | (Some(a), Some("")) if !a.is_empty() => a.to_string(),
        _ => format!("compel_{}", compel_id),
    };
    format!("{}.mp4", sanitize_filename(&name))
}

/// Fetch compel list from server (blocking - run on background thread)
fn fetch_compel_list(auth_token: &str, since: i64) -> Result<Vec<CompelInfo>> {
    let url = format!("{}/react/compels?since={}", config::API_HOST, since);

    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", auth_token))
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| anyhow!("Failed to fetch compels: {}", e))?;

    let resp: CompelsResponse = response
        .into_json()
        .map_err(|e| anyhow!("Failed to parse compels: {}", e))?;

    if !resp.success {
        return Err(anyhow!("API returned success=false"));
    }

    log::info!("[CompelSync] Found {} compels from server", resp.compels.len());
    Ok(resp.compels)
}

/// Download a single video with resume support and Bearer auth (blocking)
fn download_compel_video(
    video_id: i64,
    dest_path: &Path,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    auth_token: &str,
) -> Result<()> {
    let part_path = dest_path.with_extension("mp4.part");
    let url = format!("{}/react/compels/video/{}", config::API_HOST, video_id);

    // Check existing progress
    let existing_size = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
    progress.store(existing_size, Ordering::Relaxed);

    // Open/create file for append
    let mut file = if existing_size > 0 {
        log::info!("[CompelSync] Resuming video {} from {} bytes", video_id, existing_size);
        OpenOptions::new().write(true).append(true).open(&part_path)?
    } else {
        File::create(&part_path)?
    };

    // Build request with Range header for resume + Bearer auth
    let mut request = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", auth_token))
        .timeout(Duration::from_secs(120));
    if existing_size > 0 {
        request = request.set("Range", &format!("bytes={}-", existing_size));
    }

    let response = request.call().map_err(|e| anyhow!("Download failed: {}", e))?;

    // Handle resume not supported
    if existing_size > 0 && response.status() != 206 {
        log::warn!("[CompelSync] Server doesn't support resume, restarting");
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

/// Compel sync manager - coordinates background sync for authenticated user's compel videos
pub struct CompelSync {
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
    /// Video directory — shared so it stays in sync when the user changes folders
    video_dir: Arc<parking_lot::Mutex<PathBuf>>,
}

impl CompelSync {
    pub fn new(video_dir: PathBuf) -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            current_progress: Arc::new(AtomicU64::new(0)),
            current_speed: Arc::new(AtomicU64::new(0)),
            current_video: Arc::new(parking_lot::Mutex::new(None)),
            status_message: Arc::new(parking_lot::Mutex::new(String::new())),
            completed_count: Arc::new(AtomicU64::new(0)),
            video_dir: Arc::new(parking_lot::Mutex::new(video_dir)),
        }
    }

    /// Update the download directory (call when user changes their asset folder)
    pub fn set_video_dir(&self, dir: PathBuf) {
        *self.video_dir.lock() = dir;
    }

    /// Get the completed count (UI polls this to know when to refresh video list)
    pub fn get_completed_count(&self) -> u64 {
        self.completed_count.load(Ordering::Relaxed)
    }

    /// Get current status message (non-blocking)
    pub fn get_status(&self) -> String {
        self.status_message
            .try_lock()
            .map(|s| s.clone())
            .unwrap_or_else(|| "Working...".to_string())
    }

    /// Is sync currently running?
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Get current download info: (filename, bytes_downloaded, speed_bps)
    pub fn get_current_download(&self) -> (Option<String>, u64, u64) {
        let name = self.current_video.try_lock().map(|v| v.clone()).unwrap_or(None);
        let progress = self.current_progress.load(Ordering::Relaxed);
        let speed = self.current_speed.load(Ordering::Relaxed);
        (name, progress, speed)
    }

    /// Refresh compel list from server and auto-start downloading (background thread)
    pub fn refresh_list(&self, db: Database, auth_token: String) {
        if self.is_running.load(Ordering::Relaxed) {
            return;
        }

        self.is_running.store(true, Ordering::Release);
        self.cancel.store(false, Ordering::Release);

        *self.status_message.lock() = "Fetching compel list...".to_string();

        let is_running = self.is_running.clone();
        let cancel = self.cancel.clone();
        let progress = self.current_progress.clone();
        let speed = self.current_speed.clone();
        let current_video = self.current_video.clone();
        let status = self.status_message.clone();
        let completed_count = self.completed_count.clone();
        let video_dir = self.video_dir.clone();

        std::thread::spawn(move || {
            // Phase 1: Fetch list from API
            let since = db.get_last_compel_sync() * 1000; // Convert seconds to millis

            match fetch_compel_list(&auth_token, since) {
                Ok(compels) => {
                    for compel in &compels {
                        let filename = compel_filename(
                            compel.artist.as_deref(),
                            compel.title.as_deref(),
                            compel.compel_id,
                        );

                        for video in &compel.videos {
                            let _ = db.upsert_compel_video(
                                compel.compel_id,
                                video.video_id,
                                &filename,
                                compel.artist.as_deref(),
                                compel.title.as_deref(),
                                video.size,
                            );
                        }
                    }

                    // Mark videos that already exist locally as completed
                    let dir_snapshot = video_dir.lock().clone();
                    mark_existing_compel_videos(&db, &dir_snapshot);

                    let (pending, _, completed, _, _, _, _) = db.get_compel_sync_stats();
                    *status.lock() = format!("{} compels pending, {} done", pending, completed);
                }
                Err(e) => {
                    *status.lock() = format!("Error: {}", e);
                    is_running.store(false, Ordering::Release);
                    return;
                }
            }

            // Phase 2: Download pending videos
            // Snapshot the current directory — if user changed folders since
            // construction, downloads land in the right place
            let video_dir = video_dir.lock().clone();
            fs::create_dir_all(&video_dir).ok();
            let _ = db.reset_interrupted_compel_downloads();

            loop {
                if cancel.load(Ordering::Relaxed) {
                    *status.lock() = "Compel sync cancelled".to_string();
                    break;
                }

                let video = match db.get_next_compel_video() {
                    Some(v) => v,
                    None => {
                        *status.lock() = "All compel videos synced!".to_string();
                        break;
                    }
                };

                *current_video.lock() = Some(video.name.clone());
                *status.lock() = format!("Downloading: {}", video.name);
                progress.store(0, Ordering::Relaxed);
                speed.store(0, Ordering::Relaxed);

                let _ = db.set_compel_video_status(video.video_id, &SyncVideoStatus::Downloading, None);

                // Progress tracker thread
                let progress_clone = progress.clone();
                let speed_clone = speed.clone();
                let db_clone = db.clone();
                let vid_id = video.video_id;
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
                            let spd = (bytes_diff as f64 / elapsed) as u64;
                            speed_clone.store(spd, Ordering::Relaxed);
                            last_progress = current;
                            last_time = std::time::Instant::now();
                        }

                        let _ = db_clone.set_compel_video_progress(vid_id, current);
                    }
                });

                // Download
                let dest_path = video_dir.join(&video.name);
                let result = download_compel_video(
                    video.video_id,
                    &dest_path,
                    &cancel,
                    &progress,
                    &auth_token,
                );

                // Stop progress tracker
                cancel.store(true, Ordering::Release);
                let _ = progress_handle.join();
                cancel.store(false, Ordering::Release);

                match result {
                    Ok(()) => {
                        let final_size = progress.load(Ordering::Relaxed);
                        let _ = db.set_compel_video_status(video.video_id, &SyncVideoStatus::Completed, None);
                        let _ = db.set_compel_video_progress(video.video_id, final_size);

                        // Auto-register in video_tracks for picker matching (priority 10)
                        let dest_str = dest_path.display().to_string();
                        let track_name = match (&video.artist, &video.title) {
                            (Some(a), Some(t)) if !a.is_empty() && !t.is_empty() => format!("{} - {}", a, t),
                            (_, Some(t)) if !t.is_empty() => t.clone(),
                            _ => video.name.trim_end_matches(".mp4").to_string(),
                        };
                        let _ = db.add_video_track(
                            &dest_str,
                            &track_name,
                            video.artist.as_deref(),
                            None,
                            10,
                        );

                        completed_count.fetch_add(1, Ordering::Release);
                        log::info!("[CompelSync] Completed: {}", video.name);
                    }
                    Err(e) => {
                        if e.to_string().contains("Cancelled") {
                            let _ = db.set_compel_video_status(video.video_id, &SyncVideoStatus::Paused, None);
                        } else {
                            let _ = db.set_compel_video_status(video.video_id, &SyncVideoStatus::Failed, Some(&e.to_string()));
                            log::warn!("[CompelSync] Failed {}: {}", video.name, e);
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

/// Check which compel videos already exist locally and mark them completed
fn mark_existing_compel_videos(db: &Database, video_dir: &Path) {
    let entries = db.get_all_compel_videos();

    for (vid_id, name, remote_size, status) in entries {
        let local_path = video_dir.join(&name);
        let part_path = local_path.with_extension("mp4.part");

        if local_path.exists() {
            if let Ok(meta) = fs::metadata(&local_path) {
                if meta.len() >= remote_size && remote_size > 0 {
                    let _ = db.set_compel_video_status(vid_id, &SyncVideoStatus::Completed, None);
                    let _ = db.set_compel_video_progress(vid_id, meta.len());
                    continue;
                }
            }
        }

        if part_path.exists() {
            if let Ok(meta) = fs::metadata(&part_path) {
                if meta.len() > 0 && meta.len() < remote_size {
                    let _ = db.set_compel_video_progress(vid_id, meta.len());
                    if status == "pending" || status == "downloading" {
                        let _ = db.set_compel_video_status(vid_id, &SyncVideoStatus::Paused, None);
                    }
                    continue;
                }
            }
        }

        // If marked completed but file doesn't exist, reset to pending
        if status == "completed" && !local_path.exists() {
            log::info!("[CompelSync] Resetting missing video to pending: {}", name);
            let _ = db.set_compel_video_status(vid_id, &SyncVideoStatus::Pending, None);
            let _ = db.set_compel_video_progress(vid_id, 0);
        }
    }
}
