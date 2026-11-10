//! OSC Configuration and Mapping Types
//!
//! Configuration for OSC output including target address, cue mappings,
//! and behavior settings.

use serde::{Deserialize, Serialize};
use crate::emerge::GenreProfile;

// ============================================================================
// TARGET PROFILES
// ============================================================================

/// OSC target profile for native protocol support
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TargetProfile {
    /// Generic /react/* namespace - users map in their DMX software
    #[default]
    Generic,
    /// MaestroDMX native protocol (/live/*)
    MaestroDMX,
    /// QLC+ native protocol
    QLCPlus,
    /// Custom profile (uses trigger_mappings for address translation)
    Custom,
}

impl TargetProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetProfile::Generic => "Generic",
            TargetProfile::MaestroDMX => "MaestroDMX",
            TargetProfile::QLCPlus => "QLC+",
            TargetProfile::Custom => "Custom",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "maestrodmx" | "maestro" => TargetProfile::MaestroDMX,
            "qlc+" | "qlcplus" | "qlc" => TargetProfile::QLCPlus,
            "custom" => TargetProfile::Custom,
            _ => TargetProfile::Generic,
        }
    }

    /// Get all available profiles
    pub fn all() -> &'static [TargetProfile] {
        &[
            TargetProfile::Generic,
            TargetProfile::MaestroDMX,
            TargetProfile::QLCPlus,
            TargetProfile::Custom,
        ]
    }

    /// Translate a /react/* address to the profile's native address
    pub fn translate_address(&self, address: &str) -> String {
        match self {
            TargetProfile::Generic => address.to_string(),
            TargetProfile::MaestroDMX => self.translate_maestro(address),
            TargetProfile::QLCPlus => self.translate_qlc(address),
            TargetProfile::Custom => address.to_string(), // Custom uses trigger_mappings
        }
    }

    fn translate_maestro(&self, address: &str) -> String {
        match address {
            // Palette
            "/react/palette" => "/live/1/palette/index".to_string(),
            "/react/palette/name" => "/live/1/palette/name".to_string(),

            // Zone/Pattern
            "/react/zone" => "/live/1/cue/index".to_string(),
            "/react/zone/name" => "/live/1/cue/name".to_string(),
            "/react/pattern" => "/live/1/pattern/index".to_string(),

            // Continuous values
            "/react/energy" => "/live/1/energy".to_string(),
            "/react/brightness" => "/live/1/brightness".to_string(),
            "/react/balance" => "/live/1/balance".to_string(),
            "/react/tension" => "/live/1/tension".to_string(),
            "/react/stability" => "/live/1/stability".to_string(),
            "/react/hue" => "/live/1/hue".to_string(),
            "/react/saturation" => "/live/1/saturation".to_string(),

            // Movers and FX
            "/react/mover/speed" => "/live/1/mover/speed".to_string(),
            "/react/fx/intensity" => "/live/1/fx/intensity".to_string(),
            "/react/fx/index" => "/live/1/fx/index".to_string(),

            // Triggers
            "/react/trigger/strobe" => "/live/1/strobe".to_string(),
            "/react/trigger/blackout" => "/global/blackout".to_string(),

            // Harmonic
            "/react/harmonic" => "/state/harmonic".to_string(),
            "/react/key" => "/state/key".to_string(),

            // Envelopes
            "/react/env/bass_pump" => "/live/1/env/bass_pump".to_string(),
            "/react/env/drop_impact" => "/live/1/env/drop_impact".to_string(),

            // Reactive accents
            "/react/accent/polarity" => "/live/1/accent/polarity".to_string(),
            "/react/accent/dominant" => "/live/1/accent/dominant".to_string(),
            "/react/energy/slope" => "/live/1/energy/slope".to_string(),
            "/react/palette/warmth" => "/live/1/palette/warmth".to_string(),

            // Pass through unknown addresses
            _ => address.to_string(),
        }
    }

    fn translate_qlc(&self, address: &str) -> String {
        // QLC+ uses a different namespace
        match address {
            "/react/palette" => "/qlc/palette".to_string(),
            "/react/zone" => "/qlc/intensity".to_string(),
            "/react/energy" => "/qlc/fader/1".to_string(),
            "/react/brightness" => "/qlc/fader/2".to_string(),
            "/react/trigger/strobe" => "/qlc/button/strobe".to_string(),
            "/react/trigger/blackout" => "/qlc/button/blackout".to_string(),
            // Pass through unknown addresses
            _ => address.to_string(),
        }
    }
}

