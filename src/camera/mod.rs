//! Multi-Camera Tracking System
//!
//! Provides person detection and tracking across multiple USB cameras,
//! with bounded resource usage suitable for real-time audio-reactive visualization.
//!
//! ## Architecture
//!
//! ```text
//! [USB Cameras] → capture (frame rings)
//!                     ↓
//!               detector (ONNX, 10-15 fps)
//!                     ↓
//!               tracker (SORT-style, per-camera IDs)
//!                     ↓
//!               CameraManager (select active entity)
//!                     ↓
//!               TrackingOutput (gate anchor, confidence)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let mut manager = CameraManager::new(TrackingConfig::default());
//! manager.start()?;
//!
//! // In render loop:
//! let output = manager.get_output();
//! if output.active {
//!     // Use output.anchor_index for gate positioning
//! }
//! ```

pub mod state;
pub mod capture;
pub mod detector;
pub mod tracker;

// ============================================================================
// Camera Media System (Background Layers)
// ============================================================================
//
// Cameras as background media = visual layers only.
// NO effect on symmetry, anchors, gates, or topology.

/// Quality tier for camera media decoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraQualityTier {
    Low,     // max_active=1, decode_fps=15, scale=0.25
    #[default]
    Medium,  // max_active=2, decode_fps=24, scale=0.5
    High,    // max_active=4, decode_fps=30, scale=1.0
}

impl CameraQualityTier {
    /// Maximum number of cameras actively compositing at this tier
    pub fn max_active(&self) -> usize {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 4,
        }
    }

    /// Target decode FPS at this tier
    pub fn decode_fps(&self) -> f32 {
        match self {
            Self::Low => 15.0,
            Self::Medium => 24.0,
            Self::High => 30.0,
        }
    }

    /// Decode scale factor at this tier
    pub fn decode_scale(&self) -> f32 {
        match self {
            Self::Low => 0.25,
            Self::Medium => 0.5,
            Self::High => 1.0,
        }
    }
}

/// Decode state machine for camera media
///
/// State transitions:
/// ```text
/// IDLE ──enabled && vis_q>0──► WARM ──warmup 500ms──► ACTIVE
///   ▲                            │                        │
///   └────disabled || vis_q=0─────┴───vis_smoothed<0.001───┘
///                                    (after 500ms idle)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraDecodeState {
    #[default]
    Idle,   // No decode, no upload
    Warm,   // Pre-allocating/decoding, not uploading yet
    Active, // Decoding and uploading to GPU
}

/// Normalization mode for multi-camera brightness
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NormalizeMode {
    /// No normalization - alphas sum directly
    #[default]
    Off,
    /// Equalized mix - if sum > 1, scale each alpha proportionally
    EqualizedMix,
}

/// ProjectM composite phase - explicit control of feedback integration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectMCompositePhase {
    /// Camera imagery feeds the feedback loop (enters projectM's feedback)
    BeforeFeedback,
    /// Camera imagery overlays but does not enter feedback (default, safer)
    #[default]
    AfterFeedback,
}

/// Camera compositor configuration (group-level settings)
#[derive(Debug, Clone)]
pub struct CameraCompositorConfig {
    /// Master enable for entire camera background group
    pub cameras_enabled: bool,
    /// Luma cap in linear space (default 0.85, prevents over-bright composites)
    pub luma_cap: f32,
    /// Normalization mode for multi-camera mix
    pub normalize_mode: NormalizeMode,
    /// Current quality tier
    pub quality_tier: CameraQualityTier,
    /// Scheduler epoch for round-robin fairness (increments every 4 frames)
    pub scheduler_epoch: u32,
    /// ProjectM composite phase (explicit feedback control)
    pub projectm_phase: ProjectMCompositePhase,
}

impl Default for CameraCompositorConfig {
    fn default() -> Self {
        Self {
            cameras_enabled: true,
            luma_cap: 0.85,
            normalize_mode: NormalizeMode::Off,
            quality_tier: CameraQualityTier::Medium,
            scheduler_epoch: 0,
            projectm_phase: ProjectMCompositePhase::AfterFeedback,
        }
    }
}

impl CameraCompositorConfig {
    /// Advance scheduler epoch (call every 4 frames)
    pub fn advance_scheduler(&mut self) {
        self.scheduler_epoch = self.scheduler_epoch.wrapping_add(1);
    }

    /// Get max active cameras for current tier
    pub fn max_active(&self) -> usize {
        self.quality_tier.max_active()
    }
}

/// Per-camera media state for background compositing
/// Separate from tracking state - cameras as visual layers only
#[derive(Debug, Clone)]
pub struct CameraMediaState {
    /// Camera index
    pub camera_index: u32,
    /// Camera name
    pub camera_name: String,
    /// Whether this camera is enabled for background display
    pub enabled: bool,
    /// Visibility quantized step (0-16)
    pub vis_q: u8,
    /// RC-eased alpha for smooth transitions (tau=120ms)
    pub vis_smoothed: f32,
    /// Stable layer order for compositing (lower = back)
    pub composite_order: i32,
    /// Current decode state
    pub decode_state: CameraDecodeState,
    /// Idle timer for delayed state transition (500ms before Idle)
    pub idle_timer: f32,
    /// Last frame decode time
    pub last_decode_time: std::time::Instant,
    /// Rate-limit timer: 1 step per vis_hold_ms
    pub vis_hold_timer: f32,
    /// Last MIDI CC value received (for pickup mode)
    pub midi_last_cc: u8,
    /// Whether MIDI pickup is active (waiting for knob to match current value)
    pub midi_pickup_active: bool,
    /// Timestamp of last user (UI) change
    pub last_user_change_ts: f64,
    /// Timestamp of last MIDI change
    pub last_midi_change_ts: f64,
}

