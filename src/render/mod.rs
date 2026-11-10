//! OpenGL render pipeline for projectM + video compositing

mod pipeline;
mod compositor;
mod projectm_renderer;
mod led_wall;
mod theme;
mod registry;
mod harmonic_geometry;
mod mythos;
mod vstage_capture;
mod bloom;

#[cfg(target_os = "macos")]
mod led_wall_metal;

pub use pipeline::{RenderPipeline, RenderMode};
pub use compositor::Compositor;
pub use projectm_renderer::ProjectMRenderer;
pub use led_wall::{LedWallRenderer, LedPattern};
pub use theme::{ThemeRenderer, ThemePreset, AudioAnalysis, RendererType, BoxedThemeRenderer};
pub use registry::{PresetRegistry, RegistryPreset, SharedPresetRegistry, create_shared_registry};
pub use harmonic_geometry::{HarmonicGeometryRenderer, HarmonicFieldState, HarmonicLabel, HeroConfig, VisualConfig, FieldSkin, SkinMode, HeroAct, InteriorState, KeyDetector, TuningReference};
pub use mythos::{MythosRenderer, CollatzOutput, MythosAct};
pub use vstage_capture::VstageCapture;
pub use bloom::BloomPass;
pub use self::HarmonicStateBuffer as StateBuffer;

#[cfg(target_os = "macos")]
pub use led_wall_metal::MetalLedWall;

// ============ Lighting Metrics ============

/// Lighting metrics for all lighting outputs (OSC, Hue, WLED, DMX-ArtNet).
/// Core audio-reactive + Collatz signals for intelligent lighting control.
#[derive(Clone, Copy, Debug, Default)]
pub struct LightingMetrics {
    /// Energy (0-1): Overall audio energy, drives zone selection
    pub energy: f32,
    /// Stability (0-1): Temporal chroma stability, gates strobe permission
    pub stability: f32,
    /// Balance (0-1): Chroma distribution (1.0 = uniform, 0.0 = single-note)
    /// Used for palette selection (low balance = cooler colors)
    pub balance: f32,
    /// Tension (0-1): Harmonic tension from chroma deviation
    /// Controls contrast/tightness, NOT strobe trigger
    pub tension: f32,
    /// Beat confidence (0-1): How confident we are in beat detection
    pub beat_confidence: f32,
    /// Whether an onset was detected this frame
    pub onset: bool,
    /// Whether a kick was detected this frame
    pub kick: bool,
    /// Continuous low-band onset strength (the analyzer's kick-envelope flux) —
    /// the onset-detection function the tempo autocorrelator consumes. Distinct
    /// from the held boolean `kick`.
    pub onset_strength: f32,
    /// Dominant pitch class changed this frame
    pub dominant_changed: bool,
    /// Current dominant pitch class (0-11)
    pub dominant_pc: u8,

    // === POLARITY STATE (for quadrant-based effects) ===
    /// Current polarity quadrant (0=E/+1, 1=N/+i, 2=W/-1, 3=S/-i)
    pub polarity_quadrant: u8,
    /// Previous polarity quadrant
    pub polarity_prev_quadrant: u8,
    /// Quadrant changed this frame
    pub polarity_changed: bool,

    // === TONALITY STATE ===
    /// Major scale likelihood (0-1): 1.0 = strongly major, 0.0 = strongly minor,
    /// 0.5 = NEUTRAL / ambiguous (no committed mood). The live KeyDetector seeds
    /// and falls back to 0.5. NB: `#[derive(Default)]` makes this 0.0 (strong
    /// minor), which the laser big-groove mood gate reads as "committed" — a trap
    /// for any default-constructed metrics fed to the show brain; build real ones.
    pub major_likelihood: f32,

    // === FREQUENCY BANDS (for macro state machine) ===
    /// Bass/low band energy (0-1, ~60-250Hz)
    pub bass: f32,
    /// Mid band energy (0-1, ~250-4000Hz)
    pub mid: f32,
    /// High band energy (0-1, ~4000-16000Hz)
    pub high: f32,

    // === RAW ENERGY (for true silence detection) ===
    /// Raw energy sum (raw_bass + raw_mid + raw_high, pre-normalization).
    /// Used by video SilenceGate and CueEngine to detect true audio absence.
    /// Threshold ~0.0003 = silence, ~0.0005 = audio present.
    pub raw_energy: f32,
    /// Raw bass band (pre-normalization). Feeds the spectral-fullness half of
    /// the intensity license — per-band ADAPTIVE normalization lifts quiet
    /// bands (the silence-detection trap), so license math must read raw.
    pub raw_bass: f32,
    /// Raw high band (pre-normalization) — see `raw_bass`.
    pub raw_high: f32,