/// OSC output configuration
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscConfig {
    /// Target IP address
    pub target_ip: String,

    /// Target port
    pub target_port: u16,

    /// Target profile for address translation
    pub target_profile: TargetProfile,

    /// Whether to send continuous values (energy, brightness, etc.)
    pub send_continuous: bool,

    /// OSC address for zone-based cue triggering
    pub zone_cue_address: String,

    /// Enable kick-triggered strobe
    pub enable_kick_strobe: bool,

    /// Enable strobe hold in Peak zone
    pub enable_strobe_hold: bool,

    /// Strobe cooldown in milliseconds (to prevent over-triggering)
    pub strobe_cooldown_ms: u64,

    /// Delay in seconds before triggering blackout after silence
    pub blackout_delay_secs: f32,

    /// Custom zone cue mappings (zone index -> cue index)
    /// If empty, uses default mapping
    pub zone_cue_overrides: Vec<ZoneCueMapping>,

    /// Custom trigger mappings
    pub trigger_mappings: Vec<OscMapping>,

    // === PALETTE NAMES (user-assignable) ===
    /// Name for ICE palette (balance > 0.7, sparse/ambient)
    pub palette_ice_name: String,
    /// Name for COOL_A palette (balance 0.4-0.7, moderate)
    pub palette_cool_a_name: String,
    /// Name for COOL_B palette (balance < 0.4, dense/saturated)
    pub palette_cool_b_name: String,

    // === PALETTE INDICES (for DMX software mapping) ===
    /// Palette index for ICE (sparse/ambient) - MaestroDMX: 0 = Cool White
    pub palette_ice_index: i32,
    /// Palette index for COOL_A (moderate) - MaestroDMX: 6 = Cyan
    pub palette_cool_a_index: i32,
    /// Palette index for COOL_B (dense/saturated) - MaestroDMX: 12 = Stained Glass
    pub palette_cool_b_index: i32,

    // === PATTERN INDICES (zone → pattern mapping) ===
    /// Pattern index for SILENT zone (no audio)
    pub pattern_silent: i32,
    /// Pattern index for STILL zone (ambient)
    pub pattern_still: i32,
    /// Pattern index for LOW zone (warm/subtle)
    pub pattern_low: i32,
    /// Pattern index for MID zone (energetic)
    pub pattern_mid: i32,
    /// Pattern index for HIGH zone (intense)
    pub pattern_high: i32,

    // === MOVER SPEED RANGE ===
    /// Minimum mover speed (low tension)
    pub mover_speed_min: f32,
    /// Maximum mover speed (high tension)
    pub mover_speed_max: f32,

    // === FX PALETTE ===
    /// Enable FX palette driven by tension
    pub enable_fx_palette: bool,

    // === GENRE PROFILE ===
    /// Active genre profile for lighting behavior tuning
    pub genre_profile: GenreProfile,

    // === REACTIVE LIGHTING FEATURES ===
    /// Enable polarity flash on quadrant changes (E↔W, N↔S transitions)
    pub enable_polarity_flash: bool,
    /// Enable dominant pitch change accents
    pub enable_dominant_accent: bool,
    /// Enable tonality-aware palette modulation (major=warm, minor=cool)
    pub enable_tonality_palettes: bool,
    /// Enable energy slope detection for build-up/breakdown effects
    pub enable_energy_slope: bool,

    // === POLARITY FLASH SETTINGS ===
    /// Polarity flash duration in milliseconds
    pub polarity_flash_duration_ms: u32,
    /// Polarity flash cooldown in milliseconds
    pub polarity_flash_cooldown_ms: u32,

    // === DOMINANT ACCENT SETTINGS ===
    /// Dominant pitch change flash duration in milliseconds
    pub dominant_flash_duration_ms: u32,
    /// Dominant pitch change cooldown in milliseconds
    pub dominant_flash_cooldown_ms: u32,

    // === REACTIVITY CONTROL ===
    /// Master reactivity slider (0.0-1.0): controls brightness floor, audio drive gain,
    /// and macro threshold sensitivity. Higher = brighter base, punchier bass, easier macro triggers.
    pub reactivity: f32,

    // === GLOBAL BRIGHTNESS ===
    /// Global brightness master dimmer (0.0-1.0). Sent as /global/brightness to MaestroDMX.
    /// Multiplies the computed brightness value and acts as an overall light level cap.
    pub global_brightness: f32,

    // === UV LEVEL ===
    /// Operator UV accent level (0.0-1.0) scaling the GigBAR UV layer
    /// (par ch4/9 + pod ch22). Stamped onto CueSnapshot for the DMX path; MIDI-learnable.
    pub uv_level: f32,

    // === BUILDUP MACRO MASTER ARM ===
    /// Master arm for the AUTO buildup macro (arc-driven BlindersHold → Pump
    /// → Recover). Off = the arc classifier never fires the macro, so the
    /// bare show-brain lanes are observable — drops are still PERCEIVED
    /// (drop_impact pulses without a phase change) so the drop-gated lanes
    /// don't go dark. Disarming releases an arc-entered hold immediately; the
    /// manual BUILDUP button / MIDI force path stays live and a forced hold
    /// plays out.
    #[serde(default = "default_enable_buildup_macro")]
    pub enable_buildup_macro: bool,

    // === LIGHTING STYLE ===
    /// Operator lighting style posture (Auto/Chill/Energetic/Banger). Clamps
    /// the intensity-license band and hard-bans vocabulary per posture; Auto
    /// is the adaptive engine untouched. Serde default keeps configs saved
    /// before this field loading as Auto.
    #[serde(default)]
    pub lighting_style: crate::emerge::LightingStyle,
}

