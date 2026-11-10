//! Compel Set upload module with chunked uploads and resume support
//!
//! Features:
//! - Chunked uploads (5MB chunks) matching server expectations
//! - Resume interrupted uploads via session tokens
//! - Database persistence for upload state
//! - Non-blocking UI (background thread uploads)
//! - Progress tracking with atomic counters
//! - SHA-256 checksum verification

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::auth::AuthManager;
use crate::config;
use crate::db::{Database, SetUploadStatus};
use crate::video::ThumbnailGenerator;

/// Chunk size for uploads (5MB - matches server CHUNK_SIZE)
const CHUNK_SIZE: usize = 5 * 1024 * 1024;

/// HTTP timeout for chunk uploads
const UPLOAD_TIMEOUT_SECS: u64 = 120;

/// Max retries per chunk
const MAX_RETRIES: u32 = 3;

/// Retry delay base (exponential backoff)
const RETRY_DELAY_MS: u64 = 2000;

// ========== API Response Types ==========

/// Response from POST /react/sets/init
#[derive(Debug, Deserialize)]
struct InitResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "sessionToken")]
    session_token: Option<String>,
    #[serde(rename = "chunkSize")]
    chunk_size: Option<u64>,
    #[serde(rename = "totalChunks")]
    total_chunks: Option<u32>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
}

/// Response from GET /react/sets/resume/{token}
#[derive(Debug, Deserialize)]
struct ResumeResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "sessionToken")]
    session_token: Option<String>,
    #[serde(rename = "uploadedSize")]
    uploaded_size: Option<u64>,
    #[serde(rename = "chunksReceived")]
    chunks_received: Option<u32>,
    #[serde(rename = "totalChunks")]
    total_chunks: Option<u32>,
    #[serde(rename = "totalSize")]
    total_size: Option<u64>,
    #[serde(rename = "nextChunkIndex")]
    next_chunk_index: Option<u32>,
    #[serde(rename = "chunkSize")]
    chunk_size: Option<u64>,
}

/// Response from PUT /react/sets/chunk/{token}/{index}
#[derive(Debug, Deserialize)]
struct ChunkResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "chunkIndex")]
    chunk_index: Option<u32>,
    #[serde(rename = "uploadedSize")]
    uploaded_size: Option<u64>,
    #[serde(rename = "chunksReceived")]
    chunks_received: Option<u32>,
    #[serde(rename = "isComplete")]
    is_complete: Option<bool>,
    progress: Option<f64>,
}

/// Response from POST /react/sets/complete/{token}
#[derive(Debug, Deserialize)]
struct CompleteResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(rename = "compelSetId")]
    compel_set_id: Option<i64>,
    title: Option<String>,
    #[serde(rename = "fileSize")]
    file_size: Option<String>,
    duration: Option<String>,
}

// ========== Upload Request Metadata ==========

/// Metadata for a set upload request
#[derive(Debug, Clone, Serialize)]
pub struct SetUploadRequest {
    /// Local file path
    pub file_path: String,
    /// Display title (optional, defaults to filename)
    pub title: Option<String>,
    /// When the recording was made (ISO timestamp)
    pub recorded_at: Option<String>,
    /// Which theme was used
    pub theme_used: Option<String>,
    /// Audio source during recording
    pub audio_source: Option<String>,
    /// Duration in seconds
    pub duration_seconds: Option<i64>,
}

// ========== Upload Manager ==========

/// Upload state
#[derive(Debug, Clone, PartialEq)]
pub enum UploadState {
    Idle,
    Uploading,
    Paused,
    Completed,
    Failed(String),
}

/// Manages background upload operations
pub struct SetUploadManager {
    /// Database for persistence
    db: Arc<Database>,
    /// Auth manager for tokens
    auth: AuthManager,
    /// Current upload state
    state: Arc<parking_lot::Mutex<UploadState>>,
    /// Cancel flag for current upload
    cancel: Arc<AtomicBool>,
    /// Current upload progress (bytes uploaded)
    progress_bytes: Arc<AtomicU64>,
    /// Current upload total size
    total_bytes: Arc<AtomicU64>,
    /// Current file being uploaded
    current_file: Arc<parking_lot::Mutex<Option<String>>>,
    /// Is upload thread running?
    is_running: Arc<AtomicBool>,
}