    // === COLLATZ STATE (Mythos engine, available to all lighting outputs) ===
    /// Accumulated Collatz instability (0-1): rises on odd steps, falls on even.
    /// Good for color temperature, intensity scaling.
    pub collatz_heat: f32,
    /// Collatz phase (0.0 = contraction, 1.0 = expansion).
    /// Good for strobe gating, movement direction.
    pub collatz_phase: f32,
    /// Normalized Collatz magnitude (0-1): how high the current value is in the sequence.
    /// Good for brightness modulation.
    pub collatz_intensity: f32,
    /// Smoothed expansion-contraction breath cycle (0-1).
    /// Good for fixture movement, smooth dimming.
    pub collatz_breath: f32,
    /// Collatz act phase: 0=Dormant, 1=Rising, 2=Peak, 3=Falling.
    pub collatz_act: u8,

    // === HARMONIC FIELD STATE (emerge field → autonomous lighting, Resonator 63960211) ===
    /// Harmonic field rotation phase (0-1, wraps) — the field-derived master
    /// motion phase. Populated from `self.rotation` in `lighting_metrics()`.
    pub phase: f32,
    /// Quaternion port angles, NORMALIZED 0-1 (port_radians / TAU). The 4 lattice
    /// anchors movers aim at; ports 0 & 2 = θ and θ+π (mirror pair). NB: a bare
    /// `LightingMetrics::default()` leaves these `[0.0; 4]` (derived Default), NOT
    /// `[0.5; 4]` — the neutral-center `[0.5; 4]` lives only on the stamp-reaching
    /// paths (CueSnapshot Default + the osc/mod.rs no-engine fallback ctor).
    pub qc_ports: [f32; 4],
    /// Emerge journey phase (HeroAct Birth/Lock/Dive/Afterglow) — the field's
    /// temporal act. Governs the B1 lighting macro (intensity + movement depth),
    /// not just the visuals (patent-canonical: field drives all modalities).
    /// Derived `Default` yields `HeroAct::Birth`.
    pub journey_phase: HeroAct,
    /// Seconds spent in the current journey phase — shapes the macro over time
    /// (Birth emerges, Dive accent cools down, Afterglow fades).
    pub journey_phase_time: f32,
    /// Harmonic coherence (0-1) = smoothstep(tonal_confidence)·smoothstep(stability)
    /// (harmonic_field.rs). Gates the tonal-vs-noise axis (energy gates silence):
    /// high = confident/saturated/full-movement, low (atonal/noisy) = desaturated/
    /// relaxed. Energy still owns brightness so a noisy drop rips while the field
    /// layer relaxes. NB: derived `Default` = 0.0; the no-op neutral `1.0` lives
    /// only on the stamp-reaching paths (CueSnapshot Default + osc/mod.rs fallback).
    pub trust: f32,
    /// Quaternion overlay strength (0-1, from fold24_t) — how strongly the lattice
    /// is "locked in" this frame. B3 uses it to punch the mover harmonic-snap
    /// accent on lattice events. 0 = no lattice = Part A baseline accent (no-op).
    pub qc_strength: f32,
    /// Active quaternion port index (0-3, hysteresis-gated) — the live lattice
    /// anchor. B3 rotates the mover pair's base port to track it (0 = Part A 0/2).
    pub qc_active_port: u8,
    /// Active symmetry order N (the harmonic quantization grid; 12 or 24 in
    /// practice — a fold24 toggle). B4 maps it to mover phase-grouping width (how
    /// many movers move as one phrase unit) across a larger rig. Derived Default 0;
    /// the base 12 lives on the stamp-reaching paths. Reduces to Part A on 2 movers.
    pub symmetry: u8,
    /// 12-bin chroma (pitch-class energy, slew-limited). B5 embeds it on the tonal
    /// circles (fifths/thirds) for a smooth harmonic-motion hue + a voice-leading
    /// "surprise" delta. Derived Default [0.0;12] = atonal/no-op (collapses the
    /// centroid to ~0 strength); the live engine fills it from chroma_smooth().
    pub chroma: [f32; 12],
}

// ============ Harmonic State History Buffer ============

