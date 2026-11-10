//! Genre System for REACT Lighting Engine
//!
//! Adjusts OSC behavior based on music style to make lighting more musical
//! by tuning thresholds, palettes, and triggers to match genre expectations.

use serde::{Deserialize, Serialize};

// ============================================================================
// PALETTE BIAS
// ============================================================================

/// Palette color temperature bias
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PaletteBias {
    /// Cool whites, cyans, blues
    #[default]
    Cool,
    /// Warm whites, oranges, reds
    Warm,
    /// Neutral whites throughout
    Neutral,
    /// Vivid saturated colors
    Vivid,
}

impl PaletteBias {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaletteBias::Cool => "Cool",
            PaletteBias::Warm => "Warm",
            PaletteBias::Neutral => "Neutral",
            PaletteBias::Vivid => "Vivid",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "warm" => PaletteBias::Warm,
            "neutral" => PaletteBias::Neutral,
            "vivid" => PaletteBias::Vivid,
            _ => PaletteBias::Cool,
        }
    }

    pub fn all() -> &'static [PaletteBias] {
        &[PaletteBias::Cool, PaletteBias::Warm, PaletteBias::Neutral, PaletteBias::Vivid]
    }

    /// Get MaestroDMX palette indices for ICE/COOL_A/COOL_B based on bias
    pub fn palette_indices(&self) -> (i32, i32, i32) {
        match self {
            PaletteBias::Cool => (0, 6, 7),      // Cool White, Cyan, Light Blue
            PaletteBias::Warm => (2, 3, 4),      // Off White, Light Orange, Orange
            PaletteBias::Neutral => (1, 1, 1),   // All White throughout
            PaletteBias::Vivid => (5, 10, 12),   // Red, Blue, Stained Glass
        }
    }
}

/// Default for enable_buildup_blinder when deserializing old profiles missing the field
fn default_enable_buildup_blinder() -> bool { true }

// ============================================================================
// ZONE THRESHOLDS
// ============================================================================

/// Energy thresholds for zone transitions
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneThresholds {
    /// Below this energy = STILL (default 0.15)
    pub still_max: f32,
    /// Below this energy = LOW (default 0.4)
    pub low_max: f32,
    /// Below this energy = MID (default 0.7), above = HIGH
    pub mid_max: f32,
}

impl Default for ZoneThresholds {
    fn default() -> Self {
        Self {
            still_max: 0.15,
            low_max: 0.40,
            mid_max: 0.70,
        }
    }
}

// ============================================================================
// MINIMUM ZONE FOR STROBE
// ============================================================================

/// Minimum zone required for strobe to fire
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StrobeZoneMinimum {
    /// Strobe can fire in LOW zone and above
    Low,
    /// Strobe can fire in MID zone and above
    #[default]
    Mid,
    /// Strobe only fires in HIGH zone
    High,
}

impl StrobeZoneMinimum {
    pub fn as_str(&self) -> &'static str {
        match self {
            StrobeZoneMinimum::Low => "LOW",
            StrobeZoneMinimum::Mid => "MID",
            StrobeZoneMinimum::High => "HIGH",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "LOW" => StrobeZoneMinimum::Low,
            "HIGH" => StrobeZoneMinimum::High,
            _ => StrobeZoneMinimum::Mid,
        }
    }

    /// Check if the given zone index meets the minimum requirement
    pub fn zone_meets_minimum(&self, zone_index: i32) -> bool {
        match self {
            StrobeZoneMinimum::Low => zone_index >= 2,  // LOW = 2
            StrobeZoneMinimum::Mid => zone_index >= 3,  // MID = 3
            StrobeZoneMinimum::High => zone_index >= 4, // HIGH = 4
        }
    }
}

// ============================================================================
// GENRE PROFILE
// ============================================================================

/// Complete genre profile defining lighting behavior for a music style
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenreProfile {
    /// Category (Electronic, Rock, Instrumental, Pop, Live/Other)
    pub category: String,
    /// Subcategory/style (Techno, Deep House, Metal, Classical, etc.)
    pub subcategory: String,

    // Zone thresholds
    pub zone_thresholds: ZoneThresholds,

    // Palette behavior
    pub palette_bias: PaletteBias,
    /// How reactive palette is to balance changes (0.5 = slow, 2.0 = fast)
    pub palette_change_rate: f32,

    // Strobe behavior
    pub strobe_enabled: bool,
    /// Strobe trigger probability (0.0 = never, 1.0 = every kick)
    pub strobe_sensitivity: f32,
    pub strobe_zone_minimum: StrobeZoneMinimum,

    // Pattern behavior
    /// How quickly patterns follow zone changes (0.5 = slow, 2.0 = fast)
    pub pattern_change_rate: f32,
    /// Pattern indices for [STILL, LOW, MID, HIGH]
    pub preferred_patterns: [i32; 4],

    // Blackout behavior
    pub blackout_on_silence: bool,
    /// Milliseconds of silence before blackout
    pub blackout_delay_ms: u32,

    // Mover behavior
    /// (min_speed, max_speed) based on tension
    pub mover_speed_range: (f32, f32),
    /// Overall movement intensity multiplier
    pub mover_intensity: f32,

    // Auto-blinder behavior
    /// Enable auto-blinder on buildup → drop detection
    #[serde(default = "default_enable_buildup_blinder")]
    pub enable_buildup_blinder: bool,
}

impl Default for GenreProfile {
    fn default() -> Self {
        Self::tech_house() // Default to Tech House as a balanced profile
    }
}