/// Default for enable_buildup_macro when deserializing old configs missing the field
fn default_enable_buildup_macro() -> bool { true }

impl Default for OscConfig {
    fn default() -> Self {
        Self {
            target_ip: crate::osc::OscSender::detect_broadcast_address(),
            target_port: 7672,
            target_profile: TargetProfile::default(),
            send_continuous: true,
            zone_cue_address: "/show/cue/index".to_string(),
            enable_kick_strobe: true,
            enable_strobe_hold: true,
            strobe_cooldown_ms: 80,  // ~150 BPM max
            blackout_delay_secs: 2.0,
            zone_cue_overrides: Vec::new(),
            trigger_mappings: Vec::new(),

            // Palette names (user-assignable)
            palette_ice_name: "All White".to_string(),
            palette_cool_a_name: "Neon Retro".to_string(),
            palette_cool_b_name: "Vivid Saturated".to_string(),

            // Palette indices (for DMX software)
            palette_ice_index: 0,      // Generic: ICE
            palette_cool_a_index: 1,   // Generic: COOL_A
            palette_cool_b_index: 2,   // Generic: COOL_B

            // Pattern indices (zone → pattern)
            pattern_silent: 0, // Silent/no audio
            pattern_still: 0,  // Ambient/chill
            pattern_low: 1,    // Warm/subtle
            pattern_mid: 2,    // Energetic/dance
            pattern_high: 3,   // Intense/party

            // Mover speed range (tension-driven)
            mover_speed_min: 0.2,
            mover_speed_max: 0.8,

            // FX palette
            enable_fx_palette: true,

            // Genre profile (defaults to Tech House)
            genre_profile: GenreProfile::default(),

            // Reactive lighting features (all enabled by default)
            enable_polarity_flash: true,
            enable_dominant_accent: true,
            enable_tonality_palettes: true,
            enable_energy_slope: true,

            // Polarity flash settings
            polarity_flash_duration_ms: 150,
            polarity_flash_cooldown_ms: 500,

            // Dominant accent settings
            dominant_flash_duration_ms: 80,
            dominant_flash_cooldown_ms: 250,

            // Reactivity control (default 0.5 = middle ground)
            reactivity: 0.5,

            // Global brightness (default full)
            global_brightness: 1.0,

            // UV accent level (default 0.35 — a tasteful accent, not a wash)
            uv_level: 0.35,

            // Buildup macro armed by default (the shipped behavior)
            enable_buildup_macro: true,

            // Style posture: adaptive engine untouched
            lighting_style: crate::emerge::LightingStyle::Auto,
        }
    }
}