/// Ring buffer for harmonic state history (freeze + scrub feature)
/// Stores recent frames for inspection and timeline export
pub struct HarmonicStateBuffer {
    /// Ring buffer of states
    buffer: Vec<HarmonicFieldState>,
    /// Ring buffer of video frames (parallel to buffer)
    video_frames: Vec<Option<Vec<u8>>>,
    /// Video frame dimensions (same for all frames)
    video_width: u32,
    video_height: u32,
    /// Ring buffer of audio samples (interleaved stereo f32)
    /// Stores ~10 seconds at 48kHz = ~960k samples = ~3.8MB
    audio_samples: Vec<f32>,
    /// Audio sample rate (typically 48000)
    audio_sample_rate: u32,
    /// Audio write position in samples (not bytes)
    audio_write_pos: usize,
    /// Audio samples per video frame (for position calculation)
    audio_samples_per_frame: usize,
    /// Current write position (next frame goes here)
    write_pos: usize,
    /// Number of frames stored (may be less than capacity during startup)
    count: usize,
    /// Capacity (max frames to store)
    capacity: usize,
    /// Whether frozen (paused)
    frozen: bool,
    /// Scrub position when frozen (0.0 = oldest, 1.0 = newest)
    scrub_pos: f32,
    /// Frame index when frozen (for stable scrubbing)
    frozen_at_count: usize,
    /// Last scrub position (for calculating playback direction/speed)
    last_scrub_pos: f32,
}

impl HarmonicStateBuffer {
    /// Create new buffer with capacity for N frames
    /// At 60fps: 300 = 5 sec, 600 = 10 sec
    pub fn new(capacity: usize) -> Self {
        // Audio: 48kHz stereo, ~800 samples per frame at 60fps
        // Total audio capacity = capacity * 800 * 2 channels
        let audio_capacity = capacity * 1600; // stereo samples per frame
        Self {
            buffer: Vec::with_capacity(capacity),
            video_frames: Vec::with_capacity(capacity),
            video_width: 0,
            video_height: 0,
            audio_samples: Vec::with_capacity(audio_capacity),
            audio_sample_rate: 48000,
            audio_write_pos: 0,
            audio_samples_per_frame: 1600, // 800 stereo pairs at 48kHz/60fps
            write_pos: 0,
            count: 0,
            capacity,
            frozen: false,
            scrub_pos: 1.0, // Start at newest
            frozen_at_count: 0,
            last_scrub_pos: 1.0,
        }
    }

    /// Push a new frame (only when not frozen)
    pub fn push(&mut self, state: HarmonicFieldState) {
        if self.frozen {
            return;
        }

        if self.buffer.len() < self.capacity {
            self.buffer.push(state);
            self.video_frames.push(None); // Placeholder for video
        } else {
            self.buffer[self.write_pos] = state;
            // Keep existing video frame slot (will be updated by push_video_frame)
        }
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.count = self.count.saturating_add(1);
    }

    /// Push video frame data for the current position
    /// Call this after push() with the same frame's video data
    pub fn push_video_frame(&mut self, rgba_data: &[u8], width: u32, height: u32) {
        if self.frozen || self.buffer.is_empty() {
            return;
        }

        // Update dimensions if changed
        if width != self.video_width || height != self.video_height {
            self.video_width = width;
            self.video_height = height;
        }

        // Get the position we just wrote to (write_pos was already incremented)
        let pos = if self.write_pos == 0 {
            self.buffer.len().saturating_sub(1)
        } else {
            self.write_pos - 1
        };

        // Store a copy of the video frame
        if pos < self.video_frames.len() {
            self.video_frames[pos] = Some(rgba_data.to_vec());
        }
    }

    /// Get current state (live or scrubbed)
    pub fn current(&self) -> Option<&HarmonicFieldState> {
        if self.buffer.is_empty() {
            return None;
        }

        if self.frozen {
            // Calculate index from scrub position
            let available = self.buffer.len();
            let idx = ((self.scrub_pos * (available - 1) as f32) as usize).min(available - 1);
            // Convert to ring buffer index (oldest to newest)
            let oldest_pos = if self.buffer.len() < self.capacity {
                0
            } else {
                self.write_pos
            };
            let actual_idx = (oldest_pos + idx) % self.buffer.len();
            self.buffer.get(actual_idx)
        } else {
            // Return newest (last written)
            let newest_pos = if self.write_pos == 0 {
                self.buffer.len().saturating_sub(1)
            } else {
                self.write_pos - 1
            };
            self.buffer.get(newest_pos)
        }
    }

    /// Get current video frame (live or scrubbed)
    /// Returns (rgba_data, width, height) if available
    pub fn current_video_frame(&self) -> Option<(&[u8], u32, u32)> {
        if self.video_frames.is_empty() || self.video_width == 0 {
            return None;
        }

        let idx = if self.frozen {
            // Calculate index from scrub position
            let available = self.video_frames.len();
            let idx = ((self.scrub_pos * (available - 1) as f32) as usize).min(available - 1);
            let oldest_pos = if self.video_frames.len() < self.capacity {
                0
            } else {
                self.write_pos
            };
            (oldest_pos + idx) % self.video_frames.len()
        } else {
            // Return newest
            if self.write_pos == 0 {
                self.video_frames.len().saturating_sub(1)
            } else {
                self.write_pos - 1
            }
        };

        self.video_frames.get(idx)
            .and_then(|opt| opt.as_ref())
            .map(|data| (data.as_slice(), self.video_width, self.video_height))
    }