impl SetUploadManager {
    pub fn new(db: Arc<Database>, auth: AuthManager) -> Self {
        Self {
            db,
            auth,
            state: Arc::new(parking_lot::Mutex::new(UploadState::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
            progress_bytes: Arc::new(AtomicU64::new(0)),
            total_bytes: Arc::new(AtomicU64::new(0)),
            current_file: Arc::new(parking_lot::Mutex::new(None)),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get current state
    pub fn get_state(&self) -> UploadState {
        self.state.lock().clone()
    }

    /// Get upload progress (0.0 - 1.0)
    pub fn get_progress(&self) -> f32 {
        let total = self.total_bytes.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let progress = self.progress_bytes.load(Ordering::Relaxed);
        (progress as f32 / total as f32).min(1.0)
    }

    /// Get progress in bytes (uploaded, total)
    pub fn get_progress_bytes(&self) -> (u64, u64) {
        (
            self.progress_bytes.load(Ordering::Relaxed),
            self.total_bytes.load(Ordering::Relaxed),
        )
    }

    /// Get current file being uploaded
    pub fn get_current_file(&self) -> Option<String> {
        self.current_file.lock().clone()
    }

    /// Is upload running?
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Queue a recording for upload
    pub fn queue_upload(&self, request: SetUploadRequest) -> Result<()> {
        let path = Path::new(&request.file_path);
        if !path.exists() {
            return Err(anyhow!("File not found: {}", request.file_path));
        }

        let metadata = std::fs::metadata(path)?;
        let total_size = metadata.len();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("recording.mp4")
            .to_string();

        self.db.add_set_upload(
            &request.file_path,
            &file_name,
            total_size,
            request.title.as_deref(),
            request.recorded_at.as_deref(),
            request.theme_used.as_deref(),
            request.audio_source.as_deref(),
            request.duration_seconds,
        )?;

        log::info!("[SetSync] Queued upload: {} ({} bytes)", file_name, total_size);
        Ok(())
    }

    /// Start uploading (processes queue on background thread)
    pub fn start_upload(&self) {
        if self.is_running.swap(true, Ordering::SeqCst) {
            log::warn!("[SetSync] Upload already running");
            return;
        }

        // Check auth
        if self.auth.get_token().is_none() {
            log::error!("[SetSync] Not authenticated - cannot upload");
            *self.state.lock() = UploadState::Failed("Not logged in".to_string());
            self.is_running.store(false, Ordering::SeqCst);
            return;
        }

        self.cancel.store(false, Ordering::SeqCst);
        *self.state.lock() = UploadState::Uploading;

        // Clone for thread
        let db = self.db.clone();
        let auth = self.auth.clone();
        let state = self.state.clone();
        let cancel = self.cancel.clone();
        let progress_bytes = self.progress_bytes.clone();
        let total_bytes = self.total_bytes.clone();
        let current_file = self.current_file.clone();
        let is_running = self.is_running.clone();

        std::thread::spawn(move || {
            // Reset interrupted uploads on start
            if let Err(e) = db.reset_interrupted_uploads() {
                log::error!("[SetSync] Failed to reset interrupted uploads: {}", e);
            }

            // Process queue
            while !cancel.load(Ordering::Relaxed) {
                let next = db.get_next_set_upload();
                match next {
                    Some(upload) => {
                        // Check if file still exists on disk
                        if !std::path::Path::new(&upload.file_path).exists() {
                            log::warn!("[SetSync] File no longer exists, removing from queue: {}", upload.file_path);
                            if let Err(e) = db.delete_set_upload(&upload.file_path) {
                                log::error!("[SetSync] Failed to remove orphaned upload: {}", e);
                            }
                            continue;
                        }

                        *current_file.lock() = Some(upload.file_path.clone());
                        total_bytes.store(upload.total_size, Ordering::Relaxed);
                        progress_bytes.store(upload.uploaded_size, Ordering::Relaxed);

                        log::info!("[SetSync] Starting upload: {}", upload.file_name);

                        let result = upload_file(
                            &db,
                            &auth,
                            &upload,
                            &cancel,
                            &progress_bytes,
                        );

                        match result {
                            Ok(remote_id) => {
                                log::info!("[SetSync] Upload complete: {} -> remote ID {}", upload.file_name, remote_id);
                                if let Err(e) = db.set_upload_completed(&upload.file_path, remote_id) {
                                    log::error!("[SetSync] Failed to mark complete: {}", e);
                                }
                            }
                            Err(e) => {
                                let error_msg = e.to_string();
                                if error_msg.contains("Cancelled") {
                                    log::info!("[SetSync] Upload cancelled: {}", upload.file_name);
                                    let _ = db.set_upload_status(&upload.file_path, &SetUploadStatus::Paused, None);
                                } else {
                                    log::error!("[SetSync] Upload failed: {} - {}", upload.file_name, error_msg);
                                    let _ = db.set_upload_status(&upload.file_path, &SetUploadStatus::Failed, Some(&error_msg));
                                }
                            }
                        }

                        *current_file.lock() = None;
                    }
                    None => {
                        // Queue empty
                        break;
                    }
                }
            }

            *state.lock() = if cancel.load(Ordering::Relaxed) {
                UploadState::Paused
            } else {
                UploadState::Completed
            };
            is_running.store(false, Ordering::SeqCst);
            log::info!("[SetSync] Upload thread finished");
        });
    }

    /// Pause current upload
    pub fn pause_upload(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        *self.state.lock() = UploadState::Paused;
    }

    /// Cancel and remove an upload from queue
    pub fn cancel_upload(&self, file_path: &str) -> Result<()> {
        // If it's the current upload, cancel it
        if self.current_file.lock().as_deref() == Some(file_path) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.db.delete_set_upload(file_path)?;
        Ok(())
    }

    /// Get upload stats
    pub fn get_stats(&self) -> (usize, usize, usize, usize, usize, u64, u64) {
        self.db.get_upload_stats()
    }

    /// Validate local sync status against SaaS
    /// Fetches the list of sets from SaaS and resets any local "completed" uploads
    /// whose remote_set_id doesn't exist in SaaS
    pub fn validate_sync_status(&self) -> Result<usize> {
        let token = match self.auth.get_token() {
            Some(t) => t,
            None => {
                log::debug!("[SetSync] Not authenticated, skipping sync validation");
                return Ok(0);
            }
        };

        let base_url = config::API_HOST;
        let url = format!("{}/react/sets/list", base_url);

        log::info!("[SetSync] Validating sync status with SaaS...");

        let response = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", token))
            .timeout(Duration::from_secs(30))
            .call();

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                return Err(anyhow!("Failed to fetch sets from SaaS: {}", e));
            }
        };

        let body: serde_json::Value = response.into_json()?;

        if body.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
            let error = body.get("error").and_then(serde_json::Value::as_str).unwrap_or("Unknown error");
            return Err(anyhow!("SaaS returned error: {}", error));
        }

        // Extract set IDs from response
        let valid_ids: Vec<i64> = body
            .get("sets")
            .and_then(serde_json::Value::as_array)
            .map(|arr: &Vec<serde_json::Value>| {
                arr.iter()
                    .filter_map(|s: &serde_json::Value| s.get("id").and_then(serde_json::Value::as_i64))
                    .collect()
            })
            .unwrap_or_default();

        log::info!("[SetSync] Found {} sets in SaaS", valid_ids.len());

        // Reset any local "completed" uploads whose remote_set_id is not in SaaS
        let reset_count = self.db.reset_invalid_completed_uploads(&valid_ids)?;

        Ok(reset_count)
    }
}

// ========== Upload Implementation ==========

/// Upload a single file with chunked upload and resume support
fn upload_file(
    db: &Database,
    auth: &AuthManager,
    upload: &crate::db::SetUploadInfo,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<i64> {
    let token = auth.get_token().ok_or_else(|| anyhow!("Not authenticated"))?;
    let base_url = config::API_HOST;

    // Step 1: Get or resume session
    let (session_token, start_chunk) = if let Some(ref existing_token) = upload.session_token {
        // Try to resume existing session
        match resume_session(base_url, &token, existing_token) {
            Ok((chunks_received, _total_chunks)) => {
                log::info!("[SetSync] Resuming session {} from chunk {}", existing_token, chunks_received);
                (existing_token.clone(), chunks_received)
            }
            Err(e) => {
                log::warn!("[SetSync] Failed to resume session, starting new: {}", e);
                let new_token = init_session(db, auth, upload)?;
                (new_token, 0)
            }
        }
    } else {
        // Initialize new session
        let new_token = init_session(db, auth, upload)?;
        (new_token, 0)
    };

    // Step 2: Calculate checksum if not already computed
    if upload.checksum.is_none() {
        log::info!("[SetSync] Computing checksum for {}", upload.file_name);
        let checksum = compute_checksum(&upload.file_path)?;
        db.set_upload_checksum(&upload.file_path, &checksum)?;
    }

    // Step 3: Upload chunks
    let file = File::open(&upload.file_path)?;
    let file_size = file.metadata()?.len();
    let total_chunks = ((file_size + CHUNK_SIZE as u64 - 1) / CHUNK_SIZE as u64) as u32;

    db.set_upload_status(&upload.file_path, &SetUploadStatus::Uploading, None)?;

    for chunk_index in start_chunk..total_chunks {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Cancelled"));
        }

        let chunk_data = read_chunk(&upload.file_path, chunk_index as usize)?;

        // Upload with retry
        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("Cancelled"));
            }

