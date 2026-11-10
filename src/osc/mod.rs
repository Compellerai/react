//! OSC (Open Sound Control) output for lighting control
//!
//! Send-only OSC implementation for triggering cues on external lighting
//! controllers (Maestro, QLC+, GrandMA, etc.).
//!
//! ## Architecture
//!
//! OSC is an OUTPUT ADAPTER — a peer to ArtNet/OLA/USB, Hue and WLED — not
//! the engine. The lighting brain (CueEngine + manual triggers + LFO bank +
//! snapshot production) lives in `crate::emerge::engine::LightingEngine`,
//! hosted by the 100Hz thread in `thread.rs`; this module only transports the
//! engine's results onto the OSC wire:
//!
//! - **OscManager**: the adapter — UDP sender lifecycle, throttling,
//!   profile-specific address/value translation, the continuous-value tier,
//!   MaestroDMX API fetches
//! - **OscSender**: thread-based UDP sender with crossbeam channel
//! - **Mappings**: configuration types for address/argument mapping
//!
//! Disabling OSC output stops THIS adapter only — the engine keeps producing
//! and DMX/Hue/WLED keep consuming the published snapshot.

mod sender;
mod mappings;
mod maestro_api;
pub mod thread;

pub use sender::OscSender;
// Cue engine lives in crate::emerge; the adapter reads its getters for the
// continuous tier.
use crate::emerge::cue::{AutoBlinderPhase, CueEngine};
pub use mappings::{OscConfig, OscMapping, OscTriggerType, TargetProfile};
pub use maestro_api::{MaestroData, MaestroPattern, MaestroPalette, MaestroShow};
use crate::emerge::harmony::KeySignal;
pub use thread::{OscThread, LightingInput, OscCommand as OscThreadCommand, OscUiState};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audio::AudioAnalysis;
use crate::emerge::engine::{EngineTick, LightingEngine};
use crate::render::HarmonicLabel;

// ============================================================================
// OSC THROTTLE - Prevents message flooding
// ============================================================================

/// Throttles OSC messages to prevent flooding the controller
struct OscThrottle {
    /// Last sent time and value for each address
    last_sent: HashMap<String, (Instant, f32)>,
}

impl OscThrottle {
    fn new() -> Self {
        Self {
            last_sent: HashMap::new(),
        }
    }

    /// Check if a message should be sent based on rate limiting and delta threshold
    /// Returns true if the message should be sent
    fn should_send(&mut self, address: &str, value: f32) -> bool {
        // Get rate and delta threshold based on address priority
        let (rate_hz, delta_threshold) = Self::get_throttle_params(address);
        let min_interval = std::time::Duration::from_secs_f32(1.0 / rate_hz);

        if let Some((last_time, last_value)) = self.last_sent.get(address) {
            let time_ok = last_time.elapsed() >= min_interval;
            let delta_ok = (value - last_value).abs() >= delta_threshold;

            // Send if enough time has passed OR if value changed significantly
            if !time_ok && !delta_ok {
                return false; // Skip this message
            }
        }

        // Update last sent state
        self.last_sent.insert(address.to_string(), (Instant::now(), value));
        true
    }

    /// Get throttle parameters (rate_hz, delta_threshold) for an address
    fn get_throttle_params(address: &str) -> (f32, f32) {
        // Tier 1: Critical (20Hz, 1% delta) - brightness/envelopes need responsiveness
        if matches!(address,
            "/react/brightness" | "/global/brightness"
            | "/react/env/strobe" | "/react/env/dominant" | "/react/env/polarity"
            | "/react/env/bass_pump"
        ) {
            return (20.0, 0.01);
        }

        // Tier 2: Medium (10Hz, 2% delta) - energy, hue, excitement
        if matches!(address,
            "/react/energy" | "/react/hue"
            | "/react/excitement" | "/react/tension"
            | "/react/env/drop_impact"
        ) {
            return (10.0, 0.02);
        }

        // Tier 3: Debug (5Hz, 3% delta) - normalizer diagnostics
        if address.starts_with("/debug/") {
            return (5.0, 0.03);
        }

        // Default: Slow (5Hz, 3% delta) - less critical values
        (5.0, 0.03)
    }

    /// Clear throttle state (call when stopping OSC)
    fn clear(&mut self) {
        self.last_sent.clear();
    }
}

