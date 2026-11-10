//! WLED LED strip integration
//!
//! Streams RGB data to WLED devices over UDP using the DRGB protocol.
//! Supports three visualization modes (Spectrum, Beat, Pulse) that leverage
//! the full audio analysis pipeline.
//!
//! ## Architecture
//!
//! - **WledManager**: Coordinator — owns strip configs, streamers, vis state
//! - **streaming**: UDP DRGB packet builder and background sender thread
//! - **mappings**: Audio analysis → per-LED RGB renderers

pub mod streaming;
pub mod mappings;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::hue::CueSnapshot;
use crate::lighting::FormFactor;

// ============================================================================
// TYPES
// ============================================================================

/// Visualization mode for a WLED strip
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StripMode {
    /// Frequency band scrolling timeline (center-mirrored)
    Spectrum,
    /// Onset envelope spatial spread from center
    Beat,
    /// Whole-strip energy-reactive wash
    Pulse,
}

impl StripMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            StripMode::Spectrum => "Spectrum",
            StripMode::Beat => "Beat",
            StripMode::Pulse => "Pulse",
        }
    }

    pub fn all() -> &'static [StripMode] {
        &[StripMode::Spectrum, StripMode::Beat, StripMode::Pulse]
    }
}

impl Default for StripMode {
    fn default() -> Self {
        StripMode::Spectrum
    }
}

/// Frequency band data extracted from AudioAnalysis for WLED renderers
#[derive(Clone, Debug)]
pub struct FrequencyBands {
    pub bass: f32,
    pub mid: f32,
    pub high: f32,
    pub kick: bool,
    pub kick_intensity: f32,
    /// 12-bin pitch class energy for note-to-color mapping
    pub chroma: [f32; 12],
}

impl Default for FrequencyBands {
    fn default() -> Self {
        Self {
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            kick: false,
            kick_intensity: 0.0,
            chroma: [0.0; 12],
        }
    }
}

/// Connection state for a single WLED strip
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StripConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

impl StripConnectionState {
    pub fn as_str(&self) -> &str {
        match self {
            StripConnectionState::Disconnected => "Disconnected",
            StripConnectionState::Connecting => "Connecting...",
            StripConnectionState::Connected => "Connected",
            StripConnectionState::Error(e) => e.as_str(),
        }
    }
}

/// Persistent config for a single WLED strip
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WledStripConfig {
    pub ip: String,
    pub name: String,
    pub led_count: usize,
    pub udp_port: u16,
    pub rgbw: bool,
    pub mode: StripMode,
    pub enabled: bool,
    /// User override for form factor (WLED defaults to Strip)
    #[serde(default)]
    pub form_factor_override: Option<FormFactor>,
}

impl WledStripConfig {
    /// Effective form factor: user override or default Strip
    pub fn form_factor(&self) -> FormFactor {
        self.form_factor_override.unwrap_or(FormFactor::Strip)
    }
}

impl Default for WledStripConfig {
    fn default() -> Self {
        Self {
            ip: String::new(),
            name: String::new(),
            led_count: 30,
            udp_port: 21324,
            rgbw: false,
            mode: StripMode::Spectrum,
            enabled: true,
            form_factor_override: None,
        }
    }
}

/// Persistent config for the WLED subsystem
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WledConfig {
    pub enabled: bool,
    pub strips: Vec<WledStripConfig>,
}

impl Default for WledConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            strips: Vec::new(),
        }
    }
}

/// Runtime state for a single strip (for UI display)
#[derive(Clone, Debug)]
pub struct WledStripState {
    pub ip: String,
    pub name: String,
    pub led_count: usize,
    pub mode: StripMode,
    pub enabled: bool,
    pub connection: StripConnectionState,
    pub form_factor: FormFactor,
}

/// Runtime state for the WLED subsystem (for UI display)
#[derive(Clone, Debug)]
pub struct WledState {
    pub enabled: bool,
    pub strips: Vec<WledStripState>,
    pub error: Option<String>,
}

impl Default for WledState {
    fn default() -> Self {
        Self {
            enabled: false,
            strips: Vec::new(),
            error: None,
        }
    }
}

// ============================================================================
// VISUALIZATION STATE (per-strip, not serialized)
// ============================================================================

