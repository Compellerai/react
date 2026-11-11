//! Control panel UI for adjusting visualization parameters

use eframe::egui;
use std::sync::Arc;
use parking_lot::Mutex;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::app::{AudioAction, OutputAction, MidiAction, OscAction, HueAction, WledAction, OscState, CameraAction, CameraState};
use crate::auth::{AuthManager, AuthState, LoginCredentials};
use crate::config;
use crate::osc::OscSender;
use crate::emerge::{LfoBankConfig, LfoTarget, LfoWaveform, TriggerButton};
use crate::audio::{AudioCapture, AudioSource, AudioSourceInfo, AudioAnalysis, MeterValues, get_available_sources};
use crate::midi::{ControlTarget, LearnState, MidiKey, MidiMapping};
use crate::track::CdjDeviceInfo;
use crate::render::{RenderParams, RenderPipeline, RenderMode, OverlayPosition, HarmonicFieldState, HarmonicLabel, HeroAct, InteriorState, TuningReference};
use crate::video::VideoDecoder;
#[cfg(target_os = "macos")]
use crate::video::ThumbnailGenerator;
use crate::db::{Database, RecordingInfo, PresetMetadata, VideoMetadata, TrackInfo, SyncVideoStatus, SyncVideoInfo, CompelVideoInfo};
use crate::sync::{VideoSync, format_bytes, format_speed};
use crate::compel_sync::CompelSync;
use crate::output::OutputMode;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

/// Action returned by the traffic light buttons
#[derive(Clone, Copy, PartialEq)]
enum TrafficLightAction {
    None,
    Close,
    Minimize,
    Fullscreen,
}

/// One-line summary for the SaaS stage sync status — feeds the small line
/// under the Stage: dropdown so the operator knows persistence is happening
/// (or queued, or offline) without any manual buttons.
fn sync_status_summary(
    stage_ui_state: Option<&crate::app::StageUiState>,
    is_authenticated: bool,
) -> (String, egui::Color32) {
    use crate::stage_sync_worker::SyncStatusKind;
    if !is_authenticated {
        return (
            "log in to Compeller to load + sync a stage".to_string(),
            egui::Color32::GRAY,
        );
    }
    let kind = stage_ui_state
        .map(|s| s.sync_status.kind.clone())
        .unwrap_or(SyncStatusKind::NoStage);
    match kind {
        SyncStatusKind::NoStage => (
            "no stage selected".to_string(),
            egui::Color32::GRAY,
        ),
        SyncStatusKind::Idle => (
            "synced to Compeller".to_string(),
            egui::Color32::from_rgb(120, 200, 120),
        ),
        SyncStatusKind::Syncing => (
            "syncing\u{2026}".to_string(),
            egui::Color32::from_rgb(180, 200, 120),
        ),
        SyncStatusKind::Pending(n) => (
            format!("queued ({}) \u{00B7} flushing\u{2026}", n),
            egui::Color32::from_rgb(180, 200, 120),
        ),
        SyncStatusKind::Offline { queued, error } => (
            format!(
                "offline \u{00B7} {} queued ({})",
                queued,
                error.lines().next().unwrap_or("retrying")
            ),
            egui::Color32::from_rgb(220, 130, 100),
        ),
    }
}

/// Renders macOS-style traffic light buttons (close/minimize/fullscreen).
fn mac_traffic_lights(ui: &mut egui::Ui) -> TrafficLightAction {
    let radius = 5.5_f32;
    let dot_spacing = 20.0_f32; // center-to-center
    let total_width = dot_spacing * 2.0 + radius * 2.0;
    let height = radius * 2.0 + 2.0;

    let (full_rect, response) = ui.allocate_exact_size(
        egui::vec2(total_width, height),
        egui::Sense::click(),
    );

    if !ui.is_rect_visible(full_rect) {
        return TrafficLightAction::None;
    }

    let hovered = response.hovered();
    let y = full_rect.center().y;

    // Button centers: red, yellow, green
    let red_center = egui::pos2(full_rect.min.x + radius, y);
    let yellow_center = egui::pos2(red_center.x + dot_spacing, y);
    let green_center = egui::pos2(yellow_center.x + dot_spacing, y);

    // When not hovered, show muted dots; when hovered, show vibrant colors
    let (red_col, yellow_col, green_col) = if hovered {
        (
            egui::Color32::from_rgb(255, 95, 87),
            egui::Color32::from_rgb(255, 189, 46),
            egui::Color32::from_rgb(40, 201, 64),
        )
    } else {
        (
            egui::Color32::from_rgb(130, 50, 45),
            egui::Color32::from_rgb(130, 100, 30),
            egui::Color32::from_rgb(30, 105, 40),
        )
    };

    let painter = ui.painter();

    // Draw circles
    painter.circle_filled(red_center, radius, red_col);
    painter.circle_filled(yellow_center, radius, yellow_col);
    painter.circle_filled(green_center, radius, green_col);

    // Draw symbols on hover
    if hovered {
        let dark_stroke = |r: u8, g: u8, b: u8| egui::Stroke::new(1.5, egui::Color32::from_rgb(r, g, b));

        // Red X
        let s = radius * 0.4;
        painter.line_segment(
            [red_center + egui::vec2(-s, -s), red_center + egui::vec2(s, s)],
            dark_stroke(80, 0, 0),
        );
        painter.line_segment(
            [red_center + egui::vec2(s, -s), red_center + egui::vec2(-s, s)],
            dark_stroke(80, 0, 0),
        );

        // Yellow minus
        let s = radius * 0.45;
        painter.line_segment(
            [yellow_center + egui::vec2(-s, 0.0), yellow_center + egui::vec2(s, 0.0)],
            dark_stroke(100, 60, 0),
        );

        // Green expand arrows
        let s = radius * 0.35;
        painter.line_segment(
            [green_center + egui::vec2(-s, s), green_center + egui::vec2(s, -s)],
            dark_stroke(0, 80, 0),
        );
    }

    // Determine which button was clicked
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if (pos - red_center).length() <= radius + 2.0 {
                return TrafficLightAction::Close;
            }
            if (pos - yellow_center).length() <= radius + 2.0 {
                return TrafficLightAction::Minimize;
            }
            if (pos - green_center).length() <= radius + 2.0 {
                return TrafficLightAction::Fullscreen;
            }
        }
    }

    TrafficLightAction::None
}

/// Output resolution options (for NDI and Screen output)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NdiResolution {
    Hd,         // 1920x1080 (16:9)
    Uhd,        // 3840x2160 (4K, 16:9)
    Ultrawide,  // 5120x1440 (32:9 ultrawide)
}

impl NdiResolution {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            NdiResolution::Hd => (1920, 1080),
            NdiResolution::Uhd => (3840, 2160),
            NdiResolution::Ultrawide => (5120, 1440),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NdiResolution::Hd => "HD (1080p)",
            NdiResolution::Uhd => "4K (2160p)",
            NdiResolution::Ultrawide => "Ultrawide (5120×1440)",
        }
    }
}

/// NDI tally info for UI
#[derive(Clone, Copy, Debug, Default)]
pub struct NdiTallyInfo {
    pub on_program: bool,
    pub on_preview: bool,
}

/// Video view mode for Compels list
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum VideoViewMode {
    /// List view with small inline thumbnails
    #[default]
    List,
    /// Grid view with larger thumbnails
    Grid,
}

/// Output state passed to control panel
#[derive(Clone)]
pub struct OutputState {
    pub ndi_enabled: bool,
    pub ndi_available: bool,
    pub ndi_resolution: NdiResolution,
    pub ndi_audio_enabled: bool,
    pub ndi_connections: u32,
    pub ndi_tally: NdiTallyInfo,
    pub ndi_audio_gain: f32,        // user gain (slider value)
    pub ndi_assist_gain: f32,       // Level Assist computed gain
    pub ndi_effective_gain: f32,    // calibration x assist x user
    pub ndi_assist_hold: bool,      // true when assist is in hold/cooldown
    pub ndi_audio_present: bool,    // true when audio signal is present
    pub output_window_enabled: bool,
    pub output_window_fullscreen: bool,
    pub recording_enabled: bool,
    pub recording_available: bool,
    pub recording_duration_secs: f32,
    pub recording_processing: bool, // True while stitching segments after recording stops
    pub rec_audio_gain: f32,
    pub rec_assist_gain: f32,
    pub rec_effective_gain: f32,
    pub rec_assist_hold: bool,
    pub rec_audio_present: bool,
    // Segmented recording fields
    pub segmented_recording: bool,
    pub current_segment: u32,
    pub committed_segments: u32,
    pub last_commit_elapsed_ms: u64,
}

/// Per-camera tracking influence state
/// Controls how much tracking data affects visualization accents/masks (NOT video compositing)
#[derive(Clone, Debug)]
pub struct CameraUiState {
    pub camera_index: u32,
    pub camera_name: String,

    // === Enable/Selection ===
    pub enabled: bool,
    pub last_toggle_time: std::time::Instant,

    // === Visibility (Tracking Influence) ===
    /// Quantized visibility step (0..16)
    pub vis_q: u8,
    /// Post-smoothing influence [0..1]
    pub vis_smoothed: f32,
    /// Last committed step (for persistence)
    pub vis_committed: u8,
    /// Rate-limit timer: 1 step per vis_hold_ms
    pub vis_hold_timer: f32,

    // === Tracking Confidence ===
    /// LP-filtered confidence [0..1], τ=200ms
    pub tracking_confidence: f32,
    /// For stall detection
    pub last_tracker_update: std::time::Instant,

    // === MIDI Pickup ===
    pub midi_last_cc: u8,
    pub midi_pickup_active: bool,

    // === Timestamps ===
    pub last_user_change_ts: f64,
    pub last_midi_change_ts: f64,
    pub last_feature_update_ts: f64,
}

impl Default for CameraUiState {
    fn default() -> Self {
        Self {
            camera_index: 0,
            camera_name: String::new(),
            enabled: true,
            last_toggle_time: std::time::Instant::now(),
            vis_q: 16,  // Default to full influence
            vis_smoothed: 1.0,
            vis_committed: 16,
            vis_hold_timer: 0.0,
            tracking_confidence: 0.0,
            last_tracker_update: std::time::Instant::now(),
            midi_last_cc: 0,
            midi_pickup_active: false,
            last_user_change_ts: 0.0,
            last_midi_change_ts: 0.0,
            last_feature_update_ts: 0.0,
        }
    }
}

impl CameraUiState {
    /// Create a new CameraUiState for a specific camera
    pub fn new(camera_index: u32, camera_name: String) -> Self {
        Self {
            camera_index,
            camera_name,
            ..Default::default()
        }
    }

    /// Compute influence target: (vis_q / 16) * tracking_confidence
    pub fn influence_target(&self) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        (self.vis_q as f32 / 16.0) * self.tracking_confidence
    }

    /// Update per-frame: confidence LP, smoothing, stall decay
    pub fn update(&mut self, dt: f32, tracker_confidence: Option<f32>) {
        const TAU_CONFIDENCE: f32 = 0.200;  // 200ms LP for confidence
        const TAU_SMOOTH: f32 = 0.120;      // 120ms smoothing
        const STALL_THRESHOLD: f32 = 0.500; // 500ms before decay
        const TAU_DECAY: f32 = 0.300;       // 300ms decay on stall

        // 1. Update tracking confidence
        if let Some(conf) = tracker_confidence {
            let alpha = 1.0 - (-dt / TAU_CONFIDENCE).exp();
            self.tracking_confidence += alpha * (conf - self.tracking_confidence);
            self.last_tracker_update = std::time::Instant::now();
        } else {
            // Decay on stall
            let stall_time = self.last_tracker_update.elapsed().as_secs_f32();
            if stall_time > STALL_THRESHOLD {
                let decay_alpha = 1.0 - (-dt / TAU_DECAY).exp();
                self.tracking_confidence *= 1.0 - decay_alpha;
            }
        }
        self.tracking_confidence = self.tracking_confidence.clamp(0.0, 1.0);

        // 2. Smooth toward influence target
        let target = self.influence_target();
        let alpha = 1.0 - (-dt / TAU_SMOOTH).exp();
        self.vis_smoothed += alpha * (target - self.vis_smoothed);

        // 3. Update hold timer
        self.vis_hold_timer = (self.vis_hold_timer - dt).max(0.0);
    }

    /// Process visibility change - MIDI bypasses rate-limiting, UI rate-limits to 1 step per 120ms
    pub fn set_visibility(&mut self, new_step: u8, is_midi: bool) -> bool {
        const VIS_HOLD_MS: f32 = 0.120;

        let clamped = new_step.min(16);
        let delta = (clamped as i8 - self.vis_q as i8).abs();

        if delta == 0 {
            return false;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        if is_midi {
            // MIDI: set directly, no rate-limiting
            self.vis_q = clamped;
            self.last_midi_change_ts = now;
            return true;
        }

        // UI: rate-limit to max 1 step per hold period
        if self.vis_hold_timer <= 0.0 {
            let step_dir = if clamped > self.vis_q { 1i8 } else { -1i8 };
            self.vis_q = (self.vis_q as i8 + step_dir).clamp(0, 16) as u8;
            self.vis_hold_timer = VIS_HOLD_MS;
            self.last_user_change_ts = now;
            return true;
        }
        false
    }

    /// Process MIDI CC with hysteresis (±2) and pickup mode
    pub fn process_midi_cc(&mut self, cc_value: u8) -> bool {
        const HYSTERESIS: u8 = 2;
        const PICKUP_WINDOW: u8 = 5;

        // Hysteresis check
        let delta = (cc_value as i16 - self.midi_last_cc as i16).abs() as u8;
        if delta < HYSTERESIS {
            return false;
        }
        self.midi_last_cc = cc_value;

        // Pickup mode
        if !self.midi_pickup_active {
            let current_cc = (self.vis_q as f32 / 16.0 * 127.0) as u8;
            if (cc_value as i16 - current_cc as i16).abs() as u8 <= PICKUP_WINDOW {
                self.midi_pickup_active = true;
            } else {
                return false;
            }
        }

        // Convert CC to step
        let new_step = (cc_value as f32 / 127.0 * 16.0).round() as u8;
        self.set_visibility(new_step, true)
    }

    /// Process enable toggle with cooldown (90ms)
    pub fn toggle_enable(&mut self) -> bool {
        const COOLDOWN_MS: u64 = 90;
        if self.last_toggle_time.elapsed().as_millis() as u64 >= COOLDOWN_MS {
            self.enabled = !self.enabled;
            self.last_toggle_time = std::time::Instant::now();
            self.midi_pickup_active = false;  // Reset pickup
            return true;
        }
        false
    }
}

/// Lighting-clock status, pushed from the app each frame for the clock UI.
#[derive(Clone, Copy, Debug, Default)]
struct ClockStatus {
    /// Ableton Link session tempo — `Some` only while a Link peer is live.
    link_bpm: Option<f32>,
    /// REACT's audio-derived tempo estimate (BPM).
    derived_bpm: f32,
    /// 0‑1 confidence in the derived estimate.
    derived_confidence: f32,
}

/// Control panel state
pub struct ControlPanel {
    /// Current render parameters
    params: RenderParams,

    /// COM-499 Phase B4: operator LFO bank — the config-only editable copy (the
    /// running phase lives on the OSC thread). Persisted as a JSON blob; pushed
    /// to the runtime on startup + on every edit.
    lfo_bank: LfoBankConfig,

    /// Selected preset category
    selected_category: Option<String>,

    /// Available categories (cached)
    categories: Vec<String>,

    /// Presets in selected category (cached)
    presets: Vec<String>,

    /// Video decoder for playback
    video_decoder: VideoDecoder,

    /// Video file path
    video_path: Option<String>,

    /// Video directory path
    video_directory: Option<String>,

    /// List of video files in directory
    video_files: Vec<String>,

    /// Currently browsed video path (for encoder navigation)
    /// None = no browse cursor, Some(path) = browsing at that video
    browsed_video_path: Option<String>,

    /// Recording output directory path
    recording_directory: Option<String>,

    /// List of recordings with metadata
    recordings: Vec<RecordingInfo>,

    /// Cached upload status map (file_path -> status)
    upload_status_cache: HashMap<String, crate::db::SetUploadStatus>,

    /// Recording being edited (file_path)
    editing_recording: Option<String>,

    /// Text buffer for editing recording name
    editing_recording_name: String,

    /// Pending recording deletion (file_path) - deferred to avoid borrow issues
    pending_delete_recording: Option<String>,

    /// Cached preset metadata (file_path -> metadata)
    preset_metadata: HashMap<String, PresetMetadata>,

    /// Preset being edited (file_path)
    editing_preset: Option<String>,

    /// Text buffer for editing preset name
    editing_preset_name: String,

    /// Whether to show hidden presets
    show_hidden_presets: bool,

    /// Preset for track management (file_path)
    managing_preset_tracks: Option<String>,

    /// Cached tracks for preset being managed
    preset_tracks: Vec<TrackInfo>,

    /// New track name input
    new_track_name: String,

    /// New track artist input
    new_track_artist: String,

    /// Cached video metadata (file_path -> metadata)
    video_metadata: HashMap<String, VideoMetadata>,

    /// Video being edited (file_path)
    editing_video: Option<String>,

    /// Text buffer for editing video name
    editing_video_name: String,

    /// Whether to show hidden videos
    show_hidden_videos: bool,

    /// Video search filter
    video_search: String,

    /// Video favorites (file paths)
    video_favorites: std::collections::HashSet<String>,

    /// Recent videos (file paths, most recent first)
    recent_videos: Vec<String>,

    /// Whether to show all videos (collapsed by default)
    show_all_videos: bool,

    /// Video view mode (list or grid)
    video_view_mode: VideoViewMode,

    /// Thumbnail generator for video previews
    #[cfg(target_os = "macos")]
    thumbnail_generator: Option<ThumbnailGenerator>,

    /// Loaded thumbnail textures (video path -> texture handle)
    thumbnail_textures: HashMap<String, egui::TextureHandle>,

    /// Thumbnails currently being loaded (to avoid duplicate loads)
    pending_thumbnail_loads: HashSet<String>,

    /// Counter for thumbnails loaded this frame (limit to prevent UI freeze)
    thumbnails_loaded_this_frame: u32,

    /// Video sync manager (public compels)
    video_sync: Option<VideoSync>,

    /// Compel video sync manager (authenticated user's compels)
    compel_sync: Option<CompelSync>,

    /// Last seen compel sync completed count (for detecting new downloads)
    last_compel_sync_completed: u64,

    /// Show sync window/popup
    show_sync_window: bool,

    /// Show Compel Manager window
    show_compel_manager: bool,

    /// Selected compel in manager (for details panel)
    manager_selected_compel: Option<String>,

    /// Show Recording Manager window
    show_recording_manager: bool,

    /// Selected recording in manager (for details panel)
    manager_selected_recording: Option<String>,

    /// Windows that are minimized (collapsed to title bar)
    minimized_windows: HashSet<String>,

    /// Window currently in fullscreen mode (fills available rect)
    fullscreen_window: Option<String>,

    /// Flag for app to hide the menu when a window is fullscreen
    pub hide_menu_for_fullscreen: bool,

    /// When true, the expanded window uses half-screen mode (right side, menu visible)
    half_screen_mode: bool,

    /// Last seen sync completed count (for detecting new downloads)
    last_sync_completed: u64,

    /// Video for track management (file_path)
    managing_video_tracks: Option<String>,

    /// Cached tracks for video being managed
    video_tracks: Vec<TrackInfo>,

    /// New video track name input
    new_video_track_name: String,

    /// New video track artist input
    new_video_track_artist: String,

    /// Audio gain (input/visualization stage)
    audio_gain: f32,

    /// Per-output audio gains (post-capture, pre-sink)
    ndi_audio_gain: f32,
    rec_audio_gain: f32,
    youtube_audio_gain: f32,
    twitch_audio_gain: f32,
    kick_audio_gain: f32,

    /// Pending gain action (set by YouTube/Twitch/Kick sub-sections, consumed by show())
    pending_gain_action: OutputAction,

    /// Whether to show audio meters
    show_audio_meters: bool,

    /// Latest fundsp meter values (set from app before show())
    meter_values: Option<MeterValues>,

    /// Kick detection sensitivity (separate from meters)
    kick_sensitivity: f32,

    /// Expanded sections
    projectm_expanded: bool,
    video_expanded: bool,
    audio_expanded: bool,
    output_expanded: bool,
    recording_expanded: bool,
    /// Recording view mode (list or grid)
    recording_view_mode: VideoViewMode,
    overlay_expanded: bool,
    prodjlink_expanded: bool,
    midi_expanded: bool,
    osc_expanded: bool,
    /// DMX panel: edited USB serial port path (pre-filled by auto-detect).
    dmx_port_edit: String,
    /// DMX panel: edited Art-Net target socket (`ip:port`).
    dmx_artnet_target_edit: String,
    /// DMX panel: edited OLA olad host.
    dmx_ola_host_edit: String,
    /// DMX panel: edited OLA olad RPC port.
    dmx_ola_port_edit: String,
    /// Lighting-menu toggle for the operator virtual stage preview.
    virtual_stage_enabled: bool,
    /// Camera base preset for the virtual stage view. Single FOH today; the
    /// operator orbits via the yaw/pitch sliders below.
    virtual_stage_view: crate::emerge::StageViewPresetKind,
    /// Focal zoom: 1.0 = preset FOV, <1 wider, >1 tighter. Implemented as a
    /// focal-length change rather than an eye translation so fixtures keep
    /// their stage positions and the rig just scales in or out.
    virtual_stage_zoom: f32,
    /// Camera orbit yaw around look_at (degrees). 0 = FOH-front; +90 stage-
    /// right; ±180 behind the wall; -90 stage-left.
    virtual_stage_yaw: f32,
    /// Camera orbit pitch (degrees above/below preset elevation). 0 = preset;
    /// +60 near-overhead; -60 low ground-level.
    virtual_stage_pitch: f32,
    /// Show the floating fixture-name labels on the virtual stage (declutter toggle).
    virtual_stage_labels: bool,
    /// Which venue/environment the virtual stage renders (Concert Stage vs the
    /// Wedding Room reception template). Operator-local, persisted as a key.
    virtual_stage_venue: crate::emerge::VenueKind,
    /// Saved camera (zoom, yaw, pitch) per venue so each room keeps its own
    /// framing. The ACTIVE venue's camera lives in `virtual_stage_{zoom,yaw,
    /// pitch}`; this holds the others and is swapped on a venue change.
    venue_cameras: std::collections::HashMap<crate::emerge::VenueKind, (f32, f32, f32)>,
    /// Silence-gate hold: seconds of continuous silence before the show fades.
    silence_threshold: f32,

    /// OSC target IP input buffer
    osc_target_ip_input: String,
    /// OSC target port input buffer
    osc_target_port_input: String,
    /// Cached resolved IP for OSC target (to avoid DNS lookups every frame)
    osc_resolved_ip_cache: Option<String>,
    /// Last target IP we resolved (to detect changes)
    osc_last_resolved_target: String,

    /// OSC Mapping section expanded
    osc_mapping_expanded: bool,
    /// OSC Advanced section expanded (within mapping)
    osc_advanced_expanded: bool,
    /// Selected target profile index
    osc_profile_index: usize,
    /// Palette name input buffers
    osc_palette_ice_name: String,
    osc_palette_cool_a_name: String,
    osc_palette_cool_b_name: String,
    /// Palette index input buffers (for DMX software mapping)
    osc_palette_ice_index: String,
    osc_palette_cool_a_index: String,
    osc_palette_cool_b_index: String,
    /// Pattern index input buffers
    osc_pattern_silent: String,
    osc_pattern_still: String,
    osc_pattern_low: String,
    osc_pattern_mid: String,
    osc_pattern_high: String,
    /// Mover speed range
    osc_mover_speed_min: f32,
    osc_mover_speed_max: f32,
    /// FX palette enabled
    osc_enable_fx_palette: bool,

    // === REACTIVE LIGHTING FEATURES ===
    /// Enable polarity flash on quadrant transitions
    osc_enable_polarity_flash: bool,
    /// Enable dominant pitch change accents
    osc_enable_dominant_accent: bool,
    /// Enable tonality-aware palette modulation
    osc_enable_tonality_palettes: bool,
    /// Enable energy slope detection
    osc_enable_energy_slope: bool,

    /// Flag to sync UI from config on first load
    osc_mapping_initialized: bool,

    // === GENRE UI STATE ===
    /// Genre section expanded
    osc_genre_expanded: bool,
    /// Selected genre category index
    osc_genre_category_index: usize,
    /// Selected genre subcategory index
    osc_genre_subcategory_index: usize,
    /// Genre advanced section expanded
    osc_genre_advanced_expanded: bool,

    // === MAESTRODMX UI STATE ===
    /// Selected pattern index in dropdown
    maestro_pattern_index: usize,
    /// Selected color palette index in dropdown
    maestro_color_palette_index: usize,
    /// Selected FX palette index in dropdown
    maestro_fx_palette_index: usize,

    // === HUE UI STATE ===
    /// Hue section expanded
    hue_expanded: bool,
    /// Hue enabled toggle
    hue_enabled: bool,
    /// Bridge IP input buffer
    hue_bridge_ip_input: String,
    /// Selected entertainment area index
    hue_area_index: usize,
    /// Whether Hue config has been initialized from state
    hue_initialized: bool,
    /// Selected Hue protocol version index (0 = V2, 1 = V1)
    hue_protocol_index: usize,
    /// Selected Hue mapping mode index (0 = Reactive, 1 = MultiBand)
    hue_mapping_mode_index: usize,

    // === WLED UI STATE ===
    /// WLED section expanded
    wled_expanded: bool,
    /// WLED enabled toggle
    wled_enabled: bool,
    /// Strip IP input buffer
    wled_strip_ip_input: String,
    /// Inline error for WLED IP validation (cleared on next valid action)
    wled_ip_error: Option<String>,
    /// Whether WLED state has been initialized from state
    wled_initialized: bool,

    /// Selected NDI resolution
    ndi_resolution: NdiResolution,

    /// Available audio sources (cached)
    audio_sources: Vec<AudioSourceInfo>,

    /// Currently selected audio source index
    selected_audio_source: usize,

    /// Show all running apps (not just known audio apps)
    show_all_apps: bool,

    /// Kick detection hold timer (for visual persistence)
    kick_hold_frames: u32,

    /// Click detection hold timer (for visual persistence)
    click_hold_frames: u32,

    /// Master deck pulse phase (0.0-1.0, cycles with BPM)
    master_pulse_phase: f32,

    /// Last known master BPM for tempo-synced pulse
    master_bpm: Option<f32>,

    /// Ableton Link section collapsing-header state.
    ableton_link_expanded: bool,
    /// Operator's lighting-clock source choice (Auto = Link > ProDJ > REACT).
    clock_override: crate::emerge::ClockOverride,
    /// Lighting-clock status, set from the app each frame before show().
    clock_status: ClockStatus,

    /// Current render mode (ProjectM or LED Wall)
    render_mode: RenderMode,

    /// Currently active preset (category, name)
    current_preset: Option<(String, String)>,

    /// Pending preset load (category, name) - deferred to next frame to show loading dialog
    pending_preset_load: Option<(String, String)>,

    /// Frames the loading dialog has been shown (need 2+ frames before blocking load)
    pending_preset_load_frames: u8,

    /// Index in favorites list for encoder browsing (None = not browsing)
    browse_favorites_index: Option<usize>,

    /// Timer for auto-hiding the browse overlay
    browse_favorites_timer: Option<std::time::Instant>,

    /// Browse overlay opacity (for smooth fade in/out)
    browse_favorites_opacity: f32,

    /// Index in compels list for encoder browsing (None = not browsing)
    browse_compels_index: Option<usize>,

    /// Timer for auto-hiding the compel browse overlay
    browse_compels_timer: Option<std::time::Instant>,

    /// Compel browse overlay opacity (for smooth fade in/out)
    browse_compels_opacity: f32,

    /// Database reference for favorites
    db: Option<Database>,

    /// Cached favorites (category, preset_name)
    favorites: Vec<(String, String)>,

    /// Cached pinned presets (category, preset_name) - from database
    pinned_presets: Vec<(String, String)>,

    /// Whether favorites/pinned need refresh
    favorites_dirty: bool,

    /// Combined list for MIDI browsing (pinned + favorites)
    midi_browse_list: Vec<(String, String)>,

    /// Whether MIDI browse list needs rebuild
    midi_browse_dirty: bool,

    /// Flag to scroll preset list to current selection
    scroll_to_preset: bool,

    /// Auto-randomize the theme on a fixed interval (60s)
    auto_randomize_theme: bool,

    /// Next deadline at which the auto-randomizer should fire
    auto_randomize_next: Option<std::time::Instant>,

    /// Last current_preset value observed by poll_auto_randomize_theme — used
    /// to detect any theme change (manual or automatic) so the timer resets.
    auto_randomize_last_seen: Option<(String, String)>,

    /// First instant FPS dropped below FPS_SHUFFLE_THRESHOLD in the current dip.
    /// Cleared whenever FPS recovers. Wall-clock so it works at any FPS.
    fps_shuffle_low_since: Option<std::time::Instant>,

    /// Wall-clock instant the FPS-shuffle cooldown expires.
    fps_shuffle_cooldown_until: Option<std::time::Instant>,

    /// Search query for fuzzy preset filtering
    search_query: String,

    /// Fuzzy matcher for preset search
    fuzzy_matcher: SkimMatcherV2,

    /// Target video scale for smooth lerping
    video_scale_target: f32,

    /// Velocity for spring physics (bounce effect)
    video_scale_velocity: f32,

    /// Flag indicating settings have changed and need saving
    settings_dirty: bool,

    /// Frame counter for debounced saving (save every N frames when dirty)
    save_debounce_frames: u32,

    /// MIDI learn mode state - true when waiting for user to click a control
    midi_learn_armed: bool,

    /// Pending MIDI action from control clicks during learn mode
    pending_midi_action: MidiAction,

    // === Mythos controls ===
    mythos_instability: f32,
    mythos_field_strength: f32,
    mythos_disturbance_size: f32,
    mythos_harmonic_influence: f32,
    mythos_decay: f32,

    // === Harmonic Geometry controls ===
    /// Show chroma key detection in menu
    show_chroma_key: bool,

    /// Key detection lock time in beats
    key_lock_beats: u32,

    /// Key detection confidence threshold
    key_confidence_threshold: f32,

    /// Tuning reference (432Hz or 440Hz)
    tuning_reference: TuningReference,

    /// Harmonic Geometry section expanded
    harmonic_expanded: bool,
    performance_expanded: bool,

    /// Show Math Overlay HUD on visualization
    show_math_overlay: bool,

    /// Show QS Wheel (Quaternion Symmetry Wheel)
    show_qs_wheel: bool,

    /// Hide numbers on QS Wheel
    hide_qs_numbers: bool,

    /// QS Wheel dominant-only mode (true = only dominant note, false = all notes with REG symmetry)
    qs_dominant_only: bool,

    /// Quality tier (0=Low, 1=Med, 2=High, 3=Ultra)
    pub quality_tier: u8,

    /// Show performance HUD
    pub show_perf_hud: bool,

    /// Show REG debug HUD (camera/QC/portal/FPS state)
    pub show_debug_hud: bool,

    /// AAA Ring style (3D torus vs flat)
    pub ring_style_aaa: bool,

    /// Ring flow lines enabled
    pub ring_flow_lines: bool,

    /// Ring specular sparkle enabled
    pub ring_sparkle: bool,

    /// Ring depth fog enabled
    pub ring_fog: bool,

    /// Ring bloom mask enabled
    pub ring_bloom_mask: bool,

    /// BG quality tier (0=Low, 1=Med, 2=High)
    pub bg_quality: u8,

    /// BG quality auto-scaling enabled
    pub bg_quality_auto: bool,

    /// Counter for low FPS warning display (frames remaining)
    pub low_fps_warning_frames: u32,

    /// Counter to debounce low FPS detection (avoid reacting to brief dips)
    fps_low_count: u32,

    /// Skin preset expanded/advanced section
    skin_advanced_expanded: bool,

    /// Selected skin preset index (0=None, 1-5=presets)
    skin_preset_index: usize,

    // === Persisted Skin Settings ===
    /// Skin mode (0=None, 1=Surface, 2=Emissive, 3=SurfaceEmissive)
    skin_mode: i32,
    /// Surface texture mix (0-1)
    skin_surface_mix: f32,
    /// Emissive strength (0-1.5)
    skin_emissive_strength: f32,
    /// Surface contrast (0.5-2.0)
    skin_surf_contrast: f32,
    /// Surface saturation (0-2.0)
    skin_surf_saturation: f32,
    /// Shadow lift (0-0.25)
    skin_shadow_lift: f32,
    /// AO clamp minimum (0-0.6)
    skin_ao_clamp: f32,
    /// Rim/fresnel strength (0-1)
    skin_rim_strength: f32,
    /// Bloom amount (0-1)
    skin_bloom_amount: f32,
    /// Specular sparkle (0-1)
    skin_specular_sparkle: f32,
    /// Edge outline strength (0-1)
    skin_edge_outline: f32,
    /// UV mode (0=cylindrical, 1=triplanar)
    skin_uv_mode: i32,
    /// Starburst lens effect enabled
    skin_starburst_enabled: bool,
    /// Lock UV to dominant pitch
    skin_lock_to_dominant: bool,
    /// Flag to apply loaded skin settings to renderer once
    skin_pending_apply: bool,

    /// "Use Genre for Tempo": when true, the selected genre's BPM range guides
    /// REACT's autonomous tempo octave (double- vs half-time). MIDI-bindable so
    /// the operator can flip it live; never affects external Link/Pro DJ clocks.
    genre_tempo_guidance: bool,

    /// Freeze visualization for scrubbing through history
    frozen: bool,

    /// Scrub position when frozen (0.0 = oldest, 1.0 = newest)
    scrub_position: f32,

    /// Frozen display state (set from app when frozen, used by octave mirror + geometry)
    frozen_display_state: Option<crate::render::HarmonicFieldState>,

    /// Camera preview texture handle
    camera_preview_texture: Option<egui::TextureHandle>,
    /// Last camera frame dimensions (to detect changes)
    camera_preview_dims: (u32, u32),
    /// Show camera preview window
    show_camera_preview: bool,
    /// Camera class filter tags (empty = no filter, tracks all classes)
    camera_filter_tags: Vec<String>,
    /// Input text for adding new filter tag
    camera_filter_input: String,
    /// Track thumbnail textures (keyed by track_id)
    track_thumbnail_textures: HashMap<u32, egui::TextureHandle>,

    // === Camera Tracking Influence (Vision) ===
    /// Per-camera UI state for tracking influence (indexed by camera_index)
    pub camera_ui_states: HashMap<u32, CameraUiState>,
    /// Vision section expanded (formerly cameras_expanded)
    vision_expanded: bool,

    // === Camera Media Background ===
    /// Per-camera media state for background compositing (indexed by camera_index)
    pub camera_media_ui_states: HashMap<u32, crate::camera::CameraMediaState>,
    /// Saved camera states for momentary solo (camera_index, vis_q, enabled)
    camera_solo_saved: Option<Vec<(u32, u8, bool)>>,
    /// Last MIDI camera toggle time (debounce rapid-fire button mashing)
    camera_toggle_last: std::time::Instant,
    /// Cameras section expanded (for background media)
    cameras_expanded: bool,

    // === Authentication ===
    /// Account section expanded
    account_expanded: bool,
    /// One-shot: a sign-in tile link was clicked — force-open the Account
    /// section and scroll to it on the next render.
    jump_to_account: bool,
    /// Login form: username input
    login_username: String,
    /// Login form: password input
    login_password: String,
    /// Show login modal (for full-screen login when trial expired)
    show_login_modal: bool,

    // === YouTube Live ===
    /// YouTube section expanded
    youtube_expanded: bool,
    /// YouTube stream title (prefix before "REACT (Live) by Compeller.ai")
    youtube_stream_title: String,
    /// YouTube stream quality
    youtube_quality: YouTubeQuality,
    /// YouTube stream privacy
    youtube_privacy: YouTubePrivacy,

    // === Twitch ===
    /// Twitch section expanded
    twitch_expanded: bool,
    /// Twitch channel name
    twitch_channel: String,
    /// Twitch stream key (entered by user)
    twitch_stream_key: String,
    /// Twitch stream title
    twitch_stream_title: String,
    /// Twitch stream category/game
    twitch_stream_category: String,
    /// Twitch stream tags (comma-separated)
    twitch_stream_tags: String,
    /// Input buffer for adding new tags
    twitch_new_tag_input: String,
    /// Flag to indicate Twitch settings need saving
    twitch_settings_dirty: bool,
    /// Twitch stream resolution (legacy - for NDI streaming)
    twitch_resolution: TwitchResolution,
    /// Twitch output mode (720p30, 720p60, 1080p30, 1080p60) for ffmpeg-next streaming
    twitch_output_mode: OutputMode,
    /// Twitch stream state
    twitch_state: TwitchStreamState,
    /// Twitch stream start time (for duration display)
    twitch_start_time: Option<std::time::Instant>,

    // === NDI Streaming (Twitch + YouTube) ===
    /// NDI streaming section expanded
    dual_stream_expanded: bool,
    /// Enable Twitch in NDI streaming
    ndi_stream_twitch: bool,
    /// Enable YouTube in NDI streaming
    ndi_stream_youtube: bool,
    /// NDI streaming state
    dual_stream_state: DualStreamState,
    /// NDI streaming start time (for duration display)
    dual_stream_start_time: Option<std::time::Instant>,
    /// Twitch ingest servers with ping results
    twitch_servers: Vec<TwitchServerInfo>,
    /// Show Twitch servers list
    twitch_servers_expanded: bool,

    // === Kick ===
    /// Kick section expanded
    kick_expanded: bool,
    /// Kick ingest stream URL (pasted from the Kick Creator Dashboard)
    kick_stream_url: String,
    /// Kick stream key (entered by user)
    kick_stream_key: String,
    /// Kick stream title
    kick_stream_title: String,
    /// Flag to indicate Kick settings need saving
    kick_settings_dirty: bool,
    /// Kick output mode (720p30, 720p60, 1080p30, 1080p60)
    kick_output_mode: OutputMode,
    /// Kick stream state (reuses the Twitch state machine — same Idle/
    /// Connecting/Streaming/Error lifecycle)
    kick_state: TwitchStreamState,
    /// Kick stream start time (for duration display)
    kick_start_time: Option<std::time::Instant>,

    // === Set Upload ===
    /// Set upload manager for syncing recordings to Compeller.ai
    set_upload_manager: Option<crate::set_sync::SetUploadManager>,
}

/// Twitch ingest server info for UI display
#[derive(Clone, Debug, Default)]
pub struct TwitchServerInfo {
    pub name: String,
    pub host: String,
    pub latency_ms: Option<u32>,
    pub selected: bool,
}

impl Default for ControlPanel {
    fn default() -> Self {
        Self {
            params: RenderParams::default(),
            selected_category: Some("★ Favorites".to_string()),
            categories: Vec::new(),
            presets: Vec::new(),
            video_decoder: VideoDecoder::new(),
            video_path: None,
            video_directory: None,
            video_files: Vec::new(),
            browsed_video_path: None,
            recording_directory: None,
            recordings: Vec::new(),
            upload_status_cache: HashMap::new(),
            editing_recording: None,
            editing_recording_name: String::new(),
            pending_delete_recording: None,
            preset_metadata: HashMap::new(),
            editing_preset: None,
            editing_preset_name: String::new(),
            show_hidden_presets: false,
            managing_preset_tracks: None,
            preset_tracks: Vec::new(),
            new_track_name: String::new(),
            new_track_artist: String::new(),
            video_metadata: HashMap::new(),
            editing_video: None,
            editing_video_name: String::new(),
            show_hidden_videos: false,
            video_search: String::new(),
            video_favorites: std::collections::HashSet::new(),
            recent_videos: Vec::new(),
            show_all_videos: false,
            video_view_mode: VideoViewMode::Grid,  // Default to grid view
            #[cfg(target_os = "macos")]
            thumbnail_generator: None,
            thumbnail_textures: HashMap::new(),
            pending_thumbnail_loads: HashSet::new(),
            thumbnails_loaded_this_frame: 0,
            video_sync: None,
            compel_sync: None,
            last_compel_sync_completed: 0,
            show_sync_window: false,
            show_compel_manager: false,
            manager_selected_compel: None,
            show_recording_manager: false,
            manager_selected_recording: None,
            minimized_windows: HashSet::new(),
            fullscreen_window: None,
            hide_menu_for_fullscreen: false,
            half_screen_mode: false,
            last_sync_completed: 0,
            managing_video_tracks: None,
            video_tracks: Vec::new(),
            new_video_track_name: String::new(),
            new_video_track_artist: String::new(),
            audio_gain: 3.5,  // Default 50% on log slider (0.1-128)
            ndi_audio_gain: 1.0,
            rec_audio_gain: 1.0,
            youtube_audio_gain: 1.0,
            twitch_audio_gain: 1.0,
            kick_audio_gain: 1.0,
            pending_gain_action: OutputAction::None,
            show_audio_meters: true,
            meter_values: None,
            kick_sensitivity: 2.75,  // Default 50% on slider (0.5-5.0)
            projectm_expanded: true,
            video_expanded: true,
            audio_expanded: true,
            output_expanded: true,
            recording_expanded: true,
            recording_view_mode: VideoViewMode::Grid,  // Default to thumbnail view
            overlay_expanded: false,
            prodjlink_expanded: true,
            midi_expanded: false,
            osc_expanded: false,
            dmx_port_edit: String::new(),
            // Seed the Art-Net/OLA fields from env (if the operator already runs
            // the env-var smoke path) else sane defaults; persisted DB values
            // override these in load_persisted_state().
            dmx_artnet_target_edit: std::env::var("REACT_DMX_ARTNET_TARGET").unwrap_or_default(),
            dmx_ola_host_edit: std::env::var("REACT_DMX_OLA_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            dmx_ola_port_edit: std::env::var("REACT_DMX_OLA_PORT").unwrap_or_else(|_| "9010".to_string()),
            virtual_stage_enabled: false,
            virtual_stage_view: crate::emerge::StageViewPresetKind::Foh,
            virtual_stage_zoom: 1.0,
            virtual_stage_yaw: 0.0,
            virtual_stage_pitch: 0.0,
            virtual_stage_labels: true,
            virtual_stage_venue: crate::emerge::VenueKind::ConcertStage,
            venue_cameras: std::collections::HashMap::new(),
            silence_threshold: 3.0,
            osc_target_ip_input: crate::osc::OscSender::detect_broadcast_address(),
            osc_target_port_input: "7672".to_string(),
            osc_resolved_ip_cache: None,
            osc_last_resolved_target: String::new(),
            osc_mapping_expanded: false,
            osc_advanced_expanded: false,
            osc_profile_index: 0,
            osc_palette_ice_name: "All White".to_string(),
            osc_palette_cool_a_name: "Neon Retro".to_string(),
            osc_palette_cool_b_name: "Vivid Saturated".to_string(),
            osc_palette_ice_index: "0".to_string(),
            osc_palette_cool_a_index: "1".to_string(),
            osc_palette_cool_b_index: "2".to_string(),
            osc_pattern_silent: "0".to_string(),
            osc_pattern_still: "0".to_string(),
            osc_pattern_low: "1".to_string(),
            osc_pattern_mid: "2".to_string(),
            osc_pattern_high: "3".to_string(),
            osc_mover_speed_min: 0.2,
            osc_mover_speed_max: 0.8,
            osc_enable_fx_palette: true,
            osc_enable_polarity_flash: true,
            osc_enable_dominant_accent: true,
            osc_enable_tonality_palettes: true,
            osc_enable_energy_slope: true,
            osc_mapping_initialized: false,
            osc_genre_expanded: true,
            osc_genre_category_index: 0,
            osc_genre_subcategory_index: 1, // Tech House
            osc_genre_advanced_expanded: false,
            // MaestroDMX UI state
            maestro_pattern_index: 0,
            maestro_color_palette_index: 0,
            maestro_fx_palette_index: 0,
            // Hue UI state
            hue_expanded: false,
            hue_enabled: false,
            hue_bridge_ip_input: String::new(),
            hue_area_index: 0,
            hue_initialized: false,
            hue_protocol_index: 0,
            hue_mapping_mode_index: 0,
            // WLED UI state
            wled_expanded: false,
            wled_enabled: false,
            wled_strip_ip_input: String::new(),
            wled_ip_error: None,
            wled_initialized: false,
            ndi_resolution: NdiResolution::Hd,
            audio_sources: Vec::new(),
            selected_audio_source: 0,
            show_all_apps: false,
            kick_hold_frames: 0,
            click_hold_frames: 0,
            master_pulse_phase: 0.0,
            master_bpm: None,
            ableton_link_expanded: true,
            clock_override: crate::emerge::ClockOverride::Auto,
            clock_status: ClockStatus::default(),
            render_mode: RenderMode::LedWall,
            current_preset: None,
            pending_preset_load: None,
            pending_preset_load_frames: 0,
            browse_favorites_index: None,
            browse_favorites_timer: None,
            browse_favorites_opacity: 0.0,
            browse_compels_index: None,
            browse_compels_timer: None,
            browse_compels_opacity: 0.0,
            db: None,
            favorites: Vec::new(),
            pinned_presets: Vec::new(),
            favorites_dirty: true,
            midi_browse_list: Vec::new(),
            midi_browse_dirty: true,
            scroll_to_preset: false,
            auto_randomize_theme: false,
            auto_randomize_next: None,
            auto_randomize_last_seen: None,
            fps_shuffle_low_since: None,
            fps_shuffle_cooldown_until: None,
            search_query: String::new(),
            fuzzy_matcher: SkimMatcherV2::default(),
            video_scale_target: 1.0,  // Match default video_scale
            video_scale_velocity: 0.0,
            settings_dirty: false,
            save_debounce_frames: 0,
            midi_learn_armed: false,
            pending_midi_action: MidiAction::None,
            lfo_bank: LfoBankConfig::default(),
            // Mythos defaults
            mythos_instability: 0.50,
            mythos_field_strength: 0.60,
            mythos_disturbance_size: 0.40,
            mythos_harmonic_influence: 0.30,
            mythos_decay: 0.50,
            // Harmonic Geometry defaults
            tuning_reference: TuningReference::Hz440,  // Standard concert pitch by default
            show_chroma_key: false,    // Chroma Wheel off by default
            key_lock_beats: 8,
            key_confidence_threshold: 0.6,
            harmonic_expanded: true,  // Start expanded so HUD toggle is visible
            performance_expanded: false,  // Performance section collapsed by default
            show_math_overlay: false,  // Math HUD off by default
            show_qs_wheel: true,       // QS Wheel visible by default
            hide_qs_numbers: true,     // Hide numbers by default
            qs_dominant_only: true,    // Default to dominant-only mode
            quality_tier: 1,           // Default to Medium quality
            show_perf_hud: false,      // Perf HUD off by default
            show_debug_hud: false,     // REG debug HUD off by default
            // AAA Ring defaults (all enabled for best look)
            ring_style_aaa: true,
            ring_flow_lines: true,
            ring_sparkle: true,
            ring_fog: true,
            ring_bloom_mask: true,
            // BG Quality defaults
            bg_quality: 1,        // Default to Medium
            bg_quality_auto: true, // Auto-scaling enabled
            // Low FPS detection
            low_fps_warning_frames: 0,
            fps_low_count: 0,
            skin_advanced_expanded: false,
            skin_preset_index: 1,  // Default to "Compel Surface" for best video visibility
            // Hero skin defaults (from FieldSkin::hero())
            skin_mode: 3,              // SurfaceEmissive
            skin_surface_mix: 0.9,
            skin_emissive_strength: 0.5,
            skin_surf_contrast: 1.35,
            skin_surf_saturation: 1.25,
            skin_shadow_lift: 0.08,
            skin_ao_clamp: 0.35,
            skin_rim_strength: 0.25,
            skin_bloom_amount: 0.3,
            skin_specular_sparkle: 0.2,
            skin_edge_outline: 0.0,
            skin_uv_mode: 1,           // Triplanar
            skin_starburst_enabled: false,
            skin_lock_to_dominant: false,
            skin_pending_apply: false,
            genre_tempo_guidance: true, // genre guides REACT tempo octave by default
            frozen: false,
            scrub_position: 1.0,       // Start at newest
            frozen_display_state: None,
            camera_preview_texture: None,
            camera_preview_dims: (0, 0),
            show_camera_preview: false,
            camera_filter_tags: Vec::new(),
            camera_filter_input: String::new(),
            track_thumbnail_textures: HashMap::new(),
            // Camera tracking influence (Vision)
            camera_ui_states: HashMap::new(),
            vision_expanded: false,
            // Camera media background
            camera_media_ui_states: HashMap::new(),
            camera_solo_saved: None,
            camera_toggle_last: std::time::Instant::now() - std::time::Duration::from_secs(1),
            cameras_expanded: false,

            // Auth fields
            account_expanded: false,
            jump_to_account: false,
            login_username: String::new(),
            login_password: String::new(),
            show_login_modal: false,

            // YouTube fields
            youtube_expanded: true,
            youtube_stream_title: String::new(),
            youtube_quality: YouTubeQuality::default(),
            youtube_privacy: YouTubePrivacy::default(),

            // Twitch fields
            twitch_expanded: false,
            twitch_channel: String::new(),
            twitch_stream_key: String::new(),
            twitch_stream_title: String::new(),
            twitch_stream_category: "Music".to_string(),
            twitch_stream_tags: "DJ, House, Techno, Electronic".to_string(),
            twitch_new_tag_input: String::new(),
            twitch_settings_dirty: false,
            twitch_resolution: TwitchResolution::default(),
            twitch_output_mode: OutputMode::default(), // 720p30 default
            twitch_state: TwitchStreamState::default(),
            twitch_start_time: None,

            // NDI streaming fields
            dual_stream_expanded: false,
            ndi_stream_twitch: true,  // Default to Twitch enabled
            ndi_stream_youtube: false, // YouTube requires setup first
            dual_stream_state: DualStreamState::default(),
            dual_stream_start_time: None,
            twitch_servers: Vec::new(),
            twitch_servers_expanded: false,

            // Kick fields
            kick_expanded: false,
            kick_stream_url: String::new(),
            kick_stream_key: String::new(),
            kick_stream_title: String::new(),
            kick_settings_dirty: false,
            kick_output_mode: OutputMode::default(), // 720p30 default
            kick_state: TwitchStreamState::default(),
            kick_start_time: None,

            // Set upload
            set_upload_manager: None,
        }
    }
}

/// Actions that can be requested from the auth section
#[derive(Clone, Debug, PartialEq)]
pub enum AuthAction {
    None,
    Login { username: String, password: String },
    /// Browser sign-in handoff (Google or password on the web).
    GoogleSignIn,
    Logout,
}

impl Default for AuthAction {
    fn default() -> Self {
        AuthAction::None
    }
}

/// YouTube stream quality preset (all 60fps)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum YouTubeQuality {
    /// 1080p @ 60fps, 12 Mbps (HD)
    #[default]
    Hd60,
    /// 1440p @ 60fps, 24 Mbps (2K)
    Qhd60,
    /// 2160p @ 60fps, 35 Mbps (4K)
    Uhd60,
}

impl YouTubeQuality {
    /// Get resolution string for YouTube API
    pub fn resolution(&self) -> &'static str {
        match self {
            YouTubeQuality::Hd60 => "1080p",
            YouTubeQuality::Qhd60 => "1440p",
            YouTubeQuality::Uhd60 => "2160p",
        }
    }

    /// Get frame rate string for YouTube API
    pub fn frame_rate(&self) -> &'static str {
        "60fps"
    }

    /// Get bitrate in kbps
    pub fn bitrate_kbps(&self) -> u32 {
        match self {
            YouTubeQuality::Hd60 => 12000,
            YouTubeQuality::Qhd60 => 24000,
            YouTubeQuality::Uhd60 => 35000,
        }
    }

    /// Get FPS as number
    pub fn fps(&self) -> u32 {
        60
    }

    /// Display name
    pub fn label(&self) -> &'static str {
        match self {
            YouTubeQuality::Hd60 => "HD 60",
            YouTubeQuality::Qhd60 => "2K 60",
            YouTubeQuality::Uhd60 => "4K 60",
        }
    }

    /// Get output mode for GStreamer
    pub fn output_mode(&self) -> crate::output::OutputMode {
        match self {
            YouTubeQuality::Hd60 => crate::output::OutputMode::P1080_60,
            YouTubeQuality::Qhd60 => crate::output::OutputMode::P1440_60,
            YouTubeQuality::Uhd60 => crate::output::OutputMode::P2160_60,
        }
    }
}

/// YouTube stream privacy setting
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum YouTubePrivacy {
    /// Visible to everyone
    Public,
    /// Only accessible via direct link
    #[default]
    Unlisted,
    /// Only visible to you
    Private,
}

impl YouTubePrivacy {
    /// Get API string
    pub fn api_value(&self) -> &'static str {
        match self {
            YouTubePrivacy::Public => "public",
            YouTubePrivacy::Unlisted => "unlisted",
            YouTubePrivacy::Private => "private",
        }
    }

    /// Display name
    pub fn label(&self) -> &'static str {
        match self {
            YouTubePrivacy::Public => "Public",
            YouTubePrivacy::Unlisted => "Unlisted",
            YouTubePrivacy::Private => "Private",
        }
    }
}

/// YouTube stream configuration
#[derive(Clone, Debug, PartialEq, Default)]
pub struct YouTubeStreamConfig {
    pub quality: YouTubeQuality,
    pub privacy: YouTubePrivacy,
}

/// Actions that can be requested from the YouTube section
#[derive(Clone, Debug, PartialEq)]
pub enum YouTubeAction {
    None,
    /// Connect YouTube account (opens browser to OAuth)
    ConnectYouTube,
    /// Disconnect YouTube account
    DisconnectYouTube,
    /// Start a new live session (GoLive clicked) with stream title and config
    GoLive { title: String, config: YouTubeStreamConfig },
    /// Stop the current live session
    StopLive,
    /// Retry after failure
    RetryLive,
    /// Dismiss ended/failed state (return to Idle)
    Dismiss,
}

impl Default for YouTubeAction {
    fn default() -> Self {
        YouTubeAction::None
    }
}

/// YouTube state passed to the UI for rendering
pub struct YouTubeState<'a> {
    pub auth: &'a crate::youtube::auth::YouTubeAuth,
    pub eligibility: &'a crate::youtube::eligibility::EligibilityCache,
    pub session: Option<&'a crate::youtube::session::LiveSession>,
}

// ==================== Twitch Types ====================

/// Twitch streaming resolution
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TwitchResolution {
    #[default]
    Hd,   // 1080p
    Uhd,  // 4K
}

impl TwitchResolution {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            TwitchResolution::Hd => (1920, 1080),
            TwitchResolution::Uhd => (3840, 2160),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TwitchResolution::Hd => "HD (1080p)",
            TwitchResolution::Uhd => "4K (2160p)",
        }
    }

    pub fn bitrate(&self) -> u32 {
        match self {
            TwitchResolution::Hd => 6000,   // 1080p60 - Twitch recommended max
            TwitchResolution::Uhd => 25000, // 4K60 (requires Twitch Partner)
        }
    }
}

/// Twitch streaming state
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TwitchStreamState {
    #[default]
    Idle,
    Connecting,
    Streaming,
    Error(String),
}

/// Actions that can be requested from the Twitch section
#[derive(Clone, Debug, PartialEq)]
pub enum TwitchAction {
    None,
    /// Start streaming to Twitch
    GoLive {
        stream_key: String,
        output_mode: OutputMode,
        channel: String,
        title: String,
        category: String,
        tags: String,
    },
    /// Stop streaming
    StopLive,
}

impl Default for TwitchAction {
    fn default() -> Self {
        TwitchAction::None
    }
}

// ==================== Kick Types ====================

/// Actions returned from the Kick section.
///
/// Kick reuses `TwitchStreamState` for its lifecycle (same Idle/Connecting/
/// Streaming/Error machine), so only the action enum is Kick-specific.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum KickAction {
    #[default]
    None,
    /// Start streaming to Kick
    GoLive {
        /// Ingest URL pasted from the Kick Creator Dashboard (e.g.
        /// `rtmps://...global-contribute.live-video.net:443/app`)
        stream_url: String,
        stream_key: String,
        output_mode: OutputMode,
        title: String,
    },
    /// Stop streaming
    StopLive,
}

/// Actions for NDI-based streaming (Twitch, YouTube, or both)
#[derive(Clone, Debug, PartialEq)]
pub enum DualStreamAction {
    None,
    /// Start streaming to Twitch only (via ffmpeg-next)
    StartTwitch {
        twitch_stream_key: String,
        /// Output mode (720p30, 720p60, 1080p30, 1080p60)
        output_mode: OutputMode,
        /// Channel name
        channel: String,
        /// Stream title
        title: String,
        /// Stream category (e.g., "DJs", "Music")
        category: String,
        /// Stream tags (e.g., "DJ, House, Techno")
        tags: String,
    },
    /// Start streaming to YouTube only (via NDI)
    StartYouTube {
        youtube_rtmp_url: String,
    },
    /// Stop streaming
    StopStream,
}

impl Default for DualStreamAction {
    fn default() -> Self {
        DualStreamAction::None
    }
}

/// Streaming target for NDI-based streaming
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StreamTarget {
    Twitch,
    YouTube,
    Both,
}

/// NDI streaming state
#[derive(Clone, Debug, PartialEq)]
pub enum DualStreamState {
    /// Not streaming
    Idle,
    /// Connecting to platform(s)
    Connecting(StreamTarget),
    /// Streaming to platform(s)
    Streaming(StreamTarget),
    /// Error occurred
    Error(String),
}

impl Default for DualStreamState {
    fn default() -> Self {
        DualStreamState::Idle
    }
}

impl ControlPanel {
    /// Mark settings as dirty (need saving)
    fn mark_dirty(&mut self) {
        self.settings_dirty = true;
        self.save_debounce_frames = 60; // Save after ~1 second at 60fps
    }

    /// Check and perform debounced save
    fn check_save(&mut self) {
        if self.settings_dirty && self.save_debounce_frames > 0 {
            self.save_debounce_frames -= 1;
            if self.save_debounce_frames == 0 {
                self.save_settings();
                self.settings_dirty = false;
                log::debug!("[UI] Settings saved to database");
            }
        }
    }

    /// Whether the operator virtual stage preview is toggled on (Lighting menu).
    pub fn virtual_stage_enabled(&self) -> bool {
        self.virtual_stage_enabled
    }

    /// Set the virtual stage toggle — cleared when the operator exits the stage.
    pub fn set_virtual_stage_enabled(&mut self, enabled: bool) {
        self.virtual_stage_enabled = enabled;
    }

    /// Selected virtual stage camera preset (Lighting menu).
    pub fn virtual_stage_view(&self) -> crate::emerge::StageViewPresetKind {
        self.virtual_stage_view
    }

    /// Focal zoom — 1.0 = preset FOV, >1 zoomed in, <1 zoomed out.
    pub fn virtual_stage_zoom(&self) -> f32 {
        self.virtual_stage_zoom
    }

    /// Camera orbit yaw around look_at (degrees).
    pub fn virtual_stage_yaw(&self) -> f32 {
        self.virtual_stage_yaw
    }

    /// Camera orbit pitch above/below preset elevation (degrees).
    pub fn virtual_stage_pitch(&self) -> f32 {
        self.virtual_stage_pitch
    }

    /// Whether the floating fixture-name labels are shown on the virtual stage.
    pub fn virtual_stage_labels(&self) -> bool {
        self.virtual_stage_labels
    }

    /// The venue/environment the virtual stage renders (Concert Stage vs Wedding Room).
    pub fn virtual_stage_venue(&self) -> crate::emerge::VenueKind {
        self.virtual_stage_venue
    }

    /// Switch the virtual-stage venue, swapping the saved camera so each room
    /// keeps its own framing. The leaving venue's active camera is stashed; the
    /// entering venue's camera is loaded (a sensible default the first time).
    /// Flags a debounced persist. No-op if the venue is unchanged.
    pub fn set_virtual_stage_venue(&mut self, venue: crate::emerge::VenueKind) {
        if venue == self.virtual_stage_venue {
            return;
        }
        // Stash the active camera under the venue we're leaving.
        self.venue_cameras.insert(
            self.virtual_stage_venue,
            (self.virtual_stage_zoom, self.virtual_stage_yaw, self.virtual_stage_pitch),
        );
        self.virtual_stage_venue = venue;
        // Load the entering venue's camera, or a default view the first time.
        let (z, y, p) = self
            .venue_cameras
            .get(&venue)
            .copied()
            .unwrap_or_else(|| Self::default_venue_camera(venue));
        self.virtual_stage_zoom = z;
        self.virtual_stage_yaw = y;
        self.virtual_stage_pitch = p;
        self.mark_dirty();
    }

    /// A sensible starting camera per venue the first time it's entered. The
    /// Wedding Room reads better from a slightly pulled-back, gently elevated
    /// angle than the concert FOH default.
    fn default_venue_camera(venue: crate::emerge::VenueKind) -> (f32, f32, f32) {
        match venue {
            crate::emerge::VenueKind::WeddingRoom => (0.9, 0.0, 18.0),
            crate::emerge::VenueKind::ConcertStage => (1.0, 0.0, 0.0),
        }
    }

    /// Silence-gate hold in seconds (Lighting-menu slider).
    pub fn silence_threshold(&self) -> f32 {
        self.silence_threshold
    }
}

impl ControlPanel {
    /// Set database reference for favorites and load persisted state
    /// COM-499 Phase B4: the persisted/edited operator LFO bank (config-only).
    /// The app pushes this to the OSC runtime on startup and on each edit.
    pub fn lfo_bank(&self) -> LfoBankConfig {
        self.lfo_bank
    }

    pub fn set_database(&mut self, db: Database) {
        // Load all persisted state
        let state = db.load_state();
        log::info!("[UI] Loading persisted state from database");
        // COM-499 Phase B4: hydrate the operator LFO bank (the app pushes it to
        // the OSC runtime right after this).
        self.lfo_bank = db.load_lfo_bank();
        // Seed the DMX panel's editable fields from each backend's persisted
        // endpoint so they match the restored output on launch (the status line
        // shows the live value regardless; this keeps the input boxes consistent).
        // Loaded per-backend (not just the active one) so switching backends shows
        // the last target you used for that backend.
        if let Some(port) = db.get_dmx_usb_port() {
            self.dmx_port_edit = port;
        }
        if let Some(target) = db.get_dmx_artnet_target() {
            self.dmx_artnet_target_edit = target;
        }
        if let Some(endpoint) = db.get_dmx_ola_endpoint() {
            // Persisted as "host:port"; split back into the two fields (bare host
            // keeps the default port already in dmx_ola_port_edit).
            if let Some((host, port)) = endpoint.rsplit_once(':') {
                if !host.is_empty() {
                    self.dmx_ola_host_edit = host.to_string();
                    self.dmx_ola_port_edit = port.to_string();
                }
            } else if !endpoint.trim().is_empty() {
                self.dmx_ola_host_edit = endpoint;
            }
        }

        // Apply settings to control panel
        self.audio_gain = state.audio_gain;
        self.kick_sensitivity = state.kick_sensitivity;
        self.silence_threshold = state.silence_threshold;
        // Restore the saved virtual-stage camera view.
        self.virtual_stage_zoom = state.virtual_stage_zoom;
        self.virtual_stage_yaw = state.virtual_stage_yaw;
        self.virtual_stage_pitch = state.virtual_stage_pitch;
        self.virtual_stage_labels = state.virtual_stage_labels;
        // Venue is an enum, so it round-trips through the raw string KV store
        // (like fixture_segment_fx) rather than the typed AppState. Unknown /
        // missing keys fall back to the default (Concert Stage).
        self.virtual_stage_venue = db
            .get_setting("virtual_stage_venue")
            .and_then(|key| crate::emerge::VenueKind::from_key(&key))
            .unwrap_or_default();
        // Per-venue cameras; the active venue's camera overrides the single-camera
        // values loaded above so each room reopens with its own framing.
        self.venue_cameras = db
            .get_setting("virtual_stage_venue_cameras")
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if let Some(&(z, y, p)) = self.venue_cameras.get(&self.virtual_stage_venue) {
            self.virtual_stage_zoom = z;
            self.virtual_stage_yaw = y;
            self.virtual_stage_pitch = p;
        }
        self.params.video_visibility = state.video_visibility;
        self.params.video_scale = state.video_scale;
        self.video_scale_target = state.video_scale;
        self.params.video_luma_key = state.video_luma_key;
        self.params.camera_luma_key = state.camera_luma_key;
        self.show_audio_meters = state.show_audio_meters;
        self.projectm_expanded = state.projectm_expanded;
        self.video_expanded = state.video_expanded;
        self.audio_expanded = state.audio_expanded;
        self.show_all_apps = state.show_all_apps;
        self.params.projectm_visibility = state.projectm_visibility;

        // Restore Harmonic Field settings
        self.tuning_reference = match state.tuning_reference.as_str() {
            "432" => TuningReference::Hz432,
            _ => TuningReference::Hz440,
        };
        self.show_chroma_key = state.show_chroma_wheel;
        self.show_math_overlay = state.show_math_hud;
        self.hide_qs_numbers = state.hide_qs_numbers;
        self.show_qs_wheel = state.show_qs_wheel;
        self.qs_dominant_only = state.qs_dominant_only;
        self.show_perf_hud = state.show_perf_hud;
        self.show_debug_hud = state.show_debug_hud;
        self.quality_tier = state.quality_tier;

        // Restore Mythos settings
        self.mythos_instability = state.mythos_instability;
        self.mythos_field_strength = state.mythos_field_strength;
        self.mythos_disturbance_size = state.mythos_disturbance_size;
        self.mythos_harmonic_influence = state.mythos_harmonic_influence;
        self.mythos_decay = state.mythos_decay;
        self.bg_quality = state.bg_quality;
        self.bg_quality_auto = state.bg_quality_auto;

        // Restore Hero skin settings
        self.skin_mode = state.skin_mode;
        self.skin_surface_mix = state.skin_surface_mix;
        self.skin_emissive_strength = state.skin_emissive_strength;
        self.skin_surf_contrast = state.skin_surf_contrast;
        self.skin_surf_saturation = state.skin_surf_saturation;
        self.skin_shadow_lift = state.skin_shadow_lift;
        self.skin_ao_clamp = state.skin_ao_clamp;
        self.skin_rim_strength = state.skin_rim_strength;
        self.skin_bloom_amount = state.skin_bloom_amount;
        self.skin_specular_sparkle = state.skin_specular_sparkle;
        self.skin_edge_outline = state.skin_edge_outline;
        self.skin_uv_mode = state.skin_uv_mode;
        self.skin_starburst_enabled = state.skin_starburst_enabled;
        self.skin_lock_to_dominant = state.skin_lock_to_dominant;
        self.skin_pending_apply = true;  // Flag to apply to renderer on first frame
        log::info!("[UI] Loaded skin settings: mode={}, contrast={}, emissive={}",
            self.skin_mode, self.skin_surf_contrast, self.skin_emissive_strength);

        // Restore render mode
        self.render_mode = match state.render_mode.as_str() {
            "led_wall" => RenderMode::LedWall,
            "harmonic_geometry" => RenderMode::HarmonicGeometry,
            "mythos" => RenderMode::Mythos,
            _ => RenderMode::ProjectM,
        };

        // Restore overlay settings
        self.params.logo_visible = state.logo_visible;
        self.params.logo_opacity = state.logo_opacity;
        self.params.logo_scale = state.logo_scale;
        self.params.logo_position = match state.logo_position.as_str() {
            "top_left" => OverlayPosition::TopLeft,
            "top_right" => OverlayPosition::TopRight,
            "bottom_left" => OverlayPosition::BottomLeft,
            "center" => OverlayPosition::Center,
            _ => OverlayPosition::BottomRight,
        };
        self.params.custom_logo_path = state.custom_logo_path.clone();

        // Always start on Favorites category (don't restore last_category)
        // But restore current preset so it continues playing where it left off
        if let (Some(cat), Some(preset)) = (state.last_category, state.last_preset) {
            self.current_preset = Some((cat, preset));
        } else {
            // Default to Hero's Journey on fresh install
            self.current_preset = Some(("Geometric Field".to_string(), "Hero's Journey".to_string()));
        }

        // Load persisted video directory
        if let Some(ref dir) = state.video_directory {
            if std::path::Path::new(&dir).exists() {
                log::info!("[UI] Restoring video directory: {}", dir);
                self.video_directory = Some(dir.clone());
                self.scan_video_directory(dir);
            }
        }

        // Load persisted recording directory
        if let Some(ref dir) = state.recording_directory {
            if std::path::Path::new(&dir).exists() {
                log::info!("[UI] Restoring recording directory: {}", dir);
                self.recording_directory = Some(dir.clone());
                self.scan_recording_directory(&db, dir);
            }
        }

        self.db = Some(db);
        self.favorites_dirty = true;
        self.midi_browse_dirty = true;

        // Load preset metadata cache
        self.refresh_preset_metadata();

        // Load video metadata cache
        self.refresh_video_metadata();

        // Load video favorites and recent
        self.load_video_favorites_and_recent();
    }

    /// Set the upload manager for syncing recordings to Compeller.ai
    pub fn set_upload_manager(&mut self, manager: crate::set_sync::SetUploadManager) {
        self.set_upload_manager = Some(manager);
        // Refresh the upload status cache after manager is set (cleanup may have run)
        if let Some(ref db) = self.db {
            self.upload_status_cache = db.get_upload_status_map();
        }
    }

    /// Get upload manager reference (for status checks)
    pub fn get_upload_manager(&self) -> Option<&crate::set_sync::SetUploadManager> {
        self.set_upload_manager.as_ref()
    }

    /// Save current settings to database
    pub fn save_settings(&self) {
        if let Some(ref db) = self.db {
            let render_mode_str = match self.render_mode {
                RenderMode::LedWall => "led_wall",
                RenderMode::ProjectM => "projectm",
                RenderMode::HarmonicGeometry => "harmonic_geometry",
                RenderMode::Mythos => "mythos",
            };

            // Save individual settings (more efficient than full state)
            let _ = db.set_float("audio_gain", self.audio_gain);
            let _ = db.set_float("kick_sensitivity", self.kick_sensitivity);
            let _ = db.set_float("silence_threshold", self.silence_threshold);
            let _ = db.set_float("video_visibility", self.params.video_visibility);
            let _ = db.set_float("video_scale", self.video_scale_target);
            let _ = db.set_float("video_luma_key", self.params.video_luma_key);
            let _ = db.set_float("camera_luma_key", self.params.camera_luma_key);
            let _ = db.set_bool("show_audio_meters", self.show_audio_meters);
            let _ = db.set_bool("projectm_expanded", self.projectm_expanded);
            let _ = db.set_bool("video_expanded", self.video_expanded);
            let _ = db.set_bool("audio_expanded", self.audio_expanded);
            let _ = db.set_bool("show_all_apps", self.show_all_apps);
            let _ = db.set_float("projectm_visibility", self.params.projectm_visibility);
            let _ = db.set_setting("render_mode", render_mode_str);

            // Harmonic Field settings
            let tuning_str = match self.tuning_reference {
                TuningReference::Hz432 => "432",
                TuningReference::Hz440 => "440",
            };
            let _ = db.set_setting("tuning_reference", tuning_str);
            let _ = db.set_bool("show_chroma_wheel", self.show_chroma_key);
            let _ = db.set_bool("show_math_hud", self.show_math_overlay);
            let _ = db.set_bool("hide_qs_numbers", self.hide_qs_numbers);
            let _ = db.set_bool("show_qs_wheel", self.show_qs_wheel);
            let _ = db.set_bool("qs_dominant_only", self.qs_dominant_only);
            let _ = db.set_bool("show_perf_hud", self.show_perf_hud);
            let _ = db.set_bool("show_debug_hud", self.show_debug_hud);

            // Mythos settings
            let _ = db.set_float("mythos_instability", self.mythos_instability);
            let _ = db.set_float("mythos_field_strength", self.mythos_field_strength);
            let _ = db.set_float("mythos_disturbance_size", self.mythos_disturbance_size);
            let _ = db.set_float("mythos_harmonic_influence", self.mythos_harmonic_influence);
            let _ = db.set_float("mythos_decay", self.mythos_decay);
            let _ = db.set_setting("quality_tier", &self.quality_tier.to_string());
            let _ = db.set_setting("bg_quality", &self.bg_quality.to_string());
            let _ = db.set_bool("bg_quality_auto", self.bg_quality_auto);

            // Hero skin settings
            let _ = db.set_setting("skin_mode", &self.skin_mode.to_string());
            let _ = db.set_float("skin_surface_mix", self.skin_surface_mix);
            let _ = db.set_float("skin_emissive_strength", self.skin_emissive_strength);
            let _ = db.set_float("skin_surf_contrast", self.skin_surf_contrast);
            let _ = db.set_float("skin_surf_saturation", self.skin_surf_saturation);
            let _ = db.set_float("skin_shadow_lift", self.skin_shadow_lift);
            let _ = db.set_float("skin_ao_clamp", self.skin_ao_clamp);
            let _ = db.set_float("skin_rim_strength", self.skin_rim_strength);
            let _ = db.set_float("skin_bloom_amount", self.skin_bloom_amount);
            let _ = db.set_float("skin_specular_sparkle", self.skin_specular_sparkle);
            let _ = db.set_float("skin_edge_outline", self.skin_edge_outline);
            let _ = db.set_setting("skin_uv_mode", &self.skin_uv_mode.to_string());
            let _ = db.set_bool("skin_starburst_enabled", self.skin_starburst_enabled);
            let _ = db.set_bool("skin_lock_to_dominant", self.skin_lock_to_dominant);

            // Overlay settings
            let _ = db.set_bool("logo_visible", self.params.logo_visible);
            let _ = db.set_float("logo_opacity", self.params.logo_opacity);
            let _ = db.set_float("logo_scale", self.params.logo_scale);
            let logo_pos_str = match self.params.logo_position {
                OverlayPosition::TopLeft => "top_left",
                OverlayPosition::TopRight => "top_right",
                OverlayPosition::BottomLeft => "bottom_left",
                OverlayPosition::BottomRight => "bottom_right",
                OverlayPosition::Center => "center",
            };
            let _ = db.set_setting("logo_position", logo_pos_str);
            if let Some(ref path) = self.params.custom_logo_path {
                let _ = db.set_setting("custom_logo_path", path);
            } else {
                let _ = db.delete_setting("custom_logo_path");
            }

            if let Some((ref cat, ref preset)) = self.current_preset {
                let _ = db.set_setting("last_category", cat);
                let _ = db.set_setting("last_preset", preset);
            }

            // Save camera tracking settings
            for (idx, cam_state) in &self.camera_ui_states {
                let _ = db.save_camera_setting(*idx, cam_state.vis_q, cam_state.enabled);
            }

            // Save camera media settings (background layers)
            for (idx, cam_state) in &self.camera_media_ui_states {
                let _ = db.save_camera_media_setting(*idx, cam_state.vis_q, cam_state.enabled);
            }

            // Virtual-stage camera view (zoom / orbit yaw / orbit pitch).
            let _ = db.set_float("virtual_stage_zoom", self.virtual_stage_zoom);
            let _ = db.set_float("virtual_stage_yaw", self.virtual_stage_yaw);
            let _ = db.set_float("virtual_stage_pitch", self.virtual_stage_pitch);
            let _ = db.set_bool("virtual_stage_labels", self.virtual_stage_labels);
            let _ = db.set_setting("virtual_stage_venue", self.virtual_stage_venue.as_key());
            // Per-venue cameras: fold the active venue's live camera in, persist all.
            let mut cams = self.venue_cameras.clone();
            cams.insert(
                self.virtual_stage_venue,
                (self.virtual_stage_zoom, self.virtual_stage_yaw, self.virtual_stage_pitch),
            );
            if let Ok(json) = serde_json::to_string(&cams) {
                let _ = db.set_setting("virtual_stage_venue_cameras", &json);
            }
        }
    }

    /// Load camera tracking settings from database and apply to UI states
    pub fn load_camera_settings(&mut self) {
        if let Some(ref db) = self.db {
            let settings = db.load_camera_settings();
            for (idx, (vis, enabled)) in settings {
                if let Some(cam_state) = self.camera_ui_states.get_mut(&idx) {
                    cam_state.vis_q = vis;
                    cam_state.vis_committed = vis;
                    cam_state.enabled = enabled;
                }
            }
        }
    }

    /// Load camera media settings from database and apply to UI states
    pub fn load_camera_media_settings(&mut self) {
        if let Some(ref db) = self.db {
            let settings = db.load_camera_media_settings();
            for (idx, (vis, enabled)) in settings {
                if let Some(cam_state) = self.camera_media_ui_states.get_mut(&idx) {
                    cam_state.vis_q = vis;
                    cam_state.enabled = enabled;
                } else {
                    // Create new state if camera not yet discovered
                    let mut state = crate::camera::CameraMediaState::new(idx, format!("Camera {}", idx));
                    state.vis_q = vis;
                    state.enabled = enabled;
                    self.camera_media_ui_states.insert(idx, state);
                }
            }
        }
    }

    /// Set the latest fundsp meter values (call before show())
    pub fn set_meter_values(&mut self, values: MeterValues) {
        self.meter_values = Some(values);
    }

    /// Set audio gain (from MIDI)
    pub fn set_audio_gain(&mut self, gain: f32) {
        self.audio_gain = gain;
        self.mark_dirty();
    }

    // Mythos parameter setters (from MIDI)
    pub fn set_mythos_instability(&mut self, v: f32) { self.mythos_instability = v; self.mark_dirty(); }
    pub fn set_mythos_field_strength(&mut self, v: f32) { self.mythos_field_strength = v; self.mark_dirty(); }
    pub fn set_mythos_disturbance_size(&mut self, v: f32) { self.mythos_disturbance_size = v; self.mark_dirty(); }
    pub fn set_mythos_harmonic_influence(&mut self, v: f32) { self.mythos_harmonic_influence = v; self.mark_dirty(); }
    pub fn set_mythos_decay(&mut self, v: f32) { self.mythos_decay = v; self.mark_dirty(); }

    /// Set kick sensitivity (from MIDI)
    pub fn set_kick_sensitivity(&mut self, sensitivity: f32) {
        self.kick_sensitivity = sensitivity;
    }

    /// Set projectM visibility (from MIDI)
    pub fn set_projectm_visibility(&mut self, visibility: f32) {
        self.params.projectm_visibility = visibility;
        self.mark_dirty();
    }

    /// Set video visibility (from MIDI)
    pub fn set_video_visibility(&mut self, visibility: f32) {
        self.params.video_visibility = visibility;
        self.mark_dirty();
    }

    /// Set video scale (from MIDI)
    pub fn set_video_scale(&mut self, scale: f32) {
        self.video_scale_target = scale;
        self.mark_dirty();
    }

    pub fn set_video_luma_key(&mut self, luma_key: f32) {
        self.params.video_luma_key = luma_key;
        self.mark_dirty();
    }

    /// Set logo opacity (from MIDI)
    pub fn set_logo_opacity(&mut self, opacity: f32) {
        self.params.logo_opacity = opacity;
        self.mark_dirty();
    }

    /// Set logo scale (from MIDI)
    pub fn set_logo_scale(&mut self, scale: f32) {
        self.params.logo_scale = scale;
        self.mark_dirty();
    }

    /// Toggle logo visibility (from MIDI)
    pub fn toggle_logo_visible(&mut self) {
        self.params.logo_visible = !self.params.logo_visible;
        self.mark_dirty();
    }

    /// Set camera visibility from MIDI CC (0-127 mapped to 0-16 steps)
    pub fn set_camera_visibility_midi(&mut self, camera_index: u32, cc_value: u8) {
        if let Some(cam_state) = self.camera_ui_states.get_mut(&camera_index) {
            if cam_state.process_midi_cc(cc_value) {
                self.mark_dirty();
            }
        }
    }

    /// Toggle camera enable from MIDI
    pub fn toggle_camera_enable(&mut self, camera_index: u32) {
        if let Some(cam_state) = self.camera_ui_states.get_mut(&camera_index) {
            if cam_state.toggle_enable() {
                self.mark_dirty();
            }
        }
    }

    /// Set camera media visibility from MIDI CC (0-127 mapped to 0-16 steps)
    pub fn set_camera_media_visibility_midi(&mut self, camera_index: u32, cc_value: u8) {
        // Direct CC→step mapping: 0-127 → 0-16, no pickup/hysteresis/rate-limiting
        let step = (cc_value as f32 / 127.0 * 16.0).round() as u8;

        if let Some(cam_state) = self.camera_media_ui_states.get_mut(&camera_index) {
            if cam_state.vis_q != step {
                cam_state.set_visibility_immediate(step);
                self.mark_dirty();
            }
        } else {
            // Camera not discovered yet - create state and set value
            let mut state = crate::camera::CameraMediaState::new(camera_index, format!("Camera {}", camera_index));
            state.set_visibility_immediate(step);
            self.camera_media_ui_states.insert(camera_index, state);
            self.mark_dirty();
        }
    }

    /// Toggle camera media enable from MIDI (exclusive: one camera at a time)
    pub fn toggle_camera_media_enable(&mut self, camera_index: u32) {
        // Debounce rapid-fire MIDI toggles (DJs mash buttons fast)
        let now = std::time::Instant::now();
        if now.duration_since(self.camera_toggle_last).as_millis() < 150 {
            return;
        }
        self.camera_toggle_last = now;

        let was_enabled = self.camera_media_ui_states
            .get(&camera_index).map(|s| s.enabled).unwrap_or(false);
        // Disable all others when enabling
        if !was_enabled {
            for (_idx, cs) in &mut self.camera_media_ui_states {
                cs.enabled = false;
            }
        }
        if let Some(cam_state) = self.camera_media_ui_states.get_mut(&camera_index) {
            cam_state.toggle_enable();
            self.mark_dirty();
        } else {
            // Camera not discovered yet - create enabled state
            let mut state = crate::camera::CameraMediaState::new(camera_index, format!("Camera {}", camera_index));
            state.enabled = true;
            self.camera_media_ui_states.insert(camera_index, state);
            self.mark_dirty();
        }
    }

    /// Momentary solo: save all camera states, enable only the target camera at full visibility
    pub fn solo_camera(&mut self, camera_index: u32) {
        // Save current state of all cameras
        let saved: Vec<(u32, u8, bool)> = self.camera_media_ui_states.iter()
            .map(|(idx, s)| (*idx, s.vis_q, s.enabled))
            .collect();
        self.camera_solo_saved = Some(saved);

        // Set all cameras to zero, then solo the target
        for (idx, cam_state) in &mut self.camera_media_ui_states {
            if *idx == camera_index {
                cam_state.enabled = true;
                cam_state.set_visibility_immediate(16);
            } else {
                cam_state.set_visibility_immediate(0);
            }
        }
        self.mark_dirty();
    }

    /// Restore camera states from before solo
    pub fn unsolo_camera(&mut self) {
        if let Some(saved) = self.camera_solo_saved.take() {
            for (idx, vis_q, enabled) in saved {
                if let Some(cam_state) = self.camera_media_ui_states.get_mut(&idx) {
                    cam_state.vis_q = vis_q;
                    cam_state.enabled = enabled;
                }
            }
            self.mark_dirty();
        }
    }

    // === Harmonic Field toggle methods (for MIDI control) ===

    /// Toggle Chroma key overlay
    pub fn toggle_chroma_key(&mut self) {
        self.show_chroma_key = !self.show_chroma_key;
        self.mark_dirty();
    }

    /// Toggle Math overlay
    pub fn toggle_math_overlay(&mut self) {
        self.show_math_overlay = !self.show_math_overlay;
        self.mark_dirty();
    }

    /// Toggle Performance HUD
    pub fn toggle_perf_hud(&mut self) {
        self.show_perf_hud = !self.show_perf_hud;
        self.mark_dirty();
    }

    /// Toggle Debug HUD
    pub fn toggle_debug_hud(&mut self) {
        self.show_debug_hud = !self.show_debug_hud;
        self.mark_dirty();
    }

    /// Toggle QS Wheel display
    pub fn toggle_qs_wheel(&mut self) {
        self.show_qs_wheel = !self.show_qs_wheel;
        self.mark_dirty();
    }

    /// Update camera tracking state per-frame (call from app update loop)
    pub fn update_camera_tracking(&mut self, dt: f32, confidences: &std::collections::HashMap<u32, f32>) {
        for (idx, cam_state) in &mut self.camera_ui_states {
            let tracker_conf = confidences.get(idx).copied();
            cam_state.update(dt, tracker_conf);
        }
    }

    /// Update camera media smoothing per-frame (call from app update loop)
    /// Returns list of (camera_index, smoothed_alpha) for render pipeline
    pub fn update_camera_media_smoothing(&mut self, dt: f32) -> Vec<(u32, f32)> {
        let mut result = Vec::new();
        for (idx, cam_state) in &mut self.camera_media_ui_states {
            cam_state.update_smoothing(dt);
            result.push((*idx, cam_state.vis_smoothed));
        }
        result
    }

    /// Get camera media states for render pipeline
    /// Returns list of (camera_index, should_upload, smoothed_alpha)
    pub fn get_camera_media_states(&self) -> &HashMap<u32, crate::camera::CameraMediaState> {
        &self.camera_media_ui_states
    }

    /// Get aggregated camera influences for render system
    /// Returns: Vec of (camera_id, vis_smoothed, enabled)
    pub fn get_camera_influences(&self) -> Vec<(u32, f32, bool)> {
        self.camera_ui_states
            .iter()
            .map(|(idx, state)| (*idx, state.vis_smoothed, state.enabled))
            .collect()
    }

    /// Get the current render params
    pub fn params(&self) -> &RenderParams {
        &self.params
    }

    /// Get mutable reference to render params (e.g. for MIDI control)
    pub fn params_mut(&mut self) -> &mut RenderParams {
        &mut self.params
    }

    /// Refresh favorites from database
    fn refresh_favorites(&mut self) {
        if let Some(ref db) = self.db {
            self.favorites = db.get_favorites();
            self.pinned_presets = db.get_pinned_presets();
            self.favorites_dirty = false;
            log::info!("[UI] Loaded {} favorites, {} pinned", self.favorites.len(), self.pinned_presets.len());
        }
    }

    /// Refresh preset metadata cache from database
    fn refresh_preset_metadata(&mut self) {
        if let Some(ref db) = self.db {
            self.preset_metadata.clear();
            for meta in db.get_all_preset_metadata() {
                self.preset_metadata.insert(meta.file_path.clone(), meta);
            }
            log::info!("[UI] Loaded {} preset metadata entries", self.preset_metadata.len());
        }
    }

    /// Refresh video metadata cache from database
    fn refresh_video_metadata(&mut self) {
        if let Some(ref db) = self.db {
            self.video_metadata.clear();
            for meta in db.get_all_video_metadata() {
                self.video_metadata.insert(meta.file_path.clone(), meta);
            }
            log::info!("[UI] Loaded {} video metadata entries", self.video_metadata.len());
        }
    }

    /// Get custom display name for a video if set
    fn get_video_display_name(&self, file_path: &str, original_name: &str) -> String {
        if let Some(meta) = self.video_metadata.get(file_path) {
            if let Some(ref custom) = meta.custom_name {
                return custom.clone();
            }
        }
        original_name.to_string()
    }

    /// Check if a video is hidden
    fn is_video_hidden(&self, file_path: &str) -> bool {
        self.video_metadata.get(file_path).map(|m| m.hidden).unwrap_or(false)
    }

    /// Get custom display name for a preset if set
    fn get_preset_display_name(&self, file_path: &str, original_name: &str) -> String {
        if let Some(meta) = self.preset_metadata.get(file_path) {
            if let Some(ref custom) = meta.custom_name {
                return custom.clone();
            }
        }
        original_name.to_string()
    }

    /// Check if a preset is hidden
    fn is_preset_hidden(&self, file_path: &str) -> bool {
        self.preset_metadata.get(file_path).map(|m| m.hidden).unwrap_or(false)
    }

    /// Check if a preset should be shown (respects show_hidden_presets toggle)
    fn should_show_preset(&self, pipeline: &RenderPipeline, cat: &str, preset: &str) -> bool {
        if self.show_hidden_presets {
            return true;
        }
        if let Some(fp) = pipeline.get_preset_path(cat, preset) {
            !self.is_preset_hidden(&fp)
        } else {
            true // Show if we can't find path
        }
    }

    /// Check if a preset is a favorite
    fn is_favorite(&self, category: &str, preset: &str) -> bool {
        self.favorites.iter().any(|(c, p)| c == category && p == preset)
    }

    /// Toggle favorite status for a preset
    fn toggle_favorite(&mut self, category: &str, preset: &str) {
        if let Some(ref db) = self.db {
            if self.is_favorite(category, preset) {
                if let Err(e) = db.remove_favorite(category, preset) {
                    log::error!("[UI] Failed to remove favorite: {}", e);
                }
            } else {
                if let Err(e) = db.add_favorite(category, preset) {
                    log::error!("[UI] Failed to add favorite: {}", e);
                }
            }
            self.favorites_dirty = true;
            self.midi_browse_dirty = true;
        }
    }
}

impl ControlPanel {
    /// Get current render parameters (applies smooth easing for transitions)
    pub fn get_params(&mut self) -> RenderParams {
        // Check for debounced settings save
        self.check_save();

        // Smooth exponential ease-out interpolation
        // This gives a natural "slow down as you approach target" feel
        let ease_speed = 0.03;  // Very slow for obvious visible easing (~1.5 sec to settle)

        let diff = self.video_scale_target - self.params.video_scale;

        // Only animate if there's meaningful difference
        if diff.abs() > 0.001 {
            // Exponential ease-out: move a percentage of remaining distance each frame
            self.params.video_scale += diff * ease_speed;

            // Clamp to valid range
            self.params.video_scale = self.params.video_scale.clamp(0.05, 4.0);

            // Snap to target when very close (prevents endless tiny movements)
            if diff.abs() < 0.005 {
                self.params.video_scale = self.video_scale_target;
            }
        }

        self.params.clone()
    }

    /// Check if animation is in progress
    pub fn is_animating(&self) -> bool {
        let diff = self.video_scale_target - self.params.video_scale;
        diff.abs() > 0.001
    }

    /// Check if control panel has a video loaded
    pub fn has_video(&self) -> bool {
        self.video_decoder.is_loaded()
    }

    /// Check if the video decoder is currently playing
    pub fn is_video_playing(&self) -> bool {
        self.video_decoder.is_playing()
    }

    /// Pause video playback (for silence gate)
    pub fn pause_video(&mut self) {
        self.video_decoder.pause();
    }

    /// Resume video playback (for silence gate)
    pub fn resume_video(&mut self) {
        self.video_decoder.play();
    }

    /// Update video playback (decode frames and upload to pipeline)
    /// This should be called every frame, regardless of whether the UI is visible
    pub fn update_video(&mut self, pipeline: Arc<Mutex<Option<RenderPipeline>>>) {
        if self.video_decoder.is_playing() {
            if let Some(ref mut p) = *pipeline.lock() {
                // Try zero-copy path first (macOS IOSurface)
                #[cfg(target_os = "macos")]
                {
                    if self.video_decoder.is_zero_copy() {
                        if let Some(pixel_buffer) = self.video_decoder.get_frame_zero_copy() {
                            // Zero-copy: create GPU texture directly from CVPixelBuffer
                            if p.set_video_pixel_buffer(&pixel_buffer) {
                                return; // Success - no fallback needed
                            }
                        }
                    }
                }

                // Fallback: copy frame data via PBO
                if let Some((frame_data, width, height)) = self.video_decoder.get_frame() {
                    p.set_video_frame(frame_data, width, height);
                }
            }
        }
    }

    /// Show the control panel UI with Arc<Mutex<>> pipeline (legacy method)
    pub fn show_with_pipeline(&mut self, ui: &mut egui::Ui, pipeline: Arc<Mutex<Option<RenderPipeline>>>) {
        let mut pipe_guard = pipeline.lock();
        self.show(ui, &mut *pipe_guard);
    }

    /// Show the control panel UI with audio controls
    pub fn show_with_audio(
        &mut self,
        ui: &mut egui::Ui,
        pipeline: Arc<Mutex<Option<RenderPipeline>>>,
        audio: Arc<Mutex<AudioCapture>>,
        audio_enabled: bool,
    ) -> AudioAction {
        self.show_with_audio_analysis(ui, pipeline, audio, audio_enabled, &AudioAnalysis::default())
    }

    /// Show the control panel UI with audio controls and analysis
    pub fn show_with_audio_analysis(
        &mut self,
        ui: &mut egui::Ui,
        pipeline: Arc<Mutex<Option<RenderPipeline>>>,
        audio: Arc<Mutex<AudioCapture>>,
        audio_enabled: bool,
        analysis: &AudioAnalysis,
    ) -> AudioAction {
        let mut pipe_guard = pipeline.lock();
        let audio_guard = audio.lock();
        let current_source = audio_guard.get_source().clone();
        drop(audio_guard);

        self.show_full_with_analysis(ui, &mut *pipe_guard, Some((current_source, audio_enabled)), analysis, None, Vec::new(), None, None, None, None, None, None, None, None).0
    }

    /// Push the lighting-clock status in before `show()` — same pattern as
    /// `meter_values`. Only the clock UI reads it.
    pub fn set_clock_status(
        &mut self,
        link_bpm: Option<f32>,
        derived_bpm: f32,
        derived_confidence: f32,
    ) {
        self.clock_status = ClockStatus { link_bpm, derived_bpm, derived_confidence };
    }

    /// The operator's lighting-clock source choice, for the app to feed into
    /// `ClockInputs.mode`.
    pub fn clock_override(&self) -> crate::emerge::ClockOverride {
        self.clock_override
    }

    /// Show the control panel UI with audio and output controls
    pub fn show_with_all(
        &mut self,
        ui: &mut egui::Ui,
        pipeline: Arc<Mutex<Option<RenderPipeline>>>,
        audio: Arc<Mutex<AudioCapture>>,
        audio_enabled: bool,
        analysis: &AudioAnalysis,
        output_state: OutputState,
        cdj_devices: Vec<CdjDeviceInfo>,
        midi_state: Option<crate::app::MidiState>,
        osc_state: Option<OscState>,
        hue_state: Option<crate::hue::HueState>,
        wled_state: Option<crate::wled::WledState>,
        camera_state: Option<CameraState>,
        auth: Option<&AuthManager>,
        youtube_state: Option<YouTubeState<'_>>,
        stage_ui_state: Option<&crate::app::StageUiState>,
    ) -> (AudioAction, OutputAction, MidiAction, OscAction, HueAction, WledAction, CameraAction, AuthAction, YouTubeAction, TwitchAction, KickAction, DualStreamAction) {
        let mut pipe_guard = pipeline.lock();
        let audio_guard = audio.lock();
        let current_source = audio_guard.get_source().clone();
        drop(audio_guard);

        self.show_full_with_analysis(ui, &mut *pipe_guard, Some((current_source, audio_enabled)), analysis, Some(output_state), cdj_devices, midi_state, osc_state, hue_state, wled_state, camera_state, auth, youtube_state, stage_ui_state)
    }

    /// Show the control panel UI
    pub fn show(&mut self, ui: &mut egui::Ui, pipeline: &mut Option<RenderPipeline>) {
        self.show_full(ui, pipeline, None);
    }

    /// Full show method with optional audio state
    fn show_full(
        &mut self,
        ui: &mut egui::Ui,
        pipeline: &mut Option<RenderPipeline>,
        audio_state: Option<(AudioSource, bool)>,
    ) -> AudioAction {
        self.show_full_with_analysis(ui, pipeline, audio_state, &AudioAnalysis::default(), None, Vec::new(), None, None, None, None, None, None, None, None).0
    }

    /// Full show method with optional audio state and analysis
    fn show_full_with_analysis(
        &mut self,
        ui: &mut egui::Ui,
        pipeline: &mut Option<RenderPipeline>,
        audio_state: Option<(AudioSource, bool)>,
        analysis: &AudioAnalysis,
        output_state: Option<OutputState>,
        cdj_devices: Vec<CdjDeviceInfo>,
        midi_state: Option<crate::app::MidiState>,
        osc_state: Option<OscState>,
        hue_state: Option<crate::hue::HueState>,
        wled_state: Option<crate::wled::WledState>,
        camera_state: Option<CameraState>,
        auth: Option<&AuthManager>,
        // Retained for the API/return tuple; the YouTube panel section is commented
        // out (hidden from the menu), so this is currently unread.
        _youtube_state: Option<YouTubeState<'_>>,
        stage_ui_state: Option<&crate::app::StageUiState>,
    ) -> (AudioAction, OutputAction, MidiAction, OscAction, HueAction, WledAction, CameraAction, AuthAction, YouTubeAction, TwitchAction, KickAction, DualStreamAction) {
        let mut audio_action = AudioAction::None;
        let mut output_action = OutputAction::None;
        let mut midi_action = MidiAction::None;
        let mut osc_action = OscAction::None;
        let mut hue_action = HueAction::None;
        let mut wled_action = WledAction::None;
        let mut camera_action = CameraAction::None;
        let mut auth_action = AuthAction::None;
        // Always None while the YouTube menu section is commented out (below).
        let youtube_action = YouTubeAction::None;
        let mut twitch_action = TwitchAction::None;
        let mut kick_action = KickAction::None;
        let dual_stream_action = DualStreamAction::None;

        // Update MIDI learn state for controls to detect
        self.midi_learn_armed = midi_state.as_ref()
            .map(|s| s.is_waiting_for_target())
            .unwrap_or(false);
        self.pending_midi_action = MidiAction::None;

        ui.vertical(|ui| {
            // Style the panel
            ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 8.0);

            // Audio section (with source selection and analysis) - at top
            if let Some((current_source, audio_enabled)) = audio_state {
                audio_action = self.show_audio_section_with_analysis(ui, current_source, audio_enabled, analysis);
            } else {
                self.show_audio_section(ui);
            }

            ui.add_space(8.0);

            // Performance section (quality settings, effects toggles) - just below Audio
            self.show_performance_section(ui);

            ui.add_space(8.0);

            // Harmonic Field section - always visible (chroma analysis useful for all modes)
            self.show_harmonic_geometry_section(ui, pipeline);

            ui.add_space(8.0);

            // projectM section (Themes browser)
            self.show_projectm_section(ui, pipeline);

            // Mythos section - right below Themes, only when a Mythos preset is active
            if self.render_mode == RenderMode::Mythos {
                ui.add_space(8.0);
                self.show_mythos_section(ui, pipeline);
            }

            // Video section (Compels)
            self.show_video_section(ui, pipeline);

            // Cameras section (background media layers)
            ui.add_space(8.0);
            let cameras_action = self.show_cameras_section(ui, camera_state.as_ref());
            if cameras_action != CameraAction::None {
                camera_action = cameras_action;
            }

            // Output section (NDI, Output Window)
            if let Some(output) = output_state.clone() {
                ui.add_space(8.0);
                output_action = self.show_output_section(ui, output);
            }

            // Recording section
            if let Some(output) = output_state {
                ui.add_space(8.0);
                let is_authenticated = auth.map(|a| a.is_authenticated()).unwrap_or(false);
                let recording_action = self.show_recording_section(ui, output, is_authenticated);
                if output_action == OutputAction::None {
                    output_action = recording_action;
                }
            }

            // Ableton Link section (LAN tempo-sync status)
            ui.add_space(8.0);
            self.show_ableton_link_section(ui);

            // Pro DJ Link section (CDJ status)
            ui.add_space(8.0);
            self.show_prodjlink_section(ui, &cdj_devices);

            // MIDI controller section
            if let Some(ref midi) = midi_state {
                ui.add_space(8.0);
                midi_action = self.show_midi_section(ui, midi);
            }

            // OSC / Lighting section (includes Hue + WLED)
            ui.add_space(8.0);
            let (lighting_osc, lighting_hue, lighting_wled) = self.show_osc_section(ui, osc_state.as_ref(), hue_state.as_ref(), wled_state.as_ref(), auth.map(|a| a.is_authenticated()).unwrap_or(false), stage_ui_state);
            osc_action = lighting_osc;
            hue_action = lighting_hue;
            wled_action = lighting_wled;

            // Vision section (camera tracking influence)
            ui.add_space(8.0);
            let vision_action = self.show_vision_section(ui, camera_state.as_ref());
            if vision_action != CameraAction::None {
                camera_action = vision_action;
            }

            // Drives the Twitch/Kick sections below.
            let is_authenticated = auth.map(|a| a.is_authenticated()).unwrap_or(false);

            // YouTube Live section — hidden from the menu per request. The whole
            // YouTube plumbing/state stays intact; only the panel section is
            // commented out (so youtube_action stays YouTubeAction::None). Uncomment
            // this block to bring the menu entry back.
            // ui.add_space(8.0);
            // let has_youtube_entitlement = auth
            //     .map(|a| a.get_entitlements().can_stream_youtube())
            //     .unwrap_or(false);
            // youtube_action = self.show_youtube_section(
            //     ui,
            //     youtube_state.as_ref().map(|s| s.auth),
            //     youtube_state.as_ref().map(|s| s.eligibility),
            //     youtube_state.as_ref().and_then(|s| s.session),
            //     is_authenticated,
            //     has_youtube_entitlement,
            // );

            // Twitch Streaming section (independent from YouTube)
            ui.add_space(8.0);
            let is_twitch_streaming = matches!(self.twitch_state, TwitchStreamState::Streaming);
            twitch_action = self.show_twitch_section(
                ui,
                is_twitch_streaming,
                is_authenticated,
            );

            // Kick Streaming section (independent from YouTube/Twitch)
            ui.add_space(8.0);
            let is_kick_streaming = matches!(self.kick_state, TwitchStreamState::Streaming);
            kick_action = self.show_kick_section(
                ui,
                is_kick_streaming,
                is_authenticated,
            );

            // Logo / overlay watermark — 2nd from the bottom of the menu.
            ui.add_space(8.0);
            self.show_overlay_section(ui);

            // Account section (Compeller.ai login) — very bottom of the menu.
            if let Some(auth_mgr) = auth {
                ui.add_space(8.0);
                auth_action = self.show_account_section(ui, auth_mgr);
            }
        });

        // Collect any MIDI action from control clicks during learn mode
        if self.pending_midi_action != MidiAction::None {
            midi_action = self.pending_midi_action.clone();
        }

        // Collect pending gain action from YouTube/Twitch/Kick sub-sections
        if self.pending_gain_action != OutputAction::None {
            output_action = std::mem::replace(&mut self.pending_gain_action, OutputAction::None);
        }

        (audio_action, output_action, midi_action, osc_action, hue_action, wled_action, camera_action, auth_action, youtube_action, twitch_action, kick_action, dual_stream_action)
    }

    /// Show a "login required" message for features that require a Compeller account
    fn show_login_required_message(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("🔒")
                .size(24.0)
                .color(egui::Color32::from_rgb(150, 150, 150)));
            ui.add_space(4.0);
            // Clicking jumps to the Account section at the bottom of the
            // menu (force-opens it and scrolls there).
            if ui
                .link(
                    egui::RichText::new("Sign in to Compeller")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(120, 180, 255)),
                )
                .clicked()
            {
                self.jump_to_account = true;
            }
            ui.label(egui::RichText::new("to unlock this feature")
                .size(11.0)
                .color(egui::Color32::from_rgb(120, 120, 120)));
            ui.add_space(4.0);
            // Account creation lives on the web; sign-in is in the Account
            // section below.
            ui.hyperlink_to(
                egui::RichText::new("No account? Create one at compeller.ai →").size(11.0),
                "https://compeller.ai",
            );
            ui.add_space(8.0);
        });
    }

    /// Helper to make a control MIDI-learnable
    /// Returns true if the control was clicked in learn mode (and sets pending action)
    fn midi_learnable_control(&mut self, ui: &mut egui::Ui, target: ControlTarget, label: &str) -> bool {
        if self.midi_learn_armed {
            // Show learnable indicator with pulsing highlight
            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
            let highlight_color = egui::Color32::from_rgba_unmultiplied(
                255, 200, 50, (100.0 + 80.0 * pulse as f32) as u8
            );

            let response = ui.horizontal(|ui| {
                // Pulsing dot indicator
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, highlight_color);
                ui.label(egui::RichText::new(label).color(highlight_color))
            }).response;

            if response.interact(egui::Sense::click()).clicked() {
                self.pending_midi_action = MidiAction::SetLearnTarget(target);
                return true;
            }
        }
        false
    }

    /// Show a slider that's MIDI-learnable
    fn midi_slider(&mut self, ui: &mut egui::Ui, target: ControlTarget, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            // Label (clickable in learn mode)
            if self.midi_learn_armed {
                let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);

                let label_response = ui.label(egui::RichText::new(label).color(highlight));
                if label_response.interact(egui::Sense::click()).clicked() {
                    self.pending_midi_action = MidiAction::SetLearnTarget(target);
                }
            } else {
                ui.label(label);
            }

            // Slider
            let old_val = *value;
            ui.add(egui::Slider::new(value, range).show_value(false));
            changed = (*value - old_val).abs() > 0.001;
        });

        changed
    }

    /// Show the Ableton Link section (LAN tempo-sync status).
    fn show_ableton_link_section(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Ableton Link").strong().size(14.0),
        )
        .default_open(self.ableton_link_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);
            match self.clock_status.link_bpm {
                Some(bpm) => {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("●").color(egui::Color32::from_rgb(80, 220, 80)),
                        );
                        ui.label(
                            egui::RichText::new("Receiving")
                                .strong()
                                .color(egui::Color32::from_rgb(255, 255, 255)),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!("{:.1}", bpm))
                                        .color(egui::Color32::from_rgb(170, 170, 170)),
                                );
                            },
                        );
                    });
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("●").color(egui::Color32::from_rgb(110, 110, 110)),
                        );
                        ui.label(
                            egui::RichText::new("No Link session on the network").weak(),
                        );
                    });
                }
            }
            ui.add_space(4.0);
        });
        self.ableton_link_expanded = header.fully_open();
    }

    /// Show the Pro DJ Link section (CDJ device status)
    fn show_prodjlink_section(&mut self, ui: &mut egui::Ui, cdj_devices: &[CdjDeviceInfo]) {
        // Update master pulse animation (tempo-synced if BPM available)
        let dt = 1.0 / 60.0;
        if let Some(bpm) = self.master_bpm {
            let beats_per_sec = bpm / 60.0;
            let phase_increment = beats_per_sec * dt;  // Once per beat
            self.master_pulse_phase = (self.master_pulse_phase + phase_increment) % 1.0;
        } else {
            self.master_pulse_phase = (self.master_pulse_phase + 0.5 * dt) % 1.0;
        }

        // Update master BPM from devices
        self.master_bpm = cdj_devices.iter()
            .find(|d| d.is_master && d.is_playing)
            .and_then(|d| d.bpm);

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Pro DJ Link").strong().size(14.0)
        )
        .default_open(self.prodjlink_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            if cdj_devices.is_empty() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Scanning for CDJs...").weak());
                });
            } else {
                // Show each CDJ device
                for device in cdj_devices {
                    let is_master_playing = device.is_master && device.is_playing;
                    let is_active = device.is_master || device.is_playing;

                    // Track row positions for highlight
                    let row_start_y = ui.cursor().min.y;
                    let row_start_x = ui.cursor().min.x;

                    // Row 1: Two-column layout - [Content | BPM]
                    ui.horizontal(|ui| {
                        // Left column: deck info + icons (content that gets highlighted)
                        ui.add_space(8.0);  // Indent for accent bar

                        // Deck label
                        let deck_label = format!("Deck {} • {}", device.device_id, device.name);
                        let label_color = if is_active {
                            egui::Color32::from_rgb(255, 255, 255)
                        } else {
                            egui::Color32::from_rgb(195, 195, 195)
                        };
                        ui.label(egui::RichText::new(&deck_label).strong().color(label_color));

                        // Status icons
                        if device.is_master {
                            let master_color = if is_master_playing {
                                let pulse = (self.master_pulse_phase * std::f32::consts::TAU).sin();
                                let brightness = 200 + ((pulse + 1.0) / 2.0 * 55.0) as u8;
                                egui::Color32::from_rgb(255, brightness, 50)
                            } else {
                                egui::Color32::from_rgb(200, 160, 50)
                            };
                            ui.label(egui::RichText::new("M").color(master_color).strong());
                        }
                        if device.is_playing {
                            ui.label(egui::RichText::new("▶").color(egui::Color32::from_rgb(80, 220, 80)));
                        } else if device.track_loaded {
                            ui.label(egui::RichText::new("■").color(egui::Color32::from_rgb(200, 140, 80)));
                        }
                        if device.is_looping {
                            ui.label(egui::RichText::new("⟳").color(egui::Color32::from_rgb(100, 140, 255)));
                        }
                        if device.is_on_air {
                            ui.label(egui::RichText::new("●").color(egui::Color32::from_rgb(220, 60, 60)));
                        }

                        // Right column: BPM (right-aligned, outside highlight)
                        if let Some(bpm) = device.bpm {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let bpm_color = if is_active {
                                    egui::Color32::from_rgb(170, 170, 170)
                                } else {
                                    egui::Color32::from_rgb(140, 140, 140)
                                };
                                ui.label(egui::RichText::new(format!("{:.1}", bpm)).color(bpm_color));
                            });
                        }
                    });

                    let row1_end_y = ui.cursor().min.y;

                    // Row 2: Title — Artist (single wrapping label)
                    let row2_end_y = if device.track_loaded {
                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.add_space(8.0);

                            if let Some(title) = &device.track_title {
                                // Combine title and artist into one string for proper wrapping
                                let display_text = if let Some(artist) = &device.track_artist {
                                    format!("{} — {}", title, artist)
                                } else {
                                    title.clone()
                                };

                                let text_color = if is_active {
                                    egui::Color32::from_rgb(220, 220, 220)
                                } else {
                                    egui::Color32::from_rgb(180, 180, 180)
                                };

                                // Single label with text wrap enabled
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&display_text).color(text_color).size(12.0)
                                    ).wrap()
                                );
                            } else {
                                ui.label(egui::RichText::new("Loading...").color(egui::Color32::from_rgb(120, 120, 120)).italics().size(11.0));
                            }
                        });
                        ui.cursor().min.y
                    } else {
                        row1_end_y
                    };

                    // Draw highlight for master deck (drawn AFTER layout, doesn't affect sizing)
                    if is_master_playing {
                        let total_height = row2_end_y - row_start_y;
                        let content_width = ui.available_width();  // Use available width, not expand
                        let pulse = (self.master_pulse_phase * std::f32::consts::TAU).sin();

                        // Subtle red background (doesn't affect layout - painted behind)
                        let bg_alpha = 0.12 + 0.06 * (pulse + 1.0) / 2.0;  // 12-18%
                        let bg_rect = egui::Rect::from_min_size(
                            egui::pos2(row_start_x + 6.0, row_start_y),
                            egui::vec2(content_width - 12.0, total_height),
                        );
                        let bg_color = egui::Color32::from_rgba_unmultiplied(
                            180, 40, 40, (bg_alpha * 255.0) as u8
                        );
                        ui.painter().rect_filled(bg_rect, egui::Rounding::same(4.0), bg_color);

                        // Left accent bar: 4px wide, 4px top/bottom inset, rounded ends
                        let inset = 4.0;
                        let bar_alpha = 0.85 + 0.15 * (pulse + 1.0) / 2.0;
                        let accent_rect = egui::Rect::from_min_size(
                            egui::pos2(row_start_x + 2.0, row_start_y + inset),
                            egui::vec2(4.0, total_height - inset * 2.0),
                        );
                        let accent_color = egui::Color32::from_rgba_unmultiplied(
                            200, 50, 50, (bar_alpha * 255.0) as u8
                        );
                        ui.painter().rect_filled(accent_rect, egui::Rounding::same(2.0), accent_color);
                    }

                    ui.add_space(8.0);
                }
            }

            // Request continuous repaint for smooth pulse animation
            if self.master_bpm.is_some() {
                ui.ctx().request_repaint();
            }
        });

        self.prodjlink_expanded = header.fully_open();
    }

    /// Show the logo section (logo watermark settings)
    fn show_overlay_section(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Logo").strong().size(14.0)
        )
        .default_open(self.overlay_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Account required notice
            ui.label(egui::RichText::new("Account required to modify this section")
                .small()
                .color(egui::Color32::from_rgb(150, 150, 150)));
            ui.add_space(4.0);

            // Custom logo upload
            ui.horizontal(|ui| {
                if self.params.custom_logo_path.is_some() {
                    ui.label(egui::RichText::new("Custom Logo").small().color(egui::Color32::from_rgb(100, 200, 100)));
                } else {
                    ui.label(egui::RichText::new("Default Logo (REACT)").small().color(egui::Color32::GRAY));
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button(format!("{} Upload", egui_phosphor::regular::UPLOAD)).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                        .pick_file()
                    {
                        self.params.custom_logo_path = Some(path.to_string_lossy().to_string());
                        self.mark_dirty();
                        log::info!("[Logo] Custom logo set: {:?}", path);
                    }
                }
                if self.params.custom_logo_path.is_some() {
                    if ui.small_button(format!("{} Delete", egui_phosphor::regular::TRASH)).on_hover_text("Revert to default REACT logo").clicked() {
                        self.params.custom_logo_path = None;
                        self.mark_dirty();
                        log::info!("[Logo] Reverted to default logo");
                    }
                }
            });
            ui.add_space(4.0);

            // Logo enable/disable toggle (MIDI learnable)
            ui.horizontal(|ui| {
                let old_visible = self.params.logo_visible;

                // MIDI learnable checkbox
                let label = if self.midi_learn_armed {
                    let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                    let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                    egui::RichText::new("Show Logo").color(highlight)
                } else {
                    egui::RichText::new("Show Logo")
                };

                let checkbox_response = ui.checkbox(&mut self.params.logo_visible, label);
                if self.midi_learn_armed && checkbox_response.clicked() {
                    self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::LogoVisible);
                    // Restore the value since we don't want to toggle in learn mode
                    self.params.logo_visible = old_visible;
                } else if self.params.logo_visible != old_visible {
                    self.mark_dirty();
                }
            });

            // Only show other controls if logo is enabled
            if self.params.logo_visible {
                ui.add_space(4.0);

                // Opacity slider (MIDI learnable)
                ui.horizontal(|ui| {
                    let label_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        ui.label(egui::RichText::new("Opacity:").color(highlight))
                    } else {
                        ui.label("Opacity:")
                    };
                    if self.midi_learn_armed && label_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::LogoOpacity);
                    }

                    let old_opacity = self.params.logo_opacity;
                    let slider_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                        let mut style = (*ui.ctx().style()).clone();
                        style.visuals.widgets.inactive.bg_fill = yellow;
                        style.visuals.widgets.hovered.bg_fill = yellow;
                        style.visuals.widgets.active.bg_fill = yellow;
                        ui.ctx().set_style(style.clone());

                        let response = ui.add(egui::Slider::new(&mut self.params.logo_opacity, 0.1..=1.0)
                            .show_value(true))
                            .on_hover_text("Click to map Logo Opacity to MIDI fader");

                        ui.ctx().set_style(egui::Style::default());
                        response
                    } else {
                        ui.add(egui::Slider::new(&mut self.params.logo_opacity, 0.1..=1.0)
                            .show_value(true))
                    };

                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::LogoOpacity);
                        self.params.logo_opacity = old_opacity;
                    } else if (self.params.logo_opacity - old_opacity).abs() > 0.001 {
                        self.mark_dirty();
                    }
                });

                // Scale slider (MIDI learnable)
                ui.horizontal(|ui| {
                    let label_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        ui.label(egui::RichText::new("Scale:").color(highlight))
                    } else {
                        ui.label("Scale:")
                    };
                    if self.midi_learn_armed && label_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::LogoScale);
                    }

                    let old_scale = self.params.logo_scale;
                    let slider_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                        let mut style = (*ui.ctx().style()).clone();
                        style.visuals.widgets.inactive.bg_fill = yellow;
                        style.visuals.widgets.hovered.bg_fill = yellow;
                        style.visuals.widgets.active.bg_fill = yellow;
                        ui.ctx().set_style(style.clone());

                        let response = ui.add(egui::Slider::new(&mut self.params.logo_scale, 0.2..=4.0)
                            .show_value(true))
                            .on_hover_text("Click to map Logo Scale to MIDI fader");

                        ui.ctx().set_style(egui::Style::default());
                        response
                    } else {
                        ui.add(egui::Slider::new(&mut self.params.logo_scale, 0.2..=4.0)
                            .show_value(true))
                    };

                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::LogoScale);
                        self.params.logo_scale = old_scale;
                    } else if (self.params.logo_scale - old_scale).abs() > 0.001 {
                        self.mark_dirty();
                    }
                });

                // Position selector - button grid
                ui.horizontal(|ui| {
                    ui.label("Position:");
                    let old_pos = self.params.logo_position;
                    let btn_size = egui::vec2(28.0, 20.0);

                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                        // Top row
                        ui.horizontal(|ui| {
                            if ui.add_sized(btn_size, egui::SelectableLabel::new(
                                self.params.logo_position == OverlayPosition::TopLeft, "TL"
                            )).on_hover_text("Top Left").clicked() {
                                self.params.logo_position = OverlayPosition::TopLeft;
                            }
                            ui.add_space(btn_size.x);
                            if ui.add_sized(btn_size, egui::SelectableLabel::new(
                                self.params.logo_position == OverlayPosition::TopRight, "TR"
                            )).on_hover_text("Top Right").clicked() {
                                self.params.logo_position = OverlayPosition::TopRight;
                            }
                        });
                        // Middle row
                        ui.horizontal(|ui| {
                            ui.add_space(btn_size.x + 2.0);
                            if ui.add_sized(btn_size, egui::SelectableLabel::new(
                                self.params.logo_position == OverlayPosition::Center, "C"
                            )).on_hover_text("Center").clicked() {
                                self.params.logo_position = OverlayPosition::Center;
                            }
                        });
                        // Bottom row
                        ui.horizontal(|ui| {
                            if ui.add_sized(btn_size, egui::SelectableLabel::new(
                                self.params.logo_position == OverlayPosition::BottomLeft, "BL"
                            )).on_hover_text("Bottom Left").clicked() {
                                self.params.logo_position = OverlayPosition::BottomLeft;
                            }
                            ui.add_space(btn_size.x);
                            if ui.add_sized(btn_size, egui::SelectableLabel::new(
                                self.params.logo_position == OverlayPosition::BottomRight, "BR"
                            )).on_hover_text("Bottom Right").clicked() {
                                self.params.logo_position = OverlayPosition::BottomRight;
                            }
                        });
                    });

                    if self.params.logo_position != old_pos {
                        self.mark_dirty();
                    }
                });
            }

            ui.add_space(4.0);
        });

        // Track expanded state
        self.overlay_expanded = header.fully_open();
    }

    /// Show the Lighting section with OSC, Hue, and WLED subsections
    fn show_osc_section(&mut self, ui: &mut egui::Ui, osc_state: Option<&OscState>, hue_state: Option<&crate::hue::HueState>, wled_state: Option<&crate::wled::WledState>, is_authenticated: bool, stage_ui_state: Option<&crate::app::StageUiState>) -> (OscAction, HueAction, WledAction) {
        let mut action = OscAction::None;
        let mut hue_action = HueAction::None;
        let mut wled_action = WledAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Lighting").strong().size(14.0)
        )
        .default_open(self.osc_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Lighting requires a Compeller sign-in — same gate as the
            // Twitch/Kick/Recording sections: signed-out users see the
            // sign-in tile, not the controls. Enforced HERE at the configure
            // surface (and at output auto-restore on boot) — never in the
            // per-frame feed, which must keep driving already-enabled
            // outputs so a transient auth hiccup can't freeze a live rig.
            if !is_authenticated {
                self.show_login_required_message(ui);
                return;
            }

            // Fixture request — if a fixture isn't in the library, the operator
            // can request a profile via the community Discord.
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Don't see your fixture?")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 150, 150)),
                );
                ui.hyperlink_to(
                    egui::RichText::new("Request it on Discord →").size(11.0),
                    "https://discord.gg/e9QgrmDJzb",
                );
            });
            ui.add_space(6.0);

            // Clock source — which tempo drives the lighting show.
            ui.label(egui::RichText::new("Clock Source").size(12.0).strong());
            ui.add_space(2.0);
            {
                use crate::emerge::ClockOverride;
                let label_for = |m: ClockOverride| match m {
                    ClockOverride::Auto => "Auto  (Link > Pro DJ > REACT)",
                    ClockOverride::ForceLink => "Ableton Link",
                    ClockOverride::ForceProDjLink => "Pro DJ Link",
                    ClockOverride::ForceDerived => "REACT",
                };
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    egui::ComboBox::from_id_salt("lighting_clock_source")
                        .selected_text(label_for(self.clock_override))
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for m in [
                                ClockOverride::Auto,
                                ClockOverride::ForceLink,
                                ClockOverride::ForceProDjLink,
                                ClockOverride::ForceDerived,
                            ] {
                                ui.selectable_value(&mut self.clock_override, m, label_for(m));
                            }
                        });
                });
                // Which source is actually driving — mirrors select_clock's
                // priority, so a pinned-but-dead source still reads honestly.
                let link_live = self.clock_status.link_bpm.is_some();
                let prodj_live = self.master_bpm.is_some();
                let (active, bpm) = match self.clock_override {
                    ClockOverride::ForceLink if link_live => {
                        ("Ableton Link", self.clock_status.link_bpm)
                    }
                    ClockOverride::ForceProDjLink if prodj_live => {
                        ("Pro DJ Link", self.master_bpm)
                    }
                    ClockOverride::ForceDerived => {
                        ("REACT", Some(self.clock_status.derived_bpm))
                    }
                    _ if link_live => ("Ableton Link", self.clock_status.link_bpm),
                    _ if prodj_live => ("Pro DJ Link", self.master_bpm),
                    _ => ("REACT", Some(self.clock_status.derived_bpm)),
                };
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Active:").size(11.0).weak());
                    ui.label(
                        egui::RichText::new(active)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(120, 220, 120)),
                    );
                    if let Some(b) = bpm {
                        ui.label(
                            egui::RichText::new(format!("· {:.1} BPM", b))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        );
                    }
                });
            }
            ui.add_space(6.0);
            ui.separator();

            // Genre subsection
            ui.add_space(4.0);
            let genre_header = egui::CollapsingHeader::new(
                egui::RichText::new("Genre").size(12.0)
            )
            .default_open(self.osc_genre_expanded)
            .show(ui, |ui| {
                let state = match osc_state {
                    Some(s) => s,
                    None => return,
                };

                ui.add_space(4.0);

                // Category dropdown
                let categories = crate::emerge::GenreProfile::categories();
                let old_cat_index = self.osc_genre_category_index;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Category:").small());
                    let current_cat = categories.get(self.osc_genre_category_index).unwrap_or(&"Electronic");
                    egui::ComboBox::from_id_salt("genre_category")
                        .selected_text(*current_cat)
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for (i, cat) in categories.iter().enumerate() {
                                if ui.selectable_value(&mut self.osc_genre_category_index, i, *cat).changed() {
                                    // Reset subcategory when category changes
                                    self.osc_genre_subcategory_index = 0;
                                }
                            }
                        });
                });
                let category_changed = self.osc_genre_category_index != old_cat_index;

                // Subcategory dropdown
                let current_cat = categories.get(self.osc_genre_category_index).unwrap_or(&"Electronic");
                let subcategories = crate::emerge::GenreProfile::subcategories_for(current_cat);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Style:").small());
                    let current_sub = subcategories.get(self.osc_genre_subcategory_index).unwrap_or(&"Custom");
                    let old_sub_index = self.osc_genre_subcategory_index;
                    egui::ComboBox::from_id_salt("genre_subcategory")
                        .selected_text(*current_sub)
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for (i, sub) in subcategories.iter().enumerate() {
                                ui.selectable_value(&mut self.osc_genre_subcategory_index, i, *sub);
                            }
                        });

                    // Apply genre change (when category OR subcategory changes)
                    if category_changed || self.osc_genre_subcategory_index != old_sub_index {
                        let new_sub = subcategories.get(self.osc_genre_subcategory_index).unwrap_or(&"Custom");
                        let new_profile = crate::emerge::GenreProfile::from_genre(current_cat, new_sub);

                        // Update config with new genre profile
                        let mut new_config = state.config.clone();
                        new_config.genre_profile = new_profile.clone();

                        // Also update pattern/palette defaults from genre
                        new_config.pattern_still = new_profile.preferred_patterns[0];
                        new_config.pattern_low = new_profile.preferred_patterns[1];
                        new_config.pattern_mid = new_profile.preferred_patterns[2];
                        new_config.pattern_high = new_profile.preferred_patterns[3];

                        // Update mover speed from genre
                        new_config.mover_speed_min = new_profile.mover_speed_range.0;
                        new_config.mover_speed_max = new_profile.mover_speed_range.1;

                        // Update UI state to match
                        self.osc_pattern_still = new_profile.preferred_patterns[0].to_string();
                        self.osc_pattern_low = new_profile.preferred_patterns[1].to_string();
                        self.osc_pattern_mid = new_profile.preferred_patterns[2].to_string();
                        self.osc_pattern_high = new_profile.preferred_patterns[3].to_string();
                        self.osc_mover_speed_min = new_profile.mover_speed_range.0;
                        self.osc_mover_speed_max = new_profile.mover_speed_range.1;

                        action = OscAction::UpdateConfig(new_config);
                        log::info!("[OSC] Genre changed to: {} - {}", current_cat, new_sub);
                    }
                });

                ui.add_space(4.0);

                // Active profile summary
                let genre = &state.config.genre_profile;
                ui.horizontal(|ui| {
                    // Strobe indicator
                    let strobe_icon = if genre.strobe_enabled {
                        egui_phosphor::regular::LIGHTNING
                    } else {
                        egui_phosphor::regular::LIGHTNING_SLASH
                    };
                    let strobe_color = if genre.strobe_enabled {
                        egui::Color32::from_rgb(255, 200, 100)
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.label(egui::RichText::new(strobe_icon).size(12.0).color(strobe_color));
                    ui.label(egui::RichText::new(if genre.strobe_enabled { "Strobe" } else { "No Strobe" }).small().color(strobe_color));

                    ui.add_space(8.0);

                    // Blackout indicator
                    let blackout_color = if genre.blackout_on_silence {
                        egui::Color32::from_rgb(150, 150, 200)
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.label(egui::RichText::new(egui_phosphor::regular::MOON).size(12.0).color(blackout_color));
                    ui.label(egui::RichText::new(if genre.blackout_on_silence { "Blackout" } else { "No Blackout" }).small().color(blackout_color));

                    ui.add_space(8.0);

                    // Palette bias
                    let bias_color = match genre.palette_bias {
                        crate::emerge::PaletteBias::Cool => egui::Color32::from_rgb(100, 180, 255),
                        crate::emerge::PaletteBias::Warm => egui::Color32::from_rgb(255, 180, 100),
                        crate::emerge::PaletteBias::Neutral => egui::Color32::from_rgb(200, 200, 200),
                        crate::emerge::PaletteBias::Vivid => egui::Color32::from_rgb(255, 100, 200),
                    };
                    ui.label(egui::RichText::new(egui_phosphor::regular::PALETTE).size(12.0).color(bias_color));
                    ui.label(egui::RichText::new(genre.palette_bias.as_str()).small().color(bias_color));
                });

                // Zone thresholds display
                ui.horizontal(|ui| {
                    let t = &genre.zone_thresholds;
                    ui.label(egui::RichText::new(format!(
                        "Zones: {:.0}% / {:.0}% / {:.0}%",
                        t.still_max * 100.0,
                        t.low_max * 100.0,
                        t.mid_max * 100.0
                    )).small().color(egui::Color32::GRAY));
                });

                // "Use Genre for Tempo" — gate genre→tempo octave guidance, MIDI
                // learnable (click the metronome while MIDI-learn is armed). Only
                // shapes REACT's *derived* clock; external Link/Pro DJ clocks win.
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let icon_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                    } else if self.genre_tempo_guidance {
                        egui::Color32::from_rgb(160, 200, 255)
                    } else {
                        egui::Color32::GRAY
                    };
                    let icon_response = ui.label(egui::RichText::new(egui_phosphor::regular::METRONOME).size(14.0).color(icon_color));
                    if self.midi_learn_armed && icon_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::GenreTempoGuidance);
                    }
                    let mut use_genre = self.genre_tempo_guidance;
                    let cb = ui.checkbox(&mut use_genre, egui::RichText::new("Use Genre for Tempo").small())
                        .on_hover_text("Genre BPM range guides REACT's beat octave (double- vs half-time). MIDI-learnable. External CDJ/Ableton clocks ignore this.");
                    if cb.changed() {
                        self.genre_tempo_guidance = use_genre;
                    }
                    let range_label = match genre.tempo_range_bpm() {
                        Some((lo, hi)) => format!("{:.0}–{:.0}", lo, hi),
                        // Custom / DJ Set carry no honest range — guidance is a no-op.
                        None => "no range".to_string(),
                    };
                    ui.label(egui::RichText::new(range_label).small().color(egui::Color32::GRAY));
                });

                ui.add_space(4.0);
            });
            self.osc_genre_expanded = genre_header.fully_open();

            // Master dimmer + reactivity — both apply to every lighting output
            // (OSC, Hue, WLED, DMX/Art-Net), so they sit at the top of the
            // Lighting section rather than inside any one adapter.
            if let Some(state) = osc_state {
                // Global Brightness — master dimmer (MIDI learnable).
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let icon_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                    } else {
                        egui::Color32::from_rgb(255, 220, 100)
                    };
                    let label_response = ui.label(egui::RichText::new(egui_phosphor::regular::SUN).size(14.0).color(icon_color));
                    if self.midi_learn_armed && label_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::GlobalBrightness);
                    }
                    let text_color = if self.midi_learn_armed { icon_color } else { egui::Color32::LIGHT_GRAY };
                    let text_response = ui.label(egui::RichText::new("Global Brightness").small().color(text_color));
                    if self.midi_learn_armed && text_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::GlobalBrightness);
                    }
                    let mut global_brightness = state.config.global_brightness;
                    let old_val = global_brightness;
                    let slider_response = ui.add(
                        egui::Slider::new(&mut global_brightness, 0.0..=1.0)
                            .show_value(false)
                    );
                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::GlobalBrightness);
                    } else if (global_brightness - old_val).abs() > 0.001 {
                        let mut new_config = state.config.clone();
                        new_config.global_brightness = global_brightness;
                        action = OscAction::UpdateConfig(new_config);
                    }
                    ui.label(egui::RichText::new(format!("{:.0}%", global_brightness * 100.0)).small().color(egui::Color32::GRAY));
                });
                // UV Level — operator UV accent master (MIDI learnable). Scales
                // the GigBAR UV layer (par ch4/9 + pod ch22) on the DMX path.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let icon_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                    } else {
                        egui::Color32::from_rgb(170, 120, 255)
                    };
                    let label_response = ui.label(egui::RichText::new(egui_phosphor::regular::MOON).size(14.0).color(icon_color));
                    if self.midi_learn_armed && label_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::UvLevel);
                    }
                    let text_color = if self.midi_learn_armed { icon_color } else { egui::Color32::LIGHT_GRAY };
                    let text_response = ui.label(egui::RichText::new("UV Level").small().color(text_color));
                    if self.midi_learn_armed && text_response.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::UvLevel);
                    }
                    let mut uv_level = state.config.uv_level;
                    let old_val = uv_level;
                    let slider_response = ui.add(
                        egui::Slider::new(&mut uv_level, 0.0..=1.0)
                            .show_value(false)
                    );
                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::UvLevel);
                    } else if (uv_level - old_val).abs() > 0.001 {
                        let mut new_config = state.config.clone();
                        new_config.uv_level = uv_level;
                        action = OscAction::UpdateConfig(new_config);
                    }
                    ui.label(egui::RichText::new(format!("{:.0}%", uv_level * 100.0)).small().color(egui::Color32::GRAY));
                });
                // Reactivity — brightness floor, audio drive gain, macro sensitivity.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(egui_phosphor::regular::GAUGE).size(14.0).color(egui::Color32::from_rgb(255, 180, 80)));
                    ui.label(egui::RichText::new("Reactivity").small());
                    let mut reactivity = state.config.reactivity;
                    if ui.add(
                        egui::Slider::new(&mut reactivity, 0.0..=1.0)
                            .show_value(false)
                    ).changed() {
                        let mut new_config = state.config.clone();
                        new_config.reactivity = reactivity;
                        action = OscAction::UpdateConfig(new_config);
                    }
                    ui.label(egui::RichText::new(format!("{:.0}%", reactivity * 100.0)).small().color(egui::Color32::GRAY));
                });
            }

            // Silence hold — seconds of continuous silence before the show fades
            // out. Tune tight for hard stops, longer to ride out quiet breakdowns.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(egui_phosphor::regular::HOURGLASS_MEDIUM).size(14.0).color(egui::Color32::from_rgb(255, 180, 80)));
                ui.label(egui::RichText::new("Silence hold").small());
                let changed = ui
                    .add(
                        egui::Slider::new(&mut self.silence_threshold, 0.5..=3.0)
                            .step_by(0.1)
                            .show_value(false),
                    )
                    .on_hover_text("Continuous silence before the show fades to black.")
                    .changed();
                if changed {
                    if let Some(ref db) = self.db {
                        let _ = db.set_float("silence_threshold", self.silence_threshold);
                    }
                }
                ui.label(egui::RichText::new(format!("{:.1}s", self.silence_threshold)).small().color(egui::Color32::GRAY));
            });

            // COM-499 Phase B4: operator LFO bank editor. Edit a Copy then assign
            // back + emit SetLfoBank on change (the OSC thread phase-preserves the
            // running phase across the swap, so a live tweak doesn't click). 8
            // routable, tempo-syncable LFO slots over brightness/reactivity/colour.
            ui.add_space(8.0);
            {
                let mut bank = self.lfo_bank;
                let before = bank;
                egui::CollapsingHeader::new(egui::RichText::new("LFO Bank").small())
                    .show(ui, |ui| {
                        const DIV_LABELS: [&str; 8] =
                            ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];
                        for (i, slot) in bank.slots.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut slot.enabled, format!("{}", i + 1));
                                egui::ComboBox::from_id_salt(format!("lfo_wave_{i}"))
                                    .width(64.0)
                                    .selected_text(format!("{:?}", slot.waveform))
                                    .show_ui(ui, |ui| {
                                        for w in [
                                            LfoWaveform::Sine,
                                            LfoWaveform::Triangle,
                                            LfoWaveform::Ramp,
                                            LfoWaveform::Saw,
                                            LfoWaveform::Square,
                                        ] {
                                            ui.selectable_value(&mut slot.waveform, w, format!("{:?}", w));
                                        }
                                    });
                                egui::ComboBox::from_id_salt(format!("lfo_target_{i}"))
                                    .width(88.0)
                                    .selected_text(format!("{:?}", slot.target))
                                    .show_ui(ui, |ui| {
                                        for t in [
                                            LfoTarget::None,
                                            LfoTarget::Brightness,
                                            LfoTarget::Reactivity,
                                            LfoTarget::Saturation,
                                            LfoTarget::HueShift,
                                        ] {
                                            ui.selectable_value(&mut slot.target, t, format!("{:?}", t));
                                        }
                                    });
                                ui.add(egui::Slider::new(&mut slot.amplitude, 0.0..=1.0).show_value(false))
                                    .on_hover_text("Depth");
                                ui.checkbox(&mut slot.tempo_sync, "sync");
                                if slot.tempo_sync {
                                    egui::ComboBox::from_id_salt(format!("lfo_div_{i}"))
                                        .width(48.0)
                                        .selected_text(
                                            DIV_LABELS.get(slot.division as usize).copied().unwrap_or("1/4"),
                                        )
                                        .show_ui(ui, |ui| {
                                            for (idx, lbl) in DIV_LABELS.iter().enumerate() {
                                                ui.selectable_value(&mut slot.division, idx as u8, *lbl);
                                            }
                                        });
                                } else {
                                    ui.add(
                                        egui::Slider::new(&mut slot.rate_hz, 0.01..=20.0)
                                            .logarithmic(true)
                                            .show_value(false),
                                    )
                                    .on_hover_text("Rate (Hz)");
                                }
                                let mut deg = slot.phase_offset * 360.0;
                                if ui
                                    .add(egui::Slider::new(&mut deg, 0.0..=360.0).show_value(false))
                                    .on_hover_text("Phase offset (deg)")
                                    .changed()
                                {
                                    slot.phase_offset = (deg / 360.0).rem_euclid(1.0);
                                }
                            });
                        }
                    });
                if bank != before {
                    self.lfo_bank = bank;
                    action = OscAction::SetLfoBank(bank);
                }
            }

            // Global manual lighting triggers. These feed the shared lighting intent
            // engine, so downstream adapters (OSC, Hue, WLED, DMX/Art-Net) can all
            // react from the same CueSnapshot.
            if let Some(state) = osc_state {
                ui.add_space(8.0);
                self.show_lighting_manual_triggers(ui, state, &mut action);
            }

            // Virtual Stage — operator 3D preview of the lighting rig. Toggling
            // this off stops all stage rendering (no render overhead); DMX
            // output to real fixtures is independent of it.
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.virtual_stage_enabled, "Virtual Stage");
                ui.label(
                    egui::RichText::new("operator 3D preview")
                        .small()
                        .color(egui::Color32::GRAY),
                );
            });
            if self.virtual_stage_enabled {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.label(egui::RichText::new("Zoom:").small());
                    if ui
                        .add(
                            egui::Slider::new(&mut self.virtual_stage_zoom, 0.4..=1.8)
                                .step_by(0.02)
                                .show_value(false),
                        )
                        .on_hover_text(
                            "Focal zoom — fixtures keep their stage positions; the rig scales in/out.",
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.label(egui::RichText::new("Rotate:").small());
                    if ui
                        .add(
                            egui::Slider::new(&mut self.virtual_stage_yaw, -180.0..=180.0)
                                .step_by(1.0)
                                .show_value(false),
                        )
                        .on_hover_text(
                            "Orbit yaw — 0 FOH-front, +90 stage-right, ±180 behind wall, -90 stage-left.",
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.label(egui::RichText::new("Tilt:").small());
                    if ui
                        .add(
                            egui::Slider::new(&mut self.virtual_stage_pitch, -60.0..=60.0)
                                .step_by(1.0)
                                .show_value(false),
                        )
                        .on_hover_text(
                            "Orbit pitch — +60 near-overhead, -60 low ground-level.",
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }
                    if ui
                        .small_button("Reset")
                        .on_hover_text("snap zoom + rotate + tilt back to the FOH base")
                        .clicked()
                    {
                        self.virtual_stage_zoom = 1.0;
                        self.virtual_stage_yaw = 0.0;
                        self.virtual_stage_pitch = 0.0;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    if ui
                        .checkbox(&mut self.virtual_stage_labels, "Fixture labels")
                        .on_hover_text("Show or hide the floating name tags on every fixture")
                        .changed()
                    {
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.label(egui::RichText::new("Stage:").small());
                    let selected_id = stage_ui_state.and_then(|s| s.selected_stage_id);
                    let stages: &[crate::stage_sync::StageSummary] = stage_ui_state
                        .map(|s| s.stages.as_slice())
                        .unwrap_or(&[]);
                    // A wedding is just a stage: this one dropdown picks the scene
                    // (a saved SaaS stage, or the local Wedding Room), and the venue
                    // follows from the choice. `Wedding Room` keeps the concert stage
                    // loaded underneath (stashed; sync paused) so switching is cheap.
                    let current_venue = self.virtual_stage_venue;
                    let is_wedding = matches!(current_venue, crate::emerge::VenueKind::WeddingRoom);
                    let selected_stage = stages.iter().find(|s| Some(s.id) == selected_id);
                    // Row/header label: a shared admin template reads as its venue
                    // ("Concert" / "Wedding") — its raw demo name is noise; a personal
                    // stage keeps its authored name ("My Concert Stage").
                    let stage_display = |s: &crate::stage_sync::StageSummary| -> String {
                        if s.shared {
                            match s.venue.unwrap_or(crate::emerge::VenueKind::ConcertStage) {
                                crate::emerge::VenueKind::WeddingRoom => "Wedding".to_string(),
                                _ => "Concert".to_string(),
                            }
                        } else {
                            s.name.clone()
                        }
                    };
                    let selected_label = selected_stage
                        .map(&stage_display)
                        .unwrap_or_else(|| "Select stage\u{2026}".to_string());
                    let mut pick_venue: Option<crate::emerge::VenueKind> = None;

                    let combo = egui::ComboBox::from_id_salt("react_stage_select")
                        .selected_text(selected_label)
                        .width(180.0);
                    let combo_response = combo.show_ui(ui, |ui| {
                        // Concert, no stage — the empty "pick a stage" state.
                        if ui
                            .selectable_label(!is_wedding && selected_id.is_none(), "Select stage\u{2026}")
                            .clicked()
                        {
                            pick_venue = Some(crate::emerge::VenueKind::ConcertStage);
                            if selected_id.is_some() {
                                action = OscAction::SelectStage(None);
                            }
                        }
                        // Split the list: shared admin templates (the canonical
                        // Concert + Wedding) pin to the top in their own section; the
                        // account's own "My …" stages sit below a separator. Only an
                        // admin login may edit a shared stage (server-enforced); a
                        // non-admin gets a "Clone to My Stages" button to fork an
                        // editable personal copy.
                        let shared_stages: Vec<&crate::stage_sync::StageSummary> =
                            stages.iter().filter(|s| s.shared).collect();
                        let my_stages: Vec<&crate::stage_sync::StageSummary> =
                            stages.iter().filter(|s| !s.shared).collect();
                        let have_my_for = |v: crate::emerge::VenueKind| -> bool {
                            my_stages.iter().any(|s| {
                                s.venue.unwrap_or(crate::emerge::VenueKind::ConcertStage) == v
                            })
                        };

                        // One row: a selectable that picks the stage, plus (shared +
                        // read-only + no personal copy yet) a Clone-to-My button.
                        let mut render_row =
                            |ui: &mut egui::Ui, stage: &crate::stage_sync::StageSummary| {
                                let stage_venue = stage
                                    .venue
                                    .unwrap_or(crate::emerge::VenueKind::ConcertStage);
                                let stage_is_wedding =
                                    stage_venue == crate::emerge::VenueKind::WeddingRoom;
                                let label = if stage.active {
                                    stage_display(stage)
                                } else {
                                    format!("{} (inactive)", stage_display(stage))
                                };
                                // Highlight only when selected AND actually the rendered
                                // scene (a concert stage isn't rendered while previewing
                                // the Wedding demo).
                                let row_active = Some(stage.id) == selected_id
                                    && (stage_is_wedding || !is_wedding);
                                let offer_clone = stage.shared
                                    && !stage.editable
                                    && !have_my_for(stage_venue);
                                ui.horizontal(|ui| {
                                    if ui.selectable_label(row_active, label).clicked() {
                                        pick_venue = Some(stage_venue);
                                        if selected_id != Some(stage.id) {
                                            action = OscAction::SelectStage(Some(stage.id));
                                        }
                                    }
                                    if offer_clone
                                        && ui
                                            .small_button("Clone to My")
                                            .on_hover_text(
                                                "Make an editable copy in your My Stages",
                                            )
                                            .clicked()
                                    {
                                        action = OscAction::CloneStage(stage.id);
                                    }
                                });
                            };

                        if !shared_stages.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new("SHARED (ADMIN)").small().weak());
                            for stage in &shared_stages {
                                render_row(ui, stage);
                            }
                        }
                        if !my_stages.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new("MY STAGES").small().weak());
                            for stage in &my_stages {
                                render_row(ui, stage);
                            }
                        }
                        if !is_authenticated {
                            ui.label(
                                egui::RichText::new("log in to load saved stages")
                                    .small()
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                        } else if stages.is_empty() {
                            ui.label(
                                egui::RichText::new("no saved stages on this account")
                                    .small()
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                        } else if my_stages.is_empty() {
                            // Only shared admin templates exist — point the operator at
                            // the Clone button so they know how to get an editable stage.
                            ui.label(
                                egui::RichText::new("clone a shared stage to start your own")
                                    .small()
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                        }
                    });
                    if let Some(v) = pick_venue {
                        self.set_virtual_stage_venue(v);
                    }
                    // Hitting the combo refreshes the cached list — cheap thanks
                    // to the in-memory TTL inside DmxManager.
                    if is_authenticated && combo_response.response.clicked() {
                        if matches!(action, OscAction::None) {
                            action = OscAction::RefreshStageList;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let (label, color) = sync_status_summary(stage_ui_state, is_authenticated);
                    ui.label(egui::RichText::new(label).small().color(color));
                });
                // COM-499 operator-launch-local: fire cached cues with ZERO SaaS —
                // works offline. Defs are cached the first time a cue is launched
                // online; launching here drives instantly and survives a network pull.
                if let Some(s) = stage_ui_state {
                    if s.selected_stage_id.is_some() {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            ui.label(egui::RichText::new("Cue:").small());
                            egui::ComboBox::from_id_salt("react_local_cue_launch")
                                .selected_text("Launch cue\u{2026}")
                                .width(180.0)
                                .show_ui(ui, |ui| {
                                    if s.cue_defs.is_empty() {
                                        ui.label(
                                            egui::RichText::new("launch a cue online once to cache it")
                                                .small()
                                                .italics()
                                                .color(egui::Color32::GRAY),
                                        );
                                    }
                                    for (key, label) in &s.cue_defs {
                                        if ui.selectable_label(false, label).clicked() {
                                            action = OscAction::LaunchLocalCue(key.clone());
                                        }
                                    }
                                });
                        });
                        for (launch_id, label) in &s.active_local_cues {
                            ui.horizontal(|ui| {
                                ui.add_space(22.0);
                                if ui
                                    .small_button("\u{2715}")
                                    .on_hover_text("stop this local cue")
                                    .clicked()
                                {
                                    action = OscAction::StopLocalCue(launch_id.clone());
                                }
                                ui.label(
                                    egui::RichText::new(format!("\u{25CF} {}", label)).small(),
                                );
                            });
                        }
                    }
                }
            }

            // OSC subsection
            egui::CollapsingHeader::new(
                egui::RichText::new("OSC").size(12.0)
            )
            .default_open(true)
            .show(ui, |ui| {
                let state = match osc_state {
                    Some(s) => s,
                    None => {
                        ui.label(egui::RichText::new("OSC not initialized").color(egui::Color32::GRAY).italics());
                        return;
                    }
                };

                ui.add_space(4.0);

                // Enable/Disable toggle
                ui.horizontal(|ui| {
                    let mut enabled = state.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        action = OscAction::SetEnabled(enabled);
                    }
                    let status_text = if state.enabled { "Enabled" } else { "Disabled" };
                    let status_color = if state.enabled {
                        egui::Color32::from_rgb(100, 200, 100)
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.label(egui::RichText::new(status_text).color(status_color));
                });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Target address
                ui.label(egui::RichText::new("Target").small().color(egui::Color32::GRAY));
                ui.horizontal(|ui| {
                    ui.label("IP:");
                    let ip_response = ui.add(
                        egui::TextEdit::singleline(&mut self.osc_target_ip_input)
                            .desired_width(100.0)
                            .hint_text("127.0.0.1")
                    );

                    ui.label("Port:");
                    let port_response = ui.add(
                        egui::TextEdit::singleline(&mut self.osc_target_port_input)
                            .desired_width(50.0)
                            .hint_text("8000")
                    );

                    // Apply button
                    if ui.button("Apply").clicked() {
                        if let Ok(port) = self.osc_target_port_input.parse::<u16>() {
                            action = OscAction::SetTarget(self.osc_target_ip_input.clone(), port);
                        }
                    }

                    // Update inputs from state if not currently editing
                    if !ip_response.has_focus() && self.osc_target_ip_input != state.target_ip {
                        self.osc_target_ip_input = state.target_ip.clone();
                    }
                    if !port_response.has_focus() && self.osc_target_port_input != state.target_port.to_string() {
                        self.osc_target_port_input = state.target_port.to_string();
                    }
                });

                // Update cached resolved IP when target changes (only resolve once per target)
                if self.osc_last_resolved_target != state.target_ip {
                    self.osc_last_resolved_target = state.target_ip.clone();
                    // Only resolve if it looks like a hostname
                    let looks_like_hostname = !state.target_ip.chars().all(|c| c.is_ascii_digit() || c == '.');
                    if looks_like_hostname {
                        self.osc_resolved_ip_cache = OscSender::resolve_hostname(&state.target_ip);
                    } else {
                        self.osc_resolved_ip_cache = None;
                    }
                }

                // Fixed height for resolved IP line (prevents jumping when hostname vs IP)
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 16.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let looks_like_hostname = !state.target_ip.chars().all(|c| c.is_ascii_digit() || c == '.');
                        if looks_like_hostname {
                            ui.label(egui::RichText::new("Resolved:").small().color(egui::Color32::GRAY));
                            if let Some(ref resolved) = self.osc_resolved_ip_cache {
                                ui.label(egui::RichText::new(resolved).small().color(egui::Color32::from_rgb(100, 180, 100)));
                            } else {
                                ui.label(egui::RichText::new("(not found)").small().color(egui::Color32::from_rgb(200, 100, 100)));
                            }
                        }
                    }
                );

                ui.add_space(8.0);

                // Fixed height status section (prevents jumping when enabled/disabled)
                let status_height = if state.enabled { 90.0 } else { 18.0 };
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), status_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        if state.enabled {
                            ui.label(egui::RichText::new("Status").small().color(egui::Color32::GRAY));

                            ui.horizontal(|ui| {
                                // Energy zone indicator
                                let zone_name = state.current_zone.as_str();
                                let zone_color = match state.current_zone {
                                    crate::emerge::EnergyZone::Silent => egui::Color32::from_rgb(60, 60, 60),
                                    crate::emerge::EnergyZone::Still => egui::Color32::from_rgb(80, 80, 120),
                                    crate::emerge::EnergyZone::Low => egui::Color32::from_rgb(80, 120, 80),
                                    crate::emerge::EnergyZone::Mid => egui::Color32::from_rgb(120, 120, 80),
                                    crate::emerge::EnergyZone::High => egui::Color32::from_rgb(200, 60, 60),
                                };
                                ui.label(egui::RichText::new(format!("Zone: {}", zone_name)).color(zone_color));

                                ui.separator();

                                // Harmonic state indicator
                                let harmonic_name = state.current_harmonic.as_str();
                                ui.label(egui::RichText::new(harmonic_name).small());
                            });

                            // Fixed height for status indicators (BLACKOUT/STROBE)
                            // Always render placeholder to maintain consistent height
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    // Always show labels (invisible when inactive) to maintain layout
                                    let blackout_color = if state.blackout_active {
                                        egui::Color32::from_rgb(100, 100, 100)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    let strobe_color = if state.strobe_hold_active {
                                        egui::Color32::from_rgb(255, 255, 100)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    ui.label(egui::RichText::new("BLACKOUT").color(blackout_color));
                                    ui.label(egui::RichText::new("STROBE").color(strobe_color));
                                }
                            );
                        } else {
                            ui.label(egui::RichText::new("OSC disabled").small().color(egui::Color32::GRAY).italics());
                        }
                    }
                );

                // Fixed height section for OSC cue mapping status (prevents jumping when enabled/disabled)
                let cues_triggers_height = 62.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), cues_triggers_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        if !state.enabled {
                            return; // Empty space when disabled
                        }

                        ui.add_space(4.0);

                        // Zone → Cue mapping display with activity indicators
                        ui.label(egui::RichText::new("Cues").small().color(egui::Color32::GRAY));

                        // Helper: resolve pattern index to name from MaestroDMX data
                        let resolve_pattern_name = |idx: i32| -> Option<String> {
                            state.maestro_patterns.iter()
                                .find(|(i, _)| *i == idx)
                                .map(|(_, name)| name.clone())
                        };

                        ui.horizontal(|ui| {
                            let zones: [(crate::emerge::EnergyZone, &str, Option<i32>, usize); 5] = [
                                (crate::emerge::EnergyZone::Silent, "SILENT", Some(state.config.pattern_silent), 0usize),
                                (crate::emerge::EnergyZone::Still, "STILL", Some(state.config.pattern_still), 1usize),
                                (crate::emerge::EnergyZone::Low, "LOW", Some(state.config.pattern_low), 2usize),
                                (crate::emerge::EnergyZone::Mid, "MID", Some(state.config.pattern_mid), 3usize),
                                (crate::emerge::EnergyZone::High, "HIGH", Some(state.config.pattern_high), 4usize),
                            ];
                            for (zone, name, pattern_idx, idx) in zones {
                                let is_current = state.current_zone == zone;
                                let activity = state.zone_activity[idx];

                                // Activity indicator using phosphor icon
                                let indicator_color = if activity > 0.0 {
                                    // Fade from bright green to dark
                                    let g = (100.0 + 155.0 * activity) as u8;
                                    egui::Color32::from_rgb(30, g, 30)
                                } else {
                                    egui::Color32::from_rgb(50, 50, 50)
                                };
                                ui.label(egui::RichText::new(egui_phosphor::regular::CIRCLE)
                                    .size(10.0)
                                    .color(indicator_color));

                                // Zone label: resolve pattern name when MaestroDMX data available
                                let text = match pattern_idx {
                                    Some(pidx) => {
                                        if let Some(pname) = resolve_pattern_name(pidx) {
                                            format!("{}→{}", name, pname)
                                        } else {
                                            format!("{}→{}", name, pidx)
                                        }
                                    }
                                    None => name.to_string(),
                                };
                                let color = if is_current {
                                    egui::Color32::from_rgb(100, 200, 100)
                                } else {
                                    egui::Color32::from_rgb(120, 120, 120)
                                };
                                let label = egui::RichText::new(text).small().color(color);
                                if is_current {
                                    ui.label(label.strong());
                                } else {
                                    ui.label(label);
                                }
                                ui.add_space(6.0);
                            }
                        });

                        // Active pattern label
                        if state.current_pattern >= 0 {
                            let pattern_label = if let Some(pname) = resolve_pattern_name(state.current_pattern) {
                                format!("Active Pattern: {} ({})", pname, state.current_pattern)
                            } else {
                                format!("Active Pattern: {}", state.current_pattern)
                            };
                            ui.label(egui::RichText::new(pattern_label).small().color(egui::Color32::from_rgb(140, 180, 140)));
                        }


                }
            );

                // Mapping subsection (only when enabled)
                if state.enabled {
                ui.add_space(4.0);
                let mapping_header = egui::CollapsingHeader::new(
                    egui::RichText::new("Mapping").size(12.0)
                )
                .default_open(self.osc_mapping_expanded)
                .show(ui, |ui| {

                // Sync UI state from config on first load
                if !self.osc_mapping_initialized {
                    self.osc_profile_index = match state.config.target_profile {
                        crate::osc::TargetProfile::Generic => 0,
                        crate::osc::TargetProfile::MaestroDMX => 1,
                        crate::osc::TargetProfile::QLCPlus => 2,
                        crate::osc::TargetProfile::Custom => 3,
                    };
                    self.osc_palette_ice_name = state.config.palette_ice_name.clone();
                    self.osc_palette_cool_a_name = state.config.palette_cool_a_name.clone();
                    self.osc_palette_cool_b_name = state.config.palette_cool_b_name.clone();
                    self.osc_palette_ice_index = state.config.palette_ice_index.to_string();
                    self.osc_palette_cool_a_index = state.config.palette_cool_a_index.to_string();
                    self.osc_palette_cool_b_index = state.config.palette_cool_b_index.to_string();
                    self.osc_pattern_silent = state.config.pattern_silent.to_string();
                    self.osc_pattern_still = state.config.pattern_still.to_string();
                    self.osc_pattern_low = state.config.pattern_low.to_string();
                    self.osc_pattern_mid = state.config.pattern_mid.to_string();
                    self.osc_pattern_high = state.config.pattern_high.to_string();
                    self.osc_mover_speed_min = state.config.mover_speed_min;
                    self.osc_mover_speed_max = state.config.mover_speed_max;
                    self.osc_enable_fx_palette = state.config.enable_fx_palette;

                    // Reactive lighting features
                    self.osc_enable_polarity_flash = state.config.enable_polarity_flash;
                    self.osc_enable_dominant_accent = state.config.enable_dominant_accent;
                    self.osc_enable_tonality_palettes = state.config.enable_tonality_palettes;
                    self.osc_enable_energy_slope = state.config.enable_energy_slope;

                    // Sync genre selection indices
                    let categories = crate::emerge::GenreProfile::categories();
                    self.osc_genre_category_index = categories.iter()
                        .position(|c| *c == state.config.genre_profile.category)
                        .unwrap_or(0);
                    let subcategories = crate::emerge::GenreProfile::subcategories_for(&state.config.genre_profile.category);
                    self.osc_genre_subcategory_index = subcategories.iter()
                        .position(|s| *s == state.config.genre_profile.subcategory)
                        .unwrap_or(0);

                    self.osc_mapping_initialized = true;
                }

                ui.add_space(4.0);

                // === TARGET PROFILE DROPDOWN ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Target Profile:").small());
                    let profiles = ["Generic", "MaestroDMX", "QLC+", "Custom"];
                    let old_index = self.osc_profile_index;
                    egui::ComboBox::from_id_salt("osc_profile")
                        .selected_text(profiles[self.osc_profile_index.min(profiles.len() - 1)])
                        .width(100.0)
                        .show_ui(ui, |ui| {
                            for (i, profile) in profiles.iter().enumerate() {
                                ui.selectable_value(&mut self.osc_profile_index, i, *profile);
                            }
                        });

                    // Apply profile change with defaults
                    if self.osc_profile_index != old_index {
                        match self.osc_profile_index {
                            1 => {
                                // MaestroDMX defaults
                                // Pattern indices: 3=Chill, 2=Ambient, 4=Energetic, 5=Party
                                self.osc_pattern_silent = "3".to_string();
                                self.osc_pattern_still = "3".to_string();
                                self.osc_pattern_low = "2".to_string();
                                self.osc_pattern_mid = "4".to_string();
                                self.osc_pattern_high = "5".to_string();
                                // Palette names (for display)
                                self.osc_palette_ice_name = "Cool White".to_string();
                                self.osc_palette_cool_a_name = "Cyan".to_string();
                                self.osc_palette_cool_b_name = "Stained Glass".to_string();
                                // Palette indices: 0=Cool White, 6=Cyan, 12=Stained Glass
                                self.osc_palette_ice_index = "0".to_string();
                                self.osc_palette_cool_a_index = "6".to_string();
                                self.osc_palette_cool_b_index = "12".to_string();
                                self.osc_mover_speed_min = 0.2;
                                self.osc_mover_speed_max = 0.8;
                            }
                            2 => {
                                // QLC+ defaults (same as generic)
                                self.osc_pattern_silent = "0".to_string();
                                self.osc_pattern_still = "0".to_string();
                                self.osc_pattern_low = "1".to_string();
                                self.osc_pattern_mid = "2".to_string();
                                self.osc_pattern_high = "3".to_string();
                                self.osc_palette_ice_name = "ICE".to_string();
                                self.osc_palette_cool_a_name = "COOL_A".to_string();
                                self.osc_palette_cool_b_name = "COOL_B".to_string();
                                self.osc_palette_ice_index = "0".to_string();
                                self.osc_palette_cool_a_index = "1".to_string();
                                self.osc_palette_cool_b_index = "2".to_string();
                                self.osc_mover_speed_min = 0.0;
                                self.osc_mover_speed_max = 1.0;
                            }
                            _ => {
                                // Generic defaults
                                self.osc_pattern_silent = "0".to_string();
                                self.osc_pattern_still = "0".to_string();
                                self.osc_pattern_low = "1".to_string();
                                self.osc_pattern_mid = "2".to_string();
                                self.osc_pattern_high = "3".to_string();
                                self.osc_palette_ice_name = "ICE".to_string();
                                self.osc_palette_cool_a_name = "COOL_A".to_string();
                                self.osc_palette_cool_b_name = "COOL_B".to_string();
                                self.osc_palette_ice_index = "0".to_string();
                                self.osc_palette_cool_a_index = "1".to_string();
                                self.osc_palette_cool_b_index = "2".to_string();
                                self.osc_mover_speed_min = 0.0;
                                self.osc_mover_speed_max = 1.0;
                            }
                        }

                        // Build and send updated config
                        let mut new_config = state.config.clone();
                        new_config.target_profile = match self.osc_profile_index {
                            1 => crate::osc::TargetProfile::MaestroDMX,
                            2 => crate::osc::TargetProfile::QLCPlus,
                            3 => crate::osc::TargetProfile::Custom,
                            _ => crate::osc::TargetProfile::Generic,
                        };
                        new_config.palette_ice_name = self.osc_palette_ice_name.clone();
                        new_config.palette_cool_a_name = self.osc_palette_cool_a_name.clone();
                        new_config.palette_cool_b_name = self.osc_palette_cool_b_name.clone();
                        new_config.palette_ice_index = self.osc_palette_ice_index.parse().unwrap_or(0);
                        new_config.palette_cool_a_index = self.osc_palette_cool_a_index.parse().unwrap_or(1);
                        new_config.palette_cool_b_index = self.osc_palette_cool_b_index.parse().unwrap_or(2);
                        new_config.pattern_silent = self.osc_pattern_silent.parse().unwrap_or(0);
                        new_config.pattern_still = self.osc_pattern_still.parse().unwrap_or(0);
                        new_config.pattern_low = self.osc_pattern_low.parse().unwrap_or(1);
                        new_config.pattern_mid = self.osc_pattern_mid.parse().unwrap_or(2);
                        new_config.pattern_high = self.osc_pattern_high.parse().unwrap_or(3);
                        new_config.mover_speed_min = self.osc_mover_speed_min;
                        new_config.mover_speed_max = self.osc_mover_speed_max;
                        new_config.enable_fx_palette = self.osc_enable_fx_palette;
                        // Reactive lighting features (preserve current state)
                        new_config.enable_polarity_flash = self.osc_enable_polarity_flash;
                        new_config.enable_dominant_accent = self.osc_enable_dominant_accent;
                        new_config.enable_tonality_palettes = self.osc_enable_tonality_palettes;
                        new_config.enable_energy_slope = self.osc_enable_energy_slope;
                        action = OscAction::UpdateConfig(new_config);
                    }
                });

                // MaestroDMX API Connection (when MaestroDMX profile is selected)
                if matches!(state.config.target_profile, crate::osc::TargetProfile::MaestroDMX) {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // Auto-fetch: trigger fetch if we don't have data and not loading
                    let current_ip = &state.target_ip;
                    let needs_fetch = !state.maestro_loading
                        && state.maestro_patterns.is_empty()
                        && state.maestro_color_palettes.is_empty()
                        && state.maestro_error.is_none();
                    let ip_changed = state.maestro_last_fetch_ip.as_ref() != Some(current_ip)
                        && !state.maestro_loading;

                    if needs_fetch || (ip_changed && state.maestro_last_fetch_ip.is_some()) {
                        action = OscAction::FetchMaestroData;
                    }

                    // Connection status line
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("API:").small());
                        if state.maestro_loading {
                            ui.spinner();
                            ui.label(egui::RichText::new("Connecting...").small().color(egui::Color32::GRAY));
                        } else if let Some(ref error) = state.maestro_error {
                            ui.label(egui::RichText::new(egui_phosphor::regular::WARNING_CIRCLE).color(egui::Color32::from_rgb(255, 100, 100)));
                            ui.label(egui::RichText::new("Offline").small().color(egui::Color32::from_rgb(255, 100, 100)));
                            if ui.small_button("Retry").clicked() {
                                action = OscAction::FetchMaestroData;
                            }
                        } else if !state.maestro_patterns.is_empty() {
                            ui.label(egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE).color(egui::Color32::from_rgb(100, 200, 100)));
                            ui.label(egui::RichText::new("Connected").small().color(egui::Color32::from_rgb(100, 200, 100)));
                            if ui.small_button(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Refresh").clicked() {
                                action = OscAction::FetchMaestroData;
                            }
                        } else {
                            ui.label(egui::RichText::new("Waiting...").small().color(egui::Color32::GRAY).italics());
                        }
                    });

                    // Pattern/Palette dropdowns (only when connected)
                    if !state.maestro_patterns.is_empty() {
                        ui.add_space(4.0);

                        // Sync dropdown to active pattern from zone transitions
                        if state.current_pattern >= 0 {
                            if let Some(pos) = state.maestro_patterns.iter().position(|(idx, _)| *idx == state.current_pattern) {
                                self.maestro_pattern_index = pos;
                            }
                        }

                        // Pattern dropdown
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Pattern:").small());
                            let current_name = state.maestro_patterns
                                .get(self.maestro_pattern_index)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("Select...");
                            let old_index = self.maestro_pattern_index;
                            egui::ComboBox::from_id_salt("maestro_pattern")
                                .selected_text(current_name)
                                .width(140.0)
                                .show_ui(ui, |ui| {
                                    for (i, (idx, name)) in state.maestro_patterns.iter().enumerate() {
                                        let label = format!("{} ({})", name, idx);
                                        ui.selectable_value(&mut self.maestro_pattern_index, i, label);
                                    }
                                });
                            if self.maestro_pattern_index != old_index {
                                if let Some((index, _)) = state.maestro_patterns.get(self.maestro_pattern_index) {
                                    action = OscAction::SelectMaestroPattern(*index);
                                }
                            }
                        });

                        // Color palette dropdown
                        if !state.maestro_color_palettes.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Color:").small());
                                let current_name = state.maestro_color_palettes
                                    .get(self.maestro_color_palette_index)
                                    .map(|(_, name)| name.as_str())
                                    .unwrap_or("Select...");
                                let old_index = self.maestro_color_palette_index;
                                egui::ComboBox::from_id_salt("maestro_color_palette")
                                    .selected_text(current_name)
                                    .width(140.0)
                                    .show_ui(ui, |ui| {
                                        for (i, (idx, name)) in state.maestro_color_palettes.iter().enumerate() {
                                            let label = format!("{} ({})", name, idx);
                                            ui.selectable_value(&mut self.maestro_color_palette_index, i, label);
                                        }
                                    });
                                if self.maestro_color_palette_index != old_index {
                                    if let Some((index, _)) = state.maestro_color_palettes.get(self.maestro_color_palette_index) {
                                        action = OscAction::SelectMaestroColorPalette(*index);
                                    }
                                }
                            });
                        }

                        // FX palette dropdown
                        if !state.maestro_fx_palettes.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("FX:").small());
                                let current_name = state.maestro_fx_palettes
                                    .get(self.maestro_fx_palette_index)
                                    .map(|(_, name)| name.as_str())
                                    .unwrap_or("Select...");
                                let old_index = self.maestro_fx_palette_index;
                                egui::ComboBox::from_id_salt("maestro_fx_palette")
                                    .selected_text(current_name)
                                    .width(140.0)
                                    .show_ui(ui, |ui| {
                                        for (i, (idx, name)) in state.maestro_fx_palettes.iter().enumerate() {
                                            let label = format!("{} ({})", name, idx);
                                            ui.selectable_value(&mut self.maestro_fx_palette_index, i, label);
                                        }
                                    });
                                if self.maestro_fx_palette_index != old_index {
                                    if let Some((index, _)) = state.maestro_fx_palettes.get(self.maestro_fx_palette_index) {
                                        action = OscAction::SelectMaestroFxPalette(*index);
                                    }
                                }
                            });
                        }
                    }
                }

                // Show live indicators when enabled
                if state.enabled {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        // Current palette
                        let (palette_label, palette_color) = match state.current_palette {
                            crate::emerge::Palette::Ice => (
                                format!("ICE: {}", state.config.palette_ice_name),
                                egui::Color32::from_rgb(200, 220, 255)
                            ),
                            crate::emerge::Palette::CoolA => (
                                format!("COOL_A: {}", state.config.palette_cool_a_name),
                                egui::Color32::from_rgb(100, 200, 200)
                            ),
                            crate::emerge::Palette::CoolB => (
                                format!("COOL_B: {}", state.config.palette_cool_b_name),
                                egui::Color32::from_rgb(180, 100, 200)
                            ),
                        };
                        ui.label(egui::RichText::new(egui_phosphor::regular::PALETTE)
                            .size(12.0)
                            .color(palette_color));
                        ui.label(egui::RichText::new(&palette_label).small().color(palette_color));
                    });

                    // Mover speed (calculated from tension)
                    let (_energy, tension, _balance, _stability) = state.smoothed_metrics;
                    let mover_speed = state.config.mover_speed_min
                        + tension * (state.config.mover_speed_max - state.config.mover_speed_min);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::ARROWS_OUT_CARDINAL)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(150, 150, 200)));
                        ui.label(egui::RichText::new(format!("Mover: {:.0}%", mover_speed * 100.0))
                            .small()
                            .color(egui::Color32::from_rgb(150, 150, 200)));

                        // FX indicator
                        if state.config.enable_fx_palette {
                            ui.add_space(12.0);
                            let fx_index = if tension > 0.7 { 3 }
                                else if tension > 0.5 { 2 }
                                else if tension > 0.3 { 1 }
                                else { 0 };
                            let fx_labels = ["Subtle", "Moderate", "Active", "Intense"];
                            ui.label(egui::RichText::new(egui_phosphor::regular::MAGIC_WAND)
                                .size(12.0)
                                .color(egui::Color32::from_rgb(200, 150, 100)));
                            ui.label(egui::RichText::new(format!("FX: {}", fx_labels[fx_index]))
                                .small()
                                .color(egui::Color32::from_rgb(200, 150, 100)));
                        }
                    });
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // === ZONE → PATTERN INDEX ===
                ui.label(egui::RichText::new("Zone → Pattern Index").small().color(egui::Color32::GRAY));
                ui.add_space(2.0);

                let mut config_changed = false;
                egui::Grid::new("osc_pattern_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        // SILENT
                        ui.label(egui::RichText::new("SILENT").small());
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_pattern_silent)
                            .desired_width(40.0)
                            .hint_text("0")).changed() {
                            config_changed = true;
                        }
                        ui.end_row();

                        // STILL
                        ui.label(egui::RichText::new("STILL").small());
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_pattern_still)
                            .desired_width(40.0)
                            .hint_text("0")).changed() {
                            config_changed = true;
                        }
                        ui.end_row();

                        // LOW
                        ui.label(egui::RichText::new("LOW").small());
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_pattern_low)
                            .desired_width(40.0)
                            .hint_text("1")).changed() {
                            config_changed = true;
                        }
                        ui.end_row();

                        // MID
                        ui.label(egui::RichText::new("MID").small());
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_pattern_mid)
                            .desired_width(40.0)
                            .hint_text("2")).changed() {
                            config_changed = true;
                        }
                        ui.end_row();

                        // HIGH
                        ui.label(egui::RichText::new("HIGH").small());
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_pattern_high)
                            .desired_width(40.0)
                            .hint_text("3")).changed() {
                            config_changed = true;
                        }
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // === PALETTE NAMES ===
                ui.label(egui::RichText::new("Palette Mapping").small().color(egui::Color32::GRAY));
                ui.add_space(2.0);

                egui::Grid::new("osc_palette_grid")
                    .num_columns(4)
                    .spacing([6.0, 4.0])
                    .show(ui, |ui| {
                        // Header
                        ui.label(egui::RichText::new("").small());
                        ui.label(egui::RichText::new("Index").small().color(egui::Color32::GRAY));
                        ui.label(egui::RichText::new("Name").small().color(egui::Color32::GRAY));
                        ui.label(egui::RichText::new("").small());
                        ui.end_row();

                        // ICE
                        ui.label(egui::RichText::new("ICE").small().color(egui::Color32::from_rgb(200, 220, 255)));
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_ice_index)
                            .desired_width(30.0)
                            .hint_text("0")).changed() {
                            config_changed = true;
                        }
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_ice_name)
                            .desired_width(120.0)
                            .hint_text("Cool White")).changed() {
                            config_changed = true;
                        }
                        ui.label(egui::RichText::new("").small());
                        ui.end_row();

                        // COOL_A
                        ui.label(egui::RichText::new("COOL_A").small().color(egui::Color32::from_rgb(100, 200, 200)));
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_cool_a_index)
                            .desired_width(30.0)
                            .hint_text("1")).changed() {
                            config_changed = true;
                        }
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_cool_a_name)
                            .desired_width(120.0)
                            .hint_text("Cyan")).changed() {
                            config_changed = true;
                        }
                        ui.label(egui::RichText::new("").small());
                        ui.end_row();

                        // COOL_B
                        ui.label(egui::RichText::new("COOL_B").small().color(egui::Color32::from_rgb(180, 100, 200)));
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_cool_b_index)
                            .desired_width(30.0)
                            .hint_text("2")).changed() {
                            config_changed = true;
                        }
                        if ui.add(egui::TextEdit::singleline(&mut self.osc_palette_cool_b_name)
                            .desired_width(120.0)
                            .hint_text("Stained Glass")).changed() {
                            config_changed = true;
                        }
                        ui.label(egui::RichText::new("").small());
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // === MOVER SPEED ===
                ui.label(egui::RichText::new("Mover Speed (tension-driven)").small().color(egui::Color32::GRAY));
                ui.add_space(2.0);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Min:").small());
                    if ui.add(egui::DragValue::new(&mut self.osc_mover_speed_min)
                        .range(0.0..=1.0)
                        .speed(0.01)
                        .fixed_decimals(2)).changed() {
                        config_changed = true;
                    }

                    ui.add_space(12.0);

                    ui.label(egui::RichText::new("Max:").small());
                    if ui.add(egui::DragValue::new(&mut self.osc_mover_speed_max)
                        .range(0.0..=1.0)
                        .speed(0.01)
                        .fixed_decimals(2)).changed() {
                        config_changed = true;
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // === FX PALETTE ===
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.osc_enable_fx_palette, "Enable FX palette").changed() {
                        config_changed = true;
                    }
                    ui.label(egui::RichText::new("(tension-driven)").small().color(egui::Color32::GRAY));
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // === REACTIVE LIGHTING FEATURES ===
                ui.label(egui::RichText::new("Reactive Accents").small().color(egui::Color32::GRAY));

                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.osc_enable_polarity_flash, "Polarity flash").changed() {
                        config_changed = true;
                    }
                    ui.label(egui::RichText::new("(E↔W, N↔S)").small().color(egui::Color32::DARK_GRAY));
                });

                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.osc_enable_dominant_accent, "Dominant accents").changed() {
                        config_changed = true;
                    }
                    ui.label(egui::RichText::new("(pitch changes)").small().color(egui::Color32::DARK_GRAY));
                });

                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.osc_enable_tonality_palettes, "Tonality palettes").changed() {
                        config_changed = true;
                    }
                    ui.label(egui::RichText::new("(major/minor)").small().color(egui::Color32::DARK_GRAY));
                });

                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.osc_enable_energy_slope, "Energy slope").changed() {
                        config_changed = true;
                    }
                    ui.label(egui::RichText::new("(build/drop)").small().color(egui::Color32::DARK_GRAY));
                });

                // Apply config changes
                if config_changed {
                    // Auto-switch to Custom profile if user edits values
                    if self.osc_profile_index != 3 {
                        // Don't auto-switch, just update values
                    }

                    let mut new_config = state.config.clone();
                    new_config.target_profile = match self.osc_profile_index {
                        1 => crate::osc::TargetProfile::MaestroDMX,
                        2 => crate::osc::TargetProfile::QLCPlus,
                        3 => crate::osc::TargetProfile::Custom,
                        _ => crate::osc::TargetProfile::Generic,
                    };
                    new_config.palette_ice_name = self.osc_palette_ice_name.clone();
                    new_config.palette_cool_a_name = self.osc_palette_cool_a_name.clone();
                    new_config.palette_cool_b_name = self.osc_palette_cool_b_name.clone();
                    new_config.palette_ice_index = self.osc_palette_ice_index.parse().unwrap_or(0);
                    new_config.palette_cool_a_index = self.osc_palette_cool_a_index.parse().unwrap_or(1);
                    new_config.palette_cool_b_index = self.osc_palette_cool_b_index.parse().unwrap_or(2);
                    new_config.pattern_silent = self.osc_pattern_silent.parse().unwrap_or(0);
                    new_config.pattern_still = self.osc_pattern_still.parse().unwrap_or(0);
                    new_config.pattern_low = self.osc_pattern_low.parse().unwrap_or(1);
                    new_config.pattern_mid = self.osc_pattern_mid.parse().unwrap_or(2);
                    new_config.pattern_high = self.osc_pattern_high.parse().unwrap_or(3);
                    new_config.mover_speed_min = self.osc_mover_speed_min;
                    new_config.mover_speed_max = self.osc_mover_speed_max;
                    new_config.enable_fx_palette = self.osc_enable_fx_palette;
                    // Reactive lighting features
                    new_config.enable_polarity_flash = self.osc_enable_polarity_flash;
                    new_config.enable_dominant_accent = self.osc_enable_dominant_accent;
                    new_config.enable_tonality_palettes = self.osc_enable_tonality_palettes;
                    new_config.enable_energy_slope = self.osc_enable_energy_slope;
                    action = OscAction::UpdateConfig(new_config);
                }

                ui.add_space(6.0);

                // === ADVANCED SECTION ===
                egui::CollapsingHeader::new(egui::RichText::new("Advanced").small())
                    .default_open(self.osc_advanced_expanded)
                    .show(ui, |ui| {
                        self.osc_advanced_expanded = true;
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("Current OSC Addresses").small().color(egui::Color32::GRAY));
                        ui.add_space(2.0);

                        let profile = &state.config.target_profile;
                        let addresses = [
                            ("Brightness", "/react/brightness"),
                            ("Palette", "/react/palette"),
                            ("Zone", "/react/zone"),
                            ("Pattern", "/react/pattern"),
                            ("Mover Speed", "/react/mover/speed"),
                            ("FX Index", "/react/fx/index"),
                            ("Strobe", "/react/trigger/strobe"),
                            ("Blackout", "/react/trigger/blackout"),
                        ];

                        egui::Grid::new("osc_address_grid")
                            .num_columns(2)
                            .spacing([8.0, 2.0])
                            .show(ui, |ui| {
                                for (name, react_addr) in addresses {
                                    let translated = profile.translate_address(react_addr);
                                    ui.label(egui::RichText::new(format!("{}:", name)).small());
                                    ui.label(egui::RichText::new(&translated).small().color(
                                        if translated == react_addr {
                                            egui::Color32::LIGHT_GRAY
                                        } else {
                                            egui::Color32::from_rgb(100, 200, 100)
                                        }
                                    ));
                                    ui.end_row();
                                }
                            });

                        ui.add_space(4.0);
                    });

                ui.add_space(4.0);
            });
            self.osc_mapping_expanded = mapping_header.fully_open();
                }
            });

            // === Unified Devices subsection (Hue + WLED) ===
            ui.add_space(4.0);
            let devices_expanded = self.hue_expanded || self.wled_expanded;
            let devices_header = egui::CollapsingHeader::new(
                egui::RichText::new("Devices").size(12.0)
            )
            .default_open(devices_expanded)
            .show(ui, |ui| {
                // --- Hue Bridge (Zigbee / single-color-per-light) ---
                ui.add_space(2.0);
                ui.label(egui::RichText::new("Hue Bridge  (Zigbee / single-color-per-light)").small().strong().color(egui::Color32::from_rgb(180, 180, 180)));
                ui.add_space(2.0);
                hue_action = self.show_hue_subsection(ui, hue_state);

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // --- WLED Strips (Wi-Fi / per-LED addressable) ---
                ui.label(egui::RichText::new("WLED Strips  (Wi-Fi / per-LED addressable)").small().strong().color(egui::Color32::from_rgb(180, 180, 180)));
                ui.add_space(1.0);
                ui.label(egui::RichText::new("WLED strips connect directly over Wi-Fi, not through the Hue Bridge.")
                    .small().italics().color(egui::Color32::from_rgb(130, 130, 130)));
                ui.add_space(2.0);
                // Pass Hue bridge IP for cross-validation
                let hue_bridge_ip = hue_state.map(|s| s.bridge_ip.as_str()).unwrap_or("");
                wled_action = self.show_wled_subsection(ui, wled_state, hue_bridge_ip);
            });
            let dev_open = devices_header.fully_open();
            self.hue_expanded = dev_open;
            self.wled_expanded = dev_open;

            // --- DMX Output (USB Open DMX / Art-Net / OLA) ---
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);
            ui.label(egui::RichText::new("DMX Output").small().strong().color(egui::Color32::from_rgb(180, 180, 180)));
            ui.add_space(1.0);
            if let Some(stage) = stage_ui_state {
                let status = &stage.dmx_status;
                let (dot, dot_color) = if status.connected {
                    ("\u{25CF}", egui::Color32::from_rgb(100, 255, 100))
                } else {
                    ("\u{25CB}", egui::Color32::from_rgb(160, 160, 160))
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(dot).color(dot_color));
                    let detail = if status.detail.is_empty() {
                        String::new()
                    } else {
                        format!("  \u{2014}  {}", status.detail)
                    };
                    ui.label(egui::RichText::new(format!("{}{}", status.backend.label(), detail)).small());
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Backend:").small());
                    egui::ComboBox::from_id_salt("dmx_backend_select")
                        .selected_text(status.backend.label())
                        .show_ui(ui, |ui| {
                            for b in [
                                crate::dmx::DmxBackend::Disabled,
                                crate::dmx::DmxBackend::Usb,
                                crate::dmx::DmxBackend::ArtNet,
                                crate::dmx::DmxBackend::Ola,
                            ] {
                                if ui.selectable_label(status.backend == b, b.label()).clicked() {
                                    // Switching to USB with no port yet → seed from auto-detect.
                                    if b == crate::dmx::DmxBackend::Usb && self.dmx_port_edit.trim().is_empty() {
                                        if let Some(p) = crate::dmx::UsbDmxSender::autodetect_port() {
                                            self.dmx_port_edit = p;
                                        }
                                    }
                                    // Seed the new backend's connection from its panel
                                    // field so the switch connects immediately (empty →
                                    // None → env/auto-detect fallback).
                                    let endpoint = match b {
                                        crate::dmx::DmxBackend::Usb => (!self.dmx_port_edit.trim().is_empty())
                                            .then(|| self.dmx_port_edit.trim().to_string()),
                                        crate::dmx::DmxBackend::ArtNet => (!self.dmx_artnet_target_edit.trim().is_empty())
                                            .then(|| self.dmx_artnet_target_edit.trim().to_string()),
                                        crate::dmx::DmxBackend::Ola => (!self.dmx_ola_host_edit.trim().is_empty())
                                            .then(|| Self::ola_endpoint(&self.dmx_ola_host_edit, &self.dmx_ola_port_edit)),
                                        crate::dmx::DmxBackend::Disabled => None,
                                    };
                                    action = OscAction::SetDmxBackend(b, endpoint);
                                }
                            }
                        });
                });
                if matches!(status.backend, crate::dmx::DmxBackend::Usb) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Port:").small());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dmx_port_edit)
                                .desired_width(170.0)
                                .hint_text("/dev/cu.usbserial-\u{2026}"),
                        );
                        if ui.small_button("Detect").clicked() {
                            if let Some(p) = crate::dmx::UsbDmxSender::autodetect_port() {
                                self.dmx_port_edit = p;
                            }
                        }
                        if ui.small_button("Connect").clicked() {
                            let port = if self.dmx_port_edit.trim().is_empty() {
                                None
                            } else {
                                Some(self.dmx_port_edit.trim().to_string())
                            };
                            action = OscAction::SetDmxBackend(crate::dmx::DmxBackend::Usb, port);
                        }
                    });
                } else if matches!(status.backend, crate::dmx::DmxBackend::ArtNet) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Target:").small());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dmx_artnet_target_edit)
                                .desired_width(170.0)
                                .hint_text("192.168.1.50:6454"),
                        );
                        if ui.small_button("Connect").clicked() {
                            let target = if self.dmx_artnet_target_edit.trim().is_empty() {
                                None
                            } else {
                                Some(self.dmx_artnet_target_edit.trim().to_string())
                            };
                            action = OscAction::SetDmxBackend(crate::dmx::DmxBackend::ArtNet, target);
                        }
                    });
                } else if matches!(status.backend, crate::dmx::DmxBackend::Ola) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Host:").small());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dmx_ola_host_edit)
                                .desired_width(110.0)
                                .hint_text("127.0.0.1"),
                        );
                        ui.label(egui::RichText::new("Port:").small());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dmx_ola_port_edit)
                                .desired_width(52.0)
                                .hint_text("9010"),
                        );
                        if ui.small_button("Connect").clicked() {
                            let endpoint = if self.dmx_ola_host_edit.trim().is_empty() {
                                None
                            } else {
                                // Snap the Port field to the numeric value we will
                                // actually use, so a non-numeric entry doesn't keep
                                // displaying a port the system silently ignores
                                // (blank/garbage → olad's default 9010).
                                self.dmx_ola_port_edit = self
                                    .dmx_ola_port_edit
                                    .trim()
                                    .parse::<u16>()
                                    .unwrap_or(9010)
                                    .to_string();
                                Some(Self::ola_endpoint(&self.dmx_ola_host_edit, &self.dmx_ola_port_edit))
                            };
                            action = OscAction::SetDmxBackend(crate::dmx::DmxBackend::Ola, endpoint);
                        }
                    });
                }
            } else {
                ui.label(egui::RichText::new("DMX status unavailable").small().italics().color(egui::Color32::DARK_GRAY));
            }
            ui.add_space(4.0);
        });

        self.osc_expanded = header.fully_open();
        (action, hue_action, wled_action)
    }

    fn show_lighting_manual_triggers(&mut self, ui: &mut egui::Ui, state: &OscState, action: &mut OscAction) {
        ui.label(egui::RichText::new("Global Triggers").small().color(egui::Color32::GRAY));

        // Shared by the momentary buttons on both trigger rows.
        let handle_trigger = |response: &egui::Response, held: bool, button: TriggerButton| -> Option<OscAction> {
            let pointer_down = response.is_pointer_button_down_on();

            if pointer_down && !held {
                log::info!("[Trigger] {} ON", button.as_str());
                Some(OscAction::TriggerPress(button))
            } else if !pointer_down && held {
                log::info!("[Trigger] {} OFF", button.as_str());
                Some(OscAction::TriggerRelease(button))
            } else {
                None
            }
        };

        ui.horizontal(|ui| {
            let strobe_held = state.trigger_strobe_held;
            let (strobe_fill, strobe_text) = if strobe_held {
                (egui::Color32::from_rgb(255, 255, 100), egui::Color32::BLACK)
            } else {
                (egui::Color32::from_rgb(60, 60, 60), egui::Color32::WHITE)
            };
            let strobe_response = ui.add(
                egui::Button::new(egui::RichText::new("STROBE").color(strobe_text).small())
                    .fill(strobe_fill)
                    .sense(egui::Sense::click_and_drag())
            );
            if let Some(a) = handle_trigger(&strobe_response, strobe_held, TriggerButton::Strobe) {
                *action = a;
            }

            // UV mode — "blackout, except blacklight": only UV-capable
            // fixtures emit while held (or latched). Placed next to STROBE
            // (operator-requested). Active when EITHER held or latched;
            // latched stays a deeper violet so the operator can tell at a
            // glance it's running hands-free (mirrors FOG below).
            let uv_held = state.trigger_uv_held;
            let uv_latched = state.trigger_uv_latched;
            let uv_fill = if uv_held {
                egui::Color32::from_rgb(168, 85, 247)
            } else if uv_latched {
                egui::Color32::from_rgb(107, 33, 168)
            } else {
                egui::Color32::from_rgb(60, 60, 60)
            };
            let uv_response = ui.add(
                egui::Button::new(egui::RichText::new("UV").color(egui::Color32::WHITE).small())
                    .fill(uv_fill)
                    .sense(egui::Sense::click_and_drag())
            );
            if let Some(a) = handle_trigger(&uv_response, uv_held, TriggerButton::Uv) {
                *action = a;
            }
            // Latch toggle next to UV — same lock affordance as FOG's. Tap
            // once to keep UV mode on hands-free; tap again to release.
            let uv_latch_label = if uv_latched { "🔒" } else { "🔓" };
            let uv_latch_fill = if uv_latched {
                egui::Color32::from_rgb(107, 33, 168)
            } else {
                egui::Color32::from_rgb(50, 50, 50)
            };
            let uv_latch_response = ui.add(
                egui::Button::new(egui::RichText::new(uv_latch_label).small().color(egui::Color32::WHITE))
                    .fill(uv_latch_fill)
            ).on_hover_text(if uv_latched {
                "UV latch ON — tap to release"
            } else {
                "Latch UV mode ON (hands-free hold)"
            });
            if uv_latch_response.clicked() {
                *action = OscAction::ToggleUvLatch;
            }

            let blackout_held = state.trigger_blackout_held;
            let blackout_fill = if blackout_held {
                egui::Color32::from_rgb(200, 50, 50)
            } else {
                egui::Color32::from_rgb(60, 60, 60)
            };
            let blackout_response = ui.add(
                egui::Button::new(egui::RichText::new("BLACKOUT").color(egui::Color32::WHITE).small())
                    .fill(blackout_fill)
                    .sense(egui::Sense::click_and_drag())
            );
            if let Some(a) = handle_trigger(&blackout_response, blackout_held, TriggerButton::Blackout) {
                *action = a;
            }

            let blinder_held = state.trigger_blinder_held;
            let macro_phase = state.macro_phase;
            let (blinder_fill, blinder_text) = if blinder_held {
                (egui::Color32::WHITE, egui::Color32::BLACK)
            } else {
                match macro_phase {
                    crate::emerge::AutoBlinderPhase::BlindersHold => {
                        (egui::Color32::from_rgb(255, 200, 0), egui::Color32::BLACK)
                    }
                    crate::emerge::AutoBlinderPhase::OverdriveStrobe => {
                        (egui::Color32::from_rgb(255, 40, 40), egui::Color32::WHITE)
                    }
                    crate::emerge::AutoBlinderPhase::Recover => {
                        (egui::Color32::from_rgb(40, 200, 80), egui::Color32::BLACK)
                    }
                    _ => (egui::Color32::from_rgb(60, 60, 60), egui::Color32::WHITE),
                }
            };
            let blinder_response = ui.add(
                egui::Button::new(egui::RichText::new("BLINDER").color(blinder_text).small())
                    .fill(blinder_fill)
                    .sense(egui::Sense::click_and_drag())
            );
            if let Some(a) = handle_trigger(&blinder_response, blinder_held, TriggerButton::Blinder) {
                *action = a;
            }

            let macro_normal = macro_phase == crate::emerge::AutoBlinderPhase::Normal;
            let buildup_fill = if macro_normal {
                egui::Color32::from_rgb(60, 60, 60)
            } else {
                egui::Color32::from_rgb(255, 200, 0)
            };
            let buildup_response = ui.add(
                egui::Button::new(egui::RichText::new("BUILDUP").color(egui::Color32::WHITE).small())
                    .fill(buildup_fill)
            );
            if buildup_response.clicked() && macro_normal {
                *action = OscAction::ForceBuildup;
            }
        });

        // Second row: the posture controls (AUTO arm + STYLE) and fog — the
        // momentary triggers above stay an uncluttered strip of their own.
        ui.horizontal(|ui| {
            // Master arm for the AUTO buildup macro — disarmed shows the bare
            // show-brain lanes (no arc-fired BlindersHold/Pump; drops are
            // still perceived so the drop-gated lanes stay live). Disarming
            // releases a running arc-entered hold immediately. The BUILDUP
            // force button above and its MIDI binding stay live either way,
            // and a forced hold plays out.
            let auto_armed = state.config.enable_buildup_macro;
            let (auto_fill, auto_text) = if auto_armed {
                (egui::Color32::from_rgb(40, 140, 90), egui::Color32::WHITE)
            } else {
                (egui::Color32::from_rgb(50, 50, 50), egui::Color32::from_rgb(140, 140, 140))
            };
            let auto_response = ui.add(
                egui::Button::new(egui::RichText::new("AUTO").color(auto_text).small())
                    .fill(auto_fill)
            ).on_hover_text(if auto_armed {
                "Auto buildup macro ARMED — tap to disarm (releases a running auto hold; bare EMeRGE lanes; manual BUILDUP stays live)"
            } else {
                "Auto buildup macro OFF — tap to re-arm the arc-driven sequence"
            });
            if auto_response.clicked() {
                let mut new_config = state.config.clone();
                new_config.enable_buildup_macro = !auto_armed;
                *action = OscAction::UpdateConfig(new_config);
            }

            // Operator style posture — a live performance control at the same
            // altitude as the AUTO arm. Chill = never lasers/strobes, slow
            // scans, measured color; Energetic = lasers + drop-only strobes,
            // no 8Hz hold; Banger = full vocabulary floored high (silence
            // still reads dark); Auto = EMeRGE flips between the three from
            // the music (measured: sustained evidence + dwell, drops promote
            // instantly). MIDI Style* bindings mutate the same config field,
            // so this dropdown reflects them automatically.
            let current_style = state.config.lighting_style;
            // Live display: under Auto, show what the resolver has flipped
            // to ("Auto · Chill"), tinted by posture, so it's visually easy
            // to watch the engine move between low periods and big moments.
            let posture_tint = |style: crate::emerge::LightingStyle| match style {
                crate::emerge::LightingStyle::Chill => egui::Color32::from_rgb(130, 180, 255),
                crate::emerge::LightingStyle::Energetic => egui::Color32::from_rgb(120, 220, 140),
                crate::emerge::LightingStyle::Banger => egui::Color32::from_rgb(255, 150, 70),
                crate::emerge::LightingStyle::Auto => egui::Color32::WHITE,
            };
            let (display_text, display_tint) =
                if current_style == crate::emerge::LightingStyle::Auto {
                    (
                        format!("Auto · {}", state.resolved_style.as_str()),
                        posture_tint(state.resolved_style),
                    )
                } else {
                    (current_style.as_str().to_string(), posture_tint(current_style))
                };
            let mut selected_style = current_style;
            egui::ComboBox::from_id_salt("lighting_style")
                .selected_text(
                    egui::RichText::new(display_text).color(display_tint).small(),
                )
                .width(118.0)
                .show_ui(ui, |ui| {
                    for style in crate::emerge::LightingStyle::ALL {
                        ui.selectable_value(&mut selected_style, style, style.as_str());
                    }
                });
            if selected_style != current_style {
                let mut new_config = state.config.clone();
                new_config.lighting_style = selected_style;
                *action = OscAction::UpdateConfig(new_config);
            }
            // Dynamics: how big the passage is against the loudest one of the
            // last few minutes. The rig settles under it, whatever the style.
            let dynamics = state.dynamics.clamp(0.0, 1.0);
            let dynamics_tint = if dynamics < 0.38 {
                egui::Color32::from_rgb(130, 180, 255)
            } else if dynamics < 0.82 {
                egui::Color32::from_rgb(120, 220, 140)
            } else {
                egui::Color32::from_rgb(255, 150, 70)
            };
            ui.label(
                egui::RichText::new(format!("{:>3.0}%", dynamics * 100.0))
                    .color(dynamics_tint)
                    .small()
                    .monospace(),
            )
            .on_hover_text(
                "Dynamics: how big this passage is against the loudest one of the last \
                 few minutes. Around 25% is a breakdown, 50% a bass-cut blend, 70% drums \
                 on their own, 100% everything in. Brightness, the number of beams lit \
                 and how far the heads move all settle under it.",
            );

            let fog_held = state.trigger_fog_held;
            let fog_latched = state.trigger_fog_latched;
            // Visually active when EITHER held or latched. Latched stays a
            // slightly different hue (deeper purple) so the operator can tell
            // at a glance that fog is running hands-free vs being held.
            let fog_fill = if fog_held {
                egui::Color32::from_rgb(180, 120, 255)
            } else if fog_latched {
                egui::Color32::from_rgb(120, 70, 200)
            } else {
                egui::Color32::from_rgb(60, 60, 60)
            };
            let fog_response = ui.add(
                egui::Button::new(egui::RichText::new("FOG").color(egui::Color32::WHITE).small())
                    .fill(fog_fill)
                    .sense(egui::Sense::click_and_drag())
            );
            if let Some(a) = handle_trigger(&fog_response, fog_held, TriggerButton::Fog) {
                *action = a;
            }
            // Latch toggle next to FOG — small lock-style button. Tap once
            // to keep fog running hands-free; tap again to release.
            let latch_label = if fog_latched { "🔒" } else { "🔓" };
            let latch_fill = if fog_latched {
                egui::Color32::from_rgb(120, 70, 200)
            } else {
                egui::Color32::from_rgb(50, 50, 50)
            };
            let latch_response = ui.add(
                egui::Button::new(egui::RichText::new(latch_label).small().color(egui::Color32::WHITE))
                    .fill(latch_fill)
            ).on_hover_text(if fog_latched {
                "Fog latch ON — tap to release"
            } else {
                "Latch fog ON (hands-free hold)"
            });
            if latch_response.clicked() {
                *action = OscAction::ToggleFogLatch;
            }
        });

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Strobe Rate:").small().color(egui::Color32::GRAY));
            let mut strobe_rate = state.trigger_strobe_rate;
            if ui.add(egui::Slider::new(&mut strobe_rate, 1.0..=20.0).suffix("Hz").show_value(false)).changed() {
                *action = OscAction::SetStrobeRate(strobe_rate);
            }
            ui.label(egui::RichText::new(format!("{:.0}Hz", strobe_rate)).small());
        });
    }

    /// Show Hue Entertainment subsection within Lighting
    fn show_hue_subsection(&mut self, ui: &mut egui::Ui, hue_state: Option<&crate::hue::HueState>) -> HueAction {
        let mut action = HueAction::None;

        let state = match hue_state {
            Some(s) => s,
            None => {
                ui.label(egui::RichText::new("Hue not initialized").color(egui::Color32::GRAY).italics());
                return action;
            }
        };

        // Initialize UI fields from state on first render
        if !self.hue_initialized {
            self.hue_enabled = state.enabled;
            if !state.bridge_ip.is_empty() {
                self.hue_bridge_ip_input = state.bridge_ip.clone();
            }
            // Find the matching area index
            if !state.entertainment_area_id.is_empty() {
                if let Some(idx) = state.areas.iter().position(|(id, _)| id == &state.entertainment_area_id) {
                    self.hue_area_index = idx;
                }
            }
            // Initialize protocol version index (V2=0, V1=1 matching HueProtocolVersion::all())
            self.hue_protocol_index = match state.protocol_version {
                crate::hue::HueProtocolVersion::V2 => 0,
                crate::hue::HueProtocolVersion::V1 => 1,
            };
            // Initialize mapping mode index (Reactive=0, MultiBand=1 matching HueMappingMode::all())
            self.hue_mapping_mode_index = match state.mapping_mode {
                crate::hue::HueMappingMode::Reactive => 0,
                crate::hue::HueMappingMode::MultiBand => 1,
            };
            self.hue_initialized = true;

            // Auto-fetch areas on startup if paired but areas list is empty
            if state.areas.is_empty() && matches!(&state.pairing, crate::hue::PairingState::Paired) {
                action = HueAction::FetchAreas;
            }
        }

        ui.add_space(4.0);

        // Enable/Disable toggle
        ui.horizontal(|ui| {
            let mut enabled = self.hue_enabled;
            if ui.checkbox(&mut enabled, "").changed() {
                self.hue_enabled = enabled;
                action = HueAction::SetEnabled(enabled);
            }
            let status_text = if self.hue_enabled { "Enabled" } else { "Disabled" };
            let status_color = if self.hue_enabled {
                egui::Color32::from_rgb(100, 200, 100)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(status_text).color(status_color));
        });

        // Protocol version selector
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Protocol:").small());
            let versions = crate::hue::HueProtocolVersion::all();
            let current_label = versions.get(self.hue_protocol_index)
                .map(|v| v.as_str())
                .unwrap_or("v2 (Recommended)");
            let old_index = self.hue_protocol_index;
            egui::ComboBox::from_id_salt("hue_protocol")
                .selected_text(current_label)
                .width(130.0)
                .show_ui(ui, |ui| {
                    for (i, version) in versions.iter().enumerate() {
                        ui.selectable_value(&mut self.hue_protocol_index, i, version.as_str());
                    }
                });
            if self.hue_protocol_index != old_index {
                if let Some(version) = versions.get(self.hue_protocol_index) {
                    action = HueAction::SetProtocolVersion(*version);
                }
            }
        });

        // Mapping mode selector
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Mapping:").small());
            let modes = crate::hue::HueMappingMode::all();
            let current_label = modes.get(self.hue_mapping_mode_index)
                .map(|m| m.as_str())
                .unwrap_or("Reactive (Compeller)");
            let old_index = self.hue_mapping_mode_index;
            egui::ComboBox::from_id_salt("hue_mapping_mode")
                .selected_text(current_label)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, mode) in modes.iter().enumerate() {
                        ui.selectable_value(&mut self.hue_mapping_mode_index, i, mode.as_str());
                    }
                });
            if self.hue_mapping_mode_index != old_index {
                if let Some(mode) = modes.get(self.hue_mapping_mode_index) {
                    action = HueAction::SetMappingMode(*mode);
                }
            }
        });

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // Bridge IP input
        ui.label(egui::RichText::new("Bridge").small().color(egui::Color32::GRAY));
        ui.horizontal(|ui| {
            ui.label("IP:");
            ui.add(
                egui::TextEdit::singleline(&mut self.hue_bridge_ip_input)
                    .desired_width(120.0)
                    .hint_text("192.168.1.x")
            );

            if ui.button("Apply").clicked() && !self.hue_bridge_ip_input.is_empty() {
                action = HueAction::SetBridgeIp(self.hue_bridge_ip_input.clone());
            }
        });

        ui.add_space(4.0);

        // Pairing status & button
        ui.horizontal(|ui| {
            let (pairing_text, pairing_color) = match &state.pairing {
                crate::hue::PairingState::Idle => ("Not Paired", egui::Color32::GRAY),
                crate::hue::PairingState::WaitingForButton => ("Press bridge button...", egui::Color32::from_rgb(255, 200, 100)),
                crate::hue::PairingState::Paired => ("Paired", egui::Color32::from_rgb(100, 200, 100)),
                crate::hue::PairingState::Failed(_) => ("Failed", egui::Color32::from_rgb(200, 100, 100)),
            };

            let icon = match &state.pairing {
                crate::hue::PairingState::Paired => egui_phosphor::regular::LINK,
                crate::hue::PairingState::WaitingForButton => egui_phosphor::regular::SPINNER,
                _ => egui_phosphor::regular::LINK_BREAK,
            };
            ui.label(egui::RichText::new(icon).size(14.0).color(pairing_color));
            ui.label(egui::RichText::new(pairing_text).small().color(pairing_color));

            if matches!(&state.pairing, crate::hue::PairingState::Idle | crate::hue::PairingState::Failed(_)) {
                if ui.small_button("Pair").clicked() {
                    action = HueAction::StartPairing;
                }
            }
        });

        // Error display
        if let Some(ref err) = state.error {
            ui.label(egui::RichText::new(err).small().color(egui::Color32::from_rgb(200, 100, 100)));
        }

        // Only show remaining controls if paired
        if !matches!(&state.pairing, crate::hue::PairingState::Paired) {
            return action;
        }

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // Entertainment area dropdown
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Area:").small());

            if state.areas.is_empty() {
                ui.label(egui::RichText::new("(none found)").small().color(egui::Color32::GRAY));
                if ui.small_button("Refresh").clicked() {
                    action = HueAction::FetchAreas;
                }
            } else {
                let area_names: Vec<&str> = state.areas.iter().map(|(_, n)| n.as_str()).collect();
                let current_name = area_names.get(self.hue_area_index).unwrap_or(&"Select...");
                let old_index = self.hue_area_index;
                egui::ComboBox::from_id_salt("hue_area")
                    .selected_text(*current_name)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in area_names.iter().enumerate() {
                            ui.selectable_value(&mut self.hue_area_index, i, *name);
                        }
                    });

                if self.hue_area_index != old_index {
                    if let Some((id, _)) = state.areas.get(self.hue_area_index) {
                        action = HueAction::SelectArea(id.clone());
                    }
                }

                // Auto-select displayed area if config has none committed yet
                if state.entertainment_area_id.is_empty() {
                    if let Some((id, _)) = state.areas.get(self.hue_area_index) {
                        action = HueAction::SelectArea(id.clone());
                    }
                }

                // Keep ComboBox index synced with the actual selected area
                // (handles DB restore + auto-select when areas arrive after init)
                if !state.entertainment_area_id.is_empty() {
                    if let Some(idx) = state.areas.iter().position(|(id, _)| id == &state.entertainment_area_id) {
                        if self.hue_area_index != idx {
                            self.hue_area_index = idx;
                        }
                    }
                }

                if ui.small_button(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Refresh areas").clicked() {
                    action = HueAction::FetchAreas;
                }
            }
        });

        // Light assignments (only show when area is selected and lights are available)
        if !state.lights.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Lights").small().color(egui::Color32::GRAY));

            for assignment in &state.light_assignments {
                let channel_id = assignment.channel_id;
                ui.horizontal(|ui| {
                    // Light name
                    ui.label(egui::RichText::new(&assignment.name).small());

                    // Form factor dropdown
                    let current_ff = assignment.form_factor();
                    let mut selected_ff = current_ff;
                    egui::ComboBox::from_id_salt(format!("hue_ff_{}", channel_id))
                        .selected_text(current_ff.as_str())
                        .width(60.0)
                        .show_ui(ui, |ui| {
                            for ff in crate::lighting::FormFactor::all() {
                                ui.selectable_value(&mut selected_ff, *ff, ff.as_str());
                            }
                        });
                    if selected_ff != current_ff {
                        // Set override (or clear if it matches auto-detected)
                        let override_val = if selected_ff == assignment.auto_form_factor {
                            None
                        } else {
                            Some(selected_ff)
                        };
                        action = HueAction::SetLightFormFactor(channel_id, override_val);
                    }

                    // Role dropdown
                    let current_role = assignment.role;
                    let roles = crate::hue::LightRole::all();
                    let mut selected_role = current_role;
                    egui::ComboBox::from_id_salt(format!("hue_role_{}", channel_id))
                        .selected_text(current_role.as_str())
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for role in roles {
                                ui.selectable_value(&mut selected_role, *role, role.as_str());
                            }
                        });
                    if selected_role != current_role {
                        action = HueAction::SetLightRole(channel_id, selected_role);
                    }

                    // Capability note
                    ui.label(egui::RichText::new(crate::lighting::DeviceCapabilities::hue().note())
                        .small().color(egui::Color32::DARK_GRAY));
                });
            }
        }

        ui.add_space(4.0);

        // Connection status & connect button
        ui.horizontal(|ui| {
            let (status_icon, status_color, status_text) = if state.streaming {
                (egui_phosphor::regular::BROADCAST, egui::Color32::from_rgb(100, 255, 100), "Streaming")
            } else if state.connected {
                (egui_phosphor::regular::PLUGS_CONNECTED, egui::Color32::from_rgb(200, 200, 100), "Connected")
            } else if state.connecting {
                (egui_phosphor::regular::SPINNER, egui::Color32::from_rgb(200, 200, 100), "Connecting...")
            } else {
                (egui_phosphor::regular::PLUGS, egui::Color32::GRAY, "Disconnected")
            };

            ui.label(egui::RichText::new(status_icon).size(14.0).color(status_color));
            ui.label(egui::RichText::new(status_text).small().color(status_color));

            if state.streaming || state.connected {
                if ui.small_button("Disconnect").clicked() {
                    action = HueAction::Disconnect;
                }
            } else if !state.connecting {
                let has_area = !state.entertainment_area_id.is_empty();
                ui.add_enabled_ui(has_area, |ui| {
                    if ui.small_button("Connect").clicked() {
                        action = HueAction::Connect;
                    }
                });
                if !has_area {
                    ui.label(egui::RichText::new("Select an area first").small().color(egui::Color32::GRAY));
                }
            }
        });

        ui.add_space(4.0);

        action
    }

    /// Show WLED subsection within Lighting
    fn show_wled_subsection(&mut self, ui: &mut egui::Ui, wled_state: Option<&crate::wled::WledState>, hue_bridge_ip: &str) -> WledAction {
        let mut action = WledAction::None;

        let state = match wled_state {
            Some(s) => s,
            None => {
                ui.label(egui::RichText::new("WLED not initialized").color(egui::Color32::GRAY).italics());
                return action;
            }
        };

        // Initialize UI fields from state on first render
        if !self.wled_initialized {
            self.wled_enabled = state.enabled;
            self.wled_initialized = true;
        }

        // Enable toggle
        ui.horizontal(|ui| {
            let prev = self.wled_enabled;
            ui.checkbox(&mut self.wled_enabled, "Enable WLED");
            if self.wled_enabled != prev {
                action = WledAction::SetEnabled(self.wled_enabled);
            }
        });

        if !self.wled_enabled {
            return action;
        }

        ui.add_space(4.0);

        // Add strip by IP
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Strip IP:").small());
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.wled_strip_ip_input)
                    .desired_width(120.0)
                    .hint_text("192.168.1.x")
            );
            if ui.button("Add Strip").clicked() || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                let ip = self.wled_strip_ip_input.trim().to_string();
                if !ip.is_empty() {
                    // Validate: reject if IP matches Hue bridge
                    if !hue_bridge_ip.is_empty() && ip == hue_bridge_ip {
                        self.wled_ip_error = Some(
                            "That IP is your Hue Bridge. WLED strips are separate devices with their own IP.".to_string()
                        );
                    }
                    // Validate: reject duplicate WLED IPs
                    else if state.strips.iter().any(|s| s.ip == ip) {
                        self.wled_ip_error = Some(
                            format!("A WLED strip at {} is already configured.", ip)
                        );
                    } else {
                        self.wled_ip_error = None;
                        action = WledAction::AddStrip(ip);
                        self.wled_strip_ip_input.clear();
                    }
                }
            }
        });

        // Show inline validation error
        if let Some(ref err) = self.wled_ip_error {
            ui.label(egui::RichText::new(err).small().color(egui::Color32::from_rgb(220, 100, 100)));
        }

        ui.add_space(4.0);

        // Strip list
        if state.strips.is_empty() {
            ui.label(egui::RichText::new("No strips configured").small().color(egui::Color32::GRAY).italics());
        } else {
            for (idx, strip) in state.strips.iter().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        // Strip name / IP
                        ui.label(egui::RichText::new(&strip.name).small().strong());
                        ui.label(egui::RichText::new(format!("({} LEDs)", strip.led_count)).small().color(egui::Color32::GRAY));

                        // Form factor dropdown
                        let current_ff = strip.form_factor;
                        let mut selected_ff = current_ff;
                        egui::ComboBox::from_id_salt(format!("wled_ff_{}", idx))
                            .selected_text(current_ff.as_str())
                            .width(60.0)
                            .show_ui(ui, |ui| {
                                for ff in crate::lighting::FormFactor::all() {
                                    ui.selectable_value(&mut selected_ff, *ff, ff.as_str());
                                }
                            });
                        if selected_ff != current_ff {
                            let override_val = if selected_ff == crate::lighting::FormFactor::Strip {
                                None // Strip is the default for WLED, clear override
                            } else {
                                Some(selected_ff)
                            };
                            action = WledAction::SetStripFormFactor(idx, override_val);
                        }

                        // Mode dropdown
                        let current_mode = strip.mode;
                        egui::ComboBox::from_id_salt(format!("wled_mode_{}", idx))
                            .selected_text(current_mode.as_str())
                            .width(80.0)
                            .show_ui(ui, |ui| {
                                for mode in crate::wled::StripMode::all() {
                                    if ui.selectable_label(current_mode == *mode, mode.as_str()).clicked() {
                                        action = WledAction::SetStripMode(idx, *mode);
                                    }
                                }
                            });

                        // Capability note
                        ui.label(egui::RichText::new(crate::lighting::DeviceCapabilities::wled(strip.led_count).note())
                            .small().color(egui::Color32::DARK_GRAY));
                    });

                    ui.horizontal(|ui| {
                        // Enable checkbox
                        let mut strip_enabled = strip.enabled;
                        if ui.checkbox(&mut strip_enabled, "").changed() {
                            action = WledAction::SetStripEnabled(idx, strip_enabled);
                        }

                        // Connection state + connect/disconnect
                        match &strip.connection {
                            crate::wled::StripConnectionState::Connected => {
                                ui.label(egui::RichText::new("Connected").small().color(egui::Color32::GREEN));
                                if ui.small_button("Disconnect").clicked() {
                                    action = WledAction::Disconnect(idx);
                                }
                            }
                            crate::wled::StripConnectionState::Connecting => {
                                ui.label(egui::RichText::new("Connecting...").small().color(egui::Color32::YELLOW));
                            }
                            crate::wled::StripConnectionState::Disconnected => {
                                ui.label(egui::RichText::new("Disconnected").small().color(egui::Color32::GRAY));
                                if ui.small_button("Connect")
                                    .on_hover_text("Connects directly to WLED over UDP. This is a separate network device from the Hue Bridge.")
                                    .clicked()
                                {
                                    action = WledAction::Connect(idx);
                                }
                            }
                            crate::wled::StripConnectionState::Error(e) => {
                                ui.label(egui::RichText::new(format!("Error: {}", e)).small().color(egui::Color32::RED));
                                if ui.small_button("Retry").clicked() {
                                    action = WledAction::Connect(idx);
                                }
                            }
                        }

                        // Remove button
                        if ui.small_button("X").clicked() {
                            action = WledAction::RemoveStrip(idx);
                        }
                    });
                });

                ui.add_space(2.0);
            }

            // Connect All / Disconnect All
            ui.horizontal(|ui| {
                if ui.button("Connect All")
                    .on_hover_text("Connects to each WLED strip directly over Wi-Fi (UDP).")
                    .clicked()
                {
                    action = WledAction::ConnectAll;
                }
                if ui.button("Disconnect All").clicked() {
                    action = WledAction::DisconnectAll;
                }
            });
        }

        ui.add_space(4.0);

        action
    }

    /// Show the vision section with tracking influence controls
    fn show_vision_section(&mut self, ui: &mut egui::Ui, state: Option<&CameraState>) -> CameraAction {
        let mut action = CameraAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Vision").strong().size(14.0)
        )
        .default_open(self.vision_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            if let Some(state) = state {
                // Enable/disable tracking
                ui.horizontal(|ui| {
                    let (icon, color) = if state.enabled {
                        (egui_phosphor::regular::VIDEO_CAMERA, egui::Color32::from_rgb(100, 255, 100))
                    } else {
                        (egui_phosphor::regular::VIDEO_CAMERA_SLASH, egui::Color32::GRAY)
                    };
                    ui.label(egui::RichText::new(icon).color(color));

                    if state.enabled {
                        ui.label(format!("{} camera(s) active", state.enabled_cameras.len()));
                        if ui.small_button("Stop").clicked() {
                            action = CameraAction::Stop;
                        }
                    } else {
                        ui.label("Vision disabled");
                        if ui.small_button("Start").clicked() {
                            action = CameraAction::Start;
                        }
                    }

                    if ui.small_button(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Scan for cameras").clicked() {
                        action = CameraAction::ScanCameras;
                    }
                });

                ui.add_space(4.0);

                // Available cameras list - fixed height to prevent menu jumping
                // Shows up to 3 cameras (~75px), scrolls if more
                const CAMERA_LIST_HEIGHT: f32 = 75.0;
                ui.label(egui::RichText::new("Cameras (Tracking Influence)").small().strong());
                ui.add_space(2.0);

                // Ensure all cameras have UI state (before fixed container)
                for cam in &state.available_cameras {
                    if !self.camera_ui_states.contains_key(&cam.index) {
                        self.camera_ui_states.insert(
                            cam.index,
                            CameraUiState::new(cam.index, cam.name.clone()),
                        );
                    }
                }

                let midi_learn_armed = self.midi_learn_armed;
                let mut needs_dirty = false;
                let mut pending_action: Option<MidiAction> = None;

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), CAMERA_LIST_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        if !state.available_cameras.is_empty() {
                            for cam in &state.available_cameras {
                                let is_program = cam.index == state.program_camera;
                                let cam_index = cam.index;

                                // Read camera state first
                                let (enabled, vis_q, tracking_confidence) = self.camera_ui_states
                                    .get(&cam_index)
                                    .map(|s| (s.enabled, s.vis_q, s.tracking_confidence))
                                    .unwrap_or((true, 16, 0.0));

                                ui.horizontal(|ui| {
                                    // Enable toggle button
                                    let icon = if enabled {
                                        egui_phosphor::regular::VIDEO_CAMERA
                                    } else {
                                        egui_phosphor::regular::VIDEO_CAMERA_SLASH
                                    };
                                    let color = if enabled {
                                        egui::Color32::from_rgb(100, 255, 100)
                                    } else {
                                        egui::Color32::GRAY
                                    };

                                    let enable_btn = ui.add(
                                        egui::Button::new(egui::RichText::new(icon).color(color))
                                            .fill(egui::Color32::TRANSPARENT)
                                            .min_size(egui::vec2(20.0, 20.0))
                                    );

                                    if enable_btn.clicked() {
                                        if !midi_learn_armed {
                                            if let Some(cam_state) = self.camera_ui_states.get_mut(&cam_index) {
                                                cam_state.toggle_enable();
                                                needs_dirty = true;
                                            }
                                        } else {
                                            pending_action = Some(MidiAction::SetLearnTarget(
                                                ControlTarget::CameraEnable(cam_index)
                                            ));
                                        }
                                    }
                                    enable_btn.on_hover_text("Toggle camera tracking");

                                    // Camera name (truncated)
                                    let name = if cam.name.len() > 12 {
                                        format!("{}…", &cam.name[..11])
                                    } else {
                                        cam.name.clone()
                                    };
                                    let name_color = if is_program && state.enabled {
                                        egui::Color32::WHITE
                                    } else {
                                        egui::Color32::LIGHT_GRAY
                                    };
                                    ui.label(egui::RichText::new(&name).size(11.0).color(name_color));

                                    // Visibility slider (tracking influence, NOT video opacity)
                                    let old_vis = vis_q;
                                    let mut norm = vis_q as f32 / 16.0;

                                    // Yellow pulsing slider when in MIDI learn mode
                                    let slider_response = if midi_learn_armed {
                                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                                        let mut style = (*ui.ctx().style()).clone();
                                        style.visuals.widgets.inactive.bg_fill = yellow;
                                        style.visuals.widgets.hovered.bg_fill = yellow;
                                        style.visuals.widgets.active.bg_fill = yellow;
                                        ui.ctx().set_style(style.clone());

                                        let response = ui.add(egui::Slider::new(&mut norm, 0.0..=1.0)
                                            .show_value(false)
                                            .step_by(1.0 / 16.0))
                                            .on_hover_text("Click to map Camera Visibility to MIDI fader");

                                        ui.ctx().set_style(egui::Style::default());
                                        response
                                    } else {
                                        ui.add(egui::Slider::new(&mut norm, 0.0..=1.0)
                                            .show_value(false)
                                            .step_by(1.0 / 16.0))
                                    };

                                    // MIDI learn for visibility slider - check both drag_started and clicked
                                    if midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                                        pending_action = Some(MidiAction::SetLearnTarget(
                                            ControlTarget::CameraVisibility(cam_index)
                                        ));
                                    } else if slider_response.changed() {
                                        let new_step = (norm * 16.0).round() as u8;
                                        if let Some(cam_state) = self.camera_ui_states.get_mut(&cam_index) {
                                            if cam_state.set_visibility(new_step, false) {
                                                needs_dirty = true;
                                            }
                                        }
                                    }

                                    slider_response.on_hover_text(
                                        "Tracking influence (controls accent probability, NOT video opacity)"
                                    );

                                    // Step indicator
                                    ui.label(
                                        egui::RichText::new(format!("{:2}", vis_q))
                                            .size(10.0)
                                            .monospace()
                                    );

                                    // Confidence indicator (colored dot)
                                    let conf_color = egui::Color32::from_rgb(
                                        (255.0 * (1.0 - tracking_confidence)) as u8,
                                        (255.0 * tracking_confidence) as u8,
                                        50,
                                    );
                                    ui.label(egui::RichText::new("●").size(8.0).color(conf_color))
                                        .on_hover_text(format!("Tracking confidence: {:.0}%", tracking_confidence * 100.0));

                                    // Program camera indicator/button
                                    if is_program && state.enabled {
                                        ui.label(
                                            egui::RichText::new(egui_phosphor::regular::BROADCAST)
                                                .size(12.0)
                                                .color(egui::Color32::from_rgb(255, 100, 100))
                                        ).on_hover_text("Program camera");
                                    } else if state.enabled {
                                        if ui.small_button("PGM").on_hover_text("Set as program camera").clicked() {
                                            action = CameraAction::SetProgramCamera(cam.index);
                                        }
                                    }
                                });
                            }
                        } else {
                            ui.label(egui::RichText::new("No cameras found. Click refresh to scan.").small().color(egui::Color32::GRAY).italics());
                        }
                    },
                );

                // Apply deferred state changes (after fixed container)
                if needs_dirty {
                    self.mark_dirty();
                }
                if let Some(midi_action) = pending_action {
                    self.pending_midi_action = midi_action;
                }

                // Active track display - fixed height to prevent jumping
                const ACTIVE_TRACK_HEIGHT: f32 = 28.0;
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ACTIVE_TRACK_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        if state.enabled && state.output.active {
                            ui.horizontal(|ui| {
                                // Active indicator
                                ui.label(egui::RichText::new(egui_phosphor::regular::CROSSHAIR)
                                    .color(egui::Color32::from_rgb(100, 255, 100)));

                                // Class name and confidence
                                let class_name = state.output.class_name.as_deref().unwrap_or("Unknown");
                                ui.label(egui::RichText::new(class_name).strong());
                                ui.label(format!("{:.0}%", state.output.confidence * 100.0));
                                ui.label(egui::RichText::new(format!("G{}", state.output.anchor_index + 1))
                                    .small()
                                    .color(egui::Color32::GRAY));

                                // Preview button
                                if ui.small_button(egui_phosphor::regular::EYE).on_hover_text("Open camera preview").clicked() {
                                    self.show_camera_preview = true;
                                }
                            });
                        }
                    },
                );

                // Filter tags section - fixed height to prevent menu jumping
                const FILTER_SECTION_HEIGHT: f32 = 55.0;  // Fixed height for filter tags + input
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Filter").small().strong());

                let mut tag_to_remove: Option<usize> = None;
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), FILTER_SECTION_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        // Show existing filter tags as chips with X buttons
                        if !self.camera_filter_tags.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for (i, tag) in self.camera_filter_tags.iter().enumerate() {
                                    // Tag chip with X button
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        let chip = egui::Frame::none()
                                            .fill(egui::Color32::from_rgb(60, 80, 100))
                                            .rounding(4.0)
                                            .inner_margin(egui::Margin::symmetric(6.0, 2.0));
                                        chip.show(ui, |ui| {
                                            ui.label(egui::RichText::new(tag).small().color(egui::Color32::WHITE));
                                            if ui.small_button("×").clicked() {
                                                tag_to_remove = Some(i);
                                            }
                                        });
                                    });
                                }
                            });
                        }

                        // Input box to add new tags
                        ui.horizontal(|ui| {
                            let hint = if self.camera_filter_tags.is_empty() {
                                "Add filter (e.g. face, hand)"
                            } else {
                                "Add another..."
                            };

                            let input_response = ui.add(
                                egui::TextEdit::singleline(&mut self.camera_filter_input)
                                    .hint_text(hint)
                                    .desired_width(140.0)
                            );

                            // Press Enter to add tag
                            if input_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let new_tag = self.camera_filter_input.trim().to_lowercase();
                                if !new_tag.is_empty() && !self.camera_filter_tags.contains(&new_tag) {
                                    self.camera_filter_tags.push(new_tag);
                                    let filter_str = self.camera_filter_tags.join(",");
                                    action = CameraAction::SetTrackFilter(filter_str);
                                }
                                self.camera_filter_input.clear();
                            }

                            // Show matching classes from current detections in a popup when typing
                            if !self.camera_filter_input.is_empty() {
                                let search_lower = self.camera_filter_input.to_lowercase();
                                // Get unique class names from currently detected tracks
                                let mut detected_classes: Vec<String> = state.preview.tracks
                                    .iter()
                                    .map(|t| t.class_name.clone())
                                    .collect();
                                detected_classes.sort();
                                detected_classes.dedup();

                                let matches: Vec<&str> = detected_classes
                                    .iter()
                                    .map(|s| s.as_str())
                                    .filter(|c| c.to_lowercase().contains(&search_lower))
                                    .filter(|c| !self.camera_filter_tags.iter().any(|t| t == *c))
                                    .take(8)
                                    .collect();

                                if !matches.is_empty() {
                                    egui::popup_below_widget(ui, input_response.id.with("class_popup"), &input_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                                        ui.set_min_width(120.0);
                                        for class in &matches {
                                            if ui.selectable_label(false, *class).clicked() {
                                                if !self.camera_filter_tags.contains(&class.to_string()) {
                                                    self.camera_filter_tags.push(class.to_string());
                                                    let filter_str = self.camera_filter_tags.join(",");
                                                    action = CameraAction::SetTrackFilter(filter_str);
                                                }
                                                self.camera_filter_input.clear();
                                            }
                                        }
                                    });
                                    if input_response.changed() || input_response.has_focus() {
                                        ui.memory_mut(|mem| mem.open_popup(input_response.id.with("class_popup")));
                                    }
                                }
                            }
                        });
                    },
                );

                // Remove tag if X was clicked (after the fixed container to avoid borrow issues)
                if let Some(idx) = tag_to_remove {
                    self.camera_filter_tags.remove(idx);
                    let filter_str = self.camera_filter_tags.join(",");
                    action = CameraAction::SetTrackFilter(filter_str);
                }


                // Show thumbnails for detected classes - fixed height to prevent menu jumping
                // Shows up to 2 thumbnails, with scrolling if more are detected
                const VISION_THUMBS_HEIGHT: f32 = 100.0;  // Fixed height for 2 thumbnails

                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), VISION_THUMBS_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        if state.enabled && !state.preview.tracks.is_empty() {
                            // Group tracks by class_name, keep best confidence per class
                            let mut best_per_class: std::collections::HashMap<&str, &crate::camera::state::TrackPreview> = std::collections::HashMap::new();
                            for track in &state.preview.tracks {
                                let entry = best_per_class.entry(&track.class_name).or_insert(track);
                                if track.confidence > entry.confidence {
                                    *entry = track;
                                }
                            }

                            // Sort by confidence (highest first)
                            let mut sorted_tracks: Vec<_> = best_per_class.values().cloned().collect();
                            sorted_tracks.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

                            // Show up to 2 thumbnails (fits in fixed height)
                            for track in sorted_tracks.iter().take(2) {
                                let is_active = state.output.active && track.id == state.output.track_id;

                                ui.horizontal(|ui| {
                                    let tex_id = track.id;

                                    // Get or create texture
                                    let texture = self.track_thumbnail_textures.entry(tex_id).or_insert_with(|| {
                                        let placeholder = vec![64u8; 32 * 32 * 3];
                                        let image = egui::ColorImage::from_rgb([32, 32], &placeholder);
                                        ui.ctx().load_texture(
                                            format!("sidebar_thumb_{}", tex_id),
                                            image,
                                            egui::TextureOptions::LINEAR,
                                        )
                                    });

                                    // Update texture with current thumbnail
                                    if track.thumbnail.len() == 32 * 32 * 3 {
                                        let image = egui::ColorImage::from_rgb([32, 32], &track.thumbnail);
                                        texture.set(image, egui::TextureOptions::LINEAR);
                                    }

                                    // Draw thumbnail - cyan border for active, yellow for others
                                    let thumb_size = egui::vec2(40.0, 40.0);
                                    let (rect, _) = ui.allocate_exact_size(thumb_size, egui::Sense::hover());

                                    let border_color = if is_active {
                                        egui::Color32::from_rgb(100, 255, 255) // Cyan for active
                                    } else {
                                        egui::Color32::from_rgb(255, 200, 50)  // Yellow for others
                                    };

                                    ui.painter().rect_stroke(
                                        rect.expand(1.0),
                                        3.0,
                                        egui::Stroke::new(2.0, border_color),
                                    );
                                    ui.painter().image(
                                        texture.id(),
                                        rect,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        egui::Color32::WHITE,
                                    );

                                    // Info next to thumbnail
                                    let text_color = if is_active {
                                        egui::Color32::from_rgb(100, 255, 255) // Cyan for active
                                    } else {
                                        egui::Color32::from_rgb(255, 200, 50)  // Yellow for others
                                    };
                                    ui.vertical(|ui| {
                                        ui.label(egui::RichText::new(&track.class_name).color(text_color).strong());
                                        ui.label(egui::RichText::new(format!("{:.0}%", track.confidence * 100.0)).small());
                                    });
                                });
                                ui.add_space(2.0);
                            }

                            // Show count if more than 2 detected
                            if sorted_tracks.len() > 2 {
                                ui.label(egui::RichText::new(format!("+{} more", sorted_tracks.len() - 2))
                                    .small()
                                    .color(egui::Color32::from_rgb(120, 120, 140)));
                            }
                        } else if state.enabled {
                            // No detections - show placeholder
                            ui.label(egui::RichText::new("No detections").small().color(egui::Color32::from_rgb(80, 80, 100)));
                        }
                    },
                );
            } else {
                ui.label(egui::RichText::new("Vision not initialized.").small().color(egui::Color32::GRAY).italics());
            }

            ui.add_space(4.0);
        });

        self.vision_expanded = header.fully_open();
        action
    }

    /// Show the cameras section for background media display
    fn show_cameras_section(&mut self, ui: &mut egui::Ui, state: Option<&CameraState>) -> CameraAction {
        let mut action = CameraAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Cameras").strong().size(14.0)
        )
        .default_open(self.cameras_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            if let Some(state) = state {
                if state.available_cameras.is_empty() {
                    ui.label(egui::RichText::new("No cameras found. Click refresh to scan.").small().color(egui::Color32::GRAY).italics());
                    ui.horizontal(|ui| {
                        if ui.small_button(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Scan for cameras").clicked() {
                            action = CameraAction::ScanCameras;
                        }
                    });
                } else {
                    // Ensure all cameras have media UI state
                    for cam in &state.available_cameras {
                        if !self.camera_media_ui_states.contains_key(&cam.index) {
                            self.camera_media_ui_states.insert(
                                cam.index,
                                crate::camera::CameraMediaState::new(cam.index, cam.name.clone()),
                            );
                        }
                    }

                    let midi_learn_armed = self.midi_learn_armed;
                    let mut needs_dirty = false;
                    let mut pending_action: Option<MidiAction> = None;

                    // Render each camera row with visibility slider
                    for cam in &state.available_cameras {
                        let cam_index = cam.index;

                        // Read camera media state
                        let (enabled, vis_q, decode_state) = self.camera_media_ui_states
                            .get(&cam_index)
                            .map(|s| (s.enabled, s.vis_q, s.decode_state))
                            .unwrap_or((false, 0, crate::camera::CameraDecodeState::Idle));

                        ui.horizontal(|ui| {
                            // Enable toggle button
                            let icon = if enabled {
                                egui_phosphor::regular::VIDEO_CAMERA
                            } else {
                                egui_phosphor::regular::VIDEO_CAMERA_SLASH
                            };
                            let color = if enabled {
                                egui::Color32::from_rgb(100, 255, 100)
                            } else {
                                egui::Color32::GRAY
                            };

                            let enable_btn = ui.add(
                                egui::Button::new(egui::RichText::new(icon).color(color))
                                    .fill(egui::Color32::TRANSPARENT)
                                    .min_size(egui::vec2(20.0, 20.0))
                            );

                            if enable_btn.clicked() {
                                if !midi_learn_armed {
                                    let was_enabled = self.camera_media_ui_states
                                        .get(&cam_index).map(|s| s.enabled).unwrap_or(false);
                                    // Disable all others first (exclusive: one camera at a time)
                                    if !was_enabled {
                                        for (_idx, cs) in &mut self.camera_media_ui_states {
                                            cs.enabled = false;
                                        }
                                    }
                                    if let Some(cam_state) = self.camera_media_ui_states.get_mut(&cam_index) {
                                        cam_state.toggle_enable();
                                        needs_dirty = true;
                                    }
                                } else {
                                    pending_action = Some(MidiAction::SetLearnTarget(
                                        ControlTarget::CameraMediaEnable(cam_index)
                                    ));
                                }
                            }
                            enable_btn.on_hover_text("Toggle camera background display");

                            // Camera name (truncated)
                            let name = if cam.name.len() > 12 {
                                format!("{}…", &cam.name[..11])
                            } else {
                                cam.name.clone()
                            };
                            ui.label(egui::RichText::new(&name).size(11.0).color(egui::Color32::LIGHT_GRAY));

                            // Visibility slider (alpha for background compositing)
                            let mut norm = vis_q as f32 / 16.0;

                            // Yellow pulsing slider when in MIDI learn mode
                            let slider_response = if midi_learn_armed {
                                let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                                let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                                let mut style = (*ui.ctx().style()).clone();
                                style.visuals.widgets.inactive.bg_fill = yellow;
                                style.visuals.widgets.hovered.bg_fill = yellow;
                                style.visuals.widgets.active.bg_fill = yellow;
                                ui.ctx().set_style(style.clone());

                                let response = ui.add(egui::Slider::new(&mut norm, 0.0..=1.0)
                                    .show_value(false)
                                    .step_by(1.0 / 16.0))
                                    .on_hover_text("Click to map Camera Media Visibility to MIDI fader");

                                ui.ctx().set_style(egui::Style::default());
                                response
                            } else {
                                ui.add(egui::Slider::new(&mut norm, 0.0..=1.0)
                                    .show_value(false)
                                    .step_by(1.0 / 16.0))
                            };

                            // MIDI learn for visibility slider - check both drag_started and clicked
                            if midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                                pending_action = Some(MidiAction::SetLearnTarget(
                                    ControlTarget::CameraMediaVisibility(cam_index)
                                ));
                            } else if slider_response.changed() {
                                let new_step = (norm * 16.0).round() as u8;
                                if let Some(cam_state) = self.camera_media_ui_states.get_mut(&cam_index) {
                                    cam_state.set_visibility_immediate(new_step);
                                    needs_dirty = true;
                                }
                            }

                            slider_response.on_hover_text(
                                "Background opacity (alpha compositing for visual layers)"
                            );

                            // Decode state indicator (colored dot)
                            let decode_color = match decode_state {
                                crate::camera::CameraDecodeState::Active => egui::Color32::from_rgb(100, 255, 100),
                                crate::camera::CameraDecodeState::Warm => egui::Color32::from_rgb(255, 200, 50),
                                crate::camera::CameraDecodeState::Idle => egui::Color32::GRAY,
                            };
                            let decode_label = match decode_state {
                                crate::camera::CameraDecodeState::Active => "Active",
                                crate::camera::CameraDecodeState::Warm => "Warming",
                                crate::camera::CameraDecodeState::Idle => "Idle",
                            };
                            ui.label(egui::RichText::new("●").size(8.0).color(decode_color))
                                .on_hover_text(format!("Decode state: {}", decode_label));
                        });
                    }

                    // Apply deferred state changes
                    if needs_dirty {
                        self.mark_dirty();
                    }
                    if let Some(midi_action) = pending_action {
                        self.pending_midi_action = midi_action;
                    }

                    // Camera layer luma key slider
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Cam Luma Key").size(11.0).color(egui::Color32::LIGHT_GRAY));

                        let slider_response = if midi_learn_armed {
                            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                            let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                            let mut style = (*ui.ctx().style()).clone();
                            style.visuals.widgets.inactive.bg_fill = yellow;
                            style.visuals.widgets.hovered.bg_fill = yellow;
                            style.visuals.widgets.active.bg_fill = yellow;
                            ui.ctx().set_style(style.clone());

                            let response = ui.add(egui::Slider::new(&mut self.params.camera_luma_key, 0.0..=1.0)
                                .show_value(false))
                                .on_hover_text("Click to map Camera Luma Key to MIDI fader");

                            ui.ctx().set_style(egui::Style::default());
                            response
                        } else {
                            ui.add(egui::Slider::new(&mut self.params.camera_luma_key, 0.0..=1.0)
                                .show_value(false))
                                .on_hover_text("Camera layer luma key — dark camera areas become transparent")
                        };

                        if midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                            self.pending_midi_action = MidiAction::SetLearnTarget(
                                ControlTarget::CameraLumaKey
                            );
                        } else if slider_response.changed() {
                            self.mark_dirty();
                        }
                    });
                }
            } else {
                ui.label(egui::RichText::new("Camera system not initialized.").small().color(egui::Color32::GRAY).italics());
            }

            ui.add_space(4.0);
        });

        self.cameras_expanded = header.fully_open();
        action
    }

    /// Show the MIDI controller section
    fn show_midi_section(&mut self, ui: &mut egui::Ui, state: &crate::app::MidiState) -> MidiAction {
        let mut action = MidiAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("MIDI Controllers").strong().size(14.0)
        )
        .default_open(self.midi_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Device status row
            ui.horizontal(|ui| {
                let (icon, color) = if state.connected_devices > 0 {
                    (egui_phosphor::regular::USB, egui::Color32::from_rgb(100, 255, 100))
                } else {
                    (egui_phosphor::regular::PLUGS, egui::Color32::GRAY)
                };
                ui.label(egui::RichText::new(icon).color(color));
                ui.label(format!("{} device{}",
                    state.connected_devices,
                    if state.connected_devices == 1 { "" } else { "s" }
                ));
                if ui.small_button(egui_phosphor::regular::ARROW_CLOCKWISE).on_hover_text("Rescan devices").clicked() {
                    action = MidiAction::RescanDevices;
                }
            });

            ui.add_space(4.0);

            // Learn mode button and status
            match &state.learn_state {
                LearnState::Off => {
                    ui.horizontal(|ui| {
                        // Simple "Learn" button - click then click a control
                        let learn_btn = ui.button(
                            egui::RichText::new(format!("{} Learn", egui_phosphor::regular::GRADUATION_CAP))
                        );
                        if learn_btn.clicked() {
                            action = MidiAction::ArmLearn;
                        }
                        learn_btn.on_hover_text("Click, then click any control to map it");

                        // Preset browser learn button (two-step)
                        let browser_btn = ui.button(
                            egui::RichText::new("★ Themes")
                                .color(egui::Color32::from_rgb(255, 215, 100))
                        );
                        if browser_btn.clicked() {
                            action = MidiAction::LearnPresetBrowserStart;
                        }
                        browser_btn.on_hover_text("Learn encoder for browsing pinned + favorite themes, then push button to select");

                        // Compel browser learn button (two-step)
                        let compel_btn = ui.button(
                            egui::RichText::new(format!("{} Compels", egui_phosphor::regular::VIDEO))
                                .color(egui::Color32::from_rgb(100, 200, 255))
                        );
                        if compel_btn.clicked() {
                            action = MidiAction::LearnCompelBrowserStart;
                        }
                        compel_btn.on_hover_text("Learn encoder for browsing all compels (videos), then push button to select");
                    });
                }
                LearnState::Armed { target, .. } => {
                    ui.horizontal(|ui| {
                        // Pulsing indicator
                        let pulse = (ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5;
                        let color = egui::Color32::from_rgb(
                            255,
                            (200.0 + 55.0 * pulse as f32) as u8,
                            (100.0 * pulse as f32) as u8
                        );

                        if let Some(t) = target {
                            // Check if this is preset browser learning with custom messages
                            let is_preset_encoder = matches!(t, ControlTarget::BrowsePresetEncoder);
                            let is_preset_load = matches!(t, ControlTarget::LoadPreset);
                            let is_compel_encoder = matches!(t, ControlTarget::BrowseCompelEncoder);
                            let is_compel_load = matches!(t, ControlTarget::LoadCompel);

                            if is_preset_encoder && state.learning_preset_browser {
                                // Step 1 of preset browser learning
                                ui.label(egui::RichText::new("★").color(egui::Color32::from_rgb(255, 215, 100)).size(16.0));
                                ui.label(egui::RichText::new("Step 1: Turn your encoder...")
                                    .color(color).strong());
                            } else if is_preset_load {
                                // Step 2 of preset browser learning
                                ui.label(egui::RichText::new("★").color(egui::Color32::from_rgb(100, 255, 100)).size(16.0));
                                ui.label(egui::RichText::new("Step 2: Push encoder to select...")
                                    .color(color).strong());
                            } else if is_compel_encoder && state.learning_compel_browser {
                                // Step 1 of compel browser learning
                                ui.label(egui::RichText::new(egui_phosphor::regular::VIDEO).color(egui::Color32::from_rgb(100, 200, 255)).size(16.0));
                                ui.label(egui::RichText::new("Step 1: Turn your encoder...")
                                    .color(color).strong());
                            } else if is_compel_load {
                                // Step 2 of compel browser learning
                                ui.label(egui::RichText::new(egui_phosphor::regular::VIDEO).color(egui::Color32::from_rgb(100, 255, 100)).size(16.0));
                                ui.label(egui::RichText::new("Step 2: Push encoder to select...")
                                    .color(color).strong());
                            } else {
                                // Regular learning
                                ui.label(egui::RichText::new(egui_phosphor::regular::WAVEFORM).color(color));
                                ui.label(egui::RichText::new(format!("Move MIDI control for {}...",
                                    t.display_name()))
                                    .color(color));
                            }
                        } else {
                            // Armed, waiting for UI click
                            ui.label(egui::RichText::new(egui_phosphor::regular::CURSOR_CLICK).color(color));
                            ui.label(egui::RichText::new("Click a control to map...")
                                .color(color));
                        }

                        if ui.small_button(egui_phosphor::regular::X).on_hover_text("Cancel").clicked() {
                            action = MidiAction::CancelLearn;
                        }
                    });
                }
                LearnState::Confirm { target, key, .. } => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                            .color(egui::Color32::from_rgb(100, 255, 100)));
                        ui.label(format!("Mapped {} → {}", key.description(), target.display_name()));
                    });
                }
            }

            ui.add_space(8.0);

            // Existing mappings
            if !state.mappings.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Active Mappings").strong().small());
                ui.add_space(4.0);

                let mut remove_idx = None;
                for (idx, mapping) in state.mappings.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(mapping.key.description()).small());
                        ui.label(egui::RichText::new(egui_phosphor::regular::CARET_RIGHT).small().color(egui::Color32::GRAY));
                        ui.label(egui::RichText::new(mapping.target.display_name()).small().strong());
                        if ui.small_button(egui_phosphor::regular::TRASH).on_hover_text("Remove").clicked() {
                            remove_idx = Some(idx);
                        }
                    });
                }
                if let Some(idx) = remove_idx {
                    action = MidiAction::RemoveMapping(idx);
                }
            }

            ui.add_space(4.0);
        });

        self.midi_expanded = header.fully_open();
        action
    }

    /// Show the video outputs section (NDI, Output Window)
    fn show_output_section(&mut self, ui: &mut egui::Ui, state: OutputState) -> OutputAction {
        let mut action = OutputAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Video Outputs")
                .strong()
                .size(14.0)
        )
        .default_open(self.output_expanded)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.add_space(4.0);

                // 🖥 Screen - output window row (MIDI learnable)
                ui.horizontal(|ui| {
                    let win_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if state.output_window_enabled {
                        egui::Color32::from_rgb(70, 130, 180)
                    } else {
                        egui::Color32::from_rgb(80, 80, 90)
                    };

                    let btn_response = ui.add(
                        egui::Button::new(egui::RichText::new("🖥").size(14.0))
                            .fill(win_color)
                            .min_size(egui::vec2(28.0, 22.0))
                    ).on_hover_text(if self.midi_learn_armed { "Click to map Output Window to MIDI" } else { "Output window (O)" });

                    if btn_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleOutputWindow);
                        } else {
                            action = OutputAction::ToggleOutputWindow;
                        }
                    }

                    // Label
                    let label_color = if state.output_window_enabled {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(120, 120, 130)
                    };
                    ui.label(egui::RichText::new("Screen").size(12.0).color(label_color));

                    ui.add_space(4.0);

                    // Fullscreen toggle - MIDI learnable
                    let fs_text = if state.output_window_fullscreen { "⛶ Full" } else { "◱ Window" };
                    let fs_fill = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if state.output_window_fullscreen {
                        egui::Color32::from_rgb(70, 130, 180)
                    } else {
                        egui::Color32::from_rgb(50, 50, 60)
                    };
                    let fs_alpha = if state.output_window_enabled { 255 } else { 100 };
                    let fs_hover = if self.midi_learn_armed { "Click to map Fullscreen toggle to MIDI" } else { "Toggle fullscreen (F)" };
                    let fs_response = ui.add(
                        egui::Button::new(egui::RichText::new(fs_text).size(10.0))
                            .fill(egui::Color32::from_rgba_unmultiplied(fs_fill.r(), fs_fill.g(), fs_fill.b(), fs_alpha))
                            .min_size(egui::vec2(50.0, 20.0))
                    ).on_hover_text(fs_hover);

                    if fs_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleOutputFullscreen);
                        } else {
                            action = OutputAction::ToggleOutputFullscreen;
                        }
                    }
                });

                ui.add_space(4.0);

                // 📡 NDI - streaming row with resolution options (MIDI learnable)
                ui.horizontal(|ui| {
                    let ndi_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if state.ndi_enabled {
                        egui::Color32::from_rgb(80, 180, 80)
                    } else {
                        egui::Color32::from_rgb(80, 80, 90)
                    };

                    let ndi_btn_response = ui.add(
                        egui::Button::new(egui::RichText::new("📡").size(14.0))
                            .fill(ndi_color)
                            .min_size(egui::vec2(28.0, 22.0))
                    ).on_hover_text(if self.midi_learn_armed { "Click to map NDI toggle to MIDI" } else { "NDI streaming (N)" });

                    if ndi_btn_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleNdi);
                        } else {
                            action = OutputAction::ToggleNdi;
                        }
                    }

                    // Label
                    let label_color = if state.ndi_enabled {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(120, 120, 130)
                    };
                    ui.label(egui::RichText::new("NDI").size(12.0).color(label_color));

                    ui.add_space(4.0);

                    // Resolution pills
                    let enabled_alpha = if state.ndi_enabled { 255 } else { 100 };

                    let hd_selected = self.ndi_resolution == NdiResolution::Hd;
                    let hd_fill = if hd_selected {
                        egui::Color32::from_rgba_unmultiplied(70, 130, 180, enabled_alpha)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(50, 50, 60, enabled_alpha)
                    };
                    if ui.add(
                        egui::Button::new(egui::RichText::new("HD").size(10.0))
                            .fill(hd_fill)
                            .min_size(egui::vec2(26.0, 20.0))
                    ).on_hover_text("1920×1080").clicked() {
                        self.ndi_resolution = NdiResolution::Hd;
                    }

                    let uhd_selected = self.ndi_resolution == NdiResolution::Uhd;
                    let uhd_fill = if uhd_selected {
                        egui::Color32::from_rgba_unmultiplied(180, 120, 60, enabled_alpha)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(50, 50, 60, enabled_alpha)
                    };
                    if ui.add(
                        egui::Button::new(egui::RichText::new("4K").size(10.0))
                            .fill(uhd_fill)
                            .min_size(egui::vec2(26.0, 20.0))
                    ).on_hover_text("3840×2160").clicked() {
                        self.ndi_resolution = NdiResolution::Uhd;
                    }

                    let uw_selected = self.ndi_resolution == NdiResolution::Ultrawide;
                    let uw_fill = if uw_selected {
                        egui::Color32::from_rgba_unmultiplied(140, 80, 180, enabled_alpha)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(50, 50, 60, enabled_alpha)
                    };
                    if ui.add(
                        egui::Button::new(egui::RichText::new("UW").size(10.0))
                            .fill(uw_fill)
                            .min_size(egui::vec2(26.0, 20.0))
                    ).on_hover_text("5120×1440 Ultrawide").clicked() {
                        self.ndi_resolution = NdiResolution::Ultrawide;
                    }

                    // Audio toggle (MIDI learnable)
                    ui.add_space(4.0);
                    let audio_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if state.ndi_audio_enabled {
                        egui::Color32::from_rgb(100, 180, 255)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(80, 80, 90, enabled_alpha)
                    };
                    let audio_icon = if state.ndi_audio_enabled { "🔊" } else { "🔇" };
                    let audio_tooltip = if self.midi_learn_armed {
                        "Click to map NDI Audio to MIDI"
                    } else if state.ndi_audio_enabled {
                        "Audio ON"
                    } else {
                        "Audio OFF"
                    };
                    let audio_btn_response = ui.add(
                        egui::Button::new(egui::RichText::new(audio_icon).size(12.0))
                            .fill(audio_color)
                            .min_size(egui::vec2(26.0, 20.0))
                    ).on_hover_text(audio_tooltip);

                    if audio_btn_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleNdiAudio);
                        } else {
                            action = OutputAction::ToggleNdiAudio;
                        }
                    }

                    // Connection status - ALWAYS rendered to prevent layout jumping
                    ui.add_space(2.0);
                    let (conn_text, conn_color) = if state.ndi_enabled && state.ndi_connections > 0 {
                        let tally_color = if state.ndi_tally.on_program {
                            egui::Color32::from_rgb(255, 80, 80)
                        } else if state.ndi_tally.on_preview {
                            egui::Color32::from_rgb(80, 255, 80)
                        } else {
                            egui::Color32::from_rgb(100, 180, 255)
                        };
                        (format!("{}⚡", state.ndi_connections), tally_color)
                    } else {
                        (" ".to_string(), egui::Color32::from_rgb(50, 50, 60))
                    };
                    ui.label(egui::RichText::new(conn_text).color(conn_color).size(11.0));
                });

                // NDI Audio Gain slider (only shown when audio is enabled)
                if state.ndi_audio_enabled {
                    ui.horizontal(|ui| {
                        ui.add_space(30.0); // indent to align with NDI row
                        ui.label(egui::RichText::new("Gain:").size(11.0).color(egui::Color32::GRAY));
                        self.ndi_audio_gain = state.ndi_audio_gain;
                        let response = ui.add(
                            egui::Slider::new(&mut self.ndi_audio_gain, 0.1..=16.0)
                                .logarithmic(true)
                                .show_value(true)
                                .custom_formatter(|v, _| format!("{:.1}x", v))
                        );
                        if response.changed() {
                            action = OutputAction::SetNdiGain(self.ndi_audio_gain);
                        }
                        // Show Level Assist badge when active
                        if state.ndi_assist_gain > 1.01 {
                            let (badge, color) = if !state.ndi_audio_present {
                                ("AUTO (WAITING)", egui::Color32::from_rgb(130, 130, 140))
                            } else if state.ndi_assist_hold {
                                ("AUTO (HOLD)", egui::Color32::from_rgb(160, 160, 100))
                            } else {
                                ("AUTO", egui::Color32::from_rgb(120, 200, 120))
                            };
                            ui.label(egui::RichText::new(format!("{} {:.1}x", badge, state.ndi_effective_gain))
                                .size(10.0).color(color))
                                .on_hover_text("Auto sets baseline loudness slowly; use slider for immediate loudness.");
                        }
                    });
                }
            });
        });

        // Track expanded state
        self.output_expanded = header.fully_open();

        action
    }

    /// Show the recording section
    fn show_recording_section(&mut self, ui: &mut egui::Ui, state: OutputState, is_authenticated: bool) -> OutputAction {
        let mut action = OutputAction::None;

        // Debug logging (once per session) - use println to bypass log filtering
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            println!(
                ">>> [Recording UI] Section rendered: auth={}, available={}, enabled={}",
                is_authenticated,
                state.recording_available,
                state.recording_enabled
            );
        }

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Record Set")
                .strong()
                .size(14.0)
        )
        .default_open(self.recording_expanded)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.add_space(4.0);

                // Show login required message if not authenticated
                if !is_authenticated {
                    self.show_login_required_message(ui);
                    return;
                }

                // 🔴 Record - compact row with duration display (MIDI learnable)
                ui.horizontal(|ui| {
                    let rec_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if state.recording_processing {
                        // Pulsing amber when processing (stitching segments)
                        let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.3 + 0.7;
                        egui::Color32::from_rgba_unmultiplied(230, 160, 40, (200.0 * pulse as f32) as u8)
                    } else if state.recording_enabled {
                        egui::Color32::from_rgb(220, 60, 60) // Bright red when recording
                    } else if state.recording_available {
                        egui::Color32::from_rgb(140, 50, 50) // Dark red when available (clickable)
                    } else {
                        egui::Color32::from_rgb(50, 50, 50) // Dimmed grey when unavailable
                    };

                    // Show hourglass when processing, otherwise stop/record icons
                    let rec_text = if state.recording_processing {
                        egui_phosphor::regular::HOURGLASS_MEDIUM
                    } else if state.recording_enabled {
                        "⏹"
                    } else {
                        "⏺"
                    };
                    let tooltip = if self.midi_learn_armed {
                        "Click to map Recording to MIDI"
                    } else if state.recording_processing {
                        "Processing recording..."
                    } else if state.recording_enabled {
                        "Stop recording"
                    } else if state.recording_available {
                        "Record to MP4"
                    } else {
                        "Recording unavailable (ffmpeg not found)"
                    };

                    // Disable button while processing
                    let btn_enabled = (state.recording_available && !state.recording_processing) || self.midi_learn_armed;
                    let rec_btn_response = ui.add_enabled(
                        btn_enabled,
                        egui::Button::new(egui::RichText::new(rec_text).size(14.0))
                            .fill(rec_color)
                            .min_size(egui::vec2(28.0, 22.0))
                    ).on_hover_text(tooltip);

                    if rec_btn_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleRecording);
                        } else {
                            action = OutputAction::ToggleRecording;
                        }
                    }

                    // Show "Processing..." when stitching segments
                    if state.recording_processing {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("Processing...")
                            .color(egui::Color32::from_rgb(230, 160, 40))
                            .size(12.0)
                            .italics());
                    }
                    // Show duration when recording
                    else if state.recording_enabled {
                        ui.add_space(4.0);
                        let mins = (state.recording_duration_secs / 60.0) as u32;
                        let secs = (state.recording_duration_secs % 60.0) as u32;
                        ui.label(egui::RichText::new(format!("{:02}:{:02}", mins, secs))
                            .color(egui::Color32::from_rgb(220, 60, 60))
                            .size(12.0)
                            .strong());

                        // Show segment indicator for segmented recording
                        if state.segmented_recording {
                            ui.add_space(4.0);
                            // Green checkmark if committed recently (within 2s), otherwise gray
                            let commit_color = if state.last_commit_elapsed_ms < 2000 {
                                egui::Color32::from_rgb(80, 200, 80) // Green - recent commit
                            } else {
                                egui::Color32::GRAY
                            };
                            ui.label(egui::RichText::new(format!("Seg {} {}", state.committed_segments, egui_phosphor::regular::CHECK_CIRCLE))
                                .color(commit_color)
                                .size(10.0))
                                .on_hover_text(format!(
                                    "Segmented recording: {} segments committed\nCurrent segment: {}\nMax data loss on crash: ~60 seconds",
                                    state.committed_segments,
                                    state.current_segment
                                ));
                        }
                    }
                });

                // Recording Audio Gain slider
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    ui.label(egui::RichText::new("Gain:").size(11.0).color(egui::Color32::GRAY));
                    self.rec_audio_gain = state.rec_audio_gain;
                    let response = ui.add(
                        egui::Slider::new(&mut self.rec_audio_gain, 0.1..=16.0)
                            .logarithmic(true)
                            .show_value(true)
                            .custom_formatter(|v, _| format!("{:.1}x", v))
                    );
                    if response.changed() {
                        action = OutputAction::SetRecGain(self.rec_audio_gain);
                    }
                    // Show Level Assist badge when active
                    if state.rec_assist_gain > 1.01 {
                        let (badge, color) = if !state.rec_audio_present {
                            ("AUTO (WAITING)", egui::Color32::from_rgb(130, 130, 140))
                        } else if state.rec_assist_hold {
                            ("AUTO (HOLD)", egui::Color32::from_rgb(160, 160, 100))
                        } else {
                            ("AUTO", egui::Color32::from_rgb(120, 200, 120))
                        };
                        ui.label(egui::RichText::new(format!("{} {:.1}x", badge, state.rec_effective_gain))
                            .size(10.0).color(color))
                            .on_hover_text("Auto sets baseline loudness slowly; use slider for immediate loudness.");
                    }
                });

                ui.add_space(8.0);

                // Recording directory selection
                ui.horizontal(|ui| {
                    // Manager button
                    if ui.button(format!("{} Set Manager", egui_phosphor::regular::SQUARES_FOUR)).on_hover_text("Open Set Manager").clicked()
                    {
                        self.show_recording_manager = true;
                    }

                    if ui.button("📁 Folder...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            let path_str = path.display().to_string();
                            log::info!("[UI] Selected recording folder: {}", path_str);
                            self.recording_directory = Some(path_str.clone());

                            // Persist and scan
                            if let Some(db) = self.db.clone() {
                                if let Err(e) = db.set_setting("recording_directory", &path_str) {
                                    log::error!("[UI] Failed to save recording directory: {}", e);
                                }
                                self.scan_recording_directory(&db, &path_str);
                            }
                        }
                    }

                    if self.recording_directory.is_some() {
                        if ui.small_button("↻").on_hover_text("Refresh").clicked() {
                            if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
                                self.scan_recording_directory(&db, &dir);
                            }
                        }
                    }
                });

                // Upload status indicator
                if let Some(ref upload_manager) = self.set_upload_manager {
                    if upload_manager.is_running() {
                        let progress = upload_manager.get_progress();
                        let (uploaded, total) = upload_manager.get_progress_bytes();
                        let current_file = upload_manager.get_current_file();

                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // Pulsing upload icon
                            let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.3 + 0.7;
                            let color = egui::Color32::from_rgba_unmultiplied(100, 180, 255, (200.0 * pulse as f32) as u8);
                            ui.label(egui::RichText::new(egui_phosphor::regular::CLOUD_ARROW_UP)
                                .size(14.0)
                                .color(color));

                            ui.label(egui::RichText::new("Uploading")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(100, 180, 255)));

                            // Progress bar
                            let progress_bar = egui::ProgressBar::new(progress)
                                .show_percentage();
                            ui.add_sized([80.0, 14.0], progress_bar);

                            // Size info
                            let uploaded_mb = uploaded as f64 / 1_000_000.0;
                            let total_mb = total as f64 / 1_000_000.0;
                            ui.label(egui::RichText::new(format!("{:.1}/{:.1} MB", uploaded_mb, total_mb))
                                .size(9.0)
                                .color(egui::Color32::GRAY));

                            // Pause button
                            if ui.small_button("⏸").on_hover_text("Pause upload").clicked() {
                                upload_manager.pause_upload();
                            }
                        });

                        // File name being uploaded
                        if let Some(file) = current_file {
                            let file_name = file.split('/').last().unwrap_or(&file);
                            let truncated = if file_name.len() > 30 {
                                format!("{}...", &file_name[..27])
                            } else {
                                file_name.to_string()
                            };
                            ui.label(egui::RichText::new(truncated)
                                .size(9.0)
                                .color(egui::Color32::DARK_GRAY));
                        }
                    } else {
                        // Show stats if there are pending/failed uploads
                        let (pending, uploading, completed, failed, paused, _, _) = upload_manager.get_stats();
                        let queued = pending + paused;

                        if queued > 0 || failed > 0 {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if queued > 0 {
                                    ui.label(egui::RichText::new(format!("{} {} queued", egui_phosphor::regular::CLOUD_ARROW_UP, queued))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(150, 150, 150)));

                                    // Resume button
                                    if ui.small_button(egui::RichText::new(egui_phosphor::regular::ARROW_FAT_UP).color(egui::Color32::from_rgb(80, 200, 80))).on_hover_text("Resume uploads").clicked() {
                                        upload_manager.start_upload();
                                    }
                                }

                                if failed > 0 {
                                    ui.label(egui::RichText::new(format!("{} failed", failed))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(200, 80, 80)));

                                    // Retry all failed button
                                    if ui.small_button(egui_phosphor::regular::ARROW_CLOCKWISE).on_hover_text("Retry all failed uploads").clicked() {
                                        if let Some(ref db) = self.db {
                                            if let Ok(count) = db.retry_all_failed_uploads() {
                                                if count > 0 {
                                                    log::info!("[UI] Retrying {} failed uploads", count);
                                                    self.upload_status_cache = db.get_upload_status_map();
                                                    upload_manager.start_upload();
                                                }
                                            }
                                        }
                                    }
                                }

                                if completed > 0 {
                                    ui.label(egui::RichText::new(format!("{} {}", egui_phosphor::regular::CHECK_CIRCLE, completed))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(80, 180, 80)));
                                }
                            });
                        }
                    }
                }

                // Show current folder and recording count
                if let Some(ref dir) = self.recording_directory.clone() {
                    let folder_name = dir.split('/').last().unwrap_or(&dir);

                    // Folder info + view mode toggle row
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("📂 {} ({} files)", folder_name, self.recordings.len()))
                            .small()
                            .color(egui::Color32::GRAY));

                        // View mode toggle buttons (right side)
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // List view button
                            let list_color = if self.recording_view_mode == VideoViewMode::List {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_rgb(80, 80, 80)
                            };
                            if ui.add(
                                egui::Button::new(egui::RichText::new(egui_phosphor::regular::LIST).size(14.0).color(list_color))
                                    .fill(egui::Color32::TRANSPARENT)
                            ).on_hover_text("List view").clicked() {
                                self.recording_view_mode = VideoViewMode::List;
                            }

                            // Grid view button
                            let grid_color = if self.recording_view_mode == VideoViewMode::Grid {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_rgb(80, 80, 80)
                            };
                            if ui.add(
                                egui::Button::new(egui::RichText::new(egui_phosphor::regular::SQUARES_FOUR).size(14.0).color(grid_color))
                                    .fill(egui::Color32::TRANSPARENT)
                            ).on_hover_text("Grid view").clicked() {
                                self.recording_view_mode = VideoViewMode::Grid;
                            }
                        });
                    });

                    // Recording list/grid (scrollable)
                    if !self.recordings.is_empty() {
                        let ctx = ui.ctx().clone();
                        egui::ScrollArea::vertical()
                            .min_scrolled_height(150.0)
                            .max_height(500.0)
                            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                            .show(ui, |ui| {
                            match self.recording_view_mode {
                                VideoViewMode::Grid => {
                                    // === GRID VIEW ===
                                    self.render_recording_grid(ui, &ctx);
                                }
                                VideoViewMode::List => {
                                    // === LIST VIEW ===
                                let mut save_edit: Option<(String, String)> = None;
                                let mut cancel_edit = false;

                                for rec in self.recordings.clone() {
                                    let is_editing = self.editing_recording.as_ref() == Some(&rec.file_path);

                                    if is_editing {
                                        // Inline edit mode
                                        ui.horizontal(|ui| {
                                            let response = ui.add(
                                                egui::TextEdit::singleline(&mut self.editing_recording_name)
                                                    .desired_width(180.0)
                                                    .font(egui::TextStyle::Small)
                                            );

                                            // Auto-focus
                                            if response.gained_focus() || self.editing_recording_name.is_empty() {
                                                response.request_focus();
                                            }

                                            // Save on Enter
                                            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                                save_edit = Some((rec.file_path.clone(), self.editing_recording_name.clone()));
                                            }

                                            // Cancel on Escape
                                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                                cancel_edit = true;
                                            }

                                            if ui.small_button("✓").on_hover_text("Save").clicked() {
                                                save_edit = Some((rec.file_path.clone(), self.editing_recording_name.clone()));
                                            }
                                            if ui.small_button("✗").on_hover_text("Cancel").clicked() {
                                                cancel_edit = true;
                                            }
                                        });
                                    } else {
                                        // Normal display mode
                                        let list_upload_status = self.upload_status_cache.get(&rec.file_path).cloned();
                                        let response = ui.horizontal(|ui| {
                                            // Sync status icon prefix
                                            if let Some(ref status) = list_upload_status {
                                                let (icon, color) = match status {
                                                    crate::db::SetUploadStatus::Completed => (
                                                        egui_phosphor::regular::CHECK_CIRCLE,
                                                        egui::Color32::from_rgb(80, 200, 80)
                                                    ),
                                                    crate::db::SetUploadStatus::Failed => (
                                                        egui_phosphor::regular::X_CIRCLE,
                                                        egui::Color32::from_rgb(220, 80, 80)
                                                    ),
                                                    crate::db::SetUploadStatus::Uploading => (
                                                        egui_phosphor::regular::CLOUD_ARROW_UP,
                                                        egui::Color32::from_rgb(100, 180, 255)
                                                    ),
                                                    crate::db::SetUploadStatus::Pending | crate::db::SetUploadStatus::Paused => (
                                                        egui_phosphor::regular::CLOUD,
                                                        egui::Color32::from_rgb(150, 150, 150)
                                                    ),
                                                };
                                                ui.label(egui::RichText::new(icon).size(11.0).color(color));
                                            }

                                            let display = rec.display_name();
                                            let label = egui::RichText::new(display).size(11.0);

                                            let text = ui.add(
                                                egui::Label::new(label).sense(egui::Sense::click())
                                            );

                                            // Size badge on right
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(egui::RichText::new(rec.size_string())
                                                    .size(9.0)
                                                    .color(egui::Color32::DARK_GRAY));
                                            });

                                            text
                                        });

                                        // Click to open
                                        if response.inner.clicked() {
                                            log::info!("[UI] Opening recording: {}", rec.file_path);
                                            #[cfg(target_os = "macos")]
                                            {
                                                let _ = std::process::Command::new("open")
                                                    .arg(&rec.file_path)
                                                    .spawn();
                                            }
                                        }

                                        // Right-click context menu (with status-aware options)
                                        let file_path_for_menu = rec.file_path.clone();
                                        let file_name_for_menu = rec.file_name.clone();
                                        let custom_name_for_menu = rec.custom_name.clone();
                                        let duration_secs_for_menu = rec.duration_secs;
                                        let created_at_for_menu = rec.created_at;
                                        response.inner.context_menu(|ui| {
                                            ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);
                                            if ui.button("✏️   Rename Recording...").clicked() {
                                                self.editing_recording = Some(file_path_for_menu.clone());
                                                self.editing_recording_name = custom_name_for_menu.clone()
                                                    .unwrap_or_else(|| file_name_for_menu.trim_end_matches(".mp4").to_string());
                                                ui.close_menu();
                                            }
                                            if ui.button("📂   Show in Finder").clicked() {
                                                #[cfg(target_os = "macos")]
                                                {
                                                    let _ = std::process::Command::new("open")
                                                        .arg("-R")
                                                        .arg(&file_path_for_menu)
                                                        .spawn();
                                                }
                                                ui.close_menu();
                                            }
                                            ui.separator();

                                            // Upload/Sync options based on status
                                            match list_upload_status {
                                                Some(crate::db::SetUploadStatus::Completed) => {
                                                    ui.add_enabled(false, egui::Button::new(format!("{}   Synced to Compeller.ai", egui_phosphor::regular::CHECK_CIRCLE)))
                                                        .on_disabled_hover_text("This recording is already synced");
                                                }
                                                Some(crate::db::SetUploadStatus::Failed) => {
                                                    if ui.button(format!("{}   Retry Upload", egui_phosphor::regular::ARROW_CLOCKWISE)).clicked() {
                                                        if let Some(ref db) = self.db {
                                                            if let Err(e) = db.retry_failed_upload(&file_path_for_menu) {
                                                                log::error!("[UI] Failed to retry upload: {}", e);
                                                            } else {
                                                                self.upload_status_cache = db.get_upload_status_map();
                                                                if let Some(ref upload_manager) = self.set_upload_manager {
                                                                    if !upload_manager.is_running() {
                                                                        upload_manager.start_upload();
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        ui.close_menu();
                                                    }
                                                }
                                                Some(crate::db::SetUploadStatus::Uploading) | Some(crate::db::SetUploadStatus::Pending) | Some(crate::db::SetUploadStatus::Paused) => {
                                                    if ui.button(format!("{}   Cancel Upload", egui_phosphor::regular::X)).clicked() {
                                                        if let Some(ref upload_manager) = self.set_upload_manager {
                                                            let _ = upload_manager.cancel_upload(&file_path_for_menu);
                                                            if let Some(ref db) = self.db {
                                                                self.upload_status_cache = db.get_upload_status_map();
                                                            }
                                                        }
                                                        ui.close_menu();
                                                    }
                                                }
                                                None => {
                                                    if let Some(ref upload_manager) = self.set_upload_manager {
                                                        if ui.button(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)).clicked() {
                                                            // Use recording metadata for upload
                                                            let recorded_at_str = chrono::DateTime::from_timestamp(created_at_for_menu, 0)
                                                                .map(|dt| dt.to_rfc3339())
                                                                .unwrap_or_else(|| chrono::Local::now().to_rfc3339());
                                                            let request = crate::set_sync::SetUploadRequest {
                                                                file_path: file_path_for_menu.clone(),
                                                                title: custom_name_for_menu.clone().or_else(|| Some(file_name_for_menu.trim_end_matches(".mp4").to_string())),
                                                                recorded_at: Some(recorded_at_str),
                                                                theme_used: None, // TODO: capture from recording session
                                                                audio_source: None, // TODO: capture from recording session
                                                                duration_seconds: duration_secs_for_menu.map(|d| d as i64),
                                                            };
                                                            if let Err(e) = upload_manager.queue_upload(request) {
                                                                log::error!("[UI] Failed to queue upload: {}", e);
                                                            } else {
                                                                if let Some(ref db) = self.db {
                                                                    self.upload_status_cache = db.get_upload_status_map();
                                                                }
                                                                if !upload_manager.is_running() {
                                                                    upload_manager.start_upload();
                                                                }
                                                            }
                                                            ui.close_menu();
                                                        }
                                                    } else {
                                                        ui.add_enabled(false, egui::Button::new(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)))
                                                            .on_disabled_hover_text("Login required to upload");
                                                    }
                                                }
                                            }

                                            ui.separator();
                                            if ui.button(format!("{}   Delete Recording", egui_phosphor::regular::TRASH)).clicked() {
                                                self.pending_delete_recording = Some(file_path_for_menu.clone());
                                                ui.close_menu();
                                            }
                                        });

                                        if response.inner.hovered() {
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                    }
                                }

                                // Handle edit save
                                if let Some((path, name)) = save_edit {
                                    let name_opt = if name.trim().is_empty() { None } else { Some(name.trim()) };
                                    if let Some(ref db) = self.db {
                                        if let Err(e) = db.set_recording_custom_name(&path, name_opt) {
                                            log::error!("[UI] Failed to save recording name: {}", e);
                                        } else {
                                            log::info!("[UI] Renamed recording: {:?}", name_opt);
                                        }
                                    }
                                    self.editing_recording = None;
                                    self.editing_recording_name.clear();
                                    // Refresh list to show new name
                                    if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
                                        self.scan_recording_directory(&db, &dir);
                                    }
                                }

                                // Handle edit cancel
                                if cancel_edit {
                                    self.editing_recording = None;
                                    self.editing_recording_name.clear();
                                }
                                } // End of VideoViewMode::List
                            } // End of match
                            });
                    }
                }
            });
        });

        // Track expanded state
        self.recording_expanded = header.fully_open();

        // Handle pending recording deletion (from list/grid context menus)
        if let Some(path) = self.pending_delete_recording.take() {
            log::info!("[UI] Deleting recording: {}", path);
            if let Err(e) = std::fs::remove_file(&path) {
                log::error!("[UI] Failed to delete file: {}", e);
            }
            if let Some(ref db) = self.db {
                let _ = db.delete_recording(&path);
            }
            if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
                self.scan_recording_directory(&db, &dir);
            }
        }

        action
    }

    /// Get current NDI resolution
    pub fn ndi_resolution(&self) -> NdiResolution {
        self.ndi_resolution
    }

    /// Get current render mode
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Check if Math Overlay HUD is enabled
    pub fn show_math_overlay(&self) -> bool {
        self.show_math_overlay
    }

    /// Check if QS Wheel is enabled
    pub fn show_qs_wheel(&self) -> bool {
        self.show_qs_wheel
    }

    /// Check if QS Wheel numbers should be hidden
    pub fn hide_qs_numbers(&self) -> bool {
        self.hide_qs_numbers
    }

    /// Check if QS Wheel is in dominant-only mode
    pub fn qs_dominant_only(&self) -> bool {
        self.qs_dominant_only
    }

    /// Check if Perf HUD is enabled
    pub fn show_perf_hud(&self) -> bool {
        self.show_perf_hud
    }

    /// Check if REG Debug HUD is enabled
    pub fn show_debug_hud(&self) -> bool {
        self.show_debug_hud
    }

    /// Check FPS and auto-adjust quality if sustained low FPS detected
    /// Returns true if an adjustment was made
    pub fn check_fps_and_adjust(&mut self, fps: f32) -> bool {
        // Decrement warning counter each frame
        if self.low_fps_warning_frames > 0 {
            self.low_fps_warning_frames -= 1;
        }

        // Already at lowest settings, no action needed
        if self.quality_tier == 0 && self.bg_quality == 0 {
            self.fps_low_count = 0;
            return false;
        }

        if fps < 35.0 {
            self.fps_low_count += 1;
            // Sustained low FPS for ~0.5 sec at 60fps
            if self.fps_low_count > 30 {
                log::warn!("[Perf] Auto-adjusting quality to Low due to sustained low FPS ({:.0})", fps);
                self.quality_tier = 0;
                self.bg_quality = 0;
                self.low_fps_warning_frames = 300; // Show warning for ~5 seconds
                self.fps_low_count = 0; // Reset to prevent repeated triggers
                self.mark_dirty();
                return true;
            }
        } else {
            // FPS recovered, reset counter
            self.fps_low_count = 0;
        }
        false
    }

    /// Check if frozen (scrub mode)
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Toggle freeze state
    pub fn toggle_freeze(&mut self) {
        self.frozen = !self.frozen;
        if self.frozen {
            self.scrub_position = 1.0; // Start at newest when freezing
        }
    }

    /// Toggle chroma wheel HUD visibility (called by 'H' key)
    pub fn toggle_chroma_hud(&mut self) {
        self.show_chroma_key = !self.show_chroma_key;
    }

    /// Whether the genre BPM range guides REACT's autonomous tempo octave.
    pub fn genre_tempo_guidance(&self) -> bool {
        self.genre_tempo_guidance
    }

    /// Toggle "Use Genre for Tempo" (MIDI button / checkbox).
    pub fn toggle_genre_tempo_guidance(&mut self) {
        self.genre_tempo_guidance = !self.genre_tempo_guidance;
    }

    /// Set "Use Genre for Tempo" explicitly.
    pub fn set_genre_tempo_guidance(&mut self, on: bool) {
        self.genre_tempo_guidance = on;
    }

    /// Combine the OLA panel host + port edit fields into a `host:port` endpoint.
    /// The port is normalized to a `u16` (blank or non-numeric → olad's default
    /// RPC port 9010) so the persisted/sent value always matches what `olad` will
    /// actually use — no silent divergence between the field and the connection.
    fn ola_endpoint(host: &str, port: &str) -> String {
        let port = port.trim().parse::<u16>().unwrap_or(9010);
        format!("{}:{}", host.trim(), port)
    }

    /// Set chroma wheel HUD visibility
    pub fn set_chroma_hud_visible(&mut self, visible: bool) {
        self.show_chroma_key = visible;
    }

    /// Get chroma wheel HUD visibility
    pub fn is_chroma_hud_visible(&self) -> bool {
        self.show_chroma_key
    }

    /// Set freeze state
    pub fn set_frozen(&mut self, frozen: bool) {
        if frozen && !self.frozen {
            self.scrub_position = 1.0;
        }
        self.frozen = frozen;
    }

    /// Get scrub position (0.0 = oldest, 1.0 = newest)
    pub fn scrub_position(&self) -> f32 {
        self.scrub_position
    }

    /// Set scrub position
    pub fn set_scrub_position(&mut self, pos: f32) {
        self.scrub_position = pos.clamp(0.0, 1.0);
    }

    /// Set frozen display state (called from app when frozen)
    pub fn set_frozen_display_state(&mut self, state: Option<crate::render::HarmonicFieldState>) {
        self.frozen_display_state = state;
    }

    /// Get current display state (frozen or None for live)
    pub fn frozen_display_state(&self) -> Option<&crate::render::HarmonicFieldState> {
        self.frozen_display_state.as_ref()
    }

    /// Set render mode
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
    }

    /// Get the current preset (category, name) if set
    pub fn get_current_preset(&self) -> Option<(String, String)> {
        self.current_preset.clone()
    }

    /// Set the current preset (category, name) - used for startup defaults
    pub fn set_current_preset(&mut self, category: String, name: String) {
        self.current_preset = Some((category, name));
    }

    /// Check if there's a pending preset load
    pub fn has_pending_preset_load(&self) -> bool {
        self.pending_preset_load.is_some()
    }

    /// Get the pending preset load info for display
    pub fn get_pending_preset_load(&self) -> Option<(String, String)> {
        self.pending_preset_load.clone()
    }

    /// Increment frame counter and check if ready to process load
    /// Returns true if the loading dialog has been shown for enough frames
    pub fn should_process_pending_load(&mut self) -> bool {
        if self.pending_preset_load.is_some() {
            self.pending_preset_load_frames += 1;
            // Wait 2 frames to ensure dialog renders before blocking load
            self.pending_preset_load_frames >= 2
        } else {
            false
        }
    }

    /// Process the pending preset load (call after showing loading dialog for 2+ frames)
    pub fn process_pending_preset_load(&mut self, pipeline: &mut RenderPipeline) {
        if let Some((cat, preset)) = self.pending_preset_load.take() {
            log::info!("[UI] Processing deferred preset load: cat='{}', preset='{}'", cat, preset);
            if let Err(e) = pipeline.load_preset_by_name(&cat, &preset) {
                log::error!("Failed to load preset: {}", e);
            } else {
                self.current_preset = Some((cat.clone(), preset.clone()));
                let new_mode = pipeline.render_mode();
                log::info!("[UI] Loaded preset: {} ({}), render_mode={:?}", preset, cat, new_mode);
                self.render_mode = new_mode;
            }
            self.pending_preset_load_frames = 0;  // Reset frame counter
            self.mark_dirty();
        }
    }

    /// Navigate to next preset in current list (for MIDI control)
    pub fn navigate_next_preset(&mut self, pipeline: &mut RenderPipeline) -> bool {
        if let Some((cat, name)) = &self.current_preset {
            if let Some(idx) = self.presets.iter().position(|n| n == name) {
                let next_idx = if idx >= self.presets.len() - 1 { 0 } else { idx + 1 };
                if let Some(next_name) = self.presets.get(next_idx) {
                    if pipeline.load_preset_by_name(cat, next_name).is_ok() {
                        self.current_preset = Some((cat.clone(), next_name.clone()));
                        self.render_mode = RenderMode::ProjectM;
                        pipeline.set_render_mode(RenderMode::ProjectM);
                        self.scroll_to_preset = true;
                        self.mark_dirty();
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Navigate to previous preset in current list (for MIDI control)
    pub fn navigate_prev_preset(&mut self, pipeline: &mut RenderPipeline) -> bool {
        if let Some((cat, name)) = &self.current_preset {
            if let Some(idx) = self.presets.iter().position(|n| n == name) {
                let prev_idx = if idx == 0 { self.presets.len().saturating_sub(1) } else { idx - 1 };
                if let Some(prev_name) = self.presets.get(prev_idx) {
                    if pipeline.load_preset_by_name(cat, prev_name).is_ok() {
                        self.current_preset = Some((cat.clone(), prev_name.clone()));
                        self.render_mode = RenderMode::ProjectM;
                        pipeline.set_render_mode(RenderMode::ProjectM);
                        self.scroll_to_preset = true;
                        self.mark_dirty();
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Set a random preset and update UI state (for MIDI control)
    pub fn set_random_preset(&mut self, pipeline: &mut RenderPipeline) -> bool {
        if let Some((category, preset)) = pipeline.set_random_preset() {
            self.selected_category = Some(category.clone());
            self.presets = pipeline.get_presets_in_category(&category);
            self.current_preset = Some((category, preset));
            self.render_mode = RenderMode::ProjectM;
            pipeline.set_render_mode(RenderMode::ProjectM);
            self.scroll_to_preset = true;
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    /// Auto-randomize fires every 60s when the toggle is on.
    const AUTO_RANDOMIZE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

    /// FPS-dip emergency shuffle: switch presets when FPS drops below 30.
    /// Always-on protection for 24/7 demos and live shows on shared hardware —
    /// not opt-in. Wall-clock based so trigger is responsive at any FPS.
    const FPS_SHUFFLE_THRESHOLD: f32 = 30.0;
    const FPS_SHUFFLE_TRIGGER: std::time::Duration = std::time::Duration::from_millis(500);
    const FPS_SHUFFLE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

    /// Returns true if the auto-randomize timer has elapsed and the caller
    /// should fire a random preset change. Detects any preset change (manual
    /// or automatic) since the last poll and resets the deadline on change.
    pub fn poll_auto_randomize_theme(&mut self) -> bool {
        let now = std::time::Instant::now();

        // Edge-detect preset changes from any source (dice button, list click,
        // MIDI next/prev, search-select, fuzzy-track-fallback, etc.) — bump the
        // deadline so the next auto-flip is a full interval after the change.
        if self.current_preset != self.auto_randomize_last_seen {
            self.auto_randomize_last_seen = self.current_preset.clone();
            if self.auto_randomize_theme {
                self.auto_randomize_next = Some(now + Self::AUTO_RANDOMIZE_INTERVAL);
            }
        }

        if !self.auto_randomize_theme {
            return false;
        }

        match self.auto_randomize_next {
            Some(t) if now >= t => {
                // Re-arm in case the caller can't fire the preset change this
                // frame (e.g., pipeline lock contention) — prevents tight-loop firing.
                self.auto_randomize_next = Some(now + Self::AUTO_RANDOMIZE_INTERVAL);
                true
            }
            None => {
                self.auto_randomize_next = Some(now + Self::AUTO_RANDOMIZE_INTERVAL);
                false
            }
            _ => false,
        }
    }

    /// Returns true when sustained FPS dip warrants shuffling to a new random
    /// preset. Always-on (not gated by the auto-randomize toggle) so live shows
    /// and 24/7 demos can't get stuck on a heavy preset. Limited to projectM
    /// mode — other render modes shouldn't yank the user off their visual.
    pub fn should_shuffle_theme_for_fps(&mut self, fps: f32) -> bool {
        let now = std::time::Instant::now();
        if let Some(until) = self.fps_shuffle_cooldown_until {
            if now < until {
                self.fps_shuffle_low_since = None;
                return false;
            }
            self.fps_shuffle_cooldown_until = None;
        }
        if !matches!(self.render_mode, RenderMode::ProjectM) {
            self.fps_shuffle_low_since = None;
            return false;
        }
        if fps < Self::FPS_SHUFFLE_THRESHOLD {
            let started = *self.fps_shuffle_low_since.get_or_insert(now);
            if now.duration_since(started) >= Self::FPS_SHUFFLE_TRIGGER {
                self.fps_shuffle_low_since = None;
                self.fps_shuffle_cooldown_until = Some(now + Self::FPS_SHUFFLE_COOLDOWN);
                log::info!(
                    "[ThemeShuffle] FPS {:.0} below {:.0} for {}ms — shuffling preset",
                    fps,
                    Self::FPS_SHUFFLE_THRESHOLD,
                    Self::FPS_SHUFFLE_TRIGGER.as_millis()
                );
                return true;
            }
        } else {
            self.fps_shuffle_low_since = None;
        }
        false
    }

    /// Load a preset by its stored file path and update UI state.
    pub fn load_preset_path(
        &mut self,
        pipeline: &mut RenderPipeline,
        preset_path: &str,
    ) -> bool {
        let categories = pipeline.get_preset_categories();
        for category in categories {
            let presets = pipeline.get_presets_in_category(&category);
            for preset in presets {
                if pipeline.get_preset_path(&category, &preset).as_deref() == Some(preset_path) {
                    match pipeline.load_preset_by_name(&category, &preset) {
                        Ok(()) => {
                            self.selected_category = Some(category.clone());
                            self.presets = pipeline.get_presets_in_category(&category);
                            self.current_preset = Some((category.clone(), preset.clone()));
                            self.render_mode = pipeline.render_mode();
                            self.scroll_to_preset = true;
                            self.mark_dirty();
                            log::info!("[UI] Loaded preset by path: {} ({})", preset, category);
                            return true;
                        }
                        Err(err) => {
                            log::warn!(
                                "[UI] Failed to load preset by path {}: {}",
                                preset_path,
                                err
                            );
                            return false;
                        }
                    }
                }
            }
        }
        log::warn!(
            "[UI] Preset path not found in loaded categories: {}",
            preset_path
        );
        false
    }

    /// Get the list of visible video paths (respecting current filters)
    fn get_visible_video_paths(&self) -> Vec<String> {
        let dir = match &self.video_directory {
            Some(d) => d,
            None => return Vec::new(),
        };

        let search_lower = self.video_search.to_lowercase();
        let has_search = !search_lower.is_empty();

        let mut visible: Vec<String> = Vec::new();

        for video_name in &self.video_files {
            let full_path = format!("{}/{}", dir, video_name);

            // Skip hidden
            if self.is_video_hidden(&full_path) && !self.show_hidden_videos {
                continue;
            }

            // Filter by search
            if has_search {
                let display_name = self.get_clean_video_display_name(&full_path, video_name);
                if !display_name.to_lowercase().contains(&search_lower) {
                    continue;
                }
            }

            visible.push(full_path);
        }

        visible
    }

    /// Browse to next video in list (for encoder rotation)
    pub fn browse_video_next(&mut self) {
        let visible = self.get_visible_video_paths();
        if visible.is_empty() {
            return;
        }

        let current_idx = self.browsed_video_path.as_ref()
            .and_then(|p| visible.iter().position(|v| v == p))
            .unwrap_or(0);

        let next_idx = if current_idx >= visible.len() - 1 { 0 } else { current_idx + 1 };
        self.browsed_video_path = visible.get(next_idx).cloned();
        log::debug!("[UI] Browse video next: {:?}", self.browsed_video_path);
    }

    /// Browse to previous video in list (for encoder rotation)
    pub fn browse_video_prev(&mut self) {
        let visible = self.get_visible_video_paths();
        if visible.is_empty() {
            return;
        }

        let current_idx = self.browsed_video_path.as_ref()
            .and_then(|p| visible.iter().position(|v| v == p))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 { visible.len().saturating_sub(1) } else { current_idx - 1 };
        self.browsed_video_path = visible.get(prev_idx).cloned();
        log::debug!("[UI] Browse video prev: {:?}", self.browsed_video_path);
    }

    /// Load the currently browsed video (for encoder push)
    /// Returns true if a video was loaded
    pub fn load_browsed_video(&mut self) -> bool {
        let full_path = match &self.browsed_video_path {
            Some(p) => p.clone(),
            None => return false,
        };

        log::info!("[UI] Loading browsed video: {}", full_path);

        if self.video_decoder.open(&full_path).is_ok() {
            self.video_path = Some(full_path.clone());
            self.video_decoder.play();

            // Add to recent videos
            self.add_to_recent_videos(&full_path);

            // Clear browse cursor after loading
            self.browsed_video_path = None;
            self.mark_dirty();
            return true;
        }

        false
    }

    /// Get the currently browsed video path
    pub fn browsed_video_path(&self) -> Option<&String> {
        self.browsed_video_path.as_ref()
    }

    /// Clear the browse cursor
    pub fn clear_browse_cursor(&mut self) {
        self.browsed_video_path = None;
        self.browse_favorites_index = None;
        self.browse_favorites_timer = None;
    }

    /// Rebuild the MIDI browse list (pinned + favorites)
    fn rebuild_midi_browse_list(&mut self, pipeline: &RenderPipeline) {
        // Ensure favorites/pinned are loaded from database
        if self.favorites_dirty {
            self.refresh_favorites();
        }

        self.midi_browse_list.clear();

        // Add Hero's Journey first (always available)
        self.midi_browse_list.push(("Geometric Field".to_string(), "Hero's Journey".to_string()));

        // Add all pinned presets from database (excluding LED Wall)
        for (cat, preset) in &self.pinned_presets {
            if cat == "LED Wall" {
                continue; // Skip LED Wall presets from pinned
            }
            if !self.midi_browse_list.iter().any(|(c, n)| c == cat && n == preset) {
                self.midi_browse_list.push((cat.clone(), preset.clone()));
            }
        }

        // Add presets from projectM "Pinned" folder (if not already added)
        let folder_pinned = pipeline.get_presets_in_category("Pinned");
        for preset_name in folder_pinned {
            if !self.midi_browse_list.iter().any(|(c, n)| c == "Pinned" && n == &preset_name) {
                self.midi_browse_list.push(("Pinned".to_string(), preset_name));
            }
        }

        // Add user favorites (skip pinned and LED Wall, already added or excluded)
        for (cat, preset) in &self.favorites {
            if cat == "LED Wall" {
                continue; // Skip LED Wall presets from favorites in browse list
            }
            // Skip if already in list
            if self.midi_browse_list.iter().any(|(pc, pn)| pc == cat && pn == preset) {
                continue;
            }
            self.midi_browse_list.push((cat.clone(), preset.clone()));
        }

        self.midi_browse_dirty = false;
        log::info!("[UI] Rebuilt MIDI browse list: {} items (pinned + favorites)", self.midi_browse_list.len());
    }

    /// Ensure MIDI browse list is up to date
    fn ensure_midi_browse_list(&mut self, pipeline: &RenderPipeline) {
        if self.midi_browse_dirty || self.midi_browse_list.is_empty() {
            self.rebuild_midi_browse_list(pipeline);
        }
    }

    /// Browse to next preset (encoder clockwise) - includes pinned + favorites
    pub fn browse_preset_next(&mut self, pipeline: &RenderPipeline) {
        self.ensure_midi_browse_list(pipeline);

        if self.midi_browse_list.is_empty() {
            log::warn!("[UI] No presets to browse!");
            return;
        }

        let current = self.browse_favorites_index.unwrap_or(self.midi_browse_list.len().saturating_sub(1));
        let next = if current >= self.midi_browse_list.len() - 1 { 0 } else { current + 1 };
        self.browse_favorites_index = Some(next);
        self.browse_favorites_timer = Some(std::time::Instant::now());

        if let Some((cat, preset)) = self.midi_browse_list.get(next) {
            log::info!("[UI] Browse preset: {}/{} ({}/{})", cat, preset, next + 1, self.midi_browse_list.len());
        }
    }

    /// Browse to previous preset (encoder counter-clockwise) - includes pinned + favorites
    pub fn browse_preset_prev(&mut self, pipeline: &RenderPipeline) {
        self.ensure_midi_browse_list(pipeline);

        if self.midi_browse_list.is_empty() {
            log::warn!("[UI] No presets to browse!");
            return;
        }

        let current = self.browse_favorites_index.unwrap_or(0);
        let prev = if current == 0 { self.midi_browse_list.len() - 1 } else { current - 1 };
        self.browse_favorites_index = Some(prev);
        self.browse_favorites_timer = Some(std::time::Instant::now());

        if let Some((cat, preset)) = self.midi_browse_list.get(prev) {
            log::info!("[UI] Browse preset: {}/{} ({}/{})", cat, preset, prev + 1, self.midi_browse_list.len());
        }
    }

    /// Load the currently browsed preset (encoder push) - uses pinned + favorites list
    pub fn load_browsed_preset(&mut self, pipeline: &mut RenderPipeline) -> bool {
        // Ensure the browse list is up to date
        self.ensure_midi_browse_list(pipeline);

        let idx = match self.browse_favorites_index {
            Some(i) => i,
            None => {
                log::warn!("[UI] LoadPreset called but no browse index set");
                return false;
            }
        };

        let (category, preset) = match self.midi_browse_list.get(idx) {
            Some(f) => f.clone(),
            None => {
                log::warn!("[UI] LoadPreset: index {} out of range (browse list len: {})", idx, self.midi_browse_list.len());
                return false;
            }
        };

        log::info!("[UI] Attempting to load preset: {}/{}", category, preset);

        match pipeline.load_preset_by_name(&category, &preset) {
            Ok(_) => {
                self.current_preset = Some((category.clone(), preset.clone()));
                // Set correct render mode based on category
                if category == "LED Wall" {
                    self.render_mode = RenderMode::LedWall;
                    pipeline.set_render_mode(RenderMode::LedWall);
                    log::info!("[UI] Activated LED Wall mode: {}", preset);
                } else if category == "Geometric Field" {
                    self.render_mode = RenderMode::HarmonicGeometry;
                    pipeline.set_render_mode(RenderMode::HarmonicGeometry);
                    log::info!("[UI] Activated Geometric Field mode: {}", preset);
                } else if category == "Mythos Field" {
                    self.render_mode = RenderMode::Mythos;
                    pipeline.set_render_mode(RenderMode::Mythos);
                    log::info!("[UI] Activated Mythos mode: {}", preset);
                } else {
                    self.render_mode = RenderMode::ProjectM;
                    pipeline.set_render_mode(RenderMode::ProjectM);
                }
                self.scroll_to_preset = true;
                self.browse_favorites_index = None; // Clear after loading
                self.browse_favorites_timer = None;
                self.browse_favorites_opacity = 0.0; // Hide overlay immediately
                self.mark_dirty();
                log::info!("[UI] Successfully loaded favorite: {}/{}", category, preset);
                true
            }
            Err(e) => {
                log::error!("[UI] Failed to load favorite {}/{}: {}", category, preset, e);
                false
            }
        }
    }

    /// Update the browse overlay opacity based on timer
    /// Call this each frame to handle fade in/out
    pub fn update_browse_overlay(&mut self, dt: f32) {
        const SHOW_DURATION: f32 = 6.0; // Auto-hide after 6 seconds (for DJ visibility)
        const FADE_SPEED: f32 = 6.0;    // Fade speed

        let target_opacity = if let Some(timer) = self.browse_favorites_timer {
            let elapsed = timer.elapsed().as_secs_f32();
            if elapsed < SHOW_DURATION {
                1.0
            } else {
                // Time to fade out
                0.0
            }
        } else {
            0.0
        };

        // Smooth opacity transition
        if self.browse_favorites_opacity < target_opacity {
            self.browse_favorites_opacity = (self.browse_favorites_opacity + dt * FADE_SPEED).min(target_opacity);
        } else if self.browse_favorites_opacity > target_opacity {
            self.browse_favorites_opacity = (self.browse_favorites_opacity - dt * FADE_SPEED).max(target_opacity);
        }

        // Clear state when fully faded out
        if self.browse_favorites_opacity < 0.01 && target_opacity == 0.0 {
            self.browse_favorites_opacity = 0.0;
            if self.browse_favorites_timer.is_some() {
                self.browse_favorites_index = None;
                self.browse_favorites_timer = None;
            }
        }
    }

    /// Get the current browse overlay state for rendering
    /// Returns: (display_name, file_name, category, index, total, opacity)
    pub fn get_browse_overlay_state(&self, pipeline: &RenderPipeline) -> Option<(String, String, String, usize, usize, f32)> {
        if self.browse_favorites_opacity < 0.01 {
            return None;
        }

        let idx = self.browse_favorites_index?;
        // Use midi_browse_list which includes pinned + favorites
        let (category, preset) = self.midi_browse_list.get(idx)?;

        // Get file path to look up custom name
        let file_path = pipeline.get_preset_path(category, preset);

        // Look up custom name from metadata cache
        let display_name = file_path
            .as_ref()
            .and_then(|fp| self.preset_metadata.get(fp))
            .and_then(|meta| meta.custom_name.clone())
            .unwrap_or_else(|| preset.clone());

        Some((
            display_name,
            preset.clone(),  // Original file name
            category.clone(),
            idx + 1,
            self.midi_browse_list.len(),
            self.browse_favorites_opacity,
        ))
    }

    /// Browse to next compel (encoder clockwise) - cycles through all videos
    pub fn browse_compel_next(&mut self) {
        if self.video_files.is_empty() {
            log::warn!("[UI] No compels to browse!");
            return;
        }

        let current = self.browse_compels_index.unwrap_or(self.video_files.len().saturating_sub(1));
        let next = if current >= self.video_files.len() - 1 { 0 } else { current + 1 };
        self.browse_compels_index = Some(next);
        self.browse_compels_timer = Some(std::time::Instant::now());

        if let Some(video) = self.video_files.get(next) {
            log::info!("[UI] Browse compel: {} ({}/{})", video, next + 1, self.video_files.len());
        }
    }

    /// Browse to previous compel (encoder counter-clockwise) - cycles through all videos
    pub fn browse_compel_prev(&mut self) {
        if self.video_files.is_empty() {
            log::warn!("[UI] No compels to browse!");
            return;
        }

        let current = self.browse_compels_index.unwrap_or(0);
        let prev = if current == 0 { self.video_files.len() - 1 } else { current - 1 };
        self.browse_compels_index = Some(prev);
        self.browse_compels_timer = Some(std::time::Instant::now());

        if let Some(video) = self.video_files.get(prev) {
            log::info!("[UI] Browse compel: {} ({}/{})", video, prev + 1, self.video_files.len());
        }
    }

    /// Load the currently browsed compel (encoder push)
    pub fn load_browsed_compel(&mut self) -> bool {
        let idx = match self.browse_compels_index {
            Some(i) => i,
            None => {
                log::warn!("[UI] LoadCompel called but no browse index set");
                return false;
            }
        };

        let video_name = match self.video_files.get(idx) {
            Some(v) => v.clone(),
            None => {
                log::warn!("[UI] LoadCompel: index {} out of range (video list len: {})", idx, self.video_files.len());
                return false;
            }
        };

        let dir = match &self.video_directory {
            Some(d) => d.clone(),
            None => {
                log::warn!("[UI] LoadCompel: no video directory set");
                return false;
            }
        };

        let full_path = format!("{}/{}", dir, video_name);
        log::info!("[UI] Loading compel: {}", full_path);

        if self.video_decoder.open(&full_path).is_ok() {
            self.video_path = Some(full_path.clone());
            self.video_decoder.play();

            // Add to recent videos
            self.add_to_recent_videos(&full_path);

            // Clear browse cursor after loading
            self.browse_compels_index = None;
            self.browse_compels_timer = None;
            self.browse_compels_opacity = 0.0;
            self.mark_dirty();
            log::info!("[UI] Successfully loaded compel: {}", video_name);
            return true;
        }

        log::error!("[UI] Failed to load compel: {}", full_path);
        false
    }

    /// Update the compel browse overlay opacity based on timer
    pub fn update_compel_browse_overlay(&mut self, dt: f32) {
        const SHOW_DURATION: f32 = 6.0; // Auto-hide after 6 seconds
        const FADE_SPEED: f32 = 6.0;    // Fade speed

        let target_opacity = if let Some(timer) = self.browse_compels_timer {
            let elapsed = timer.elapsed().as_secs_f32();
            if elapsed < SHOW_DURATION {
                1.0
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Smooth opacity transition
        if self.browse_compels_opacity < target_opacity {
            self.browse_compels_opacity = (self.browse_compels_opacity + dt * FADE_SPEED).min(target_opacity);
        } else if self.browse_compels_opacity > target_opacity {
            self.browse_compels_opacity = (self.browse_compels_opacity - dt * FADE_SPEED).max(target_opacity);
        }

        // Clear state when fully faded out
        if self.browse_compels_opacity < 0.01 && target_opacity == 0.0 {
            self.browse_compels_opacity = 0.0;
            if self.browse_compels_timer.is_some() {
                self.browse_compels_index = None;
                self.browse_compels_timer = None;
            }
        }
    }

    /// Get the current compel browse overlay state for rendering
    /// Returns: (video_name, file_path, index, total, opacity)
    pub fn get_compel_browse_overlay_state(&self) -> Option<(String, String, usize, usize, f32)> {
        if self.browse_compels_opacity < 0.01 {
            return None;
        }

        let idx = self.browse_compels_index?;
        let video_name = self.video_files.get(idx)?.clone();

        // Look up custom name from video metadata cache
        let dir = self.video_directory.as_ref()?;
        let full_path = format!("{}/{}", dir, video_name);
        let display_name = self.video_metadata.get(&full_path)
            .and_then(|meta| meta.custom_name.clone())
            .unwrap_or_else(|| {
                // Strip extension for display
                video_name.strip_suffix(".mp4")
                    .or_else(|| video_name.strip_suffix(".mov"))
                    .or_else(|| video_name.strip_suffix(".webm"))
                    .unwrap_or(&video_name)
                    .to_string()
            });

        Some((
            display_name,
            full_path,
            idx + 1,
            self.video_files.len(),
            self.browse_compels_opacity,
        ))
    }

    /// Truncate text to fit within max_chars, adding ellipsis
    fn truncate_text(text: &str, max_chars: usize) -> String {
        if text.chars().count() <= max_chars {
            text.to_string()
        } else {
            let truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
            format!("{}…", truncated)
        }
    }

    /// Fixed panel width for preset section
    const PRESET_PANEL_WIDTH: f32 = 380.0;
    const PRESET_NAME_MAX_CHARS: usize = 28;

    /// projectM controls section
    fn show_projectm_section(&mut self, ui: &mut egui::Ui, pipeline: &mut Option<RenderPipeline>) {
        // Fixed width panel - never expands
        ui.set_max_width(Self::PRESET_PANEL_WIDTH);

        // Use standard CollapsingHeader for consistency
        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Themes").strong().size(14.0)
        )
        .default_open(self.projectm_expanded)
        .show(ui, |ui| {
            // Controls row: visibility toggle + random button
            ui.horizontal(|ui| {
                // Visibility toggle (eye icon - larger, MIDI learnable)
                let visible = self.params.projectm_visibility > 0.5;
                let eye_color = if self.midi_learn_armed {
                    let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                    egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                } else if visible {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(120, 120, 120)
                };
                let eye_tooltip = if self.midi_learn_armed {
                    "Click to map Toggle Themes to MIDI"
                } else if visible {
                    "Hide visuals (V)"
                } else {
                    "Show visuals (V)"
                };
                let eye_response = ui.add(
                    egui::Button::new(egui::RichText::new("👁").size(20.0).color(eye_color))
                        .fill(egui::Color32::TRANSPARENT)
                        .min_size(egui::vec2(32.0, 28.0))
                ).on_hover_text(eye_tooltip);

                if eye_response.clicked() {
                    if self.midi_learn_armed {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleProjectM);
                    } else {
                        self.params.projectm_visibility = if visible { 0.0 } else { 1.0 };
                        self.mark_dirty();
                    }
                }

                // Random preset button (dice icon - next to eye)
                // MIDI learnable: click when in learn mode to map
                let dice_btn = if self.midi_learn_armed {
                    let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                    let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (200.0 + 55.0 * pulse as f32) as u8);
                    ui.add(
                        egui::Button::new(egui::RichText::new("🎲").size(20.0).color(highlight))
                            .min_size(egui::vec2(32.0, 28.0))
                    ).on_hover_text("Click to map Random Preset to MIDI")
                } else {
                    ui.add(
                        egui::Button::new(egui::RichText::new("🎲").size(20.0))
                            .min_size(egui::vec2(32.0, 28.0))
                    ).on_hover_text("Random preset (R)")
                };

                if dice_btn.clicked() {
                    if self.midi_learn_armed {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::RandomPreset);
                    } else if let Some(ref mut p) = pipeline {
                        // Use the same method as MIDI control for consistency
                        if self.set_random_preset(p) {
                            self.search_query.clear();
                        }
                    }
                }

                // Auto-randomize toggle (recycle icon - sits next to dice)
                let auto_color = if self.auto_randomize_theme {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(120, 120, 120)
                };
                let auto_tooltip = if self.auto_randomize_theme {
                    "Auto-randomize ON (every 60s) — click to disable"
                } else {
                    "Auto-randomize theme every 60s"
                };
                let auto_btn = ui.add(
                    egui::Button::new(egui::RichText::new("🔀").size(20.0).color(auto_color))
                        .fill(egui::Color32::TRANSPARENT)
                        .min_size(egui::vec2(32.0, 28.0))
                ).on_hover_text(auto_tooltip);
                if auto_btn.clicked() {
                    self.auto_randomize_theme = !self.auto_randomize_theme;
                    self.auto_randomize_next = if self.auto_randomize_theme {
                        Some(std::time::Instant::now() + Self::AUTO_RANDOMIZE_INTERVAL)
                    } else {
                        None
                    };
                }

                // Show hidden toggle (right-aligned)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let hidden_color = if self.show_hidden_presets {
                        egui::Color32::from_rgb(180, 100, 180)
                    } else {
                        egui::Color32::from_rgb(80, 80, 80)
                    };
                    if ui.add(
                        egui::Button::new(egui::RichText::new("🙈").size(16.0).color(hidden_color))
                            .fill(egui::Color32::TRANSPARENT)
                            .min_size(egui::vec2(24.0, 24.0))
                    ).on_hover_text(if self.show_hidden_presets { "Hide hidden themes" } else { "Show hidden themes" }).clicked() {
                        self.show_hidden_presets = !self.show_hidden_presets;
                    }
                });
            });

            // Theme Visibility slider - MIDI learnable (mirrors Video Visibility in show_video_section)
            ui.horizontal(|ui| {
                let vis_icon_color = if self.midi_learn_armed {
                    let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                    egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                } else {
                    egui::Color32::from_rgb(150, 150, 150)
                };
                ui.label(egui::RichText::new(egui_phosphor::regular::EYE).color(vis_icon_color))
                    .on_hover_text("Theme Visibility");

                let old_vis = self.params.projectm_visibility;

                // Yellow pulsing slider when in learn mode
                let slider_response = if self.midi_learn_armed {
                    let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                    let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                    let mut style = (*ui.ctx().style()).clone();
                    style.visuals.widgets.inactive.bg_fill = yellow;
                    style.visuals.widgets.hovered.bg_fill = yellow;
                    style.visuals.widgets.active.bg_fill = yellow;
                    ui.ctx().set_style(style.clone());

                    let response = ui.add(egui::Slider::new(&mut self.params.projectm_visibility, 0.0..=1.0)
                        .show_value(false).fixed_decimals(2))
                        .on_hover_text("Click to map Theme Visibility to MIDI fader");

                    ui.ctx().set_style(egui::Style::default());
                    response
                } else {
                    ui.add(egui::Slider::new(&mut self.params.projectm_visibility, 0.0..=1.0)
                        .show_value(false).fixed_decimals(2))
                };

                if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                    self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ProjectMVisibility);
                    self.params.projectm_visibility = old_vis;
                } else if (self.params.projectm_visibility - old_vis).abs() > 0.001 {
                    self.mark_dirty();
                }
            });

            ui.add_space(4.0);

            // Search box - works for all presets (projectM + LED Wall + future renderers)
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Search...")
                        .desired_width(ui.available_width() - 24.0)
                );

                // Clear button
                if !self.search_query.is_empty() {
                    if ui.small_button("x").clicked() {
                        self.search_query.clear();
                    }
                }
            });

            // Refresh favorites if needed
            if self.favorites_dirty {
                self.refresh_favorites();
            }

            // Category dropdown - unified across all renderers
            ui.add_space(4.0);

            if let Some(ref mut p) = pipeline {
                // Load categories from all renderers if empty
                if self.categories.is_empty() {
                    self.categories = p.get_preset_categories();
                    log::info!("[UI] Loaded {} categories", self.categories.len());
                }

                ui.horizontal(|ui| {
                    ui.label("Category:");
                    // Refresh button
                    if ui.add(
                        egui::Button::new(egui::RichText::new("↻").small())
                            .fill(egui::Color32::TRANSPARENT)
                            .min_size(egui::vec2(18.0, 18.0))
                    ).on_hover_text("Refresh categories").clicked() {
                        self.categories = p.get_preset_categories();
                        log::info!("[UI] Refreshed categories: {} found", self.categories.len());
                    }
                    egui::ComboBox::from_id_salt("preset_category")
                        .selected_text(
                            self.selected_category.as_deref().unwrap_or("Select...")
                        )
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            // Favorites at top
                            let fav_label = format!("★ Favorites ({})", self.favorites.len());
                            if ui.selectable_label(
                                self.selected_category.as_deref() == Some("★ Favorites"),
                                &fav_label
                            ).clicked() {
                                self.selected_category = Some("★ Favorites".to_string());
                                self.presets.clear();
                                self.search_query.clear();
                            }

                            ui.separator();

                            // All categories from all renderers
                            for cat in &self.categories.clone() {
                                if ui.selectable_label(
                                    self.selected_category.as_ref() == Some(cat),
                                    cat.as_str()
                                ).clicked() {
                                    self.selected_category = Some(cat.clone());
                                    self.presets = p.get_presets_in_category(cat);
                                    self.search_query.clear();
                                }
                            }
                        });
                });

                // === HERO SKIN (Geometric Field only) ===
                if self.render_mode == RenderMode::HarmonicGeometry && self.selected_category.is_some() {
                    // Apply loaded skin settings from database on first frame
                    // (must happen outside collapsing header so it runs even when collapsed)
                    if self.skin_pending_apply {
                        if let Some(skin) = p.get_harmonic_skin_mut() {
                            skin.mode = crate::render::SkinMode::from_i32(self.skin_mode);
                            skin.surface_mix = self.skin_surface_mix;
                            skin.emissive_strength = self.skin_emissive_strength;
                            skin.surf_contrast = self.skin_surf_contrast;
                            skin.surf_saturation = self.skin_surf_saturation;
                            skin.shadow_lift = self.skin_shadow_lift;
                            skin.ao_clamp = self.skin_ao_clamp;
                            skin.rim_strength = self.skin_rim_strength;
                            skin.bloom_amount = self.skin_bloom_amount;
                            skin.specular_sparkle = self.skin_specular_sparkle;
                            skin.edge_outline = self.skin_edge_outline;
                            skin.uv_mode = self.skin_uv_mode;
                            skin.starburst_enabled = self.skin_starburst_enabled;
                            skin.lock_to_dominant = self.skin_lock_to_dominant;
                            log::info!("[UI] Applied loaded skin settings to renderer");
                        }
                        self.skin_pending_apply = false;
                    }

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Hero Skin:").size(11.0));

                        // Simple toggle: Hero On/Off based on actual skin mode (not preset index)
                        let hero_enabled = self.skin_mode != 0; // 0 = SkinMode::None
                        if ui.selectable_label(hero_enabled, if hero_enabled { "On" } else { "Off" }).clicked() {
                            if hero_enabled {
                                self.skin_mode = 0; // SkinMode::None
                                p.disable_hero_skin();
                                self.mark_dirty();
                            } else {
                                self.skin_mode = 3; // SkinMode::SurfaceEmissive (Hero default)
                                p.enable_hero_skin();
                                self.mark_dirty();
                            }
                        }
                    });

                    // Advanced skin settings (collapsible)
                    let advanced_header = egui::CollapsingHeader::new(
                        egui::RichText::new("Advanced Skin").size(10.0).color(egui::Color32::from_rgb(120, 120, 140))
                    )
                    .default_open(self.skin_advanced_expanded)
                    .show(ui, |ui| {
                        if let Some(skin) = p.get_harmonic_skin_mut() {
                            // Mode dropdown
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Mode:").size(10.0))
                                    .on_hover_text("How the video skin is applied:\n• None: No skin, pure geometry\n• Surface: Video as albedo texture\n• Emissive: Video drives glow only\n• Both: Combined surface + emissive (recommended)");
                                let modes = ["None", "Surface", "Emissive", "Both"];
                                let mode_idx = skin.mode.as_i32() as usize;
                                egui::ComboBox::from_id_salt("skin_mode_adv")
                                    .selected_text(*modes.get(mode_idx).unwrap_or(&"None"))
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for (idx, mode) in modes.iter().enumerate() {
                                            if ui.selectable_label(mode_idx == idx, *mode).clicked() {
                                                skin.mode = crate::render::SkinMode::from_i32(idx as i32);
                                                self.skin_mode = idx as i32;
                                                self.skin_preset_index = 0;
                                                self.mark_dirty();
                                            }
                                        }
                                    });
                            });

                            // === PRIMARY CONTROLS: Video Visibility ===
                            ui.label(egui::RichText::new("Video Visibility").size(10.0).color(egui::Color32::from_rgb(180, 180, 200)));

                            // Surface Mix (the main "Skin Visibility" slider)
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Surface Mix:").size(10.0))
                                    .on_hover_text("How much the video texture replaces the base geometry color.\n0 = geometry only, 1 = full video texture.");
                                if ui.add(egui::Slider::new(&mut skin.surface_mix, 0.0..=1.0).show_value(false)).changed() {
                                    self.skin_surface_mix = skin.surface_mix;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Emissive (the main "Skin Glow" slider)
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Skin Glow:").size(10.0))
                                    .on_hover_text("Makes the video texture emit light, creating a glowing effect.\nHigher values make bright areas of the video glow intensely.");
                                if ui.add(egui::Slider::new(&mut skin.emissive_strength, 0.0..=1.5).show_value(false)).changed() {
                                    self.skin_emissive_strength = skin.emissive_strength;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Grade").size(10.0).color(egui::Color32::from_rgb(180, 180, 200)));

                            // Contrast
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Contrast:").size(10.0))
                                    .on_hover_text("Adjusts the difference between light and dark areas.\n1.0 = neutral, >1.0 = punchier, <1.0 = flatter.");
                                if ui.add(egui::Slider::new(&mut skin.surf_contrast, 0.5..=2.0).show_value(false)).changed() {
                                    self.skin_surf_contrast = skin.surf_contrast;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Saturation
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Saturation:").size(10.0))
                                    .on_hover_text("Color intensity of the video texture.\n0 = grayscale, 1.0 = neutral, >1.0 = vivid colors.");
                                if ui.add(egui::Slider::new(&mut skin.surf_saturation, 0.0..=2.0).show_value(false)).changed() {
                                    self.skin_surf_saturation = skin.surf_saturation;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Shadow Lift
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Shadow Lift:").size(10.0))
                                    .on_hover_text("Raises the brightness of dark areas in the video.\nPrevents ambient occlusion from crushing video details in shadows.");
                                if ui.add(egui::Slider::new(&mut skin.shadow_lift, 0.0..=0.25).show_value(false)).changed() {
                                    self.skin_shadow_lift = skin.shadow_lift;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // AO Clamp
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("AO Floor:").size(10.0))
                                    .on_hover_text("Minimum ambient occlusion value - prevents crevices from going pure black.\nHigher values preserve video detail in geometry folds.");
                                if ui.add(egui::Slider::new(&mut skin.ao_clamp, 0.0..=0.6).show_value(false)).changed() {
                                    self.skin_ao_clamp = skin.ao_clamp;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Effects").size(10.0).color(egui::Color32::from_rgb(180, 180, 200)));

                            // Rim
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Rim:").size(10.0))
                                    .on_hover_text("Fresnel rim lighting - creates a glowing edge effect on geometry silhouettes.\nMakes the shape pop against the background.");
                                if ui.add(egui::Slider::new(&mut skin.rim_strength, 0.0..=1.0).show_value(false)).changed() {
                                    self.skin_rim_strength = skin.rim_strength;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Bloom
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Bloom:").size(10.0))
                                    .on_hover_text("Soft glow around bright areas - simulates camera lens diffusion.\nAdds a dreamy, cinematic quality.");
                                if ui.add(egui::Slider::new(&mut skin.bloom_amount, 0.0..=1.0).show_value(false)).changed() {
                                    self.skin_bloom_amount = skin.bloom_amount;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Sparkle
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Sparkle:").size(10.0))
                                    .on_hover_text("Specular highlights that shimmer and sparkle on the surface.\nAdds a metallic or glittery quality to the skin.");
                                if ui.add(egui::Slider::new(&mut skin.specular_sparkle, 0.0..=1.0).show_value(false)).changed() {
                                    self.skin_specular_sparkle = skin.specular_sparkle;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Edge
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Edge:").size(10.0))
                                    .on_hover_text("Dark outline around geometry edges - adds definition and a stylized look.\nUseful for making shapes more distinct.");
                                if ui.add(egui::Slider::new(&mut skin.edge_outline, 0.0..=1.0).show_value(false)).changed() {
                                    self.skin_edge_outline = skin.edge_outline;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Options").size(10.0).color(egui::Color32::from_rgb(180, 180, 200)));

                            // UV Mode dropdown
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("UV Map:").size(10.0))
                                    .on_hover_text("How the video texture is mapped onto the 3D geometry:\n• Cylindrical: Wraps around like a label on a bottle\n• Triplanar: Projects from 3 directions, better for complex shapes");
                                let uv_modes = ["Cylindrical", "Triplanar"];
                                let uv_idx = skin.uv_mode as usize;
                                egui::ComboBox::from_id_salt("skin_uv_mode")
                                    .selected_text(*uv_modes.get(uv_idx).unwrap_or(&"Cylindrical"))
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for (idx, mode) in uv_modes.iter().enumerate() {
                                            if ui.selectable_label(uv_idx == idx, *mode).clicked() {
                                                skin.uv_mode = idx as i32;
                                                self.skin_uv_mode = idx as i32;
                                                self.skin_preset_index = 0;
                                                self.mark_dirty();
                                            }
                                        }
                                    });
                            });

                            // Toggles
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut skin.starburst_enabled, "Starburst")
                                    .on_hover_text("Adds lens flare streaks radiating from bright spots.\nCreates a dramatic, cinematic light effect.")
                                    .changed()
                                {
                                    self.skin_starburst_enabled = skin.starburst_enabled;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                                if ui.checkbox(&mut skin.lock_to_dominant, "Lock Pitch")
                                    .on_hover_text("Rotates the texture so the dominant pitch is always at the top.\nOff: Video flows naturally with fixed orientation.\nOn: Texture rotates with the music - dominant note always at top.")
                                    .changed()
                                {
                                    self.skin_lock_to_dominant = skin.lock_to_dominant;
                                    self.skin_preset_index = 0;
                                    self.mark_dirty();
                                }
                            });

                            // Mode dropdown (less prominent now)
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Mode:").size(10.0))
                                    .on_hover_text("How the video skin is applied:\n• None: No skin, pure geometry\n• Surface: Video as albedo texture\n• Emissive: Video drives glow only\n• Both: Combined surface + emissive (recommended)");
                                let modes = ["None", "Surface", "Emissive", "Both"];
                                let mode_idx = skin.mode.as_i32() as usize;
                                egui::ComboBox::from_id_salt("skin_mode_adv_bottom")
                                    .selected_text(*modes.get(mode_idx).unwrap_or(&"None"))
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for (idx, mode) in modes.iter().enumerate() {
                                            if ui.selectable_label(mode_idx == idx, *mode).clicked() {
                                                skin.mode = crate::render::SkinMode::from_i32(idx as i32);
                                                self.skin_mode = idx as i32;
                                                self.skin_preset_index = 0;
                                                self.mark_dirty();
                                            }
                                        }
                                    });
                            });
                        }
                    });
                    self.skin_advanced_expanded = advanced_header.fully_open();
                }
            }

            ui.add_space(4.0);

            // Preset list - unified for all renderers
            if let Some(ref mut p) = pipeline {
                let is_favorites = self.selected_category.as_deref() == Some("★ Favorites");
                let is_searching = !self.search_query.is_empty();

                if is_searching {
                    // Fuzzy search mode
                    let scroll_to = self.scroll_to_preset;

                    let mut matches: Vec<(i64, String, String)> = Vec::new();
                    let query = self.search_query.to_lowercase();

                    for cat in &self.categories.clone() {
                        let presets_in_cat = p.get_presets_in_category(cat);
                        for preset in presets_in_cat {
                            // Skip hidden presets unless showing hidden
                            if !self.show_hidden_presets {
                                if let Some(fp) = p.get_preset_path(cat, &preset) {
                                    if self.is_preset_hidden(&fp) {
                                        continue;
                                    }
                                }
                            }
                            let search_text = format!("{} {}", cat, preset);
                            if let Some(score) = self.fuzzy_matcher.fuzzy_match(&search_text, &query) {
                                matches.push((score, cat.clone(), preset));
                            }
                        }
                    }

                    matches.sort_by(|a, b| b.0.cmp(&a.0));
                    let matches: Vec<_> = matches.into_iter().take(50).collect();

                    if matches.is_empty() {
                        ui.label(egui::RichText::new("No matches").color(egui::Color32::GRAY));
                    } else {
                        ui.label(egui::RichText::new(format!("{} results", matches.len())).small().color(egui::Color32::GRAY));
                        egui::ScrollArea::vertical()
                            .max_height(400.0)
                            .show(ui, |ui| {
                                for (_score, cat, preset) in &matches {
                                    let is_current = self.current_preset.as_ref()
                                        .map(|(c, n)| c == cat && n == preset)
                                        .unwrap_or(false);

                                    let response = self.render_preset_row(ui, p, cat, preset, is_current, true);

                                    if is_current && scroll_to {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                    }
                    if scroll_to {
                        self.scroll_to_preset = false;
                    }
                } else if is_favorites {
                    // Favorites list - unified across all renderers
                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .show(ui, |ui| {
                            // === PINNED SECTION ===
                            // Hero's Journey is always first
                            let hj_is_current = self.current_preset.as_ref()
                                .map(|(c, n)| c == "Geometric Field" && n == "Hero's Journey")
                                .unwrap_or(false);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("📌").small());
                                self.render_preset_row(ui, p, "Geometric Field", "Hero's Journey", hj_is_current, true);
                            });

                            // Collect pinned presets (excluding LED Wall and Hero's Journey)
                            let mut all_pinned: Vec<(String, String)> = self.pinned_presets
                                .iter()
                                .filter(|(cat, preset)| {
                                    cat != "LED Wall" && !(cat == "Geometric Field" && preset == "Hero's Journey")
                                })
                                .cloned()
                                .collect();

                            // Add presets from projectM "Pinned" folder (if not already in database pins)
                            let folder_pinned = p.get_presets_in_category("Pinned");
                            for preset_name in folder_pinned {
                                if !all_pinned.iter().any(|(c, n)| c == "Pinned" && n == &preset_name) {
                                    all_pinned.push(("Pinned".to_string(), preset_name));
                                }
                            }

                            // Show remaining pinned presets
                            for (cat, preset) in &all_pinned {
                                let is_current = self.current_preset.as_ref()
                                    .map(|(c, n)| c == cat && n == preset)
                                    .unwrap_or(false);

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("📌").small());
                                    self.render_preset_row(ui, p, cat, preset, is_current, true);
                                });
                            }

                            if !self.favorites.is_empty() {
                                ui.separator();
                            }

                            // User favorites - works for ALL renderer types (excluding LED Wall)
                            for (cat, preset) in self.favorites.clone() {
                                // Skip LED Wall presets
                                if cat == "LED Wall" {
                                    continue;
                                }

                                // Skip pinned presets (shown above) including Hero's Journey
                                if cat == "Geometric Field" && preset == "Hero's Journey" {
                                    continue;
                                }
                                if all_pinned.iter().any(|(pc, pn)| pc == &cat && pn == &preset) {
                                    continue;
                                }

                                // Skip hidden presets unless showing hidden
                                if !self.show_hidden_presets {
                                    if let Some(fp) = p.get_preset_path(&cat, &preset) {
                                        if self.is_preset_hidden(&fp) {
                                            continue;
                                        }
                                    }
                                }

                                let is_current = self.current_preset.as_ref()
                                    .map(|(c, n)| c == &cat && n == &preset)
                                    .unwrap_or(false);

                                self.render_preset_row(ui, p, &cat, &preset, is_current, true);
                            }
                        });
                } else if !self.presets.is_empty() {
                    // Regular preset list
                    let scroll_to = self.scroll_to_preset;
                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .show(ui, |ui| {
                            for preset in &self.presets.clone() {
                                if let Some(ref cat) = self.selected_category.clone() {
                                    // Skip hidden presets unless showing hidden
                                    if !self.show_hidden_presets {
                                        if let Some(fp) = p.get_preset_path(cat, preset) {
                                            if self.is_preset_hidden(&fp) {
                                                continue;
                                            }
                                        }
                                    }

                                    let is_current = self.current_preset.as_ref()
                                        .map(|(c, n)| c == cat && n == preset)
                                        .unwrap_or(false);

                                    let response = self.render_preset_row(ui, p, cat, preset, is_current, false);

                                    if is_current && scroll_to {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            }
                        });
                    if scroll_to {
                        self.scroll_to_preset = false;
                    }
                }

                // Footer - current preset (inside pipeline block for subcategory access)
                if let Some((ref cat, ref preset)) = self.current_preset {
                    ui.add_space(4.0);
                    let display = Self::truncate_text(preset, 25);

                    // Build tooltip with subcategory hierarchy
                    let subcategory = p.get_preset_subcategory(cat, preset);
                    let tooltip = {
                        let mut parts = vec![cat.to_string()];
                        if let Some(ref sub) = subcategory {
                            for part in sub.split('/') {
                                if !part.is_empty() {
                                    parts.push(part.to_string());
                                }
                            }
                        }
                        parts.push(preset.to_string());
                        parts.join(" > ")
                    };

                    ui.label(egui::RichText::new(format!("> {}", display)).small().color(egui::Color32::from_rgb(100, 200, 100)))
                        .on_hover_text(&tooltip);
                }
            }
        });

        // Track expanded state
        self.projectm_expanded = header.fully_open();

        // Track management popup window
        self.show_track_management_window(ui);
    }

    /// Show track management popup window
    fn show_track_management_window(&mut self, ui: &mut egui::Ui) {
        let mut close_window = false;

        if let Some(ref preset_path) = self.managing_preset_tracks.clone() {
            egui::Window::new("Manage Theme Tracks")
                .collapsible(false)
                .resizable(true)
                .default_width(350.0)
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        match mac_traffic_lights(ui) {
                            TrafficLightAction::Close => close_window = true,
                            TrafficLightAction::Minimize => {
                                // Small utility window — minimize just closes
                                close_window = true;
                            }
                            _ => {}
                        }
                        ui.label(egui::RichText::new("Associated Tracks").strong());
                    });
                    ui.add_space(4.0);

                    // List existing tracks
                    if self.preset_tracks.is_empty() {
                        ui.label(egui::RichText::new("No tracks associated").color(egui::Color32::GRAY).italics());
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                let mut track_to_remove: Option<(String, Option<String>)> = None;
                                for track in &self.preset_tracks {
                                    ui.horizontal(|ui| {
                                        // Track info
                                        let track_display = if let Some(ref artist) = track.artist {
                                            format!("{} - {}", track.name, artist)
                                        } else {
                                            track.name.clone()
                                        };
                                        ui.label(&track_display);

                                        // Delete button
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("❌").on_hover_text("Remove track").clicked() {
                                                track_to_remove = Some((track.name.clone(), track.artist.clone()));
                                            }
                                        });
                                    });
                                }

                                // Remove track if requested
                                if let Some((name, artist)) = track_to_remove {
                                    if let Some(ref db) = self.db {
                                        let _ = db.remove_preset_track(&preset_path, &name, artist.as_deref());
                                        self.preset_tracks = db.get_preset_tracks(&preset_path);
                                    }
                                }
                            });
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // Add new track
                    ui.label(egui::RichText::new("Add Track").strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Track:");
                        ui.add(egui::TextEdit::singleline(&mut self.new_track_name)
                            .hint_text("Track name")
                            .desired_width(150.0));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Artist:");
                        ui.add(egui::TextEdit::singleline(&mut self.new_track_artist)
                            .hint_text("Artist (optional)")
                            .desired_width(150.0));
                    });

                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        if ui.button("Add Track").clicked() {
                            let name = self.new_track_name.trim();
                            if !name.is_empty() {
                                if let Some(ref db) = self.db {
                                    let artist = if self.new_track_artist.trim().is_empty() {
                                        None
                                    } else {
                                        Some(self.new_track_artist.trim())
                                    };
                                    let _ = db.add_preset_track(&preset_path, name, artist, None, 0);
                                    self.preset_tracks = db.get_preset_tracks(&preset_path);
                                    self.new_track_name.clear();
                                    self.new_track_artist.clear();
                                }
                            }
                        }
                    });
                });
        }

        if close_window {
            self.managing_preset_tracks = None;
            self.preset_tracks.clear();
            self.new_track_name.clear();
            self.new_track_artist.clear();
        }
    }

    /// Show video sync window with progress (database-backed, non-blocking)
    fn render_sync_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_sync_window {
            return;
        }

        // Check if a download completed - refresh video list if so
        let mut needs_refresh = if let Some(ref sync) = self.video_sync {
            let completed = sync.get_completed_count();
            if completed > self.last_sync_completed {
                self.last_sync_completed = completed;
                true
            } else {
                false
            }
        } else {
            false
        };

        // Also check compel sync completions
        if let Some(ref sync) = self.compel_sync {
            let completed = sync.get_completed_count();
            if completed > self.last_compel_sync_completed {
                self.last_compel_sync_completed = completed;
                needs_refresh = true;
            }
        }

        if needs_refresh {
            if let Some(ref dir) = self.video_directory.clone() {
                self.scan_video_directory(&dir);
            }
        }

        let win_key = "sync";
        let is_minimized = self.minimized_windows.contains(win_key);
        let is_fullscreen = self.fullscreen_window.as_deref() == Some(win_key);

        let mut close_window = false;
        let mut toggle_minimize = false;
        let mut toggle_fullscreen = false;

        let ctx = ui.ctx().clone();
        let mut window = egui::Window::new(format!("{} Sync", egui_phosphor::regular::CLOUD_ARROW_DOWN))
            .collapsible(false)
            .resizable(!is_fullscreen || is_minimized);
        if is_fullscreen && !is_minimized {
            let rect = ctx.available_rect();
            if self.half_screen_mode {
                let half_width = rect.width() / 2.0;
                window = window.fixed_pos(rect.min)
                               .fixed_size(egui::vec2(half_width, rect.height()));
            } else {
                window = window.fixed_pos(rect.min).fixed_size(rect.size());
            }
        } else {
            window = window.default_width(480.0).default_height(500.0);
        }
        window.show(&ctx, |ui| {
                // Traffic light buttons always visible
                ui.horizontal(|ui| {
                    match mac_traffic_lights(ui) {
                        TrafficLightAction::Close => close_window = true,
                        TrafficLightAction::Minimize => toggle_minimize = true,
                        TrafficLightAction::Fullscreen => toggle_fullscreen = true,
                        TrafficLightAction::None => {}
                    }
                    ui.label(egui::RichText::new("Sync").strong().size(14.0));
                });

                if is_minimized {
                    return; // Only show traffic lights row when minimized
                }

                if let (Some(ref sync), Some(ref db)) = (&self.video_sync, &self.db) {
                    let is_running = sync.is_running();
                    let (current_name, current_progress, current_speed) = sync.get_current_download();
                    let videos = db.get_sync_videos();
                    let (pending, _downloading, completed, failed, paused, total_bytes, downloaded_bytes) = db.get_sync_stats();
                    let resumable = pending + paused;
                    let video_dir = self.video_directory.clone();

                    // Also gather compel data
                    let compel_data = if let Some(ref compel_sync) = &self.compel_sync {
                        let c_running = compel_sync.is_running();
                        let (c_name, c_progress, c_speed) = compel_sync.get_current_download();
                        let c_videos = db.get_compel_videos();
                        let (cp, _cd, cc, cf, _ct, c_total_bytes, c_downloaded_bytes) = db.get_compel_sync_stats();
                        Some((c_running, c_name, c_progress, c_speed, c_videos, cp, cc, cf, c_total_bytes, c_downloaded_bytes))
                    } else {
                        None
                    };

                    // === COMBINED OVERALL PROGRESS ===
                    let combined_total = total_bytes + compel_data.as_ref().map(|d| d.8).unwrap_or(0);
                    let combined_downloaded = downloaded_bytes + compel_data.as_ref().map(|d| d.9).unwrap_or(0);
                    let combined_speed = current_speed + compel_data.as_ref().map(|d| d.3).unwrap_or(0);
                    let any_running = is_running || compel_data.as_ref().map(|d| d.0).unwrap_or(false);

                    ui.add_space(6.0);

                    if combined_total > 0 {
                        let progress = combined_downloaded as f32 / combined_total as f32;
                        let file_count = videos.len() + compel_data.as_ref().map(|d| d.4.len()).unwrap_or(0);
                        let done_count = completed + compel_data.as_ref().map(|d| d.6).unwrap_or(0);
                        ui.add(egui::ProgressBar::new(progress)
                            .text(format!("{} / {}  ({}/{})",
                                format_bytes(combined_downloaded),
                                format_bytes(combined_total),
                                done_count,
                                file_count)));
                    }

                    if combined_speed > 0 {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{} {}", egui_phosphor::regular::ARROW_DOWN, format_speed(combined_speed)))
                                .color(egui::Color32::from_rgb(100, 200, 255)).size(12.0));
                        });
                    }

                    ui.add_space(4.0);

                    // === CONTROL BUTTONS ===
                    ui.horizontal(|ui| {
                        if is_running {
                            if ui.button(format!("{} Cancel", egui_phosphor::regular::STOP)).clicked() {
                                sync.cancel();
                            }
                        } else if videos.is_empty() {
                            if ui.button(format!("{} Fetch Videos", egui_phosphor::regular::CLOUD_ARROW_DOWN)).clicked() {
                                sync.refresh_list(db.clone());
                            }
                        } else {
                            if resumable > 0 {
                                let btn_text = if paused > 0 {
                                    format!("{} Resume ({})", egui_phosphor::regular::PLAY, resumable)
                                } else {
                                    format!("{} Download ({})", egui_phosphor::regular::PLAY, resumable)
                                };
                                if ui.button(btn_text).clicked() {
                                    sync.start_sync(db.clone());
                                }
                            }
                            if ui.button(format!("{} Refresh", egui_phosphor::regular::ARROWS_CLOCKWISE)).clicked() {
                                sync.refresh_list(db.clone());
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.separator();

                    // === SCROLLABLE FILE LIST ===
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {

                            // Helper: render a section of videos grouped by status
                            let render_video_group = |ui: &mut egui::Ui,
                                                      label: &str,
                                                      icon: &str,
                                                      header_color: egui::Color32,
                                                      items: &[&SyncVideoInfo],
                                                      cur_name: &Option<String>,
                                                      cur_prog: u64,
                                                      cur_spd: u64,
                                                      vdir: &Option<String>,
                                                      default_open: bool| {
                                if items.is_empty() { return; }
                                let total_size: u64 = items.iter().map(|v| v.remote_size).sum();
                                let header = format!("{} {} ({}) - {}", icon, label, items.len(), format_bytes(total_size));
                                egui::CollapsingHeader::new(egui::RichText::new(header).color(header_color).size(12.0))
                                    .default_open(default_open)
                                    .show(ui, |ui| {
                                        for video in items {
                                            let file_missing = video.status == SyncVideoStatus::Completed
                                                && vdir.as_ref().map(|d| !std::path::Path::new(d).join(&video.name).exists()).unwrap_or(false);

                                            // Truncate name safely at char boundary
                                            let max_chars = 40;
                                            let display_name = if video.name.chars().count() > max_chars {
                                                let end = video.name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(video.name.len());
                                                format!("{}...", &video.name[..end])
                                            } else {
                                                video.name.clone()
                                            };

                                            ui.horizontal(|ui| {
                                                // Status indicator dot
                                                let dot_color = if file_missing {
                                                    egui::Color32::from_rgb(255, 180, 50)
                                                } else {
                                                    match video.status {
                                                        SyncVideoStatus::Completed => egui::Color32::from_rgb(80, 200, 80),
                                                        SyncVideoStatus::Downloading => egui::Color32::from_rgb(100, 200, 255),
                                                        SyncVideoStatus::Pending => egui::Color32::from_rgb(100, 100, 110),
                                                        SyncVideoStatus::Paused => egui::Color32::YELLOW,
                                                        SyncVideoStatus::Failed => egui::Color32::from_rgb(255, 80, 80),
                                                    }
                                                };
                                                let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                                ui.painter().circle_filled(r.center(), 4.0, dot_color);

                                                ui.label(egui::RichText::new(&display_name).size(12.0));

                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(egui::RichText::new(format_bytes(video.remote_size))
                                                        .size(10.0).color(egui::Color32::from_rgb(120, 120, 130)));
                                                });
                                            });

                                            // Progress bar for active downloads
                                            if video.status == SyncVideoStatus::Downloading {
                                                let progress = if cur_name.as_ref() == Some(&video.name) {
                                                    cur_prog as f32 / video.remote_size.max(1) as f32
                                                } else {
                                                    video.progress()
                                                };
                                                let speed_text = if cur_name.as_ref() == Some(&video.name) && cur_spd > 0 {
                                                    format!("{}  {}", format!("{:.0}%", progress * 100.0), format_speed(cur_spd))
                                                } else {
                                                    format!("{:.0}%", progress * 100.0)
                                                };
                                                ui.add(egui::ProgressBar::new(progress)
                                                    .desired_width(ui.available_width() - 16.0)
                                                    .text(speed_text));
                                                ui.add_space(2.0);
                                            }

                                            // Progress bar for paused (partial) downloads
                                            if video.status == SyncVideoStatus::Paused && video.local_size > 0 {
                                                let progress = video.progress();
                                                ui.add(egui::ProgressBar::new(progress)
                                                    .desired_width(ui.available_width() - 16.0)
                                                    .text(format!("{} / {} paused",
                                                        format_bytes(video.local_size),
                                                        format_bytes(video.remote_size)))
                                                    .fill(egui::Color32::from_rgb(180, 160, 40)));
                                                ui.add_space(2.0);
                                            }

                                            // Warning/error details
                                            if file_missing {
                                                ui.label(egui::RichText::new("  file missing from disk")
                                                    .size(10.0).color(egui::Color32::from_rgb(255, 180, 50)));
                                            }
                                            if video.status == SyncVideoStatus::Failed {
                                                if let Some(ref err) = video.error {
                                                    ui.label(egui::RichText::new(format!("  {}", err))
                                                        .size(10.0).color(egui::Color32::from_rgb(255, 120, 120)));
                                                }
                                            }
                                        }
                                    });
                            };

                            // Partition public videos by status group
                            let downloading: Vec<&SyncVideoInfo> = videos.iter().filter(|v| v.status == SyncVideoStatus::Downloading).collect();
                            let queued: Vec<&SyncVideoInfo> = videos.iter().filter(|v| v.status == SyncVideoStatus::Pending || v.status == SyncVideoStatus::Paused).collect();
                            let done: Vec<&SyncVideoInfo> = videos.iter().filter(|v| v.status == SyncVideoStatus::Completed).collect();
                            let errored: Vec<&SyncVideoInfo> = videos.iter().filter(|v| v.status == SyncVideoStatus::Failed).collect();

                            if !videos.is_empty() {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Public Videos").strong().size(13.0));
                                ui.add_space(2.0);
                            }

                            render_video_group(ui, "Downloading", egui_phosphor::regular::ARROW_DOWN, egui::Color32::from_rgb(100, 200, 255),
                                &downloading, &current_name, current_progress, current_speed, &video_dir, true);
                            render_video_group(ui, "Queued", egui_phosphor::regular::CLOCK, egui::Color32::from_rgb(180, 180, 190),
                                &queued, &current_name, current_progress, current_speed, &video_dir, true);
                            render_video_group(ui, "Failed", egui_phosphor::regular::WARNING, egui::Color32::from_rgb(255, 100, 100),
                                &errored, &current_name, current_progress, current_speed, &video_dir, true);
                            render_video_group(ui, "Completed", egui_phosphor::regular::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 80),
                                &done, &current_name, current_progress, current_speed, &video_dir, false);

                            // === MY COMPELS SECTION ===
                            if let Some((c_running, ref c_name, c_progress, c_speed, ref c_videos, cp, cc, cf, _, _)) = compel_data {
                                if !c_videos.is_empty() || cp > 0 {
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new("My Compels").strong().size(13.0));
                                    ui.add_space(2.0);

                                    // Partition compel videos into groups
                                    // Convert CompelVideoInfo refs into a compatible format for rendering
                                    for video in c_videos {
                                        let is_downloading = video.status == SyncVideoStatus::Downloading;
                                        let is_paused = video.status == SyncVideoStatus::Paused;
                                        let is_failed = video.status == SyncVideoStatus::Failed;

                                        let max_chars = 40;
                                        let display_name = if video.name.chars().count() > max_chars {
                                            let end = video.name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(video.name.len());
                                            format!("{}...", &video.name[..end])
                                        } else {
                                            video.name.clone()
                                        };

                                        let dot_color = match video.status {
                                            SyncVideoStatus::Completed => egui::Color32::from_rgb(80, 200, 80),
                                            SyncVideoStatus::Downloading => egui::Color32::from_rgb(100, 200, 255),
                                            SyncVideoStatus::Pending => egui::Color32::from_rgb(100, 100, 110),
                                            SyncVideoStatus::Paused => egui::Color32::YELLOW,
                                            SyncVideoStatus::Failed => egui::Color32::from_rgb(255, 80, 80),
                                        };

                                        ui.horizontal(|ui| {
                                            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                            ui.painter().circle_filled(r.center(), 4.0, dot_color);
                                            ui.label(egui::RichText::new(&display_name).size(12.0));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(egui::RichText::new(format_bytes(video.remote_size))
                                                    .size(10.0).color(egui::Color32::from_rgb(120, 120, 130)));
                                            });
                                        });

                                        if is_downloading {
                                            let progress = if c_name.as_ref() == Some(&video.name) {
                                                c_progress as f32 / video.remote_size.max(1) as f32
                                            } else {
                                                video.progress()
                                            };
                                            let speed_text = if c_name.as_ref() == Some(&video.name) && c_speed > 0 {
                                                format!("{:.0}%  {}", progress * 100.0, format_speed(c_speed))
                                            } else {
                                                format!("{:.0}%", progress * 100.0)
                                            };
                                            ui.add(egui::ProgressBar::new(progress)
                                                .desired_width(ui.available_width() - 16.0)
                                                .text(speed_text));
                                            ui.add_space(2.0);
                                        }
                                        if is_paused && video.local_size > 0 {
                                            let progress = video.progress();
                                            ui.add(egui::ProgressBar::new(progress)
                                                .desired_width(ui.available_width() - 16.0)
                                                .text(format!("{} / {} paused",
                                                    format_bytes(video.local_size),
                                                    format_bytes(video.remote_size)))
                                                .fill(egui::Color32::from_rgb(180, 160, 40)));
                                            ui.add_space(2.0);
                                        }
                                        if is_failed {
                                            if let Some(ref err) = video.error {
                                                ui.label(egui::RichText::new(format!("  {}", err))
                                                    .size(10.0).color(egui::Color32::from_rgb(255, 120, 120)));
                                            }
                                        }
                                    }

                                    if c_running {
                                        ui.ctx().request_repaint();
                                    }
                                }
                            }
                        });

                    // Request repaint while syncing for real-time updates
                    if any_running {
                        ui.ctx().request_repaint();
                    }
                } else {
                    ui.label("No sync directory configured");
                }
            });

        if close_window {
            self.show_sync_window = false;
            if is_fullscreen {
                self.fullscreen_window = None;
                self.hide_menu_for_fullscreen = false;
                self.half_screen_mode = false;
            }
        }
        if toggle_minimize {
            if !self.minimized_windows.remove(win_key) {
                self.minimized_windows.insert(win_key.to_string());
                if is_fullscreen && !self.half_screen_mode {
                    self.hide_menu_for_fullscreen = false;
                }
            } else if is_fullscreen && !self.half_screen_mode {
                self.hide_menu_for_fullscreen = true;
            }
        }
        if toggle_fullscreen {
            self.minimized_windows.remove(win_key);
            if is_fullscreen {
                if self.hide_menu_for_fullscreen {
                    // Fullscreen → Half-screen
                    self.hide_menu_for_fullscreen = false;
                    self.half_screen_mode = true;
                } else {
                    // Half-screen → Normal
                    self.fullscreen_window = None;
                    self.half_screen_mode = false;
                }
            } else {
                // Normal → Fullscreen
                self.fullscreen_window = Some(win_key.to_string());
                self.hide_menu_for_fullscreen = true;
                self.half_screen_mode = false;
            }
        }
    }

    /// Show video track management popup window (internal, called from video section)
    fn show_video_track_management_window(&mut self, ui: &mut egui::Ui) {
        self.render_track_management_window(ui.ctx());
    }

    /// Render video track management popup window (floating, independent of menu)
    pub fn render_track_management_window(&mut self, ctx: &egui::Context) {
        let mut close_window = false;

        if let Some(ref video_path) = self.managing_video_tracks.clone() {
            egui::Window::new("Manage Compel Tracks")
                .collapsible(false)
                .resizable(true)
                .default_width(350.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        match mac_traffic_lights(ui) {
                            TrafficLightAction::Close => close_window = true,
                            TrafficLightAction::Minimize => {
                                // Small utility window — minimize just closes
                                close_window = true;
                            }
                            _ => {}
                        }
                        ui.label(egui::RichText::new("Associated Tracks").strong());
                    });
                    ui.add_space(4.0);

                    // List existing tracks
                    if self.video_tracks.is_empty() {
                        ui.label(egui::RichText::new("No tracks associated").color(egui::Color32::GRAY).italics());
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                let mut track_to_remove: Option<(String, Option<String>)> = None;
                                for track in &self.video_tracks {
                                    ui.horizontal(|ui| {
                                        let track_display = if let Some(ref artist) = track.artist {
                                            format!("{} - {}", track.name, artist)
                                        } else {
                                            track.name.clone()
                                        };
                                        ui.label(&track_display);

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("❌").on_hover_text("Remove track").clicked() {
                                                track_to_remove = Some((track.name.clone(), track.artist.clone()));
                                            }
                                        });
                                    });
                                }

                                if let Some((name, artist)) = track_to_remove {
                                    if let Some(ref db) = self.db {
                                        let _ = db.remove_video_track(&video_path, &name, artist.as_deref());
                                        self.video_tracks = db.get_video_tracks(&video_path);
                                    }
                                }
                            });
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.label(egui::RichText::new("Add Track").strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Track:");
                        ui.add(egui::TextEdit::singleline(&mut self.new_video_track_name)
                            .hint_text("Track name")
                            .desired_width(150.0));
                    });

                    ui.horizontal(|ui| {
                        ui.label("Artist:");
                        ui.add(egui::TextEdit::singleline(&mut self.new_video_track_artist)
                            .hint_text("Artist (optional)")
                            .desired_width(150.0));
                    });

                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        if ui.button("Add Track").clicked() {
                            let name = self.new_video_track_name.trim();
                            if !name.is_empty() {
                                if let Some(ref db) = self.db {
                                    let artist = if self.new_video_track_artist.trim().is_empty() {
                                        None
                                    } else {
                                        Some(self.new_video_track_artist.trim())
                                    };
                                    let _ = db.add_video_track(&video_path, name, artist, None, 0);
                                    self.video_tracks = db.get_video_tracks(&video_path);
                                    self.new_video_track_name.clear();
                                    self.new_video_track_artist.clear();
                                }
                            }
                        }
                    });
                });
        }

        if close_window {
            self.managing_video_tracks = None;
            self.video_tracks.clear();
            self.new_video_track_name.clear();
            self.new_video_track_artist.clear();
        }
    }

    /// Show Compel Manager window for browsing and managing all compels
    pub fn render_compel_manager(&mut self, ctx: &egui::Context, pipeline: &mut Option<RenderPipeline>) {
        if !self.show_compel_manager {
            return;
        }

        let win_key = "compel_manager";
        let is_minimized = self.minimized_windows.contains(win_key);
        let is_fullscreen = self.fullscreen_window.as_deref() == Some(win_key);

        let mut close_window = false;
        let mut toggle_minimize = false;
        let mut toggle_fullscreen = false;
        let mut action_favorite: Option<String> = None;
        let mut action_hide: Option<(String, bool)> = None;
        let mut action_play: Option<String> = None;
        let mut action_manage_tracks: Option<String> = None;
        let mut action_sync = false;
        let mut action_folder = false;
        let mut action_refresh = false;

        let mut window = egui::Window::new(format!("{} Compel Manager", egui_phosphor::regular::SQUARES_FOUR))
            .collapsible(false)
            .resizable(!is_fullscreen || is_minimized);
        if is_fullscreen && !is_minimized {
            let rect = ctx.available_rect();
            if self.half_screen_mode {
                let half_width = rect.width() / 2.0;
                window = window.fixed_pos(rect.min)
                               .fixed_size(egui::vec2(half_width, rect.height()));
            } else {
                window = window.fixed_pos(rect.min).fixed_size(rect.size());
            }
        } else {
            window = window.default_width(800.0).default_height(600.0);
        }
        window.show(ctx, |ui| {
                // Header with traffic light close
                ui.horizontal(|ui| {
                    match mac_traffic_lights(ui) {
                        TrafficLightAction::Close => close_window = true,
                        TrafficLightAction::Minimize => toggle_minimize = true,
                        TrafficLightAction::Fullscreen => toggle_fullscreen = true,
                        TrafficLightAction::None => {}
                    }
                    ui.label(egui::RichText::new(format!("{} compels", self.video_files.len())).strong());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Toggle hidden videos
                        let eye_icon = if self.show_all_videos {
                            egui_phosphor::regular::EYE
                        } else {
                            egui_phosphor::regular::EYE_SLASH
                        };
                        if ui.small_button(eye_icon).on_hover_text(
                            if self.show_all_videos { "Hide hidden videos" } else { "Show hidden videos" }
                        ).clicked() {
                            self.show_all_videos = !self.show_all_videos;
                        }
                    });
                });

                if is_minimized {
                    return; // Only show traffic lights row when minimized
                }

                // Toolbar: Sync, Folder, Refresh
                ui.horizontal(|ui| {
                    // Sync button
                    let is_syncing = self.video_sync.as_ref().map(|s| s.is_running()).unwrap_or(false);
                    let sync_icon = if is_syncing {
                        egui_phosphor::regular::SPINNER
                    } else {
                        egui_phosphor::regular::CLOUD_ARROW_DOWN
                    };
                    if ui.button(format!("{} Sync", sync_icon)).on_hover_text("Sync from Compeller.ai").clicked() {
                        action_sync = true;
                    }

                    // Folder button
                    if ui.button(format!("{} Folder", egui_phosphor::regular::FOLDER_OPEN)).on_hover_text("Select compel folder").clicked() {
                        action_folder = true;
                    }

                    // Refresh button
                    if ui.button(format!("{} Refresh", egui_phosphor::regular::ARROWS_CLOCKWISE)).on_hover_text("Refresh compel list").clicked() {
                        action_refresh = true;
                    }

                    // Show current folder path
                    if let Some(ref dir) = self.video_directory {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let folder_name = dir.split('/').last().unwrap_or(dir);
                            ui.label(egui::RichText::new(format!("📂 {}", folder_name)).small().color(egui::Color32::GRAY));
                        });
                    }
                });

                ui.separator();

                // Search box
                ui.horizontal(|ui| {
                    ui.label(egui_phosphor::regular::MAGNIFYING_GLASS);
                    ui.add(egui::TextEdit::singleline(&mut self.video_search)
                        .hint_text("Search compels...")
                        .desired_width(300.0));
                    if !self.video_search.is_empty() {
                        if ui.small_button(egui_phosphor::regular::X).clicked() {
                            self.video_search.clear();
                        }
                    }
                });

                ui.add_space(8.0);

                // Main content - grid of compels
                let dir = self.video_directory.clone();
                if dir.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(50.0);
                        ui.label(egui::RichText::new("No compel folder selected").size(16.0).color(egui::Color32::GRAY));
                        ui.add_space(10.0);
                        if ui.button(format!("{} Select Folder...", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                let path_str = path.display().to_string();
                                self.video_directory = Some(path_str.clone());
                                self.scan_video_directory(&path_str);
                                if let Some(ref db) = self.db {
                                    let _ = db.set_setting("video_directory", &path_str);
                                }
                                if let Some(ref sync) = self.compel_sync {
                                    sync.set_video_dir(std::path::PathBuf::from(&path_str));
                                }
                            }
                        }
                    });
                    return;
                }

                let dir = dir.unwrap();

                // Build filtered video list
                let search_lower = self.video_search.to_lowercase();
                let videos: Vec<(String, String)> = self.video_files.iter()
                    .map(|name| {
                        let full_path = format!("{}/{}", dir, name);
                        let display_name = self.get_video_display_name(&full_path, name);
                        (full_path, display_name)
                    })
                    .filter(|(full_path, display_name)| {
                        // Filter by search
                        if !search_lower.is_empty() {
                            if !display_name.to_lowercase().contains(&search_lower) {
                                return false;
                            }
                        }
                        // Filter hidden unless showing all
                        if !self.show_all_videos && self.is_video_hidden(full_path) {
                            return false;
                        }
                        true
                    })
                    .collect();

                // Scrollable grid
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);

                            for (full_path, display_name) in &videos {
                                let is_current = self.video_path.as_ref().map(|p| p == full_path).unwrap_or(false);
                                let is_favorite = self.video_favorites.contains(full_path);
                                let is_hidden = self.is_video_hidden(full_path);
                                let is_selected = self.manager_selected_compel.as_ref().map(|s| s == full_path).unwrap_or(false);

                                let thumb_width = 160.0;
                                let thumb_height = 90.0;
                                let total_height = thumb_height + 24.0;

                                // Allocate space
                                let (item_rect, item_response) = ui.allocate_exact_size(
                                    egui::vec2(thumb_width, total_height),
                                    egui::Sense::click()
                                );

                                let thumb_rect = egui::Rect::from_min_size(
                                    item_rect.min,
                                    egui::vec2(thumb_width, thumb_height)
                                );

                                // Background
                                let frame_color = if is_selected {
                                    egui::Color32::from_rgb(60, 80, 120)
                                } else if is_current {
                                    egui::Color32::from_rgb(40, 100, 40)
                                } else if item_response.hovered() {
                                    egui::Color32::from_rgb(50, 50, 60)
                                } else {
                                    egui::Color32::from_rgb(35, 35, 40)
                                };
                                ui.painter().rect_filled(thumb_rect, 6.0, frame_color);

                                // Thumbnail - fit within bounds preserving aspect ratio
                                if let Some(texture) = self.get_or_load_thumbnail(ctx, full_path) {
                                    let tex_size = texture.size_vec2();
                                    let tex_aspect = tex_size.x / tex_size.y;
                                    let thumb_aspect = thumb_width / thumb_height;

                                    // Calculate display rect that fits image within thumb_rect preserving aspect
                                    let display_rect = if tex_aspect > thumb_aspect {
                                        // Image is wider - fit to width, letterbox vertically
                                        let display_height = thumb_width / tex_aspect;
                                        let y_offset = (thumb_height - display_height) / 2.0;
                                        egui::Rect::from_min_size(
                                            thumb_rect.min + egui::vec2(0.0, y_offset),
                                            egui::vec2(thumb_width, display_height)
                                        )
                                    } else {
                                        // Image is taller - fit to height, pillarbox horizontally
                                        let display_width = thumb_height * tex_aspect;
                                        let x_offset = (thumb_width - display_width) / 2.0;
                                        egui::Rect::from_min_size(
                                            thumb_rect.min + egui::vec2(x_offset, 0.0),
                                            egui::vec2(display_width, thumb_height)
                                        )
                                    };

                                    ui.painter().image(
                                        texture.id(),
                                        display_rect,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        if is_hidden { egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100) } else { egui::Color32::WHITE },
                                    );
                                } else {
                                    ui.painter().text(
                                        thumb_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        egui_phosphor::regular::VIDEO,
                                        egui::FontId::proportional(32.0),
                                        egui::Color32::from_rgb(80, 80, 90),
                                    );
                                }

                                // Playing indicator
                                if is_current {
                                    ui.painter().text(
                                        thumb_rect.left_top() + egui::vec2(6.0, 6.0),
                                        egui::Align2::LEFT_TOP,
                                        "▶",
                                        egui::FontId::proportional(16.0),
                                        egui::Color32::from_rgb(100, 255, 100),
                                    );
                                }

                                // Favorite star
                                if is_favorite {
                                    ui.painter().text(
                                        thumb_rect.right_top() + egui::vec2(-6.0, 6.0),
                                        egui::Align2::RIGHT_TOP,
                                        "★",
                                        egui::FontId::proportional(14.0),
                                        egui::Color32::from_rgb(255, 200, 50),
                                    );
                                }

                                // Hidden indicator
                                if is_hidden {
                                    ui.painter().text(
                                        thumb_rect.left_bottom() + egui::vec2(6.0, -6.0),
                                        egui::Align2::LEFT_BOTTOM,
                                        egui_phosphor::regular::EYE_SLASH,
                                        egui::FontId::proportional(12.0),
                                        egui::Color32::from_rgb(150, 150, 150),
                                    );
                                }

                                // Border
                                let border_color = if is_selected {
                                    egui::Color32::from_rgb(100, 150, 255)
                                } else if is_current {
                                    egui::Color32::from_rgb(100, 255, 100)
                                } else {
                                    egui::Color32::from_rgb(60, 60, 65)
                                };
                                ui.painter().rect_stroke(thumb_rect, 6.0, egui::Stroke::new(2.0, border_color));

                                // Name label (truncated at char boundary)
                                let max_chars = 20;
                                let truncated_name = if display_name.chars().count() > max_chars {
                                    let end = display_name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(display_name.len());
                                    format!("{}...", &display_name[..end])
                                } else {
                                    display_name.clone()
                                };
                                let name_color = if is_current {
                                    egui::Color32::from_rgb(100, 255, 100)
                                } else if is_hidden {
                                    egui::Color32::from_rgb(120, 120, 120)
                                } else {
                                    egui::Color32::from_rgb(200, 200, 200)
                                };
                                ui.painter().text(
                                    egui::pos2(item_rect.min.x + 4.0, thumb_rect.max.y + 4.0),
                                    egui::Align2::LEFT_TOP,
                                    &truncated_name,
                                    egui::FontId::proportional(11.0),
                                    name_color,
                                );

                                // Context menu
                                let fp = full_path.clone();
                                let fp2 = full_path.clone();
                                let fp3 = full_path.clone();
                                let fp4 = full_path.clone();
                                let dn = display_name.clone();
                                item_response.clone().context_menu(|ui| {
                                    ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

                                    if ui.button(format!("{} Play", egui_phosphor::regular::PLAY)).clicked() {
                                        action_play = Some(fp.clone());
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    let fav_text = if is_favorite {
                                        format!("{} Remove Favorite", egui_phosphor::regular::STAR)
                                    } else {
                                        format!("{} Add to Favorites", egui_phosphor::regular::STAR)
                                    };
                                    if ui.button(fav_text).clicked() {
                                        action_favorite = Some(fp2.clone());
                                        ui.close_menu();
                                    }

                                    if ui.button(format!("{} Manage Tracks...", egui_phosphor::regular::MUSIC_NOTE)).clicked() {
                                        action_manage_tracks = Some(fp3.clone());
                                        ui.close_menu();
                                    }

                                    if ui.button(format!("{} Rename...", egui_phosphor::regular::PENCIL)).clicked() {
                                        self.editing_video = Some(fp4.clone());
                                        self.editing_video_name = dn.clone();
                                        ui.close_menu();
                                    }

                                    let hide_text = if is_hidden {
                                        format!("{} Show", egui_phosphor::regular::EYE)
                                    } else {
                                        format!("{} Hide", egui_phosphor::regular::EYE_SLASH)
                                    };
                                    if ui.button(hide_text).clicked() {
                                        action_hide = Some((full_path.clone(), !is_hidden));
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    if ui.button(format!("{} Show in Finder", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                                        #[cfg(target_os = "macos")]
                                        {
                                            let _ = std::process::Command::new("open")
                                                .arg("-R")
                                                .arg(full_path)
                                                .spawn();
                                        }
                                        ui.close_menu();
                                    }
                                });

                                // Click to play
                                if item_response.clicked() {
                                    action_play = Some(full_path.clone());
                                }

                                // Double-click could be used for other actions
                                if item_response.double_clicked() {
                                    action_manage_tracks = Some(full_path.clone());
                                }

                                // Hover cursor
                                if item_response.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                            }
                        });
                    });
            });

        // Handle actions outside the closure
        if close_window {
            self.show_compel_manager = false;
            if is_fullscreen {
                self.fullscreen_window = None;
                self.hide_menu_for_fullscreen = false;
                self.half_screen_mode = false;
            }
        }
        if toggle_minimize {
            if !self.minimized_windows.remove(win_key) {
                self.minimized_windows.insert(win_key.to_string());
                if is_fullscreen && !self.half_screen_mode {
                    self.hide_menu_for_fullscreen = false;
                }
            } else if is_fullscreen && !self.half_screen_mode {
                self.hide_menu_for_fullscreen = true;
            }
        }
        if toggle_fullscreen {
            self.minimized_windows.remove(win_key);
            if is_fullscreen {
                if self.hide_menu_for_fullscreen {
                    // Fullscreen → Half-screen
                    self.hide_menu_for_fullscreen = false;
                    self.half_screen_mode = true;
                } else {
                    // Half-screen → Normal
                    self.fullscreen_window = None;
                    self.half_screen_mode = false;
                }
            } else {
                // Normal → Fullscreen
                self.fullscreen_window = Some(win_key.to_string());
                self.hide_menu_for_fullscreen = true;
                self.half_screen_mode = false;
            }
        }

        if let Some(path) = action_favorite {
            self.toggle_video_favorite(&path);
        }

        if let Some((path, hide)) = action_hide {
            if let Some(ref db) = self.db {
                let _ = db.set_video_hidden(&path, hide);
                self.refresh_video_metadata();
            }
        }

        if let Some(path) = action_play {
            if let Ok(()) = self.video_decoder.open(&path) {
                self.add_to_recent_videos(&path);
                self.video_path = Some(path);
                self.video_decoder.play();
            }
        }

        if let Some(path) = action_manage_tracks {
            self.managing_video_tracks = Some(path.clone());
            if let Some(ref db) = self.db {
                self.video_tracks = db.get_video_tracks(&path);
            }
        }

        // Handle sync button
        if action_sync {
            let is_syncing = self.video_sync.as_ref().map(|s| s.is_running()).unwrap_or(false);
            if is_syncing {
                self.show_sync_window = !self.show_sync_window;
            } else if let Some(ref dir) = self.video_directory.clone() {
                if self.video_sync.is_none() {
                    self.video_sync = Some(VideoSync::new(PathBuf::from(&dir)));
                }
                if let (Some(ref sync), Some(ref db)) = (&self.video_sync, &self.db) {
                    sync.refresh_list(db.clone());
                }
                self.show_sync_window = true;
            }
        }

        // Handle folder button
        if action_folder {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let path_str = path.display().to_string();
                self.video_directory = Some(path_str.clone());
                self.scan_video_directory(&path_str);
                if let Some(ref db) = self.db {
                    let _ = db.set_setting("video_directory", &path_str);
                }
                self.video_sync = Some(VideoSync::new(PathBuf::from(&path_str)));
                if let Some(ref sync) = self.compel_sync {
                    sync.set_video_dir(PathBuf::from(&path_str));
                }
            }
        }

        // Handle refresh button
        if action_refresh {
            if let Some(ref dir) = self.video_directory.clone() {
                self.scan_video_directory(&dir);
            }
        }
    }

    /// Render the Set Manager window
    pub fn render_recording_manager(&mut self, ctx: &egui::Context) {
        if !self.show_recording_manager {
            return;
        }

        let win_key = "set_manager";
        let is_minimized = self.minimized_windows.contains(win_key);
        let is_fullscreen = self.fullscreen_window.as_deref() == Some(win_key);

        let mut close_window = false;
        let mut toggle_minimize = false;
        let mut toggle_fullscreen = false;
        let mut action_folder = false;
        let mut action_refresh = false;
        let mut action_open: Option<String> = None;
        let mut action_rename: Option<(String, String, Option<String>)> = None;
        let mut action_delete: Option<String> = None;

        let mut window = egui::Window::new(format!("{} Set Manager", egui_phosphor::regular::RECORD))
            .collapsible(false)
            .resizable(!is_fullscreen || is_minimized);
        if is_fullscreen && !is_minimized {
            let rect = ctx.available_rect();
            if self.half_screen_mode {
                let half_width = rect.width() / 2.0;
                window = window.fixed_pos(rect.min)
                               .fixed_size(egui::vec2(half_width, rect.height()));
            } else {
                window = window.fixed_pos(rect.min).fixed_size(rect.size());
            }
        } else {
            window = window.default_width(800.0).default_height(600.0);
        }
        window.show(ctx, |ui| {
                // Traffic light buttons
                ui.horizontal(|ui| {
                    match mac_traffic_lights(ui) {
                        TrafficLightAction::Close => close_window = true,
                        TrafficLightAction::Minimize => toggle_minimize = true,
                        TrafficLightAction::Fullscreen => toggle_fullscreen = true,
                        TrafficLightAction::None => {}
                    }
                    ui.label(egui::RichText::new(format!("{} recordings", self.recordings.len())).strong());
                });

                if is_minimized {
                    return; // Only show traffic lights row when minimized
                }

                // Toolbar: Folder, Refresh, and status counts
                ui.horizontal(|ui| {
                    // Folder button
                    if ui.button(format!("{} Folder", egui_phosphor::regular::FOLDER_OPEN)).on_hover_text("Select recording folder").clicked() {
                        action_folder = true;
                    }

                    // Refresh button
                    if ui.button(format!("{} Refresh", egui_phosphor::regular::ARROWS_CLOCKWISE)).on_hover_text("Refresh recording list").clicked() {
                        action_refresh = true;
                    }

                    ui.add_space(16.0);

                    // Calculate upload status counts
                    let mut synced_count = 0;
                    let mut pending_count = 0;
                    let mut failed_count = 0;
                    for rec in &self.recordings {
                        match self.upload_status_cache.get(&rec.file_path) {
                            Some(crate::db::SetUploadStatus::Completed) => synced_count += 1,
                            Some(crate::db::SetUploadStatus::Pending) | Some(crate::db::SetUploadStatus::Uploading) | Some(crate::db::SetUploadStatus::Paused) => pending_count += 1,
                            Some(crate::db::SetUploadStatus::Failed) => failed_count += 1,
                            None => {}
                        }
                    }

                    // Display status counts with icons and action buttons
                    if synced_count > 0 {
                        ui.label(egui::RichText::new(format!("{} {}", egui_phosphor::regular::CHECK_CIRCLE, synced_count))
                            .color(egui::Color32::from_rgb(80, 200, 80))
                            .small());
                    }
                    if pending_count > 0 {
                        ui.label(egui::RichText::new(format!("{} {} queued", egui_phosphor::regular::CLOUD_ARROW_UP, pending_count))
                            .color(egui::Color32::from_rgb(100, 180, 255))
                            .small());

                        // Resume button if uploads are paused
                        if let Some(ref upload_manager) = self.set_upload_manager {
                            if !upload_manager.is_running() {
                                if ui.small_button(egui::RichText::new(egui_phosphor::regular::ARROW_FAT_UP).color(egui::Color32::from_rgb(80, 200, 80))).on_hover_text("Resume uploads").clicked() {
                                    upload_manager.start_upload();
                                }
                            }
                        }
                    }
                    if failed_count > 0 {
                        ui.label(egui::RichText::new(format!("{} {} failed", egui_phosphor::regular::X_CIRCLE, failed_count))
                            .color(egui::Color32::from_rgb(220, 80, 80))
                            .small());

                        // Retry all failed button
                        if ui.small_button(egui_phosphor::regular::ARROW_CLOCKWISE).on_hover_text("Retry all failed uploads").clicked() {
                            if let Some(ref db) = self.db {
                                if let Ok(count) = db.retry_all_failed_uploads() {
                                    if count > 0 {
                                        log::info!("[UI] Retrying {} failed uploads", count);
                                        self.upload_status_cache = db.get_upload_status_map();
                                        if let Some(ref upload_manager) = self.set_upload_manager {
                                            upload_manager.start_upload();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // View mode toggle buttons and folder path (right-aligned)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Folder path
                        if let Some(ref dir) = self.recording_directory {
                            let folder_name = dir.split('/').last().unwrap_or(dir);
                            ui.label(egui::RichText::new(format!("📂 {}", folder_name)).small().color(egui::Color32::GRAY));
                        }

                        ui.add_space(8.0);

                        // List view button
                        let list_color = if self.recording_view_mode == VideoViewMode::List {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(80, 80, 80)
                        };
                        if ui.add(
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::LIST).size(14.0).color(list_color))
                                .fill(egui::Color32::TRANSPARENT)
                        ).on_hover_text("List view").clicked() {
                            self.recording_view_mode = VideoViewMode::List;
                        }

                        // Grid view button
                        let grid_color = if self.recording_view_mode == VideoViewMode::Grid {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(80, 80, 80)
                        };
                        if ui.add(
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::SQUARES_FOUR).size(14.0).color(grid_color))
                                .fill(egui::Color32::TRANSPARENT)
                        ).on_hover_text("Grid view").clicked() {
                            self.recording_view_mode = VideoViewMode::Grid;
                        }
                    });
                });

                ui.separator();

                // Main content
                let dir = self.recording_directory.clone();
                if dir.is_none() || self.recordings.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(50.0);
                        if dir.is_none() {
                            ui.label(egui::RichText::new("No recording folder selected").size(16.0).color(egui::Color32::GRAY));
                        } else {
                            ui.label(egui::RichText::new("No recordings found").size(16.0).color(egui::Color32::GRAY));
                        }
                        ui.add_space(10.0);
                        if ui.button(format!("{} Select Folder...", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                            action_folder = true;
                        }
                    });

                    // Handle folder action here for early return case
                    if action_folder {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            let path_str = path.display().to_string();
                            self.recording_directory = Some(path_str.clone());
                            if let Some(db) = self.db.clone() {
                                let _ = db.set_setting("recording_directory", &path_str);
                                self.scan_recording_directory(&db, &path_str);
                            }
                        }
                    }
                    return;
                }

                // Scrollable content area
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.recording_view_mode {
                            VideoViewMode::Grid => {
                                // === GRID VIEW ===
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);

                            for rec in self.recordings.clone() {
                                let is_selected = self.manager_selected_recording.as_ref().map(|s| s == &rec.file_path).unwrap_or(false);

                                // Use larger thumbnails for the manager
                                let thumb_width = 160.0;
                                let thumb_height = 90.0;
                                let total_height = thumb_height + 20.0;

                                let (item_rect, item_response) = ui.allocate_exact_size(
                                    egui::vec2(thumb_width, total_height),
                                    egui::Sense::click()
                                );

                                let thumb_rect = egui::Rect::from_min_size(
                                    item_rect.min,
                                    egui::vec2(thumb_width, thumb_height)
                                );

                                // Item frame with selection/hover highlight
                                let frame_color = if is_selected {
                                    egui::Color32::from_rgb(60, 80, 120)
                                } else if item_response.hovered() {
                                    egui::Color32::from_rgb(50, 50, 60)
                                } else {
                                    egui::Color32::from_rgb(35, 35, 40)
                                };

                                // Draw thumbnail background
                                ui.painter().rect_filled(thumb_rect, 4.0, frame_color);

                                // Try to get thumbnail
                                if let Some(texture) = self.get_or_load_thumbnail(ctx, &rec.file_path) {
                                    ui.painter().image(
                                        texture.id(),
                                        thumb_rect,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        egui::Color32::WHITE,
                                    );
                                } else {
                                    // Draw placeholder with recording icon
                                    let icon_color = egui::Color32::from_rgb(80, 80, 90);
                                    ui.painter().text(
                                        thumb_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        egui_phosphor::regular::RECORD,
                                        egui::FontId::proportional(28.0),
                                        icon_color,
                                    );
                                }

                                // File size badge in top-right
                                let size_str = rec.size_string();
                                ui.painter().text(
                                    thumb_rect.right_top() + egui::vec2(-4.0, 4.0),
                                    egui::Align2::RIGHT_TOP,
                                    &size_str,
                                    egui::FontId::proportional(10.0),
                                    egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                                );

                                // Sync status badge in top-left
                                let upload_status = self.upload_status_cache.get(&rec.file_path);
                                if let Some(status) = upload_status {
                                    let (icon, color) = match status {
                                        crate::db::SetUploadStatus::Completed => (
                                            egui_phosphor::regular::CHECK_CIRCLE,
                                            egui::Color32::from_rgb(80, 200, 80)
                                        ),
                                        crate::db::SetUploadStatus::Failed => (
                                            egui_phosphor::regular::X_CIRCLE,
                                            egui::Color32::from_rgb(220, 80, 80)
                                        ),
                                        crate::db::SetUploadStatus::Uploading => (
                                            egui_phosphor::regular::CLOUD_ARROW_UP,
                                            egui::Color32::from_rgb(100, 180, 255)
                                        ),
                                        crate::db::SetUploadStatus::Pending | crate::db::SetUploadStatus::Paused => (
                                            egui_phosphor::regular::CLOUD,
                                            egui::Color32::from_rgb(150, 150, 150)
                                        ),
                                    };
                                    // Draw background pill for contrast
                                    let badge_pos = thumb_rect.left_top() + egui::vec2(4.0, 4.0);
                                    ui.painter().circle_filled(
                                        badge_pos + egui::vec2(7.0, 7.0),
                                        10.0,
                                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                                    );
                                    ui.painter().text(
                                        badge_pos,
                                        egui::Align2::LEFT_TOP,
                                        icon,
                                        egui::FontId::proportional(14.0),
                                        color,
                                    );
                                }

                                // Border - highlight synced recordings with green, failed with red, selected with blue
                                let border_color = if is_selected {
                                    egui::Color32::from_rgb(100, 140, 200)
                                } else {
                                    match upload_status {
                                        Some(crate::db::SetUploadStatus::Completed) => egui::Color32::from_rgb(60, 120, 60),
                                        Some(crate::db::SetUploadStatus::Failed) => egui::Color32::from_rgb(120, 50, 50),
                                        _ => egui::Color32::from_rgb(50, 50, 55),
                                    }
                                };
                                ui.painter().rect_stroke(thumb_rect, 4.0, egui::Stroke::new(1.0, border_color));

                                // Name label (truncated at char boundary)
                                let display_name = rec.display_name();
                                let max_chars = 20;
                                let truncated_name = if display_name.chars().count() > max_chars {
                                    let end = display_name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(display_name.len());
                                    format!("{}...", &display_name[..end])
                                } else {
                                    display_name.to_string()
                                };

                                let name_color = if is_selected {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_rgb(180, 180, 180)
                                };

                                // Draw name below thumbnail
                                let label_pos = egui::pos2(item_rect.min.x, thumb_rect.max.y + 3.0);
                                ui.painter().text(
                                    label_pos,
                                    egui::Align2::LEFT_TOP,
                                    &truncated_name,
                                    egui::FontId::proportional(11.0),
                                    name_color,
                                );

                                // Context menu
                                let file_path = rec.file_path.clone();
                                let file_name = rec.file_name.clone();
                                let custom_name = rec.custom_name.clone();
                                let duration_secs = rec.duration_secs;
                                let created_at = rec.created_at;
                                let upload_status_for_menu = upload_status.cloned();
                                item_response.clone().context_menu(|ui| {
                                    ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

                                    if ui.button(format!("{} Open", egui_phosphor::regular::PLAY)).clicked() {
                                        action_open = Some(file_path.clone());
                                        ui.close_menu();
                                    }

                                    if ui.button(format!("{} Rename...", egui_phosphor::regular::PENCIL)).clicked() {
                                        action_rename = Some((file_path.clone(), file_name.clone(), custom_name.clone()));
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    if ui.button(format!("{} Show in Finder", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                                        #[cfg(target_os = "macos")]
                                        {
                                            let _ = std::process::Command::new("open")
                                                .arg("-R")
                                                .arg(&file_path)
                                                .spawn();
                                        }
                                        ui.close_menu();
                                    }

                                    ui.separator();

                                    // Upload/Sync options based on current status
                                    match upload_status_for_menu {
                                        Some(crate::db::SetUploadStatus::Completed) => {
                                            // Already synced - show disabled "Synced" indicator
                                            ui.add_enabled(false, egui::Button::new(format!("{}   Synced to Compeller.ai", egui_phosphor::regular::CHECK_CIRCLE)))
                                                .on_disabled_hover_text("This recording is already synced");
                                        }
                                        Some(crate::db::SetUploadStatus::Failed) => {
                                            // Failed - show retry option
                                            if ui.button(format!("{}   Retry Upload", egui_phosphor::regular::ARROW_CLOCKWISE)).clicked() {
                                                if let Some(ref db) = self.db {
                                                    if let Err(e) = db.retry_failed_upload(&file_path) {
                                                        log::error!("[UI] Failed to retry upload: {}", e);
                                                    } else {
                                                        log::info!("[UI] Retrying upload: {}", file_name);
                                                        // Refresh cache
                                                        self.upload_status_cache = db.get_upload_status_map();
                                                        // Start upload if not running
                                                        if let Some(ref upload_manager) = self.set_upload_manager {
                                                            if !upload_manager.is_running() {
                                                                upload_manager.start_upload();
                                                            }
                                                        }
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        }
                                        Some(crate::db::SetUploadStatus::Uploading) | Some(crate::db::SetUploadStatus::Pending) | Some(crate::db::SetUploadStatus::Paused) => {
                                            // In progress or queued - show cancel option
                                            if ui.button(format!("{}   Cancel Upload", egui_phosphor::regular::X)).clicked() {
                                                if let Some(ref upload_manager) = self.set_upload_manager {
                                                    let _ = upload_manager.cancel_upload(&file_path);
                                                    log::info!("[UI] Cancelled upload: {}", file_name);
                                                    // Refresh cache
                                                    if let Some(ref db) = self.db {
                                                        self.upload_status_cache = db.get_upload_status_map();
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        }
                                        None => {
                                            // Not in queue - show upload option
                                            if let Some(ref upload_manager) = self.set_upload_manager {
                                                if ui.button(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)).clicked() {
                                                    // Use recording metadata for upload
                                                    let recorded_at_str = chrono::DateTime::from_timestamp(created_at, 0)
                                                        .map(|dt| dt.to_rfc3339())
                                                        .unwrap_or_else(|| chrono::Local::now().to_rfc3339());
                                                    let request = crate::set_sync::SetUploadRequest {
                                                        file_path: file_path.clone(),
                                                        title: custom_name.clone().or_else(|| Some(file_name.trim_end_matches(".mp4").to_string())),
                                                        recorded_at: Some(recorded_at_str),
                                                        theme_used: None,
                                                        audio_source: None,
                                                        duration_seconds: duration_secs.map(|d| d as i64),
                                                    };
                                                    if let Err(e) = upload_manager.queue_upload(request) {
                                                        log::error!("[UI] Failed to queue upload: {}", e);
                                                    } else {
                                                        log::info!("[UI] Queued recording for upload: {}", file_name);
                                                        // Refresh cache
                                                        if let Some(ref db) = self.db {
                                                            self.upload_status_cache = db.get_upload_status_map();
                                                        }
                                                        if !upload_manager.is_running() {
                                                            upload_manager.start_upload();
                                                        }
                                                    }
                                                    ui.close_menu();
                                                }
                                            } else {
                                                ui.add_enabled(false, egui::Button::new(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)))
                                                    .on_disabled_hover_text("Login required to upload");
                                            }
                                        }
                                    }

                                    ui.separator();

                                    if ui.button(format!("{} Delete Recording", egui_phosphor::regular::TRASH)).clicked() {
                                        action_delete = Some(file_path.clone());
                                        ui.close_menu();
                                    }
                                });

                                // Handle click
                                if item_response.clicked() {
                                    action_open = Some(rec.file_path.clone());
                                }

                                // Hover cursor
                                if item_response.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                            }
                        });
                            }
                            VideoViewMode::List => {
                                // === LIST VIEW ===
                                for rec in self.recordings.clone() {
                                    let upload_status = self.upload_status_cache.get(&rec.file_path).cloned();

                                    let response = ui.horizontal(|ui| {
                                        // Sync status icon prefix
                                        if let Some(ref status) = upload_status {
                                            let (icon, color) = match status {
                                                crate::db::SetUploadStatus::Completed => (
                                                    egui_phosphor::regular::CHECK_CIRCLE,
                                                    egui::Color32::from_rgb(80, 200, 80)
                                                ),
                                                crate::db::SetUploadStatus::Failed => (
                                                    egui_phosphor::regular::X_CIRCLE,
                                                    egui::Color32::from_rgb(220, 80, 80)
                                                ),
                                                crate::db::SetUploadStatus::Uploading => (
                                                    egui_phosphor::regular::CLOUD_ARROW_UP,
                                                    egui::Color32::from_rgb(100, 180, 255)
                                                ),
                                                crate::db::SetUploadStatus::Pending | crate::db::SetUploadStatus::Paused => (
                                                    egui_phosphor::regular::CLOUD,
                                                    egui::Color32::from_rgb(150, 150, 150)
                                                ),
                                            };
                                            ui.label(egui::RichText::new(icon).size(12.0).color(color));
                                        }

                                        // Display name
                                        let display = rec.display_name();
                                        let label = egui::RichText::new(display).size(12.0);
                                        let text = ui.add(egui::Label::new(label).sense(egui::Sense::click()));

                                        // Size and duration on right
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(egui::RichText::new(rec.size_string())
                                                .size(10.0)
                                                .color(egui::Color32::DARK_GRAY));
                                            if let Some(dur) = rec.duration_secs {
                                                let mins = dur as i64 / 60;
                                                let secs = dur as i64 % 60;
                                                ui.label(egui::RichText::new(format!("{}:{:02}", mins, secs))
                                                    .size(10.0)
                                                    .color(egui::Color32::DARK_GRAY));
                                            }
                                        });

                                        text
                                    });

                                    // Click to open
                                    if response.inner.clicked() {
                                        action_open = Some(rec.file_path.clone());
                                    }

                                    // Context menu (same as grid view)
                                    let file_path = rec.file_path.clone();
                                    let file_name = rec.file_name.clone();
                                    let custom_name = rec.custom_name.clone();
                                    let duration_secs = rec.duration_secs;
                                    let created_at = rec.created_at;
                                    let upload_status_for_menu = upload_status.clone();
                                    response.response.context_menu(|ui| {
                                        ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

                                        if ui.button(format!("{} Open", egui_phosphor::regular::PLAY)).clicked() {
                                            action_open = Some(file_path.clone());
                                            ui.close_menu();
                                        }

                                        if ui.button(format!("{} Rename...", egui_phosphor::regular::PENCIL)).clicked() {
                                            action_rename = Some((file_path.clone(), file_name.clone(), custom_name.clone()));
                                            ui.close_menu();
                                        }

                                        ui.separator();

                                        if ui.button(format!("{} Show in Finder", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                                            #[cfg(target_os = "macos")]
                                            {
                                                let _ = std::process::Command::new("open")
                                                    .arg("-R")
                                                    .arg(&file_path)
                                                    .spawn();
                                            }
                                            ui.close_menu();
                                        }

                                        ui.separator();

                                        // Upload/Sync options
                                        match upload_status_for_menu {
                                            Some(crate::db::SetUploadStatus::Completed) => {
                                                ui.add_enabled(false, egui::Button::new(format!("{}   Synced", egui_phosphor::regular::CHECK_CIRCLE)));
                                            }
                                            Some(crate::db::SetUploadStatus::Failed) => {
                                                if ui.button(format!("{}   Retry Upload", egui_phosphor::regular::ARROW_CLOCKWISE)).clicked() {
                                                    if let Some(ref db) = self.db {
                                                        let _ = db.retry_failed_upload(&file_path);
                                                        self.upload_status_cache = db.get_upload_status_map();
                                                        if let Some(ref upload_manager) = self.set_upload_manager {
                                                            if !upload_manager.is_running() {
                                                                upload_manager.start_upload();
                                                            }
                                                        }
                                                    }
                                                    ui.close_menu();
                                                }
                                            }
                                            Some(crate::db::SetUploadStatus::Uploading) | Some(crate::db::SetUploadStatus::Pending) | Some(crate::db::SetUploadStatus::Paused) => {
                                                if ui.button(format!("{}   Cancel Upload", egui_phosphor::regular::X)).clicked() {
                                                    if let Some(ref upload_manager) = self.set_upload_manager {
                                                        let _ = upload_manager.cancel_upload(&file_path);
                                                        if let Some(ref db) = self.db {
                                                            self.upload_status_cache = db.get_upload_status_map();
                                                        }
                                                    }
                                                    ui.close_menu();
                                                }
                                            }
                                            None => {
                                                if let Some(ref upload_manager) = self.set_upload_manager {
                                                    if ui.button(format!("{}   Upload", egui_phosphor::regular::CLOUD_ARROW_UP)).clicked() {
                                                        let recorded_at_str = chrono::DateTime::from_timestamp(created_at, 0)
                                                            .map(|dt| dt.to_rfc3339())
                                                            .unwrap_or_else(|| chrono::Local::now().to_rfc3339());
                                                        let request = crate::set_sync::SetUploadRequest {
                                                            file_path: file_path.clone(),
                                                            title: custom_name.clone().or_else(|| Some(file_name.trim_end_matches(".mp4").to_string())),
                                                            recorded_at: Some(recorded_at_str),
                                                            theme_used: None,
                                                            audio_source: None,
                                                            duration_seconds: duration_secs.map(|d| d as i64),
                                                        };
                                                        let _ = upload_manager.queue_upload(request);
                                                        if let Some(ref db) = self.db {
                                                            self.upload_status_cache = db.get_upload_status_map();
                                                        }
                                                        if !upload_manager.is_running() {
                                                            upload_manager.start_upload();
                                                        }
                                                        ui.close_menu();
                                                    }
                                                }
                                            }
                                        }

                                        ui.separator();

                                        if ui.button(format!("{} Delete", egui_phosphor::regular::TRASH)).clicked() {
                                            action_delete = Some(file_path.clone());
                                            ui.close_menu();
                                        }
                                    });
                                }
                            }
                        }
                    });
            });

        // Handle actions outside the closure
        if close_window {
            self.show_recording_manager = false;
            if is_fullscreen {
                self.fullscreen_window = None;
                self.hide_menu_for_fullscreen = false;
                self.half_screen_mode = false;
            }
        }
        if toggle_minimize {
            if !self.minimized_windows.remove(win_key) {
                self.minimized_windows.insert(win_key.to_string());
                if is_fullscreen && !self.half_screen_mode {
                    self.hide_menu_for_fullscreen = false;
                }
            } else if is_fullscreen && !self.half_screen_mode {
                self.hide_menu_for_fullscreen = true;
            }
        }
        if toggle_fullscreen {
            self.minimized_windows.remove(win_key);
            if is_fullscreen {
                if self.hide_menu_for_fullscreen {
                    // Fullscreen → Half-screen
                    self.hide_menu_for_fullscreen = false;
                    self.half_screen_mode = true;
                } else {
                    // Half-screen → Normal
                    self.fullscreen_window = None;
                    self.half_screen_mode = false;
                }
            } else {
                // Normal → Fullscreen
                self.fullscreen_window = Some(win_key.to_string());
                self.hide_menu_for_fullscreen = true;
                self.half_screen_mode = false;
            }
        }

        if action_folder {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let path_str = path.display().to_string();
                self.recording_directory = Some(path_str.clone());
                if let Some(db) = self.db.clone() {
                    let _ = db.set_setting("recording_directory", &path_str);
                    self.scan_recording_directory(&db, &path_str);
                }
            }
        }

        if action_refresh {
            if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
                self.scan_recording_directory(&db, &dir);
                // Also refresh the upload status cache
                self.upload_status_cache = db.get_upload_status_map();
            }
        }

        if let Some(path) = action_open {
            log::info!("[UI] Opening recording: {}", path);
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .arg(&path)
                    .spawn();
            }
        }

        if let Some((file_path, file_name, custom_name)) = action_rename {
            self.editing_recording = Some(file_path);
            self.editing_recording_name = custom_name
                .unwrap_or_else(|| file_name.trim_end_matches(".mp4").to_string());
        }

        if let Some(path) = action_delete {
            log::info!("[UI] Deleting recording: {}", path);
            if let Err(e) = std::fs::remove_file(&path) {
                log::error!("[UI] Failed to delete file: {}", e);
            }
            if let Some(ref db) = self.db {
                let _ = db.delete_recording(&path);
                // Refresh the upload status cache
                self.upload_status_cache = db.get_upload_status_map();
            }
            if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
                self.scan_recording_directory(&db, &dir);
            }
        }
    }

    /// Render the Camera Preview window with detection boxes
    pub fn render_camera_preview(&mut self, ctx: &egui::Context, camera_state: Option<&crate::app::CameraState>) {
        if !self.show_camera_preview {
            return;
        }

        // Request continuous repaints while preview is open
        ctx.request_repaint();

        let win_key = "react_vision";
        let is_minimized = self.minimized_windows.contains(win_key);
        let is_fullscreen = self.fullscreen_window.as_deref() == Some(win_key);

        let mut close_window = false;
        let mut toggle_minimize = false;
        let mut toggle_fullscreen = false;

        let mut window = egui::Window::new(format!("{} REACT VISION", egui_phosphor::regular::VIDEO_CAMERA))
            .collapsible(false)
            .resizable(!is_fullscreen || is_minimized);
        if is_fullscreen && !is_minimized {
            let rect = ctx.available_rect();
            if self.half_screen_mode {
                let half_width = rect.width() / 2.0;
                window = window.fixed_pos(rect.min)
                               .fixed_size(egui::vec2(half_width, rect.height()));
            } else {
                window = window.fixed_pos(rect.min).fixed_size(rect.size());
            }
        } else {
            window = window.default_width(680.0).default_height(560.0);
        }
        window.show(ctx, |ui| {
                // Header with traffic light close
                ui.horizontal(|ui| {
                    match mac_traffic_lights(ui) {
                        TrafficLightAction::Close => close_window = true,
                        TrafficLightAction::Minimize => toggle_minimize = true,
                        TrafficLightAction::Fullscreen => toggle_fullscreen = true,
                        TrafficLightAction::None => {}
                    }
                    ui.label(egui::RichText::new("COMPEL Integration").strong());
                });

                if is_minimized {
                    return; // Only show traffic lights row when minimized
                }

                ui.separator();

                // Show current filter tags (controlled from Vision section)
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Filter:").small());
                    if self.camera_filter_tags.is_empty() {
                        ui.label(egui::RichText::new("All Classes").small().color(egui::Color32::GRAY));
                    } else {
                        for tag in &self.camera_filter_tags {
                            let chip = egui::Frame::none()
                                .fill(egui::Color32::from_rgb(60, 80, 100))
                                .rounding(3.0)
                                .inner_margin(egui::Margin::symmetric(4.0, 1.0));
                            chip.show(ui, |ui| {
                                ui.label(egui::RichText::new(tag).small().color(egui::Color32::WHITE));
                            });
                        }
                    }
                });

                ui.add_space(4.0);

                // Camera preview image with detection boxes
                if let Some(state) = camera_state {
                    let preview = &state.preview;

                    if preview.width > 0 && preview.height > 0 && !preview.data.is_empty() {
                        // Update texture if dimensions changed or texture doesn't exist
                        let needs_update = self.camera_preview_texture.is_none()
                            || self.camera_preview_dims != (preview.width, preview.height);

                        if needs_update {
                            let image = egui::ColorImage::from_rgb(
                                [preview.width as usize, preview.height as usize],
                                &preview.data,
                            );
                            self.camera_preview_texture = Some(ctx.load_texture(
                                "camera_preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                            self.camera_preview_dims = (preview.width, preview.height);
                        } else if let Some(ref mut texture) = self.camera_preview_texture {
                            // Update existing texture with new frame data
                            let image = egui::ColorImage::from_rgb(
                                [preview.width as usize, preview.height as usize],
                                &preview.data,
                            );
                            texture.set(image, egui::TextureOptions::LINEAR);
                        }

                        // Draw the camera image
                        if let Some(ref texture) = self.camera_preview_texture {
                            let available = ui.available_size();
                            let aspect = preview.width as f32 / preview.height as f32;
                            let display_width = available.x.min(640.0);
                            let display_height = display_width / aspect;

                            let (rect, _response) = ui.allocate_exact_size(
                                egui::vec2(display_width, display_height),
                                egui::Sense::hover(),
                            );

                            // Draw the image
                            ui.painter().image(
                                texture.id(),
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );

                            // Draw boxes for ALL tracked objects
                            for track in &preview.tracks {
                                let bbox = &track.bbox;
                                let x1 = rect.min.x + (bbox.x - bbox.w / 2.0) * display_width;
                                let y1 = rect.min.y + (bbox.y - bbox.h / 2.0) * display_height;
                                let x2 = rect.min.x + (bbox.x + bbox.w / 2.0) * display_width;
                                let y2 = rect.min.y + (bbox.y + bbox.h / 2.0) * display_height;

                                let box_rect = egui::Rect::from_min_max(
                                    egui::pos2(x1, y1),
                                    egui::pos2(x2, y2),
                                );

                                // Active track gets cyan, others get yellow
                                let is_active = state.output.track_id == track.id;
                                let color = if is_active {
                                    egui::Color32::from_rgb(100, 255, 255) // Cyan for active
                                } else {
                                    egui::Color32::from_rgb(255, 200, 100) // Yellow for others
                                };
                                let stroke_width = if is_active { 3.0 } else { 1.5 };

                                ui.painter().rect_stroke(box_rect, 0.0, egui::Stroke::new(stroke_width, color));

                                // Draw label with class name and confidence
                                let label_text = format!("{} {:.0}%", track.class_name, track.confidence * 100.0);
                                ui.painter().text(
                                    egui::pos2(x1, y1 - 2.0),
                                    egui::Align2::LEFT_BOTTOM,
                                    label_text,
                                    egui::FontId::proportional(if is_active { 12.0 } else { 10.0 }),
                                    color,
                                );
                            }
                        }
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.add_space(30.0);
                            ui.label(egui::RichText::new("No camera frame").color(egui::Color32::GRAY));
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Start vision to see preview").small().color(egui::Color32::DARK_GRAY));
                        });
                    }

                    // Active tracking info at bottom
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if state.output.active {
                            ui.label(egui::RichText::new("Active").color(egui::Color32::from_rgb(100, 255, 100)));
                            if let Some(ref class_name) = state.output.class_name {
                                ui.label(egui::RichText::new(class_name).strong());
                            }
                            ui.label(format!("{:.0}%", state.output.confidence * 100.0));
                            ui.label(format!("Gate {}/{}", state.output.anchor_index + 1, state.output.anchor_n));
                        } else {
                            ui.label(egui::RichText::new("No active track").color(egui::Color32::GRAY));
                        }
                    });

                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(50.0);
                        ui.label(egui::RichText::new("Camera not initialized").color(egui::Color32::GRAY));
                    });
                }
            });

        if close_window {
            self.show_camera_preview = false;
            if is_fullscreen {
                self.fullscreen_window = None;
                self.hide_menu_for_fullscreen = false;
                self.half_screen_mode = false;
            }
        }
        if toggle_minimize {
            if !self.minimized_windows.remove(win_key) {
                self.minimized_windows.insert(win_key.to_string());
                if is_fullscreen && !self.half_screen_mode {
                    self.hide_menu_for_fullscreen = false;
                }
            } else if is_fullscreen && !self.half_screen_mode {
                self.hide_menu_for_fullscreen = true;
            }
        }
        if toggle_fullscreen {
            self.minimized_windows.remove(win_key);
            if is_fullscreen {
                if self.hide_menu_for_fullscreen {
                    // Fullscreen → Half-screen
                    self.hide_menu_for_fullscreen = false;
                    self.half_screen_mode = true;
                } else {
                    // Half-screen → Normal
                    self.fullscreen_window = None;
                    self.half_screen_mode = false;
                }
            } else {
                // Normal → Fullscreen
                self.fullscreen_window = Some(win_key.to_string());
                self.hide_menu_for_fullscreen = true;
                self.half_screen_mode = false;
            }
        }
    }

    /// Render a single preset row with star, truncated name, and tooltip
    fn render_preset_row(
        &mut self,
        ui: &mut egui::Ui,
        p: &mut RenderPipeline,
        cat: &str,
        preset: &str,
        is_current: bool,
        show_category: bool,
    ) -> egui::Response {
        // Get file path for metadata lookup
        let file_path = p.get_preset_path(cat, preset);

        // Check if we're editing this preset's name
        let is_editing = self.editing_preset.as_ref().map(|ep| {
            file_path.as_ref().map(|fp| ep == fp).unwrap_or(false)
        }).unwrap_or(false);

        ui.horizontal(|ui| {
            // Star button
            let is_fav = self.is_favorite(cat, preset);
            let star = if is_fav { "★" } else { "☆" };
            let star_color = if is_fav {
                egui::Color32::from_rgb(255, 200, 50)
            } else {
                egui::Color32::from_rgb(100, 100, 100)
            };
            if ui.add(
                egui::Button::new(egui::RichText::new(star).color(star_color).size(12.0))
                    .fill(egui::Color32::TRANSPARENT)
                    .min_size(egui::vec2(20.0, 20.0))
            ).clicked() {
                self.toggle_favorite(cat, preset);
            }

            if is_editing {
                // Inline edit mode
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.editing_preset_name)
                        .desired_width(150.0)
                );

                // Save on Enter or when losing focus
                if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(ref fp) = file_path {
                        if let Some(ref db) = self.db {
                            let name = self.editing_preset_name.trim();
                            if !name.is_empty() {
                                let _ = db.set_preset_custom_name(fp, name);
                                self.refresh_preset_metadata();
                            }
                        }
                    }
                    self.editing_preset = None;
                    self.editing_preset_name.clear();
                }

                // Cancel on Escape
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.editing_preset = None;
                    self.editing_preset_name.clear();
                }

                response.request_focus();
            } else {
                // Get custom display name if set
                let display_name = if let Some(ref fp) = file_path {
                    self.get_preset_display_name(fp, preset)
                } else {
                    preset.to_string()
                };

                // Check if has custom name (for visual indicator)
                let has_custom_name = file_path.as_ref()
                    .and_then(|fp| self.preset_metadata.get(fp))
                    .and_then(|m| m.custom_name.as_ref())
                    .is_some();

                // Preset name - truncated with tooltip
                let full_name = if show_category {
                    format!("{} ({})", display_name, cat)
                } else {
                    display_name.clone()
                };
                let display = Self::truncate_text(&full_name, Self::PRESET_NAME_MAX_CHARS);

                let text = if is_current {
                    egui::RichText::new(format!("▶ {}", display))
                        .color(egui::Color32::from_rgb(100, 255, 100))
                        .strong()
                } else if has_custom_name {
                    egui::RichText::new(&display)
                        .color(egui::Color32::from_rgb(180, 180, 255))
                } else {
                    egui::RichText::new(&display)
                };

                let response = ui.selectable_label(is_current, text);

                // Get subcategory for tooltip
                let subcategory = p.get_preset_subcategory(cat, preset);

                // Tooltip shows full hierarchy: category - subcategory - name
                let tooltip = {
                    let mut parts = vec![cat.to_string()];
                    if let Some(ref sub) = subcategory {
                        // Split subcategory path and add each part
                        for part in sub.split('/') {
                            if !part.is_empty() {
                                parts.push(part.to_string());
                            }
                        }
                    }
                    parts.push(preset.to_string());
                    let hierarchy = parts.join(" > ");

                    if has_custom_name {
                        format!("{}\n{}", display_name, hierarchy)
                    } else if full_name.len() > display.len() || subcategory.is_some() {
                        hierarchy
                    } else {
                        String::new()
                    }
                };
                if !tooltip.is_empty() {
                    response.clone().on_hover_text(&tooltip);
                }

                // Right-click context menu
                response.clone().context_menu(|ui| {
                    ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

                    if ui.button("✏️   Rename Theme...").clicked() {
                        if let Some(ref fp) = file_path {
                            self.editing_preset = Some(fp.clone());
                            self.editing_preset_name = display_name.clone();
                        }
                        ui.close_menu();
                    }

                    // Hide/Show toggle
                    let is_hidden = file_path.as_ref()
                        .map(|fp| self.is_preset_hidden(fp))
                        .unwrap_or(false);

                    if is_hidden {
                        if ui.button("👁   Show Theme").clicked() {
                            if let (Some(ref fp), Some(ref db)) = (&file_path, &self.db) {
                                let _ = db.set_preset_hidden(fp, false);
                                self.refresh_preset_metadata();
                            }
                            ui.close_menu();
                        }
                    } else {
                        if ui.button("🙈   Hide Theme").clicked() {
                            if let (Some(ref fp), Some(ref db)) = (&file_path, &self.db) {
                                let _ = db.set_preset_hidden(fp, true);
                                self.refresh_preset_metadata();
                            }
                            ui.close_menu();
                        }
                    }

                    ui.separator();

                    // Pin/Unpin toggle (works for ALL renderer types)
                    let is_pinned = self.db.as_ref()
                        .map(|db| db.is_pinned(cat, preset))
                        .unwrap_or(false);

                    if is_pinned {
                        if ui.button("📌   Unpin Theme").clicked() {
                            if let Some(ref db) = self.db {
                                let _ = db.unpin_preset(cat, preset);
                                self.favorites_dirty = true;
                                self.midi_browse_dirty = true;
                            }
                            ui.close_menu();
                        }
                    } else {
                        if ui.button("📌   Pin Theme").clicked() {
                            if let Some(ref db) = self.db {
                                let _ = db.pin_preset(cat, preset);
                                self.favorites_dirty = true;
                                self.midi_browse_dirty = true;
                            }
                            ui.close_menu();
                        }
                    }

                    ui.separator();

                    if ui.button("🎵   Manage Tracks...").clicked() {
                        if let Some(ref fp) = file_path {
                            self.managing_preset_tracks = Some(fp.clone());
                            // Load tracks for this preset
                            if let Some(ref db) = self.db {
                                self.preset_tracks = db.get_preset_tracks(fp);
                            }
                        }
                        ui.close_menu();
                    }
                });

                if response.clicked() {
                    log::info!("[UI] Preset row clicked: cat='{}', preset='{}'", cat, preset);
                    // Defer load to next frame so loading dialog can render
                    // This is especially important for Geometric Field which compiles shaders
                    self.pending_preset_load = Some((cat.to_string(), preset.to_string()));
                    self.pending_preset_load_frames = 0;  // Reset frame counter for new load
                    self.mark_dirty();
                }
            }
        }).response
    }

    /// Scan directory for video files
    fn scan_video_directory(&mut self, dir: &str) {
        self.video_files.clear();
        let video_extensions = ["mp4", "mov", "webm", "mkv", "avi", "m4v", "MP4", "MOV", "MKV", "AVI", "M4V"];

        let mut full_paths: Vec<String> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if video_extensions.iter().any(|e| ext == *e) {
                            if let Some(name) = path.file_name() {
                                self.video_files.push(name.to_string_lossy().to_string());
                                full_paths.push(path.display().to_string());
                            }
                        }
                    }
                }
            }
        }
        // Sort alphabetically
        self.video_files.sort();
        log::info!("Found {} videos in {}", self.video_files.len(), dir);

        // Sync metadata from database
        self.refresh_video_metadata();

        // Queue thumbnails for generation and cleanup orphaned
        #[cfg(target_os = "macos")]
        {
            // Initialize thumbnail generator if needed
            if self.thumbnail_generator.is_none() {
                if let Ok(mut gen) = ThumbnailGenerator::new() {
                    gen.start();
                    self.thumbnail_generator = Some(gen);
                }
            }

            if let Some(ref gen) = self.thumbnail_generator {
                // Queue videos for thumbnail generation
                gen.queue_videos(&full_paths);

                // Cleanup orphaned thumbnails (for deleted videos)
                gen.cleanup_orphaned(&full_paths);
            }

            // Cleanup stale texture handles for deleted videos
            let valid_paths: std::collections::HashSet<&String> = full_paths.iter().collect();
            self.thumbnail_textures.retain(|path, _| valid_paths.contains(path));
        }
    }

    /// Scan directory for recordings and sync with database
    fn scan_recording_directory(&mut self, db: &Database, dir: &str) {
        // Prune recordings that no longer exist
        let _ = db.prune_missing_recordings(dir);

        // Scan for MP4 files
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("mp4") {
                            let file_path = path.display().to_string();
                            let file_name = path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();

                            // Get file size
                            let file_size = std::fs::metadata(&path)
                                .map(|m| m.len() as i64)
                                .ok();

                            // Upsert to database (preserves custom_name and notes)
                            let _ = db.upsert_recording(
                                &file_path,
                                &file_name,
                                None, // duration - would need ffprobe
                                file_size,
                                None, // width
                                None, // height
                                None, // fps
                            );
                        }
                    }
                }
            }
        }

        // Load recordings from database
        self.recordings = db.get_recordings(Some(dir));
        log::info!("[UI] Found {} recordings in {}", self.recordings.len(), dir);

        // Refresh upload status cache
        self.upload_status_cache = db.get_upload_status_map();

        // Queue recordings for thumbnail generation
        #[cfg(target_os = "macos")]
        {
            // Initialize thumbnail generator if needed
            if self.thumbnail_generator.is_none() {
                if let Ok(mut gen) = ThumbnailGenerator::new() {
                    gen.start();
                    self.thumbnail_generator = Some(gen);
                }
            }

            if let Some(ref gen) = self.thumbnail_generator {
                let recording_paths: Vec<String> = self.recordings.iter()
                    .map(|r| r.file_path.clone())
                    .collect();
                gen.queue_videos(&recording_paths);
            }
        }
    }

    /// Get the recording output directory (for use by app.rs)
    pub fn get_recording_directory(&self) -> Option<&str> {
        self.recording_directory.as_deref()
    }

    /// Set the video directory (called from setup wizard)
    pub fn set_video_directory(&mut self, path: String) {
        self.video_directory = Some(path.clone());
        self.scan_video_directory(&path);
        // Create sync manager for this directory
        self.video_sync = Some(VideoSync::new(std::path::PathBuf::from(&path)));
        // Keep compel sync pointed at the same folder
        if let Some(ref sync) = self.compel_sync {
            sync.set_video_dir(std::path::PathBuf::from(&path));
        }
    }

    /// Get the video directory path
    pub fn get_video_directory(&self) -> Option<String> {
        self.video_directory.clone()
    }

    /// Set the compel sync manager (called from app startup)
    pub fn set_compel_sync(&mut self, sync: CompelSync) {
        self.compel_sync = Some(sync);
    }

    /// Start compels sync - opens sync window and begins downloading
    pub fn start_compels_sync(&mut self) {
        // Expand the video/compels section
        self.video_expanded = true;

        // Open the sync window
        self.show_sync_window = true;

        // Ensure sync manager exists
        if self.video_sync.is_none() {
            if let Some(ref dir) = self.video_directory.clone() {
                self.video_sync = Some(VideoSync::new(std::path::PathBuf::from(&dir)));
            }
        }

        // Refresh list and start sync
        if let (Some(ref sync), Some(ref db)) = (&self.video_sync, &self.db) {
            // Fetch remote list first
            sync.refresh_list(db.clone());
            // Start syncing
            sync.start_sync(db.clone());
            log::info!("[Sync] Started compels sync from prompt");
        }
    }

    /// Start compel sync (authenticated user's compels) with the given auth token
    pub fn start_compel_sync(&mut self, auth_token: Option<String>) {
        let token = match auth_token {
            Some(t) => t,
            None => return,
        };

        // Ensure compel sync manager exists
        if self.compel_sync.is_none() {
            if let Some(ref dir) = self.video_directory.clone() {
                self.compel_sync = Some(CompelSync::new(std::path::PathBuf::from(dir)));
            }
        }

        if let (Some(ref sync), Some(ref db)) = (&self.compel_sync, &self.db) {
            if !sync.is_running() {
                sync.refresh_list(db.clone(), token.clone());
                log::info!("[CompelSync] Started compel sync");
            }
        }
    }

    /// Set the recording directory (called from setup wizard)
    pub fn set_recording_directory(&mut self, path: String) {
        self.recording_directory = Some(path.clone());
        if let Some(db) = self.db.clone() {
            self.scan_recording_directory(&db, &path);
        }
    }

    /// Load a video by path (for startup defaults)
    pub fn load_video(&mut self, path: &str) -> bool {
        if self.video_decoder.open(path).is_ok() {
            self.add_to_recent_videos(path);
            self.video_path = Some(path.to_string());
            self.video_decoder.play();
            log::info!("[Video] Loaded default video: {}", path);
            true
        } else {
            log::warn!("[Video] Failed to load video: {}", path);
            false
        }
    }

    /// Get current video path (if any)
    pub fn get_current_video_path(&self) -> Option<&String> {
        self.video_path.as_ref()
    }

    /// Refresh recordings list (call after recording ends)
    pub fn refresh_recordings(&mut self) {
        if let (Some(dir), Some(db)) = (self.recording_directory.clone(), self.db.clone()) {
            self.scan_recording_directory(&db, &dir);
        }
    }

    /// Compels (video) controls section
    fn show_video_section(&mut self, ui: &mut egui::Ui, pipeline: &mut Option<RenderPipeline>) {
        let _header = egui::CollapsingHeader::new(egui::RichText::new("Compels").strong().size(14.0))
            .default_open(true)
            .show(ui, |ui| {

                // Check if video feature is available
                if !VideoDecoder::is_available() {
                    ui.label(egui::RichText::new("Video not available on this platform").color(egui::Color32::YELLOW));
                    return;
                }

                // === VISIBILITY (row 1) ===
                ui.horizontal(|ui| {
                    // Toggle video visibility button (eye icon, MIDI learnable)
                    let video_visible = self.params.video_visibility > 0.5;
                    let video_eye_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8)
                    } else if video_visible {
                        egui::Color32::from_rgb(100, 220, 100)
                    } else {
                        egui::Color32::from_rgb(120, 120, 120)
                    };
                    let video_eye_tooltip = if self.midi_learn_armed {
                        "Click to map Toggle Video to MIDI"
                    } else if video_visible {
                        "Hide video"
                    } else {
                        "Show video"
                    };
                    let video_eye_response = ui.add(
                        egui::Button::new(egui::RichText::new(egui_phosphor::regular::EYE).color(video_eye_color))
                            .fill(egui::Color32::TRANSPARENT)
                            .min_size(egui::vec2(24.0, 20.0))
                    ).on_hover_text(video_eye_tooltip);

                    if video_eye_response.clicked() {
                        if self.midi_learn_armed {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleVideo);
                        } else {
                            self.params.video_visibility = if video_visible { 0.0 } else { 1.0 };
                            self.mark_dirty();
                        }
                    }

                    // Video Visibility slider - MIDI learnable
                    let old_vis = self.params.video_visibility;

                    // Yellow pulsing slider when in learn mode
                    let slider_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                        // Override slider colors for learn mode
                        let mut style = (*ui.ctx().style()).clone();
                        style.visuals.widgets.inactive.bg_fill = yellow;
                        style.visuals.widgets.hovered.bg_fill = yellow;
                        style.visuals.widgets.active.bg_fill = yellow;
                        ui.ctx().set_style(style.clone());

                        let response = ui.add(egui::Slider::new(&mut self.params.video_visibility, 0.0..=1.0)
                            .show_value(false).fixed_decimals(2))
                            .on_hover_text("Click to map Video Visibility to MIDI fader");

                        // Restore default style
                        ui.ctx().set_style(egui::Style::default());
                        response
                    } else {
                        ui.add(egui::Slider::new(&mut self.params.video_visibility, 0.0..=1.0)
                            .show_value(false).fixed_decimals(2))
                    };

                    // If learn mode is armed and slider is being dragged, map to VideoVisibility
                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::VideoVisibility);
                        // Restore value since we're learning, not adjusting
                        self.params.video_visibility = old_vis;
                    } else if (self.params.video_visibility - old_vis).abs() > 0.001 {
                        self.mark_dirty();
                    }
                });

                // === SCALE (row 2) ===
                ui.horizontal(|ui| {
                    // Video Scale icon
                    let scale_icon_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                    } else {
                        egui::Color32::from_rgb(150, 150, 150)
                    };
                    ui.label(egui::RichText::new(egui_phosphor::regular::FRAME_CORNERS).color(scale_icon_color))
                        .on_hover_text("Scale");

                    // Video Scale slider - MIDI learnable
                    let old_scale = self.video_scale_target;

                    // Yellow pulsing slider when in learn mode
                    let slider_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                        let mut style = (*ui.ctx().style()).clone();
                        style.visuals.widgets.inactive.bg_fill = yellow;
                        style.visuals.widgets.hovered.bg_fill = yellow;
                        style.visuals.widgets.active.bg_fill = yellow;
                        ui.ctx().set_style(style.clone());

                        let response = ui.add(egui::Slider::new(&mut self.video_scale_target, 0.05..=4.0)
                            .show_value(false).fixed_decimals(2))
                            .on_hover_text("Click to map Video Scale to MIDI fader");

                        ui.ctx().set_style(egui::Style::default());
                        response
                    } else {
                        ui.add(egui::Slider::new(&mut self.video_scale_target, 0.05..=4.0)
                            .show_value(false).fixed_decimals(2))
                    };

                    // If learn mode is armed and slider is being dragged, map to VideoScale
                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::VideoScale);
                        self.video_scale_target = old_scale;
                    } else if (self.video_scale_target - old_scale).abs() > 0.001 {
                        self.mark_dirty();
                    }
                });

                // Luma Key slider (makes dark areas transparent) - MIDI learnable
                ui.horizontal(|ui| {
                    let luma_icon_color = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                    } else {
                        egui::Color32::from_rgb(150, 150, 150)
                    };
                    ui.label(egui::RichText::new("Luma Key:").size(11.0).color(luma_icon_color));

                    let old_luma = self.params.video_luma_key;

                    let slider_response = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);
                        let mut style = (*ui.ctx().style()).clone();
                        style.visuals.widgets.inactive.bg_fill = yellow;
                        style.visuals.widgets.hovered.bg_fill = yellow;
                        style.visuals.widgets.active.bg_fill = yellow;
                        ui.ctx().set_style(style.clone());
                        let response = ui.add(egui::Slider::new(&mut self.params.video_luma_key, 0.0..=1.0)
                            .show_value(false))
                            .on_hover_text("Click to map Video Luma Key to MIDI fader");
                        ui.ctx().set_style(egui::Style::default());
                        response
                    } else {
                        ui.add(egui::Slider::new(&mut self.params.video_luma_key, 0.0..=1.0)
                            .show_value(false))
                            .on_hover_text("Make dark areas transparent (0=off, 1=full)")
                    };

                    if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::VideoLumaKey);
                        self.params.video_luma_key = old_luma;
                    } else if (self.params.video_luma_key - old_luma).abs() > 0.001 {
                        self.mark_dirty();
                    }
                });

                ui.add_space(4.0);

                // === NOW PLAYING SECTION ===
                if self.video_decoder.is_loaded() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("NOW PLAYING").size(11.0).color(egui::Color32::from_rgb(100, 255, 100)));
                        });

                        // Current video name
                        if let Some(ref path) = self.video_path {
                            let name = Self::clean_video_name(path.split('/').last().unwrap_or(path));
                            ui.label(egui::RichText::new(format!("▶ {}", name)).strong());
                        }

                        // Playback controls
                        ui.horizontal(|ui| {
                            let btn_text = if self.video_decoder.is_playing() { "⏸" } else { "▶" };
                            if ui.button(btn_text).clicked() {
                                self.video_decoder.toggle();
                            }
                            if ui.button("⏹").on_hover_text("Clear video").clicked() {
                                self.video_decoder.close();
                                self.video_path = None;
                                if let Some(ref mut p) = pipeline {
                                    p.clear_video();
                                }
                            }

                            // Compact stats
                            let dur = self.video_decoder.duration();
                            let pos = self.video_decoder.position();
                            ui.label(egui::RichText::new(format!("{:.0}s / {:.0}s", pos, dur))
                                .size(11.0).color(egui::Color32::GRAY));
                        });
                    });
                    ui.add_space(4.0);
                }

                // === FOLDER SELECTION & SYNC ===
                ui.horizontal(|ui| {
                    // Manager button
                    if ui.small_button(format!("{}", egui_phosphor::regular::SQUARES_FOUR))
                        .on_hover_text("Open Compel Manager")
                        .clicked()
                    {
                        self.show_compel_manager = true;
                    }

                    if ui.small_button(format!("{} Folder...", egui_phosphor::regular::FOLDER_OPEN)).clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            let path_str = path.display().to_string();
                            self.video_directory = Some(path_str.clone());
                            self.scan_video_directory(&path_str);
                            if let Some(ref db) = self.db {
                                let _ = db.set_setting("video_directory", &path_str);
                            }
                            // Create sync manager for this directory
                            self.video_sync = Some(VideoSync::new(PathBuf::from(&path_str)));
                            if let Some(ref sync) = self.compel_sync {
                                sync.set_video_dir(PathBuf::from(&path_str));
                            }
                        }
                    }
                    if ui.small_button(format!("{} File...", egui_phosphor::regular::FILE_VIDEO)).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Video", &["mp4", "mov", "webm", "mkv", "avi", "m4v"])
                            .pick_file()
                        {
                            let path_str = path.display().to_string();
                            if let Ok(()) = self.video_decoder.open(&path_str) {
                                self.add_to_recent_videos(&path_str);
                                self.video_path = Some(path_str);
                                self.video_decoder.play();
                            }
                        }
                    }

                    // Sync button (always visible)
                    let is_syncing = self.video_sync.as_ref().map(|s| s.is_running()).unwrap_or(false);

                    let sync_btn_text = if is_syncing {
                        egui_phosphor::regular::SPINNER
                    } else {
                        egui_phosphor::regular::CLOUD_ARROW_DOWN
                    };
                    let sync_hover = if is_syncing {
                        "Syncing..."
                    } else if self.video_directory.is_some() {
                        "Sync from Compeller.ai"
                    } else {
                        "Select folder first to sync"
                    };
                    let sync_btn = ui.small_button(sync_btn_text).on_hover_text(sync_hover);

                    if sync_btn.clicked() {
                        if is_syncing {
                            self.show_sync_window = !self.show_sync_window;
                        } else if let Some(ref dir) = self.video_directory.clone() {
                            if self.video_sync.is_none() {
                                self.video_sync = Some(VideoSync::new(PathBuf::from(&dir)));
                            }
                            // Fetch list from server (non-blocking)
                            if let (Some(ref sync), Some(ref db)) = (&self.video_sync, &self.db) {
                                sync.refresh_list(db.clone());
                            }
                            self.show_sync_window = true;
                        } else {
                            // No directory selected - prompt to select one
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                let path_str = path.display().to_string();
                                self.video_directory = Some(path_str.clone());
                                self.scan_video_directory(&path_str);
                                if let Some(ref db) = self.db {
                                    let _ = db.set_setting("video_directory", &path_str);
                                }
                                self.video_sync = Some(VideoSync::new(path.clone()));
                                if let Some(ref sync) = self.compel_sync {
                                    sync.set_video_dir(path);
                                }
                                // Fetch list from server (non-blocking)
                                if let (Some(ref sync), Some(ref db)) = (&self.video_sync, &self.db) {
                                    sync.refresh_list(db.clone());
                                }
                                self.show_sync_window = true;
                            }
                        }
                    }
                    if let Some(ref dir) = self.video_directory.clone() {
                        // Refresh button
                        if ui.small_button(egui_phosphor::regular::ARROWS_CLOCKWISE).on_hover_text("Refresh video list").clicked() {
                            self.scan_video_directory(&dir);
                        }
                    }
                });

                // === SEARCH BOX + VIEW MODE ===
                ui.horizontal(|ui| {
                    ui.label(egui_phosphor::regular::MAGNIFYING_GLASS);
                    ui.add(egui::TextEdit::singleline(&mut self.video_search)
                        .hint_text("Search videos...")
                        .desired_width(ui.available_width() - 80.0));
                    if !self.video_search.is_empty() {
                        if ui.small_button(egui_phosphor::regular::X).clicked() {
                            self.video_search.clear();
                        }
                    }

                    // View mode toggle
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let grid_color = if self.video_view_mode == VideoViewMode::Grid {
                            egui::Color32::from_rgb(100, 180, 255)
                        } else {
                            egui::Color32::from_rgb(100, 100, 100)
                        };
                        let list_color = if self.video_view_mode == VideoViewMode::List {
                            egui::Color32::from_rgb(100, 180, 255)
                        } else {
                            egui::Color32::from_rgb(100, 100, 100)
                        };

                        if ui.add(
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::SQUARES_FOUR).color(grid_color))
                                .fill(egui::Color32::TRANSPARENT)
                                .min_size(egui::vec2(20.0, 18.0))
                        ).on_hover_text("Grid view").clicked() {
                            self.video_view_mode = VideoViewMode::Grid;
                        }

                        if ui.add(
                            egui::Button::new(egui::RichText::new(egui_phosphor::regular::LIST).color(list_color))
                                .fill(egui::Color32::TRANSPARENT)
                                .min_size(egui::vec2(20.0, 18.0))
                        ).on_hover_text("List view").clicked() {
                            self.video_view_mode = VideoViewMode::List;
                        }
                    });
                });

                // === VIDEO LISTS ===
                if let Some(ref dir) = self.video_directory.clone() {
                    let search_lower = self.video_search.to_lowercase();
                    let has_search = !search_lower.is_empty();

                    // Collect videos into groups
                    let mut favorites: Vec<(String, String)> = Vec::new();
                    let mut recent: Vec<(String, String)> = Vec::new();
                    let mut all_videos: Vec<(String, String)> = Vec::new();

                    for video_name in &self.video_files {
                        let full_path = format!("{}/{}", dir, video_name);
                        let display_name = self.get_clean_video_display_name(&full_path, video_name);

                        // Skip hidden
                        if self.is_video_hidden(&full_path) && !self.show_hidden_videos {
                            continue;
                        }

                        // Filter by search
                        if has_search && !display_name.to_lowercase().contains(&search_lower) {
                            continue;
                        }

                        all_videos.push((full_path.clone(), display_name.clone()));

                        if self.video_favorites.contains(&full_path) {
                            favorites.push((full_path.clone(), display_name.clone()));
                        }

                        if self.recent_videos.contains(&full_path) {
                            recent.push((full_path.clone(), display_name.clone()));
                        }
                    }

                    // Sort recent by order in recent_videos
                    recent.sort_by_key(|(path, _)| {
                        self.recent_videos.iter().position(|p| p == path).unwrap_or(999)
                    });

                    // Get egui context for texture loading
                    let ctx = ui.ctx().clone();

                    egui::ScrollArea::vertical()
                        .min_scrolled_height(200.0)
                        .max_height(800.0)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                        .show(ui, |ui| {
                            match self.video_view_mode {
                                VideoViewMode::Grid => {
                                    // === GRID VIEW (always show all videos) ===

                                    // Header with count and hidden toggle
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(format!("{} videos", all_videos.len())).size(11.0).color(egui::Color32::GRAY));

                                        // Show hidden toggle
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let hidden_color = if self.show_hidden_videos {
                                                egui::Color32::from_rgb(180, 100, 180)
                                            } else {
                                                egui::Color32::from_rgb(60, 60, 60)
                                            };
                                            let eye_icon = if self.show_hidden_videos {
                                                egui_phosphor::regular::EYE
                                            } else {
                                                egui_phosphor::regular::EYE_SLASH
                                            };
                                            if ui.add(
                                                egui::Button::new(egui::RichText::new(eye_icon).size(12.0).color(hidden_color))
                                                    .fill(egui::Color32::TRANSPARENT)
                                            ).on_hover_text(if self.show_hidden_videos { "Hide hidden" } else { "Show hidden" }).clicked() {
                                                self.show_hidden_videos = !self.show_hidden_videos;
                                            }
                                        });
                                    });

                                    // Render all videos in grid (favorites shown with star overlay)
                                    self.render_video_grid(ui, &ctx, &all_videos, "video_grid_all", pipeline);
                                }
                                VideoViewMode::List => {
                                    // === LIST VIEW ===

                                    // Favorites
                                    if !favorites.is_empty() || !has_search {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(format!("{} FAVORITES", egui_phosphor::regular::STAR)).size(11.0).color(egui::Color32::from_rgb(255, 200, 50)));
                                            ui.label(egui::RichText::new(format!("({})", favorites.len())).size(10.0).color(egui::Color32::GRAY));
                                        });
                                        for (full_path, display_name) in &favorites {
                                            self.render_video_row(ui, full_path, display_name, &dir, pipeline);
                                        }
                                        if favorites.is_empty() && !has_search {
                                            ui.label(egui::RichText::new("  Right-click to add favorites").size(10.0).color(egui::Color32::from_rgb(80, 80, 80)));
                                        }
                                        ui.add_space(4.0);
                                    }

                                    // Recent
                                    if !recent.is_empty() {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(format!("{} RECENT", egui_phosphor::regular::CLOCK_COUNTER_CLOCKWISE)).size(11.0).color(egui::Color32::from_rgb(150, 150, 200)));
                                            ui.label(egui::RichText::new(format!("({})", recent.len())).size(10.0).color(egui::Color32::GRAY));
                                        });
                                        for (full_path, display_name) in &recent {
                                            self.render_video_row(ui, full_path, display_name, &dir, pipeline);
                                        }
                                        ui.add_space(4.0);
                                    }

                                    // All (collapsed unless searching)
                                    let all_count = all_videos.len();
                                    let show_all = has_search || self.show_all_videos;

                                    ui.horizontal(|ui| {
                                        let arrow = if show_all {
                                            egui_phosphor::regular::CARET_DOWN
                                        } else {
                                            egui_phosphor::regular::CARET_RIGHT
                                        };
                                        if ui.add(egui::Label::new(
                                            egui::RichText::new(format!("{} ALL", arrow)).size(11.0).color(egui::Color32::from_rgb(120, 120, 120))
                                        ).sense(egui::Sense::click())).clicked() {
                                            self.show_all_videos = !self.show_all_videos;
                                        }
                                        ui.label(egui::RichText::new(format!("({})", all_count)).size(10.0).color(egui::Color32::GRAY));

                                        // Show hidden toggle
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let hidden_color = if self.show_hidden_videos {
                                                egui::Color32::from_rgb(180, 100, 180)
                                            } else {
                                                egui::Color32::from_rgb(60, 60, 60)
                                            };
                                            let eye_icon = if self.show_hidden_videos {
                                                egui_phosphor::regular::EYE
                                            } else {
                                                egui_phosphor::regular::EYE_SLASH
                                            };
                                            if ui.add(
                                                egui::Button::new(egui::RichText::new(eye_icon).size(12.0).color(hidden_color))
                                                    .fill(egui::Color32::TRANSPARENT)
                                            ).on_hover_text(if self.show_hidden_videos { "Hide hidden" } else { "Show hidden" }).clicked() {
                                                self.show_hidden_videos = !self.show_hidden_videos;
                                            }
                                        });
                                    });

                                    if show_all {
                                        for (full_path, display_name) in &all_videos {
                                            self.render_video_row(ui, full_path, display_name, &dir, pipeline);
                                        }
                                    }
                                }
                            }
                        });
                }

                // Video track management popup rendered in app.rs (floating, independent of menu)

                // Sync window popup
                self.render_sync_window(ui);
            });
    }

    /// Clean video name: remove extension and leading numbers
    fn clean_video_name(name: &str) -> String {
        let name = name
            .trim_end_matches(".mp4")
            .trim_end_matches(".mov")
            .trim_end_matches(".webm")
            .trim_end_matches(".mkv")
            .trim_end_matches(".avi")
            .trim_end_matches(".m4v");

        // Remove leading numbers and separators (e.g., "01 - ", "001_", "1. ")
        let trimmed = name.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '_' || c == ' ');
        if trimmed.is_empty() { name.to_string() } else { trimmed.to_string() }
    }

    /// Get clean display name for video
    fn get_clean_video_display_name(&self, full_path: &str, filename: &str) -> String {
        // Check for custom name first
        if let Some(meta) = self.video_metadata.get(full_path) {
            if let Some(ref custom) = meta.custom_name {
                return custom.clone();
            }
        }
        Self::clean_video_name(filename)
    }

    /// Add video to recent list
    fn add_to_recent_videos(&mut self, path: &str) {
        // Remove if already exists
        self.recent_videos.retain(|p| p != path);
        // Add to front
        self.recent_videos.insert(0, path.to_string());
        // Keep only last 7
        self.recent_videos.truncate(7);

        // Persist to database
        if let Some(ref db) = self.db {
            let _ = db.add_recent_video(path);
        }
    }

    /// Toggle video favorite
    fn toggle_video_favorite(&mut self, path: &str) {
        let is_favorite = self.video_favorites.contains(path);
        if is_favorite {
            self.video_favorites.remove(path);
        } else {
            self.video_favorites.insert(path.to_string());
        }

        // Persist to database
        if let Some(ref db) = self.db {
            let _ = db.set_video_favorite(path, !is_favorite);
        }
    }

    /// Load video favorites and recent from database
    fn load_video_favorites_and_recent(&mut self) {
        if let Some(ref db) = self.db {
            // Load favorites
            self.video_favorites = db.get_video_favorites().into_iter().collect();
            // Load recent
            self.recent_videos = db.get_recent_videos();
        }
    }

    /// Get all video candidates for compel matcher as (full_path, display_name) pairs
    /// Optionally filtered by artist (case-insensitive contains match)
    pub fn get_compel_candidates(&self, artist_filter: Option<&str>) -> Vec<(String, String)> {
        let dir = match &self.video_directory {
            Some(d) => d,
            None => return Vec::new(),
        };

        self.video_files.iter()
            .filter_map(|filename| {
                // Build full path
                let full_path = format!("{}/{}", dir, filename);

                // Skip hidden videos
                if let Some(meta) = self.video_metadata.get(&full_path) {
                    if meta.hidden {
                        return None;
                    }
                }

                // Get display name
                let display_name = self.get_clean_video_display_name(&full_path, filename);

                // Apply artist filter if provided
                if let Some(artist) = artist_filter {
                    let artist_lower = artist.to_lowercase();
                    let display_lower = display_name.to_lowercase();
                    // Skip if artist doesn't match (contains)
                    if !display_lower.contains(&artist_lower) && !artist_lower.contains(&display_lower) {
                        return None;
                    }
                }

                Some((full_path, display_name))
            })
            .collect()
    }

    /// Render a single video row
    fn render_video_row(&mut self, ui: &mut egui::Ui, full_path: &str, display_name: &str, dir: &str, pipeline: &mut Option<RenderPipeline>) {
        let is_current = self.video_path.as_ref().map(|p| p == full_path).unwrap_or(false);
        let is_browsed = self.browsed_video_path.as_ref().map(|p| p == full_path).unwrap_or(false);
        let is_hidden = self.is_video_hidden(full_path);
        let is_favorite = self.video_favorites.contains(full_path);

        // Check if editing
        let is_editing = self.editing_video.as_ref().map(|ep| ep == full_path).unwrap_or(false);

        if is_editing {
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.editing_video_name)
                        .desired_width(150.0)
                );
                if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(ref db) = self.db {
                        let name = self.editing_video_name.trim();
                        if !name.is_empty() {
                            let _ = db.set_video_custom_name(full_path, name);
                            self.refresh_video_metadata();
                        }
                    }
                    self.editing_video = None;
                    self.editing_video_name.clear();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.editing_video = None;
                    self.editing_video_name.clear();
                }
                response.request_focus();
            });
            return;
        }

        ui.horizontal(|ui| {
            // Favorite star
            let star = if is_favorite { "★" } else { "☆" };
            let star_color = if is_favorite {
                egui::Color32::from_rgb(255, 200, 50)
            } else {
                egui::Color32::from_rgb(60, 60, 60)
            };
            if ui.add(
                egui::Button::new(egui::RichText::new(star).color(star_color).size(12.0))
                    .fill(egui::Color32::TRANSPARENT)
                    .min_size(egui::vec2(18.0, 18.0))
            ).on_hover_text(if is_favorite { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                self.toggle_video_favorite(full_path);
            }

            // Video name - with browse cursor highlight
            let label = if is_current {
                egui::RichText::new(format!("▶ {}", display_name))
                    .color(egui::Color32::from_rgb(100, 255, 100))
                    .strong()
            } else if is_browsed {
                // Pulsing yellow highlight for browsed item
                let pulse = (ui.input(|i| i.time) * 3.0).sin() * 0.3 + 0.7;
                let brightness = (200.0 * pulse as f32) as u8;
                egui::RichText::new(format!("→ {}", display_name))
                    .color(egui::Color32::from_rgb(255, brightness, 50))
                    .strong()
            } else if is_hidden {
                egui::RichText::new(display_name).color(egui::Color32::from_rgb(80, 80, 80))
            } else {
                egui::RichText::new(display_name).color(egui::Color32::from_rgb(180, 180, 180))
            };

            let response = ui.selectable_label(is_current || is_browsed, label);

            // Context menu
            response.clone().context_menu(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

                let fav_text = if is_favorite { "★  Remove Favorite" } else { "☆  Add to Favorites" };
                if ui.button(fav_text).clicked() {
                    self.toggle_video_favorite(full_path);
                    ui.close_menu();
                }

                if ui.button("✏️  Rename...").clicked() {
                    self.editing_video = Some(full_path.to_string());
                    self.editing_video_name = display_name.to_string();
                    ui.close_menu();
                }

                if is_hidden {
                    if ui.button("👁  Show").clicked() {
                        if let Some(ref db) = self.db {
                            let _ = db.set_video_hidden(full_path, false);
                            self.refresh_video_metadata();
                        }
                        ui.close_menu();
                    }
                } else {
                    if ui.button("🙈  Hide").clicked() {
                        if let Some(ref db) = self.db {
                            let _ = db.set_video_hidden(full_path, true);
                            self.refresh_video_metadata();
                        }
                        ui.close_menu();
                    }
                }

                ui.separator();

                if ui.button("🎵  Manage Tracks...").clicked() {
                    self.managing_video_tracks = Some(full_path.to_string());
                    if let Some(ref db) = self.db {
                        self.video_tracks = db.get_video_tracks(full_path);
                    }
                    ui.close_menu();
                }
            });

            if response.clicked() {
                log::info!("Loading video: {}", full_path);
                match self.video_decoder.open(full_path) {
                    Ok(()) => {
                        self.add_to_recent_videos(full_path);
                        self.video_path = Some(full_path.to_string());
                        self.video_decoder.play();
                    }
                    Err(e) => {
                        log::error!("Failed to load video: {}", e);
                    }
                }
            }
        });
    }

    /// Load a thumbnail texture if available
    #[cfg(target_os = "macos")]
    fn get_or_load_thumbnail(&mut self, ctx: &egui::Context, full_path: &str) -> Option<egui::TextureHandle> {
        // Return cached texture if available
        if let Some(texture) = self.thumbnail_textures.get(full_path) {
            return Some(texture.clone());
        }

        // Check if already pending
        if self.pending_thumbnail_loads.contains(full_path) {
            return None;
        }

        // Check if thumbnail exists on disk
        if let Some(ref gen) = self.thumbnail_generator {
            let thumb_path = gen.get_thumbnail_path(full_path);
            if thumb_path.exists() {
                // Load the image
                if let Ok(img) = image::open(&thumb_path) {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let pixels = rgba.into_raw();
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                    let texture = ctx.load_texture(
                        full_path,
                        color_image,
                        egui::TextureOptions::LINEAR
                    );
                    self.thumbnail_textures.insert(full_path.to_string(), texture.clone());
                    return Some(texture);
                }
            } else {
                // Queue for generation
                gen.queue_video(full_path);
            }
        }

        None
    }

    /// Fallback for non-macOS
    #[cfg(not(target_os = "macos"))]
    fn get_or_load_thumbnail(&mut self, _ctx: &egui::Context, _full_path: &str) -> Option<egui::TextureHandle> {
        None
    }

    /// Get cached thumbnail texture (doesn't trigger loading)
    /// Used for overlays where we just want the cached texture if available
    pub fn get_thumbnail_texture(&self, full_path: &str) -> Option<egui::TextureHandle> {
        self.thumbnail_textures.get(full_path).cloned()
    }

    /// Get or load thumbnail texture for overlays
    /// Triggers loading if not already cached
    pub fn get_or_load_thumbnail_for_overlay(&mut self, ctx: &egui::Context, full_path: &str) -> Option<egui::TextureHandle> {
        self.get_or_load_thumbnail(ctx, full_path)
    }

    /// Render a video grid item (thumbnail + name)
    fn render_video_grid_item(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_path: &str,
        display_name: &str,
        _pipeline: &mut Option<RenderPipeline>,
    ) -> bool {
        let is_current = self.video_path.as_ref().map(|p| p == full_path).unwrap_or(false);
        let is_browsed = self.browsed_video_path.as_ref().map(|p| p == full_path).unwrap_or(false);
        let is_favorite = self.video_favorites.contains(full_path);
        let is_hidden = self.is_video_hidden(full_path);

        let thumb_width = 160.0;
        let thumb_height = 90.0;
        let total_height = thumb_height + 22.0; // thumbnail + label

        // Allocate space for entire item (thumbnail + label)
        let (item_rect, item_response) = ui.allocate_exact_size(
            egui::vec2(thumb_width, total_height),
            egui::Sense::click()
        );

        let thumb_rect = egui::Rect::from_min_size(
            item_rect.min,
            egui::vec2(thumb_width, thumb_height)
        );

        // Item frame with selection highlight
        let frame_color = if is_current {
            egui::Color32::from_rgb(40, 100, 40)
        } else if is_browsed {
            let pulse = (ui.input(|i| i.time) * 3.0).sin() * 0.3 + 0.7;
            let brightness = (80.0 * pulse as f32) as u8;
            egui::Color32::from_rgb(80, brightness, 20)
        } else if item_response.hovered() {
            egui::Color32::from_rgb(50, 50, 60)
        } else {
            egui::Color32::from_rgb(35, 35, 40)
        };

        // Draw thumbnail background
        ui.painter().rect_filled(thumb_rect, 4.0, frame_color);

        // Try to get thumbnail - fit within bounds preserving aspect ratio
        if let Some(texture) = self.get_or_load_thumbnail(ctx, full_path) {
            let tex_size = texture.size_vec2();
            let tex_aspect = tex_size.x / tex_size.y;
            let thumb_aspect = thumb_width / thumb_height;

            // Calculate display rect that fits image within thumb_rect preserving aspect
            let display_rect = if tex_aspect > thumb_aspect {
                // Image is wider - fit to width, letterbox vertically
                let display_height = thumb_width / tex_aspect;
                let y_offset = (thumb_height - display_height) / 2.0;
                egui::Rect::from_min_size(
                    thumb_rect.min + egui::vec2(0.0, y_offset),
                    egui::vec2(thumb_width, display_height)
                )
            } else {
                // Image is taller - fit to height, pillarbox horizontally
                let display_width = thumb_height * tex_aspect;
                let x_offset = (thumb_width - display_width) / 2.0;
                egui::Rect::from_min_size(
                    thumb_rect.min + egui::vec2(x_offset, 0.0),
                    egui::vec2(display_width, thumb_height)
                )
            };

            ui.painter().image(
                texture.id(),
                display_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            // Draw placeholder with video icon
            let icon_color = egui::Color32::from_rgb(80, 80, 90);
            ui.painter().text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::VIDEO,
                egui::FontId::proportional(24.0),
                icon_color,
            );
        }

        // Playing indicator overlay
        if is_current {
            ui.painter().text(
                thumb_rect.left_top() + egui::vec2(4.0, 4.0),
                egui::Align2::LEFT_TOP,
                "▶",
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(100, 255, 100),
            );
        }

        // Favorite star overlay
        if is_favorite {
            ui.painter().text(
                thumb_rect.right_top() + egui::vec2(-4.0, 4.0),
                egui::Align2::RIGHT_TOP,
                "★",
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(255, 200, 50),
            );
        }

        // Border
        let border_color = if is_current {
            egui::Color32::from_rgb(100, 255, 100)
        } else if is_browsed {
            egui::Color32::from_rgb(255, 200, 50)
        } else {
            egui::Color32::from_rgb(50, 50, 55)
        };
        ui.painter().rect_stroke(thumb_rect, 4.0, egui::Stroke::new(1.0, border_color));

        // Name label (truncated at char boundary to avoid panics on multi-byte UTF-8)
        let max_chars = 18;
        let truncated_name = if display_name.chars().count() > max_chars {
            let end = display_name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(display_name.len());
            format!("{}...", &display_name[..end])
        } else {
            display_name.to_string()
        };

        let name_color = if is_current {
            egui::Color32::from_rgb(100, 255, 100)
        } else {
            egui::Color32::from_rgb(180, 180, 180)
        };

        // Draw name below thumbnail
        let label_pos = egui::pos2(item_rect.min.x, thumb_rect.max.y + 2.0);
        ui.painter().text(
            label_pos,
            egui::Align2::LEFT_TOP,
            &truncated_name,
            egui::FontId::proportional(10.0),
            name_color,
        );

        // Context menu
        item_response.clone().context_menu(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

            let fav_text = if is_favorite { "★  Remove Favorite" } else { "☆  Add to Favorites" };
            if ui.button(fav_text).clicked() {
                self.toggle_video_favorite(full_path);
                ui.close_menu();
            }

            if ui.button("✏️  Rename...").clicked() {
                self.editing_video = Some(full_path.to_string());
                self.editing_video_name = display_name.to_string();
                ui.close_menu();
            }

            if is_hidden {
                if ui.button("👁  Show").clicked() {
                    if let Some(ref db) = self.db {
                        let _ = db.set_video_hidden(full_path, false);
                        self.refresh_video_metadata();
                    }
                    ui.close_menu();
                }
            } else {
                if ui.button("🙈  Hide").clicked() {
                    if let Some(ref db) = self.db {
                        let _ = db.set_video_hidden(full_path, true);
                        self.refresh_video_metadata();
                    }
                    ui.close_menu();
                }
            }

            ui.separator();

            if ui.button("🎵  Manage Tracks...").clicked() {
                self.managing_video_tracks = Some(full_path.to_string());
                if let Some(ref db) = self.db {
                    self.video_tracks = db.get_video_tracks(full_path);
                }
                ui.close_menu();
            }
        });

        // Handle click - load and play video
        if item_response.clicked() {
            log::info!("[Grid] Loading video: {}", full_path);
            match self.video_decoder.open(full_path) {
                Ok(()) => {
                    self.add_to_recent_videos(full_path);
                    self.video_path = Some(full_path.to_string());
                    self.video_decoder.play();
                    log::info!("[Grid] Video loaded and playing: {}", full_path);
                    return true;
                }
                Err(e) => {
                    log::error!("[Grid] Failed to load video: {}", e);
                }
            }
        }

        false
    }

    /// Render a grid of videos using horizontal_wrapped for proper click handling
    fn render_video_grid(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        videos: &[(String, String)],
        _grid_id: &str,
        pipeline: &mut Option<RenderPipeline>,
    ) {
        if videos.is_empty() {
            return;
        }

        // Use horizontal_wrapped for flow layout with proper click handling
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            for (full_path, display_name) in videos {
                self.render_video_grid_item(ui, ctx, full_path, display_name, pipeline);
            }
        });
    }

    /// Render a grid of recordings with thumbnails
    fn render_recording_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.recordings.is_empty() {
            return;
        }

        // Use horizontal_wrapped for flow layout
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
            let recordings = self.recordings.clone();
            for rec in recordings {
                self.render_recording_grid_item(ui, ctx, &rec);
            }
        });
    }

    /// Render a single recording grid item with thumbnail
    fn render_recording_grid_item(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rec: &RecordingInfo,
    ) {
        let thumb_width = 160.0;
        let thumb_height = 90.0;
        let total_height = thumb_height + 22.0; // thumbnail + label

        // Allocate space for entire item (thumbnail + label)
        let (item_rect, item_response) = ui.allocate_exact_size(
            egui::vec2(thumb_width, total_height),
            egui::Sense::click()
        );

        let thumb_rect = egui::Rect::from_min_size(
            item_rect.min,
            egui::vec2(thumb_width, thumb_height)
        );

        // Item frame with hover highlight
        let frame_color = if item_response.hovered() {
            egui::Color32::from_rgb(50, 50, 60)
        } else {
            egui::Color32::from_rgb(35, 35, 40)
        };

        // Draw thumbnail background
        ui.painter().rect_filled(thumb_rect, 4.0, frame_color);

        // Try to get thumbnail
        if let Some(texture) = self.get_or_load_thumbnail(ctx, &rec.file_path) {
            ui.painter().image(
                texture.id(),
                thumb_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            // Draw placeholder with recording icon
            let icon_color = egui::Color32::from_rgb(80, 80, 90);
            ui.painter().text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::RECORD,
                egui::FontId::proportional(24.0),
                icon_color,
            );
        }

        // File size badge in top-right
        let size_str = rec.size_string();
        ui.painter().text(
            thumb_rect.right_top() + egui::vec2(-4.0, 4.0),
            egui::Align2::RIGHT_TOP,
            &size_str,
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
        );

        // Sync status badge in top-left
        let upload_status = self.upload_status_cache.get(&rec.file_path);
        if let Some(status) = upload_status {
            let (icon, color) = match status {
                crate::db::SetUploadStatus::Completed => (
                    egui_phosphor::regular::CHECK_CIRCLE,
                    egui::Color32::from_rgb(80, 200, 80)
                ),
                crate::db::SetUploadStatus::Failed => (
                    egui_phosphor::regular::X_CIRCLE,
                    egui::Color32::from_rgb(220, 80, 80)
                ),
                crate::db::SetUploadStatus::Uploading => (
                    egui_phosphor::regular::CLOUD_ARROW_UP,
                    egui::Color32::from_rgb(100, 180, 255)
                ),
                crate::db::SetUploadStatus::Pending | crate::db::SetUploadStatus::Paused => (
                    egui_phosphor::regular::CLOUD,
                    egui::Color32::from_rgb(150, 150, 150)
                ),
            };
            // Draw background pill for contrast
            let badge_pos = thumb_rect.left_top() + egui::vec2(4.0, 4.0);
            ui.painter().circle_filled(
                badge_pos + egui::vec2(7.0, 7.0),
                10.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
            ui.painter().text(
                badge_pos,
                egui::Align2::LEFT_TOP,
                icon,
                egui::FontId::proportional(14.0),
                color,
            );
        }

        // Border - highlight synced recordings with green, failed with red
        let border_color = match upload_status {
            Some(crate::db::SetUploadStatus::Completed) => egui::Color32::from_rgb(60, 120, 60),
            Some(crate::db::SetUploadStatus::Failed) => egui::Color32::from_rgb(120, 50, 50),
            _ => egui::Color32::from_rgb(50, 50, 55),
        };
        ui.painter().rect_stroke(thumb_rect, 4.0, egui::Stroke::new(1.0, border_color));

        // Name label (truncated at char boundary)
        let display_name = rec.display_name();
        let max_chars = 18;
        let truncated_name = if display_name.chars().count() > max_chars {
            let end = display_name.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(display_name.len());
            format!("{}...", &display_name[..end])
        } else {
            display_name.to_string()
        };

        let name_color = egui::Color32::from_rgb(180, 180, 180);

        // Draw name below thumbnail
        let label_pos = egui::pos2(item_rect.min.x, thumb_rect.max.y + 2.0);
        ui.painter().text(
            label_pos,
            egui::Align2::LEFT_TOP,
            &truncated_name,
            egui::FontId::proportional(10.0),
            name_color,
        );

        // Context menu
        let file_path = rec.file_path.clone();
        let file_name = rec.file_name.clone();
        let custom_name = rec.custom_name.clone();
        let duration_secs = rec.duration_secs;
        let created_at = rec.created_at;
        let upload_status_for_menu = upload_status.cloned();
        item_response.clone().context_menu(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(16.0, 6.0);

            if ui.button("✏️   Rename Recording...").clicked() {
                self.editing_recording = Some(file_path.clone());
                self.editing_recording_name = custom_name.clone()
                    .unwrap_or_else(|| file_name.trim_end_matches(".mp4").to_string());
                ui.close_menu();
            }
            if ui.button("📂   Show in Finder").clicked() {
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open")
                        .arg("-R")
                        .arg(&file_path)
                        .spawn();
                }
                ui.close_menu();
            }
            ui.separator();

            // Upload/Sync options based on current status
            match upload_status_for_menu {
                Some(crate::db::SetUploadStatus::Completed) => {
                    // Already synced - show disabled "Synced" indicator
                    ui.add_enabled(false, egui::Button::new(format!("{}   Synced to Compeller.ai", egui_phosphor::regular::CHECK_CIRCLE)))
                        .on_disabled_hover_text("This recording is already synced");
                }
                Some(crate::db::SetUploadStatus::Failed) => {
                    // Failed - show retry option
                    if ui.button(format!("{}   Retry Upload", egui_phosphor::regular::ARROW_CLOCKWISE)).clicked() {
                        if let Some(ref db) = self.db {
                            if let Err(e) = db.retry_failed_upload(&file_path) {
                                log::error!("[UI] Failed to retry upload: {}", e);
                            } else {
                                log::info!("[UI] Retrying upload: {}", file_name);
                                // Refresh cache
                                self.upload_status_cache = db.get_upload_status_map();
                                // Start upload if not running
                                if let Some(ref upload_manager) = self.set_upload_manager {
                                    if !upload_manager.is_running() {
                                        upload_manager.start_upload();
                                    }
                                }
                            }
                        }
                        ui.close_menu();
                    }
                }
                Some(crate::db::SetUploadStatus::Uploading) | Some(crate::db::SetUploadStatus::Pending) | Some(crate::db::SetUploadStatus::Paused) => {
                    // In progress or queued - show cancel option
                    if ui.button(format!("{}   Cancel Upload", egui_phosphor::regular::X)).clicked() {
                        if let Some(ref upload_manager) = self.set_upload_manager {
                            let _ = upload_manager.cancel_upload(&file_path);
                            log::info!("[UI] Cancelled upload: {}", file_name);
                            // Refresh cache
                            if let Some(ref db) = self.db {
                                self.upload_status_cache = db.get_upload_status_map();
                            }
                        }
                        ui.close_menu();
                    }
                }
                None => {
                    // Not in queue - show upload option
                    if let Some(ref upload_manager) = self.set_upload_manager {
                        if ui.button(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)).clicked() {
                            // Use recording metadata for upload
                            let recorded_at_str = chrono::DateTime::from_timestamp(created_at, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| chrono::Local::now().to_rfc3339());
                            let request = crate::set_sync::SetUploadRequest {
                                file_path: file_path.clone(),
                                title: custom_name.clone().or_else(|| Some(file_name.trim_end_matches(".mp4").to_string())),
                                recorded_at: Some(recorded_at_str),
                                theme_used: None, // TODO: capture from recording session
                                audio_source: None, // TODO: capture from recording session
                                duration_seconds: duration_secs.map(|d| d as i64),
                            };
                            if let Err(e) = upload_manager.queue_upload(request) {
                                log::error!("[UI] Failed to queue upload: {}", e);
                            } else {
                                log::info!("[UI] Queued recording for upload: {}", file_name);
                                // Refresh cache
                                if let Some(ref db) = self.db {
                                    self.upload_status_cache = db.get_upload_status_map();
                                }
                                if !upload_manager.is_running() {
                                    upload_manager.start_upload();
                                }
                            }
                            ui.close_menu();
                        }
                    } else {
                        ui.add_enabled(false, egui::Button::new(format!("{}   Upload to Compeller.ai", egui_phosphor::regular::CLOUD_ARROW_UP)))
                            .on_disabled_hover_text("Login required to upload");
                    }
                }
            }

            ui.separator();
            if ui.button(format!("{}   Delete Recording", egui_phosphor::regular::TRASH)).clicked() {
                self.pending_delete_recording = Some(file_path.clone());
                ui.close_menu();
            }
        });

        // Handle click - open recording
        if item_response.clicked() {
            log::info!("[UI] Opening recording: {}", rec.file_path);
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .arg(&rec.file_path)
                    .spawn();
            }
        }

        // Hover cursor
        if item_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    /// Audio controls section (basic, without source selection)
    fn show_audio_section(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new(egui::RichText::new("Audio").strong().size(14.0))
            .default_open(self.audio_expanded)
            .show(ui, |ui| {
                // Gain slider
                ui.horizontal(|ui| {
                    ui.label("Gain:");
                    ui.add(egui::Slider::new(&mut self.audio_gain, 0.1..=128.0)
                        .show_value(true)
                        .logarithmic(true));
                });

                // Audio meters toggle
                ui.checkbox(&mut self.show_audio_meters, "Show Audio Meters");

                // Audio meters (placeholder)
                if self.show_audio_meters {
                    self.show_audio_meters_display(ui);
                }
            });

        // Track expanded state
        self.audio_expanded = header.fully_open();
    }

    /// Audio controls section with source selection
    fn show_audio_section_with_sources(
        &mut self,
        ui: &mut egui::Ui,
        current_source: AudioSource,
        audio_enabled: bool,
    ) -> AudioAction {
        self.show_audio_section_with_analysis(ui, current_source, audio_enabled, &AudioAnalysis::default())
    }

    /// Audio controls section with source selection and analysis
    fn show_audio_section_with_analysis(
        &mut self,
        ui: &mut egui::Ui,
        current_source: AudioSource,
        audio_enabled: bool,
        analysis: &AudioAnalysis,
    ) -> AudioAction {
        let mut action = AudioAction::None;

        let header = egui::CollapsingHeader::new(egui::RichText::new("Audio").strong().size(14.0))
            .default_open(self.audio_expanded)
            .show(ui, |ui| {
                // Refresh audio sources if empty or on button click
                if self.audio_sources.is_empty() {
                    self.audio_sources = get_available_sources();
                }

                // Start/Stop button
                ui.horizontal(|ui| {
                    if audio_enabled {
                        if ui.button("Stop Capture").clicked() {
                            action = AudioAction::Stop;
                        }
                        ui.label(egui::RichText::new("Capturing").color(egui::Color32::GREEN));
                    } else {
                        if ui.button("Start Capture").clicked() {
                            action = AudioAction::Start;
                        }
                        ui.label(egui::RichText::new("Stopped").color(egui::Color32::GRAY));
                    }
                });

                ui.separator();

                // Audio source selection
                ui.label("Audio Source:");

                // Refresh button
                ui.horizontal(|ui| {
                    if ui.button("Refresh Sources").clicked() {
                        self.audio_sources = get_available_sources();
                    }
                    if ui.checkbox(&mut self.show_all_apps, "Show all apps").changed() {
                        self.mark_dirty();
                    }
                });

                // Get current selection name
                let current_name = match &current_source {
                    AudioSource::SystemAudio => "System Audio (All)".to_string(),
                    AudioSource::Application { name, .. } => name.clone(),
                    AudioSource::InputDevice { name, .. } => format!("🎤 {}", name),
                    AudioSource::Microphone => "Microphone".to_string(),
                };

                // Source dropdown
                egui::ComboBox::from_id_salt("audio_source")
                    .selected_text(&current_name)
                    .width(280.0)  // Fixed width to prevent menu jumping
                    .show_ui(ui, |ui| {
                        for source_info in &self.audio_sources {
                            let selected = match (&current_source, &source_info.source) {
                                (AudioSource::SystemAudio, AudioSource::SystemAudio) => true,
                                (AudioSource::Application { bundle_id: b1, .. },
                                 AudioSource::Application { bundle_id: b2, .. }) => b1 == b2,
                                (AudioSource::InputDevice { device_name: d1, .. },
                                 AudioSource::InputDevice { device_name: d2, .. }) => d1 == d2,
                                (AudioSource::Microphone, AudioSource::Microphone) => true,
                                _ => false,
                            };

                            if ui.selectable_label(selected, &source_info.display_name).clicked() {
                                action = AudioAction::SetSource(source_info.source.clone());
                            }
                        }
                    });

                ui.separator();

                // Audio meters toggle
                if ui.checkbox(&mut self.show_audio_meters, "Show Audio Meters").changed() {
                    self.mark_dirty();
                }

                // Audio meters - prefer fundsp meters if available, fall back to FFT analysis
                if self.show_audio_meters {
                    if let Some(meters) = self.meter_values {
                        self.show_audio_meters_fundsp(ui, &meters);
                    } else {
                        self.show_audio_meters_with_analysis(ui, analysis);
                    }

                    // Sensitivity sliders - compact layout with phosphor icons
                    // MIDI learnable: click label when in learn mode to map
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let label_text = egui_phosphor::regular::LIGHTNING;
                        let label_response = if self.midi_learn_armed {
                            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                            let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                            ui.label(egui::RichText::new(label_text).color(highlight))
                                .on_hover_text("Click to map Audio Gain to MIDI")
                        } else {
                            ui.label(label_text).on_hover_text("Sensitivity - meters & visualizer reactivity")
                        };
                        let old_gain = self.audio_gain;

                        // Yellow pulsing slider when in learn mode
                        let slider_response = if self.midi_learn_armed {
                            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                            let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                            let mut style = (*ui.ctx().style()).clone();
                            style.visuals.widgets.inactive.bg_fill = yellow;
                            style.visuals.widgets.hovered.bg_fill = yellow;
                            style.visuals.widgets.active.bg_fill = yellow;
                            ui.ctx().set_style(style.clone());

                            let response = ui.add(egui::Slider::new(&mut self.audio_gain, 0.1..=128.0)
                                .show_value(false)
                                .logarithmic(true))
                                .on_hover_text("Click to map Audio Gain to MIDI fader");

                            ui.ctx().set_style(egui::Style::default());
                            response
                        } else {
                            ui.add(egui::Slider::new(&mut self.audio_gain, 0.1..=128.0)
                                .show_value(false)
                                .logarithmic(true))
                        };

                        // If learn mode is armed and slider is being dragged, map to AudioGain
                        if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::AudioGain);
                            self.audio_gain = old_gain;
                        } else if (self.audio_gain - old_gain).abs() > 0.001 {
                            action = AudioAction::SetGain(self.audio_gain);
                            self.mark_dirty();
                        }
                    });
                    ui.horizontal(|ui| {
                        let label_text = egui_phosphor::regular::PULSE;
                        let label_color = if self.midi_learn_armed {
                            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                            egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8)
                        } else {
                            egui::Color32::from_rgb(150, 150, 150)
                        };
                        ui.label(egui::RichText::new(label_text).color(label_color))
                            .on_hover_text("Kick detection sensitivity");

                        let old_kick = self.kick_sensitivity;

                        // Yellow pulsing slider when in learn mode
                        let slider_response = if self.midi_learn_armed {
                            let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                            let yellow = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (180.0 + 75.0 * pulse as f32) as u8);

                            let mut style = (*ui.ctx().style()).clone();
                            style.visuals.widgets.inactive.bg_fill = yellow;
                            style.visuals.widgets.hovered.bg_fill = yellow;
                            style.visuals.widgets.active.bg_fill = yellow;
                            ui.ctx().set_style(style.clone());

                            let response = ui.add(egui::Slider::new(&mut self.kick_sensitivity, 0.5..=5.0)
                                .show_value(false))
                                .on_hover_text("Click to map Kick Sensitivity to MIDI fader");

                            ui.ctx().set_style(egui::Style::default());
                            response
                        } else {
                            ui.add(egui::Slider::new(&mut self.kick_sensitivity, 0.5..=5.0)
                                .show_value(false))
                        };

                        // If learn mode is armed and slider is being dragged, map to KickSensitivity
                        if self.midi_learn_armed && (slider_response.drag_started() || slider_response.clicked()) {
                            self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::KickSensitivity);
                            self.kick_sensitivity = old_kick;
                        } else if (self.kick_sensitivity - old_kick).abs() > 0.001 {
                            self.mark_dirty();
                        }
                    });
                }
            });

        // Track expanded state
        self.audio_expanded = header.fully_open();

        action
    }

    /// Display audio meters (placeholder)
    fn show_audio_meters_display(&mut self, ui: &mut egui::Ui) {
        self.show_audio_meters_with_analysis(ui, &AudioAnalysis::default());
    }

    /// Display audio meters with real analysis data
    fn show_audio_meters_with_analysis(&mut self, ui: &mut egui::Ui, analysis: &AudioAnalysis) {
        ui.separator();

        // Update hold timers
        const HOLD_FRAMES: u32 = 10; // Hold for ~10 frames (~167ms at 60fps)
        if analysis.kick {
            self.kick_hold_frames = HOLD_FRAMES;
        } else if self.kick_hold_frames > 0 {
            self.kick_hold_frames -= 1;
        }

        let show_kick = self.kick_hold_frames > 0;

        // Bass/Mid/High meters with real FFT data
        let meter_height = 80.0;
        let meter_width = 24.0;

        ui.horizontal(|ui| {
            // Use actual analysis data
            let bass_level = analysis.bass;
            let mid_level = analysis.mid;
            let high_level = analysis.high;

            ui.vertical(|ui| {
                ui.label("Bass");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                let filled_height = rect.height() * bass_level;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                // Color based on intensity (red/orange for bass)
                let color = if show_kick {
                    egui::Color32::from_rgb(255, 50, 50) // Bright red on kick
                } else {
                    egui::Color32::from_rgb(255, 100, 100)
                };
                painter.rect_filled(filled_rect, 2.0, color);
            });

            ui.vertical(|ui| {
                ui.label("Mid");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                let filled_height = rect.height() * mid_level;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(100, 255, 100));
            });

            ui.vertical(|ui| {
                ui.label("High");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                let filled_height = rect.height() * high_level;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(100, 100, 255));
            });
        });

        // Kick indicator - red circle (no interaction)
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let circle_radius = 8.0;
            let (indicator_rect, _) = ui.allocate_exact_size(
                egui::vec2(circle_radius * 2.0 + 4.0, circle_radius * 2.0 + 4.0),
                egui::Sense::focusable_noninteractive(),
            );
            let painter = ui.painter_at(indicator_rect);
            let center = indicator_rect.center();

            painter.circle_stroke(center, circle_radius + 1.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 30, 30)));
            let fill_color = if show_kick {
                egui::Color32::from_rgb(255, 50, 50)
            } else {
                egui::Color32::from_rgb(45, 20, 20)
            };
            painter.circle_filled(center, circle_radius, fill_color);

            ui.add_space(4.0);
            ui.label(egui::RichText::new("Kick").size(11.0).color(
                if show_kick { egui::Color32::from_rgb(255, 150, 150) } else { egui::Color32::from_rgb(100, 60, 60) }
            ));
        });
    }

    /// Display audio meters with fundsp-based MeterValues (band splitting + peak hold)
    pub fn show_audio_meters_fundsp(&mut self, ui: &mut egui::Ui, meters: &MeterValues) {
        ui.separator();

        // Apply audio_gain to raw values (clamp to 0-1 for display)
        // Extra display boost makes meters more visible without affecting visualizer
        const METER_DISPLAY_BOOST: f32 = 1.5;
        let gain = self.audio_gain * METER_DISPLAY_BOOST;
        let bass = (meters.bass * gain).clamp(0.0, 1.0);
        let bass_peak = (meters.bass_peak * gain).clamp(0.0, 1.0);
        let mid = (meters.mid * gain).clamp(0.0, 1.0);
        let mid_peak = (meters.mid_peak * gain).clamp(0.0, 1.0);
        let high = (meters.high * gain).clamp(0.0, 1.0);
        let high_peak = (meters.high_peak * gain).clamp(0.0, 1.0);
        let rms = (meters.rms * gain).clamp(0.0, 1.0);
        let peak = (meters.peak * gain).clamp(0.0, 1.0);

        // Update kick hold timer
        // Kick triggers from DSP detection OR confidence * sensitivity exceeding threshold
        const HOLD_FRAMES: u32 = 10;
        let kick_triggered = meters.kick || (meters.kick_confidence * self.kick_sensitivity > 0.75);
        if kick_triggered {
            self.kick_hold_frames = HOLD_FRAMES;
        } else if self.kick_hold_frames > 0 {
            self.kick_hold_frames -= 1;
        }

        let show_kick = self.kick_hold_frames > 0;

        // Bass/Mid/High meters with fundsp band splitting + peak hold indicators
        let meter_height = 80.0;
        let meter_width = 30.0;  // All meters same width
        let baseline_color = egui::Color32::from_rgb(80, 80, 95);  // Visible baseline for silence

        ui.horizontal(|ui| {
            // Bass meter
            ui.vertical(|ui| {
                ui.label("Bass");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);

                // Background
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                // Baseline for silence
                painter.line_segment(
                    [egui::pos2(rect.min.x, rect.max.y - 1.0), egui::pos2(rect.max.x, rect.max.y - 1.0)],
                    egui::Stroke::new(1.0, baseline_color),
                );

                // Filled level (using gain-adjusted value)
                let filled_height = rect.height() * bass;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                let color = if show_kick {
                    egui::Color32::from_rgb(255, 50, 50) // Bright red on kick
                } else {
                    egui::Color32::from_rgb(255, 100, 100)
                };
                painter.rect_filled(filled_rect, 2.0, color);

                // Peak hold indicator (thin line)
                let peak_y = rect.max.y - (rect.height() * bass_peak);
                painter.line_segment(
                    [egui::pos2(rect.min.x, peak_y), egui::pos2(rect.max.x, peak_y)],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 200)),
                );
            });

            // Mid meter
            ui.vertical(|ui| {
                ui.label("Mid");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);

                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                painter.line_segment(
                    [egui::pos2(rect.min.x, rect.max.y - 1.0), egui::pos2(rect.max.x, rect.max.y - 1.0)],
                    egui::Stroke::new(1.0, baseline_color),
                );

                let filled_height = rect.height() * mid;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(100, 255, 100));

                // Peak hold indicator
                let peak_y = rect.max.y - (rect.height() * mid_peak);
                painter.line_segment(
                    [egui::pos2(rect.min.x, peak_y), egui::pos2(rect.max.x, peak_y)],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(200, 255, 200)),
                );
            });

            // High meter
            ui.vertical(|ui| {
                ui.label("High");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);

                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                painter.line_segment(
                    [egui::pos2(rect.min.x, rect.max.y - 1.0), egui::pos2(rect.max.x, rect.max.y - 1.0)],
                    egui::Stroke::new(1.0, baseline_color),
                );

                let filled_height = rect.height() * high;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(100, 100, 255));

                // Peak hold indicator
                let peak_y = rect.max.y - (rect.height() * high_peak);
                painter.line_segment(
                    [egui::pos2(rect.min.x, peak_y), egui::pos2(rect.max.x, peak_y)],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(200, 200, 255)),
                );
            });

            // RMS meter (overall level)
            ui.vertical(|ui| {
                ui.label("RMS");
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(meter_width, meter_height),
                    egui::Sense::hover(),
                );
                let painter = ui.painter_at(rect);

                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                painter.line_segment(
                    [egui::pos2(rect.min.x, rect.max.y - 1.0), egui::pos2(rect.max.x, rect.max.y - 1.0)],
                    egui::Stroke::new(1.0, baseline_color),
                );

                let filled_height = rect.height() * rms;
                let filled_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                    egui::vec2(rect.width(), filled_height),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(180, 180, 180));

                // Subtle peak hold tick
                let peak_y = rect.max.y - (rect.height() * peak);
                painter.line_segment(
                    [egui::pos2(rect.min.x, peak_y), egui::pos2(rect.max.x, peak_y)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 220, 220)),
                );
            });
        });

        // Kick section with breathing room
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            // KICK indicator - red circle (no interaction)
            let circle_radius = 8.0;
            let (indicator_rect, _) = ui.allocate_exact_size(
                egui::vec2(circle_radius * 2.0 + 4.0, circle_radius * 2.0 + 4.0),
                egui::Sense::focusable_noninteractive(),
            );
            let painter = ui.painter_at(indicator_rect);
            let center = indicator_rect.center();

            // Outer ring (subtle)
            painter.circle_stroke(center, circle_radius + 1.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 30, 30)));

            // Filled circle
            let fill_color = if show_kick {
                egui::Color32::from_rgb(255, 50, 50)
            } else {
                egui::Color32::from_rgb(45, 20, 20)
            };
            painter.circle_filled(center, circle_radius, fill_color);

            ui.add_space(4.0);
            ui.label(egui::RichText::new("Kick").size(11.0).color(
                if show_kick { egui::Color32::from_rgb(255, 150, 150) } else { egui::Color32::from_rgb(100, 60, 60) }
            ));

            ui.add_space(12.0);

            // Kick confidence/strength section
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Strength").size(10.0).color(egui::Color32::from_rgb(150, 150, 150)));
                let (conf_rect, _) = ui.allocate_exact_size(egui::vec2(80.0, 14.0), egui::Sense::hover());
                let painter = ui.painter_at(conf_rect);
                painter.rect_filled(conf_rect, 2.0, egui::Color32::from_rgb(40, 40, 50));
                let scaled_confidence = (meters.kick_confidence * self.kick_sensitivity).min(1.0);
                let filled_width = conf_rect.width() * scaled_confidence;
                let filled_rect = egui::Rect::from_min_size(
                    conf_rect.min,
                    egui::vec2(filled_width, conf_rect.height()),
                );
                painter.rect_filled(filled_rect, 2.0, egui::Color32::from_rgb(255, 150, 50));
            });
        });
    }

    /// Show Mythos engine parameters (with MIDI learn support)
    fn show_mythos_section(&mut self, ui: &mut egui::Ui, pipeline: &mut Option<RenderPipeline>) {
        use crate::midi::ControlTarget;

        let _header = egui::CollapsingHeader::new(egui::RichText::new("Mythos").strong().size(14.0))
            .default_open(true)
            .show(ui, |ui| {
                ui.separator();

                let label_color = egui::Color32::from_rgb(130, 130, 150);
                let learn_armed = self.midi_learn_armed;
                let mut changed = false;

                // Helper: create label with MIDI learn pulse highlight
                let make_label = |text: &str, armed: bool, ui: &egui::Ui| -> egui::RichText {
                    if armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new(text).size(11.0).color(highlight)
                    } else {
                        egui::RichText::new(text).size(11.0).color(label_color)
                    }
                };

                // Instability
                ui.horizontal(|ui| {
                    let label = make_label("Instability", learn_armed, ui);
                    let resp = ui.label(label);
                    if learn_armed && resp.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::MythosInstability);
                    }
                    if ui.add(egui::Slider::new(&mut self.mythos_instability, 0.0..=1.0).show_value(false)).changed() {
                        changed = true;
                    }
                });

                // Field Strength
                ui.horizontal(|ui| {
                    let label = make_label("Field Strength", learn_armed, ui);
                    let resp = ui.label(label);
                    if learn_armed && resp.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::MythosFieldStrength);
                    }
                    if ui.add(egui::Slider::new(&mut self.mythos_field_strength, 0.0..=1.0).show_value(false)).changed() {
                        changed = true;
                    }
                });

                // Disturbance
                ui.horizontal(|ui| {
                    let label = make_label("Disturbance", learn_armed, ui);
                    let resp = ui.label(label);
                    if learn_armed && resp.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::MythosDisturbanceSize);
                    }
                    if ui.add(egui::Slider::new(&mut self.mythos_disturbance_size, 0.0..=1.0).show_value(false)).changed() {
                        changed = true;
                    }
                });

                // Harmonic
                ui.horizontal(|ui| {
                    let label = make_label("Harmonic", learn_armed, ui);
                    let resp = ui.label(label);
                    if learn_armed && resp.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::MythosHarmonicInfluence);
                    }
                    if ui.add(egui::Slider::new(&mut self.mythos_harmonic_influence, 0.0..=1.0).show_value(false)).changed() {
                        changed = true;
                    }
                });

                // Decay
                ui.horizontal(|ui| {
                    let label = make_label("Decay", learn_armed, ui);
                    let resp = ui.label(label);
                    if learn_armed && resp.interact(egui::Sense::click()).clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::MythosDecay);
                    }
                    if ui.add(egui::Slider::new(&mut self.mythos_decay, 0.0..=1.0).show_value(false)).changed() {
                        changed = true;
                    }
                });

                if changed {
                    if let Some(ref mut p) = pipeline {
                        p.set_mythos_params(
                            self.mythos_instability,
                            self.mythos_field_strength,
                            self.mythos_disturbance_size,
                            self.mythos_harmonic_influence,
                            self.mythos_decay,
                        );
                    }
                    self.mark_dirty();
                }
            });
    }

    /// Show Robert Edward Grant section with REG-aligned visualization
    /// Geometry emerges from the 12-bin chroma, not named shapes
    fn show_harmonic_geometry_section(&mut self, ui: &mut egui::Ui, pipeline: &mut Option<RenderPipeline>) {
        let header = egui::CollapsingHeader::new(egui::RichText::new("Harmonic Field").strong().size(14.0))
            .default_open(self.harmonic_expanded)
            .show(ui, |ui| {
                // Get harmonic field state: use frozen state when frozen, otherwise live from pipeline
                let field_state = if self.frozen {
                    self.frozen_display_state.clone().unwrap_or_default()
                } else {
                    pipeline.as_ref()
                        .and_then(|p| p.get_harmonic_field_state())
                        .unwrap_or_default()
                };

                // Sync tuning reference to pipeline (ensures persisted setting is applied on startup)
                if let Some(ref mut p) = pipeline {
                    p.set_tuning_reference(self.tuning_reference);
                }

                ui.separator();

                // === DISPLAY OPTIONS (MIDI learnable) ===
                ui.horizontal(|ui| {
                    // Chroma checkbox - MIDI learnable
                    let old_chroma = self.show_chroma_key;
                    let chroma_label = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new("Chroma").color(highlight)
                    } else {
                        egui::RichText::new("Chroma")
                    };
                    let chroma_response = ui.checkbox(&mut self.show_chroma_key, chroma_label);
                    if self.midi_learn_armed && chroma_response.clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleChromaKey);
                        self.show_chroma_key = old_chroma;
                    } else if self.show_chroma_key != old_chroma {
                        self.mark_dirty();
                    }

                    // Math checkbox - MIDI learnable
                    let old_math = self.show_math_overlay;
                    let math_label = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new("Math").color(highlight)
                    } else {
                        egui::RichText::new("Math")
                    };
                    let math_response = ui.checkbox(&mut self.show_math_overlay, math_label);
                    if self.midi_learn_armed && math_response.clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleMathOverlay);
                        self.show_math_overlay = old_math;
                    } else if self.show_math_overlay != old_math {
                        self.mark_dirty();
                    }

                    // Perf checkbox - MIDI learnable
                    let old_perf = self.show_perf_hud;
                    let perf_label = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new("Perf").color(highlight)
                    } else {
                        egui::RichText::new("Perf")
                    };
                    let perf_response = ui.checkbox(&mut self.show_perf_hud, perf_label);
                    if self.midi_learn_armed && perf_response.clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::TogglePerfHud);
                        self.show_perf_hud = old_perf;
                    } else if self.show_perf_hud != old_perf {
                        self.mark_dirty();
                    }

                    // Debug checkbox - MIDI learnable
                    let old_debug = self.show_debug_hud;
                    let debug_label = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new("Debug").color(highlight)
                    } else {
                        egui::RichText::new("Debug")
                    };
                    let debug_response = ui.checkbox(&mut self.show_debug_hud, debug_label);
                    if self.midi_learn_armed && debug_response.clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleDebugHud);
                        self.show_debug_hud = old_debug;
                    } else if self.show_debug_hud != old_debug {
                        self.mark_dirty();
                    }
                });

                // === TUNING REFERENCE ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Tuning").size(11.0).color(egui::Color32::from_rgb(130, 130, 150)));
                    let selected_text = match self.tuning_reference {
                        TuningReference::Hz440 => "440 Hz",
                        TuningReference::Hz432 => "432 Hz",
                    };
                    egui::ComboBox::from_id_salt("tuning_reference")
                        .selected_text(selected_text)
                        .width(72.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(self.tuning_reference == TuningReference::Hz440, "440 Hz").clicked() {
                                self.tuning_reference = TuningReference::Hz440;
                                if let Some(ref mut p) = pipeline {
                                    p.set_tuning_reference(TuningReference::Hz440);
                                }
                                self.mark_dirty();
                            }
                            if ui.selectable_label(self.tuning_reference == TuningReference::Hz432, "432 Hz").clicked() {
                                self.tuning_reference = TuningReference::Hz432;
                                if let Some(ref mut p) = pipeline {
                                    p.set_tuning_reference(TuningReference::Hz432);
                                }
                                self.mark_dirty();
                            }
                        });
                });

                // === QUATERNION SYMMETRY SUBSECTION (Wheel is MIDI learnable) ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Quaternion Symmetry").size(10.0).color(egui::Color32::from_rgb(130, 130, 150)));

                    // Wheel checkbox - MIDI learnable
                    let old_wheel = self.show_qs_wheel;
                    let wheel_label = if self.midi_learn_armed {
                        let pulse = (ui.input(|i| i.time) * 4.0).sin() * 0.5 + 0.5;
                        let highlight = egui::Color32::from_rgba_unmultiplied(255, 200, 50, (150.0 + 80.0 * pulse as f32) as u8);
                        egui::RichText::new("Wheel").color(highlight)
                    } else {
                        egui::RichText::new("Wheel")
                    };
                    let wheel_response = ui.checkbox(&mut self.show_qs_wheel, wheel_label);
                    if self.midi_learn_armed && wheel_response.clicked() {
                        self.pending_midi_action = MidiAction::SetLearnTarget(ControlTarget::ToggleQsWheel);
                        self.show_qs_wheel = old_wheel;
                    } else if self.show_qs_wheel != old_wheel {
                        self.mark_dirty();
                    }

                    // Always show Hide #s and Dom (with placeholders when Wheel is off) to prevent layout jumping
                    let dim = egui::Color32::from_rgb(50, 50, 60);
                    if self.show_qs_wheel {
                        if ui.checkbox(&mut self.hide_qs_numbers, "Hide #s").changed() {
                            self.mark_dirty();
                        }
                        if ui.checkbox(&mut self.qs_dominant_only, "Dom").changed() {
                            self.mark_dirty();
                        }
                    } else {
                        ui.label(egui::RichText::new("☐ Hide #s").size(12.0).color(dim));
                        ui.label(egui::RichText::new("☐ Dom").size(12.0).color(dim));
                    }
                });

                // === Freeze + Scrub ===
                ui.horizontal(|ui| {
                    let freeze_text = if self.frozen { "FROZEN" } else { "Freeze" };
                    let freeze_color = if self.frozen {
                        egui::Color32::from_rgb(100, 200, 255)
                    } else {
                        egui::Color32::from_rgb(180, 180, 180)
                    };
                    if ui.add(egui::Button::new(
                        egui::RichText::new(freeze_text).color(freeze_color).size(11.0)
                    ).min_size(egui::vec2(60.0, 20.0))).clicked() {
                        self.frozen = !self.frozen;
                        if self.frozen {
                            self.scrub_position = 1.0;
                        }
                    }

                    if self.frozen {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Scrub:").size(10.0).color(egui::Color32::from_rgb(150, 150, 150)));
                        let slider = egui::Slider::new(&mut self.scrub_position, 0.0..=1.0)
                            .show_value(false)
                            .clamp_to_range(true);
                        ui.add(slider);
                    }
                });

                ui.add_space(4.0);
                ui.separator();

                // === Journey Phase Display (Hero's Journey State) - Fixed height vertical layout ===
                const STATE_SECTION_HEIGHT: f32 = 130.0;  // Fixed height for all conditional rows (phase, interior, torus, vortex, dodeca, topology)

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), STATE_SECTION_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        // Check for INERTIA state: sustained low energy AND low tension = no audio
                        let is_inertia = field_state.energy_vis < 0.08 && field_state.tension_slow < 0.05;

                        // Check for WAKING state: transitioning from INERTIA to FORMING
                        let is_waking = !is_inertia
                            && field_state.journey_phase == HeroAct::Birth
                            && field_state.energy_vis < 0.18;

                        // Map journey phase to user-friendly display names
                        let (phase_name, phase_color) = if is_inertia {
                            ("INERTIA", egui::Color32::from_rgb(200, 50, 60))
                        } else if is_waking {
                            ("WAKING", egui::Color32::from_rgb(220, 120, 80))
                        } else {
                            match field_state.journey_phase {
                                HeroAct::Birth => ("FORMING", egui::Color32::from_rgb(150, 180, 255)),
                                HeroAct::Lock => ("LOCKED", egui::Color32::from_rgb(100, 255, 150)),
                                HeroAct::Dive => ("INSIDE", egui::Color32::from_rgb(255, 200, 50)),
                                HeroAct::Afterglow => ("FADING", egui::Color32::from_rgb(180, 100, 180)),
                            }
                        };

                        // Row 1: Phase + time
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(phase_name)
                                .size(14.0)
                                .strong()
                                .color(phase_color));
                            let phase_time_str = if field_state.journey_phase_time < 60.0 {
                                format!("{:.0}s", field_state.journey_phase_time)
                            } else {
                                format!("{:.0}m", field_state.journey_phase_time / 60.0)
                            };
                            ui.label(egui::RichText::new(phase_time_str)
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 120, 140)));
                        });

                        // Row 2: Interior state + Symmetry
                        ui.horizontal(|ui| {
                            let (interior_name, interior_color) = match field_state.interior_state {
                                InteriorState::Outside => ("○", egui::Color32::from_rgb(100, 100, 120)),
                                InteriorState::Entering => ("◐", egui::Color32::from_rgb(255, 200, 100)),
                                InteriorState::Inside => ("●", egui::Color32::from_rgb(100, 255, 200)),
                                InteriorState::Exiting => ("◑", egui::Color32::from_rgb(200, 150, 100)),
                            };
                            ui.label(egui::RichText::new(format!("{} {}", interior_name, field_state.interior_state.name()))
                                .size(11.0)
                                .color(interior_color));

                            ui.add_space(8.0);

                            let (sym_text, sym_color) = if field_state.fold24_t > 0.01 {
                                let pct = (field_state.fold24_t * 100.0) as u8;
                                let color = if field_state.fold24_t > 0.5 {
                                    egui::Color32::from_rgb(255, 215, 100)
                                } else {
                                    egui::Color32::from_rgb(100, 255, 220)
                                };
                                (format!("24-fold {}%", pct), color)
                            } else {
                                (format!("{}-fold", field_state.symmetry), egui::Color32::from_rgb(100, 100, 120))
                            };
                            ui.label(egui::RichText::new(sym_text)
                                .size(10.0)
                                .color(sym_color));
                        });

                        // Row 3: Torus Knot - ALWAYS rendered (placeholder when inactive)
                        let tk_active = field_state.torus_knot_active || field_state.torus_knot_t > 0.01;
                        let (tk_text, tk_color) = if tk_active {
                            let pct = (field_state.torus_knot_t * 100.0) as u8;
                            (format!("◈ T({}:{}) {}% {}",
                                field_state.torus_knot_p,
                                field_state.torus_knot_q,
                                pct,
                                field_state.torus_knot_phase),
                             egui::Color32::from_rgb(255, 150, 255))
                        } else {
                            ("◈ T —".to_string(), egui::Color32::from_rgb(50, 50, 60))
                        };
                        ui.label(egui::RichText::new(tk_text).size(10.0).color(tk_color));

                        // Row 4: Vortex - ALWAYS rendered (placeholder when inactive)
                        let vortex_active = field_state.vortex_active || field_state.vortex_t > 0.01;
                        let (vortex_text, vortex_color) = if vortex_active {
                            let pct = (field_state.vortex_t * 100.0) as u8;
                            let dir_symbol = if field_state.vortex_direction == "Inward" { "⟲" } else { "⟳" };
                            (format!("{}Vortex G{} {}%", dir_symbol, field_state.vortex_gate, pct),
                             egui::Color32::from_rgb(100, 200, 255))
                        } else {
                            ("⟲Vortex —".to_string(), egui::Color32::from_rgb(50, 50, 60))
                        };
                        ui.label(egui::RichText::new(vortex_text).size(10.0).color(vortex_color));

                        // Row 5: Dodecahedral - ALWAYS rendered (placeholder when inactive)
                        let dodeca_relevant = field_state.topology_name == "Vortex" || field_state.dodeca_active || field_state.dodeca_t > 0.01;
                        let (dodeca_text, dodeca_color) = if dodeca_relevant {
                            let color = if field_state.dodeca_active || field_state.dodeca_t > 0.01 {
                                egui::Color32::from_rgb(220, 200, 255)
                            } else if field_state.dodeca_entry_ok {
                                egui::Color32::from_rgb(180, 160, 220)
                            } else {
                                egui::Color32::from_rgb(100, 80, 120)
                            };
                            let text = if field_state.dodeca_active || field_state.dodeca_t > 0.01 {
                                format!("◇Dodeca {:.0}% φ{:.0}%", field_state.dodeca_t * 100.0, field_state.dodeca_phi * 100.0)
                            } else if field_state.dodeca_cooldown > 0.0 {
                                format!("◇Dodeca cd:{:.1}s", field_state.dodeca_cooldown)
                            } else if field_state.dodeca_entry_ok {
                                "◇Dodeca READY".to_string()
                            } else {
                                format!("◇Dodeca H={:.2}", field_state.dodeca_entropy)
                            };
                            (text, color)
                        } else {
                            ("◇Dodeca —".to_string(), egui::Color32::from_rgb(50, 50, 60))
                        };
                        ui.label(egui::RichText::new(dodeca_text).size(10.0).color(dodeca_color));

                        // Row 6: Topology - ALWAYS rendered (shows current topology)
                        let topo_color = if field_state.topology_name != "Scaffold24" || field_state.topology_xfade_t > 0.01 {
                            egui::Color32::from_rgb(180, 180, 100)
                        } else {
                            egui::Color32::from_rgb(50, 50, 60)
                        };
                        let topo_text = if field_state.topology_xfade_t > 0.01 && field_state.topology_xfade_t < 0.99 {
                            format!("Topo:{} xf={:.0}%", field_state.topology_name, field_state.topology_xfade_t * 100.0)
                        } else {
                            format!("Topo:{}", field_state.topology_name)
                        };
                        ui.label(egui::RichText::new(topo_text).size(9.0).color(topo_color));
                    });

                // === Torus Knot Controls ===
                // All rows ALWAYS rendered to prevent layout jumping
                let torus_active = field_state.torus_knot_active;
                let dim = egui::Color32::from_rgb(50, 50, 60);

                ui.separator();

                // Row 1: Header - always shown
                let header_color = if torus_active {
                    egui::Color32::from_rgb(220, 220, 230)
                } else {
                    dim
                };
                ui.label(egui::RichText::new("Torus Knot").size(11.0).strong().color(header_color));

                // Row 2: Family + Paint - always shown
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(140.0, 14.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let (family_name, family_color) = if torus_active {
                                let name = if field_state.torus_canvas_name.is_empty() { "—" } else { field_state.torus_canvas_name };
                                let color = if field_state.torus_canvas_name.is_empty() {
                                    egui::Color32::from_rgb(80, 80, 80)
                                } else {
                                    egui::Color32::from_rgb(255, 200, 100)
                                };
                                (name, color)
                            } else {
                                ("—", dim)
                            };
                            ui.label(egui::RichText::new(format!("Family:{}", family_name)).size(10.0).color(family_color));
                        }
                    );
                    let paint_color = if torus_active { egui::Color32::from_rgb(100, 200, 255) } else { dim };
                    let paint_name = if torus_active { &field_state.torus_paint_name } else { "—" };
                    ui.label(egui::RichText::new(format!("Paint:{}", paint_name)).size(10.0).color(paint_color));
                });

                // Row 3: Stats - always shown
                ui.horizontal(|ui| {
                    let (stats_text, stats_color) = if torus_active {
                        let wi_str = format!("{:>3.0}°", field_state.torus_winding_interval_eff);
                        (format!("T({:>2}:{:<2}) | WI={} | Beads={:>3} | Links={}",
                            field_state.torus_knot_p,
                            field_state.torus_knot_q,
                            wi_str,
                            field_state.torus_bead_count,
                            field_state.torus_link_count),
                         egui::Color32::from_rgb(150, 150, 150))
                    } else {
                        ("T( —: —) | WI=  —° | Beads=  — | Links=—".to_string(), dim)
                    };
                    ui.label(egui::RichText::new(stats_text).size(9.0).color(stats_color).monospace());
                });

                // Row 4: Tuning status - always shown
                ui.horizontal(|ui| {
                    let (status_text, status_color) = if torus_active {
                        let spice_color = if !field_state.torus_spice_eligible {
                            egui::Color32::from_rgb(80, 80, 80)
                        } else if field_state.torus_spice_active {
                            egui::Color32::from_rgb(255, 180, 80)
                        } else {
                            egui::Color32::from_rgb(120, 120, 120)
                        };
                        let elig_mark = if field_state.torus_spice_eligible { "✓" } else { "✗" };
                        let spice_str = format!("Sp:{}→{}{}", field_state.torus_spice_level, field_state.torus_spice_level_target, elig_mark);
                        let phrase_bias_str = match field_state.torus_phrase_bias { -1 => "↓", 1 => "↑", _ => "•" };
                        let phrase_str = format!("Ph:{:>3.0}%{}", field_state.torus_phrase_env * 100.0, phrase_bias_str);
                        let orn_name = match field_state.torus_ornament_id { 1 => "Fil ", 2 => "Ntch", 3 => "Halo", _ => " —  " };
                        let wi_str = if field_state.torus_wi_accent_active {
                            format!("WI▸{}", field_state.torus_wi_accent_step)
                        } else {
                            "    ".to_string()
                        };
                        (format!("{} | {} | O:{} | {}", spice_str, phrase_str, orn_name, wi_str), spice_color)
                    } else {
                        ("Sp:—→—✗ | Ph:  —%• | O: —   |     ".to_string(), dim)
                    };
                    ui.label(egui::RichText::new(status_text).size(8.0).color(status_color).monospace());
                });

                // Row 5: Debug line - always shown
                ui.horizontal(|ui| {
                    let (debug_text, debug_color) = if torus_active {
                        let gate_str = if field_state.torus_spice_active { "G" } else { "g" };
                        let coh_str = if field_state.torus_spice_coherent { "C" } else { "c" };
                        let elig_str = if field_state.torus_spice_eligible { "E" } else { "e" };
                        let debug_str = format!(
                            "{}{}{}|L:{:>2} pM:{:>4.2} p2:{:>4.2} H:{:>4.2} t:{:>4.1}s",
                            gate_str, coh_str, elig_str,
                            field_state.torus_spice_prime_limit,
                            field_state.torus_spice_p_max,
                            field_state.torus_spice_p2,
                            field_state.torus_spice_entropy,
                            field_state.torus_spice_stable_time
                        );
                        let color = if !field_state.torus_spice_eligible {
                            egui::Color32::from_rgb(100, 100, 100)
                        } else if field_state.torus_spice_active {
                            egui::Color32::from_rgb(150, 255, 150)
                        } else if field_state.torus_spice_coherent {
                            egui::Color32::from_rgb(255, 255, 150)
                        } else {
                            egui::Color32::from_rgb(180, 100, 100)
                        };
                        (debug_str, color)
                    } else {
                        ("gce|L: — pM:   — p2:   — H:   — t:   —s".to_string(), dim)
                    };
                    ui.label(egui::RichText::new(debug_text).size(7.0).color(debug_color).monospace());
                });

                // Row 6: Warning line - always shown (placeholder when no warning)
                let (warn_text, warn_color) = if torus_active && field_state.torus_is_high_complexity {
                    ("High complexity - may impact framerate".to_string(), egui::Color32::from_rgb(255, 180, 50))
                } else {
                    (" ".to_string(), dim)  // Single space placeholder to maintain row height
                };
                ui.label(egui::RichText::new(warn_text).size(9.0).color(warn_color));

                // Notes meter always visible (independent of shader's Chroma Wheel toggle)
                // === Harmonic Metrics Section - Fixed height to prevent menu jumping ===
                const METRICS_SECTION_HEIGHT: f32 = 220.0;  // Fixed height for badges, meters, and chroma wheel

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), METRICS_SECTION_HEIGHT),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                ui.add_space(8.0);

                // === Compute REG-aligned metrics from raw chroma ===
                    // Use raw_chroma (not mean-subtracted) for variance/balance calculation
                    let raw_chroma = &field_state.raw_chroma;
                    let raw_sum: f32 = raw_chroma.iter().sum();
                    let raw_mean = raw_sum / 12.0;

                    // Variance from mean (low = uniform field = high balance)
                    let variance: f32 = raw_chroma.iter()
                        .map(|&c| (c - raw_mean).powi(2))
                        .sum::<f32>() / 12.0;

                    // Tension = sum of positive deviations (the lobes)
                    let tension: f32 = raw_chroma.iter()
                        .map(|&c| (c - raw_mean).max(0.0))
                        .sum();

                    // Normalize: max variance when one bin=1, others=0 is ~0.076
                    // Max tension when one bin=1 is ~0.917
                    // When no audio (raw_sum near zero), show 0% for both
                    let (balance_pct, tension_pct) = if raw_sum < 0.01 {
                        (0, 0)
                    } else {
                        (
                            ((1.0 - (variance / 0.08).min(1.0)) * 100.0) as i32,
                            ((tension / 1.0).min(1.0) * 100.0) as i32
                        )
                    };

                    // === 1. Harmonic Label Badge ===
                    // Uses hysteresis-based labeling from KeyDetector:
                    // - Stable: confidence >= 0.25 (clear dominant)
                    // - Rotating: 0.12 <= confidence < 0.25 (multi-lobe harmony)
                    // - Ambiguous: confidence < 0.12 (no clear center)
                    // - Atonal: noise/silence
                    let (badge, badge_color) = match field_state.harmonic_label {
                        HarmonicLabel::Stable => ("Stable Harmony", egui::Color32::from_rgb(100, 255, 100)), // Green
                        HarmonicLabel::Rotating => ("Rotating Harmony", egui::Color32::from_rgb(100, 200, 255)), // Cyan - multi-lobe
                        HarmonicLabel::Ambiguous => ("Ambiguous", egui::Color32::from_rgb(255, 200, 50)), // Yellow
                        HarmonicLabel::Atonal => ("Atonal Field", egui::Color32::from_rgb(180, 180, 200)), // Gray-blue
                    };

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(badge)
                            .size(12.0)
                            .color(badge_color)
                            .strong())
                            .on_hover_text("Stable: Clear dominant pitch (conf >= 0.25)\nRotating: Multi-lobe harmony, rotating center\nAmbiguous: No clear harmonic center\nAtonal: Noise or silence\n\nDominant uses hysteresis (0.6s hold, 25% boost required)");
                    });

                    ui.add_space(4.0);

                    // === 2. Dominant Pitch Display ===
                    ui.horizontal(|ui| {
                        let dom_color = if field_state.is_tonal {
                            egui::Color32::from_rgb(100, 255, 150)
                        } else {
                            egui::Color32::from_rgb(120, 120, 140)
                        };
                        ui.label(egui::RichText::new(format!("Dominant: {}", field_state.dominant_name))
                            .size(16.0)
                            .color(dom_color)
                            .strong());
                    });

                    ui.add_space(4.0);

                    // === 3. Peak Separation Meter ===
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Dominance:").size(10.0).color(egui::Color32::from_rgb(150, 150, 150)));
                        let (sep_rect, _) = ui.allocate_exact_size(egui::vec2(80.0, 8.0), egui::Sense::hover());
                        let painter = ui.painter_at(sep_rect);
                        painter.rect_filled(sep_rect, 2.0, egui::Color32::from_rgb(30, 30, 40));

                        // Threshold marker at 20%
                        let threshold_x = sep_rect.min.x + sep_rect.width() * 0.20;
                        painter.line_segment(
                            [egui::pos2(threshold_x, sep_rect.min.y), egui::pos2(threshold_x, sep_rect.max.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80)),
                        );

                        // Fill based on separation
                        let sep_width = sep_rect.width() * field_state.tonal_confidence.min(1.0);
                        let sep_color = if field_state.tonal_confidence > 0.5 {
                            egui::Color32::from_rgb(100, 255, 100)
                        } else if field_state.tonal_confidence > 0.2 {
                            egui::Color32::from_rgb(255, 200, 50)
                        } else {
                            egui::Color32::from_rgb(255, 100, 100)
                        };
                        painter.rect_filled(
                            egui::Rect::from_min_size(sep_rect.min, egui::vec2(sep_width, sep_rect.height())),
                            2.0,
                            sep_color,
                        );

                        ui.label(egui::RichText::new(format!("{:.0}%", field_state.tonal_confidence * 100.0))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(150, 150, 150)));
                    });

                    ui.add_space(4.0);

                    // === 4. Harmonic Balance vs Tension (REG-aligned) ===
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Balance: {}%", balance_pct))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(100, 200, 255)));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(format!("Tension: {}%", tension_pct))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(255, 150, 100)));
                    });

                    ui.add_space(4.0);

                    // === 5. Audio Energy meter (perceptual) ===
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Energy:").size(10.0));
                        let (energy_rect, _) = ui.allocate_exact_size(egui::vec2(50.0, 8.0), egui::Sense::hover());
                        let painter = ui.painter_at(energy_rect);
                        painter.rect_filled(energy_rect, 2.0, egui::Color32::from_rgb(30, 30, 40));
                        // Use perceptual energy (remapped to 0-1 visible range)
                        let energy_width = energy_rect.width() * field_state.energy_vis.min(1.0);
                        let energy_color = if field_state.energy_vis > 0.6 {
                            egui::Color32::from_rgb(100, 200, 255)  // Bright blue for high energy
                        } else if field_state.energy_vis > 0.3 {
                            egui::Color32::from_rgb(150, 180, 200)  // Medium blue
                        } else {
                            egui::Color32::from_rgb(100, 100, 120)  // Dim for low energy
                        };
                        painter.rect_filled(
                            egui::Rect::from_min_size(energy_rect.min, egui::vec2(energy_width, energy_rect.height())),
                            2.0,
                            energy_color,
                        );
                    });

                    // === 6. Stability Meter ===
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Stability:").size(10.0));
                        let (stab_rect, stab_response) = ui.allocate_exact_size(egui::vec2(50.0, 8.0), egui::Sense::hover());
                        let painter = ui.painter_at(stab_rect);
                        painter.rect_filled(stab_rect, 2.0, egui::Color32::from_rgb(30, 30, 40));

                        // Threshold marker at 60% (visual reference, not used for labeling)
                        let threshold_x = stab_rect.min.x + stab_rect.width() * 0.60;
                        painter.line_segment(
                            [egui::pos2(threshold_x, stab_rect.min.y), egui::pos2(threshold_x, stab_rect.max.y)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80)),
                        );

                        let stab_width = stab_rect.width() * field_state.stability.min(1.0);
                        let stab_color = if field_state.stability > 0.6 {
                            egui::Color32::from_rgb(100, 255, 100)  // Green - stable
                        } else if field_state.stability > 0.3 {
                            egui::Color32::from_rgb(255, 200, 50)  // Yellow - moderate
                        } else {
                            egui::Color32::from_rgb(255, 100, 100)  // Red - unstable
                        };
                        painter.rect_filled(
                            egui::Rect::from_min_size(stab_rect.min, egui::vec2(stab_width, stab_rect.height())),
                            2.0,
                            stab_color,
                        );

                        // Tooltip explaining stability
                        stab_response.on_hover_text("Temporal stability of chroma frame-to-frame.\nHigh = consistent pitch content\nLow = rapidly changing\n\nNote: Labels now use confidence thresholds, not stability.");
                    });

                    ui.add_space(8.0);

                    // === Chroma Wheel (12 bars) ===
                    const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
                    let bar_width = 22.0;
                    let bar_max_height = 50.0;
                    let spacing = 2.0;

                    ui.horizontal(|ui| {
                        for i in 0..12 {
                            let chroma_value = field_state.chroma[i];
                            // Only highlight dominant when is_tonal (avoid random highlight during noise)
                            let is_dominant = field_state.is_tonal && i == field_state.dominant_pitch;

                            ui.vertical(|ui| {
                                // Bar
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(bar_width, bar_max_height),
                                    egui::Sense::hover(),
                                );
                                let painter = ui.painter_at(rect);

                                // Background
                                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(30, 30, 40));

                                // Filled bar
                                let filled_height = rect.height() * chroma_value.clamp(0.0, 1.0);
                                let filled_rect = egui::Rect::from_min_size(
                                    egui::pos2(rect.min.x, rect.max.y - filled_height),
                                    egui::vec2(rect.width(), filled_height),
                                );

                                // Color based on state
                                let bar_color = if is_dominant {
                                    egui::Color32::from_rgb(100, 255, 150) // Green for dominant
                                } else if chroma_value > 0.3 {
                                    egui::Color32::from_rgb(180, 180, 100) // Yellow for strong
                                } else if field_state.is_tonal {
                                    egui::Color32::from_rgb(100, 150, 200) // Blue when tonal
                                } else {
                                    egui::Color32::from_rgb(80, 80, 100) // Dim when noise
                                };
                                painter.rect_filled(filled_rect, 2.0, bar_color);

                                // Dominant highlight (only when tonal)
                                if is_dominant {
                                    painter.rect_stroke(
                                        rect.expand(1.0),
                                        2.0,
                                        egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 255, 150)),
                                    );
                                }

                                // Note name
                                let note_color = if is_dominant {
                                    egui::Color32::from_rgb(100, 255, 150)
                                } else {
                                    egui::Color32::from_rgb(150, 150, 150)
                                };
                                ui.label(egui::RichText::new(NOTE_NAMES[i]).size(9.0).color(note_color));
                            });

                            if i < 11 {
                                ui.add_space(spacing);
                            }
                        }
                    });
                });  // End of METRICS_SECTION_HEIGHT fixed-height container
            });

        // Track expanded state
        self.harmonic_expanded = header.fully_open();
    }

    /// Performance settings section (quality, effects, background)
    fn show_performance_section(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new(egui::RichText::new("Performance").strong().size(14.0))
            .default_open(self.performance_expanded)
            .show(ui, |ui| {
                ui.separator();

                // === RENDER QUALITY ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Render Quality")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 150, 160)));
                });

                ui.horizontal(|ui| {
                    for (i, (name, desc)) in [
                        ("Low", "Fastest, lower detail"),
                        ("Medium", "Balanced"),
                        ("High", "Full resolution"),
                        ("Ultra", "1.5x supersampling for big screens"),
                    ].iter().enumerate() {
                        let is_selected = self.quality_tier == i as u8;
                        let color = if is_selected {
                            egui::Color32::from_rgb(100, 200, 255)
                        } else {
                            egui::Color32::from_rgb(120, 120, 120)
                        };
                        if ui.add(egui::Button::new(
                            egui::RichText::new(*name).size(11.0).color(color)
                        ).min_size(egui::vec2(55.0, 22.0))).on_hover_text(*desc).clicked() {
                            self.quality_tier = i as u8;
                            self.mark_dirty();
                        }
                    }
                });

                ui.add_space(8.0);

                // === VISUAL EFFECTS ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Visual Effects")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 150, 160)));

                    // AAA toggle
                    let aaa_color = if self.ring_style_aaa {
                        egui::Color32::from_rgb(255, 200, 100)
                    } else {
                        egui::Color32::from_rgb(100, 100, 100)
                    };
                    if ui.add(egui::Button::new(
                        egui::RichText::new("AAA").size(10.0).color(aaa_color)
                    ).min_size(egui::vec2(36.0, 18.0))).on_hover_text("Enable premium visual effects").clicked() {
                        self.ring_style_aaa = !self.ring_style_aaa;
                    }
                });

                // AAA sub-options - ALWAYS rendered to prevent layout jumping
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let dim = egui::Color32::from_rgb(50, 50, 60);
                    if self.ring_style_aaa {
                        ui.checkbox(&mut self.ring_flow_lines, "Flow Lines");
                        ui.checkbox(&mut self.ring_sparkle, "Sparkle");
                        ui.checkbox(&mut self.ring_fog, "Fog");
                    } else {
                        // Placeholders with same visual space
                        ui.label(egui::RichText::new("☐ Flow Lines").size(12.0).color(dim));
                        ui.label(egui::RichText::new("☐ Sparkle").size(12.0).color(dim));
                        ui.label(egui::RichText::new("☐ Fog").size(12.0).color(dim));
                    }
                });

                ui.add_space(8.0);

                // === BACKGROUND QUALITY ===
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Background Quality")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 150, 160)));

                    // Auto checkbox
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let was_auto = self.bg_quality_auto;
                        ui.checkbox(&mut self.bg_quality_auto, "Auto");
                        if self.bg_quality_auto != was_auto {
                            self.mark_dirty();
                        }
                    });
                });

                ui.horizontal(|ui| {
                    for (i, (name, desc)) in [
                        ("Low", "Minimal background processing"),
                        ("Medium", "Balanced background"),
                        ("High", "Full background detail"),
                    ].iter().enumerate() {
                        let is_selected = self.bg_quality == i as u8;
                        let color = if self.bg_quality_auto {
                            egui::Color32::from_rgb(70, 70, 70)
                        } else if is_selected {
                            egui::Color32::from_rgb(100, 200, 255)
                        } else {
                            egui::Color32::from_rgb(120, 120, 120)
                        };
                        if ui.add(egui::Button::new(
                            egui::RichText::new(*name).size(11.0).color(color)
                        ).min_size(egui::vec2(55.0, 22.0))).on_hover_text(*desc).clicked() {
                            self.bg_quality = i as u8;
                            self.bg_quality_auto = false;
                            self.mark_dirty();
                        }
                    }
                });

                if self.bg_quality_auto {
                    ui.label(egui::RichText::new("Background quality adjusts automatically based on framerate")
                        .size(9.0)
                        .color(egui::Color32::from_rgb(100, 100, 110)));
                }
            });

        // Track expanded state
        self.performance_expanded = header.fully_open();
    }

    /// Show the Account section for Compeller.ai login
    pub fn show_account_section(&mut self, ui: &mut egui::Ui, auth: &AuthManager) -> AuthAction {
        let mut action = AuthAction::None;

        let auth_state = auth.get_state();
        let is_authenticated = auth.is_authenticated();
        let is_logging_in = auth.is_logging_in();

        // Section header with status indicator
        let header_text = if is_authenticated {
            if let Some(user) = auth.get_user() {
                format!("Account ({})", user.username)
            } else {
                "Account".to_string()
            }
        } else {
            "Account".to_string()
        };

        // A sign-in tile link was clicked: force the header open this frame
        // (default_open only applies on first render; egui remembers the
        // collapsed state otherwise) and scroll the menu down to it.
        let jump = self.jump_to_account;
        let mut collapsing = egui::CollapsingHeader::new(
            egui::RichText::new(header_text).strong().size(14.0)
        )
        .default_open(self.account_expanded);
        if jump {
            collapsing = collapsing.open(Some(true));
        }
        let header = collapsing
        .show(ui, |ui| {
            ui.add_space(4.0);

            if is_authenticated {
                // === Logged in state ===
                if let Some(user) = auth.get_user() {
                    let ent = auth.get_entitlements();

                    // User info
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgb(100, 200, 100)));
                        ui.label(egui::RichText::new(&user.username)
                            .color(egui::Color32::WHITE));
                    });

                    // Plan badge. If no plan metadata is available, show no label
                    // instead of incorrectly calling the account FREE.
                    if !ent.plan_code.is_empty() {
                        let plan_color = match ent.plan_code.as_str() {
                            "PRO" => egui::Color32::from_rgb(255, 200, 50),
                            "ENTERPRISE" => egui::Color32::from_rgb(200, 150, 255),
                            "STARTER" => egui::Color32::from_rgb(100, 200, 255),
                            _ => egui::Color32::from_rgb(150, 150, 150),
                        };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&ent.plan_code)
                                .size(10.0)
                                .color(plan_color)
                                .strong());
                            if ent.plan_code == "FREE" && ent.watermark {
                                ui.label(egui::RichText::new("(free tier)")
                                    .size(10.0)
                                    .color(egui::Color32::from_rgb(140, 140, 140)));
                            }
                        });
                    }

                    ui.add_space(8.0);

                    // Logout button
                    if ui.button("Logout").clicked() {
                        action = AuthAction::Logout;
                    }
                }
            } else {
                // === Not logged in state ===
                if is_logging_in {
                    // Show loading spinner
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new("Signing in...")
                            .color(egui::Color32::GRAY)
                            .italics());
                    });
                } else {
                    // Show error if any
                    if let AuthState::Error(ref msg) = auth_state {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("●")
                                .color(egui::Color32::from_rgb(255, 100, 100)));
                            ui.label(egui::RichText::new(msg)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 150, 150)));
                        });
                        ui.add_space(4.0);
                    }

                    // Login form
                    ui.label(egui::RichText::new("Username / Email")
                        .size(11.0)
                        .color(egui::Color32::GRAY));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.login_username)
                            .desired_width(180.0)
                            .hint_text("email@example.com")
                    );

                    ui.add_space(4.0);

                    ui.label(egui::RichText::new("Password")
                        .size(11.0)
                        .color(egui::Color32::GRAY));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.login_password)
                            .password(true)
                            .desired_width(180.0)
                            .hint_text("••••••••")
                    );

                    ui.add_space(8.0);

                    // Login button + Enter key submit
                    let can_login = !self.login_username.is_empty() && !self.login_password.is_empty();
                    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add_enabled(can_login, egui::Button::new("Sign In")).clicked()
                        || (can_login && enter_pressed)
                    {
                        action = AuthAction::Login {
                            username: self.login_username.clone(),
                            password: self.login_password.clone(),
                        };
                        // Clear password after submission for security
                        self.login_password.clear();
                    }

                    ui.add_space(6.0);

                    // Browser sign-in (Google or password on the web): opens
                    // the system browser; REACT polls until the web session
                    // authorizes the one-time code.
                    if ui.add(egui::Button::new(
                        egui::RichText::new("\u{1F310} Sign in with Google")
                            .color(egui::Color32::from_rgb(230, 230, 230))
                    )).clicked() {
                        action = AuthAction::GoogleSignIn;
                    }
                    ui.label(egui::RichText::new("opens your browser to sign in")
                        .size(10.0)
                        .color(egui::Color32::from_rgb(140, 140, 140)));

                    ui.add_space(8.0);

                    // Create account button (more visible than a small link)
                    if ui.add(egui::Button::new(
                        egui::RichText::new("Create Free Account")
                            .color(egui::Color32::from_rgb(100, 180, 255))
                    )).clicked() {
                        #[cfg(target_os = "macos")]
                        {
                            let url = format!("{}/signup", config::oauth_base());
                            let _ = std::process::Command::new("open")
                                .arg(&url)
                                .spawn();
                        }
                    }
                    ui.label(egui::RichText::new("Free forever \u{2014} create visuals, stream, record")
                        .size(10.0)
                        .color(egui::Color32::from_rgb(140, 140, 140)));
                }
            }
        });

        // Track expanded state
        self.account_expanded = header.fully_open();
        if jump {
            // Scroll the menu's ScrollArea down to the (now open) section.
            header
                .header_response
                .scroll_to_me(Some(egui::Align::Min));
            self.jump_to_account = false;
        }

        action
    }

    /// Show the YouTube Live section
    pub fn show_youtube_section(
        &mut self,
        ui: &mut egui::Ui,
        youtube_auth: Option<&crate::youtube::auth::YouTubeAuth>,
        youtube_eligibility: Option<&crate::youtube::eligibility::EligibilityCache>,
        youtube_session: Option<&crate::youtube::session::LiveSession>,
        is_authenticated: bool,
        has_youtube_entitlement: bool,
    ) -> YouTubeAction {
        use crate::youtube::auth::YouTubeAuthState;
        use crate::youtube::eligibility::EligibilityState;
        use crate::youtube::session::SessionState;

        let mut action = YouTubeAction::None;

        // Get auth state if available
        let auth_state = youtube_auth.map(|a| a.get_state()).unwrap_or(YouTubeAuthState::LoggedOut);
        let is_connected = youtube_auth.map(|a| a.is_ready()).unwrap_or(false);
        let eligibility_state = youtube_eligibility.map(|e| e.get_state()).unwrap_or(EligibilityState::Unknown);

        // Section header with status
        let header_text = if is_connected {
            if let Some(channel) = youtube_auth.and_then(|a| a.get_channel_info()) {
                format!("YouTube Live ({})", channel.channel_name)
            } else {
                "YouTube Live".to_string()
            }
        } else {
            "YouTube Live".to_string()
        };

        let header = egui::CollapsingHeader::new(
            egui::RichText::new(header_text).strong().size(14.0)
        )
        .default_open(self.youtube_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Show login required message if not authenticated
            if !is_authenticated {
                self.show_login_required_message(ui);
                return;
            }

            // Show entitlement message if authenticated but no YouTube entitlement
            if !has_youtube_entitlement {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("⭐")
                        .size(24.0)
                        .color(egui::Color32::from_rgb(255, 200, 50)));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Upgrade your plan")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(180, 180, 180)));
                    ui.label(egui::RichText::new("to stream to YouTube Live")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(120, 120, 120)));
                    ui.add_space(8.0);
                });
                return;
            }

            // YouTube not connected
            if !is_connected {
                match auth_state {
                    YouTubeAuthState::LoggedOut => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("●")
                                .color(egui::Color32::GRAY));
                            ui.label(egui::RichText::new("Not connected")
                                .color(egui::Color32::GRAY));
                        });

                        ui.add_space(4.0);

                        if ui.button("Connect YouTube").clicked() {
                            action = YouTubeAction::ConnectYouTube;
                        }
                    }
                    YouTubeAuthState::Authenticating => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Connecting...")
                                .color(egui::Color32::GRAY)
                                .italics());
                        });

                        ui.add_space(4.0);

                        ui.label(egui::RichText::new("Complete sign-in in your browser")
                            .size(10.0)
                            .color(egui::Color32::GRAY));
                    }
                    YouTubeAuthState::Error(ref msg) => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("●")
                                .color(egui::Color32::from_rgb(255, 100, 100)));
                            ui.label(egui::RichText::new(msg)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 150, 150)));
                        });

                        ui.add_space(4.0);

                        if ui.button("Retry Connect").clicked() {
                            action = YouTubeAction::ConnectYouTube;
                        }
                    }
                    YouTubeAuthState::Ready => {
                        // Shouldn't reach here (is_connected would be true)
                    }
                }
            } else {
            // YouTube is connected - show eligibility
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●")
                    .color(egui::Color32::from_rgb(255, 0, 0))); // YouTube red
                if let Some(channel) = youtube_auth.and_then(|a| a.get_channel_info()) {
                    ui.label(egui::RichText::new(&channel.channel_name)
                        .color(egui::Color32::WHITE));
                }
            });

            // Eligibility status
            match eligibility_state {
                EligibilityState::Unknown | EligibilityState::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new("Checking eligibility...")
                            .size(10.0)
                            .color(egui::Color32::GRAY));
                    });
                }
                EligibilityState::Ineligible(ref reason) => {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("⚠")
                            .color(egui::Color32::from_rgb(255, 200, 50)));
                        ui.label(egui::RichText::new(reason)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(255, 200, 50)));
                    });
                }
                EligibilityState::Eligible => {
                    // Show session status
                    if let Some(session) = youtube_session {
                        let state = session.get_state();

                        ui.add_space(8.0);

                        // State indicator with detailed status
                        let step = session.get_step();
                        let (color, text): (egui::Color32, String) = match &state {
                            SessionState::Idle => (egui::Color32::GRAY, "Ready to go live".to_string()),
                            SessionState::Creating => {
                                let step_text = match step {
                                    crate::youtube::session::SessionStep::CreateBroadcast => "Creating broadcast...",
                                    crate::youtube::session::SessionStep::CreateStream => "Creating stream...",
                                    crate::youtube::session::SessionStep::BindBroadcastToStream => "Binding stream...",
                                    crate::youtube::session::SessionStep::FetchIngestionInfo => "Getting stream URL...",
                                    crate::youtube::session::SessionStep::Complete => "Setup complete...",
                                };
                                (egui::Color32::from_rgb(100, 150, 255), step_text.to_string())
                            },
                            SessionState::Bound => (egui::Color32::from_rgb(100, 150, 255), "Starting encoder...".to_string()),
                            SessionState::Connecting => (egui::Color32::from_rgb(100, 150, 255), "Connecting to YouTube...".to_string()),
                            SessionState::Pushing => (egui::Color32::from_rgb(255, 200, 50), "Streaming, waiting for YouTube...".to_string()),
                            SessionState::Testing => (egui::Color32::from_rgb(255, 200, 50), "Preview mode - going live...".to_string()),
                            SessionState::Live => (egui::Color32::from_rgb(255, 0, 0), "LIVE".to_string()),
                            SessionState::Ending => (egui::Color32::GRAY, "Stopping stream...".to_string()),
                            SessionState::Ended => (egui::Color32::GRAY, "Stream ended".to_string()),
                            SessionState::Failed(ref msg) => (egui::Color32::from_rgb(255, 100, 100), msg.clone()),
                        };

                        ui.horizontal(|ui| {
                            // Pulsing indicator for Live state
                            if matches!(state, SessionState::Live) {
                                let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.5 + 0.5;
                                let alpha = (200.0 + 55.0 * pulse as f32) as u8;
                                ui.label(egui::RichText::new("●")
                                    .color(egui::Color32::from_rgba_unmultiplied(255, 0, 0, alpha)));
                            } else if matches!(state, SessionState::Creating | SessionState::Bound | SessionState::Connecting | SessionState::Pushing | SessionState::Testing | SessionState::Ending) {
                                ui.spinner();
                            } else {
                                ui.label(egui::RichText::new("●").color(color));
                            }
                            ui.label(egui::RichText::new(text).color(color));
                        });

                        // Session actions
                        ui.add_space(8.0);

                        match &state {
                            SessionState::Idle => {
                                // Stream title input
                                ui.horizontal(|ui| {
                                    ui.label("Title:");
                                    ui.add(egui::TextEdit::singleline(&mut self.youtube_stream_title)
                                        .hint_text("My Stream")
                                        .desired_width(120.0));
                                });
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(
                                    if self.youtube_stream_title.is_empty() {
                                        "REACT (Live) by Compeller.ai".to_string()
                                    } else {
                                        format!("{} - REACT (Live) by Compeller.ai", self.youtube_stream_title)
                                    })
                                    .size(10.0)
                                    .color(egui::Color32::GRAY));
                                ui.add_space(4.0);

                                // Show actual resolution from output settings
                                let ndi_res = self.ndi_resolution;
                                let (w, h) = ndi_res.dimensions();
                                let res_label = if h >= 2160 { "4K" } else { "HD" };
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Resolution:").size(11.0));
                                    ui.label(egui::RichText::new(format!("{} ({}x{})", res_label, w, h))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(100, 200, 255)));
                                    ui.label(egui::RichText::new("← from Output settings")
                                        .size(9.0)
                                        .color(egui::Color32::DARK_GRAY));
                                });

                                // Resolution selector (all 60fps)
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Quality:").size(11.0));
                                    for quality in [YouTubeQuality::Hd60, YouTubeQuality::Qhd60, YouTubeQuality::Uhd60] {
                                        let selected = self.youtube_quality == quality;
                                        let btn = egui::Button::new(egui::RichText::new(quality.label()).size(10.0))
                                            .fill(if selected { egui::Color32::from_rgb(80, 80, 80) } else { egui::Color32::from_rgb(50, 50, 50) });
                                        if ui.add(btn).clicked() {
                                            self.youtube_quality = quality;
                                        }
                                    }
                                });

                                // Privacy selector
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Privacy:").size(11.0));
                                    for privacy in [YouTubePrivacy::Public, YouTubePrivacy::Unlisted, YouTubePrivacy::Private] {
                                        let selected = self.youtube_privacy == privacy;
                                        let btn = egui::Button::new(egui::RichText::new(privacy.label()).size(10.0))
                                            .fill(if selected { egui::Color32::from_rgb(80, 80, 80) } else { egui::Color32::from_rgb(50, 50, 50) });
                                        if ui.add(btn).clicked() {
                                            self.youtube_privacy = privacy;
                                        }
                                    }
                                });

                                // Audio gain slider
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Audio Gain:").size(11.0).color(egui::Color32::GRAY));
                                    let response = ui.add(
                                        egui::Slider::new(&mut self.youtube_audio_gain, 0.1..=16.0)
                                            .logarithmic(true)
                                            .show_value(true)
                                            .custom_formatter(|v, _| format!("{:.1}x", v))
                                    );
                                    if response.changed() {
                                        self.pending_gain_action = OutputAction::SetYouTubeGain(self.youtube_audio_gain);
                                    }
                                });

                                ui.add_space(8.0);

                                let go_live_text = egui::RichText::new("Go Live")
                                    .color(egui::Color32::WHITE);

                                if ui.add(egui::Button::new(go_live_text)
                                    .fill(egui::Color32::from_rgb(200, 50, 50)))
                                    .clicked()
                                {
                                    if !session.is_go_live_latched() {
                                        let full_title = if self.youtube_stream_title.is_empty() {
                                            "REACT (Live) by Compeller.ai".to_string()
                                        } else {
                                            format!("{} - REACT (Live) by Compeller.ai", self.youtube_stream_title)
                                        };
                                        let config = YouTubeStreamConfig {
                                            quality: self.youtube_quality,
                                            privacy: self.youtube_privacy,
                                        };
                                        action = YouTubeAction::GoLive { title: full_title, config };
                                    }
                                }
                            }
                            SessionState::Creating | SessionState::Bound | SessionState::Connecting | SessionState::Pushing | SessionState::Testing | SessionState::Live => {
                                // Show stream title
                                let title = session.get_title();
                                ui.label(egui::RichText::new(&title)
                                    .size(11.0)
                                    .color(egui::Color32::WHITE));
                                ui.add_space(2.0);

                                // Show stream info header (resolution, bitrate, privacy)
                                let resolution = session.get_resolution();
                                let frame_rate = session.get_frame_rate();
                                let bitrate = session.get_bitrate_kbps();
                                let privacy = session.get_privacy();
                                let privacy_icon = match privacy.as_str() {
                                    "public" => "🌐",
                                    "unlisted" => "🔗",
                                    "private" => "🔒",
                                    _ => "📺",
                                };

                                ui.horizontal(|ui| {
                                    // Resolution badge
                                    let res_label = if resolution == "2160p" { "4K" } else { "HD" };
                                    let fps_label = frame_rate.replace("fps", "");
                                    ui.label(egui::RichText::new(format!("{} {}fps", res_label, fps_label))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(100, 200, 255)));

                                    ui.label(egui::RichText::new("•").size(10.0).color(egui::Color32::DARK_GRAY));

                                    // Bitrate
                                    let bitrate_label = if bitrate >= 1000 {
                                        format!("{} Mbps", bitrate / 1000)
                                    } else {
                                        format!("{} kbps", bitrate)
                                    };
                                    ui.label(egui::RichText::new(bitrate_label)
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(200, 200, 200)));

                                    ui.label(egui::RichText::new("•").size(10.0).color(egui::Color32::DARK_GRAY));

                                    // Privacy with icon
                                    ui.label(egui::RichText::new(format!("{} {}", privacy_icon, privacy))
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(180, 180, 180)));

                                    // YouTube health indicator (when pushing or later)
                                    if matches!(state, SessionState::Pushing | SessionState::Testing | SessionState::Live) {
                                        let obs = session.get_observations();
                                        ui.label(egui::RichText::new("•").size(10.0).color(egui::Color32::DARK_GRAY));
                                        let (health_icon, health_color, health_tip) = if obs.stream_active {
                                            ("📡", egui::Color32::from_rgb(100, 255, 100), "YouTube receiving stream")
                                        } else {
                                            ("⏳", egui::Color32::from_rgb(255, 200, 100), "Waiting for YouTube to receive stream")
                                        };
                                        ui.label(egui::RichText::new(health_icon)
                                            .size(10.0)
                                            .color(health_color))
                                            .on_hover_text(health_tip);
                                    }
                                });

                                ui.add_space(4.0);

                                // Show stream URL when Testing or Live
                                if matches!(state, SessionState::Testing | SessionState::Live) {
                                    if let Some(broadcast_id) = session.get_broadcast_id() {
                                        let url = format!("youtube.com/watch?v={}", broadcast_id);
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("▶")
                                                .size(10.0)
                                                .color(egui::Color32::from_rgb(255, 0, 0)));
                                            ui.label(egui::RichText::new(&url)
                                                .size(10.0)
                                                .color(egui::Color32::from_rgb(150, 150, 255)));
                                            if ui.small_button("📋").on_hover_text("Copy URL").clicked() {
                                                ui.output_mut(|o| o.copied_text = format!("https://{}", url));
                                            }
                                        });
                                        ui.add_space(4.0);
                                    }
                                }

                                // Main control row: Stop + Duration + Stats
                                ui.horizontal(|ui| {
                                    if ui.button("Stop").clicked() {
                                        action = YouTubeAction::StopLive;
                                    }

                                    // Show streaming duration
                                    let elapsed = session.get_elapsed_secs();
                                    let hours = (elapsed / 3600.0) as u32;
                                    let minutes = ((elapsed % 3600.0) / 60.0) as u32;
                                    let seconds = (elapsed % 60.0) as u32;
                                    let duration_str = if hours > 0 {
                                        format!("{}:{:02}:{:02}", hours, minutes, seconds)
                                    } else {
                                        format!("{:02}:{:02}", minutes, seconds)
                                    };
                                    ui.label(egui::RichText::new(duration_str)
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(150, 255, 150)));

                                    // Show dropped frames if any
                                    let dropped = session.get_dropped_frames();
                                    if dropped > 0 {
                                        ui.label(egui::RichText::new(format!("⚠ {}", dropped))
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(255, 200, 50)))
                                            .on_hover_text(format!("{} dropped frames", dropped));
                                    }
                                });
                            }
                            SessionState::Failed(ref error_msg) => {
                                // Show error details
                                ui.label(egui::RichText::new(error_msg)
                                    .size(10.0)
                                    .color(egui::Color32::from_rgb(255, 150, 150)));
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Retry").clicked() {
                                        action = YouTubeAction::RetryLive;
                                    }
                                    if ui.button("Dismiss").clicked() {
                                        action = YouTubeAction::Dismiss;
                                    }
                                });
                            }
                            SessionState::Ended | SessionState::Ending => {
                                // Show stream summary
                                let elapsed = session.get_elapsed_secs();
                                let hours = (elapsed / 3600.0) as u32;
                                let minutes = ((elapsed % 3600.0) / 60.0) as u32;
                                let seconds = (elapsed % 60.0) as u32;
                                let duration_str = if hours > 0 {
                                    format!("{}:{:02}:{:02}", hours, minutes, seconds)
                                } else {
                                    format!("{:02}:{:02}", minutes, seconds)
                                };

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Duration:")
                                        .size(10.0)
                                        .color(egui::Color32::GRAY));
                                    ui.label(egui::RichText::new(&duration_str)
                                        .size(10.0)
                                        .color(egui::Color32::WHITE));
                                });

                                let dropped = session.get_dropped_frames();
                                if dropped > 0 {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Dropped:")
                                            .size(10.0)
                                            .color(egui::Color32::GRAY));
                                        ui.label(egui::RichText::new(format!("{} frames", dropped))
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(255, 200, 50)));
                                    });
                                }

                                ui.add_space(4.0);
                                if ui.button("OK").clicked() {
                                    action = YouTubeAction::Dismiss;
                                }
                            }
                        }
                    } else {
                        // No session yet - show title input and Go Live button
                        ui.add_space(8.0);

                        // Stream title input
                        ui.horizontal(|ui| {
                            ui.label("Title:");
                            ui.add(egui::TextEdit::singleline(&mut self.youtube_stream_title)
                                .hint_text("My Stream")
                                .desired_width(120.0));
                        });
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(
                            if self.youtube_stream_title.is_empty() {
                                "REACT (Live) by Compeller.ai".to_string()
                            } else {
                                format!("{} - REACT (Live) by Compeller.ai", self.youtube_stream_title)
                            })
                            .size(10.0)
                            .color(egui::Color32::GRAY));
                        ui.add_space(4.0);

                        // Show actual resolution from output settings
                        let ndi_res = self.ndi_resolution;
                        let (w, h) = ndi_res.dimensions();
                        let res_label = if h >= 2160 { "4K" } else { "HD" };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Resolution:").size(11.0));
                            ui.label(egui::RichText::new(format!("{} ({}x{})", res_label, w, h))
                                .size(10.0)
                                .color(egui::Color32::from_rgb(100, 200, 255)));
                            ui.label(egui::RichText::new("← from Output settings")
                                .size(9.0)
                                .color(egui::Color32::DARK_GRAY));
                        });

                        // Resolution selector (all 60fps)
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Quality:").size(11.0));
                            for quality in [YouTubeQuality::Hd60, YouTubeQuality::Qhd60, YouTubeQuality::Uhd60] {
                                let selected = self.youtube_quality == quality;
                                let btn = egui::Button::new(egui::RichText::new(quality.label()).size(10.0))
                                    .fill(if selected { egui::Color32::from_rgb(80, 80, 80) } else { egui::Color32::from_rgb(50, 50, 50) });
                                if ui.add(btn).clicked() {
                                    self.youtube_quality = quality;
                                }
                            }
                        });

                        // Privacy selector
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Privacy:").size(11.0));
                            for privacy in [YouTubePrivacy::Public, YouTubePrivacy::Unlisted, YouTubePrivacy::Private] {
                                let selected = self.youtube_privacy == privacy;
                                let btn = egui::Button::new(egui::RichText::new(privacy.label()).size(10.0))
                                    .fill(if selected { egui::Color32::from_rgb(80, 80, 80) } else { egui::Color32::from_rgb(50, 50, 50) });
                                if ui.add(btn).clicked() {
                                    self.youtube_privacy = privacy;
                                }
                            }
                        });

                        ui.add_space(8.0);

                        let go_live_text = egui::RichText::new("Go Live")
                            .color(egui::Color32::WHITE);

                        if ui.add(egui::Button::new(go_live_text)
                            .fill(egui::Color32::from_rgb(200, 50, 50)))
                            .clicked()
                        {
                            let full_title = if self.youtube_stream_title.is_empty() {
                                "REACT (Live) by Compeller.ai".to_string()
                            } else {
                                format!("{} - REACT (Live) by Compeller.ai", self.youtube_stream_title)
                            };
                            let config = YouTubeStreamConfig {
                                quality: self.youtube_quality,
                                privacy: self.youtube_privacy,
                            };
                            action = YouTubeAction::GoLive { title: full_title, config };
                        }
                    }
                }
            }

            // Disconnect option (small, at bottom)
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.small_button("Disconnect").clicked() {
                    action = YouTubeAction::DisconnectYouTube;
                }
            });
            } // end else (is_connected)
        });

        // Track expanded state
        self.youtube_expanded = header.fully_open();

        action
    }

    /// Show the Twitch section
    pub fn show_twitch_section(
        &mut self,
        ui: &mut egui::Ui,
        is_streaming: bool,
        is_authenticated: bool,
    ) -> TwitchAction {
        let mut action = TwitchAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Twitch")
                .color(egui::Color32::from_rgb(145, 70, 255)) // Twitch purple
        )
        .default_open(self.twitch_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Require Compeller login
            if !is_authenticated {
                self.show_login_required_message(ui);
                return;
            }

            match &self.twitch_state {
                TwitchStreamState::Idle => {
                    // Channel name
                    ui.horizontal(|ui| {
                        ui.label("Channel:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_channel)
                        .hint_text("Your Twitch channel name")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }

                    ui.add_space(4.0);

                    // Stream key input
                    ui.horizontal(|ui| {
                        ui.label("Stream Key:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_key)
                        .password(true)
                        .hint_text("Enter your Twitch stream key")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }

                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Get your stream key from Twitch Dashboard → Settings → Stream")
                        .size(9.0)
                        .color(egui::Color32::GRAY));

                    ui.add_space(8.0);

                    // Stream title
                    ui.horizontal(|ui| {
                        ui.label("Title:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_title)
                        .hint_text("Stream title")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }

                    ui.add_space(4.0);

                    // Category/Game
                    ui.horizontal(|ui| {
                        ui.label("Category:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_category)
                        .hint_text("Music, Just Chatting, etc.")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }

                    ui.add_space(4.0);

                    // Tags with pill UI
                    ui.horizontal(|ui| {
                        ui.label("Tags:");
                    });

                    // Parse existing tags
                    let tags: Vec<String> = self.twitch_stream_tags
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    // Display tag pills
                    let mut tags_to_remove: Vec<usize> = Vec::new();
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (idx, tag) in tags.iter().enumerate() {
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(145, 70, 255))
                                .rounding(egui::Rounding::same(10.0))
                                .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        ui.label(egui::RichText::new(tag)
                                            .size(10.0)
                                            .color(egui::Color32::WHITE));
                                        if ui.add(egui::Label::new(
                                            egui::RichText::new("×")
                                                .size(11.0)
                                                .color(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)))
                                            .sense(egui::Sense::click()))
                                            .clicked()
                                        {
                                            tags_to_remove.push(idx);
                                        }
                                    });
                                });
                        }
                    });

                    // Remove tags that were clicked
                    if !tags_to_remove.is_empty() {
                        let remaining_tags: Vec<String> = tags.into_iter().enumerate()
                            .filter(|(i, _)| !tags_to_remove.contains(i))
                            .map(|(_, t)| t)
                            .collect();
                        self.twitch_stream_tags = remaining_tags.join(", ");
                        self.twitch_settings_dirty = true;
                    }

                    // Input for new tag
                    ui.horizontal(|ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut self.twitch_new_tag_input)
                            .hint_text("Add tag...")
                            .desired_width(100.0));

                        // Add tag on Enter or button click
                        let add_clicked = ui.add(egui::Button::new("+")
                            .min_size(egui::vec2(20.0, 18.0)))
                            .clicked();

                        if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || add_clicked {
                            let new_tag = self.twitch_new_tag_input.trim().to_string();
                            if !new_tag.is_empty() {
                                if self.twitch_stream_tags.is_empty() {
                                    self.twitch_stream_tags = new_tag;
                                } else {
                                    self.twitch_stream_tags.push_str(", ");
                                    self.twitch_stream_tags.push_str(&new_tag);
                                }
                                self.twitch_new_tag_input.clear();
                                self.twitch_settings_dirty = true;
                            }
                        }
                    });

                    ui.add_space(8.0);

                    // Output mode selection (720p/1080p × 30/60fps)
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Quality")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 180)));

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let modes = [
                                (OutputMode::P720_30, "720p30"),
                                (OutputMode::P720_60, "720p60"),
                                (OutputMode::P1080_30, "1080p30"),
                                (OutputMode::P1080_60, "1080p60"),
                            ];

                            for (mode, label) in modes.iter().rev() {
                                let is_selected = self.twitch_output_mode == *mode;
                                let btn_color = if is_selected {
                                    egui::Color32::from_rgb(145, 70, 255)
                                } else {
                                    egui::Color32::from_rgb(60, 60, 70)
                                };
                                let text_color = if is_selected {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_rgb(150, 150, 150)
                                };

                                if ui.add(egui::Button::new(
                                    egui::RichText::new(*label)
                                        .size(9.0)
                                        .color(text_color))
                                    .fill(btn_color)
                                    .min_size(egui::vec2(50.0, 18.0)))
                                    .clicked()
                                {
                                    self.twitch_output_mode = *mode;
                                }
                            }
                        });
                    });

                    // Audio gain slider
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Audio Gain:").size(11.0).color(egui::Color32::GRAY));
                        let response = ui.add(
                            egui::Slider::new(&mut self.twitch_audio_gain, 0.1..=16.0)
                                .logarithmic(true)
                                .show_value(true)
                                .custom_formatter(|v, _| format!("{:.1}x", v))
                        );
                        if response.changed() {
                            self.pending_gain_action = OutputAction::SetTwitchGain(self.twitch_audio_gain);
                        }
                    });

                    ui.add_space(8.0);

                    // Go Live button
                    let can_go_live = !self.twitch_stream_key.is_empty();
                    ui.add_enabled_ui(can_go_live, |ui| {
                        if ui.add(egui::Button::new(
                            egui::RichText::new("Go Live")
                                .color(egui::Color32::WHITE))
                            .fill(egui::Color32::from_rgb(145, 70, 255)))
                            .clicked()
                        {
                            action = TwitchAction::GoLive {
                                stream_key: self.twitch_stream_key.clone(),
                                output_mode: self.twitch_output_mode,
                                channel: self.twitch_channel.clone(),
                                title: self.twitch_stream_title.clone(),
                                category: self.twitch_stream_category.clone(),
                                tags: self.twitch_stream_tags.clone(),
                            };
                        }
                    });
                }
                TwitchStreamState::Connecting => {
                    // Show connecting status
                    ui.horizontal(|ui| {
                        let spin = (ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5;
                        let alpha = (150.0 + 105.0 * spin as f32) as u8;
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgba_unmultiplied(145, 70, 255, alpha)));
                        ui.label(egui::RichText::new("Connecting to Twitch...")
                            .color(egui::Color32::from_rgb(180, 180, 180)));
                    });

                    ui.add_space(8.0);

                    // Cancel button
                    if ui.button("Cancel").clicked() {
                        action = TwitchAction::StopLive;
                    }
                }
                TwitchStreamState::Streaming => {
                    // Show streaming status
                    ui.horizontal(|ui| {
                        let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.5 + 0.5;
                        let alpha = (200.0 + 55.0 * pulse as f32) as u8;
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgba_unmultiplied(145, 70, 255, alpha)));
                        ui.label(egui::RichText::new("LIVE on Twitch")
                            .color(egui::Color32::from_rgb(145, 70, 255)));
                    });

                    // Show duration
                    if let Some(start) = self.twitch_start_time {
                        let elapsed = start.elapsed().as_secs_f32();
                        let hours = (elapsed / 3600.0) as u32;
                        let minutes = ((elapsed % 3600.0) / 60.0) as u32;
                        let seconds = (elapsed % 60.0) as u32;
                        let duration_str = if hours > 0 {
                            format!("{}:{:02}:{:02}", hours, minutes, seconds)
                        } else {
                            format!("{:02}:{:02}", minutes, seconds)
                        };
                        ui.label(egui::RichText::new(duration_str)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(150, 255, 150)));
                    }

                    ui.add_space(8.0);

                    // Stop button
                    if ui.button("Stop").clicked() {
                        action = TwitchAction::StopLive;
                    }
                }
                TwitchStreamState::Error(msg) => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgb(255, 100, 100)));
                        ui.label(egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(255, 150, 150)));
                    });

                    ui.add_space(8.0);

                    if ui.button("Dismiss").clicked() {
                        self.twitch_state = TwitchStreamState::Idle;
                    }
                }
            }
        });

        self.twitch_expanded = header.fully_open();

        action
    }

    /// Render the Kick streaming section.
    ///
    /// Mirrors `show_twitch_section`, with two differences: Kick supplies a
    /// full ingest URL (so there's no server-ping discovery — the user pastes
    /// the URL from their dashboard), and Kick has no ingest-time category/tags
    /// API, so the section is URL + key + title + quality + gain only.
    /// Reuses `TwitchStreamState` for the Idle/Connecting/Streaming/Error
    /// lifecycle.
    pub fn show_kick_section(
        &mut self,
        ui: &mut egui::Ui,
        _is_streaming: bool,
        is_authenticated: bool,
    ) -> KickAction {
        let mut action = KickAction::None;

        // Kick brand green. Dark text reads better than white on this fill.
        let kick_green = egui::Color32::from_rgb(83, 252, 24);

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Kick").color(kick_green)
        )
        .default_open(self.kick_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Require Compeller login
            if !is_authenticated {
                self.show_login_required_message(ui);
                return;
            }

            match &self.kick_state {
                TwitchStreamState::Idle => {
                    // Stream URL (Kick gives a full ingest URL, not just a host)
                    ui.horizontal(|ui| {
                        ui.label("Stream URL:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.kick_stream_url)
                        .hint_text("rtmps://...live-video.net/app")
                        .desired_width(180.0)).changed() {
                        self.kick_settings_dirty = true;
                    }

                    ui.add_space(4.0);

                    // Stream key input
                    ui.horizontal(|ui| {
                        ui.label("Stream Key:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.kick_stream_key)
                        .password(true)
                        .hint_text("Enter your Kick stream key")
                        .desired_width(180.0)).changed() {
                        self.kick_settings_dirty = true;
                    }

                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Get your Stream URL + Key from Kick Creator Dashboard → Settings → Stream Key")
                        .size(9.0)
                        .color(egui::Color32::GRAY));

                    ui.add_space(8.0);

                    // Stream title
                    ui.horizontal(|ui| {
                        ui.label("Title:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.kick_stream_title)
                        .hint_text("Stream title")
                        .desired_width(180.0)).changed() {
                        self.kick_settings_dirty = true;
                    }

                    ui.add_space(8.0);

                    // Output mode selection (720p/1080p × 30/60fps)
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Quality")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 180)));

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let modes = [
                                (OutputMode::P720_30, "720p30"),
                                (OutputMode::P720_60, "720p60"),
                                (OutputMode::P1080_30, "1080p30"),
                                (OutputMode::P1080_60, "1080p60"),
                            ];

                            for (mode, label) in modes.iter().rev() {
                                let is_selected = self.kick_output_mode == *mode;
                                let btn_color = if is_selected {
                                    kick_green
                                } else {
                                    egui::Color32::from_rgb(60, 60, 70)
                                };
                                let text_color = if is_selected {
                                    egui::Color32::BLACK
                                } else {
                                    egui::Color32::from_rgb(150, 150, 150)
                                };

                                if ui.add(egui::Button::new(
                                    egui::RichText::new(*label)
                                        .size(9.0)
                                        .color(text_color))
                                    .fill(btn_color)
                                    .min_size(egui::vec2(50.0, 18.0)))
                                    .clicked()
                                {
                                    self.kick_output_mode = *mode;
                                }
                            }
                        });
                    });

                    // Audio gain slider
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Audio Gain:").size(11.0).color(egui::Color32::GRAY));
                        let response = ui.add(
                            egui::Slider::new(&mut self.kick_audio_gain, 0.1..=16.0)
                                .logarithmic(true)
                                .show_value(true)
                                .custom_formatter(|v, _| format!("{:.1}x", v))
                        );
                        if response.changed() {
                            self.pending_gain_action = OutputAction::SetKickGain(self.kick_audio_gain);
                        }
                    });

                    ui.add_space(8.0);

                    // Go Live button — needs both the ingest URL and the key
                    let can_go_live = !self.kick_stream_url.is_empty()
                        && !self.kick_stream_key.is_empty();
                    ui.add_enabled_ui(can_go_live, |ui| {
                        if ui.add(egui::Button::new(
                            egui::RichText::new("Go Live")
                                .color(egui::Color32::BLACK))
                            .fill(kick_green))
                            .clicked()
                        {
                            action = KickAction::GoLive {
                                stream_url: self.kick_stream_url.clone(),
                                stream_key: self.kick_stream_key.clone(),
                                output_mode: self.kick_output_mode,
                                title: self.kick_stream_title.clone(),
                            };
                        }
                    });
                }
                TwitchStreamState::Connecting => {
                    ui.horizontal(|ui| {
                        let spin = (ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5;
                        let alpha = (150.0 + 105.0 * spin as f32) as u8;
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgba_unmultiplied(83, 252, 24, alpha)));
                        ui.label(egui::RichText::new("Connecting to Kick...")
                            .color(egui::Color32::from_rgb(180, 180, 180)));
                    });

                    ui.add_space(8.0);

                    if ui.button("Cancel").clicked() {
                        action = KickAction::StopLive;
                    }
                }
                TwitchStreamState::Streaming => {
                    ui.horizontal(|ui| {
                        let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.5 + 0.5;
                        let alpha = (200.0 + 55.0 * pulse as f32) as u8;
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgba_unmultiplied(83, 252, 24, alpha)));
                        ui.label(egui::RichText::new("LIVE on Kick")
                            .color(kick_green));
                    });

                    if let Some(start) = self.kick_start_time {
                        let elapsed = start.elapsed().as_secs_f32();
                        let hours = (elapsed / 3600.0) as u32;
                        let minutes = ((elapsed % 3600.0) / 60.0) as u32;
                        let seconds = (elapsed % 60.0) as u32;
                        let duration_str = if hours > 0 {
                            format!("{}:{:02}:{:02}", hours, minutes, seconds)
                        } else {
                            format!("{:02}:{:02}", minutes, seconds)
                        };
                        ui.label(egui::RichText::new(duration_str)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(150, 255, 150)));
                    }

                    ui.add_space(8.0);

                    if ui.button("Stop").clicked() {
                        action = KickAction::StopLive;
                    }
                }
                TwitchStreamState::Error(msg) => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("●")
                            .color(egui::Color32::from_rgb(255, 100, 100)));
                        ui.label(egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(255, 150, 150)));
                    });

                    ui.add_space(8.0);

                    if ui.button("Dismiss").clicked() {
                        self.kick_state = TwitchStreamState::Idle;
                    }
                }
            }
        });

        self.kick_expanded = header.fully_open();

        action
    }

    /// Set Kick streaming state (Kick reuses the Twitch state machine)
    pub fn set_kick_state(&mut self, state: TwitchStreamState) {
        let is_active = matches!(state, TwitchStreamState::Streaming | TwitchStreamState::Connecting);
        self.kick_state = state;
        if is_active && self.kick_start_time.is_none() {
            self.kick_start_time = Some(std::time::Instant::now());
        } else if !is_active {
            self.kick_start_time = None;
        }
    }

    /// Get Kick streaming state
    pub fn get_kick_state(&self) -> &TwitchStreamState {
        &self.kick_state
    }

    /// Get how long we've been in active Kick state (connecting or streaming)
    pub fn get_kick_elapsed_secs(&self) -> Option<f32> {
        self.kick_start_time.map(|t| t.elapsed().as_secs_f32())
    }

    /// Set Kick ingest stream URL (for loading saved value)
    pub fn set_kick_stream_url(&mut self, url: String) {
        self.kick_stream_url = url;
    }

    /// Set Kick stream key (for loading saved value)
    pub fn set_kick_stream_key(&mut self, key: String) {
        self.kick_stream_key = key;
    }

    /// Set Kick stream title (for loading saved value)
    pub fn set_kick_stream_title(&mut self, title: String) {
        self.kick_stream_title = title;
    }

    /// Check if Kick settings need saving
    pub fn is_kick_settings_dirty(&self) -> bool {
        self.kick_settings_dirty
    }

    /// Clear the Kick settings dirty flag
    pub fn clear_kick_settings_dirty(&mut self) {
        self.kick_settings_dirty = false;
    }

    /// Get Kick settings for persistence: (stream_url, stream_key, title)
    pub fn get_kick_settings(&self) -> (String, String, String) {
        (
            self.kick_stream_url.clone(),
            self.kick_stream_key.clone(),
            self.kick_stream_title.clone(),
        )
    }

    /// Set Twitch streaming state
    pub fn set_twitch_state(&mut self, state: TwitchStreamState) {
        let is_active = matches!(state, TwitchStreamState::Streaming | TwitchStreamState::Connecting);
        self.twitch_state = state;
        if is_active && self.twitch_start_time.is_none() {
            self.twitch_start_time = Some(std::time::Instant::now());
        } else if !is_active {
            self.twitch_start_time = None;
        }
    }

    /// Get Twitch streaming state
    pub fn get_twitch_state(&self) -> &TwitchStreamState {
        &self.twitch_state
    }

    /// Get how long we've been in active Twitch state (connecting or streaming)
    pub fn get_twitch_elapsed_secs(&self) -> Option<f32> {
        self.twitch_start_time.map(|t| t.elapsed().as_secs_f32())
    }

    /// Set Twitch stream key (for loading saved value)
    pub fn set_twitch_stream_key(&mut self, key: String) {
        self.twitch_stream_key = key;
    }

    /// Set Twitch channel name (for loading saved value)
    pub fn set_twitch_channel(&mut self, channel: String) {
        self.twitch_channel = channel;
    }

    /// Set Twitch stream title (for loading saved value)
    pub fn set_twitch_stream_title(&mut self, title: String) {
        self.twitch_stream_title = title;
    }

    /// Set Twitch stream category (for loading saved value)
    pub fn set_twitch_stream_category(&mut self, category: String) {
        self.twitch_stream_category = category;
    }

    /// Set Twitch stream tags (for loading saved value)
    pub fn set_twitch_stream_tags(&mut self, tags: String) {
        self.twitch_stream_tags = tags;
    }

    /// Check if Twitch settings need saving
    pub fn is_twitch_settings_dirty(&self) -> bool {
        self.twitch_settings_dirty
    }

    /// Reset Twitch settings dirty flag after saving
    pub fn clear_twitch_settings_dirty(&mut self) {
        self.twitch_settings_dirty = false;
    }

    /// Get current Twitch settings for saving
    pub fn get_twitch_settings(&self) -> (String, String, String, String, String) {
        (
            self.twitch_channel.clone(),
            self.twitch_stream_key.clone(),
            self.twitch_stream_title.clone(),
            self.twitch_stream_category.clone(),
            self.twitch_stream_tags.clone(),
        )
    }

    /// Load saved login credentials from database (for auto-fill)
    pub fn load_saved_credentials(&mut self, db: &Database) {
        if let Some((username, password)) = db.get_login_credentials() {
            self.login_username = username;
            self.login_password = password;
        }
    }

    /// Clear login form fields
    pub fn clear_login_form(&mut self) {
        self.login_username.clear();
        self.login_password.clear();
    }

    /// Set login modal visibility
    pub fn set_show_login_modal(&mut self, show: bool) {
        self.show_login_modal = show;
    }

    /// Check if login modal should be shown
    pub fn should_show_login_modal(&self) -> bool {
        self.show_login_modal
    }

    // === NDI Streaming (Twitch and/or YouTube) ===

    /// Show the Twitch streaming section
    pub fn show_dual_stream_section(
        &mut self,
        ui: &mut egui::Ui,
        is_authenticated: bool,
        youtube_rtmp_url: Option<&str>,
    ) -> DualStreamAction {
        let mut action = DualStreamAction::None;

        let header = egui::CollapsingHeader::new(
            egui::RichText::new("Twitch")
                .color(egui::Color32::from_rgb(145, 70, 255)) // Twitch purple
        )
        .default_open(self.dual_stream_expanded)
        .show(ui, |ui| {
            ui.add_space(4.0);

            // Require Compeller login
            if !is_authenticated {
                self.show_login_required_message(ui);
                return;
            }

            // Check current state
            let is_idle = matches!(self.dual_stream_state, DualStreamState::Idle);
            let is_streaming = matches!(self.dual_stream_state, DualStreamState::Streaming(_));
            let is_connecting = matches!(self.dual_stream_state, DualStreamState::Connecting(_));

            // Always enable Twitch for this section
            self.ndi_stream_twitch = true;
            self.ndi_stream_youtube = false;

            if is_idle {
                    // Channel name
                    ui.horizontal(|ui| {
                        ui.label("Channel:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_channel)
                        .hint_text("Your Twitch channel name")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }

                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Stream Key:");
                    });
                    if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_key)
                        .password(true)
                        .hint_text("Enter your Twitch stream key")
                        .desired_width(180.0)).changed() {
                        self.twitch_settings_dirty = true;
                    }
                    ui.label(egui::RichText::new("Get from Twitch Dashboard → Settings → Stream")
                        .size(9.0)
                        .color(egui::Color32::GRAY));
                    ui.add_space(4.0);

                    // Twitch ingest servers list (collapsible)
                    if !self.twitch_servers.is_empty() {
                        let server_header = egui::CollapsingHeader::new(
                            egui::RichText::new("Ingest Servers")
                                .size(10.0)
                                .color(egui::Color32::from_rgb(145, 70, 255))
                        )
                        .default_open(self.twitch_servers_expanded)
                        .show(ui, |ui| {
                            for server in &self.twitch_servers {
                                ui.horizontal(|ui| {
                                    // Selection indicator
                                    if server.selected {
                                        ui.label(egui::RichText::new("●")
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(100, 255, 100)));
                                    } else {
                                        ui.label(egui::RichText::new("○")
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(100, 100, 100)));
                                    }

                                    // Server name
                                    let name_color = if server.selected {
                                        egui::Color32::WHITE
                                    } else {
                                        egui::Color32::from_rgb(180, 180, 180)
                                    };
                                    ui.label(egui::RichText::new(&server.name)
                                        .size(10.0)
                                        .color(name_color));

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        // Latency
                                        if let Some(ms) = server.latency_ms {
                                            let latency_color = if ms < 50 {
                                                egui::Color32::from_rgb(100, 255, 100) // Green
                                            } else if ms < 100 {
                                                egui::Color32::from_rgb(255, 255, 100) // Yellow
                                            } else {
                                                egui::Color32::from_rgb(255, 150, 100) // Orange
                                            };
                                            ui.label(egui::RichText::new(format!("{}ms", ms))
                                                .size(10.0)
                                                .color(latency_color));
                                        } else {
                                            ui.label(egui::RichText::new("--")
                                                .size(10.0)
                                                .color(egui::Color32::from_rgb(100, 100, 100)));
                                        }
                                    });
                                });
                            }
                        });
                        self.twitch_servers_expanded = server_header.fully_open();
                    }

                ui.add_space(8.0);

                // Stream title
                ui.horizontal(|ui| {
                    ui.label("Title:");
                });
                if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_title)
                    .hint_text("Stream title")
                    .desired_width(180.0)).changed() {
                    self.twitch_settings_dirty = true;
                }

                ui.add_space(4.0);

                // Category/Game
                ui.horizontal(|ui| {
                    ui.label("Category:");
                });
                if ui.add(egui::TextEdit::singleline(&mut self.twitch_stream_category)
                    .hint_text("DJs, Music, Just Chatting")
                    .desired_width(180.0)).changed() {
                    self.twitch_settings_dirty = true;
                }

                ui.add_space(4.0);

                // Tags with pill UI
                ui.horizontal(|ui| {
                    ui.label("Tags:");
                });

                // Parse existing tags
                let tags: Vec<String> = self.twitch_stream_tags
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                // Display tag pills
                let mut tags_to_remove: Vec<usize> = Vec::new();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (idx, tag) in tags.iter().enumerate() {
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(145, 70, 255))
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.label(egui::RichText::new(tag)
                                        .size(10.0)
                                        .color(egui::Color32::WHITE));
                                    if ui.add(egui::Label::new(
                                        egui::RichText::new("×")
                                            .size(11.0)
                                            .color(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)))
                                        .sense(egui::Sense::click()))
                                        .clicked()
                                    {
                                        tags_to_remove.push(idx);
                                    }
                                });
                            });
                    }
                });

                if !tags_to_remove.is_empty() {
                    let remaining_tags: Vec<String> = tags.into_iter().enumerate()
                        .filter(|(i, _)| !tags_to_remove.contains(i))
                        .map(|(_, t)| t)
                        .collect();
                    self.twitch_stream_tags = remaining_tags.join(", ");
                    self.twitch_settings_dirty = true;
                }

                ui.horizontal(|ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut self.twitch_new_tag_input)
                        .hint_text("Add tag...")
                        .desired_width(100.0));

                    let add_clicked = ui.add(egui::Button::new("+")
                        .min_size(egui::vec2(20.0, 18.0)))
                        .clicked();

                    if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || add_clicked {
                        let new_tag = self.twitch_new_tag_input.trim().to_string();
                        if !new_tag.is_empty() {
                            if self.twitch_stream_tags.is_empty() {
                                self.twitch_stream_tags = new_tag;
                            } else {
                                self.twitch_stream_tags.push_str(", ");
                                self.twitch_stream_tags.push_str(&new_tag);
                            }
                            self.twitch_new_tag_input.clear();
                            self.twitch_settings_dirty = true;
                        }
                    }
                });

                ui.add_space(8.0);

                // Output mode selection (720p/1080p × 30/60fps)
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Quality")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(180, 180, 180)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Mode buttons in a compact row
                        let modes = [
                            (OutputMode::P720_30, "720p30"),
                            (OutputMode::P720_60, "720p60"),
                            (OutputMode::P1080_30, "1080p30"),
                            (OutputMode::P1080_60, "1080p60"),
                        ];

                        // Reverse iteration for right-to-left layout
                        for (mode, label) in modes.iter().rev() {
                            let is_selected = self.twitch_output_mode == *mode;
                            let btn_color = if is_selected {
                                egui::Color32::from_rgb(145, 70, 255) // Twitch purple
                            } else {
                                egui::Color32::from_rgb(60, 60, 70)
                            };
                            let text_color = if is_selected {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_rgb(150, 150, 150)
                            };

                            if ui.add(egui::Button::new(
                                egui::RichText::new(*label)
                                    .size(9.0)
                                    .color(text_color))
                                .fill(btn_color)
                                .min_size(egui::vec2(50.0, 18.0)))
                                .clicked()
                            {
                                self.twitch_output_mode = *mode;
                            }
                        }
                    });
                });

                // Show bitrate for selected mode
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Bitrate")
                        .size(10.0)
                        .color(egui::Color32::from_rgb(120, 120, 120)));
                    ui.label(egui::RichText::new(format!("{} kbps CBR", self.twitch_output_mode.twitch_video_bitrate_kbps()))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(100, 200, 100)));
                });

                ui.add_space(8.0);

                // Go Live button - enabled when stream key is entered
                let can_go_live = !self.twitch_stream_key.is_empty();

                ui.add_enabled_ui(can_go_live, |ui| {
                    if ui.add(egui::Button::new(
                        egui::RichText::new("Go Live")
                            .color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(145, 70, 255))) // Twitch purple
                        .clicked()
                    {
                        action = DualStreamAction::StartTwitch {
                            twitch_stream_key: self.twitch_stream_key.clone(),
                            output_mode: self.twitch_output_mode,
                            channel: self.twitch_channel.clone(),
                            title: self.twitch_stream_title.clone(),
                            category: self.twitch_stream_category.clone(),
                            tags: self.twitch_stream_tags.clone(),
                        };
                    }
                });

                // Warning if no stream key
                if !can_go_live {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("⚠ Enter Twitch stream key")
                        .size(9.0)
                        .color(egui::Color32::from_rgb(255, 200, 100)));
                }
            } else if is_connecting {
                // Show connecting status
                let target = match &self.dual_stream_state {
                    DualStreamState::Connecting(t) => *t,
                    _ => StreamTarget::Both,
                };
                let target_str = match target {
                    StreamTarget::Twitch => "Twitch",
                    StreamTarget::YouTube => "YouTube",
                    StreamTarget::Both => "Twitch + YouTube",
                };
                ui.horizontal(|ui| {
                    let spin = (ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5;
                    let alpha = (150.0 + 105.0 * spin as f32) as u8;
                    ui.label(egui::RichText::new("●")
                        .color(egui::Color32::from_rgba_unmultiplied(255, 80, 80, alpha)));
                    ui.label(egui::RichText::new(format!("Connecting to {}...", target_str))
                        .color(egui::Color32::from_rgb(180, 180, 180)));
                });

                ui.add_space(8.0);

                if ui.button("Cancel").clicked() {
                    action = DualStreamAction::StopStream;
                }
            } else if is_streaming {
                // Show streaming status
                let target = match &self.dual_stream_state {
                    DualStreamState::Streaming(t) => *t,
                    _ => StreamTarget::Both,
                };
                let target_str = match target {
                    StreamTarget::Twitch => "Twitch",
                    StreamTarget::YouTube => "YouTube",
                    StreamTarget::Both => "Twitch + YouTube",
                };
                ui.horizontal(|ui| {
                    let pulse = (ui.input(|i| i.time) * 2.0).sin() * 0.5 + 0.5;
                    let alpha = (200.0 + 55.0 * pulse as f32) as u8;
                    ui.label(egui::RichText::new("●")
                        .color(egui::Color32::from_rgba_unmultiplied(255, 50, 50, alpha)));
                    ui.label(egui::RichText::new(format!("LIVE on {}", target_str))
                        .color(egui::Color32::from_rgb(255, 80, 80)));

                    // Show current mode for Twitch streams
                    if target == StreamTarget::Twitch {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{} · {} kbps CBR",
                                self.twitch_output_mode.label(),
                                self.twitch_output_mode.twitch_video_bitrate_kbps()))
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 120, 120)));
                        });
                    }
                });

                // Show duration
                if let Some(start) = self.dual_stream_start_time {
                    let elapsed = start.elapsed().as_secs_f32();
                    let hours = (elapsed / 3600.0) as u32;
                    let minutes = ((elapsed % 3600.0) / 60.0) as u32;
                    let seconds = (elapsed % 60.0) as u32;
                    let duration_str = if hours > 0 {
                        format!("{}:{:02}:{:02}", hours, minutes, seconds)
                    } else {
                        format!("{:02}:{:02}", minutes, seconds)
                    };
                    ui.label(egui::RichText::new(duration_str)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 255, 150)));
                }

                ui.add_space(8.0);

                if ui.button("Stop").clicked() {
                    action = DualStreamAction::StopStream;
                }
            } else if let DualStreamState::Error(msg) = &self.dual_stream_state {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("●")
                        .color(egui::Color32::from_rgb(255, 100, 100)));
                    ui.label(egui::RichText::new(msg)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(255, 150, 150)));
                });

                ui.add_space(8.0);

                if ui.button("Dismiss").clicked() {
                    self.dual_stream_state = DualStreamState::Idle;
                }
            }
        });

        self.dual_stream_expanded = header.fully_open();

        action
    }

    /// Set dual streaming state
    pub fn set_dual_stream_state(&mut self, state: DualStreamState) {
        let is_active = matches!(state, DualStreamState::Streaming(_) | DualStreamState::Connecting(_));
        self.dual_stream_state = state;
        if is_active && self.dual_stream_start_time.is_none() {
            self.dual_stream_start_time = Some(std::time::Instant::now());
        } else if !is_active {
            self.dual_stream_start_time = None;
        }
    }

    /// Get dual streaming state
    pub fn get_dual_stream_state(&self) -> &DualStreamState {
        &self.dual_stream_state
    }

    /// Update Twitch ingest servers list (called after ping test)
    pub fn set_twitch_servers(&mut self, servers: Vec<TwitchServerInfo>) {
        self.twitch_servers = servers;
    }

    /// Get Twitch servers for display
    pub fn get_twitch_servers(&self) -> &[TwitchServerInfo] {
        &self.twitch_servers
    }
}