/// Zone to cue index mapping
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneCueMapping {
    /// Zone name (Silent, Still, Low, Mid, High, Peak)
    pub zone: String,
    /// Cue index to trigger
    pub cue_index: i32,
}

/// Custom OSC mapping for triggers
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscMapping {
    /// Unique ID for this mapping
    pub id: String,

    /// What triggers this mapping
    pub trigger_type: OscTriggerType,

    /// Trigger value/condition (depends on trigger_type)
    pub trigger_value: Option<String>,

    /// OSC address to send to
    pub osc_address: String,

    /// OSC argument type and value
    pub osc_argument: OscArgument,

    /// Whether this mapping is enabled
    pub enabled: bool,
}

/// Types of triggers that can fire OSC messages
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OscTriggerType {
    /// Energy zone change
    ZoneChange,
    /// Harmonic state change
    HarmonicChange,
    /// Kick detected
    Kick,
    /// Key (root note) change
    KeyChange,
    /// Manual (test button)
    Manual,
}

impl OscTriggerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OscTriggerType::ZoneChange => "zone",
            OscTriggerType::HarmonicChange => "harmonic",
            OscTriggerType::Kick => "kick",
            OscTriggerType::KeyChange => "key_change",
            OscTriggerType::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "zone" => OscTriggerType::ZoneChange,
            "harmonic" => OscTriggerType::HarmonicChange,
            "kick" => OscTriggerType::Kick,
            "key_change" => OscTriggerType::KeyChange,
            "manual" => OscTriggerType::Manual,
            _ => OscTriggerType::Manual,
        }
    }
}

/// OSC argument specification
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OscArgument {
    /// No argument (trigger only)
    None,
    /// Fixed integer value
    Int(i32),
    /// Fixed float value
    Float(f32),
    /// Fixed string value
    String(String),
    /// Dynamic value from audio analysis
    Dynamic(DynamicValue),
}

/// Dynamic values that can be mapped to OSC arguments
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DynamicValue {
    /// Current energy (0.0-1.0)
    Energy,
    /// Kick intensity (0.0-1.0)
    KickIntensity,
    /// Dominant pitch as hue (0.0-1.0)
    Hue,
    /// Key root (0-11)
    KeyRoot,
    /// Harmonic state (0-3)
    HarmonicState,
    /// Zone index (0-5)
    ZoneIndex,
    /// Bass level (0.0-1.0)
    Bass,
    /// Mid level (0.0-1.0)
    Mid,
    /// High level (0.0-1.0)
    High,
}

impl DynamicValue {
    pub fn as_str(&self) -> &'static str {
        match self {
            DynamicValue::Energy => "energy",
            DynamicValue::KickIntensity => "kick_intensity",
            DynamicValue::Hue => "hue",
            DynamicValue::KeyRoot => "key_root",
            DynamicValue::HarmonicState => "harmonic_state",
            DynamicValue::ZoneIndex => "zone_index",
            DynamicValue::Bass => "bass",
            DynamicValue::Mid => "mid",
            DynamicValue::High => "high",
        }
    }
}

/// Database row for OSC configuration
#[derive(Clone, Debug)]
pub struct OscConfigRow {
    pub key: String,
    pub value: String,
}

/// Database row for OSC mappings
#[derive(Clone, Debug)]
pub struct OscMappingRow {
    pub id: i64,
    pub trigger_type: String,
    pub trigger_value: Option<String>,
    pub osc_address: String,
    pub osc_argument: String,  // JSON serialized
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configs persisted before the lighting_style field existed must load
    /// as Auto — the adaptive engine untouched — not fail or guess.
    #[test]
    fn osc_config_style_serde_default() {
        let mut value = serde_json::to_value(OscConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("lighting_style");
        let revived: OscConfig = serde_json::from_value(value).unwrap();
        assert_eq!(revived.lighting_style, crate::emerge::LightingStyle::Auto);

        // And the db string path round-trips every posture.
        for style in crate::emerge::LightingStyle::ALL {
            assert_eq!(crate::emerge::LightingStyle::from_str(style.as_str()), style);
        }
    }
}
