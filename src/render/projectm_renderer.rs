//! projectM renderer for REACT
//!
//! Renders Milkdrop-style visualizations directly to the OpenGL context.
//! Much simpler than the React version since we don't need IPC or texture streaming.

use anyhow::{anyhow, Result};
use eframe::glow::{self, HasContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::PathBuf;
use std::collections::HashMap;
use parking_lot::{Mutex, RwLock};

use super::theme::{ThemeRenderer, ThemePreset, AudioAnalysis, RendererType};

// projectM bindings
use projectm::core::{ProjectM, STEREO};

/// Thread-safe double-buffered audio for transferring samples between audio and render threads
/// Uses swap instead of copy to avoid allocation in hot path
struct AudioBuffer {
    /// Write buffer (audio thread writes here)
    write_buf: Vec<f32>,
    /// Read buffer (render thread reads from here)
    read_buf: Vec<f32>,
    /// Length of valid data in write buffer
    write_len: usize,
    /// Length of valid data in read buffer (after swap)
    read_len: usize,
    /// Flag indicating new data is available
    has_new_data: bool,
}

impl AudioBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            write_buf: vec![0.0; capacity],
            read_buf: vec![0.0; capacity],
            write_len: 0,
            read_len: 0,
            has_new_data: false,
        }
    }

    /// Add samples to write buffer (called from audio thread)
    fn push(&mut self, sample_left: f32, sample_right: f32) {
        let idx = self.write_len;
        if idx + 2 <= self.write_buf.len() {
            self.write_buf[idx] = sample_left;
            self.write_buf[idx + 1] = sample_right;
            self.write_len += 2;
            self.has_new_data = true;
        }
    }

    /// Swap buffers and get read access (called from render thread)
    /// Returns (slice, samples_per_channel) or None if no new data
    fn swap_and_get(&mut self) -> Option<(&[f32], u32)> {
        if !self.has_new_data || self.write_len == 0 {
            return None;
        }

        // Swap buffers (just swap pointers, no copy!)
        std::mem::swap(&mut self.write_buf, &mut self.read_buf);
        self.read_len = self.write_len;
        self.write_len = 0;
        self.has_new_data = false;

        // Return slice of valid data
        // samples_per_channel = total floats / 2 (stereo)
        let samples_per_channel = (self.read_len / 2) as u32;
        Some((&self.read_buf[..self.read_len], samples_per_channel))
    }
}

/// Preset information
#[derive(Clone, Debug)]
pub struct PresetInfo {
    pub name: String,
    pub path: String,
    pub category: String,
    /// Subfolder path within category (e.g., "City" for "Geometric/City/preset.milk")
    pub subcategory: Option<String>,
}

/// Container for scanned preset data - shared between threads
#[derive(Default)]
struct PresetData {
    presets: Vec<PresetInfo>,
    categories: Vec<String>,
    by_category: HashMap<String, Vec<PresetInfo>>,
    transition_presets: Vec<String>,
}

/// projectM renderer
pub struct ProjectMRenderer {
    gl: Arc<glow::Context>,
    projectm: Option<ProjectM>,
    width: u32,
    height: u32,
    preset_path: PathBuf,

    // Preset cache - shared with background scanning thread
    preset_data: Arc<RwLock<PresetData>>,
    /// Flag indicating preset scanning is complete (atomic for lock-free reads)
    presets_loaded: Arc<AtomicBool>,
    current_preset: Option<String>,

    // Thread-safe audio buffer (protected by mutex for audio→render thread transfer)
    // Using parking_lot::Mutex which is optimized for short critical sections
    audio_buffer: Arc<Mutex<AudioBuffer>>,

    // Transition support
    pending_preset: Option<String>,       // Preset to load after transition
    transition_start: Option<std::time::Instant>,  // When transition started
    transition_duration: f32,             // Seconds to wait for transition
    transitions_enabled: bool,            // Whether to use transitions

    /// Skip frames after loading a preset to avoid texture crashes
    skip_frames: u32,

    initialized: bool,
}