/// UI/MIDI timing constants
pub const VIS_HOLD_MS: f32 = 0.120;      // Rate limit: 1 step per 120ms
pub const MIDI_COOLDOWN_MS: f32 = 0.090; // 90ms cooldown between MIDI toggles
pub const MIDI_HYSTERESIS: i16 = 2;      // ±2 CC units hysteresis
pub const PICKUP_WINDOW: f32 = 0.06;     // 6% pickup threshold
pub const IDLE_DELAY_MS: f32 = 0.500;    // 500ms before transitioning to Idle

impl Default for CameraMediaState {
    fn default() -> Self {
        Self {
            camera_index: 0,
            camera_name: String::new(),
            enabled: false,
            vis_q: 0,
            vis_smoothed: 0.0,
            composite_order: 0,
            decode_state: CameraDecodeState::Idle,
            idle_timer: 0.0,
            last_decode_time: std::time::Instant::now(),
            vis_hold_timer: 0.0,
            midi_last_cc: 0,
            midi_pickup_active: false,
            last_user_change_ts: 0.0,
            last_midi_change_ts: 0.0,
        }
    }
}

impl CameraMediaState {
    /// Create a new CameraMediaState for a specific camera
    pub fn new(camera_index: u32, camera_name: String) -> Self {
        Self {
            camera_index,
            camera_name,
            composite_order: camera_index as i32, // Default order by index
            ..Default::default()
        }
    }

    /// Set visibility step (0-16) with rate limiting
    /// Returns true if changed
    pub fn set_visibility(&mut self, step: u8) -> bool {
        let step = step.min(16);
        if self.vis_q != step && self.vis_hold_timer <= 0.0 {
            self.vis_q = step;
            self.vis_hold_timer = VIS_HOLD_MS; // Rate limit next change
            true
        } else {
            false
        }
    }

    /// Set visibility step bypassing rate limit (for initialization)
    pub fn set_visibility_immediate(&mut self, step: u8) {
        self.vis_q = step.min(16);
    }

    /// Toggle enabled state
    pub fn toggle_enable(&mut self) {
        self.enabled = !self.enabled;
        if self.enabled {
            self.idle_timer = 0.0; // Reset idle timer on enable
        }
    }

    /// Get current visibility as normalized float (0.0-1.0)
    pub fn visibility_normalized(&self) -> f32 {
        self.vis_q as f32 / 16.0
    }

    /// Get alpha target (before smoothing, after normalization if applied externally)
    pub fn alpha_target(&self) -> f32 {
        if self.enabled {
            self.visibility_normalized()
        } else {
            0.0
        }
    }

    /// Update the smoothed visibility with RC filter (tau=120ms)
    /// Also updates decode state machine and rate-limit timer
    /// Returns true if decode state changed
    pub fn update_smoothing(&mut self, dt: f32) -> bool {
        // Update rate-limit timer
        self.vis_hold_timer = (self.vis_hold_timer - dt).max(0.0);

        let target = self.alpha_target();

        // RC filter: tau = 120ms (smooth feel for visibility transitions)
        const TAU: f32 = 0.120;
        let alpha = 1.0 - (-dt / TAU).exp();
        self.vis_smoothed += alpha * (target - self.vis_smoothed);

        // Clamp very small values to zero
        if self.vis_smoothed < 0.001 {
            self.vis_smoothed = 0.0;
        }

        // Update decode state machine
        let old_state = self.decode_state;
        self.decode_state = self.compute_decode_state(dt);

        old_state != self.decode_state
    }

    /// Compute the decode state based on current enabled/visibility
    fn compute_decode_state(&mut self, dt: f32) -> CameraDecodeState {
        if !self.enabled || self.vis_q == 0 {
            // Should transition to Idle, but with delay
            if self.decode_state != CameraDecodeState::Idle {
                self.idle_timer += dt;
                if self.idle_timer >= IDLE_DELAY_MS {
                    return CameraDecodeState::Idle;
                }
                // Stay in current state during transition
                return self.decode_state;
            }
            CameraDecodeState::Idle
        } else {
            // Reset idle timer when enabled
            self.idle_timer = 0.0;

            if self.vis_smoothed < 0.001 {
                // Warming up (enabled but smoothed value still near zero)
                CameraDecodeState::Warm
            } else {
                // Active (enabled, visibility > 0, smoothed > threshold)
                CameraDecodeState::Active
            }
        }
    }

    /// Should this camera be decoding frames?
    pub fn should_decode(&self) -> bool {
        matches!(self.decode_state, CameraDecodeState::Warm | CameraDecodeState::Active)
    }

    /// Should this camera upload frames to GPU?
    pub fn should_upload(&self) -> bool {
        self.decode_state == CameraDecodeState::Active
    }