impl GenreProfile {
    // ========================================================================
    // GENRE CATEGORIES
    // ========================================================================

    pub fn categories() -> &'static [&'static str] {
        &["Electronic", "Rock", "Instrumental", "Pop", "Hip Hop / R&B", "Live/Other"]
    }

    pub fn subcategories_for(category: &str) -> Vec<&'static str> {
        match category {
            "Electronic" => vec![
                // House variants
                "House", "Deep House", "Tech House", "Progressive House",
                // Techno variants
                "Techno", "Minimal Techno", "Melodic Techno", "Industrial Techno",
                // Trance variants
                "Trance", "Progressive Trance", "Psytrance", "Goa Trance",
                // Bass music
                "Drum & Bass", "Dubstep", "Bassline", "UK Garage", "Jungle",
                // Hard dance
                "Hardstyle", "Hardcore", "Gabber",
                // Breaks
                "Breakbeat",
                // Modern/Festival
                "EDM", "Future Bass", "Future Rave", "Electro",
                // Retro/Synth
                "Synthwave", "Electro Pop", "Vaporwave",
                // Experimental
                "IDM", "Glitch",
                // Chill
                "Ambient", "Chillout",
            ],
            "Rock" => vec![
                "Alternative", "Metal", "Indie", "Classic Rock", "Post-Rock/Shoegaze"
            ],
            "Instrumental" => vec![
                "Classical", "Jazz", "Soundtrack/Cinematic", "Lo-Fi/Chill"
            ],
            "Pop" => vec![
                "Pop/Top 40", "Hip Hop/R&B", "Latin"
            ],
            "Hip Hop / R&B" => vec![
                "Classic R&B (Slow Jams & Ballads)",
                "Boom Bap & 90s Hip Hop",
                "Modern Hip Hop & R&B",
                "Trap & Modern Rap",
            ],
            "Live/Other" => vec![
                "Live Band", "DJ Set (Mixed)", "Custom"
            ],
            _ => vec!["Custom"],
        }
    }

    // ========================================================================
    // PRESET PROFILES
    // ========================================================================

    pub fn from_genre(category: &str, subcategory: &str) -> Self {
        match (category, subcategory) {
            // Electronic - House variants
            ("Electronic", "House") => Self::house(),
            ("Electronic", "Deep House") => Self::deep_house(),
            ("Electronic", "Tech House") => Self::tech_house(),
            ("Electronic", "Progressive House") => Self::progressive_house(),
            // Electronic - Techno variants
            ("Electronic", "Techno") => Self::techno(),
            ("Electronic", "Minimal Techno") => Self::minimal_techno(),
            ("Electronic", "Melodic Techno") => Self::melodic_techno(),
            ("Electronic", "Industrial Techno") => Self::industrial_techno(),
            // Electronic - Trance variants
            ("Electronic", "Trance") => Self::trance(),
            ("Electronic", "Progressive Trance") => Self::progressive_trance(),
            ("Electronic", "Psytrance") => Self::psytrance(),
            ("Electronic", "Goa Trance") => Self::goa_trance(),
            // Electronic - Bass music
            ("Electronic", "Drum & Bass") => Self::drum_and_bass(),
            ("Electronic", "Dubstep") => Self::dubstep(),
            ("Electronic", "Bassline") => Self::bassline(),
            ("Electronic", "UK Garage") => Self::uk_garage(),
            ("Electronic", "Jungle") => Self::jungle(),
            // Electronic - Hard dance
            ("Electronic", "Hardstyle") => Self::hardstyle(),
            ("Electronic", "Hardcore") => Self::hardcore(),
            ("Electronic", "Gabber") => Self::gabber(),
            // Electronic - Breaks
            ("Electronic", "Breakbeat") => Self::breakbeat(),
            // Electronic - Modern/Festival
            ("Electronic", "EDM") => Self::edm(),
            ("Electronic", "Future Bass") => Self::future_bass(),
            ("Electronic", "Future Rave") => Self::future_rave(),
            ("Electronic", "Electro") => Self::electro(),
            // Electronic - Retro/Synth
            ("Electronic", "Synthwave") => Self::synthwave(),
            ("Electronic", "Electro Pop") => Self::electro_pop(),
            ("Electronic", "Vaporwave") => Self::vaporwave(),
            // Electronic - Experimental
            ("Electronic", "IDM") => Self::idm(),
            ("Electronic", "Glitch") => Self::glitch(),
            // Electronic - Chill
            ("Electronic", "Ambient") | ("Electronic", "Ambient/Downtempo") => Self::ambient(),
            ("Electronic", "Chillout") => Self::chillout(),

            // Rock
            ("Rock", "Alternative") => Self::alternative(),
            ("Rock", "Metal") => Self::metal(),
            ("Rock", "Indie") => Self::indie(),
            ("Rock", "Classic Rock") => Self::classic_rock(),
            ("Rock", "Post-Rock/Shoegaze") => Self::post_rock(),

            // Instrumental
            ("Instrumental", "Classical") => Self::classical(),
            ("Instrumental", "Jazz") => Self::jazz(),
            ("Instrumental", "Soundtrack/Cinematic") => Self::cinematic(),
            ("Instrumental", "Lo-Fi/Chill") => Self::lofi(),

            // Pop
            ("Pop", "Pop/Top 40") => Self::pop(),
            ("Pop", "Hip Hop/R&B") => Self::hip_hop(),
            ("Pop", "Latin") => Self::latin(),

            // Live/Other
            ("Live/Other", "Live Band") => Self::live_band(),
            ("Live/Other", "DJ Set (Mixed)") => Self::dj_set(),

            // Hip Hop / R&B (tempo ranges differ per style — see tempo_range_bpm)
            ("Hip Hop / R&B", "Classic R&B (Slow Jams & Ballads)") => Self::rnb_classic(),
            ("Hip Hop / R&B", "Boom Bap & 90s Hip Hop") => Self::boom_bap(),
            ("Hip Hop / R&B", "Modern Hip Hop & R&B") => Self::modern_hiphop(),
            ("Hip Hop / R&B", "Trap & Modern Rap") => Self::trap(),

            // Default
            _ => Self::custom(),
        }
    }

    // ========================================================================
    // HIP HOP / R&B PRESETS
    // Visual params reuse the Pop `hip_hop` groove; they differ mainly in the
    // tempo range (see `tempo_range_bpm`). Operator refines visuals later.
    // ========================================================================

    pub fn rnb_classic() -> Self {
        Self {
            category: "Hip Hop / R&B".to_string(),
            subcategory: "Classic R&B (Slow Jams & Ballads)".to_string(),
            ..Self::hip_hop()
        }
    }

    pub fn boom_bap() -> Self {
        Self {
            category: "Hip Hop / R&B".to_string(),
            subcategory: "Boom Bap & 90s Hip Hop".to_string(),
            ..Self::hip_hop()
        }
    }

    pub fn modern_hiphop() -> Self {
        Self {
            category: "Hip Hop / R&B".to_string(),
            subcategory: "Modern Hip Hop & R&B".to_string(),
            ..Self::hip_hop()
        }
    }

    pub fn trap() -> Self {
        Self {
            category: "Hip Hop / R&B".to_string(),
            subcategory: "Trap & Modern Rap".to_string(),
            ..Self::hip_hop()
        }
    }

    /// Typical BEAT-tempo range (min,max BPM) per genre — the prior REACT's
    /// autonomous tempo estimator uses to resolve the double-/half-time octave
    /// (Lo-Fi 70-90 -> 75; Trap 130-160 -> 150). Keyed by (category, subcategory).
    /// `None` = genuinely no guidance (Custom / DJ Set / unknown): the estimator
    /// keeps its plain 85-170 fold and an unbiased autocorrelation.
    /// Operator-tunable. External clocks (Pro DJ Link / Ableton) ignore this.
    pub fn tempo_range_bpm(&self) -> Option<(f32, f32)> {
        match (self.category.as_str(), self.subcategory.as_str()) {
            // Hip Hop / R&B
            ("Hip Hop / R&B", "Classic R&B (Slow Jams & Ballads)") => Some((60.0, 80.0)),
            ("Hip Hop / R&B", "Boom Bap & 90s Hip Hop") => Some((85.0, 105.0)),
            ("Hip Hop / R&B", "Modern Hip Hop & R&B") => Some((110.0, 130.0)),
            ("Hip Hop / R&B", "Trap & Modern Rap") => Some((130.0, 160.0)),
            // Electronic
            ("Electronic", "House") => Some((120.0, 128.0)),
            ("Electronic", "Deep House") => Some((118.0, 125.0)),
            ("Electronic", "Tech House") => Some((122.0, 130.0)),
            ("Electronic", "Progressive House") => Some((124.0, 132.0)),
            ("Electronic", "Techno") => Some((125.0, 135.0)),
            ("Electronic", "Minimal Techno") => Some((124.0, 132.0)),
            ("Electronic", "Melodic Techno") => Some((118.0, 126.0)),
            ("Electronic", "Industrial Techno") => Some((130.0, 140.0)),
            ("Electronic", "Trance") => Some((132.0, 142.0)),
            ("Electronic", "Progressive Trance") => Some((130.0, 138.0)),
            ("Electronic", "Psytrance") => Some((140.0, 150.0)),
            ("Electronic", "Goa Trance") => Some((138.0, 148.0)),
            ("Electronic", "Drum & Bass") => Some((165.0, 180.0)),
            ("Electronic", "Dubstep") => Some((138.0, 150.0)),
            ("Electronic", "Bassline") => Some((130.0, 140.0)),
            ("Electronic", "UK Garage") => Some((130.0, 138.0)),
            ("Electronic", "Jungle") => Some((160.0, 180.0)),
            ("Electronic", "Hardstyle") => Some((145.0, 155.0)),
            ("Electronic", "Hardcore") => Some((160.0, 185.0)),
            ("Electronic", "Gabber") => Some((175.0, 200.0)),
            ("Electronic", "Breakbeat") => Some((125.0, 140.0)),
            ("Electronic", "EDM") => Some((124.0, 132.0)),
            ("Electronic", "Future Bass") => Some((140.0, 160.0)),
            ("Electronic", "Future Rave") => Some((126.0, 134.0)),
            ("Electronic", "Electro") => Some((124.0, 130.0)),
            ("Electronic", "Synthwave") => Some((100.0, 118.0)),
            ("Electronic", "Electro Pop") => Some((110.0, 128.0)),
            ("Electronic", "Vaporwave") => Some((65.0, 90.0)),
            ("Electronic", "IDM") => Some((85.0, 130.0)),
            ("Electronic", "Glitch") => Some((100.0, 140.0)),
            ("Electronic", "Ambient") | ("Electronic", "Ambient/Downtempo") => Some((60.0, 90.0)),
            ("Electronic", "Chillout") => Some((85.0, 110.0)),
            // Rock
            ("Rock", "Alternative") => Some((110.0, 140.0)),
            ("Rock", "Metal") => Some((120.0, 180.0)),
            ("Rock", "Indie") => Some((108.0, 130.0)),
            ("Rock", "Classic Rock") => Some((110.0, 140.0)),
            // Post-rock sits at ROCK tempos under ATMOSPHERIC dynamics — the
            // whole reason this preset exists (Mogwai at 115 BPM mis-clocked
            // to ~62 under the Lo-Fi 70-90 prior chosen for its calm lights).
            ("Rock", "Post-Rock/Shoegaze") => Some((100.0, 140.0)),
            // Instrumental
            ("Instrumental", "Classical") => Some((60.0, 100.0)),
            ("Instrumental", "Jazz") => Some((100.0, 140.0)),
            ("Instrumental", "Soundtrack/Cinematic") => Some((60.0, 100.0)),
            ("Instrumental", "Lo-Fi/Chill") => Some((70.0, 90.0)),
            // Pop
            ("Pop", "Pop/Top 40") => Some((100.0, 128.0)),
            ("Pop", "Hip Hop/R&B") => Some((85.0, 115.0)),
            ("Pop", "Latin") => Some((90.0, 115.0)),
            // Live/Other
            ("Live/Other", "Live Band") => Some((100.0, 140.0)),
            // DJ Set is *explicitly* mixed-genre: any range here becomes a real prior
            // (sigma floors at 0.10 in the consumer) and punishes the off-style
            // segments a mixed set promises. No guidance is the honest answer.
            ("Live/Other", "DJ Set (Mixed)") => None,
            // Custom / unknown -> None: NO guidance. (A (60,180) fallback is NOT
            // neutral: the consumer centers a fold at sqrt(60*180) ~ 104 BPM,
            // whose window tops out at ~147 - it halved 147-170 BPM tracks.)
            _ => None,
        }
    }

    /// Map the operator's genre profile onto the show-brain's per-lane gate
    /// profile, so the genre dropdown shapes the PHYSICAL rig (DMX lanes), not
    /// just the OSC/CueEngine path. Gate thresholds in `fixture_brain`:
    /// `strobe_tolerance < 0.30` and `laser_tolerance < 0.25` hard-disable
    /// those lanes; `density < 0.35` stills the BLINDER lane; `movement`
    /// scales mover intensity/speed.
    ///
    /// Mapping intent: "enabled but rare" genres (low `strobe_sensitivity`)
    /// keep the lane OPEN just above the gate — rarity is the CueEngine's
    /// probability job, not a lane ban — while `strobe_enabled: false` genres
    /// (ambient, classical, lo-fi) close the strobe AND laser lanes outright.
    pub fn fixture_profile(&self) -> crate::emerge::FixtureGenreProfile {
        let strobe_open = self.strobe_enabled && self.strobe_sensitivity > 0.0;
        let strobe_tolerance = if strobe_open {
            (0.30 + 0.55 * self.strobe_sensitivity.clamp(0.0, 1.0)).min(0.85)
        } else {
            0.0
        };
        // No genre carries a laser field yet; lasers follow the strobe posture
        // (high-energy genres open them, chill genres close them) with a floor
        // below the 0.25 lane gate when strobes are off.
        let laser_tolerance = if strobe_open {
            (0.30 + 0.50 * self.strobe_sensitivity.clamp(0.0, 1.0)).min(0.80)
        } else {
            0.15
        };
        let movement = self.mover_intensity.clamp(0.0, 1.0);
        let density = self.pattern_change_rate.clamp(0.0, 1.0);
        crate::emerge::FixtureGenreProfile {
            aggression: 0.5 * (strobe_tolerance + movement),
            density,
            movement,
            // Saturation taste stays with the intent renderer's default; no
            // genre field expresses it yet.
            color_saturation: crate::emerge::FixtureGenreProfile::default().color_saturation,
            strobe_tolerance,
            laser_tolerance,
        }
    }

    // ========================================================================
    // ELECTRONIC PRESETS
    // ========================================================================

    pub fn techno() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Techno".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.7,
            strobe_enabled: true,
            strobe_sensitivity: 0.9,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5], // Chill, Ambient, Energetic, Party
            blackout_on_silence: true,
            blackout_delay_ms: 300,
            mover_speed_range: (0.4, 0.95),
            mover_intensity: 0.9,
            enable_buildup_blinder: true,
        }
    }

    pub fn tech_house() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Tech House".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.6,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: true,
        }
    }

    pub fn deep_house() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Deep House".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.5,
            strobe_enabled: false,
            strobe_sensitivity: 0.2,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 3, 2, 4], // Chill, Chill, Ambient, Energetic
            blackout_on_silence: true,
            blackout_delay_ms: 800,
            mover_speed_range: (0.2, 0.6),
            mover_intensity: 0.5,
            enable_buildup_blinder: false,
        }
    }

    pub fn progressive_house() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Progressive House".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.5,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 600,
            mover_speed_range: (0.25, 0.75),
            mover_intensity: 0.6,
            enable_buildup_blinder: true,
        }
    }

    pub fn drum_and_bass() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Drum & Bass".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.30,
                mid_max: 0.55,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.5,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.2,
            preferred_patterns: [2, 4, 5, 5], // Ambient, Energetic, Party, Party
            blackout_on_silence: true,
            blackout_delay_ms: 200,
            mover_speed_range: (0.5, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: true,
        }
    }

    /// Post-Rock/Shoegaze: rock tempos, atmospheric manners. Calm lighting
    /// personality (no strobes/blinders, slow movers, cool palette) with an
    /// HONEST 100-140 BPM tempo range — picking Lo-Fi for the calm lights
    /// fed the tempo estimator a 70-90 claim this material breaks.
    pub fn post_rock() -> Self {
        Self {
            category: "Rock".to_string(),
            subcategory: "Post-Rock/Shoegaze".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.20,
                low_max: 0.50,
                mid_max: 0.80,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.4,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.3,
            preferred_patterns: [3, 3, 3, 2], // sustained washes throughout
            blackout_on_silence: false,
            blackout_delay_ms: 2000,
            mover_speed_range: (0.1, 0.45),
            mover_intensity: 0.35,
            enable_buildup_blinder: false,
        }
    }

    pub fn ambient() -> Self {
        Self {
            category: "Electronic".to_string(),
            // Must match the `subcategories_for("Electronic")` dropdown entry
            // exactly — the UI resync finds the index by string equality and a
            // miss silently falls back to "House". The "Ambient/Downtempo"
            // alias arms in `from_genre`/`tempo_range_bpm` keep previously
            // serialized profiles routing correctly.
            subcategory: "Ambient".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.20,
                low_max: 0.50,
                mid_max: 0.80,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.3,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.3,
            preferred_patterns: [3, 3, 3, 2], // Chill throughout
            blackout_on_silence: false,
            blackout_delay_ms: 2000,
            mover_speed_range: (0.1, 0.3),
            mover_intensity: 0.2,
            enable_buildup_blinder: false,
        }
    }

    pub fn trance() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Trance".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.6,
            strobe_enabled: true,
            strobe_sensitivity: 0.8,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.6,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 400,
            mover_speed_range: (0.3, 0.9),
            mover_intensity: 0.8,
            enable_buildup_blinder: true,
        }
    }

    /// Classic house - steady 4/4 groove, warm and inviting
    pub fn house() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "House".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.6,
            strobe_enabled: true,
            strobe_sensitivity: 0.5,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.6,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 600,
            mover_speed_range: (0.25, 0.7),
            mover_intensity: 0.6,
            enable_buildup_blinder: true,
        }
    }

    /// Minimal techno - hypnotic, subtle changes, restrained lighting
    pub fn minimal_techno() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Minimal Techno".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.4,
            strobe_enabled: true,
            strobe_sensitivity: 0.4,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.4,
            preferred_patterns: [3, 3, 2, 4], // More restrained
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.2, 0.6),
            mover_intensity: 0.5,
            enable_buildup_blinder: false,
        }
    }

    /// Melodic techno - atmospheric, emotional builds, smooth transitions
    pub fn melodic_techno() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Melodic Techno".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.5,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.25, 0.75),
            mover_intensity: 0.65,
            enable_buildup_blinder: true,
        }
    }

    /// Industrial techno - dark, aggressive, mechanical
    pub fn industrial_techno() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Industrial Techno".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.32,
                mid_max: 0.60,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.9,
            strobe_enabled: true,
            strobe_sensitivity: 0.95,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.9,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 250,
            mover_speed_range: (0.45, 0.95),
            mover_intensity: 0.95,
            enable_buildup_blinder: true,
        }
    }

    /// Progressive trance - building energy, epic moments
    pub fn progressive_trance() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Progressive Trance".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.5,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.85),
            mover_intensity: 0.75,
            enable_buildup_blinder: true,
        }
    }

    /// Psytrance - fast, trippy, psychedelic visuals
    pub fn psytrance() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Psytrance".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.30,
                mid_max: 0.58,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.2,
            strobe_enabled: true,
            strobe_sensitivity: 0.9,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.0,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 300,
            mover_speed_range: (0.45, 1.0),
            mover_intensity: 0.95,
            enable_buildup_blinder: true,
        }
    }

    /// Goa trance - classic psychedelic trance, trippy
    pub fn goa_trance() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Goa Trance".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.32,
                mid_max: 0.60,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.0,
            strobe_enabled: true,
            strobe_sensitivity: 0.85,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.9,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 350,
            mover_speed_range: (0.4, 0.95),
            mover_intensity: 0.9,
            enable_buildup_blinder: true,
        }
    }

    /// Dubstep - heavy bass drops, dramatic contrasts
    pub fn dubstep() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Dubstep".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.60,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.2,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.0,
            preferred_patterns: [3, 2, 5, 5], // Dramatic on drops
            blackout_on_silence: true,
            blackout_delay_ms: 300,
            mover_speed_range: (0.3, 1.0),
            mover_intensity: 0.95,
            enable_buildup_blinder: true,
        }
    }

    /// Bassline - UK bass music, groovy and energetic
    pub fn bassline() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Bassline".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.9,
            strobe_enabled: true,
            strobe_sensitivity: 0.8,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 350,
            mover_speed_range: (0.35, 0.9),
            mover_intensity: 0.8,
            enable_buildup_blinder: true,
        }
    }

    /// UK Garage - bouncy, soulful, energetic
    pub fn uk_garage() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "UK Garage".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 450,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: true,
        }
    }

    /// Jungle - fast breakbeats, aggressive energy
    pub fn jungle() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Jungle".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.30,
                mid_max: 0.55,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.4,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.2,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 200,
            mover_speed_range: (0.5, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: true,
        }
    }

    /// Hardstyle - 150 BPM, big kicks, dramatic
    pub fn hardstyle() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Hardstyle".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.08,
                low_max: 0.30,
                mid_max: 0.55,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.0,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Low,
            pattern_change_rate: 1.0,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 200,
            mover_speed_range: (0.5, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: true,
        }
    }

    /// Hardcore - fast, aggressive, intense strobing
    pub fn hardcore() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Hardcore".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.06,
                low_max: 0.25,
                mid_max: 0.50,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.3,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Low,
            pattern_change_rate: 1.2,
            preferred_patterns: [2, 5, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 150,
            mover_speed_range: (0.6, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: true,
        }
    }

    /// Gabber - extremely fast, aggressive, maximum intensity
    pub fn gabber() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Gabber".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.05,
                low_max: 0.20,
                mid_max: 0.45,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.5,
            strobe_enabled: true,
            strobe_sensitivity: 1.0,
            strobe_zone_minimum: StrobeZoneMinimum::Low,
            pattern_change_rate: 1.5,
            preferred_patterns: [5, 5, 5, 5], // Maximum energy always
            blackout_on_silence: true,
            blackout_delay_ms: 100,
            mover_speed_range: (0.7, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: true,
        }
    }

    /// Breakbeat - broken beats, funky energy
    pub fn breakbeat() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Breakbeat".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.0,
            strobe_enabled: true,
            strobe_sensitivity: 0.8,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.9,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 350,
            mover_speed_range: (0.35, 0.9),
            mover_intensity: 0.8,
            enable_buildup_blinder: true,
        }
    }

    /// EDM - mainstream festival sound, big drops
    pub fn edm() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "EDM".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.0,
            strobe_enabled: true,
            strobe_sensitivity: 0.9,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.0,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 300,
            mover_speed_range: (0.35, 0.95),
            mover_intensity: 0.9,
            enable_buildup_blinder: true,
        }
    }

    /// Future Bass - emotional, melodic drops, colorful
    pub fn future_bass() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Future Bass".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.9,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 400,
            mover_speed_range: (0.3, 0.85),
            mover_intensity: 0.75,
            enable_buildup_blinder: true,
        }
    }

    /// Future Rave - David Guetta/MORTEN style, melodic with energy
    pub fn future_rave() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Future Rave".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.9,
            strobe_enabled: true,
            strobe_sensitivity: 0.85,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.9,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 350,
            mover_speed_range: (0.35, 0.9),
            mover_intensity: 0.85,
            enable_buildup_blinder: true,
        }
    }

    /// Electro - mechanical, funky, robotic
    pub fn electro() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Electro".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.65,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.75,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 400,
            mover_speed_range: (0.3, 0.85),
            mover_intensity: 0.75,
            enable_buildup_blinder: true,
        }
    }

    /// Synthwave - retro 80s vibes, neon colors
    pub fn synthwave() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Synthwave".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Vivid, // Neon pinks, cyans
            palette_change_rate: 0.5,
            strobe_enabled: false,
            strobe_sensitivity: 0.3,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 3, 2, 4],
            blackout_on_silence: true,
            blackout_delay_ms: 700,
            mover_speed_range: (0.2, 0.6),
            mover_intensity: 0.5,
            enable_buildup_blinder: false,
        }
    }

    /// Electro Pop - pop with electronic elements
    pub fn electro_pop() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Electro Pop".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.7,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.75),
            mover_intensity: 0.65,
            enable_buildup_blinder: true,
        }
    }

    /// Vaporwave - dreamy, aesthetic, slow
    pub fn vaporwave() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Vaporwave".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.18,
                low_max: 0.45,
                mid_max: 0.75,
            },
            palette_bias: PaletteBias::Vivid, // Pastel/aesthetic colors
            palette_change_rate: 0.3,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.3,
            preferred_patterns: [3, 3, 3, 2],
            blackout_on_silence: false,
            blackout_delay_ms: 1500,
            mover_speed_range: (0.1, 0.35),
            mover_intensity: 0.25,
            enable_buildup_blinder: false,
        }
    }

    /// IDM (Intelligent Dance Music) - experimental, complex
    pub fn idm() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "IDM".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.6,
            strobe_enabled: true,
            strobe_sensitivity: 0.5,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.6,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 600,
            mover_speed_range: (0.2, 0.7),
            mover_intensity: 0.55,
            enable_buildup_blinder: false,
        }
    }

    /// Glitch - experimental, unpredictable, digital artifacts
    pub fn glitch() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Glitch".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.2, // Fast changes for glitchy feel
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.0,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 400,
            mover_speed_range: (0.3, 0.85),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }

    /// Chillout - relaxed electronic, no strobe
    pub fn chillout() -> Self {
        Self {
            category: "Electronic".to_string(),
            subcategory: "Chillout".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.20,
                low_max: 0.50,
                mid_max: 0.80,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.3,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.3,
            preferred_patterns: [3, 3, 3, 2],
            blackout_on_silence: false,
            blackout_delay_ms: 2000,
            mover_speed_range: (0.1, 0.3),
            mover_intensity: 0.2,
            enable_buildup_blinder: false,
        }
    }

    // ========================================================================
    // ROCK PRESETS
    // ========================================================================

    pub fn alternative() -> Self {
        Self {
            category: "Rock".to_string(),
            subcategory: "Alternative".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 400,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }

    pub fn metal() -> Self {
        Self {
            category: "Rock".to_string(),
            subcategory: "Metal".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.10,
                low_max: 0.35,
                mid_max: 0.60,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 1.0,
            strobe_enabled: true,
            strobe_sensitivity: 0.95,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 1.0,
            preferred_patterns: [2, 4, 5, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 250,
            mover_speed_range: (0.5, 1.0),
            mover_intensity: 1.0,
            enable_buildup_blinder: false,
        }
    }

    pub fn indie() -> Self {
        Self {
            category: "Rock".to_string(),
            subcategory: "Indie".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.6,
            strobe_enabled: false,
            strobe_sensitivity: 0.3,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.6,
            preferred_patterns: [3, 3, 2, 4],
            blackout_on_silence: true,
            blackout_delay_ms: 600,
            mover_speed_range: (0.2, 0.6),
            mover_intensity: 0.5,
            enable_buildup_blinder: false,
        }
    }

    pub fn classic_rock() -> Self {
        Self {
            category: "Rock".to_string(),
            subcategory: "Classic Rock".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.7,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }

    // ========================================================================
    // INSTRUMENTAL PRESETS
    // ========================================================================

    pub fn classical() -> Self {
        Self {
            category: "Instrumental".to_string(),
            subcategory: "Classical".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.4,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.4,
            preferred_patterns: [3, 2, 2, 4], // Chill, Ambient, Ambient, Energetic
            blackout_on_silence: false,
            blackout_delay_ms: 1500,
            mover_speed_range: (0.15, 0.5),
            mover_intensity: 0.4,
            enable_buildup_blinder: false,
        }
    }

    pub fn jazz() -> Self {
        Self {
            category: "Instrumental".to_string(),
            subcategory: "Jazz".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.5,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 3, 2, 4],
            blackout_on_silence: false,
            blackout_delay_ms: 1200,
            mover_speed_range: (0.2, 0.5),
            mover_intensity: 0.4,
            enable_buildup_blinder: false,
        }
    }

    pub fn cinematic() -> Self {
        Self {
            category: "Instrumental".to_string(),
            subcategory: "Soundtrack/Cinematic".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.5,
            strobe_enabled: false,
            strobe_sensitivity: 0.2,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.5,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: false,
            blackout_delay_ms: 1000,
            mover_speed_range: (0.2, 0.7),
            mover_intensity: 0.6,
            enable_buildup_blinder: false,
        }
    }

    pub fn lofi() -> Self {
        Self {
            category: "Instrumental".to_string(),
            subcategory: "Lo-Fi/Chill".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.18,
                low_max: 0.45,
                mid_max: 0.75,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.3,
            strobe_enabled: false,
            strobe_sensitivity: 0.0,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.3,
            preferred_patterns: [3, 3, 3, 2],
            blackout_on_silence: false,
            blackout_delay_ms: 2000,
            mover_speed_range: (0.1, 0.3),
            mover_intensity: 0.2,
            enable_buildup_blinder: false,
        }
    }

    // ========================================================================
    // POP PRESETS
    // ========================================================================

    pub fn pop() -> Self {
        Self {
            category: "Pop".to_string(),
            subcategory: "Pop/Top 40".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.40,
                mid_max: 0.70,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }

    pub fn hip_hop() -> Self {
        Self {
            category: "Pop".to_string(),
            subcategory: "Hip Hop/R&B".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Vivid,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }

    pub fn latin() -> Self {
        Self {
            category: "Pop".to_string(),
            subcategory: "Latin".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Warm,
            palette_change_rate: 0.8,
            strobe_enabled: true,
            strobe_sensitivity: 0.6,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.8,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.35, 0.85),
            mover_intensity: 0.8,
            enable_buildup_blinder: false,
        }
    }

    // ========================================================================
    // LIVE/OTHER PRESETS
    // ========================================================================

    pub fn live_band() -> Self {
        Self {
            category: "Live/Other".to_string(),
            subcategory: "Live Band".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.15,
                low_max: 0.42,
                mid_max: 0.72,
            },
            palette_bias: PaletteBias::Neutral,
            palette_change_rate: 0.6,
            strobe_enabled: true,
            strobe_sensitivity: 0.5,
            strobe_zone_minimum: StrobeZoneMinimum::High,
            pattern_change_rate: 0.6,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 600,
            mover_speed_range: (0.25, 0.7),
            mover_intensity: 0.6,
            enable_buildup_blinder: false,
        }
    }

    pub fn dj_set() -> Self {
        Self {
            category: "Live/Other".to_string(),
            subcategory: "DJ Set (Mixed)".to_string(),
            zone_thresholds: ZoneThresholds {
                still_max: 0.12,
                low_max: 0.38,
                mid_max: 0.68,
            },
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.7,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.3, 0.85),
            mover_intensity: 0.75,
            enable_buildup_blinder: false,
        }
    }

    pub fn custom() -> Self {
        Self {
            category: "Live/Other".to_string(),
            subcategory: "Custom".to_string(),
            zone_thresholds: ZoneThresholds::default(),
            palette_bias: PaletteBias::Cool,
            palette_change_rate: 0.7,
            strobe_enabled: true,
            strobe_sensitivity: 0.7,
            strobe_zone_minimum: StrobeZoneMinimum::Mid,
            pattern_change_rate: 0.7,
            preferred_patterns: [3, 2, 4, 5],
            blackout_on_silence: true,
            blackout_delay_ms: 500,
            mover_speed_range: (0.2, 0.8),
            mover_intensity: 0.7,
            enable_buildup_blinder: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hip_hop_rnb_category_resolves_all_styles() {
        // The new Hip Hop / R&B category routes each style to a distinct preset
        // with the operator-supplied tempo range.
        let cases = [
            ("Classic R&B (Slow Jams & Ballads)", (60.0, 80.0)),
            ("Boom Bap & 90s Hip Hop", (85.0, 105.0)),
            ("Modern Hip Hop & R&B", (110.0, 130.0)),
            ("Trap & Modern Rap", (130.0, 160.0)),
        ];
        for (style, range) in cases {
            let p = GenreProfile::from_genre("Hip Hop / R&B", style);
            assert_eq!(p.category, "Hip Hop / R&B", "category for {style}");
            assert_eq!(p.subcategory, style, "subcategory for {style}");
            assert_eq!(p.tempo_range_bpm(), Some(range), "range for {style}");
        }
    }

    #[test]
    fn hip_hop_rnb_styles_listed_in_category() {
        // The dropdown source must surface the 4 styles (so they're selectable).
        assert!(GenreProfile::categories().contains(&"Hip Hop / R&B"));
        let subs = GenreProfile::subcategories_for("Hip Hop / R&B");
        assert_eq!(subs.len(), 4);
        assert!(subs.contains(&"Trap & Modern Rap"));
    }

    #[test]
    fn existing_genres_have_tempo_ranges() {
        // A few canonical existing genres still resolve and carry a sane range
        // (regression guard for the tempo_range_bpm table).
        let lofi = GenreProfile::from_genre("Instrumental", "Lo-Fi/Chill");
        assert_eq!(lofi.tempo_range_bpm(), Some((70.0, 90.0)));
        let dnb = GenreProfile::from_genre("Electronic", "Drum & Bass");
        assert_eq!(dnb.tempo_range_bpm(), Some((165.0, 180.0)));
        let house = GenreProfile::from_genre("Electronic", "House");
        assert_eq!(house.tempo_range_bpm(), Some((120.0, 128.0)));
    }

    #[test]
    fn unknown_and_mixed_genres_give_no_guidance() {
        // Custom / unrecognized / explicitly-mixed → None: NO tempo prior. A
        // fake-wide (60,180) range is not neutral — it acted as a ~104 BPM fold
        // center that halved 147–170 BPM tracks.
        let custom = GenreProfile::from_genre("Live/Other", "Custom");
        assert_eq!(custom.tempo_range_bpm(), None);
        let dj_set = GenreProfile::from_genre("Live/Other", "DJ Set (Mixed)");
        assert_eq!(dj_set.tempo_range_bpm(), None);
    }

    #[test]
    fn every_listed_subcategory_round_trips_from_genre() {
        // Every dropdown entry must resolve to a profile whose `subcategory`
        // matches the entry EXACTLY — the UI resync does
        // `subcategories.iter().position(|s| *s == profile.subcategory)` and a
        // mismatch silently falls back to index 0 ("House" thresholds over a
        // chill set). Regression: ambient() said "Ambient/Downtempo" while the
        // dropdown lists "Ambient".
        for category in GenreProfile::categories() {
            for sub in GenreProfile::subcategories_for(category) {
                let p = GenreProfile::from_genre(category, sub);
                assert_eq!(
                    p.subcategory, sub,
                    "from_genre({category:?}, {sub:?}) round-trips its dropdown entry"
                );
            }
        }
    }

    #[test]
    fn post_rock_preset_is_calm_with_rock_tempos() {
        let p = GenreProfile::from_genre("Rock", "Post-Rock/Shoegaze");
        assert_eq!(p.subcategory, "Post-Rock/Shoegaze");
        assert_eq!(p.tempo_range_bpm(), Some((100.0, 140.0)), "honest rock-tempo prior");
        let fp = p.fixture_profile();
        assert!(fp.strobe_tolerance < 0.30, "post-rock closes the strobe lane");
        assert!(fp.laser_tolerance < 0.25, "post-rock closes the laser lane");
        assert!(!p.enable_buildup_blinder, "no auto blinder on crescendos");
    }

    #[test]
    fn fixture_profile_gates_match_genre_intent() {
        // Chill genres must close the intense DMX lanes; hard genres open them.
        // Gate thresholds in fixture_brain: strobe_tolerance < 0.30 and
        // laser_tolerance < 0.25 hard-disable those lanes; density < 0.35
        // stills the blinder lane.
        let ambient = GenreProfile::from_genre("Electronic", "Ambient").fixture_profile();
        assert!(ambient.strobe_tolerance < 0.30, "ambient closes the strobe lane");
        assert!(ambient.laser_tolerance < 0.25, "ambient closes the laser lane");
        assert!(ambient.density < 0.35, "ambient stills the blinder lane");

        let hardstyle = GenreProfile::from_genre("Electronic", "Hardstyle").fixture_profile();
        assert!(hardstyle.strobe_tolerance >= 0.30, "hardstyle opens the strobe lane");
        assert!(hardstyle.laser_tolerance >= 0.25, "hardstyle opens the laser lane");
        assert!(hardstyle.movement > 0.5, "hardstyle moves the movers");

        // The default fixture profile must stay a sane open-ish fallback.
        let default_fp = crate::emerge::FixtureGenreProfile::default();
        assert!(default_fp.strobe_tolerance >= 0.30);
    }
}