/// OSC output adapter — transports the LightingEngine's results onto the OSC
/// wire. Holds NO engine state: trigger truth, cue state and the operator
/// config live on `crate::emerge::engine::LightingEngine`; this struct owns
/// only the sender lifecycle, throttling, the OSC-side smoothing clones and
/// the MaestroDMX API cache.
pub struct OscManager {
    /// UDP sender thread
    sender: Option<OscSender>,
    /// Whether OSC output is enabled
    enabled: bool,
    /// Frame counter for throttling continuous messages (advances once per
    /// fresh LightingInput, the old `update()` cadence)
    frame_counter: u64,
    /// Message throttle to prevent flooding
    throttle: OscThrottle,
    /// MaestroDMX API data (patterns, palettes, shows)
    maestro_data: Arc<Mutex<MaestroData>>,

    // === Clone Streams (OSC-specific smoothing, independent of UI/display;
    // advanced once per fresh input in `observe`) ===
    /// Cloned energy stream for stable zone detection
    osc_energy: f32,
    /// Cloned frequency bands for DMX output
    osc_bass: f32,
    osc_mid: f32,
    osc_high: f32,
    /// Cloned chroma for hue/saturation (slower for stable colors)
    osc_chroma: [f32; 12],

    /// Latest Collatz output from Mythos engine (for continuous OSC sends)
    last_collatz: Option<crate::render::CollatzOutput>,
    /// COM-499 Phase B2: transient per-frame LFO brightness modulation, added to
    /// the operator's `global_brightness` master at send time (clamped). Set from
    /// the engine tick before `emit`; never mutates the persisted config slider.
    /// 0.0 = no modulation.
    lfo_brightness_delta: f32,
}

impl OscManager {
    // OSC smoothing rates (slower = more stable lighting)
    const OSC_ENERGY_SMOOTHING: f32 = 0.15;  // Slow for stable zones
    const OSC_BAND_SMOOTHING: f32 = 0.25;    // Moderate for DMX response
    const OSC_CHROMA_SMOOTHING: f32 = 0.10;  // Very slow for stable hue

    /// Create a new OscManager (disabled by default)
    pub fn new() -> Self {
        Self {
            sender: None,
            enabled: false,
            frame_counter: 0,
            throttle: OscThrottle::new(),
            maestro_data: Arc::new(Mutex::new(MaestroData::new())),
            // Clone streams start at zero
            osc_energy: 0.0,
            osc_bass: 0.0,
            osc_mid: 0.0,
            osc_high: 0.0,
            osc_chroma: [0.0; 12],
            last_collatz: None,
            lfo_brightness_delta: 0.0,
        }
    }

    /// Set whether OSC output is enabled. `config` supplies the target for
    /// the sender start (ignored on disable).
    pub fn set_enabled(&mut self, enabled: bool, config: &OscConfig) {
        if enabled && !self.enabled {
            // Start sender thread
            self.start_sender(config);
            log::info!("[OSC] Output enabled -> {}:{}", config.target_ip, config.target_port);
        } else if !enabled && self.enabled {
            // Stop sender thread
            self.stop_sender();
            log::info!("[OSC] Output disabled");
        }
        self.enabled = enabled;
    }

    /// Check if OSC output is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enabled AND the sender thread is up — the gate every send path uses.
    pub fn is_live(&self) -> bool {
        self.enabled && self.sender.is_some()
    }

    /// Retarget the live sender (the engine's config copy is updated by the
    /// thread command handler).
    pub fn set_target(&mut self, ip: &str, port: u16) {
        // If already running, update the sender
        if let Some(ref sender) = self.sender {
            sender.set_target(ip, port);
        }

        log::info!("[OSC] Target set to {}:{}", ip, port);
    }

    /// Resolve a target hostname to IP address (for UI display)
    /// Returns None if target is already an IP or resolution fails
    pub fn resolve_target_ip(config: &OscConfig) -> Option<String> {
        OscSender::resolve_hostname(&config.target_ip)
    }

    /// Get a clone of the MaestroDMX API data
    pub fn maestro_data(&self) -> MaestroData {
        self.maestro_data.lock().unwrap().clone()
    }

    /// Get the shared MaestroData Arc (for threading — allows main thread to read)
    pub fn maestro_data_arc(&self) -> Arc<Mutex<MaestroData>> {
        Arc::clone(&self.maestro_data)
    }