    /// Update from MIDI CC with pickup mode and hysteresis
    /// Returns true if value was applied
    pub fn update_from_midi_cc(&mut self, cc_value: u8, current_time: f64) -> bool {
        let cc_step = (cc_value as f32 / 127.0 * 16.0).round() as u8;

        if self.midi_pickup_active {
            // Check if the knob has reached the current value (pickup)
            let current_step = self.vis_q;
            let diff = (cc_step as i16 - current_step as i16).abs();
            if diff <= MIDI_HYSTERESIS {
                // Pickup satisfied - now tracking
                self.midi_pickup_active = false;
                self.midi_last_cc = cc_value;
                self.last_midi_change_ts = current_time;
                return self.set_visibility(cc_step);
            }
            // Still waiting for pickup
            return false;
        }

        // Check hysteresis - only update if change is significant
        let last_step = (self.midi_last_cc as f32 / 127.0 * 16.0).round() as u8;
        let step_diff = (cc_step as i16 - last_step as i16).abs();
        if step_diff < MIDI_HYSTERESIS {
            return false;
        }

        // Normal tracking mode
        self.midi_last_cc = cc_value;
        self.last_midi_change_ts = current_time;
        self.set_visibility(cc_step)
    }

    /// Arm pickup mode (called when MIDI learn is initiated)
    pub fn arm_midi_pickup(&mut self) {
        self.midi_pickup_active = true;
    }
}

/// Select cameras for compositing based on scheduler and cap
/// Returns indices of cameras to composite this frame (sorted by composite_order)
pub fn select_cameras_for_compositing(
    states: &[(&u32, &CameraMediaState)],
    max_active: usize,
    scheduler_epoch: u32,
) -> Vec<u32> {
    // Filter to only active cameras with vis_smoothed > 0
    let mut active: Vec<_> = states
        .iter()
        .filter(|(_, s)| s.decode_state == CameraDecodeState::Active && s.vis_smoothed > 0.001)
        .map(|(idx, s)| (**idx, s.vis_smoothed, s.composite_order))
        .collect();

    if active.len() <= max_active {
        // All fit, sort by composite_order
        active.sort_by_key(|(_, _, order)| *order);
        return active.iter().map(|(idx, _, _)| *idx).collect();
    }

    // Need to select - sort by vis_smoothed descending, tie-break by scheduler
    active.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                // Round-robin tie-break using scheduler_epoch
                let hash_a = scheduler_epoch.wrapping_add(a.0);
                let hash_b = scheduler_epoch.wrapping_add(b.0);
                hash_a.cmp(&hash_b)
            })
    });

    // Take top max_active, then re-sort by composite_order
    let mut selected: Vec<_> = active.into_iter().take(max_active).collect();
    selected.sort_by_key(|(_, _, order)| *order);
    selected.iter().map(|(idx, _, _)| *idx).collect()
}