            match upload_chunk(base_url, &token, &session_token, chunk_index, &chunk_data) {
                Ok(response) => {
                    if response.success {
                        let uploaded = response.uploaded_size.unwrap_or(0);
                        progress.store(uploaded, Ordering::Relaxed);
                        db.set_upload_progress(&upload.file_path, uploaded)?;
                        log::debug!("[SetSync] Chunk {}/{} uploaded", chunk_index + 1, total_chunks);
                        last_error = None;
                        break;
                    } else {
                        last_error = Some(response.error.unwrap_or_else(|| "Unknown error".to_string()));
                    }
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                }
            }

            if attempt < MAX_RETRIES - 1 {
                let delay = RETRY_DELAY_MS * (2_u64.pow(attempt));
                log::warn!("[SetSync] Chunk {} failed, retrying in {}ms: {:?}", chunk_index, delay, last_error);
                std::thread::sleep(Duration::from_millis(delay));
            }
        }

        if let Some(error) = last_error {
            return Err(anyhow!("Failed to upload chunk {}: {}", chunk_index, error));
        }
    }

    // Step 4: Complete upload
    log::info!("[SetSync] Completing upload: {}", upload.file_name);
    let complete_response = complete_upload(base_url, &token, &session_token)?;

    if !complete_response.success {
        return Err(anyhow!("Failed to complete: {}", complete_response.error.unwrap_or_else(|| "Unknown".to_string())));
    }

    let remote_id = complete_response.compel_set_id.ok_or_else(|| anyhow!("No remote ID returned"))?;

    // Step 5: Upload thumbnail if it exists
    if let Ok(thumb_gen) = ThumbnailGenerator::new() {
        let thumb_path = thumb_gen.get_thumbnail_path(&upload.file_path);
        if thumb_path.exists() {
            log::info!("[SetSync] Uploading thumbnail for set {}", remote_id);
            match upload_thumbnail(base_url, &token, remote_id, &thumb_path) {
                Ok(()) => {
                    log::info!("[SetSync] Thumbnail uploaded successfully for set {}", remote_id);
                }
                Err(e) => {
                    // Don't fail the whole upload if thumbnail fails - just log it
                    log::warn!("[SetSync] Failed to upload thumbnail for set {}: {}", remote_id, e);
                }
            }
        } else {
            log::debug!("[SetSync] No thumbnail found for {}", upload.file_path);
        }
    }

    Ok(remote_id)
}