    /// Fetch MaestroDMX data from the REST API (spawns background thread)
    ///
    /// This will update the internal maestro_data once the fetch completes.
    /// The UI should poll `maestro_data()` to check for updates.
    pub fn fetch_maestro_data(&self, config: &OscConfig) {
        // Use the original target (domain name or IP) for HTTP — ureq handles
        // DNS resolution itself and will pick appropriate addresses. Only fall
        // back to our resolved IP if the original target can't be used directly.
        let target_ip = config.target_ip.clone();
        let http_host = if target_ip.contains(':') && !target_ip.starts_with('[') {
            // Bare IPv6 address — wrap in brackets for HTTP URLs
            format!("[{}]", target_ip)
        } else {
            // Domain name or IPv4 — pass through as-is so ureq resolves it
            // (domain names let ureq pick IPv4 vs IPv6 correctly)
            target_ip.clone()
        };

        // Mark as loading and store target_ip immediately to prevent re-triggers
        {
            let mut data = self.maestro_data.lock().unwrap();
            data.loading = true;
            data.error = None;
            data.last_fetch_ip = Some(target_ip.clone());
        }

        let maestro_data = Arc::clone(&self.maestro_data);

        std::thread::spawn(move || {
            log::info!("[MaestroAPI] Fetching data from {} (target: {})", http_host, target_ip);

            match maestro_api::fetch_maestro_data(&http_host) {
                Ok(mut new_data) => {
                    new_data.loading = false;
                    // Keep the target_ip (not resolved IP) for comparison
                    new_data.last_fetch_ip = Some(target_ip);
                    let mut data = maestro_data.lock().unwrap();
                    *data = new_data;
                    log::info!(
                        "[MaestroAPI] Fetch complete: {} patterns, {} color palettes, {} fx palettes, {} shows",
                        data.patterns.len(),
                        data.color_palettes.len(),
                        data.fx_palettes.len(),
                        data.shows.len()
                    );
                }
                Err(e) => {
                    let mut data = maestro_data.lock().unwrap();
                    data.loading = false;
                    data.error = Some(e.clone());
                    // Keep last_fetch_ip so we don't retry immediately
                    log::error!("[MaestroAPI] Fetch failed: {}", e);
                }
            }
        });
    }

    /// Clear cached MaestroDMX data
    pub fn clear_maestro_data(&self) {
        let mut data = self.maestro_data.lock().unwrap();
        data.clear();
    }

    /// COM-499 Phase B2: stash this tick's operator-LFO modulation. Only the
    /// brightness delta reaches OSC (the `/global/*` tier has no reactivity
    /// channel); it is added to the `global_brightness` master in the continuous
    /// send (clamped), so OSC software pulses with the bank like Hue/WLED/DMX —
    /// without mutating the operator's slider. Call once per tick before `emit`.
    pub fn set_lfo_modulation(&mut self, m: crate::emerge::LfoModulation) {
        self.lfo_brightness_delta = m.brightness;
    }

    /// Advance the OSC-side smoothing clones + Collatz cache from a fresh
    /// LightingInput. Runs once per fresh input regardless of `enabled`
    /// (exactly the old `update()` cadence), so re-enabling OSC mid-show
    /// resumes from warm streams.
    pub fn observe(&mut self, li: &LightingInput) {
        let analysis = &li.analysis;
        self.osc_energy += (analysis.energy_vis - self.osc_energy) * Self::OSC_ENERGY_SMOOTHING;
        self.osc_bass += (analysis.bass - self.osc_bass) * Self::OSC_BAND_SMOOTHING;
        self.osc_mid += (analysis.mid - self.osc_mid) * Self::OSC_BAND_SMOOTHING;
        self.osc_high += (analysis.high - self.osc_high) * Self::OSC_BAND_SMOOTHING;

        for i in 0..12 {
            self.osc_chroma[i] += (analysis.display_chroma[i] - self.osc_chroma[i]) * Self::OSC_CHROMA_SMOOTHING;
        }

        self.frame_counter += 1;

        // Extract Collatz state from LightingMetrics for continuous value sends
        self.last_collatz = li.metrics.as_ref().and_then(|m| {
            if m.collatz_heat > 0.0 || m.collatz_intensity > 0.0 {
                Some(crate::render::CollatzOutput {
                    heat: m.collatz_heat,
                    phase: m.collatz_phase,
                    intensity: m.collatz_intensity,
                    value: 0.0, // Not in LightingMetrics (renderer-internal)
                    breath: m.collatz_breath,
                    act: m.collatz_act,
                    act_t: 0.0, // Not in LightingMetrics (renderer-internal)
                })
            } else {
                None
            }
        });
    }