/// Normalize alpha targets if sum exceeds 1.0 (equalized mix)
/// Returns adjusted alpha for each camera index
pub fn normalize_alphas(
    states: &[(&u32, &CameraMediaState)],
    mode: NormalizeMode,
) -> std::collections::HashMap<u32, f32> {
    let mut result = std::collections::HashMap::new();

    let sum: f32 = states
        .iter()
        .filter(|(_, s)| s.enabled)
        .map(|(_, s)| s.alpha_target())
        .sum();

    for (idx, state) in states {
        let alpha = state.alpha_target();
        let adjusted = match mode {
            NormalizeMode::Off => alpha,
            NormalizeMode::EqualizedMix => {
                if sum > 1.0 {
                    alpha / sum
                } else {
                    alpha
                }
            }
        };
        result.insert(**idx, adjusted);
    }

    result
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use parking_lot::Mutex;
use anyhow::Result;
use std::collections::HashMap;

pub use state::*;
pub use capture::{CameraCapture, CameraFrame};
use detector::Detector;
use tracker::Tracker;

/// Camera device info
#[derive(Clone, Debug)]
pub struct CameraInfo {
    pub index: u32,
    pub name: String,
    pub resolution: (u32, u32),
    /// Stable device identifier (AVFoundation uniqueID on macOS)
    /// Used to open the correct physical camera regardless of enumeration order
    pub device_id: String,
}

/// Camera manager - orchestrates capture, detection, and tracking
pub struct CameraManager {
    config: TrackingConfig,
    active: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,

    // Shared state (read by render thread)
    output: Arc<Mutex<TrackingOutput>>,
    camera_states: Arc<Mutex<Vec<CameraState>>>,
    preview_frame: Arc<Mutex<PreviewFrame>>,

    // Shared track filter (can be updated while thread is running)
    track_filter: Arc<Mutex<String>>,

    // Available cameras
    available_cameras: Vec<CameraInfo>,
    enabled_cameras: Vec<u32>,  // Indices of cameras to use

    // Program camera (operator-selected, V1)
    program_camera: u32,
}

impl CameraManager {
    pub fn new(config: TrackingConfig) -> Self {
        let track_filter = config.track_filter.clone();
        Self {
            config,
            active: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            output: Arc::new(Mutex::new(TrackingOutput::default())),
            camera_states: Arc::new(Mutex::new(Vec::new())),
            preview_frame: Arc::new(Mutex::new(PreviewFrame::default())),
            track_filter: Arc::new(Mutex::new(track_filter)),
            available_cameras: Vec::new(),
            enabled_cameras: Vec::new(),
            program_camera: 0,
        }
    }

    /// Scan for available cameras
    pub fn scan_cameras(&mut self) -> Result<Vec<CameraInfo>> {
        self.available_cameras = capture::enumerate_cameras()?;
        Ok(self.available_cameras.clone())
    }

    /// Get list of available cameras
    pub fn available_cameras(&self) -> &[CameraInfo] {
        &self.available_cameras
    }

    /// Enable specific cameras for tracking
    pub fn set_enabled_cameras(&mut self, indices: Vec<u32>) {
        self.enabled_cameras = indices;
    }

    /// Set the program camera (which camera drives visualization)
    pub fn set_program_camera(&mut self, index: u32) {
        self.program_camera = index;
    }

    /// Get current program camera
    pub fn program_camera(&self) -> u32 {
        self.program_camera
    }

    /// Start the tracking system
    pub fn start(&mut self) -> Result<()> {
        if self.active.load(Ordering::Acquire) {
            return Ok(());
        }

        if self.enabled_cameras.is_empty() {
            if self.available_cameras.is_empty() {
                self.scan_cameras()?;
            }
            // Default: enable first camera
            if !self.available_cameras.is_empty() {
                self.enabled_cameras = vec![0];
            }
        }

        if self.enabled_cameras.is_empty() {
            anyhow::bail!("No cameras available");
        }

        log::info!("[Camera] Starting with {} cameras", self.enabled_cameras.len());

        self.active.store(true, Ordering::Release);

        let active = self.active.clone();
        let output = self.output.clone();
        let camera_states = self.camera_states.clone();
        let preview_frame = self.preview_frame.clone();
        let track_filter = self.track_filter.clone();
        let config = self.config.clone();
        let enabled_indices = self.enabled_cameras.clone();
        let program_camera = self.program_camera;

        let handle = thread::Builder::new()
            .name("camera-tracking".to_string())
            .spawn(move || {
                Self::tracking_thread(
                    active,
                    output,
                    camera_states,
                    preview_frame,
                    track_filter,
                    config,
                    enabled_indices,
                    program_camera,
                );
            })?;

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Stop the tracking system
    pub fn stop(&mut self) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }

        log::info!("[Camera] Stopping...");
        self.active.store(false, Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        log::info!("[Camera] Stopped");
    }

    /// Is tracking active?
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Get current tracking output (non-blocking)
    pub fn get_output(&self) -> TrackingOutput {
        self.output.lock().clone()
    }

    /// Get current preview frame (non-blocking)
    pub fn get_preview_frame(&self) -> PreviewFrame {
        self.preview_frame.lock().clone()
    }

    /// Get camera states for debug UI
    pub fn get_camera_states(&self) -> Vec<(u32, usize)> {
        self.camera_states
            .lock()
            .iter()
            .map(|s| (s.camera_id, s.tracks.len()))
            .collect()
    }

    /// Get config
    pub fn config(&self) -> &TrackingConfig {
        &self.config
    }

    /// Update config
    pub fn set_config(&mut self, config: TrackingConfig) {
        self.config = config;
    }

    /// Set track filter (empty = all classes)
    /// This updates the shared filter so the tracking thread sees changes immediately
    pub fn set_track_filter(&mut self, filter: String) {
        self.config.track_filter = filter.clone();
        *self.track_filter.lock() = filter;
    }

    /// Get current track filter
    pub fn track_filter(&self) -> String {
        self.track_filter.lock().clone()
    }

    fn tracking_thread(
        active: Arc<AtomicBool>,
        output: Arc<Mutex<TrackingOutput>>,
        camera_states: Arc<Mutex<Vec<CameraState>>>,
        preview_frame: Arc<Mutex<PreviewFrame>>,
        track_filter: Arc<Mutex<String>>,
        config: TrackingConfig,
        enabled_indices: Vec<u32>,
        program_camera: u32,
    ) {
        log::info!("[Camera] Thread started, {} cameras", enabled_indices.len());

        // Initialize cameras
        let mut captures: Vec<CameraCapture> = Vec::new();
        for &idx in &enabled_indices {
            match CameraCapture::new(idx, config.detection_resolution) {
                Ok(cap) => {
                    log::info!("[Camera] Opened camera {}", idx);
                    captures.push(cap);
                }
                Err(e) => {
                    log::error!("[Camera] Failed to open camera {}: {}", idx, e);
                }
            }
        }

        if captures.is_empty() {
            log::error!("[Camera] No cameras opened, exiting thread");
            active.store(false, Ordering::Release);
            return;
        }

        // Initialize detector with configured model path
        let detector_config = detector::DetectorConfig::default()
            .with_model_path(config.model_path.clone());

        let mut detector = match Detector::with_config(detector_config) {
            Ok(d) => {
                log::info!("[Camera] ONNX detector initialized");
                d
            }
            Err(e) => {
                log::error!("[Camera] Failed to create detector: {}", e);
                active.store(false, Ordering::Release);
                return;
            }
        };

        // Initialize per-camera trackers and state
        let mut trackers: Vec<Tracker> = captures
            .iter()
            .map(|c| Tracker::new(c.camera_id(), &config))
            .collect();

        {
            let mut states = camera_states.lock();
            states.clear();
            for cap in &captures {
                states.push(CameraState::new(cap.camera_id()));
            }
        }

        // Active selection state
        let mut selection = ActiveSelection::default();
        selection.camera_id = program_camera;

        // Timing
        let frame_interval = std::time::Duration::from_secs_f32(1.0 / config.detection_fps);
        let mut last_frame = std::time::Instant::now();

        while active.load(Ordering::Acquire) {
            let now = std::time::Instant::now();

            // Rate limit detection
            if now.duration_since(last_frame) < frame_interval {
                thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            last_frame = now;

            // Process each camera
            let mut total_tracks = 0u32;
            let mut program_best_track: Option<Track> = None;
            let mut program_frame: Option<capture::CameraFrame> = None;
            let mut program_detections: Vec<Detection> = Vec::new();
            let mut program_valid_tracks: Vec<Track> = Vec::new();

            for (i, (cap, tracker)) in captures.iter_mut().zip(trackers.iter_mut()).enumerate() {
                // Get frame (non-blocking)
                let frame = match cap.get_frame() {
                    Some(f) => f,
                    None => continue,
                };

                // Run detection
                let detections = match detector.detect(&frame, config.detection_threshold) {
                    Ok(dets) => dets,
                    Err(e) => {
                        log::warn!("[Camera] Detection error: {}", e);
                        continue;
                    }
                };

                // Update tracker
                tracker.update(&detections, config.smooth_alpha);

                // Update shared state
                {
                    let mut states = camera_states.lock();
                    if let Some(state) = states.get_mut(i) {
                        state.tracks = tracker.tracks().to_vec();
                        state.frame_count += 1;
                        state.last_detection_time = now;
                    }
                }

                total_tracks += tracker.tracks().len() as u32;

                // If this is the program camera, get best track and store frame for preview
                if cap.camera_id() == program_camera {
                    program_frame = Some(frame);
                    program_detections = detections;

                    // Collect ALL active tracks for UI display (thumbnails show everything)
                    program_valid_tracks = tracker.active_tracks()
                        .cloned()
                        .collect();

                    // Apply class filter for selecting best track (empty = all classes)
                    // Supports comma-separated list: "person, cup, phone"
                    // Read from shared filter so UI updates take effect immediately
                    let filter = track_filter.lock().clone();
                    let best = if filter.is_empty() {
                        // No filter - find best track of any class
                        tracker.best_track(config.min_track_age, config.max_missed_frames).cloned()
                    } else {
                        // Parse comma-separated filter list
                        let filter_classes: Vec<&str> = filter
                            .split(',')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();

                        // Filter by class name (match any in the list)
                        tracker.tracks()
                            .iter()
                            .filter(|t| t.is_valid(config.min_track_age, config.max_missed_frames))
                            .filter(|t| filter_classes.iter().any(|fc| t.class_name.eq_ignore_ascii_case(fc)))
                            .max_by(|a, b| a.score_smooth.partial_cmp(&b.score_smooth).unwrap())
                            .cloned()
                    };
                    if let Some(track) = best {
                        program_best_track = Some(track);
                    }
                }
            }

            // Update preview frame from program camera
            if let Some(frame) = program_frame {
                let mut preview = preview_frame.lock();
                // Downscale for UI preview (max 640 wide)
                let scale = (640.0 / frame.width as f32).min(1.0);
                let preview_w = (frame.width as f32 * scale) as u32;
                let preview_h = (frame.height as f32 * scale) as u32;

                if scale < 1.0 {
                    // Downscale the frame
                    preview.data = Self::downscale_frame(&frame.data, frame.width, frame.height, preview_w, preview_h);
                } else {
                    preview.data = frame.data.clone();
                }
                preview.width = preview_w;
                preview.height = preview_h;
                preview.detections = program_detections;

                // Generate track previews with thumbnails
                preview.tracks = program_valid_tracks.iter().map(|track| {
                    let thumbnail = Self::crop_thumbnail(
                        &frame.data, frame.width, frame.height,
                        &track.bbox_smooth, 32, 32
                    );
                    TrackPreview {
                        id: track.id,
                        class_name: track.class_name.clone(),
                        confidence: track.score_smooth,
                        bbox: track.bbox_smooth,
                        thumbnail,
                    }
                }).collect();
            }

            // Update active selection
            let dt = frame_interval.as_secs_f32();
            selection.hold_time += dt;
            selection.switch_cooldown = (selection.switch_cooldown - dt).max(0.0);

            if let Some(ref track) = program_best_track {
                // Check if we should switch tracks
                let should_switch = selection.track_id != track.id
                    && track.score_smooth >= config.confidence_threshold
                    && selection.switch_cooldown <= 0.0
                    && selection.hold_time >= config.min_hold_time;

                if should_switch || selection.track_id == track.id {
                    if should_switch {
                        selection.track_id = track.id;
                        selection.hold_time = 0.0;
                        selection.switch_cooldown = config.switch_cooldown;
                    }

                    // Update anchor with hysteresis
                    let new_index = ActiveSelection::quantize_to_n(track.bbox_smooth.x, config.anchor_n);
                    selection.update_anchor_with_hysteresis(new_index, config.anchor_n);
                    selection.confidence = track.score_smooth;
                }
            }

            // Update output
            {
                let mut out = output.lock();
                out.active = program_best_track.is_some();
                out.camera_id = program_camera;
                out.track_id = selection.track_id;
                out.anchor_index = selection.anchor_index;
                out.anchor_n = config.anchor_n;
                out.confidence = selection.confidence;
                out.num_tracks = total_tracks;
                out.num_cameras = captures.len() as u32;

                if let Some(ref track) = program_best_track {
                    out.position = track.bbox_smooth.center();
                    out.velocity = track.velocity;
                    out.class_name = Some(track.class_name.clone());
                    out.bbox = Some(track.bbox_smooth);
                } else {
                    out.class_name = None;
                    out.bbox = None;
                }
            }
        }

        // Cleanup
        for cap in &mut captures {
            cap.stop();
        }

        log::info!("[Camera] Thread stopped");
    }

    /// Downscale RGB frame using simple averaging
    fn downscale_frame(data: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
        let channels = if data.len() == (src_w * src_h * 4) as usize { 4 } else { 3 };
        let mut result = vec![0u8; (dst_w * dst_h * 3) as usize];

        let x_ratio = src_w as f32 / dst_w as f32;
        let y_ratio = src_h as f32 / dst_h as f32;

        for dy in 0..dst_h {
            for dx in 0..dst_w {
                let sx = (dx as f32 * x_ratio) as u32;
                let sy = (dy as f32 * y_ratio) as u32;

                let src_idx = ((sy * src_w + sx) * channels as u32) as usize;
                let dst_idx = ((dy * dst_w + dx) * 3) as usize;

                if src_idx + 2 < data.len() && dst_idx + 2 < result.len() {
                    result[dst_idx] = data[src_idx];
                    result[dst_idx + 1] = data[src_idx + 1];
                    result[dst_idx + 2] = data[src_idx + 2];
                }
            }
        }

        result
    }

    /// Crop and resize a region from the frame to a thumbnail
    fn crop_thumbnail(data: &[u8], src_w: u32, src_h: u32, bbox: &BBox, dst_w: u32, dst_h: u32) -> Vec<u8> {
        let channels = if data.len() == (src_w * src_h * 4) as usize { 4 } else { 3 };
        let mut result = vec![0u8; (dst_w * dst_h * 3) as usize];

        // Convert normalized bbox to pixel coords
        let x1 = ((bbox.x - bbox.w / 2.0) * src_w as f32).max(0.0) as u32;
        let y1 = ((bbox.y - bbox.h / 2.0) * src_h as f32).max(0.0) as u32;
        let x2 = ((bbox.x + bbox.w / 2.0) * src_w as f32).min(src_w as f32) as u32;
        let y2 = ((bbox.y + bbox.h / 2.0) * src_h as f32).min(src_h as f32) as u32;

        let crop_w = (x2 - x1).max(1);
        let crop_h = (y2 - y1).max(1);

        let x_ratio = crop_w as f32 / dst_w as f32;
        let y_ratio = crop_h as f32 / dst_h as f32;

        for dy in 0..dst_h {
            for dx in 0..dst_w {
                let sx = x1 + (dx as f32 * x_ratio) as u32;
                let sy = y1 + (dy as f32 * y_ratio) as u32;

                if sx < src_w && sy < src_h {
                    let src_idx = ((sy * src_w + sx) * channels as u32) as usize;
                    let dst_idx = ((dy * dst_w + dx) * 3) as usize;

                    if src_idx + 2 < data.len() && dst_idx + 2 < result.len() {
                        result[dst_idx] = data[src_idx];
                        result[dst_idx + 1] = data[src_idx + 1];
                        result[dst_idx + 2] = data[src_idx + 2];
                    }
                }
            }
        }

        result
    }
}

impl Drop for CameraManager {
    fn drop(&mut self) {
        self.stop();
    }
}

// ============================================================================
// Tracking Influence Aggregation (for UI control)
// ============================================================================

/// Per-camera feature values (extracted from tracker)
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraFeatures {
    /// Motion magnitude [0..1]
    pub motion_magnitude: f32,
    /// Centroid stability [0..1]
    pub centroid_stability: f32,
    /// Onset assist (beat correlation) [0..1]
    pub onset_assist: f32,
}

/// Tracking mix mode for aggregating multiple cameras
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TrackingMixMode {
    /// Weighted max with hold (prevents shimmer)
    #[default]
    WeightedMax,
}

/// Symmetry-safe output from tracking aggregation
/// These are the ONLY allowed render connections for camera tracking
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackingInfluenceOutput {
    /// Multiplies accent trigger probability [0..1]
    pub accent_gate_weight: f32,
    /// Scales symmetric radial mask [0..1]
    pub mask_amplitude: f32,
    /// Increases min hold times when low [0..1]
    pub stability_bias: f32,
}

/// Multi-camera tracking influence aggregator
/// Uses weighted_max with hold to prevent shimmer
#[derive(Debug)]
pub struct TrackingMix {
    pub mode: TrackingMixMode,
    /// Hold time in seconds (default 250ms)
    pub hold_ms: f32,
    /// Tie-break seed for deterministic selection
    pub tie_break_seed: u32,

    // Per-feature winner tracking: (camera_id, value, since)
    accent_winner: Option<(u32, f32, std::time::Instant)>,
    mask_winner: Option<(u32, f32, std::time::Instant)>,
    stability_winner: Option<(u32, f32, std::time::Instant)>,
}

impl Default for TrackingMix {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackingMix {
    pub fn new() -> Self {
        Self {
            mode: TrackingMixMode::WeightedMax,
            hold_ms: 0.250,
            tie_break_seed: 0xDEADBEEF,
            accent_winner: None,
            mask_winner: None,
            stability_winner: None,
        }
    }

    /// Aggregate camera influences into symmetry-safe outputs
    /// cameras: Vec of (camera_id, vis_smoothed, feature_values)
    pub fn aggregate(
        &mut self,
        cameras: &[(u32, f32, CameraFeatures)],
    ) -> TrackingInfluenceOutput {
        let now = std::time::Instant::now();

        if cameras.is_empty() {
            return TrackingInfluenceOutput::default();
        }

        // Compute weighted features per camera
        let weighted: Vec<(u32, f32, f32, f32)> = cameras
            .iter()
            .map(|(id, vis, feat)| {
                (
                    *id,
                    vis * feat.motion_magnitude,
                    vis * feat.centroid_stability,
                    vis * feat.onset_assist,
                )
            })
            .collect();

        // Update each winner with max-hold
        let accent = Self::select_winner_static(
            &weighted, 0, &mut self.accent_winner, now, self.hold_ms, self.tie_break_seed
        );
        let mask = Self::select_winner_static(
            &weighted, 1, &mut self.mask_winner, now, self.hold_ms, self.tie_break_seed
        );
        let stability = Self::select_winner_static(
            &weighted, 2, &mut self.stability_winner, now, self.hold_ms, self.tie_break_seed
        );

        // Quantize to 8 steps before output to avoid shimmer
        TrackingInfluenceOutput {
            accent_gate_weight: (accent * 8.0).round() / 8.0,
            mask_amplitude: (mask * 8.0).round() / 8.0,
            stability_bias: (stability * 8.0).round() / 8.0,
        }
    }

    fn select_winner_static(
        weighted: &[(u32, f32, f32, f32)],
        feature_idx: usize,
        winner: &mut Option<(u32, f32, std::time::Instant)>,
        now: std::time::Instant,
        hold_ms: f32,
        tie_break_seed: u32,
    ) -> f32 {
        // Get feature value by index
        let get_val = |w: &(u32, f32, f32, f32)| match feature_idx {
            0 => w.1,
            1 => w.2,
            _ => w.3,
        };

        // Find max with stable tie-breaking
        let (best_id, best_val) = weighted
            .iter()
            .map(|w| (w.0, get_val(w)))
            .max_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        // Tie-break by stable hash
                        let hash_a = tie_break_seed ^ a.0;
                        let hash_b = tie_break_seed ^ b.0;
                        hash_a.cmp(&hash_b)
                    })
            })
            .unwrap_or((0, 0.0));

        // Check hold
        if let Some((held_id, held_val, since)) = winner {
            if now.duration_since(*since).as_secs_f32() < hold_ms {
                // Still in hold period, keep current winner
                return *held_val;
            }
            // Only switch if new winner is different
            if *held_id == best_id {
                // Same winner, update value
                *winner = Some((best_id, best_val, *since));
                return best_val;
            }
        }

        // Update winner
        *winner = Some((best_id, best_val, now));
        best_val
    }

    /// Reset all winner states (call when cameras change)
    pub fn reset(&mut self) {
        self.accent_winner = None;
        self.mask_winner = None;
        self.stability_winner = None;
    }
}

