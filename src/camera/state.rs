//! Core types for multi-camera tracking system
//!
//! Defines the state structures for detections, tracks, and entities
//! that flow through the tracking pipeline.

/// Bounding box in normalized coordinates [0..1]
#[derive(Clone, Copy, Debug, Default)]
pub struct BBox {
    pub x: f32,      // center x
    pub y: f32,      // center y
    pub w: f32,      // width
    pub h: f32,      // height
}

impl BBox {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Intersection over Union
    pub fn iou(&self, other: &BBox) -> f32 {
        let x1 = (self.x - self.w / 2.0).max(other.x - other.w / 2.0);
        let y1 = (self.y - self.h / 2.0).max(other.y - other.h / 2.0);
        let x2 = (self.x + self.w / 2.0).min(other.x + other.w / 2.0);
        let y2 = (self.y + self.h / 2.0).min(other.y + other.h / 2.0);

        let inter_w = (x2 - x1).max(0.0);
        let inter_h = (y2 - y1).max(0.0);
        let inter_area = inter_w * inter_h;

        let self_area = self.w * self.h;
        let other_area = other.w * other.h;
        let union_area = self_area + other_area - inter_area;

        if union_area > 0.0 {
            inter_area / union_area
        } else {
            0.0
        }
    }

    /// Center point
    pub fn center(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// Area
    pub fn area(&self) -> f32 {
        self.w * self.h
    }
}

/// Load class names from a JSON file alongside the model
/// Returns empty Vec if file doesn't exist (will use generic "class_N" names)
pub fn load_classes_from_json(model_path: &std::path::Path) -> Vec<String> {
    // Look for classes.json in same directory as model
    let classes_path = match model_path.parent() {
        Some(p) => p.join("classes.json"),
        None => return Vec::new(),
    };

    if !classes_path.exists() {
        log::warn!("[Classes] No classes.json found at {:?}", classes_path);
        log::warn!("[Classes] Export your model with classes.json for proper labels");
        return Vec::new();
    }

    match std::fs::read_to_string(&classes_path) {
        Ok(contents) => {
            match serde_json::from_str::<Vec<String>>(&contents) {
                Ok(classes) => {
                    log::info!("[Classes] Loaded {} classes from {:?}: {:?}",
                        classes.len(), classes_path, &classes[..classes.len().min(5)]);
                    classes
                }
                Err(e) => {
                    log::error!("[Classes] Failed to parse classes.json: {}", e);
                    Vec::new()
                }
            }
        }
        Err(e) => {
            log::error!("[Classes] Failed to read classes.json: {}", e);
            Vec::new()
        }
    }
}

/// Raw detection from the detector
#[derive(Clone, Debug)]
pub struct Detection {
    pub class_id: usize,         // COCO class ID (0-79)
    pub class_name: String,      // Human-readable class name
    pub bbox: BBox,
    pub score: f32,
    pub embedding: Option<Vec<f32>>,  // V2: for re-ID
}

impl Detection {
    /// Create detection with class name (primary constructor)
    pub fn new(class_id: usize, class_name: String, bbox: BBox, score: f32) -> Self {
        Self {
            class_id,
            class_name,
            bbox,
            score,
            embedding: None,
        }
    }
}

/// Per-camera track with temporal state
#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub class_id: usize,             // COCO class ID
    pub class_name: String,          // Human-readable class name
    pub bbox: BBox,
    pub bbox_smooth: BBox,           // EMA-smoothed for stable output
    pub velocity: (f32, f32),        // Predicted motion (dx, dy per frame)
    pub score: f32,
    pub score_smooth: f32,           // EMA-smoothed confidence
    pub age: u32,                    // Frames since creation
    pub missed: u32,                 // Consecutive frames without match
    pub embedding: Option<Vec<f32>>, // V2: running average embedding
}

impl Track {
    pub fn new(id: u32, det: &Detection) -> Self {
        Self {
            id,
            class_id: det.class_id,
            class_name: det.class_name.clone(),
            bbox: det.bbox,
            bbox_smooth: det.bbox,
            velocity: (0.0, 0.0),
            score: det.score,
            score_smooth: det.score,
            age: 0,
            missed: 0,
            embedding: det.embedding.clone(),
        }
    }