    /// Push audio samples to the buffer (interleaved stereo f32)
    /// Call this from the audio callback
    pub fn push_audio(&mut self, samples: &[f32]) {
        if self.frozen || samples.is_empty() {
            return;
        }

        let audio_capacity = self.capacity * self.audio_samples_per_frame;

        // Grow buffer if needed
        for &sample in samples {
            if self.audio_samples.len() < audio_capacity {
                self.audio_samples.push(sample);
            } else {
                // Ring buffer wrap
                self.audio_samples[self.audio_write_pos] = sample;
            }
            self.audio_write_pos = (self.audio_write_pos + 1) % audio_capacity;
        }
    }

    /// Get audio samples for current scrub position
    /// Returns a slice of samples to play (for one frame worth of audio)
    /// The slice length varies based on scrub speed/direction
    pub fn get_scrub_audio(&mut self, output_buffer: &mut [f32]) {
        if self.audio_samples.is_empty() || !self.frozen {
            // Fill with silence
            output_buffer.fill(0.0);
            return;
        }

        let audio_capacity = self.audio_samples.len();
        let total_frames = self.buffer.len().max(1);

        // Calculate audio position from scrub position
        let scrub_delta = self.scrub_pos - self.last_scrub_pos;
        self.last_scrub_pos = self.scrub_pos;

        // If not moving, output silence (or could loop small section)
        if scrub_delta.abs() < 0.0001 {
            output_buffer.fill(0.0);
            return;
        }

        // Calculate start position in audio buffer
        // scrub_pos 0.0 = oldest, 1.0 = newest
        let oldest_audio_pos = if self.audio_samples.len() < self.capacity * self.audio_samples_per_frame {
            0
        } else {
            self.audio_write_pos
        };

        let frame_pos = (self.scrub_pos * (total_frames - 1) as f32) as usize;
        let audio_offset = frame_pos * self.audio_samples_per_frame;
        let start_pos = (oldest_audio_pos + audio_offset) % audio_capacity;

        // Calculate playback speed based on scrub movement
        let speed = (scrub_delta * total_frames as f32).abs();
        let direction = if scrub_delta >= 0.0 { 1.0 } else { -1.0 };

        // Fill output buffer with samples at the scrub position
        // Apply speed scaling for natural scrub sound
        let output_len = output_buffer.len().max(1);
        let samples_to_read = (output_len as f32 * speed.max(0.1)).min(output_len as f32) as usize;

        for (i, out_sample) in output_buffer.iter_mut().enumerate() {
            let src_idx = if direction >= 0.0 {
                (start_pos + (i * samples_to_read / output_len)) % audio_capacity
            } else {
                // Reverse playback
                (start_pos + audio_capacity - (i * samples_to_read / output_len)) % audio_capacity
            };

            *out_sample = self.audio_samples.get(src_idx).copied().unwrap_or(0.0) * 0.7; // Slight volume reduction for scrub
        }
    }

    /// Toggle freeze state
    pub fn toggle_freeze(&mut self) {
        self.frozen = !self.frozen;
        if self.frozen {
            self.frozen_at_count = self.count;
            self.scrub_pos = 1.0; // Start at newest when freezing
        }
    }

    /// Set freeze state
    pub fn set_frozen(&mut self, frozen: bool) {
        if frozen && !self.frozen {
            self.frozen_at_count = self.count;
            self.scrub_pos = 1.0;
        }
        self.frozen = frozen;
    }

    /// Check if frozen
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Set scrub position (0.0 = oldest, 1.0 = newest)
    pub fn set_scrub(&mut self, pos: f32) {
        self.scrub_pos = pos.clamp(0.0, 1.0);
    }

    /// Get scrub position
    pub fn scrub_pos(&self) -> f32 {
        self.scrub_pos
    }

    /// Get number of frames available
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get frame at specific index (0 = oldest available)
    pub fn get(&self, idx: usize) -> Option<&HarmonicFieldState> {
        if idx >= self.buffer.len() {
            return None;
        }
        let oldest_pos = if self.buffer.len() < self.capacity {
            0
        } else {
            self.write_pos
        };
        let actual_idx = (oldest_pos + idx) % self.buffer.len();
        self.buffer.get(actual_idx)
    }