// ============================================================================
// Camera Media Manager (Background Capture)
// ============================================================================

/// Manages camera capture for background media display
/// Separate from Vision tracking - purely for visual layers
pub struct CameraMediaManager {
    /// Active camera captures, keyed by camera index
    captures: HashMap<u32, CameraCapture>,
    /// Target decode FPS (from quality tier)
    target_fps: f32,
    /// Last frame times per camera for rate limiting
    last_frame_times: HashMap<u32, std::time::Instant>,
    /// Quality tier for decode settings
    quality_tier: CameraQualityTier,
    /// Cameras waiting to start after a stop (deferred to let hardware release)
    /// (camera_index, earliest_start_time)
    pending_starts: Vec<(u32, std::time::Instant)>,
    /// Stable device IDs for each camera index (populated from media states)
    device_ids: HashMap<u32, String>,
}

impl CameraMediaManager {
    /// Create a new camera media manager
    pub fn new() -> Self {
        Self {
            captures: HashMap::new(),
            target_fps: CameraQualityTier::Medium.decode_fps(),
            last_frame_times: HashMap::new(),
            quality_tier: CameraQualityTier::Medium,
            pending_starts: Vec::new(),
            device_ids: HashMap::new(),
        }
    }

    /// Set quality tier (affects decode FPS and resolution)
    pub fn set_quality_tier(&mut self, tier: CameraQualityTier) {
        self.quality_tier = tier;
        self.target_fps = tier.decode_fps();
    }