    /// Update track with matched detection
    pub fn update(&mut self, det: &Detection, smooth_alpha: f32) {
        // Update velocity estimate
        self.velocity = (
            det.bbox.x - self.bbox.x,
            det.bbox.y - self.bbox.y,
        );

        // Update bbox
        self.bbox = det.bbox;

        // Smooth bbox with EMA
        self.bbox_smooth.x += smooth_alpha * (det.bbox.x - self.bbox_smooth.x);
        self.bbox_smooth.y += smooth_alpha * (det.bbox.y - self.bbox_smooth.y);
        self.bbox_smooth.w += smooth_alpha * (det.bbox.w - self.bbox_smooth.w);
        self.bbox_smooth.h += smooth_alpha * (det.bbox.h - self.bbox_smooth.h);

        // Smooth score
        self.score = det.score;
        self.score_smooth += smooth_alpha * (det.score - self.score_smooth);

        // Reset missed counter
        self.missed = 0;
        self.age += 1;

        // Update embedding if provided (running average)
        if let Some(ref new_emb) = det.embedding {
            if let Some(ref mut cur_emb) = self.embedding {
                for (c, n) in cur_emb.iter_mut().zip(new_emb.iter()) {
                    *c += 0.1 * (*n - *c); // Slow embedding update
                }
            } else {
                self.embedding = Some(new_emb.clone());
            }
        }
    }

    /// Predict next position using velocity
    pub fn predict(&self) -> BBox {
        BBox {
            x: self.bbox.x + self.velocity.0,
            y: self.bbox.y + self.velocity.1,
            w: self.bbox.w,
            h: self.bbox.h,
        }
    }

    /// Mark as missed (no detection matched this frame)
    pub fn mark_missed(&mut self) {
        self.missed += 1;
        self.age += 1;
        // Decay score when missed
        self.score_smooth *= 0.9;
    }

    /// Is this track valid for output?
    pub fn is_valid(&self, min_age: u32, max_missed: u32) -> bool {
        self.age >= min_age && self.missed <= max_missed
    }
}

/// Per-camera state
#[derive(Debug)]
pub struct CameraState {
    pub camera_id: u32,
    pub tracks: Vec<Track>,
    pub next_track_id: u32,
    pub frame_count: u64,
    pub last_detection_time: std::time::Instant,
}

impl CameraState {
    pub fn new(camera_id: u32) -> Self {
        Self {
            camera_id,
            tracks: Vec::with_capacity(16),
            next_track_id: 1,
            frame_count: 0,
            last_detection_time: std::time::Instant::now(),
        }
    }

    /// Get next track ID (monotonically increasing)
    pub fn next_id(&mut self) -> u32 {
        let id = self.next_track_id;
        self.next_track_id = self.next_track_id.wrapping_add(1);
        if self.next_track_id == 0 {
            self.next_track_id = 1; // Skip 0
        }
        id
    }

    /// Get the highest-confidence valid track
    pub fn best_track(&self, min_age: u32, max_missed: u32) -> Option<&Track> {
        self.tracks
            .iter()
            .filter(|t| t.is_valid(min_age, max_missed))
            .max_by(|a, b| a.score_smooth.partial_cmp(&b.score_smooth).unwrap())
    }
}

/// V2: Global entity for cross-camera identity
#[derive(Clone, Debug)]
pub struct GlobalEntity {
    pub global_id: u32,
    pub class_id: usize,
    pub class_name: String,
    pub canonical_embedding: Vec<f32>,
    pub best_camera_id: u32,
    pub best_track_id: u32,
    pub confidence: f32,
    pub last_seen: std::time::Instant,
}

/// Active entity selection state (what drives REACT)
#[derive(Clone, Debug, Default)]
pub struct ActiveSelection {
    pub camera_id: u32,
    pub track_id: u32,
    pub anchor_index: u32,       // Quantized position (0..N-1)
    pub confidence: f32,
    pub hold_time: f32,          // Time held at current selection
    pub switch_cooldown: f32,    // Time until switch allowed
}

impl ActiveSelection {
    /// Quantize track center X to gate index
    pub fn quantize_to_n(x: f32, n: u32) -> u32 {
        let clamped = x.clamp(0.0, 1.0);
        let idx = (clamped * n as f32).floor() as u32;
        idx.min(n - 1)
    }

    /// Update with hysteresis (max ±1 step per update)
    pub fn update_anchor_with_hysteresis(&mut self, new_index: u32, n: u32) {
        let current = self.anchor_index;
        if new_index > current && current < n - 1 {
            self.anchor_index = current + 1;
        } else if new_index < current && current > 0 {
            self.anchor_index = current - 1;
        }
        // If equal or at bounds, no change
    }
}

/// Tracking output for harmonic geometry integration
#[derive(Clone, Debug, Default)]
pub struct TrackingOutput {
    pub active: bool,
    pub camera_id: u32,
    pub track_id: u32,
    pub anchor_index: u32,       // 0..11 or 0..23
    pub anchor_n: u32,           // 12 or 24
    pub position: (f32, f32),    // Normalized center (0..1, 0..1)
    pub velocity: (f32, f32),    // Motion vector
    pub confidence: f32,         // Track confidence
    pub num_tracks: u32,         // Total tracks across cameras
    pub num_cameras: u32,        // Active cameras
    pub class_name: Option<String>,  // What was detected (e.g., "person", "cup")
    pub bbox: Option<BBox>,      // Bounding box of active track
}