    /// Transport one engine tick onto the OSC wire: held-trigger messages,
    /// CueEngine cue messages (profile-converted), and — on fresh-input ticks,
    /// every 3rd frame, outside manual overrides — the continuous-value tiers.
    /// Everything is gated on the adapter being live; the engine has already
    /// done all state advancement.
    pub fn emit(&mut self, engine: &LightingEngine, tick: &EngineTick) {
        if !self.is_live() {
            return;
        }

        // Held manual-trigger messages (generated by the engine on its own
        // clock — they flow with or without audio).
        if let Some(ref sender) = self.sender {
            for (address, value) in &tick.trigger_messages {
                sender.send(address, vec![value.clone()]);
            }
        }

        // CueEngine cue messages (only produced on fresh ticks outside manual
        // overrides).
        let config = engine.config();
        for (address, args) in &tick.cue_messages {
            self.send_with_profile_conversion(config, address, args.clone());
        }

        // Continuous values (OSC-only): fresh-input ticks, every 3rd frame,
        // suppressed while a manual trigger override is active — the old
        // `update()` gating verbatim.
        if tick.fresh
            && config.send_continuous
            && self.frame_counter % 3 == 0
            && !engine.manual_triggers().is_any_active()
        {
            if let Some(ref key_signal) = tick.key_signal {
                let analysis = *engine.last_analysis();
                self.send_continuous_values(engine.cue_engine(), config, &analysis, key_signal);
            }
        }
    }

    /// Send raw `(address, value)` messages (the held-trigger vocabulary).
    pub fn send_bools(&self, messages: &[(String, rosc::OscType)]) {
        if let Some(ref sender) = self.sender {
            for (address, value) in messages {
                sender.send(address, vec![value.clone()]);
            }
        }
    }

    /// Send raw `(address, args)` messages (force-buildup vocabulary).
    pub fn send_args(&self, messages: &[(String, Vec<rosc::OscType>)]) {
        if let Some(ref sender) = self.sender {
            for (address, args) in messages {
                sender.send(address, args.clone());
            }
        }
    }

    /// Send the global-brightness master immediately (MIDI/UI slider).
    pub fn send_global_brightness(&self, value: f32) {
        if let Some(ref sender) = self.sender {
            sender.send("/global/brightness", vec![rosc::OscType::Float(value)]);
        }
        log::info!("[OSC] Global brightness set to {:.0}%", value * 100.0);
    }

    /// Send a manual cue (for test buttons + latch edges)
    pub fn send_manual_cue(&self, address: &str, args: Vec<rosc::OscType>) {
        if let Some(ref sender) = self.sender {
            sender.send(address, args);
            log::info!("[OSC] Manual cue sent: {}", address);
        }
    }

    /// Send with profile translation
    fn send_translated(&self, config: &OscConfig, sender: &OscSender, address: &str, args: Vec<rosc::OscType>) {
        let translated = config.target_profile.translate_address(address);
        sender.send(&translated, args);
    }

    /// Send with profile-specific address translation and value conversion
    /// Handles MaestroDMX-specific conversions like blackout → global brightness
    fn send_with_profile_conversion(&self, config: &OscConfig, address: &str, args: Vec<rosc::OscType>) {
        let sender = match &self.sender {
            Some(s) => s,
            None => return,
        };

        match &config.target_profile {
            TargetProfile::MaestroDMX => {
                match address {
                    // Blackout: MaestroDMX uses /global/brightness with inverted float values
                    "/react/trigger/blackout" => {
                        // Convert Int(1) for blackout ON → Float(0.0) for brightness OFF
                        // Convert Int(0) for blackout OFF → restore to global_brightness slider value
                        let brightness = match args.first() {
                            Some(rosc::OscType::Int(1)) => 0.0f32,
                            Some(rosc::OscType::Int(0)) => config.global_brightness,
                            _ => config.global_brightness,
                        };
                        sender.send("/global/brightness", vec![rosc::OscType::Float(brightness)]);
                        log::info!("[OSC] MaestroDMX blackout: /global/brightness = {}", brightness);
                    }
                    // All other addresses: translate and send as-is
                    _ => {
                        let translated = config.target_profile.translate_address(address);
                        sender.send(&translated, args);
                    }
                }
            }
            // For other profiles, just translate the address
            _ => {
                let translated = config.target_profile.translate_address(address);
                sender.send(&translated, args);
            }
        }
    }