/// Initialize a new upload session
fn init_session(db: &Database, auth: &AuthManager, upload: &crate::db::SetUploadInfo) -> Result<String> {
    let token = auth.get_token().ok_or_else(|| anyhow!("Not authenticated"))?;
    let base_url = config::API_HOST;
    let url = format!("{}/react/sets/init", base_url);

    let file = File::open(&upload.file_path)?;
    let file_size = file.metadata()?.len();

    let body = serde_json::json!({
        "fileName": upload.file_name,
        "totalSize": file_size,
        "title": upload.title,
        "recordedAt": upload.recorded_at,
        "themeUsed": upload.theme_used,
        "audioSource": upload.audio_source,
        "durationSeconds": upload.duration_seconds,
    });

    let response: InitResponse = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .send_json(body)
        .map_err(|e| anyhow!("Init request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse init response: {}", e))?;

    if !response.success {
        return Err(anyhow!("Init failed: {}", response.error.unwrap_or_else(|| "Unknown".to_string())));
    }

    let session_token = response.session_token.ok_or_else(|| anyhow!("No session token"))?;

    // Save session token to database
    db.set_upload_session(&upload.file_path, &session_token)?;

    log::info!("[SetSync] Created session: {} ({} chunks)", session_token, response.total_chunks.unwrap_or(0));
    Ok(session_token)
}