/// Simplified track info for UI preview
#[derive(Clone, Debug)]
pub struct TrackPreview {
    pub id: u32,
    pub class_name: String,
    pub confidence: f32,
    pub bbox: BBox,              // Normalized bbox for cropping thumbnail
    pub thumbnail: Vec<u8>,      // Cropped RGB thumbnail (32x32)
}

/// Camera frame for UI preview (downscaled)
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub data: Vec<u8>,           // RGB data
    pub width: u32,
    pub height: u32,
    pub detections: Vec<Detection>,  // Current detections for overlay
    pub tracks: Vec<TrackPreview>,   // Active tracks with thumbnails
}

impl Default for PreviewFrame {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            width: 0,
            height: 0,
            detections: Vec::new(),
            tracks: Vec::new(),
        }
    }
}

impl TrackingOutput {
    /// Convert anchor_index to phase (0..1) for gate system
    pub fn anchor_phase(&self) -> f32 {
        if self.anchor_n == 0 {
            return 0.0;
        }
        (self.anchor_index as f32 + 0.5) / self.anchor_n as f32
    }
}

/// Configuration for the tracking system
#[derive(Clone, Debug)]
pub struct TrackingConfig {
    // Detection
    pub detection_fps: f32,
    pub detection_resolution: (u32, u32),
    pub detection_threshold: f32,
    pub model_path: Option<std::path::PathBuf>,  // Path to ONNX model

    // Tracking
    pub max_tracks_per_camera: usize,
    pub iou_threshold: f32,
    pub min_track_age: u32,
    pub max_missed_frames: u32,
    pub smooth_alpha: f32,

    // Selection
    pub anchor_n: u32,           // 12 or 24 gates
    pub min_hold_time: f32,
    pub switch_cooldown: f32,
    pub confidence_threshold: f32,

    // Class filter (empty = track all, otherwise track only matching class)
    pub track_filter: String,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            detection_fps: 12.0,
            detection_resolution: (480, 360),
            detection_threshold: 0.5,
            model_path: None,  // Will auto-detect

            max_tracks_per_camera: 12,
            iou_threshold: 0.3,
            min_track_age: 3,
            max_missed_frames: 5,
            smooth_alpha: 0.3,

            anchor_n: 12,
            min_hold_time: 0.5,
            switch_cooldown: 1.0,
            confidence_threshold: 0.6,

            track_filter: String::new(),  // Empty = track all classes
        }
    }
}

impl TrackingConfig {
    /// Low quality preset (1 camera, lower resolution)
    pub fn low() -> Self {
        Self {
            detection_fps: 10.0,
            detection_resolution: (320, 240),
            max_tracks_per_camera: 8,
            ..Default::default()
        }
    }

    /// High quality preset (more cameras, higher resolution)
    pub fn high() -> Self {
        Self {
            detection_fps: 15.0,
            detection_resolution: (640, 480),
            max_tracks_per_camera: 16,
            ..Default::default()
        }
    }

    /// Find the ONNX model, checking Resources for bundled apps
    /// Prefers LVIS model (1203 classes), falls back to world model (custom classes)
    pub fn find_model_path() -> Option<std::path::PathBuf> {
        use std::path::PathBuf;

        // Model names in preference order (LVIS first for more classes)
        let model_names = [
            "yolov8m-worldv2-lvis.onnx",  // 1203 LVIS classes
            "yolov8m-world-lvis.onnx",     // Alternative LVIS naming
            "yolov8m-world.onnx",          // Original custom classes
        ];

        for model_name in &model_names {
            // Check configured path first (relative to cwd)
            let default_path = PathBuf::from(format!("models/{}", model_name));
            if default_path.exists() {
                log::info!("[Model] Found at: {:?}", default_path);
                return Some(default_path);
            }

            // Check compile-time path (for development builds)
            let compile_time_path = PathBuf::from(format!(
                "{}/models/{}", env!("CARGO_MANIFEST_DIR"), model_name
            ));
            if compile_time_path.exists() {
                log::info!("[Model] Found at compile-time path: {:?}", compile_time_path);
                return Some(compile_time_path);
            }

            if let Ok(exe) = std::env::current_exe() {
                // Check in Resources for bundled macOS app
                if let Some(macos_dir) = exe.parent() {
                    let resources = macos_dir.parent().map(|p| p.join("Resources"));
                    if let Some(ref res) = resources {
                        let bundled = res.join(format!("models/{}", model_name));
                        if bundled.exists() {
                            log::info!("[Model] Found in bundle: {:?}", bundled);
                            return Some(bundled);
                        }
                    }
                }

                // Check for development builds
                if let Some(parent) = exe.parent() {
                    if let Some(target_dir) = parent.parent() {
                        if let Some(repo_root) = target_dir.parent() {
                            let dev_model = repo_root.join(format!("models/{}", model_name));
                            if dev_model.exists() {
                                log::info!("[Model] Found at dev path: {:?}", dev_model);
                                return Some(dev_model);
                            }
                        }
                    }
                }
            }
        }

        None
    }
}