/// Per-strip visualization state (envelopes, buffers)
struct StripVisState {
    /// Spectrum mode: scrolling color buffer
    spectrum_buffer: VecDeque<(f32, f32, f32)>,
    /// Beat mode: kick envelope (~2 decays/sec)
    kick_env: mappings::DynamicDecay,
    /// Beat mode: dominant pitch change envelope (~4 decays/sec)
    dominant_env: mappings::DynamicDecay,
    /// Beat mode: polarity flash envelope (200ms fixed decay)
    polarity_env: mappings::FixedDecay,
}

impl StripVisState {
    fn new() -> Self {
        Self {
            spectrum_buffer: VecDeque::new(),
            kick_env: mappings::DynamicDecay::new(2.0),
            dominant_env: mappings::DynamicDecay::new(4.0),
            polarity_env: mappings::FixedDecay::new(0.2),
        }
    }
}

// ============================================================================
// PENDING DEVICE INFO (from HTTP fetch)
// ============================================================================

/// Device info fetched from WLED HTTP API
struct PendingDeviceInfo {
    ip: String,
    name: String,
    led_count: usize,
    udp_port: u16,
    rgbw: bool,
}

// ============================================================================
// WLED MANAGER
// ============================================================================

/// Coordinates WLED strip connections, UDP streaming, and visualization rendering
pub struct WledManager {
    config: WledConfig,
    /// Per-strip connection state (for UI)
    strip_connections: Vec<StripConnectionState>,
    /// Per-strip UDP streamers (None if disconnected)
    streamers: Vec<Option<streaming::WledStreamer>>,
    /// Per-strip visualization state
    vis_states: Vec<StripVisState>,
    /// Pending device info from background HTTP fetches
    pending_info: Arc<Mutex<Vec<PendingDeviceInfo>>>,
    /// Frame timing for throttling
    last_frame_time: Instant,
    /// Diagnostic frame counter
    diag_frame: u64,
}

impl WledManager {
    /// WLED max recommended streaming rate
    const MAX_FPS: f32 = 50.0;
    const MIN_FRAME_INTERVAL: f32 = 1.0 / Self::MAX_FPS;

    pub fn new() -> Self {
        Self {
            config: WledConfig::default(),
            strip_connections: Vec::new(),
            streamers: Vec::new(),
            vis_states: Vec::new(),
            pending_info: Arc::new(Mutex::new(Vec::new())),
            last_frame_time: Instant::now(),
            diag_frame: 0,
        }
    }