impl ProjectMRenderer {
    /// Create a new projectM renderer
    pub fn new(_gl: Arc<glow::Context>) -> Result<Self> {
        log::info!("[projectM] Creating renderer");

        // Find presets directory
        let preset_path = Self::find_presets_dir()?;
        log::info!("[projectM] Using presets from: {:?}", preset_path);

        // Create shared preset data for async loading
        let preset_data = Arc::new(RwLock::new(PresetData::default()));
        let presets_loaded = Arc::new(AtomicBool::new(false));

        let mut renderer = Self {
            gl: _gl,
            projectm: None,
            width: 1920,
            height: 1080,
            preset_path: preset_path.clone(),
            preset_data: preset_data.clone(),
            presets_loaded: presets_loaded.clone(),
            current_preset: None,
            // Thread-safe audio buffer - 8192 floats = 4096 stereo samples
            // This is enough for ~4 audio callbacks at 960 samples each
            audio_buffer: Arc::new(Mutex::new(AudioBuffer::new(8192))),
            // Transition support
            pending_preset: None,
            transition_start: None,
            transition_duration: 1.5, // 1.5 second transition
            transitions_enabled: true,
            // Skip a few frames after loading preset to let textures initialize
            skip_frames: 0,
            initialized: false,
        };

        // Spawn background thread to scan presets (non-blocking)
        // This prevents the ~1 minute pause after loading screen for 10k+ presets
        std::thread::spawn(move || {
            log::info!("[projectM] Background preset scan started");
            let start = std::time::Instant::now();

            let mut data = PresetData::default();

            // Scan main presets
            Self::scan_presets_into(&preset_path, &mut data);

            // Scan transition presets
            Self::scan_transition_presets_into(&preset_path, &mut data);

            // Update shared data
            {
                let mut shared = preset_data.write();
                *shared = data;
            }

            // Signal completion
            presets_loaded.store(true, Ordering::Release);

            log::info!("[projectM] Background preset scan complete: {} presets in {} categories ({:?})",
                preset_data.read().presets.len(),
                preset_data.read().categories.len(),
                start.elapsed());
        });

        // Initialize projectM immediately (don't wait for presets)
        renderer.init_projectm()?;

        Ok(renderer)
    }

    /// Find the presets directory
    ///
    /// Search order (first match wins):
    /// 1. Relative to executable (for distributed apps)
    /// 2. macOS .app bundle Resources folder
    /// 3. Relative to working directory (development)
    fn find_presets_dir() -> Result<PathBuf> {
        let mut possible_paths: Vec<PathBuf> = Vec::new();

        // Get executable location for runtime-relative paths
        if let Ok(exe) = std::env::current_exe() {
            // Resolve symlinks to get actual executable location
            let exe_resolved = exe.canonicalize().unwrap_or(exe);
            if let Some(exe_dir) = exe_resolved.parent() {
                // 1. Presets alongside executable (standalone distribution)
                possible_paths.push(exe_dir.join("presets"));

                // 2. macOS .app bundle: Contents/Resources/presets
                possible_paths.push(exe_dir.join("../Resources/presets"));

                // 3. Development: executable is in target/release or target/debug
                //    Go up to the repo root to find presets/
                if let Some(target_dir) = exe_dir.parent() {
                    if let Some(repo_root) = target_dir.parent() {
                        possible_paths.push(repo_root.join("presets"));
                    }
                }
            }
        }

        // 4. Relative to working directory (cargo run from the repo root)
        possible_paths.push(PathBuf::from("presets"));

        log::info!("[projectM] Searching for presets in: {:?}", possible_paths);

        for path in &possible_paths {
            if path.exists() && path.is_dir() {
                let resolved = path.canonicalize().unwrap_or_else(|_| path.clone());
                log::info!("[projectM] Found presets at: {:?}", resolved);
                return Ok(resolved);
            }
        }

        Err(anyhow!("Could not find presets directory. Searched {} locations", possible_paths.len()))
    }

    /// Scan presets directory recursively (instance method - triggers rescan)
    fn scan_presets(&mut self) {
        log::info!("[projectM] Rescanning presets from {:?}", self.preset_path);

        let mut data = PresetData::default();
        Self::scan_presets_into(&self.preset_path, &mut data);
        Self::scan_transition_presets_into(&self.preset_path, &mut data);

        // Update shared data
        {
            let mut shared = self.preset_data.write();
            *shared = data;
        }
        self.presets_loaded.store(true, Ordering::Release);

        let shared = self.preset_data.read();
        log::info!("[projectM] Rescan found {} presets in {} categories",
            shared.presets.len(), shared.categories.len());
    }