    /// Send continuous values with throttling.
    ///
    /// Split into three tiers:
    /// 1. **Production controls** (`/react/`, `/live/1/`): normalized+smoothed, 10-20Hz
    /// 2. **State** (`/state/`): integer state changes, 5Hz
    /// 3. **Debug** (`/debug/`): raw normalizer diagnostics, 5Hz
    ///
    /// All values are now from the adaptive normalizer (not fixed-scale clipped values).
    fn send_continuous_values(
        &mut self,
        cue_engine: &CueEngine,
        config: &OscConfig,
        analysis: &AudioAnalysis,
        key_signal: &KeySignal,
    ) {
        let sender = match &self.sender {
            Some(s) => s,
            None => return,
        };

        // Get smoothed metrics from cue engine (already consuming normalized inputs)
        let (energy, tension, balance, stability) = cue_engine.smoothed_metrics();
        let mut brightness = energy.powf(0.7);

        // Bass pump: kick-triggered brightness boost
        let bass_pump = cue_engine.bass_pump();
        if bass_pump > 0.01 {
            brightness = (brightness + bass_pump * 0.35).min(1.0);
        }

        // Macro phase brightness modulation (matches Hue compute_brightness behaviour)
        let phase = cue_engine.auto_blinder_phase();
        match phase {
            AutoBlinderPhase::OverdriveStrobe => {
                // Pump phase: bass-squared for kick-driven pump on top of energy base
                // Use max(energy-based, bass-based) so lights never dip below energy floor
                let b = self.osc_bass.clamp(0.0, 1.0);
                let bass_brightness = ((b * b) * 2.0).clamp(0.0, 1.0);
                brightness = brightness.max(bass_brightness).max(0.50);
            }
            AutoBlinderPhase::Recover => {
                // Cool phase: ease back to 85 %
                brightness *= 0.85;
            }
            AutoBlinderPhase::Normal => {
                // Sustained mode: energy-based floor keeps lights alive
                let floor = 0.40 + energy * 0.20;
                brightness = brightness.max(floor);
            }
            _ => {}
        }

        // Drop impact boost — push brightness up through the drop
        // During Pump: skip boost so bass dynamics have full 0.50–1.0 swing range
        // During Cool/Normal: additive boost + floor keeps lights alive post-drop
        let drop_impact = cue_engine.drop_impact_level();
        if drop_impact > 0.01 && phase != AutoBlinderPhase::OverdriveStrobe {
            brightness = (brightness + 0.40 * drop_impact).min(1.0);
            brightness = brightness.max(0.70 * drop_impact);
        }
        // Apply global brightness master dimmer. COM-499 Phase B2: fold in the
        // transient operator-LFO brightness modulation so OSC software pulses with
        // the bank too (clamped; never mutates the operator's config slider).
        let global_master =
            (config.global_brightness + self.lfo_brightness_delta).clamp(0.0, 1.0);
        let brightness = (brightness * global_master).clamp(0.0, 1.0);

        // Hue from dominant chroma (compute once for both namespaces)
        let mut max_idx = 0;
        let mut max_val = 0.0f32;
        for (i, &v) in self.osc_chroma.iter().enumerate() {
            if v > max_val {
                max_val = v;
                max_idx = i;
            }
        }
        let hue = max_idx as f32 / 12.0;
        let saturation = key_signal.confidence;

        // Mover speed: genre speed range × energy, boosted by excitement
        let genre = &config.genre_profile;
        let excitement = cue_engine.excitement();
        let (spd_min, spd_max) = genre.mover_speed_range;
        let excitement_speed_boost = if excitement > 0.7 { 0.15 * (excitement - 0.7) / 0.3 } else { 0.0 };
        let mover_speed = (spd_min + energy * (spd_max - spd_min) + excitement_speed_boost).min(1.0);

        // FX values: tension × genre intensity, modulated by tension bank
        let tension_bank = cue_engine.tension_bank();
        let tension_bank_mod = 0.5 + 0.5 * tension_bank; // Low bank = subtle, high bank = dramatic
        let fx_intensity = (tension * genre.mover_intensity * tension_bank_mod).min(1.0);
        let fx_index = if fx_intensity > 0.7 { 3 }
            else if fx_intensity > 0.5 { 2 }
            else if fx_intensity > 0.3 { 1 }
            else { 0 };

        let harmonic_state = match key_signal.harmonic_label {
            HarmonicLabel::Stable => 0,
            HarmonicLabel::Rotating => 1,
            HarmonicLabel::Ambiguous => 2,
            HarmonicLabel::Atonal => 3,
        };
        let key_root = key_signal.key_root as i32;
        let mode = if key_signal.major_likelihood > key_signal.minor_likelihood { 1 } else { 0 };

        // Envelope values from cue engine
        let strobe_env = cue_engine.strobe_env();
        let dominant_env = cue_engine.dominant_env();
        let polarity_env = cue_engine.polarity_env();

        // === TIER 1: Production controls (normalized+smoothed, 10-20Hz) ===

        // /react/ namespace (profile-translated, critical values)
        if self.throttle.should_send("/react/energy", energy) {
            self.send_translated(config, sender, "/react/energy", vec![rosc::OscType::Float(energy)]);
        }
        if self.throttle.should_send("/react/brightness", brightness) {
            self.send_translated(config, sender, "/react/brightness", vec![rosc::OscType::Float(brightness)]);
        }
        // Global brightness master dimmer (sent directly, not translated)
        if self.throttle.should_send("/global/brightness", global_master) {
            sender.send("/global/brightness", vec![rosc::OscType::Float(global_master)]);
        }
        if self.throttle.should_send("/react/balance", balance) {
            self.send_translated(config, sender, "/react/balance", vec![rosc::OscType::Float(balance)]);
        }
        if self.throttle.should_send("/react/tension", tension) {
            self.send_translated(config, sender, "/react/tension", vec![rosc::OscType::Float(tension)]);
        }
        if self.throttle.should_send("/react/stability", stability) {
            self.send_translated(config, sender, "/react/stability", vec![rosc::OscType::Float(stability)]);
        }
        if self.throttle.should_send("/react/hue", hue) {
            self.send_translated(config, sender, "/react/hue", vec![rosc::OscType::Float(hue)]);
        }
        if self.throttle.should_send("/react/saturation", saturation) {
            self.send_translated(config, sender, "/react/saturation", vec![rosc::OscType::Float(saturation)]);
        }
        if self.throttle.should_send("/react/excitement", excitement) {
            self.send_translated(config, sender, "/react/excitement", vec![rosc::OscType::Float(excitement)]);
        }

        // Envelope controls (0-1 smooth decay values for downstream consumers)
        if self.throttle.should_send("/react/env/strobe", strobe_env) {
            self.send_translated(config, sender, "/react/env/strobe", vec![rosc::OscType::Float(strobe_env)]);
        }
        if self.throttle.should_send("/react/env/dominant", dominant_env) {
            self.send_translated(config, sender, "/react/env/dominant", vec![rosc::OscType::Float(dominant_env)]);
        }
        if self.throttle.should_send("/react/env/polarity", polarity_env) {
            self.send_translated(config, sender, "/react/env/polarity", vec![rosc::OscType::Float(polarity_env)]);
        }

        // Bass pump and drop impact envelopes (for downstream OSC consumers)
        let bass_pump_val = cue_engine.bass_pump();
        if self.throttle.should_send("/react/env/bass_pump", bass_pump_val) {
            self.send_translated(config, sender, "/react/env/bass_pump", vec![rosc::OscType::Float(bass_pump_val)]);
        }
        let drop_impact_val = cue_engine.drop_impact_level();
        if self.throttle.should_send("/react/env/drop_impact", drop_impact_val) {
            self.send_translated(config, sender, "/react/env/drop_impact", vec![rosc::OscType::Float(drop_impact_val)]);
        }

        // Mover speed
        if self.throttle.should_send("/react/mover/speed", mover_speed) {
            self.send_translated(config, sender, "/react/mover/speed", vec![rosc::OscType::Float(mover_speed)]);
        }

        // FX palette (if enabled)
        if config.enable_fx_palette {
            if self.throttle.should_send("/react/fx/intensity", fx_intensity) {
                self.send_translated(config, sender, "/react/fx/intensity", vec![rosc::OscType::Float(fx_intensity)]);
            }
            if self.throttle.should_send("/react/fx/index", fx_index as f32) {
                self.send_translated(config, sender, "/react/fx/index", vec![rosc::OscType::Int(fx_index)]);
            }
        }

        // === TIER 2: State (integer values, 5Hz) ===
        if self.throttle.should_send("/state/harmonic", harmonic_state as f32) {
            sender.send("/state/harmonic", vec![rosc::OscType::Int(harmonic_state)]);
        }
        if self.throttle.should_send("/state/key", key_root as f32) {
            sender.send("/state/key", vec![rosc::OscType::Int(key_root)]);
        }
        if self.throttle.should_send("/state/mode", mode as f32) {
            sender.send("/state/mode", vec![rosc::OscType::Int(mode)]);
        }

        // === TIER 3: Debug (normalizer diagnostics, 5Hz) ===
        // These use /debug/ prefix so downstream consumers can filter them out.
        // Provides raw normalizer state for tuning without breaking production.
        if self.throttle.should_send("/debug/clip_ratio", analysis.norm_clip_ratio) {
            sender.send("/debug/clip_ratio", vec![rosc::OscType::Float(analysis.norm_clip_ratio)]);
        }
        if self.throttle.should_send("/debug/floor_ratio", analysis.norm_floor_ratio) {
            sender.send("/debug/floor_ratio", vec![rosc::OscType::Float(analysis.norm_floor_ratio)]);
        }
        if self.throttle.should_send("/debug/p10_energy", analysis.norm_p10_energy) {
            sender.send("/debug/p10_energy", vec![rosc::OscType::Float(analysis.norm_p10_energy)]);
        }
        if self.throttle.should_send("/debug/p90_energy", analysis.norm_p90_energy) {
            sender.send("/debug/p90_energy", vec![rosc::OscType::Float(analysis.norm_p90_energy)]);
        }
        if self.throttle.should_send("/debug/warmup", analysis.norm_warmup) {
            sender.send("/debug/warmup", vec![rosc::OscType::Float(analysis.norm_warmup)]);
        }

        // === COLLATZ STATE (Mythos engine, Tier 1: 10-20Hz) ===
        if let Some(ref cz) = self.last_collatz {
            if self.throttle.should_send("/mythos/heat", cz.heat) {
                sender.send("/mythos/heat", vec![rosc::OscType::Float(cz.heat)]);
            }
            if self.throttle.should_send("/mythos/phase", cz.phase) {
                sender.send("/mythos/phase", vec![rosc::OscType::Float(cz.phase)]);
            }
            if self.throttle.should_send("/mythos/intensity", cz.intensity) {
                sender.send("/mythos/intensity", vec![rosc::OscType::Float(cz.intensity)]);
            }
            if self.throttle.should_send("/mythos/value", cz.value) {
                sender.send("/mythos/value", vec![rosc::OscType::Float(cz.value)]);
            }
            if self.throttle.should_send("/mythos/breath", cz.breath) {
                sender.send("/mythos/breath", vec![rosc::OscType::Float(cz.breath)]);
            }
            if self.throttle.should_send("/mythos/act", cz.act as f32 / 3.0) {
                sender.send("/mythos/act", vec![rosc::OscType::Int(cz.act as i32)]);
            }
        }
    }

    /// Start the sender thread
    fn start_sender(&mut self, config: &OscConfig) {
        if self.sender.is_some() {
            return;
        }

        match OscSender::new(&config.target_ip, config.target_port) {
            Ok(sender) => {
                self.sender = Some(sender);
                log::info!("[OSC] Sender thread started");
            }
            Err(e) => {
                log::error!("[OSC] Failed to start sender: {}", e);
            }
        }
    }

    /// Stop the sender thread
    fn stop_sender(&mut self) {
        if let Some(sender) = self.sender.take() {
            sender.shutdown();
            self.throttle.clear(); // Reset throttle state for clean restart
            log::info!("[OSC] Sender thread stopped");
        }
    }
}

impl Default for OscManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for OscManager {
    fn drop(&mut self) {
        self.stop_sender();
    }
}