    pub fn config(&self) -> &WledConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: WledConfig) {
        let strip_count = config.strips.len();
        self.config = config;
        // Resize parallel vectors
        self.strip_connections.resize(strip_count, StripConnectionState::Disconnected);
        self.streamers.resize_with(strip_count, || None);
        while self.vis_states.len() < strip_count {
            self.vis_states.push(StripVisState::new());
        }
        self.vis_states.truncate(strip_count);
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if !enabled {
            self.disconnect_all();
        }
        log::info!("[WLED] {}", if enabled { "Enabled" } else { "Disabled" });
    }

    /// Get current state snapshot for UI
    pub fn state(&self) -> WledState {
        WledState {
            enabled: self.config.enabled,
            strips: self.config.strips.iter().enumerate().map(|(idx, strip)| {
                WledStripState {
                    ip: strip.ip.clone(),
                    name: if strip.name.is_empty() { strip.ip.clone() } else { strip.name.clone() },
                    led_count: strip.led_count,
                    mode: strip.mode,
                    enabled: strip.enabled,
                    connection: self.strip_connections.get(idx)
                        .cloned()
                        .unwrap_or(StripConnectionState::Disconnected),
                    form_factor: strip.form_factor(),
                }
            }).collect(),
            error: None,
        }
    }

    // ========================================================================
    // STRIP MANAGEMENT
    // ========================================================================

    /// Add a strip by IP address. Spawns a background HTTP fetch for device info.
    pub fn add_strip(&mut self, ip: String) {
        // Don't add duplicates
        if self.config.strips.iter().any(|s| s.ip == ip) {
            log::warn!("[WLED] Strip {} already exists, skipping", ip);
            return;
        }

        // Add with defaults (will be updated when HTTP info arrives)
        let strip = WledStripConfig {
            ip: ip.clone(),
            ..WledStripConfig::default()
        };
        self.config.strips.push(strip);
        self.strip_connections.push(StripConnectionState::Disconnected);
        self.streamers.push(None);
        self.vis_states.push(StripVisState::new());

        log::info!("[WLED] Added strip at {} (fetching info...)", ip);

        // Spawn background HTTP fetch for device info
        let pending = Arc::clone(&self.pending_info);
        std::thread::spawn(move || {
            log::info!("[WLED] Fetching device info from http://{}...", ip);
            match fetch_device_info(&ip) {
                Ok(info) => {
                    log::info!("[WLED] Device '{}': {} LEDs, port {}, rgbw={}",
                        info.name, info.led_count, info.udp_port, info.rgbw);
                    let mut pending = pending.lock().unwrap();
                    pending.push(info);
                }
                Err(e) => {
                    log::warn!("[WLED] Failed to fetch info from {}: {}", ip, e);
                }
            }
        });
    }

    /// Remove a strip by index
    pub fn remove_strip(&mut self, idx: usize) {
        if idx >= self.config.strips.len() {
            return;
        }
        // Disconnect if connected
        if let Some(streamer) = self.streamers.get_mut(idx).and_then(|s| s.take()) {
            streamer.shutdown();
        }
        let ip = self.config.strips[idx].ip.clone();
        self.config.strips.remove(idx);
        self.strip_connections.remove(idx);
        self.streamers.remove(idx);
        self.vis_states.remove(idx);
        log::info!("[WLED] Removed strip at {}", ip);
    }

    /// Set the visualization mode for a strip
    pub fn set_strip_mode(&mut self, idx: usize, mode: StripMode) {
        if let Some(strip) = self.config.strips.get_mut(idx) {
            strip.mode = mode;
            // Reset vis state for this strip
            if let Some(vs) = self.vis_states.get_mut(idx) {
                *vs = StripVisState::new();
            }
            log::info!("[WLED] Strip {} mode set to {}", strip.ip, mode.as_str());
        }
    }

    /// Set or clear form factor override for a strip
    pub fn set_strip_form_factor(&mut self, idx: usize, form_factor: Option<FormFactor>) {
        if let Some(strip) = self.config.strips.get_mut(idx) {
            strip.form_factor_override = form_factor;
        }
    }

    /// Set enabled state for a strip
    pub fn set_strip_enabled(&mut self, idx: usize, enabled: bool) {
        if let Some(strip) = self.config.strips.get_mut(idx) {
            strip.enabled = enabled;
            if !enabled {
                self.disconnect(idx);
            }
        }
    }

    // ========================================================================
    // CONNECTION
    // ========================================================================

    /// Connect a strip (creates UDP streamer)
    pub fn connect(&mut self, idx: usize) {
        if idx >= self.config.strips.len() {
            return;
        }
        // Already connected?
        if self.streamers.get(idx).map(|s| s.is_some()).unwrap_or(false) {
            return;
        }

        let strip = &self.config.strips[idx];
        let ip = strip.ip.clone();
        let port = strip.udp_port;

        self.strip_connections[idx] = StripConnectionState::Connecting;

        match streaming::WledStreamer::new(&ip, port) {
            Ok(streamer) => {
                self.streamers[idx] = Some(streamer);
                self.strip_connections[idx] = StripConnectionState::Connected;
                log::info!("[WLED] Connected to {}:{}", ip, port);
            }
            Err(e) => {
                self.strip_connections[idx] = StripConnectionState::Error(e.clone());
                log::error!("[WLED] Failed to connect to {}:{} — {}", ip, port, e);
            }
        }
    }

    /// Disconnect a strip
    pub fn disconnect(&mut self, idx: usize) {
        if idx >= self.streamers.len() {
            return;
        }
        if let Some(streamer) = self.streamers[idx].take() {
            streamer.shutdown();
        }
        if idx < self.strip_connections.len() {
            self.strip_connections[idx] = StripConnectionState::Disconnected;
        }
    }

    /// Connect all enabled strips
    pub fn connect_all(&mut self) {
        let count = self.config.strips.len();
        for idx in 0..count {
            if self.config.strips[idx].enabled {
                self.connect(idx);
            }
        }
    }

    /// Disconnect all strips
    pub fn disconnect_all(&mut self) {
        let count = self.streamers.len();
        for idx in 0..count {
            if let Some(streamer) = self.streamers[idx].take() {
                streamer.shutdown();
            }
            if idx < self.strip_connections.len() {
                self.strip_connections[idx] = StripConnectionState::Disconnected;
            }
        }
    }

    // ========================================================================
    // PER-FRAME UPDATE
    // ========================================================================

    /// Apply pending device info from background HTTP fetches.
    /// Returns true if any config was updated (caller should persist).
    pub fn apply_pending_info(&mut self) -> bool {
        let pending: Vec<PendingDeviceInfo> = {
            let mut p = self.pending_info.lock().unwrap();
            std::mem::take(&mut *p)
        };

        if pending.is_empty() {
            return false;
        }

        let mut changed = false;
        for info in pending {
            if let Some(strip) = self.config.strips.iter_mut().find(|s| s.ip == info.ip) {
                strip.name = info.name;
                strip.led_count = info.led_count;
                strip.udp_port = info.udp_port;
                strip.rgbw = info.rgbw;
                changed = true;
            }
        }
        changed
    }

    /// Update all connected strips with current audio state.
    /// Throttled to 50fps internally.
    pub fn update(&mut self, snapshot: &CueSnapshot, bands: &FrequencyBands) {
        if !self.config.enabled {
            return;
        }

        let elapsed = self.last_frame_time.elapsed().as_secs_f32();
        if elapsed < Self::MIN_FRAME_INTERVAL {
            return;
        }
        let dt = elapsed;
        self.last_frame_time = Instant::now();
        self.diag_frame += 1;

        let strip_count = self.config.strips.len();
        for idx in 0..strip_count {
            // Skip disabled or disconnected strips
            if !self.config.strips[idx].enabled {
                continue;
            }
            let streamer = match &self.streamers[idx] {
                Some(s) => s,
                None => continue,
            };

            let led_count = self.config.strips[idx].led_count;
            if led_count == 0 {
                continue;
            }

            // Render based on mode
            let leds = match self.config.strips[idx].mode {
                StripMode::Spectrum => {
                    let vs = &mut self.vis_states[idx];
                    mappings::render_spectrum(&mut vs.spectrum_buffer, bands, led_count)
                }
                StripMode::Beat => {
                    let vs = &mut self.vis_states[idx];
                    mappings::render_beat(
                        &mut vs.kick_env,
                        &mut vs.dominant_env,
                        &mut vs.polarity_env,
                        snapshot,
                        bands,
                        led_count,
                        dt,
                    )
                }
                StripMode::Pulse => {
                    mappings::render_pulse(snapshot, bands, led_count)
                }
            };

            // Build and send DRGB packet
            let packet = streaming::build_drgb_packet(2, &leds);
            streamer.send(packet);

            // Periodic diagnostic logging (~every 2 seconds at 50fps)
            if self.diag_frame % 100 == 1 && idx == 0 {
                log::info!(
                    "[WLED] frame={} strips={} mode={} leds={} bass={:.2} mid={:.2} high={:.2} kick={}",
                    self.diag_frame, strip_count,
                    self.config.strips[idx].mode.as_str(),
                    led_count, bands.bass, bands.mid, bands.high, bands.kick,
                );
            }
        }
    }
}

impl Default for WledManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WledManager {
    fn drop(&mut self) {
        self.disconnect_all();
    }
}

// ============================================================================
// HTTP DEVICE INFO FETCH
// ============================================================================

/// Fetch device info from WLED's HTTP JSON API
fn fetch_device_info(ip: &str) -> Result<PendingDeviceInfo, String> {
    let url = format!("http://{}/json/info", ip);
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .map_err(|e| format!("HTTP GET {}: {}", url, e))?;

    let body: String = resp.into_string()
        .map_err(|e| format!("Read body: {}", e))?;

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Parse JSON: {}", e))?;

    let name = json.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("WLED")
        .to_string();

    let udp_port = json.get("udpport")
        .and_then(|v| v.as_u64())
        .unwrap_or(21324) as u16;

    let leds_obj = json.get("leds");
    let led_count = leds_obj
        .and_then(|l| l.get("count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(30) as usize;

    let rgbw = leds_obj
        .and_then(|l| l.get("rgbw"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(PendingDeviceInfo {
        ip: ip.to_string(),
        name,
        led_count,
        udp_port,
        rgbw,
    })
}