/// Resume an existing upload session
fn resume_session(base_url: &str, token: &str, session_token: &str) -> Result<(u32, u32)> {
    let url = format!("{}/react/sets/resume/{}", base_url, session_token);

    let response: ResumeResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| anyhow!("Resume request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse resume response: {}", e))?;

    if !response.success {
        return Err(anyhow!("Resume failed: {}", response.error.unwrap_or_else(|| "Unknown".to_string())));
    }

    let chunks_received = response.chunks_received.unwrap_or(0);
    let total_chunks = response.total_chunks.unwrap_or(0);

    Ok((chunks_received, total_chunks))
}

/// Upload a single chunk
fn upload_chunk(base_url: &str, token: &str, session_token: &str, chunk_index: u32, data: &[u8]) -> Result<ChunkResponse> {
    let url = format!("{}/react/sets/chunk/{}/{}", base_url, session_token, chunk_index);

    let response: ChunkResponse = ureq::put(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/octet-stream")
        .timeout(Duration::from_secs(UPLOAD_TIMEOUT_SECS))
        .send_bytes(data)
        .map_err(|e| anyhow!("Chunk upload failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse chunk response: {}", e))?;

    Ok(response)
}

/// Complete the upload
fn complete_upload(base_url: &str, token: &str, session_token: &str) -> Result<CompleteResponse> {
    let url = format!("{}/react/sets/complete/{}", base_url, session_token);

    let response: CompleteResponse = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| anyhow!("Complete request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse complete response: {}", e))?;

    Ok(response)
}

/// Read a specific chunk from a file
fn read_chunk(file_path: &str, chunk_index: usize) -> Result<Vec<u8>> {
    let mut file = File::open(file_path)?;
    let file_size = file.metadata()?.len() as usize;

    let start = chunk_index * CHUNK_SIZE;
    if start >= file_size {
        return Err(anyhow!("Chunk index out of bounds"));
    }

    let end = std::cmp::min(start + CHUNK_SIZE, file_size);
    let chunk_len = end - start;

    file.seek(SeekFrom::Start(start as u64))?;

    let mut buffer = vec![0u8; chunk_len];
    file.read_exact(&mut buffer)?;

    Ok(buffer)
}

/// Compute SHA-256 checksum of a file
fn compute_checksum(file_path: &str) -> Result<String> {
    let mut file = File::open(file_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

/// Upload thumbnail for a completed set
fn upload_thumbnail(base_url: &str, token: &str, set_id: i64, thumb_path: &Path) -> Result<()> {
    let url = format!("{}/react/sets/thumbnail/{}", base_url, set_id);

    // Read thumbnail file
    let thumb_data = std::fs::read(thumb_path)
        .map_err(|e| anyhow!("Failed to read thumbnail: {}", e))?;

    // Determine content type from extension
    let content_type = match thumb_path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => "image/png",
    };

    #[derive(Debug, Deserialize)]
    struct ThumbnailResponse {
        success: bool,
        #[serde(default)]
        error: Option<String>,
    }

    let response: ThumbnailResponse = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", content_type)
        .timeout(Duration::from_secs(60))
        .send_bytes(&thumb_data)
        .map_err(|e| anyhow!("Thumbnail upload failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow!("Failed to parse thumbnail response: {}", e))?;

    if !response.success {
        return Err(anyhow!("Thumbnail upload failed: {}", response.error.unwrap_or_else(|| "Unknown".to_string())));
    }

    Ok(())
}