    /// Get current quality tier
    pub fn quality_tier(&self) -> CameraQualityTier {
        self.quality_tier
    }

    /// Register available cameras so the manager knows stable device IDs
    pub fn set_available_cameras(&mut self, cameras: &[CameraInfo]) {
        for cam in cameras {
            self.device_ids.insert(cam.index, cam.device_id.clone());
        }
    }

    /// Update which cameras should be capturing based on media states
    /// Call this each frame with the current UI state
    pub fn update_captures(&mut self, media_states: &HashMap<u32, CameraMediaState>) {
        // Determine which cameras should be capturing (enabled and vis > 0)
        let should_capture: std::collections::HashSet<u32> = media_states
            .iter()
            .filter(|(_, state)| state.should_decode())
            .map(|(idx, _)| *idx)
            .collect();

        // Stop captures that are no longer needed
        let to_stop: Vec<u32> = self.captures
            .keys()
            .filter(|idx| !should_capture.contains(idx))
            .cloned()
            .collect();

        let stopped_any = !to_stop.is_empty();
        for idx in to_stop {
            log::info!("[CameraMedia] Stopping capture for camera {}", idx);
            if let Some(mut cap) = self.captures.remove(&idx) {
                cap.stop();
            }
            self.last_frame_times.remove(&idx);
        }

        // Determine which cameras need starting
        let to_start: Vec<u32> = should_capture
            .iter()
            .filter(|idx| !self.captures.contains_key(idx))
            .filter(|idx| !self.pending_starts.iter().any(|(pi, _)| pi == *idx))
            .cloned()
            .collect();

        if !to_start.is_empty() {
            if stopped_any {
                // Defer start to let old capture thread release the hardware
                let start_at = std::time::Instant::now() + std::time::Duration::from_millis(300);
                for idx in to_start {
                    log::info!("[CameraMedia] Deferring start of camera {} (waiting for hardware release)", idx);
                    self.pending_starts.push((idx, start_at));
                }
            } else {
                // No cameras were stopped — start immediately
                for idx in to_start {
                    self.start_capture(idx);
                }
            }
        }

        // Process pending starts that are ready
        let now = std::time::Instant::now();
        let ready: Vec<u32> = self.pending_starts
            .iter()
            .filter(|(_, start_at)| now >= *start_at)
            .map(|(idx, _)| *idx)
            .collect();

        for idx in &ready {
            // Only start if still wanted (user might have changed selection during the delay)
            if should_capture.contains(idx) && !self.captures.contains_key(idx) {
                self.start_capture(*idx);
            }
        }
        self.pending_starts.retain(|(_, start_at)| now < *start_at);
    }