    /// Get all frames as iterator (oldest to newest)
    pub fn iter(&self) -> impl Iterator<Item = &HarmonicFieldState> {
        let len = self.buffer.len();
        let oldest_pos = if len < self.capacity { 0 } else { self.write_pos };
        (0..len).map(move |i| {
            let idx = (oldest_pos + i) % len;
            &self.buffer[idx]
        })
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.write_pos = 0;
        self.count = 0;
        self.frozen = false;
        self.scrub_pos = 1.0;
    }
}

/// Overlay position presets
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Default for OverlayPosition {
    fn default() -> Self {
        Self::BottomRight
    }
}

/// Track overlay data for rendering
#[derive(Clone, Debug, Default)]
pub struct TrackOverlay {
    /// Track title
    pub title: String,
    /// Artist name
    pub artist: String,
    /// Opacity (0.0 - 1.0)
    pub opacity: f32,
    /// Animation progress (0.0 = center/large, 1.0 = corner/small)
    pub animation_progress: f32,
    /// Whether to show the overlay
    pub visible: bool,
}

/// Preset browser overlay data for center-screen display
#[derive(Clone, Debug, Default)]
pub struct PresetBrowserOverlay {
    /// Display name (custom name from DB, or file name if no custom name)
    pub preset_name: String,
    /// Original file name (shown smaller below the display name)
    pub file_name: String,
    /// Category of the current preset
    pub category: String,
    /// Current index in the list (1-based for display)
    pub current_index: usize,
    /// Total number of items in the list
    pub total_count: usize,
    /// Opacity (0.0 - 1.0)
    pub opacity: f32,
    /// Whether to show the overlay
    pub visible: bool,
}

/// Render parameters passed from UI controls
/// PERFORMANCE: Copy not possible due to String fields - use Clone sparingly
#[derive(Clone, Debug)]
pub struct RenderParams {
    /// projectM visibility (0.0 - 1.0)
    pub projectm_visibility: f32,
    /// projectM scale (0.1 - 2.0)
    pub projectm_scale: f32,

    /// Video visibility (0.0 - 1.0)
    pub video_visibility: f32,
    /// Video scale (0.05 - 4.0)
    pub video_scale: f32,
    /// Video X position (-1.0 to 1.0)
    pub video_x: f32,
    /// Video Y position (-1.0 to 1.0)
    pub video_y: f32,
    /// Video luma key (0.0 = off, 1.0 = full - dark pixels become transparent)
    pub video_luma_key: f32,
    /// Camera layer luma key (0.0 = off, 1.0 = full - dark camera areas become transparent)
    pub camera_luma_key: f32,

    /// Theme silence flag — true when the app's SilenceGate has confirmed
    /// silence. Drives projectM's fade-to-black so the theme darkens together
    /// with the compels and lights when the audio stops.
    pub theme_silent: bool,

    /// Bloom/glow intensity (0.0 = off, 1.0 = full)
    pub bloom_intensity: f32,
    /// Vignette intensity (0.0 = off, 1.0 = strong)
    pub vignette_intensity: f32,

    // === Overlay settings ===
    /// Logo overlay enabled
    pub logo_visible: bool,
    /// Logo opacity (0.0 - 1.0)
    pub logo_opacity: f32,
    /// Logo scale (0.1 - 2.0)
    pub logo_scale: f32,
    /// Logo position preset
    pub logo_position: OverlayPosition,
    /// Custom logo path (None = use default REACT logo)
    pub custom_logo_path: Option<String>,

    /// Track announcement overlay
    pub track_overlay: TrackOverlay,

    /// Preset browser overlay (center screen)
    pub preset_browser_overlay: PresetBrowserOverlay,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            projectm_visibility: 1.0,
            projectm_scale: 1.0,
            video_visibility: 0.30,
            video_scale: 1.0,
            video_x: 0.0,
            video_y: 0.0,
            video_luma_key: 0.0,     // Off by default (no transparency)
            camera_luma_key: 0.0,    // Off by default (no transparency)
            theme_silent: false,     // Audio assumed present until the gate says otherwise
            bloom_intensity: 0.3,    // Subtle bloom by default
            vignette_intensity: 0.2, // Subtle vignette by default
            // Overlay defaults
            logo_visible: true,
            logo_opacity: 0.20,
            logo_scale: 2.0,
            logo_position: OverlayPosition::BottomLeft,
            custom_logo_path: None,
            track_overlay: TrackOverlay::default(),
            preset_browser_overlay: PresetBrowserOverlay::default(),
        }
    }
}