    /// Scan presets into a PresetData struct (static - used by background thread)
    fn scan_presets_into(preset_path: &PathBuf, data: &mut PresetData) {
        log::info!("[projectM] Scanning presets from {:?}", preset_path);

        data.presets.clear();
        data.categories.clear();
        data.by_category.clear();

        let mut category_set = std::collections::HashSet::new();

        // Scan top-level category directories
        if let Ok(entries) = std::fs::read_dir(preset_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let category = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string();

                    // Skip hidden directories and transition folder
                    if category.starts_with('.') || category.starts_with("! ") {
                        continue;
                    }

                    category_set.insert(category.clone());

                    // Recursively scan all presets in this category, passing category base path
                    Self::scan_presets_recursive_into(&path, &category, &path, data);
                }
            }
        }

        data.categories = category_set.into_iter().collect();
        data.categories.sort();

        log::info!("[projectM] Found {} presets in {} categories",
            data.presets.len(), data.categories.len());
    }

    /// Recursively scan a directory for preset files (static version)
    /// category_base: the root directory of this category (for computing subcategory)
    fn scan_presets_recursive_into(dir: &std::path::Path, category: &str, category_base: &std::path::Path, data: &mut PresetData) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                if path.is_dir() {
                    // Skip hidden directories
                    if path.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.starts_with('.'))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    // Recurse into subdirectory
                    Self::scan_presets_recursive_into(&path, category, category_base, data);
                } else if let Some(ext) = path.extension() {
                    if ext == "milk" || ext == "prjm" {
                        let name = path.file_stem()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                            .to_string();

                        // Compute subcategory from relative path
                        let subcategory = if let Some(parent) = path.parent() {
                            parent.strip_prefix(category_base)
                                .ok()
                                .and_then(|rel| {
                                    let rel_str = rel.to_string_lossy().to_string();
                                    if rel_str.is_empty() {
                                        None
                                    } else {
                                        Some(rel_str)
                                    }
                                })
                        } else {
                            None
                        };

                        let info = PresetInfo {
                            name: name.clone(),
                            path: path.to_string_lossy().to_string(),
                            category: category.to_string(),
                            subcategory,
                        };

                        data.presets.push(info.clone());
                        data.by_category
                            .entry(category.to_string())
                            .or_insert_with(Vec::new)
                            .push(info);
                    }
                }
            }
        }
    }

    /// Scan for transition presets into PresetData (static - used by background thread)
    fn scan_transition_presets_into(preset_path: &PathBuf, data: &mut PresetData) {
        data.transition_presets.clear();

        // Look for "! Transition" folder
        let transition_dir = preset_path.join("! Transition");
        if transition_dir.exists() && transition_dir.is_dir() {
            Self::scan_transition_dir_into(&transition_dir, data);
        }

        log::info!("[projectM] Found {} transition presets", data.transition_presets.len());
    }

    /// Recursively scan transition directory (static version)
    fn scan_transition_dir_into(dir: &std::path::Path, data: &mut PresetData) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    Self::scan_transition_dir_into(&path, data);
                } else if let Some(ext) = path.extension() {
                    if ext == "milk" || ext == "prjm" {
                        data.transition_presets.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    /// Check if presets have finished loading
    pub fn presets_ready(&self) -> bool {
        self.presets_loaded.load(Ordering::Acquire)
    }

    /// Update transition state - call each frame
    pub fn update_transition(&mut self) {
        // Check if a transition is in progress
        if let Some(start_time) = self.transition_start {
            let elapsed = start_time.elapsed().as_secs_f32();

            // If transition time has elapsed, load the pending preset
            if elapsed >= self.transition_duration {
                if let Some(pending) = self.pending_preset.take() {
                    log::info!("[projectM] Transition complete, loading: {}", pending);
                    self.transition_start = None;

                    // Load the actual preset (bypass transition logic)
                    if let Some(ref pm) = self.projectm {
                        pm.set_preset_locked(true);
                        pm.load_preset_file(&pending, false);
                        self.current_preset = Some(pending);
                        pm.set_preset_locked(true);

                        // Skip frames to let textures initialize (needs more time for GPU upload)
                        self.skip_frames = 30;
                    }
                }
            }
        }
    }

    /// Get a random transition preset
    fn get_random_transition(&self) -> Option<String> {
        let data = self.preset_data.read();
        if data.transition_presets.is_empty() {
            return None;
        }
        use rand::Rng;
        let idx = rand::thread_rng().gen_range(0..data.transition_presets.len());
        Some(data.transition_presets[idx].clone())
    }

    /// Check if transitions are enabled and available
    pub fn has_transitions(&self) -> bool {
        let data = self.preset_data.read();
        self.transitions_enabled && !data.transition_presets.is_empty()
    }

    /// Enable or disable transitions
    pub fn set_transitions_enabled(&mut self, enabled: bool) {
        self.transitions_enabled = enabled;
        log::info!("[projectM] Transitions {}", if enabled { "enabled" } else { "disabled" });
    }

    /// Initialize projectM library
    fn init_projectm(&mut self) -> Result<()> {
        log::info!("[projectM] Initializing library at {}x{}", self.width, self.height);

        // Create projectM instance
        let pm = ProjectM::create();

        // Set window size
        pm.set_window_size(self.width as usize, self.height as usize);

        // CRITICAL: Set preset duration to prevent rapid auto-switching
        // Default is often very short which causes crashes
        pm.set_preset_duration(30.0);  // 30 seconds between presets
        pm.set_soft_cut_duration(3.0);  // 3 second smooth transitions

        // Set texture search paths for presets that use external textures
        let texture_paths = vec![
            self.preset_path.join("textures").to_string_lossy().to_string(),
            self.preset_path.to_string_lossy().to_string(),
        ];
        log::info!("[projectM] Setting texture search paths: {:?}", texture_paths);
        pm.set_texture_search_paths(&texture_paths, texture_paths.len());

        // Lock preset initially to prevent any auto-switching during render
        // We'll manually control preset changes
        pm.set_preset_locked(true);

        self.projectm = Some(pm);
        self.initialized = true;

        // Load a random initial preset
        self.set_random_preset();

        log::info!("[projectM] Initialization complete (preset locked, duration=30s)");
        Ok(())
    }

    /// Set render size
    pub fn set_size(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }

        log::info!("[projectM] Resizing from {}x{} to {}x{}", self.width, self.height, width, height);
        self.width = width;
        self.height = height;

        if let Some(ref pm) = self.projectm {
            pm.set_window_size(width as usize, height as usize);
        }
    }

    /// Render a frame to the currently bound FBO
    pub fn render_frame(&mut self) {
        if !self.initialized {
            return;
        }

        // Skip frames after loading a preset to let textures initialize
        // This prevents crashes when a preset references textures that aren't ready
        if self.skip_frames > 0 {
            self.skip_frames -= 1;
            log::debug!("[projectM] Skipping frame after preset load ({} remaining)", self.skip_frames);
            return;
        }

        if let Some(ref pm) = self.projectm {
            // Track frame count for debugging (only in debug builds)
            #[cfg(debug_assertions)]
            static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            #[cfg(debug_assertions)]
            let frame = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Feed any buffered audio to projectM BEFORE rendering
            // Lock briefly, swap buffers (no allocation!), then release before FFI call
            let audio_slice = {
                let mut buffer = self.audio_buffer.lock();
                buffer.swap_and_get().map(|(slice, _count)| {
                    // Copy the slice since we need to release the lock before calling projectM
                    slice.to_vec()
                })
            };  // Mutex released here

            if let Some(samples) = audio_slice {
                #[cfg(debug_assertions)]
                if frame % 60 == 0 {
                    let max_amp = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                    log::debug!("[projectM/Render] Frame {}: Feeding {} samples, max: {:.3}",
                        frame, samples.len(), max_amp);
                }

                // Get max samples allowed by projectM (total floats in slice)
                let max_samples = ProjectM::pcm_get_max_samples() as usize;

                // Feed audio in chunks if needed
                for chunk in samples.chunks(max_samples) {
                    pm.pcm_add_float(chunk, STEREO);
                }
            }

            // Render projectM frame
            pm.render_frame();
        }
    }

    /// Render a frame to a specific FBO (using projectM 4.2+ API)
    /// Note: The new projectm crate doesn't expose FBO rendering directly,
    /// so we use the standard render_frame which renders to the current FBO
    pub fn render_frame_to_fbo(&mut self, _fbo_id: u32) {
        // Just delegate to render_frame - caller should bind FBO first
        self.render_frame();
    }

    /// Load a specific preset
    pub fn load_preset(&mut self, path: &str) -> Result<()> {
        if let Some(ref pm) = self.projectm {
            log::info!("[projectM] Loading preset: {}", path);
            pm.load_preset_file(path, false);  // smooth transition = false
            self.current_preset = Some(path.to_string());
            Ok(())
        } else {
            Err(anyhow!("projectM not initialized"))
        }
    }

    /// Set a random preset (safe - handles locking)
    /// Returns (category, preset_name) if successful
    pub fn set_random_preset(&mut self) -> Option<(String, String)> {
        // Get preset info under read lock, then release before loading
        let (path, name, category) = {
            let data = self.preset_data.read();
            if data.presets.is_empty() {
                log::warn!("[projectM] No presets available (loading: {})", !self.presets_loaded.load(Ordering::Acquire));
                return None;
            }

            use rand::Rng;
            let idx = rand::thread_rng().gen_range(0..data.presets.len());

            // Clone path and info to avoid borrow conflict
            (
                data.presets[idx].path.clone(),
                data.presets[idx].name.clone(),
                data.presets[idx].category.clone(),
            )
        };

        log::info!("[projectM] Random preset: {} ({})", name, category);

        if let Err(e) = self.load_preset_safe(&path) {
            log::error!("[projectM] Failed to load preset: {}", e);
            return None;
        }

        Some((category, name))
    }

    /// Load preset safely with proper lock handling to avoid texture corruption
    fn load_preset_safe(&mut self, path: &str) -> Result<()> {
        if self.projectm.is_none() {
            return Err(anyhow!("projectM not initialized"));
        }

        // Check if transitions are enabled and we have transition presets
        let has_transitions = {
            let data = self.preset_data.read();
            self.transitions_enabled && !data.transition_presets.is_empty()
        };

        if has_transitions {
            // Don't start a new transition if one is already in progress
            if self.pending_preset.is_some() {
                log::info!("[projectM] Transition already in progress, queueing: {}", path);
                self.pending_preset = Some(path.to_string());
                return Ok(());
            }

            // Get a random transition preset
            if let Some(transition_path) = self.get_random_transition() {
                log::info!("[projectM] Starting transition with: {}",
                    std::path::Path::new(&transition_path).file_name().unwrap_or_default().to_string_lossy());

                // Store the target preset to load after transition
                self.pending_preset = Some(path.to_string());
                self.transition_start = Some(std::time::Instant::now());

                // Load the transition preset
                if let Some(ref pm) = self.projectm {
                    // Clear audio buffer
                    {
                        let mut buffer = self.audio_buffer.lock();
                        buffer.write_len = 0;
                        buffer.read_len = 0;
                        buffer.has_new_data = false;
                    }

                    pm.set_preset_locked(true);
                    pm.load_preset_file(&transition_path, false);
                    pm.set_preset_locked(true);

                    // Skip frames to let textures initialize (needs more time for GPU upload)
                    self.skip_frames = 30;
                }

                return Ok(());
            }
        }

        // No transitions - load directly
        if let Some(ref pm) = self.projectm {
            log::info!("[projectM] Loading preset: {}", path);

            // Clear audio buffer before loading to ensure fresh audio state
            {
                let mut buffer = self.audio_buffer.lock();
                buffer.write_len = 0;
                buffer.read_len = 0;
                buffer.has_new_data = false;
            }

            pm.set_preset_locked(true);
            pm.load_preset_file(path, false);
            self.current_preset = Some(path.to_string());
            pm.set_preset_locked(true);

            // Skip frames to let textures initialize (needs more time for GPU upload)
            self.skip_frames = 30;

            log::info!("[projectM] Preset loaded successfully, audio buffer cleared");
        }

        Ok(())
    }

    /// Get available categories
    pub fn get_categories(&self) -> Vec<String> {
        let data = self.preset_data.read();
        data.categories.clone()
    }

    /// Get presets in a category
    pub fn get_presets_in_category(&self, category: &str) -> Vec<String> {
        let data = self.preset_data.read();
        data.by_category
            .get(category)
            .map(|presets| presets.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Get the full path for a preset by category and name
    pub fn get_preset_path(&self, category: &str, name: &str) -> Option<String> {
        let data = self.preset_data.read();
        data.by_category
            .get(category)
            .and_then(|presets| presets.iter().find(|p| p.name == name))
            .map(|p| p.path.clone())
    }

    /// Get the subcategory for a preset by category and name
    pub fn get_preset_subcategory(&self, category: &str, name: &str) -> Option<String> {
        let data = self.preset_data.read();
        data.by_category
            .get(category)
            .and_then(|presets| presets.iter().find(|p| p.name == name))
            .and_then(|p| p.subcategory.clone())
    }

    /// Load a preset by category and name
    pub fn load_preset_by_name(&mut self, category: &str, name: &str) -> Result<()> {
        if let Some(path) = self.get_preset_path(category, name) {
            self.load_preset_safe(&path)
        } else {
            Err(anyhow!("Preset not found: {} in {}", name, category))
        }
    }

    /// Add audio samples for visualization - buffers for render thread to feed
    /// This is called from the audio capture thread, so it must be thread-safe
    /// PERFORMANCE: Takes &self (not &mut self) to avoid locking the entire RenderPipeline
    pub fn add_audio_samples(&self, samples: &[f32]) {
        if !self.initialized {
            return;
        }

        // Lock the buffer for the duration of the write
        // parking_lot::Mutex is optimized for short critical sections like this
        let mut buffer = self.audio_buffer.lock();

        // Input is interleaved stereo [L, R, L, R, ...] — push as stereo pairs
        for chunk in samples.chunks_exact(2) {
            buffer.push(chunk[0], chunk[1]);
        }
    }

    /// Clean up resources
    pub fn cleanup(&mut self) {
        log::info!("[projectM] Cleaning up");

        // ProjectM cleanup is handled automatically when the handle is dropped
        self.projectm = None;
        self.initialized = false;
    }

    /// Convert internal PresetInfo to ThemePreset
    fn to_theme_preset(&self, info: &PresetInfo) -> ThemePreset {
        ThemePreset {
            id: info.path.clone(),
            name: info.name.clone(),
            category: info.category.clone(),
            subcategory: info.subcategory.clone(),
            renderer_type: Some(RendererType::ProjectM),
        }
    }
}

// === ThemeRenderer trait implementation ===

impl ThemeRenderer for ProjectMRenderer {
    fn renderer_type(&self) -> RendererType {
        RendererType::ProjectM
    }

    fn init(&mut self, _gl: Arc<glow::Context>) -> Result<()> {
        // Already initialized in new()
        // This could be used for re-initialization if needed
        if !self.initialized {
            self.init_projectm()?;
        }
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn update(&mut self, audio: &AudioAnalysis, _dt: f32) {
        // Feed audio samples to projectM buffer
        if !audio.samples.is_empty() {
            self.add_audio_samples(&audio.samples);
        }
        // Update transition state
        self.update_transition();
    }

    fn render(&mut self, _width: u32, _height: u32) {
        self.render_frame();
    }

    fn set_size(&mut self, width: u32, height: u32) {
        // Call existing set_size method
        ProjectMRenderer::set_size(self, width, height);
    }

    fn cleanup(&mut self) {
        // Call existing cleanup method
        ProjectMRenderer::cleanup(self);
    }

    fn scan_presets(&mut self) -> Vec<ThemePreset> {
        self.scan_presets();
        let data = self.preset_data.read();
        data.presets.iter().map(|p| self.to_theme_preset(p)).collect()
    }

    fn get_categories(&self) -> Vec<String> {
        // Call existing method
        ProjectMRenderer::get_categories(self)
    }

    fn get_presets_in_category(&self, category: &str) -> Vec<ThemePreset> {
        let data = self.preset_data.read();
        data.by_category
            .get(category)
            .map(|presets| presets.iter().map(|p| self.to_theme_preset(p)).collect())
            .unwrap_or_default()
    }

    fn load_preset(&mut self, preset_id: &str) -> Result<()> {
        self.load_preset_safe(preset_id)
    }

    fn load_preset_by_name(&mut self, category: &str, name: &str) -> Result<()> {
        ProjectMRenderer::load_preset_by_name(self, category, name)
    }

    fn current_preset(&self) -> Option<ThemePreset> {
        let data = self.preset_data.read();
        self.current_preset.as_ref().and_then(|path| {
            data.presets
                .iter()
                .find(|p| &p.path == path)
                .map(|p| self.to_theme_preset(p))
        })
    }

    fn set_random_preset(&mut self) -> Option<ThemePreset> {
        ProjectMRenderer::set_random_preset(self).map(|(category, name)| {
            ThemePreset {
                id: self.current_preset.clone().unwrap_or_default(),
                name,
                category,
                subcategory: None,
                renderer_type: Some(RendererType::ProjectM),
            }
        })
    }

    fn has_presets(&self) -> bool {
        true
    }

    fn set_transitions_enabled(&mut self, enabled: bool) {
        ProjectMRenderer::set_transitions_enabled(self, enabled);
    }

    fn has_transitions(&self) -> bool {
        ProjectMRenderer::has_transitions(self)
    }
}