    /// Start a camera capture using stable device ID
    fn start_capture(&mut self, idx: u32) {
        let device_id = self.device_ids.get(&idx).cloned();
        log::info!("[CameraMedia] Starting capture for camera {} (device_id={:?})", idx, device_id);
        let resolution = match self.quality_tier {
            CameraQualityTier::Low => (320, 240),
            CameraQualityTier::Medium => (640, 480),
            CameraQualityTier::High => (1280, 720),
        };

        match CameraCapture::new_with_device_id(idx, resolution, device_id) {
            Ok(cap) => {
                self.captures.insert(idx, cap);
                self.last_frame_times.insert(idx, std::time::Instant::now());
            }
            Err(e) => {
                log::error!("[CameraMedia] Failed to start camera {}: {}", idx, e);
            }
        }
    }

    /// Get frame for a specific camera if available and rate-limited
    /// Returns None if camera not capturing or rate limit not reached
    pub fn get_frame(&mut self, camera_index: u32) -> Option<CameraFrame> {
        let capture = self.captures.get(&camera_index)?;

        // Rate limit frame retrieval to target FPS
        let now = std::time::Instant::now();
        let last_time = self.last_frame_times.get(&camera_index).copied()
            .unwrap_or(std::time::Instant::now() - std::time::Duration::from_secs(1));

        let frame_interval = std::time::Duration::from_secs_f32(1.0 / self.target_fps);
        if now.duration_since(last_time) < frame_interval {
            return None;
        }

        // Get frame from capture
        let frame = capture.get_frame()?;
        self.last_frame_times.insert(camera_index, now);
        Some(frame)
    }

    /// Get all available frames (for cameras that are ready)
    /// Returns vec of (camera_index, frame) pairs
    pub fn get_all_frames(&mut self) -> Vec<(u32, CameraFrame)> {
        let indices: Vec<u32> = self.captures.keys().cloned().collect();
        let mut frames = Vec::new();

        for idx in indices {
            if let Some(frame) = self.get_frame(idx) {
                frames.push((idx, frame));
            }
        }

        frames
    }

    /// Check if a camera is currently capturing
    pub fn is_capturing(&self, camera_index: u32) -> bool {
        self.captures.contains_key(&camera_index)
    }

    /// Get list of currently capturing camera indices
    pub fn capturing_cameras(&self) -> Vec<u32> {
        self.captures.keys().cloned().collect()
    }

    /// Stop all captures
    pub fn stop_all(&mut self) {
        for (_, mut cap) in self.captures.drain() {
            cap.stop();
        }
        self.last_frame_times.clear();
    }
}

impl Default for CameraMediaManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CameraMediaManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
